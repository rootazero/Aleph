//! Pure, deterministic renderers for the welded `Strategy`. These produce the
//! INNER text only — the prompt layers wrap them in `<strategy>` /
//! `<strategy_reminder>` envelopes.
//!
//! DETERMINISM CONTRACT (spec §5): no timestamps, no `now_ms`, no `HashMap`
//! iteration order. Every field is a `Vec`/`String` rendered in declaration
//! order, so the same `Strategy` renders to byte-identical output across calls.
//! This is what lets the Stable body ride the KV-cache prefix unchanged across
//! every turn of a long task (mirrors `curated_memory_envelope`).

use crate::strategy::types::Strategy;

/// Full `<strategy>` body for the Stable `StrategyLayer` — objective, approach,
/// the coarse phase arc, the concrete guardrails, and the success statement.
/// Rendered once, injected verbatim.
#[must_use]
pub fn render_strategy_summary(s: &Strategy) -> String {
    let mut out = String::new();
    out.push_str("Objective: ");
    out.push_str(s.objective.trim());
    out.push('\n');
    out.push_str("Approach: ");
    out.push_str(s.approach.trim());
    out.push('\n');

    let phases: Vec<&str> = s
        .phases
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if !phases.is_empty() {
        out.push_str("Phases:\n");
        for (i, phase) in phases.iter().enumerate() {
            out.push_str(&format!("  {}. {phase}\n", i + 1));
        }
    }

    let guardrails: Vec<&str> = s
        .guardrails
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .collect();
    if !guardrails.is_empty() {
        out.push_str("Guardrails (advisory — stay sovereign over moment-to-moment relevance):\n");
        for g in &guardrails {
            out.push_str(&format!("  - {g}\n"));
        }
    }

    let success = s.success_criteria.trim();
    if !success.is_empty() {
        out.push_str("Success: ");
        out.push_str(success);
        out.push('\n');
    }

    // Trim the single trailing newline so the layer controls envelope spacing.
    out.truncate(out.trim_end().len());
    out
}

/// Guardrail lines only, for the Dynamic `StrategyPointerLayer` tail near the
/// read head. Deliberately omits the objective (StandingGoalLayer already
/// re-injects that every turn — restating it here would cause reminder-blindness)
/// and the phases.
///
/// **Bounded on purpose.** Guardrails are free text (LLM-authored at `plan`,
/// user-editable) and `StrategyPointerLayer` only passes this render through —
/// it sits in `prompt_contract::CONDITIONALLY_SILENT`, so the per-layer byte
/// ratchet measures it as 0 B no matter how large the list gets. The bound
/// lives here, in the producer: at most [`GUARDRAIL_PROMPT_MAX_ITEMS`] lines
/// of [`GUARDRAIL_PROMPT_MAX_CHARS`] chars each, with an elision footer naming
/// the hidden count so a capped list never reads as complete.
#[must_use]
pub fn render_guardrails_only(s: &Strategy) -> String {
    let mut out = String::new();
    let mut shown = 0usize;
    let mut hidden = 0usize;
    for g in &s.guardrails {
        let g = g.trim();
        if g.is_empty() {
            continue;
        }
        if shown >= GUARDRAIL_PROMPT_MAX_ITEMS {
            hidden += 1;
            continue;
        }
        shown += 1;
        out.push_str("- ");
        out.push_str(&crate::utils::text_format::truncate_reserving(
            g,
            GUARDRAIL_PROMPT_MAX_CHARS,
            "…",
        ));
        out.push('\n');
    }
    if hidden > 0 {
        out.push_str(&format!("- … ({hidden} more guardrails elided)\n"));
    }
    out.truncate(out.trim_end().len());
    out
}

/// Prompt-side ceilings for [`render_guardrails_only`] — see its doc.
const GUARDRAIL_PROMPT_MAX_ITEMS: usize = 10;
const GUARDRAIL_PROMPT_MAX_CHARS: usize = 300;

/// Workflow per-node global frame: the run-global objective + cross-cutting
/// guardrails ONLY. Drops the coarse phase list — in a heterogeneous DAG the
/// graph itself is the phase structure, and a global phase list would conflict
/// with each node's local objective.
#[must_use]
pub fn render_workflow_global_frame(s: &Strategy) -> String {
    let mut out = String::new();
    out.push_str("Objective: ");
    out.push_str(s.objective.trim());
    out.push('\n');

    let guardrails: Vec<&str> = s
        .guardrails
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .collect();
    if !guardrails.is_empty() {
        out.push_str("Cross-cutting guardrails:\n");
        for g in &guardrails {
            out.push_str(&format!("  - {g}\n"));
        }
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::types::Strategy;

    fn sample() -> Strategy {
        Strategy {
            objective: "Migrate auth to the new API".into(),
            approach: "Incremental, behind a feature flag".into(),
            phases: vec![
                "understand the current failure".into(),
                "implement the migration".into(),
                "verify against the gate".into(),
            ],
            guardrails: vec![
                "do not refactor the unrelated parser".into(),
                "do not add new config keys".into(),
            ],
            success_criteria: "the objective gate passes and old callers still work".into(),
            goal_id: Some("goal-deadbeef".into()),
        }
    }

    #[test]
    fn summary_contains_all_sections() {
        let out = render_strategy_summary(&sample());
        assert!(out.contains("Migrate auth to the new API"));
        assert!(out.contains("Incremental, behind a feature flag"));
        assert!(out.contains("understand the current failure"));
        assert!(out.contains("do not refactor the unrelated parser"));
        assert!(out.contains("the objective gate passes"));
    }

    #[test]
    fn summary_is_deterministic_across_two_renders() {
        // PURE + DETERMINISTIC: same input rendered twice => identical bytes.
        // No timestamps, no HashMap ordering — guards the cache-prefix invariant.
        let s = sample();
        let a = render_strategy_summary(&s);
        let b = render_strategy_summary(&s);
        assert_eq!(a, b, "render must be byte-identical for identical input");
        // No timestamp / clock leak: pin concrete timestamp shapes, not the
        // bare substring "ms" — legitimate content like "100ms" or guardrail
        // text "do not let latency exceed 200ms" would otherwise trip a
        // false positive (issue STRAT-005).
        assert!(
            !contains_iso_date(&a) && !contains_clock_time(&a) && !contains_epoch_ms(&a),
            "no timestamp may appear in the stable body"
        );
    }

    fn contains_iso_date(s: &str) -> bool {
        // YYYY-MM-DD
        let bytes = s.as_bytes();
        if bytes.len() < 10 {
            return false;
        }
        for i in 0..=bytes.len() - 10 {
            if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit())
                && bytes[i + 4] == b'-'
                && bytes[i + 5..i + 7].iter().all(|b| b.is_ascii_digit())
                && bytes[i + 7] == b'-'
                && bytes[i + 8..i + 10].iter().all(|b| b.is_ascii_digit())
            {
                return true;
            }
        }
        false
    }

    fn contains_clock_time(s: &str) -> bool {
        // HH:MM:SS or HH:MM — must be preceded by start/whitespace to avoid
        // matching identifiers like "12:34:56" inside URLs.
        let bytes = s.as_bytes();
        let mut i = 0;
        while i + 5 <= bytes.len() {
            if i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\n' || bytes[i - 1] == b'\t' {
                if bytes[i..i + 2].iter().all(|b| b.is_ascii_digit())
                    && bytes[i + 2] == b':'
                    && bytes[i + 3..i + 5].iter().all(|b| b.is_ascii_digit())
                {
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    fn contains_epoch_ms(s: &str) -> bool {
        // 10-13 digit run standing alone (epoch seconds / epoch millis).
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if (10..=13).contains(&(i - start)) {
                    let left_ok = start == 0
                        || !bytes[start - 1].is_ascii_alphanumeric();
                    let right_ok =
                        i == bytes.len() || !bytes[i].is_ascii_alphanumeric();
                    if left_ok && right_ok {
                        return true;
                    }
                }
            } else {
                i += 1;
            }
        }
        false
    }

    #[test]
    fn guardrails_only_lists_guardrails_and_nothing_else() {
        let out = render_guardrails_only(&sample());
        assert!(out.contains("do not refactor the unrelated parser"));
        assert!(out.contains("do not add new config keys"));
        // De-dup vs StandingGoal: the tail must NOT restate the objective.
        assert!(
            !out.contains("Migrate auth to the new API"),
            "guardrail tail omits the objective to avoid reminder-blindness"
        );
        assert!(
            !out.contains("understand the current failure"),
            "no phases in tail"
        );
    }

    #[test]
    fn guardrails_only_skips_blank_lines() {
        let s = Strategy {
            guardrails: vec!["  ".into(), "keep the change surgical".into(), "".into()],
            ..sample()
        };
        let out = render_guardrails_only(&s);
        assert!(out.contains("keep the change surgical"));
        // Blank guardrails are dropped, not rendered as empty bullets.
        assert!(
            !out.contains("- \n"),
            "no empty bullet for a blank guardrail"
        );
    }

    #[test]
    fn guardrails_only_is_deterministic() {
        let s = sample();
        assert_eq!(render_guardrails_only(&s), render_guardrails_only(&s));
    }

    #[test]
    fn workflow_global_frame_excludes_phases() {
        // The DAG *is* the phase structure — the per-node weld drops the phase
        // list and welds only the run-global objective + cross-cutting guardrails.
        let out = render_workflow_global_frame(&sample());
        assert!(
            out.contains("Migrate auth to the new API"),
            "objective present"
        );
        assert!(
            out.contains("do not refactor the unrelated parser"),
            "guardrails present"
        );
        assert!(
            !out.contains("understand the current failure"),
            "phase 1 must not leak into the workflow global frame"
        );
        assert!(
            !out.contains("implement the migration"),
            "phase 2 must not leak into the workflow global frame"
        );
        assert!(
            !out.contains("verify against the gate"),
            "phase 3 must not leak into the workflow global frame"
        );
    }

    #[test]
    fn workflow_global_frame_is_deterministic() {
        let s = sample();
        assert_eq!(
            render_workflow_global_frame(&s),
            render_workflow_global_frame(&s)
        );
    }
}
