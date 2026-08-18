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
    use crate::i18n_census::offending_lines;

    /// Every crate module a phone module names in a `use crate::…` line,
    /// resolved one hop at module granularity.
    ///
    /// `use crate::appearance::{read_mode, ThemeMode}` resolves to
    /// `appearance.rs`; `use crate::components::ui::SwatchButton` resolves to
    /// `components/mod.rs` **and** `components/ui/mod.rs`. Paths that name no
    /// file (an item, a re-export, a macro) simply contribute nothing.
    ///
    /// ⚠️ **One hop, on purpose.** Following imports transitively reaches most
    /// of the crate within two or three, which would turn this guard into a
    /// crate-wide rule carrying an exemption list that nothing would shrink —
    /// the shape this file already argued against. One hop is the boundary
    /// where the set stays closed by fixing it rather than by exempting it.
    /// A screen that renders Chinese sourced two hops away is still invisible
    /// here; that is a stated limit, not an oversight.
    ///
    /// It also over-approximates in the other direction, and that is the safe
    /// direction: `platform/phone/chat/history.rs` imports one *function* from
    /// `components/chat_sidebar.rs`, so the whole module counts as reachable
    /// even though no phone screen renders its views. The cost is translating
    /// a string the phone may never show; item-granular resolution would cost
    /// a name resolver, and its failure mode would be a silent miss.
    fn directly_imported_modules(phone_files: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
        let root = src_dir();
        let mut found = std::collections::BTreeSet::new();
        for file in phone_files {
            let Ok(src) = std::fs::read_to_string(file) else {
                continue;
            };
            for line in src.lines() {
                let Some(rest) = line.trim_start().strip_prefix("use crate::") else {
                    continue;
                };
                // `a::b::{C, D}` / `a::b::C;` / `a::b as x;` -> ["a", "b"]
                let head: String = rest
                    .chars()
                    .take_while(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == ':'
                    })
                    .collect();
                let segments: Vec<&str> = head.split("::").filter(|seg| !seg.is_empty()).collect();
                for depth in 1..=segments.len() {
                    let base = segments[..depth]
                        .iter()
                        .fold(root.clone(), |p, s| p.join(s));
                    for cand in [base.with_extension("rs"), base.join("mod.rs")] {
                        if cand.is_file() {
                            found.insert(cand);
                        }
                    }
                }
            }
        }
        found.into_iter().collect()
    }

    /// Nothing a phone screen renders is hard-coded Chinese — including copy
    /// that lives outside `platform/phone/`.
    ///
    /// # Why the scope is reachability and not a directory
    ///
    /// The first version of this test asked *where the literal is written*.
    /// It was green on 2026-08-18 while `/settings/appearance` rendered eight
    /// Chinese words on a phone, because those came from
    /// `ThemeMode::label()` and friends in `crate::appearance` — a shared
    /// module one `use` away and one directory up. **A guard's green only
    /// covers the files its walk reaches**, and a walk defined by a directory
    /// answers a question nobody asked: the user sees a screen, not a path.
    ///
    /// So the walk now follows `use crate::…` out of `platform/phone/` (one
    /// hop — see `directly_imported_modules` for why not further). Closing
    /// that gap took three files: `appearance.rs` (24 literals, now
    /// `label(i18n)` reading `appearance.*`), `components/settings_sidebar.rs`
    /// (one settings group added after the i18n pass), and
    /// `memory_graph/markdown_excerpt.rs` (CJK test fixtures, already excluded
    /// by `production_lines`).
    ///
    /// Copy belongs in `locales/{zh,en}.json` and reaches the view through
    /// `t!` / `t_string!`, which `leptos_i18n` checks at compile time — a
    /// missing key is a build error, not a silent fallback.
    ///
    /// ⚠️ Still **not** a crate-wide rule — but the complement is no longer
    /// unmeasured. The copy outside this walk's reach (126 lines across
    /// `platform/wide/` and `components/` as of 2026-08-18) is held by
    /// [`crate::i18n_census`]'s ratchet, which shares this guard's detector so
    /// the two cannot drift apart on what "Chinese copy" means. Zero here,
    /// only-shrinking there.
    #[test]
    fn no_module_a_phone_screen_reaches_hardcodes_chinese_copy() {
        let root = src_dir().join("platform").join("phone");
        let phone = rust_sources(&root);
        assert!(
            phone.len() > 20,
            "found {} phone sources — the walk is broken, not the code",
            phone.len(),
        );

        let imported = directly_imported_modules(&phone);
        assert!(
            imported.iter().any(|p| p.ends_with("appearance.rs")),
            "the import walk resolved no `crate::appearance` — it is broken, and \
             a walk that reaches nothing is indistinguishable from a clean tree",
        );

        let mut files = phone;
        files.extend(imported);
        files.sort_unstable();
        files.dedup();

        let mut offenders = Vec::new();
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            for line in offending_lines(&src) {
                let rel = path.strip_prefix(src_dir()).unwrap_or(path);
                offenders.push(format!("{}:{line}", rel.display()));
            }
        }
        assert!(
            offenders.is_empty(),
            "hard-coded Chinese copy on a phone screen's reachable path — move it \
             to locales/{{zh,en}}.json and read it with t!/t_string!:\n  {}",
            offenders.join("\n  "),
        );
    }
}
