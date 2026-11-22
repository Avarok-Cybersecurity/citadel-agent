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
/// refuse to start. 2: the release on SDK protocol 0.12.4, whose accounts and
/// sessions no earlier agent's data can hold (owner, 2026-11-21: abandoned).
pub const AGENT_DATA_FORMAT: u32 = 2;

/// The file holding it, inside the data directory.
pub const FORMAT_MARKER: &str = ".citadel-agent-format";

/// What to do about the version found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatVerdict {
    /// Usable as is.
    Proceed,
    /// Usable, and the marker should be written with this value.
    Stamp(u32),
    /// Data from an earlier agent: refused, never read.
    Earlier,
    /// Data from a newer agent: refused, never read.
    Newer(u32),
}

/// Decide from what was found, without touching the filesystem: the parsed marker, and whether
/// the directory already holds anything. Only an empty directory is stamped; data with no marker,
/// or an older one, was written by an earlier agent, which this one does not read.
pub fn verdict(found: Option<u32>, holds_data: bool) -> FormatVerdict {
    match found {
        Some(version) if version == AGENT_DATA_FORMAT => FormatVerdict::Proceed,
        Some(version) if version > AGENT_DATA_FORMAT => FormatVerdict::Newer(version),
        Some(_) => FormatVerdict::Earlier,
        None if !holds_data => FormatVerdict::Stamp(AGENT_DATA_FORMAT),
        None => FormatVerdict::Earlier,
    }
}

/// What the person running the agent is told about a refused directory.
pub fn refusal_copy(verdict: &FormatVerdict, dir: &Path) -> Option<String> {
    let dir = dir.display();
    match verdict {
        FormatVerdict::Earlier => Some(format!(
            "This agent's data is from an earlier version of Citadel, which this version cannot \
             read.\n\nStart fresh by removing {dir} (with the agent stopped), then start the agent \
             again and sign up. Or point --data-dir or INTERNAL_SERVICE_DATA_DIR at a different \
             folder."
        )),
        FormatVerdict::Newer(version) => Some(format!(
            "This agent's data at {dir} was written by a newer version of Citadel (format \
             v{version}; this one reads v{AGENT_DATA_FORMAT}).\n\nRun the newer agent again, or \
             point --data-dir or INTERNAL_SERVICE_DATA_DIR at a different folder. Do not delete \
             the folder to get past this message: it holds the keys for every account on this \
             device."
        )),
        FormatVerdict::Proceed | FormatVerdict::Stamp(_) => None,
    }
}

/// Read the marker, decide, and write it when the decision says to. A directory that does not
/// exist yet is created (owner-only on unix) and stamped.
pub fn check_data_dir(dir: &Path) -> Result<(), std::io::Error> {
    let verdict = read_verdict(dir)?;
    if let Some(message) = refusal_copy(&verdict, dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        ));
    }
    match verdict {
        FormatVerdict::Stamp(version) => {
            std::fs::write(dir.join(FORMAT_MARKER), version.to_string())
        }
        _ => Ok(()),
    }
}

/// The verdict on `dir` as found, without writing the marker: what `check_data_dir` decides
/// on, for a caller that acts on a refusal by kind rather than by its message. A directory that
/// does not exist yet is created (owner-only on unix), as `check_data_dir` creates it.
pub fn read_verdict(dir: &Path) -> Result<FormatVerdict, std::io::Error> {
    if !dir.exists() {
        create_private_dir(dir)?;
    }
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
    Ok(verdict(found, holds_data))
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

#[cfg(test)]
#[path = "data_format_tests.rs"]
mod tests;
