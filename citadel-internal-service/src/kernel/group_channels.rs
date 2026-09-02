//! Group-channel bookkeeping for a session's `Connection`.
//!
//! `Connection.groups` used to map group keys straight to the SDK's
//! `GroupChannelSendHalf` and was insert-only: nothing anywhere in the crate
//! ever removed an entry. That send half is just a clone of the session-level
//! request queue, so it keeps accepting sends after the user has left the
//! group — which meant a `GroupMessage` for a group the user had already left
//! found the stale entry, enqueued the broadcast, and reported
//! `GroupMessageSuccess` for a message no group member would ever receive.
//! Dead entries also accumulated for the life of the session, which
//! deliberately survives TCP drops and is therefore unbounded.
//!
//! The teardown signals that would justify removal (`LeaveRoomResponse`,
//! `EndResponse`, `Disconnected`) are processed in `responses/group_event.rs`
//! and the spawned group-channel receiver, outside this module. The one place
//! departure IS observable from here without new wiring is the send half
//! itself: `leave()` and `end()` are only ever issued through the clone this
//! map hands out. So every stored channel shares a departure flag with all
//! clones of its sender: a successfully-issued `leave()`/`end()` sets it,
//! membership lookups treat a flagged entry as absent (so the check can no
//! longer be satisfied by a stale entry), and mutable access drops flagged
//! entries from the map.
//!
//! Paths that now expire an entry: the session's own `GroupLeave` and
//! `GroupEnd` requests (flag set the moment the leave/end is enqueued), with
//! the physical removal happening on the next mutable touch of the map; and
//! session teardown (disconnect/deregister), which drops the whole map.
//! Paths that still do NOT: departures this session only learns about from
//! the server — being kicked, or a group ended by another member (the
//! `Disconnected`/`EndResponse` events) — those are handled in
//! `responses/group_event.rs`, which would need to call into this map to
//! expire the entry; entries for such groups remain stale until session end.

use citadel_sdk::prelude::{GroupChannelSendHalf, MessageGroupKey, NetworkError, SecBuffer};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::GroupConnection;

/// A group channel the session believes it is still a member of. Produced by
/// [`GroupChannels::insert`] from the `GroupConnection` the SDK handed us.
pub struct ActiveGroupChannel {
    #[allow(dead_code)]
    pub(crate) key: MessageGroupKey,
    #[allow(dead_code)]
    pub(crate) cid: u64,
    pub(crate) tx: GroupTx,
}

/// Cloneable group sender. Clones share one departure flag with the map entry
/// they were cloned from, so a `leave()`/`end()` issued on a clone (the
/// request handlers clone before awaiting) is visible to every later
/// membership lookup.
#[derive(Clone)]
pub struct GroupTx {
    inner: GroupChannelSendHalf,
    departed: Arc<AtomicBool>,
}

impl GroupTx {
    pub(crate) fn departed(&self) -> bool {
        self.departed.load(Ordering::Acquire)
    }

    fn reject_if_departed(&self, op: &str) -> Result<(), NetworkError> {
        if self.departed() {
            Err(NetworkError::msg(format!(
                "Cannot {op}: this session has left the group"
            )))
        } else {
            Ok(())
        }
    }

    pub(crate) async fn send_message(&self, message: SecBuffer) -> Result<(), NetworkError> {
        self.reject_if_departed("message group")?;
        self.inner.send_message(message).await
    }

    pub(crate) async fn invite(&self, peer_cid: u64) -> Result<(), NetworkError> {
        self.reject_if_departed("invite to group")?;
        self.inner.invite(peer_cid).await
    }

    pub(crate) async fn kick(&self, peer_cid: u64) -> Result<(), NetworkError> {
        self.reject_if_departed("kick from group")?;
        self.inner.kick(peer_cid).await
    }

    /// Leaves the group. The flag is set only after the SDK accepted the
    /// leave for processing, so a failed enqueue (dead session channel) does
    /// not strand a live membership behind a departed flag.
    pub(crate) async fn leave(&self) -> Result<(), NetworkError> {
        self.inner.leave().await?;
        self.departed.store(true, Ordering::Release);
        Ok(())
    }

    /// Ends the group (owner only — the SDK's permission gate rejects
    /// non-owners before anything is sent, in which case the flag stays
    /// unset and membership is unaffected).
    pub(crate) async fn end(&self) -> Result<(), NetworkError> {
        self.inner.end().await?;
        self.departed.store(true, Ordering::Release);
        Ok(())
    }
}

/// The `Connection.groups` map. Lookup methods hide departed entries so no
/// membership check can pass on a group this session has left, and mutable
/// paths physically remove them so they do not pile up for the session's
/// (unbounded) lifetime.
#[derive(Default)]
pub struct GroupChannels {
    inner: HashMap<MessageGroupKey, ActiveGroupChannel>,
}

impl GroupChannels {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, group_key: MessageGroupKey, channel: GroupConnection) {
        // Every insert also sweeps entries whose group has since been
        // left/ended, so a session that keeps joining groups cannot
        // accumulate departed carcasses between get_mut touches.
        self.inner.retain(|_, entry| !entry.tx.departed());
        let GroupConnection { key, tx, cid } = channel;
        self.inner.insert(
            group_key,
            ActiveGroupChannel {
                key,
                cid,
                tx: GroupTx {
                    inner: tx,
                    departed: Arc::new(AtomicBool::new(false)),
                },
            },
        );
    }

    pub(crate) fn get(&self, key: &MessageGroupKey) -> Option<&ActiveGroupChannel> {
        self.inner.get(key).filter(|entry| !entry.tx.departed())
    }

    /// The departure flag for a group, so a long-lived task can set it without
    /// holding the connection map.
    ///
    /// `spawn_group_channel_receiver` is the OTHER path a `Disconnected` /
    /// `EndResponse` broadcast takes to the client, and it has no access to
    /// `Connection`. Handing it the flag is what makes the two paths agree.
    pub(crate) fn departure_flag(&self, key: &MessageGroupKey) -> Option<Arc<AtomicBool>> {
        self.inner.get(key).map(|entry| entry.tx.departed.clone())
    }

    /// Record that this session is no longer in the group, on the strength of
    /// something the SERVER said rather than something we asked for.
    ///
    /// `leave()` and `end()` cover the departures this session initiates. They
    /// are not the only ones: being kicked, or another member ending the group,
    /// arrive as `Disconnected` / `EndResponse` events, and without this the
    /// entry survived them — so a `GroupMessage` to a group we had been removed
    /// from still passed the membership check and was answered with success.
    ///
    /// Returns whether an entry was actually there to mark, so the caller can
    /// tell "we were in it" from "we never were".
    pub(crate) fn mark_departed(&mut self, key: &MessageGroupKey) -> bool {
        match self.inner.get(key) {
            Some(entry) if !entry.tx.departed() => {
                entry.tx.departed.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn get_mut(&mut self, key: &MessageGroupKey) -> Option<&mut ActiveGroupChannel> {
        if self.inner.get(key).is_some_and(|entry| entry.tx.departed()) {
            // A departed entry is dead weight; reclaim it instead of merely
            // hiding it.
            self.inner.remove(key);
            return None;
        }
        self.inner.get_mut(key)
    }
}
