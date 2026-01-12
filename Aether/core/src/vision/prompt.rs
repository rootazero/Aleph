//! Vision prompt templates
//!
//! Provides prompt templates for different vision tasks.

use super::{VisionConfig, VisionTask};

/// Build the appropriate prompt for a vision task
pub fn build_prompt(task: &VisionTask, config: &VisionConfig, user_prompt: Option<&str>) -> String {
    match task {
        VisionTask::OcrOnly => config.ocr_prompt.clone(),
        VisionTask::Describe => config.describe_prompt.clone(),
        VisionTask::OcrWithContext => {
            let prompt = user_prompt.unwrap_or("Please describe the content of this image");
            config
                .ocr_with_context_prompt
                .replace("{prompt}", prompt)
        }
    }
}

/// Default OCR prompt optimized for Chinese and English text extraction
pub const DEFAULT_OCR_PROMPT: &str = r#"Please extract all text from the image, preserving the original format and line breaks.

Requirements:
1. Output only the extracted text without any explanations
2. Preserve the original layout and structure as much as possible
3. If there are multiple columns, extract from left to right, top to bottom
4. For mixed Chinese and English text, maintain the original language
5. Do not add any commentary or interpretation

Output the text now:"#;

/// Default description prompt for image analysis
pub const DEFAULT_DESCRIBE_PROMPT: &str = r#"Please describe the content of this image in detail.

Include:
1. Main elements and their arrangement
2. Text content if visible (summarize if lengthy)
3. Colors, style, and overall appearance
4. Any notable features or details

Be concise but comprehensive."#;

/// Default OCR with context prompt
pub const DEFAULT_OCR_WITH_CONTEXT_PROMPT: &str = r#"Please analyze this image and respond to the user's question.

Steps:
1. First, extract any visible text from the image
2. Then, use the extracted content to answer the user's question

User question: {prompt}

Your response:"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ocr_prompt() {
        let config = VisionConfig::default();
        let prompt = build_prompt(&VisionTask::OcrOnly, &config, None);
        assert!(!prompt.is_empty());
        assert!(prompt.contains("extract"));
    }

    #[test]
    fn test_build_describe_prompt() {
        let config = VisionConfig::default();
        let prompt = build_prompt(&VisionTask::Describe, &config, None);
        assert!(!prompt.is_empty());
        assert!(prompt.contains("describe"));
    }

    #[test]
    fn test_build_ocr_with_context_prompt() {
        let config = VisionConfig::default();
        let prompt = build_prompt(
            &VisionTask::OcrWithContext,
            &config,
            Some("What does this error message say?"),
        );
        assert!(prompt.contains("What does this error message say?"));
    }

    #[test]
    fn test_build_ocr_with_context_default_prompt() {
        let config = VisionConfig::default();
        let prompt = build_prompt(&VisionTask::OcrWithContext, &config, None);
        assert!(prompt.contains("describe the content"));
    }
}
