use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Skill,
    Plugin,
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

    /// Ordering rank, higher = more trusted. Used to clamp a wire-declared tier
    /// to the tier of the source that published it.
    const fn rank(self) -> u8 {
        match self {
            Self::Official => 3,
            Self::Verified => 2,
            Self::Community => 1,
            Self::Unverified => 0,
        }
    }

    /// The lower of two tiers. A catalog entry may not claim more trust than the
    /// source that served it: the wire is attacker-controlled if the hub is ever
    /// compromised or MITM'd, so `official` must be earned by the *source*, not
    /// self-declared per entry.
    #[must_use]
    pub fn clamped_to(self, ceiling: Self) -> Self {
        if self.rank() <= ceiling.rank() {
            self
        } else {
            ceiling
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnvDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Where the user obtains this value (a console / signup page). Surfaced in
    /// the install Configure step next to the field — without it the user is
    /// asked for a key with no clue where it comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how_to_get_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderDecl {
    pub name: String,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallSpec {
    McpStdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<EnvDecl>,
    },
    McpRemote {
        url: String,
        transport: McpTransport,
        #[serde(default)]
        headers: Vec<HeaderDecl>,
    },
    OciImage {
        image: String,
    },
    GitDir {
        git_url: String,
        subdir: Option<String>,
        git_ref: Option<String>,
        sha256: Option<String>,
    },
}

impl InstallSpec {
    /// True iff installing requires collecting user-supplied config/secrets.
    pub fn requires_config(&self) -> bool {
        match self {
            Self::McpStdio { env, .. } => env.iter().any(|e| e.required),
            Self::McpRemote { headers, .. } => headers.iter().any(|h| h.secret),
            Self::OciImage { .. } | Self::GitDir { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionEntry {
    pub id: String,
    pub kind: ExtensionKind,
    pub category: ExtensionCategory,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    pub trust_tier: TrustTier,
    #[serde(default)]
    pub requires_config: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub update_available: bool,
    /// Upstream provenance label (e.g. "clawhub", "github:owner"); filled from
    /// the published catalog. None for local/installed entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Resolved install spec carried by the catalog entry; None for local
    /// entries. Install resolution is a pure cache lookup of this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_spec: Option<InstallSpec>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ExtensionKind::Mcp).unwrap(),
            "\"mcp\""
        );
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

    #[test]
    fn trust_tier_clamps_to_source_ceiling() {
        // A wire entry cannot out-rank its source.
        assert_eq!(
            TrustTier::Official.clamped_to(TrustTier::Verified),
            TrustTier::Verified
        );
        // At or below the ceiling passes through untouched.
        assert_eq!(
            TrustTier::Community.clamped_to(TrustTier::Verified),
            TrustTier::Community
        );
        assert_eq!(
            TrustTier::Verified.clamped_to(TrustTier::Verified),
            TrustTier::Verified
        );
        // An Official source may publish Official entries.
        assert_eq!(
            TrustTier::Official.clamped_to(TrustTier::Official),
            TrustTier::Official
        );
    }

    #[test]
    fn install_spec_tagged_json() {
        let spec = InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            env: vec![EnvDecl {
                name: "GITHUB_TOKEN".into(),
                required: true,
                secret: true,
                ..Default::default()
            }],
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["type"], "mcp_stdio");
        assert_eq!(v["command"], "npx");
        assert!(spec.requires_config());
    }

    #[test]
    fn oci_image_needs_no_config() {
        let spec = InstallSpec::OciImage {
            image: "mcp/foo@sha256:abc".into(),
        };
        assert!(!spec.requires_config());
    }

    fn sample_entry() -> ExtensionEntry {
        ExtensionEntry {
            id: "mcp-official:io.github.acme/foo".into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Developer,
            name: "Foo".into(),
            description: "Does foo.".into(),
            author: Some("acme".into()),
            icon: None,
            tags: vec!["mcp".into(), "developer".into()],
            version: Some("1.0.0".into()),
            source_id: "mcp-official".into(),
            repo_url: Some("https://github.com/acme/foo".into()),
            trust_tier: TrustTier::Community,
            requires_config: true,
            config_schema: Some(serde_json::json!({"type":"object"})),
            installed: false,
            enabled: false,
            update_available: false,
            via: None,
            install_spec: None,
        }
    }

    #[test]
    fn entry_carries_via_and_install_spec() {
        let mut e = sample_entry();
        e.via = Some("aleph-hub".into());
        e.install_spec = Some(InstallSpec::OciImage {
            image: "x@sha256:abc".into(),
        });
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["via"], "aleph-hub");
        assert!(j["install_spec"].is_object());
        let back: ExtensionEntry = serde_json::from_value(j).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn entry_roundtrips_through_json() {
        let e = sample_entry();
        let json = serde_json::to_string(&e).unwrap();
        let back: ExtensionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
        assert_eq!(back.category, ExtensionCategory::Developer);
    }
}
