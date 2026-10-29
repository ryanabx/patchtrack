//! Local mail on disk: Maildirs, directories of .eml files, and mbox files.
//!
//! What to keep is the core's (`patchtrack_core::local`); this reads the
//! bytes. A directory is read incrementally by file time, with a day of
//! slack, because delivery may date a file a little before it appears. An
//! mbox not modified since it was last read is skipped.
//!
//! Maildir file names carry flags after ":2," (or "!2," on Windows, where
//! mbsync and others avoid ':'); every file in cur/ and new/ is a message
//! either way, so names are never parsed.

use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

use patchtrack_core::mbox;

const SLACK: i64 = 86_400;

fn mtime(path: &Path) -> io::Result<i64> {
    let t = std::fs::metadata(path)?.modified()?;
    Ok(t.duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX)))
}

/// Every message in a file: an mbox if it starts with "From ", else one message.
pub fn messages(raw: Vec<u8>) -> Vec<Vec<u8>> {
    if raw.starts_with(b"From ") { mbox::split(&raw) } else { vec![raw] }
}

/// The messages at path that may be new since `since` (epoch seconds; None
/// reads everything).
pub fn read(path: &Path, since: Option<i64>) -> io::Result<Vec<Vec<u8>>> {
    if !path.is_dir() {
        if let Some(s) = since
            && mtime(path)? < s
        {
            return Ok(vec![]);
        }
        return Ok(messages(std::fs::read(path)?));
    }
    let maildir: Vec<_> = ["cur", "new"].iter().map(|d| path.join(d)).filter(|d| d.is_dir()).collect();
    let dirs = if maildir.is_empty() { vec![path.to_path_buf()] } else { maildir };
    let mut out = vec![];
    for dir in dirs {
        let mut files: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|f| f.is_file() && !f.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .collect();
        files.sort();
        for f in files {
            if since.is_some_and(|s| mtime(&f).is_ok_and(|t| t < s - SLACK)) {
                continue;
            }
            out.extend(messages(std::fs::read(&f)?));
        }
    }
    Ok(out)
}
