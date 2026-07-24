//! Declarative workflow template schema.
//!
//! A [`WorkflowDef`] is a **named, reusable, user/LLM-authored** orchestration
//! template — the "workflow" half of Anthropic's *workflow vs agent*
//! distinction (predefined code paths, not LLM-directed control flow). It is
//! pure data: validation and topological ordering are deterministic, the
//! actual reasoning happens inside each step's agent run.
//!
//! Templates compile (see [`super::compile`]) into the existing
//! `coord_tasks` DAG and execute on the existing `TeamDispatcher` — this
//! module adds **no scheduler and no reasoning** (R10 / R7 safe).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::{AlephError, Result};

/// A reusable multi-step workflow template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowDef {
    /// Logical name. Used as the storage key and as the `coord_task` subject
    /// prefix at materialisation time. Sanitised on save.
    pub name: String,
    /// Human-facing summary of what the workflow does.
    #[serde(default)]
    pub description: String,
    /// Ordered list of steps. Execution order is derived from `depends_on`
    /// edges, not list position — list order is only a human convenience.
    pub steps: Vec<WorkflowStepDef>,
}

/// What a step *does* when it runs.
///
/// Defaults to [`Agent`](WorkflowStepKind::Agent) so every pre-existing
/// template (which has no `kind` field) deserialises unchanged, and an agent
/// step serialises without a `kind` key (byte-identical on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepKind {
    /// Run the step's `agent` against its `prompt` — the original behaviour.
    #[default]
    Agent,
    /// Pause the run, ask the user the `prompt` (optionally offering `choices`),
    /// and resume once they reply. Executed by the dispatcher via the shared
    /// clarification machinery — no agent runs (see [`crate::workflow::clarify`]).
    Clarify,
}

impl WorkflowStepKind {
    /// Whether this is the default agent kind (used to skip serialisation).
    #[must_use]
    pub const fn is_agent(&self) -> bool {
        matches!(self, Self::Agent)
    }
}

/// One step in a workflow. Compiles to a single `coord_task`.
///
/// An **agent** step is owned by `agent` and runs a full agent loop. A
/// **clarify** step pauses the DAG to collect a structured answer from the user;
/// its `prompt` is the question and `choices` (if any) the menu — `agent` is
/// ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStepDef {
    /// Step-local identifier, unique within the workflow. Referenced by other
    /// steps' [`depends_on`](Self::depends_on). NOT the runtime `coord_task`
    /// id — the compiler maps these to freshly-minted task ids.
    pub id: String,
    /// Agent that executes this step. Becomes `coord_task.owner`; resolved
    /// against the `AgentRegistry` by the dispatcher at run time. Ignored (and
    /// may be omitted) for a [`Clarify`](WorkflowStepKind::Clarify) step.
    #[serde(default)]
    pub agent: String,
    /// Prompt for this step. `{input}` is substituted with the run input at
    /// materialisation time. Outputs of upstream (`depends_on`) steps are
    /// injected automatically by `build_handoff_context` at run time, so the
    /// prompt only needs to describe *this* step's job. For a clarify step this
    /// is the question shown to the user.
    pub prompt: String,
    /// Step-local ids this step waits for. Maps to `coord_task.blocked_by`.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// What this step does. Defaults to [`Agent`](WorkflowStepKind::Agent).
    #[serde(default, skip_serializing_if = "WorkflowStepKind::is_agent")]
    pub kind: WorkflowStepKind,
    /// For a [`Clarify`](WorkflowStepKind::Clarify) step: the menu of answers.
    /// Empty → free-text answer. Ignored for agent steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// Gate this step behind lead review: a successful run parks the task in
    /// `WaitingReview` instead of `Completed`, and downstream steps stay
    /// blocked until the leader resolves it via `workflow_step_review`
    /// (approve / reject / retry / skip). Only valid on agent steps — a
    /// clarify step has no run to review. Defaults to `false`, and a
    /// non-reviewed step serialises without the key (byte-identical on the
    /// wire for every pre-existing template).
    #[serde(default, skip_serializing_if = "is_false")]
    pub review: bool,
    /// Per-step wall-clock timeout (seconds) for the member run. Stamped into
    /// the materialised task's metadata (`timeout_secs` — the same override
    /// channel `task_create` uses), so a deep-research step and a quick
    /// formatting step no longer share the dispatcher's one global budget.
    /// `None` → the global `[team_dispatcher] task_timeout_secs`. Absent on
    /// the wire when unset (byte-identical legacy templates). Accepts the
    /// legacy `timeout_secs` spelling for old saved workflows.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "timeout_secs"
    )]
    pub timeout_seconds: Option<u64>,
    /// Per-step automatic retry ceiling. Stamped into the materialised task's
    /// metadata (`max_retries`), overriding the dispatcher's
    /// `default_max_retries` for this step only. `0` = first failure is
    /// terminal. Absent on the wire when unset (byte-identical legacy
    /// templates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

/// serde `skip_serializing_if` helper — keeps non-reviewed steps byte-identical.
const fn is_false(v: &bool) -> bool {
    !*v
}

impl WorkflowStepDef {
    /// Whether this step pauses the run to ask the user a question.
    #[must_use]
    pub const fn is_clarify(&self) -> bool {
        matches!(self.kind, WorkflowStepKind::Clarify)
    }
}

impl WorkflowDef {
    /// Validate the template's internal consistency. Pure — touches no store.
    ///
    /// Checks: non-empty name, at least one step, unique step ids, every
    /// `depends_on` references an existing step, no self-dependency, and the
    /// dependency graph is acyclic. A passing `validate()` guarantees
    /// [`topo_order`](Self::topo_order) succeeds.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(AlephError::invalid_input("workflow name must not be empty"));
        }
        if self.steps.is_empty() {
            return Err(AlephError::invalid_input(
                "workflow must have at least one step",
            ));
        }

        let mut ids: HashSet<&str> = HashSet::with_capacity(self.steps.len());
        for step in &self.steps {
            if step.id.trim().is_empty() {
                return Err(AlephError::invalid_input("step id must not be empty"));
            }
            match step.kind {
                WorkflowStepKind::Agent => {
                    if step.agent.trim().is_empty() {
                        return Err(AlephError::invalid_input(format!(
                            "step '{}' has no agent",
                            step.id
                        )));
                    }
                }
                WorkflowStepKind::Clarify => {
                    // A clarify step needs a question (the prompt); `agent` is
                    // unused so it may be empty.
                    if step.prompt.trim().is_empty() {
                        return Err(AlephError::invalid_input(format!(
                            "clarify step '{}' has no question (prompt must not be empty)",
                            step.id
                        )));
                    }
                    if step.review {
                        return Err(AlephError::invalid_input(format!(
                            "clarify step '{}' cannot require review — there is no agent run to review",
                            step.id
                        )));
                    }
                    if step.timeout_seconds.is_some() || step.max_retries.is_some() {
                        return Err(AlephError::invalid_input(format!(
                            "clarify step '{}' cannot set timeout_seconds/max_retries — it runs no agent",
                            step.id
                        )));
                    }
                }
            }
            // A zero timeout would create a born-dead step (the very first
            // attempt times out immediately) — reject at the boundary (P7).
            // `max_retries: 0` is legitimate ("first failure is terminal").
            if step.timeout_seconds == Some(0) {
                return Err(AlephError::invalid_input(format!(
                    "step '{}' has timeout_seconds=0 — omit the field for the global default",
                    step.id
                )));
            }
            if !ids.insert(step.id.as_str()) {
                return Err(AlephError::invalid_input(format!(
                    "duplicate step id '{}'",
                    step.id
                )));
            }
        }

        for step in &self.steps {
            for dep in &step.depends_on {
                if dep == &step.id {
                    return Err(AlephError::invalid_input(format!(
                        "step '{}' depends on itself",
                        step.id
                    )));
                }
                if !ids.contains(dep.as_str()) {
                    return Err(AlephError::invalid_input(format!(
                        "step '{}' depends on unknown step '{dep}'",
                        step.id
                    )));
                }
            }
        }

        // Acyclicity: a successful topological sort proves the graph is a DAG.
        self.topo_order()?;
        Ok(())
    }

    /// Return step indices in dependency order: every step appears after all
    /// the steps it `depends_on`. Kahn's algorithm — `O(V + E)`.
    ///
    /// Errors with a cycle message if the graph is not acyclic, or with an
    /// unknown-dependency message if a `depends_on` references a step that does
    /// not exist. Assumes ids are unique (guaranteed once the id checks in
    /// [`validate`](Self::validate) pass; callable standalone otherwise).
    pub fn topo_order(&self) -> Result<Vec<usize>> {
        let index_of: HashMap<&str, usize> = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.as_str(), i))
            .collect();

        let mut indegree = vec![0usize; self.steps.len()];
        // dependents[i] = steps that wait for step i.
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); self.steps.len()];
        for (i, step) in self.steps.iter().enumerate() {
            for dep in &step.depends_on {
                let Some(&j) = index_of.get(dep.as_str()) else {
                    return Err(AlephError::invalid_input(format!(
                        "step '{}' depends on unknown step '{dep}'",
                        step.id
                    )));
                };
                dependents[j].push(i);
                indegree[i] += 1;
            }
        }

        // Seed with all roots, preserving list order for deterministic output.
        let mut queue: VecDeque<usize> = (0..self.steps.len())
            .filter(|&i| indegree[i] == 0)
            .collect();
        let mut order = Vec::with_capacity(self.steps.len());
        while let Some(i) = queue.pop_front() {
            order.push(i);
            for &child in &dependents[i] {
                indegree[child] -= 1;
                if indegree[child] == 0 {
                    queue.push_back(child);
                }
            }
        }

        if order.len() != self.steps.len() {
            return Err(AlephError::invalid_input(
                "workflow dependency graph contains a cycle",
            ));
        }
        Ok(order)
    }
}

/// Substitute `{input}` placeholders in a step prompt with the run input.
///
/// Deliberately minimal: a single well-known placeholder, not a templating
/// engine (R6 — KISS). Upstream step outputs are injected at run time by the
/// dispatcher's `build_handoff_context`, so the prompt never needs to
/// reference other steps.
#[must_use]
pub fn render_prompt(template: &str, input: &str) -> String {
    template.replace("{input}", input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, deps: &[&str]) -> WorkflowStepDef {
        WorkflowStepDef {
            id: id.into(),
            agent: "worker".into(),
            prompt: "do {input}".into(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            kind: WorkflowStepKind::Agent,
            choices: vec![],
            review: false,
            timeout_seconds: None,
            max_retries: None,
        }
    }

    fn clarify_step(id: &str, question: &str, choices: &[&str], deps: &[&str]) -> WorkflowStepDef {
        WorkflowStepDef {
            id: id.into(),
            agent: String::new(),
            prompt: question.into(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            kind: WorkflowStepKind::Clarify,
            choices: choices.iter().map(|s| s.to_string()).collect(),
            review: false,
            timeout_seconds: None,
            max_retries: None,
        }
    }

    fn def(steps: Vec<WorkflowStepDef>) -> WorkflowDef {
        WorkflowDef {
            name: "wf".into(),
            description: String::new(),
            steps,
        }
    }

    #[test]
    fn validate_accepts_linear_chain() {
        let d = def(vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])]);
        assert!(d.validate().is_ok());
    }

    #[test]
    fn validate_accepts_diamond() {
        let d = def(vec![
            step("a", &[]),
            step("b", &["a"]),
            step("c", &["a"]),
            step("d", &["b", "c"]),
        ]);
        assert!(d.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut d = def(vec![step("a", &[])]);
        d.name = "  ".into();
        assert!(d.validate().is_err());
    }

    #[test]
    fn validate_rejects_no_steps() {
        assert!(def(vec![]).validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let d = def(vec![step("a", &[]), step("a", &[])]);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"), "got: {err}");
    }

    #[test]
    fn validate_rejects_unknown_dependency() {
        let d = def(vec![step("a", &["ghost"])]);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("unknown step"), "got: {err}");
    }

    #[test]
    fn validate_rejects_self_dependency() {
        let d = def(vec![step("a", &["a"])]);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("itself"), "got: {err}");
    }

    #[test]
    fn validate_rejects_cycle() {
        // a → b → c → a
        let d = def(vec![
            step("a", &["c"]),
            step("b", &["a"]),
            step("c", &["b"]),
        ]);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("cycle"), "got: {err}");
    }

    #[test]
    fn validate_rejects_empty_agent() {
        let mut d = def(vec![step("a", &[])]);
        d.steps[0].agent = "".into();
        assert!(d.validate().is_err());
    }

    #[test]
    fn topo_order_places_deps_before_dependents() {
        let d = def(vec![step("c", &["b"]), step("b", &["a"]), step("a", &[])]);
        let order = d.topo_order().expect("acyclic");
        // Map back to ids and assert a precedes b precedes c.
        let ids: Vec<&str> = order.iter().map(|&i| d.steps[i].id.as_str()).collect();
        let pos = |id: &str| ids.iter().position(|x| *x == id).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("b") < pos("c"));
    }

    #[test]
    fn render_prompt_substitutes_input() {
        assert_eq!(
            render_prompt("summarise {input}", "the logs"),
            "summarise the logs"
        );
        assert_eq!(render_prompt("no placeholder", "x"), "no placeholder");
        assert_eq!(render_prompt("{input} and {input}", "a"), "a and a");
    }

    #[test]
    fn def_roundtrips_through_json() {
        let d = def(vec![step("a", &[]), step("b", &["a"])]);
        let s = serde_json::to_string(&d).unwrap();
        let back: WorkflowDef = serde_json::from_str(&s).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn step_depends_on_defaults_to_empty() {
        let json = r#"{"id":"a","agent":"w","prompt":"go"}"#;
        let s: WorkflowStepDef = serde_json::from_str(json).unwrap();
        assert!(s.depends_on.is_empty());
    }

    // ---- Clarify steps -----------------------------------------------------

    #[test]
    fn agent_step_kind_defaults_and_skips_serialisation() {
        // A legacy step (no `kind`/`choices`) deserialises as an agent step,
        // and re-serialising an agent step omits both keys → byte-identical.
        let json = r#"{"id":"a","agent":"w","prompt":"go"}"#;
        let s: WorkflowStepDef = serde_json::from_str(json).unwrap();
        assert_eq!(s.kind, WorkflowStepKind::Agent);
        assert!(!s.is_clarify());
        let out = serde_json::to_string(&s).unwrap();
        assert!(!out.contains("kind"), "agent step omits kind: {out}");
        assert!(!out.contains("choices"), "agent step omits choices: {out}");
    }

    #[test]
    fn clarify_step_roundtrips_with_choices() {
        let json = r#"{"id":"ask","prompt":"Deploy where?","kind":"clarify","choices":["staging","prod"]}"#;
        let s: WorkflowStepDef = serde_json::from_str(json).unwrap();
        assert!(s.is_clarify());
        assert_eq!(s.choices, vec!["staging", "prod"]);
        assert!(s.agent.is_empty(), "agent optional for clarify steps");
    }

    #[test]
    fn validate_accepts_clarify_without_agent() {
        let d = def(vec![
            clarify_step("ask", "Pick env", &["a", "b"], &[]),
            step("deploy", &["ask"]),
        ]);
        assert!(d.validate().is_ok(), "{:?}", d.validate());
    }

    #[test]
    fn validate_rejects_clarify_without_question() {
        let d = def(vec![clarify_step("ask", "   ", &[], &[])]);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("no question"), "got: {err}");
    }

    // ---- Review gate -------------------------------------------------------

    #[test]
    fn review_defaults_false_and_skips_serialisation() {
        // Legacy steps (no `review` key) deserialise as non-reviewed, and a
        // non-reviewed step serialises without the key → byte-identical.
        let json = r#"{"id":"a","agent":"w","prompt":"go"}"#;
        let s: WorkflowStepDef = serde_json::from_str(json).unwrap();
        assert!(!s.review);
        let out = serde_json::to_string(&s).unwrap();
        assert!(
            !out.contains("review"),
            "non-reviewed step omits key: {out}"
        );
    }

    #[test]
    fn review_step_roundtrips_through_json() {
        let json = r#"{"id":"a","agent":"w","prompt":"go","review":true}"#;
        let s: WorkflowStepDef = serde_json::from_str(json).unwrap();
        assert!(s.review);
        let out = serde_json::to_string(&s).unwrap();
        let back: WorkflowStepDef = serde_json::from_str(&out).unwrap();
        assert!(back.review);
    }

    #[test]
    fn validate_accepts_review_on_agent_step() {
        let mut d = def(vec![step("a", &[])]);
        d.steps[0].review = true;
        assert!(d.validate().is_ok());
    }

    #[test]
    fn validate_rejects_review_on_clarify_step() {
        let mut d = def(vec![clarify_step("ask", "Pick env", &[], &[])]);
        d.steps[0].review = true;
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("cannot require review"), "got: {err}");
    }

    // ---- Per-step timeout / retry overrides --------------------------------

    #[test]
    fn timeout_and_retries_roundtrip_and_skip_when_unset() {
        let mut d = def(vec![step("a", &[])]);
        d.steps[0].timeout_seconds = Some(1800);
        d.steps[0].max_retries = Some(0);
        assert!(d.validate().is_ok(), "{:?}", d.validate());
        let s = serde_json::to_string(&d).unwrap();
        let back: WorkflowDef = serde_json::from_str(&s).unwrap();
        assert_eq!(back.steps[0].timeout_seconds, Some(1800));
        assert_eq!(
            back.steps[0].max_retries,
            Some(0),
            "0 = no auto-retry, legal"
        );

        // Unset fields stay off the wire (byte-identical legacy templates).
        let plain = serde_json::to_string(&def(vec![step("a", &[])])).unwrap();
        assert!(!plain.contains("timeout_secs"));
        assert!(!plain.contains("max_retries"));
    }

    #[test]
    fn validate_rejects_zero_timeout() {
        let mut d = def(vec![step("a", &[])]);
        d.steps[0].timeout_seconds = Some(0);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("timeout_seconds=0"), "got: {err}");
    }

    #[test]
    fn validate_rejects_timeout_on_clarify_step() {
        let mut d = def(vec![clarify_step("ask", "Pick env", &[], &[])]);
        d.steps[0].timeout_seconds = Some(60);
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("runs no agent"), "got: {err}");
    }

    #[test]
    fn workflow_step_timeout_accepts_legacy_alias_for_saved_workflows() {
        let step: WorkflowStepDef = serde_json::from_value(serde_json::json!({
            "id": "a",
            "agent": "w",
            "prompt": "go",
            "timeout_secs": 300
        }))
        .unwrap();
        assert_eq!(step.timeout_seconds, Some(300));
    }
}
