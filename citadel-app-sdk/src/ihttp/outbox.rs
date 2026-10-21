//! Frames waiting for credit. Body frames spend one credit each; heads, `End` and `Cancel` do not.

use citadel_app_sdk_types::abi::{IHTTP_INITIAL_CREDIT, IHTTP_MAX_CHUNK_BYTES};
use citadel_app_sdk_types::calls::FrameKind;
use citadel_app_sdk_types::Bytes;
use std::collections::VecDeque;

pub(crate) struct Outbox {
    credit: u32,
    queued: VecDeque<FrameKind>,
}

impl Outbox {
    pub fn new() -> Outbox {
        Outbox {
            credit: IHTTP_INITIAL_CREDIT,
            queued: VecDeque::new(),
        }
    }

    pub fn grant(&mut self, n: u32) {
        self.credit = self.credit.saturating_add(n);
    }

    /// A head, `body` in chunks, then `End`.
    pub fn message(&mut self, head: FrameKind, body: &[u8]) {
        self.queued.push_back(head);
        for chunk in body.chunks(IHTTP_MAX_CHUNK_BYTES as usize) {
            self.queued.push_back(FrameKind::Body {
                bytes: Bytes(chunk.to_vec()),
            });
        }
        self.queued.push_back(FrameKind::End);
    }

    /// The next frame the credit allows.
    pub fn next(&mut self) -> Option<FrameKind> {
        if matches!(self.queued.front(), Some(FrameKind::Body { .. })) {
            if self.credit == 0 {
                return None;
            }
            self.credit -= 1;
        }
        self.queued.pop_front()
    }
}
