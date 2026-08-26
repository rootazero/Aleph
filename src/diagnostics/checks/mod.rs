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
    const CONFLATING: [(&str, &str); 4] = [
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
        // Two markers, one rule: `Err(_` reaches `Err(_)`, `Err(_e)`,
        // `Err(_err)` and anything else whose binding starts with `_`, and
        // `Err(..)` is the remaining spelling of the same discard. Measured
        // before widening: zero occurrences of either in this directory's
        // production halves, so covering all of them costs nothing — and a
        // rule that only knew the one spelling its author happened to meet
        // would be tighter in the doc than in the tree.
        (
            "Err(_",
            "a bound error — `Err(e)`, not `Err(_)` / `Err(_e)` / `Err(..)` — and one \
             arm per error that actually MEANS absence, everything else through \
             `check::unknown_finding`. A discarded error cannot be told apart from the \
             answer the check then invents",
        ),
        (
            "Err(..)",
            "a bound error — `Err(e)` — and one arm per error that actually MEANS \
             absence, everything else through `check::unknown_finding`. A discarded \
             error cannot be told apart from the answer the check then invents",
        ),
    ];

    /// Does `line` use `marker` as a token, rather than as the tail of a longer
    /// identifier?
    ///
    /// Whether a left boundary is required is DERIVED from the marker itself
    /// rather than carried as a per-entry flag, so a marker added later cannot
    /// forget to declare which kind it is: a marker that opens with an
    /// identifier character needs one, a marker that opens with punctuation
    /// does not. `.exists()` opens with `.`, so `path.exists()` must still
    /// match even though `h` precedes the dot. `Err(_` and `read_dir` open with
    /// a letter, so `ParseErr(_)` and a hypothetical `spread_dir` must not.
    ///
    /// This exists because the widened `Err(` marker fired on
    /// `enum FakeEnum { ParseErr(u32) }` — no `Result`, no discarded error,
    /// nothing to do with the rule. Measured across `src/`, three real
    /// occurrences of that lexical shape exist today (`UnwrapErr(SysRng)`, in
    /// `gateway/security/{crypto,canvas_caps,artifact_caps}.rs`), none of them
    /// inside this directory — so the class is real rather than hypothetical,
    /// and tightening changes nothing about today's verdict.
    ///
    /// A rule that is LOOSER in the tree than in its doc is worse than one that
    /// is tighter: a guard that can fire on innocent code gets edited around by
    /// whoever it blocks, or gets cited as evidence for something it did not
    /// see. Both cost more than missing a spelling.
    ///
    /// All occurrences on the line are considered, not just the first — a
    /// `match` arm list can hold an innocent lookalike and a real offender on
    /// one line.
    fn uses_marker(line: &str, marker: &str) -> bool {
        fn ident(c: char) -> bool {
            c.is_alphanumeric() || c == '_'
        }
        let needs_left_boundary = marker.chars().next().is_some_and(ident);
        line.match_indices(marker).any(|(at, _)| {
            !needs_left_boundary || line[..at].chars().next_back().is_none_or(|c| !ident(c))
        })
    }

    /// The guard's one permanent NEGATIVE case: proof it stays quiet when it
    /// should, not just that it fires when it should.
    ///
    /// Every falsification of this guard so far has been "break the production
    /// code, watch it go RED". None of them could show the other half, because
    /// a green scan of a directory that contains no lookalikes proves nothing
    /// about lookalikes. Asserting on the predicate is the level where both
    /// halves are expressible: `uses_marker` is the scan's *only* decision, so
    /// a predicate that is right on these inputs is a scanner that is right on
    /// them.
    ///
    /// That equivalence was checked once rather than argued: the whole
    /// lookalike set below was planted into `vault.rs` as real production text
    /// — `enum FakeEnum { ParseErr(u32), IoErr(u32) }` with matching arms,
    /// `UnwrapErr(SysRng)`, `fn spread_dir()` — and the scanner stayed GREEN,
    /// while the five real spellings planted across five files each went RED
    /// naming their file. A permanent file-level negative is deliberately NOT
    /// kept: it would mean shipping a fixture inside
    /// `src/diagnostics/checks/`, i.e. production code whose only purpose is
    /// to be scanned. Stated so the narrower standing guarantee is not read as
    /// the wider one-off check.
    ///
    /// The lookalikes are not invented: `ParseErr(_)` is the plant that
    /// exposed the bug, and `UnwrapErr(` occurs three times in `src/` today.
    #[test]
    fn the_marker_matcher_fires_on_real_spellings_and_stays_quiet_on_lookalikes() {
        // Fires — every spelling the rule claims to cover.
        for (line, marker) in [
            ("        Err(_) => ChromiumProbe::Missing,", "Err(_"),
            ("        Err(_e) => Ok(NodeProbe::Missing),", "Err(_"),
            ("        Err(_err) => 0,", "Err(_"),
            ("        Err(..) => Ok(()),", "Err(..)"),
            ("        Result::Err(_) => 0,", "Err(_"),
            ("        Ok(v) => v, Err(_) => 0,", "Err(_"),
            ("Err(_) => 0,", "Err(_"),
            ("    let e = std::fs::read_dir(dir);", "read_dir"),
            ("    if !self.vault_path.exists() {", ".exists()"),
        ] {
            assert!(uses_marker(line, marker), "must flag `{marker}` in: {line}");
        }

        // Stays quiet — the tail of a longer identifier is not the token.
        for (line, marker) in [
            ("        ParseErr(_) => 0,", "Err(_"),
            ("        IoErr(_e) => 0,", "Err(_"),
            ("        MyErr(..) => 0,", "Err(..)"),
            ("    let _: UnwrapErr(SysRng);", "Err(_"),
            ("    fn spread_dir() {}", "read_dir"),
            ("    let x = 0;", "Err(_"),
        ] {
            assert!(
                !uses_marker(line, marker),
                "must NOT flag `{marker}` in: {line}"
            );
        }
    }

    /// A check must never dress "I could not look" as "there is nothing there".
    ///
    /// Eight production sites across **eight** files in this directory did
    /// exactly that, in three different directions: six answered a stat error
    /// with a reassuring `Finding::ok` ("no secrets stored yet" in front of an
    /// unreadable vault), one answered it with the wrong problem and then let
    /// `--fix` report a repair it had not performed, and one walked past an
    /// unreadable ancestor and reported free space for a different filesystem.
    /// (An earlier revision of this comment said "seven files"; the figure was
    /// inherited from a brief rather than counted, and `probe_users >= 8` three
    /// screens down — which counts files — contradicted it.)
    ///
    /// `browser_runtime.rs` was a ninth, in a shape the first sweep could not
    /// see: `Err(_) => …::Missing` and `.unwrap_or(…::Missing)` on a
    /// `spawn_blocking` `JoinError`, all rendering `[ok]`. A panicked probe task
    /// reported "no browser installed", reassuringly. The `Err(_)` rule below
    /// exists because of it.
    ///
    /// Converting the sites one by one would have closed instances and left the
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
    /// - *Blind to* `unwrap_or` / `unwrap_or_else` / `unwrap_or_default`
    ///   applied to a `Result` — the other spelling of "discard the error and
    ///   invent the answer", and the one `browser_runtime.rs` used on its
    ///   `spawn_blocking` `JoinError`. `Option::unwrap_or` is lexically
    ///   identical, and the production halves of this directory hold **nine
    ///   such occurrences across eight lines** that are not this defect: eight
    ///   `Option::unwrap_or*`, plus one `Result::unwrap_or`
    ///   (`u32::try_from(..).unwrap_or(0)` in `cache_health.rs`, clamping an
    ///   out-of-range streak — and note that this one shares a line with an
    ///   `Option` use, which is why "eight lines" and "nine occurrences" are
    ///   different numbers). So a rule on the spelling would need an allowlist
    ///   covering both kinds. A statement-bounded "`.await` and `unwrap_or` in
    ///   one expression" rule WOULD be clean against today's tree (measured:
    ///   none of those nine has an `.await` in its statement) and is
    ///   deliberately not shipped — the first legitimate `Option`-yielding
    ///   `.await` makes it a false accuser, and a guard that accuses falsely
    ///   gets cited as evidence.
    /// - *Blind to* runtime behaviour generally: this is a spelling rule. It
    ///   cannot see a new conflating API, only the shapes that were used here.
    /// - Markers are matched as tokens, not as bare substrings — see
    ///   [`uses_marker`]. That is a deliberate trade in the safe direction: the
    ///   rule now misses an `Err(_)` written with a zero-width character before
    ///   it, and no longer accuses `ParseErr(_)` of anything.
    /// - **No allowlist, by construction.** If a site genuinely needs a bare
    ///   `.exists()`, the answer is that `check::Presence` is missing a case —
    ///   extend it there, where every check inherits the fix.
    ///
    /// CRLF-safe: `production_prefix` and `code_text` both drop `\r` before
    /// anything else, so nothing here is anchored to a bare `\n`.
    ///
    /// `CARGO_MANIFEST_DIR` is baked in at COMPILE time, but
    /// `rust_sources_under` reads file *contents* at run time — so this reads
    /// the CURRENT tree at that path, not a snapshot. The hazard that leaves is
    /// narrower and worth naming precisely: a test binary built in worktree A
    /// scans worktree A even when the command is run from worktree B.
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
                    if uses_marker(line, marker) {
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
