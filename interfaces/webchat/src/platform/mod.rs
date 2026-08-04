//! Per-form-factor UI layers.
//!
//! Each platform owns its own screens / navigation / layout. Shared data
//! (`api`, `state`, `context`), design tokens, and leaf components live at the
//! crate root and are reused by every platform — only the presentation layer
//! diverges per device. This keeps iPhone/iPad work physically isolated from
//! the desktop/browser UI: code in `phone`/`tablet` cannot reach into `wide`.
//!
//! - [`wide`]   — desktop app + browser (one wide UI; only the shell differs)
//! - [`phone`]  — iPhone, iOS-native, panel-only (remote core)
//! - [`tablet`] — iPad (future), panel-only

pub mod phone;
pub mod tablet;
pub mod wide;

/// The two form-factor branches share one reactive `Owner`, so a phone screen
/// must never provide a context type a desktop view reads.
///
/// `MainContent` swaps `PhoneX` ↔ `WideX` inside a dynamic child. A
/// `#[component]` is a plain function call with no `Owner` of its own (only
/// `#[island]` gets one), so a `provide_context` inside either branch lands in
/// the *branch's* owner — the one shared with the other branch. That owner is
/// re-run through `Owner::with_cleanup`, and `reactive_graph`'s `cleanup()`
/// takes `cleanups`, `nodes` and `children` but **not `contexts`**: the signals
/// are disposed, the context entry pointing at them survives. The next branch
/// is then built under that same owner and `expect_context` hands it the
/// corpse — a panic in the middle of `Render::build`, on a plain window resize.
///
/// `PhoneTeams` re-provided `TeamsTabState` (already owned by `AppContent`) and
/// that is exactly what happened: widening past 640 px crashed the panel every
/// time. See `platform::phone::teams` for the full write-up.
///
/// Only this direction is checkable. The mirror — a *wide* component providing
/// something a phone file reads — has a legitimate form, because phone screens
/// reuse desktop components and are therefore sometimes genuinely *inside* a
/// wide provider's subtree (`board.rs` provides `KanbanDnd` for its own
/// children, and the phone Kanban leaf is one of them). A phone screen
/// providing for a desktop view has no such excuse: they are siblings, never
/// ancestor and descendant.
#[cfg(test)]
mod context_ownership {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    fn platform_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform")
    }

    /// Every `.rs` file under `root`. Walking the tree (rather than
    /// `include_str!`) is deliberate: a file added later must be covered
    /// without anyone remembering this test exists.
    fn rust_sources(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    /// The type name an expression constructs, or `None` when the expression is
    /// a plain variable. Handles the four shapes in the tree: `Type { .. }`,
    /// `Type(..)`, `a::b::Type::new(..)` and a bare `Type`.
    fn type_of_expr(expr: &str) -> Option<String> {
        let head: String = expr
            .trim()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        head.split("::")
            .filter(|seg| seg.chars().next().is_some_and(char::is_uppercase))
            .last()
            .map(str::to_string)
    }

    /// Types handed to `provide_context` in one file, resolving a variable
    /// argument through its `let` binding in the same file.
    ///
    /// Returns `Err` for a site it cannot resolve. A guard that shrugs at input
    /// it does not understand is a guard that stops guarding — silently, and
    /// exactly when someone writes the shape it was built to catch.
    fn provided_types(src: &str) -> Result<BTreeSet<String>, String> {
        let mut bindings: BTreeMap<&str, String> = BTreeMap::new();
        for line in src.lines() {
            let Some(rest) = line.trim_start().strip_prefix("let ") else {
                continue;
            };
            let Some((name, value)) = rest.split_once(" = ") else {
                continue;
            };
            if let Some(ty) = type_of_expr(value) {
                bindings.insert(name.trim(), ty);
            }
        }

        let mut out = BTreeSet::new();
        for (idx, _) in src.match_indices("provide_context(") {
            let arg = &src[idx + "provide_context(".len()..];
            if let Some(ty) = type_of_expr(arg) {
                out.insert(ty);
                continue;
            }
            let var: String = arg
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            match bindings.get(var.as_str()) {
                Some(ty) => {
                    out.insert(ty.clone());
                }
                None => {
                    return Err(format!(
                        "cannot resolve the type provided at `provide_context({var}…)` — \
                         name the type at the call site so this guard can see it"
                    ))
                }
            }
        }
        Ok(out)
    }

    /// Types read out of context in one file.
    fn consumed_types(src: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for opener in ["expect_context::<", "use_context::<"] {
            for (idx, _) in src.match_indices(opener) {
                let rest = &src[idx + opener.len()..];
                if let Some(end) = rest.find('>') {
                    if let Some(ty) = rest[..end].rsplit("::").next() {
                        out.insert(ty.trim().to_string());
                    }
                }
            }
        }
        out
    }

    fn scan(sub: &str, f: fn(&str) -> BTreeSet<String>) -> BTreeSet<String> {
        rust_sources(&platform_dir().join(sub))
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .flat_map(|src| f(&src))
            .collect()
    }

    /// Sanity: the scrape found real source. Without this the assertion below
    /// passes vacuously the day the layout moves or the crate is built from a
    /// different manifest dir.
    #[test]
    fn the_scan_reaches_the_source_tree() {
        assert!(
            rust_sources(&platform_dir().join("phone")).len() > 10,
            "found almost no phone sources — the walk is broken, not the code"
        );
        // Deliberately NOT `TeamsTabState`: that is the type under test, so
        // pinning the sanity check to it makes this test fail for the same
        // reason as the real one and stops being an independent signal.
        // `DashboardState` is read by every screen in both trees.
        assert!(
            consumed_types(
                &std::fs::read_to_string(platform_dir().join("phone/teams/mod.rs")).unwrap()
            )
            .contains("DashboardState"),
            "the consumer scrape no longer sees a known expect_context call"
        );
        assert!(
            scan("wide", consumed_types).len() > 5,
            "the desktop consumer scrape came back nearly empty — the walk is broken"
        );
    }

    #[test]
    fn every_provide_context_in_the_phone_tree_resolves() {
        for path in rust_sources(&platform_dir().join("phone")) {
            let src = std::fs::read_to_string(&path).unwrap();
            if let Err(e) = provided_types(&src) {
                panic!("{}: {e}", path.display());
            }
        }
    }

    #[test]
    fn no_phone_screen_provides_a_context_the_desktop_reads() {
        // `expect`, not `unwrap_or_default`: a site this scanner cannot read
        // must fail *this* test too. Letting it contribute an empty set would
        // make the check pass by not looking — the precise failure this guard
        // exists to prevent, reproduced inside the guard.
        let provided = rust_sources(&platform_dir().join("phone"))
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .flat_map(|src| provided_types(&src).expect("unreadable provide_context site"))
            .collect::<BTreeSet<_>>();
        let consumed = scan("wide", consumed_types);
        let clash: Vec<_> = provided.intersection(&consumed).collect();
        assert!(
            clash.is_empty(),
            "phone screens provide {clash:?}, which desktop views read from context. \
             Both branches share one Owner and `cleanup()` does not clear contexts, \
             so crossing the 640 px line hands the desktop view disposed signals and \
             panics mid-render. Read that type from its existing owner instead of \
             providing a second copy."
        );
    }

    /// RED proof: the pre-fix shape must fail. `PhoneTeams` built its own
    /// `TeamsTabState` and provided it while `TeamsView` read the same type.
    #[test]
    fn the_clash_check_rejects_the_shape_that_crashed() {
        let phone_before = "
            let tab_state = TeamsTabState {
                sub_tab: RwSignal::new(TeamsSubTab::Overview),
            };
            provide_context(tab_state);
        ";
        let wide = "let tab_state = expect_context::<TeamsTabState>();";
        let provided = provided_types(phone_before).unwrap();
        assert!(provided.contains("TeamsTabState"));
        assert!(!provided.is_disjoint(&consumed_types(wide)));
    }

    /// …and does not fire on a phone-private context, which is the whole point
    /// of intersecting with what the desktop actually reads.
    #[test]
    fn the_clash_check_allows_a_phone_private_context() {
        let phone = "
            let st = PhoneMemoryState { page: RwSignal::new(0) };
            provide_context(st);
        ";
        let wide = "let chat = expect_context::<ChatState>();";
        assert!(provided_types(phone)
            .unwrap()
            .is_disjoint(&consumed_types(wide)));
    }

    /// An unreadable call site is a failure, not a pass.
    #[test]
    fn an_unresolvable_provide_site_fails_the_guard() {
        assert!(provided_types("provide_context(whatever_this_is);").is_err());
    }
}
