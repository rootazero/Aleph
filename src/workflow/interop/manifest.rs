//! AWI (Aleph Workflow Interchange) manifest — the declarative superset that
//! bridges Aleph's `WorkflowDef` and Claude Code's `.workflow.js` format.
//!
//! Pure data + field shuffling, no reasoning (R7/R10). The manifest carries the
//! full `.workflow.js`-compatible metadata (`whenToUse`, `phases` with optional
//! per-phase `model`, per-step `label`/`model`/`phase`/`schema`/`isolation`/
//! `agentType`); only the executable core round-trips into `WorkflowDef`, the
//! rest is preserved for lossless export. Per-step `model` and `effort` are the
//! two extras that are *also executable*: the `workflow` tool's `run` threads
//! them past `to_def` into the materialised task metadata, where the dispatcher
//! turns them into a per-member model override / think-level. The remaining
//! agent-opt fields (`isolation`, `agentType`) and `phase.model` stay
//! interchange-only — the Aleph executor never consumes them (R10), exactly
//! like the opaque `schema` pass-through.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Opaque JSON Schema, passed through verbatim. Aleph never interprets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// `.workflow.js` `agent(..., { isolation })` hint (e.g. `"worktree"`).
    /// Interchange-only — preserved for faithful export, never executed.
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
    /// Per-step run timeout in seconds (see `WorkflowStepDef::timeout_seconds`).
    /// Executable core; omitted on the wire when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Per-step retry ceiling (see `WorkflowStepDef::max_retries`).
    /// Executable core; omitted on the wire when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

/// serde `skip_serializing_if` helper — keeps non-reviewed steps byte-identical.
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
                    timeout_secs: s.timeout_seconds,
                    max_retries: s.max_retries,
                })
                .collect(),
        }
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
        }
        self.to_def().validate()
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
}
