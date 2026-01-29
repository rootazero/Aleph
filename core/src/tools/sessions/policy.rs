//! Agent-to-Agent communication policy.
//!
//! Controls which agents can communicate with each other.

/// Agent-to-Agent policy configuration
#[derive(Debug, Clone, Default)]
pub struct AgentToAgentPolicy {
    /// Whether A2A communication is enabled
    pub enabled: bool,
    /// Allow rules
    rules: Vec<A2ARule>,
}

/// A2A allow rule
#[derive(Debug, Clone)]
pub struct A2ARule {
    pub from: RuleMatcher,
    pub to: RuleMatcher,
}

/// Rule matcher for agent IDs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleMatcher {
    /// Matches any agent ("*")
    Any,
    /// Matches specific agent ID
    Specific(String),
}

impl AgentToAgentPolicy {
    /// Create a new policy with rules
    pub fn new(enabled: bool, rules: Vec<A2ARule>) -> Self {
        Self { enabled, rules }
    }

    /// Create from config allow list
    pub fn from_allow_list(enabled: bool, allows: &[String]) -> Self {
        let rules = allows.iter().filter_map(|s| A2ARule::parse(s)).collect();
        Self { enabled, rules }
    }

    /// Check if communication from one agent to another is allowed
    pub fn is_allowed(&self, from_agent: &str, to_agent: &str) -> bool {
        // Disabled = no A2A communication
        if !self.enabled {
            return false;
        }

        // Same agent always allowed
        if from_agent.eq_ignore_ascii_case(to_agent) {
            return true;
        }

        // Check rules
        self.rules
            .iter()
            .any(|rule| rule.from.matches(from_agent) && rule.to.matches(to_agent))
    }

    /// Add a rule
    pub fn add_rule(&mut self, rule: A2ARule) {
        self.rules.push(rule);
    }
}

impl A2ARule {
    /// Create a new rule
    pub fn new(from: RuleMatcher, to: RuleMatcher) -> Self {
        Self { from, to }
    }

    /// Parse a rule from string format: "from -> to" or "*"
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();

        // "*" means any to any
        if s == "*" {
            return Some(Self {
                from: RuleMatcher::Any,
                to: RuleMatcher::Any,
            });
        }

        // "from -> to" format
        let parts: Vec<&str> = s.split("->").map(|p| p.trim()).collect();
        match parts.as_slice() {
            [from, to] => Some(Self {
                from: RuleMatcher::parse(from),
                to: RuleMatcher::parse(to),
            }),
            _ => None,
        }
    }
}

impl RuleMatcher {
    /// Parse from string: "*" or specific agent ID
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s == "*" {
            Self::Any
        } else {
            Self::Specific(s.to_lowercase())
        }
    }

    /// Check if this matcher matches the given agent ID
    pub fn matches(&self, agent_id: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Specific(id) => id.eq_ignore_ascii_case(agent_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_disabled() {
        let policy = AgentToAgentPolicy::new(false, vec![]);
        assert!(!policy.is_allowed("main", "work"));
    }

    #[test]
    fn test_policy_same_agent_always_allowed() {
        let policy = AgentToAgentPolicy::new(true, vec![]);
        assert!(policy.is_allowed("main", "main"));
        assert!(policy.is_allowed("MAIN", "main")); // case insensitive
    }

    #[test]
    fn test_policy_no_rules_denies_cross_agent() {
        let policy = AgentToAgentPolicy::new(true, vec![]);
        assert!(!policy.is_allowed("main", "work"));
    }

    #[test]
    fn test_policy_wildcard_rule() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["*".to_string()]);
        assert!(policy.is_allowed("main", "work"));
        assert!(policy.is_allowed("work", "main"));
    }

    #[test]
    fn test_policy_specific_rule() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["main -> work".to_string()]);
        assert!(policy.is_allowed("main", "work"));
        assert!(!policy.is_allowed("work", "main")); // reverse not allowed
    }

    #[test]
    fn test_policy_wildcard_from() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["* -> monitor".to_string()]);
        assert!(policy.is_allowed("main", "monitor"));
        assert!(policy.is_allowed("work", "monitor"));
        assert!(!policy.is_allowed("main", "work"));
    }

    #[test]
    fn test_policy_wildcard_to() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["main -> *".to_string()]);
        assert!(policy.is_allowed("main", "work"));
        assert!(policy.is_allowed("main", "monitor"));
        assert!(!policy.is_allowed("work", "main"));
    }

    #[test]
    fn test_rule_parse_invalid() {
        assert!(A2ARule::parse("").is_none());
        assert!(A2ARule::parse("invalid").is_none());
        assert!(A2ARule::parse("a -> b -> c").is_none());
    }

    #[test]
    fn test_rule_matcher() {
        assert!(RuleMatcher::Any.matches("anything"));
        assert!(RuleMatcher::Specific("main".to_string()).matches("main"));
        assert!(RuleMatcher::Specific("main".to_string()).matches("MAIN"));
        assert!(!RuleMatcher::Specific("main".to_string()).matches("work"));
    }
}
