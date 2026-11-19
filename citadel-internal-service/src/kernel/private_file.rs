//! A small private record on disk: written whole to a temporary file and renamed over the old
//! one, so a crash leaves either record and never half of one. The directory is created private
//! (0700) and the file 0600. Shared by the identity store and the RE-VFS seed vault.

use std::path::Path;

/// The record at `dir/file_name`, `None` when there is none.
pub(crate) async fn read(dir: &Path, file_name: &str) -> Result<Option<Vec<u8>>, String> {
    match tokio::fs::read(dir.join(file_name)).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("reading {}: {err}", dir.display())),
    }
}

/// Replaces the record at `dir/file_name` through `dir/temp_name`.
pub(crate) async fn write(
    dir: &Path,
    file_name: &'static str,
    temp_name: &'static str,
    record: Vec<u8>,
) -> Result<(), String> {
    let owned = dir.to_path_buf();
    tokio::task::spawn_blocking(move || write_blocking(&owned, file_name, temp_name, &record))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| format!("writing {}: {err}", dir.display()))
}

fn write_blocking(
    dir: &Path,
    file_name: &str,
    temp_name: &str,
    record: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    let temp = dir.join(temp_name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temp)?;
    file.write_all(record)?;
    file.sync_all()?;
    std::fs::rename(&temp, dir.join(file_name))
}
