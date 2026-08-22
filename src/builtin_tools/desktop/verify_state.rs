//! `verify_state` action — a bounded, tri-state accessibility postcondition
//! check.
//!
//! After a mutating action, "did it land?" is usually answered today by a fixed
//! settle delay plus a screenshot the model squints at. `verify_state` replaces
//! the guess with a structured question over the target app's accessibility
//! tree: assert one to eight predicates (an element exists, holds focus, or
//! carries a value), poll until they hold *stably* or a bounded window elapses,
//! and get back a tri-state verdict per predicate.
//!
//! The design is ported from cua-driver's `verify_state`, and keeps its three
//! load-bearing decisions:
//!
//! 1. **`unknown` is a first-class outcome and never promotes to success.** An
//!    observation that could not prove either answer (no tree, an ambiguous
//!    match, a value withheld because the field is secure) is `unknown`, not a
//!    pass and not a fail.
//! 2. **Only presence can be asserted.** An AX walk is node-budgeted and never
//!    exhaustive, so "no element matches" is `unknown`, not `false`. There is no
//!    "absent" predicate — see [`StateAssertion`].
//! 3. **Settling is a parameter, not a magic sleep.** `stable_samples` says how
//!    many consecutive satisfied samples are required, and the result reports
//!    how long it actually took (`elapsed_ms`, `samples`).
//!
//! Pure orchestration over the existing [`AccessibilityCapability`] — no new
//! trait methods, no platform code touched (R10-aligned thin harness). Secure
//! values are read through [`safe_value`], so a password field's contents are
//! never compared or echoed: a value predicate over a secure element resolves
//! to `unknown`, never to a confirmed match.

use std::time::{Duration, Instant};

use aleph_desktop::AccessibilityCapability;
use aleph_protocol::desktop_bridge::methods::ax::{
    AxElement, QueryFocusedParams, QueryTreeParams, DEFAULT_MAX_NODES,
};
use serde_json::json;

use super::interactable::safe_value;
use super::types::{DesktopOutput, StateAssertion, StatePredicate};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
/// A postcondition check is a foreground-blocking loop that re-walks another
/// process's AX tree every poll; 10 s is a generous ceiling for "did the UI
/// settle" without letting it monopolise the turn. (cua-driver caps at the same
/// 10 s.)
const MAX_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_STABLE_SAMPLES: u64 = 2;
const MAX_STABLE_SAMPLES: u64 = 5;
/// Interval between samples. Long enough not to hammer the bridge with a full
/// tree walk on every tick, short enough that a satisfied state is noticed
/// promptly.
const POLL_MS: u64 = 300;
const MAX_PREDICATES: usize = 8;
/// Depth for the verification walk. Deeper than the default read (6) because a
/// postcondition often names a control buried in a form, list or web view; the
/// node budget ([`DEFAULT_MAX_NODES`]) still bounds the total work per sample.
const VERIFY_DEPTH: u32 = 32;

/// The status of one predicate against one sample of the tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PredStatus {
    /// Proven to hold.
    Satisfied,
    /// Proven not to hold (the element was observed and did not match / the
    /// value was read and differed / focus was read and belonged elsewhere).
    Unsatisfied,
    /// The observation could not prove either answer. Carries the reason.
    Unknown(UnknownReason),
}

/// Closed set of reasons a predicate is [`PredStatus::Unknown`]. A closed enum
/// is what keeps "we could not tell" from ever quietly becoming "it passed".
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnknownReason {
    /// No element matched the selector, and a bounded walk cannot prove absence.
    TargetMissing,
    /// More than one element matched a predicate that needs exactly one (a
    /// value assertion), so which value to check is ambiguous.
    MultiMatch,
    /// The tree / focus could not be read this sample (bridge error), or the
    /// matched element's value is withheld because the field is secure.
    ObservationUnavailable,
}

impl UnknownReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TargetMissing => "target_missing",
            Self::MultiMatch => "multi_match",
            Self::ObservationUnavailable => "observation_unavailable",
        }
    }
}

/// One predicate's evaluation against one sample, kept for the final report.
struct PredEval {
    assert: StateAssertion,
    status: PredStatus,
    /// How many elements matched the selector this sample (for the report; also
    /// what distinguishes a `target_missing` from a `multi_match`).
    matched: usize,
}

/// What one sample observed about the target app, fetched once and shared
/// across all predicates so a check with N predicates still costs one tree walk
/// (plus one focus query when any predicate needs it).
struct Observation {
    /// The tree root, or `None` when the app is inaccessible.
    root: Option<AxElement>,
    /// Whether `query_tree` returned `Ok` at all (an empty `Ok` tree is a real
    /// observation; an `Err` is not).
    tree_ok: bool,
    focus: FocusObs,
}

/// The focus half of an [`Observation`].
enum FocusObs {
    /// No predicate needed focus, so it was not queried.
    NotQueried,
    /// The focus query failed (bridge error).
    Failed,
    /// The focus query succeeded: `Some(el)` is the focused element, `None`
    /// means the app reported nothing focused.
    Read(Option<AxElement>),
}

/// Run a `verify_state` check. Assumes the AX capability is present — the
/// dispatcher refuses with a clear message when `platform.ax()` is `None`,
/// mirroring the other AX tools.
pub async fn run_verify_state(
    ax: &dyn AccessibilityCapability,
    pid: Option<i32>,
    expect: &[StatePredicate],
    timeout_ms: Option<u64>,
    stable_samples: Option<u64>,
) -> DesktopOutput {
    if let Err(message) = validate(expect) {
        return DesktopOutput {
            success: false,
            data: None,
            message: Some(super::recovery::with_hint(message)),
        };
    }

    let timeout =
        Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS));
    // Zero means "sample once and report" — an instantaneous check with no
    // stability loop, which the caller uses to read current state rather than
    // wait for it.
    let single = timeout.is_zero();
    let need_stable = stable_samples
        .unwrap_or(DEFAULT_STABLE_SAMPLES)
        .clamp(1, MAX_STABLE_SAMPLES);
    let poll = Duration::from_millis(POLL_MS);

    let start = Instant::now();
    let mut samples: u64 = 0;
    let mut consecutive: u64 = 0;

    // A park owes an arm to a mid-loop steer: the user's message is durably in
    // the session log and answered at the running turn's next boundary, and for
    // up to `MAX_TIMEOUT_MS` this loop *is* that turn. Same rule and same seam
    // as `wait_visual` — hooked to the inter-sample sleep, not wrapped around
    // the whole call, so a sample already in flight is never abandoned. See
    // `crate::session::steer_signal`.
    let mut steer = crate::session::steer_signal::watch_current_turn();

    loop {
        samples += 1;
        let evals = sample(ax, pid, expect).await;
        let overall = fold(&evals);
        let satisfied = overall == PredStatus::Satisfied;
        consecutive = if satisfied { consecutive + 1 } else { 0 };

        if single {
            return report(satisfied, overall, samples, start.elapsed(), &evals, None);
        }
        if satisfied && consecutive >= need_stable {
            return report(true, overall, samples, start.elapsed(), &evals, None);
        }
        if start.elapsed() >= timeout {
            return report(
                false,
                overall,
                samples,
                start.elapsed(),
                &evals,
                Some("timeout"),
            );
        }

        tokio::select! {
            biased;
            () = steer.steered() => {
                // Not a verdict — we stopped looking. `unknown` is the honest
                // status for "observation cut short", distinct from the timeout
                // arm which watched the whole window.
                return report(
                    false,
                    PredStatus::Unknown(UnknownReason::ObservationUnavailable),
                    samples,
                    start.elapsed(),
                    &evals,
                    Some("user_input"),
                );
            }
            () = tokio::time::sleep(poll) => {}
        }
    }
}

/// Validate the predicate list at the request boundary (fail fast, P7).
fn validate(expect: &[StatePredicate]) -> Result<(), String> {
    if expect.is_empty() {
        return Err("verify_state needs at least one `expect` predicate (e.g. \
             {assert:'exists', role:'AXButton', title:'Save'})."
            .to_string());
    }
    if expect.len() > MAX_PREDICATES {
        return Err(format!(
            "verify_state takes at most {MAX_PREDICATES} predicates; got {}. Split the check.",
            expect.len()
        ));
    }
    for (i, p) in expect.iter().enumerate() {
        let has_selector = p.role.is_some() || p.title.is_some() || p.title_contains.is_some();
        match p.assert {
            StateAssertion::ValueEquals | StateAssertion::ValueContains => {
                if p.value.is_none() {
                    return Err(format!(
                        "predicate {i} asserts a value but has no `value` to compare against."
                    ));
                }
                if !has_selector {
                    return Err(format!(
                        "predicate {i} needs a selector (role / title / title_contains) so it \
                         names which element's value to read."
                    ));
                }
            }
            StateAssertion::Exists => {
                if !has_selector {
                    return Err(format!(
                        "predicate {i} asserts existence but names no element — give it a \
                         role / title / title_contains selector (asserting that *anything* \
                         exists is always trivially true)."
                    ));
                }
            }
            // `focused` with no selector is meaningful: "something holds focus".
            StateAssertion::Focused => {}
        }
    }
    Ok(())
}

/// Take one observation of the app and evaluate every predicate against it.
async fn sample(
    ax: &dyn AccessibilityCapability,
    pid: Option<i32>,
    expect: &[StatePredicate],
) -> Vec<PredEval> {
    let (root, tree_ok) = match ax
        .query_tree(QueryTreeParams {
            pid,
            max_depth: VERIFY_DEPTH,
            max_nodes: DEFAULT_MAX_NODES,
        })
        .await
    {
        Ok(result) => (result.element, true),
        Err(_) => (None, false),
    };

    let need_focus = expect.iter().any(|p| p.assert == StateAssertion::Focused);
    let focus = if need_focus {
        match ax.query_focused(QueryFocusedParams { pid }).await {
            Ok(element) => FocusObs::Read(element),
            Err(_) => FocusObs::Failed,
        }
    } else {
        FocusObs::NotQueried
    };

    let obs = Observation {
        root,
        tree_ok,
        focus,
    };
    expect.iter().map(|p| eval(p, &obs)).collect()
}

/// Evaluate one predicate against one observation.
fn eval(p: &StatePredicate, obs: &Observation) -> PredEval {
    match p.assert {
        StateAssertion::Exists => {
            let n = obs.root.as_ref().map_or(0, |root| count_matches(root, p));
            let status = if n >= 1 {
                PredStatus::Satisfied
            } else if obs.tree_ok {
                // The walk succeeded and found nothing — but a node-budgeted
                // walk cannot prove absence, so this is unknown, not false.
                PredStatus::Unknown(UnknownReason::TargetMissing)
            } else {
                PredStatus::Unknown(UnknownReason::ObservationUnavailable)
            };
            PredEval {
                assert: p.assert,
                status,
                matched: n,
            }
        }
        StateAssertion::Focused => {
            let (status, matched) = match &obs.focus {
                FocusObs::Read(Some(el)) => {
                    if element_matches(el, p) {
                        (PredStatus::Satisfied, 1)
                    } else {
                        // Focus was read and belongs to a different element:
                        // a provable negative.
                        (PredStatus::Unsatisfied, 1)
                    }
                }
                // The app was asked and reported nothing focused — provable.
                FocusObs::Read(None) => (PredStatus::Unsatisfied, 0),
                FocusObs::Failed | FocusObs::NotQueried => (
                    PredStatus::Unknown(UnknownReason::ObservationUnavailable),
                    0,
                ),
            };
            PredEval {
                assert: p.assert,
                status,
                matched,
            }
        }
        StateAssertion::ValueEquals | StateAssertion::ValueContains => {
            let matches: Vec<&AxElement> = obs
                .root
                .as_ref()
                .map(|root| collect_matches(root, p))
                .unwrap_or_default();
            let status = match matches.as_slice() {
                [] if obs.tree_ok => PredStatus::Unknown(UnknownReason::TargetMissing),
                [] => PredStatus::Unknown(UnknownReason::ObservationUnavailable),
                [only] => value_status(only, p),
                _ => PredStatus::Unknown(UnknownReason::MultiMatch),
            };
            PredEval {
                assert: p.assert,
                status,
                matched: matches.len(),
            }
        }
    }
}

/// Compare a single matched element's value against the predicate. The value is
/// read through [`safe_value`], so a secure field withholds its contents and
/// the result is `unknown` — never a confirmed match on a password.
fn value_status(el: &AxElement, p: &StatePredicate) -> PredStatus {
    let Some(expected) = p.value.as_deref() else {
        // Guarded at the boundary; defensive.
        return PredStatus::Unknown(UnknownReason::ObservationUnavailable);
    };
    let Some(actual) = safe_value(el) else {
        return PredStatus::Unknown(UnknownReason::ObservationUnavailable);
    };
    let hit = match p.assert {
        StateAssertion::ValueEquals => actual == expected,
        StateAssertion::ValueContains => actual.contains(expected),
        _ => false,
    };
    if hit {
        PredStatus::Satisfied
    } else {
        PredStatus::Unsatisfied
    }
}

/// Does one element satisfy the predicate's selector (role / title /
/// title_contains, all ANDed)? Role and title compare case-insensitively.
fn element_matches(el: &AxElement, p: &StatePredicate) -> bool {
    if let Some(role) = p.role.as_deref() {
        if !el.role.eq_ignore_ascii_case(role) {
            return false;
        }
    }
    if let Some(title) = p.title.as_deref() {
        match el.title.as_deref() {
            Some(actual) if actual.eq_ignore_ascii_case(title) => {}
            _ => return false,
        }
    }
    if let Some(needle) = p.title_contains.as_deref() {
        let hit = el
            .title
            .as_deref()
            .is_some_and(|actual| actual.to_lowercase().contains(&needle.to_lowercase()));
        if !hit {
            return false;
        }
    }
    true
}

/// Count elements in the subtree that match the selector.
fn count_matches(root: &AxElement, p: &StatePredicate) -> usize {
    let mut n = 0;
    walk(root, &mut |el| {
        if element_matches(el, p) {
            n += 1;
        }
    });
    n
}

/// Collect references to elements in the subtree that match the selector.
fn collect_matches<'a>(root: &'a AxElement, p: &StatePredicate) -> Vec<&'a AxElement> {
    let mut out = Vec::new();
    walk(root, &mut |el| {
        if element_matches(el, p) {
            out.push(el);
        }
    });
    out
}

/// Depth-first walk over an element and its descendants.
fn walk<'a>(node: &'a AxElement, visit: &mut impl FnMut(&'a AxElement)) {
    visit(node);
    for child in &node.children {
        walk(child, visit);
    }
}

/// Fold per-predicate statuses (ANDed) into one overall status. A provable
/// `unsatisfied` wins over `unknown`, because the conjunction is definitively
/// false the moment one conjunct is — and `unsatisfied` is the more actionable
/// answer. `unknown` never yields `satisfied`.
fn fold(evals: &[PredEval]) -> PredStatus {
    let mut unknown: Option<UnknownReason> = None;
    for e in evals {
        match e.status {
            PredStatus::Unsatisfied => return PredStatus::Unsatisfied,
            PredStatus::Unknown(r) => unknown = unknown.or(Some(r)),
            PredStatus::Satisfied => {}
        }
    }
    match unknown {
        Some(r) => PredStatus::Unknown(r),
        None => PredStatus::Satisfied,
    }
}

/// Build the structured `DesktopOutput`. `success` is always `true`: a verdict —
/// including `unsatisfied` and `unknown` — is a successful observation, and the
/// caller reads `status`. Only a malformed request (handled in [`validate`])
/// returns `success: false`.
fn report(
    stable: bool,
    overall: PredStatus,
    samples: u64,
    elapsed: Duration,
    evals: &[PredEval],
    reason: Option<&str>,
) -> DesktopOutput {
    let status_str = status_word(overall);
    let predicates: Vec<serde_json::Value> = evals
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (word, unknown_reason) = match e.status {
                PredStatus::Satisfied => ("satisfied", None),
                PredStatus::Unsatisfied => ("unsatisfied", None),
                PredStatus::Unknown(r) => ("unknown", Some(r.as_str())),
            };
            json!({
                "index": i,
                "assert": e.assert,
                "status": word,
                "unknown_reason": unknown_reason,
                "matched": e.matched,
            })
        })
        .collect();

    DesktopOutput {
        success: true,
        data: Some(json!({
            "status": status_str,
            "stable": stable,
            "elapsed_ms": elapsed.as_millis() as u64,
            "samples": samples,
            "predicates": predicates,
            "reason": reason,
        })),
        message: message_for(overall, reason),
    }
}

const fn status_word(status: PredStatus) -> &'static str {
    match status {
        PredStatus::Satisfied => "satisfied",
        PredStatus::Unsatisfied => "unsatisfied",
        PredStatus::Unknown(_) => "unknown",
    }
}

fn message_for(overall: PredStatus, reason: Option<&str>) -> Option<String> {
    if reason == Some("user_input") {
        return Some(
            "verify_state: the user sent new input, so this check returned early instead of \
             watching for the full window — their message is in your context, read it first. \
             The postconditions were NOT confirmed and NOT refuted; nothing was observed after \
             this point."
                .to_string(),
        );
    }
    match overall {
        PredStatus::Satisfied => None,
        PredStatus::Unsatisfied => Some(super::recovery::with_hint(
            "verify_state: the postconditions did not hold — the state you expected is not the \
             state that is there. Re-observe (screenshot / ax_snapshot) before acting."
                .to_string(),
        )),
        PredStatus::Unknown(_) => Some(super::recovery::with_hint(
            "verify_state: could not confirm the postconditions. `unknown` means the observation \
             could not prove either answer (element not found within the node budget, an \
             ambiguous match, or a value withheld as secure) — it is never a pass. Narrow the \
             selector, name a `pid`, or re-observe."
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::sync_primitives::Mutex;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use aleph_desktop::Result as DResult;
    use aleph_protocol::desktop_bridge::methods::ax::QueryResult;
    use async_trait::async_trait;

    /// A scripted AX capability: returns a fixed sequence of tree roots (one per
    /// sample) and a fixed focused element. `None` in the tree sequence models a
    /// failed/empty observation.
    struct ScriptedAx {
        trees: Mutex<Vec<Option<AxElement>>>,
        idx: Mutex<usize>,
        focused: Option<AxElement>,
        tree_err: bool,
    }

    impl ScriptedAx {
        fn with_trees(trees: Vec<Option<AxElement>>) -> Self {
            Self {
                trees: Mutex::new(trees),
                idx: Mutex::new(0),
                focused: None,
                tree_err: false,
            }
        }
        fn focused(mut self, el: Option<AxElement>) -> Self {
            self.focused = el;
            self
        }
        fn tree_err(mut self) -> Self {
            self.tree_err = true;
            self
        }
    }

    #[async_trait]
    impl AccessibilityCapability for ScriptedAx {
        async fn query_focused(&self, _p: QueryFocusedParams) -> DResult<Option<AxElement>> {
            Ok(self.focused.clone())
        }
        async fn query_tree(&self, _p: QueryTreeParams) -> DResult<QueryResult> {
            if self.tree_err {
                return Err(aleph_desktop::DesktopError::NotImplemented(
                    "query_tree".into(),
                ));
            }
            let mut idx = self.idx.lock().unwrap_or_else(|e| e.into_inner());
            let trees = self.trees.lock().unwrap_or_else(|e| e.into_inner());
            let i = (*idx).min(trees.len().saturating_sub(1));
            *idx = (*idx + 1).min(trees.len().saturating_sub(1));
            Ok(QueryResult {
                element: trees.get(i).cloned().flatten(),
                node_count: 1,
                truncated: false,
            })
        }
        async fn query_by_role(
            &self,
            _p: aleph_protocol::desktop_bridge::methods::ax::QueryByRoleParams,
        ) -> DResult<aleph_protocol::desktop_bridge::methods::ax::QueryListResult> {
            unimplemented!()
        }
    }

    fn el(role: &str, title: Option<&str>) -> AxElement {
        AxElement {
            role: role.to_string(),
            title: title.map(str::to_string),
            ..Default::default()
        }
    }

    fn tree(role: &str, children: Vec<AxElement>) -> AxElement {
        AxElement {
            role: role.to_string(),
            children,
            ..Default::default()
        }
    }

    fn exists(role: &str, title: &str) -> StatePredicate {
        StatePredicate {
            assert: StateAssertion::Exists,
            role: Some(role.to_string()),
            title: Some(title.to_string()),
            title_contains: None,
            value: None,
        }
    }

    #[tokio::test]
    async fn satisfied_when_the_element_is_present_and_stable() {
        let root = tree("AXWindow", vec![el("AXButton", Some("Save"))]);
        let ax = ScriptedAx::with_trees(vec![Some(root.clone()), Some(root.clone()), Some(root)]);
        let out = run_verify_state(
            &ax,
            None,
            &[exists("AXButton", "Save")],
            Some(5_000),
            Some(2),
        )
        .await;
        assert!(out.success);
        let data = out.data.unwrap();
        assert_eq!(data["status"], "satisfied");
        assert_eq!(data["stable"], serde_json::Value::Bool(true));
        // Two consecutive satisfied samples were required.
        assert!(data["samples"].as_u64().unwrap() >= 2);
    }

    #[tokio::test]
    async fn missing_element_is_unknown_not_unsatisfied() {
        // A bounded walk that finds nothing cannot prove absence.
        let root = tree("AXWindow", vec![el("AXButton", Some("Cancel"))]);
        let ax = ScriptedAx::with_trees(vec![Some(root)]);
        let out = run_verify_state(&ax, None, &[exists("AXButton", "Save")], Some(0), None).await;
        let data = out.data.unwrap();
        assert_eq!(data["status"], "unknown", "absence is unknown, never false");
        assert_eq!(data["predicates"][0]["unknown_reason"], "target_missing");
        assert_eq!(data["stable"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn a_read_tree_error_is_observation_unavailable() {
        let ax = ScriptedAx::with_trees(vec![None]).tree_err();
        let out = run_verify_state(&ax, None, &[exists("AXButton", "Save")], Some(0), None).await;
        let data = out.data.unwrap();
        assert_eq!(data["status"], "unknown");
        assert_eq!(
            data["predicates"][0]["unknown_reason"],
            "observation_unavailable"
        );
    }

    #[tokio::test]
    async fn value_mismatch_is_a_provable_unsatisfied() {
        let field = AxElement {
            role: "AXTextField".to_string(),
            title: Some("Email".to_string()),
            value: Some("wrong@example.com".to_string()),
            ..Default::default()
        };
        let root = tree("AXWindow", vec![field]);
        let ax = ScriptedAx::with_trees(vec![Some(root)]);
        let pred = StatePredicate {
            assert: StateAssertion::ValueEquals,
            role: Some("AXTextField".to_string()),
            title: Some("Email".to_string()),
            title_contains: None,
            value: Some("right@example.com".to_string()),
        };
        let out = run_verify_state(&ax, None, &[pred], Some(0), None).await;
        let data = out.data.unwrap();
        assert_eq!(
            data["status"], "unsatisfied",
            "the value was read and differed"
        );
    }

    #[tokio::test]
    async fn a_secure_value_is_never_confirmed() {
        // A password field withholds its value via safe_value → unknown, never
        // a confirmed match, even when the raw bytes would equal the assertion.
        let field = AxElement {
            role: "AXTextField".to_string(),
            title: Some("Password".to_string()),
            value: Some("hunter2".to_string()),
            secure: Some(true),
            ..Default::default()
        };
        let root = tree("AXWindow", vec![field]);
        let ax = ScriptedAx::with_trees(vec![Some(root)]);
        let pred = StatePredicate {
            assert: StateAssertion::ValueEquals,
            role: Some("AXTextField".to_string()),
            title: Some("Password".to_string()),
            title_contains: None,
            value: Some("hunter2".to_string()),
        };
        let out = run_verify_state(&ax, None, &[pred], Some(0), None).await;
        let data = out.data.unwrap();
        assert_eq!(data["status"], "unknown");
        assert_eq!(
            data["predicates"][0]["unknown_reason"],
            "observation_unavailable"
        );
    }

    #[tokio::test]
    async fn ambiguous_value_match_is_multi_match() {
        let a = AxElement {
            role: "AXTextField".to_string(),
            title: Some("Cell".to_string()),
            value: Some("1".to_string()),
            ..Default::default()
        };
        let b = AxElement {
            role: "AXTextField".to_string(),
            title: Some("Cell".to_string()),
            value: Some("2".to_string()),
            ..Default::default()
        };
        let root = tree("AXWindow", vec![a, b]);
        let ax = ScriptedAx::with_trees(vec![Some(root)]);
        let pred = StatePredicate {
            assert: StateAssertion::ValueEquals,
            role: Some("AXTextField".to_string()),
            title: Some("Cell".to_string()),
            title_contains: None,
            value: Some("1".to_string()),
        };
        let out = run_verify_state(&ax, None, &[pred], Some(0), None).await;
        let data = out.data.unwrap();
        assert_eq!(data["status"], "unknown");
        assert_eq!(data["predicates"][0]["unknown_reason"], "multi_match");
    }

    #[tokio::test]
    async fn focused_reads_the_apps_own_focus() {
        let root = tree("AXWindow", vec![el("AXTextField", Some("Search"))]);
        let ax = ScriptedAx::with_trees(vec![Some(root)])
            .focused(Some(el("AXTextField", Some("Search"))));
        let pred = StatePredicate {
            assert: StateAssertion::Focused,
            role: Some("AXTextField".to_string()),
            title: Some("Search".to_string()),
            title_contains: None,
            value: None,
        };
        let out = run_verify_state(&ax, None, &[pred], Some(0), None).await;
        assert_eq!(out.data.unwrap()["status"], "satisfied");
    }

    #[tokio::test]
    async fn focus_elsewhere_is_a_provable_unsatisfied() {
        let root = tree("AXWindow", vec![el("AXTextField", Some("Search"))]);
        let ax = ScriptedAx::with_trees(vec![Some(root)]).focused(Some(el("AXButton", Some("OK"))));
        let pred = StatePredicate {
            assert: StateAssertion::Focused,
            role: Some("AXTextField".to_string()),
            title: Some("Search".to_string()),
            title_contains: None,
            value: None,
        };
        let out = run_verify_state(&ax, None, &[pred], Some(0), None).await;
        assert_eq!(out.data.unwrap()["status"], "unsatisfied");
    }

    #[tokio::test]
    async fn unsatisfied_wins_over_unknown_in_the_fold() {
        // One predicate provably false, one unknown → overall unsatisfied,
        // because the conjunction is definitively false.
        let field = AxElement {
            role: "AXTextField".to_string(),
            title: Some("Email".to_string()),
            value: Some("wrong".to_string()),
            ..Default::default()
        };
        let root = tree("AXWindow", vec![field]);
        let ax = ScriptedAx::with_trees(vec![Some(root)]);
        let value_pred = StatePredicate {
            assert: StateAssertion::ValueEquals,
            role: Some("AXTextField".to_string()),
            title: Some("Email".to_string()),
            title_contains: None,
            value: Some("right".to_string()),
        };
        let out = run_verify_state(
            &ax,
            None,
            &[value_pred, exists("AXButton", "Nope")],
            Some(0),
            None,
        )
        .await;
        assert_eq!(out.data.unwrap()["status"], "unsatisfied");
    }

    #[tokio::test]
    async fn empty_expect_is_refused() {
        let ax = ScriptedAx::with_trees(vec![None]);
        let out = run_verify_state(&ax, None, &[], Some(0), None).await;
        assert!(!out.success);
        assert!(out.message.unwrap().contains("at least one"));
    }

    #[tokio::test]
    async fn a_value_predicate_without_a_value_is_refused() {
        let ax = ScriptedAx::with_trees(vec![None]);
        let pred = StatePredicate {
            assert: StateAssertion::ValueEquals,
            role: Some("AXTextField".to_string()),
            title: None,
            title_contains: None,
            value: None,
        };
        let out = run_verify_state(&ax, None, &[pred], Some(0), None).await;
        assert!(!out.success);
        assert!(out.message.unwrap().contains("no `value`"));
    }

    #[tokio::test]
    async fn timeout_reports_the_last_status_without_stability() {
        // Element never appears; the check watches the whole (tiny) window and
        // reports unknown/timeout rather than hanging.
        let empty = tree("AXWindow", vec![]);
        let ax = ScriptedAx::with_trees(vec![Some(empty)]);
        let out =
            run_verify_state(&ax, None, &[exists("AXButton", "Save")], Some(250), Some(2)).await;
        let data = out.data.unwrap();
        assert_eq!(data["status"], "unknown");
        assert_eq!(data["reason"], "timeout");
        assert_eq!(data["stable"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn a_steer_cuts_the_check_short() {
        // Never-satisfied so only a steer or the timeout can end it; the steer
        // must win and report user_input, never a verdict.
        let empty = tree("AXWindow", vec![]);
        let ax = ScriptedAx::with_trees(vec![Some(empty)]);
        let session = SessionKey::peer("main", "verify-steer");
        let turn = TurnContext {
            session_key: session.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        };
        let steered = session.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            crate::session::steer_signal::note_steer(&steered);
        });
        let out = tokio::time::timeout(
            Duration::from_secs(10),
            TURN_CONTEXT.scope(
                turn,
                run_verify_state(
                    &ax,
                    None,
                    &[exists("AXButton", "Save")],
                    Some(10_000),
                    Some(2),
                ),
            ),
        )
        .await
        .expect("a steered check must not run its full window");
        let data = out.data.unwrap();
        assert_eq!(data["reason"], "user_input");
        assert_eq!(data["status"], "unknown");
    }
}
