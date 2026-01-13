/// Capability enumeration - Agent capability types
///
/// Simplified capabilities for AI-first architecture:
/// - Memory: RAG context enrichment
/// - Mcp: AI tool access (includes all native tools)
/// - Skills: Dynamic instruction injection
///
/// Executed in fixed order: Memory → Mcp → Skills
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Memory retrieval (local vector database for RAG)
    Memory = 0,

    /// MCP tool/resource calls (AI decides which tools to use)
    Mcp = 1,

    /// Skills - dynamic instruction injection (Claude Agent Skills standard)
    Skills = 2,
}

impl Capability {
    /// Parse from string (for config files)
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "memory" => Ok(Capability::Memory),
            "mcp" => Ok(Capability::Mcp),
            "skills" => Ok(Capability::Skills),
            _ => Err(format!("Unknown capability: {}", s)),
        }
    }

    /// Convert to string (for logging/config)
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Memory => "memory",
            Capability::Mcp => "mcp",
            Capability::Skills => "skills",
        }
    }

    /// Sort capabilities by priority
    pub fn sort_by_priority(caps: Vec<Capability>) -> Vec<Capability> {
        let mut sorted = caps;
        sorted.sort(); // Uses PartialOrd (0 < 1 < 2)
        sorted
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_parse() {
        assert_eq!(Capability::parse("memory").unwrap(), Capability::Memory);
        assert_eq!(Capability::parse("MEMORY").unwrap(), Capability::Memory);
        assert_eq!(Capability::parse("mcp").unwrap(), Capability::Mcp);
        assert_eq!(Capability::parse("Mcp").unwrap(), Capability::Mcp);
        assert_eq!(Capability::parse("skills").unwrap(), Capability::Skills);
        assert_eq!(Capability::parse("SKILLS").unwrap(), Capability::Skills);
        assert!(Capability::parse("invalid").is_err());
        // Legacy capabilities should fail
        assert!(Capability::parse("search").is_err());
        assert!(Capability::parse("video").is_err());
        assert!(Capability::parse("webfetch").is_err());
    }

    #[test]
    fn test_capability_as_str() {
        assert_eq!(Capability::Memory.as_str(), "memory");
        assert_eq!(Capability::Mcp.as_str(), "mcp");
        assert_eq!(Capability::Skills.as_str(), "skills");
    }

    #[test]
    fn test_capability_sort() {
        let caps = vec![Capability::Skills, Capability::Memory, Capability::Mcp];
        let sorted = Capability::sort_by_priority(caps);
        assert_eq!(
            sorted,
            vec![Capability::Memory, Capability::Mcp, Capability::Skills]
        );
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(Capability::Memory.to_string(), "memory");
        assert_eq!(Capability::Mcp.to_string(), "mcp");
        assert_eq!(Capability::Skills.to_string(), "skills");
    }

    #[test]
    fn test_capability_ord() {
        assert!(Capability::Memory < Capability::Mcp);
        assert!(Capability::Mcp < Capability::Skills);
        assert!(Capability::Memory < Capability::Skills);
    }
}
