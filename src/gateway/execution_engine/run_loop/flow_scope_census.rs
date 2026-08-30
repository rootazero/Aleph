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
//! Five layers now, each answering a different question, none subsuming
//! another:
//!
//! 1. **The type.** [`crate::scope::FlowScope`] has private fields, no
//!    `Default`, and one non-empty constructor. So the two strings cannot be
//!    written into the `FlowRequest` field directly: the raw pair at that site
//!    is `E0308` (measured), and a struct literal does not compile outside
//!    `crate::scope`. What it refuses is that SHAPE. It says nothing about
//!    where the `ScopeAttribution` handed to `resolved` came from — see the
//!    bounds below, because an earlier version of this list claimed it did.
//!    It is also the only layer that reaches beyond this module.
//! 2. **The negative census**, now run over BOTH views of each file: key
//!    identifiers in `code_text`, and the key values as exact quoted literals
//!    in [`crate::utils::source_scan::code_keeping_literals`]. This is what
//!    covers a raw read that never touches `FlowRequest` — one taken to decide
//!    something, where no type stands in the way.
//! 3. **The call counts**, over the module's production TEXT:
//!    `scope_from_metadata` exactly once, the literal `FlowScope::resolved(`
//!    exactly once, `FlowScope::unscoped(` never. What they object to is a
//!    second OCCURRENCE — a second call site, a second mint, an import that
//!    names one — **not a second answer**. An earlier version of this list
//!    said the counts "object to a second answer existing at all"; that was
//!    measured false and the bound is below.
//! 4. **Two behavioural tests**, in `run_loop::tests`:
//!    `the_flow_request_projection_carries_the_room_upgrade` and
//!    `the_projection_round_trips_through_the_dispatch_rebuild`. These are the
//!    layer that owns PROVENANCE, and they are the strongest guard in this
//!    package because they assert the property — a claimed key reaches the
//!    harness as the room — rather than a spelling. So a resolution that LOSES
//!    the room upgrade is red however it is written, `as` alias included.
//!    Precisely that, and not more: what they assert is a VALUE, so anything
//!    reaching the same value satisfies them, whatever computed it.
//!    Named here because 1–3 are the layers a reader can see, and a defence
//!    nobody names is one refactor away from being deleted as redundant with
//!    the layers that are named.
//! 5. **One positive structural claim**, in
//!    `tests::the_flow_request_site_derives_its_scope_from_request_scope`:
//!    `request_scope_strings`'s BODY must call `request_scope`. Layers 1–4 are
//!    all "this text must not appear" or "this value must hold", and the one
//!    shape all four miss is a projection that forks and re-derives the answer
//!    correctly. This one says the projection must be a projection. Measured:
//!    one red naming the offending body, where the same fork was `41 passed;
//!    0 failed` before it existed.
//!
//! The bounds, stated so nobody has to infer them. Three generations of this
//! paragraph shipped false — each narrower than the last and each still wide —
//! because prose about coverage has no test and drifts every time it is
//! touched, so each layer's coverage IS a case now:
//! `the_layer_3_counts_object_to_a_second_occurrence_not_a_second_answer`,
//! `layer_2_sees_both_spellings_of_a_raw_read_and_neither_of_a_built_one`,
//! `the_projection_body_must_call_request_scope`,
//! `the_declined_from_persisted_count_would_have_caught_the_duplicate`,
//! `the_default_premise_reads_the_derive_list_and_this_file_only`, and
//! `run_loop::tests::layer_4_discriminates_the_answer_and_only_the_answer`.
//! **A coverage claim added here that COULD be a case and is not, is the
//! fourth generation.**
//!
//! Not every bound below can be, and saying "every one of them is" would be
//! that same error one level up — the first draft of this paragraph said it.
//! So each bound is labelled with what actually holds it: a named test, a
//! recorded measurement, or another module's guard. The two compile-failure
//! claims in layer 1 (`E0308` at the field; a struct literal outside
//! `crate::scope`) are held by the field-privacy assertion plus rustc, and
//! nothing short of a `trybuild` fixture reaches them directly.
//!
//! - **Layer 1 constrains a shape, not a provenance, and one public call
//!   bridges the gap.** [`crate::scope::ScopeAttribution`] is `pub` with `pub`
//!   fields and is reachable three ways: `from_persisted`, which takes exactly
//!   the pair of `Option<&str>` a metadata map yields; `personal`; and a struct
//!   literal, which `tests/gateway_chat_room_author_across_spawn.rs` uses from
//!   outside the crate. A review drove that: `FlowScope::resolved` fed from
//!   `from_persisted` on the raw map, with the keys spelled by `concat!` so
//!   neither census view sees them, **compiles**, and when it was measured it
//!   left every census test GREEN while layer 4's two went RED with
//!   `left: Personal(…) / right: Project(…)`. **Layer 5 moved that number, so
//!   the record is re-measured rather than carried**: the same bypass is now
//!   `43 passed; 4 failed`, because a body fed from `from_persisted` is a body
//!   that stopped calling `request_scope`. Layers 2 and 3 are still green on
//!   it, and that is the half this bound is about — no rule about a SPELLING
//!   reaches provenance. Sealing `ScopeAttribution` is not on the table — it is `pub`
//!   with `pub` fields and used repo-wide — so provenance is layer 4's, and
//!   only layer 4's. **Held by a recorded measurement, not by a test**: the
//!   numbers above are probe results on a mutated tree, and nothing goes red
//!   if `ScopeAttribution` stops being reachable that way.
//! - **Two of the five tests in that block are not provenance guards at all.**
//!   `an_off_roster_speaker_is_projected_exactly_as_the_raw_read_was` asserts
//!   equality WITH the raw read, so the bypass above satisfies it rather than
//!   tripping it, and `an_unstamped_turn_projects_no_strings` stays green
//!   because `from_persisted` is fail-closed on the pair too. Both are
//!   load-bearing for what they do say; neither is a provenance guard, and
//!   counting them as one would restate the same overclaim one level down.
//!   The other three DO go red for a resolution that loses the upgrade —
//!   measured under the bypass above: the two named as layer 4, plus
//!   `layer_4_discriminates_the_answer_and_only_the_answer`, whose first
//!   assertion needs the same value. That split is a MEASUREMENT, not a
//!   pinned claim: nothing here goes red if a sixth test lands in the block
//!   on either side of it, so re-measure before citing the count.
//! - **A duplicate resolution that AGREES is layer 5's, and only in layer 5's
//!   narrow form.** A second, independent resolution of this run's scope —
//!   `ScopeAttribution::from_persisted` on the raw map with the keys spelled
//!   by `concat!`, then `request_scope`'s room-claim upgrade re-implemented so
//!   it reaches the same answer — leaves layers 1–4 green, measured twice at
//!   `41 passed; 0 failed`: it names no raw key, adds no `scope_from_metadata`
//!   occurrence, REPLACES the single mint rather than adding one, and layer 4
//!   is correctly green because the answer matches. Layer 5 is red for it, and
//!   is the only thing here that is. What layer 5 does NOT catch, measured on
//!   the live file: the same duplicate with a call to `request_scope` left
//!   standing beside it — dead, logged, or used for something else — which is
//!   `47 passed; 0 failed`. **That residue is open and belongs to no layer
//!   here.** It is worth naming rather than rounding off, because it is 地雷 Q
//!   clause ③'s own recorded history: a later reader forking away from
//!   `request_scope` because a fork was easier to write than a call. **Held by
//!   `tests::the_projection_body_must_call_request_scope` and
//!   `tests::the_layer_3_counts_object_to_a_second_occurrence_not_a_second_answer`**,
//!   both halves — the catch and the residue.
//! - **A raw read OUTSIDE this module** is not covered by 2, 3 or 5, and
//!   `BusyInputMode::for_shared_room` (`execution_engine/mod.rs`) is one — it
//!   runs on the admission path, before `request_scope` can have run, and
//!   answers a different question with a different predicate. **Held by that
//!   function's own guard, not by anything here** — see its doc.

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

    // =====================================================================
    // The predicates, as functions
    //
    // Layers 2, 3 and 5 are pure functions of module text. Written as loops
    // inline in the live tests they could only be claimed about in prose,
    // which is how the paragraphs above got written wrong three times. As
    // functions the claims tests below drive the SAME code the live census
    // drives, so a claim and its measurement cannot drift apart.
    // =====================================================================

    /// Layer 3's three counts over an arbitrary corpus.
    #[derive(Debug, PartialEq, Eq)]
    struct Layer3 {
        resolutions: usize,
        mints: usize,
        empties: usize,
    }

    fn layer3_counts<'a>(sources: impl IntoIterator<Item = &'a str>) -> Layer3 {
        let mut out = Layer3 {
            resolutions: 0,
            mints: 0,
            empties: 0,
        };
        for text in sources {
            let code = code_text(&production_prefix(text));
            out.resolutions += code.matches("scope_from_metadata").count();
            out.mints += code.matches("FlowScope::resolved(").count();
            out.empties += code.matches("FlowScope::unscoped(").count();
        }
        out
    }

    /// Layer 2's offenders for one file, both views.
    fn layer2_offenders(rel: &str, text: &str) -> Vec<String> {
        let literals = forbidden_literals();
        // `production_code_lines` is common to both views: it blanks the
        // `#[cfg(test)]` items and the comment-only lines while PRESERVING
        // line numbers, so an offender can be opened at the number this
        // prints.
        let production = production_code_lines(text);
        let mut offenders = Vec::new();
        // View 1 - identifiers. `code_text` then removes string-literal
        // payloads, so a guard message or a fixture quoting a key is not a
        // hit. That is also why this view cannot see view 2's needles.
        for (i, line) in code_text(&production).lines().enumerate() {
            for key in FORBIDDEN {
                if line.contains(key) {
                    offenders.push(format!("{rel}:{}: [constant] {}", i + 1, line.trim()));
                }
            }
        }
        // View 2 - the values. `code_keeping_literals` keeps payloads and
        // drops ALL comment text (including a comment trailing live code,
        // which `production_code_lines` alone leaves standing), so prose
        // cannot trip this and a raw read spelled by the key's value cannot
        // hide from it. The needles carry their quotes, so only an exact
        // payload matches.
        for (i, line) in code_keeping_literals(&production).lines().enumerate() {
            for needle in &literals {
                if line.contains(needle.as_str()) {
                    offenders.push(format!("{rel}:{}: [literal] {}", i + 1, line.trim()));
                }
            }
        }
        offenders
    }

    /// Layer 5's subject: the brace-matched body of `fn
    /// request_scope_strings`, from module text already reduced by
    /// `production_prefix` + `code_text` — so no brace can be hiding inside a
    /// comment or a string payload.
    ///
    /// `None` means the declaration was not found or its braces do not
    /// balance. The live test reports that separately from a body that
    /// forked: "the projection is gone" and "the projection re-derives" are
    /// two different reds.
    fn projection_body(module_code: &str) -> Option<&str> {
        let from_fn = &module_code[module_code.find("fn request_scope_strings")?..];
        let open = from_fn.find('{')?;
        let mut depth = 0usize;
        for (i, b) in from_fn.bytes().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(&from_fn[open + 1..i]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Layer 1's two source-level reads off a `scope/mod.rs`-shaped text:
    /// `(struct body, derive list)`.
    fn layer1_reading(scope_mod_production: &str) -> Option<(&str, &str)> {
        let (before, after) = scope_mod_production.split_once("struct FlowScope {")?;
        let body = after.split_once('}')?.0;
        let derives = before
            .rsplit_once("#[derive(")
            .and_then(|(_, tail)| tail.split_once(")]"))
            .map(|(list, _)| list)
            .unwrap_or_default();
        Some((body, derives))
    }

    /// Layer 1's third read: a hand-written `Default` in the type's own file.
    /// A function so the claims test drives the same needle rather than a
    /// second copy of it.
    fn declares_a_default_impl(scope_mod_production: &str) -> bool {
        scope_mod_production.contains("impl Default for FlowScope")
    }

    /// The count this package DECLINED to add, kept as a function so the
    /// reason for declining it can be measured instead of asserted. See
    /// `the_declined_from_persisted_count_would_have_caught_the_duplicate`.
    fn from_persisted_mentions(text: &str) -> usize {
        code_text(&production_prefix(text))
            .matches("from_persisted")
            .count()
    }

    // =====================================================================
    // Probe corpora
    //
    // The smallest module text carrying each shape the module doc makes a
    // claim about. All of them live inside `mod tests`, which
    // `production_prefix` and `production_code_lines` excise, so none can
    // trip the live census on this file.
    // =====================================================================

    /// The shipped shape: one resolution, one projection that reads it.
    const LIVE_SHAPE: &str = r#"
fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    crate::scope::scope_from_metadata(&request.metadata)
}
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    crate::scope::FlowScope::resolved(request_scope(request).as_ref())
}
"#;

    /// The duplicate that AGREES — the shape a review drove end to end.
    ///
    /// The projection forks and re-derives this run's scope itself, by a
    /// route that names no raw key (`concat!`), calls no
    /// `scope_from_metadata`, and REPLACES the single mint rather than adding
    /// one. The room-claim upgrade is elided behind `upgrade_for_room`
    /// because no predicate in layers 1-5 reads it; the review inlined
    /// `request_scope`'s upgrade verbatim and measured `41 passed; 0 failed`,
    /// and this file's own task reproduced that before changing anything.
    const FORKED_BODY: &str = r#"
fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    crate::scope::scope_from_metadata(&request.metadata)
}
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    let dup = crate::scope::ScopeAttribution::from_persisted(
        request.metadata.get(concat!("scope_owner", "_user_id")).map(String::as_str),
        request.metadata.get(concat!("scope", "_id")).map(String::as_str),
    );
    crate::scope::FlowScope::resolved(upgrade_for_room(request, dup).as_ref())
}
"#;

    /// The same duplicate with a call to `request_scope` left standing —
    /// dead, or used for something else on the way past. This is layer 5's
    /// bound.
    const FORKED_BODY_WITH_SURVIVING_CALL: &str = r#"
fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    crate::scope::scope_from_metadata(&request.metadata)
}
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    let _audit = request_scope(request);
    let dup = crate::scope::ScopeAttribution::from_persisted(
        request.metadata.get(concat!("scope_owner", "_user_id")).map(String::as_str),
        request.metadata.get(concat!("scope", "_id")).map(String::as_str),
    );
    crate::scope::FlowScope::resolved(upgrade_for_room(request, dup).as_ref())
}
"#;

    /// A second answer spelled the way layer 3 DOES see: the resolver called
    /// again, under a new name.
    const SECOND_RESOLUTION: &str = r#"
fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    crate::scope::scope_from_metadata(&request.metadata)
}
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    let own = crate::scope::scope_from_metadata(&request.metadata);
    crate::scope::FlowScope::resolved(own.as_ref())
}
"#;

    /// A second answer spelled as a second mint.
    const SECOND_MINT: &str = r#"
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    if request.metadata.is_empty() {
        return crate::scope::FlowScope::resolved(None);
    }
    crate::scope::FlowScope::resolved(request_scope(request).as_ref())
}
"#;

    /// The empty pair, minted by name.
    const EXPLICIT_UNSCOPED: &str = r#"
fn request_scope_strings(_request: &RunRequest) -> crate::scope::FlowScope {
    crate::scope::FlowScope::unscoped()
}
"#;

    /// The single mint under a type alias. Layer 3's needle carries the TYPE
    /// name, so an `as` alias really does defeat THIS count.
    const ALIASED_MINT: &str = r#"
use crate::scope::FlowScope as FS;
fn request_scope_strings(request: &RunRequest) -> FS {
    FS::resolved(request_scope(request).as_ref())
}
"#;

    /// The raw read layer 2 exists for, in both spellings.
    const RAW_READ_BY_CONSTANT: &str =
        "fn f(r: &RunRequest) { let _ = r.metadata.get(crate::scope::OWNER_META_KEY); }";
    const RAW_READ_BY_LITERAL: &str =
        r#"fn f(r: &RunRequest) { let _ = r.metadata.get("scope_id"); }"#;

    /// The duplicate with the TYPE aliased. `from_persisted` is an inherent
    /// associated function and Rust has no `use Type::assoc_fn`, so the alias
    /// renames the type and the identifier survives.
    const FORKED_BODY_ALIASED_TYPE: &str = r#"
use crate::scope::ScopeAttribution as SA;
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    let dup = SA::from_persisted(
        request.metadata.get(concat!("scope_owner", "_user_id")).map(String::as_str),
        request.metadata.get(concat!("scope", "_id")).map(String::as_str),
    );
    crate::scope::FlowScope::resolved(upgrade_for_room(request, dup).as_ref())
}
"#;

    /// The duplicate built by struct literal. `ScopeAttribution` is `pub`
    /// with `pub` fields, so this compiles anywhere in the crate and names no
    /// constructor at all.
    const FORKED_BODY_STRUCT_LITERAL: &str = r#"
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    let dup = request
        .metadata
        .get(concat!("scope_owner", "_user_id"))
        .and_then(|owner| {
            let raw = request.metadata.get(concat!("scope", "_id"))?;
            Some(crate::scope::ScopeAttribution {
                owner_user_id: owner.clone(),
                scope: crate::scope::ScopeId::parse(raw)?,
            })
        });
    crate::scope::FlowScope::resolved(upgrade_for_room(request, dup).as_ref())
}
"#;

    /// A `scope/mod.rs` whose `FlowScope` derives `Default`.
    const SCOPE_MOD_WITH_DERIVE: &str = r#"
#[derive(Clone, Debug, Default)]
pub struct FlowScope {
    owner_user_id: Option<String>,
    scope_id: Option<String>,
}
"#;

    /// The same file with a HAND-WRITTEN impl and a clean derive list.
    const SCOPE_MOD_WITH_HANDWRITTEN_DEFAULT: &str = r#"
#[derive(Clone, Debug)]
pub struct FlowScope {
    owner_user_id: Option<String>,
    scope_id: Option<String>,
}
impl Default for FlowScope {
    fn default() -> Self {
        Self { owner_user_id: None, scope_id: None }
    }
}
"#;

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
        // LAYER 5. Everything else here is negative ("this text must not
        // appear") and layer 4 is a value assertion; both are satisfied by a
        // projection whose BODY forked. `inner.rs` still calls it, `mod.rs`
        // still declares it, and a body that re-derives the answer from
        // `ScopeAttribution::from_persisted` with the keys spelled by
        // `concat!` names no raw key, adds no `scope_from_metadata`
        // occurrence, and REPLACES the single mint rather than adding one.
        // That shape was measured green on layers 1-4 twice. This is the one
        // claim in the package that catches it, and
        // `the_projection_body_must_call_request_scope` states how far it
        // reaches.
        let body = projection_body(&projection).expect(
            "`fn request_scope_strings` must be declared in mod.rs with balanced \
             braces — it is the projection `inner.rs` hands to the harness, and \
             layer 5 has nothing to read without it",
        );
        assert!(
            body.contains("request_scope("),
            "`request_scope_strings` must CALL `request_scope`, not re-derive this \
             run's scope itself. A body that resolves the pair a second way is a \
             second answer to 'what scope is this run under', and a second answer \
             that agrees today is invisible to every other layer here: it names no \
             raw key (layer 2), adds no `scope_from_metadata` occurrence and no \
             second mint (layer 3), and satisfies the two behavioural tests (layer \
             4) for exactly as long as it keeps agreeing. The needle is the literal \
             text `request_scope(`, so a call made through a function-pointer \
             binding reds this instead of passing it — the safe direction. Body \
             was:\n{body}"
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
        let files = production_files();
        let counts = layer3_counts(files.iter().map(|(_, text)| text.as_str()));
        assert_eq!(
            counts.resolutions, 1,
            "`scope_from_metadata` must be named exactly once in run_loop — inside \
             `request_scope`, which is where the room-claim correction is applied on \
             top of it. A second occurrence is a second answer to 'what scope is \
             this run under', and the one that skips the correction is the one that \
             ships the producer's `personal:<speaker>` stamp."
        );
        // Same shape, on the other end of the projection. `FlowScope`'s
        // private fields stop a pair of raw strings from reaching the field,
        // but they do not stop a SECOND mint from a differently-resolved
        // attribution, and `unscoped()` mints the empty pair outright — which
        // drops the room just as completely, and reads as deliberate.
        assert_eq!(
            counts.mints, 1,
            "`FlowScope::resolved` must be minted exactly once in run_loop — inside \
             `request_scope_strings`, from `request_scope`'s answer. A second mint is \
             a second answer to the same question that names no raw key and would \
             pass every other check here."
        );
        assert_eq!(
            counts.empties, 0,
            "nothing in run_loop may hand the harness a deliberately empty \
             `FlowScope`. `request_scope` is already fail-closed — `resolved(None)` \
             is how an unstamped turn projects to the empty pair — so an explicit \
             `unscoped()` here can only be a scope being thrown away."
        );
    }

    /// Layer 1's three source-level premises: `FlowScope`'s fields stay
    /// private, it grows no `Default` derive, and its own file declares no
    /// `impl Default for FlowScope`.
    ///
    /// All three are load-bearing and the first two were measured so. Make
    /// the two fields `pub` and every check above stays green while
    /// `owner_user_id: request.metadata.get(…)` compiles again — the exact
    /// build a review took end to end. `#[derive(Default)]` is refused for
    /// the same reason `unscoped` is counted above: it would be a second,
    /// unnamed way to mint the empty pair, and no text search can find
    /// `Default::default()`.
    ///
    /// A source-level check because there is nothing to observe at runtime: a
    /// public field and a private one behave identically once something has
    /// been built out of them.
    ///
    /// # What it does NOT establish
    ///
    /// This test was called `…_stays_unassemblable_by_hand`, and that name was
    /// read — including by the doc above it — as authority for more than these
    /// assertions cover. A `FlowScope` carrying any two chosen strings IS
    /// assemblable by hand, in two public calls, because
    /// `ScopeAttribution::from_persisted` takes exactly the pair of
    /// `Option<&str>` a metadata map yields; a struct-literal
    /// `ScopeAttribution` reaches the same constructor from outside the crate,
    /// which `tests/gateway_chat_room_author_across_spawn.rs` does.
    ///
    /// The `Default` half reaches **two spellings in one file**, and that is
    /// the whole of it. A review measured the residue: the derive assertion
    /// reads a derive LIST, so a hand-written `impl Default for FlowScope` was
    /// invisible to it — hence the third assertion, which reads the same file
    /// for the impl. Rust's orphan rule keeps that impl inside this crate but
    /// not inside this file, and a single-file reader cannot see one written
    /// elsewhere. What catches its USE in `run_loop` is layer 4, measured: a
    /// `Default` mint projects the empty pair, the empty pair is a different
    /// VALUE, and
    /// `run_loop::tests::layer_4_discriminates_the_answer_and_only_the_answer`
    /// pins that step so this sentence is not the only thing holding it.
    ///
    /// What these assertions pin is the SHAPE at the field: no raw pair
    /// written straight into `FlowScope`, and no unnamed mint of the empty one
    /// from its own file. Provenance is layer 4's — the two behavioural tests
    /// named in this module's doc — and nothing here reaches it.
    #[test]
    fn the_flow_request_scope_type_refuses_a_raw_pair_and_an_unnamed_empty() {
        let scope_mod = production_code_lines(include_str!("../../../scope/mod.rs"));
        let (body, derives) = layer1_reading(&scope_mod).expect(
            "`src/scope/mod.rs` must still declare `struct FlowScope` with a closing \
             brace — it is what makes the raw-read spelling a compile error at the \
             `FlowRequest` site, and layer 1 has nothing to read without it",
        );
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
        assert!(
            !declares_a_default_impl(&scope_mod),
            "`src/scope/mod.rs` must not hand-write `impl Default for FlowScope` \
             either. The derive assertion above reads a derive LIST and a review \
             measured that it is blind to the impl — same empty mint, same \
             unfindable `Default::default()` call site, one spelling further away."
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
        for (rel, text) in production_files() {
            scanned += 1;
            offenders.extend(layer2_offenders(&rel, &text));
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

    // =====================================================================
    // The claims, as measurements
    //
    // Every sentence in this module's doc that says a layer catches X, or
    // does not catch Y, is a case below. Three generations of that paragraph
    // shipped false because prose about coverage has no test: each was
    // narrower than the last and each was still wide. A claim added up there
    // without a case down here is the fourth.
    //
    // The cases that assert a hole is OPEN are the load-bearing half. They
    // are written so that STRENGTHENING a layer turns them red by name, which
    // forces the paragraph to be corrected in the same change instead of
    // quietly going stale in the other direction.
    // =====================================================================

    /// Layer 3 objects to a second OCCURRENCE, not to a second ANSWER.
    ///
    /// The distinction is the whole of the finding that produced this file's
    /// current wording: a doc that assigns "a second answer existing at all"
    /// to these counts sends a reader who checks the assignment, finds three
    /// counts, and concludes the hole is closed.
    #[test]
    fn the_layer_3_counts_object_to_a_second_occurrence_not_a_second_answer() {
        assert_eq!(
            layer3_counts([LIVE_SHAPE]),
            Layer3 {
                resolutions: 1,
                mints: 1,
                empties: 0
            },
            "premise: the shipped shape is what these counts are calibrated to. If \
             this drifts, every case below is measuring a different baseline."
        );

        // The hole, asserted OPEN.
        assert_eq!(
            layer3_counts([FORKED_BODY]),
            Layer3 {
                resolutions: 1,
                mints: 1,
                empties: 0
            },
            "layer 3 does not object to a duplicate resolution that reaches the same \
             answer: it names no raw key, calls no `scope_from_metadata`, and \
             REPLACES the single mint rather than adding one. If this ever fires, a \
             layer has been strengthened to cover the shape — say so in the module \
             doc, which states this hole as open and assigns it to layer 5 only in \
             layer 5's narrow form."
        );

        // What the counts do object to.
        assert_eq!(
            layer3_counts([SECOND_RESOLUTION]).resolutions,
            2,
            "a second `scope_from_metadata` is the spelling layer 3 does see"
        );
        assert_eq!(
            layer3_counts([SECOND_MINT]).mints,
            2,
            "a second `FlowScope::resolved(` is the other spelling layer 3 sees"
        );
        assert_eq!(
            layer3_counts([EXPLICIT_UNSCOPED]).empties,
            1,
            "a named mint of the empty pair is the third"
        );

        // And the bound on the mint count: its needle carries the type name.
        assert_eq!(
            layer3_counts([ALIASED_MINT]).mints,
            0,
            "the mint count's needle is the literal `FlowScope::resolved(`, so a \
             type alias hides the mint from it. This is a lexical count and the doc \
             states it as one; do not let a later sentence call it a guarantee."
        );
    }

    /// Layer 2 sees a raw read in both spellings, and neither of a built one.
    #[test]
    fn layer_2_sees_both_spellings_of_a_raw_read_and_neither_of_a_built_one() {
        assert_eq!(
            layer2_offenders("probe.rs", RAW_READ_BY_CONSTANT).len(),
            1,
            "the identifier view is what the first version of this census had"
        );
        assert_eq!(
            layer2_offenders("probe.rs", RAW_READ_BY_LITERAL).len(),
            1,
            "the literal-value view is what a review added after driving the same \
             read, spelled by the key's VALUE, past all three checks of the first \
             version"
        );
        assert!(
            layer2_offenders("probe.rs", FORKED_BODY).is_empty(),
            "keys assembled by `concat!` are invisible to BOTH views by \
             construction: the payloads are the halves, and no half is a needle. \
             This is why layer 2 cannot be the answer to provenance, and why the \
             duplicate is measured against layers 3 and 5 instead."
        );
    }

    /// Layer 5 is red for a body that forked, and green for one that kept a
    /// call to `request_scope` while forking anyway.
    ///
    /// The second half is the bound. It is asserted rather than described
    /// because a bound that is only described is what this file is under
    /// repair for.
    #[test]
    fn the_projection_body_must_call_request_scope() {
        assert!(
            projection_body(LIVE_SHAPE).is_some_and(|b| b.contains("request_scope(")),
            "premise: the shipped shape satisfies layer 5, or the live assertion is \
             measuring something other than what this case describes"
        );
        assert!(
            projection_body(FORKED_BODY).is_some_and(|b| !b.contains("request_scope(")),
            "layer 5 must be RED for the duplicate that agrees — it is the only \
             thing in this package that catches it, and the module doc says so"
        );
        // The bound, asserted OPEN.
        assert!(
            projection_body(FORKED_BODY_WITH_SURVIVING_CALL)
                .is_some_and(|b| b.contains("request_scope(")),
            "layer 5 is satisfied by a body that keeps ANY call to `request_scope` \
             — dead, logged, or used for something else — while resolving a second \
             time beside it. That residue is stated as open in the module doc. If \
             this ever fires, layer 5 has grown past a lexical call check and the \
             doc must stop calling it one."
        );
    }

    /// The reason recorded for declining a `from_persisted == 0` count.
    ///
    /// Three reasons were recorded. The third — "every case it could catch,
    /// layer 4 already catches by property" — was measured false, and the
    /// counterexample is the duplicate above: the count sees it and layer 4
    /// does not. The decision to decline still stands, and this case exists
    /// so the false reason cannot come back as its justification.
    ///
    /// It also corrects the second reason by one notch. "An `as` alias
    /// defeats it" is true of layer 3's MINT count, whose needle carries the
    /// type name, and false of a bare `from_persisted` needle: Rust has no
    /// `use Type::assoc_fn`, so an alias renames only the type. What actually
    /// defeats it is a struct literal.
    #[test]
    fn the_declined_from_persisted_count_would_have_caught_the_duplicate() {
        assert_eq!(
            from_persisted_mentions(LIVE_SHAPE),
            0,
            "premise: the count would read `== 0` on the shipped shape, so it is a \
             rule that could have been adopted rather than one that never fit"
        );
        assert_eq!(
            from_persisted_mentions(FORKED_BODY),
            1,
            "the declined count WOULD fire on the duplicate that agrees, which every \
             other layer here is green on. That is why the recorded reason for \
             declining it may not be 'layer 4 already catches everything it would'."
        );
        assert_eq!(
            from_persisted_mentions(FORKED_BODY_ALIASED_TYPE),
            1,
            "an `as` alias on `ScopeAttribution` does NOT defeat a bare \
             `from_persisted` needle: the alias renames the type, and the inherent \
             associated function's own identifier survives"
        );
        assert_eq!(
            from_persisted_mentions(FORKED_BODY_STRUCT_LITERAL),
            0,
            "what defeats it is a struct literal — `ScopeAttribution` is `pub` with \
             `pub` fields, so the same duplicate can be built naming no constructor \
             at all. THIS is the honest reason the count is too weak to be worth a \
             second lexical rule."
        );
    }

    /// Layer 1's `Default` premise reads a derive LIST and one file.
    ///
    /// Asserted rather than described because the doc's own justification —
    /// "no text search can find it" — applies verbatim to a hand-written
    /// impl, which is how that sentence came to be one notch wider than the
    /// assertion under it.
    #[test]
    fn the_default_premise_reads_the_derive_list_and_this_file_only() {
        let (_, derives) =
            layer1_reading(SCOPE_MOD_WITH_DERIVE).expect("premise: the corpus parses");
        assert!(
            derives.contains("Default"),
            "the derive spelling is what the derive assertion sees"
        );
        assert!(
            !declares_a_default_impl(SCOPE_MOD_WITH_DERIVE),
            "premise: the two reads are independent — the derive corpus has no impl"
        );

        let (_, derives) = layer1_reading(SCOPE_MOD_WITH_HANDWRITTEN_DEFAULT)
            .expect("premise: the corpus parses");
        assert!(
            !derives.contains("Default"),
            "the derive assertion is BLIND to a hand-written `impl Default for \
             FlowScope`. If this ever fires, the derive read has grown to cover the \
             impl and the doc must stop describing them as two separate reads."
        );
        assert!(
            declares_a_default_impl(SCOPE_MOD_WITH_HANDWRITTEN_DEFAULT),
            "the second read is what catches the impl — in the type's own file, \
             which is as far as a single-file reader goes. An impl written in \
             another module of this crate is not covered here; layer 4 catches its \
             USE in run_loop, because the empty pair it mints is a different value."
        );
    }
}
