//! Resume credentials on the agent: what a session holds to come back from a dropped link
//! without its user, when that runs out, and how its row is marked meanwhile.
//!
//! A full sign-in that asked for one (`Connect::resume_lifetime_s`) leaves the session a
//! credential in place of the password (`reconnect::Reauth::Resume`). It lives in this
//! process's memory and nowhere else, exactly as the password did: nothing of it is written
//! to disk, and signing the session out drops it with the session.
//!
//! At the moment it expires the agent says so once (`watch`), whether or not the link is up,
//! and from then on the session's row is marked (`status`). The session itself goes on until
//! its link next drops; that reconnect is refused by name and ends in a sign-in.

pub(crate) mod expiry;
pub(crate) mod status;
pub(crate) mod watch;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod watch_tests;

/// This computer's clock, as Unix milliseconds. The one place the resume code reads it.
pub(crate) fn wall_clock_ms() -> u64 {
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    // A clock before 1970 reads as 1970: nothing has expired yet by it.
    since_epoch.map_or(0, |elapsed| {
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    })
}
