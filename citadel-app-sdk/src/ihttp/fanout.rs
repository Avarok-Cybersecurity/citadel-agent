//! `fanout`: one request to many servers, over `client::call_stream`, under [`Plan`]'s bounds.
//! Partial failure is normal: `done` runs exactly once, with every target's outcome. Time
//! advances on the app's frames (`tick`), so a fan-out's clocks stand still while no frames
//! arrive (a suspended surface); the host bounds each stream on its own clock as well.

pub use super::fanout_plan::{FanoutLimits, Outcome};

use super::client::{self, ClientEvent, Key};
use super::fanout_plan::Plan;
use super::{Outbound, Response};
use crate::abi::RawAbi;
use citadel_app_sdk_types::calls::{Address, ResponseHead, Transport, Via};
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

type Each = Box<dyn FnMut(&mut dyn Any, usize, &Outcome)>;
type Done = Box<dyn FnOnce(&mut dyn Any, Vec<(Address, Outcome, Via)>)>;
type Starter = Rc<dyn Fn(u64, usize) -> Key>;
type Canceller = Rc<dyn Fn(&Key)>;

struct Run {
    plan: Plan,
    targets: Vec<Address>,
    vias: Vec<Via>,
    keys: BTreeMap<usize, Key>,
    partial: BTreeMap<usize, (Option<ResponseHead>, Vec<u8>)>,
    each: Option<Each>,
    done: Option<Done>,
    start: Starter,
    cancel: Canceller,
}

#[derive(Default)]
struct Fanouts {
    last: u64,
    runs: BTreeMap<u64, Run>,
}

thread_local! {
    static FANOUTS: RefCell<Fanouts> = RefCell::new(Fanouts::default());
}

/// A fan-out in flight. Dropping it lets it run; [`FanoutHandle::cancel`] ends it.
#[derive(Debug)]
#[must_use = "keep the handle to cancel the fan-out, or drop it to let it run"]
pub struct FanoutHandle {
    pub(crate) id: u64,
}

/// The plane a transport tries first, until a frame says which one answered.
fn first_via(t: Transport) -> Via {
    match t {
        Transport::Relay => Via::Relay,
        Transport::P2p | Transport::PreferP2pThenRelay => Via::P2p,
    }
}

fn with_run<T>(id: u64, f: impl FnOnce(&mut Run) -> T) -> Option<T> {
    FANOUTS.with(|r| r.borrow_mut().runs.get_mut(&id).map(f))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start<A: 'static, R: RawAbi + 'static>(
    raw: Rc<R>,
    targets: Vec<Address>,
    request: Outbound,
    transport: Transport,
    limits: FanoutLimits,
    mut each: impl FnMut(&mut A, usize, &Outcome) + 'static,
    done: impl FnOnce(&mut A, Vec<(Address, Outcome, Via)>) + 'static,
) -> FanoutHandle {
    let each: Each = Box::new(move |app, i, o| each(own::<A>(app), i, o));
    let done: Done = Box::new(move |app, all| done(own::<A>(app), all));
    let addresses = targets.clone();
    let (call_raw, cancel_raw) = (raw.clone(), raw);
    let start: Starter = Rc::new(move |id, i| {
        let on = Box::new(move |app: &mut dyn Any, event| on_event(id, i, app, event));
        client::open(
            &*call_raw,
            addresses[i].clone(),
            transport,
            request.clone(),
            on,
        )
    });
    let cancel: Canceller = Rc::new(move |k| client::cancel(&*cancel_raw, k));
    let run = Run {
        plan: Plan::new(targets.len(), limits),
        vias: vec![first_via(transport); targets.len()],
        targets,
        keys: BTreeMap::new(),
        partial: BTreeMap::new(),
        each: Some(each),
        done: Some(done),
        start,
        cancel,
    };
    let id = FANOUTS.with(|r| {
        let mut r = r.borrow_mut();
        r.last += 1;
        let id = r.last;
        r.runs.insert(id, run);
        id
    });
    admit(id);
    crate::pending::spawn(Box::new(move |app| finish_if_done(id, app)));
    FanoutHandle { id }
}

fn own<A: 'static>(app: &mut dyn Any) -> &mut A {
    app.downcast_mut::<A>()
        .expect("fanout names the app's own type")
}

fn admit(id: u64) {
    let Some((ready, start)) = with_run(id, |r| (r.plan.admit(), r.start.clone())) else {
        return;
    };
    for i in ready {
        let k = start(id, i);
        with_run(id, |r| r.keys.insert(i, k));
    }
}

fn on_event(id: u64, i: usize, app: &mut dyn Any, event: ClientEvent) {
    let outcome = with_run(id, |r| match event {
        ClientEvent::Head(head, via) => {
            r.vias[i] = via;
            r.partial.entry(i).or_default().0 = Some(head);
            None
        }
        ClientEvent::Body(bytes) => {
            let over = r.plan.bytes(i, bytes.len());
            r.partial.entry(i).or_default().1.extend_from_slice(&bytes);
            over.map(Outcome::Failed)
        }
        ClientEvent::End => {
            let (head, body) = r.partial.remove(&i).unwrap_or_default();
            Some(head.map_or(Outcome::Failed(no_head()), |head| {
                Outcome::Ok(Response {
                    head,
                    via: r.vias[i],
                    body,
                })
            }))
        }
        ClientEvent::Failed(e) => Some(Outcome::Failed(e)),
    });
    if let Some(Some(outcome)) = outcome {
        let stop = !matches!(outcome, Outcome::Ok(_));
        settle(id, i, outcome, stop, app);
    }
}

fn no_head() -> citadel_app_sdk_types::Refused {
    citadel_app_sdk_types::Refused::Unavailable("response without a head".into())
}

/// Record `i`'s outcome, tell the app, stop its stream if it is still open, start the next.
fn settle(id: u64, i: usize, outcome: Outcome, stop: bool, app: &mut dyn Any) {
    let taken = with_run(id, |r| {
        r.plan.settle(i, outcome.clone()).then(|| {
            r.partial.remove(&i);
            (r.keys.remove(&i), r.cancel.clone(), r.each.take())
        })
    });
    let Some(Some((key, cancel, each))) = taken else {
        return;
    };
    if let (true, Some(k)) = (stop, key) {
        cancel(&k);
    }
    if let Some(mut each) = each {
        each(app, i, &outcome);
        with_run(id, |r| r.each = Some(each));
    }
    admit(id);
    finish_if_done(id, app);
}

fn finish_if_done(id: u64, app: &mut dyn Any) {
    let finished = FANOUTS.with(|r| {
        let mut r = r.borrow_mut();
        let done = r.runs.get(&id).is_some_and(|run| run.plan.is_done());
        if done {
            r.runs.remove(&id)
        } else {
            None
        }
    });
    let Some(mut run) = finished else { return };
    let outcomes = run.plan.outcomes();
    let all = run
        .targets
        .into_iter()
        .zip(outcomes)
        .zip(run.vias)
        .map(|((t, o), v)| (t, o, v))
        .collect();
    if let Some(done) = run.done.take() {
        done(app, all);
    }
}

fn stop(id: u64, app: &mut dyn Any, stops: impl FnOnce(&mut Plan) -> Vec<(usize, Outcome)>) {
    for (i, outcome) in with_run(id, |r| stops(&mut r.plan)).unwrap_or_default() {
        settle(id, i, outcome, true, app);
    }
    finish_if_done(id, app);
}

/// `dt_ms` passed for every fan-out (the runtime calls this on each frame).
pub(crate) fn tick(dt_ms: u32, app: &mut dyn Any) {
    let ids: Vec<u64> = FANOUTS.with(|r| r.borrow().runs.keys().copied().collect());
    for id in ids {
        stop(id, app, |plan| plan.elapse(dt_ms));
    }
}

/// End the fan-out: what runs is cancelled and skipped, what waits is skipped.
pub(crate) fn cancel(id: u64) {
    crate::pending::spawn(Box::new(move |app| stop(id, app, Plan::stop_all)));
}

#[cfg(test)]
#[path = "fanout_tests.rs"]
mod tests;
