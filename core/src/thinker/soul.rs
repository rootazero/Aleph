//! Soul - Identity and personality system for AI embodiment
//!
//! This module defines the SoulManifest and related types that describe
//! the AI's personality, voice, and behavioral characteristics. It upgrades
//! the simple `persona: String` to a comprehensive identity system.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      SoulManifest                           │
//! │  ┌─────────────────┐  ┌──────────────┐  ┌───────────────┐  │
//! │  │    identity     │  │  SoulVoice   │  │  directives   │  │
//! │  │                 │  │              │  │               │  │
//! │  │ First-person    │  │ • tone       │  │ • positive    │  │
//! │  │ declaration     │  │ • verbosity  │  │   guidance    │  │
//! │  │ of who I am     │  │ • formatting │  │ • expertise   │  │
//! │  │                 │  │ • lang notes │  │ • anti-patt.  │  │
//! │  └─────────────────┘  └──────────────┘  └───────────────┘  │
//! │  ┌─────────────────────────────────────────────────────┐   │
//! │  │ RelationshipMode: Peer | Mentor | Assistant | ...   │   │
//! │  └─────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Response verbosity preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// Brief, to-the-point responses
    Concise,
    /// Standard balanced responses
    #[default]
    Balanced,
    /// Detailed, comprehensive responses
    Elaborate,
}

/// Formatting style preference for responses
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FormattingStyle {
    /// Plain text with minimal formatting
    Minimal,
    /// Standard markdown formatting
    #[default]
    Markdown,
    /// Rich formatting with full feature usage
    Rich,
}

/// Relationship mode defining how AI relates to the user
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipMode {
    /// Collaborative peer relationship
    Peer,
    /// Guiding mentor relationship
    Mentor,
    /// Helpful assistant relationship
    #[default]
    Assistant,
    /// Domain expert consultation
    Expert,
    /// Custom relationship with description
    Custom(String),
}

impl RelationshipMode {
    /// Get description for prompt injection
    pub fn description(&self) -> &str {
        match self {
            Self::Peer => "We collaborate as equals, sharing ideas and working toward shared goals.",
            Self::Mentor => "I guide and teach, helping you grow while solving problems together.",
            Self::Assistant => "I help you accomplish your goals efficiently and effectively.",
            Self::Expert => "I provide expert consultation and professional advice in my domains.",
            Self::Custom(desc) => desc,
        }
    }
}

/// Voice and communication style configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoulVoice {
    /// Communication tone (formal, casual, playful, technical, etc.)
    #[serde(default)]
    pub tone: String,

    /// Response verbosity preference
    #[serde(default)]
    pub verbosity: Verbosity,

    /// Formatting preferences
    #[serde(default)]
    pub formatting_style: FormattingStyle,

    /// Language style notes (e.g., "use British English")
    #[serde(default)]
    pub language_notes: Option<String>,
}

/// Complete soul definition for AI personality
///
/// SoulManifest encapsulates the full identity of an AI persona, including:
/// - Core identity declaration (first-person)
/// - Voice and communication style
/// - Behavioral directives (what to do)
/// - Anti-patterns (what to avoid)
/// - Relationship mode with the user
/// - Domain expertise areas
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoulManifest {
    /// Core identity declaration (first-person, who I am)
    #[serde(default)]
    pub identity: String,

    /// Voice and communication style
    #[serde(default)]
    pub voice: SoulVoice,

    /// Behavioral directives (positive guidance)
    #[serde(default)]
    pub directives: Vec<String>,

    /// Anti-patterns (what I never do)
    #[serde(default)]
    pub anti_patterns: Vec<String>,

    /// Relationship mode with user
    #[serde(default)]
    pub relationship: RelationshipMode,

    /// Optional: specialized knowledge domains
    #[serde(default)]
    pub expertise: Vec<String>,

    /// Optional: custom prompt addendum (raw text)
    #[serde(default)]
    pub addendum: Option<String>,
}

/// Error type for soul loading
#[derive(Debug)]
pub enum SoulLoadError {
    /// Soul file not found at path
    NotFound(PathBuf),
    /// IO error reading file
    Io(std::io::Error),
    /// Parse error in file content
    Parse(String),
    /// Unsupported file format
    UnsupportedFormat(String),
}

impl std::fmt::Display for SoulLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "Soul file not found: {}", p.display()),
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Parse(e) => write!(f, "Parse error: {}", e),
            Self::UnsupportedFormat(ext) => write!(f, "Unsupported format: {}", ext),
        }
    }
}

impl std::error::Error for SoulLoadError {}

impl SoulManifest {
    /// Create a new empty soul manifest
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this is an empty/default soul
    pub fn is_empty(&self) -> bool {
        self.identity.is_empty() && self.directives.is_empty()
    }

    /// Load soul manifest from a file path
    ///
    /// Supports:
    /// - JSON files (.json)
    /// - TOML files (.toml)
    /// - Markdown files (.md, .markdown) with YAML frontmatter
    pub fn from_file(path: &Path) -> Result<Self, SoulLoadError> {
        use std::fs;

        if !path.exists() {
            return Err(SoulLoadError::NotFound(path.to_path_buf()));
        }

        let content = fs::read_to_string(path).map_err(SoulLoadError::Io)?;

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match ext {
            "json" => {
                serde_json::from_str(&content).map_err(|e| SoulLoadError::Parse(e.to_string()))
            }
            "toml" => toml::from_str(&content).map_err(|e| SoulLoadError::Parse(e.to_string())),
            "md" | "markdown" => Self::from_markdown(&content),
            _ => Err(SoulLoadError::UnsupportedFormat(ext.to_string())),
        }
    }

    /// Parse soul from markdown content with YAML frontmatter
    pub fn from_markdown(content: &str) -> Result<Self, SoulLoadError> {
        let (frontmatter, body) = Self::split_frontmatter(content)?;

        // Start with frontmatter values
        let mut manifest: SoulManifest = if !frontmatter.is_empty() {
            serde_yaml::from_str(&frontmatter)
                .map_err(|e| SoulLoadError::Parse(format!("YAML frontmatter error: {}", e)))?
        } else {
            SoulManifest::default()
        };

        // Parse markdown body sections
        Self::parse_markdown_body(&mut manifest, &body)?;

        Ok(manifest)
    }

    /// Split content into YAML frontmatter and markdown body
    fn split_frontmatter(content: &str) -> Result<(String, String), SoulLoadError> {
        let trimmed = content.trim_start();

        if !trimmed.starts_with("---") {
            // No frontmatter, entire content is body
            return Ok((String::new(), content.to_string()));
        }

        // Find the closing ---
        let after_first = &trimmed[3..];
        if let Some(end_pos) = after_first.find("\n---") {
            let frontmatter = after_first[..end_pos].trim().to_string();
            let body = after_first[end_pos + 4..].trim_start().to_string();
            Ok((frontmatter, body))
        } else {
            Err(SoulLoadError::Parse(
                "Unclosed YAML frontmatter".to_string(),
            ))
        }
    }

    /// Parse markdown body sections into manifest
    fn parse_markdown_body(manifest: &mut SoulManifest, body: &str) -> Result<(), SoulLoadError> {
        let mut current_section: Option<String> = None;
        let mut current_content = String::new();

        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                // New section - save previous
                if let Some(ref section) = current_section {
                    Self::apply_section(manifest, section, &current_content);
                }
                current_section = Some(rest.trim().to_lowercase());
                current_content.clear();
            } else if let Some(rest) = line.strip_prefix("# ") {
                // Check if this is a section header (e.g., "# Identity")
                // vs a title line (e.g., "# Soul: Aleph")
                let header = rest.trim().to_lowercase();
                if Self::is_known_section(&header) {
                    // Treat as section header
                    if let Some(ref section) = current_section {
                        Self::apply_section(manifest, section, &current_content);
                    }
                    current_section = Some(header);
                    current_content.clear();
                }
                // Otherwise skip title lines
            } else {
                current_content.push_str(line);
                current_content.push('\n');
            }
        }

        // Apply final section
        if let Some(ref section) = current_section {
            Self::apply_section(manifest, section, &current_content);
        }

        Ok(())
    }

    /// Check if a header name is a known section
    fn is_known_section(name: &str) -> bool {
        matches!(
            name,
            "identity"
                | "directives"
                | "anti-patterns"
                | "antipatterns"
                | "anti patterns"
                | "addendum"
                | "additional context"
                | "context"
                | "communication style"
                | "voice"
                | "expertise"
        )
    }

    /// Apply parsed section content to manifest
    fn apply_section(manifest: &mut SoulManifest, section: &str, content: &str) {
        let content = content.trim();

        match section {
            "identity" => {
                if manifest.identity.is_empty() {
                    manifest.identity = content.to_string();
                }
            }
            "directives" => {
                if manifest.directives.is_empty() {
                    manifest.directives = Self::parse_list_items(content);
                }
            }
            "anti-patterns" | "antipatterns" | "anti patterns" => {
                if manifest.anti_patterns.is_empty() {
                    manifest.anti_patterns = Self::parse_list_items(content);
                }
            }
            "addendum" | "additional context" | "context" => {
                if manifest.addendum.is_none() {
                    manifest.addendum = Some(content.to_string());
                }
            }
            _ => {
                // Unknown section - ignore
            }
        }
    }

    /// Parse markdown list items (lines starting with - or *)
    fn parse_list_items(content: &str) -> Vec<String> {
        content
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
                    Some(rest.trim().to_string())
                } else if !trimmed.is_empty() {
                    // Continuation of previous item or standalone text
                    Some(trimmed.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Merge with another soul manifest (self takes priority)
    ///
    /// For each field:
    /// - If self has a non-empty/non-default value, use it
    /// - Otherwise, fall back to base's value
    pub fn merge_with(&self, base: &SoulManifest) -> SoulManifest {
        SoulManifest {
            identity: if self.identity.is_empty() {
                base.identity.clone()
            } else {
                self.identity.clone()
            },
            voice: SoulVoice {
                tone: if self.voice.tone.is_empty() {
                    base.voice.tone.clone()
                } else {
                    self.voice.tone.clone()
                },
                verbosity: self.voice.verbosity,
                formatting_style: self.voice.formatting_style,
                language_notes: self
                    .voice
                    .language_notes
                    .clone()
                    .or_else(|| base.voice.language_notes.clone()),
            },
            directives: if self.directives.is_empty() {
                base.directives.clone()
            } else {
                self.directives.clone()
            },
            anti_patterns: if self.anti_patterns.is_empty() {
                base.anti_patterns.clone()
            } else {
                self.anti_patterns.clone()
            },
            relationship: if self.relationship == RelationshipMode::default() {
                base.relationship.clone()
            } else {
                self.relationship.clone()
            },
            expertise: if self.expertise.is_empty() {
                base.expertise.clone()
            } else {
                self.expertise.clone()
            },
            addendum: self.addendum.clone().or_else(|| base.addendum.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soul_manifest_default() {
        let soul = SoulManifest::default();

        assert!(soul.identity.is_empty());
        assert!(soul.directives.is_empty());
        assert!(soul.anti_patterns.is_empty());
        assert!(soul.expertise.is_empty());
        assert!(soul.addendum.is_none());
        assert_eq!(soul.relationship, RelationshipMode::Assistant);
        assert!(soul.is_empty());
    }

    #[test]
    fn test_soul_voice_default() {
        let voice = SoulVoice::default();

        assert!(voice.tone.is_empty());
        assert_eq!(voice.verbosity, Verbosity::Balanced);
        assert_eq!(voice.formatting_style, FormattingStyle::Markdown);
        assert!(voice.language_notes.is_none());
    }

    #[test]
    fn test_verbosity_serde() {
        // Test serialization
        let concise = serde_json::to_string(&Verbosity::Concise).unwrap();
        let balanced = serde_json::to_string(&Verbosity::Balanced).unwrap();
        let elaborate = serde_json::to_string(&Verbosity::Elaborate).unwrap();

        assert_eq!(concise, "\"concise\"");
        assert_eq!(balanced, "\"balanced\"");
        assert_eq!(elaborate, "\"elaborate\"");

        // Test deserialization
        let parsed: Verbosity = serde_json::from_str("\"concise\"").unwrap();
        assert_eq!(parsed, Verbosity::Concise);

        let parsed: Verbosity = serde_json::from_str("\"balanced\"").unwrap();
        assert_eq!(parsed, Verbosity::Balanced);

        let parsed: Verbosity = serde_json::from_str("\"elaborate\"").unwrap();
        assert_eq!(parsed, Verbosity::Elaborate);
    }

    #[test]
    fn test_formatting_style_serde() {
        // Test serialization
        let minimal = serde_json::to_string(&FormattingStyle::Minimal).unwrap();
        let markdown = serde_json::to_string(&FormattingStyle::Markdown).unwrap();
        let rich = serde_json::to_string(&FormattingStyle::Rich).unwrap();

        assert_eq!(minimal, "\"minimal\"");
        assert_eq!(markdown, "\"markdown\"");
        assert_eq!(rich, "\"rich\"");

        // Test deserialization
        let parsed: FormattingStyle = serde_json::from_str("\"minimal\"").unwrap();
        assert_eq!(parsed, FormattingStyle::Minimal);

        let parsed: FormattingStyle = serde_json::from_str("\"markdown\"").unwrap();
        assert_eq!(parsed, FormattingStyle::Markdown);

        let parsed: FormattingStyle = serde_json::from_str("\"rich\"").unwrap();
        assert_eq!(parsed, FormattingStyle::Rich);
    }

    #[test]
    fn test_relationship_mode_serde() {
        // Test serialization of simple variants
        let peer = serde_json::to_string(&RelationshipMode::Peer).unwrap();
        let mentor = serde_json::to_string(&RelationshipMode::Mentor).unwrap();
        let assistant = serde_json::to_string(&RelationshipMode::Assistant).unwrap();
        let expert = serde_json::to_string(&RelationshipMode::Expert).unwrap();

        assert_eq!(peer, "\"peer\"");
        assert_eq!(mentor, "\"mentor\"");
        assert_eq!(assistant, "\"assistant\"");
        assert_eq!(expert, "\"expert\"");

        // Test deserialization
        let parsed: RelationshipMode = serde_json::from_str("\"peer\"").unwrap();
        assert_eq!(parsed, RelationshipMode::Peer);

        let parsed: RelationshipMode = serde_json::from_str("\"mentor\"").unwrap();
        assert_eq!(parsed, RelationshipMode::Mentor);

        // Test Custom variant
        let custom = RelationshipMode::Custom("Custom relationship".to_string());
        let serialized = serde_json::to_string(&custom).unwrap();
        let parsed: RelationshipMode = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed, custom);
    }

    #[test]
    fn test_relationship_description() {
        assert!(RelationshipMode::Peer.description().contains("equals"));
        assert!(RelationshipMode::Mentor.description().contains("guide"));
        assert!(RelationshipMode::Assistant.description().contains("goals"));
        assert!(RelationshipMode::Expert.description().contains("expert"));

        let custom = RelationshipMode::Custom("My custom relationship".to_string());
        assert_eq!(custom.description(), "My custom relationship");
    }

    #[test]
    fn test_soul_merge_identity_override() {
        let base = SoulManifest {
            identity: "I am the base soul".to_string(),
            voice: SoulVoice {
                tone: "formal".to_string(),
                ..Default::default()
            },
            directives: vec!["Be helpful".to_string()],
            relationship: RelationshipMode::Mentor,
            ..Default::default()
        };

        let overlay = SoulManifest {
            identity: "I am the overlay soul".to_string(),
            voice: SoulVoice {
                tone: "casual".to_string(),
                ..Default::default()
            },
            relationship: RelationshipMode::Peer,
            ..Default::default()
        };

        let merged = overlay.merge_with(&base);

        // Overlay takes priority
        assert_eq!(merged.identity, "I am the overlay soul");
        assert_eq!(merged.voice.tone, "casual");
        assert_eq!(merged.relationship, RelationshipMode::Peer);

        // Overlay directives are empty, so base is used
        assert_eq!(merged.directives, vec!["Be helpful".to_string()]);
    }

    #[test]
    fn test_soul_merge_fallback() {
        let base = SoulManifest {
            identity: "I am the base soul".to_string(),
            voice: SoulVoice {
                tone: "formal".to_string(),
                verbosity: Verbosity::Elaborate,
                formatting_style: FormattingStyle::Rich,
                language_notes: Some("Use British English".to_string()),
            },
            directives: vec!["Be helpful".to_string(), "Be precise".to_string()],
            anti_patterns: vec!["Never be rude".to_string()],
            relationship: RelationshipMode::Expert,
            expertise: vec!["Rust".to_string(), "Python".to_string()],
            addendum: Some("Additional instructions".to_string()),
        };

        let empty_overlay = SoulManifest::default();

        let merged = empty_overlay.merge_with(&base);

        // All values should fall back to base
        assert_eq!(merged.identity, "I am the base soul");
        assert_eq!(merged.voice.tone, "formal");
        assert_eq!(
            merged.voice.language_notes,
            Some("Use British English".to_string())
        );
        assert_eq!(merged.directives.len(), 2);
        assert_eq!(merged.anti_patterns, vec!["Never be rude".to_string()]);
        assert_eq!(merged.relationship, RelationshipMode::Expert);
        assert_eq!(merged.expertise.len(), 2);
        assert_eq!(merged.addendum, Some("Additional instructions".to_string()));
    }

    #[test]
    fn test_serde_roundtrip() {
        let soul = SoulManifest {
            identity: "I am a helpful AI assistant".to_string(),
            voice: SoulVoice {
                tone: "friendly and professional".to_string(),
                verbosity: Verbosity::Balanced,
                formatting_style: FormattingStyle::Markdown,
                language_notes: Some("Use clear, simple language".to_string()),
            },
            directives: vec![
                "Always explain your reasoning".to_string(),
                "Provide examples when helpful".to_string(),
            ],
            anti_patterns: vec![
                "Never make up information".to_string(),
                "Avoid jargon without explanation".to_string(),
            ],
            relationship: RelationshipMode::Mentor,
            expertise: vec!["Programming".to_string(), "Writing".to_string()],
            addendum: Some("Remember to be patient with beginners".to_string()),
        };

        let json = serde_json::to_string(&soul).expect("serialize");
        let deserialized: SoulManifest = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.identity, soul.identity);
        assert_eq!(deserialized.voice.tone, soul.voice.tone);
        assert_eq!(deserialized.voice.verbosity, soul.voice.verbosity);
        assert_eq!(deserialized.voice.formatting_style, soul.voice.formatting_style);
        assert_eq!(deserialized.voice.language_notes, soul.voice.language_notes);
        assert_eq!(deserialized.directives, soul.directives);
        assert_eq!(deserialized.anti_patterns, soul.anti_patterns);
        assert_eq!(deserialized.relationship, soul.relationship);
        assert_eq!(deserialized.expertise, soul.expertise);
        assert_eq!(deserialized.addendum, soul.addendum);
    }

    #[test]
    fn test_soul_new() {
        let soul = SoulManifest::new();
        assert!(soul.is_empty());
    }

    #[test]
    fn test_is_empty() {
        // Empty soul
        let empty = SoulManifest::default();
        assert!(empty.is_empty());

        // Soul with identity is not empty
        let with_identity = SoulManifest {
            identity: "I am someone".to_string(),
            ..Default::default()
        };
        assert!(!with_identity.is_empty());

        // Soul with directives is not empty
        let with_directives = SoulManifest {
            directives: vec!["Do something".to_string()],
            ..Default::default()
        };
        assert!(!with_directives.is_empty());

        // Soul with only voice/relationship is still "empty" by our definition
        let with_voice = SoulManifest {
            voice: SoulVoice {
                tone: "casual".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(with_voice.is_empty());
    }

    // ========== Markdown Parser Tests ==========

    #[test]
    fn test_split_frontmatter() {
        let content = "---\nkey: value\n---\n# Title\n\nBody text";
        let (fm, body) = SoulManifest::split_frontmatter(content).unwrap();
        assert_eq!(fm, "key: value");
        assert!(body.contains("Title"));
        assert!(body.contains("Body text"));
    }

    #[test]
    fn test_split_frontmatter_no_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here";
        let (fm, body) = SoulManifest::split_frontmatter(content).unwrap();
        assert!(fm.is_empty());
        assert!(body.contains("Just markdown"));
    }

    #[test]
    fn test_split_frontmatter_unclosed() {
        let content = "---\nkey: value\nNo closing delimiter";
        let result = SoulManifest::split_frontmatter(content);
        assert!(result.is_err());
        if let Err(SoulLoadError::Parse(msg)) = result {
            assert!(msg.contains("Unclosed"));
        }
    }

    #[test]
    fn test_parse_list_items() {
        let content = "- First item\n- Second item\n* Third item";
        let items = SoulManifest::parse_list_items(content);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "First item");
        assert_eq!(items[1], "Second item");
        assert_eq!(items[2], "Third item");
    }

    #[test]
    fn test_parse_list_items_with_empty_lines() {
        let content = "- First item\n\n- Second item";
        let items = SoulManifest::parse_list_items(content);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], "First item");
        assert_eq!(items[1], "Second item");
    }

    #[test]
    fn test_from_markdown_full() {
        let content = r#"---
relationship: mentor
voice:
  tone: encouraging
  verbosity: elaborate
expertise:
  - rust
  - systems
---

# Soul: Test

## Identity

I am a test soul for unit testing purposes.

## Directives

- Be thorough
- Be accurate

## Anti-Patterns

- Never skip tests
- Never ignore errors

## Addendum

This is additional context for the soul.
"#;

        let manifest = SoulManifest::from_markdown(content).unwrap();

        assert_eq!(manifest.relationship, RelationshipMode::Mentor);
        assert_eq!(manifest.voice.tone, "encouraging");
        assert_eq!(manifest.voice.verbosity, Verbosity::Elaborate);
        assert_eq!(manifest.expertise, vec!["rust", "systems"]);
        assert!(manifest.identity.contains("test soul"));
        assert_eq!(manifest.directives.len(), 2);
        assert_eq!(manifest.directives[0], "Be thorough");
        assert_eq!(manifest.directives[1], "Be accurate");
        assert_eq!(manifest.anti_patterns.len(), 2);
        assert_eq!(manifest.anti_patterns[0], "Never skip tests");
        assert_eq!(manifest.anti_patterns[1], "Never ignore errors");
        assert!(manifest.addendum.is_some());
        assert!(manifest
            .addendum
            .as_ref()
            .unwrap()
            .contains("additional context"));
    }

    #[test]
    fn test_from_markdown_no_frontmatter() {
        let content = r#"# Soul: Simple

## Identity

A simple soul without frontmatter.

## Directives

- Be simple
"#;

        let manifest = SoulManifest::from_markdown(content).unwrap();
        assert!(manifest.identity.contains("simple soul"));
        assert_eq!(manifest.directives.len(), 1);
        assert_eq!(manifest.directives[0], "Be simple");
        // Default values for unparsed fields
        assert_eq!(manifest.relationship, RelationshipMode::default());
    }

    #[test]
    fn test_from_markdown_frontmatter_priority() {
        // Frontmatter values should take priority over body sections
        let content = r#"---
identity: I am defined in frontmatter
directives:
  - Frontmatter directive
---

## Identity

I am defined in the body.

## Directives

- Body directive
"#;

        let manifest = SoulManifest::from_markdown(content).unwrap();
        // Frontmatter takes priority
        assert_eq!(manifest.identity, "I am defined in frontmatter");
        assert_eq!(manifest.directives, vec!["Frontmatter directive"]);
    }

    #[test]
    fn test_from_markdown_antipatterns_variations() {
        // Test different section name variations
        let content1 = "## Anti-Patterns\n\n- Pattern 1";
        let content2 = "## antipatterns\n\n- Pattern 2";
        let content3 = "## anti patterns\n\n- Pattern 3";

        let mut manifest1 = SoulManifest::default();
        SoulManifest::parse_markdown_body(&mut manifest1, content1).unwrap();
        assert_eq!(manifest1.anti_patterns, vec!["Pattern 1"]);

        let mut manifest2 = SoulManifest::default();
        SoulManifest::parse_markdown_body(&mut manifest2, content2).unwrap();
        assert_eq!(manifest2.anti_patterns, vec!["Pattern 2"]);

        let mut manifest3 = SoulManifest::default();
        SoulManifest::parse_markdown_body(&mut manifest3, content3).unwrap();
        assert_eq!(manifest3.anti_patterns, vec!["Pattern 3"]);
    }

    #[test]
    fn test_from_markdown_addendum_variations() {
        // Test different addendum section name variations
        let content1 = "## Addendum\n\nContext 1";
        let content2 = "## Additional Context\n\nContext 2";
        let content3 = "## Context\n\nContext 3";

        let mut manifest1 = SoulManifest::default();
        SoulManifest::parse_markdown_body(&mut manifest1, content1).unwrap();
        assert_eq!(manifest1.addendum, Some("Context 1".to_string()));

        let mut manifest2 = SoulManifest::default();
        SoulManifest::parse_markdown_body(&mut manifest2, content2).unwrap();
        assert_eq!(manifest2.addendum, Some("Context 2".to_string()));

        let mut manifest3 = SoulManifest::default();
        SoulManifest::parse_markdown_body(&mut manifest3, content3).unwrap();
        assert_eq!(manifest3.addendum, Some("Context 3".to_string()));
    }

    #[test]
    fn test_from_markdown_empty_content() {
        let content = "";
        let manifest = SoulManifest::from_markdown(content).unwrap();
        assert!(manifest.is_empty());
    }

    #[test]
    fn test_from_markdown_only_frontmatter() {
        let content = r#"---
relationship: peer
voice:
  tone: casual
---
"#;

        let manifest = SoulManifest::from_markdown(content).unwrap();
        assert_eq!(manifest.relationship, RelationshipMode::Peer);
        assert_eq!(manifest.voice.tone, "casual");
        assert!(manifest.identity.is_empty());
    }
}
