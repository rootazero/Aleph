use std::fmt;

/// Identifies a component (skill, agent, tool, command) with optional plugin namespace.
///
/// Plugin components use `plugin-name:component-name` format.
/// Built-in components use simple names without a namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ComponentId {
    pub plugin: Option<String>,
    pub name: String,
}

impl ComponentId {
    /// Create a namespaced component ID for a plugin component.
    pub fn plugin(plugin_name: &str, component_name: &str) -> Self {
        Self {
            plugin: Some(plugin_name.to_string()),
            name: component_name.to_string(),
        }
    }

    /// Create a simple component ID for a built-in component.
    pub fn builtin(name: &str) -> Self {
        Self {
            plugin: None,
            name: name.to_string(),
        }
    }

    /// Parse a component ID from a string.
    ///
    /// "plugin:component" → plugin=Some("plugin"), name="component"
    /// "component"        → plugin=None, name="component"
    pub fn parse(s: &str) -> Self {
        match s.splitn(2, ':').collect::<Vec<_>>().as_slice() {
            [plugin, name] => Self {
                plugin: Some((*plugin).to_string()),
                name: (*name).to_string(),
            },
            _ => Self {
                plugin: None,
                name: s.to_string(),
            },
        }
    }

    /// Return the qualified name (same as Display output).
    pub fn qualified_name(&self) -> String {
        self.to_string()
    }

    /// Return true if this component belongs to a plugin.
    pub fn is_plugin(&self) -> bool {
        self.plugin.is_some()
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.plugin {
            Some(plugin) => write!(f, "{}:{}", plugin, self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

impl From<&str> for ComponentId {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_id_display() {
        let id = ComponentId::plugin("diagnostics", "health-check");
        assert_eq!(id.to_string(), "diagnostics:health-check");
    }

    #[test]
    fn test_component_id_builtin() {
        let id = ComponentId::builtin("web_search");
        assert_eq!(id.to_string(), "web_search");
    }

    #[test]
    fn test_component_id_parse_namespaced() {
        let id = ComponentId::parse("diagnostics:health-check");
        assert_eq!(id.plugin, Some("diagnostics".to_string()));
        assert_eq!(id.name, "health-check");
    }

    #[test]
    fn test_component_id_parse_simple() {
        let id = ComponentId::parse("web_search");
        assert_eq!(id.plugin, None);
        assert_eq!(id.name, "web_search");
    }

    #[test]
    fn test_component_id_is_plugin() {
        let plugin_id = ComponentId::plugin("diagnostics", "health-check");
        let builtin_id = ComponentId::builtin("web_search");
        assert!(plugin_id.is_plugin());
        assert!(!builtin_id.is_plugin());
    }
}
