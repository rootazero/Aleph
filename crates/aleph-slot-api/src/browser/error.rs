use std::fmt;

use thiserror::Error;

use aleph_protocol::browser_live::ViewerBinding;

use super::engine::Engine;
use super::text::quote;

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
    /// The exact target is under a human lease; no agent mutation may start.
    #[error("human has control of browser target '{binding:?}'; wait for explicit cede")]
    HumanHasControl { binding: Box<ViewerBinding> },

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

    /// A tab this profile previously recorded is gone from the browser: the
    /// engine reported its target destroyed, or its targetId is absent from
    /// a live target enumeration.
    ///
    /// Distinct from [`Self::TabNotFound`], which means "this registry never
    /// heard of that id" (判据 §8 — the two answers license different
    /// recoveries). `TabGone` carries the last URL we recorded so the reader
    /// can recognise WHICH page vanished; it never claims anything about the
    /// page's state beyond that last observation.
    ///
    /// Produced by the event pump's `Target.targetDestroyed` fold
    /// (`engine::EngineHandle::ensure_tab` answering from the table's dead
    /// set) and by the Core `TabRegistry::resolve_identity` path —
    /// the page's own `window.close`, a browser restart, a re-attach onto a
    /// browser whose tabs changed. The recovery it names is a fresh listing,
    /// because every id the model holds may be stale, not only this one.
    #[error("{}", tab_gone_text(tab_id, last_url))]
    TabGone {
        tab_id: String,
        last_url: Option<String>,
    },

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
    /// `install_hint` is built by the Core browser error constructor
    /// and never by a caller. Two engines have two different remedies —
    /// Chromium's runs through `playwright-cli`, obscura's only through the
    /// runtime ledger — and a caller that assembled its own would eventually
    /// offer the wrong one, which costs the reader a turn before they find
    /// out it does not exist.
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

    /// A `browser_record` stop/status asked about a recording that is not
    /// running and has no finalized receipt waiting — a plain fact, not a
    /// transport failure. Carries the tab id (or recording id) that was asked
    /// about.
    #[error("no recording in progress for '{0}' — browser_record{{action:\"status\"}} lists what IS running")]
    NoActiveRecording(String),

    /// A `switch_engine` to the engine that is already running. Refused rather
    /// than answered `Ok`: a no-op reported as a success is indistinguishable
    /// from a switch that worked, and the model would believe it had taken the
    /// escape hatch when it had not (判据 §11).
    ///
    /// The mirror of [`Self::EngineMismatch`], which says "a DIFFERENT engine is
    /// running"; the two cannot be one variant because the recovery differs —
    /// there is nothing to recover from here, which is why the text names no
    /// verb.
    #[error("profile '{profile}' is already running {engine}; nothing to switch to")]
    AlreadyOnEngine { profile: String, engine: Engine },

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
    /// ⚠️ **Forward reference, now half-closed.** The message names
    /// `browser_session{action:"switch_engine", engine:"…"}`. Task 16 made
    /// `BrowserSessionArgs.name` an `Option<String>` (for `action:
    /// "capabilities"`), so that half no longer rejects the call — but
    /// `SessionAction` still has no `SwitchEngine` variant and the args carry
    /// no `engine` field, so until Task 19 adds both, this is still a message
    /// naming a call the schema rejects. Task 19's acceptance list has to close
    /// the remaining half.
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

    /// The engine ACCEPTED the dispatch, but the effect probe's one-shot
    /// listener saw no matching event arrive at the target element.
    ///
    /// A dedicated variant rather than `ActionFailed(String)` plus a message
    /// prefix — a deliberate, approved deviation from spec §3: the tool
    /// layer's failure classifier (browser_tools::recovery, Task B1) is an
    /// EXHAUSTIVE match over these variants, so smuggling this verdict
    /// through a string would force the classifier to string-match inside
    /// `classify` — exactly the 判据 violation the enum exists to prevent.
    /// The failure category gains a 15th variant `EffectNotDelivered` there.
    ///
    /// The distinction this variant protects: the engine answered (so this is
    /// NOT `EngineBusy`, NOT `Cdp`) and the dispatch went out — but the page
    /// never saw the effect. 「它答了」和「它答的是真的」是两个问题, and this
    /// is the second one answered in the negative. `verb` names the tool verb
    /// (`browser_click` / `browser_type` / `browser_fill_form`); `detail`
    /// says what the probe observed instead. The most common cause is an
    /// element that moved or was replaced mid-dispatch, so the door named is
    /// a fresh snapshot.
    #[error(
        "{verb}: the engine accepted the dispatch, but no matching event arrived at the \
         target element ({detail}). The element may have moved or been replaced \
         mid-dispatch. Re-run browser_snapshot and retry with a fresh ref."
    )]
    EffectNotDelivered { verb: &'static str, detail: String },

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

/// The sentence for [`BrowserError::TabGone`]. Names the tab and, when we
/// recorded one, the last URL we saw it at — QUOTED, because that URL is a
/// page-influenced observation (a redirect target is the page's choice, R40).
/// The page's own state is never claimed: "gone" is a fact about the target
/// enumeration, not about what the page contains now.
fn tab_gone_text(tab_id: &str, last_url: &Option<String>) -> String {
    let seen = match last_url {
        Some(url) => format!(", last seen at {}", quote(url)),
        None => String::new(),
    };
    format!(
        "tab {tab_id}{seen} is gone — the browser no longer has that target (the page \
         may have closed itself, or the engine was restarted). Run \
         browser_tabs{{action:\"list\"}} to see what is open now, and use an id from \
         that listing: every id captured before this error may be stale."
    )
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
