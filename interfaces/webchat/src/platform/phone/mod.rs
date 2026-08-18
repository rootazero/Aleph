//! iPhone (iOS-native) layer — panel-only, connects to a remote core.
//!
//! Screens are rebuilt 1:1 from the aleph-mobile design system
//! (`docs/design-system/aleph-mobile/screens/exported/*.dc.html` +
//! `styles/aleph.css`): Chat / Memory / Agents / Settings / Voice /
//! Notifications. iOS component classes (`.cell` / `.list` / `.cell-leading`
//! / `.tabbar` / `.swatch` …) are ported into `styles/ios.css`; shared data
//! hooks reuse the crate-root `api` / `state`.
//!
//! Isolated from [`super::wide`] by construction — phone code never touches the
//! desktop/browser UI. Screens are added in subsequent steps.

pub mod agents;
pub mod alerts;
pub mod canvas;
pub mod chat;
pub mod dashboard;
pub mod extensions;
pub mod memory;
pub mod more;
pub mod settings;
pub mod shell;
pub mod teams;

#[cfg(test)]
mod i18n_census {
    use crate::disposed_reads::{rust_sources, src_dir};

    /// A character that only appears in this codebase inside Chinese copy.
    ///
    /// Han ideographs plus the two punctuation blocks that travel with them
    /// (`。，、（）` and the fullwidth forms). `…` is deliberately absent — it is
    /// used in English strings here too, so flagging it would train the next
    /// author to weaken the rule rather than obey it.
    fn is_chinese(c: char) -> bool {
        matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3000}'..='\u{303F}' | '\u{FF01}'..='\u{FF60}')
    }

    /// Production half of a source file: everything before its test module,
    /// minus whole-line comments.
    ///
    /// `\r` is stripped first. A `"\n#[cfg(test)]"` split matches nothing on a
    /// CRLF checkout, which silently turns "production prefix" into "the whole
    /// file" — the scanner then reads its own fixtures and reports them.
    fn production_lines(src: &str) -> Vec<(usize, String)> {
        let src = src.replace('\r', "");
        let head = src.split("#[cfg(test)]").next().unwrap_or(&src).to_string();
        head.lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_string()))
            .filter(|(_, l)| !l.trim_start().starts_with("//"))
            .collect()
    }

    /// No file *under `platform/phone/`* ships a hard-coded Chinese string.
    ///
    /// Fifty-three literals across seven files were converted on 2026-08-18;
    /// this stops the fifty-fourth, because a new screen written the old way
    /// looks exactly like the six that were already wrong. Copy belongs in
    /// `locales/{zh,en}.json` and reaches the view through `t!` / `t_string!`,
    /// which `leptos_i18n` checks at compile time — a missing key is a build
    /// error, not a silent fallback.
    ///
    /// ⚠️ **Read the scope in the name literally.** This is a source-file rule,
    /// not a rendered-screen rule, and the gap between them is observable
    /// today: `/settings/appearance` on a phone still renders eight Chinese
    /// words (`跟随系统` / `明亮` / `奢华磨砂` / …) because they come from
    /// `ThemeMode::label()` and friends in `crate::appearance`, a shared module
    /// outside this walk. The real-machine QA caught that while this test was
    /// green — which is the whole failure mode: **a guard's green only covers
    /// the files its walk reaches.**
    ///
    /// Widening it to the crate is not possible yet, and the number says why:
    /// **170 such literals across 25 files** as of 2026-08-18, nearly all in
    /// `platform/wide/` and `components/`. So the honest boundary is this
    /// directory, stated in the name, rather than a crate-wide rule carrying a
    /// 25-entry exemption list that nothing would ever shrink.
    #[test]
    fn no_file_under_platform_phone_hardcodes_chinese_copy() {
        let root = src_dir().join("platform").join("phone");
        let files = rust_sources(&root);
        assert!(
            files.len() > 20,
            "found {} phone sources — the walk is broken, not the code",
            files.len(),
        );

        let mut offenders = Vec::new();
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            for (line, text) in production_lines(&src) {
                // A `"` narrows this to literals; Chinese in a trailing comment
                // is a different rule (CLAUDE.md: comments are English) and not
                // this guard's business.
                if text.contains('"') && text.chars().any(is_chinese) {
                    let rel = path.strip_prefix(src_dir()).unwrap_or(path);
                    offenders.push(format!("{}:{line}", rel.display()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "hard-coded Chinese copy under platform/phone/ — move it to \
             locales/{{zh,en}}.json and read it with t!/t_string!:\n  {}",
            offenders.join("\n  "),
        );
    }

    /// The detector itself, on input the tree no longer contains.
    ///
    /// Without this, `no_phone_screen_hardcodes_chinese_copy` goes green the
    /// day `is_chinese` or `production_lines` stops matching anything, and a
    /// scanner that sees nothing is indistinguishable from a clean tree.
    #[test]
    fn the_detector_still_recognises_what_it_removed() {
        let sample = "let a = \"保存中…\";\n// 这行是注释\nlet b = \"Save\";\n";
        let hits: Vec<usize> = production_lines(sample)
            .into_iter()
            .filter(|(_, t)| t.contains('"') && t.chars().any(is_chinese))
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            hits,
            vec![1],
            "detector missed the literal or ate the comment"
        );
    }

    /// CRLF does not turn the production prefix into the whole file.
    #[test]
    fn the_test_module_is_cut_off_on_a_crlf_checkout() {
        let sample = "let a = \"ok\";\r\n#[cfg(test)]\r\nmod t { const X: &str = \"保存\"; }\r\n";
        assert!(
            !production_lines(sample)
                .iter()
                .any(|(_, t)| t.chars().any(is_chinese)),
            "the #[cfg(test)] cut missed on CRLF, so the scanner reads test fixtures",
        );
    }
}
