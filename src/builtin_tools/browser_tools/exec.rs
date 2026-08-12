// Browser exec tool — runs ONE whole browser sub-procedure in ONE tool call:
// navigate, act, wait, and read, as an ordered action list.
//
// Why the tool exists: each single-action tool call costs a full model
// round-trip, and every action can invalidate the snapshot refs the next
// action targets. The tool started life as `browser_batch`, nine *write*
// actions — which left the reads outside, so the cheapest "search a site and
// read the answer" was still browser_open → browser_snapshot → browser_batch
// → browser_snapshot: four turns, two of which each ship a 30k-char page. A
// procedure that cannot read cannot be a procedure. `navigate` / `snapshot` /
// `evaluate` close that gap and the tool is now named for what it does.
//
// What it deliberately is NOT: the reference design (hermes-agent) collapses
// the whole browser surface into one tool whose argument is *Python with raw
// CDP access*, defended by a regex over http literals in the model's own
// source — defeated by string concatenation, and conceded by its authors to be
// terminal-equivalent. Every step here instead re-enters the SAME chokepoint
// the standalone tool uses: `ProfileManager::check_navigation` before a
// navigation and the backend's own landed-URL audit after it,
// `check_input_secret_block` before any keystroke, `current_page_block` before
// any page read, `redact_wrap` on the way out, and the per-`ActionType`
// approval gate on every action that has one. This tool is a scheduler, never
// a second, unguarded path into the browser.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::backend::BrowserBackend;
use crate::browser::manager::ProfileManager;
use crate::browser::types::{ActionTarget, ScrollDirection, WaitCondition};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Maximum actions in one call. openclaw allows 100 per `act batch`; 50 is the
/// deliberate divergence — a procedure runs inside a single tool call, and 50
/// actions bounds that call's output size without giving up the round-trip
/// savings the tool exists for.
///
/// Deliberately NOT raised when the read actions landed: the binding limit on
/// a long procedure is [`MAX_EXEC_BUDGET_MS`], not the action count (50 waits
/// at their 120s clamp is 100 minutes of nominal work against a 10-minute
/// budget), and reads make each step's *output* larger, not smaller. A higher
/// count would only let a caller queue steps the budget can never reach.
const MAX_EXEC_ACTIONS: usize = 50;

/// The procedure's own wall-clock budget, enforced between actions.
///
/// Two properties matter and only one of them used to hold. **(a)** It is real
/// elapsed time, not openclaw's *estimated* per-action budget (a heuristic sum
/// that drifts far from reality on a slow page), so a procedure of waits that
/// each legitimately consume their clamped timeout still terminates.
/// **(b)** It must fire INSIDE the harness's per-tool budget, or it never
/// fires at all: this tool was absent from `BUILTIN_TOOL_BUDGETS_MS` and
/// declares no `max_duration_ms`, so `resolve_tool_budget_ms` handed it
/// `DEFAULT_TOOL_BUDGET_MS` (300s) and the harness killed the call 300s before
/// this clock could — discarding the partial `results` and the "take a fresh
/// snapshot" recovery message in favour of an opaque overrun. The table now
/// carries `browser_exec` with headroom above this number, the same fix
/// `ask_user` and `task_wait` already carry, and
/// `budget::tests::browser_exec_budget_outlives_its_own_wall_clock` keeps the
/// two ordered.
pub(crate) const MAX_EXEC_BUDGET_MS: u64 = 600_000;

/// Hard upper bound on a batched `evaluate` payload — the same cap
/// `browser_evaluate` applies for the same reason (a multi-MB script blocks
/// the backend serializer or starves the browser process), so a script refused
/// as a standalone call is refused as a step.
///
/// Declared here rather than in `evaluate.rs` because this is the module that
/// needs it visible; `evaluate.rs` still declares its own private copy and
/// [`tests::evaluate_script_cap_matches_the_standalone_tool`] fails if the two
/// ever drift. Collapsing that copy into this constant is a one-line follow-up
/// in a file this change does not own.
pub(crate) const MAX_EVAL_SCRIPT_CHARS: usize = 64 * 1024;

/// Default per-wait timeout when a `wait` action omits `timeout_ms` — the
/// same 5s default `browser_wait_for` uses.
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 5000;

/// One action in a procedure. Serde's internal tag renders the variants as
/// `{"action": "click", ...}` / `{"action": "type", ...}` /
/// `{"action": "navigate", ...}` etc.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ExecAction {
    /// Navigate the tab to a URL (SSRF-checked, landed URL re-audited).
    Navigate {
        /// The URL to navigate to.
        url: String,
    },
    /// Click an element by snapshot ref_id or viewport coordinates.
    Click {
        /// Accessibility `ref_id` from a previous snapshot.
        #[serde(default)]
        ref_id: Option<String>,
        /// X coordinate (requires `y`).
        #[serde(default)]
        x: Option<f64>,
        /// Y coordinate (requires `x`).
        #[serde(default)]
        y: Option<f64>,
    },
    /// Double-click an element by snapshot ref_id.
    Dblclick {
        /// Accessibility `ref_id` from a previous snapshot.
        ref_id: String,
    },
    /// Type text into an element by snapshot ref_id (default: the focused
    /// element).
    Type {
        /// Accessibility `ref_id` from a previous snapshot; omit to type into
        /// whatever currently holds focus.
        #[serde(default)]
        ref_id: Option<String>,
        /// Text to type.
        text: String,
    },
    /// Set an input's value directly (no keystrokes) by snapshot ref_id.
    Fill {
        /// Accessibility `ref_id` from a previous snapshot.
        ref_id: String,
        /// Value to set.
        value: String,
    },
    /// Hover an element by snapshot ref_id.
    Hover {
        /// Accessibility `ref_id` from a previous snapshot.
        ref_id: String,
    },
    /// Scroll the viewport in a direction.
    Scroll {
        /// Direction to scroll: up, down, left, or right.
        direction: ScrollDirection,
    },
    /// Select a `<select>` option by snapshot ref_id.
    Select {
        /// Accessibility `ref_id` from a previous snapshot.
        ref_id: String,
        /// Option value to select.
        value: String,
    },
    /// Press a single key (e.g. "Enter", "Tab", "Escape").
    PressKey {
        /// Key name to press.
        key: String,
    },
    /// Wait for a page condition before continuing. Exactly one of
    /// text / text_gone / selector / url_contains / time_ms.
    Wait {
        /// Text to wait for on the page.
        #[serde(default)]
        text: Option<String>,
        /// Text to wait for DISAPPEARING from the page.
        #[serde(default)]
        text_gone: Option<String>,
        /// CSS selector to wait for (at least one matching element).
        #[serde(default)]
        selector: Option<String>,
        /// Substring to wait for in the tab's current URL.
        #[serde(default)]
        url_contains: Option<String>,
        /// Fixed delay in milliseconds (clamped to 500–120000).
        #[serde(default)]
        time_ms: Option<u64>,
        /// Timeout in milliseconds (default: 5000; clamped to 500–120000).
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Read the accessibility tree — the fresh refs later steps target. Its
    /// text comes back in this step's `output`.
    Snapshot {
        /// Maximum output characters (default: 30000, clamped to 1000..=120000).
        #[serde(default)]
        max_chars: Option<usize>,
    },
    /// Evaluate JavaScript in the page and return its value in this step's
    /// `output`.
    Evaluate {
        /// JavaScript to execute in the page context.
        js: String,
    },
}

/// Arguments for the `browser_exec` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserExecArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Actions to run in order (max 50). Execution stops at the first failure.
    pub actions: Vec<ExecAction>,
}

/// What one executed step did, and — for a read — what it saw.
///
/// The `action` label deliberately never echoes typed/filled text or an
/// `evaluate` script: only the secret gate vets those values, and every field
/// here flows back into the model context. `output` is the opposite case — it
/// is page-derived by construction, so it arrives already bounded, redacted
/// and fenced.
#[derive(Debug, Serialize)]
pub struct StepResult {
    /// 1-based ordinal, matching `failed_at`.
    pub step: usize,
    /// Short label, e.g. `click ref=e5`.
    pub action: String,
    /// `ok` / `navigated` / `found` / `not found`.
    pub status: &'static str,
    /// The read payload for `snapshot` / `evaluate`; absent for write actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// Output from the `browser_exec` tool.
#[derive(Debug, Serialize)]
pub struct BrowserExecOutput {
    pub success: bool,
    /// Total actions requested.
    pub total: usize,
    /// Actions that completed successfully.
    pub completed: usize,
    /// 1-based ordinal of the action execution aborted at (None on success).
    pub failed_at: Option<usize>,
    /// Per-step results for the completed prefix, reads included.
    pub results: Vec<StepResult>,
    pub message: Option<String>,
}

/// Runs a browser sub-procedure — an ordered action list — in one tool call.
#[derive(Clone)]
pub struct BrowserExecTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserExecTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate every action behind the user-defined approval policy. With no
    /// policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

/// The wait-condition fields of a `wait` action, factored out of the enum
/// variant so [`resolve_wait`] can be exercised without wrapping every test
/// case in an [`ExecAction`]. `timeout_ms` is not part of the condition: it is
/// clamped at the plan site into [`PlannedAction::Wait`].
#[derive(Debug, Clone, Default)]
struct WaitFields {
    text: Option<String>,
    text_gone: Option<String>,
    selector: Option<String>,
    url_contains: Option<String>,
    time_ms: Option<u64>,
}

/// Resolve a `wait` action's fields into its [`WaitCondition`]. Exactly one
/// of the five condition fields must be set — the same mutual-exclusion
/// contract as `browser_wait_for` (a combined "any-of" wait would be
/// ambiguous in the step result). `time_ms` maps to a clamped
/// [`WaitCondition::Time`].
fn resolve_wait(w: &WaitFields) -> std::result::Result<WaitCondition, String> {
    use super::wait_for::clamp_timeout;
    let set = [
        w.text.as_ref().map(|t| ("text", t)),
        w.text_gone.as_ref().map(|t| ("text_gone", t)),
        w.selector.as_ref().map(|s| ("selector", s)),
        w.url_contains.as_ref().map(|u| ("url_contains", u)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    match (set.as_slice(), w.time_ms) {
        ([("text", t)], None) => Ok(WaitCondition::Text((*t).clone())),
        ([("text_gone", t)], None) => Ok(WaitCondition::TextGone((*t).clone())),
        ([("selector", s)], None) => Ok(WaitCondition::Selector((*s).clone())),
        ([("url_contains", u)], None) => Ok(WaitCondition::UrlContains((*u).clone())),
        ([], Some(ms)) => Ok(WaitCondition::Time(clamp_timeout(ms))),
        ([], None) => Err(
            "wait action needs exactly one condition: set one of 'text', 'text_gone', \
             'selector', 'url_contains' or 'time_ms'"
                .into(),
        ),
        _ => Err(
            "wait action conditions are mutually exclusive: set exactly one of 'text', \
             'text_gone', 'selector', 'url_contains' or 'time_ms', not several"
                .into(),
        ),
    }
}

/// An action with its targeting, wait condition and read budget fully
/// resolved — everything [`execute_actions`] needs, with no validation left
/// to do.
#[derive(Debug, Clone)]
enum PlannedAction {
    Navigate(String),
    Click(ActionTarget),
    Dblclick(ActionTarget),
    Type(ActionTarget, String),
    Fill(ActionTarget, String),
    Hover(ActionTarget),
    Scroll(ScrollDirection),
    Select(ActionTarget, String),
    PressKey(String),
    Wait {
        condition: WaitCondition,
        timeout_ms: u64,
    },
    Snapshot {
        max_chars: usize,
    },
    Evaluate(String),
}

/// Resolve a ref_id-or-coordinates target with `browser_click` semantics:
/// ref_id wins, both coordinates are required together, and CSS selector
/// targeting does not exist (refs come from a snapshot).
fn resolve_xy_target(
    action: &str,
    ref_id: Option<&String>,
    x: Option<f64>,
    y: Option<f64>,
) -> std::result::Result<ActionTarget, String> {
    if let Some(rid) = ref_id {
        Ok(ActionTarget::Ref {
            ref_id: rid.clone(),
        })
    } else if let (Some(x), Some(y)) = (x, y) {
        Ok(ActionTarget::Coordinates { x, y })
    } else {
        Err(format!(
            "{action} requires a target: 'ref_id' from a snapshot, or both 'x' and 'y'"
        ))
    }
}

/// A required-ref target (dblclick / fill / hover / select have no
/// coordinate form on either backend).
fn ref_target(ref_id: &str) -> ActionTarget {
    ActionTarget::Ref {
        ref_id: ref_id.to_string(),
    }
}

/// The target `browser_type` uses: the named ref, or the literal `focused`
/// pseudo-ref when none is given.
///
/// The `type` action used to advertise `x` / `y` like `click` does. Neither
/// backend honours them: the playwright CLI drops the coordinates and types
/// into whatever holds focus, and the Chrome DevTools backend rejects a
/// coordinate target outright. The schema described a capability that never
/// existed, so it is gone; "type into the focused element" — the behaviour the
/// managed backend actually had — is now what omitting `ref_id` means, exactly
/// as in `browser_type`.
fn type_target(ref_id: Option<&String>) -> ActionTarget {
    ref_target(ref_id.map_or("focused", String::as_str))
}

/// Validate the whole procedure and lower it to executable form. Fails BEFORE
/// any approval check or backend call — a malformed action list is a model
/// mistake and must not consume a user approval or touch the page.
fn plan_actions(actions: &[ExecAction]) -> std::result::Result<Vec<PlannedAction>, String> {
    if actions.is_empty() {
        return Err("browser_exec requires at least one action".into());
    }
    if actions.len() > MAX_EXEC_ACTIONS {
        return Err(format!(
            "browser_exec is limited to {MAX_EXEC_ACTIONS} actions per call (got {}); \
             split the procedure across several calls",
            actions.len()
        ));
    }
    actions
        .iter()
        .map(|action| {
            let planned = match action {
                ExecAction::Navigate { url } => {
                    if url.trim().is_empty() {
                        return Err("navigate requires a non-empty 'url'".to_string());
                    }
                    PlannedAction::Navigate(url.clone())
                }
                ExecAction::Click { ref_id, x, y } => {
                    PlannedAction::Click(resolve_xy_target("click", ref_id.as_ref(), *x, *y)?)
                }
                ExecAction::Dblclick { ref_id } => PlannedAction::Dblclick(ref_target(ref_id)),
                ExecAction::Type { ref_id, text } => {
                    PlannedAction::Type(type_target(ref_id.as_ref()), text.clone())
                }
                ExecAction::Fill { ref_id, value } => {
                    PlannedAction::Fill(ref_target(ref_id), value.clone())
                }
                ExecAction::Hover { ref_id } => PlannedAction::Hover(ref_target(ref_id)),
                ExecAction::Scroll { direction } => PlannedAction::Scroll(direction.clone()),
                ExecAction::Select { ref_id, value } => {
                    PlannedAction::Select(ref_target(ref_id), value.clone())
                }
                ExecAction::PressKey { key } => PlannedAction::PressKey(key.clone()),
                ExecAction::Wait {
                    text,
                    text_gone,
                    selector,
                    url_contains,
                    time_ms,
                    timeout_ms,
                } => {
                    let fields = WaitFields {
                        text: text.clone(),
                        text_gone: text_gone.clone(),
                        selector: selector.clone(),
                        url_contains: url_contains.clone(),
                        time_ms: *time_ms,
                    };
                    PlannedAction::Wait {
                        condition: resolve_wait(&fields)?,
                        timeout_ms: super::wait_for::clamp_timeout(
                            timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS),
                        ),
                    }
                }
                ExecAction::Snapshot { max_chars } => PlannedAction::Snapshot {
                    // The same clamp the standalone tool applies to the same knob.
                    max_chars: super::snapshot::resolve_max_chars(*max_chars),
                },
                ExecAction::Evaluate { js } => {
                    let chars = js.chars().count();
                    if chars > MAX_EVAL_SCRIPT_CHARS {
                        return Err(format!(
                            "evaluate script is {chars} chars; the cap is \
                             {MAX_EVAL_SCRIPT_CHARS} chars. Split it into smaller evaluate \
                             steps, or use a snapshot step for bulk DOM work"
                        ));
                    }
                    PlannedAction::Evaluate(js.clone())
                }
            };
            Ok(planned)
        })
        .collect()
}

/// Short human-readable label for the step result. Deliberately never echoes
/// typed/filled text or an `evaluate` script — only the secret gate vets those
/// values, and the step results flow back into the model context.
fn action_label(action: &PlannedAction) -> String {
    let target = |t: &ActionTarget| match t {
        ActionTarget::Ref { ref_id } => format!("ref={ref_id}"),
        ActionTarget::Coordinates { x, y } => format!("x={x} y={y}"),
    };
    match action {
        PlannedAction::Navigate(url) => format!("navigate {url}"),
        PlannedAction::Click(t) => format!("click {}", target(t)),
        PlannedAction::Dblclick(t) => format!("dblclick {}", target(t)),
        PlannedAction::Type(t, _) => format!("type {}", target(t)),
        PlannedAction::Fill(t, _) => format!("fill {}", target(t)),
        PlannedAction::Hover(t) => format!("hover {}", target(t)),
        PlannedAction::Scroll(d) => format!("scroll {d:?}"),
        PlannedAction::Select(t, _) => format!("select {}", target(t)),
        PlannedAction::PressKey(key) => format!("press_key {key}"),
        PlannedAction::Wait { condition, .. } => match condition {
            WaitCondition::Text(t) => format!("wait text '{t}'"),
            WaitCondition::TextGone(t) => format!("wait text_gone '{t}'"),
            WaitCondition::Selector(s) => format!("wait selector '{s}'"),
            WaitCondition::UrlContains(u) => format!("wait url_contains '{u}'"),
            WaitCondition::Time(ms) => format!("wait delay {ms}ms"),
        },
        PlannedAction::Snapshot { max_chars } => format!("snapshot max_chars={max_chars}"),
        PlannedAction::Evaluate(js) => format!("evaluate {} chars of JS", js.chars().count()),
    }
}

/// The approval surface of one planned action: its existing per-action
/// [`ActionType`] and a target string for the policy. Actions that map to
/// `None` have no approval surface — `wait` polls, and `snapshot` is the read
/// `browser_snapshot` performs with no gate of its own.
///
/// Deliberate: this tool introduces NO new ActionType knob — it inherits the
/// per-action policy semantics of the single tools, so a policy that governs
/// `browser_click` governs a batched click identically, and one that governs
/// `browser_navigate` governs a batched navigation identically.
fn approval_surface(action: &PlannedAction) -> Option<(ActionType, &'static str, String)> {
    let target = |t: &ActionTarget| format!("{t:?}");
    match action {
        PlannedAction::Navigate(url) => {
            Some((ActionType::BrowserNavigate, "navigate", url.clone()))
        }
        PlannedAction::Click(t) => Some((ActionType::BrowserClick, "click", target(t))),
        PlannedAction::Dblclick(t) => Some((ActionType::BrowserClick, "dblclick", target(t))),
        PlannedAction::Type(t, _) => Some((ActionType::BrowserType, "type", target(t))),
        PlannedAction::Fill(t, _) => Some((ActionType::BrowserFill, "fill", target(t))),
        PlannedAction::Hover(t) => Some((ActionType::BrowserHover, "hover", target(t))),
        PlannedAction::Scroll(d) => Some((ActionType::BrowserScroll, "scroll", format!("{d:?}"))),
        PlannedAction::Select(t, _) => Some((ActionType::BrowserSelect, "select", target(t))),
        PlannedAction::PressKey(key) => {
            Some((ActionType::BrowserPressKey, "press_key", key.clone()))
        }
        PlannedAction::Evaluate(js) => Some((ActionType::BrowserEvaluate, "evaluate", js.clone())),
        PlannedAction::Wait { .. } | PlannedAction::Snapshot { .. } => None,
    }
}

/// Assert the tab's CURRENT url still passes the SSRF policy before a page
/// read — the read-time guard `make_backend_and_tab_guarded` performs for the
/// standalone read tools, at the one point in a procedure where it can be
/// asked meaningfully.
///
/// The predicate is [`super::current_page_block`] itself, not a copy of it:
/// only the plumbing differs, because a procedure resolves its tab ONCE and
/// must keep acting on that tab, whereas `make_backend_and_tab_guarded`
/// re-resolves the active tab on every call. Re-checked per read step rather
/// than once up front, since an intervening `navigate` step is exactly what
/// can move the tab onto a blocked origin.
async fn read_guard(
    manager: &ProfileManager,
    backend: &dyn BrowserBackend,
    tab_id: &str,
) -> std::result::Result<(), String> {
    let tabs_text = backend
        .list_tabs()
        .await
        .map_err(|e| super::backend_error_text(manager, &e))?;
    match super::current_page_block(manager, &tabs_text, tab_id).await {
        Some(violation) => Err(format!(
            "current page blocked by SSRF policy ({violation}); \
             navigate to an allowed URL before reading page content"
        )),
        None => Ok(()),
    }
}

/// Read the accessibility tree for a `snapshot` step, bounded by that step's
/// own `max_chars`, then redacted and fenced through [`super::redact_wrap`] —
/// the same three transforms `browser_snapshot` applies, in the same order.
///
/// Unlike `browser_snapshot` a truncated tree is NOT offloaded to the tool
/// result store: that spill path is the snapshot tool's own, keyed to its call
/// id, and a second writer of it would be a second source. The note names the
/// lever that exists here instead.
fn snapshot_output(manager: &ProfileManager, raw: &str, max_chars: usize) -> String {
    let (text, truncated) = super::bound_content(raw, max_chars);
    let wrapped = super::redact_wrap(manager, &text);
    if truncated {
        format!(
            "{wrapped}\n[snapshot truncated to {max_chars} chars; raise this step's max_chars, \
             or take a standalone browser_snapshot — it offloads the full tree for ctx_search]"
        )
    } else {
        wrapped
    }
}

/// Run the planned actions in order against the already-resolved tab.
///
/// Returns the per-step results plus `Some((ordinal, error))` at the first
/// failure — the ordinal is 1-based, matching what the model sees in `step`. A
/// wait that times out (`Ok(false)`) is NOT a failure: absence is an answer,
/// the step records `not found` and the procedure continues. The wall-clock
/// budget is checked before each action; running out of budget aborts with
/// that action's ordinal as the failure point.
///
/// The approval check for a step runs immediately BEFORE that step executes,
/// not in an upfront pass over all N. The upfront pass borrowed its shape from
/// the secret pre-scan, but the two are not alike: the pre-scan is a pure
/// predicate, whereas an approval is *recorded* — so a procedure that aborted
/// at step 3 left an audit trail claiming steps 4..N had been approved and
/// taken, and spent an `Ask` prompt on actions that never happened. A one-time
/// stamp cannot be spent before the action is confirmed. Deciding each step in
/// order also means a `click` is judged against the page the preceding
/// `navigate` actually produced.
async fn execute_actions(
    manager: &ProfileManager,
    approval_policy: Option<&Arc<dyn ApprovalPolicy>>,
    backend: &dyn BrowserBackend,
    tab_id: &str,
    planned: &[PlannedAction],
) -> (Vec<StepResult>, Option<(usize, String)>) {
    let started = std::time::Instant::now();
    let mut results = Vec::with_capacity(planned.len());
    for (i, action) in planned.iter().enumerate() {
        let ordinal = i + 1;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if elapsed_ms >= MAX_EXEC_BUDGET_MS {
            return (
                results,
                Some((
                    ordinal,
                    format!(
                        "wall-clock budget of {MAX_EXEC_BUDGET_MS}ms exhausted after {i} actions"
                    ),
                )),
            );
        }
        if let Some((action_type, verb, target)) = approval_surface(action) {
            if let Some(message) =
                super::check_browser_approval(approval_policy, action_type, verb, &target).await
            {
                return (results, Some((ordinal, message)));
            }
        }
        let label = action_label(action);
        match run_one(manager, backend, tab_id, action).await {
            Ok((status, output)) => results.push(StepResult {
                step: ordinal,
                action: label,
                status,
                output,
            }),
            Err(message) => return (results, Some((ordinal, message))),
        }
    }
    (results, None)
}

/// Execute a single planned action, returning its status word and — for a
/// read — its already-bounded/redacted/fenced payload.
async fn run_one(
    manager: &ProfileManager,
    backend: &dyn BrowserBackend,
    tab_id: &str,
    action: &PlannedAction,
) -> std::result::Result<(&'static str, Option<String>), String> {
    let plain = |r: std::result::Result<(), crate::browser::BrowserError>| {
        r.map(|()| ("ok", None)).map_err(|e| e.to_string())
    };
    match action {
        // The navigation chain of `browser_navigate`, reused rather than
        // re-implemented: `check_navigation` (SSRF + URL-embedded secret) in
        // front, and the backend's own `navigate` — which runs
        // `post_nav::audit_landed_tab`, re-checking the URL the redirect chain
        // actually landed on and quarantining the tab on a violation.
        //
        // CLAUDE.md §3.12 recorded navigation as deliberately excluded from
        // this tool ("导航有独立 SSRF+审批链，批量只收页内动作"). That decision is
        // overturned here, and the justification is the reuse itself: the step
        // re-enters the same chain per navigation instead of bypassing it, so
        // nothing about the guard weakens — while excluding it is what forced
        // a model to spend a round-trip in the middle of every procedure that
        // crosses a page boundary, which is why the reads were unreachable in
        // one call.
        PlannedAction::Navigate(url) => {
            if let Err(violation) = manager.check_navigation(url).await {
                return Err(format!("Blocked: {violation}"));
            }
            backend
                .navigate(tab_id, url)
                .await
                .map(|()| ("navigated", None))
                .map_err(|e| e.to_string())
        }
        PlannedAction::Click(t) => plain(backend.click(tab_id, t.clone()).await),
        PlannedAction::Dblclick(t) => plain(backend.dblclick(tab_id, t.clone()).await),
        PlannedAction::Type(t, text) => plain(backend.type_text(tab_id, t.clone(), text).await),
        PlannedAction::Fill(t, value) => plain(backend.fill(tab_id, t.clone(), value).await),
        PlannedAction::Hover(t) => plain(backend.hover(tab_id, t.clone()).await),
        PlannedAction::Scroll(d) => {
            // Viewport scroll: the backend ignores the target, so a
            // viewport-origin placeholder is passed (same as browser_scroll).
            let target = ActionTarget::Coordinates { x: 0.0, y: 0.0 };
            plain(backend.scroll(tab_id, target, d.clone()).await)
        }
        PlannedAction::Select(t, value) => plain(backend.select(tab_id, t.clone(), value).await),
        PlannedAction::PressKey(key) => plain(backend.press_key(tab_id, key).await),
        PlannedAction::Wait {
            condition,
            timeout_ms,
        } => backend
            .wait_for(tab_id, condition, *timeout_ms)
            .await
            .map(|found| (if found { "found" } else { "not found" }, None))
            .map_err(|e| e.to_string()),
        PlannedAction::Snapshot { max_chars } => {
            read_guard(manager, backend, tab_id).await?;
            let snap = backend
                .snapshot(tab_id)
                .await
                .map_err(|e| super::backend_error_text(manager, &e))?;
            Ok((
                "ok",
                Some(snapshot_output(manager, &snap.snapshot_text, *max_chars)),
            ))
        }
        PlannedAction::Evaluate(js) => {
            read_guard(manager, backend, tab_id).await?;
            let raw = backend
                .evaluate(tab_id, js)
                .await
                .map_err(|e| super::backend_error_text(manager, &e))?;
            // The same unwrap-then-bound/redact/fence pipeline
            // `browser_evaluate` returns; it hands back a JSON string, which is
            // exactly what `output` is.
            let out = match super::process_evaluate_result(manager, &raw) {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            Ok(("ok", Some(out)))
        }
    }
}

#[async_trait]
impl AlephTool for BrowserExecTool {
    const NAME: &'static str = "browser_exec";
    const DESCRIPTION: &'static str =
        "Run one whole browser sub-procedure in ONE call: an ordered action list of navigate, \
         click/dblclick/type/fill/hover/scroll/select/press_key, wait, and the READS (snapshot, \
         evaluate). Prefer one call per sub-procedure — navigate, act, wait, then snapshot — \
         over a call per action; read output comes back in results[].output, so no extra call \
         is needed to see the page. Every step runs the same guards as its standalone tool \
         (navigation SSRF, input secret scan, output redaction, per-action approval), and \
         execution stops at the first failure reporting {completed, failed_at, results}. A ref \
         is only valid on the page it was captured from, so put a snapshot step after anything \
         that changes the page and target only refs read later in the same call; on failure, \
         snapshot again before retrying.";
    type Args = BrowserExecArgs;
    type Output = BrowserExecOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let total = args.actions.len();
        let refuse = |message: String| BrowserExecOutput {
            success: false,
            total,
            completed: 0,
            failed_at: None,
            results: Vec::new(),
            message: Some(message),
        };
        // Validate first: a malformed action list degrades to success:false
        // with the contract spelled out, never a hard Err — and never consumes
        // an approval or touches the page.
        let planned = match plan_actions(&args.actions) {
            Ok(p) => p,
            Err(message) => return Ok(refuse(message)),
        };

        // Secret pre-scan over EVERY text-bearing action, before any
        // execution: a procedure whose step 7 would be blocked by the
        // input-secret gate must be refused at step 0, not after six actions
        // already mutated the page. Unlike the approval gate this is a pure
        // predicate — it records nothing, so scanning ahead costs nothing a
        // later abort would have to give back.
        for action in &planned {
            let text = match action {
                PlannedAction::Type(_, text)
                | PlannedAction::Fill(_, text)
                | PlannedAction::Select(_, text) => text,
                _ => continue,
            };
            if let Some(message) = super::check_input_secret_block(&self.manager, text) {
                return Ok(refuse(message));
            }
        }

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                // The tab is resolved ONCE and every step acts on it, so a
                // `navigate` step moves this procedure's own page rather than
                // stranding later steps on a tab nobody is looking at.
                let (results, failure) = execute_actions(
                    &self.manager,
                    self.approval_policy.as_ref(),
                    backend.as_ref(),
                    &tab_id,
                    &planned,
                )
                .await;
                match failure {
                    None => Ok(BrowserExecOutput {
                        success: true,
                        total,
                        completed: total,
                        failed_at: None,
                        results,
                        message: Some(format!(
                            "All {total} actions completed in profile '{}'",
                            args.profile
                        )),
                    }),
                    Some((ordinal, err)) => {
                        let completed = ordinal - 1;
                        Ok(BrowserExecOutput {
                            success: false,
                            total,
                            completed,
                            failed_at: Some(ordinal),
                            results,
                            message: Some(format!(
                                "aborted at #{ordinal}: {err} — {completed}/{total} actions \
                                 completed; earlier refs may be stale, add a snapshot step \
                                 before retrying"
                            )),
                        })
                    }
                }
            }
            Err(e) => Ok(refuse(super::backend_error_text(&self.manager, &e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::browser::testkit::FakeBackend;

    /// An sk-ant-shaped credential — matched by the same `Critical` PII rules
    /// the input gate and the content redactor both consult.
    const FAKE_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";

    fn manager() -> ProfileManager {
        ProfileManager::new(BrowserSystemConfig::default())
    }

    /// A manager whose SSRF floor is off, for the tests whose subject is not
    /// the floor.
    ///
    /// `block_private` resolves the host, and a resolver that answers NXDOMAIN
    /// with a captive-portal address (198.18.0.0/15 on many home and corporate
    /// networks) makes even `https://example.com` "private" — so leaving it on
    /// here would make the happy-path assertions depend on whose DNS ran them.
    /// The tests that are about the floor use a literal loopback address,
    /// which needs no resolver at all.
    fn permissive_manager() -> ProfileManager {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = false;
        ProfileManager::new(config)
    }

    fn wait_action(fields: WaitFields) -> ExecAction {
        ExecAction::Wait {
            text: fields.text,
            text_gone: fields.text_gone,
            selector: fields.selector,
            url_contains: fields.url_contains,
            time_ms: fields.time_ms,
            timeout_ms: None,
        }
    }

    fn click(ref_id: &str) -> ExecAction {
        ExecAction::Click {
            ref_id: Some(ref_id.into()),
            x: None,
            y: None,
        }
    }

    /// Drive the planned actions with no approval policy wired.
    async fn run(
        manager: &ProfileManager,
        backend: &FakeBackend,
        planned: &[PlannedAction],
    ) -> (Vec<StepResult>, Option<(usize, String)>) {
        execute_actions(manager, None, backend, "1", planned).await
    }

    #[test]
    fn resolve_wait_maps_each_single_condition() {
        let cases = [
            (
                WaitFields {
                    text: Some("Done".into()),
                    ..Default::default()
                },
                WaitCondition::Text("Done".into()),
            ),
            (
                WaitFields {
                    text_gone: Some("Loading".into()),
                    ..Default::default()
                },
                WaitCondition::TextGone("Loading".into()),
            ),
            (
                WaitFields {
                    selector: Some("#app".into()),
                    ..Default::default()
                },
                WaitCondition::Selector("#app".into()),
            ),
            (
                WaitFields {
                    url_contains: Some("/done".into()),
                    ..Default::default()
                },
                WaitCondition::UrlContains("/done".into()),
            ),
            (
                WaitFields {
                    time_ms: Some(1500),
                    ..Default::default()
                },
                WaitCondition::Time(1500),
            ),
        ];
        for (fields, expected) in cases {
            assert_eq!(resolve_wait(&fields).unwrap(), expected);
        }
        // time_ms rides the same clamp window as browser_wait_for.
        let fields = WaitFields {
            time_ms: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(
            resolve_wait(&fields).unwrap(),
            WaitCondition::Time(crate::builtin_tools::browser_tools::wait_for::MAX_TIMEOUT_MS)
        );
    }

    #[test]
    fn resolve_wait_rejects_zero_and_multiple_conditions() {
        let err = resolve_wait(&WaitFields::default()).unwrap_err();
        assert!(err.contains("exactly one"), "got: {err}");
        let err = resolve_wait(&WaitFields {
            text: Some("a".into()),
            selector: Some("#b".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
        let err = resolve_wait(&WaitFields {
            time_ms: Some(1000),
            url_contains: Some("/x".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    #[test]
    fn plan_actions_rejects_empty_action_list() {
        let err = plan_actions(&[]).unwrap_err();
        assert!(err.contains("at least one"), "got: {err}");
    }

    #[test]
    fn plan_actions_rejects_oversized_action_list() {
        let actions: Vec<ExecAction> = (0..=MAX_EXEC_ACTIONS)
            .map(|_| ExecAction::PressKey {
                key: "Enter".into(),
            })
            .collect();
        let err = plan_actions(&actions).unwrap_err();
        assert!(err.contains("limited to 50"), "got: {err}");
    }

    #[test]
    fn plan_actions_rejects_bad_wait_combo() {
        let err = plan_actions(&[wait_action(WaitFields::default())]).unwrap_err();
        assert!(err.contains("exactly one"), "got: {err}");
    }

    #[test]
    fn plan_actions_rejects_targetless_click() {
        let err = plan_actions(&[ExecAction::Click {
            ref_id: None,
            x: None,
            y: None,
        }])
        .unwrap_err();
        assert!(err.contains("requires a target"), "got: {err}");
        // One coordinate without the other is also not a target.
        let err = plan_actions(&[ExecAction::Click {
            ref_id: None,
            x: Some(10.0),
            y: None,
        }])
        .unwrap_err();
        assert!(err.contains("requires a target"), "got: {err}");
    }

    /// A `type` with no ref means the focused element, exactly as in
    /// `browser_type` — not a coordinate target neither backend honours.
    #[test]
    fn plan_actions_types_into_the_focused_element_by_default() {
        let planned = plan_actions(&[ExecAction::Type {
            ref_id: None,
            text: "hi".into(),
        }])
        .unwrap();
        assert!(matches!(
            &planned[0],
            PlannedAction::Type(ActionTarget::Ref { ref_id }, _) if ref_id == "focused"
        ));
    }

    #[test]
    fn plan_actions_rejects_an_oversized_evaluate_script() {
        let err = plan_actions(&[ExecAction::Evaluate {
            js: "x".repeat(MAX_EVAL_SCRIPT_CHARS + 1),
        }])
        .unwrap_err();
        assert!(err.contains("the cap is"), "got: {err}");
    }

    /// `browser_evaluate` keeps a private copy of this cap. Until it imports
    /// [`MAX_EVAL_SCRIPT_CHARS`], this is what stops the two from drifting — a
    /// script accepted as a step but refused as a standalone call (or the
    /// reverse) is one tool telling the model two different things.
    ///
    /// CRLF-safe: `\r` is stripped before matching (CLAUDE.md §10).
    #[test]
    fn evaluate_script_cap_matches_the_standalone_tool() {
        let src = include_str!("evaluate.rs").replace('\r', "");
        let decl = src
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(&src)
            .lines()
            .find(|l| l.contains("MAX_EVAL_SCRIPT_CHARS") && l.contains('='))
            .expect("browser_evaluate must still declare MAX_EVAL_SCRIPT_CHARS")
            .to_string();
        assert!(
            decl.contains("64 * 1024"),
            "browser_evaluate's script cap drifted from browser_exec's \
             ({MAX_EVAL_SCRIPT_CHARS}): {decl}"
        );
    }

    #[test]
    fn plan_actions_resolves_targets_and_waits() {
        let planned = plan_actions(&[
            click("e5"),
            ExecAction::Click {
                ref_id: None,
                x: Some(1.0),
                y: Some(2.0),
            },
            wait_action(WaitFields {
                text: Some("Done".into()),
                ..Default::default()
            }),
        ])
        .unwrap();
        assert!(matches!(
            &planned[0],
            PlannedAction::Click(ActionTarget::Ref { ref_id }) if ref_id == "e5"
        ));
        assert!(matches!(
            &planned[1],
            PlannedAction::Click(ActionTarget::Coordinates { x, y }) if *x == 1.0 && *y == 2.0
        ));
        assert!(matches!(
            &planned[2],
            PlannedAction::Wait {
                condition: WaitCondition::Text(t),
                timeout_ms: DEFAULT_WAIT_TIMEOUT_MS,
            } if t == "Done"
        ));
    }

    /// The headline: navigate → act → wait → read, one call, reads included.
    #[tokio::test]
    async fn one_call_carries_a_navigate_act_read_procedure() {
        let backend = FakeBackend::new(None).with_snapshot_text("- searchbox [ref=e9]");
        let manager = permissive_manager();
        let planned = plan_actions(&[
            ExecAction::Navigate {
                url: "https://example.com/search".into(),
            },
            ExecAction::Type {
                ref_id: Some("e3".into()),
                text: "hello".into(),
            },
            ExecAction::PressKey {
                key: "Enter".into(),
            },
            wait_action(WaitFields {
                text: Some("Results".into()),
                ..Default::default()
            }),
            ExecAction::Snapshot { max_chars: None },
        ])
        .unwrap();

        let (results, failure) = run(&manager, &backend, &planned).await;
        assert!(failure.is_none(), "unexpected failure: {failure:?}");
        assert_eq!(
            backend.calls(),
            vec![
                "navigate:1:https://example.com/search",
                "type_text:hello",
                "press_key:Enter",
                "wait:Text(\"Results\")",
                // The read step's SSRF re-check, then the read itself.
                "list_tabs",
                "snapshot",
            ]
        );
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].status, "navigated");
        // Writes carry no output; the read does, and it is what the model
        // would otherwise have spent a whole extra turn to obtain.
        assert!(results[1].output.is_none());
        assert!(results[4]
            .output
            .as_deref()
            .is_some_and(|o| o.contains("[ref=e9]")));
        // The typed text is never echoed back in a step label.
        assert!(
            !results[1].action.contains("hello"),
            "got: {}",
            results[1].action
        );
    }

    /// A `navigate` step is not a hole in the SSRF wall: the blocked URL fails
    /// at its own ordinal and nothing after it runs.
    #[tokio::test]
    async fn a_blocked_navigate_step_aborts_the_procedure() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = ProfileManager::new(config);
        let backend = FakeBackend::new(None);
        let planned = plan_actions(&[
            click("e1"),
            ExecAction::Navigate {
                url: "http://127.0.0.1:8080/admin".into(),
            },
            click("e2"),
        ])
        .unwrap();

        let (results, failure) = run(&manager, &backend, &planned).await;
        let (ordinal, err) = failure.expect("a blocked navigation must abort");
        assert_eq!(ordinal, 2);
        assert!(err.contains("Blocked"), "got: {err}");
        assert_eq!(results.len(), 1);
        // The backend never saw the navigation, nor the click behind it.
        assert_eq!(backend.calls(), vec!["click:Ref{ref_id:\"e1\"}"]);
    }

    /// A read's payload crosses the same egress boundary the standalone read
    /// tools use: credentials scrubbed, page bytes fenced.
    #[tokio::test]
    async fn a_read_step_output_is_redacted_and_fenced() {
        let page = format!("- text \"api key {FAKE_KEY}\" [ref=e2]");
        let backend = FakeBackend::new(None).with_snapshot_text(page);
        let manager = permissive_manager();
        let planned = plan_actions(&[ExecAction::Snapshot { max_chars: None }]).unwrap();

        let (results, failure) = run(&manager, &backend, &planned).await;
        assert!(failure.is_none(), "unexpected failure: {failure:?}");
        let output = results[0].output.as_deref().expect("a snapshot step reads");
        assert!(
            !output.contains(FAKE_KEY),
            "the credential survived: {output}"
        );
        assert!(output.contains("[REDACTED:"), "got: {output}");
        assert!(
            output.contains(crate::security::content_sanitizer::FENCE_OPEN_PREFIX)
                && output.contains(crate::security::content_sanitizer::FENCE_CLOSE_PREFIX),
            "page content reached the model unfenced: {output}"
        );
    }

    /// The read-time guard runs per read step, so a page an earlier step (or a
    /// redirect) parked on a blocked origin cannot be read.
    #[tokio::test]
    async fn a_read_on_a_blocked_page_aborts_before_the_read() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = ProfileManager::new(config);
        let backend = FakeBackend::new(None).with_tabs_text("1: http://127.0.0.1:9000/internal");
        let planned = plan_actions(&[ExecAction::Snapshot { max_chars: None }]).unwrap();

        let (results, failure) = run(&manager, &backend, &planned).await;
        let (ordinal, err) = failure.expect("a read on a blocked page must abort");
        assert_eq!(ordinal, 1);
        assert!(err.contains("blocked by SSRF policy"), "got: {err}");
        assert!(results.is_empty());
        // list_tabs happened; the snapshot did not.
        assert_eq!(backend.calls(), vec!["list_tabs"]);
    }

    #[tokio::test]
    async fn execution_aborts_at_the_first_failure() {
        // The 3rd recorded call fails.
        let backend = FakeBackend::new(Some(3));
        let manager = manager();
        let planned = plan_actions(&[click("e1"), click("e2"), click("e3"), click("e4")]).unwrap();
        let (results, failure) = run(&manager, &backend, &planned).await;
        let (ordinal, err) = failure.expect("execution must abort");
        assert_eq!(ordinal, 3);
        assert!(err.contains("boom"), "got: {err}");
        // Two completed steps; the 4th action never ran.
        assert_eq!(results.len(), 2);
        assert_eq!(
            backend.calls(),
            vec![
                "click:Ref{ref_id:\"e1\"}",
                "click:Ref{ref_id:\"e2\"}",
                "click:Ref{ref_id:\"e3\"}",
            ]
        );
    }

    fn exec_args(actions: Vec<ExecAction>) -> BrowserExecArgs {
        BrowserExecArgs {
            profile: "default".into(),
            actions,
        }
    }

    #[tokio::test]
    async fn secret_pre_scan_refuses_before_any_execution() {
        // Default config has block_secrets_in_input = true; the credential in
        // the SECOND action's text must refuse the whole procedure before the
        // backend is even resolved (no browser running → any execution would
        // surface as a backend error, not this message).
        let tool = BrowserExecTool::new(Arc::new(manager()));
        let out = tool
            .call(exec_args(vec![
                click("e1"),
                ExecAction::Type {
                    ref_id: Some("e2".into()),
                    text: format!("token {FAKE_KEY}"),
                },
            ]))
            .await
            .unwrap();
        assert!(!out.success);
        assert_eq!(out.completed, 0);
        assert_eq!(out.failed_at, None);
        assert!(
            out.message
                .as_deref()
                .is_some_and(|m| m.contains("Blocked")),
            "got: {:?}",
            out.message
        );
    }

    /// A denied action fails at its own ordinal, and — the point of moving the
    /// gate next to the execution — the approvals the run never reached are
    /// never asked for and never recorded.
    #[tokio::test]
    async fn approval_is_decided_and_recorded_only_for_steps_that_run() {
        use crate::approval::{ActionRequest, ApprovalDecision};
        use std::sync::Mutex;

        /// Denies clicks, allows everything else, and remembers what it was asked.
        struct DenyClicks {
            checked: Mutex<Vec<ActionType>>,
            recorded: Mutex<Vec<ActionType>>,
        }
        #[async_trait]
        impl ApprovalPolicy for DenyClicks {
            async fn check(&self, req: &ActionRequest) -> ApprovalDecision {
                self.checked
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(req.action_type.clone());
                if req.action_type == ActionType::BrowserClick {
                    ApprovalDecision::Deny {
                        reason: "blocked in test".into(),
                    }
                } else {
                    ApprovalDecision::Allow
                }
            }
            async fn record(&self, req: &ActionRequest, _dec: &ApprovalDecision) {
                self.recorded
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(req.action_type.clone());
            }
        }

        let policy = Arc::new(DenyClicks {
            checked: Mutex::new(Vec::new()),
            recorded: Mutex::new(Vec::new()),
        });
        let backend = FakeBackend::new(None);
        let manager = manager();
        let planned = plan_actions(&[
            ExecAction::PressKey { key: "Tab".into() },
            click("e1"),
            ExecAction::PressKey {
                key: "Enter".into(),
            },
        ])
        .unwrap();

        let policy_dyn: Arc<dyn ApprovalPolicy> = policy.clone();
        let (results, failure) =
            execute_actions(&manager, Some(&policy_dyn), &backend, "1", &planned).await;
        let (ordinal, err) = failure.expect("a denied action must abort");
        assert_eq!(ordinal, 2);
        assert!(err.contains("denied by approval policy"), "got: {err}");
        assert_eq!(results.len(), 1);
        // Step 3 was never reached, so it was never judged...
        assert_eq!(
            *policy.checked.lock().unwrap_or_else(|e| e.into_inner()),
            vec![ActionType::BrowserPressKey, ActionType::BrowserClick]
        );
        // ...and the audit trail carries exactly the two decisions that were
        // made, not an approval for an action that never happened.
        assert_eq!(
            *policy.recorded.lock().unwrap_or_else(|e| e.into_inner()),
            vec![ActionType::BrowserPressKey, ActionType::BrowserClick]
        );
        // And the denied click never reached the page.
        assert_eq!(backend.calls(), vec!["press_key:Tab"]);
    }

    #[tokio::test]
    async fn invalid_action_list_is_graceful_failure() {
        let tool = BrowserExecTool::new(Arc::new(manager()));
        let out = tool.call(exec_args(Vec::new())).await.unwrap();
        assert!(!out.success);
        assert!(
            out.message
                .as_deref()
                .is_some_and(|m| m.contains("at least one")),
            "got: {:?}",
            out.message
        );
    }
}
