/// Content formatters for different context types
///
/// This module contains formatting functions for various context data types:
/// - Memory entries (conversation history)
/// - Memory facts (extracted facts)
/// - Search results
/// - MCP tool results
/// - WebFetch content
use crate::memory::{MemoryEntry, MemoryFact};
use crate::payload::{McpToolResult, WebFetchContent};
use crate::search::SearchResult;
use crate::utils::text_format::{escape_markdown, format_timestamp, truncate_text};

/// Format compressed facts as Markdown (Layer 2 - priority)
///
/// Facts are pre-extracted key information from past conversations,
/// more concise and directly relevant than raw conversation history.
pub fn format_facts_markdown(facts: &[MemoryFact]) -> String {
    let mut lines = vec!["**Known User Information**:".to_string()];

    for fact in facts {
        // Format as bullet point with fact content
        lines.push(format!("- {}", fact.content));

        // Optionally show confidence if it's notably high or low
        if fact.confidence < 0.7 {
            lines.push(format!("  _(confidence: {:.0}%)_", fact.confidence * 100.0));
        }
    }

    lines.join("\n")
}

/// Format memory entries as Markdown (Layer 1 - fallback)
pub fn format_memory_markdown(memories: &[MemoryEntry]) -> String {
    let mut lines = vec!["**Related Conversation History**:".to_string()];

    for (i, entry) in memories.iter().enumerate() {
        lines.push(format!(
            "\n{}. **Conversation at {}**",
            i + 1,
            format_timestamp(entry.context.timestamp)
        ));
        lines.push(format!("   Window: {}", entry.context.window_title));
        lines.push(format!(
            "   User: {}",
            truncate_text(&entry.user_input, 200)
        ));
        lines.push(format!("   AI: {}", truncate_text(&entry.ai_output, 200)));

        // Show similarity score if available
        if let Some(score) = entry.similarity_score {
            lines.push(format!("   Relevance: {:.0}%", score * 100.0));
        }
    }

    lines.join("\n")
}

/// Format search results as Markdown
///
/// Creates a numbered list of search results with:
/// - Title as clickable Markdown link
/// - Snippet/excerpt text
/// - Optional published date
/// - Optional relevance score
///
/// Also includes instructions to help AI understand that these results
/// were fetched by its own search capability, not provided by the user.
pub fn format_search_results_markdown(results: &[SearchResult]) -> String {
    let mut lines = vec![
        "**Web Search Results** (Retrieved by your search capability):".to_string(),
        String::new(),
        "_CRITICAL: These results were just fetched by YOUR search capability in real-time. You HAVE successfully accessed the internet. Do NOT say \"I cannot access the internet\" or ask the user for more search results. Answer directly based on this data._".to_string(),
        String::new(),
    ];

    for (i, result) in results.iter().enumerate() {
        // Main entry with title as link
        lines.push(format!(
            "\n{}. [{}]({})",
            i + 1,
            escape_markdown(&result.title),
            result.url
        ));

        // Snippet/excerpt (truncate if too long)
        if !result.snippet.is_empty() {
            let snippet = truncate_text(&result.snippet, 300);
            lines.push(format!("   {}", snippet));
        }

        // Optional metadata
        let mut metadata = Vec::new();

        // Published date
        if let Some(timestamp) = result.published_date {
            let date = format_timestamp(timestamp);
            metadata.push(format!("Published: {}", date));
        }

        // Relevance score
        if let Some(score) = result.relevance_score {
            metadata.push(format!("Relevance: {:.0}%", score * 100.0));
        }

        // Source type
        if let Some(ref source_type) = result.source_type {
            metadata.push(format!("Type: {}", source_type));
        }

        // Provider attribution
        if let Some(ref provider) = result.provider {
            metadata.push(format!("Source: {}", provider));
        }

        if !metadata.is_empty() {
            lines.push(format!("   _{}_", metadata.join(" | ")));
        }
    }

    lines.join("\n")
}

/// Format MCP tool result as Markdown
pub fn format_mcp_tool_result_markdown(result: &McpToolResult) -> String {
    let mut lines = vec![
        format!(
            "**MCP Tool Execution Result** (Tool: `{}`)",
            result.tool_name
        ),
        String::new(),
    ];

    if result.success {
        lines.push("_Status: SUCCESS_".to_string());
        lines.push(String::new());

        // Format the content based on its type
        if let Some(obj) = result.content.as_object() {
            // Handle structured results
            for (key, value) in obj {
                if key == "data" || key == "content" || key == "result" {
                    // For main data fields, show more content
                    match value {
                        serde_json::Value::String(s) => {
                            let truncated = truncate_text(s, 2000);
                            lines.push(format!("**{}**:", key));
                            lines.push("```".to_string());
                            lines.push(truncated);
                            lines.push("```".to_string());
                        }
                        serde_json::Value::Array(arr) => {
                            lines.push(format!("**{}** ({} items):", key, arr.len()));
                            for (i, item) in arr.iter().take(10).enumerate() {
                                lines.push(format!("{}. {}", i + 1, item));
                            }
                            if arr.len() > 10 {
                                lines.push(format!("... and {} more items", arr.len() - 10));
                            }
                        }
                        _ => {
                            let formatted = serde_json::to_string_pretty(value)
                                .unwrap_or_else(|_| value.to_string());
                            let truncated = truncate_text(&formatted, 1000);
                            lines.push(format!("**{}**: {}", key, truncated));
                        }
                    }
                } else if key == "image" || key == "screenshot" || key == "image_data" {
                    // Handle image data (base64)
                    if let Some(s) = value.as_str() {
                        lines.push(format!("**{}**: [Image data, {} bytes]", key, s.len()));
                        // Note: In a real implementation, you might want to pass the image
                        // to the AI as an attachment for multimodal processing
                    } else {
                        lines.push(format!("**{}**: [Image data]", key));
                    }
                } else if key == "path" || key == "file" {
                    // File paths
                    lines.push(format!("**{}**: `{}`", key, value));
                } else {
                    // Other fields
                    let formatted = value.to_string();
                    let truncated = truncate_text(&formatted, 200);
                    lines.push(format!("**{}**: {}", key, truncated));
                }
                lines.push(String::new());
            }
        } else if let Some(s) = result.content.as_str() {
            // Plain string result
            let truncated = truncate_text(s, 2000);
            lines.push("**Result**:".to_string());
            lines.push("```".to_string());
            lines.push(truncated);
            lines.push("```".to_string());
        } else if result.content.is_null() {
            lines.push("_Tool executed successfully with no output._".to_string());
        } else {
            // Fallback: JSON format
            let formatted = serde_json::to_string_pretty(&result.content)
                .unwrap_or_else(|_| result.content.to_string());
            let truncated = truncate_text(&formatted, 2000);
            lines.push("**Result**:".to_string());
            lines.push("```json".to_string());
            lines.push(truncated);
            lines.push("```".to_string());
        }
    } else {
        lines.push("_Status: FAILED_".to_string());
        lines.push(String::new());
        if let Some(ref error) = result.error {
            lines.push(format!("**Error**: {}", error));
        } else {
            lines.push("**Error**: Unknown error occurred during tool execution.".to_string());
        }
    }

    lines.push(String::new());
    lines.push("_IMPORTANT: Use the above tool result to answer the user's question. If the tool execution failed, explain what went wrong and suggest alternatives._".to_string());

    lines.join("\n")
}

/// Format web page content fetched via WebFetch capability
pub fn format_webfetch_content_markdown(content: &WebFetchContent) -> String {
    let mut lines = vec![
        "**Web Page Content** (Fetched by WebFetch capability):".to_string(),
        String::new(),
        "_CRITICAL: This content was just fetched by YOUR WebFetch capability from the URL. You HAVE successfully accessed this web page. Answer directly based on this content._".to_string(),
        String::new(),
    ];

    // URL and title
    lines.push(format!("**URL**: {}", content.url));
    if let Some(ref title) = content.title {
        lines.push(format!("**Title**: {}", title));
    }
    lines.push(String::new());

    // Content
    let truncated = truncate_text(&content.content, 8000);
    lines.push("**Content**:".to_string());
    lines.push("```".to_string());
    lines.push(truncated);
    lines.push("```".to_string());

    // Metadata
    lines.push(String::new());
    lines.push(format!(
        "_Content length: {} bytes{}_",
        content.content_length,
        if content.was_truncated {
            " (truncated)"
        } else {
            ""
        }
    ));

    lines.join("\n")
}
