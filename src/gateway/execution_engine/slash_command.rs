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

// Shorthand slash aliases now live in the tool-metadata layer
// (`crate::tool_metadata::aliases`) as the single source shared by the
// execution fast path (here), the inbound router's namespace check, and the
// `ToolCatalog` discovery seed. Re-exported here so existing
// `gateway::execution_engine::{is_shorthand_alias}` call sites keep resolving.
pub(crate) use crate::tool_metadata::aliases::{is_shorthand_alias, resolve_shorthand};

/// Continuation-driven slash tools (`/loop`, `/goal`) that must NOT take the
/// L0 direct-tool fast path on ANY surface.
///
/// The fast path returns before the post-run continuation hook, so a loop
/// started (or goal set) through it registers its state but its first tick /
/// first pursuit is never scheduled — a silent stall on the surface that used
/// it. Both the Panel/CLI resolver (`try_resolve_slash_command`) and the
/// channel router (`serialize_parsed_command`) consult this single source so
/// the exclusion can never drift between surfaces — the drift that this fixes
/// was exactly that: only the resolver excluded them, so a channel `/loop`
/// stalled. `moa` is excluded for a different, surface-specific reason (its
/// one-shot form is intercepted earlier) and is handled inline where relevant.
pub(crate) fn is_continuation_driven_slash(name: &str) -> bool {
    matches!(name, "loop" | "goal")
}

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
        let cmd_name = resolve_shorthand(&cmd_name).map_or(cmd_name, ToString::to_string);

        // Continuation-driven tools must NOT take the L0 fast path: it
        // returns before the post-run continuation hook, so a loop started
        // (or goal set) here would sit registered but never scheduled — the
        // loop's first tick / the goal's first pursuit only fire from a full
        // agent run's completion. Falling through also lets the LLM map the
        // free-text args onto the tool's structured schema, which the fast
        // path's generic arg mapping cannot deserialize for these tools.
        //
        // `moa` is excluded for a different reason: its one-shot form
        // (`/moa <prompt>`) is intercepted earlier and never reaches here
        // (the input is rewritten to a plain prompt before this function
        // runs). A bare `/moa` (no prompt) falls through so the LLM maps it
        // onto the tool's structured action schema instead of the fast
        // path's generic arg mapping.
        if is_continuation_driven_slash(&cmd_name) || cmd_name == "moa" {
            return None;
        }

        // Check if this matches a registered tool
        if self.tool_registry.get_tool(&cmd_name).is_some() {
            let mode = serde_json::json!({
                "type": "direct_tool",
                "tool_id": cmd_name,
                "args": args,
            });
            let mode_json = serde_json::to_string(&mode).ok()?;
            info!("[Engine] Inline slash command resolved: /{}", cmd_name);
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
        // Deliberately NO per-run write to the shared `ToolContextHandle`: the
        // per-run `FsScope` published around the tool call below (the
        // `with_fs_scope` wrapper) carries this run's workspace artifact dir and
        // is preferred over the handle at path resolution, so a write here would
        // be a redundant cross-run mutation of a shared anchor.
        let mode: serde_json::Value = serde_json::from_str(mode_json)
            .map_err(|e| ExecutionError::Failed(format!("Invalid slash command metadata: {e}")))?;

        let mode_type = mode["type"].as_str().unwrap_or("");

        info!(
            run_id = %run_id,
            mode_type = %mode_type,
            "Slash command fast path executing"
        );

        match mode_type {
            "direct_tool" => {
                // `/moa <prompt>` arriving through a channel: the inbound
                // router's command parser already classified this as
                // tool_id "moa" with the prompt as `args`, before this run
                // was constructed. Unlike the Panel/CLI path (handled at the
                // `try_resolve_slash_command` call site), `request` here is
                // an immutable borrow — the raw "/moa ..." text still
                // reaches the agent loop as `request.input` on fallthrough.
                // What matters for the one-shot semantics is that MoA is
                // armed BEFORE the run is constructed, which holds either
                // way: Fallthrough always routes to the full agent loop,
                // constructed further down the same `execute()` call.
                if mode["tool_id"].as_str() == Some("moa") {
                    let args = mode["args"].as_str().unwrap_or("");
                    // Mirrors the guard in `execute.rs`'s Panel/CLI intercept: if
                    // `args` is itself a slash command, do not arm MoA — a nested
                    // command can resolve as its own fast path before run
                    // construction, so `take_for_run` never consumes the pref and
                    // it leaks into the user's next turn.
                    if !args.is_empty() && !args.trim_start().starts_with('/') {
                        let key = request.session_key.to_key_string();
                        // Round-3 F3: operator gate (mirror the `moa` tool).
                        if crate::tools::turn_context::role_is_operator(
                            request.metadata.get("caller_role").map(String::as_str),
                        ) {
                            // Round-3 F1: single arm source (one-shot).
                            if let Err(msg) =
                                crate::providers::moa::activation::arm_one_shot(&key, None)
                            {
                                crate::builtin_tools::notify_tool_result("moa", &msg, false);
                            }
                        } else {
                            crate::builtin_tools::notify_tool_result(
                                "moa",
                                "MoA advisory requires operator; running normally.",
                                false,
                            );
                        }
                    }
                    return Err(ExecutionError::Fallthrough {
                        reason: "moa one-shot".to_string(),
                    });
                }
                self.execute_direct_tool(run_id, &mode, request, &agent, emitter)
                    .await
            }

            "skill" => {
                let skill_name = mode["display_name"].as_str().unwrap_or("skill");
                // A `/<skill>` invocation IS a use, and this is the one place
                // both faces reach it: the channel router and the Panel/CLI
                // resolver both land here with the same serialized mode. The
                // `skill_read` tool records its own uses, but a skill invoked
                // only by slash command never touches that tool — it is
                // expanded straight into the prompt below — so without this it
                // aged into `stale` while being used daily, and the dream
                // pipeline's co-occurrence miner never saw it at all.
                if let Some(id) = mode["skill_id"].as_str().filter(|s| !s.is_empty()) {
                    if let Some(mgr) = crate::extension::try_extension_manager() {
                        mgr.skill_system()
                            .record_use(&crate::domain::skill::SkillId::from(id))
                            .await;
                    }
                }
                // Skills need LLM processing with injected instructions — fall through
                Err(ExecutionError::Fallthrough {
                    reason: format!("skill '{skill_name}'"),
                })
            }

            "mcp" => {
                // MCP tools cannot run on the L0 fast path: it dispatches
                // through `BuiltinToolRegistry`, which only knows builtin +
                // plugin tools and holds no MCP client handle. The sole place an
                // MCP tool actually executes is the full agent loop, where
                // `McpRegistryTool` / `ScopedToolService` carry the live
                // transport. So fall through — exactly like `skill` / `custom` —
                // and let the LLM invoke the loop-visible MCP tool from the
                // original `/<tool> <args>` input.
                //
                // (Previously this arm called a deterministic `execute_mcp_tool`
                // that built a `mcp__<server>_<tool>` name and handed it to
                // `BuiltinToolRegistry::execute_tool`, which has no MCP arm — so
                // every MCP slash command hard-failed with "Unknown tool" and
                // never reached the loop that could serve it.)
                let server = mode["server_name"].as_str().unwrap_or("mcp");
                Err(ExecutionError::Fallthrough {
                    reason: format!("mcp command '{server}'"),
                })
            }

            "custom" => {
                // Custom commands need LLM with a custom system prompt — fall through
                Err(ExecutionError::Fallthrough {
                    reason: "custom command".to_string(),
                })
            }

            _ => Err(ExecutionError::Failed(format!(
                "Unknown slash command type: {mode_type}"
            ))),
        }
    }

    /// Execute a direct tool slash command (e.g. /search, /bash).
    ///
    /// The fast path dispatches through the raw [`ToolRegistry`], which holds
    /// none of the loop's gates. Anything the loop would gate must therefore
    /// NOT run here — see [`Self::slash_gate_reason`].
    async fn execute_direct_tool<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        mode: &serde_json::Value,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        let tool_id = mode["tool_id"]
            .as_str()
            .ok_or_else(|| ExecutionError::Failed("Missing tool_id".to_string()))?;
        let args_str = mode["args"].as_str().unwrap_or("");

        // Build tool arguments — map slash command args to the
        // correct field names for each tool's expected schema.
        let arguments = build_tool_arguments(tool_id, args_str, &request.input);

        // Fail closed: a gated call leaves the fast path entirely rather than
        // running ungated here.
        if let Some(reason) = self
            .slash_gate_reason(tool_id, &arguments, request, agent)
            .await
        {
            return Err(ExecutionError::Fallthrough { reason });
        }

        // Emit reasoning event
        let _ = emitter
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq: 0,
                content: format!("Executing /{tool_id} ..."),
                is_complete: true,
            })
            .await;

        // Execute the tool directly. Publish this run's `FsScope` for the call
        // so file / pdf tools anchor relative output at THIS run's workspace
        // artifact dir instead of the shared `ToolContextHandle` — which a
        // concurrent run (the full agent loop or another slash command) can
        // rewrite mid-execution. Mirrors the scope `run_agent_loop` publishes;
        // the fast path has no worktree / project-override notion, so a plain
        // workspace scope over `agent.workspace()` is the sole per-run anchor
        // (this path no longer writes the shared handle).
        let fs_scope =
            crate::tools::fs_scope::FsScope::workspace(agent.workspace().join("output/documents"));
        let execution = crate::tools::fs_scope::with_fs_scope(
            Some(fs_scope),
            self.tool_registry.execute_tool(tool_id, arguments),
        );
        match execution.await {
            Ok(result) => {
                // Settle any `_media` the tool declared into BOTH lanes — the
                // artifact store (workspace pane) and this run's channel
                // delivery buffer. This fast path never touches
                // `ScopedToolService`, where the harvest normally hangs, and it
                // never enters the run-loop scope that publishes the buffer as
                // a task-local, so it hands its own `RunRequest`'s buffer over
                // explicitly. Best-effort by construction: the harvest swallows
                // its own failures and cannot fail the command.
                // The failures it returns are dropped on purpose: a slash
                // command has no model turn to correct anything in, and its own
                // response text is already on its way to the user. They are
                // logged inside the harvest.
                // This path wraps the execution in no wall clock of its own, so
                // without a deadline the harvest here is genuinely unbounded —
                // up to `MAX_MEDIA_PER_RUN` fetches of 60 s in front of a user
                // who is watching a `/image` finish. The bound comes from the
                // same `tools::budget` the agent-loop chokepoint's timeout is
                // built from, so "how long may this tool take" keeps one answer
                // across both entry points.
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis(
                        crate::tools::budget::resolve_tool_budget_ms(tool_id, None),
                    );
                let _ = crate::tools::scoped::artifact_harvest::harvest_media_for_session(
                    tool_id,
                    &result,
                    &request.session_key.to_key_string(),
                    Some(run_id),
                    Some(&request.pending_media),
                    deadline,
                )
                .await;

                let response = extract_tool_response(&result);

                // Stream response
                let _ = emitter
                    .emit(StreamEvent::ResponseChunk {
                        run_id: run_id.to_string(),
                        seq: 1,
                        delta: response.clone(),
                        full_text: String::new(),
                        chunk_index: 0,
                        is_final: true,
                        is_intermediate: false,
                    })
                    .await;

                Ok(response)
            }
            Err(e) => Err(ExecutionError::Failed(format!(
                "Tool '{tool_id}' execution failed: {e}"
            ))),
        }
    }

    /// Why this slash call may not take the fast path, or `None` when it may.
    ///
    /// The fast path is the one tool-dispatch surface with no approval
    /// transport, no `TurnContext` and no `ScopedToolService` — it calls
    /// `ToolRegistry::execute_tool` directly. So it cannot ENFORCE the gates;
    /// it can only decline. A gated call returns
    /// [`ExecutionError::Fallthrough`], which routes it into the full agent
    /// loop, where `ScopedToolService` re-evaluates it with the real tool facts
    /// and can raise the approval card, apply the operator gate, and deny.
    /// Capability is preserved; only the ungated shortcut is withdrawn.
    ///
    /// Every clause is a deterministic hard filter over declared metadata and a
    /// static role — no message-content inspection, no intent classification.
    async fn slash_gate_reason(
        &self,
        name: &str,
        arguments: &serde_json::Value,
        request: &RunRequest,
        agent: &AgentInstance,
    ) -> Option<String> {
        use crate::config::types::policies::{effective_permission, ToolFacts};
        use crate::extension::PermissionAction;

        let (exec_tier, tool_permissions) = self.resolve_turn_permissions(request, agent).await;
        // The permissions resolver above persists a request-carried tier onto
        // the session as a side effect (stamp-on-carry). The mode and
        // think-level twins carry the same contract, and a fast-path dispatch
        // is the turn: run their resolvers too (values unused here — the fast
        // path builds no tool surface and no prompt) so a mode/thinking pick
        // riding a slash message is not silently dropped.
        let _ = self.resolve_turn_mode(request).await;
        let _ = self.resolve_turn_think_level(request).await;
        let caller_role = request.metadata.get("caller_role").map(String::as_str);
        let caller_is_operator = crate::tools::turn_context::role_is_operator(caller_role);

        // The same facts `ScopedToolService` builds. `requires_approval` is read
        // from the adapter's own declaration list rather than guessed: the fast
        // path can only reach builtin and plugin tools (MCP and skill modes fall
        // through above), and those are exactly the tools that list covers — so
        // this is the tier's real input, not a fail-closed stand-in that would
        // make every command fall through.
        let facts = ToolFacts {
            name,
            idempotent: crate::tools::retry::is_idempotent_builtin_name(name),
            requires_approval: crate::security::dangerous_tools::is_confirmation_gated(name),
        };
        let permission = effective_permission(tool_permissions.as_ref(), Some(exec_tier), facts);

        if permission != PermissionAction::Allow {
            return Some(format!(
                "`/{name}` resolves to {permission:?} under tier `{}`",
                exec_tier.id()
            ));
        }
        // The argument-keyed filter: `file_ops` hides `delete` behind the same
        // name as `list`, so no name-keyed rule can see it.
        if exec_tier.asks_for_arguments(name, arguments) {
            return Some(format!(
                "`/{name}` arguments trip the tier's destructive filter"
            ));
        }
        // Declared `requires_confirmation` gates at EVERY tier, including Full.
        if facts.requires_approval {
            return Some(format!("`/{name}` requires confirmation"));
        }
        if !caller_is_operator {
            // The config-tier gate (`method_authz`) and the untrusted-surface
            // hard floor. Both are scoped to non-operator callers by their own
            // contract — a Panel / CLI / loopback operator is never restricted
            // here, so their `/bash` keeps its fast path.
            if crate::gateway::method_authz::tool_requires_operator(name) {
                return Some(format!("`/{name}` requires an operator caller"));
            }
            if crate::security::dangerous_tools::is_dangerous_tool(name) {
                return Some(format!(
                    "`/{name}` is off-limits to untrusted surfaces by default"
                ));
            }
        }
        None
    }
}

/// Build tool arguments from slash command args, mapping to the correct field
/// names for each tool's expected schema.
fn build_tool_arguments(tool_id: &str, args_str: &str, raw_input: &str) -> serde_json::Value {
    match tool_id {
        "agent_delete" | "agent_switch" => serde_json::json!({
            "agent_id": args_str,
        }),
        "agent_create" => serde_json::json!({
            "input": args_str,
        }),
        "session_rename" => serde_json::json!({
            "topic": args_str,
        }),
        // `/compact [instructions]` — the trailing free text steers what the
        // summary must preserve (codex passes its `/compact` input straight to
        // the summarizer; pi documents `/compact [instructions]`; kimi-cli
        // threads a `custom_instruction`). Without this arm the generic
        // `{input, query, args}` fallback below silently dropped it: none of
        // those keys exist on `SessionCompactArgs`, and serde ignores unknown
        // fields, so the user's directive vanished with no error.
        "session_compact" => {
            if args_str.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "instructions": args_str })
            }
        }
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
            let (sel, txt) = args_str.split_once(' ').unwrap_or((args_str, ""));
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
        "browser_screenshot" | "browser_snapshot" | "browser_profile" => {
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
/// Arrays: comma-separated values for known array fields (`blocked_by`)
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
