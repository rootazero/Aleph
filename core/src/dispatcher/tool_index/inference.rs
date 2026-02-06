//! Semantic purpose inference for tools
//!
//! Implements ranked inference strategy:
//! - L0: Extract from tool's structured_meta (preferred, already curated)
//! - L1: Rule-based template using name, category, description (fallback)
//! - L2: Async LLM enhancement (eventual consistency, background optimization)

use std::sync::Arc;
use crate::providers::AiProvider;

/// Result of semantic purpose inference
#[derive(Debug, Clone)]
pub struct InferredPurpose {
    /// The inferred semantic description
    pub description: String,
    /// Which inference level was used (0 = structured_meta, 1 = template, 2 = LLM)
    pub level: u8,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
}

/// Infers semantic purpose descriptions for tools
pub struct SemanticPurposeInferrer {
    /// Optional LLM provider for L2 async enhancement
    llm_provider: Option<Arc<dyn AiProvider>>,
}

impl SemanticPurposeInferrer {
    /// Create a new inferrer without LLM support (L0/L1 only)
    pub fn new() -> Self {
        Self {
            llm_provider: None,
        }
    }

    /// Create a new inferrer with LLM support for L2 optimization
    pub fn with_llm(llm_provider: Arc<dyn AiProvider>) -> Self {
        Self {
            llm_provider: Some(llm_provider),
        }
    }

    /// Check if L2 optimization is available
    pub fn has_l2_support(&self) -> bool {
        self.llm_provider.is_some()
    }

    /// Determine if L2 optimization should be triggered
    ///
    /// L2 is triggered when:
    /// - LLM provider is available
    /// - L1 confidence is below threshold (< 0.7)
    /// - No structured_meta available (level != 0)
    pub fn should_trigger_l2(&self, inferred: &InferredPurpose) -> bool {
        self.llm_provider.is_some()
            && inferred.level != 0  // Not L0 (already high quality)
            && inferred.confidence < 0.7  // Low confidence from L1
    }

    /// Infer semantic purpose using ranked strategy
    ///
    /// # Arguments
    /// * `name` - Tool name
    /// * `description` - Tool's existing description (if any)
    /// * `category` - Tool category (e.g., "file", "search", "code")
    /// * `structured_meta` - Optional curated semantic metadata
    pub fn infer(
        &self,
        name: &str,
        description: Option<&str>,
        category: Option<&str>,
        structured_meta: Option<&str>,
    ) -> InferredPurpose {
        // L0: Try structured_meta first (highest quality)
        if let Some(meta) = structured_meta {
            if !meta.trim().is_empty() {
                return InferredPurpose {
                    description: meta.to_string(),
                    level: 0,
                    confidence: 0.95,
                };
            }
        }

        // L1: Fall back to template-based inference
        self.infer_from_template(name, description, category)
    }

    /// L1: Template-based inference from name, description, category
    fn infer_from_template(
        &self,
        name: &str,
        description: Option<&str>,
        category: Option<&str>,
    ) -> InferredPurpose {
        let mut parts = Vec::new();

        // Build semantic description from available parts
        if let Some(cat) = category {
            parts.push(format!("[{}]", cat));
        }

        // Use description if available, otherwise derive from name
        if let Some(desc) = description {
            if !desc.trim().is_empty() {
                parts.push(desc.to_string());
            } else {
                parts.push(Self::humanize_name(name));
            }
        } else {
            parts.push(Self::humanize_name(name));
        }

        let description = parts.join(" ");
        let confidence = self.calculate_confidence(description.as_str(), category.is_some());

        InferredPurpose {
            description,
            level: 1,
            confidence,
        }
    }

    /// Convert tool name to human-readable description
    /// e.g., "read_file" -> "Read file", "searchCode" -> "Search code"
    fn humanize_name(name: &str) -> String {
        // Handle snake_case
        let words: Vec<&str> = name.split('_').collect();
        if words.len() > 1 {
            let result: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
            let mut humanized = result.join(" ");
            // Capitalize first letter
            if let Some(first) = humanized.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            return humanized;
        }

        // Handle camelCase
        let mut result = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                result.push(' ');
                result.push(c.to_lowercase().next().unwrap_or(c));
            } else if i == 0 {
                result.push(c.to_uppercase().next().unwrap_or(c));
            } else {
                result.push(c);
            }
        }
        result
    }

    /// Calculate confidence based on available information
    fn calculate_confidence(&self, description: &str, has_category: bool) -> f32 {
        let mut confidence: f32 = 0.5; // Base confidence for L1

        // Boost if we have category
        if has_category {
            confidence += 0.15;
        }

        // Boost based on description quality
        let word_count = description.split_whitespace().count();
        if word_count >= 5 {
            confidence += 0.15;
        } else if word_count >= 3 {
            confidence += 0.1;
        }

        confidence.min(0.85) // Cap at 0.85 for template-based
    }

    /// L2: Async LLM-based semantic enhancement
    ///
    /// Generates a high-quality semantic description using LLM.
    /// This is meant to be called asynchronously in the background.
    ///
    /// # Arguments
    /// * `tool_id` - Unique tool identifier
    /// * `name` - Tool name
    /// * `description` - Tool's existing description
    /// * `category` - Tool category
    ///
    /// # Returns
    /// Enhanced semantic description with L2 confidence (0.9)
    pub async fn enhance_with_llm(
        &self,
        tool_id: &str,
        name: &str,
        description: Option<&str>,
        category: Option<&str>,
    ) -> Result<InferredPurpose, crate::error::AlephError> {
        let provider = self.llm_provider.as_ref().ok_or_else(|| {
            crate::error::AlephError::config("LLM provider not configured for L2 optimization")
        })?;

        // Build LLM prompt for semantic enhancement
        let (system_prompt, user_prompt) = self.build_l2_prompts(tool_id, name, description, category);

        // Call LLM using process method
        let response = provider
            .process(&user_prompt, Some(&system_prompt))
            .await
            .map_err(|e| {
                crate::error::AlephError::provider(format!("L2 LLM enhancement failed: {}", e))
            })?;

        // Extract and clean the response
        let enhanced_description = response.trim().to_string();

        // Validate response quality
        if enhanced_description.is_empty() || enhanced_description.len() < 10 {
            return Err(crate::error::AlephError::provider(
                "L2 LLM returned invalid description",
            ));
        }

        Ok(InferredPurpose {
            description: enhanced_description,
            level: 2,
            confidence: 0.9, // High confidence for LLM-generated
        })
    }

    /// Build prompts for L2 LLM enhancement
    fn build_l2_prompts(
        &self,
        tool_id: &str,
        name: &str,
        description: Option<&str>,
        category: Option<&str>,
    ) -> (String, String) {
        let system_prompt = String::from(
            "You are a technical writer specializing in tool documentation. \
             Generate concise, actionable descriptions that explain WHEN to use a tool \
             and WHAT problems it solves. Keep responses under 100 words."
        );

        let mut user_prompt = String::from("Generate a semantic description for this tool:\n\n");
        user_prompt.push_str(&format!("Tool ID: {}\n", tool_id));
        user_prompt.push_str(&format!("Name: {}\n", name));

        if let Some(cat) = category {
            user_prompt.push_str(&format!("Category: {}\n", cat));
        }

        if let Some(desc) = description {
            user_prompt.push_str(&format!("Current Description: {}\n", desc));
        }

        user_prompt.push_str(
            "\nProvide a single-sentence description in this format:\n\
             \"Use this tool when you need to [specific use case].\""
        );

        (system_prompt, user_prompt)
    }
}

impl Default for SemanticPurposeInferrer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_l0_structured_meta() {
        let inferrer = SemanticPurposeInferrer::new();
        let result = inferrer.infer(
            "read_file",
            Some("Read file contents"),
            Some("file"),
            Some("Read and retrieve content from local filesystem files"),
        );
        assert_eq!(result.level, 0);
        assert_eq!(result.confidence, 0.95);
        assert_eq!(
            result.description,
            "Read and retrieve content from local filesystem files"
        );
    }

    #[test]
    fn test_l1_template_with_description() {
        let inferrer = SemanticPurposeInferrer::new();
        let result = inferrer.infer(
            "search_code",
            Some("Search code in repository"),
            Some("code"),
            None,
        );
        assert_eq!(result.level, 1);
        assert!(result.confidence > 0.5);
        assert!(result.description.contains("[code]"));
        assert!(result.description.contains("Search code"));
    }

    #[test]
    fn test_l1_template_without_description() {
        let inferrer = SemanticPurposeInferrer::new();
        let result = inferrer.infer("read_file", None, Some("file"), None);
        assert_eq!(result.level, 1);
        assert!(result.description.contains("[file]"));
        assert!(result.description.contains("Read file"));
    }

    #[test]
    fn test_humanize_snake_case() {
        assert_eq!(
            SemanticPurposeInferrer::humanize_name("read_file"),
            "Read file"
        );
        assert_eq!(
            SemanticPurposeInferrer::humanize_name("search_and_replace"),
            "Search and replace"
        );
    }

    #[test]
    fn test_humanize_camel_case() {
        assert_eq!(
            SemanticPurposeInferrer::humanize_name("readFile"),
            "Read file"
        );
        assert_eq!(
            SemanticPurposeInferrer::humanize_name("searchCode"),
            "Search code"
        );
    }

    #[test]
    fn test_empty_structured_meta_falls_back() {
        let inferrer = SemanticPurposeInferrer::new();
        let result = inferrer.infer(
            "tool_name",
            Some("Description"),
            None,
            Some("  "), // Empty/whitespace meta
        );
        assert_eq!(result.level, 1); // Should fall back to L1
    }
}
