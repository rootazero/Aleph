//! SoulLayer — identity and personality injection (priority 50)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SoulLayer;

impl PromptLayer for SoulLayer {
    fn name(&self) -> &'static str { "soul" }
    fn priority(&self) -> u32 { 50 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let soul = match input.soul {
            Some(s) => s,
            None => return,
        };

        if soul.is_empty() {
            return;
        }

        output.push_str("# Identity\n\n");

        // Core identity statement
        if !soul.identity.is_empty() {
            output.push_str(&soul.identity);
            output.push_str("\n\n");
        }

        // Communication style
        if !soul.voice.tone.is_empty() {
            output.push_str("## Communication Style\n\n");
            output.push_str(&format!("- **Tone**: {}\n", soul.voice.tone));
            output.push_str(&format!("- **Verbosity**: {:?}\n", soul.voice.verbosity));
            output.push_str(&format!(
                "- **Formatting**: {:?}\n",
                soul.voice.formatting_style
            ));
            if let Some(ref notes) = soul.voice.language_notes {
                output.push_str(&format!("- **Language Notes**: {}\n", notes));
            }
            output.push('\n');
        }

        // Relationship mode
        output.push_str("## Relationship with User\n\n");
        output.push_str(soul.relationship.description());
        output.push_str("\n\n");

        // Expertise domains
        if !soul.expertise.is_empty() {
            output.push_str("## Areas of Expertise\n\n");
            for domain in &soul.expertise {
                output.push_str(&format!("- {}\n", domain));
            }
            output.push('\n');
        }

        // Behavioral directives
        if !soul.directives.is_empty() {
            output.push_str("## Behavioral Directives\n\n");
            for directive in &soul.directives {
                output.push_str(&format!("- {}\n", directive));
            }
            output.push('\n');
        }

        // Anti-patterns
        if !soul.anti_patterns.is_empty() {
            output.push_str("## What I Never Do\n\n");
            for anti in &soul.anti_patterns {
                output.push_str(&format!("- {}\n", anti));
            }
            output.push('\n');
        }

        // Custom addendum
        if let Some(ref addendum) = soul.addendum {
            output.push_str("## Additional Context\n\n");
            output.push_str(addendum);
            output.push_str("\n\n");
        }

        output.push_str("---\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::soul::{SoulManifest, SoulVoice, Verbosity};

    #[test]
    fn test_soul_basic() {
        let layer = SoulLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest {
            identity: "I am Aleph.".to_string(),
            voice: SoulVoice {
                tone: "friendly".to_string(),
                verbosity: Verbosity::Balanced,
                ..Default::default()
            },
            directives: vec!["Be helpful".to_string()],
            anti_patterns: vec!["Never lie".to_string()],
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("# Identity"));
        assert!(out.contains("I am Aleph."));
        assert!(out.contains("Communication Style"));
        assert!(out.contains("friendly"));
        assert!(out.contains("Behavioral Directives"));
        assert!(out.contains("Be helpful"));
        assert!(out.contains("What I Never Do"));
        assert!(out.contains("Never lie"));
        assert!(out.contains("---"));
    }

    #[test]
    fn test_soul_empty() {
        let layer = SoulLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let input = LayerInput::soul(&config, &tools, &soul);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_soul_none() {
        let layer = SoulLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools); // no soul
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_soul_paths() {
        let paths = SoulLayer.paths();
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&AssemblyPath::Soul));
    }
}
