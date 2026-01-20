//! Generation Response Parser
//!
//! Parses AI responses for generation requests in the format:
//! `[GENERATE:type:provider:model:prompt]`
//!
//! This allows the AI to request media generation (images, videos, etc.)
//! when users mention generation models in natural language.

use regex::Regex;
use std::sync::LazyLock;

/// Parsed generation request from AI response
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedGenerationRequest {
    /// Generation type (image, video, audio, speech)
    pub gen_type: String,
    /// Provider name (e.g., "midjourney", "dalle")
    pub provider: String,
    /// Model name or alias (e.g., "nanobanana")
    pub model: String,
    /// Generation prompt
    pub prompt: String,
    /// Original matched text (for replacement)
    pub original_text: String,
}

/// Parse result containing all generation requests and cleaned response
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// Extracted generation requests
    pub requests: Vec<ParsedGenerationRequest>,
    /// Response text with generation tags removed
    pub cleaned_response: String,
}

// Regex pattern for [GENERATE:type:provider:model:prompt]
static GENERATE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[GENERATE:([^:]+):([^:]+):([^:]+):([^\]]+)\]").unwrap());

/// Parse AI response for generation requests
///
/// Looks for patterns like `[GENERATE:image:midjourney:nanobanana:一只可爱的猫]`
/// and extracts them into structured requests.
///
/// # Arguments
/// * `response` - The AI response text to parse
///
/// # Returns
/// ParseResult containing extracted requests and cleaned response text
pub fn parse_generation_requests(response: &str) -> ParseResult {
    let mut requests = Vec::new();
    let mut cleaned_response = response.to_string();

    for cap in GENERATE_PATTERN.captures_iter(response) {
        let original_text = cap.get(0).map(|m| m.as_str()).unwrap_or_default();
        let gen_type = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let provider = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
        let model = cap.get(3).map(|m| m.as_str()).unwrap_or_default();
        let prompt = cap.get(4).map(|m| m.as_str()).unwrap_or_default();

        requests.push(ParsedGenerationRequest {
            gen_type: gen_type.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            prompt: prompt.to_string(),
            original_text: original_text.to_string(),
        });

        // Replace the generation tag with a user-friendly message
        let replacement = format!("🎨 正在使用 {} ({}) 生成: {}", provider, model, prompt);
        cleaned_response = cleaned_response.replace(original_text, &replacement);
    }

    ParseResult {
        requests,
        cleaned_response,
    }
}

/// Check if response contains any generation requests
pub fn has_generation_requests(response: &str) -> bool {
    GENERATE_PATTERN.is_match(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_request() {
        let response =
            "好的，我来帮你生成一张图片。\n[GENERATE:image:midjourney:nanobanana:一只可爱的猫]";
        let result = parse_generation_requests(response);

        assert_eq!(result.requests.len(), 1);
        let req = &result.requests[0];
        assert_eq!(req.gen_type, "image");
        assert_eq!(req.provider, "midjourney");
        assert_eq!(req.model, "nanobanana");
        assert_eq!(req.prompt, "一只可爱的猫");
    }

    #[test]
    fn test_parse_multiple_requests() {
        let response = "我会生成两张图片：\n[GENERATE:image:dalle:dall-e-3:sunset]\n[GENERATE:image:midjourney:nanobanana:mountains]";
        let result = parse_generation_requests(response);

        assert_eq!(result.requests.len(), 2);
        assert_eq!(result.requests[0].provider, "dalle");
        assert_eq!(result.requests[1].provider, "midjourney");
    }

    #[test]
    fn test_cleaned_response() {
        let response = "生成图片：[GENERATE:image:midjourney:nanobanana:cat]";
        let result = parse_generation_requests(response);

        assert!(!result.cleaned_response.contains("[GENERATE"));
        assert!(result.cleaned_response.contains("🎨 正在使用"));
    }

    #[test]
    fn test_no_generation_request() {
        let response = "这是一个普通的回复，没有生成请求。";
        let result = parse_generation_requests(response);

        assert!(result.requests.is_empty());
        assert_eq!(result.cleaned_response, response);
    }

    #[test]
    fn test_has_generation_requests() {
        assert!(has_generation_requests(
            "[GENERATE:image:dalle:model:prompt]"
        ));
        assert!(!has_generation_requests("no generation here"));
    }

    #[test]
    fn test_prompt_with_chinese() {
        let response = "[GENERATE:image:midjourney:fast:一只在海边奔跑的金毛猎犬，夕阳西下]";
        let result = parse_generation_requests(response);

        assert_eq!(result.requests.len(), 1);
        assert_eq!(
            result.requests[0].prompt,
            "一只在海边奔跑的金毛猎犬，夕阳西下"
        );
    }
}
