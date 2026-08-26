//! `core/disk-space` — the filesystem hosting `~/.aleph/data` must have headroom.
//!
//! SQLite stores, the vault, and session transcripts all grow under the data
//! dir; a full disk surfaces later as opaque write failures (corrupted WAL,
//! failed vault saves). Below 256 MiB free this is an Error (imminent write
//! failure); below 1 GiB a Warning. **Not repairable** — what to delete is a
//! human decision; the finding only points at the problem.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture, Presence};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/disk-space";

/// Below this many free bytes the finding is an Error (256 MiB).
const ERROR_THRESHOLD_BYTES: u64 = 256 * 1024 * 1024;
/// Below this many free bytes the finding is a Warning (1 GiB).
const WARN_THRESHOLD_BYTES: u64 = 1024 * 1024 * 1024;

/// Threshold classification, pure so it can be unit tested without a filesystem.
/// `None` means enough headroom (ok finding).
fn classify_free_space(free_bytes: u64) -> Option<Severity> {
    if free_bytes < ERROR_THRESHOLD_BYTES {
        Some(Severity::Error)
    } else if free_bytes < WARN_THRESHOLD_BYTES {
        Some(Severity::Warning)
    } else {
        None
    }
}

fn format_mib(bytes: u64) -> String {
    format!("{} MiB", bytes / (1024 * 1024))
}

pub struct DiskSpaceCheck {
    data_dir: PathBuf,
}

impl DiskSpaceCheck {
    #[must_use]
    pub const fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait]
impl HealthCheck for DiskSpaceCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Disk space"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let display = self.data_dir.display().to_string();

        // Free space of the filesystem HOSTING the data dir (fs2 resolves the
        // mount from the path). Anchor on an existing ancestor: the data dir
        // itself may legitimately not exist yet (first run — the data-dir
        // check owns that finding).
        //
        // An UNREADABLE ancestor must stop the walk, not be stepped over.
        // `Path::exists()` cannot tell the two apart, so the walk used to
        // climb past a directory it merely could not stat and hand `fs2` an
        // ancestor on the far side of a mount point — reporting free space
        // for a different filesystem than the one hosting the data dir, as a
        // confident number with no hint that it is the wrong one.
        let mut anchor = self.data_dir.as_path();
        loop {
            match Presence::of(ID, "Free disk space", anchor) {
                Err(f) => return vec![f],
                Ok(Presence::Present) => break,
                Ok(Presence::Absent) => match anchor.parent() {
                    Some(parent) => anchor = parent,
                    None => {
                        return vec![Finding::problem(
                            ID,
                            Severity::Warning,
                            "Free disk space unknown",
                            format!("no existing ancestor of {display} to stat."),
                        )];
                    }
                },
            }
        }

        let free = match fs2::free_space(anchor) {
            Ok(free) => free,
            Err(e) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Free disk space unknown",
                    format!("could not stat free space for {display}: {e}"),
                )];
            }
        };

        match classify_free_space(free) {
            Some(Severity::Error) => vec![Finding::problem(
                ID,
                Severity::Error,
                "Disk almost full",
                format!(
                    "{display} has only {} free; SQLite stores and the vault will start failing writes.",
                    format_mib(free)
                ),
            )
            .with_fix_hint(
                "Free space on the volume hosting the data dir (clear old sessions, \
                 vacuum *.db files, or move ~/.aleph to a larger volume).",
            )],
            Some(_) => vec![Finding::problem(
                ID,
                Severity::Warning,
                "Disk space is low",
                format!(
                    "{display} has {} free; below 1 GiB headroom.",
                    format_mib(free)
                ),
            )
            .with_fix_hint("Consider freeing space on the volume hosting the data dir.")],
            None => vec![Finding::ok(
                ID,
                "Disk space OK",
                format!("{display} has {} free.", format_mib(free)),
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn classify_thresholds() {
        let mib = 1024 * 1024;
        assert_eq!(classify_free_space(0), Some(Severity::Error));
        assert_eq!(classify_free_space(256 * mib - 1), Some(Severity::Error));
        assert_eq!(classify_free_space(256 * mib), Some(Severity::Warning));
        assert_eq!(classify_free_space(1024 * mib - 1), Some(Severity::Warning));
        assert_eq!(classify_free_space(1024 * mib), None);
        assert_eq!(classify_free_space(u64::MAX), None);
    }

    #[tokio::test]
    async fn reports_one_finding_for_real_dir() {
        let tmp = tempdir().unwrap();
        let check = DiskSpaceCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check_id, ID);
        // Whatever the threshold outcome, disk fullness is never mechanically
        // repairable.
        assert!(!findings[0].repairable);
    }

    /// An unreadable ancestor must STOP the walk. `Path::exists()` cannot tell
    /// it from a missing one, so the walk stepped over it and handed `fs2` an
    /// ancestor that can be on the far side of a mount point — a confident
    /// free-space number for a different filesystem.
    ///
    /// The discriminator here is that the parent directory is a real, readable
    /// temp dir with plenty of space: if the walk still climbed, this check
    /// would report a healthy "Disk space OK" for it.
    #[tokio::test]
    async fn an_unreadable_ancestor_stops_the_walk_instead_of_being_stepped_over() {
        let tmp = tempdir().unwrap();
        let unreadable = tmp.path().join("aleph\u{0}data");
        let findings = DiskSpaceCheck::new(unreadable).run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].is_problem(),
            "climbing past the unreadable component would report the parent \
             filesystem as healthy: {:?}",
            findings[0]
        );
        assert!(
            findings[0].detail.contains("would not say"),
            "expected the refusal, not the give-up-at-the-root finding: {}",
            findings[0].detail
        );
    }
}
