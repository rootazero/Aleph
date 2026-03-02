//! ProfileLayer — workspace persona overlay (priority 75)
//!
//! Injects the active workspace profile's system_prompt into the prompt
//! pipeline, between Soul (50) and Role (100).  This allows workspaces
//! to add role-specific context without overriding the base identity.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ProfileLayer;

impl PromptLayer for ProfileLayer {
    fn name(&self) -> &'static str { "profile" }
    fn priority(&self) -> u32 { 75 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let profile = match input.profile {
            Some(p) => p,
            None => return,
        };

        let prompt = match profile.system_prompt.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };

        output.push_str("## Current Role Context\n\n");
        output.push_str(prompt);
        output.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProfileConfig;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::soul::SoulManifest;

    #[test]
    fn test_injects_when_prompt_exists() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: Some("You are a senior Rust engineer.".to_string()),
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Current Role Context"));
        assert!(out.contains("You are a senior Rust engineer."));
    }

    #[test]
    fn test_skips_when_no_prompt() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: None,
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_skips_when_no_profile() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let input = LayerInput::soul(&config, &tools, &soul);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_skips_when_prompt_is_empty() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: Some("".to_string()),
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_profile_paths() {
        let paths = ProfileLayer.paths();
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(!paths.contains(&AssemblyPath::Basic));
    }

    #[test]
    fn test_profile_priority() {
        assert_eq!(ProfileLayer.priority(), 75);
    }
}
