//! Strategy artifact — a short, welded "map" produced once at the top of a
//! long task (`/goal` · `/loop` · `/workflow`) and pinned into every
//! downstream execution prompt (the StraTA application-layer pattern).
//!
//! Immutable by construction (CLAUDE.md coding-style §immutability): the planner
//! mints a `Strategy`, the store overwrites the row; nothing mutates in place.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A lightly-structured strategy. The `guardrails` field is the StraTA secret
/// sauce and carries the fine resolution; `phases` stay coarse and
/// outcome-phrased (never tool names / arg shapes).
///
/// `JsonSchema` is derived so the `strategy` builtin tool can accept a full
/// `Strategy` as a `revise` argument (its `StrategyArgs` derives `JsonSchema`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct Strategy {
    /// One-line north star — restates the user's end goal.
    pub objective: String,
    /// The chosen overall play (advisory: "initial plan, adapt as you learn").
    pub approach: String,
    /// Coarse, ordered arc (NOT a tactical TODO). Outcome-phrased.
    pub phases: Vec<String>,
    /// 1–3 concrete, named, observable distractors to avoid. If every entry is
    /// blank the strategy is non-concrete → self-gated to nothing (`is_empty`).
    pub guardrails: Vec<String>,
    /// Semantic/human success statement — references the existing objective
    /// gate, never re-implements verification.
    pub success_criteria: String,
    /// Cross-ref to the originating goal (`goal.id`, FNV of `session:objective`)
    /// so a changed objective auto-invalidates a stale strategy. `#[serde(default)]`
    /// → payloads minted before this field read `None`.
    #[serde(default)]
    pub goal_id: Option<String>,
}

impl Strategy {
    /// A strategy with no concrete guardrail is no strategy at all: the planner
    /// self-gates to `None` and the prompt stays byte-identical. True when every
    /// guardrail is blank (or there are none).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.guardrails.iter().all(|g| g.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Strategy {
        Strategy {
            objective: "Migrate auth to new API".into(),
            approach: "Incremental, behind a feature flag".into(),
            phases: vec![
                "understand the failure".into(),
                "implement".into(),
                "verify".into(),
            ],
            guardrails: vec!["do not refactor unrelated modules".into()],
            success_criteria: "gate command passes and old callers unaffected".into(),
            goal_id: Some("goal-deadbeef".into()),
        }
    }

    #[test]
    fn is_empty_false_when_concrete_guardrail_present() {
        assert!(!sample().is_empty());
    }

    #[test]
    fn is_empty_true_when_no_guardrails() {
        let s = Strategy {
            guardrails: Vec::new(),
            ..sample()
        };
        assert!(s.is_empty(), "no guardrail at all => non-strategy");
    }

    #[test]
    fn is_empty_true_when_all_guardrails_blank() {
        // Whitespace-only guardrails carry no concrete distractor (self-gate).
        let s = Strategy {
            guardrails: vec!["   ".into(), "\t".into(), "".into()],
            ..sample()
        };
        assert!(s.is_empty(), "all-blank guardrails => non-strategy");
    }

    #[test]
    fn is_empty_false_when_one_guardrail_nonblank() {
        let s = Strategy {
            guardrails: vec!["  ".into(), "avoid touching the parser".into()],
            ..sample()
        };
        assert!(!s.is_empty());
    }

    #[test]
    fn roundtrips_through_serde_json() {
        let s = sample();
        let json = serde_json::to_string(&s).unwrap();
        let back: Strategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn old_payload_without_goal_id_deserializes_none() {
        // goal_id is #[serde(default)] — payloads minted before the cross-ref
        // field read None.
        let json = r#"{"objective":"o","approach":"a","phases":[],
            "guardrails":["x"],"success_criteria":"s"}"#;
        let s: Strategy = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(s.goal_id, None);
    }
}
