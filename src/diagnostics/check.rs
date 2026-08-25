//! The `HealthCheck` contract, run posture, and the shared filesystem probes
//! a check answers "is it there?" with.
//!
//! Every domain check (filesystem, lock, config, consent) implements one
//! trait. The engine treats them uniformly and runs them concurrently. A
//! check owns BOTH detection and — when asked — mechanical repair, so the
//! "can I fix this?" knowledge lives next to the "is this broken?" logic
//! rather than in a separate switchboard (avoids the openclaw detect/repair
//! id-matching seam).

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::finding::{Finding, Severity};

/// Wall-clock ceiling a check gets before the engine gives up on it.
///
/// Every check either touches the filesystem, scans the process table, shells
/// out (`which`), or opens a SQLite file — all of which can block on something
/// outside this process. Without a deadline one wedged check hangs the whole
/// report, and because the `doctor` tool runs inside an agent turn, that means
/// a hung *turn* whose only symptom is silence. A timed-out check is folded
/// into an ordinary `Warning` finding naming the check, so the rest of the
/// battery still reports.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Controls whether a run is allowed to mutate state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// Read-only detection for human output.
    Inspect,
    /// Read-only detection for machine output (CLI renders JSON). Behaves
    /// identically to `Inspect` at the check level — the difference is only
    /// in how the engine's report is formatted.
    Lint,
    /// Detect, then apply mechanical repairs for repairable findings.
    Fix,
}

impl Posture {
    /// Whether checks may mutate state on this run.
    #[must_use]
    pub const fn allows_repair(self) -> bool {
        matches!(self, Self::Fix)
    }
}

/// A single diagnostic domain. Implementations resolve the resources they
/// inspect at construction time (paths, registries) so they can be unit
/// tested against temp fixtures without touching the real home directory.
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Stable, namespaced identifier, e.g. `core/data-dir`.
    fn id(&self) -> &'static str;

    /// Short human label for headers.
    fn title(&self) -> &'static str;

    /// Run detection. When `posture.allows_repair()` and a problem is
    /// repairable, the check should apply the repair and record the outcome
    /// on the returned finding via `Finding::with_repair`.
    async fn run(&self, posture: Posture) -> Vec<Finding>;

    /// This check's wall-clock ceiling. Override only when the check has a
    /// *known* inner bound that legitimately exceeds
    /// [`DEFAULT_CHECK_TIMEOUT`] — state the inner bound in the override's
    /// doc comment so the number stays traceable to a real budget rather than
    /// becoming a magic constant.
    fn timeout(&self) -> Duration {
        DEFAULT_CHECK_TIMEOUT
    }
}

/// Whether a path is there — with the third answer moved out of the enum.
///
/// [`Path::exists`] returns `false` for two different worlds: the path is not
/// there, and *the filesystem refused to tell me*. In ordinary code that
/// conflation is usually harmless. In a diagnostic it is the whole job: this
/// subsystem exists to tell an operator what is true, and it is the one place
/// where "I could not look" must never render as "there is nothing there".
/// Six checks in `checks/` used to answer the second world with the first
/// world's reassuring sentence — "no secrets stored yet" in front of a vault
/// that is present but unreadable being the worst of them.
///
/// # Why the third answer is an `Err` and not a third variant
///
/// A `Presence::Unknown(e)` variant leaves `matches!(p, Presence::Absent)` and
/// a catch-all `_ =>` arm writable, which is the exact shape of the defect:
/// the unknown falls into the absent arm and the check prints the reassuring
/// line. Putting it behind a [`Result`] puts it where the compiler will not
/// let it be read as a `Presence` at all. It can still be spent as absence —
/// by writing `unwrap_or(Presence::Absent)` — but that is a deliberate
/// sentence a reviewer can see, not an omission.
///
/// The `Err` payload is already the finding, in this file family's house style
/// for "unknown" (`"SQLite integrity unknown"`, `"Free disk space unknown"`),
/// so a caller's entire obligation is `Err(f) => return vec![f]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The path is there.
    Present,
    /// The path is *determinately* not there — `ErrorKind::NotFound`, not a
    /// refusal to look.
    Absent,
}

impl Presence {
    /// Probe `path`. `subject` is the noun phrase the "unknown" finding is
    /// titled with (`"Free disk space"` renders as `"Free disk space
    /// unknown"`), so it should name what the check could not determine, not
    /// the path.
    ///
    /// # Errors
    ///
    /// Returns the ready-made `Warning` finding when [`Path::try_exists`]
    /// reports an error other than "not found" — a permission error on a
    /// parent, an IO error, a filesystem that went away.
    // The `Err` IS the finding this check will report, so it is exactly as
    // large as a `Finding` by design; boxing it would make every call site
    // deref for no gain. Same house shape as the
    // `Result<_, JsonRpcResponse>` gates in `gateway::handlers`.
    #[allow(clippy::result_large_err)]
    pub fn of(check_id: &'static str, subject: &str, path: &Path) -> Result<Self, Finding> {
        match path.try_exists() {
            Ok(true) => Ok(Self::Present),
            Ok(false) => Ok(Self::Absent),
            Err(e) => Err(unreadable_path_finding(check_id, subject, path, &e)),
        }
    }

    /// True when the path is determinately not there.
    #[must_use]
    pub const fn is_absent(self) -> bool {
        matches!(self, Self::Absent)
    }
}

/// What a directory walk found, with the same discipline as [`Presence`].
///
/// `read_dir` fails for the same two worlds, and dropping its `Err` turns
/// "I could not open this directory" into "this directory holds nothing" —
/// which for `core/sqlite-integrity` reads as *"nothing to verify (normal
/// before first run)"* on a data dir that may be full of stores it could not
/// see.
///
/// [`Self::Absent`] is kept separate from an empty [`Self::Listed`] rather
/// than folded into it: "not there" is a determinate answer, and a caller may
/// legitimately want to say something different about it (`core/data-dir`
/// owns the "missing data directory" finding, so `core/sqlite-integrity` must
/// *not* re-report it as a scary error). Folding an indeterminate answer into
/// a determinate one is the defect; folding one determinate answer into
/// another is a caller's choice, made where a reader can see it.
pub enum DirListing {
    /// The directory itself is not there.
    Absent,
    /// The directory was walked.
    Listed {
        /// Paths of the entries the OS did hand over.
        entries: Vec<PathBuf>,
        /// How many entries the OS refused **part-way through** the walk.
        ///
        /// Non-zero means this listing is INCOMPLETE, so a caller must not
        /// report "I found nothing" as "there is nothing". `read_dir`'s
        /// iterator yields `io::Result<DirEntry>`; the usual
        /// `.filter_map(Result::ok)` silently shortens the listing instead.
        unreadable_entries: usize,
    },
}

impl DirListing {
    /// Walk `dir` one level. See [`Presence::of`] for what `subject` is.
    ///
    /// # Errors
    ///
    /// Returns the ready-made `Warning` finding when the directory exists but
    /// cannot be opened.
    // The `Err` IS the finding this check will report, so it is exactly as
    // large as a `Finding` by design; boxing it would make every call site
    // deref for no gain. Same house shape as the
    // `Result<_, JsonRpcResponse>` gates in `gateway::handlers`.
    #[allow(clippy::result_large_err)]
    pub fn of(check_id: &'static str, subject: &str, dir: &Path) -> Result<Self, Finding> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Absent),
            Err(e) => return Err(unreadable_path_finding(check_id, subject, dir, &e)),
        };
        let mut listed = Vec::new();
        let mut unreadable_entries = 0usize;
        for entry in entries {
            match entry {
                Ok(entry) => listed.push(entry.path()),
                Err(_) => unreadable_entries += 1,
            }
        }
        Ok(Self::Listed {
            entries: listed,
            unreadable_entries,
        })
    }
}

/// The house style for "this check could not determine its own subject":
/// `Severity::Warning`, titled `"<subject> unknown"`.
///
/// One constructor rather than the phrase spelled per check, so "unknown"
/// keeps meaning the same severity everywhere — `Info` would render
/// byte-identically to a genuine pass (`render_human` maps it to `[ok]` and
/// prints `detail` only when [`Finding::is_problem`]), which is exactly the
/// invisibility this whole family of findings exists to avoid.
#[must_use]
pub fn unknown_finding(
    check_id: &'static str,
    subject: &str,
    detail: impl Into<String>,
) -> Finding {
    Finding::problem(
        check_id,
        Severity::Warning,
        format!("{subject} unknown"),
        detail,
    )
}

/// [`unknown_finding`] specialised to "the filesystem refused to answer about
/// this path", which is the only way [`Presence::of`] and [`DirListing::of`]
/// fail.
fn unreadable_path_finding(
    check_id: &'static str,
    subject: &str,
    path: &Path,
    e: &std::io::Error,
) -> Finding {
    let display = path.display();
    unknown_finding(
        check_id,
        subject,
        format!(
            "the filesystem would not say whether {display} is there: {e}. That is not the \
             same answer as \"it is not there\", so this check reports the question instead \
             of the reassuring guess."
        ),
    )
    .with_fix_hint(format!(
        "Check ownership and permissions on {display} and every directory above it: \
         ls -ld \"{display}\""
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path the OS will refuse to answer about, without needing root, a
    /// `chmod 000` fixture, or anything platform-specific: `std` rejects an
    /// interior NUL before the syscall, on every platform.
    ///
    /// It produces `ErrorKind::InvalidInput` rather than `PermissionDenied`,
    /// which is the point — the branch under test is "any `Err` that is not
    /// `NotFound`", and this reaches it deterministically where a permission
    /// fixture would be skipped when the suite happens to run as root.
    fn unanswerable() -> &'static Path {
        Path::new("aleph\u{0}doctor")
    }

    /// The defect, stated as a test: the API this replaced answers `false`
    /// here, which every caller then reads as "not there".
    #[test]
    fn the_api_this_replaced_calls_an_unanswerable_path_absent() {
        assert!(!unanswerable().exists());
    }

    #[test]
    fn an_unanswerable_path_is_not_absent() {
        let err = Presence::of("core/test", "Widget state", unanswerable())
            .expect_err("a refusal to look must not be reported as a Presence");
        assert_eq!(err.severity, Severity::Warning);
        assert_eq!(err.title, "Widget state unknown");
        assert!(
            err.detail.contains("would not say"),
            "detail: {}",
            err.detail
        );
        assert!(err.fix_hint.is_some());
        // Not repairable: nothing here is a mechanical fix, and offering one
        // is how `core/data-dir` came to report "Created" for a no-op.
        assert!(!err.repairable);
    }

    #[test]
    fn a_missing_path_is_absent_and_a_real_one_is_present() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            Presence::of("core/test", "Widget state", &tmp.path().join("nope")).unwrap(),
            Presence::Absent
        );
        assert_eq!(
            Presence::of("core/test", "Widget state", tmp.path()).unwrap(),
            Presence::Present
        );
    }

    #[test]
    fn a_directory_that_will_not_open_is_not_an_empty_listing() {
        let err = DirListing::of("core/test", "Widget inventory", unanswerable())
            .err()
            .expect("a refusal to open must not be reported as a listing");
        assert_eq!(err.severity, Severity::Warning);
        assert_eq!(err.title, "Widget inventory unknown");
    }

    #[test]
    fn a_missing_directory_is_absent_and_a_real_one_is_listed() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            DirListing::of("core/test", "Widget inventory", &tmp.path().join("nope")).unwrap(),
            DirListing::Absent
        ));
        std::fs::write(tmp.path().join("a.db"), b"x").unwrap();
        match DirListing::of("core/test", "Widget inventory", tmp.path()).unwrap() {
            DirListing::Listed {
                entries,
                unreadable_entries,
            } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(unreadable_entries, 0);
            }
            DirListing::Absent => panic!("a directory that exists must not read as absent"),
        }
    }

    /// `unknown_finding` is the single source of the "unknown" house style, so
    /// severity cannot drift back to `Info` in one check while the rest keep
    /// `Warning`. `Info` would render byte-identically to a genuine pass.
    #[test]
    fn the_unknown_house_style_is_a_warning_titled_subject_unknown() {
        let f = unknown_finding("core/test", "Free disk space", "because");
        assert_eq!(f.severity, Severity::Warning);
        assert_eq!(f.title, "Free disk space unknown");
        assert!(f.is_problem(), "an unknown must never read as a pass");
    }
}
