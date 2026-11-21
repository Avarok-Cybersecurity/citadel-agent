//! The on-disk format version of the agent's data directory.
//!
//! The workspace server versions its persisted state and refuses to boot on a
//! schema it does not understand. The agent — whose data directory holds users'
//! ratchet keys and accounts, and which local users pull on their own cadence —
//! had no version handling at all.
//!
//! That makes an SDK serialization change indistinguishable from corruption: a
//! routine `docker compose pull` produces an agent that cannot read its own
//! accounts, with no statement of why, and deleting the volume as the only
//! apparent remedy. Those keys do not come back.
//!
//! A stamped version does not migrate anything. It converts silent unreadable
//! state into a refusal that names the cause, which is the difference between
//! "restore your backup / roll back the image" and "delete everything and hope".

use std::path::Path;

/// The format this build writes and understands.
///
/// Bump this ONLY together with a migration, or with a deliberate decision that
/// existing data is to be abandoned — a bump alone makes every existing agent
/// refuse to start.
pub const AGENT_DATA_FORMAT: u32 = 1;

/// The file holding it, inside the data directory.
pub const FORMAT_MARKER: &str = ".citadel-agent-format";

/// What to do about the version found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatVerdict {
    /// Usable as is.
    Proceed,
    /// Usable, and the marker should be written with this value.
    Stamp(u32),
    /// Not usable. The string is for the operator, not the log.
    Refuse(String),
}

/// Decide from what was found, without touching the filesystem: the parsed marker, and whether
/// the directory already holds anything. Only an empty directory is stamped; data with no marker
/// was written by a build before the marker, which this one does not read.
pub fn verdict(found: Option<u32>, holds_data: bool) -> FormatVerdict {
    match found {
        Some(version) if version == AGENT_DATA_FORMAT => FormatVerdict::Proceed,

        Some(version) if version > AGENT_DATA_FORMAT => FormatVerdict::Refuse(format!(
            "This data directory was written by a NEWER Citadel agent (format v{version}; \
             this build understands v{AGENT_DATA_FORMAT}).\n\
             \n\
             Reading it with an older build risks corrupting account and key material, so \
             the agent will not start. Either run the newer image again, or point \
             INTERNAL_SERVICE_DATA_DIR at a different directory.\n\
             \n\
             Do not delete this directory to get past this message: it holds the keys for \
             every account registered on this device, and they cannot be regenerated."
        )),

        Some(version) => FormatVerdict::Refuse(format!(
            "This data directory is in an OLDER format (v{version}; this build writes \
             v{AGENT_DATA_FORMAT}) and no migration exists for it.\n\
             \n\
             The agent will not start rather than read it and risk misinterpreting account \
             and key material. Run the image that wrote v{version}, or point \
             INTERNAL_SERVICE_DATA_DIR at a different directory."
        )),

        None if !holds_data => FormatVerdict::Stamp(AGENT_DATA_FORMAT),

        None => FormatVerdict::Refuse(format!(
            "This data directory holds data but no format marker, so it was written by a              Citadel agent older than any this build reads (it writes v{AGENT_DATA_FORMAT}).
             
             The agent will not start rather than read it and risk misinterpreting account              and key material. Run the image that wrote it, or point              INTERNAL_SERVICE_DATA_DIR at a different directory."
        )),
    }
}

/// Read the marker, decide, and write it when the decision says to.
pub fn check_data_dir(dir: &Path) -> Result<(), std::io::Error> {
    let marker = dir.join(FORMAT_MARKER);

    let found = match std::fs::read_to_string(&marker) {
        Ok(contents) => match contents.trim().parse::<u32>() {
            Ok(version) => Some(version),
            // A marker we cannot parse is not the same as no marker: something
            // wrote it, and guessing is how key material gets misread.
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{} exists but does not contain a version number. Refusing to \
                         guess at the format of a directory holding account keys.",
                        marker.display()
                    ),
                ))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };

    let holds_data = std::fs::read_dir(dir)?.next().is_some();
    match verdict(found, holds_data) {
        FormatVerdict::Proceed => Ok(()),
        FormatVerdict::Stamp(version) => std::fs::write(&marker, version.to_string()),
        FormatVerdict::Refuse(message) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_version_proceeds() {
        assert_eq!(
            verdict(Some(AGENT_DATA_FORMAT), true),
            FormatVerdict::Proceed
        );
    }

    #[test]
    fn a_fresh_directory_is_stamped() {
        assert_eq!(
            verdict(None, false),
            FormatVerdict::Stamp(AGENT_DATA_FORMAT)
        );
    }

    #[test]
    fn a_directory_that_predates_the_marker_is_refused() {
        assert!(matches!(verdict(None, true), FormatVerdict::Refuse(_)));
    }

    #[test]
    fn data_without_a_marker_is_refused_on_disk_and_left_unstamped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("accounts.db"), b"older agent").unwrap();
        assert!(check_data_dir(dir.path()).is_err());
        assert!(!dir.path().join(FORMAT_MARKER).exists());
        let empty = tempfile::tempdir().unwrap();
        check_data_dir(empty.path()).unwrap();
        assert!(empty.path().join(FORMAT_MARKER).exists());
    }

    #[test]
    fn a_newer_format_refuses_and_says_not_to_delete_it() {
        let FormatVerdict::Refuse(message) = verdict(Some(AGENT_DATA_FORMAT + 1), true) else {
            panic!("a newer format must refuse");
        };
        assert!(
            message.contains("cannot be regenerated"),
            "an operator told only 'refusing to start' deletes the volume: {message}"
        );
    }

    #[test]
    fn an_older_format_refuses_rather_than_guessing() {
        assert!(matches!(verdict(Some(0), true), FormatVerdict::Refuse(_)));
    }

    #[test]
    fn every_refusal_names_a_way_out() {
        for found in [Some(0), Some(AGENT_DATA_FORMAT + 1), None] {
            let FormatVerdict::Refuse(message) = verdict(found, true) else {
                panic!("expected a refusal for {found:?}");
            };
            assert!(
                message.contains("INTERNAL_SERVICE_DATA_DIR"),
                "a refusal with no next step is a dead end: {message}"
            );
        }
    }
}
