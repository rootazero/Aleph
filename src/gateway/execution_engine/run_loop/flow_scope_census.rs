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
//!
//! # What a rule about a SPELLING could not do, and how the layers divide
//!
//! The first version of this census searched for the two key IDENTIFIERS in
//! [`crate::utils::source_scan::code_text`] output, and `code_text` deletes
//! string-literal payloads by design. So `request.metadata.get("scope_id")` —
//! the same read, spelled by the key's VALUE — was invisible to all three
//! checks, and a review drove exactly that to a live `43 / 7 / 2` on the
//! real-machine fixture from a build where this module reported clean. Adding
//! the values to `FORBIDDEN` cannot fix it: the scanner deletes them before
//! the search runs.
//!
//! Three layers now, each answering a different question, none subsuming
//! another:
//!
//! 1. **The type.** [`crate::scope::FlowScope`] has private fields and one
//!    non-empty constructor taking an already-resolved `ScopeAttribution`, so
//!    the `FlowRequest` site — where the defect actually lived — cannot be
//!    handed a pair of strings at all. That is not a spelling rule; it is a
//!    compile error. It is also the only layer that reaches beyond this
//!    module.
//! 2. **The negative census**, now run over BOTH views of each file: key
//!    identifiers in `code_text`, and the key values as exact quoted literals
//!    in [`crate::utils::source_scan::code_keeping_literals`]. This is what
//!    covers a raw read that never touches `FlowRequest` — one taken to decide
//!    something, where no type stands in the way.
//! 3. **The call counts**: `scope_from_metadata` exactly once (a second answer
//!    to "what scope is this run under", under a new name, naming no raw key),
//!    `FlowScope::resolved` exactly once, `FlowScope::unscoped` never.
//!
//! The bound, stated so nobody has to infer it: a raw read OUTSIDE this module
//! is not covered by 2 or 3, and `BusyInputMode::for_shared_room`
//! (`execution_engine/mod.rs`) is one — it runs on the admission path, before
//! `request_scope` can have run, and answers a different question with a
//! different predicate. See its doc.

#[cfg(test)]
mod tests {
    use crate::utils::source_scan::{
        cfg_test_portion, code_keeping_literals, code_text, production_code_lines,
        production_prefix, rust_sources_under,
    };

    /// The keys this module may not name. Spelled here as the identifiers a
    /// reader would type; `code_text` deletes string-literal payloads, so
    /// these two literals cannot match themselves when this file is scanned.
    const FORBIDDEN: [&str; 2] = ["OWNER_META_KEY", "SCOPE_META_KEY"];

    /// The same two keys as the LITERAL a reader types instead of the
    /// constant, quotes included.
    ///
    /// Derived from the constants rather than typed out, for two reasons. The
    /// bare text `scope_id` is also the `FlowRequest` field name on the line
    /// being protected, so an unquoted needle would fire on the fixed code;
    /// and a literal typed here would be a second copy of a value this module
    /// does not own, free to drift from `src/scope/mod.rs`.
    ///
    /// Quotes are part of the needle, which makes this an EXACT payload match:
    /// a message that mentions the key in prose (`"scope_id missing"`) is not
    /// a hit, and neither is an escaped inner quote. That is what lets the
    /// search run over text that still HAS its literals without becoming a
    /// guard that cries wolf.
    fn forbidden_literals() -> [String; 2] {
        [
            format!("\"{}\"", crate::scope::OWNER_META_KEY),
            format!("\"{}\"", crate::scope::SCOPE_META_KEY),
        ]
    }

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
    ///
    /// Both views are proved, because the census now searches both and a
    /// half-proof would leave the newer half exactly as vacuous as the hole it
    /// closes. `pub const SCOPE_META_KEY: &str = "scope_id";` is one line
    /// carrying the identifier AND the quoted value, so the same anchor line
    /// serves both directions.
    #[test]
    fn the_census_pipeline_can_still_see_the_keys_it_looks_for() {
        let owner = include_str!("../../../scope/mod.rs");
        let production = production_code_lines(owner);
        let by_identifier = code_text(&production);
        for key in FORBIDDEN {
            assert!(
                by_identifier.contains(key),
                "src/scope/mod.rs defines {key} in production code, but this census's \
                 own scanning pipeline cannot find it there. The negative assertion in \
                 `no_run_loop_file_reads_the_scope_metadata_keys` is therefore vacuous: \
                 it would pass on a module that had gone back to the raw reads."
            );
        }
        let by_value = code_keeping_literals(&production);
        for needle in forbidden_literals() {
            assert!(
                by_value.contains(&needle),
                "src/scope/mod.rs assigns {needle} in production code, but the \
                 literal-value half of this census cannot find it there. That half \
                 exists because `code_text` deletes literal payloads and a raw read \
                 spelled `get({needle})` was therefore invisible; if the value view \
                 stops carrying values, the hole is open again and silent."
            );
            assert!(
                !by_identifier.contains(&needle),
                "premise of the two-view split: {needle} must NOT survive \
                 `code_text` — if it did, the identifier view would already have \
                 covered the literal spelling and the review that found this hole \
                 could not have reproduced the defect."
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
        // Same shape, on the other end of the projection. `FlowScope`'s
        // private fields stop a pair of raw strings from reaching the field,
        // but they do not stop a SECOND mint from a differently-resolved
        // attribution, and `unscoped()` mints the empty pair outright — which
        // drops the room just as completely, and reads as deliberate.
        let mut mints = 0usize;
        let mut empties = 0usize;
        for (_, text) in production_files() {
            let code = code_text(&production_prefix(&text));
            mints += code.matches("FlowScope::resolved(").count();
            empties += code.matches("FlowScope::unscoped(").count();
        }
        assert_eq!(
            mints, 1,
            "`FlowScope::resolved` must be minted exactly once in run_loop — inside \
             `request_scope_strings`, from `request_scope`'s answer. A second mint is \
             a second answer to the same question that names no raw key and would \
             pass every other check here."
        );
        assert_eq!(
            empties, 0,
            "nothing in run_loop may hand the harness a deliberately empty \
             `FlowScope`. `request_scope` is already fail-closed — `resolved(None)` \
             is how an unstamped turn projects to the empty pair — so an explicit \
             `unscoped()` here can only be a scope being thrown away."
        );
    }

    /// The layer the other two checks lean on: `FlowScope` must stay
    /// unassemblable by hand.
    ///
    /// This is the one guard here that is about a TYPE rather than about text
    /// in this module, and it is the reason the census's own claim can now be
    /// narrower than the invariant without leaving a hole at the `FlowRequest`
    /// site. Make the two fields `pub` and every check above stays green while
    /// `owner_user_id: request.metadata.get(…)` compiles again — the exact
    /// build a review took end to end.
    ///
    /// A source-level check because there is nothing to observe at runtime: a
    /// public field and a private one behave identically once something has
    /// been built out of them. `#[derive(Default)]` is refused for the same
    /// reason `unscoped` is counted below — it would be a second, unnamed way
    /// to mint the empty pair.
    #[test]
    fn the_flow_request_scope_type_stays_unassemblable_by_hand() {
        let scope_mod = production_code_lines(include_str!("../../../scope/mod.rs"));
        let (before, after) = scope_mod.split_once("struct FlowScope {").expect(
            "`src/scope/mod.rs` must still declare `struct FlowScope` — it is what \
             makes the raw-read spelling a compile error at the `FlowRequest` site",
        );
        let body = after
            .split_once('}')
            .expect("`struct FlowScope` must have a closing brace")
            .0;
        assert!(
            body.contains("owner_user_id") && body.contains("scope_id"),
            "premise: this is the struct carrying the two strings, got:\n{body}"
        );
        assert!(
            !body.contains("pub"),
            "`FlowScope`'s fields must stay private. Public fields put the raw \
             `(Option<String>, Option<String>)` pair back within reach of the \
             `FlowRequest` construction site, and NOTHING else in this module \
             would notice — the census below only reads `run_loop`, and a raw \
             read written there names no key at all. Got:\n{body}"
        );
        let derives = before
            .rsplit_once("#[derive(")
            .and_then(|(_, tail)| tail.split_once(")]"))
            .map(|(list, _)| list)
            .unwrap_or_default();
        assert!(
            derives.contains("Clone"),
            "premise: the text just before the declaration is `FlowScope`'s own \
             derive list, and `dispatch` needs it `Clone`. Got: {derives:?}"
        );
        assert!(
            !derives.contains("Default"),
            "`FlowScope` must not derive `Default`: `Default::default()` cannot be \
             found by any text search, so it would be a mint of the empty pair that \
             `FlowScope::unscoped` is counted precisely to keep out of `run_loop`. \
             Got: {derives:?}"
        );
    }

    /// No production line in this module names either key — by CONSTANT or by
    /// VALUE.
    ///
    /// Two views of every file, because one view cannot answer both. Which
    /// half a given offender trips is printed with it, so the red says which
    /// spelling was used rather than leaving the reader to guess.
    #[test]
    fn no_run_loop_file_reads_the_scope_metadata_keys() {
        let mut scanned = 0usize;
        let mut offenders: Vec<String> = Vec::new();
        let literals = forbidden_literals();
        for (rel, text) in production_files() {
            scanned += 1;
            // `production_code_lines` is common to both views: it blanks the
            // `#[cfg(test)]` items and the comment-only lines while PRESERVING
            // line numbers, so an offender can be opened at the number this
            // prints.
            let production = production_code_lines(&text);
            // View 1 — identifiers. `code_text` then removes string-literal
            // payloads, so a guard message or a fixture quoting a key is not a
            // hit. That is also why this view cannot see view 2's needles.
            for (i, line) in code_text(&production).lines().enumerate() {
                for key in FORBIDDEN {
                    if line.contains(key) {
                        offenders.push(format!("{rel}:{}: [constant] {}", i + 1, line.trim()));
                    }
                }
            }
            // View 2 — the values. `code_keeping_literals` keeps payloads and
            // drops ALL comment text (including a comment trailing live code,
            // which `production_code_lines` alone leaves standing), so prose
            // cannot trip this and `get("scope_id")` cannot hide from it. The
            // needles carry their quotes, so only an exact payload matches.
            for (i, line) in code_keeping_literals(&production).lines().enumerate() {
                for needle in &literals {
                    if line.contains(needle.as_str()) {
                        offenders.push(format!("{rel}:{}: [literal] {}", i + 1, line.trim()));
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
             own `personal:<speaker>` stamp past this point. `[constant]` names \
             the key by its identifier, `[literal]` by its value; both are the \
             same read and the second one is the one that shipped. Offenders:\n{}",
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
