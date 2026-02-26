//! LanguageLayer — response language setting (priority 1600)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct LanguageLayer;

impl PromptLayer for LanguageLayer {
    fn name(&self) -> &'static str { "language" }
    fn priority(&self) -> u32 { 1600 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(lang) = &input.config.language {
            let language_name = match lang.as_str() {
                "zh-Hans" => "Chinese (Simplified)",
                "zh-Hant" => "Chinese (Traditional)",
                "en" => "English",
                "ja" => "Japanese",
                "ko" => "Korean",
                "de" => "German",
                "fr" => "French",
                "es" => "Spanish",
                "it" => "Italian",
                "pt" => "Portuguese",
                "ru" => "Russian",
                _ => lang.as_str(),
            };
            output.push_str("## Response Language\n");
            output.push_str(&format!(
                "Respond in {} by default. Exception: If the task explicitly requires a different language \
                (e.g., translation, writing in a specific language), use the requested language instead.\n\n",
                language_name
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_language_chinese() {
        let layer = LanguageLayer;
        let config = PromptConfig {
            language: Some("zh-Hans".to_string()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Response Language"));
        assert!(out.contains("Chinese (Simplified)"));
    }

    #[test]
    fn test_language_unknown_code() {
        let layer = LanguageLayer;
        let config = PromptConfig {
            language: Some("ar".to_string()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Respond in ar by default"));
    }

    #[test]
    fn test_language_absent() {
        let layer = LanguageLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
