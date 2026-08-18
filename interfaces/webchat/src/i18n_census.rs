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
//!   *break* search. They are not exempted (an exemption list is the thing
//!   this guard exists without), so the ceiling has a floor of 11 rather than
//!   0. When a sweep reaches that floor, the decision to make then — with the
//!   whole tree clean and real information in hand — is what shape those
//!   aliases should take so the scan can tell them from copy. Not before.
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
const HARDCODED_CHINESE_LINE_CEILING: usize = 57;

/// A character that only appears in this codebase inside Chinese copy.
///
/// Han ideographs plus the two punctuation blocks that travel with them
/// (`。，、（）` and the fullwidth forms). `…` is deliberately absent — it is
/// used in English strings here too, so flagging it would train the next
/// author to weaken the rule rather than obey it.
pub(crate) fn is_chinese(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3000}'..='\u{303F}' | '\u{FF01}'..='\u{FF60}')
}

/// Production half of a source file: everything before its test module, minus
/// whole-line comments.
///
/// `\r` is stripped first. A `"\n#[cfg(test)]"` split matches nothing on a
/// CRLF checkout, which silently turns "production prefix" into "the whole
/// file" — the scanner then reads its own fixtures and reports them.
pub(crate) fn production_lines(src: &str) -> Vec<(usize, String)> {
    let src = src.replace('\r', "");
    let head = src.split("#[cfg(test)]").next().unwrap_or(&src).to_string();
    head.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.to_string()))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
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
