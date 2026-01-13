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

/// Default OCR prompt optimized for Chinese and English text extraction with Markdown formatting
pub const DEFAULT_OCR_PROMPT: &str = r#"Extract all content from the image and output it in Markdown format, preserving the original structure and layout.

Formatting Rules:
1. **Tables**: Reproduce using Markdown table syntax (| column | headers |)
2. **Lists**: Use - for bullet points, 1. 2. 3. for numbered lists
3. **Headings**: Use # ## ### based on visual hierarchy
4. **Code**: Wrap code snippets in ```language``` blocks
5. **Math formulas**: Use LaTeX syntax wrapped in $ or $$ (e.g., $E=mc^2$)
6. **Emphasis**: Use **bold** for bold text, *italic* for italic
7. **Paragraphs**: Preserve paragraph breaks with blank lines

Content Requirements:
- Output ONLY the extracted content in Markdown, no explanations
- For multi-column layouts: left to right, top to bottom
- Preserve original language (Chinese, English, or mixed)
- Maintain the visual hierarchy and relationships between elements
- If content structure is unclear, use best judgment to represent it

Output the Markdown now:"#;

/// Default description prompt for image analysis
pub const DEFAULT_DESCRIBE_PROMPT: &str = r#"Please describe the content of this image in detail.

Include:
1. Main elements and their arrangement
2. Text content if visible (summarize if lengthy)
3. Colors, style, and overall appearance
4. Any notable features or details

Be concise but comprehensive."#;

/// Default OCR with context prompt
pub const DEFAULT_OCR_WITH_CONTEXT_PROMPT: &str = r#"Analyze this image and respond to the user's question using Markdown format.

Steps:
1. Extract visible content, preserving structure with Markdown:
   - Tables → Markdown tables
   - Lists → bullet/numbered lists
   - Code → fenced code blocks
   - Math → LaTeX ($...$)
2. Answer the user's question based on the extracted content

User question: {prompt}

Respond in Markdown:"#;

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
