//! Census: no lib test reaches a `$HOME`-rooted Claude scan without moving
//! `$HOME` or switching the Claude root off (P5.11).
//!
//! The round's rule is "never read the developer's real `~/.claude`". A lib
//! test that does is a standing violation and is non-hermetic: its verdict
//! depends on what is installed on the machine running it. Two guards hold the
//! rule, and each sees what the other cannot:
//!
//! - this source scan, which fails at the CALL SITE in test code, on every
//!   machine, whether or not that machine has a `~/.claude`; and
//! - the runtime tripwire `utils::paths::assert_not_real_claude_home`, called
//!   where production code resolves the Claude root into something it will
//!   list or read (`default_skill_dirs`, `DirectoryScanner::new`,
//!   `discover_claude_cache`, `SkillSystem::rescan_dirs`, `skill_list`,
//!   `skill_read`). It catches the indirect paths this scan is blind to — a
//!   tool `call` that reaches `ensure_shared_skill_system_initialized` three
//!   frames down, or a real root another test left registered in the
//!   process-wide shared `SkillSystem` — at the cost of firing only on the
//!   path a test actually executes.
//!
//! # What this scan recognises
//!
//! Over [`test_text`] (inline `#[cfg(test)]` code AND whole-file test
//! modules) with comments and literal payloads blanked ([`code_text`]):
//!
//! - an ENTRY is a line naming one of [`ENTRY_POINTS`] — the constructors and
//!   resolvers that turn `$HOME` into a Claude root that is then scanned;
//! - a REDIRECT is a line naming one of [`REDIRECTS`] — `$HOME` moved to a
//!   tempdir with `HomeEnvGuards::acquire_and_set`, an explicit
//!   `claude_home_override: Some(..)`, or `scan_claude_dirs: false`;
//! - a fn's verdict is over its brace-matched body. A same-file helper whose
//!   body carries a REDIRECT counts as a redirect where it is called (the
//!   `hermetic_home()` / `isolated_home()` shape); a same-file helper whose
//!   body carries an ENTRY and no REDIRECT counts as an entry where it is
//!   called (the `manager_with_project_plugin(..)` shape). One level of
//!   indirection, same file only. "Called" means an UNQUALIFIED call: a
//!   qualified one (`Owner::helper(`) does not count — in Rust a qualified
//!   path names an associated fn or a method, never a free helper.
//!
//! A TEST fn — one carrying a recognised test attribute (`#[test]`,
//! `#[tokio::test]`, `#[rstest]`, `#[test_case]`, `#[quickcheck]`) — whose
//! body (with that one level of helpers) has an ENTRY and no REDIRECT is a
//! violation. So is a reaching, non-redirecting helper that no test in its
//! own file calls: its callers live elsewhere, and this scan will not guess
//! whether they redirect.
//!
//! # What it does NOT recognise (判据 §3: a green covers only these shapes)
//!
//! - Helper indirection deeper than one level, or across files (a test in
//!   `a.rs` calling a reaching helper in `b.rs` is not charged to that test;
//!   the helper itself is reported as uncalled-in-file, so it cannot hide).
//! - Production indirection: a test that calls a tool, handler or service
//!   whose production body reaches an entry point (`HubCatalogSearchTool::call`
//!   → `collect_installed` → `ensure_shared_skill_system_initialized`). That
//!   is the tripwire's half.
//! - Macros that expand to an entry point, and entry points spelled through a
//!   `use` alias or a fn pointer.
//! - A redirect that is held for only PART of the body (a guard dropped
//!   before the entry), or one that redirects to the real home: presence of
//!   the marker in the body is the whole check.
//! - Stat-only reaches: `get_all_skills_dirs` / `user_skills_dirs` test
//!   `$HOME/.claude/skills` for `is_dir()` and return the path; they are not
//!   ENTRY points here because no content is read on that call. Tests that
//!   assert on their output are still machine-dependent in principle — see
//!   the P5.11 report's concerns.
//! - `$HOME/.agents`, which `DirectoryScanner::new` resolves regardless of
//!   `scan_claude_dirs`: out of this rule's `~/.claude` scope.
//! - A test attribute outside the list above: a macro that expands to a test
//!   fn (`#[my_test_macro]`), or a parameterised-test attribute spelled some
//!   other way. Such a fn is filed as a HELPER here, not as a test.
//! - The drift from `utils::source_scan::scan_test_bodies`, the shared
//!   implementation of the two questions this file re-derives ("which fn is
//!   a test" and "where does its body end"): the shared scanner keeps every
//!   attribute TAG attached to a test (`TestBody::attrs`), while this copy
//!   reduces the attributes to one boolean, so the parameterised-test
//!   attributes had to be named here separately. Reconciling the two is a
//!   follow-up, not this change.

#[cfg(test)]
mod tests {
    use crate::utils::source_scan::{code_text, rust_sources_under, test_text};

    /// Calls that resolve `$HOME` into a Claude root that is then listed or
    /// read. `ExtensionManager::new` and the two discovery constructors take a
    /// `DiscoveryConfig`, so a body that builds one with the Claude root off
    /// carries the `scan_claude_dirs: false` REDIRECT next to them.
    const ENTRY_POINTS: &[&str] = &[
        "ExtensionManager::with_defaults(",
        "ExtensionManager::new(",
        "DiscoveryManager::new(",
        "DirectoryScanner::new(",
        "default_skill_dirs(",
        "ensure_shared_skill_system_initialized(",
    ];

    const REDIRECTS: &[&str] = &[
        "HomeEnvGuards::acquire_and_set(",
        "claude_home_override: Some(",
        "scan_claude_dirs: false",
    ];

    /// One `fn` in a file's test code, brace-matched.
    struct FnBody {
        name: String,
        is_test: bool,
        entry: bool,
        redirect: bool,
        calls: Vec<String>,
    }

    fn ident_after_fn(line: &str) -> Option<String> {
        let at = line.find("fn ")?;
        if at > 0 {
            let before = line[..at].chars().next_back()?;
            if before.is_ascii_alphanumeric() || before == '_' {
                return None;
            }
        }
        let name: String = line[at + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    }

    /// Whether `line` calls `name(` as a whole identifier (not `x_name(`, not
    /// its own `fn name(` definition).
    ///
    /// A QUALIFIED call — `Owner::name(` or `owner.name(` — is NOT a call to
    /// the helper `name`. In Rust a qualified path never names a free
    /// function (`Owner::name(` is an associated fn, `owner.name(` a method),
    /// so the helper cannot be what they call; and where the census cannot
    /// prove the owner, it reports the file. The collision is not
    /// hypothetical:
    /// `src/gateway/execution_engine/slash_command_body/tests.rs` defines a
    /// redirecting helper named `new` (its body carries `scan_claude_dirs:
    /// false`) and also calls `TempDir::new(`, `Arc::new(` and
    /// `PluginRecord::new(`. Under a rule that accepts any `name(`, every one
    /// of those constructors was read as a call to that helper, so a test in
    /// that file was credited with the redirect on the strength of an
    /// unrelated `X::new(` — a false green on the exact rule this census
    /// exists to enforce. A qualified call therefore never counts: where the
    /// census cannot prove the owner, it reports the file.
    fn calls(line: &str, name: &str) -> bool {
        let needle = format!("{name}(");
        let mut from = 0;
        while let Some(at) = line.get(from..).and_then(|r| r.find(&needle)) {
            let start = from + at;
            let before = line[..start].chars().next_back();
            let is_def = line[..start].trim_end().ends_with("fn");
            let is_qualified = before.is_some_and(|c| c == ':' || c == '.');
            if !is_def
                && !is_qualified
                && !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return true;
            }
            from = start + needle.len();
        }
        false
    }

    /// Every fn in `code` with its own ENTRY/REDIRECT verdict. Nested fns are
    /// listed on their own and also lie inside their parent's body.
    ///
    /// DUPLICATES `utils::source_scan::scan_test_bodies`, the shared
    /// implementation (the pty census uses it). The two answer the same two
    /// questions — "which fn is a test" and "where does its body end" — and
    /// they can drift, because they are two answers to one fact: the shared
    /// scanner keeps every attribute TAG attached to a test
    /// (`TestBody::attrs`), while this copy reduces the attributes to one
    /// boolean, so the parameterised-test attributes had to be named here
    /// separately. Reconciling them needs the shared scanner to grow, which
    /// is a file this change does not own — a follow-up, not a silent
    /// assumption that the two agree.
    fn fn_bodies(code: &str) -> Vec<FnBody> {
        let lines: Vec<&str> = code.lines().collect();
        let mut out = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let Some(name) = ident_after_fn(l) else {
                continue;
            };
            let mut above = i;
            let mut is_test = false;
            while above > 0 && lines[above - 1].trim().starts_with('#') {
                above -= 1;
                is_test |= is_test_attribute(lines[above].trim());
            }
            let (mut depth, mut opened, mut end) = (0i32, false, None);
            for (k, b) in lines.iter().enumerate().skip(i) {
                if !opened && b.contains(';') && !b.contains('{') {
                    break; // a declaration (`fn x();`), no body
                }
                depth += i32::try_from(b.matches('{').count()).unwrap_or(0);
                depth -= i32::try_from(b.matches('}').count()).unwrap_or(0);
                opened |= b.contains('{');
                if opened && depth <= 0 {
                    end = Some(k);
                    break;
                }
            }
            let Some(end) = end else {
                continue;
            };
            let body = &lines[i..=end];
            out.push(FnBody {
                name,
                is_test,
                entry: body
                    .iter()
                    .any(|b| ENTRY_POINTS.iter().any(|n| b.contains(n))),
                redirect: body.iter().any(|b| REDIRECTS.iter().any(|n| b.contains(n))),
                calls: body.iter().map(|b| (*b).to_string()).collect(),
            });
        }
        out
    }

    /// Whether an attribute line marks the fn it is attached to as a TEST.
    ///
    /// `#[test]` and `#[tokio::test]` are the spellings this repo uses
    /// today; the parameterised-test attributes are named too, so a
    /// `#[rstest]` / `#[test_case]` / `#[quickcheck]` fn is charged as a
    /// test rather than filed as a helper. None of the three is in use under
    /// `src/` today (checked 2026-10-31), so this is future-proofing rather
    /// than a live fix — but it is the fix for the shape, which is what a
    /// census is cited for.
    fn is_test_attribute(attr: &str) -> bool {
        attr == "#[test]"
            || attr.starts_with("#[tokio::test")
            || attr.starts_with("#[rstest")
            || attr.starts_with("#[test_case")
            || attr.starts_with("#[quickcheck")
    }

    /// The census over one file's test code: violations, plus the number of
    /// test fns it found reaching an entry point (redirected or not).
    fn census_file(path: &str, code: &str) -> (Vec<String>, usize) {
        let fns = fn_bodies(code);
        let helpers = |pred: &dyn Fn(&FnBody) -> bool| -> Vec<String> {
            fns.iter()
                .filter(|f| !f.is_test && pred(f))
                .map(|f| f.name.clone())
                .collect()
        };
        let redirecting = helpers(&|f| f.redirect);
        let reaching = helpers(&|f| f.entry && !f.redirect);
        let body_calls = |f: &FnBody, set: &[String]| {
            set.iter()
                .any(|h| *h != f.name && f.calls.iter().any(|l| calls(l, h)))
        };

        let mut violations = Vec::new();
        let mut reaching_tests = 0;
        for f in fns.iter().filter(|f| f.is_test) {
            let entry = f.entry || body_calls(f, &reaching);
            if !entry {
                continue;
            }
            reaching_tests += 1;
            if !(f.redirect || body_calls(f, &redirecting)) {
                violations.push(format!(
                    "{path}: test `{}` reaches a $HOME-rooted Claude scan without redirecting \
                     $HOME or turning the Claude root off",
                    f.name
                ));
            }
        }
        for h in fns.iter().filter(|f| !f.is_test && f.entry && !f.redirect) {
            let called = fns
                .iter()
                .any(|t| t.is_test && t.calls.iter().any(|l| calls(l, &h.name)));
            if !called {
                violations.push(format!(
                    "{path}: helper `{}` reaches a $HOME-rooted Claude scan without a \
                     redirect and no test in this file calls it, so this census cannot see \
                     whether its callers redirect. Redirect inside it, or move it next to them",
                    h.name
                ));
            }
        }
        (violations, reaching_tests)
    }

    /// Every test that reaches a `$HOME`-rooted Claude scan redirects first.
    ///
    /// Mutation red (P5.11 report): dropping the `HomeEnvGuards` redirect from
    /// `builtin_tools::skill_status`'s test turns this red, naming it.
    #[test]
    fn no_lib_test_reaches_the_real_claude_home() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        let mut reaching_tests = 0usize;
        let mut reaching_files = Vec::new();
        for (path, src) in rust_sources_under(&root) {
            if !ENTRY_POINTS.iter().any(|n| src.contains(n)) {
                continue;
            }
            let code = code_text(&test_text(std::path::Path::new(&path), &src));
            let (v, n) = census_file(&path, &code);
            if n > 0 {
                reaching_files.push(path.clone());
            }
            reaching_tests += n;
            violations.extend(v);
        }
        assert!(
            violations.is_empty(),
            "lib tests that can read the developer's real ~/.claude (P5.11). Redirect with \
             `runtimes::post_install::HomeEnvGuards::acquire_and_set(aleph_tmp, home_tmp)`, set \
             `DiscoveryConfig::claude_home_override`, or set `scan_claude_dirs: false`:\n  {}",
            violations.join("\n  ")
        );
        // A scan that finds nothing passes vacuously.
        for known in [
            "src/extension/mod.rs",
            // The scanner's tests moved into their own module file
            // (`src/discovery/scanner/tests.rs`) when the inline test module
            // was split out; the reaching test lives there now.
            "src/discovery/scanner/tests.rs",
            "src/builtin_tools/skill_status.rs",
            "src/gateway/execution_engine/run_loop/tests.rs",
        ] {
            assert!(
                reaching_files.iter().any(|p| p.ends_with(known)),
                "the scan no longer sees {known} reaching an entry point: the call moved \
                 (update this list) or the scanner broke. Found: {reaching_files:?}"
            );
        }
        assert!(
            reaching_tests >= 20,
            "only {reaching_tests} test fns found reaching an entry point directly or through a \
             same-file reaching helper; at P5.11 there were 23. A scan that finds nothing \
             passes vacuously"
        );
    }

    /// The scan's own shapes, on fixtures: each recognised shape both ways.
    #[test]
    fn census_recognises_its_documented_shapes() {
        let run = |src: &str| census_file("f.rs", &code_text(src)).0;

        // Direct entry, no redirect: red. IsolatedAlephHome alone is not one.
        let v = run(
            "#[test]\nfn t() {\n    let _h = IsolatedAlephHome::new();\n    \
                     let m = ExtensionManager::with_defaults();\n}\n",
        );
        assert_eq!(v.len(), 1, "{v:?}");
        // Direct entry, inline redirect: green.
        assert!(run(
            "#[test]\nfn t() {\n    let _g = HomeEnvGuards::acquire_and_set(a, b);\n    \
                     let d = default_skill_dirs();\n}\n"
        )
        .is_empty());
        // Entry with the Claude root off in the same body: green.
        assert!(run(
            "#[tokio::test]\nasync fn t() {\n    let c = DiscoveryConfig {\n        \
                     scan_claude_dirs: false,\n        ..Default::default()\n    };\n    \
                     let m = DiscoveryManager::new(c);\n}\n"
        )
        .is_empty());
        // Redirect through a same-file helper: green; without the call: red.
        let helper = "fn hermetic_home() -> G {\n    HomeEnvGuards::acquire_and_set(a, b)\n}\n";
        assert!(run(&format!(
            "{helper}#[test]\nfn t() {{\n    let _h = hermetic_home();\n    \
             ExtensionManager::with_defaults();\n}}\n"
        ))
        .is_empty());
        assert_eq!(
            run(&format!(
                "{helper}#[test]\nfn t() {{\n    ExtensionManager::with_defaults();\n}}\n"
            ))
            .len(),
            1
        );
        // Entry through a same-file helper: charged to the calling test.
        let reacher = "async fn mk() -> M {\n    ExtensionManager::with_defaults().await\n}\n";
        let v = run(&format!(
            "{reacher}#[tokio::test]\nasync fn t() {{\n    let m = mk().await;\n}}\n"
        ));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("test `t`"), "{v:?}");
        assert!(run(&format!(
            "{reacher}#[tokio::test]\nasync fn t() {{\n    \
             let _g = HomeEnvGuards::acquire_and_set(a, b);\n    let m = mk().await;\n}}\n"
        ))
        .is_empty());
        // A reaching helper no test in the file calls: reported, not guessed.
        let v = run(reacher);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("helper `mk`"), "{v:?}");
        // A mention in a comment or a string is not an entry.
        assert!(run(
            "#[test]\nfn t() {\n    // ExtensionManager::with_defaults()\n    \
                     let s = \"default_skill_dirs()\";\n}\n"
        )
        .is_empty());
    }

    /// A QUALIFIED call is not a call to the helper: `X::new(` must not
    /// stand in for a redirecting helper named `new`.
    ///
    /// The false green this pins: `slash_command_body/tests.rs` defines a
    /// redirecting helper named `new` (its body carries `scan_claude_dirs:
    /// false`) and also calls `TempDir::new(`, `Arc::new(` and
    /// `PluginRecord::new(`. Under a rule that accepts any `name(`, each of
    /// those constructors was read as a call to that helper, so a test in
    /// that file was credited with the redirect on the strength of an
    /// unrelated `X::new(` — and was never reported.
    #[test]
    fn a_qualified_call_is_not_a_call_to_the_helper() {
        let run = |src: &str| census_file("f.rs", &code_text(src)).0;
        let helper = "impl Fixture {\n    fn new() -> Self {\n        let c = \
                      DiscoveryConfig { scan_claude_dirs: false, ..Default::default() };\n        \
                      let _ = c;\n        Self\n    }\n}\n";
        // The test reaches an entry point itself, and its only call to the
        // redirecting helper is QUALIFIED — so it does not carry the
        // helper's redirect.
        let v = run(&format!(
            "{helper}#[test]\nfn t() {{\n    let _f = Fixture::new();\n    \
             let m = ExtensionManager::with_defaults();\n}}\n"
        ));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("test `t`"), "{v:?}");
        // An UNQUALIFIED call to the same helper still counts as the
        // redirect it is.
        assert!(run(&format!(
            "{helper}#[test]\nfn t() {{\n    let _f = new();\n    \
             let m = ExtensionManager::with_defaults();\n}}\n"
        ))
        .is_empty());
    }

    /// A parameterised-test attribute marks a TEST: a `#[rstest]` fn that
    /// reaches an entry point without redirecting is reported as a test, not
    /// filed as a helper.
    #[test]
    fn a_parameterised_test_attribute_marks_a_test() {
        let run = |src: &str| census_file("f.rs", &code_text(src)).0;
        for attr in ["#[rstest]", "#[test_case]", "#[quickcheck]"] {
            let v = run(&format!(
                "{attr}\nfn t() {{\n    let m = ExtensionManager::with_defaults();\n}}\n"
            ));
            assert_eq!(v.len(), 1, "{attr}: {v:?}");
            assert!(v[0].contains("test `t`"), "{attr}: {v:?}");
        }
    }
}
