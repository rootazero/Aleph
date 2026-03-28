//! Slash command resolution and fast-path execution.
//!
//! Extracted from `engine.rs` to keep the main execution engine focused
//! on lifecycle orchestration.

use tracing::info;

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Try to resolve a `/command args` input to a slash command mode JSON.
    ///
    /// Used for non-router paths (Panel, CLI) where the inbound router's
    /// command resolution doesn't run. Returns `Some(mode_json)` if the
    /// command matches a registered tool, `None` otherwise.
    pub(super) fn try_resolve_slash_command(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        let without_slash = trimmed.strip_prefix('/')?;
        if without_slash.is_empty() {
            return None;
        }

        let (cmd_name, args) = match without_slash.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.to_lowercase(), rest.trim().to_string()),
            None => (without_slash.to_lowercase(), String::new()),
        };

        // Strip @botname suffix (e.g. "gen@mybot" → "gen")
        let cmd_name = match cmd_name.split_once('@') {
            Some((name, _)) => name.to_string(),
            None => cmd_name,
        };

        // Map common shorthand commands to their actual tool names
        let cmd_name = match cmd_name.as_str() {
            "rename" => "session_set_topic".to_string(),
            "video" => "video_generate".to_string(),
            "image" => "image_generate".to_string(),
            "audio" => "audio_generate".to_string(),
            "speech" => "speech_generate".to_string(),
            other => other.to_string(),
        };

        // Check if this matches a registered tool
        if self.tool_registry.get_tool(&cmd_name).is_some() {
            let mode = serde_json::json!({
                "type": "direct_tool",
                "tool_id": cmd_name,
                "args": args,
            });
            let mode_json = serde_json::to_string(&mode).ok()?;
            info!(
                "[Engine] Inline slash command resolved: /{}",
                cmd_name
            );
            Some(mode_json)
        } else {
            None
        }
    }

    /// Execute a slash command directly, bypassing the full agent loop.
    ///
    /// This is the L0 fast path: parse the serialized execution mode from metadata,
    /// call the tool via the tool registry, and stream the result back.
    /// Falls back to an error if the tool is not found or execution fails.
    pub(super) async fn execute_slash_command_fast_path<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        mode_json: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        // Write workspace-scoped output paths (same as run_agent_loop)
        if let Some(tc_handle) = self.tool_registry.tool_context_handle() {
            let workspace_path = agent.workspace();
            if let Ok(ctx) = crate::tools::ToolContext::from_workspace(workspace_path) {
                let mut tc = tc_handle.write().await;
                *tc = ctx;
            }
        }

        let mode: serde_json::Value = serde_json::from_str(mode_json)
            .map_err(|e| ExecutionError::Failed(format!("Invalid slash command metadata: {}", e)))?;

        let mode_type = mode["type"].as_str().unwrap_or("");

        info!(
            run_id = %run_id,
            mode_type = %mode_type,
            "Slash command fast path executing"
        );

        match mode_type {
            "direct_tool" => {
                self.execute_direct_tool(run_id, &mode, request, emitter).await
            }

            "skill" => {
                let skill_name = mode["display_name"].as_str().unwrap_or("skill");
                // Skills need LLM processing with injected instructions — fall through
                Err(ExecutionError::Fallthrough {
                    reason: format!("skill '{}'", skill_name),
                })
            }

            "mcp" => {
                self.execute_mcp_tool(run_id, &mode, request, emitter).await
            }

            "custom" => {
                // Custom commands need LLM with a custom system prompt — fall through
                Err(ExecutionError::Fallthrough {
                    reason: "custom command".to_string(),
                })
            }

            _ => {
                Err(ExecutionError::Failed(format!(
                    "Unknown slash command type: {}",
                    mode_type
                )))
            }
        }
    }

    /// Execute a direct tool slash command (e.g. /search, /bash).
    async fn execute_direct_tool<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        mode: &serde_json::Value,
        request: &RunRequest,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        let tool_id = mode["tool_id"]
            .as_str()
            .ok_or_else(|| ExecutionError::Failed("Missing tool_id".to_string()))?;
        let args_str = mode["args"].as_str().unwrap_or("");

        // Build tool arguments — map slash command args to the
        // correct field names for each tool's expected schema.
        let arguments = build_tool_arguments(tool_id, args_str, &request.input);

        // Emit reasoning event
        let _ = emitter
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq: 0,
                content: format!("Executing /{} ...", tool_id),
                is_complete: true,
            })
            .await;

        // Execute the tool directly
        match self.tool_registry.execute_tool(tool_id, arguments).await {
            Ok(result) => {
                // Extract _media from tool output and push to pending_media buffer
                if let Some(media_val) = result.get("_media") {
                    if let Ok(items) = serde_json::from_value::<Vec<crate::gateway::media::MediaItem>>(media_val.clone()) {
                        if !items.is_empty() {
                            info!(
                                run_id = %run_id,
                                tool = %tool_id,
                                count = items.len(),
                                "Fast-path: extracted _media from tool output"
                            );
                            let mut pending = request.pending_media.lock().unwrap_or_else(|e| e.into_inner());
                            let remaining = crate::gateway::media::MAX_MEDIA_PER_RUN.saturating_sub(pending.len());
                            pending.extend(items.into_iter().take(remaining));
                        }
                    }
                }

                let response = extract_tool_response(&result);

                // Stream response
                let _ = emitter
                    .emit(StreamEvent::ResponseChunk {
                        run_id: run_id.to_string(),
                        seq: 1,
                        delta: response.clone(),
                        content: response.clone(),
                        full_text: String::new(),
                        chunk_index: 0,
                        is_final: true,
                        is_intermediate: false,
                    })
                    .await;

                Ok(response)
            }
            Err(e) => {
                Err(ExecutionError::Failed(format!(
                    "Tool '{}' execution failed: {}",
                    tool_id, e
                )))
            }
        }
    }

    /// Execute an MCP tool slash command.
    async fn execute_mcp_tool<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        mode: &serde_json::Value,
        request: &RunRequest,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        let server_name = mode["server_name"].as_str().unwrap_or("");
        let tool_name = mode["tool_name"].as_str();
        let mcp_tool_id = if let Some(tn) = tool_name {
            format!("mcp__{}_{}", server_name, tn)
        } else {
            format!("mcp__{}", server_name)
        };
        let args_str = mode["args"].as_str().unwrap_or("");

        let arguments = serde_json::json!({
            "input": args_str,
            "args": args_str,
            "input_text": request.input,
        });

        let _ = emitter
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq: 0,
                content: format!("Executing MCP tool /{} ...", server_name),
                is_complete: true,
            })
            .await;

        match self.tool_registry.execute_tool(&mcp_tool_id, arguments).await {
            Ok(result) => {
                let response = if let Some(s) = result.as_str() {
                    s.to_string()
                } else {
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                };

                let _ = emitter
                    .emit(StreamEvent::ResponseChunk {
                        run_id: run_id.to_string(),
                        seq: 1,
                        delta: response.clone(),
                        content: response.clone(),
                        full_text: String::new(),
                        chunk_index: 0,
                        is_final: true,
                        is_intermediate: false,
                    })
                    .await;

                Ok(response)
            }
            Err(e) => Err(ExecutionError::Failed(format!(
                "MCP tool '{}' execution failed: {}",
                mcp_tool_id, e
            ))),
        }
    }
}

/// Build tool arguments from slash command args, mapping to the correct field
/// names for each tool's expected schema.
fn build_tool_arguments(tool_id: &str, args_str: &str, raw_input: &str) -> serde_json::Value {
    match tool_id {
        "agent_delete" => serde_json::json!({
            "agent_id": args_str,
        }),
        "agent_create" => serde_json::json!({
            "input": args_str,
        }),
        "session_set_topic" => serde_json::json!({
            "topic": args_str,
        }),
        // URL-based tools
        "browser_open" | "web_fetch" => serde_json::json!({
            "url": args_str,
        }),
        // Selector-based browser tools
        "browser_click" | "browser_select" => serde_json::json!({
            "selector": args_str,
        }),
        "browser_type" => {
            // /browser_type <selector> <text>
            let (sel, txt) = args_str.split_once(' ')
                .unwrap_or((args_str, ""));
            serde_json::json!({
                "selector": sel,
                "text": txt,
            })
        }
        "browser_evaluate" => serde_json::json!({
            "script": args_str,
        }),
        // Query-based tools
        "search" | "memory_search" => serde_json::json!({
            "query": args_str,
        }),
        // Tabs: action is required, default to "list"
        "browser_tabs" => serde_json::json!({
            "action": if args_str.is_empty() { "list" } else { args_str },
        }),
        // Navigate: action is required, default to "refresh"
        "browser_navigate" => serde_json::json!({
            "action": if args_str.is_empty() { "refresh" } else { args_str },
        }),
        // Tools with no required args
        "browser_screenshot" | "browser_snapshot"
        | "browser_profile" => {
            if args_str.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "input": args_str })
            }
        }
        // Generation tools: /video <prompt>, /image <prompt>, /audio <prompt>
        "video_generate" | "image_generate" | "audio_generate" => serde_json::json!({
            "prompt": args_str,
        }),
        // Speech tool: /speech <text>
        "speech_generate" => serde_json::json!({
            "text": args_str,
        }),
        _ => {
            // Generic --key value parser: if args contain "--", parse as named parameters.
            // This handles tools with structured schemas (team_create, task_create, etc.)
            if args_str.contains("--") {
                parse_cli_args(args_str)
            } else {
                serde_json::json!({
                    "input": args_str,
                    "query": args_str,
                    "args": args_str,
                    "input_text": raw_input,
                })
            }
        }
    }
}

/// Parse CLI-style `--key value` arguments into a JSON object.
///
/// Handles: `--name foo --leader main --blocked_by id1,id2`
/// Quoted values: `--name "My Team"` or `--name 'My Team'`
/// Special: values containing `{` are parsed as JSON (for --variables, --metadata)
/// Arrays: comma-separated values for known array fields (blocked_by)
fn parse_cli_args(args_str: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut current_key: Option<String> = None;
    let mut current_vals: Vec<String> = Vec::new();

    let flush = |map: &mut serde_json::Map<String, serde_json::Value>,
                 key: &Option<String>,
                 vals: &[String]| {
        if let Some(ref k) = key {
            let combined = vals.join(" ");
            if combined.is_empty() {
                // Boolean flag
                map.insert(k.clone(), serde_json::Value::Bool(true));
            } else if combined.starts_with('{') || combined.starts_with('[') {
                // Try JSON parse
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&combined) {
                    map.insert(k.clone(), v);
                } else {
                    map.insert(k.clone(), serde_json::Value::String(combined));
                }
            } else if k == "blocked_by" || k == "task_ids" {
                // Always parse as array (comma-separated or single value)
                let arr: Vec<serde_json::Value> = combined
                    .split(',')
                    .map(|s| serde_json::Value::String(s.trim().to_string()))
                    .filter(|v| !v.as_str().is_none_or(|s| s.is_empty()))
                    .collect();
                map.insert(k.clone(), serde_json::Value::Array(arr));
            } else {
                map.insert(k.clone(), serde_json::Value::String(combined));
            }
        }
    };

    // Tokenize with quote awareness: "My Team" and 'My Team' become single tokens
    let tokens = tokenize_with_quotes(args_str);
    for token in &tokens {
        if let Some(key) = token.strip_prefix("--") {
            flush(&mut map, &current_key, &current_vals);
            current_key = Some(key.to_string());
            current_vals.clear();
        } else {
            current_vals.push(token.clone());
        }
    }
    flush(&mut map, &current_key, &current_vals);

    serde_json::Value::Object(map)
}

/// Split a string into tokens, respecting double and single quotes.
///
/// `--name "My Team" --flag` → `["--name", "My Team", "--flag"]`
///
/// Note: escaped quotes (`\"`) inside quoted strings are not supported.
fn tokenize_with_quotes(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in input.chars() {
        match in_quote {
            Some(q) if ch == q => {
                // Closing quote — don't include the quote char itself
                in_quote = None;
            }
            Some(_) => {
                current.push(ch);
            }
            None if ch == '"' || ch == '\'' => {
                in_quote = Some(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Extract a human-readable response from a tool result.
///
/// Prefers `_display` field, then `message`, then string value, then JSON.
fn extract_tool_response(result: &serde_json::Value) -> String {
    if let Some(display) = result.get("_display").and_then(|v| v.as_str()) {
        display.to_string()
    } else if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
        msg.to_string()
    } else if let Some(s) = result.as_str() {
        s.to_string()
    } else {
        serde_json::to_string_pretty(result).unwrap_or_default()
    }
}
