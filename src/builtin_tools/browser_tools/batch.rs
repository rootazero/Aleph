// Browser batch tool — runs a sequence of in-page actions in ONE tool call
// (openclaw `act batch` parity), aborting at the first failure.
//
// Why a batch tool at all: each single-action tool call costs a full model
// round-trip, and every action can invalidate the snapshot refs the next
// action targets. A form fill that needs click → type → press_key → wait is
// four round-trips as single tools, one as a batch. The batch resolves its
// tab ONCE and aborts on the first error so a half-applied sequence is
// reported, never silently continued.

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

/// Maximum actions in one batch. openclaw allows 100 per `act batch`; 50 is
/// the deliberate divergence — a batch runs inside a single tool call, and
/// 50 actions bounds that call's worst-case latency and output size without
/// giving up the round-trip savings the tool exists for.
const MAX_BATCH_ACTIONS: usize = 50;

/// Real wall-clock budget for a whole batch, enforced between actions.
/// openclaw spends an *estimated* per-action budget (a heuristic sum that can
/// drift far from reality on a slow page); this measures elapsed time, so a
/// batch of waits that each legitimately consume their clamped timeout still
/// terminates. Exceeding the budget aborts the batch and counts as the
/// failure point.
const MAX_BATCH_BUDGET_MS: u64 = 600_000;

/// Default per-wait timeout when a `wait` action omits `timeout_ms` — the
/// same 5s default `browser_wait_for` uses.
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 5000;

/// One action in a batch sequence. Serde's internal tag renders the variants
/// as `{"action": "click", ...}` / `{"action": "type", ...}` /
/// `{"action": "press_key", ...}` etc.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BatchAction {
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
    /// Type text into an element by snapshot ref_id or coordinates.
    Type {
        /// Accessibility `ref_id` from a previous snapshot.
        #[serde(default)]
        ref_id: Option<String>,
        /// X coordinate (requires `y`).
        #[serde(default)]
        x: Option<f64>,
        /// Y coordinate (requires `x`).
        #[serde(default)]
        y: Option<f64>,
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
    /// Wait for a page condition before continuing the batch. Exactly one of
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
}

/// Arguments for the `browser_batch` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserBatchArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Actions to run in order (max 50). The batch aborts at the first failure.
    pub actions: Vec<BatchAction>,
}

/// Output from the `browser_batch` tool.
#[derive(Debug, Serialize)]
pub struct BrowserBatchOutput {
    pub success: bool,
    /// Total actions requested.
    pub total: usize,
    /// Actions that completed successfully.
    pub completed: usize,
    /// 1-based ordinal of the action the batch aborted at (None on success).
    pub failed_at: Option<usize>,
    /// Per-action result lines for the completed prefix, e.g.
    /// `#3 click ref=e5: ok`.
    pub results: Vec<String>,
    pub message: Option<String>,
}

/// Runs a fixed sequence of in-page actions in one tool call.
#[derive(Clone)]
pub struct BrowserBatchTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserBatchTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate every action in the batch behind the user-defined approval policy.
    /// With no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

/// The wait-condition fields of a `wait` batch action, factored out of the
/// enum variant so [`resolve_wait`] can be exercised without wrapping every
/// test case in a [`BatchAction`]. `timeout_ms` is not part of the condition:
/// it is clamped at the plan site into [`PlannedAction::Wait`].
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
/// ambiguous in the result line). `time_ms` maps to a clamped
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

/// A batch action with its targeting and wait condition fully resolved —
/// everything [`execute_batch`] needs, with no validation left to do.
#[derive(Debug, Clone)]
enum PlannedAction {
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
}

/// Resolve a ref_id-or-coordinates target with `browser_click` semantics:
/// ref_id wins, both coordinates are required together, and CSS selector
/// targeting does not exist (refs come from `browser_snapshot`).
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
            "{action} requires a target: 'ref_id' from browser_snapshot, or both 'x' and 'y'"
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

/// Validate the whole batch and lower it to executable form. Fails BEFORE any
/// approval check or backend call — a malformed batch is a model mistake and
/// must not consume a user approval or touch the page.
fn plan_actions(actions: &[BatchAction]) -> std::result::Result<Vec<PlannedAction>, String> {
    if actions.is_empty() {
        return Err("batch requires at least one action".into());
    }
    if actions.len() > MAX_BATCH_ACTIONS {
        return Err(format!(
            "batch is limited to {MAX_BATCH_ACTIONS} actions per call (got {}); \
             split it into several batches",
            actions.len()
        ));
    }
    actions
        .iter()
        .map(|action| {
            let planned = match action {
                BatchAction::Click { ref_id, x, y } => {
                    PlannedAction::Click(resolve_xy_target("click", ref_id.as_ref(), *x, *y)?)
                }
                BatchAction::Dblclick { ref_id } => PlannedAction::Dblclick(ref_target(ref_id)),
                BatchAction::Type { ref_id, x, y, text } => PlannedAction::Type(
                    resolve_xy_target("type", ref_id.as_ref(), *x, *y)?,
                    text.clone(),
                ),
                BatchAction::Fill { ref_id, value } => {
                    PlannedAction::Fill(ref_target(ref_id), value.clone())
                }
                BatchAction::Hover { ref_id } => PlannedAction::Hover(ref_target(ref_id)),
                BatchAction::Scroll { direction } => PlannedAction::Scroll(direction.clone()),
                BatchAction::Select { ref_id, value } => {
                    PlannedAction::Select(ref_target(ref_id), value.clone())
                }
                BatchAction::PressKey { key } => PlannedAction::PressKey(key.clone()),
                BatchAction::Wait {
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
            };
            Ok(planned)
        })
        .collect()
}

/// Short human-readable label for the per-action result line. Deliberately
/// never echoes typed/filled text — only the secret gate vets those values,
/// and the result lines flow back into the model context.
fn action_label(action: &PlannedAction) -> String {
    let target = |t: &ActionTarget| match t {
        ActionTarget::Ref { ref_id } => format!("ref={ref_id}"),
        ActionTarget::Coordinates { x, y } => format!("x={x} y={y}"),
    };
    match action {
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
    }
}

/// The approval surface of one planned action: its existing per-action
/// [`ActionType`] and a target string for the policy. `Wait` has no approval
/// surface (read-only polling) and maps to `None`.
///
/// Deliberate: the batch introduces NO new ActionType knob — it inherits the
/// per-action policy semantics of the single tools, so a policy that governs
/// `browser_click` governs a batched click identically.
fn approval_surface(action: &PlannedAction) -> Option<(ActionType, &'static str, String)> {
    let target = |t: &ActionTarget| format!("{t:?}");
    match action {
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
        PlannedAction::Wait { .. } => None,
    }
}

/// Run the planned actions in order against the already-resolved tab.
///
/// Returns the per-action result lines plus `Some((ordinal, error))` at the
/// first failure — the ordinal is 1-based, matching what the model sees in
/// the result lines. A wait that times out (`Ok(false)`) is NOT a failure:
/// absence is an answer, the line records `not found` and the batch
/// continues. The wall-clock budget is checked before each action; running
/// out of budget aborts with that action's ordinal as the failure point.
async fn execute_batch(
    backend: &dyn BrowserBackend,
    tab_id: &str,
    planned: &[PlannedAction],
) -> (Vec<String>, Option<(usize, String)>) {
    let started = std::time::Instant::now();
    let mut results = Vec::with_capacity(planned.len());
    for (i, action) in planned.iter().enumerate() {
        let ordinal = i + 1;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        if elapsed_ms >= MAX_BATCH_BUDGET_MS {
            return (
                results,
                Some((
                    ordinal,
                    format!(
                        "batch wall-clock budget of {}ms exhausted after {} actions",
                        MAX_BATCH_BUDGET_MS, i
                    ),
                )),
            );
        }
        let label = action_label(action);
        let outcome: std::result::Result<&'static str, crate::browser::BrowserError> = match action
        {
            PlannedAction::Click(t) => backend.click(tab_id, t.clone()).await.map(|_| "ok"),
            PlannedAction::Dblclick(t) => backend.dblclick(tab_id, t.clone()).await.map(|_| "ok"),
            PlannedAction::Type(t, text) => backend
                .type_text(tab_id, t.clone(), text)
                .await
                .map(|_| "ok"),
            PlannedAction::Fill(t, value) => {
                backend.fill(tab_id, t.clone(), value).await.map(|_| "ok")
            }
            PlannedAction::Hover(t) => backend.hover(tab_id, t.clone()).await.map(|_| "ok"),
            PlannedAction::Scroll(d) => {
                // Viewport scroll: the backend ignores the target, so a
                // viewport-origin placeholder is passed (same as browser_scroll).
                let target = ActionTarget::Coordinates { x: 0.0, y: 0.0 };
                backend
                    .scroll(tab_id, target, d.clone())
                    .await
                    .map(|_| "ok")
            }
            PlannedAction::Select(t, value) => {
                backend.select(tab_id, t.clone(), value).await.map(|_| "ok")
            }
            PlannedAction::PressKey(key) => backend.press_key(tab_id, key).await.map(|_| "ok"),
            PlannedAction::Wait {
                condition,
                timeout_ms,
            } => backend
                .wait_for(tab_id, condition, *timeout_ms)
                .await
                .map(|found| if found { "found" } else { "not found" }),
        };
        match outcome {
            Ok(detail) => results.push(format!("#{ordinal} {label}: {detail}")),
            Err(e) => return (results, Some((ordinal, e.to_string()))),
        }
    }
    (results, None)
}

#[async_trait]
impl AlephTool for BrowserBatchTool {
    const NAME: &'static str = "browser_batch";
    const DESCRIPTION: &'static str =
        "Run a sequence of page actions (click, dblclick, type, fill, hover, scroll, select, \
         press_key, wait) in one call, in order, aborting at the first failure. Use after \
         browser_snapshot to act on several refs without a round-trip per action. Refs can go \
         stale mid-batch — on failure, take a fresh browser_snapshot before retrying.";
    type Args = BrowserBatchArgs;
    type Output = BrowserBatchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let total = args.actions.len();
        // Validate first: a malformed batch degrades to success:false with the
        // contract spelled out, never a hard Err — and never consumes an
        // approval or touches the page.
        let planned = match plan_actions(&args.actions) {
            Ok(p) => p,
            Err(message) => {
                return Ok(BrowserBatchOutput {
                    success: false,
                    total,
                    completed: 0,
                    failed_at: None,
                    results: Vec::new(),
                    message: Some(message),
                });
            }
        };

        // Secret pre-scan over EVERY text-bearing action, before approval and
        // before any execution: a batch whose step 7 would be blocked by the
        // input-secret gate must be refused at step 0, not after six actions
        // already mutated the page.
        for action in &planned {
            let text = match action {
                PlannedAction::Type(_, text)
                | PlannedAction::Fill(_, text)
                | PlannedAction::Select(_, text) => text,
                _ => continue,
            };
            if let Some(message) = super::check_input_secret_block(&self.manager, text) {
                return Ok(BrowserBatchOutput {
                    success: false,
                    total,
                    completed: 0,
                    failed_at: None,
                    results: Vec::new(),
                    message: Some(message),
                });
            }
        }

        // Approvals run UPFRONT, in action order, against each action's
        // existing ActionType — the batch inherits per-action policy semantics
        // (no new ActionType knob, deliberate). A denied or needs-confirmation
        // action fails the whole batch before anything executes.
        for action in &planned {
            let Some((action_type, verb, target)) = approval_surface(action) else {
                continue;
            };
            if let Some(message) = super::check_browser_approval(
                self.approval_policy.as_ref(),
                action_type,
                verb,
                &target,
            )
            .await
            {
                return Ok(BrowserBatchOutput {
                    success: false,
                    total,
                    completed: 0,
                    failed_at: None,
                    results: Vec::new(),
                    message: Some(message),
                });
            }
        }

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                // The tab is resolved ONCE. Actions that change the active tab
                // mid-batch (a link opening a new tab) still act on the
                // resolved tab — backends operate on their current/selected page.
                let (results, failure) = execute_batch(backend.as_ref(), &tab_id, &planned).await;
                match failure {
                    None => Ok(BrowserBatchOutput {
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
                        Ok(BrowserBatchOutput {
                            success: false,
                            total,
                            completed,
                            failed_at: Some(ordinal),
                            results,
                            message: Some(format!(
                                "aborted at #{ordinal}: {err} — {completed}/{total} actions \
                                 completed; earlier refs may be stale, take a fresh \
                                 browser_snapshot"
                            )),
                        })
                    }
                }
            }
            Err(e) => Ok(BrowserBatchOutput {
                success: false,
                total,
                completed: 0,
                failed_at: None,
                results: Vec::new(),
                message: Some(format!("{e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::browser::testkit::FakeBackend;

    fn wait_action(fields: WaitFields) -> BatchAction {
        BatchAction::Wait {
            text: fields.text,
            text_gone: fields.text_gone,
            selector: fields.selector,
            url_contains: fields.url_contains,
            time_ms: fields.time_ms,
            timeout_ms: None,
        }
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
    fn plan_actions_rejects_empty_batch() {
        let err = plan_actions(&[]).unwrap_err();
        assert!(err.contains("at least one"), "got: {err}");
    }

    #[test]
    fn plan_actions_rejects_oversized_batch() {
        let actions: Vec<BatchAction> = (0..=MAX_BATCH_ACTIONS)
            .map(|_| BatchAction::PressKey {
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
    fn plan_actions_rejects_targetless_click_and_type() {
        let err = plan_actions(&[BatchAction::Click {
            ref_id: None,
            x: None,
            y: None,
        }])
        .unwrap_err();
        assert!(err.contains("requires a target"), "got: {err}");
        // One coordinate without the other is also not a target.
        let err = plan_actions(&[BatchAction::Type {
            ref_id: None,
            x: Some(10.0),
            y: None,
            text: "hi".into(),
        }])
        .unwrap_err();
        assert!(err.contains("requires a target"), "got: {err}");
    }

    #[test]
    fn plan_actions_resolves_targets_and_waits() {
        let planned = plan_actions(&[
            BatchAction::Click {
                ref_id: Some("e5".into()),
                x: None,
                y: None,
            },
            BatchAction::Click {
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

    #[tokio::test]
    async fn execute_batch_runs_actions_in_order() {
        let backend = FakeBackend::new(None);
        let planned = plan_actions(&[
            BatchAction::Click {
                ref_id: Some("e5".into()),
                x: None,
                y: None,
            },
            BatchAction::Type {
                ref_id: Some("e3".into()),
                x: None,
                y: None,
                text: "hello".into(),
            },
            BatchAction::PressKey {
                key: "Enter".into(),
            },
            wait_action(WaitFields {
                text: Some("x".into()),
                ..Default::default()
            }),
        ])
        .unwrap();
        let (results, failure) = execute_batch(&backend, "1", &planned).await;
        assert!(failure.is_none(), "unexpected failure: {failure:?}");
        assert_eq!(
            backend.calls(),
            vec![
                "click:Ref{ref_id:\"e5\"}",
                "type_text:hello",
                "press_key:Enter",
                "wait:Text(\"x\")",
            ]
        );
        assert_eq!(results.len(), 4);
        assert_eq!(results[0], "#1 click ref=e5: ok");
        assert_eq!(results[3], "#4 wait text 'x': found");
    }

    #[tokio::test]
    async fn execute_batch_aborts_at_first_failure() {
        // The 3rd recorded call fails.
        let backend = FakeBackend::new(Some(3));
        let planned = plan_actions(&[
            BatchAction::Click {
                ref_id: Some("e1".into()),
                x: None,
                y: None,
            },
            BatchAction::Click {
                ref_id: Some("e2".into()),
                x: None,
                y: None,
            },
            BatchAction::Click {
                ref_id: Some("e3".into()),
                x: None,
                y: None,
            },
            BatchAction::Click {
                ref_id: Some("e4".into()),
                x: None,
                y: None,
            },
        ])
        .unwrap();
        let (results, failure) = execute_batch(&backend, "1", &planned).await;
        let (ordinal, err) = failure.expect("batch must abort");
        assert_eq!(ordinal, 3);
        assert!(err.contains("boom"), "got: {err}");
        // Two completed lines; the 4th action never ran.
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

    fn batch_args(actions: Vec<BatchAction>) -> BrowserBatchArgs {
        BrowserBatchArgs {
            profile: "default".into(),
            actions,
        }
    }

    #[tokio::test]
    async fn secret_pre_scan_refuses_before_any_execution() {
        // Default config has block_secrets_in_input = true; the credential in
        // the SECOND action's text must refuse the whole batch before the
        // backend is even resolved (no browser running → any execution would
        // surface as a backend error, not this message).
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserBatchTool::new(manager);
        let out = tool
            .call(batch_args(vec![
                BatchAction::Click {
                    ref_id: Some("e1".into()),
                    x: None,
                    y: None,
                },
                BatchAction::Type {
                    ref_id: Some("e2".into()),
                    x: None,
                    y: None,
                    text: "token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789".into(),
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

    #[tokio::test]
    async fn approval_runs_upfront_and_denies_before_execution() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        // Deny clicks outright; the batch must fail before any backend call.
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserClick, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserBatchTool::new(manager).with_approval_policy(policy);
        let out = tool
            .call(batch_args(vec![BatchAction::Click {
                ref_id: Some("e1".into()),
                x: None,
                y: None,
            }]))
            .await
            .unwrap();
        assert!(!out.success);
        assert_eq!(out.completed, 0);
        assert!(
            out.message
                .as_deref()
                .is_some_and(|m| m.contains("denied by approval policy")),
            "got: {:?}",
            out.message
        );
    }

    #[tokio::test]
    async fn invalid_batch_is_graceful_failure() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserBatchTool::new(manager);
        let out = tool.call(batch_args(Vec::new())).await.unwrap();
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
