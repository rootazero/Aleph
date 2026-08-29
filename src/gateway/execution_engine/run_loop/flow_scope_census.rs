//! Source-level census: nothing in `run_loop` reads the scope metadata keys
//! directly.
//!
//! `run_loop::request_scope` is this module's single answer to "what scope is
//! this run under": `crate::scope::scope_from_metadata` corrected by the room
//! a session key was claimed for. Every consumer here has to go through it,
//! because the correction — a bound channel conversation's
//! `personal:<speaker>` becoming the room's scope — exists nowhere else.
//!
//! `src/gateway/CLAUDE.md` 地雷 Q already said so, in as many words, and it
//! still did not hold: the doc enumerated the three readers of the day and
//! said "wire the fourth one to this function". The fourth reader arrived —
//! the `FlowRequest` handed to the harness — read
//! `request.metadata.get(OWNER_META_KEY)` / `.get(SCOPE_META_KEY)` instead,
//! and nothing failed. The session row was stamped with the room while
//! everything past `orchestrator::dispatch`'s `tokio::spawn` ran under the
//! producer's personal stamp.
//!
//! A count in prose goes quiet when the set grows, so this census is a rule
//! rather than a roster: it walks the module directory and fails on ANY
//! production line naming either key, whoever wrote it and whenever. There is
//! deliberately no carve-out for `request_scope` itself — it reaches the keys
//! through `scope_from_metadata`, which is where they are owned. A future
//! reader that genuinely needs the raw keys inside `request_scope` has to
//! come here and say so, which is the whole point.

#[cfg(test)]
mod tests {
    use crate::utils::source_scan::{
        cfg_test_portion, code_text, production_code_lines, production_prefix, rust_sources_under,
    };

    /// The keys this module may not name. Spelled here as the identifiers a
    /// reader would type; `code_text` deletes string-literal payloads, so
    /// these two literals cannot match themselves when this file is scanned.
    const FORBIDDEN: [&str; 2] = ["OWNER_META_KEY", "SCOPE_META_KEY"];

    fn module_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/gateway/execution_engine/run_loop")
    }

    /// Child modules of `run_loop` that exist only in a `cfg(test)` build,
    /// derived from `mod.rs`'s own `#[cfg(test)] mod x;` lines.
    ///
    /// `production_prefix` works one file at a time, so `tests.rs` — which
    /// carries no `#[cfg(test)]` of its own because its PARENT applies one —
    /// looks entirely like production to a per-file scanner. It legitimately
    /// names both keys (it builds metadata maps to drive `request_scope`),
    /// and excluding it by filename would be the enumeration this census
    /// exists to avoid. Reading the declaration instead means a renamed or a
    /// second test module is covered without touching this list.
    fn test_only_children() -> std::collections::BTreeSet<String> {
        let mod_rs = std::fs::read_to_string(module_dir().join("mod.rs"))
            .expect("run_loop/mod.rs must be readable; the census input is the module itself");
        let mut out = std::collections::BTreeSet::new();
        for line in cfg_test_portion(&mod_rs).lines() {
            let t = line.trim_start();
            let t = t
                .strip_prefix("pub(crate) ")
                .or_else(|| t.strip_prefix("pub(super) "))
                .or_else(|| t.strip_prefix("pub "))
                .unwrap_or(t);
            let Some(rest) = t.strip_prefix("mod ") else {
                continue;
            };
            // `mod tests {` is inline and already handled by
            // `production_prefix`; only a `mod x;` names a separate file.
            let Some(name) = rest.strip_suffix(';') else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                out.insert(format!("{name}.rs"));
                out.insert(format!("{name}/mod.rs"));
            }
        }
        out
    }

    /// Every `run_loop` file that is not a test-only child, as
    /// `(repo-relative path, raw contents)`.
    ///
    /// One list for both checks below. Two walks with two independently
    /// spelled skip filters is exactly the drift this module is about: the
    /// negative census would exclude `tests.rs` and the call count would not,
    /// and only one of them would say so.
    fn production_files() -> Vec<(String, String)> {
        let skip = test_only_children();
        rust_sources_under(&module_dir())
            .into_iter()
            .filter(|(rel, _)| {
                let leaf = rel.rsplit_once("run_loop/").map(|(_, t)| t).unwrap_or(rel);
                !skip.contains(leaf)
            })
            .collect()
    }

    /// Proves the needle is findable by this exact pipeline before any
    /// conclusion is drawn from not finding it.
    ///
    /// The census below is a negative assertion over text that has been
    /// through `production_code_lines` and then `code_text`. Either stage
    /// could stop emitting what the search expects — a lexer change, a
    /// partition change — and the census would go green on a module that had
    /// gone back to reading the raw keys. `src/scope/mod.rs` is where both
    /// constants are DEFINED, so it is the one file that cannot stop
    /// containing them, and running it through the same two functions shows
    /// the search string survives them.
    #[test]
    fn the_census_pipeline_can_still_see_the_keys_it_looks_for() {
        let owner = include_str!("../../../scope/mod.rs");
        let scanned = code_text(&production_code_lines(owner));
        for key in FORBIDDEN {
            assert!(
                scanned.contains(key),
                "src/scope/mod.rs defines {key} in production code, but this census's \
                 own scanning pipeline cannot find it there. The negative assertion in \
                 `no_run_loop_file_reads_the_scope_metadata_keys` is therefore vacuous: \
                 it would pass on a module that had gone back to the raw reads."
            );
        }
    }

    /// The site the defect lived at must be reading the resolver.
    ///
    /// Paired with the negative census on purpose: "no file names the keys"
    /// is also satisfied by a `FlowRequest` that forwards nothing at all, or
    /// by deleting the fields. This says what the site must do, the census
    /// says what it must not.
    #[test]
    fn the_flow_request_site_derives_its_scope_from_request_scope() {
        let inner = code_text(&production_prefix(include_str!("inner.rs")));
        assert!(
            inner.contains("request_scope_strings(request)"),
            "the FlowRequest handed to the harness must take its owner/scope from \
             `request_scope_strings`, the projection of `request_scope`. Without it \
             the room upgrade a bound channel conversation earned is dropped at the \
             spawn boundary, and only the session row keeps it."
        );
        let projection = code_text(&production_prefix(include_str!("mod.rs")));
        assert!(
            projection.contains("fn request_scope_strings"),
            "`request_scope_strings` must live beside `request_scope`, so the \
             conversion to FlowRequest's two strings has one owner"
        );
        // Deliberately a COUNT over the module, not `projection.contains(
        // "request_scope(request)")`: that spelling is already in `mod.rs`
        // twice over (`with_request_scope`, `ensure_session_under_request_
        // scope`), so it would have been satisfied no matter what
        // `request_scope_strings` did — a check that cannot go red. What
        // actually has to hold is that `request_scope` stays the module's
        // ONLY caller of `scope_from_metadata`; a projection that called it
        // a second time would drop the room upgrade under a new name and
        // still pass the negative census below, since it names no raw key.
        let mut calls = 0usize;
        for (_, text) in production_files() {
            calls += code_text(&production_prefix(&text))
                .matches("scope_from_metadata")
                .count();
        }
        assert_eq!(
            calls, 1,
            "`scope_from_metadata` must be called exactly once in run_loop — inside \
             `request_scope`, which is where the room-claim correction is applied on \
             top of it. A second call is a second answer to 'what scope is this run \
             under', and the one that skips the correction is the one that ships the \
             producer's `personal:<speaker>` stamp."
        );
    }

    /// No production line in this module names either key.
    #[test]
    fn no_run_loop_file_reads_the_scope_metadata_keys() {
        let mut scanned = 0usize;
        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in production_files() {
            scanned += 1;
            // Two stages, both required. `production_code_lines` blanks the
            // `#[cfg(test)]` items and the comment lines while PRESERVING
            // line numbers, so an offender can be opened at the number this
            // prints; `code_text` then removes string-literal payloads, so a
            // guard message or a test fixture quoting a key is not a hit.
            for (i, line) in code_text(&production_code_lines(&text)).lines().enumerate() {
                for key in FORBIDDEN {
                    if line.contains(key) {
                        offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "run_loop production code must reach this run's scope through \
             `request_scope`, never through the raw metadata keys — only \
             `request_scope` applies the room upgrade a bound channel \
             conversation earned, and a raw read silently ships the producer's \
             own `personal:<speaker>` stamp past this point. Offenders:\n{}",
            offenders.join("\n")
        );
        // A measured floor beside the walk. `rust_sources_under` drops files
        // it cannot read and returns an empty vec for a path that does not
        // exist, and `test_only_children` could over-exclude — each of which
        // turns this census into a scan of nothing that reports clean. Four
        // is the module minus its test file (mod/inner/project_context/
        // author_census) and this file itself makes five; the floor is one
        // below that so adding a file is never a red, and removing the
        // subject files is.
        assert!(
            scanned >= 4,
            "the census inspected {scanned} run_loop files; the module has at least \
             four non-test ones, so a smaller number means the walk found nothing \
             and the clean result above is a scan of an empty corpus"
        );
    }
}
