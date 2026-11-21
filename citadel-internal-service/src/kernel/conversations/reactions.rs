//! Reactions: how one change folds into a message's list.
//!
//! Ported from the web UI's lib/reactions (reaction-state.ts).

use citadel_internal_service_types::Reaction;

/// The six reactions the UI offers; anything else from a peer is ignored.
pub(crate) const REACTION_EMOJIS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🎉"];

/// `current` with `change` applied, or `None` when it changes nothing: an
/// unknown emoji, a non-finite time, or a change no newer than the one held
/// for the same reactor and emoji.
pub(crate) fn fold(current: Option<&[Reaction]>, change: &Reaction) -> Option<Vec<Reaction>> {
    if !REACTION_EMOJIS.contains(&change.emoji.as_str()) || !change.at.is_finite() {
        return None;
    }
    let list = current.unwrap_or_default();
    let same = |r: &Reaction| r.reactor_cid == change.reactor_cid && r.emoji == change.emoji;
    match list.iter().find(|r| same(r)) {
        Some(held) if held.at >= change.at => None,
        Some(_) => Some(
            list.iter()
                .map(|r| if same(r) { change.clone() } else { r.clone() })
                .collect(),
        ),
        None => Some(list.iter().cloned().chain([change.clone()]).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reaction(emoji: &str, reactor: u64, at: f64, active: bool) -> Reaction {
        Reaction {
            emoji: emoji.into(),
            reactor_cid: reactor,
            at,
            active,
        }
    }

    #[test]
    fn a_newer_change_replaces_and_an_older_one_is_ignored() {
        let held = vec![reaction("👍", 1, 10.0, true)];
        let retract = reaction("👍", 1, 11.0, false);
        assert_eq!(fold(Some(&held), &retract), Some(vec![retract.clone()]));
        assert_eq!(fold(Some(&held), &reaction("👍", 1, 10.0, false)), None);
        let other = reaction("🎉", 1, 1.0, true);
        assert_eq!(
            fold(Some(&held), &other),
            Some(vec![held[0].clone(), other])
        );
        assert_eq!(fold(None, &reaction("💩", 1, 1.0, true)), None);
        assert_eq!(fold(None, &reaction("👍", 1, f64::NAN, true)), None);
    }
}
