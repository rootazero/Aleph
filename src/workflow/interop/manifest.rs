//! AWI (Aleph Workflow Interchange) manifest — the declarative superset that
//! bridges Aleph's `WorkflowDef` and Claude Code's `.workflow.js` format.
//!
//! Pure data + field shuffling, no reasoning (R7/R10). The manifest carries the
//! full `.workflow.js`-compatible metadata (`whenToUse`, `phases` with optional
//! per-phase `model`, per-step `label`/`model`/`phase`/`schema`/`isolation`/
//! `agentType`); only the executable core round-trips into `WorkflowDef`, the
//! rest is preserved for lossless export.
//!
//! **Four of the extras leave interchange** via [`WorkflowManifest::step_pins`]
//! → [`StepPins`] → task metadata:
//!
//! | field | what it reaches |
//! |---|---|
//! | `model` | `RunRequest.model_override` (dispatcher, at launch) |
//! | `effort` | the member run's `think_level` |
//! | `phase` | grouped `workflow(action='status')` reporting |
//! | `schema` | an `## Output Contract` section in the member's handoff — the model is **asked**, never validated |
//!
//! `phase` and `schema` used to be inert: they round-tripped through
//! `import`/`export` and had zero runtime consumers, so a phased `.workflow.js`
//! imported its phase plan and then reported a flat step list, and a step's
//! JSON Schema was carried the whole way to disk and told to nobody.
//!
//! Genuinely interchange-only: `label`, `agentType`, and `phase.model` — the
//! Aleph executor resolves execution via the team member named in `agent`, and
//! the run's model is decided per step, not per phase. `isolation` is a third
//! kind: it is *validated* (see [`ISOLATION_VOCABULARY`]) but not applied,
//! because every in-process agent member run is already worktree-isolated.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::workflow::compile::StepPins;
use crate::workflow::def::{WorkflowDef, WorkflowStepDef, WorkflowStepKind};

// NOTE: no `JsonSchema` derive — these types never appear in a tool arg schema
// (the `workflow` tool's args use `WorkflowDef`), so deriving it would be dead
// surface (R10/YAGNI) and needlessly assume `serde_json::Value: JsonSchema`.

/// Declarative interchange manifest. JSON keys are camelCase to match the
/// `.workflow.js` `meta` block (`dependsOn`, `whenToUse`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when_to_use: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<WorkflowPhase>,
    pub steps: Vec<WorkflowManifestStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Optional per-phase model override, mirroring the `.workflow.js`
    /// convention of adding `model` to a `meta.phases` entry. Interchange-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowManifestStep {
    pub id: String,
    pub agent: String,
    pub prompt: String,
    // `alias = "depends_on"` lets a legacy `WorkflowDef.json` (snake_case, written
    // before the store persisted the manifest superset) deserialise as a manifest
    // unchanged — backward compatibility for already-saved templates.
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "depends_on")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The phase title this step sits under (the `.workflow.js` `phase("…")`
    /// marker preceding its `agent()` call). Carried into task metadata by
    /// [`step_pins`](WorkflowManifest::step_pins) so `workflow(action='status')`
    /// can group a run's steps by phase the way the `.workflow.js` live view
    /// does. Not a scheduling input — the DAG decides order (R10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Requested output contract, a JSON Schema, passed through verbatim.
    ///
    /// Carried into task metadata by
    /// [`step_pins`](WorkflowManifest::step_pins) and rendered as an
    /// `## Output Contract` section in the member's handoff, so the step's
    /// agent is *told* what shape to return. Aleph does **not** validate the
    /// reply against it — see `WORKFLOW_SCHEMA_KEY` for why enforcement is a
    /// harness change rather than a wire. Say "requested", not "guaranteed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// `.workflow.js` `agent(..., { isolation })` hint (e.g. `"worktree"`).
    ///
    /// Validated against [`ISOLATION_VOCABULARY`] and preserved for faithful
    /// export, but never *applied*: the dispatcher already gives every
    /// in-process agent member run its own git worktree, so `"worktree"` is a
    /// declaration that already holds. This field previously read
    /// "never executed", which says the opposite of what happens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    /// `.workflow.js` `agent(..., { agentType })` — a custom subagent type.
    /// Interchange-only; Aleph resolves execution via `agent` (the team member).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// `.workflow.js` `agent(..., { effort })` — the subagent's reasoning-effort
    /// tier (`low`/`medium`/`high`/`xhigh`/`max`). Like per-step
    /// [`model`](Self::model) this is *also executable*: the `workflow` tool's
    /// `run` threads it past `to_def` into the materialised task metadata
    /// (`WORKFLOW_EFFORT_KEY`), where the dispatcher turns it into the member
    /// run's `think_level`. Validated against the live think-level vocabulary
    /// in [`validate`](WorkflowManifest::validate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Step kind — `agent` (default) or `clarify`. Part of the executable core,
    /// so it round-trips through [`to_def`](WorkflowManifest::to_def). Omitted on
    /// the wire for agent steps (byte-identical to legacy manifests).
    #[serde(default, skip_serializing_if = "WorkflowStepKind::is_agent")]
    pub kind: WorkflowStepKind,
    /// Clarify menu of answers (empty → free-text). Executable core; meaningful
    /// only for a clarify step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// Lead-review gate (see `WorkflowStepDef::review`). Executable core, so it
    /// round-trips through [`to_def`](WorkflowManifest::to_def). Omitted on the
    /// wire when false (byte-identical to legacy manifests).
    #[serde(default, skip_serializing_if = "is_false")]
    pub review: bool,
    /// Reviewer must attach measured evidence to approve (see
    /// `WorkflowStepDef::require_grounding`). Executable core; omitted on the
    /// wire when false. `require_grounding` is accepted as an alias so a
    /// serialised `WorkflowDef` round-trips.
    #[serde(default, skip_serializing_if = "is_false", alias = "require_grounding")]
    pub require_grounding: bool,
    /// Per-step run timeout in seconds (see `WorkflowStepDef::timeout_seconds`).
    /// Executable core; omitted on the wire when unset.
    ///
    /// The `timeout_seconds` alias accepts a serialised `WorkflowDef` — the
    /// exact document `workflow(action='describe')` hands back. Without it,
    /// `describe` → edit → `import` deserialised a def-shaped JSON into a
    /// manifest and dropped this field wordlessly (no `deny_unknown_fields`,
    /// so the camelCase rename made it invisible), silently reverting the step
    /// to the dispatcher's global timeout. `depends_on` already carried this
    /// treatment; these two were simply forgotten.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "timeout_seconds"
    )]
    pub timeout_secs: Option<u64>,
    /// Per-step retry ceiling (see `WorkflowStepDef::max_retries`).
    /// Executable core; omitted on the wire when unset. `max_retries` is
    /// accepted as an alias for the same def-shaped-document reason.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "max_retries"
    )]
    pub max_retries: Option<u32>,
}

/// serde `skip_serializing_if` helper — keeps non-reviewed steps byte-identical.

/// The `isolation` values a manifest step may declare.
///
/// Aleph does not *apply* this hint — it does not need to: the dispatcher gives
/// **every** in-process agent member run its own git worktree unconditionally
/// (`schedule/mod.rs` sets `isolate = matches!(target, MemberDispatchTarget::Agent{..})`,
/// and `runner.rs` takes a `WorktreeHandle` from it). So `isolation: "worktree"`
/// is a declaration that already holds, and `"none"` is a declaration Aleph
/// cannot honour for an agent step. The module used to describe this field as
/// "never executed", which reads as *your isolation request is ignored* — the
/// alarming direction, and false. What is worth doing here is refusing a value
/// nobody can interpret, so a typo (`"worktee"`) fails at the boundary instead
/// of riding through import → save → export as data that looks meaningful.
pub const ISOLATION_VOCABULARY: &[&str] = &["worktree", "none"];
const fn is_false(v: &bool) -> bool {
    !*v
}

impl WorkflowManifest {
    /// Build a manifest from the executable core. Extra metadata fields start
    /// empty/None — a `WorkflowDef` carries none of them.
    #[must_use]
    pub fn from_def(def: &WorkflowDef) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            when_to_use: String::new(),
            phases: Vec::new(),
            steps: def
                .steps
                .iter()
                .map(|s| WorkflowManifestStep {
                    id: s.id.clone(),
                    agent: s.agent.clone(),
                    prompt: s.prompt.clone(),
                    depends_on: s.depends_on.clone(),
                    label: None,
                    model: None,
                    phase: None,
                    schema: None,
                    isolation: None,
                    agent_type: None,
                    effort: None,
                    kind: s.kind,
                    choices: s.choices.clone(),
                    review: s.review,
                    require_grounding: s.require_grounding,
                    timeout_secs: s.timeout_seconds,
                    max_retries: s.max_retries,
                })
                .collect(),
        }
    }

    /// Re-author the executable core from `def`, KEEPING every extra this
    /// manifest already carries (per-step `model` / `effort` / `label` /
    /// `phase` / `schema` / `isolation` / `agentType`, plus `whenToUse` and the
    /// phase plan), matched by step id.
    ///
    /// `save` used to persist `from_def(&definition)` unconditionally, i.e. an
    /// extras-stripped copy — and `import`'s own remediation note tells the
    /// user to "retarget the agents (edit + save) before running". Following
    /// that instruction after importing an engineering `.mjs` silently deleted
    /// its per-step model and effort pins, which are NOT decoration: `run`
    /// reads them into the `WORKFLOW_MODEL_KEY` / `WORKFLOW_EFFORT_KEY` stamps
    /// that decide which model and reasoning tier each step actually executes
    /// on. Steps the def added or renamed simply get empty extras.
    #[must_use]
    pub fn with_core_from(&self, def: &WorkflowDef) -> Self {
        let mut fresh = Self::from_def(def);
        // A def cannot express these at all, so an author working through
        // `describe` → edit → `save` can never intend to clear them.
        fresh.when_to_use = self.when_to_use.clone();
        fresh.phases = self.phases.clone();
        for step in &mut fresh.steps {
            let Some(prev) = self.steps.iter().find(|s| s.id == step.id) else {
                continue;
            };
            step.label = prev.label.clone();
            step.model = prev.model.clone();
            step.phase = prev.phase.clone();
            step.schema = prev.schema.clone();
            step.isolation = prev.isolation.clone();
            step.agent_type = prev.agent_type.clone();
            step.effort = prev.effort.clone();
        }
        fresh
    }

    /// Validate the manifest: the executable projection (`WorkflowDef` checks —
    /// unique ids, resolvable deps, acyclic) plus the one executable extra with
    /// a vocabulary of its own — per-step `effort`, which must normalise
    /// through the live think-level table (`low`..`max` and its aliases) so an
    /// unknown tier fails at the save/import boundary instead of being
    /// silently ignored at run time. The remaining metadata
    /// (`label`/`model`/`phase`/`schema`/`isolation`/`agentType`/`phases`/
    /// `whenToUse`) is pure interchange data with no invariants. Pure —
    /// touches no store.
    pub fn validate(&self) -> Result<()> {
        for step in &self.steps {
            if let Some(effort) = step
                .effort
                .as_deref()
                .map(str::trim)
                .filter(|e| !e.is_empty())
            {
                if crate::agents::thinking::normalize_think_level(effort).is_none() {
                    return Err(crate::error::AlephError::invalid_input(format!(
                        "step '{}': unknown effort '{effort}' — use one of \
                         low/medium/high/xhigh/max",
                        step.id
                    )));
                }
            }
            if let Some(iso) = step
                .isolation
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if !ISOLATION_VOCABULARY.contains(&iso) {
                    return Err(crate::error::AlephError::invalid_input(format!(
                        "step '{}': unknown isolation '{iso}' — use one of {}",
                        step.id,
                        ISOLATION_VOCABULARY.join("/")
                    )));
                }
            }
        }
        self.to_def().validate()
    }

    /// Per-step [`StepPins`] keyed by step-local id, for every **agent** step
    /// that pins at least one override. Clarify steps run no agent, so they are
    /// skipped — stamping them would put keys on a row nothing reads.
    ///
    /// This is the single place the manifest's four executable-or-reportable
    /// extras become a materialisation input. `run` used to build two parallel
    /// `HashMap<String, String>`s inline (one for `model`, one for `effort`),
    /// which is why `effort` reached task metadata and no reporting surface:
    /// the maps were separate, so growing a face for one grew nothing for the
    /// other.
    #[must_use]
    pub fn step_pins(&self) -> std::collections::HashMap<String, StepPins> {
        let mut out = std::collections::HashMap::new();
        for step in &self.steps {
            if step.kind == WorkflowStepKind::Clarify {
                continue;
            }
            let non_blank = |v: &Option<String>| {
                v.as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let pins = StepPins {
                model: non_blank(&step.model),
                effort: non_blank(&step.effort),
                phase: non_blank(&step.phase),
                schema: step.schema.clone().filter(|s| !s.is_null()),
            };
            if !pins.is_empty() {
                out.insert(step.id.clone(), pins);
            }
        }
        out
    }

    /// Name every kind of metadata this manifest carries that a
    /// [`WorkflowDef`] cannot express, in a stable order.
    ///
    /// Two faces need this same answer and had two different partial versions
    /// of it: `save` reported "per-step model/effort pins preserved" (a
    /// hand-written pair that never learned about `phase` / `schema` /
    /// `whenToUse` / the phase plan), and `import` reported nothing at all —
    /// while its own remediation advice ("retarget the agents: edit + save")
    /// routes a freshly-parsed rich `.workflow.js` through a lean `WorkflowDef`
    /// with **nothing stored yet**, so `save`'s preservation path
    /// ([`with_core_from`](Self::with_core_from)) cannot fire and every extra
    /// is dropped on the first save.
    ///
    /// Derived by exhaustive check rather than by a remembered list, so a new
    /// manifest-only field is a compile-visible edit here instead of a silent
    /// omission on both faces at once.
    #[must_use]
    pub fn def_inexpressible_extras(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.when_to_use.trim().is_empty() {
            out.push("whenToUse");
        }
        if !self.phases.is_empty() {
            out.push("phases");
        }
        let any = |f: fn(&WorkflowManifestStep) -> bool| self.steps.iter().any(f);
        if any(|s| s.label.is_some()) {
            out.push("label");
        }
        if any(|s| s.model.is_some()) {
            out.push("model");
        }
        if any(|s| s.effort.is_some()) {
            out.push("effort");
        }
        if any(|s| s.phase.is_some()) {
            out.push("phase");
        }
        if any(|s| s.schema.is_some()) {
            out.push("schema");
        }
        if any(|s| s.isolation.is_some()) {
            out.push("isolation");
        }
        if any(|s| s.agent_type.is_some()) {
            out.push("agentType");
        }
        out
    }
    /// Project to the executable core, dropping extra metadata. Callers
    /// typically `validate()` the result before persisting or running.
    #[must_use]
    pub fn to_def(&self) -> WorkflowDef {
        WorkflowDef {
            name: self.name.clone(),
            description: self.description.clone(),
            steps: self
                .steps
                .iter()
                .map(|s| WorkflowStepDef {
                    id: s.id.clone(),
                    agent: s.agent.clone(),
                    prompt: s.prompt.clone(),
                    depends_on: s.depends_on.clone(),
                    kind: s.kind,
                    choices: s.choices.clone(),
                    review: s.review,
                    require_grounding: s.require_grounding,
                    timeout_seconds: s.timeout_secs,
                    max_retries: s.max_retries,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_def() -> WorkflowDef {
        WorkflowDef {
            name: "rep".into(),
            description: "demo".into(),
            steps: vec![
                WorkflowStepDef {
                    id: "a".into(),
                    agent: "researcher".into(),
                    prompt: "research {input}".into(),
                    depends_on: vec![],
                    kind: WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    timeout_seconds: None,
                    max_retries: None,
                },
                WorkflowStepDef {
                    id: "b".into(),
                    agent: "writer".into(),
                    prompt: "write".into(),
                    depends_on: vec!["a".into()],
                    kind: WorkflowStepKind::Agent,
                    choices: vec![],
                    review: false,
                    require_grounding: false,
                    timeout_seconds: None,
                    max_retries: None,
                },
            ],
        }
    }

    #[test]
    fn from_def_then_to_def_preserves_core() {
        let def = core_def();
        let manifest = WorkflowManifest::from_def(&def);
        assert_eq!(manifest.to_def(), def);
    }

    /// `save` re-authors the executable core but must not DELETE what the core
    /// cannot express. It used to persist `from_def(&definition)` — an
    /// extras-stripped copy — and import's own remediation note ("retarget the
    /// agents: edit + save") walked users straight into it, silently dropping
    /// per-step `model` / `effort`, which are executable.
    #[test]
    fn with_core_from_keeps_executable_extras_of_surviving_steps() {
        let mut stored = WorkflowManifest::from_def(&core_def());
        stored.when_to_use = "nightly briefings".into();
        stored.phases = vec![WorkflowPhase {
            title: "Verify".into(),
            detail: "four adversarial reviewers".into(),
            model: Some("opus".into()),
        }];
        stored.steps[0].model = Some("opus".into());
        stored.steps[0].effort = Some("max".into());
        stored.steps[0].schema = Some(serde_json::json!({"type": "object"}));

        // The author edits an unrelated thing and saves the def back.
        let mut edited = core_def();
        edited.steps[0].prompt = "a reworded prompt".into();
        let merged = stored.with_core_from(&edited);

        assert_eq!(
            merged.steps[0].prompt, "a reworded prompt",
            "core is re-authored"
        );
        assert_eq!(merged.steps[0].model.as_deref(), Some("opus"));
        assert_eq!(merged.steps[0].effort.as_deref(), Some("max"));
        assert!(merged.steps[0].schema.is_some());
        assert_eq!(merged.when_to_use, "nightly briefings");
        assert_eq!(merged.phases.len(), 1);
    }

    /// A serialised `WorkflowDef` — exactly what `describe` hands back — must
    /// deserialise into a manifest without losing its executable budget fields.
    /// Only `depends_on` carried a back-compat alias; these two were forgotten,
    /// so `describe` → edit → `import` silently reverted the step to the
    /// dispatcher's global timeout and retry ceiling.
    #[test]
    fn def_shaped_json_keeps_timeout_retries_and_grounding() {
        let step: WorkflowManifestStep = serde_json::from_value(serde_json::json!({
            "id": "crawl",
            "agent": "researcher",
            "prompt": "go",
            "depends_on": ["seed"],
            "review": true,
            "require_grounding": true,
            "timeout_seconds": 3600,
            "max_retries": 2
        }))
        .unwrap();
        assert_eq!(step.depends_on, vec!["seed".to_string()]);
        assert_eq!(step.timeout_secs, Some(3600));
        assert_eq!(step.max_retries, Some(2));
        assert!(step.require_grounding);
    }

    #[test]
    fn from_def_leaves_extras_empty() {
        let manifest = WorkflowManifest::from_def(&core_def());
        assert!(manifest.when_to_use.is_empty());
        assert!(manifest.phases.is_empty());
        assert!(manifest.steps.iter().all(|s| s.label.is_none()
            && s.model.is_none()
            && s.phase.is_none()
            && s.schema.is_none()
            && s.isolation.is_none()
            && s.agent_type.is_none()
            && s.effort.is_none()));
    }

    #[test]
    fn to_def_drops_extra_metadata() {
        let manifest = WorkflowManifest {
            name: "x".into(),
            description: "d".into(),
            when_to_use: "when".into(),
            phases: vec![WorkflowPhase {
                title: "P".into(),
                detail: "det".into(),
                model: Some("opus".into()),
            }],
            steps: vec![WorkflowManifestStep {
                id: "s".into(),
                agent: "ag".into(),
                prompt: "p".into(),
                depends_on: vec![],
                label: Some("L".into()),
                model: Some("haiku".into()),
                phase: Some("P".into()),
                schema: Some(serde_json::json!({"type":"object"})),
                isolation: Some("worktree".into()),
                agent_type: Some("Explore".into()),
                effort: Some("high".into()),
                kind: WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let def = manifest.to_def();
        assert_eq!(def.name, "x");
        assert_eq!(def.steps.len(), 1);
        assert_eq!(def.steps[0].id, "s");
        // WorkflowStepDef has no label/model/phase/schema/isolation/agentType
        // fields to carry — their absence is structural.
    }

    #[test]
    fn serde_uses_camel_case_keys() {
        let manifest = WorkflowManifest {
            name: "x".into(),
            description: String::new(),
            when_to_use: "w".into(),
            phases: vec![],
            steps: vec![WorkflowManifestStep {
                id: "s".into(),
                agent: "ag".into(),
                prompt: "p".into(),
                depends_on: vec!["dep".into()],
                label: None,
                model: None,
                phase: None,
                schema: None,
                isolation: Some("worktree".into()),
                agent_type: Some("code-reviewer".into()),
                effort: Some("max".into()),
                kind: WorkflowStepKind::Agent,
                choices: vec![],
                review: false,
                require_grounding: false,
                timeout_secs: None,
                max_retries: None,
            }],
        };
        let v = serde_json::to_value(&manifest).unwrap();
        assert!(v.get("whenToUse").is_some(), "whenToUse camelCase");
        assert!(
            v["steps"][0].get("dependsOn").is_some(),
            "dependsOn camelCase"
        );
        // `agent_type` serialises as the `.workflow.js` `agentType` key.
        assert!(
            v["steps"][0].get("agentType").is_some(),
            "agentType camelCase"
        );
        assert_eq!(v["steps"][0]["isolation"], "worktree");
        // `effort` rides the same interchange lane as isolation/agentType.
        assert_eq!(v["steps"][0]["effort"], "max");
        // Empty extras are skipped on the wire.
        assert!(v.get("phases").is_none(), "empty phases skipped");
    }

    #[test]
    fn validate_checks_effort_against_think_level_vocabulary() {
        let mut manifest = WorkflowManifest::from_def(&core_def());
        // Every .workflow.js effort tier normalises through the live table.
        for tier in ["low", "medium", "high", "xhigh", "max"] {
            manifest.steps[0].effort = Some(tier.into());
            manifest
                .validate()
                .unwrap_or_else(|e| panic!("'{tier}' must validate: {e}"));
        }
        // An unknown tier fails at the boundary, naming the step.
        manifest.steps[0].effort = Some("turbo".into());
        let err = manifest.validate().expect_err("unknown effort rejected");
        assert!(err.to_string().contains("unknown effort 'turbo'"), "{err}");
        assert!(err.to_string().contains("step 'a'"), "{err}");
        // Absent / blank effort stays valid (interchange rows unchanged).
        manifest.steps[0].effort = Some("  ".into());
        manifest.validate().expect("blank effort is no effort");
        manifest.steps[0].effort = None;
        manifest.validate().expect("absent effort validates");
    }

    #[test]
    fn manifest_roundtrips_through_json() {
        let manifest = WorkflowManifest::from_def(&core_def());
        let s = serde_json::to_string(&manifest).unwrap();
        let back: WorkflowManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(manifest, back);
    }

    #[test]
    fn review_flag_roundtrips_through_def_and_json() {
        let mut def = core_def();
        def.steps[1].review = true;

        // The review gate survives the manifest projection both ways.
        let manifest = WorkflowManifest::from_def(&def);
        assert!(!manifest.steps[0].review);
        assert!(manifest.steps[1].review);
        assert_eq!(manifest.to_def(), def);

        // On the wire: present only on the gated step (legacy byte-identical).
        let v = serde_json::to_value(&manifest).unwrap();
        assert!(v["steps"][0].get("review").is_none(), "ungated omits key");
        assert_eq!(v["steps"][1]["review"], true);
        let s = serde_json::to_string(&manifest).unwrap();
        let back: WorkflowManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(manifest, back);
    }

    #[test]
    fn clarify_step_roundtrips_through_def_and_json() {
        let def = WorkflowDef {
            name: "deploy".into(),
            description: String::new(),
            steps: vec![WorkflowStepDef {
                id: "ask".into(),
                agent: String::new(),
                prompt: "Deploy where?".into(),
                depends_on: vec![],
                kind: WorkflowStepKind::Clarify,
                choices: vec!["staging".into(), "prod".into()],
                review: false,
                require_grounding: false,
                timeout_seconds: None,
                max_retries: None,
            }],
        };
        // The clarify kind + choices survive the manifest projection both ways.
        let manifest = WorkflowManifest::from_def(&def);
        assert_eq!(manifest.steps[0].kind, WorkflowStepKind::Clarify);
        assert_eq!(manifest.steps[0].choices, vec!["staging", "prod"]);
        assert_eq!(manifest.to_def(), def);

        // And they survive the on-disk JSON round-trip (camelCase keys present).
        let v = serde_json::to_value(&manifest).unwrap();
        assert_eq!(v["steps"][0]["kind"], "clarify");
        let s = serde_json::to_string(&manifest).unwrap();
        let back: WorkflowManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(manifest, back);
    }

    #[test]
    fn step_pins_collects_only_agent_steps_with_pins() {
        let mut m = WorkflowManifest::from_def(&core_def());
        m.steps[0].model = Some("opus".into());
        m.steps[0].effort = Some("max".into());
        m.steps[0].phase = Some("Scan".into());
        m.steps[0].schema = Some(serde_json::json!({"type": "object"}));
        // Blank strings are "unset spelled differently".
        m.steps[1].model = Some("   ".into());

        let pins = m.step_pins();
        assert_eq!(pins.len(), 1, "only the really-pinned step appears");
        let p = pins.get("a").expect("step a pinned");
        assert_eq!(p.model.as_deref(), Some("opus"));
        assert_eq!(p.effort.as_deref(), Some("max"));
        assert_eq!(p.phase.as_deref(), Some("Scan"));
        assert!(p.schema.is_some());
        assert_eq!(p.census(), StepPins::all_fields());
    }

    #[test]
    fn step_pins_skips_clarify_steps() {
        // A clarify step runs no agent — stamping it would put keys on a row
        // nothing reads.
        let mut m = WorkflowManifest::from_def(&core_def());
        m.steps[0].kind = WorkflowStepKind::Clarify;
        m.steps[0].agent = String::new();
        m.steps[0].model = Some("opus".into());
        assert!(m.step_pins().is_empty());
    }

    #[test]
    fn validate_rejects_unknown_isolation() {
        // `isolation` is validated so a typo fails at the boundary instead of
        // riding through import → save → export as meaningful-looking data.
        let mut m = WorkflowManifest::from_def(&core_def());
        m.steps[0].isolation = Some("worktee".into());
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("unknown isolation"), "{err}");
        m.steps[0].isolation = Some("worktree".into());
        assert!(m.validate().is_ok(), "{:?}", m.validate());
    }
}
