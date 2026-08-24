//! The membership rule for capability handles, and the guards that close it.
//!
//! # The rule (derived, never a hand-written list)
//!
//! A `static` of an install-once container (`OnceLock` / `OnceCell` /
//! `ArcSwap*`) is a **capability handle** iff something writes it
//! (`set` / `store` / `swap` / `get_or_try_init`). A container that is only
//! ever `get_or_init`-ed is a lazy cache: "not built yet" is the correct
//! answer there, so it cannot write an honest `MissingSemantics` and is
//! excluded by derivation, not by an exemption list.
//!
//! ⚠️ The type pattern MUST accept qualified paths (`std::sync::OnceLock`,
//! `once_cell::sync::OnceCell`, `arc_swap::ArcSwap`). A first pass that
//! matched only bare type names counted 29 boot handles where the true number
//! is 40 — and `spend::GLOBAL_LEDGER`, the anchor of the round-7 fix this
//! generalises, is written in the qualified form. A guard's green only covers
//! the shapes its recogniser knows.
//!
//! ⚠️ It must also accept a leading visibility modifier. `static ` alone misses
//! `pub static NAME: std::sync::OnceLock<…>`; measured 2026-08-24 that admits
//! exactly one further candidate in `src/`
//! (`extension/manifest/mod.rs::GLOBAL_MANIFEST_CACHE`), which the writer
//! predicate then excludes as a lazy cache — so accepting the form costs
//! nothing today and closes a blind spot that would otherwise open silently the
//! first time a handle is declared `pub`.
//!
//! # What this rule provably does NOT see
//!
//! The predicate is **syntactic**: it asks whether the *container* is written.
//! A handle whose container is lazily built but whose *contents* are installed
//! at boot through interior mutability —
//! `OnceLock<RwLock<Option<T>>>` reached by `get_or_init(|| RwLock::new(None))`
//! and then filled by a `store_*` that takes a write guard — has the round-7
//! failure semantics (an uninstalled read yields a legal-looking `None`) while
//! being invisible here. Two such handles are known and named in the Task 6
//! report: `providers/moa/config_handle.rs::MOA_CONFIG` and
//! `providers/route_handle.rs::GLOBAL`. They are excluded by this rule, and the
//! measured total confirms the specification's 46 excluded them too. Widening
//! the rule to cover interior-mutable installs is a separate, larger question
//! (it needs a "who writes through the guard" predicate, not a method-name
//! scan) and would move the number this round is pinned to — so it is reported,
//! not silently absorbed.

use crate::utils::source_scan::{production_prefix, rust_sources_under, strip_comment_lines};

/// One process-global container `static`, as the rule sees it.
pub(crate) struct HandleSite {
    pub file: String,
    pub name: String,
    pub container: String,
    pub is_slot: bool,
}

/// Install-once container types. Compared against the FINAL path segment, so a
/// qualified path resolves to the same member as a bare name.
const CONTAINERS: &[&str] = &["OnceLock", "OnceCell", "ArcSwapOption", "ArcSwapAny", "ArcSwap"];

/// The ways a caller writes an install-once container from outside it.
/// `get_or_init` is deliberately absent: it is the lazy-cache shape.
const WRITERS: &[&str] = &["set(", "store(", "swap(", "get_or_try_init("];

/// Strip an optional leading visibility modifier, returning the rest.
///
/// Handles `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`. Anything it does
/// not recognise is returned unchanged, so an unparsed line simply fails the
/// `static ` test below rather than being mangled into a false match.
fn strip_visibility(t: &str) -> &str {
    let Some(rest) = t.strip_prefix("pub") else {
        return t;
    };
    if rest.starts_with('(') {
        return match rest.split_once(')') {
            Some((_, tail)) => tail.trim_start(),
            None => t,
        };
    }
    if rest.starts_with(char::is_whitespace) {
        rest.trim_start()
    } else {
        t // `pub` was the prefix of some other identifier
    }
}

/// Parse `[vis] static NAME : <maybe::qualified::>Container <`.
fn parse_static_decl(line: &str) -> Option<(String, String)> {
    let t = strip_visibility(line.trim_start());
    let rest = t.strip_prefix("static ")?;
    let (name, after) = rest.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
    {
        return None;
    }
    // Drop any qualifying path segments before the type name.
    let ty = after.trim().split('<').next()?.trim();
    let last = ty.rsplit("::").next()?.trim();
    let container = CONTAINERS.iter().find(|c| **c == last)?;
    Some((name.to_string(), (*container).to_string()))
}

/// Every container `static` in `src/`, split by the rule into the handles it
/// selects and the lazy caches it excludes.
///
/// Returned as one pair from one walk on purpose: two functions that each
/// re-scanned would be two answers to "what did the rule see", and the
/// self-counting assertion below needs both halves of a single verdict.
fn partition_container_statics() -> (Vec<HandleSite>, Vec<HandleSite>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources_under(&root);
    assert!(
        sources.len() > 100,
        "the source walk found only {} files under src/ — the census scanned \
         nothing, which is not the same as finding nothing wrong",
        sources.len()
    );

    let mut selected = Vec::new();
    let mut lazy = Vec::new();
    for (rel, text) in sources {
        let prod = strip_comment_lines(&production_prefix(&text));
        for line in prod.lines() {
            let Some((name, container)) = parse_static_decl(line) else {
                continue;
            };
            let written = WRITERS.iter().any(|m| prod.contains(&format!("{name}.{m}")));
            let site =
                HandleSite { file: rel.clone(), name, container, is_slot: false };
            if written {
                selected.push(site);
            } else {
                lazy.push(site); // lazy cache — excluded by the rule, not by a list
            }
        }
        // Slots are declared with the newtype, not a raw container. Migration
        // moves handles from the first loop to this one; the SUM is what the
        // guard pins, so a half-finished migration cannot go quiet.
        for line in prod.lines() {
            let t = strip_visibility(line.trim_start());
            let Some(rest) = t.strip_prefix("static ") else {
                continue;
            };
            if !rest.contains("CapabilitySlot<") {
                continue; // also covers MutableCapabilitySlot<, which contains it
            }
            let Some((name, _)) = rest.split_once(':') else {
                continue;
            };
            selected.push(HandleSite {
                file: rel.clone(),
                name: name.trim().to_string(),
                container: "CapabilitySlot".into(),
                is_slot: true,
            });
        }
    }
    (selected, lazy)
}

/// The authoritative inventory Tasks 7–10 migrate and Task 11 closes.
///
/// Regenerable by construction: the rule is deterministic over `src/`, so a
/// lost copy of the printed inventory is one test run, not a blocked task.
pub(crate) fn capability_handles() -> Vec<HandleSite> {
    partition_container_statics().0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inventory this round migrates. Asserted, not printed: a census that
    /// silently shrinks and a census that stopped matching look identical.
    #[test]
    fn the_capability_handle_inventory_is_the_size_we_measured() {
        let (sites, lazy) = partition_container_statics();
        let raw = sites.iter().filter(|s| !s.is_slot).count();
        let slots = sites.iter().filter(|s| s.is_slot).count();

        // ONE write, not one per line. libtest prints its own progress lines to
        // the same stderr from another thread, and it spliced one of them into
        // the MIDDLE of a per-line `eprintln!` on the first run here -- which
        // silently dropped a handle from the grep-extracted inventory and cost
        // an hour chasing a census defect that did not exist. The inventory is a
        // cross-task interface; it must not be reassembled from interleaved
        // output.
        let mut report = format!("--- capability handles: {raw} raw, {slots} slots ---\n");
        for s in sites.iter().filter(|s| !s.is_slot) {
            report.push_str(&format!("  RAW  {:14} {:32} {}\n", s.container, s.name, s.file));
        }
        for s in sites.iter().filter(|s| s.is_slot) {
            report.push_str(&format!("  SLOT {:14} {:32} {}\n", s.container, s.name, s.file));
        }
        for s in &lazy {
            report.push_str(&format!("  LAZY {:14} {:32} {}\n", s.container, s.name, s.file));
        }
        report.push_str(&format!(
            "--- candidates {} = handles {} + lazy caches {} ---",
            sites.len() + lazy.len(),
            sites.len(),
            lazy.len()
        ));
        eprintln!("{report}");

        // Self-count: the rule must DISCRIMINATE. A predicate stuck at "always
        // written" would select everything and a predicate stuck at "never
        // written" would select nothing; the count below catches both, but only
        // this one names which way the recogniser broke.
        assert!(
            !lazy.is_empty(),
            "the rule excluded no lazy caches at all — the writer predicate is \
             answering the same thing for every static"
        );

        assert_eq!(
            raw + slots,
            46,
            "the rule selected {} handles, not 46.\n\
             ⚠️ DO NOT EDIT THIS NUMBER, AND DO NOT WIDEN THE RULE TO REACH IT. \
             This gap is INVESTIGATED AND EXPLAINED — see \
             .superpowers/sdd/2026-08-24-capability-wiring/task-6-report.md §2. \
             In short: 46 was NOT measured with this algorithm. This rule looks \
             for a writer in the static's OWN FILE and finds 45. 46 is \
             reproducible only with a corpus-wide word-boundary search, whose \
             46th member is providers/route_handle.rs::GLOBAL -- which has no \
             writer anywhere in its own file and is selected only because three \
             UNRELATED statics are also named GLOBAL and are `.set(` in theirs. \
             It is a real member (see the_forty_sixth_member_is_invisible_to_any_\
             setter_rule below) reached by a reason that a rename would silently \
             break. Pinning 45, or deriving 46 soundly, is the controller's call.",
            raw + slots
        );
    }

    /// Edge checks drawn from `task-6-boundary-cases.md`.
    ///
    /// ⚠️ This is NOT the membership definition — that is the rule above, and a
    /// guard that lists its own members goes blind the moment the set grows.
    /// This pins the two handles the whole round is ANCHORED on, because a count
    /// assertion alone cannot see them leave: a rule that dropped
    /// `spend::GLOBAL_*` while picking up two unrelated statics still counts 46.
    #[test]
    fn the_round_seven_anchors_are_selected_by_the_rule() {
        let sites = capability_handles();
        let found = |file: &str, name: &str| {
            sites.iter().any(|s| s.file.ends_with(file) && s.name == name)
        };
        assert!(
            found("src/spend/mod.rs", "GLOBAL_LEDGER"),
            "GLOBAL_LEDGER is the §5.22 round-7 anchor and is written in the \
             QUALIFIED form (`std::sync::OnceLock`); a recogniser that matches \
             only bare type names loses it while still reporting green"
        );
        assert!(
            found("src/spend/mod.rs", "GLOBAL_POLICY"),
            "GLOBAL_POLICY is the round-7 anchor and the sole member \
             MutableCapabilitySlot exists for"
        );
    }

    /// The self-initialising exclusions, verified rather than assumed.
    ///
    /// `WITNESSES` is the shape the rule's derivation names: `get_or_init` with
    /// a `default()`, so "not installed" is not a state it can hold. The two
    /// `*_CACHE` entries were listed as "EXCLUDE (verify)" — excluded on the
    /// absence of a setter, never on the word "cache" in the name.
    #[test]
    fn self_initialising_containers_are_excluded_by_derivation() {
        let (sites, lazy) = partition_container_statics();
        for (file, name) in [
            ("src/providers/route_witness.rs", "WITNESSES"),
            ("src/skill/mod.rs", "CACHED_MANIFEST"),
            ("src/thinker/runtime_context.rs", "REPO_ROOT_CACHE"),
        ] {
            assert!(
                !sites.iter().any(|s| s.file.ends_with(file) && s.name == name),
                "{name} has no setter and must not be selected as a handle"
            );
            assert!(
                lazy.iter().any(|s| s.file.ends_with(file) && s.name == name),
                "{name} was neither selected nor seen as a candidate — the \
                 recogniser stopped matching its declaration form, which looks \
                 exactly like a correct exclusion"
            );
        }
    }

    /// The finding of Task 6's investigation, pinned as a fact rather than left
    /// in prose. A ruling that lives only in a report cannot stop the next
    /// sincere fixer from "correcting" the count.
    ///
    /// `providers/route_handle.rs::GLOBAL` is boot-installed (`orchestrator_init`
    /// is its first caller) and has the `ConsumerDecides` shape, so it IS a
    /// capability handle. But it is installed by `get_or_init` **from a config
    /// argument** — first-caller-wins, no setter — so no predicate that looks for
    /// a writer can select it, and the corpus-wide search that appeared to do so
    /// was matching three unrelated statics that share its very short name.
    ///
    /// This test fails the day someone gives it a real setter: at that point the
    /// same-file rule selects it on its own merits, the count becomes 46 by
    /// derivation, and this test and the note in the count assertion should both
    /// be deleted.
    #[test]
    fn the_forty_sixth_member_is_invisible_to_any_setter_rule() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/providers/route_handle.rs"),
        )
        .expect("route_handle.rs");
        let prod = strip_comment_lines(&production_prefix(&src));
        assert!(
            prod.contains("static GLOBAL:"),
            "route_handle::GLOBAL is gone or renamed — re-run the Task 6 \
             investigation before trusting any count here"
        );
        for w in WRITERS {
            assert!(
                !prod.contains(&format!("GLOBAL.{w}")),
                "route_handle::GLOBAL now has a `{w}` writer in its own file. The \
                 same-file rule can select it on its own merits now: expect 46, \
                 and delete this test with the note in the count assertion."
            );
        }
        assert!(
            prod.contains("GLOBAL\n        .get_or_init(") || prod.contains("GLOBAL.get_or_init("),
            "route_handle::GLOBAL is no longer get-or-init'd — its install shape \
             changed and the Task 6 finding needs re-deriving"
        );
    }

    /// The other disagreement with `task-6-boundary-cases.md`, pinned the same
    /// way: `MOA_CONFIG` is `OnceLock<RwLock<Option<T>>>`, lazily built and then
    /// filled THROUGH the lock. It has the round-7 failure semantics and no
    /// container-setter rule can see it — the whole `OnceLock<Lock<Option<T>>>`
    /// class is below this rule's resolution.
    #[test]
    fn the_interior_mutable_install_class_is_below_this_rules_resolution() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/providers/moa/config_handle.rs"),
        )
        .expect("config_handle.rs");
        let prod = strip_comment_lines(&production_prefix(&src));
        assert!(prod.contains("static MOA_CONFIG:"), "MOA_CONFIG is gone or renamed");
        for w in WRITERS {
            assert!(
                !prod.contains(&format!("MOA_CONFIG.{w}")),
                "MOA_CONFIG now has a `{w}` writer: it is selectable by the rule, \
                 the count moves, and this note should be deleted."
            );
        }
    }

    /// Test-only statics are removed by the extractor, not by a name filter.
    ///
    /// Both of these sit inside a `#[cfg(test)]` **function** — the shape the
    /// deleted `split("#[cfg(test)]")` idiom got right only by cutting the whole
    /// tail of the file. They must be absent from BOTH halves of the verdict:
    /// absent from the handles is not evidence (the writer predicate would also
    /// have excluded them), absent from the candidates is.
    #[test]
    fn test_only_statics_are_not_candidates_at_all() {
        let (sites, lazy) = partition_container_statics();
        for (file, name) in [
            ("src/providers/moa/config_handle.rs", "LOCK"),
            ("src/session/store.rs", "TEST_STORE"),
        ] {
            let seen = sites.iter().chain(lazy.iter());
            assert!(
                !seen.into_iter().any(|s| s.file.ends_with(file) && s.name == name),
                "{name} lives in a #[cfg(test)] fn in {file} and must not reach \
                 the census at all — seeing it means production_prefix failed to \
                 remove the enclosing item"
            );
        }
    }
}
