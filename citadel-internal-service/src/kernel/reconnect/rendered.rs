//! Recognising an error a server rendered and forwarded as text. Pure; moved out of
//! policy.rs unchanged (its tests are policy_tests.rs).

/// Whether `message` is the registry form `form` rendered, at its start or right after
/// a `": "` (the server prefixes some errors with its own reason, e.g. "CID not
/// registered to this node: CID 7 is not registered to this node").
///
/// Every literal part of the form must appear, in order, with something in each
/// placeholder, and a form ending in literal text must end the message. A form that is
/// merely mentioned mid-sentence ("timed out after Invalid password") is not a match.
pub(super) fn renders(form: &str, message: &str) -> bool {
    let parts: Vec<&str> = form.split("{}").collect();
    let (first, placeholders) = match parts.split_first() {
        Some((first, rest)) if !first.is_empty() => (*first, rest),
        _ => return false,
    };
    clauses(message).any(|clause| {
        let Some(mut rest) = clause.strip_prefix(first) else {
            return false;
        };
        // Each later part follows a placeholder, which must hold something.
        for (index, part) in placeholders.iter().enumerate() {
            if index == placeholders.len() - 1 {
                return if part.is_empty() {
                    !rest.is_empty()
                } else {
                    rest.len() > part.len() && rest.ends_with(part)
                };
            }
            match rest.get(1..).and_then(|tail| tail.find(part)) {
                Some(at) => rest = &rest[1 + at + part.len()..],
                None => return false,
            }
        }
        true
    })
}

/// Whether `message` leads with `lead`, at its start or right after a `": "`.
pub(super) fn states(message: &str, lead: &str) -> bool {
    clauses(message).any(|clause| clause.starts_with(lead))
}

/// `message` from its start and from after each `": "`, where a server's own reason
/// may prefix the error it forwards.
fn clauses(message: &str) -> impl Iterator<Item = &str> {
    let after_separator = message.match_indices(": ").map(|(at, sep)| at + sep.len());
    std::iter::once(0)
        .chain(after_separator)
        .map(move |start| &message[start..])
}
