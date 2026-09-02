//! Concrete health checks registered with the [`DiagnosticEngine`](super::DiagnosticEngine).
//!
//! Each submodule is one diagnostic domain and owns both its detection and
//! (where safe) its mechanical repair. Add a new domain by implementing
//! [`HealthCheck`](super::HealthCheck) here and registering it in
//! [`super::DiagnosticEngine::default_registry`] (OCP — no engine changes) —
//! **unless it cannot answer from a cold process**, in which case it belongs
//! on one of the opt-in builders instead. `default_registry` is what the
//! offline `aleph-server doctor` builds, and a check that always fires there
//! turns that command's exit code into a constant; see that function's doc.
//!
//! Answering "is this path there?" is [`super::check::Presence`]'s job, never
//! `Path::exists()` — see the `presence_discipline` test module at the foot of
//! this file for why, and for the guard that keeps it that way.

pub mod browser_runtime;
pub mod cache_health;
pub mod cache_hit_rate;
pub mod capability_wiring;
pub mod config_parse;
pub mod data_dir;
pub mod disk_space;
pub mod duplicate_instance;
pub mod hooks_consent;
pub mod idle_extensions;
pub mod loop_graph;
pub mod media_codecs;
pub mod projection_holes;
pub mod providers_connectivity;
pub mod sqlite_integrity;
pub mod stale_lock;
pub mod vault;

pub use browser_runtime::BrowserRuntimeCheck;
pub use cache_health::CacheHealthCheck;
pub use cache_hit_rate::CacheHitRateCheck;
pub use capability_wiring::CapabilityWiringCheck;
pub use config_parse::ConfigParseCheck;
pub use data_dir::DataDirCheck;
pub use disk_space::DiskSpaceCheck;
pub use duplicate_instance::DuplicateInstanceCheck;
pub use hooks_consent::HooksConsentCheck;
pub use idle_extensions::IdleExtensionsCheck;
pub use loop_graph::LoopGraphCheck;
pub use media_codecs::MediaCodecsCheck;
pub use projection_holes::ProjectionHolesCheck;
pub use providers_connectivity::ProvidersConnectivityCheck;
pub use sqlite_integrity::SqliteIntegrityCheck;
pub use stale_lock::StaleLockCheck;
pub use vault::VaultCheck;

#[cfg(test)]
mod presence_discipline {
    use crate::utils::source_scan::{
        code_text, production_code_lines, production_prefix, rust_sources_under,
    };

    /// Spellings that answer "is it there?" with `false` for BOTH "it is not
    /// there" and "the filesystem would not tell me".
    ///
    /// Paired with the replacement a reader is supposed to reach for, because
    /// a rule that only forbids is a rule people work around.
    const CONFLATING: [(&str, &str); 4] = [
        (
            ".exists()",
            "`check::Presence::of(ID, \"<subject>\", path)?` — it returns the third \
             answer as an `Err(Finding)` you cannot spend as absence by accident",
        ),
        (
            "read_dir",
            "`check::DirListing::of(ID, \"<subject>\", dir)?` — it separates \"the \
             directory is not there\" from \"the directory would not open\", and counts \
             entries the walk could not read",
        ),
        // Two markers, one rule: `Err(_` reaches `Err(_)`, `Err(_e)`,
        // `Err(_err)` and anything else whose binding starts with `_`, and
        // `Err(..)` is the remaining spelling of the same discard. Measured
        // before widening: zero occurrences of either in this directory's
        // production halves, so covering all of them costs nothing — and a
        // rule that only knew the one spelling its author happened to meet
        // would be tighter in the doc than in the tree.
        (
            "Err(_",
            "a bound error — `Err(e)`, not `Err(_)` / `Err(_e)` / `Err(..)` — and one \
             arm per error that actually MEANS absence, everything else through \
             `check::unknown_finding`. A discarded error cannot be told apart from the \
             answer the check then invents",
        ),
        (
            "Err(..)",
            "a bound error — `Err(e)` — and one arm per error that actually MEANS \
             absence, everything else through `check::unknown_finding`. A discarded \
             error cannot be told apart from the answer the check then invents",
        ),
    ];

    /// Does `line` use `marker` as a token, rather than as the tail of a longer
    /// identifier?
    ///
    /// Whether a left boundary is required is DERIVED from the marker itself
    /// rather than carried as a per-entry flag, so a marker added later cannot
    /// forget to declare which kind it is: a marker that opens with an
    /// identifier character needs one, a marker that opens with punctuation
    /// does not. `.exists()` opens with `.`, so `path.exists()` must still
    /// match even though `h` precedes the dot. `Err(_` and `read_dir` open with
    /// a letter, so `ParseErr(_)` and a hypothetical `spread_dir` must not.
    ///
    /// This exists because the widened `Err(` marker fired on
    /// `enum FakeEnum { ParseErr(u32) }` — no `Result`, no discarded error,
    /// nothing to do with the rule. Measured across `src/`, three real
    /// occurrences of that lexical shape exist today (`UnwrapErr(SysRng)`, in
    /// `gateway/security/{crypto,canvas_caps,artifact_caps}.rs`), none of them
    /// inside this directory — so the class is real rather than hypothetical,
    /// and tightening changes nothing about today's verdict.
    ///
    /// A rule that is LOOSER in the tree than in its doc is worse than one that
    /// is tighter: a guard that can fire on innocent code gets edited around by
    /// whoever it blocks, or gets cited as evidence for something it did not
    /// see. Both cost more than missing a spelling.
    ///
    /// All occurrences on the line are considered, not just the first — a
    /// `match` arm list can hold an innocent lookalike and a real offender on
    /// one line.
    fn uses_marker(line: &str, marker: &str) -> bool {
        fn ident(c: char) -> bool {
            c.is_alphanumeric() || c == '_'
        }
        let needs_left_boundary = marker.chars().next().is_some_and(ident);
        line.match_indices(marker).any(|(at, _)| {
            !needs_left_boundary || line[..at].chars().next_back().is_none_or(|c| !ident(c))
        })
    }

    /// The guard's one permanent NEGATIVE case: proof it stays quiet when it
    /// should, not just that it fires when it should.
    ///
    /// Every falsification of this guard so far has been "break the production
    /// code, watch it go RED". None of them could show the other half, because
    /// a green scan of a directory that contains no lookalikes proves nothing
    /// about lookalikes. Asserting on the predicate is the level where both
    /// halves are expressible: `uses_marker` is the scan's *only* decision, so
    /// a predicate that is right on these inputs is a scanner that is right on
    /// them.
    ///
    /// That equivalence was checked once rather than argued: the whole
    /// lookalike set below was planted into `vault.rs` as real production text
    /// — `enum FakeEnum { ParseErr(u32), IoErr(u32) }` with matching arms,
    /// `UnwrapErr(SysRng)`, `fn spread_dir()` — and the scanner stayed GREEN,
    /// while the five real spellings planted across five files each went RED
    /// naming their file. A permanent file-level negative is deliberately NOT
    /// kept: it would mean shipping a fixture inside
    /// `src/diagnostics/checks/`, i.e. production code whose only purpose is
    /// to be scanned. Stated so the narrower standing guarantee is not read as
    /// the wider one-off check.
    ///
    /// The lookalikes are not invented: `ParseErr(_)` is the plant that
    /// exposed the bug, and `UnwrapErr(` occurs three times in `src/` today.
    #[test]
    fn the_marker_matcher_fires_on_real_spellings_and_stays_quiet_on_lookalikes() {
        // Fires — every spelling the rule claims to cover.
        for (line, marker) in [
            ("        Err(_) => ChromiumProbe::Missing,", "Err(_"),
            ("        Err(_e) => Ok(NodeProbe::Missing),", "Err(_"),
            ("        Err(_err) => 0,", "Err(_"),
            ("        Err(..) => Ok(()),", "Err(..)"),
            ("        Result::Err(_) => 0,", "Err(_"),
            ("        Ok(v) => v, Err(_) => 0,", "Err(_"),
            ("Err(_) => 0,", "Err(_"),
            ("    let e = std::fs::read_dir(dir);", "read_dir"),
            ("    if !self.vault_path.exists() {", ".exists()"),
        ] {
            assert!(uses_marker(line, marker), "must flag `{marker}` in: {line}");
        }

        // Stays quiet — the tail of a longer identifier is not the token.
        for (line, marker) in [
            ("        ParseErr(_) => 0,", "Err(_"),
            ("        IoErr(_e) => 0,", "Err(_"),
            ("        MyErr(..) => 0,", "Err(..)"),
            ("    let _: UnwrapErr(SysRng);", "Err(_"),
            ("    fn spread_dir() {}", "read_dir"),
            ("    let x = 0;", "Err(_"),
        ] {
            assert!(
                !uses_marker(line, marker),
                "must NOT flag `{marker}` in: {line}"
            );
        }
    }

    /// One `Err(<binding>)` match arm, sliced out of a source file.
    struct ErrArm {
        /// The identifier the error is bound to.
        binding: String,
        /// Text between the pattern and `=>` — empty when the arm has no
        /// match guard.
        guard: String,
        /// Everything the arm evaluates, block braces included.
        body: String,
        /// 1-based line of the `Err(` token **in the file itself**, so an
        /// offender can be opened.
        ///
        /// This is why the scan reads `production_code_lines` rather than
        /// `strip_comment_lines(production_prefix(..))`: both of those delete
        /// lines, so counting `'\n'` in their output produced a number that
        /// pointed at innocent code — 19 lines short on the first real
        /// offender, in a directory that is ~40% doc comment.
        line: usize,
    }

    /// Is `ident` present in `text` as a whole token?
    fn mentions(text: &str, ident: &str) -> bool {
        fn part(c: char) -> bool {
            c.is_alphanumeric() || c == '_'
        }
        text.match_indices(ident).any(|(at, _)| {
            let left_ok = text[..at].chars().next_back().is_none_or(|c| !part(c));
            let right_ok = text[at + ident.len()..]
                .chars()
                .next()
                .is_none_or(|c| !part(c));
            left_ok && right_ok
        })
    }

    /// Invocations of macros that only *emit* the error, removed.
    ///
    /// This is the whole point of the rule below: `tracing::warn!("{e}")`
    /// mentions the error without the error reaching anything the check
    /// reports. Both live sites were spelled exactly that way — log it, then
    /// answer as though it had not happened — so a rule that accepted any
    /// mention of the binding would have passed them.
    ///
    /// `eprintln!` / `println!` are here for the same reason and were NOT in
    /// the first version: a re-review re-ran the exact `stale_lock` mutation
    /// with `eprintln!` substituted for `tracing::warn!` and the guard was
    /// GREEN — a byte-equivalent reinstatement of the defect it was written
    /// for, walking past it because the binding "reached" a macro the stripper
    /// had not been told about. Nothing in this directory prints to stdio
    /// today, so that was latent rather than live; it was also squarely inside
    /// the rule's own stated question, which is what makes it a hole rather
    /// than a documented edge.
    ///
    /// The list is closed on purpose and this is where it can rot: a macro
    /// that only emits and is not named here reads as carrying the error. The
    /// membership test is *"does this macro do anything with its argument
    /// other than emit it"* — `format!` is the standing counter-example and is
    /// deliberately absent, because it produces a value, and a value is what
    /// "carrying the error" means.
    ///
    /// Only the `name!(` form is stripped: `warn! {…}` and `warn! (…)` are
    /// not. rustfmt normalises the space form and the brace form is vanishingly
    /// rare for these macros, so this is a stated limit rather than a hole
    /// worth code.
    ///
    /// ⚠️ **A parenthesis inside the log message mis-terminates this walk.**
    /// `depth` counts `(` and `)` without skipping string literals — unlike
    /// `balanced` in [`err_arms`], which does. Both directions below were
    /// MEASURED by running the predicate, not reasoned about: two readers
    /// derived them and both got the labels backwards, because it is easy to
    /// describe the *stripper's* error instead of the *guard's verdict*.
    ///
    /// - `tracing::warn!("oops :) {e}")` — depth reaches 0 inside the string,
    ///   the strip stops early, and the message's own tail survives into the
    ///   output carrying the `{e}` with it. The binding is then "found", so an
    ///   arm that really does fold is **not flagged**: a false NEGATIVE. This
    ///   is the shape that matters, because a log line normally interpolates
    ///   the binding, so the surviving tail normally contains it.
    /// - `tracing::warn!("oops :( ")` — depth never returns to 0 at the
    ///   macro's real end, so the walk overshoots and strips the rest of the
    ///   arm. A carry written AFTER the log line is destroyed and a correct arm
    ///   is **flagged**: a false POSITIVE.
    ///
    /// It predates this list's widening (it reproduces on `warn!`) and is
    /// unreachable in this directory today — one production log call, and every
    /// `println!` / `eprintln!` here is inside `#[cfg(test)]`. Closing it is not
    /// expensive (share `balanced`, which already skips literals); that is a
    /// code change and this is the doc that says what the guard cannot see.
    /// Both directions are asserted in
    /// [`the_fold_detector_fires_on_the_three_shapes_that_shipped`], so this
    /// paragraph cannot drift from the behaviour, and a future fix turns those
    /// two assertions red at the lines that name it.
    fn without_logging(body: &str) -> String {
        const LOG_MACROS: [&str; 7] = [
            "trace!",
            "debug!",
            "info!",
            "warn!",
            "error!",
            "eprintln!",
            "println!",
        ];
        let mut out = String::with_capacity(body.len());
        let mut i = 0usize;
        let bytes: Vec<char> = body.chars().collect();
        'outer: while i < bytes.len() {
            for m in LOG_MACROS {
                let rest: String = bytes[i..].iter().take(m.len() + 1).collect();
                if rest.starts_with(m) && rest[m.len()..].starts_with('(') {
                    // Left boundary: `warn!` must not match `my_warn!`.
                    let left_ok =
                        i == 0 || !(bytes[i - 1].is_alphanumeric() || bytes[i - 1] == '_');
                    if left_ok {
                        let mut depth = 0i32;
                        let mut j = i + m.len();
                        while j < bytes.len() {
                            match bytes[j] {
                                '(' => depth += 1,
                                ')' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        j += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        i = j;
                        continue 'outer;
                    }
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        out
    }

    /// Every `Err(<binding>) [if guard] => <body>` arm in `src`.
    ///
    /// `src` is expected to be comment-stripped production text. String
    /// literals are skipped while balancing so a `{` or `)` inside one cannot
    /// end an arm early.
    fn err_arms(src: &str) -> Vec<ErrArm> {
        let ch: Vec<char> = src.chars().collect();
        let mut arms = Vec::new();
        let mut i = 0usize;
        while i + 4 <= ch.len() {
            // A literal can hold anything, `Err(` included.
            if ch[i] == '"' {
                i += 1;
                while i < ch.len() && ch[i] != '"' {
                    if ch[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                continue;
            }
            if !(ch[i] == 'E' && ch[i + 1] == 'r' && ch[i + 2] == 'r' && ch[i + 3] == '(') {
                i += 1;
                continue;
            }
            if i > 0 && (ch[i - 1].is_alphanumeric() || ch[i - 1] == '_') {
                i += 1;
                continue;
            }
            let Some(close) = balanced(&ch, i + 3, '(', ')') else {
                i += 1;
                continue;
            };
            let inner: String = ch[i + 4..close].iter().collect();
            // Only a plain binding. `Err(_)` / `Err(_e)` / `Err(..)` are the
            // existing discard rule's business, and `Err(io::ErrorKind::X)`
            // is a variant the author named on purpose.
            let is_binding = !inner.is_empty()
                && inner.chars().next().is_some_and(char::is_alphabetic)
                && inner.chars().all(|c| c.is_alphanumeric() || c == '_');
            if !is_binding {
                i += 1;
                continue;
            }
            // Between the pattern and `=>`: nothing, or a match guard.
            let mut j = close + 1;
            while j + 1 < ch.len() && !(ch[j] == '=' && ch[j + 1] == '>') {
                // A `{`, `;` or `,` before the arrow means this was never a
                // match arm (`let x = Err(e);`, `Err(e).unwrap()`, …).
                if matches!(ch[j], '{' | ';' | ',') {
                    break;
                }
                j += 1;
            }
            if !(j + 1 < ch.len() && ch[j] == '=' && ch[j + 1] == '>') {
                i += 1;
                continue;
            }
            let guard: String = ch[close + 1..j].iter().collect();
            let mut b = j + 2;
            while b < ch.len() && ch[b].is_whitespace() {
                b += 1;
            }
            let body_end = if ch.get(b) == Some(&'{') {
                balanced(&ch, b, '{', '}').map_or(ch.len(), |e| e + 1)
            } else {
                arm_expr_end(&ch, b)
            };
            arms.push(ErrArm {
                binding: inner,
                guard,
                body: ch[b..body_end.min(ch.len())].iter().collect(),
                line: ch[..i].iter().filter(|c| **c == '\n').count() + 1,
            });
            i = body_end.max(i + 1);
        }
        arms
    }

    /// Index of the delimiter closing the one that opens at or after `from`,
    /// skipping string literals.
    fn balanced(ch: &[char], from: usize, open: char, close: char) -> Option<usize> {
        let mut depth = 0i32;
        let mut i = from;
        while i < ch.len() {
            match ch[i] {
                '"' => {
                    i += 1;
                    while i < ch.len() && ch[i] != '"' {
                        if ch[i] == '\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// End of a non-block arm body: the `,` that terminates it at depth zero.
    fn arm_expr_end(ch: &[char], from: usize) -> usize {
        let mut depth = 0i32;
        let mut i = from;
        while i < ch.len() {
            match ch[i] {
                '"' => {
                    i += 1;
                    while i < ch.len() && ch[i] != '"' {
                        if ch[i] == '\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    if depth == 0 {
                        return i;
                    }
                    depth -= 1;
                }
                ',' if depth == 0 => return i,
                _ => {}
            }
            i += 1;
        }
        ch.len()
    }

    /// A bound error that only reaches a log line is a discarded error with a
    /// receipt.
    ///
    /// # Why this is a second rule and not a wider marker
    ///
    /// [`CONFLATING`]'s `Err(_` / `Err(..)` entries catch the error that is
    /// *thrown away at the pattern*. They are green on
    ///
    /// ```ignore
    /// Err(e) => { tracing::warn!("probe failed: {e}"); None }
    /// ```
    ///
    /// because the error IS bound — and that spelling shipped three times in
    /// this directory while the guard reported the class closed:
    /// `stale_lock.rs` folded a `JoinError` into `None` and rendered
    /// `[ok] No lock held`; `duplicate_instance.rs` folded one into `0` and
    /// rendered `[ok] Single instance`; `providers_connectivity.rs` folded a
    /// vault error into `None` twice, once per provider and once as
    /// `vec![None; probes.len()]`, and published `{name}: unreachable` with a
    /// fix hint routing to the one repair that cannot help. **Binding is
    /// necessary, not sufficient.**
    ///
    /// So the question this asks is not "was the error bound" and not "what
    /// value did the arm produce" — the three sites produced three different
    /// values (`None`, `0`, `vec![None; …]`), and a rule enumerating those
    /// would be the same mistake one level up. It asks: **does the error
    /// reach anything other than a log line?**
    ///
    /// # What it can and cannot see
    ///
    /// - *Sees*: `Err(<binding>) => …` arms whose body, with logging macro
    ///   invocations removed, never mentions the binding again — whatever the
    ///   arm produces instead.
    /// - *Accepts a match guard as classification*: `Err(e) if e.kind() ==
    ///   NotFound => Ok(Absent)` is the shape the round asks for ("one arm per
    ///   error that actually MEANS absence"), so an arm whose guard reads the
    ///   binding passes even when its body does not. Zero such arms in this
    ///   directory today; `check::DirListing::of` one module over is the
    ///   pattern being permitted.
    /// - *Accepts carrying the error through a local*: the binding only has to
    ///   appear somewhere outside a log line, not in the tail expression —
    ///   `Err(f) => { let why = f.detail.clone(); … }` passes.
    /// - *Blind to* `Err(_)` and `Err(..)`: those are [`CONFLATING`]'s job, and
    ///   deliberately not re-reported here.
    /// - *Blind to* a named-variant arm (`Err(BrowserError::ChromiumNotFound)
    ///   => Missing`). It binds nothing, so there is no error to carry; that
    ///   arm is an author's classification and `browser_runtime.rs` argues for
    ///   it in prose.
    /// - *Blind to* the same fold written with `?`, `.ok()`, `.unwrap_or*` or
    ///   `if let Err(e)` rather than a `match` arm. `.unwrap_or*` is discussed
    ///   at length under [`no_check_answers_a_stat_error_with_absence`] — the
    ///   `Option`/`Result` ambiguity makes a spelling rule a false accuser.
    ///   `if let Err(e) = …` is not covered because its body is a statement,
    ///   not the check's answer.
    /// - *Blind to* an error that reaches a value which is then itself
    ///   discarded. This rule tracks one hop, not dataflow.
    /// - *Blind to* runtime behaviour: it cannot see that `None` means
    ///   "reassuring" in one check and "irrelevant" in another. That is why it
    ///   asks about the error rather than about the value.
    ///
    /// - *Blind to* a trailing comment on a code line. This scans
    ///   `production_code_lines`, not `code_text`, because `code_text` blanks
    ///   string INTERIORS — and `format!("… {e}")` is how almost every correct
    ///   arm in this directory carries its error, so scanning `code_text` here
    ///   reported all sixteen of them as offenders. `strip_comment_lines` drops
    ///   whole comment lines and leaves literals intact; a comment sharing a
    ///   line with code survives and could mention the binding. That is the
    ///   under-see direction on a rule whose over-see direction is a false
    ///   accusation, which is the trade this directory already made once.
    ///
    /// CRLF-safe by the same route as its sibling: `production_code_lines`
    /// drops `\r` before anything else.
    #[test]
    fn no_check_folds_a_bound_error_into_an_answer() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("diagnostics")
            .join("checks");
        let sources = rust_sources_under(&root);

        let mut offenders: Vec<String> = Vec::new();
        let mut arms_examined = 0usize;

        for (rel, text) in &sources {
            for arm in err_arms(&production_code_lines(text)) {
                arms_examined += 1;
                if mentions(&arm.guard, &arm.binding) {
                    continue;
                }
                if mentions(&without_logging(&arm.body), &arm.binding) {
                    continue;
                }
                offenders.push(format!(
                    "{rel}:{}: `Err({})` is bound and then answered without it — \
                     `{}`. Logging the error is not reporting it. Carry it into the value \
                     the check publishes (`check::unknown_finding` / \
                     `check::settle_probe`), or name the variant that really does mean \
                     absence so the arm classifies instead of guessing.",
                    arm.line,
                    arm.binding,
                    arm.body.split_whitespace().collect::<Vec<_>>().join(" "),
                ));
            }
        }

        // Self-count: a parser that sliced nothing is green and blind.
        // MEASURED, not estimated — 36 `Err(<binding>)` arms across this
        // directory at the time of writing, read off this assertion's own
        // failure message with the floor temporarily raised. The floor sits
        // well below that so ordinary edits do not trip it, and far enough
        // above zero to catch a slicer that stopped working.
        assert!(
            arms_examined >= 20,
            "the scan sliced only {arms_examined} `Err(<binding>)` arm(s) out of \
             src/diagnostics/checks/ — a guard that examined nothing is green and blind"
        );
        assert!(
            offenders.is_empty(),
            "a bound error that only reaches a log line is a discarded error with a \
             receipt:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The predicate's own negative half, on the three shapes that shipped and
    /// the shapes that must stay quiet.
    ///
    /// Same argument as
    /// [`the_marker_matcher_fires_on_real_spellings_and_stays_quiet_on_lookalikes`]:
    /// a green scan of a directory containing no offenders proves nothing
    /// about offenders, and `err_arms` + `without_logging` + `mentions` are
    /// the scan's only decisions.
    #[test]
    fn the_fold_detector_fires_on_the_three_shapes_that_shipped() {
        // Each of the three real sites, verbatim in shape. They produced three
        // DIFFERENT values, which is why the rule asks about the error.
        for (src, why) in [
            (
                "match p { Ok(h) => h, Err(e) => { tracing::warn!(\"probe failed: {e}\"); None } }",
                "stale_lock: folded into None",
            ),
            (
                "match p { Ok(n) => n, Err(e) => { tracing::warn!(\"probe failed: {e}\"); 0 } }",
                "duplicate_instance: folded into 0",
            ),
            (
                "match p { Ok(k) => k, Err(e) => { tracing::warn!(\"task failed: {e}\"); vec![None; probes.len()] } }",
                "providers_connectivity: folded into vec![None; n]",
            ),
            (
                "match v { Ok(Some(s)) => Some(s), Ok(None) => None, Err(e) => { tracing::warn!(error = %e, \"vault read failed\"); None } }",
                "providers_connectivity resolve_key: folded into None",
            ),
            // The same defect emitted through stdio rather than `tracing`.
            // This shape was GREEN until `eprintln!`/`println!` joined
            // `LOG_MACROS`; a re-review found it by substituting one token.
            (
                "match p { Ok(h) => h, Err(e) => { eprintln!(\"lock probe failed: {e}\"); None } }",
                "stale_lock via eprintln!: folded into None",
            ),
            (
                "match p { Ok(h) => h, Err(e) => { println!(\"probe failed: {e}\"); 0 } }",
                "folded into 0 via println!",
            ),
        ] {
            let arms = err_arms(src);
            assert_eq!(arms.len(), 1, "{why}: expected exactly one bound Err arm");
            assert!(
                !mentions(&without_logging(&arms[0].body), &arms[0].binding),
                "{why}: must be flagged"
            );
        }

        // Stays quiet: the error reaches the value, one way or another.
        for (src, why) in [
            (
                "match p { Err(f) => return vec![f], Ok(v) => v }",
                "returned directly",
            ),
            (
                "match p { Err(e) => Err(format!(\"lookup failed: {e}\")), Ok(v) => Ok(v) }",
                "formatted into the value",
            ),
            (
                "match p { Err(e) => { tracing::warn!(\"x: {e}\"); return vec![unknown(format!(\"{e}\"))]; } Ok(v) => v }",
                "logged AND carried",
            ),
            (
                "match p { Err(f) => { let why = f.detail.clone(); v.map(|_| why.clone()).collect() } Ok(v) => v }",
                "carried through a local",
            ),
            (
                "match p { Err(e) if e.kind() == NotFound => Ok(Absent), Err(e) => Err(f(e)), Ok(v) => Ok(v) }",
                "classified by a match guard",
            ),
        ] {
            for arm in err_arms(src) {
                assert!(
                    mentions(&arm.guard, &arm.binding)
                        || mentions(&without_logging(&arm.body), &arm.binding),
                    "{why}: must NOT be flagged (arm body `{}`)",
                    arm.body
                );
            }
        }

        // The paren-in-string limit, pinned in BOTH directions so the doc on
        // `without_logging` cannot drift from it. These assert the CURRENT
        // behaviour, which is wrong in two different ways; a fix that shares
        // `balanced`'s literal-skipping turns both red, and the doc paragraph
        // they belong to is then the thing to delete.
        {
            let fold_with_smiley =
                "match p { Ok(h) => h, Err(e) => { tracing::warn!(\"oops :) {e}\"); None } }";
            let arms = err_arms(fold_with_smiley);
            assert_eq!(arms.len(), 1);
            assert!(
                mentions(&without_logging(&arms[0].body), &arms[0].binding),
                "measured: a `)` inside the message stops the strip early and the \
                 message's own tail carries `{{e}}` out with it, so this real fold is \
                 NOT flagged -- a false negative"
            );

            let carry_with_frowny = "match p { Ok(h) => h, Err(e) => { tracing::warn!(\"oops :( \");                                      return Err(unknown(format!(\"{e}\"))); } }";
            let arms = err_arms(carry_with_frowny);
            assert_eq!(arms.len(), 1);
            assert!(
                !mentions(&without_logging(&arms[0].body), &arms[0].binding),
                "measured: a `(` inside the message makes the walk overshoot and swallow \
                 the carry that follows, so this CORRECT arm IS flagged -- a false positive"
            );
        }

        // `println!` is a suffix of `eprintln!`; the left boundary is what
        // keeps a user macro whose name merely ends in a listed one from being
        // stripped, which would hide a real carry.
        assert!(
            mentions(&without_logging("{ my_warn!(\"{e}\"); None }"), "e"),
            "a macro that is not in the list must keep its argument visible"
        );
        assert!(
            !mentions(&without_logging("{ eprintln!(\"{e}\"); None }"), "e"),
            "eprintln! must be stripped"
        );

        // Not this rule's business: discarded at the pattern (CONFLATING's
        // job), and a named variant, which binds nothing to carry.
        assert!(err_arms("match p { Err(_) => None, Ok(v) => v }").is_empty());
        assert!(err_arms("match p { Err(_e) => None, Ok(v) => v }").is_empty());
        assert!(err_arms("match p { Err(..) => None, Ok(v) => v }").is_empty());
        assert!(
            err_arms("match p { Err(BrowserError::ChromiumNotFound) => Missing, Ok(v) => v }")
                .is_empty()
        );
        // Not a match arm at all.
        assert!(err_arms("let x = Err(e); x.unwrap()").is_empty());
        // A string literal holding a brace must not end the arm early.
        let arms = err_arms("match p { Err(e) => { warn!(\"} {e}\"); None } Ok(v) => v }");
        assert_eq!(arms.len(), 1);
        assert!(!mentions(&without_logging(&arms[0].body), &arms[0].binding));
    }

    /// A check must never dress "I could not look" as "there is nothing there".
    ///
    /// Eight production sites across **eight** files in this directory did
    /// exactly that, in three different directions: six answered a stat error
    /// with a reassuring `Finding::ok` ("no secrets stored yet" in front of an
    /// unreadable vault), one answered it with the wrong problem and then let
    /// `--fix` report a repair it had not performed, and one walked past an
    /// unreadable ancestor and reported free space for a different filesystem.
    /// (An earlier revision of this comment said "seven files"; the figure was
    /// inherited from a brief rather than counted, and `probe_users >= 8` three
    /// screens down — which counts files — contradicted it.)
    ///
    /// `browser_runtime.rs` was a ninth, in a shape the first sweep could not
    /// see: `Err(_) => …::Missing` and `.unwrap_or(…::Missing)` on a
    /// `spawn_blocking` `JoinError`, all rendering `[ok]`. A panicked probe task
    /// reported "no browser installed", reassuringly. The `Err(_)` rule below
    /// exists because of it.
    ///
    /// Converting the sites one by one would have closed instances and left the
    /// class open; this closes the class.
    ///
    /// # What it can and cannot see
    ///
    /// - *Sees*: the literal spellings in [`CONFLATING`] anywhere in the
    ///   production half of any `.rs` file under `src/diagnostics/checks/`.
    /// - *Blind to*: `Path::is_file()`, `Path::is_dir()` and `Path::metadata()`,
    ///   which conflate exactly the same way. A text rule cannot tell
    ///   `Path::is_file()` from `Metadata::is_file()`, and
    ///   `sqlite_integrity::list_databases` deliberately uses the latter — so a
    ///   rule covering them would need an exemption, and an exemption is the
    ///   thing that later hides a real hit. Named here rather than left for
    ///   someone to discover.
    /// - *Blind to*: the conflation happening in a helper this directory
    ///   delegates to. `stale_lock.rs` has no `.exists()` because
    ///   `utils::instance_lock::diagnose_holder` does the probing, and its
    ///   `read_to_string(..).ok()?` reads an unreadable holder file as "no lock
    ///   file at all". That is the same class one directory over, and out of
    ///   this rule's stated scope.
    /// - *Blind to* `unwrap_or` / `unwrap_or_else` / `unwrap_or_default`
    ///   applied to a `Result` — the other spelling of "discard the error and
    ///   invent the answer", and the one `browser_runtime.rs` used on its
    ///   `spawn_blocking` `JoinError`. `Option::unwrap_or` is lexically
    ///   identical, and the production halves of this directory hold **nine
    ///   such occurrences across eight lines** that are not this defect: eight
    ///   `Option::unwrap_or*`, plus one `Result::unwrap_or`
    ///   (`u32::try_from(..).unwrap_or(0)` in `cache_health.rs`, clamping an
    ///   out-of-range streak — and note that this one shares a line with an
    ///   `Option` use, which is why "eight lines" and "nine occurrences" are
    ///   different numbers). So a rule on the spelling would need an allowlist
    ///   covering both kinds. A statement-bounded "`.await` and `unwrap_or` in
    ///   one expression" rule WOULD be clean against today's tree (measured:
    ///   none of those nine has an `.await` in its statement) and is
    ///   deliberately not shipped — the first legitimate `Option`-yielding
    ///   `.await` makes it a false accuser, and a guard that accuses falsely
    ///   gets cited as evidence.
    /// - *Blind to* runtime behaviour generally: this is a spelling rule. It
    ///   cannot see a new conflating API, only the shapes that were used here.
    /// - Markers are matched as tokens, not as bare substrings — see
    ///   [`uses_marker`]. That is a deliberate trade in the safe direction: the
    ///   rule now misses an `Err(_)` written with a zero-width character before
    ///   it, and no longer accuses `ParseErr(_)` of anything.
    /// - **No allowlist, by construction.** If a site genuinely needs a bare
    ///   `.exists()`, the answer is that `check::Presence` is missing a case —
    ///   extend it there, where every check inherits the fix.
    ///
    /// CRLF-safe: `production_prefix` and `code_text` both drop `\r` before
    /// anything else, so nothing here is anchored to a bare `\n`.
    ///
    /// `CARGO_MANIFEST_DIR` is baked in at COMPILE time, but
    /// `rust_sources_under` reads file *contents* at run time — so this reads
    /// the CURRENT tree at that path, not a snapshot. The hazard that leaves is
    /// narrower and worth naming precisely: a test binary built in worktree A
    /// scans worktree A even when the command is run from worktree B.
    #[test]
    fn no_check_answers_a_stat_error_with_absence() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("diagnostics")
            .join("checks");
        let sources = rust_sources_under(&root);

        let mut offenders: Vec<String> = Vec::new();
        // Self-count #2: how many files were seen to call the replacement.
        // File count alone cannot separate "scanned real code" from "scanned
        // empty strings" — `code_text` returning blanks would look identical.
        let mut probe_users = 0usize;

        for (rel, text) in &sources {
            let prod = code_text(&production_prefix(text));
            if prod.contains("Presence::of(") || prod.contains("DirListing::of(") {
                probe_users += 1;
            }
            for line in prod.lines() {
                for (marker, replacement) in CONFLATING {
                    if uses_marker(line, marker) {
                        offenders.push(format!(
                            "{rel}: `{}` — `{marker}` cannot tell absence from a refusal to \
                             look. Use {replacement}.",
                            line.trim()
                        ));
                    }
                }
            }
        }

        // Self-count #1: the walk reached this directory at all. 17 files at
        // the time of writing (16 checks + this mod.rs).
        assert!(
            sources.len() >= 15,
            "the walk found only {} .rs files under src/diagnostics/checks/ — a guard \
             that examined nothing is green and blind, not clean",
            sources.len()
        );
        assert!(
            probe_users >= 8,
            "only {probe_users} file(s) under src/diagnostics/checks/ were seen calling \
             `Presence::of(` / `DirListing::of(`; eight were converted when this rule was \
             written, so a lower number means either the scanner stopped reading code or \
             a conversion was reverted. Deleting a check legitimately lowers it — lower \
             this floor deliberately, do not delete the assertion."
        );
        assert!(
            offenders.is_empty(),
            "a diagnostic is the one place where \"I could not look\" must never render \
             as \"there is nothing there\":\n  {}",
            offenders.join("\n  ")
        );
    }
}
