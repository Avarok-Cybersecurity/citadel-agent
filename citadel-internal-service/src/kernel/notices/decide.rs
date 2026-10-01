//! What to tell the user, from what happened and how they set the account.
//!
//! Pure: no locks, no store, no clock. The agent gathers the context
//! (raise.rs); everything that decides lives here and is tested on its own.
//!
//! - A muted account raises nothing but calls. A missed call is the one thing
//!   a user who muted the chatter still wants to hear.
//! - A window that has the conversation (for a message or a file) or the
//!   account (for anything else) in front of the user shows it already.
//! - The preview setting decides whether the text appears. `SenderOnly`, the
//!   default, says who wrote and not what. A call has no text to hide.
//! - The click target names the account, its server and the target id; never
//!   any content, which a URL would carry into history and logs.

use citadel_internal_service_types::{NativeNotice, NoticeKind, NoticeTarget, NotificationPreview};

/// The longest message text a notice shows.
pub(crate) const PREVIEW_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeSource {
    Message {
        peer: u64,
        peer_username: Option<String>,
        text: String,
    },
    PeerRequest {
        peer: u64,
        peer_username: Option<String>,
    },
    GroupInvite {
        peer: u64,
        peer_username: Option<String>,
    },
    FileOffer {
        peer: u64,
        peer_username: Option<String>,
        file_name: String,
    },
    IncomingCall {
        peer: u64,
        peer_username: Option<String>,
    },
}

/// The account a notice is for, as the agent knows it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoticeContext {
    pub cid: u64,
    pub account: String,
    pub server_host: Option<String>,
    pub preview: NotificationPreview,
    pub muted: bool,
    /// What each focused window of the account shows: a conversation, or the
    /// account with none open.
    pub focused: Vec<Option<u64>>,
}

pub(crate) fn decide(source: &NoticeSource, ctx: &NoticeContext) -> Option<NativeNotice> {
    let (kind, peer) = kind_and_peer(source);
    if ctx.muted && kind != NoticeKind::IncomingCall {
        return None;
    }
    let shown = match source {
        NoticeSource::Message { .. } | NoticeSource::FileOffer { .. } => {
            ctx.focused.contains(&Some(peer))
        }
        _ => !ctx.focused.is_empty(),
    };
    if shown {
        return None;
    }
    let previews = ctx.preview == NotificationPreview::Text;
    let (title, body, open) = match source {
        NoticeSource::Message {
            peer_username,
            text,
            ..
        } => (
            who(peer_username),
            if previews {
                text.chars().take(PREVIEW_CHARS).collect()
            } else {
                "New message".to_string()
            },
            format!("conversation:{peer}"),
        ),
        NoticeSource::FileOffer {
            peer_username,
            file_name,
            ..
        } => (
            who(peer_username),
            if previews {
                format!("Sent you {file_name}")
            } else {
                "Sent you a file".to_string()
            },
            format!("conversation:{peer}"),
        ),
        NoticeSource::PeerRequest { peer_username, .. } => (
            format!("{} wants to connect", who(peer_username)),
            "Open Citadel to accept or decline.".to_string(),
            "requests".to_string(),
        ),
        NoticeSource::GroupInvite { peer_username, .. } => (
            "Group invitation".to_string(),
            format!("{} invited you to a group.", who(peer_username)),
            "requests".to_string(),
        ),
        NoticeSource::IncomingCall { peer_username, .. } => (
            format!("{} is calling", who(peer_username)),
            "Incoming call".to_string(),
            format!("call:{peer}"),
        ),
    };
    Some(NativeNotice {
        cid: ctx.cid,
        kind,
        title,
        body,
        target: NoticeTarget {
            account: ctx.account.clone(),
            server_host: ctx.server_host.clone(),
            open,
        },
        request_id: None,
    })
}

fn kind_and_peer(source: &NoticeSource) -> (NoticeKind, u64) {
    match source {
        NoticeSource::Message { peer, .. } => (NoticeKind::Message, *peer),
        NoticeSource::PeerRequest { peer, .. } => (NoticeKind::PeerRequest, *peer),
        NoticeSource::GroupInvite { peer, .. } => (NoticeKind::GroupInvite, *peer),
        NoticeSource::FileOffer { peer, .. } => (NoticeKind::FileOffer, *peer),
        NoticeSource::IncomingCall { peer, .. } => (NoticeKind::IncomingCall, *peer),
    }
}

fn who(peer_username: &Option<String>) -> String {
    peer_username
        .clone()
        .unwrap_or_else(|| "Someone".to_string())
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
