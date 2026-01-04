/// Capability enumeration - Agent capability types
///
/// Executed in fixed order: Memory → Search → MCP
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Memory retrieval (local vector database)
    Memory = 0,

    /// Web search (Google/Bing API)
    Search = 1,

    /// MCP tool/resource calls
    Mcp = 2,
}

impl Capability {
    /// Parse from string (for config files)
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "memory" => Ok(Capability::Memory),
            "search" => Ok(Capability::Search),
            "mcp" => Ok(Capability::Mcp),
            _ => Err(format!("Unknown capability: {}", s)),
        }
    }

    /// Convert to string (for logging/config)
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Memory => "memory",
            Capability::Search => "search",
            Capability::Mcp => "mcp",
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
        assert_eq!(Capability::parse("SEARCH").unwrap(), Capability::Search);
        assert_eq!(Capability::parse("Mcp").unwrap(), Capability::Mcp);
        assert!(Capability::parse("invalid").is_err());
    }

    #[test]
    fn test_capability_as_str() {
        assert_eq!(Capability::Memory.as_str(), "memory");
        assert_eq!(Capability::Search.as_str(), "search");
        assert_eq!(Capability::Mcp.as_str(), "mcp");
    }

    #[test]
    fn test_capability_sort() {
        let caps = vec![Capability::Mcp, Capability::Memory, Capability::Search];
        let sorted = Capability::sort_by_priority(caps);
        assert_eq!(
            sorted,
            vec![Capability::Memory, Capability::Search, Capability::Mcp]
        );
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(Capability::Memory.to_string(), "memory");
        assert_eq!(Capability::Search.to_string(), "search");
        assert_eq!(Capability::Mcp.to_string(), "mcp");
    }

    #[test]
    fn test_capability_ord() {
        assert!(Capability::Memory < Capability::Search);
        assert!(Capability::Search < Capability::Mcp);
        assert!(Capability::Memory < Capability::Mcp);
    }
}
