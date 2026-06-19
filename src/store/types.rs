use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Skill,
    Plugin,
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCategory {
    Search,
    Developer,
    Data,
    Productivity,
    Writing,
    Communication,
    Knowledge,
    Files,
    Design,
    Automation,
    Finance,
    Utilities,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Official,
    Verified,
    Community,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
    Sse,
}

impl ExtensionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Plugin => "plugin",
            Self::Mcp => "mcp",
        }
    }
}

impl ExtensionCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Developer => "developer",
            Self::Data => "data",
            Self::Productivity => "productivity",
            Self::Writing => "writing",
            Self::Communication => "communication",
            Self::Knowledge => "knowledge",
            Self::Files => "files",
            Self::Design => "design",
            Self::Automation => "automation",
            Self::Finance => "finance",
            Self::Utilities => "utilities",
            Self::Other => "other",
        }
    }
}

impl TrustTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Verified => "verified",
            Self::Community => "community",
            Self::Unverified => "unverified",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&ExtensionKind::Mcp).unwrap(), "\"mcp\"");
        assert_eq!(ExtensionKind::Plugin.as_str(), "plugin");
    }

    #[test]
    fn category_roundtrips() {
        let c = ExtensionCategory::Developer;
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, "\"developer\"");
        assert_eq!(serde_json::from_str::<ExtensionCategory>(&s).unwrap(), c);
    }

    #[test]
    fn trust_tier_as_str() {
        assert_eq!(TrustTier::Unverified.as_str(), "unverified");
    }
}
