//! Team template data model.
//!
//! Parsed verbatim from TOML; carries no runtime state. Materialization
//! happens in [`crate::teams::templates::materialize`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Priority hint copied into the materialized [`CoordTask`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TemplatePriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl TemplatePriority {
    #[must_use]
    pub const fn as_coord_priority(self) -> crate::agents::swarm::tasks::Priority {
        use crate::agents::swarm::tasks::Priority;
        match self {
            Self::Low => Priority::Low,
            Self::Normal => Priority::Normal,
            Self::High => Priority::High,
            Self::Critical => Priority::Critical,
        }
    }
}

/// Leader definition. The leader is auto-enrolled as the team leader at
/// materialization time and inherits the calling agent's identity when
/// `id = "self"` (the conventional placeholder).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TemplateLeader {
    /// Agent id. Use `"self"` to mean "the calling agent becomes leader".
    pub id: String,
    /// Display name (defaults to id when None).
    #[serde(default)]
    pub name: Option<String>,
    /// Role label written into `team_members.role` (default: `"leader"`).
    #[serde(default)]
    pub role: Option<String>,
    /// LLM model override; if None the calling agent's model is reused.
    #[serde(default)]
    pub model: Option<String>,
    /// Prompt addendum appended to SOUL.md under a `## Team Role` heading.
    #[serde(default)]
    pub prompt_addendum: Option<String>,
    /// Tools this leader may call. `None` (the default) = every tool, which is
    /// how every team behaved before members could declare a surface. Entries
    /// support a trailing-`*` prefix glob (`task_*`). A declared list must
    /// still admit the orchestration verbs the leader prompt contracts it to
    /// call — see `teams::member_provision::LEADER_ESSENTIAL_TOOLS`.
    /// Ignored when `id = "self"`: the caller is an existing agent and keeps
    /// its own surface.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Tools explicitly withheld from this leader, applied over `tools`.
    #[serde(default)]
    pub tools_denied: Option<Vec<String>>,
}

/// Worker (non-leader) member definition.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TemplateMember {
    /// Agent id. Reused if it already exists; created inline otherwise.
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Role label (e.g. "backend", "qa"). Written into `team_members.role`.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Prompt addendum appended to SOUL.md under `## Team Role`.
    #[serde(default)]
    pub prompt_addendum: Option<String>,
    /// Tools this member may call. `None` (the default) = every tool, matching
    /// the behaviour of every team created before this field existed. Entries
    /// support a trailing-`*` prefix glob (`task_*`). A declared list must
    /// still admit the hand-off verbs the member prompt contracts it to call —
    /// see `teams::member_provision::WORKER_ESSENTIAL_TOOLS`. Ignored when the
    /// id names an agent that already exists (it keeps its own surface).
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Tools explicitly withheld from this member, applied over `tools`.
    #[serde(default)]
    pub tools_denied: Option<Vec<String>>,
}

/// Initial task definition. References sibling tasks via `depends_on` keys
/// rather than subjects, so substitution can't accidentally break a DAG edge.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TemplateTask {
    /// Stable identifier used by other tasks' `depends_on` lists.
    /// MUST be unique within the template.
    pub key: String,
    /// Task subject. Supports `{goal}`/`{team_name}`/`{leader}` substitution.
    pub subject: String,
    /// Task description. Supports substitution.
    #[serde(default)]
    pub description: String,
    /// Owner agent id — must match a leader/member `id` in the same template.
    pub owner: String,
    #[serde(default)]
    pub priority: TemplatePriority,
    /// Keys of tasks that must complete before this one is schedulable.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Top-level template document.
///
/// `name` is conventionally provided by the loader (file stem or registry
/// key) rather than the TOML body itself — including it as `#[serde(default)]`
/// keeps user TOML files brief while preventing two files from claiming the
/// same registry id.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamTemplate {
    /// Template identifier (filename stem if loaded from disk). Always
    /// overwritten by the loader to match the registry key.
    #[serde(default)]
    pub name: String,
    /// One-line description shown in the picker.
    #[serde(default)]
    pub description: String,
    /// Optional default goal used when the caller leaves `goal` unset.
    #[serde(default)]
    pub default_goal: Option<String>,
    /// Optional team-level strategy prompt — folded into every member's
    /// SOUL.md as a `## Team Strategy` section during materialization.
    ///
    /// R3 (`ClawTeam` parity): lets a template encode the collaboration
    /// mode (plan-then-act, parallel-debate, review-loop, ...) once at
    /// the team scope instead of repeating it on every member's
    /// `prompt_addendum`. Per R7/R10 this is **pure prompt injection** —
    /// the dispatcher never branches on strategy.
    ///
    /// Supports `{goal}`/`{team_name}`/`{leader}` substitution like
    /// other prompt fields.
    #[serde(default)]
    pub strategy: Option<String>,
    /// Leader spec (required — every team has exactly one leader).
    pub leader: TemplateLeader,
    /// Worker members (zero or more).
    #[serde(default)]
    pub members: Vec<TemplateMember>,
    /// Initial task DAG (zero or more).
    #[serde(default)]
    pub tasks: Vec<TemplateTask>,
}

impl TeamTemplate {
    /// Reject structurally invalid templates: duplicate ids, dangling
    /// `owner`/`depends_on` references, duplicate task keys.
    pub fn validate(&self) -> Result<(), TeamTemplateError> {
        let mut seen_member_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        seen_member_ids.insert(self.leader.id.as_str());

        for m in &self.members {
            if !seen_member_ids.insert(m.id.as_str()) {
                return Err(TeamTemplateError::DuplicateMember(m.id.clone()));
            }
        }

        let mut seen_keys: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in &self.tasks {
            if !seen_keys.insert(t.key.as_str()) {
                return Err(TeamTemplateError::DuplicateTaskKey(t.key.clone()));
            }
            if !seen_member_ids.contains(t.owner.as_str()) {
                return Err(TeamTemplateError::UnknownOwner {
                    task_key: t.key.clone(),
                    owner: t.owner.clone(),
                });
            }
        }
        // depends_on validation done in a second pass since keys may
        // forward-reference siblings.
        for t in &self.tasks {
            for dep in &t.depends_on {
                if !seen_keys.contains(dep.as_str()) {
                    return Err(TeamTemplateError::UnknownDependency {
                        task_key: t.key.clone(),
                        depends_on: dep.clone(),
                    });
                }
                if dep == &t.key {
                    return Err(TeamTemplateError::SelfDependency(t.key.clone()));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum TeamTemplateError {
    #[error("template not found: {0}")]
    NotFound(String),
    #[error("failed to parse template TOML: {0}")]
    Parse(String),
    #[error("template I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("duplicate member id `{0}`")]
    DuplicateMember(String),
    #[error("duplicate task key `{0}`")]
    DuplicateTaskKey(String),
    #[error("task `{task_key}` references unknown owner `{owner}`")]
    UnknownOwner { task_key: String, owner: String },
    #[error("task `{task_key}` depends on unknown key `{depends_on}`")]
    UnknownDependency {
        task_key: String,
        depends_on: String,
    },
    #[error("task `{0}` cannot depend on itself")]
    SelfDependency(String),
    #[error("materialization failed: {0}")]
    Materialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_template() -> TeamTemplate {
        TeamTemplate {
            name: "t".into(),
            description: String::new(),
            default_goal: None,
            strategy: None,
            leader: TemplateLeader {
                id: "lead".into(),
                name: None,
                role: None,
                model: None,
                prompt_addendum: None,
                tools: None,
                tools_denied: None,
            },
            members: vec![],
            tasks: vec![],
        }
    }

    #[test]
    fn rejects_duplicate_member_id() {
        let mut t = minimal_template();
        t.members.push(TemplateMember {
            id: "lead".into(), // collides with leader
            name: None,
            role: None,
            model: None,
            prompt_addendum: None,
            tools: None,
            tools_denied: None,
        });
        assert!(matches!(
            t.validate(),
            Err(TeamTemplateError::DuplicateMember(_))
        ));
    }

    #[test]
    fn rejects_dangling_owner() {
        let mut t = minimal_template();
        t.tasks.push(TemplateTask {
            key: "k1".into(),
            subject: "s".into(),
            description: String::new(),
            owner: "ghost".into(),
            priority: TemplatePriority::default(),
            depends_on: vec![],
        });
        assert!(matches!(
            t.validate(),
            Err(TeamTemplateError::UnknownOwner { .. })
        ));
    }

    #[test]
    fn rejects_dangling_dep() {
        let mut t = minimal_template();
        t.tasks.push(TemplateTask {
            key: "k1".into(),
            subject: "s".into(),
            description: String::new(),
            owner: "lead".into(),
            priority: TemplatePriority::default(),
            depends_on: vec!["k2".into()],
        });
        assert!(matches!(
            t.validate(),
            Err(TeamTemplateError::UnknownDependency { .. })
        ));
    }

    #[test]
    fn rejects_self_dep() {
        let mut t = minimal_template();
        t.tasks.push(TemplateTask {
            key: "k1".into(),
            subject: "s".into(),
            description: String::new(),
            owner: "lead".into(),
            priority: TemplatePriority::default(),
            depends_on: vec!["k1".into()],
        });
        assert!(matches!(
            t.validate(),
            Err(TeamTemplateError::SelfDependency(_))
        ));
    }

    #[test]
    fn accepts_well_formed_dag() {
        let mut t = minimal_template();
        t.members.push(TemplateMember {
            id: "w".into(),
            name: None,
            role: None,
            model: None,
            prompt_addendum: None,
            tools: None,
            tools_denied: None,
        });
        t.tasks.push(TemplateTask {
            key: "k1".into(),
            subject: "s".into(),
            description: String::new(),
            owner: "lead".into(),
            priority: TemplatePriority::default(),
            depends_on: vec![],
        });
        t.tasks.push(TemplateTask {
            key: "k2".into(),
            subject: "s".into(),
            description: String::new(),
            owner: "w".into(),
            priority: TemplatePriority::default(),
            depends_on: vec!["k1".into()],
        });
        assert!(t.validate().is_ok());
    }
}
