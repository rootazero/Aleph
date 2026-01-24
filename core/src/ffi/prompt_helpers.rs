//! Prompt helper functions for agent execution
//!
//! This module provides utility functions for building prompts,
//! extracting content, and formatting generation models.

use std::sync::Arc;

use crate::agents::rig::{ChatMessage, MessageRole};

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// Format generation model information for system prompt injection
///
/// This formats the configured generation providers and their model aliases
/// so the LLM knows what models are available for image/video/audio generation.
pub fn format_generation_models_for_prompt(
    config: &crate::config::GenerationConfig,
) -> Option<String> {
    use crate::generation::GenerationType;

    let enabled_providers: Vec<_> = config
        .providers
        .iter()
        .filter(|(_, cfg)| cfg.enabled)
        .collect();

    if enabled_providers.is_empty() {
        return None;
    }

    let mut lines = vec![];
    lines.push("**Use generate_image tool for image generation**".to_string());
    lines.push(String::new());
    lines.push("**Model Alias Mapping (Important):**".to_string());

    for (name, cfg) in &enabled_providers {
        // Model aliases
        for (alias, model) in &cfg.models {
            lines.push(format!(
                "- \"{}\" → provider: \"{}\", model: \"{}\"",
                alias, name, model
            ));
        }

        // Capability description
        let caps: Vec<&str> = cfg
            .capabilities
            .iter()
            .map(|c| match c {
                GenerationType::Image => "图像",
                GenerationType::Video => "视频",
                GenerationType::Audio => "音频",
                GenerationType::Speech => "语音",
            })
            .collect();

        if let Some(ref default_model) = cfg.model {
            lines.push(format!(
                "- **{}** ({}) - default: {}",
                name,
                caps.join("/"),
                default_model
            ));
        }
    }

    Some(lines.join("\n"))
}

/// Build history summary from conversation histories for cross-session context
///
/// This extracts recent messages from the conversation history for a given topic
/// and formats them as a summary for the agent loop's initial context.
pub fn build_history_summary_from_conversations(
    histories: &Arc<std::sync::RwLock<std::collections::HashMap<String, Vec<ChatMessage>>>>,
    topic_id: &Option<String>,
    max_chars: usize,
) -> String {
    let tid = match topic_id {
        Some(t) => t,
        None => return String::new(),
    };

    let histories_guard = match histories.read() {
        Ok(g) => g,
        Err(_) => return String::new(),
    };

    let messages = match histories_guard.get(tid) {
        Some(m) if !m.is_empty() => m,
        _ => return String::new(),
    };

    let mut summary = String::from("[Previous conversation]\n");
    let mut current_len = summary.len();

    // Take recent messages, preserving order (oldest to newest)
    for msg in messages
        .iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let (role, content_str) = match msg.role {
            MessageRole::User => ("User", msg.content.clone().unwrap_or_default()),
            MessageRole::Assistant => ("Assistant", msg.content.clone().unwrap_or_default()),
            MessageRole::System => continue, // Skip system messages
            MessageRole::Tool => continue,   // Skip tool messages
        };

        if content_str.is_empty() {
            continue;
        }

        let display_content = truncate_str(&content_str, 75);

        let line = format!("{}: {}\n", role, display_content);
        if current_len + line.len() > max_chars {
            summary.push_str("...(earlier messages truncated)\n");
            break;
        }
        summary.push_str(&line);
        current_len += line.len();
    }

    summary
}

/// Check if the response indicates the user needs to provide more input
///
/// Detects patterns that suggest the AI is waiting for user response.
pub fn response_needs_user_input(response: &str) -> bool {
    let lower = response.to_lowercase();

    // Check for explicit question patterns
    let has_question = lower.contains("请问")
        || lower.contains("你想")
        || lower.contains("你需要")
        || lower.contains("你希望")
        || lower.contains("请选择")
        || lower.contains("请确认")
        || lower.contains("请提供")
        || lower.contains("please")
        || lower.contains("would you")
        || lower.contains("do you want")
        || lower.contains("which")
        || lower.contains("what would");

    // Check for choice patterns (numbered options)
    let has_choices = (lower.contains("1.") || lower.contains("1、") || lower.contains("1)"))
        && (lower.contains("2.") || lower.contains("2、") || lower.contains("2)"));

    // Check for waiting patterns
    let has_waiting = lower.contains("等待")
        || lower.contains("waiting")
        || lower.contains("请输入")
        || lower.contains("请回复");

    has_question || has_choices || has_waiting
}

/// Extract text content from attachments for context injection
pub fn extract_attachment_text(
    attachments: Option<&[crate::core::MediaAttachment]>,
) -> Option<String> {
    use tracing::debug;

    let attachments = attachments?;
    if attachments.is_empty() {
        return None;
    }

    let mut text_parts = Vec::new();
    for att in attachments {
        // Check if this is a text-based attachment we can extract
        let is_text = att.mime_type.starts_with("text/")
            || att.mime_type == "application/json"
            || att.mime_type == "application/xml"
            || att.filename.as_ref().map_or(false, |n: &String| {
                n.ends_with(".md")
                    || n.ends_with(".txt")
                    || n.ends_with(".json")
                    || n.ends_with(".xml")
                    || n.ends_with(".yaml")
                    || n.ends_with(".yml")
                    || n.ends_with(".toml")
                    || n.ends_with(".csv")
            });

        if is_text {
            // For base64 encoded attachments, decode first
            let text_result = if att.encoding == "base64" {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &att.data)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
            } else {
                // Try direct UTF-8 conversion for non-base64 data
                String::from_utf8(att.data.as_bytes().to_vec()).ok()
            };

            if let Some(text) = text_result {
                let file_name = att.filename.as_deref().unwrap_or("attachment");
                text_parts.push(format!(
                    "--- Attachment: {} ({}) ---\n{}",
                    file_name, att.mime_type, text
                ));
            }
        }
    }

    if text_parts.is_empty() {
        debug!("No text attachments found to extract");
        None
    } else {
        Some(text_parts.join("\n\n"))
    }
}
