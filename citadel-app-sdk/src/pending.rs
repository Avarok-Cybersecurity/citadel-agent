//! Replies that arrive later. A call answers a [`Pending`]; the app either attaches a
//! continuation with [`Pending::then`] (run with `&mut` the app once the reply is in) or keeps the
//! `Pending` and takes it when its `AppEvent::ReplyReady` arrives. The executor is a queue drained
//! after every export call returns, so a continuation never runs while the app is borrowed.

use crate::abi::{decode_reply, read, send, RawAbi, Sent};
use citadel_app_sdk_types::{encode, HostOp, Refused};
use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

type Continuation = Box<dyn FnOnce(&mut dyn Any, Result<Vec<u8>, Refused>)>;
pub(crate) type Task = Box<dyn FnOnce(&mut dyn Any)>;

#[derive(Default)]
struct Executor {
    /// Pending handles the SDK owns: a continuation, or `None` to read and discard.
    waiting: BTreeMap<i32, Option<Continuation>>,
    ready: VecDeque<Task>,
}

thread_local! {
    static EXECUTOR: RefCell<Executor> = RefCell::new(Executor::default());
    /// Failures of the SDK's own background calls, for the runtime to log.
    static NOTES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn note(message: String) {
    NOTES.with(|n| n.borrow_mut().push(message));
}

pub(crate) fn take_notes() -> Vec<String> {
    NOTES.with(|n| std::mem::take(&mut *n.borrow_mut()))
}

pub(crate) fn spawn(task: Task) {
    EXECUTOR.with(|e| e.borrow_mut().ready.push_back(task));
}

fn wait(handle: i32, then: Option<Continuation>) {
    EXECUTOR.with(|e| e.borrow_mut().waiting.insert(handle, then));
}

/// `ReplyReady { handle }` arrived. True when the SDK owned the handle (its continuation is now
/// queued, or the reply was discarded); false hands the event to the app.
pub(crate) fn reply_ready(raw: &impl RawAbi, handle: i32) -> bool {
    let Some(then) = EXECUTOR.with(|e| e.borrow_mut().waiting.remove(&handle)) else {
        return false;
    };
    match (read(raw, handle), then) {
        (Sent::Pending(h), then) => wait(h, then),
        (Sent::Done(r), Some(then)) => spawn(Box::new(move |app| then(app, r))),
        (Sent::Done(_), None) => {}
    }
    true
}

/// Run every queued continuation, including those queued while running.
pub(crate) fn drain(app: &mut dyn Any) {
    while let Some(task) = EXECUTOR.with(|e| e.borrow_mut().ready.pop_front()) {
        task(app);
    }
}

enum State {
    Done(Result<Vec<u8>, Refused>),
    Waiting(i32),
}

type Decode<T> = Box<dyn FnOnce(Vec<u8>) -> Result<T, Refused>>;

/// A reply that may not have arrived. Dropping it discards the reply when it does.
#[must_use = "a dropped Pending discards its reply; attach `then` or keep it"]
pub struct Pending<T> {
    state: Option<State>,
    decode: Option<Decode<T>>,
}

impl<T: 'static> Pending<T> {
    fn new(sent: Sent, decode: Decode<T>) -> Pending<T> {
        let state = match sent {
            Sent::Done(r) => State::Done(r),
            Sent::Pending(h) => State::Waiting(h),
        };
        Pending {
            state: Some(state),
            decode: Some(decode),
        }
    }

    /// The handle while the reply is outstanding.
    pub fn handle(&self) -> Option<i32> {
        match self.state {
            Some(State::Waiting(h)) => Some(h),
            _ => None,
        }
    }

    /// The reply if it came back with the call (a refusal, `ungranted`, a limit, or a port that
    /// answers at once); `None` while it is pending.
    pub fn take_ready(&mut self) -> Option<Result<T, Refused>> {
        match self.state.take()? {
            State::Done(r) => Some(r.and_then(self.decode.take().expect("decoded once"))),
            waiting => {
                self.state = Some(waiting);
                None
            }
        }
    }

    pub(crate) fn poll_with(&mut self, raw: &impl RawAbi) -> Option<Result<T, Refused>> {
        if let Some(State::Waiting(h)) = self.state {
            self.state = Some(match read(raw, h) {
                Sent::Done(r) => State::Done(r),
                Sent::Pending(h) => State::Waiting(h),
            });
        }
        self.take_ready()
    }

    pub fn map<U: 'static>(mut self, f: impl FnOnce(T) -> U + 'static) -> Pending<U> {
        let decode = self.decode.take().expect("decoded once");
        Pending {
            state: self.state.take(),
            decode: Some(Box::new(move |b| decode(b).map(f))),
        }
    }

    /// Run `f` with the app once the reply is in. `A` must be the app's own type.
    pub fn then<A: 'static>(mut self, f: impl FnOnce(&mut A, Result<T, Refused>) + 'static) {
        let decode = self.decode.take().expect("decoded once");
        let then: Continuation = Box::new(move |app, r| {
            let app = app
                .downcast_mut::<A>()
                .expect("Pending::then names the app's own type");
            f(app, r.and_then(decode));
        });
        match self.state.take() {
            Some(State::Done(r)) => spawn(Box::new(move |app| then(app, r))),
            Some(State::Waiting(h)) => wait(h, Some(then)),
            None => {}
        }
    }
}

impl Pending<()> {
    /// The SDK's own call whose only outcome of interest is failure: noted for the log.
    pub(crate) fn watch(mut self, what: &'static str) {
        match self.take_ready() {
            Some(Err(e)) => note(format!("{what}: {e:?}")),
            Some(Ok(())) => {}
            None => {
                let decode = self.decode.take().expect("decoded once");
                let then: Continuation = Box::new(move |_, r| {
                    if let Err(e) = r.and_then(decode) {
                        note(format!("{what}: {e:?}"));
                    }
                });
                if let Some(State::Waiting(h)) = self.state.take() {
                    wait(h, Some(then));
                }
            }
        }
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if let Some(State::Waiting(h)) = self.state.take() {
            wait(h, None);
        }
    }
}

/// Start `op`; its reply may be pending.
#[inline(always)]
pub(crate) fn start<O: HostOp + 'static>(raw: &impl RawAbi, op: &O) -> Pending<O::Reply>
where
    O::Reply: 'static,
{
    Pending::new(
        send(raw, O::GROUP, O::OP, &encode(op)),
        Box::new(|b| decode_reply(&b)),
    )
}

/// Make an op the host answers at once (`canvas`, `log`). A pending reply is refused here and
/// discarded when it arrives.
#[inline(always)]
pub(crate) fn now<O: HostOp>(raw: &impl RawAbi, op: &O) -> Result<O::Reply, Refused> {
    match send(raw, O::GROUP, O::OP, &encode(op)) {
        Sent::Done(r) => r.and_then(|b| decode_reply(&b)),
        Sent::Pending(h) => {
            wait(h, None);
            Err(Refused::Unavailable(
                "an immediate op answered pending".into(),
            ))
        }
    }
}

#[cfg(test)]
#[path = "pending_tests.rs"]
mod tests;
