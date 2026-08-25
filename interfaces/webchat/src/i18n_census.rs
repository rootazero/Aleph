//! What counts as hard-coded Chinese copy — and how much of it this crate is
//! still allowed to carry.
//!
//! Copy belongs in `locales/{zh,en}.json` and reaches a view through `t!` /
//! `t_string!`, which `leptos_i18n` resolves at compile time: a missing key is
//! a build error, not a silent fallback. A string literal with Chinese in it
//! is what that looks like when someone forgot — it renders identically for a
//! reader who set the language to English, and nothing in Settings → General
//! moves it.
//!
//! # Two guards, one detector
//!
//! There are two questions here and they have different answers, which is why
//! the detector lives at the crate root rather than next to either of them:
//!
//! * *does anything a **phone screen** can reach hard-code copy?* — answered by
//!   [`crate::platform::phone`]'s `i18n_census`, scope defined by following
//!   `use crate::…` one hop out of `platform/phone/`, tolerance **zero**;
//! * *is the crate as a whole getting worse?* — answered by
//!   `hardcoded_chinese_line_ratchet` below, scope the whole `src/` tree,
//!   tolerance a ceiling that may only go down.
//!
//! Before this module the first guard owned the predicate privately and the
//! second did not exist, so the 126 lines outside the phone tree's reach were
//! recorded in a doc comment as "a separate round" with nothing measuring them
//! between rounds. Two guards deciding independently what "Chinese copy" means
//! is the same defect one level up, hence one `is_chinese` and one
//! `production_lines` for both.
//!
//! # Why the crate-wide half is a ratchet and not a sweep
//!
//! The debt is real and it is copy, not structure: retiring it means writing
//! locale keys in two languages, which is a round of its own. A ratchet is
//! green today, red the moment a new literal lands, and it carries its own
//! force to shrink — the only direction it can be edited is down. An exemption
//! list would have been the alternative, and this repo has already paid for
//! one of those: a permit with nothing to make it expire.
//!
//! # What this measures, and both directions it is wrong in
//!
//! "A production string literal containing Chinese" is a **proxy** for "copy
//! that ignores the locale", and the first sweep run against it found the
//! proxy failing in each direction:
//!
//! * **It counts data.** `components/command_palette.rs` carries 11 lines of
//!   deliberately bilingual `keywords:` — search aliases so a Chinese speaker
//!   can find a command while the UI is in English. Translating them would
//!   *break* search. So the ceiling has a floor of 11 rather than 0, and once
//!   the sweep reached that floor the shape question came due. The answer was
//!   to leave them where they are and **pin the budget to that one file**
//!   ([`the_remaining_chinese_all_lives_in_the_alias_table`]): an alias has to
//!   stay next to the command it belongs to, or the next author to add a
//!   command adds it without one. Moving them to a data file would have bought
//!   a zero and paid for it with that separation. Pinning is strictly tighter
//!   than a bare count — see [`ALIAS_TABLE`] for why that matters.
//! * **It misses copy in the other language.** The same file rendered
//!   `"Theme: Light"`, `"Navigation"` and `"Type a command or search…"` as
//!   hardcoded **English** — the identical defect, invisible here because the
//!   detector looks for Han characters. Those were localised in the same round
//!   that found them; nothing in this crate measures the class they belong to.
//!
//! The CJK that is genuinely *data* and stays that way — the sentence-splitter
//! fixtures in `views/voice/sentence.rs`, which feed Chinese text in on purpose
//! — lives under `#[cfg(test)]`, and [`production_lines`] stops there. This
//! file obeys the same cut: its own fixtures sit in the test module below, so
//! the scan reads zero from it without needing to know its own name.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Production lines in `src/` that hard-code Chinese copy.
///
/// **This number may only go down.** Lower it in the same commit that removes
/// copy; a sweep that leaves the ceiling above the new measurement hands the
/// next author that difference as free budget.
///
/// 2026-08-18, first measurement: **126** across 20 files, all of it under
/// `platform/wide/` and `components/` — the complement of what the phone
/// guard's reachability walk already holds at zero.
///
/// 2026-08-18, first sweep, same day: **126 -> 57** across 16 files.
/// `views/settings/network/cluster.rs` (24),
/// `components/project_page/settings.rs` (19), `views/settings/search.rs`
/// (18), `components/project_page.rs` (8) — every one of them now resolves
/// through `locales/{en,zh}.json`. The fourth file the sweep set out to clear,
/// `components/command_palette.rs` (11), turned out to hold no translatable
/// copy at all: its Chinese is bilingual search aliases (see the module doc),
/// so 11 of the remaining 57 will not come out this way and the effective
/// floor is 11, not 0.
///
/// 2026-08-18, second sweep: **57 -> 11** across 15 files — the whole
/// remainder except those aliases. Three of the 46 lines were not view copy
/// and could not simply take a `t!`, which is worth recording because the
/// shape recurs: `chat/plan.rs::archive_summary` and
/// `chat/transcript.rs::transcript_markdown` are pure host-testable functions
/// with no reactive owner to resolve a key from, and `voice/mod.rs`'s
/// repeated-miss caption is read on the far side of an `.await`. All three
/// took the copy as a parameter resolved by the caller — the component, which
/// has the context — rather than growing an i18n handle of their own.
///
/// This is the floor. What is left is [`ALIAS_TABLE`]'s bilingual `keywords:`,
/// which the module doc above already argues is data rather than copy;
/// lowering this number past 11 means changing what shape those aliases have,
/// not translating them.
const HARDCODED_CHINESE_LINE_CEILING: usize = 11;

/// The one file whose Chinese literals are match-only data, not copy.
///
/// A bare ceiling is a **fungible** budget: at 11-of-11 it still goes red on a
/// new copy literal, but only until someone deletes an alias — then the freed
/// line pays for a hardcoded sentence somewhere else and the guard stays green
/// through a real regression. Naming the file spends that budget in one place,
/// so the two events stop being interchangeable.
///
/// This is not the exemption list the module doc argues against. An exemption
/// permits something the rule forbids and has nothing to make it expire; this
/// only *narrows* where an already-counted line may sit, and the count still
/// has to be lowered by hand. A twelfth alias line reddens the ratchet exactly
/// like a twelfth sentence would.
const ALIAS_TABLE: &str = "components/command_palette.rs";

/// A character that only appears in this codebase inside Chinese copy.
///
/// Han ideographs plus the two punctuation blocks that travel with them
/// (`。，、（）` and the fullwidth forms). `…` is deliberately absent — it is
/// used in English strings here too, so flagging it would train the next
/// author to weaken the rule rather than obey it.
pub(crate) fn is_chinese(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3000}'..='\u{303F}' | '\u{FF01}'..='\u{FF60}')
}

/// Production lines of a source file: everything outside a `#[cfg(test)]`-gated
/// item, minus whole-line comments.
///
/// `\r` is stripped first. A `"\n#[cfg(test)]"` split matches nothing on a
/// CRLF checkout, which silently turns "production prefix" into "the whole
/// file" — the scanner then reads its own fixtures and reports them.
///
/// This walks *items* rather than cutting at the first marker, and that is not
/// a refinement — the cut version was blind. `#[cfg(test)]` is an attribute,
/// not a file position: ten files in this crate carry one above their trailing
/// test module (on a `use`, a helper `fn`, a gated `mod`), and everything below
/// it — **2 266 lines**, including all of `views/chat/state.rs` and
/// `views/chat/events.rs` — was silently excluded from both guards. It cost
/// nothing to close: re-measured item-wise, those 2 266 lines carry zero
/// hard-coded copy, so the count is the same 11 either way. That is the whole
/// argument for closing a blind spot the day you find it rather than writing
/// down that it exists.
///
/// # This is the crate's single answer, and why it is a second one
///
/// Every source-level guard in this crate that needs "production code only"
/// asks here — the copy census, `disposed_reads`'s window-listener guard,
/// `views/settings/network/cluster.rs`'s role-gate pin and
/// `views/canvas/shape_view.rs`'s iframe-sandbox pin. Each of those three
/// used to hand-roll `src.split("#[cfg(test)]").next()`, i.e. the blind cut
/// this function's doc above already argues against, and each was blind in
/// exactly the way described there.
///
/// The server crate answers the same question in
/// `alephcore::utils::source_scan::{production_prefix, cfg_test_portion}`,
/// which is more careful still (it lexes raw strings, char literals and block
/// comments across lines). **This crate cannot call it**: `aleph-panel` is a
/// wasm frontend and does not depend on `alephcore`, and adding that edge to
/// reach two functions would pull the whole server library into the frontend
/// (R1/R3). Moving the functions into a crate both already share was
/// considered and ruled out by the capability-wiring spec, non-goal 1 — 不拆
/// crate: `alephcore` stays a single crate this round. So this is a deliberate
/// second implementation of one question, kept to ONE per crate rather than
/// one per guard, and this doc is where that decision is recorded.
pub(crate) fn production_lines(src: &str) -> Vec<(usize, String)> {
    let src = src.replace('\r', "");
    let lines: Vec<&str> = src.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("#[cfg(test)]") {
            i = end_of_gated_item(&lines, i + 1);
            continue;
        }
        if !lines[i].trim_start().starts_with("//") {
            out.push((i + 1, lines[i].to_string()));
        }
        i += 1;
    }
    out
}

/// Index one past the item that starts at or after `from`.
///
/// A braced item (`mod tests { … }`, `fn helper() { … }`) ends where its braces
/// balance; a bare one (`mod tests;`, `use …;`) ends at its semicolon. Braces
/// inside string literals do not count — a fixture holding `"{"` would
/// otherwise close the block early and the scan would read the rest of the test
/// module as production code.
fn end_of_gated_item(lines: &[&str], from: usize) -> usize {
    let mut depth: i32 = 0;
    let mut opened = false;
    let mut i = from;
    while i < lines.len() {
        let code = outside_string_literals(lines[i]);
        depth += i32::try_from(code.matches('{').count()).unwrap_or(0);
        depth -= i32::try_from(code.matches('}').count()).unwrap_or(0);
        if code.contains('{') {
            opened = true;
        }
        i += 1;
        if opened && depth <= 0 {
            return i;
        }
        if !opened && code.trim_end().ends_with(';') {
            return i;
        }
    }
    i
}

/// `line` with the contents of its double-quoted literals removed, and any
/// trailing `//` comment cut off. Escapes (`\"`) are honoured; raw strings are
/// not — a `r#"…"#` holding an unbalanced brace is a known, visible limitation
/// (the guard reddens on a fixture rather than going quietly blind).
fn outside_string_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut prev = '\0';
    for c in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else {
            if c == '/' && prev == '/' {
                out.pop();
                break;
            }
            out.push(c);
        }
        prev = c;
    }
    out
}

/// 1-based line numbers of production lines that hard-code Chinese copy.
///
/// The `"` narrows this to literals: Chinese in a trailing comment is a
/// different rule (CLAUDE.md: comments are English) and not this guard's
/// business. Measured against the tree on 2026-08-18, that narrowing costs
/// nothing — no counted line carries its Chinese only in a comment.
pub(crate) fn offending_lines(src: &str) -> Vec<usize> {
    production_lines(src)
        .into_iter()
        .filter(|(_, text)| text.contains('"') && text.chars().any(is_chinese))
        .map(|(line, _)| line)
        .collect()
}

// ── The other language ──────────────────────────────────────────────────────
//
// Everything above looks for Han characters, which in this crate only ever
// appear in copy. English has no such tell: identifiers, CSS classes, RPC
// method names, route paths and SVG path data are all English string literals
// and none of them is copy. So this half cannot ask *what the literal says* —
// it asks **where the literal sits**, which in an RSX tree is decidable:
//
//  * a bare literal in **child position** is always painted, and
//  * a literal assigned to an attribute or prop a person reads is always
//    painted.
//
// Child position is read off the neighbours rather than off a `view!` block,
// because "inside `view!`" is far too coarse: `class=move || { "px-4 py-3 …" }`
// puts a Tailwind string on a line of its own inside the macro, and the first
// measurement counted 174 of those as copy. A text node has a tag on one side
// of it (`>` above, or `<` below); a class string inside a closure has a brace
// on both.
//
// A third rule joined them once the first two had been run against the tree:
//
//  * a literal that a **braced child expression** evaluates to is painted too.
//
// RSX wraps every Rust expression in `{…}`, so `{move || if saving.get() {
// "Saving..." } else { "Save" }}` paints as surely as `<span>"Save"</span>`
// does — the first two rules simply stop at the brace. Carrying on past it is
// not a matter of matching more patterns: see [`rendered_literals`], where the
// measured cost of matching those patterns *without* the position test is
// recorded.

/// Production lines in `src/` that hard-code English copy.
///
/// **This number may only go down**, on the same terms as
/// [`HARDCODED_CHINESE_LINE_CEILING`].
///
/// 2026-08-18, first measurement: **298** across 68 files. Worst:
/// `settings/channels/discord.rs` (29), `settings/moa/preset_editor.rs` (23),
/// `settings/skills.rs` (19), `settings/search.rs` (18).
///
/// This class was named in this module's own doc for a round before anything
/// measured it — `command_palette.rs` was found rendering `"Navigation"` and
/// `"Theme: Light"`, the Chinese guard was structurally blind to it, and the
/// note said so and stopped there. A named gap with no number is a gap that
/// grows.
///
/// The number was arrived at twice, by two implementations, and they disagreed
/// by one line. The scanner was the one that was wrong: it took the *first*
/// painted literal on a line, and on
/// `<span title="Default agent">"★"</span>` that is the star. A ratchet cannot
/// recover from a false negative the way it recovers from a false positive —
/// nothing ever reddens — so the second reading is what caught it, not the
/// tests. See `a_symbol_child_does_not_shadow_the_attribute_next_to_it`.
///
/// ## Both directions this proxy is wrong in
///
/// * **It counts things that should not be translated.** Brand names
///   (`"Discord"`, `"MoA"`, `"MP3"`), example values in placeholders
///   (`"e.g. dall-e-3, stable-diffusion-xl"`) and URL specimens
///   (`"https://api.example.com"`) all sit in painted position. A sweep will
///   have to decide those case by case, and the ones that stay are the floor —
///   the same shape as the Chinese side's search aliases.
/// * **It misses copy that is not in painted position.** `.unwrap_or("Never")`,
///   a `match` arm returning a bare `&str`, `format!("Loading {n} items")`,
///   and a literal inside `{if x { "Yes" } else { "No" }}` were all named here
///   as invisible. Three of the four now are not — see [`rendered_literals`]
///   for why the fix was to require *shape and position together*, and what is
///   still outside. This measures the tractable half; it does not claim to be
///   the class.
///
/// ## 2026-08-18, second measurement: 298 -> 332
///
/// **Nothing was added to the crate between those two numbers.** 34 lines that
/// were always there became visible when [`rendered_literals`] closed the
/// braced-child blind spot — `"Needs Setup ({})"`, `"New MoA preset"`,
/// `"Refreshing..."`, `"Latency: {ms}ms"`. This is the same event as the tool
/// description ratchet's 82,462 -> 93,358 and it deserves the same sentence:
/// the budget did not get spent, the instrument stopped being blind. Reading a
/// ratchet's history requires knowing which of the two happened, so the jump is
/// recorded here rather than folded into a sweep's arithmetic.
///
/// ## 2026-08-18, first English sweep: 332 -> 184
///
/// The ten worst files, and the sweep turned out to be mostly *wiring* rather
/// than authoring: **347 of the crate's 2 289 locale keys had no call site at
/// all**. `settings.channels` alone carried 54 of them — `bot_online`,
/// `perm_all_granted`, `validating`, `refresh` — every one already translated
/// in both locales while `channels/discord.rs` went on rendering the English
/// literal beside it. A key with no reader and a hard-coded string are the two
/// halves of the same defect, and only one half was visible before this ratchet
/// existed.
///
/// What stayed is the floor, and it is the same shape as the Chinese side's
/// search aliases — literals in painted position that are not copy:
///
/// * **brand and product names**: `"Discord"`, `"crawl4ai"`, `"Firecrawl"`;
/// * **codec and protocol tokens**: `"MP3"`, `"Opus"`, `"AAC"`, `"FLAC"`,
///   `"TTS"`;
/// * **URL specimens** in placeholders: `"https://api.example.com/search"`,
///   `"http://localhost:11235"`.
///
/// Example *values* stayed and their English *prose* did not — the precedent
/// was already in the tree: `settings.generation.model_placeholder` translates
/// `e.g.` to `例如` and leaves `dall-e-3, stable-diffusion-xl` alone. The
/// remaining 184 lines are the tail: 70 files, none above eight.
///
/// 2026-08-19: **184 -> 182**. Not a translation round — the plugin install
/// dialog's three-option source `<select>` was removed because nothing read
/// its value (all three branches sent the same git-clone RPC), and its two
/// surviving hardcoded placeholders went with it. Re-pinned rather than left
/// as slack: a ceiling above the measurement hands the next author that
/// difference for free.
const HARDCODED_ENGLISH_LINE_CEILING: usize = 182;

/// Human-facing names the derivation cannot see, because the crate has never
/// localised one.
///
/// [`localised_attributes`] answers "what does this crate treat as copy" by
/// reading its own `t_string!` sites, which is the right question and a
/// self-updating answer — but it is silent about an attribute that has been
/// hard-coded every single time. `alt` is exactly that: `<img alt="Bot avatar">`
/// is copy a screen reader speaks aloud, and there is not one `alt=t_string!`
/// in the tree to derive it from. A seed is only sound in that direction: it
/// widens what gets counted, never narrows it.
const HUMAN_FACING_SEED: &[&str] = &["alt"];

/// Attribute and component-prop names whose value a person reads.
///
/// Derived from the crate's own localisation sites plus [`HUMAN_FACING_SEED`].
/// Enumerating them by hand was the alternative and it would have been wrong on
/// the day it was written: the derivation turns up `label`, `hint` and
/// `confirm_label` — component props, not HTML attributes — which no list of
/// "human-facing HTML attributes" would have contained.
fn human_facing_attributes() -> BTreeSet<String> {
    let mut set = localised_attributes();
    set.extend(HUMAN_FACING_SEED.iter().map(|s| (*s).to_string()));
    set
}

/// Names this crate assigns a `t_string!` to — i.e. names it has already
/// decided carry copy.
fn localised_attributes() -> BTreeSet<String> {
    let root = crate::disposed_reads::src_dir();
    let mut set = BTreeSet::new();
    for path in crate::disposed_reads::rust_sources(&root) {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in src.replace('\r', "").lines() {
            if let Some(name) = localised_attribute_name(line) {
                set.insert(name);
            }
        }
    }
    set
}

/// `title=move || t_string!(…)` -> `Some("title")`; `let x = t_string!(…)` and
/// everything else -> `None`.
fn localised_attribute_name(line: &str) -> Option<String> {
    let line = line.trim_start();
    if line.starts_with("//") || line.starts_with("let ") {
        return None;
    }
    let name: String = line
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-')
        .collect();
    if name.is_empty() {
        return None;
    }
    let rest = line[name.len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix("move || ").unwrap_or(rest);
    rest.starts_with("t_string!").then_some(name)
}

/// 1-based line numbers of production lines that hard-code English copy.
pub(crate) fn english_copy_lines(src: &str, attrs: &BTreeSet<String>) -> Vec<usize> {
    let lines = production_lines(src);
    let trimmed: Vec<String> = lines.iter().map(|(_, l)| l.trim().to_string()).collect();
    let mut hits = Vec::new();
    for (i, (number, line)) in lines.iter().enumerate() {
        // Every painted literal on the line, not the first one: a line may
        // carry both, and the first match is not the copy. `<span
        // title="Default agent">"★"</span>` is one real line of this crate —
        // stopping at the child text node reads a star, decides it is not
        // copy, and never looks at the attribute that is.
        let mut painted = literals_between_tags(line);
        painted.extend(lone_child_literal(&trimmed, i));
        painted.extend(human_attribute_literals(line, attrs));
        for expr in braced_child_expressions(&trimmed, i) {
            painted.extend(rendered_literals(&expr));
        }
        if painted.iter().any(|t| looks_like_copy(t)) {
            hits.push(*number);
        }
    }
    hits
}

/// Contents of the brace expressions sitting in **child position** on this line.
///
/// RSX writes every Rust expression inside `{…}`, so a literal that reaches the
/// screen through `format!` or an `if` arm is painted just as surely as
/// `<span>"Save"</span>` — it is one brace further away, and the two rules
/// above stop at the brace. This is where they carry on.
///
/// Both spellings count: `<span>{…}</span>` on one line, and a `{` opening the
/// line with a tag on one side of it (the same neighbour test
/// [`lone_child_literal`] uses).
///
/// The scan is **balanced**, not to end-of-line, and that is not a nicety.
/// `<p class="…">{t!(i18n, k)}</p><p class="…">{…}</p>` is one real line of
/// this crate; running to the end of it collects the second `class=` and
/// reports a Tailwind string as copy.
fn braced_child_expressions(trimmed: &[String], i: usize) -> Vec<String> {
    let line = &trimmed[i];
    let mut out = Vec::new();
    for (at, _) in line.match_indices(">{") {
        out.push(balanced_from(line, at + 1));
    }
    if line.starts_with('{') {
        let prev = trimmed[..i].iter().rev().find(|l| !l.is_empty());
        let next = trimmed[i + 1..].iter().find(|l| !l.is_empty());
        let adjacent =
            prev.is_some_and(|l| l.ends_with('>')) || next.is_some_and(|l| l.starts_with('<'));
        if adjacent {
            out.push(balanced_from(line, 0));
        }
    }
    out
}

/// Inside of the brace expression opening at byte `at`, up to its matching
/// `}`. Braces inside string literals do not count; an expression that runs
/// past the end of the line yields the rest of it.
fn balanced_from(line: &str, at: usize) -> String {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, c) in line.char_indices().skip_while(|(i, _)| *i < at) {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                // A `}` before any `{` means `at` did not point at a brace;
                // yield nothing rather than underflow.
                if depth == 0 {
                    return String::new();
                }
                depth -= 1;
                if depth == 0 {
                    return line[at + 1..idx].to_string();
                }
            }
            _ => {}
        }
    }
    line[at + 1..].to_string()
}

/// The literals a braced expression actually *renders*, as opposed to the ones
/// it merely mentions.
///
/// This is the half of the census that the module doc used to record as
/// unreachable: `.unwrap_or("Never")`, a `match` arm returning a bare `&str`,
/// and `format!("Loading {n} items")` are all copy and none of them sits in
/// painted position. The note said they were invisible and stopped there.
///
/// They are invisible to **shape alone** — matching those three patterns across
/// the crate turns up **1 130** hits, of which the overwhelming majority are
/// not copy at all: `Self::Jina => "jina"` is a serialization tag,
/// `Self::Mauve => "oklch(0.60 0.13 310)"` is a colour, and
/// `format!("Failed to parse: {e}")` is a server error on its way to
/// `admin_refusal`. A ratchet fed that would be 90% noise, and the first
/// person to hit it would weaken the rule rather than obey it.
///
/// They are also invisible to **position alone** — that is the two rules above.
///
/// The conjunction is precise: a literal in one of those three shapes, *inside
/// a braced child expression*, is being painted. 34 lines, and reading all of
/// them turns up copy like `"Needs Setup ({})"`, `"New MoA preset"` and
/// `"Refreshing..."` that nothing in this crate could previously see.
///
/// What is still outside: the same three shapes anywhere else — a `match` in a
/// helper function whose `&str` is returned to a caller that paints it. That
/// needs to follow a value across a function boundary, which is a different
/// kind of program than this one, and the doc above still does not claim to
/// measure the class.
fn rendered_literals(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (start, end, text) in literals_with_span(content) {
        let before = content[..start].trim_end();
        let after = content[end..].trim_start();
        // `format!("…", …)` — the format string is the output.
        let is_format = ["format!(", "write!(", "writeln!("]
            .iter()
            .any(|m| before.ends_with(m));
        // `.unwrap_or("…")` / `.unwrap_or_else(|| "…")` — the fallback is what
        // gets painted precisely when there is nothing else to paint.
        let is_fallback = [".unwrap_or(", ".unwrap_or_else(", ".unwrap_or_else(||"]
            .iter()
            .any(|m| before.ends_with(m));
        // `{ "…" }` / `=> "…"` — an `if` or `match` arm evaluating to a literal.
        let is_arm = (before.ends_with('{') || before.ends_with("=>"))
            && (after.is_empty() || after.starts_with('}') || after.starts_with(','));
        if is_format || is_fallback || is_arm {
            out.push(text);
        }
    }
    out
}

/// Every `"…"` in `content` with the byte range it occupies, escapes honoured.
fn literals_with_span(content: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut idx = 0;
    let bytes = content.as_bytes();
    while idx < bytes.len() {
        if bytes[idx] == b'"' {
            if let Some(text) = read_literal(content, idx) {
                let end = idx + text.len() + 2;
                out.push((idx, end, text));
                idx = end;
                continue;
            }
            break;
        }
        idx += 1;
    }
    out
}

/// A literal alone on its line, with a tag on one side of it.
fn lone_child_literal(trimmed: &[String], i: usize) -> Option<String> {
    let text = read_literal(&trimmed[i], 0)?;
    if !trimmed[i][text.len() + 2..].trim().is_empty() {
        return None;
    }
    let prev = trimmed[..i].iter().rev().find(|l| !l.is_empty());
    let next = trimmed[i + 1..].iter().find(|l| !l.is_empty());
    let adjacent =
        prev.is_some_and(|l| l.ends_with('>')) || next.is_some_and(|l| l.starts_with('<'));
    adjacent.then_some(text)
}

/// Every `<span>"Save"</span>` text node on one line.
fn literals_between_tags(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    for (i, w) in bytes.windows(2).enumerate() {
        if w[0] == b'>' && w[1] == b'"' {
            let Some(text) = read_literal(line, i + 1) else {
                continue;
            };
            if line[i + 3 + text.len()..].starts_with('<') {
                out.push(text);
            }
        }
    }
    out
}

/// Every `title="Save"` on the line, for any name a person reads.
fn human_attribute_literals(line: &str, attrs: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for name in attrs {
        let mut from = 0;
        while let Some(rel) = line[from..].find(name.as_str()) {
            let at = from + rel;
            let boundary = at == 0 || {
                let c = line[..at].chars().next_back().unwrap_or(' ');
                !c.is_ascii_alphanumeric() && c != '_' && c != '-'
            };
            let rest = line[at + name.len()..].trim_start();
            if boundary && rest.starts_with("=\"") {
                let quote = line.len() - rest.len() + 1;
                if let Some(text) = read_literal(line, quote) {
                    out.push(text);
                }
            }
            from = at + name.len();
        }
    }
    out
}

/// Contents of the `"…"` starting at byte `at`, or `None` if one does not.
fn read_literal(line: &str, at: usize) -> Option<String> {
    let rest = line.get(at..)?.strip_prefix('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for c in rest.chars() {
        if escaped {
            out.push('\\');
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}

/// Does this literal read as a sentence rather than as a token?
///
/// Two ASCII letters, counted after two kinds of non-letter are collapsed:
///
/// * `\u{…}` escapes — the hex in `"\u{00B7}"` is letters to
///   `char::is_alphabetic` and the glyph it denotes is a middle dot, which is
///   punctuation in every language;
/// * `{…}` format placeholders — the identifier inside `"{prefix} {err}"` is a
///   *variable name*, and in this crate that variable is very often already
///   localised. `format!("{by_label} {agent} · {ts_label}")` assembles three
///   resolved strings and contributes no copy of its own; counting `by_label`
///   as five letters of English would send a sweep to translate a template
///   that has nothing to translate.
///
/// The second rule is what makes [`rendered_literals`] precise enough to be
/// worth having: it costs nothing on the literals this file already counted
/// (measured — none of the 298 carries a placeholder) and removes 19 of the 53
/// lines the braced-child rule would otherwise have added, all of them pure
/// assembly like `"@{name}"`, `"v{v}"` and `"{completed}/{total}"`.
fn looks_like_copy(text: &str) -> bool {
    let mut collapsed = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'u') {
            chars.next();
            if chars.peek() == Some(&'{') {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '{' {
            // `{{` is an escaped brace, i.e. a literal `{` in the output.
            if chars.peek() == Some(&'{') {
                chars.next();
                collapsed.push('{');
                continue;
            }
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
            }
            continue;
        }
        collapsed.push(c);
    }
    collapsed.chars().filter(char::is_ascii_alphabetic).count() >= 2
}

/// Per-file English counts across the crate, omitting files already clean.
fn english_census() -> Vec<(PathBuf, usize)> {
    let root = crate::disposed_reads::src_dir();
    let attrs = human_facing_attributes();
    let mut rows: Vec<(PathBuf, usize)> = crate::disposed_reads::rust_sources(&root)
        .into_iter()
        .filter_map(|path| {
            let src = std::fs::read_to_string(&path).ok()?;
            let hits = english_copy_lines(&src, &attrs).len();
            (hits > 0).then_some((path, hits))
        })
        .collect();
    rows.sort_by_key(|(path, n)| (std::cmp::Reverse(*n), path.clone()));
    rows
}

/// Per-file counts across the crate, omitting files that are already clean.
fn census() -> Vec<(PathBuf, usize)> {
    let root = crate::disposed_reads::src_dir();
    let mut rows: Vec<(PathBuf, usize)> = crate::disposed_reads::rust_sources(&root)
        .into_iter()
        .filter_map(|path| {
            let src = std::fs::read_to_string(&path).ok()?;
            let hits = offending_lines(&src).len();
            (hits > 0).then_some((path, hits))
        })
        .collect();
    rows.sort_by_key(|(path, n)| (std::cmp::Reverse(*n), path.clone()));
    rows
}

/// Does `line` contain a string literal that OPENS with `#[cfg(test)]`?
///
/// Leading `\n` / `\r\n` escapes are stepped over, because the line-anchored
/// spelling (`find("\n#[cfg(test)]")`) is the same cut with a CRLF story
/// attached. Anything where the attribute sits further into the literal is
/// prose or a fixture — an assertion message saying "the #[cfg(test)] split
/// matched nothing" is not a second cut — so the offset is the whole
/// discriminator, and it needs no list of method names to stay accurate when
/// someone reaches for `splitn` or `match_indices` next.
///
/// Every `"` on the line is tried, including closing ones. That over-matches
/// in the loud direction only: the worst it can do is flag a literal that
/// genuinely starts with the attribute, which is the thing being flagged.
fn opens_a_cfg_test_literal(line: &str) -> bool {
    const ATTR: &str = "#[cfg(test)]";
    let bytes = line.as_bytes();
    for (quote, _) in line.match_indices('"') {
        let mut j = quote + 1;
        while j + 1 < bytes.len() && bytes[j] == b'\\' && matches!(bytes[j + 1], b'n' | b'r') {
            j += 2;
        }
        if line[j..].starts_with(ATTR) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector itself, on input the tree no longer contains.
    ///
    /// Without this the ratchet goes green the day `is_chinese` or
    /// `production_lines` stops matching anything, and a scanner that sees
    /// nothing is indistinguishable from a clean tree.
    #[test]
    fn the_detector_still_recognises_what_it_removed() {
        let sample = "let a = \"保存中…\";\n// 这行是注释\nlet b = \"Save\";\n";
        assert_eq!(
            offending_lines(sample),
            vec![1],
            "detector missed the literal or ate the comment"
        );
    }

    /// A `#[cfg(test)]` above the trailing test module does not blind the rest
    /// of the file.
    ///
    /// The cut-at-the-first-marker version this replaced answered `[]` here:
    /// line 3 is production copy sitting below a gated `mod` declaration, which
    /// is how 2 266 lines of this crate went unscanned. Line 6 must stay out —
    /// closing the blind spot must not start reporting test fixtures.
    #[test]
    fn a_gated_item_above_the_test_module_hides_only_itself() {
        let sample = concat!(
            "#[cfg(test)]\n",
            "mod helper;\n",
            "let a = \"保存中…\";\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    const X: &str = \"测试用例\";\n",
            "}\n",
        );
        assert_eq!(
            offending_lines(sample),
            vec![3],
            "the scan either stopped at the first #[cfg(test)] or read past the \
             test module",
        );
    }

    /// A brace inside a fixture string does not close the test module early.
    #[test]
    fn a_string_literal_brace_does_not_end_the_gated_item() {
        let sample = concat!(
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    const A: &str = \"}\";\n",
            "    const B: &str = \"测试\";\n",
            "}\n",
            "let c = \"保存\";\n",
        );
        assert_eq!(
            offending_lines(sample),
            vec![6],
            "a closing brace inside a literal ended the block, so fixtures were scanned",
        );
    }

    /// CRLF does not turn the production prefix into the whole file.
    #[test]
    fn the_test_module_is_cut_off_on_a_crlf_checkout() {
        let sample = "let a = \"ok\";\r\n#[cfg(test)]\r\nmod t { const X: &str = \"保存\"; }\r\n";
        assert!(
            offending_lines(sample).is_empty(),
            "the #[cfg(test)] cut missed on CRLF, so the scanner reads test fixtures",
        );
    }

    /// Sanity: the walk reaches a real source tree.
    ///
    /// Deliberately not pinned to any file that currently carries copy — those
    /// are exactly what a sweep will empty, so a check anchored there would
    /// fail for the same reason as the ratchet and stop being an independent
    /// signal.
    #[test]
    fn the_scan_reaches_the_source_tree() {
        let files = crate::disposed_reads::rust_sources(&crate::disposed_reads::src_dir());
        assert!(
            files.len() > 100,
            "the walk found {} source files — it is broken, not the crate",
            files.len()
        );
    }

    /// Where the floor is allowed to sit.
    ///
    /// Falsified by planting `let _ = "预算";` in another file: this reddens
    /// naming that file, while the ratchet above stays green whenever an alias
    /// line was removed in the same change — which is the whole reason this
    /// assertion is separate from the count.
    #[test]
    fn the_remaining_chinese_all_lives_in_the_alias_table() {
        let root = crate::disposed_reads::src_dir();
        let strays: Vec<String> = census()
            .into_iter()
            .map(|(path, n)| {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                (rel, n)
            })
            .filter(|(rel, _)| rel.replace('\\', "/") != ALIAS_TABLE)
            .map(|(rel, n)| format!("{rel} ({n})"))
            .collect();

        assert!(
            strays.is_empty(),
            "hard-coded Chinese copy outside {ALIAS_TABLE}: {}.\n\
             The remaining budget belongs to that file's bilingual search \
             aliases, which are matched against and never rendered. Copy \
             anywhere else goes in locales/{{zh,en}}.json and is read with \
             t!/t_string! — deleting an alias does not buy room for it.",
            strays.join(", "),
        );
    }

    /// The English detector, on the shapes it must and must not read.
    ///
    /// The `class=` arm is the one that matters: it is a lone literal inside a
    /// `view!` block, and counting it is what the first draft of this scan did
    /// 174 times.
    #[test]
    fn the_english_detector_reads_position_not_vocabulary() {
        let attrs: BTreeSet<String> = ["title", "placeholder"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let sample = concat!(
            "<span class=\"x\">\n",
            "    \"Navigation\"\n",
            "</span>\n",
            "<button title=\"Save changes\">\n",
            "<p>\"Inline copy\"</p>\n",
            "class=move || {\n",
            "    \"px-4 py-3 border-b-2 text-sm\"\n",
            "}\n",
            "<span>\"\\u{00B7}\"</span>\n",
        );
        assert_eq!(
            english_copy_lines(sample, &attrs),
            vec![2, 4, 5],
            "position test drifted: 2/4/5 are painted, 7 is a class string and \
             9 is a middle dot",
        );
    }

    /// A non-copy text node does not hide the copy attribute beside it.
    ///
    /// The real line: `<span class="…" title="Default agent">"★"</span>`. An
    /// `or_else` chain answers "★", decides that is not copy, and the line goes
    /// uncounted — a false *negative*, which is the direction a ratchet cannot
    /// recover from on its own.
    #[test]
    fn a_symbol_child_does_not_shadow_the_attribute_next_to_it() {
        let attrs: BTreeSet<String> = ["title"].into_iter().map(str::to_string).collect();
        let sample = "<span class=\"x\" title=\"Default agent\">\"★\"</span>\n";
        assert_eq!(english_copy_lines(sample, &attrs), vec![1]);
    }

    /// The derivation is not silently empty.
    ///
    /// `human_facing_attributes` is what decides which attribute values the
    /// English scan even looks at. If `localised_attribute_name` stops parsing
    /// — a formatting change, a macro rename — the set collapses to the seed,
    /// the count drops, and a `<=` ratchet calls that success. A scanner that
    /// sees nothing is indistinguishable from a clean tree.
    #[test]
    fn the_human_facing_attribute_set_is_really_derived() {
        let derived = localised_attributes();
        for expected in ["title", "placeholder", "aria-label", "label"] {
            assert!(
                derived.contains(expected),
                "`{expected}=t_string!` exists in this crate but the derivation \
                 missed it — it found {derived:?}",
            );
        }
        assert!(
            derived.len() >= 4,
            "the derivation collapsed to {derived:?}; the English census is now \
             reading almost nothing and will pass for that reason",
        );
    }

    #[test]
    fn localised_attribute_name_ignores_a_let_binding() {
        assert_eq!(
            localised_attribute_name("    title=move || t_string!(i18n, k).to_string()"),
            Some("title".to_string()),
        );
        assert_eq!(
            localised_attribute_name("    let label = t_string!(i18n, k).to_string();"),
            None,
            "a local binding is not an attribute; counting it fills the set with \
             variable names",
        );
    }

    /// The braced-child rule, on all three shapes and on what must stay out.
    ///
    /// Lines 2/4/6 are painted through a brace; 8 is the same `format!` in a
    /// *helper*, which this rule deliberately does not reach (see
    /// `rendered_literals`); 10 carries a `class=format!(…)` on its *second*
    /// element, which only a scan that ran past its own closing brace would
    /// reach — and which is a Tailwind string, not copy.
    #[test]
    fn a_braced_child_expression_is_painted_position_too() {
        let attrs: BTreeSet<String> = BTreeSet::new();
        let sample = concat!(
            "<span>\n",
            "    {move || if saving.get() { \"Saving...\" } else { \"Save\" }}\n",
            "</span>\n",
            "<span>{format!(\"Needs Setup ({})\", n)}</span>\n",
            "<p>\n",
            "    {label.clone().unwrap_or(\"Untitled\")}\n",
            "</p>\n",
            "fn helper() -> String { format!(\"Needs Setup ({})\", n) }\n",
            "<div>\n",
            "<p>{n}</p><p class=format!(\"px-4 {x}\")>{m}</p>\n",
        );
        assert_eq!(
            english_copy_lines(sample, &attrs),
            vec![2, 4, 6],
            "the brace hop drifted: 2/4/6 are painted, 8 is a helper the rule \
             does not claim to reach, and 10's Tailwind string sits past the \
             closing brace of the expression before it",
        );
    }

    /// A literal a braced expression merely *mentions* is not painted.
    ///
    /// `{seg(store.kind_filter, "all", t_string!(…))}` is a real line: `"all"`
    /// is the filter key it compares against, and the copy beside it is already
    /// localised. An unqualified "any literal inside the braces" rule counts
    /// the key and sends a sweep to translate it.
    #[test]
    fn an_argument_inside_the_braces_is_not_painted() {
        let attrs: BTreeSet<String> = BTreeSet::new();
        let sample = concat!(
            "<div>\n",
            "    {seg(store.kind_filter, \"all\", t_string!(i18n, k).to_string())}\n",
            "</div>\n",
        );
        assert!(
            english_copy_lines(sample, &attrs).is_empty(),
            "a lookup key was counted as copy",
        );
    }

    /// A template that only assembles already-resolved strings carries no copy.
    ///
    /// Falsified by deleting the `{…}` arm of `looks_like_copy`: every one of
    /// these reads as several letters of English, and a sweep sent after them
    /// finds nothing to translate.
    #[test]
    fn a_placeholder_only_template_is_not_copy() {
        for assembly in [
            "{by_label} {agent} · {ts_label}",
            "@{name}",
            "v{v}",
            "{completed}/{total}",
            "{next_prefix}{relative}",
        ] {
            assert!(
                !looks_like_copy(assembly),
                "{assembly:?} is assembly of resolved parts, not copy",
            );
        }
        for copy in ["Needs Setup ({})", "{m} members", "Latency: {ms}ms"] {
            assert!(looks_like_copy(copy), "{copy:?} is copy and was skipped");
        }
    }

    /// `{{` is an escaped brace — a literal `{` in the output, not a placeholder.
    #[test]
    fn an_escaped_brace_is_not_a_placeholder() {
        assert!(looks_like_copy("use {{ and }} to escape"));
    }

    /// The balanced scan stops at its own closing brace.
    #[test]
    fn a_brace_expression_does_not_run_to_the_end_of_the_line() {
        assert_eq!(balanced_from("<p>{a}</p><p class=\"x\">{b}</p>", 3), "a");
        assert_eq!(balanced_from("{outer {inner} rest} tail", 0), "outer {inner} rest");
        assert_eq!(
            balanced_from("{\"}\" is a literal}", 0),
            "\"}\" is a literal",
            "a brace inside a string literal closed the expression early",
        );
    }

    #[test]
    fn hardcoded_english_line_ratchet() {
        let rows = english_census();
        let total: usize = rows.iter().map(|(_, n)| n).sum();

        let root = crate::disposed_reads::src_dir();
        let worst: Vec<String> = rows
            .iter()
            .take(8)
            .map(|(path, n)| {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                format!("{} ({n})", rel.display())
            })
            .collect();

        assert!(
            total <= HARDCODED_ENGLISH_LINE_CEILING,
            "hard-coded English copy grew to {total} lines across {} files \
             (ceiling {HARDCODED_ENGLISH_LINE_CEILING}).\n  worst: {}\n\
             This copy renders the same for a reader who picked Chinese — the \
             mirror image of the Chinese ratchet, and the same fix: put it in \
             locales/{{zh,en}}.json and read it with t!/t_string!. If you are \
             here because you *removed* copy, lower \
             HARDCODED_ENGLISH_LINE_CEILING to {total} in this same commit.",
            rows.len(),
            worst.join(", "),
        );
    }

    #[test]
    fn hardcoded_chinese_line_ratchet() {
        let rows = census();
        let total: usize = rows.iter().map(|(_, n)| n).sum();

        let root = crate::disposed_reads::src_dir();
        let worst: Vec<String> = rows
            .iter()
            .take(8)
            .map(|(path, n)| {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                format!("{} ({n})", rel.display())
            })
            .collect();

        assert!(
            total <= HARDCODED_CHINESE_LINE_CEILING,
            "hard-coded Chinese copy grew to {total} lines across {} files \
             (ceiling {HARDCODED_CHINESE_LINE_CEILING}).\n  worst: {}\n\
             This copy renders the same for a reader who picked English. Put it \
             in locales/{{zh,en}}.json and read it with t!/t_string!. If you are \
             here because you *removed* copy, lower \
             HARDCODED_CHINESE_LINE_CEILING to {total} in this same commit.",
            rows.len(),
            worst.join(", "),
        );
    }
    /// One author for "where does production code end", crate-wide.
    ///
    /// [`production_lines`] is that author. Three guards used to hand-roll
    /// `src.split("#[cfg(test)]").next()` instead — `disposed_reads`'s
    /// window-listener rule, `views/settings/network/cluster.rs`'s role-gate
    /// pin, `views/canvas/shape_view.rs`'s iframe-sandbox pin — and all three
    /// were blind in the direction that reports success: a prefix cut can only
    /// ever UNDER-scan, so a `#[cfg(test)]` on anything above the trailing test
    /// module truncated the file there and everything below went unseen. That
    /// is the same defect [`production_lines`]'s own doc records costing 2 266
    /// lines on the copy census. All three now delegate; this guard is what
    /// keeps the fourth one from being written.
    ///
    /// # What this scan does NOT reach
    ///
    /// It is textual and it keys on ONE property: a string literal whose first
    /// characters are the attribute. Indirection through a named constant is
    /// still caught — the `const ATTR: &str = "#[cfg(test)]";` line is itself a
    /// literal opening with the attribute — but a needle assembled from pieces
    /// (`concat!`, two constants joined) is not, and neither is a cut that
    /// never spells the attribute at all, e.g. one that searches for
    /// `"mod tests {"`. Named rather than approximated: closing those needs
    /// the value flow, and a guard that states what it cannot see is worth more
    /// than one that implies it sees everything.
    ///
    /// # The one exemption
    ///
    /// `components/admin_refusal.rs` bounds a single function's BODY (`the
    /// next \npub fn `, with `\n#[cfg(test)]` as a fallback for the last one
    /// in the file), not a file's production region. Its failure mode is a
    /// window that runs long, not a scan that stops early, and it is named
    /// here rather than swallowed by a narrower predicate — a predicate that
    /// stopped matching it would stop matching the real thing too. The size is
    /// pinned so this exemption cannot grow into a licence.
    #[test]
    fn no_guard_in_this_crate_hand_rolls_the_cfg_test_cut() {
        let root = crate::disposed_reads::src_dir();
        let mut files = crate::disposed_reads::rust_sources(&root);
        // `rust_sources` drops `disposed_reads.rs` deliberately: its RED
        // fixtures are the exact shape ITS OWN rule forbids. That reason does
        // not apply to this rule, and that file is one of the three this guard
        // exists to hold in place, so put it back explicitly rather than fork
        // the walker into a second answer to "where is this crate's source".
        let disposed = root.join("disposed_reads.rs");
        assert!(
            disposed.is_file(),
            "disposed_reads.rs moved — this guard was silently not scanning \
             one of the three files it was written for"
        );
        files.push(disposed);
        assert!(
            files.len() > 300,
            "only {} sources — the walk is broken, not the code",
            files.len()
        );

        let mut offenders = Vec::new();
        for path in &files {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string()
                .replace('\\', "/");
            if rel == "i18n_census.rs" {
                continue; // defines the replacement; its fixtures are the old shape
            }
            let Ok(raw) = std::fs::read_to_string(path) else {
                continue;
            };
            for (n, line) in raw.replace('\r', "").split('\n').enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if opens_a_cfg_test_literal(line) {
                    offenders.push(format!("{rel}:{}", n + 1));
                }
            }
        }

        let (exempted, rest): (Vec<String>, Vec<String>) = offenders
            .into_iter()
            .partition(|o| o.starts_with("components/admin_refusal.rs:"));
        assert!(
            rest.is_empty(),
            "these cut production code at the first `#[cfg(test)]` marker \
             instead of calling `i18n_census::production_lines`, which walks \
             gated ITEMS. The cut only under-scans, so it reports a clean pass \
             for whatever it could not see:\n  {}",
            rest.join("\n  ")
        );
        assert_eq!(
            exempted.len(),
            1,
            "the admin_refusal exemption matched {} lines, not exactly one: \
             {exempted:?}. It is named for ONE call site that bounds a \
             function body rather than a file's production region. A second \
             one is a decision for a human to write here explicitly — name it, \
             do not widen the exemption to swallow it.",
            exempted.len()
        );
    }

    /// The detector, on both shapes and on the prose it must not flag.
    #[test]
    fn the_cfg_test_literal_detector_reads_the_offset_not_the_method() {
        assert!(opens_a_cfg_test_literal(r##"    .split("#[cfg(test)]")"##));
        assert!(opens_a_cfg_test_literal(
            r##"    let p = src.split("#[cfg(test)]\nmod ").next();"##
        ));
        assert!(opens_a_cfg_test_literal(
            r##"    .or_else(|| body[1..].find("\n#[cfg(test)]"))"##
        ));
        assert!(opens_a_cfg_test_literal(
            r##"    .position(|l| l.starts_with("#[cfg(test)]"))"##
        ));
        // Prose and fixtures: the attribute is inside the literal, not at its
        // start. Flagging these would make the guard an allowlist maintenance
        // job within a round.
        assert!(!opens_a_cfg_test_literal(
            r##"    "the #[cfg(test)] split matched nothing — this test would be","##
        ));
        assert!(!opens_a_cfg_test_literal(
            r##"    let src = "pub fn a() {}\n#[cfg(test)]\nmod t {}";"##
        ));
        assert!(!opens_a_cfg_test_literal("#[cfg(test)]"));
        assert!(!opens_a_cfg_test_literal("mod tests {"));
    }
}
