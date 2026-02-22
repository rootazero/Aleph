//! Workspace isolation for memory system
//!
//! Provides workspace-scoped memory isolation, allowing users to organize
//! facts and memories into separate logical spaces with independent
//! configuration.

use serde::{Deserialize, Serialize};

use crate::memory::context::FactType;

/// Default workspace identifier
pub const DEFAULT_WORKSPACE: &str = "default";

/// A workspace represents an isolated memory context with its own configuration.
///
/// Workspaces allow users to partition their memory facts into separate spaces
/// (e.g., "work", "personal", "project-x"), each with independent decay rates,
/// allowed tools, and model overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// Unique workspace identifier (URL-safe slug)
    pub id: String,

    /// Human-readable display name
    pub name: String,

    /// Optional description of the workspace purpose
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional emoji or icon identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Workspace-specific configuration overrides
    #[serde(default)]
    pub config: WorkspaceConfig,

    /// Whether this is the default workspace
    #[serde(default)]
    pub is_default: bool,

    /// Whether this workspace is archived (soft-deleted)
    #[serde(default)]
    pub is_archived: bool,

    /// Creation timestamp (unix seconds)
    pub created_at: i64,

    /// Last update timestamp (unix seconds)
    pub updated_at: i64,
}

impl Workspace {
    /// Create a new workspace with the given id and name.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            icon: None,
            config: WorkspaceConfig::default(),
            is_default: false,
            is_archived: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create the default workspace instance.
    pub fn default_workspace() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: DEFAULT_WORKSPACE.to_string(),
            name: "Default".to_string(),
            description: Some("Default workspace for all memories".to_string()),
            icon: None,
            config: WorkspaceConfig::default(),
            is_default: true,
            is_archived: false,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Configuration overrides for a workspace.
///
/// All fields are optional; when `None`, the global default applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Memory decay rate override (0.0 = no decay, 1.0 = maximum decay)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_rate: Option<f64>,

    /// Fact types that should never decay in this workspace
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permanent_fact_types: Vec<FactType>,

    /// Default AI provider override (e.g., "anthropic", "openai")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    /// Default model override (e.g., "claude-sonnet-4-20250514")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// System prompt override for this workspace
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,

    /// Allowlist of tool names available in this workspace (empty = all tools)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            decay_rate: None,
            permanent_fact_types: Vec::new(),
            default_provider: None,
            default_model: None,
            system_prompt_override: None,
            allowed_tools: Vec::new(),
        }
    }
}

/// Filter for selecting workspaces when querying memory facts.
#[derive(Debug, Clone)]
pub enum WorkspaceFilter {
    /// Filter to a single workspace by id
    Single(String),
    /// Filter to multiple workspaces by id
    Multiple(Vec<String>),
    /// No filtering — include all workspaces
    All,
}

impl WorkspaceFilter {
    /// Convert the filter to a SQL WHERE clause fragment.
    ///
    /// Returns a string suitable for use in a SQL `WHERE` clause that filters
    /// on the `workspace` column.
    pub fn to_sql_filter(&self) -> String {
        match self {
            WorkspaceFilter::Single(id) => {
                format!("workspace = '{}'", id.replace('\'', "''"))
            }
            WorkspaceFilter::Multiple(ids) => {
                if ids.is_empty() {
                    return "1=0".to_string(); // match nothing
                }
                let escaped: Vec<String> = ids
                    .iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect();
                format!("workspace IN ({})", escaped.join(", "))
            }
            WorkspaceFilter::All => "1=1".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_workspace() {
        let ws = Workspace::default_workspace();
        assert_eq!(ws.id, DEFAULT_WORKSPACE);
        assert_eq!(ws.name, "Default");
        assert!(ws.is_default);
        assert!(!ws.is_archived);
        assert!(ws.description.is_some());
        assert!(ws.created_at > 0);
        assert_eq!(ws.created_at, ws.updated_at);
    }

    #[test]
    fn test_new_workspace() {
        let ws = Workspace::new("my-project", "My Project");
        assert_eq!(ws.id, "my-project");
        assert_eq!(ws.name, "My Project");
        assert!(!ws.is_default);
        assert!(!ws.is_archived);
        assert!(ws.description.is_none());
        assert!(ws.icon.is_none());
        assert!(ws.created_at > 0);
    }

    #[test]
    fn test_workspace_config_defaults() {
        let config = WorkspaceConfig::default();
        assert!(config.decay_rate.is_none());
        assert!(config.permanent_fact_types.is_empty());
        assert!(config.default_provider.is_none());
        assert!(config.default_model.is_none());
        assert!(config.system_prompt_override.is_none());
        assert!(config.allowed_tools.is_empty());
    }

    #[test]
    fn test_workspace_filter_sql() {
        // Single filter
        let f = WorkspaceFilter::Single("work".to_string());
        assert_eq!(f.to_sql_filter(), "workspace = 'work'");

        // Multiple filter
        let f = WorkspaceFilter::Multiple(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(f.to_sql_filter(), "workspace IN ('a', 'b')");

        // Empty multiple produces match-nothing
        let f = WorkspaceFilter::Multiple(vec![]);
        assert_eq!(f.to_sql_filter(), "1=0");

        // All filter
        let f = WorkspaceFilter::All;
        assert_eq!(f.to_sql_filter(), "1=1");
    }

    #[test]
    fn test_workspace_filter_sql_injection_escape() {
        let f = WorkspaceFilter::Single("it's".to_string());
        assert_eq!(f.to_sql_filter(), "workspace = 'it''s'");

        let f = WorkspaceFilter::Multiple(vec!["o'reilly".to_string()]);
        assert_eq!(f.to_sql_filter(), "workspace IN ('o''reilly')");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut ws = Workspace::new("test-ws", "Test Workspace");
        ws.description = Some("A test workspace".to_string());
        ws.icon = Some("🧪".to_string());
        ws.config.decay_rate = Some(0.5);
        ws.config.permanent_fact_types = vec![FactType::Preference, FactType::Personal];
        ws.config.allowed_tools = vec!["web_search".to_string()];

        let json = serde_json::to_string(&ws).unwrap();
        let deserialized: Workspace = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "test-ws");
        assert_eq!(deserialized.name, "Test Workspace");
        assert_eq!(deserialized.description.as_deref(), Some("A test workspace"));
        assert_eq!(deserialized.icon.as_deref(), Some("🧪"));
        assert_eq!(deserialized.config.decay_rate, Some(0.5));
        assert_eq!(deserialized.config.permanent_fact_types.len(), 2);
        assert_eq!(deserialized.config.allowed_tools, vec!["web_search"]);
        assert_eq!(deserialized.created_at, ws.created_at);
    }

    #[test]
    fn test_deserialization_with_missing_optional_fields() {
        let json = r#"{"id":"minimal","name":"Minimal","config":{},"is_default":false,"is_archived":false,"created_at":1000,"updated_at":1000}"#;
        let ws: Workspace = serde_json::from_str(json).unwrap();
        assert_eq!(ws.id, "minimal");
        assert!(ws.description.is_none());
        assert!(ws.icon.is_none());
        assert!(ws.config.decay_rate.is_none());
        assert!(ws.config.permanent_fact_types.is_empty());
        assert!(ws.config.allowed_tools.is_empty());
    }

    #[test]
    fn test_optional_fields_skip_serialization() {
        let ws = Workspace::new("clean", "Clean");
        let json = serde_json::to_string(&ws).unwrap();
        // Optional None fields should not appear in serialized output
        assert!(!json.contains("description"));
        assert!(!json.contains("icon"));
        assert!(!json.contains("decay_rate"));
        assert!(!json.contains("default_provider"));
    }
}
