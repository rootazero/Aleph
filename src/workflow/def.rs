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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// One step in a workflow. Compiles to a single `coord_task` owned by `agent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStepDef {
    /// Step-local identifier, unique within the workflow. Referenced by other
    /// steps' [`depends_on`](Self::depends_on). NOT the runtime `coord_task`
    /// id — the compiler maps these to freshly-minted task ids.
    pub id: String,
    /// Agent that executes this step. Becomes `coord_task.owner`; resolved
    /// against the `AgentRegistry` by the dispatcher at run time.
    pub agent: String,
    /// Prompt for this step. `{input}` is substituted with the run input at
    /// materialisation time. Outputs of upstream (`depends_on`) steps are
    /// injected automatically by `build_handoff_context` at run time, so the
    /// prompt only needs to describe *this* step's job.
    pub prompt: String,
    /// Step-local ids this step waits for. Maps to `coord_task.blocked_by`.
    #[serde(default)]
    pub depends_on: Vec<String>,
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
            if step.agent.trim().is_empty() {
                return Err(AlephError::invalid_input(format!(
                    "step '{}' has no agent",
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
    /// Errors with a cycle message if the graph is not acyclic. Assumes ids
    /// are unique and deps resolve (guaranteed once the id/dep checks in
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
                if let Some(&j) = index_of.get(dep.as_str()) {
                    dependents[j].push(i);
                    indegree[i] += 1;
                }
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
}
