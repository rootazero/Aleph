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
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

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
    /// Require the reviewer to attach real measured evidence before approving
    /// this step (exit code / count / line count — never a self-report).
    ///
    /// Only meaningful together with [`review`](Self::review): it stamps the
    /// same `require_grounding` metadata `task_create` already writes, which
    /// `workflow_step_review`'s approve arm bounces on. Without it, the
    /// declarative path could declare a review gate but never demand that the
    /// gate touch reality — the review verdict was one model's word about
    /// another model's word, which is the failure mode the whole review gate
    /// exists to prevent. Absent on the wire when false (byte-identical legacy
    /// templates).
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_grounding: bool,
    /// Run this step even if a step it `depends_on` ended `Failed` or
    /// `Cancelled`, instead of leaving it permanently `Unsatisfiable`.
    ///
    /// Off by default: a dependency edge normally means "I need this step's
    /// output", so a dead upstream kills the branch. Set it on a **synthesis /
    /// report / cleanup** step that can still do useful work with one input
    /// missing — the failed upstream's error text is handed to this step in
    /// place of a deliverable, so the agent knows what it did not get.
    ///
    /// Scope is deliberately narrow: it tolerates only this step's **direct**
    /// dependencies, and only for this step. A step two hops below a failure
    /// whose own (tolerant) parent then ran successfully is unblocked by that
    /// parent's success, not by this flag; a step whose direct parent is
    /// itself unsatisfiable stays blocked. Absent on the wire when false
    /// (byte-identical legacy templates).
    #[serde(default, skip_serializing_if = "is_false")]
    pub tolerate_failed_deps: bool,
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
                    if step.prompt.trim().is_empty() {
                        return Err(AlephError::invalid_input(format!(
                            "step '{}' has no prompt",
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
                    if step.review || step.require_grounding {
                        return Err(AlephError::invalid_input(format!(
                            "clarify step '{}' cannot require review/grounding — there is no agent run to review",
                            step.id
                        )));
                    }
                    // The tolerance flag is stamped onto the materialised
                    // task's metadata by the AGENT branch of `materialize`;
                    // a clarify row carries a different metadata shape and
                    // never gets it. Accepting the flag here would let it ride
                    // through save → export → import as data nothing applies —
                    // a declared guarantee with no machinery behind it, which
                    // is worse than refusing it at the boundary.
                    if step.tolerate_failed_deps {
                        return Err(AlephError::invalid_input(format!(
                            "clarify step '{}' cannot set tolerate_failed_deps — the flag is \
                             applied to an agent run's readiness, and a clarify step runs none",
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
            // Grounding is enforced at the review gate, so demanding it without
            // one means nobody is ever asked for the evidence: the flag would
            // read as a guarantee while doing nothing at all.
            if step.require_grounding && !step.review {
                return Err(AlephError::invalid_input(format!(
                    "step '{}' sets require_grounding without review — the evidence is \
                     demanded at the review gate, so add `review: true` (or drop the flag)",
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

    /// Every `{{name}}` this template's prompts reference, sorted and deduped.
    ///
    /// **Derived, never declared.** A `vars: Vec<String>` field on the manifest
    /// would be a second spelling of a fact the prompts already state, and the
    /// two would drift the first time someone edits a prompt without editing
    /// the list — after which `run` would either demand an arg no prompt uses
    /// or accept a launch missing one that every prompt does. Scanning the
    /// prompts means the declaration cannot be wrong, because there is no
    /// declaration.
    ///
    /// Covers clarify steps too: a clarify step's `prompt` IS the question put
    /// to the user, so an unsubstituted placeholder there is shown to a human
    /// rather than to a model.
    #[must_use]
    pub fn referenced_vars(&self) -> BTreeSet<String> {
        let mut vars = BTreeSet::new();
        for step in &self.steps {
            // `None` input and a lookup that never substitutes: the scan is
            // run purely for its side effect, so the names and the renderer's
            // notion of "what is a placeholder" cannot disagree.
            let _ = scan_prompt(&step.prompt, None, &mut |name| {
                vars.insert(name.to_string());
                None
            });
        }
        vars
    }

    /// The referenced vars `args` does not supply, in report order. Empty
    /// means the run can be rendered without leaving a placeholder behind.
    ///
    /// Fail-closed (P7) is the caller's job and `run`'s policy: an absent arg
    /// is "I do not know what this step should say", not "substitute nothing".
    /// Rendering it as the literal `{{region}}` would ship that question to the
    /// agent as if it were the instruction.
    #[must_use]
    pub fn missing_vars(&self, args: &HashMap<String, String>) -> Vec<String> {
        self.referenced_vars()
            .into_iter()
            .filter(|v| !args.contains_key(v))
            .collect()
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

/// Everything a single `run` substitutes into its step prompts.
///
/// One value rather than a growing positional argument list: `materialize`
/// already threads eight parameters, and the run's *inputs* are one fact with
/// two spellings — the anonymous `{input}` every template has always had, and
/// the named `{{var}}` args. A caller that forgets one of them is a caller
/// that constructed this struct, not one that mis-ordered a parameter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunInputs {
    /// Substituted for `{input}`.
    pub input: String,
    /// Substituted for `{{name}}`, by name.
    pub args: HashMap<String, String>,
}

impl RunInputs {
    /// The anonymous input alone — the shape every run had before named args.
    #[must_use]
    pub fn from_input(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            args: HashMap::new(),
        }
    }
}

/// Walk `template` once, resolving both placeholder forms in a single pass.
///
/// A single pass is the point: substituting in two passes would re-scan the
/// text a value produced, so an arg whose value happens to contain `{input}`
/// (or vice versa) would be expanded a second time — a template injection
/// through a run's own data. Nothing a lookup returns is ever re-examined.
///
/// `{{name}}` matches only `[A-Za-z0-9_]+` between the braces. Anything else
/// after `{{` is not a placeholder and is copied through untouched, so a
/// prompt containing JSON or a shell brace expansion survives verbatim.
/// `lookup` returning `None` also leaves the placeholder untouched — that is
/// what makes this same scanner usable for *collecting* the names (see
/// [`WorkflowDef::referenced_vars`]) rather than only for rendering.
fn scan_prompt(
    template: &str,
    input: Option<&str>,
    lookup: &mut dyn FnMut(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(pos) = rest.find('{') {
        out.push_str(&rest[..pos]);
        let at = &rest[pos..];
        if let Some(after) = at.strip_prefix("{{") {
            // Identifier chars only; the first char that is not one ends the
            // candidate name (and `}}` must follow immediately).
            let name_len = after
                .char_indices()
                .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
                .map_or(after.len(), |(i, _)| i);
            let (name, tail) = after.split_at(name_len);
            if !name.is_empty() && tail.starts_with("}}") {
                match lookup(name) {
                    Some(value) => out.push_str(&value),
                    None => out.push_str(&at[..name_len + 4]),
                }
                rest = &tail[2..];
                continue;
            }
            // Not a placeholder — emit the braces and resume after them, so a
            // `{{` that opens nothing cannot swallow the rest of the prompt.
            out.push_str("{{");
            rest = after;
            continue;
        }
        if let (Some(input), Some(tail)) = (input, at.strip_prefix("{input}")) {
            out.push_str(input);
            rest = tail;
            continue;
        }
        out.push('{');
        rest = &at[1..];
    }
    out.push_str(rest);
    out
}

/// Substitute a step prompt's placeholders with the run's inputs.
///
/// Two forms, both resolved in one pass: `{input}` (the run's anonymous input)
/// and `{{name}}` (a named `args` entry). A `{{name}}` with no matching arg is
/// left as written — `run` refuses such a launch up front (see
/// [`WorkflowDef::referenced_vars`]), so reaching here with one means the
/// caller deliberately rendered without validating.
///
/// Still not a templating engine (R6 — KISS): no conditionals, no loops, no
/// nesting. Upstream step outputs are injected at run time by the dispatcher's
/// `build_handoff_context`, so the prompt never needs to reference other steps.
#[must_use]
pub fn render_prompt(template: &str, inputs: &RunInputs) -> String {
    scan_prompt(template, Some(&inputs.input), &mut |name| {
        inputs.args.get(name).cloned()
    })
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
            require_grounding: false,
            tolerate_failed_deps: false,
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
            require_grounding: false,
            tolerate_failed_deps: false,
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
        let inputs = |s: &str| RunInputs::from_input(s);
        assert_eq!(
            render_prompt("summarise {input}", &inputs("the logs")),
            "summarise the logs"
        );
        assert_eq!(
            render_prompt("no placeholder", &inputs("x")),
            "no placeholder"
        );
        assert_eq!(
            render_prompt("{input} and {input}", &inputs("a")),
            "a and a"
        );
    }

    fn with_args(input: &str, pairs: &[(&str, &str)]) -> RunInputs {
        RunInputs {
            input: input.into(),
            args: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    #[test]
    fn render_prompt_substitutes_named_args() {
        let inputs = with_args("the logs", &[("region", "eu-west"), ("env", "prod")]);
        assert_eq!(
            render_prompt("deploy to {{env}} in {{region}}: {input}", &inputs),
            "deploy to prod in eu-west: the logs"
        );
        // Repeated use, and adjacency to ordinary text.
        assert_eq!(render_prompt("{{env}}/{{env}}!", &inputs), "prod/prod!");
    }

    #[test]
    fn a_run_input_is_never_re_scanned_as_a_template() {
        // Two passes would let a run's own DATA become template text: an arg
        // value containing `{input}` (or an input containing `{{env}}`) would
        // be expanded by the pass that follows. One pass, no re-entry.
        let inputs = with_args("{{env}}", &[("env", "prod"), ("literal", "{input}")]);
        assert_eq!(render_prompt("{input}", &inputs), "{{env}}");
        assert_eq!(render_prompt("{{literal}}", &inputs), "{input}");
    }

    #[test]
    fn a_brace_run_that_is_not_a_placeholder_survives_verbatim() {
        // Prompts carry JSON, shell braces and prose. Only `{{ident}}` is a
        // placeholder; everything else must reach the agent as written.
        let inputs = with_args("x", &[("a", "A")]);
        for text in [
            r#"return {"shape": {"n": 1}}"#,
            "{{ spaced }}",
            "{{}}",
            "{{a-b}}",
            "{{unclosed",
            "{notinput}",
        ] {
            assert_eq!(
                render_prompt(text, &inputs),
                text,
                "left as written: {text}"
            );
        }
        // …and an unsupplied name is left as written too (run refuses first).
        assert_eq!(render_prompt("{{ghost}}", &inputs), "{{ghost}}");
    }

    #[test]
    fn referenced_vars_are_derived_from_the_prompts() {
        // Single source of truth: the prompts declare the vars. Includes
        // clarify steps, whose prompt is the question shown to a human.
        let mut steps = vec![step("a", &[]), step("b", &["a"])];
        steps[0].prompt = "audit {{region}} for {{env}}, given {input}".into();
        steps[1].kind = WorkflowStepKind::Clarify;
        steps[1].prompt = "ship to {{env}}?".into();
        let d = def(steps);
        assert_eq!(
            d.referenced_vars().into_iter().collect::<Vec<_>>(),
            vec!["env".to_string(), "region".to_string()],
            "sorted, deduped, both step kinds"
        );
        let mut args = HashMap::new();
        args.insert("env".to_string(), "prod".to_string());
        assert_eq!(d.missing_vars(&args), vec!["region".to_string()]);
        args.insert("region".to_string(), "eu".to_string());
        assert!(d.missing_vars(&args).is_empty());

        // A template with no `{{...}}` references nothing — the byte-identical
        // path for every workflow written before named args existed.
        assert!(def(vec![step("a", &[])]).referenced_vars().is_empty());
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


    // ---- Tolerant fan-in ---------------------------------------------------

    #[test]
    fn tolerate_failed_deps_roundtrips_and_stays_off_the_wire_when_false() {
        let mut d = def(vec![step("a", &[]), step("b", &["a"])]);
        d.steps[1].tolerate_failed_deps = true;
        assert!(d.validate().is_ok(), "{:?}", d.validate());
        let s = serde_json::to_string(&d).unwrap();
        let back: WorkflowDef = serde_json::from_str(&s).unwrap();
        assert!(back.steps[1].tolerate_failed_deps);
        assert!(
            !back.steps[0].tolerate_failed_deps,
            "the flag is per step, not per template"
        );

        // A template that does not use it is byte-identical to a legacy one.
        let plain = serde_json::to_string(&def(vec![step("a", &[])])).unwrap();
        assert!(!plain.contains("tolerate_failed_deps"), "{plain}");
    }

    /// A clarify step's readiness stamp is written only by the agent branch of
    /// `materialize`, so accepting the flag here would let it ride through
    /// save/export as a guarantee with no machinery behind it.
    #[test]
    fn validate_rejects_tolerate_failed_deps_on_clarify_step() {
        let mut d = def(vec![clarify_step("ask", "Pick env", &[], &[])]);
        d.steps[0].tolerate_failed_deps = true;
        let err = d.validate().unwrap_err().to_string();
        assert!(err.contains("cannot set tolerate_failed_deps"), "got: {err}");
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
