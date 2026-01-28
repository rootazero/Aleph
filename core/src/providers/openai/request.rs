/// OpenAI request building
///
/// Functions for constructing chat completion requests.

use crate::config::ProviderConfig;
use crate::core::MediaAttachment;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
    should_use_prepend_mode,
};

use super::types::{
    ChatCompletionRequest, ContentBlock, ImageUrl, Message, MessageContent,
};

/// Build text content for image/multimodal requests.
/// Handles prepend mode for system prompts and provides default description for images.
pub fn build_text_content(
    input: &str,
    system_prompt: Option<&str>,
    use_prepend_mode: bool,
) -> String {
    const DEFAULT_IMAGE_DESC: &str = "Describe this image in detail.";

    match (use_prepend_mode, system_prompt, input.is_empty()) {
        // Prepend mode with system prompt
        (true, Some(prompt), false) => format!("{}\n\n{}", prompt, input),
        (true, Some(prompt), true) => format!("{}\n\n{}", prompt, DEFAULT_IMAGE_DESC),
        // No prepend mode or no system prompt
        (_, _, false) => input.to_string(),
        (_, _, true) => DEFAULT_IMAGE_DESC.to_string(),
    }
}

/// Build request body for chat completion
pub fn build_request(
    config: &ProviderConfig,
    input: &str,
    system_prompt: Option<&str>,
) -> ChatCompletionRequest {
    build_request_with_mode(config, input, system_prompt, false)
}

/// Build request body with explicit mode control.
pub fn build_request_with_mode(
    config: &ProviderConfig,
    input: &str,
    system_prompt: Option<&str>,
    force_standard_mode: bool,
) -> ChatCompletionRequest {
    let mut messages = Vec::new();

    // Check system_prompt_mode: default to prepend for better compatibility
    // Only use standard mode if explicitly set to "standard" OR if force_standard_mode is true
    let use_prepend_mode = !force_standard_mode && should_use_prepend_mode(config);

    if use_prepend_mode {
        // Prepend system prompt to user message (for APIs that ignore system role)
        // Use a clearer format that separates instruction from user input
        let user_content = if let Some(prompt) = system_prompt {
            // Format with strong emphasis on following instructions
            // The <<< >>> markers and CRITICAL language help model understand importance
            format!(
                "<<< SYSTEM INSTRUCTIONS - YOU MUST FOLLOW EXACTLY >>>\n\n{}\n\n<<< END INSTRUCTIONS >>>\n\n<<< USER INPUT >>>\n{}",
                prompt, input
            )
        } else {
            input.to_string()
        };

        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: user_content,
            },
        });
    } else {
        // Standard mode: use separate system message
        if let Some(prompt) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: MessageContent::Text {
                    content: prompt.to_string(),
                },
            });
        }

        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: input.to_string(),
            },
        });
    }

    ChatCompletionRequest {
        model: config.model.clone(),
        messages,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        reasoning_effort: None, // Set via apply_thinking_config if needed
    }
}

/// Apply thinking configuration to an existing request
///
/// For OpenAI o1/o3 models, this sets the reasoning_effort field.
/// Call this after building the base request if thinking is enabled.
pub fn apply_thinking_config(
    request: &mut ChatCompletionRequest,
    reasoning_effort: Option<&str>,
) {
    request.reasoning_effort = reasoning_effort.map(|s| s.to_string());
}

/// Build request body with image for vision API
pub fn build_vision_request(
    config: &ProviderConfig,
    input: &str,
    image: &crate::clipboard::ImageData,
    system_prompt: Option<&str>,
) -> ChatCompletionRequest {
    let mut messages = Vec::new();

    let use_prepend_mode = should_use_prepend_mode(config);

    // Add system prompt if provided and not using prepend mode
    if !use_prepend_mode {
        if let Some(prompt) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: MessageContent::Text {
                    content: prompt.to_string(),
                },
            });
        }
    }

    // Build multimodal user message with text and image
    let mut content_blocks = Vec::new();

    // Determine text content (with prepended system prompt if in prepend mode)
    let text_content = build_text_content(input, system_prompt, use_prepend_mode);

    content_blocks.push(ContentBlock::Text { text: text_content });

    // Add image as data URI
    content_blocks.push(ContentBlock::ImageUrl {
        image_url: ImageUrl {
            url: image.to_base64(),
            detail: Some("auto".to_string()),
        },
    });

    messages.push(Message {
        role: "user".to_string(),
        content: MessageContent::Multimodal {
            content: content_blocks,
        },
    });

    // Use vision model and higher max_tokens for image analysis
    ChatCompletionRequest {
        model: "gpt-4o".to_string(), // Use gpt-4o which supports vision
        messages,
        max_tokens: Some(4096), // Vision responses can be longer
        temperature: config.temperature,
        reasoning_effort: None,
    }
}

/// Build request body with MediaAttachment for vision API (add-multimodal-content-support)
pub fn build_multimodal_request(
    config: &ProviderConfig,
    input: &str,
    attachments: &[MediaAttachment],
    system_prompt: Option<&str>,
) -> ChatCompletionRequest {
    let mut messages = Vec::new();

    let use_prepend_mode = should_use_prepend_mode(config);

    // Add system prompt if provided and not using prepend mode
    if !use_prepend_mode {
        if let Some(prompt) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: MessageContent::Text {
                    content: prompt.to_string(),
                },
            });
        }
    }

    // Separate images and documents
    let (images, documents) = separate_attachments(attachments);

    // Build document context and combine with user input
    let doc_context = build_document_context(&documents);
    let full_input = combine_with_document_context(&doc_context, input);

    // Build multimodal user message with text and images
    let mut content_blocks = Vec::new();

    // Determine text content (with prepended system prompt if in prepend mode)
    let text_content = build_text_content(&full_input, system_prompt, use_prepend_mode);

    content_blocks.push(ContentBlock::Text { text: text_content });

    // Add images from MediaAttachment
    for attachment in images {
        // Build data URI from MediaAttachment
        // Format: data:image/png;base64,<base64_data>
        let data_uri = format!("data:{};base64,{}", attachment.mime_type, attachment.data);
        content_blocks.push(ContentBlock::ImageUrl {
            image_url: ImageUrl {
                url: data_uri,
                detail: Some("auto".to_string()),
            },
        });
    }

    messages.push(Message {
        role: "user".to_string(),
        content: MessageContent::Multimodal {
            content: content_blocks,
        },
    });

    // For multimodal requests, always use the configured model.
    // Custom endpoints (OpenRouter, Azure, relay APIs) should configure vision-capable models.
    // We trust the user's configuration - if they send images to a non-vision model,
    // the API will return an appropriate error.
    ChatCompletionRequest {
        model: config.model.clone(),
        messages,
        max_tokens: Some(config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
        temperature: config.temperature,
        reasoning_effort: None,
    }
}
