//! Concrete health checks registered with the [`DiagnosticEngine`](super::DiagnosticEngine).
//!
//! Each submodule is one diagnostic domain and owns both its detection and
//! (where safe) its mechanical repair. Add a new domain by implementing
//! [`HealthCheck`](super::HealthCheck) here and registering it in
//! [`super::DiagnosticEngine::default_registry`] (OCP — no engine changes) —
//! **unless it cannot answer from a cold process**, in which case it belongs
//! on one of the opt-in builders instead. `default_registry` is what the
//! offline `aleph-server doctor` builds, and a check that always fires there
//! turns that command's exit code into a constant; see that function's doc.
//!
//! Answering "is this path there?" is [`super::check::Presence`]'s job, never
//! `Path::exists()` — see the `presence_discipline` test module at the foot of
//! this file for why, and for the guard that keeps it that way.

pub mod browser_runtime;
pub mod cache_health;
pub mod cache_hit_rate;
pub mod capability_wiring;
pub mod config_parse;
pub mod data_dir;
pub mod disk_space;
pub mod duplicate_instance;
pub mod hooks_consent;
pub mod idle_extensions;
pub mod loop_graph;
pub mod media_codecs;
pub mod providers_connectivity;
pub mod sqlite_integrity;
pub mod stale_lock;
pub mod vault;

pub use browser_runtime::BrowserRuntimeCheck;
pub use cache_health::CacheHealthCheck;
pub use cache_hit_rate::CacheHitRateCheck;
pub use capability_wiring::CapabilityWiringCheck;
pub use config_parse::ConfigParseCheck;
pub use data_dir::DataDirCheck;
pub use disk_space::DiskSpaceCheck;
pub use duplicate_instance::DuplicateInstanceCheck;
pub use hooks_consent::HooksConsentCheck;
pub use idle_extensions::IdleExtensionsCheck;
pub use loop_graph::LoopGraphCheck;
pub use media_codecs::MediaCodecsCheck;
pub use providers_connectivity::ProvidersConnectivityCheck;
pub use sqlite_integrity::SqliteIntegrityCheck;
pub use stale_lock::StaleLockCheck;
pub use vault::VaultCheck;

#[cfg(test)]
mod presence_discipline {
    use crate::utils::source_scan::{code_text, production_prefix, rust_sources_under};

    /// Spellings that answer "is it there?" with `false` for BOTH "it is not
    /// there" and "the filesystem would not tell me".
    ///
    /// Paired with the replacement a reader is supposed to reach for, because
    /// a rule that only forbids is a rule people work around.
    const CONFLATING: [(&str, &str); 2] = [
        (
            ".exists()",
            "`check::Presence::of(ID, \"<subject>\", path)?` — it returns the third \
             answer as an `Err(Finding)` you cannot spend as absence by accident",
        ),
        (
            "read_dir",
            "`check::DirListing::of(ID, \"<subject>\", dir)?` — it separates \"the \
             directory is not there\" from \"the directory would not open\", and counts \
             entries the walk could not read",
        ),
    ];

    /// A check must never dress "I could not look" as "there is nothing there".
    ///
    /// Eight production sites across seven files in this directory did exactly
    /// that, in three different directions: six answered a stat error with a
    /// reassuring `Finding::ok` ("no secrets stored yet" in front of an
    /// unreadable vault), one answered it with the wrong problem and then let
    /// `--fix` report a repair it had not performed, and one walked past an
    /// unreadable ancestor and reported free space for a different filesystem.
    /// Converting those eight would have closed eight instances and left the
    /// class open; this closes the class.
    ///
    /// # What it can and cannot see
    ///
    /// - *Sees*: the literal spellings in [`CONFLATING`] anywhere in the
    ///   production half of any `.rs` file under `src/diagnostics/checks/`.
    /// - *Blind to*: `Path::is_file()`, `Path::is_dir()` and `Path::metadata()`,
    ///   which conflate exactly the same way. A text rule cannot tell
    ///   `Path::is_file()` from `Metadata::is_file()`, and
    ///   `sqlite_integrity::list_databases` deliberately uses the latter — so a
    ///   rule covering them would need an exemption, and an exemption is the
    ///   thing that later hides a real hit. Named here rather than left for
    ///   someone to discover.
    /// - *Blind to*: the conflation happening in a helper this directory
    ///   delegates to. `stale_lock.rs` has no `.exists()` because
    ///   `utils::instance_lock::diagnose_holder` does the probing, and its
    ///   `read_to_string(..).ok()?` reads an unreadable holder file as "no lock
    ///   file at all". That is the same class one directory over, and out of
    ///   this rule's stated scope.
    /// - *Blind to* runtime behaviour generally: this is a spelling rule. It
    ///   cannot see a new conflating API, only the two that were used here.
    /// - **No allowlist, by construction.** If a site genuinely needs a bare
    ///   `.exists()`, the answer is that `check::Presence` is missing a case —
    ///   extend it there, where every check inherits the fix.
    ///
    /// CRLF-safe: `production_prefix` and `code_text` both drop `\r` before
    /// anything else, so nothing here is anchored to a bare `\n`.
    ///
    /// Reads the tree `CARGO_MANIFEST_DIR` pointed at when this binary was
    /// COMPILED, not the tree on disk now — rebuild before believing a green.
    #[test]
    fn no_check_answers_a_stat_error_with_absence() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("diagnostics")
            .join("checks");
        let sources = rust_sources_under(&root);

        let mut offenders: Vec<String> = Vec::new();
        // Self-count #2: how many files were seen to call the replacement.
        // File count alone cannot separate "scanned real code" from "scanned
        // empty strings" — `code_text` returning blanks would look identical.
        let mut probe_users = 0usize;

        for (rel, text) in &sources {
            let prod = code_text(&production_prefix(text));
            if prod.contains("Presence::of(") || prod.contains("DirListing::of(") {
                probe_users += 1;
            }
            for line in prod.lines() {
                for (marker, replacement) in CONFLATING {
                    if line.contains(marker) {
                        offenders.push(format!(
                            "{rel}: `{}` — `{marker}` cannot tell absence from a refusal to \
                             look. Use {replacement}.",
                            line.trim()
                        ));
                    }
                }
            }
        }

        // Self-count #1: the walk reached this directory at all. 17 files at
        // the time of writing (16 checks + this mod.rs).
        assert!(
            sources.len() >= 15,
            "the walk found only {} .rs files under src/diagnostics/checks/ — a guard \
             that examined nothing is green and blind, not clean",
            sources.len()
        );
        assert!(
            probe_users >= 8,
            "only {probe_users} file(s) under src/diagnostics/checks/ were seen calling \
             `Presence::of(` / `DirListing::of(`; eight were converted when this rule was \
             written, so a lower number means either the scanner stopped reading code or \
             a conversion was reverted. Deleting a check legitimately lowers it — lower \
             this floor deliberately, do not delete the assertion."
        );
        assert!(
            offenders.is_empty(),
            "a diagnostic is the one place where \"I could not look\" must never render \
             as \"there is nothing there\":\n  {}",
            offenders.join("\n  ")
        );
    }
}
