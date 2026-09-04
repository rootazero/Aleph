//! Strategic planner node (strategist) — a one-shot, tool-FREE LLM call that produces
//! a short `Strategy` at the top of a long task (`/goal` · `/loop` ·
//! `/workflow`), before any tool runs. StraTA's "plan-first, then weld" move,
//! application-layer only (no RL — R7). Fully fail-soft: ANY failure (provider
//! error, unparseable output, self-gate) yields `None`, leaving the downstream
//! prompt byte-identical and the command free to proceed (R9 / P7).

use std::sync::Arc;

use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::strategy::Strategy;

/// What the planner is allowed to see (tool-FREE): the curated tool
/// *descriptions* available to this run, a light env summary (OS / cwd), and —
/// for `/goal` — the existing goal lessons. It is told these are the only
/// capabilities and must not name specific tool calls.
#[derive(Debug)]
pub struct PlannerContext {
    pub tool_descriptions: Vec<String>,
    pub env_summary: String,
    pub lessons: Vec<String>,
}

/// Light env summary for the planner (OS + cwd), never failing. Single source
/// of truth shared by the goal / loop / workflow planner call sites.
pub fn env_summary() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!("os={} cwd={}", std::env::consts::OS, cwd)
}

/// System prompt enforcing the §3 content contract and §4 tool-free rules.
/// Kept as a `const` so it is a single source of truth and trivially testable.
const PLANNER_SYSTEM: &str = "You are a strategist planning a long task before any work begins. \
You produce a SHORT, high-level Strategy that an executor will keep in view for the whole task. \
You CANNOT call tools; do not name specific tool calls or argument shapes. \
Reply with ONLY a single JSON object, no prose, with these fields:\n\
  objective: one line restating the user's end goal.\n\
  approach: the overall play, advisory (an initial plan to adapt as you learn).\n\
  phases: a coarse, outcome-phrased arc (e.g. \"understand the failure\", \"implement\", \"verify\"). \
NOT a tactical TODO; never name tools.\n\
  guardrails: 1-3 CONCRETE, named, observable distractors to avoid. CONTRACT: each must name a \
specific distractor tied to this task's real capability surface and be violable by a concrete next \
action. REJECT tautologies like \"stay focused\" or \"avoid scope creep\". Prefer scope-positive, \
observable phrasing. These are advisory, not hard prohibitions.\n\
  success_criteria: a semantic statement of done; reference the task's own gate, do not re-implement \
verification.\n\
CRITICAL self-gate: if you cannot produce at least ONE concrete (non-tautological) guardrail, return \
an EMPTY guardrails array — a trivial task deserves no Strategy. Do not invent filler guardrails.";

/// Tool-free, fail-soft planner. Returns `None` when the provider errors, the
/// output cannot be parsed, or the plan self-gates (no concrete guardrail, i.e.
/// `Strategy::is_empty()`). On success the supplied `goal_id` is stamped onto
/// the Strategy for objective-change auto-invalidation.
pub async fn plan_strategy(
    provider: &Arc<dyn AiProvider>,
    objective: &str,
    ctx: &PlannerContext,
    goal_id: Option<String>,
) -> Option<Strategy> {
    let prompt = build_planner_prompt(objective, ctx);
    let msgs = [UnifiedMessage::user(&prompt)];
    let payload = RequestPayload::new(&msgs).with_system(Some(PLANNER_SYSTEM));

    let response = match provider.process(payload).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "strategy planner LLM call failed; proceeding with no Strategy");
            return None;
        }
    };

    let mut strategy = parse_strategy(&response.text_content())?;
    // Self-gate: a Strategy with no concrete guardrail is welding noise; storing
    // nothing leaves the prompt byte-identical (strictly better). This is an LLM
    // judgement surfaced as data, not a code classifier (R7).
    if strategy.is_empty() {
        return None;
    }
    // Stamp the cross-ref id (overrides whatever the LLM emitted) so an
    // objective change can auto-invalidate the stored Strategy later.
    strategy.goal_id = goal_id;
    Some(strategy)
}

/// Render the user-side planner prompt from the objective + curated context.
/// An empty `tool_descriptions` is rendered explicitly so the model knows the
/// surface is unknown rather than empty-by-omission.
fn build_planner_prompt(objective: &str, ctx: &PlannerContext) -> String {
    let mut p = format!("Task objective:\n{objective}\n\n");
    p.push_str("Environment:\n");
    p.push_str(&ctx.env_summary);
    p.push_str("\n\nAvailable capabilities (the ONLY ones; do not assume others):\n");
    if ctx.tool_descriptions.is_empty() {
        p.push_str(
            "(capability surface not enumerated — keep guardrails about scope, not tools)\n",
        );
    } else {
        for d in &ctx.tool_descriptions {
            p.push_str("- ");
            p.push_str(d);
            p.push('\n');
        }
    }
    if !ctx.lessons.is_empty() {
        p.push_str("\nPrior lessons from this objective:\n");
        for l in &ctx.lessons {
            p.push_str("- ");
            p.push_str(l);
            p.push('\n');
        }
    }
    p.push_str("\nReturn the Strategy JSON now.");
    p
}

/// Tolerant parse: extract the outermost `{...}` JSON object from the LLM
/// response and deserialize it as a `Strategy`. Returns `None` on any failure
/// (mirrors `skill_distill::parse_distill_response`). The `goal_id` field is
/// `#[serde(default)]` on `Strategy`, so the planner JSON need not supply it.
///
/// Robust to stray braces in prose: EVERY `{` is tried as a start, in order,
/// and the first one that both balances and deserializes wins. Walking
/// forward from a candidate honours JSON string literals and escape
/// sequences, so a brace inside a string is content, not structure.
///
/// Trying every `{` rather than only the first is the whole fix. Committing
/// to the first one answers trailing prose (`} use }`) but NOT a preamble —
/// `important for the task {` opens a depth that the real object's `}` only
/// returns to 1, so the scan runs off the end and the function returns
/// `None`. The doc comment claimed that shape survived and it did not
/// (判据 §1); `parse_strategy_with_stray_brace_in_preamble_extracts_valid_plan`
/// has been red ever since it was written.
fn parse_strategy(text: &str) -> Option<Strategy> {
    let bytes = text.as_bytes();
    let mut last_error = None;
    let mut candidates = 0usize;
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        // Bounded so prose full of braces cannot turn one response into a
        // quadratic scan. A model that opens 32 braces before the plan is
        // not producing a plan.
        candidates += 1;
        if candidates > MAX_JSON_START_CANDIDATES {
            break;
        }
        let Some(end) = balanced_object_end(bytes, start) else {
            continue;
        };
        // `{` and `}` are ASCII, so both indices are char boundaries.
        match serde_json::from_str::<Strategy>(&text[start..=end]) {
            Ok(s) => return Some(s),
            Err(e) => last_error = Some(e),
        }
    }
    // One log line for the whole response, not one per candidate: a preamble
    // brace would otherwise report a "failure" that the next candidate went
    // on to parse successfully.
    if let Some(e) = last_error {
        tracing::debug!(
            error = ?e,
            response_preview = %&text[..text.len().min(200)],
            "strategy planner: JSON parse failed"
        );
    }
    None
}

/// How many `{` positions are tried as an object start before giving up.
const MAX_JSON_START_CANDIDATES: usize = 32;

/// The index of the `}` that closes the object opening at `start`, or `None`
/// when it never closes. String-literal aware: a brace inside `"…"` is
/// content, and `\\` escapes the character after it.
fn balanced_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AiProvider, MockError, MockProvider};
    use std::sync::Arc;

    fn ctx() -> PlannerContext {
        PlannerContext {
            tool_descriptions: vec!["bash — run shell commands".to_string()],
            env_summary: "os=macos cwd=/tmp/work".to_string(),
            lessons: vec![],
        }
    }

    /// A well-formed plan with a concrete guardrail round-trips into a Strategy.
    #[tokio::test]
    async fn plans_strategy_with_concrete_guardrail() {
        let json = r#"{
            "objective": "Migrate auth to the new API",
            "approach": "Port endpoints one module at a time",
            "phases": ["understand current auth", "port endpoints", "verify"],
            "guardrails": ["do not touch the billing module while migrating auth"],
            "success_criteria": "all auth endpoints answer on the new API and tests pass"
        }"#;
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new(json));
        let s = plan_strategy(&provider, "Migrate auth to the new API", &ctx(), None)
            .await
            .expect("a concrete-guardrail plan must yield a Strategy");
        assert_eq!(s.objective, "Migrate auth to the new API");
        assert_eq!(s.guardrails.len(), 1);
        assert!(!s.is_empty());
    }

    /// The planner self-gates: an empty / blank guardrail set yields no Strategy
    /// (the most important regression — `Strategy::is_empty()` must be enforced).
    #[tokio::test]
    async fn self_gates_to_none_on_empty_guardrails() {
        let json = r#"{
            "objective": "say hi",
            "approach": "respond",
            "phases": ["respond"],
            "guardrails": ["", "   "],
            "success_criteria": "greeted"
        }"#;
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new(json));
        let s = plan_strategy(&provider, "say hi", &ctx(), None).await;
        assert!(s.is_none(), "blank-only guardrails must self-gate to None");
    }

    /// Unparseable LLM output fails soft to None (never panics, never errors out).
    #[tokio::test]
    async fn unparseable_output_is_none() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new("I cannot help with that."));
        let s = plan_strategy(&provider, "do a thing", &ctx(), None).await;
        assert!(s.is_none());
    }

    /// A provider error fails soft to None.
    #[tokio::test]
    async fn provider_error_is_none() {
        let provider: Arc<dyn AiProvider> =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Timeout));
        let s = plan_strategy(&provider, "do a thing", &ctx(), None).await;
        assert!(s.is_none());
    }

    /// The supplied goal_id is threaded into the Strategy for cross-ref
    /// auto-invalidation (overrides whatever the LLM emitted).
    #[tokio::test]
    async fn goal_id_is_stamped_into_strategy() {
        let json = r#"{
            "objective": "Ship X",
            "approach": "build then verify",
            "phases": ["build", "verify"],
            "guardrails": ["do not refactor the unrelated logging module"],
            "success_criteria": "X ships and tests pass"
        }"#;
        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new(json));
        let s = plan_strategy(
            &provider,
            "Ship X",
            &ctx(),
            Some("goal:sess-1:abc".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(s.goal_id.as_deref(), Some("goal:sess-1:abc"));
    }

    // ----------------------------------------------------------------------
    // Brace-balance robustness tests (P2 planner bug fix):
    // verify `parse_strategy` no longer silently drops a valid plan when the
    // model emits stray `{` / `}` in prose around the JSON object.
    // ----------------------------------------------------------------------

    const SAMPLE_PLAN_JSON: &str = r#"{
        "objective": "Ship X",
        "approach": "build then verify",
        "phases": ["build", "verify"],
        "guardrails": ["do not refactor the unrelated logging module"],
        "success_criteria": "X ships and tests pass"
    }"#;

    /// A stray opening brace in the preamble must NOT pull the slice left of
    /// the real JSON (the old `find('{')..rfind('}')` collapsed on this).
    #[test]
    fn parse_strategy_with_stray_brace_in_preamble_extracts_valid_plan() {
        let text = format!("important for the task {{\n{SAMPLE_PLAN_JSON}");
        let s = parse_strategy(&text).expect("stray opening brace in preamble must be skipped");
        assert_eq!(s.objective, "Ship X");
        assert!(!s.is_empty());
    }

    /// A stray closing brace in trailing prose must NOT widen the slice past
    /// the real JSON's closing brace (the old `rfind('}')` collapsed on this).
    #[test]
    fn parse_strategy_with_stray_brace_in_trailing_prose_extracts_valid_plan() {
        let text = format!("{SAMPLE_PLAN_JSON} use }}");
        let s =
            parse_strategy(&text).expect("stray closing brace in trailing prose must be ignored");
        assert_eq!(s.objective, "Ship X");
        assert!(!s.is_empty());
    }

    /// Braces inside a JSON string literal are content, not structure. A naive
    /// brace counter that ignores string state would mis-match here.
    #[test]
    fn parse_strategy_with_brace_inside_json_string_extracts_valid_plan() {
        let text = r#"{
        "objective": "Ship X {with braces} inside",
        "approach": "build then verify",
        "phases": ["build", "verify"],
        "guardrails": ["do not refactor the unrelated logging module"],
        "success_criteria": "X ships and tests pass"
    }"#;
        let s = parse_strategy(text)
            .expect("braces inside a JSON string must not affect depth tracking");
        assert_eq!(s.objective, "Ship X {with braces} inside");
    }

    /// Unbalanced input (no matching `}`) must fail soft to `None`, matching
    /// the old behaviour. The new scanner just has to not panic / spin.
    #[test]
    fn parse_strategy_with_unbalanced_brace_returns_none() {
        let text = r#"{
        "objective": "Ship X",
        "approach": "build then verify",
        "guardrails": ["do not refactor the unrelated logging module"]"#;
        assert!(
            parse_strategy(text).is_none(),
            "unbalanced JSON must fail soft to None"
        );
    }
}
