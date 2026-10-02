//! Reactions: the stored CBOR list, and how one change folds into it.
//!
//! Ported from the web UI's lib/reactions (stored-reactions.ts, reaction-state.ts)
//! and pinned by the same cbor-x fixture (tests/fixtures/p2p_commands/reactions.cbor).

use super::cbor::Value;
use citadel_internal_service_types::Reaction;

/// The six reactions the UI offers; anything else from a peer is ignored.
pub(crate) const REACTION_EMOJIS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🎉"];

/// The list as stored inside a page: cbor-x bytes, or nothing for an empty list.
pub(crate) fn encode(list: &[Reaction]) -> Option<Vec<u8>> {
    if list.is_empty() {
        return None;
    }
    let items = list
        .iter()
        .map(|r| {
            Value::object(vec![
                ("emoji", Value::text(r.emoji.clone())),
                ("reactorCid", Value::bigint(r.reactor_cid)),
                ("at", Value::number(r.at)),
                ("active", Value::Bool(r.active)),
            ])
        })
        .collect();
    Some(Value::Array(items).encode())
}

/// The stored bytes back to a list. An entry that is not a whole reaction is
/// dropped; bytes that are not a list at all are no reactions -- as the UI reads them.
pub(crate) fn decode(bytes: &[u8]) -> Option<Vec<Reaction>> {
    let Ok(Value::Array(items)) = Value::decode(bytes) else {
        return None;
    };
    let list: Vec<Reaction> = items
        .iter()
        .filter_map(|item| {
            Some(Reaction {
                emoji: item.get("emoji")?.as_str()?.to_string(),
                reactor_cid: item.get("reactorCid")?.as_bigint()?,
                at: item.get("at")?.as_number()?,
                active: item.get("active")?.as_bool()?,
            })
        })
        .collect();
    (!list.is_empty()).then_some(list)
}

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
    fn a_list_encodes_to_the_bytes_the_ui_stores() {
        let list = [
            reaction("👍", 1001, 1790000003000.0, true),
            reaction("❤️", 2002, 1790000003500.0, false),
            reaction("🎉", 7, 3.0, true),
        ];
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/p2p_commands/reactions.cbor"
        ))
        .expect("fixture");
        assert_eq!(encode(&list).expect("non-empty"), fixture);
        assert_eq!(decode(&fixture).expect("decodes"), list);
    }

    #[test]
    fn nothing_and_garbage_are_no_reactions() {
        assert_eq!(encode(&[]), None);
        assert_eq!(decode(&[0x01]), None);
        assert_eq!(decode(&[0x80]), None);
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
