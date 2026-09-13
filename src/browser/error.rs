use std::fmt;

use thiserror::Error;

use super::engine::Engine;

/// Why a snapshot ref no longer addresses anything.
///
/// Lives with the error rather than with `RefTable`, because the error is what
/// crosses every layer boundary and a second definition beside the table would
/// be one fact with two spellings (判据 §1). `page_state::refs` re-exports
/// this one.
///
/// `Unknown` is not a filler variant: `DOM.resolveNode` can fail for reasons
/// neither of the other two names, and answering `NodeGone` for those would
/// tell the model something we did not observe (判据 §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// The document changed under the ref — a navigation, a new loader id.
    Navigated,
    /// The document is the same and the node is not in it any more.
    NodeGone,
    /// The ref could not be resolved and we cannot say why.
    Unknown,
}

impl fmt::Display for StaleReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Navigated => "the page navigated",
            Self::NodeGone => "that element is no longer in the page",
            Self::Unknown => "it could not be resolved",
        })
    }
}

#[derive(Debug, Error)]
pub enum BrowserError {
    /// A launch that did not reach a usable browser, tagged with **which
    /// step** failed. The stage is not decoration: "the binary would not
    /// spawn", "the process died before it published a port" and "the port
    /// file never appeared" are three different operator problems, and a
    /// single opaque string made the tool answer identical for all three.
    /// `stage` values in use: `"spawn"`, `"chromium-exit"`, `"devtools-port"`
    /// (this module's Chromium launch), `"chrome-mcp"` (the existing-session
    /// driver's Chrome launch), `"engine-process"` (no launcher registered for
    /// the profile's engine), `"cdp-endpoint"` (the engine process is up and
    /// its websocket refused the connection) and `"cdp-ready"` (connected, but
    /// it did not pass `engine::readiness::ready_gate`).
    #[error("Failed to launch browser at stage '{stage}': {detail}")]
    LaunchFailed { stage: &'static str, detail: String },

    #[error("Tab not found: {0}")]
    TabNotFound(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Browser action failed: {0}")]
    ActionFailed(String),

    #[error("Browser operation timed out after {0}ms")]
    Timeout(u64),

    #[error("Chromium binary not found. Install Chrome/Chromium or specify a binary path.")]
    ChromiumNotFound,

    /// No browser to launch for this engine: the pin (if any) is gone, and
    /// neither the system nor the runtime ledger has one. The message names
    /// the command that fixes it, because a fail-closed answer that does not
    /// say how to open the gate is fail-dead (判据 §14).
    ///
    /// The FIRST door named must be one the reader can actually open (Final
    /// Review M8): `runtime_manage` is in `method_authz::OPERATOR_TOOLS`
    /// (checked against the array itself, not this comment — the array holds
    /// `"runtime_manage"` and does NOT hold `"bash"`, and `tool_requires_operator`
    /// is what `tools/scoped/dispatch.rs`'s channel gate actually calls), so a
    /// chat-tier caller who reads "ask me to run `runtime_manage{...}`" as its
    /// next step tries it and is refused. `bash` is absent from that array, so
    /// it stays open to chat tier — the plain `playwright-cli` command goes
    /// first and is labelled as self-runnable; the operator-only remedies
    /// follow, labelled as such, so a chat-tier reader is told which doors are
    /// not its own rather than discovering that by being refused.
    ///
    /// `install_hint` is built by [`engine_unavailable`] and never by a
    /// caller. Two engines have two different remedies — Chromium's runs
    /// through `playwright-cli`, obscura's only through the runtime ledger —
    /// and a caller that assembled its own would eventually offer the wrong
    /// one, which costs the reader a turn before they find out it does not
    /// exist.
    #[error("{install_hint}")]
    EngineUnavailable {
        engine: Engine,
        /// The raw "here is where I looked" sentence, kept beside the built
        /// hint rather than only interpolated into it.
        ///
        /// The doctor's `missing_finding` (`engine_missing.rs`) wraps its
        /// argument in its OWN sentence and appends its OWN fix hint, so
        /// handing it `install_hint` would nest a full remedy inside a
        /// parenthetical and print the remedy twice from two authors — and
        /// that file's tests assert only id/title/severity, so nothing would
        /// notice. `chromium_resolve.rs`'s two tests also assert on this
        /// field and keep working unchanged.
        tried: String,
        install_hint: String,
    },

    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),

    #[error("Failed to attach to browser: {0}")]
    AttachFailed(String),

    /// The MCP server answered, and its answer was a failure — the tool's own
    /// verdict about the page (element missing, wait elapsed, …).
    #[error("Chrome DevTools MCP error: {0}")]
    ChromeMcpError(String),

    /// The MCP call never got an answer (broken pipe, dead server, client-side
    /// request timeout). Distinct from [`Self::ChromeMcpError`] because "the
    /// tool said no" and "nothing ever looked" are different facts, and a
    /// caller that folds a negative verdict into a value (`wait_for` →
    /// `Ok(false)`) must never fold this one.
    #[error("Chrome DevTools MCP transport failure: {0}")]
    ChromeMcpTransport(String),

    #[error("Playwright CLI error: {0}")]
    PlaywrightCliError(String),

    #[error("Playwright CLI not installed. Open Settings → Browser → Install All.")]
    PlaywrightCliNotInstalled,

    #[error("No active browser session for '{0}'. Call open/goto first.")]
    NoSession(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Browser profile not found: {0}")]
    ProfileNotFound(String),

    /// The engine took the command but has not answered inside the budget.
    ///
    /// **Not a failure verdict.** The request is still queued and the engine
    /// may still answer; what elapsed is our patience, not its life. Says so
    /// explicitly, because a model that reads "timed out" as "the page is
    /// broken" retries the whole plan instead of the one call.
    #[error(
        "{engine} did not answer {method} within {waited_secs}s — the request is still \
         queued, not refused. Retry the same call, or move this profile to the other \
         engine with browser_session{{action:\"switch_engine\"}} if it keeps stalling."
    )]
    EngineBusy {
        engine: Engine,
        method: String,
        waited_secs: u64,
    },

    /// The capability table says this engine cannot do this verb.
    ///
    /// The text is built rather than a fixed format string because
    /// `supported_by: None` must NOT advertise a switch that would change
    /// nothing — a remedy that does not work is worse than none.
    #[error("{}", unsupported_text(*engine, verb, *supported_by))]
    UnsupportedByEngine {
        engine: Engine,
        verb: &'static str,
        supported_by: Option<Engine>,
    },

    /// The engine process died or its connection dropped.
    #[error(
        "{engine} stopped: {reason}. Nothing is driving this profile now — browser_open \
         starts a fresh one."
    )]
    EngineFailure { engine: Engine, reason: String },

    /// The profile already has a live engine, and it is not the one asked for.
    ///
    /// ⚠️ **Forward reference.** The message names `browser_session{action:
    /// "switch_engine", engine:"…"}`, and at HEAD `BrowserSessionArgs.name` is
    /// a required `String` — so between this task and Task 19, which makes it
    /// `Option<String>`, the call it names is one the schema rejects. Task 19's
    /// acceptance list must keep that field change, or this message has to name
    /// the profile too.
    ///
    /// Not a silent swap: changing engines under an open profile discards its
    /// cookies, its tabs and every ref the model is holding. That is
    /// `switch_engine`'s job (spec §5.3) — it migrates the state and returns a
    /// fresh page — so this refusal names it rather than performing half of it.
    #[error(
        "browser profile '{profile}' is already running {running}, but this call asked \
         for {requested}. Use browser_session{{action:\"switch_engine\", \
         engine:\"{requested}\"}} to move cookies and open tabs across, or close the \
         profile first."
    )]
    EngineMismatch {
        profile: String,
        running: Engine,
        requested: Engine,
    },

    /// A snapshot ref that no longer addresses an element.
    ///
    /// Never degraded into "element not found": the ref was valid when it was
    /// minted, and the model's correct next move (re-snapshot) is different
    /// from the one a missing selector calls for (pick another target).
    #[error(
        "ref {ref_id} is stale — {reason}. Re-run browser_snapshot and use a ref from the \
         new listing; refs are not stable across a navigation."
    )]
    StaleRef { ref_id: String, reason: StaleReason },

    /// The engine answered the CDP call with a protocol error.
    ///
    /// Verbatim, both halves: the code is what a reader looks up against the
    /// CDP docs, and the message is the engine's own words. Neither is
    /// paraphrased — a paraphrase of an engine's error is a second author for
    /// a fact we did not observe.
    ///
    /// `engine` (fix round 1, F2/M3): this is the one variant with no
    /// recovery verb (see the exemption table on
    /// `every_engine_error_names_the_engine_and_a_verb_the_model_can_call`),
    /// so the engine name is the only routing signal it can carry — a
    /// protocol error that cannot say which engine produced it is one the
    /// reader cannot route. Deviates from the brief's `Produces` (three flat
    /// fields); this is a controller ruling, not drift: at the time this was
    /// written `Cdp` had exactly one construction site in the whole tree (a
    /// test), and `map_cdp_err` (Task 12) does not exist yet, so adding the
    /// field now is a struct-field change, not a live-call-site change.
    #[error("{engine} cdp {method} failed: {code} {message}")]
    Cdp {
        engine: Engine,
        method: String,
        code: i64,
        message: String,
    },
}

/// The sentence for [`BrowserError::UnsupportedByEngine`].
///
/// Two shapes, because `None` means "no engine here can do this" and offering
/// `switch_engine` for it would send the model on a round trip that lands it
/// exactly where it started.
fn unsupported_text(engine: Engine, verb: &str, supported_by: Option<Engine>) -> String {
    match supported_by {
        Some(other) => format!(
            "{engine} cannot do {verb}. {other} can — switch this profile with \
             browser_session{{action:\"switch_engine\", engine:\"{other}\"}}, then repeat \
             the call."
        ),
        None => format!(
            "{verb} is supported by no engine Aleph drives — switching engines will not \
             help. Use a different approach for this step."
        ),
    }
}

/// The ONLY constructor for [`BrowserError::EngineUnavailable`].
///
/// The Chromium command text is `format!`-built from `CHROMIUM_INSTALL_ARGS`
/// (`browser::chromium_resolve`), not typed out a second time: a literal here
/// compared against a literal in the test only proves the two agree with each
/// other, never that either still names a command that exists. Sourced from
/// `chromium_resolve`, not `builtin_tools::runtime_manage`, because
/// `chromium_resolve` owns the install route, so it owns the fact. (Not an
/// acyclicity argument any more: after this task `chromium_resolve` also calls
/// `error::engine_unavailable`, so the two modules reference each other —
/// legal in Rust, and the point here is which module authors the command, not
/// the shape of the graph.)
pub fn engine_unavailable(engine: Engine, tried: impl fmt::Display) -> BrowserError {
    let install_hint = match engine {
        Engine::Chromium => {
            let args = crate::browser::chromium_resolve::CHROMIUM_INSTALL_ARGS.join(" ");
            format!(
                "No chromium for the browser ({tried}). Run `playwright-cli {args}` yourself \
                 — a plain local command, not an operator-gated tool. Operators can instead \
                 run `runtime_manage{{action:\"install\", capability:\"chromium\"}}` or pin \
                 one with [general.browser.runtime] binary_path."
            )
        }
        Engine::Obscura => format!(
            "No obscura for the browser ({tried}). It is installed only from Aleph's \
             runtime ledger. Operators can run \
             `runtime_manage{{action:\"install\", capability:\"obscura\"}}` or pin a binary \
             with [general.browser.obscura] binary_path. To keep working right now, switch \
             this profile to the other engine: \
             `browser_session{{action:\"switch_engine\", engine:\"chromium\"}}`."
        ),
    };
    BrowserError::EngineUnavailable {
        engine,
        tried: tried.to_string(),
        install_hint,
    }
}

/// [`BrowserError::EngineUnavailable`] for the ONE cause [`engine_unavailable`]
/// cannot name correctly (fix round 1, F1): `playwright-cli` itself is not on
/// PATH or in the runtime ledger. `engine_unavailable`'s ordinary Chromium
/// hint says "Run `playwright-cli install-browser chromium` yourself" —
/// correct when the CLI exists and only the browser is missing, a dead end
/// when the CLI is the thing that is missing. `engine_unavailable` cannot
/// tell the two causes apart from a `Display`; only the caller that tried to
/// locate the CLI knows which one happened (`chromium.rs`'s
/// `managed_cli_path()` call, currently the only call site).
///
/// Chromium only: there is no obscura equivalent — obscura's hint never
/// mentions `playwright-cli`, so this cause cannot occur for it.
///
/// The install hint keeps exactly one author per cause: this function is the
/// only place that builds THIS hint, same as [`engine_unavailable`] is the
/// only place that builds the ordinary one, and both defer the actual remedy
/// sentence to [`super::chromium_resolve::PLAYWRIGHT_CLI_MISSING_REMEDY`]
/// rather than typing it out here a second time.
pub fn engine_unavailable_no_launcher(tried: impl fmt::Display) -> BrowserError {
    let install_hint = format!(
        "No chromium for the browser ({tried}). {}",
        crate::browser::chromium_resolve::PLAYWRIGHT_CLI_MISSING_REMEDY
    );
    BrowserError::EngineUnavailable {
        engine: Engine::Chromium,
        tried: tried.to_string(),
        install_hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::engine::Engine;
    use crate::browser::profile::BrowserDriver;

    /// M1 (task-8 fix round, promoted from Minor): this message names
    /// `runtime_manage` as the fix for a missing engine — a bare literal,
    /// unpinned against a rename. Derived from the tool's own name constant,
    /// not a fresh literal: a fresh literal here would only prove the two
    /// strings agree with each other, not that either still names a real
    /// tool (判据 §10).
    #[test]
    fn the_chromium_unavailable_message_names_a_tool_that_actually_exists() {
        use crate::builtin_tools::runtime_manage::RuntimeManageTool;
        use crate::tools::AlephTool;

        for engine in Engine::ALL {
            let err = engine_unavailable(engine, "no system browser");
            assert!(
                err.to_string().contains(RuntimeManageTool::NAME),
                "the fix hint must name a tool that still exists: {err}"
            );
            assert!(
                err.to_string().contains(engine.as_str()),
                "the message must say WHICH engine is missing: {err}"
            );
        }
    }

    /// M8: the first remedy named must be one a chat-tier caller can act on
    /// itself. `runtime_manage` is in `method_authz::OPERATOR_TOOLS` — naming
    /// it first (as "ask me to run …") reads as the model's own next step and
    /// gets it refused. The plain `playwright-cli` command (which `bash`,
    /// absent from that array, can run for any caller) must come first, and
    /// the operator-gated remedy must be labelled as such.
    ///
    /// The CLI command is matched against `CHROMIUM_INSTALL_ARGS` itself, not
    /// a second typed-out copy — a literal-vs-literal match would stay green
    /// the day the real command changed (判据 §10, §17).
    ///
    /// Chromium only: obscura has no `playwright-cli` route at all, so its
    /// hint names the ledger and nothing else. Asserting the same shape for
    /// both engines would force a sentence about a command obscura cannot run.
    #[test]
    fn the_first_remedy_named_is_one_a_chat_tier_caller_can_actually_run() {
        let err = engine_unavailable(Engine::Chromium, "no system browser");
        let text = err.to_string();
        let install_args = crate::browser::chromium_resolve::CHROMIUM_INSTALL_ARGS.join(" ");
        let cli_marker = format!("playwright-cli {install_args}");
        let cli_at = text
            .find(&cli_marker)
            .unwrap_or_else(|| panic!("names the plain CLI command ({cli_marker:?}): {text}"));
        let runtime_manage_at = text
            .find("runtime_manage")
            .expect("still names runtime_manage as an alternative");
        assert!(
            cli_at < runtime_manage_at,
            "the self-runnable remedy must be named before the operator-gated \
             one, not after: {text}"
        );
        assert!(
            text.contains("Operators can instead"),
            "the operator-gated remedy must be labelled as such, not left \
             looking equally available to every caller: {text}"
        );
        assert!(
            text.contains("[general.browser.runtime]"),
            "the pin route must still be named, and with the path Config \
             actually reads: {text}"
        );
    }

    /// obscura is installed from the runtime ledger; there is no
    /// `playwright-cli install-browser obscura` and never will be. The hint
    /// must not offer one — an unrunnable first remedy is worse than none,
    /// because the reader spends a turn on it before finding out.
    #[test]
    fn the_obscura_hint_names_the_ledger_and_never_the_playwright_cli() {
        let text = engine_unavailable(Engine::Obscura, "not in the runtime ledger").to_string();
        assert!(
            !text.contains("playwright-cli"),
            "obscura cannot be installed by playwright-cli: {text}"
        );
        assert!(
            text.contains("capability:\"obscura\""),
            "must name the exact ledger capability to install: {text}"
        );
        assert!(text.contains("runtime_manage"), "{text}");
        assert!(
            text.contains("[general.browser.obscura]"),
            "the pin route must name the section Config reads: {text}"
        );
    }

    /// F1 (fix round 1): the ordinary chromium-missing case (cli present,
    /// browser absent) must still point at the CLI install command —
    /// `engine_unavailable_no_launcher` is a new SIBLING for the other cause,
    /// not a replacement, and must not have regressed this one.
    #[test]
    fn the_ordinary_chromium_missing_hint_still_names_playwright_cli() {
        let text = engine_unavailable(Engine::Chromium, "no system browser").to_string();
        assert!(
            text.contains("Run `playwright-cli install-browser chromium`"),
            "cli present, browser missing: must still point at the CLI \
             install command: {text}"
        );
    }

    /// F1 (fix round 1): the falsifying test for the defect itself.
    /// `engine_unavailable`'s ordinary hint tells the reader to run
    /// `playwright-cli` — correct when the CLI exists, a dead end when the
    /// CLI itself is what is missing. This asserts the NEW no-launcher hint
    /// does not make that mistake, and instead names the one route that
    /// actually installs playwright-cli.
    #[test]
    fn the_no_launcher_hint_never_tells_the_reader_to_run_playwright_cli() {
        let text = engine_unavailable_no_launcher(
            "no playwright-cli found on PATH or in the runtime ledger",
        )
        .to_string();
        assert!(
            !text.contains("Run `playwright-cli"),
            "playwright-cli is the thing reported missing; telling the \
             reader to run it is the dead end this fixes: {text}"
        );
        assert!(
            text.contains("capability:\"playwright-cli\""),
            "must name the ledger capability that actually installs it: {text}"
        );
        assert!(
            text.contains("chromium"),
            "must still say which engine: {text}"
        );
    }

    /// Exhaustive classification of spec §7.1's two signals — names a
    /// recovery verb the model can call; names the engine involved — for
    /// EVERY `BrowserError` variant. No `_` arm.
    ///
    /// This is the enforcement now, not the doc table on the test below (fix
    /// round 2): a hand-written prose table already carried an overclaim
    /// TWICE inside the very artifact built to retire the first overclaim
    /// (`UnsupportedByEngine`'s row said "yes | yes" unconditionally, when
    /// its `None` arm names neither), and nothing in the suite caught it,
    /// because prose cannot be falsified. A `match` can: add a variant in
    /// Task 12 or Task 15 and this function fails to compile until it is
    /// classified here, which a table on a doc comment could never guarantee
    /// on its own (判据 §5 — a list written on legislation day).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RecoverySignal {
        /// Predates spec §7.1 — not one of the six variants Task 8 added, so
        /// the property does not apply. A real classification, reviewed once
        /// while writing this match, not a shrug.
        NotApplicable,
        /// Names both a recovery verb and the engine involved.
        VerbAndEngine,
        /// Names a recovery verb but not an engine — the recovery does not
        /// depend on which engine is running.
        VerbOnly,
        /// Names the engine but no verb — no Aleph tool fixes this class of
        /// failure.
        EngineOnly,
        /// Names neither.
        Neither,
    }

    fn spec_7_1_signal(err: &BrowserError) -> RecoverySignal {
        match err {
            BrowserError::LaunchFailed { .. }
            | BrowserError::TabNotFound(_)
            | BrowserError::NavigationFailed(_)
            | BrowserError::ActionFailed(_)
            | BrowserError::Timeout(_)
            | BrowserError::ChromiumNotFound
            | BrowserError::ScreenshotFailed(_)
            | BrowserError::AttachFailed(_)
            | BrowserError::ChromeMcpError(_)
            | BrowserError::ChromeMcpTransport(_)
            | BrowserError::PlaywrightCliError(_)
            | BrowserError::PlaywrightCliNotInstalled
            | BrowserError::NoSession(_)
            | BrowserError::Io(_)
            | BrowserError::ProfileNotFound(_) => RecoverySignal::NotApplicable,

            BrowserError::EngineUnavailable { .. } => RecoverySignal::VerbAndEngine,
            BrowserError::EngineBusy { .. } => RecoverySignal::VerbAndEngine,
            BrowserError::UnsupportedByEngine {
                supported_by: Some(_),
                ..
            } => RecoverySignal::VerbAndEngine,
            BrowserError::UnsupportedByEngine {
                supported_by: None, ..
            } => RecoverySignal::Neither,
            BrowserError::EngineFailure { .. } => RecoverySignal::VerbAndEngine,
            // Names BOTH engines (the one running and the one asked for) and
            // the one verb that reconciles them, `browser_session
            // {action:"switch_engine"}`. The reader's next move is a call, not
            // a diagnosis, which is what separates this from `Cdp`.
            BrowserError::EngineMismatch { .. } => RecoverySignal::VerbAndEngine,
            BrowserError::StaleRef { .. } => RecoverySignal::VerbOnly,
            BrowserError::Cdp { .. } => RecoverySignal::EngineOnly,
        }
    }

    /// Spec §7.1, precisely (fix round 2 — the fix-round-1 table itself
    /// overclaimed, in exactly the shape it was written to retire:
    /// `UnsupportedByEngine`'s row said "yes | yes" unconditionally, but its
    /// `None` arm — [`unsupported_text`] two functions up — names NEITHER a
    /// verb nor an engine, by design (判据 §8: naming a switch that would
    /// send the model on a round trip back to where it started is worse than
    /// naming nothing). Nothing caught it: a mutation dropping `{engine}`
    /// from the `Some` arm stayed green, because nothing asserted that arm's
    /// OWN `engine` field — every existing assertion covered only
    /// `supported_by`/`other`. That gap is closed below, and
    /// `spec_7_1_signal` (above this test) is now the enforced source of
    /// truth; this table is a reading aid for it, not a second authority —
    /// if the two disagree, the match is right and this table is stale):
    ///
    /// | Variant                        | Verb | Engine | Reason |
    /// |---------------------------------|:---:|:---:|---|
    /// | `EngineUnavailable`              | yes | yes | covered by the `engine_unavailable*` tests above, not this one |
    /// | `EngineBusy`                     | yes | yes | — |
    /// | `UnsupportedByEngine` (`Some`)   | yes | yes | — |
    /// | `UnsupportedByEngine` (`None`)   | **no** | **no** | nothing supports the verb — no engine to route on, and offering `switch_engine` would send the model on a round trip back to where it started |
    /// | `EngineFailure`                  | yes | yes | — |
    /// | `EngineMismatch`                 | yes | yes | names both the running engine and the requested one, plus `switch_engine` — the call that reconciles them |
    /// | `StaleRef`                       | yes | **no** | no `engine` field — re-snapshotting the SAME tab fixes it regardless of which engine drives it |
    /// | `Cdp`                            | **no** | yes | no Aleph tool fixes a CDP protocol error; the actionable content IS the verbatim triple, and inventing a verb would name a door that does not exist |
    ///
    /// Every row below is checked against both a real `.to_string()` and
    /// `spec_7_1_signal`'s classification, except `StaleRef`'s "no engine" —
    /// the type has no `engine` field, so the compiler enforces that one, not
    /// a runtime assertion.
    ///
    /// The expected substrings for the verb-bearing variants are the tools'
    /// own `NAME` constants, never typed literals:
    /// `BrowserSessionTool::NAME` (`browser_tools/session.rs:131`),
    /// `BrowserOpenTool::NAME` (`open.rs:58`), `BrowserSnapshotTool::NAME`
    /// (`snapshot.rs:79`) — all reachable, since every one of those modules is
    /// `pub mod` in `browser_tools/mod.rs`. A literal here would only prove
    /// the message text agrees with the test, and would stay green the day a
    /// tool was renamed and the error started naming one that does not exist
    /// (判据 §10). HEAD's own
    /// `the_chromium_unavailable_message_names_a_tool_that_actually_exists`
    /// is the precedent.
    #[test]
    fn every_engine_error_names_the_engine_and_a_verb_the_model_can_call() {
        use crate::builtin_tools::browser_tools::{
            open::BrowserOpenTool, session::BrowserSessionTool, snapshot::BrowserSnapshotTool,
        };
        use crate::tools::AlephTool;

        let cases: Vec<(BrowserError, &str, RecoverySignal)> = vec![
            (
                BrowserError::EngineBusy {
                    engine: Engine::Obscura,
                    method: "Page.navigate".into(),
                    waited_secs: 30,
                },
                BrowserSessionTool::NAME,
                RecoverySignal::VerbAndEngine,
            ),
            (
                BrowserError::UnsupportedByEngine {
                    engine: Engine::Obscura,
                    verb: "browser_pdf",
                    supported_by: Some(Engine::Chromium),
                },
                BrowserSessionTool::NAME,
                RecoverySignal::VerbAndEngine,
            ),
            (
                BrowserError::EngineFailure {
                    engine: Engine::Chromium,
                    reason: "websocket closed".into(),
                },
                BrowserOpenTool::NAME,
                RecoverySignal::VerbAndEngine,
            ),
            (
                BrowserError::EngineMismatch {
                    profile: "default".into(),
                    running: Engine::Chromium,
                    requested: Engine::Obscura,
                },
                BrowserSessionTool::NAME,
                RecoverySignal::VerbAndEngine,
            ),
            (
                BrowserError::StaleRef {
                    ref_id: "e12".into(),
                    reason: StaleReason::Navigated,
                },
                BrowserSnapshotTool::NAME,
                RecoverySignal::VerbOnly,
            ),
        ];
        for (err, verb, expected_signal) in cases {
            assert_eq!(
                spec_7_1_signal(&err),
                expected_signal,
                "classification drifted from the code for {err:?}"
            );
            let text = err.to_string();
            assert!(text.contains(verb), "no recovery verb in: {text}");
        }

        // The busy text carries the two numbers a reader needs to decide
        // whether to wait or to switch, and the method that stalled.
        let busy_err = BrowserError::EngineBusy {
            engine: Engine::Obscura,
            method: "Page.navigate".into(),
            waited_secs: 30,
        };
        assert_eq!(spec_7_1_signal(&busy_err), RecoverySignal::VerbAndEngine);
        let busy = busy_err.to_string();
        assert!(busy.contains("obscura"), "{busy}");
        assert!(busy.contains("Page.navigate"), "{busy}");
        assert!(busy.contains("30"), "{busy}");

        // The unsupported text names the engine that DOES support the verb —
        // "not supported" alone leaves the model with nowhere to go. It ALSO
        // names the engine that CANNOT (fix round 2): the assertion whose
        // absence let the fix-round-1 table's overclaim survive a mutation
        // dropping `{engine}` from this arm.
        let unsupported_err = BrowserError::UnsupportedByEngine {
            engine: Engine::Obscura,
            verb: "browser_pdf",
            supported_by: Some(Engine::Chromium),
        };
        assert_eq!(
            spec_7_1_signal(&unsupported_err),
            RecoverySignal::VerbAndEngine
        );
        let unsupported = unsupported_err.to_string();
        assert!(unsupported.contains("browser_pdf"), "{unsupported}");
        assert!(
            unsupported.contains("obscura"),
            "must name the engine that CANNOT do it, not only the one that \
             can: {unsupported}"
        );
        assert!(unsupported.contains("chromium"), "{unsupported}");
        assert!(
            unsupported.contains("switch_engine"),
            "must name the argument that gets there: {unsupported}"
        );

        // Nothing supports it: the text must say so plainly rather than
        // implying a switch that would change nothing — and it names NEITHER
        // engine, not even the one that asked, because naming one here would
        // wrongly imply the engine matters (fix round 2's finding: this is
        // the row the fix-round-1 table got wrong).
        let nowhere_err = BrowserError::UnsupportedByEngine {
            engine: Engine::Obscura,
            verb: "browser_screencast",
            supported_by: None,
        };
        assert_eq!(spec_7_1_signal(&nowhere_err), RecoverySignal::Neither);
        let nowhere = nowhere_err.to_string();
        assert!(
            !nowhere.contains("switch_engine"),
            "a verb no engine supports must not advertise a switch: {nowhere}"
        );
        assert!(
            !nowhere.contains("obscura"),
            "must not name an engine either — none of them can do it: {nowhere}"
        );
        assert!(nowhere.contains("no engine"), "{nowhere}");

        // A stale ref must always tell the model to re-snapshot, whichever
        // reason it carries — the ref is unusable in every case.
        for reason in [
            StaleReason::Navigated,
            StaleReason::NodeGone,
            StaleReason::Unknown,
        ] {
            let err = BrowserError::StaleRef {
                ref_id: "e12".into(),
                reason,
            };
            assert_eq!(spec_7_1_signal(&err), RecoverySignal::VerbOnly);
            let text = err.to_string();
            assert!(text.contains("e12"), "{text}");
            assert!(text.contains(BrowserSnapshotTool::NAME), "{text}");
        }

        // A protocol error is reported verbatim, both halves: a code with no
        // message is unactionable and a message with no code cannot be looked
        // up against the CDP docs. It names the engine (F2) — its only
        // routing signal, since it names no verb (see the table above this
        // test) — and it names no tool, enforcing the "no verb" exemption
        // rather than only claiming it in prose.
        let cdp_err = BrowserError::Cdp {
            engine: Engine::Obscura,
            method: "DOM.getBoxModel".into(),
            code: -32000,
            message: "Could not compute box model.".into(),
        };
        assert_eq!(spec_7_1_signal(&cdp_err), RecoverySignal::EngineOnly);
        let cdp = cdp_err.to_string();
        assert!(cdp.contains("DOM.getBoxModel"), "{cdp}");
        assert!(cdp.contains("-32000"), "{cdp}");
        assert!(cdp.contains("Could not compute box model."), "{cdp}");
        assert!(cdp.contains("obscura"), "must name which engine: {cdp}");
        for tool in [
            BrowserSessionTool::NAME,
            BrowserOpenTool::NAME,
            BrowserSnapshotTool::NAME,
        ] {
            assert!(
                !cdp.contains(tool),
                "Cdp names no recovery verb — no Aleph tool fixes a protocol \
                 error, and inventing one would name a door that does not \
                 exist: {cdp}"
            );
        }
    }

    /// The one-sided trait defaults used to name Chrome DevTools MCP by hand,
    /// which stopped being true the moment a third driver existed. The text
    /// must say which driver DOES serve the verb, derived from the driver
    /// enum rather than typed out.
    #[test]
    fn a_one_sided_default_names_the_driver_that_serves_the_verb() {
        let err = crate::browser::backend::unsupported_by_driver("pdf", BrowserDriver::Managed);
        let text = err.to_string();
        assert!(text.contains("pdf"), "{text}");
        assert!(
            text.contains(BrowserDriver::Managed.as_wire()),
            "must name the driver that supports it, spelled the way a config \
             file spells it: {text}"
        );
        assert!(
            !text.contains("Chrome DevTools MCP server exposes"),
            "the hardcoded Chrome-MCP sentence must be gone: {text}"
        );
    }
}
