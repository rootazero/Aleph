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
///   and a literal inside `{if x { "Yes" } else { "No" }}` are all copy and all
///   invisible here. This measures the tractable half; it does not claim to be
///   the class.
const HARDCODED_ENGLISH_LINE_CEILING: usize = 298;

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
        if painted.iter().any(|t| looks_like_copy(t)) {
            hits.push(*number);
        }
    }
    hits
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
/// Two ASCII letters, counted after `\u{…}` escapes are collapsed — the hex in
/// `"\u{00B7}"` is letters to `char::is_alphabetic` and the glyph it denotes is
/// a middle dot, which is punctuation in every language.
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
}
