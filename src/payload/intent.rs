/// Intent enumeration - User intent types
///
/// Distinguishes between "hard logic features", "Skills workflows", and "Prompt transformations"
use crate::config::RoutingRuleConfig;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Intent {
    /// Built-in feature: Web search
    /// Commands: /search, /google, /web
    BuiltinSearch,

    /// Built-in feature: MCP tool calls
    /// Commands: /mcp, /tool
    BuiltinMcp,

    /// 🔮 Skills workflow (reserved for Solution C)
    ///
    /// Claude Code Skills complex workflow (multi-step + MCP Tools + knowledge base)
    ///
    /// **This implementation**: Only enum definition, no execution logic
    /// **Solution C**: Implement WorkflowEngine and SkillsRegistry
    ///
    /// Parameter: skill_id (e.g., "build-macos-apps", "pdf", "mcp-builder")
    ///
    /// # Distinction
    ///
    /// - `Intent::Custom("translation")` - Simple Prompt transformation
    /// - `Intent::Skills("build-macos-apps")` - Complex multi-step workflow + tool calls
    Skills(String),

    /// Custom command (Prompt transformation)
    /// Parameter: intent name (e.g., "translation", "research", "code")
    Custom(String),

    /// Default conversation (no special command)
    GeneralChat,
}

impl Intent {
    /// Infer Intent from RoutingRuleConfig
    pub fn from_rule(rule: &RoutingRuleConfig) -> Self {
        if let Some(intent_type) = &rule.intent_type {
            match intent_type.as_str() {
                "search" | "web_search" => Intent::BuiltinSearch,
                "mcp" | "tool_call" => Intent::BuiltinMcp,
                "general" => Intent::GeneralChat,
                // 🔮 Skills format: "skills:xxx"
                s if s.starts_with("skills:") => {
                    let skill_id = s.strip_prefix("skills:").unwrap_or("");
                    Intent::Skills(skill_id.to_string())
                }
                custom => Intent::Custom(custom.to_string()),
            }
        } else {
            Intent::GeneralChat
        }
    }

    /// Check if this is a built-in feature (requires special handling)
    pub fn is_builtin(&self) -> bool {
        matches!(self, Intent::BuiltinSearch | Intent::BuiltinMcp)
    }

    /// 🔮 Check if this is a Skills workflow (reserved for Solution C)
    pub fn is_skills(&self) -> bool {
        matches!(self, Intent::Skills(_))
    }

    /// 🔮 Get Skill ID (reserved for Solution C)
    ///
    /// # Returns
    ///
    /// - `Some(skill_id)` if Intent::Skills
    /// - `None` otherwise
    pub fn skills_id(&self) -> Option<&str> {
        match self {
            Intent::Skills(id) => Some(id.as_str()),
            _ => None,
        }
    }
}

impl std::fmt::Display for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Intent::BuiltinSearch => write!(f, "builtin_search"),
            Intent::BuiltinMcp => write!(f, "builtin_mcp"),
            Intent::Skills(id) => write!(f, "skills:{}", id),
            Intent::Custom(name) => write!(f, "custom:{}", name),
            Intent::GeneralChat => write!(f, "general_chat"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_from_rule_search() {
        let mut rule = RoutingRuleConfig::command("^/search", "openai", None);
        rule.intent_type = Some("search".to_string());

        assert_eq!(Intent::from_rule(&rule), Intent::BuiltinSearch);
    }

    #[test]
    fn test_intent_from_rule_custom() {
        let mut rule = RoutingRuleConfig::command("^/translate", "openai", None);
        rule.intent_type = Some("translation".to_string());

        assert_eq!(
            Intent::from_rule(&rule),
            Intent::Custom("translation".to_string())
        );
    }

    #[test]
    fn test_intent_from_rule_skills() {
        let mut rule = RoutingRuleConfig::command("^/build-ios", "claude", None);
        rule.intent_type = Some("skills:build-macos-apps".to_string());

        let intent = Intent::from_rule(&rule);
        assert!(intent.is_skills());
        assert_eq!(intent.skills_id(), Some("build-macos-apps"));
    }

    #[test]
    fn test_intent_is_builtin() {
        assert!(Intent::BuiltinSearch.is_builtin());
        assert!(Intent::BuiltinMcp.is_builtin());
        assert!(!Intent::GeneralChat.is_builtin());
        assert!(!Intent::Custom("test".to_string()).is_builtin());
    }

    #[test]
    fn test_intent_display() {
        assert_eq!(Intent::BuiltinSearch.to_string(), "builtin_search");
        assert_eq!(Intent::GeneralChat.to_string(), "general_chat");
        assert_eq!(
            Intent::Custom("translation".to_string()).to_string(),
            "custom:translation"
        );
        assert_eq!(Intent::Skills("pdf".to_string()).to_string(), "skills:pdf");
    }
}
