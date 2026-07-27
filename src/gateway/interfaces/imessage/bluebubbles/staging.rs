//! Attachment staging.
//!
//! Inbound BlueBubbles attachments are downloaded to a dedicated temp subdir so
//! the sweep only ever touches Aleph's own files, never the rest of the system
//! temp dir. Downloads used to land directly in `temp_dir()` and were never
//! removed — an unbounded disk leak. Files are swept once they age past
//! [`RETENTION`]; the agent consumes a download within seconds–minutes, so the
//! window is generous while still bounding disk use.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Retention window for staged attachments (6h — ample margin over the
/// seconds–minutes an agent needs to read a freshly-downloaded file).
pub const RETENTION: Duration = Duration::from_secs(6 * 3600);

/// Directory inbound attachments are downloaded to.
#[must_use]
pub fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("aleph-imessage-attachments")
}

/// Create the staging dir if missing and return it. Best-effort — callers fall
/// back to the bare temp dir if creation fails.
pub fn ensure_staging_dir() -> std::io::Result<PathBuf> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Remove staged files older than `max_age`. Best-effort: a missing dir or an
/// individual removal failure is logged at debug and skipped. Returns how many
/// files were removed.
pub fn sweep_stale(max_age: Duration) -> usize {
    sweep_stale_in(&staging_dir(), max_age)
}

fn sweep_stale_in(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0; // dir doesn't exist yet / unreadable → nothing to sweep
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // `duration_since` errors if mtime is in the future (clock skew); treat
        // that as "not stale" so we never delete a file we can't age-confirm.
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|mtime| now.duration_since(mtime).is_ok_and(|age| age > max_age))
            .unwrap_or(false);
        if stale {
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(e) => {
                    tracing::debug!("attachment sweep: remove {} failed: {e}", path.display())
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_removes_stale_keeps_fresh() {
        // Isolated subdir so parallel tests don't interfere.
        let dir = std::env::temp_dir().join(format!("aleph-sweep-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let stale = dir.join("old.bin");
        let fresh = dir.join("new.bin");
        std::fs::write(&stale, b"x").unwrap();
        std::fs::write(&fresh, b"y").unwrap();

        // Backdate the stale file well past the retention window.
        let old_time = SystemTime::now() - Duration::from_secs(7 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old_time)
            .unwrap();

        let removed = sweep_stale_in(&dir, RETENTION);
        assert_eq!(removed, 1);
        assert!(!stale.exists(), "stale file should be swept");
        assert!(fresh.exists(), "fresh file must survive");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sweep_missing_dir_is_noop() {
        let dir = std::env::temp_dir().join("aleph-sweep-does-not-exist-xyz");
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(sweep_stale_in(&dir, RETENTION), 0);
    }
}
