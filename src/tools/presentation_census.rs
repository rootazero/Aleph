//! Two censuses that keep the presentation contract honest.
//!
//! 1. Every registered builtin whose `mutates_file_content()` is true must
//!    attach `_presentation` when run against a fixture — and every fixture
//!    must name a registered tool that answers true. A new mutating tool
//!    without a fixture is RED, a fixture for a tool that stopped mutating is
//!    RED. The mutating set is derived by scanning the trait impls that own
//!    `mutates_file_content`, not from a list of names (判据 §3/§5) — see
//!    "Why (1) is a source scan" below. The registry is consulted only to
//!    execute the fixtures and to cross-check the names the scan read.
//! 2. Every registered builtin renders a non-blank row label through the REAL
//!    `shared_ui_logic::transcript::display_name` — called, not re-derived
//!    here (判据 §1/§10: a scraper of that table's source is a second
//!    representation of it and can only ever prove a superset). Whether a
//!    given tool gets there via an explicit `DISPLAY_NAMES` entry or via the
//!    `humanize()` fallback is not this census's business — `humanize` is an
//!    honest default, not a lie — so there is no per-tool coverage list. What
//!    IS asserted: no `DISPLAY_NAMES` key is a ghost (nothing under `src/`
//!    declares a const with that value), the two spellings that direction
//!    caught in Task 4 still do not resolve, no key is duplicated, no entry is
//!    blank on either side, nothing in `registered ∪ DISPLAY_NAMES keys`
//!    renders a blank label (and `display_name` is still *capable* of
//!    rendering one, so that assertion can go red), and every content-mutating
//!    tool (the rows that carry a rich diff) has an explicit entry rather than
//!    a fallback.
//!
//!    The two directions of (2) are derived DIFFERENTLY on purpose — one from
//!    source, one from a registry instance — and must not be unified; see the
//!    comments on each inside
//!    [`every_registered_tool_resolves_to_a_display_label`].
//!
//! # Why (1) is a source scan, not a registry accessor
//!
//! The obvious shape for "ask the registry which tools answer
//! `mutates_file_content() == true`" is a name → `&dyn AlephToolDyn` lookup.
//! That does not exist: `UnifiedTool` (`tool_metadata::types::unified`) is
//! plain metadata — name, schema, description — with no handle back to the
//! concrete tool object, and `BuiltinToolRegistry` holds one differently-typed
//! field per builtin, dispatched by a ~190-arm hand-written `match` in
//! `tool_registry_impl.rs`. Growing a second name → `&dyn AlephToolDyn` map to
//! match it would be a second copy of that dispatch table, and the two would
//! drift — silently, in the direction that matters: a tool present in
//! `execute_tool` but missing from the mirror is invisible to the census, so
//! it reports green about a tool it never looked at. A guard whose blind
//! spots grow by omission is the exact failure this task exists to prevent.
//!
//! So this scans source instead, the same discipline
//! `executor::builtin_registry::dispatchable` and `gateway::events::frame_census`
//! already use: `mutates_file_content()`'s only overrides live in the trait
//! impls that define it, so reading THOSE is asking the fact's owner directly.
//! A source scan has its own characteristic failure — finding nothing and
//! passing — so the self-checks below guard against exactly that (the
//! numbered list on the test is the authority on how many there are):
//! [`census_finds_the_known_overrides_and_confines_them_to_builtin_tools`]
//! (1) asserts the scan's own yield is non-trivial, (2) every occurrence the
//! scan finds is classified as an `impl AlephTool(Dyn) for <Concrete>`
//! override, a `trait AlephTool(Dyn)` declaration, or the blanket
//! `impl<T: AlephTool> AlephToolDyn for T` forward — anything else panics
//! rather than being silently skipped — and (3) the scan is re-run over the
//! whole crate and asserted to find the identical set of concrete overrides,
//! proving none hides outside `src/builtin_tools/`.
//!
//! That is the fourth self-check: this file's OWN source — and only this one,
//! [`SELF_PATH`] — is excluded from the scan, because its prose and panic
//! messages quote the scanner's own needle text and marker strings, which
//! would otherwise be classified as if they were code. The exclusion is
//! asserted from both sides rather than left as a silent `continue`: the scan
//! counts what it skipped and the test requires that count to be exactly one
//! (a path that matches nothing and one that matches too much are both
//! invisible otherwise), and the excluded file's own raw occurrence count is
//! pinned to [`SELF_OCCURRENCE_COUNT`] so a real override added here later
//! cannot hide behind the exemption.
//!
//! Both scanners also take nothing from files their PARENT declares
//! `#[cfg(test)] mod <stem>;` — those carry no `#[cfg(test)]` of their own, so
//! a text-only partition hands back the whole file as production and the
//! census scans test doubles as though they shipped. That skip is counted and
//! asserted `> 0` (not pinned to a number, which would be bumped on reflex);
//! see [`production_of`].

use std::path::Path;

use serde_json::{json, Value};

use crate::executor::ToolRegistry as _;

/// (tool name, args builder run inside a tempdir) — one per content-mutating tool.
const PRESENTATION_FIXTURES: &[(&str, fn(&Path) -> Value)] = &[
    ("file_edit", |dir| {
        let p = dir.join("e.txt");
        std::fs::write(&p, "alpha\nbeta\n").unwrap();
        json!({"file_path": p.to_string_lossy(), "old_string": "beta", "new_string": "BETA"})
    }),
    (
        "file_write",
        |dir| json!({"file_path": dir.join("w.txt").to_string_lossy(), "content": "new\n"}),
    ),
    ("apply_patch", |dir| {
        let p = dir.join("p.txt");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        // Relative, on purpose: `ApplyPatchTool::resolve_via` rejects an
        // absolute path outright ("apply_patch requires relative paths") and
        // resolves a relative one against the active `FsScope` instead — the
        // execution loop below runs every fixture inside a scope rooted at
        // `dir`, so "p.txt" here means exactly the file just written above.
        json!({"patch": "*** Begin Patch\n*** Update File: p.txt\n@@\n-two\n+TWO\n*** End Patch"})
    }),
];

/// This module's own path, as [`crate::utils::source_scan::rust_sources_under`]
/// reports it (repo-relative, forward-slashed) — excluded from
/// [`scan_mutates_file_content`]. See [`SELF_OCCURRENCE_COUNT`] for why the
/// exclusion is asserted rather than silent.
const SELF_PATH: &str = "src/tools/presentation_census.rs";

/// How many raw occurrences of the literal text `fn mutates_file_content`
/// this file's own production text (comments/docs stripped) contains today.
/// All of them are the scanner's own needle string or a panic message
/// quoting the method name — none is a real trait impl.
///
/// Pinned and asserted (`census_finds_the_known_overrides_and_confines_them_to_builtin_tools`)
/// rather than the exclusion being a bare `if rel_path == SELF_PATH { continue }`
/// with no check on it: if this file later grows a real
/// `impl AlephTool for X { fn mutates_file_content .. }` (unlikely, but the
/// point of an asserted count is not to trust "unlikely"), the occurrence
/// count changes and this assertion goes red, forcing a human to look before
/// the exclusion is silently re-approved.
const SELF_OCCURRENCE_COUNT: usize = 7;

async fn registry() -> crate::executor::BuiltinToolRegistry {
    crate::executor::BuiltinToolRegistry::with_config(crate::executor::BuiltinToolConfig {
        injection_mode: crate::config::types::memory::MemoryInjectionMode::Hybrid,
        ..Default::default()
    })
    .await
    .unwrap()
}

// =============================================================================
// `mutates_file_content` source scan
// =============================================================================

/// The production text of one walked file, and whether the walk took nothing
/// from it because its PARENT declares it `#[cfg(test)] mod <stem>;`.
///
/// Shared by both scanners so they cannot disagree about what counts as
/// production. `production_text` is the path-aware helper
/// [`crate::utils::source_scan`] documents as the one a directory-walking
/// census should call: `production_prefix` partitions on an `#[cfg(test)]`
/// *inside* the file, so a whole-file test module — which carries none, its
/// parent applies one — came back as 100% production and was scanned as if it
/// shipped. `src/tools/server/tests.rs` already holds an
/// `impl AlephToolDyn for DynamicMockTool`, one method away from making this
/// census panic while pointing at a test double.
///
/// The skip flag asks the owning predicate, `declared_as_a_test_module` — the
/// same one `production_text` consults — rather than inferring the answer from
/// the returned text. An earlier version of this function inferred it: empty
/// text, non-blank file, and no `#[cfg(test)]` marker anywhere in the source.
/// That reasoning was sound and the flag it produced had no false positives,
/// but it was a SECOND reading of the marker that decides where test code
/// begins, free to drift from the first — and `source_scan`'s own
/// `no_module_hand_rolls_the_cfg_test_prefix_cut` caught it as exactly that.
/// It was also strictly worse: it missed a file that is parent-declared AND
/// carries a `#[cfg(test)]` of its own (`src/gateway/runtime/tests.rs` is one),
/// so the count it produced was silently low. Asking the owner is smaller,
/// exact, and leaves one author for the question.
///
/// The two calls read the same file's parent twice, which is the price of not
/// re-deriving `production_text`'s own branch here. It is a test-only walk;
/// correctness of the count outranks one small extra read per file.
///
/// The guard this census relies on for `production_text` staying path-aware is
/// `utils::source_scan::tests::test_text_sees_a_whole_file_test_module_that_cfg_test_portion_cannot`
/// — named here because it lives in the other file, so someone refactoring
/// `source_scan` who greps for its dependants finds this call site instead of
/// reading it as a test with no beneficiary.
fn production_of(rel_path: &str, raw: &str) -> (String, bool) {
    let path = Path::new(rel_path);
    let parent_declared = crate::utils::source_scan::declared_as_a_test_module(path);
    (
        crate::utils::source_scan::production_text(path, raw),
        parent_declared,
    )
}

/// One `fn mutates_file_content` occurrence, classified by the block it sits in.
#[derive(Debug)]
enum Occurrence {
    /// `impl AlephTool for X` or `impl AlephToolDyn for X` (`X` a concrete,
    /// non-generic type) — an actual tool's own answer.
    ConcreteOverride {
        tool_name: String,
        returns_true: bool,
    },
    /// `impl<T: AlephTool> AlephToolDyn for T` (or any other generic/blanket
    /// impl) — forwards to something else, names no tool of its own.
    Blanket,
    /// `trait AlephTool { .. }` / `trait AlephToolDyn { .. }` — the method's
    /// own declaration/default body, not an implementation.
    TraitDeclaration,
}

/// One [`scan_mutates_file_content`] walk: what it classified, and how many
/// files it refused to look at.
struct Scan {
    occurrences: Vec<Occurrence>,
    /// How many files the walk skipped because their path is exactly
    /// [`SELF_PATH`]. Carried out of the scan rather than recomputed by the
    /// caller: the number asserted must be the one the scan actually acted on,
    /// not a second derivation of it that is free to agree while the walk
    /// skipped something else — or nothing at all, if `SELF_PATH` is a typo
    /// (判据 §1, §3).
    excluded: usize,
    /// How many files the walk took nothing from because their parent declares
    /// them a test module (see [`production_of`]). Asserted `> 0` rather than
    /// pinned to a number: the failure that matters is the mechanism silently
    /// ceasing to match, and an exact count would go red on every new test
    /// module anywhere in the crate until someone bumped it on reflex — the
    /// shape [`SELF_OCCURRENCE_COUNT`] can afford only because it is pinned to
    /// one named file.
    test_modules: usize,
}

/// Every `fn mutates_file_content` occurrence under `root`, classified —
/// except this module's own source ([`SELF_PATH`]), which the census excludes
/// (see the module doc and [`SELF_OCCURRENCE_COUNT`]). The exclusion is
/// counted into [`Scan::excluded`] so a caller can assert it fired exactly
/// once; a `continue` nothing counts is indistinguishable from a path typo
/// that matches nothing.
///
/// Panics — failing whichever test called it — on any occurrence it cannot
/// place in one of the three shapes above, or whose body (for a concrete
/// override) is not a bare `true`/`false` literal. A guard that silently
/// skipped what it could not parse would be exactly the blind spot this
/// module's doc comment describes; see
/// [`census_finds_the_known_overrides_and_confines_them_to_builtin_tools`] for
/// the assertions that turn "the scan found nothing odd" into "the scan
/// looked at something and can prove it."
fn scan_mutates_file_content(root: &Path) -> Scan {
    let mut out = Scan {
        occurrences: Vec::new(),
        excluded: 0,
        test_modules: 0,
    };
    for (rel_path, raw) in crate::utils::source_scan::rust_sources_under(root) {
        if rel_path == SELF_PATH {
            out.excluded += 1;
            continue;
        }
        let (production, parent_declared) = production_of(&rel_path, &raw);
        if parent_declared {
            out.test_modules += 1;
        }
        let text = crate::utils::source_scan::strip_comment_lines(&production);
        for (idx, _) in text.match_indices("fn mutates_file_content") {
            out.occurrences
                .push(classify_occurrence(&rel_path, &text, idx));
        }
    }
    out
}

/// Raw occurrence count of the scan needle in `rel_path`'s own text, bypassing
/// classification entirely. Used only to keep [`SELF_OCCURRENCE_COUNT`] honest
/// — never by [`scan_mutates_file_content`], which excludes the file outright.
///
/// # Why this one keeps `production_prefix` while both scanners moved to `production_text`
///
/// Do not "unify" the three call sites. This module is itself declared
/// `#[cfg(test)] mod presentation_census;` by `src/tools/mod.rs`, so it IS a
/// parent-declared test module: `production_text` answers the empty string for
/// it, every time, forever. Ask it here and the needle count is 0 by
/// construction, [`SELF_OCCURRENCE_COUNT`] has to become 0 to match, and the
/// fourth self-check turns into a predicate that cannot go red (判据 §2) —
/// exactly the guard it exists to be. The scanners want "what ships"; this
/// wants "what is written in this one file", and those are different
/// questions that happen to share a helper.
fn raw_occurrence_count(root: &Path, rel_path: &str) -> Option<usize> {
    crate::utils::source_scan::rust_sources_under(root)
        .into_iter()
        .find(|(rel, _)| rel == rel_path)
        .map(|(_, raw)| {
            let text = crate::utils::source_scan::strip_comment_lines(
                &crate::utils::source_scan::production_prefix(&raw),
            );
            text.match_indices("fn mutates_file_content").count()
        })
}

/// Byte offset of the last occurrence of `needle` in `head`, if any.
///
/// `head` is the text BEFORE the occurrence being classified, so "last in
/// `head`" is "nearest preceding candidate".
fn nearest_preceding(head: &str, needle: &str) -> Option<usize> {
    head.rfind(needle)
}

/// Classify the `fn mutates_file_content` occurrence starting at byte `idx`
/// in `text`, by finding the nearest enclosing block header among the three
/// known shapes and verifying `idx` is genuinely inside that header's own
/// brace pair (not just textually nearby — a header whose block already
/// closed before `idx` is not the enclosing one, and is skipped in favour of
/// the next-closest candidate).
fn classify_occurrence(rel_path: &str, text: &str, idx: usize) -> Occurrence {
    let head = &text[..idx];
    let mut candidates: Vec<(usize, Marker)> = Vec::new();
    if let Some(p) = nearest_preceding(head, "impl AlephTool for ") {
        candidates.push((p, Marker::ConcreteImpl));
    }
    if let Some(p) = nearest_preceding(head, "impl AlephToolDyn for ") {
        candidates.push((p, Marker::ConcreteImpl));
    }
    if let Some(p) = nearest_preceding(head, "impl<") {
        candidates.push((p, Marker::Blanket));
    }
    if let Some(p) = nearest_preceding(head, "trait AlephTool") {
        candidates.push((p, Marker::TraitDecl));
    }
    // Ascending by offset — the `pop()` below is what takes the closest
    // (largest offset) candidate first. Do not "fix" this sort to descending
    // to match the walk order: `pop()` would then hand back the FURTHEST
    // header first and the classifier would invert.
    candidates.sort_by_key(|(p, _)| *p);

    while let Some((start, marker)) = candidates.pop() {
        let Some(rel_open) = text[start..].find('{') else {
            continue;
        };
        let open = start + rel_open;
        if open >= idx {
            continue; // this header's own body starts after our fn — can't enclose it
        }
        let Some(close) = matching_brace(text, open) else {
            continue;
        };
        if idx >= close {
            continue; // this block already closed before our fn — not the enclosing one
        }
        return match marker {
            Marker::TraitDecl => Occurrence::TraitDeclaration,
            Marker::Blanket => Occurrence::Blanket,
            Marker::ConcreteImpl => {
                let returns_true = classify_body(rel_path, text, idx);
                let block = &text[start..close];
                let tool_name = resolve_name(block).unwrap_or_else(|| {
                    panic!(
                        "{rel_path}: the impl overriding `mutates_file_content` at byte \
                         {idx} has no resolvable `NAME` const in its own block — the \
                         census cannot name the tool this override belongs to"
                    )
                });
                Occurrence::ConcreteOverride {
                    tool_name,
                    returns_true,
                }
            }
        };
    }
    panic!(
        "{rel_path}: `fn mutates_file_content` at byte {idx} is not inside any \
         `impl AlephTool for _`, `impl AlephToolDyn for _`, generic `impl<..> for _`, or \
         `trait AlephTool{{Dyn}}` block this census recognizes — teach it this shape \
         before trusting it again, don't skip the occurrence"
    );
}

#[derive(Debug, Clone, Copy)]
enum Marker {
    ConcreteImpl,
    Blanket,
    TraitDecl,
}

/// Extract and classify the body of the `fn mutates_file_content` starting at
/// `fn_idx`. Only a bare `true` or `false` is trusted; anything else — a
/// call, a field read, a `match` — panics instead of guessing.
fn classify_body(rel_path: &str, text: &str, fn_idx: usize) -> bool {
    let Some(rel_open) = text[fn_idx..].find('{') else {
        panic!("{rel_path}: `fn mutates_file_content` at byte {fn_idx} has no body brace");
    };
    let open = fn_idx + rel_open;
    let close = matching_brace(text, open).unwrap_or_else(|| {
        panic!("{rel_path}: `fn mutates_file_content` at byte {fn_idx} has an unterminated body")
    });
    match text[open + 1..close].trim() {
        "true" => true,
        "false" => false,
        other => panic!(
            "{rel_path}: `fn mutates_file_content` at byte {fn_idx} returns a non-literal \
             body (`{other}`) — this census only trusts a bare `true`/`false`; give it one \
             or teach the scanner to parse this shape"
        ),
    }
}

/// Byte index of the `}` matching the `{` at byte index `open` in `text`.
///
/// Naive — no string/comment awareness beyond the comment-stripping already
/// done by the caller — which is safe here only because every block this
/// module inspects is small and its brace count was checked by hand against
/// the source it was written against. A desync would make the extracted body
/// text almost certainly fail to trim to a bare `true`/`false`, so corruption
/// fails loudly via [`classify_body`] rather than misreporting.
fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for (i, &b) in bytes[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// First `NAME: &'static str = "…"` payload inside `block`.
fn resolve_name(block: &str) -> Option<String> {
    const NEEDLE: &str = "NAME: &'static str = \"";
    let idx = block.find(NEEDLE)?;
    let after = &block[idx + NEEDLE.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// The self-assertions Option B is approved only together with: a source
/// scan's characteristic failure is finding nothing and passing, so each of
/// these must be a test that goes red on a concrete input, not a comment
/// asserting good faith.
///
/// 1. **Vacuity.** The scan must find at least the three known overrides
///    (`apply_patch`, `file_edit`, `file_write`) under `src/builtin_tools/`.
///    Goes red if: someone deletes or renames one of the three overrides, or
///    the scanner's marker text (`"impl AlephTool for "`, etc.) stops
///    matching after a refactor.
/// 2. **Full classification.** Enforced inside [`scan_mutates_file_content`]
///    itself (every occurrence is placed in one of three shapes or the scan
///    panics) rather than as a separate assertion here — there is no
///    "unclassified, but let it slide" bucket. Goes red if: someone reformats
///    an override's body to anything but a bare `true`/`false` (a `match`, a
///    field read, a call), or adds a `fn mutates_file_content` this census's
///    three shapes don't recognize.
/// 3. **Scope confinement.** The identical scan is re-run over the whole
///    crate (`src/`) and must find the exact same set of `ConcreteOverride`
///    occurrences as the `src/builtin_tools/`-scoped run — proving nothing
///    hides outside the root this module's fixtures cover. Goes red if:
///    a concrete `impl AlephTool for X { fn mutates_file_content .. }`
///    appears anywhere under `src/` outside `src/builtin_tools/` (a real
///    override the fixture-execution test below would never see because it
///    only trusts the `src/builtin_tools/`-scoped scan). The trait-declaration
///    and blanket-impl counts are pinned too, so growing either of those
///    non-override buckets — say, a second blanket impl — is equally visible
///    rather than silently absorbed as "not a concrete override, ignore".
/// 4. **The self-exclusion is asserted, not silent.** Two independent halves,
///    because an exclusion can fail in two directions.
///    *Did it fire, and only once?* The whole-crate scan must report
///    `excluded == 1` and the `src/builtin_tools/`-scoped scan `excluded == 0`
///    — an exclusion nothing counts is indistinguishable from a `SELF_PATH`
///    typo that silently matches nothing (which would put the panic back), or
///    from one that grew a second arm nobody noticed. Goes red if: this file
///    is renamed or moved without updating `SELF_PATH`, or the walk's path
///    spelling changes.
///    *Is the file still safe to exclude?* This test independently counts that
///    file's own raw occurrences (bypassing classification) and requires the
///    count to match [`SELF_OCCURRENCE_COUNT`] exactly. Goes red if: this
///    file's own text gains or loses an occurrence of the needle (e.g. a new
///    panic message quoting it) without updating the constant — so a FUTURE
///    real trait impl accidentally added to this file cannot hide behind an
///    unexamined exemption.
#[test]
fn census_finds_the_known_overrides_and_confines_them_to_builtin_tools() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let builtin_root = manifest.join("src").join("builtin_tools");
    let whole_root = manifest.join("src");

    let self_count = raw_occurrence_count(&whole_root, SELF_PATH).unwrap_or_else(|| {
        panic!(
            "{SELF_PATH} was not found by rust_sources_under — SELF_PATH is stale or the \
             walk is broken"
        )
    });
    assert_eq!(
        self_count, SELF_OCCURRENCE_COUNT,
        "presentation_census.rs's own occurrence count of the `fn mutates_file_content` \
         needle changed (now {self_count}, expected {SELF_OCCURRENCE_COUNT}) — update \
         SELF_OCCURRENCE_COUNT after confirming none of the new occurrences is a real \
         trait impl; the scan silently excludes this file by path, so a real override \
         added here would hide behind that exemption unless this count is kept honest"
    );

    let root_scan = scan_mutates_file_content(&builtin_root);
    let whole_scan = scan_mutates_file_content(&whole_root);

    let whole_excluded = whole_scan.excluded;
    let root_excluded = root_scan.excluded;
    assert_eq!(
        whole_excluded, 1,
        "the whole-crate scan excluded {whole_excluded} file(s), expected exactly 1 — \
         SELF_PATH (`{SELF_PATH}`) must name this file exactly as the walk spells it \
         (repo-relative, forward slashes). Renaming or moving this file, or letting a \
         second path match it, has to update SELF_PATH: an exclusion that matches nothing \
         puts the unrecognised-shape panic back, and one that matches more than intended \
         blinds the census to a real override"
    );
    // Narrow by construction: every rel_path in this walk begins
    // `src/builtin_tools/`, and SELF_PATH does not, so only moving this module
    // under that root can turn it red. Kept as the statement of that fact, not
    // as a live "and nothing else" check — `whole_excluded == 1` above is the
    // one that can catch a SELF_PATH typo.
    assert_eq!(
        root_excluded, 0,
        "the src/builtin_tools/-scoped scan excluded {root_excluded} file(s), expected 0 \
         — this module does not live under that root, so SELF_PATH now matches a file \
         inside it: either this module moved, or SELF_PATH was edited to something that \
         matches one"
    );

    // The path-aware skip fired. `> 0` rather than a pinned count on purpose:
    // what must never silently change is that whole-file test modules are
    // recognised AT ALL, and a pinned number would go red on every new test
    // module in the crate until someone bumped it without looking.
    let whole_test_modules = whole_scan.test_modules;
    assert!(
        whole_test_modules > 0,
        "the whole-crate scan took nothing from 0 parent-declared test modules — \
         source_scan::production_text has stopped recognising `#[cfg(test)] mod <stem>;` \
         files, so this census is once again scanning test doubles as production (an \
         `impl AlephToolDyn` in a test module can make it panic while pointing at a mock)"
    );
    // Asserted on the ROOT scan too, not just the whole-crate one. The
    // whole-crate assertion is a superset and would catch a general
    // regression, but it would stay green while path-awareness broke for
    // `src/builtin_tools/` alone — which is precisely the tree whose fixtures
    // this census executes. `Scan::excluded` is checked on both scans; a
    // number computed on one and asserted only on the other is the
    // computed-but-unchecked shape this module exists to reject (判据 §2).
    // Measured true at this commit: src/builtin_tools/ declares at least six
    // parent-declared test modules (desktop/mod.rs:27, note_manage/mod.rs:38,
    // pdf_generate/mod.rs:24, skill_reader/mod.rs:17, terminal.rs:784,
    // agent_manage/mod.rs:32), so this cannot be a false red.
    assert!(
        root_scan.test_modules > 0,
        "the src/builtin_tools/ scan took nothing from 0 parent-declared test modules, \
         while the whole-crate scan found {whole_test_modules} — path-awareness has \
         broken for exactly the tree this census executes fixtures from"
    );

    let root_true: Vec<&str> = root_scan
        .occurrences
        .iter()
        .filter_map(|o| match o {
            Occurrence::ConcreteOverride {
                tool_name,
                returns_true: true,
            } => Some(tool_name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        root_true.len() >= 3,
        "vacuity: expected at least apply_patch/file_edit/file_write to answer true \
         under src/builtin_tools/, found {root_true:?}"
    );

    let root_concrete = root_scan
        .occurrences
        .iter()
        .filter(|o| matches!(o, Occurrence::ConcreteOverride { .. }))
        .count();
    let whole_concrete = whole_scan
        .occurrences
        .iter()
        .filter(|o| matches!(o, Occurrence::ConcreteOverride { .. }))
        .count();
    assert_eq!(
        whole_concrete, root_concrete,
        "a concrete `mutates_file_content` override exists somewhere under src/ outside \
         src/builtin_tools/ (whole-crate scan found {whole_concrete}, \
         src/builtin_tools/-scoped scan found {root_concrete}) — widen the root this \
         module scans, or the fixture-execution test below will never see it"
    );

    let trait_decls = whole_scan
        .occurrences
        .iter()
        .filter(|o| matches!(o, Occurrence::TraitDeclaration))
        .count();
    assert_eq!(
        trait_decls, 2,
        "expected exactly the two trait declarations \
         (AlephTool::mutates_file_content, AlephToolDyn::mutates_file_content) — the \
         trait shape changed, update this census's classifier"
    );
    let blankets = whole_scan
        .occurrences
        .iter()
        .filter(|o| matches!(o, Occurrence::Blanket))
        .count();
    assert_eq!(
        blankets, 1,
        "expected exactly the one blanket `impl<T: AlephTool> AlephToolDyn for T` \
         forward — a second blanket-shaped impl appeared, update this census's classifier"
    );
}

#[tokio::test]
async fn every_content_mutating_tool_attaches_a_presentation_and_every_fixture_names_one() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let reg = registry().await;

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scanned = scan_mutates_file_content(&manifest.join("src").join("builtin_tools"));
    let mutating: Vec<String> = scanned
        .occurrences
        .into_iter()
        .filter_map(|o| match o {
            Occurrence::ConcreteOverride {
                tool_name,
                returns_true: true,
            } => Some(tool_name),
            _ => None,
        })
        .collect();
    assert!(
        mutating.len() >= 3,
        "vacuity: expected file_edit/file_write/apply_patch to answer true, got {mutating:?}"
    );

    // The scan reads each tool's own `NAME` const; cross-check it against the
    // registry's dispatch key so a `NAME` typo or drift is caught here rather
    // than the fixture below simply failing to find the tool.
    //
    // Yes, this is registry-derived, and yes, that is the shape that misfired
    // in census 2's ghosts direction. It is safe HERE for a reason that does
    // not transfer: this census does not merely ask whether a name exists, it
    // EXECUTES the tool a few lines below through `reg.execute_tool`. Census 1
    // is registry-coupled by necessity — there is no source-derived way to run
    // a tool — so this assertion adds no exposure the fixture loop does not
    // already have; it only converts a confusing downstream failure ("fixture
    // failed: unknown tool") into a clear upstream one. The ghosts direction
    // had a choice and took the source-derived side because it only needed to
    // know a name was declared.
    //
    // What this DOES mean: if a future content-mutating tool is config-gated
    // the way `memory_search` and `note_manage` are, this goes red about a
    // fixture setting. The fix then is to widen `registry()`'s config so the
    // tool is present to be executed — not to weaken this assertion, which
    // would leave the fixture loop failing anyway with a worse message.
    let registered: std::collections::HashSet<String> =
        reg.unified_tools().map(|t| t.name.clone()).collect();
    let unregistered: Vec<&String> = mutating
        .iter()
        .filter(|n| !registered.contains(*n))
        .collect();
    assert!(
        unregistered.is_empty(),
        "tool(s) whose own NAME const claims mutates_file_content=true are not registered \
         under that name: {unregistered:?}"
    );

    let fixture_names: Vec<&str> = PRESENTATION_FIXTURES.iter().map(|(n, _)| *n).collect();
    let missing: Vec<&String> = mutating
        .iter()
        .filter(|m| !fixture_names.contains(&m.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "content-mutating tools with no presentation fixture (add one here): {missing:?}"
    );
    let stale: Vec<&&str> = fixture_names
        .iter()
        .filter(|f| !mutating.contains(&(**f).to_string()))
        .collect();
    assert!(
        stale.is_empty(),
        "fixtures naming tools that no longer mutate content: {stale:?}"
    );

    for (name, build) in PRESENTATION_FIXTURES {
        let dir = tempfile::tempdir().unwrap();
        let args = build(dir.path());
        // `apply_patch` requires a relative path, resolved against the active
        // `FsScope` (see the fixture's own comment); `file_edit`/`file_write`
        // pass absolute paths, which resolve unaffected by any scope. Scoping
        // every fixture uniformly is simpler than special-casing one.
        //
        // `FsScope` and not `std::env::set_current_dir`: the scope is a
        // `tokio::task_local!` (`src/tools/fs_scope.rs:78`), so it is visible
        // only to this future. The CWD is per-PROCESS, and `cargo test` runs
        // these tests in threads of one process — changing it here would
        // reach into every other test running concurrently. Never swap this
        // for a CWD change.
        let scope = crate::tools::fs_scope::FsScope::workspace(dir.path().to_path_buf());
        let out = crate::tools::fs_scope::with_fs_scope(Some(scope), reg.execute_tool(name, args))
            .await
            .unwrap_or_else(|e| panic!("{name} fixture failed: {e}"));
        let p = out
            .get(aleph_protocol::PRESENTATION_KEY)
            .unwrap_or_else(|| panic!("{name} output carries no `_presentation`: {out}"));
        let parsed: aleph_protocol::Presentation = serde_json::from_value(p.clone())
            .unwrap_or_else(|e| panic!("{name} `_presentation` is not a Presentation: {e}"));
        let aleph_protocol::Presentation::FileChanges { changes } = parsed;
        assert!(!changes.is_empty(), "{name} attached an empty change list");
    }
}

// =============================================================================
// Row-label census (every registered tool renders a non-blank label)
// =============================================================================

/// Spellings that must NOT resolve to any tool-name const under `src/`.
///
/// These two are the exact dead keys the ghosts direction caught in Task 4:
/// the shell tool registers as `bash`, the web search tool as `search`, so a
/// `DISPLAY_NAMES` entry under either of these names would have rendered on no
/// row at all. They are kept as live negative probes because the derivation
/// underneath the ghosts check was rewritten (registry instance → source
/// scan), and a rewritten guard that has never been falsified is not yet a
/// guard (判据 §3). If either ever starts resolving — a legacy alias const
/// appearing somewhere under `src/` — that is a finding to investigate, not a
/// probe to delete.
///
/// Two independent reasons this declaration cannot satisfy its own probe.
/// It is array-valued, so [`parse_const_string`] rejects it (the text after
/// `=` is `&[`, not a string literal); and this whole file is a
/// parent-declared test module (`#[cfg(test)] mod presentation_census;` in
/// `src/tools/mod.rs`), so [`production_of`] hands the scan nothing from it in
/// the first place. The second reason arrived with the path-aware scan and
/// supersedes the first: even a plain `const X: &str = "shell_exec";` written
/// here would now go unseen, so do not read this probe as guarding against a
/// spelling revived *inside this file* — it guards against one revived in
/// production code, which is the only place it could mislead a reader of
/// `DISPLAY_NAMES`.
const DEAD_SPELLINGS: &[&str] = &["shell_exec", "web_search"];

/// Inputs that `display_name` must render BLANK — the falsification probe for
/// the "no tool renders a blank label" assertion below.
///
/// That assertion says a set is empty. Empty of what? If `display_name` could
/// never return a blank for ANY input, it would be a predicate incapable of
/// going red, sitting there looking like a guard forever (判据 §2 — the
/// "恒绿" face). These two make the capability itself falsifiable:
///
/// - `"__"` — every char is a separator, so `humanize` emits `"  "`, which
///   `split_whitespace` drops to nothing. It is also the MCP-shaped-with-empty-
///   halves case: `mcp_display` splits it into `("", "")`, rejects both halves
///   and returns `None`, so it falls through to `humanize` — which is the
///   second failure mode the assertion's own comment claims to cover.
/// - `"---"` — the same, down the hyphen branch rather than the underscore
///   one, so a change to only one of the two separators still leaves a probe.
///
/// These are NOT claims that any real tool is named this. If `display_name`
/// ever stops being able to return blank, this goes red and the right response
/// is to give the assertion below a different predicate — not to delete the
/// probe.
const BLANK_LABEL_PROBES: &[&str] = &["__", "---"];

/// One `const <IDENT>: <ty> = "<literal>";` declaration found under `src/`.
///
/// All three fields are read by the assertions below — the identifier and the
/// file are what let a failing probe name the declaration it found, instead of
/// reporting only that one exists.
#[derive(Debug)]
struct ConstString {
    /// The const's own identifier — `NAME` for a tool's associated const,
    /// `SUBAGENT_TOOL_NAME` for a free one.
    ident: String,
    /// Its string-literal value.
    value: String,
    /// Repo-relative file it was declared in, so a failing probe can name the
    /// declaration instead of merely reporting that one exists.
    file: String,
}

/// One [`scan_const_strings`] walk.
struct ConstScan {
    consts: Vec<ConstString>,
    /// Files the walk took nothing from because their parent declares them a
    /// test module. Same meaning, and the same `> 0` treatment, as
    /// [`Scan::test_modules`].
    test_modules: usize,
}

/// Every `const <IDENT>: <ty> = "<literal>";` under `root`, through the same
/// pipeline the `mutates_file_content` scan uses (`rust_sources_under` +
/// [`production_of`] + `strip_comment_lines`) — so a name that appears only in
/// a doc comment, only inside a `#[cfg(test)]` item, or only in a file whose
/// parent declares it `#[cfg(test)] mod <stem>;` does not count as a
/// declaration. That third case is not free: it needs the path-aware
/// `production_text`, and asking `production_prefix` instead collected test
/// fixtures such as `src/builtin_tools/desktop/tests.rs`'s `const SECRET` as
/// though they were production.
///
/// # Why the ghosts direction reads source rather than a registry instance
///
/// `reg.unified_tools()` reports what THIS config enabled. The census builds a
/// `BuiltinToolConfig::default()` with no memory backend, so `memory_search`
/// and `note_manage` are absent from it, and `tool_search` / `subagent` are
/// registered by other subsystems entirely (the run loop and the agents
/// subsystem) — none of which makes their `DISPLAY_NAMES` entries dead. A
/// guard that cannot tell a defect from a config setting reports on the
/// fixture, not on the table, and its red is a misfire — the expensive kind,
/// because a misfiring guard gets cited as evidence.
///
/// # What this deliberately does NOT prove
///
/// That the const is a *tool name*. Any named const whose value happens to
/// equal a `DISPLAY_NAMES` key satisfies it, so a genuinely dead key that
/// collides with an unrelated string const would pass. That residual hole is
/// the price of config-independence, and [`DEAD_SPELLINGS`] is what keeps it
/// bounded: the two spellings this direction actually caught are asserted to
/// still not resolve. Escaped quotes inside a value are not handled (the value
/// would be truncated at the escape) — no tool name contains one, and the
/// failure direction is a missing entry, never a fabricated one.
fn scan_const_strings(root: &Path) -> ConstScan {
    const KEYWORD: &str = "const ";
    let mut out = ConstScan {
        consts: Vec::new(),
        test_modules: 0,
    };
    for (rel_path, raw) in crate::utils::source_scan::rust_sources_under(root) {
        let (production, parent_declared) = production_of(&rel_path, &raw);
        if parent_declared {
            out.test_modules += 1;
        }
        let text = crate::utils::source_scan::strip_comment_lines(&production);
        for (idx, _) in text.match_indices(KEYWORD) {
            if let Some(decl) = parse_const_string(&rel_path, &text[idx + KEYWORD.len()..]) {
                out.consts.push(decl);
            }
        }
    }
    out
}

/// Parse `IDENT: <ty> = "<literal>"` out of the text immediately following a
/// `const ` keyword. `None` for every other shape — `const fn`, a non-string
/// initializer (`= 7`, `= &[..]`), or a declaration with no `=` at all.
fn parse_const_string(file: &str, after: &str) -> Option<ConstString> {
    let ident_len = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    let ident = &after[..ident_len];
    // `const fn` is the one shape that otherwise parses as an identifier.
    if ident.is_empty() || ident == "fn" {
        return None;
    }
    // A const's type annotation is mandatory, so the `:` is what separates a
    // real declaration from an incidental `const ` inside other text.
    let rest = after[ident_len..].trim_start().strip_prefix(':')?;
    let eq = rest.find('=')?;
    // A `;` before the `=` means the `=` belongs to some later statement.
    if rest[..eq].contains(';') {
        return None;
    }
    let literal = rest[eq + 1..].trim_start().strip_prefix('"')?;
    let end = literal.find('"')?;
    Some(ConstString {
        ident: ident.to_string(),
        value: literal[..end].to_string(),
        file: file.to_string(),
    })
}

/// [`parse_const_string`] falsified directly, in both directions.
///
/// The two live probes around it are asymmetric: an under-match turns `ghosts`
/// red with the lost key named, so that direction is well covered. An
/// OVER-match is not — a spurious `ConstString` can only ever help a key
/// resolve, nothing bounds the collection's size or shape, and
/// [`DEAD_SPELLINGS`] notices only if the over-match happens to produce one of
/// two exact values out of an unbounded space. So the parser is the one thing
/// in this module whose correctness otherwise rests on an argument.
///
/// Each rejected case is a shape that reaches the parser in this repo today,
/// not an invented one: `const fn` (`fs_scope.rs:53`), an initializer-less
/// associated const (`tools/traits.rs`), an array-typed const whose `[u8; 4]`
/// puts a `;` before the `=`, and a non-literal initializer.
#[test]
fn parse_const_string_accepts_the_two_tool_name_shapes_and_rejects_the_rest() {
    let parse = |after: &str| parse_const_string("f.rs", after).map(|c| (c.ident, c.value));

    // Accepted: the two shapes tool names are actually written in.
    assert_eq!(
        parse("NAME: &'static str = \"file_read\";"),
        Some(("NAME".to_string(), "file_read".to_string())),
        "an associated `NAME` const is the shape most builtins use"
    );
    assert_eq!(
        parse("SUBAGENT_TOOL_NAME: &str = \"subagent\";"),
        Some(("SUBAGENT_TOOL_NAME".to_string(), "subagent".to_string())),
        "a free const naming a tool is the shape `src/agents/subagent_tool` uses"
    );

    // Rejected: shapes that must not contribute a tool name.
    assert_eq!(
        parse("fn workspace(base: PathBuf) -> Self {"),
        None,
        "`const fn`"
    );
    // Followed by real text, so this exercises the `;`-before-`=` guard rather
    // than the trivial "no `=` anywhere" path: in a real file the parser sees
    // the rest of the source after the declaration, and the next `=` belongs
    // to some later statement.
    assert_eq!(
        parse(
            "NAME: &'static str;\n    fn name(&self) -> &str { Self::NAME }\n\
               const OTHER: &str = \"stolen\";"
        ),
        None,
        "initializer-less associated const must not capture a later statement's literal"
    );
    assert_eq!(
        parse("BUF: [u8; 4] = [0; 4];"),
        None,
        "array type, and a `;` before the `=`"
    );
    assert_eq!(parse("COUNT: usize = 7;"), None, "non-string initializer");
    assert_eq!(
        parse("LIST: &[&str] = &[\"a\"];"),
        None,
        "array-valued — the probe consts"
    );
    assert_eq!(
        parse("X: &str = OTHER;"),
        None,
        "initialised from another const, not a literal"
    );

    // Boundaries of the literal itself.
    assert_eq!(
        parse("X: &str = \"\";").map(|(_, v)| v),
        Some(String::new()),
        "an empty literal parses as an empty value rather than failing — it matches no \
         DISPLAY_NAMES key, since a blank key is rejected separately"
    );
    assert_eq!(
        parse("X: &str = \"a\\\"b\";").map(|(_, v)| v),
        Some("a\\".to_string()),
        "a value containing an escaped quote TRUNCATES at that quote, keeping the \
         backslash. Pinned as known behaviour, not endorsed: no tool name contains one, \
         and the failure direction is a value that matches no key (a false ghost, which \
         is red) rather than a fabricated match"
    );
    assert_eq!(
        parse("X: &str = r\"y\";"),
        None,
        "a raw string is not parsed — same safe direction: it can only fail to resolve a \
         key, never invent one"
    );
}

/// Asserted against the REAL table and the REAL lookup, both called from the
/// `shared-ui-logic` dev-dependency (root `Cargo.toml`, `default-features =
/// false`). The earlier shape of this test scraped `DISPLAY_NAMES` out of
/// `summarize.rs` with `include_str!` and string splitting; that was a second
/// representation of the same table, free to drift from it, and a parser of a
/// literal can only ever prove a superset of what the compiler sees
/// (判据 §1/§10).
///
/// What this does NOT assert: that any particular tool has a curated label.
/// `display_name("node_list")` returns "Node List" through `humanize` — an
/// honest default, not a lie, so there is no correctness defect to guard and
/// no reason to enumerate the ~95 tools that take that path. The one
/// exception is the content-mutating tools, whose rows render a diff and
/// whose label is therefore a deliberate product decision.
///
/// The two directions over the table are derived from DIFFERENT sources —
/// ghosts from a source scan, blank labels from `reg.unified_tools()` unioned
/// with the table's own keys — and the comments at each explain why unifying
/// them for consistency would reintroduce a misfire. That asymmetry is a
/// decision, not an oversight.
///
/// Each direction carries a probe proving it can still fail:
/// [`DEAD_SPELLINGS`] for the ghosts check, [`BLANK_LABEL_PROBES`] for the
/// blank-label one. An assertion that a set is empty is only a guard while
/// something can still land in that set.
#[tokio::test]
async fn every_registered_tool_resolves_to_a_display_label() {
    use shared_ui_logic::transcript::{display_name, DISPLAY_NAMES};

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let reg = registry().await;

    // Vacuity: the table has to actually be a table, with both halves of
    // every entry present. A blank key matches no tool and a blank label
    // renders an empty row — 判据 §17, a wrong label costs more than a
    // missing one, because it reads as a fact.
    assert!(
        !DISPLAY_NAMES.is_empty(),
        "DISPLAY_NAMES is empty — every assertion below would pass vacuously"
    );
    let malformed: Vec<&(&str, &str)> = DISPLAY_NAMES
        .iter()
        .filter(|(name, label)| name.trim().is_empty() || label.trim().is_empty())
        .collect();
    assert!(
        malformed.is_empty(),
        "DISPLAY_NAMES entries with a blank key or a blank label: {malformed:?}"
    );

    // A duplicate key is unreachable-by-construction dead curation: the
    // lookup is a `find`, so only the first copy can ever render, and an
    // edit to any later one changes nothing while looking like it did.
    let mut seen = std::collections::HashSet::new();
    let duplicates: Vec<&str> = DISPLAY_NAMES
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !seen.insert(*n))
        .collect();
    assert!(
        duplicates.is_empty(),
        "DISPLAY_NAMES has duplicate key(s): {duplicates:?} — the lookup is a `find`, so \
         every copy after the first is dead curation"
    );

    // ---- Direction A: no key is a ghost. Derived from SOURCE, not from a
    // registry instance — see `scan_const_strings`' own doc for why. In
    // short: this direction asks "does anything under src/ declare this
    // name?", and a name declared by a subsystem this fixture did not boot
    // (or gated behind a backend this fixture did not configure) is still
    // declared. Asking a `BuiltinToolConfig::default()` instance instead made
    // this fire on `memory_search` / `note_manage` / `tool_search` /
    // `subagent` — four live tools — which is a misfire, not a finding.
    let scan = scan_const_strings(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    assert!(
        scan.test_modules > 0,
        "the const scan took nothing from 0 parent-declared test modules — \
         source_scan::production_text has stopped recognising `#[cfg(test)] mod <stem>;` \
         files, so a DISPLAY_NAMES key can now be kept alive by a const in a test fixture"
    );

    // One relation, used by the check and by its probe below — the same
    // discipline `renders_blank` applies to direction B. Two spellings of
    // "does this value resolve" would be free to drift, and then the probe
    // would be falsifying something other than what the check asserts
    // (判据 §9).
    let resolves = |key: &str| scan.consts.iter().any(|c| c.value == key);

    let ghosts: Vec<&str> = DISPLAY_NAMES
        .iter()
        .map(|(n, _)| *n)
        .filter(|key| !resolves(key))
        .collect();
    // Asserted BEFORE the negative probes on purpose: `consts` is the input to
    // both, so a broken walk that returned nothing would make the probes pass
    // vacuously (nothing resolves) while this one goes red on all 17 keys.
    // This assertion is therefore also the scan's own vacuity guard (判据 §2).
    assert!(
        ghosts.is_empty(),
        "DISPLAY_NAMES key(s) that no `const <IDENT>: <ty> = \"…\"` under src/ declares: \
         {ghosts:?} — a curated label attached to a name nothing declares renders on no \
         row, or worse, on the wrong one (判据 §17)"
    );

    // ---- Direction A, falsified. Without these, the rewrite above would be a
    // predicate nobody has ever seen go red.
    let resolved_dead: Vec<&str> = DEAD_SPELLINGS
        .iter()
        .copied()
        .filter(|dead| resolves(dead))
        .collect();
    if !resolved_dead.is_empty() {
        // Only on failure: name the declarations, so the report is actionable
        // rather than "one exists somewhere".
        let sites: Vec<String> = scan
            .consts
            .iter()
            .filter(|c| resolved_dead.contains(&c.value.as_str()))
            .map(|c| format!("`{}` declared as `{}` in {}", c.value, c.ident, c.file))
            .collect();
        panic!(
            "a DEAD_SPELLINGS probe resolved: {sites:?} — these are the spellings the \
             ghosts direction caught in Task 4 (the real names are `bash` and `search`). \
             If a legacy alias const has appeared, report it and decide whether the probe \
             or the alias is wrong; do not just drop the probe"
        );
    }

    // ---- Direction B: nothing we can name renders a blank label. Whatever
    // path a name takes — explicit entry, MCP `Server · Tool` split, or
    // `humanize` — it must come out with something to print.
    //
    // Derived from the registry PLUS the table's keys, and deliberately NOT
    // unified with direction A above. The asymmetry is the point, so do not
    // "fix" it for consistency: A iterates a fixed table against the whole
    // source tree and asks whether each key is *declared* anywhere, so a
    // narrow fixture config would make it report live tools as dead — a
    // misfire, and the expensive kind, because a misfiring guard gets cited as
    // evidence and then the guard gets weakened. B asks whether a name
    // *renders*, which is a pure function of the name: it cannot misfire on a
    // narrow config, it can only cover fewer names.
    //
    // Hence the union rather than `registered` alone. Under a default
    // `BuiltinToolConfig` the registry omits `memory_search` / `note_manage`
    // (no memory backend) and never sees `tool_search` / `subagent` (other
    // subsystems register those), so a registry-only input set left exactly
    // those four curated labels unasserted. A has already proved every key
    // names something declared under `src/`, so rendering the keys too costs
    // nothing. What remains uncovered is only the reverse case: a tool this
    // fixture does not enable AND that has no curated label — one of the ~95
    // that take the `humanize` path.
    let registered: Vec<String> = reg.unified_tools().map(|t| t.name.clone()).collect();
    assert!(
        !registered.is_empty(),
        "vacuity: the registry reported no tools at all"
    );
    let mut label_inputs: Vec<&str> = registered.iter().map(String::as_str).collect();
    label_inputs.extend(DISPLAY_NAMES.iter().map(|(n, _)| *n));
    label_inputs.sort_unstable();
    label_inputs.dedup();

    // One predicate, shared by the assertion and its probe below. A probe that
    // falsifies a DIFFERENT predicate than the one it vouches for proves
    // nothing about it (判据 §9: one verb, one derivation) — so `trim()` here
    // and `trim()` there have to be the same `trim()`.
    let renders_blank = |name: &str| display_name(name).trim().is_empty();

    let blank: Vec<&str> = label_inputs
        .iter()
        .copied()
        .filter(|name| renders_blank(name))
        .collect();
    assert!(
        blank.is_empty(),
        "name(s) whose display_name() renders blank — the row would carry no label at \
         all: {blank:?}"
    );

    // ---- Direction B, falsified. The assertion above says a set is empty;
    // this says the set CAN be non-empty, i.e. that `display_name` is still
    // capable of producing the failure it guards against. Without it, a
    // refactor that made blank labels impossible would leave that assertion
    // permanently green and indistinguishable from a working guard (判据 §2).
    let unfalsifiable: Vec<&str> = BLANK_LABEL_PROBES
        .iter()
        .copied()
        .filter(|p| !renders_blank(p))
        .collect();
    assert!(
        unfalsifiable.is_empty(),
        "BLANK_LABEL_PROBES that no longer render blank: {unfalsifiable:?} — display_name \
         can apparently no longer return an empty label, which makes the assertion above \
         a predicate that can never go red. Give that assertion a different predicate; do \
         not delete the probe (see BLANK_LABEL_PROBES' own doc)"
    );

    // Content-mutating tools carry a rich diff, so their label is a product
    // decision rather than whatever `humanize` happens to produce. Read off
    // PRESENTATION_FIXTURES instead of spelled out a second time here: that
    // table is already pinned to the source scan's own set of mutating tools
    // by the census above, so a fourth mutating tool arriving tomorrow
    // inherits this requirement instead of quietly shipping a fallback label
    // (判据 §5 — a hand-written list only covers legislation day).
    let unlabelled_mutators: Vec<&str> = PRESENTATION_FIXTURES
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !DISPLAY_NAMES.iter().any(|(k, _)| k == n))
        .collect();
    assert!(
        unlabelled_mutators.is_empty(),
        "content-mutating tool(s) with no explicit DISPLAY_NAMES entry: \
         {unlabelled_mutators:?} — these are the rows that render a diff, so their label \
         is a deliberate call; add one rather than accepting the humanize() fallback"
    );
}
