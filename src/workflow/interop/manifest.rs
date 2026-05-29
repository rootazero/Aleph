//! AWI (Aleph Workflow Interchange) manifest — the declarative superset that
//! bridges Aleph's `WorkflowDef` and Claude Code's `.workflow.js` format.
//!
//! Pure data + field shuffling, no reasoning (R7/R10). The manifest carries the
//! full `.workflow.js`-compatible metadata (`whenToUse`, `phases`, per-step
//! `label`/`model`/`phase`/`schema`); only the executable core round-trips into
//! `WorkflowDef`, the rest is preserved for lossless export.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::workflow::def::{WorkflowDef, WorkflowStepDef};

// NOTE: no `JsonSchema` derive — these types never appear in a tool arg schema
// (the `workflow` tool's args use `WorkflowDef`), so deriving it would be dead
// surface (R10/YAGNI) and needlessly assume `serde_json::Value: JsonSchema`.

/// Declarative interchange manifest. JSON keys are camelCase to match the
/// `.workflow.js` `meta` block (`dependsOn`, `whenToUse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowManifestStep {
    pub id: String,
    pub agent: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
}

impl WorkflowManifest {
    /// Build a manifest from the executable core. Extra metadata fields start
    /// empty/None — a `WorkflowDef` carries none of them.
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
                })
                .collect(),
        }
    }

    /// Project to the executable core, dropping extra metadata. Callers
    /// typically `validate()` the result before persisting or running.
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
                },
                WorkflowStepDef {
                    id: "b".into(),
                    agent: "writer".into(),
                    prompt: "write".into(),
                    depends_on: vec!["a".into()],
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
            && s.schema.is_none()));
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
            }],
        };
        let def = manifest.to_def();
        assert_eq!(def.name, "x");
        assert_eq!(def.steps.len(), 1);
        assert_eq!(def.steps[0].id, "s");
        // WorkflowStepDef has no label/model/phase/schema fields to carry —
        // their absence is structural.
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
            }],
        };
        let v = serde_json::to_value(&manifest).unwrap();
        assert!(v.get("whenToUse").is_some(), "whenToUse camelCase");
        assert!(v["steps"][0].get("dependsOn").is_some(), "dependsOn camelCase");
        // Empty extras are skipped on the wire.
        assert!(v.get("phases").is_none(), "empty phases skipped");
    }

    #[test]
    fn manifest_roundtrips_through_json() {
        let manifest = WorkflowManifest::from_def(&core_def());
        let s = serde_json::to_string(&manifest).unwrap();
        let back: WorkflowManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(manifest, back);
    }
}
