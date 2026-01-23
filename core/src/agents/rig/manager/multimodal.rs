//! Multimodal message building utilities

use crate::core::MediaAttachment;
use rig::completion::message::{
    Document, DocumentMediaType, DocumentSourceKind, Image, ImageMediaType, Text, UserContent,
};
use rig::completion::Message;
use rig::OneOrMany;
use tracing::{debug, warn};

/// Build a multimodal Message from text input and attachments
///
/// Handles both image and document attachments based on their encoding:
/// - encoding == "base64": Binary content (images) - sent as Image content
/// - encoding == "utf8": Text content (documents) - sent as Text content with header
pub fn build_multimodal_message(input: &str, attachments: &[MediaAttachment]) -> Message {
    let mut content_items: Vec<UserContent> = Vec::new();

    // Add text content first (even if empty, to have at least one item)
    content_items.push(UserContent::Text(Text {
        text: if input.is_empty() {
            "Describe this content in detail.".to_string()
        } else {
            input.to_string()
        },
    }));

    // Process attachments based on encoding
    for attachment in attachments {
        match attachment.encoding.as_str() {
            "base64" => {
                match attachment.media_type.as_str() {
                    "image" => {
                        // Binary content (images)
                        let media_type = match attachment.mime_type.as_str() {
                            "image/png" => Some(ImageMediaType::PNG),
                            "image/jpeg" => Some(ImageMediaType::JPEG),
                            "image/gif" => Some(ImageMediaType::GIF),
                            "image/webp" => Some(ImageMediaType::WEBP),
                            _ => None,
                        };
                        content_items.push(UserContent::Image(Image {
                            data: DocumentSourceKind::base64(&attachment.data),
                            media_type,
                            detail: None,
                            additional_params: None,
                        }));
                    }
                    "document" | "file" => {
                        // Document content - handle based on mime_type
                        let filename = attachment.filename.as_deref().unwrap_or("document");

                        match attachment.mime_type.as_str() {
                            "application/pdf" => {
                                // PDF: use Document type (supported by Claude, Gemini)
                                content_items.push(UserContent::Document(Document {
                                    data: DocumentSourceKind::base64(&attachment.data),
                                    media_type: Some(DocumentMediaType::PDF),
                                    additional_params: None,
                                }));
                                debug!(filename = filename, "Added PDF document attachment");
                            }
                            "text/plain" | "text/markdown" => {
                                // Text files: decode base64 and add as text
                                if let Ok(decoded) = base64::Engine::decode(
                                    &base64::engine::general_purpose::STANDARD,
                                    &attachment.data,
                                ) {
                                    if let Ok(text) = String::from_utf8(decoded) {
                                        let doc_content =
                                            format!("\n\n--- {} ---\n{}", filename, text);
                                        content_items.push(UserContent::Text(Text {
                                            text: doc_content,
                                        }));
                                        debug!(
                                            filename = filename,
                                            "Added text document attachment"
                                        );
                                    } else {
                                        warn!(
                                            filename = filename,
                                            "Failed to decode text as UTF-8"
                                        );
                                    }
                                } else {
                                    warn!(filename = filename, "Failed to decode base64 content");
                                }
                            }
                            _ => {
                                // Other document types: try to decode as text, fallback to skip
                                if let Ok(decoded) = base64::Engine::decode(
                                    &base64::engine::general_purpose::STANDARD,
                                    &attachment.data,
                                ) {
                                    if let Ok(text) = String::from_utf8(decoded) {
                                        let doc_content =
                                            format!("\n\n--- {} ---\n{}", filename, text);
                                        content_items.push(UserContent::Text(Text {
                                            text: doc_content,
                                        }));
                                        debug!(filename = filename, mime_type = %attachment.mime_type, "Added document as text");
                                    } else {
                                        warn!(
                                            filename = filename,
                                            mime_type = %attachment.mime_type,
                                            "Binary document skipped (not UTF-8 decodable)"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        warn!(
                            media_type = %attachment.media_type,
                            "Unknown media_type for base64 attachment, skipping"
                        );
                    }
                }
            }
            "utf8" => {
                // Text content (documents) - add as text block with header
                let filename = attachment.filename.as_deref().unwrap_or("document");
                let doc_content = format!("\n\n--- {} ---\n{}", filename, attachment.data);
                content_items.push(UserContent::Text(Text { text: doc_content }));
            }
            _ => {
                // Unknown encoding - log and skip
                warn!(
                    encoding = %attachment.encoding,
                    media_type = %attachment.media_type,
                    "Unknown attachment encoding, skipping"
                );
            }
        }
    }

    // Build Message with OneOrMany (guaranteed non-empty due to text above)
    Message::User {
        content: OneOrMany::many(content_items).expect("content_items is guaranteed non-empty"),
    }
}
