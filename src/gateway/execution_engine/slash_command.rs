//! Slash command resolution and fast-path execution.
//!
//! Extracted from `engine.rs` to keep the main execution engine focused
//! on lifecycle orchestration.

use std::collections::HashMap;

use tracing::info;

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, StreamEvent};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

// Shorthand slash aliases live in the tool-metadata layer
// (`crate::tool_metadata::aliases`) as the single source shared by the inbound
// router's namespace check and the `ToolCatalog` discovery seed. Re-exported
// here so existing `gateway::execution_engine::{is_shorthand_alias}` call
// sites keep resolving.
//
// `resolve_shorthand` used to be re-exported alongside it, for a hand-rolled
// alias pass in this module's resolver. That pass is gone: the resolver now
// goes through `ToolCatalog::find_best_match`, whose alias tier is seeded from
// this same `SHORTHAND_ALIASES` table (via `shorthand_aliases_for`) and which
// additionally handles multi-word command paths and `.`/`_` leniency.
pub(crate) use crate::tool_metadata::aliases::is_shorthand_alias;

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

/// Stamp `BTW_METADATA_KEY` into `metadata` if `input` is a side question.
///
/// Free-standing and parser-free on purpose. `try_resolve_slash_command`
/// returns `None` whenever the shared `CommandParser` cell is empty, and a
/// side question that silently degraded to a normal turn under that condition
/// would run at the session's real tier with the main session's key — the
/// two failures this feature exists to prevent, in the one configuration
/// (tests, simulated mode) where nobody would notice.
pub(crate) fn stamp_btw(input: &str, metadata: &mut HashMap<String, String>) {
    use crate::gateway::btw::{BtwTurn, BTW_METADATA_KEY};
    if metadata.contains_key(BTW_METADATA_KEY) {
        return;
    }
    if let Some(turn) = BtwTurn::resolve(input) {
        metadata.insert(
            BTW_METADATA_KEY.to_string(),
            if turn.promote {
                "promote".to_string()
            } else {
                turn.question
            },
        );
    }
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Stamp `SLASH_COMMAND_MODE_KEY` into `metadata` if `input` is a slash
    /// command. Idempotent: an already-stamped request is left alone.
    ///
    /// **Call this before the request enters the busy wait lane.**
    /// `steering::carries_more_than_text` reads this key to decide that a
    /// slash command must be redelivered as its own run rather than folded
    /// into a running sibling as plain steering text. A surface that stamps
    /// only inside `execute()` — i.e. after the lane gate — therefore has
    /// every slash command silently swallowed whenever a run is already in
    /// flight: the text lands in the transcript, the loop reads it as an
    /// interjection, and the client gets no events and no error.
    pub async fn stamp_slash_mode(&self, input: &str, metadata: &mut HashMap<String, String>) {
        // Side questions first: they are resolved without the command parser,
        // and they must be stamped even when the parser cell is empty.
        stamp_btw(input, metadata);
        if metadata.contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY) {
            return;
        }
        if let Some(mode_json) = self.try_resolve_slash_command(input).await {
            metadata.insert(
                crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY.to_string(),
                mode_json,
            );
        }
    }

    /// Resolve a `/command args` input to the slash-command mode JSON.
    ///
    /// Delegates to the one [`CommandParser`](crate::command::CommandParser)
    /// every surface shares, then to
    /// [`serialize_parsed_command`](crate::gateway::inbound_router::serialize_parsed_command)
    /// — the single producer of that JSON.
    ///
    /// This used to be a bespoke `tool_registry.get_tool()` lookup that could
    /// only ever emit `type: "direct_tool"`, while the router emitted four
    /// kinds. Any surface that fell back to it therefore lost skills, MCP
    /// tools and custom commands *silently*: `/my-skill` resolved to nothing,
    /// so it reached the model as literal text with no instructions overlay,
    /// no `allowed_tools` narrowing, and no recorded use. `agent.run` — i.e.
    /// the TUI — was that surface.
    ///
    /// Returns `None` when the parser cell is still empty (tests, simulated
    /// mode). Deliberately *not* a degraded second derivation: resolving
    /// `/foo` a different way is the drift this convergence removes.
    pub(super) async fn try_resolve_slash_command(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let parser = self.command_parser.read().await.clone()?;
        let parsed = parser.parse_async(trimmed).await?;

        // `moa` stays surface-specific, as it was before this convergence and
        // for the reason recorded then: `/moa <prompt>` is rewritten into a
        // plain prompt earlier in `execute()` and never arrives here, while a
        // bare `/moa` must reach the LLM so it can be mapped onto the tool's
        // structured action schema — the fast path's generic argument mapping
        // cannot deserialize it. Checked against the canonical
        // `command_name`, so an alias cannot slip past it.
        //
        // (`/loop` and `/goal` are excluded too, but inside
        // `serialize_parsed_command` via `is_continuation_driven_slash`, so
        // that exclusion is shared with the router rather than repeated here.)
        if parsed.command_name == "moa" {
            return None;
        }

        let mode_json = crate::gateway::inbound_router::serialize_parsed_command(&parsed)?;
        info!(
            "[Engine] Slash command resolved: /{} ({:?})",
            parsed.command_name, parsed.source_type
        );
        Some(mode_json)
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

        // `plan_gate` is deliberately dropped: the fast path enforces nothing
        // (it only DECLINES the shortcut), and a plan-mode slash call resolves
        // to `Deny` on the very next line, which is a fallthrough reason. The
        // full loop then re-evaluates it with the run's own gate — the one
        // that a plan approval can actually lift.
        let super::turn_permissions::TurnPermissions {
            tier: exec_tier,
            explicit: tool_permissions,
            plan_gate: _,
        } = self.resolve_turn_permissions(request, agent).await;
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
        // name as `list`, so no name-keyed rule can see it. Includes the floor
        // (`ExecTier::floor_asks_for_arguments`), which is why this clause
        // fires at `full` too — `/self_config` writing the approval settings
        // has to reach the gated path whatever the tier says.
        if exec_tier.asks_for_arguments(name, arguments) {
            return Some(format!(
                "`/{name}` arguments trip the tier's destructive filter or the \
                 gate-removal floor"
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
        // `/model <id>` — `SelectModelArgs.model` is a required `String`, and
        // the generic `{input, query, args, input_text}` fallback below carries
        // none of those names, so without this arm every `/model` on every
        // surface (TUI typed, TUI palette, Panel composer, every channel) died
        // with a `Validation` error before reaching the tool. `call_json`'s
        // retry only coerces scalars — it cannot invent a missing field.
        //
        // The provider half is deliberately NOT parsed here: qualified
        // `provider/model` names are resolved downstream by
        // `thinker::resolve_model_to_provider_and_model`, which knows which
        // prefixes are configured providers. A second parser here would be a
        // second answer to the same question.
        //
        // Empty args pass through as `""` so `select_model` can emit its own
        // "model id required" refusal — a message, not a schema error.
        "select_model" => serde_json::json!({
            "model": args_str,
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

#[cfg(test)]
mod arg_mapping_tests {
    use super::build_tool_arguments;
    use crate::executor::create_tool_boxed;
    use crate::tool_metadata::aliases::SHORTHAND_ALIASES;

    /// Required-field names declared by a tool's own JSON schema.
    fn required_fields(schema: &serde_json::Value) -> Vec<String> {
        schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every shorthand must produce a payload the target tool can actually
    /// deserialize.
    ///
    /// The pre-existing guard (`aliases::every_shorthand_target_is_executable`)
    /// only proved the *target exists*. `/model` passed it while being broken on
    /// every surface: `build_tool_arguments` had no `select_model` arm, the
    /// generic fallback emitted `{input, query, args, input_text}`, and
    /// `SelectModelArgs.model` is a required `String` — so `call_json` failed
    /// validation every single time.
    ///
    /// This guard is **derived, not enumerated**: the required-field set comes
    /// from each tool's own schema, so a tool that grows a new required field
    /// turns this red without anyone remembering to update a list. Targets
    /// without a boxed constructor are skipped, but the skip is re-derived every
    /// run (`create_tool_boxed(..).is_none()`) rather than read off an allowlist
    /// that would rot into a permission slip.
    #[test]
    fn every_shorthand_payload_satisfies_its_targets_required_fields() {
        const SAMPLE: &str = "sample-value";
        let mut checked = 0usize;

        for (alias, target) in SHORTHAND_ALIASES {
            // Runtime-gated tools (registered by the live registry, not
            // constructible standalone) expose no schema here.
            let Some(tool) = create_tool_boxed(target, None) else {
                continue;
            };
            checked += 1;

            let schema = tool.definition().parameters;
            let required = required_fields(&schema);
            if required.is_empty() {
                continue;
            }

            let raw_input = format!("/{alias} {SAMPLE}");
            let args = build_tool_arguments(target, SAMPLE, &raw_input);
            let obj = args.as_object().unwrap_or_else(|| {
                panic!("/{alias} -> `{target}`: build_tool_arguments produced a non-object payload")
            });

            for field in &required {
                assert!(
                    obj.contains_key(field),
                    "/{alias} -> `{target}`: payload {args} is missing required field `{field}`. \
                     Add an arm to `build_tool_arguments` mapping the slash argument onto it — \
                     do NOT give the field `#[serde(default)]`, which swaps a loud validation \
                     error for a silent no-op."
                );
            }
        }

        assert!(
            checked > 0,
            "no shorthand target was constructible — this guard went blind"
        );
    }

    /// The specific regression: `/model claude-sonnet-5` must reach
    /// `select_model` with the id in `model`, not smeared across the generic
    /// `{input, query, args}` fallback.
    #[test]
    fn model_shorthand_maps_the_argument_onto_the_model_field() {
        let args =
            build_tool_arguments("select_model", "claude-sonnet-5", "/model claude-sonnet-5");
        assert_eq!(args["model"], "claude-sonnet-5");
        assert!(
            args.get("input").is_none(),
            "select_model must not fall through to the generic arm: {args}"
        );
    }

    /// A bare `/model` still reaches the tool (which answers with its own
    /// refusal); it must not fail schema validation before getting there.
    #[test]
    fn bare_model_shorthand_still_carries_the_required_field() {
        let args = build_tool_arguments("select_model", "", "/model");
        assert_eq!(args["model"], "");
    }

    // ================================================================
    // Source-level guards: the slash-mode JSON must keep one producer
    // ================================================================

    /// Every `.rs` under this crate's `src/`, as (repo-relative, contents).
    /// Includes `src/bin/`, which is where the two run-start handlers live.
    fn all_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");
        files
            .into_iter()
            .filter_map(|file| {
                let rel = file
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                std::fs::read_to_string(&file).ok().map(|t| (rel, t))
            })
            .collect()
    }

    /// The part of a file that ships. Split on the bare attribute — anchoring
    /// the separator to a line start would match nothing on a CRLF checkout
    /// and silently turn "production prefix" into "the whole file".
    fn production_prefix(text: &str) -> String {
        text.replace('\r', "")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && !t.starts_with('*')
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `serialize_parsed_command` is the only place the fast path's mode JSON
    /// is built. It was not always: this module held a second, weaker
    /// derivation that could only emit `direct_tool`, so every surface falling
    /// back to it lost skills, MCP tools and custom commands — silently, since
    /// an unresolved slash command simply reaches the model as text.
    #[test]
    fn the_slash_mode_json_has_exactly_one_producer() {
        const PRODUCER: &str = "src/gateway/inbound_router/command_handler.rs";
        let kinds = ["skill", "mcp", "custom", "direct_tool"];

        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in all_sources() {
            if rel == PRODUCER {
                continue;
            }
            // Files that exist only as a test module have no `#[cfg(test)]`
            // marker of their own to split on, so name them out structurally.
            if rel.ends_with("/tests.rs") || rel.contains("/tests/") {
                continue;
            }
            for (n, line) in production_prefix(&text).lines().enumerate() {
                if line.contains("\"type\":")
                    && kinds.iter().any(|k| line.contains(&format!("\"{k}\"")))
                {
                    offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "the slash-command mode JSON must be built only by \
             `serialize_parsed_command`; a second producer drifts from it one \
             variant at a time and the loss is silent:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The stamp must land before the request enters the busy wait lane.
    ///
    /// `steering::carries_more_than_text` reads the key to keep a slash
    /// command out of a running sibling's steering fold. `agent.run` used to
    /// stamp nowhere at all, so every slash command it carried was swallowed
    /// whenever a run was already in flight — no events, no error.
    ///
    /// Phrased over *whichever* functions spawn, not over a list of handler
    /// names: a third run-start handler inherits the requirement instead of
    /// having to be told about it.
    #[test]
    fn every_run_start_handler_stamps_the_slash_mode_before_the_busy_lane() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/bin/aleph-server/server_init.rs");
        let text = production_prefix(&std::fs::read_to_string(&path).expect("server_init.rs"));

        let mut checked = 0usize;
        let mut offenders: Vec<String> = Vec::new();
        for chunk in text.split("pub async fn handle_").skip(1) {
            let Some(spawn) = chunk.find("spawn_queued_run(") else {
                continue;
            };
            checked += 1;
            let name = chunk.lines().next().unwrap_or("?").trim().to_string();
            match chunk.find("stamp_slash_mode(") {
                Some(stamp) if stamp < spawn => {}
                _ => offenders.push(name),
            }
        }

        assert!(
            checked >= 2,
            "expected to find both run-start handlers; found {checked} — the \
             scan stopped matching, so its green means nothing"
        );
        assert!(
            offenders.is_empty(),
            "these start a run without resolving slash input first, so any \
             slash command they carry is folded into a running sibling as \
             plain text and never executes: {offenders:?}"
        );
    }

    #[test]
    fn btw_is_stamped_and_therefore_never_folded_into_a_running_sibling() {
        use crate::gateway::btw::BTW_METADATA_KEY;
        let mut metadata = std::collections::HashMap::new();

        // The pure half of stamp_slash_mode: btw resolution needs no parser and
        // must therefore work even when the command-parser cell is empty (tests,
        // simulated mode) — the exact condition under which try_resolve_slash_command
        // returns None.
        super::stamp_btw("/btw what was that file called?", &mut metadata);
        assert_eq!(
            metadata.get(BTW_METADATA_KEY).map(String::as_str),
            Some("what was that file called?")
        );

        let mut plain = std::collections::HashMap::new();
        super::stamp_btw("just a message", &mut plain);
        assert!(plain.is_empty());
    }
}
