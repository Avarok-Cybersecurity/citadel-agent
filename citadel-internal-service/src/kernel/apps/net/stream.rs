//! A streamed `AppFetch` (`host_fetch_stream`): the head, then chunks only against credit.
//!
//! The page grants credit one chunk at a time (`AppFetchCredit`), starting from the same
//! `INITIAL_CREDIT` internal HTTP uses, so a slow page holds the upstream read back instead of
//! the agent buffering the body for it. A stream lives within the policy's time budget,
//! waiting for credit included, and within its response cap.

use super::body::{network, shown_headers};
use super::fetch::Opened;
use super::ports::UpstreamBody;
use super::rate::InstallKey;
use super::AppNet;
use citadel_app_net_policy::MAX_RESPONSE_BODY_BYTES;
use citadel_ihttp_types::{INITIAL_CREDIT, MAX_BODY_CHUNK_BYTES, MAX_CREDIT_GRANT};
use citadel_internal_service_types::{
    AppFetchFailure, AppFetchLimit, AppFetchRefusal, AppFetchStreamChunk, AppFetchStreamEnd,
    AppFetchStreamHead, AppFetchUnavailable, InternalServiceResponse,
};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

/// What the page says to a stream it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Credit(u32),
    Cancel,
}

/// Where a stream's events go: the connection that opened it, in order.
pub(crate) trait StreamSink: Send + Sync {
    /// `false` when the connection is gone, which ends the stream.
    fn emit(&self, response: InternalServiceResponse) -> bool;
}

/// Open streams, by the connection that opened them and the `AppFetch`'s request id. Only
/// that connection's `AppFetchCredit` and `AppFetchCancel` reach a stream.
#[derive(Default)]
pub(crate) struct Streams {
    open: Mutex<HashMap<(Uuid, Uuid), UnboundedSender<Control>>>,
}

impl Streams {
    /// `None` when this connection already has a stream under `fetch_id`.
    pub(crate) fn register(
        &self,
        connection: Uuid,
        fetch_id: Uuid,
    ) -> Option<UnboundedReceiver<Control>> {
        let mut open = self.open.lock();
        if open.contains_key(&(connection, fetch_id)) {
            return None;
        }
        let (tx, rx) = unbounded_channel();
        open.insert((connection, fetch_id), tx);
        Some(rx)
    }

    /// `false` when no such stream is open for this connection.
    pub(crate) fn control(&self, connection: Uuid, fetch_id: Uuid, control: Control) -> bool {
        self.open
            .lock()
            .get(&(connection, fetch_id))
            .is_some_and(|tx| tx.send(control).is_ok())
    }

    pub(crate) fn close(&self, connection: Uuid, fetch_id: Uuid) {
        self.open.lock().remove(&(connection, fetch_id));
    }
}

/// Which stream this is, for the events it emits and the meter it counts into.
pub(crate) struct StreamIds {
    pub cid: u64,
    pub fetch_id: Uuid,
    pub key: InstallKey,
}

impl AppNet {
    /// Emit the head, then pump the body against credit, then `End` or the failure.
    pub(crate) async fn stream(
        &self,
        opened: Opened,
        ids: &StreamIds,
        controls: &mut UnboundedReceiver<Control>,
        sink: &dyn StreamSink,
        deadline: Instant,
    ) {
        let Opened {
            upstream,
            redirects,
        } = opened;
        let head = InternalServiceResponse::AppFetchStreamHead(AppFetchStreamHead {
            cid: ids.cid,
            request_id: Some(ids.fetch_id),
            status: upstream.head.status,
            headers: shown_headers(&upstream.head.headers),
            redirects,
        });
        if !sink.emit(head) {
            return;
        }
        let last = match self
            .pump(upstream.body, ids, controls, sink, deadline)
            .await
        {
            Ok(body_bytes) => InternalServiceResponse::AppFetchStreamEnd(AppFetchStreamEnd {
                cid: ids.cid,
                request_id: Some(ids.fetch_id),
                body_bytes,
            }),
            Err(refusal) => InternalServiceResponse::AppFetchFailure(AppFetchFailure {
                cid: ids.cid,
                request_id: Some(ids.fetch_id),
                refusal,
            }),
        };
        let _gone = !sink.emit(last);
    }

    async fn pump(
        &self,
        mut body: Box<dyn UpstreamBody>,
        ids: &StreamIds,
        controls: &mut UnboundedReceiver<Control>,
        sink: &dyn StreamSink,
        deadline: Instant,
    ) -> Result<u64, AppFetchRefusal> {
        let cancelled = AppFetchRefusal::Unavailable(AppFetchUnavailable::Cancelled);
        let mut credit: u32 = INITIAL_CREDIT;
        let mut pieces: VecDeque<Vec<u8>> = VecDeque::new();
        let mut total: u64 = 0;
        let mut seq: u32 = 0;
        loop {
            while let Ok(control) = controls.try_recv() {
                credit = apply(credit, control).map_err(|r| self.refuse(&ids.key, r))?;
            }
            if credit == 0 {
                let control = timeout_at(deadline, controls.recv())
                    .await
                    .map_err(|_| self.limit(&ids.key, AppFetchLimit::Time))?
                    .ok_or(cancelled)?;
                credit = apply(credit, control).map_err(|r| self.refuse(&ids.key, r))?;
                continue;
            }
            if pieces.is_empty() {
                let chunk = timeout_at(deadline, body.next_chunk())
                    .await
                    .map_err(|_| self.limit(&ids.key, AppFetchLimit::Time))?
                    .map_err(|why| network(&ids.key, &why))?;
                let Some(chunk) = chunk else {
                    return Ok(total);
                };
                self.meter.received(&ids.key, chunk.len());
                total += chunk.len() as u64;
                if total > MAX_RESPONSE_BODY_BYTES {
                    return Err(self.limit(&ids.key, AppFetchLimit::ResponseBody));
                }
                pieces.extend(chunk.chunks(MAX_BODY_CHUNK_BYTES).map(<[u8]>::to_vec));
                continue;
            }
            let Some(bytes) = pieces.pop_front() else {
                continue;
            };
            let event = InternalServiceResponse::AppFetchStreamChunk(AppFetchStreamChunk {
                cid: ids.cid,
                request_id: Some(ids.fetch_id),
                seq,
                bytes,
            });
            if !sink.emit(event) {
                return Err(cancelled);
            }
            seq = seq.saturating_add(1);
            credit -= 1;
        }
    }

    fn refuse(&self, key: &InstallKey, refusal: AppFetchRefusal) -> AppFetchRefusal {
        if matches!(refusal, AppFetchRefusal::Limit(_)) {
            self.meter.refused(key);
        }
        refusal
    }
}

/// Credit after `control`: a grant of 1 to `MAX_CREDIT_GRANT` adds to it, anything else ends
/// the stream.
fn apply(credit: u32, control: Control) -> Result<u32, AppFetchRefusal> {
    match control {
        Control::Credit(n) if (1..=MAX_CREDIT_GRANT).contains(&n) => Ok(credit.saturating_add(n)),
        Control::Credit(_) => Err(AppFetchRefusal::Limit(AppFetchLimit::Credit)),
        Control::Cancel => Err(AppFetchRefusal::Unavailable(AppFetchUnavailable::Cancelled)),
    }
}
