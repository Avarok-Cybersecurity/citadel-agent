//! The arithmetic that keeps a page and its metadata consistent, and the
//! delivery ladder. Ported from message-page-append.ts and message-status.ts.

use citadel_internal_service_types::{
    ConversationMessage, ConversationMetadata, ConversationPage, MessageStatus, PageTimestamps,
};

pub(super) fn empty_page(peer: u64, number: u32, at: f64) -> ConversationPage {
    ConversationPage {
        peer_cid: peer,
        page_number: number,
        messages: Vec::new(),
        page_timestamps: PageTimestamps {
            min_timestamp: at,
            max_timestamp: at,
        },
    }
}

/// Insert in timestamp order and re-derive the page's bounds.
pub(super) fn place(page: &mut ConversationPage, message: ConversationMessage) {
    page.messages.push(message);
    page.messages
        .sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    page.page_timestamps = PageTimestamps {
        min_timestamp: page.messages[0].timestamp,
        max_timestamp: page.messages[page.messages.len() - 1].timestamp,
    };
}

pub(super) fn record_append(
    metadata: &mut ConversationMetadata,
    message: &ConversationMessage,
    is_new: bool,
    own: u64,
    now: f64,
) {
    metadata.total_message_count += 1.0;
    metadata.newest_message_timestamp = message.timestamp;
    if is_new
        || metadata.total_message_count == 1.0
        || message.timestamp < metadata.oldest_message_timestamp
    {
        metadata.oldest_message_timestamp = message.timestamp;
    }
    metadata.last_message_index = metadata.last_message_index.max(message.index);
    metadata.last_updated = now;
    if message.sender_cid != own && message.status == MessageStatus::Delivered {
        metadata.unread_count += 1.0;
    }
}

/// The delivery ladder: pending < sent < delivered < read; `failed` replaces
/// anything below delivered, and anything positive replaces `failed`.
pub(crate) fn status_advances(current: MessageStatus, next: MessageStatus) -> bool {
    use MessageStatus::*;
    let rank = |s: MessageStatus| match s {
        Sent => 0,
        Delivered => 1,
        Read => 2,
        Pending | Failed => -1,
    };
    match (current, next) {
        (c, n) if c == n => false,
        (c, Failed) => c != Delivered && c != Read,
        (Failed, _) => true,
        (c, n) => rank(n) > rank(c),
    }
}
