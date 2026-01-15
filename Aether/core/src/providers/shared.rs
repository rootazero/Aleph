/// Shared utility functions for AI providers
///
/// This module contains common helper functions used across multiple provider
/// implementations to reduce code duplication.
use crate::config::ProviderConfig;
use crate::core::MediaAttachment;

/// Separate attachments into images and documents.
///
/// Returns a tuple of (images, documents) filtered from the input attachments.
pub fn separate_attachments(
    attachments: &[MediaAttachment],
) -> (Vec<&MediaAttachment>, Vec<&MediaAttachment>) {
    let images: Vec<_> = attachments
        .iter()
        .filter(|a| a.media_type == "image")
        .collect();
    let documents: Vec<_> = attachments
        .iter()
        .filter(|a| a.media_type == "document")
        .collect();
    (images, documents)
}

/// Build document context string from document attachments.
///
/// Formats each document with a header containing the filename and joins them
/// with double newlines for clear separation.
///
/// # Returns
///
/// Empty string if no documents provided, otherwise formatted document content.
pub fn build_document_context(documents: &[&MediaAttachment]) -> String {
    if documents.is_empty() {
        return String::new();
    }

    documents
        .iter()
        .map(|d| {
            let name = d.filename.as_deref().unwrap_or("document");
            format!("--- {} ---\n{}", name, d.data)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Combine document context with user input.
///
/// If document context is empty, returns the input unchanged.
/// Otherwise, prepends the document context to the input.
pub fn combine_with_document_context(doc_context: &str, input: &str) -> String {
    if doc_context.is_empty() {
        input.to_string()
    } else {
        format!("{}\n\n{}", doc_context, input)
    }
}

/// Check if prepend mode should be used for system prompts.
///
/// Prepend mode prepends the system prompt to the user message instead of
/// sending it as a separate system role message. This is useful for APIs
/// that don't properly respect the system role.
///
/// # Arguments
///
/// * `config` - Provider configuration containing system_prompt_mode setting
///
/// # Returns
///
/// * `true` - Use prepend mode (default, for better compatibility)
/// * `false` - Use standard mode (when explicitly set to "standard")
pub fn should_use_prepend_mode(config: &ProviderConfig) -> bool {
    config
        .system_prompt_mode
        .as_ref()
        .map(|m| m.to_lowercase() != "standard")
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_attachment(media_type: &str, filename: Option<&str>, data: &str) -> MediaAttachment {
        MediaAttachment {
            media_type: media_type.to_string(),
            mime_type: if media_type == "image" { "image/png".to_string() } else { "text/plain".to_string() },
            data: data.to_string(),
            encoding: if media_type == "image" { "base64".to_string() } else { "utf8".to_string() },
            filename: filename.map(|s| s.to_string()),
            size_bytes: data.len() as u64,
        }
    }

    #[test]
    fn test_separate_attachments() {
        let attachments = vec![
            create_test_attachment("image", Some("photo.png"), "base64data"),
            create_test_attachment("document", Some("readme.txt"), "document content"),
            create_test_attachment("image", Some("icon.png"), "more base64"),
        ];

        let (images, documents) = separate_attachments(&attachments);

        assert_eq!(images.len(), 2);
        assert_eq!(documents.len(), 1);
    }

    #[test]
    fn test_separate_attachments_empty() {
        let attachments: Vec<MediaAttachment> = vec![];
        let (images, documents) = separate_attachments(&attachments);

        assert!(images.is_empty());
        assert!(documents.is_empty());
    }

    #[test]
    fn test_build_document_context() {
        let doc1 = create_test_attachment("document", Some("file1.txt"), "content1");
        let doc2 = create_test_attachment("document", Some("file2.txt"), "content2");
        let documents = vec![&doc1, &doc2];

        let context = build_document_context(&documents);

        assert!(context.contains("--- file1.txt ---"));
        assert!(context.contains("content1"));
        assert!(context.contains("--- file2.txt ---"));
        assert!(context.contains("content2"));
    }

    #[test]
    fn test_build_document_context_no_filename() {
        let doc = create_test_attachment("document", None, "content");
        let documents = vec![&doc];

        let context = build_document_context(&documents);

        assert!(context.contains("--- document ---"));
    }

    #[test]
    fn test_build_document_context_empty() {
        let documents: Vec<&MediaAttachment> = vec![];
        let context = build_document_context(&documents);

        assert!(context.is_empty());
    }

    #[test]
    fn test_combine_with_document_context() {
        assert_eq!(
            combine_with_document_context("doc content", "user input"),
            "doc content\n\nuser input"
        );
        assert_eq!(
            combine_with_document_context("", "user input"),
            "user input"
        );
    }

    #[test]
    fn test_should_use_prepend_mode() {
        let mut config = ProviderConfig::test_config("model");

        // Default: prepend mode
        config.system_prompt_mode = None;
        assert!(should_use_prepend_mode(&config));

        // Explicit "prepend": prepend mode
        config.system_prompt_mode = Some("prepend".to_string());
        assert!(should_use_prepend_mode(&config));

        // Explicit "standard": standard mode
        config.system_prompt_mode = Some("standard".to_string());
        assert!(!should_use_prepend_mode(&config));

        // Case insensitive
        config.system_prompt_mode = Some("STANDARD".to_string());
        assert!(!should_use_prepend_mode(&config));
    }
}
