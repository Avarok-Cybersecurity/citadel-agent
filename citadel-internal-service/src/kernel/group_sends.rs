//! Which `GroupMessage` request each of the server's answers belongs to.
//!
//! The server answers every group message with exactly one `MessageResponse` (its delivery
//! report) or `MessageRefused`, in the order the sender sent them, and names neither the message
//! nor the request. A channel therefore queues each send's request id as it hands the message
//! to the session, under one lock so the queue is in the order the messages were sent, and the
//! channel's receiver pairs each answer with the oldest unanswered send.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct PendingSends {
    queue: tokio::sync::Mutex<VecDeque<Uuid>>,
}

impl PendingSends {
    /// Runs `send` with `request_id` queued, in send order; withdrawn if `send` fails, since
    /// nothing will answer it.
    pub(crate) async fn send<T, E>(
        &self,
        request_id: Uuid,
        send: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let mut queue = self.queue.lock().await;
        queue.push_back(request_id);
        let sent = send.await;
        if sent.is_err() {
            let _withdrawn = queue.pop_back();
        }
        sent
    }

    /// The request the next answer belongs to.
    pub(crate) async fn answered(&self) -> Option<Uuid> {
        self.queue.lock().await.pop_front()
    }
}

/// What a channel's map entry shares with its receiver task, which does not hold the map.
#[derive(Clone)]
pub(crate) struct ChannelState {
    pub(crate) departed: Arc<AtomicBool>,
    pub(crate) sends: Arc<PendingSends>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_pair_with_sends_in_order_and_a_failed_send_is_withdrawn() {
        let sends = PendingSends::default();
        let (first, failed, second) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        assert!(sends.send(first, async { Ok::<_, ()>(()) }).await.is_ok());
        assert!(sends
            .send(failed, async { Err::<(), _>(()) })
            .await
            .is_err());
        assert!(sends.send(second, async { Ok::<_, ()>(()) }).await.is_ok());
        assert_eq!(sends.answered().await, Some(first));
        assert_eq!(sends.answered().await, Some(second));
        assert_eq!(
            sends.answered().await,
            None,
            "an extra answer names no request"
        );
    }
}
