//! A bounded, latest-frame queue for media on its way to a localhost client.
//!
//! Media and control share one WebSocket but must not share one queue. The
//! client channel is unbounded, and the writer task awaits the socket for every
//! response, so a browser that cannot keep up does not slow the pump — it just
//! makes the queue grow. What accumulates is video that was stale before it was
//! sent: memory spent to deliver frames whose moment has passed, and latency
//! the user experiences as a call running behind reality.
//!
//! Dropping is the correct response to a full media queue, and dropping the
//! OLDEST is the correct choice of which. Refusing the newest frame instead
//! keeps a queue permanently full of the stalest frames it holds, which is the
//! worst of both: maximum latency and no fresh media. Evicting from the front
//! bounds latency to the queue depth no matter how long congestion lasts.
//!
//! Control traffic keeps the reliable unbounded path. Losing a video frame
//! costs a sixtieth of a second; losing a "session closed" leaves both sides
//! staring at a call that is over.

use citadel_internal_service_types::InternalServiceResponse;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// How many frames may wait for a client that has stopped keeping up.
///
/// At 30-60 fps this is roughly half a second of video. Deep enough to ride out
/// an ordinary scheduling hiccup without discarding anything, shallow enough
/// that a client which is genuinely behind never gets more than that far behind
/// — past which the picture is no longer worth the delay it costs.
pub const MEDIA_LANE_CAPACITY: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum PushOutcome {
    Queued,
    /// The queue was full; the oldest frame was evicted to make room.
    DroppedOldest,
    /// The client is gone. Callers stop producing rather than filling a queue
    /// nobody will ever read.
    Closed,
}

struct LaneState {
    queue: VecDeque<InternalServiceResponse>,
    closed: bool,
    dropped: u64,
}

struct Inner {
    state: Mutex<LaneState>,
    capacity: usize,
}

/// The producer half, held by the inbound pump. Cheap to clone.
#[derive(Clone)]
pub struct MediaLaneTx {
    inner: Arc<Inner>,
    /// Capacity-1 wakeup. Only its arrival matters, never its contents, so a
    /// full signal channel means "already awake" and is not an error.
    signal: mpsc::Sender<()>,
}

/// The consumer half, owned by the one task writing to the client socket.
pub struct MediaLaneRx {
    inner: Arc<Inner>,
    signal: mpsc::Receiver<()>,
}

/// Split rather than shared because the receive half must survive being used
/// inside `tokio::select!`.
///
/// The obvious implementation parks the reader on a `Notify`. That is NOT
/// cancel-safe: `select!` drops the losing branch's future, and a notification
/// already handed to that future dies with it — so a frame sits in the queue
/// with nobody scheduled to take it. Waking through an mpsc instead borrows a
/// primitive whose `recv` is documented to lose nothing when cancelled.
pub fn media_lane(capacity: usize) -> (MediaLaneTx, MediaLaneRx) {
    let inner = Arc::new(Inner {
        state: Mutex::new(LaneState {
            queue: VecDeque::with_capacity(capacity),
            closed: false,
            dropped: 0,
        }),
        capacity,
    });
    let (signal_tx, signal_rx) = mpsc::channel(1);
    (
        MediaLaneTx {
            inner: inner.clone(),
            signal: signal_tx,
        },
        MediaLaneRx {
            inner,
            signal: signal_rx,
        },
    )
}

impl MediaLaneTx {
    /// Never blocks and never awaits: this is called from the inbound pump,
    /// which must not be held up by a slow consumer — that is the whole point.
    pub fn push(&self, item: InternalServiceResponse) -> PushOutcome {
        let outcome = {
            let mut state = self.inner.state.lock().expect("media lane poisoned");
            if state.closed {
                return PushOutcome::Closed;
            }
            let evicted = if state.queue.len() >= self.inner.capacity {
                state.queue.pop_front();
                state.dropped += 1;
                true
            } else {
                false
            };
            state.queue.push_back(item);
            if evicted {
                PushOutcome::DroppedOldest
            } else {
                PushOutcome::Queued
            }
        };
        // Full means a wakeup is already pending, which is all one is for.
        let _ = self.signal.try_send(());
        outcome
    }

    pub fn close(&self) {
        self.inner.state.lock().expect("media lane poisoned").closed = true;
        let _ = self.signal.try_send(());
    }

    /// Frames discarded because the client could not keep up. This is the
    /// congestion signal: it is nonzero exactly when the lane is doing its job.
    pub fn dropped(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("media lane poisoned")
            .dropped
    }

    pub fn len(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("media lane poisoned")
            .queue
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl MediaLaneRx {
    /// `None` once the lane is closed AND drained, so a writer shutting down
    /// still delivers what it already holds.
    ///
    /// Cancel-safe: nothing is removed from the queue until it is returned, and
    /// a lost wakeup cannot strand a frame because the queue is re-inspected at
    /// the top of every call.
    pub async fn recv(&mut self) -> Option<InternalServiceResponse> {
        loop {
            {
                let mut state = self.inner.state.lock().expect("media lane poisoned");
                if let Some(item) = state.queue.pop_front() {
                    return Some(item);
                }
                if state.closed {
                    return None;
                }
            }
            // All producers gone and nothing queued: the lane can yield nothing
            // further, so report the end rather than parking forever.
            self.signal.recv().await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::MediaFrameNotification;

    fn frame(timestamp: u32) -> InternalServiceResponse {
        InternalServiceResponse::MediaFrameNotification(MediaFrameNotification {
            cid: 1,
            peer_cid: 2,
            track: 0,
            kind: 0,
            timestamp,
            flags: 0,
            payload: vec![0u8; 4],
            sequence: timestamp,
            request_id: None,
        })
    }

    fn timestamp_of(response: &InternalServiceResponse) -> u32 {
        match response {
            InternalServiceResponse::MediaFrameNotification(f) => f.timestamp,
            other => panic!("expected a media frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delivers_in_order_while_the_client_keeps_up() {
        let (lane, mut rx) = media_lane(4);
        for t in 0..3 {
            assert_eq!(lane.push(frame(t)), PushOutcome::Queued);
        }
        for t in 0..3 {
            assert_eq!(timestamp_of(&rx.recv().await.unwrap()), t);
        }
        assert_eq!(lane.dropped(), 0);
    }

    #[tokio::test]
    async fn evicts_the_oldest_frame_rather_than_refusing_the_newest() {
        // The distinction that matters. Refusing the newest would leave the
        // queue permanently full of the stalest frames it holds -- maximum
        // latency AND no fresh media.
        let (lane, mut rx) = media_lane(3);
        for t in 0..5 {
            lane.push(frame(t));
        }

        assert_eq!(lane.len(), 3);
        assert_eq!(lane.dropped(), 2);
        // 0 and 1 evicted; what survives is the most recent, in order.
        for t in 2..5 {
            assert_eq!(timestamp_of(&rx.recv().await.unwrap()), t);
        }
    }

    #[tokio::test]
    async fn a_full_lane_never_blocks_the_pump() {
        // push() is the inbound pump's call. If congestion could stall it, a
        // slow browser would stall decoding for everyone on the call.
        let (lane, _rx) = media_lane(2);
        for t in 0..1_000 {
            assert_ne!(lane.push(frame(t)), PushOutcome::Closed);
        }
        assert_eq!(lane.len(), 2);
        assert_eq!(lane.dropped(), 998);
    }

    #[tokio::test]
    async fn a_waiting_receiver_wakes_on_a_later_push() {
        // The race the recv() loop is written around: registering interest
        // AFTER inspecting the queue would lose a push landing in between, and
        // the lane would stall until the next frame happened to arrive.
        let (lane, mut rx) = media_lane(4);
        let reader = tokio::spawn(async move { rx.recv().await });

        tokio::task::yield_now().await;
        lane.push(frame(7));

        let got = reader.await.unwrap().expect("a queued frame");
        assert_eq!(timestamp_of(&got), 7);
    }

    #[tokio::test]
    async fn a_closed_lane_still_drains_what_it_holds() {
        // Shutdown must not discard frames already accepted; only then does
        // recv() report the end.
        let (lane, mut rx) = media_lane(4);
        lane.push(frame(1));
        lane.close();

        assert_eq!(timestamp_of(&rx.recv().await.unwrap()), 1);
        assert!(rx.recv().await.is_none());
        assert_eq!(lane.push(frame(2)), PushOutcome::Closed);
    }

    #[tokio::test]
    async fn a_receiver_parked_on_an_empty_lane_observes_the_close() {
        let (lane, mut rx) = media_lane(4);
        let reader = tokio::spawn(async move { rx.recv().await });

        tokio::task::yield_now().await;
        lane.close();

        assert!(reader.await.unwrap().is_none());
    }
}
