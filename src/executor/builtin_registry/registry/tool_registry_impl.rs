//! `impl ToolRegistry for BuiltinToolRegistry`.
//!
//! Holds the trait accessor methods plus `execute_tool`, the registry's
//! central tool-dispatch `match`. `execute_tool` is left whole: it is the
//! single `match tool_name` over every builtin, and a trait impl block
//! cannot be split across files, so this method is an intentionally
//! indivisible unit (see the module-split notes).
#![allow(unused_imports)]

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::pin::Pin;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::error::{AlephError, Result};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::context::GatewayContext;
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;
use tokio::sync::RwLock;

use super::super::ToolRegistry;
use super::BuiltinToolRegistry;

impl ToolRegistry for BuiltinToolRegistry {
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool> {
        self.tools.get(name)
    }

    fn workspace_handle(&self) -> Option<Arc<RwLock<String>>> {
        self.memory_workspace_handle.clone()
    }

    fn smart_recall_config_handle(
        &self,
    ) -> Option<Arc<RwLock<HashMap<String, crate::config::types::profile::SmartRecallConfig>>>>
    {
        self.memory_search_tool
            .as_ref()
            .map(|t| t.smart_recall_config_handle())
    }

    fn session_context_handle(
        &self,
    ) -> Option<Arc<RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        self.session_context_handle.clone()
    }

    fn session_key_handle(&self) -> Option<Arc<RwLock<String>>> {
        self.memory_session_key_handle.clone()
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        debug!(tool = tool_name, "Executing builtin tool");

        // Per-agent tool filtering happens before dispatch reaches here:
        // AgentProfile whitelist at registry build (run_loop) and the
        // src/tools/scoped/ enforcement point. (A parallel ToolPolicy rail
        // that never had a writer was withdrawn — R10 YAGNI.)

        // Use AlephTool::call_json directly for migrated tools
        // This simplifies the code by avoiding intermediate execute_* methods
        match tool_name {
            // Core tools - use call_json directly via AlephTool trait
            "search" => Box::pin(async move { self.search_tool.call_json(arguments).await }),
            "web_fetch" => Box::pin(async move { self.web_fetch_tool.call_json(arguments).await }),
            "file_ops" => Box::pin(async move { self.file_ops_tool.call_json(arguments).await }),
            "file_read" => Box::pin(async move { self.file_read_tool.call_json(arguments).await }),
            "file_write" => {
                Box::pin(async move { self.file_write_tool.call_json(arguments).await })
            }
            "file_edit" => Box::pin(async move { self.file_edit_tool.call_json(arguments).await }),
            "apply_patch" => {
                Box::pin(async move { self.apply_patch_tool.call_json(arguments).await })
            }
            "bash" => Box::pin(async move { self.bash_tool.call_json(arguments).await }),
            "code_exec" => Box::pin(async move { self.code_exec_tool.call_json(arguments).await }),
            "code_check" => {
                Box::pin(async move { self.code_check_tool.call_json(arguments).await })
            }
            "pdf_generate" => {
                Box::pin(async move { self.pdf_generate_tool.call_json(arguments).await })
            }

            // Generation tools - image uses AlephTool, video/audio use legacy execute_* methods
            "image_generate" => Box::pin(async move {
                let tool = self.image_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Image generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "video_generate" => Box::pin(async move {
                let tool = self.video_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Video generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "audio_generate" => Box::pin(async move {
                let tool = self.audio_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Audio generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "speech_generate" => Box::pin(async move {
                let tool = self.speech_generate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "Speech generation not available: no generation registry configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),

            // Self-management tools
            // Project-mode (round 3): resolve skills against the active
            // project's `.aleph/skills` / `.claude/skills` (walked up to the
            // git root) on top of the global dirs. `current_project_root()`
            // is the per-run task-local published by `run_agent_loop`; it is
            // `None` outside project mode, in which case discovery falls back
            // to `with_auto_discover(None)` — byte-for-byte the pre-round-3
            // behaviour, so non-project runs are unaffected.
            "skill_list" => {
                let project = crate::projects::current_project_root();
                Box::pin(async move {
                    crate::builtin_tools::skill_reader::ListSkillsTool::with_auto_discover(
                        project.as_deref(),
                    )
                    .call_json(arguments)
                    .await
                })
            }
            "skill_read" => {
                let project = crate::projects::current_project_root();
                Box::pin(async move {
                    crate::builtin_tools::skill_reader::ReadSkillTool::with_auto_discover(
                        project.as_deref(),
                    )
                    .call_json(arguments)
                    .await
                })
            }
            "read_config_guide" => {
                Box::pin(async move { self.config_guide_tool.call_json(arguments).await })
            }
            "ctx_search" => {
                Box::pin(async move { self.ctx_search_tool.call_json(arguments).await })
            }
            "recall_events" => {
                Box::pin(async move { self.recall_events_tool.call_json(arguments).await })
            }
            "self_manage" => {
                Box::pin(async move { self.self_manage_tool.call_json(arguments).await })
            }
            "hooks_manage" => {
                Box::pin(async move { self.hooks_manage_tool.call_json(arguments).await })
            }
            "self_config" => {
                Box::pin(async move { self.self_config_tool.call_json(arguments).await })
            }
            "moa" => Box::pin(async move { self.moa_manage_tool.call_json(arguments).await }),
            "list_models" => {
                Box::pin(async move { self.list_models_tool.call_json(arguments).await })
            }
            "vault_store" => Box::pin(async move {
                let tool = self.vault_store_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("vault_store not available: no SharedTokenManager configured")
                })?;
                tool.call_json(arguments).await
            }),
            "hub_catalog_search" => Box::pin(async move {
                let tool = self
                    .hub_catalog_search_tool
                    .as_ref()
                    .ok_or_else(|| AlephError::tool("hub_catalog_search not configured"))?;
                tool.call_json(arguments).await
            }),
            "hub_catalog_sync" => Box::pin(async move {
                let tool = self
                    .hub_catalog_sync_tool
                    .as_ref()
                    .ok_or_else(|| AlephError::tool("hub_catalog_sync not configured"))?;
                tool.call_json(arguments).await
            }),
            "hub_resolve_spec" => Box::pin(async move {
                let tool = self
                    .hub_resolve_spec_tool
                    .as_ref()
                    .ok_or_else(|| AlephError::tool("hub_resolve_spec not configured"))?;
                tool.call_json(arguments).await
            }),
            "hub_install_run" => Box::pin(async move {
                let tool = self
                    .hub_install_run_tool
                    .as_ref()
                    .ok_or_else(|| AlephError::tool("hub_install_run not configured"))?;
                tool.call_json(arguments).await
            }),
            "hub_install_verify" => Box::pin(async move {
                let tool = self
                    .hub_install_verify_tool
                    .as_ref()
                    .ok_or_else(|| AlephError::tool("hub_install_verify not configured"))?;
                tool.call_json(arguments).await
            }),
            "hub_fetch_docs" => {
                Box::pin(async move { self.hub_fetch_docs_tool.call_json(arguments).await })
            }
            "desktop" => Box::pin(async move { self.desktop_tool.call_json(arguments).await }),
            "desktop_ax_query_focused" => Box::pin(async move {
                self.desktop_ax_query_focused_tool
                    .call_json(arguments)
                    .await
            }),
            "desktop_ax_query_tree" => {
                Box::pin(async move { self.desktop_ax_query_tree_tool.call_json(arguments).await })
            }
            "desktop_ax_query_by_role" => Box::pin(async move {
                self.desktop_ax_query_by_role_tool
                    .call_json(arguments)
                    .await
            }),
            "desktop_ax_snapshot" => {
                Box::pin(async move { self.desktop_ax_snapshot_tool.call_json(arguments).await })
            }
            "desktop_som" => {
                Box::pin(async move { self.desktop_som_tool.call_json(arguments).await })
            }
            "desktop_gui_locate" => {
                Box::pin(async move { self.desktop_gui_locate_tool.call_json(arguments).await })
            }
            "desktop_check_permissions" => Box::pin(async move {
                self.desktop_check_permissions_tool
                    .call_json(arguments)
                    .await
            }),
            "pim" => Box::pin(async move { self.pim_tool.call_json(arguments).await }),
            "system" => Box::pin(async move { self.system_tool.call_json(arguments).await }),
            "automation" => {
                Box::pin(async move { self.automation_tool.call_json(arguments).await })
            }
            "permission" => {
                Box::pin(async move { self.permission_tool.call_json(arguments).await })
            }
            "media" => Box::pin(async move { self.media_tool.call_json(arguments).await }),
            "scratchpad" => {
                Box::pin(async move { self.scratchpad_tool.call_json(arguments).await })
            }
            "goal" => Box::pin(async move { self.goal_tool.call_json(arguments).await }),
            "loop_graph" => {
                // Same delivery plumbing `cron_manage` gets, for the same
                // reason: `enable_audit` / `pair` INSTALL cron jobs, and those
                // jobs' whole point is to report a governance verdict back to
                // the user (AUDIT_TEMPLATE step 7 / WATCH_TEMPLATE_FOOTER).
                // Without a channel they run, decide, and deliver nowhere.
                // Prefer the race-free per-turn context over the process-global
                // mirror, which a concurrent run can swap mid-turn.
                let arguments = {
                    let mut args = arguments;
                    let (channel, conversation_id) =
                        match crate::tools::turn_context::current_turn_context() {
                            Some(t) => {
                                (Some(t.channel_id.clone()), Some(t.conversation_id.clone()))
                            }
                            None => self
                                .session_context_handle
                                .as_ref()
                                .and_then(|h| h.try_read().ok())
                                .map(|ctx| {
                                    (Some(ctx.channel.clone()), Some(ctx.conversation_id.clone()))
                                })
                                .unwrap_or((None, None)),
                        };
                    // Only inject a route that names something. `TurnContext`
                    // carries `channel_id`/`conversation_id` as `String`, and
                    // they are EMPTY for exactly the turns that have no channel
                    // — a governance or steward cron, a heartbeat probe, a
                    // webhook turn, a `tools.invoke` call. Wrapping those in
                    // `Some("")` is not "no route", it is a route that fails
                    // every `is_some()` test the delivery side runs: the
                    // installed job would be stamped
                    // `source_channel_id = Some("")`, which makes
                    // `cron::executor`'s `approval_is_routable` true, so
                    // `UNATTENDED_KEY` is never set and the weekly audit run is
                    // treated as attended; and `build_cron_prompt` then tells
                    // the auditor "the runtime will deliver it to the user
                    // automatically. Do NOT call any messaging tool" — taking
                    // away the model's own fallback in favour of a delivery
                    // that cannot happen. `None` is the honest value and every
                    // one of those paths already handles it.
                    if let Some(obj) = args.as_object_mut() {
                        if let Some(channel) = channel.filter(|c| !c.is_empty()) {
                            obj.insert("__channel".into(), serde_json::Value::String(channel));
                        }
                        if let Some(conversation_id) = conversation_id.filter(|c| !c.is_empty()) {
                            obj.insert(
                                "__conversation_id".into(),
                                serde_json::Value::String(conversation_id),
                            );
                        }
                    }
                    args
                };
                Box::pin(async move { self.loop_graph_tool.call_json(arguments).await })
            }
            "loop" => Box::pin(async move { self.loop_tool.call_json(arguments).await }),
            "strategy" => Box::pin(async move { self.strategy_tool.call_json(arguments).await }),

            // Memory tools - search and browse personal memory
            "memory_search" => Box::pin(async move {
                let tool = self.memory_search_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_search not available: no memory backend or embedding provider configured")
                })?;
                tool.call_json(arguments).await
            }),
            "memory_browse" => Box::pin(async move {
                let tool = self.memory_browse_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_browse not available: no memory backend configured")
                })?;
                tool.call_json(arguments).await
            }),
            "memory_explore" => Box::pin(async move {
                let tool = self.memory_explore_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_explore not available: no memory backend or embedding provider configured")
                })?;
                tool.call_json(arguments).await
            }),
            "memory_timeline" => Box::pin(async move {
                let tool = self.memory_timeline_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("memory_timeline not available: no event store configured")
                })?;
                tool.call_json(arguments).await
            }),
            // Corrections are filed per PARTITION and read back per partition,
            // so the identity has to be this turn's composed one — not the bare
            // persona, and not the one the registry was built with at boot.
            //
            // Reading the persona filed every user's verbatim correction into
            // the ORG partition (`main`), where `memory.list_corrections` —
            // member-open, Panel-rendered, and defaulting to `main` — handed it
            // to everyone: `partition_visible("main")` has no suffix to split,
            // so the predicate that was installed to stop exactly this is
            // structurally incapable of firing on that id. The night's
            // `feedback_distill` then promoted it into an always-on standing
            // directive in every user's prompt.
            //
            // The READ side staying org-wide is a deliberate ruling
            // (`project_scope`'s feedback-floor note); the fix is the writer, so
            // that org-wide standing rules come from the org tier rather than
            // from whoever typed last.
            "flag_user_correction" => {
                let agent_id = self.caller_memory_partition("main");
                Box::pin(async move {
                    let db = self.flag_user_correction_db.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "flag_user_correction not available: no memory backend configured",
                        )
                    })?;
                    Self::build_flag_user_correction(db, agent_id)
                        .call_json(arguments)
                        .await
                })
            }

            // Governance audit reality probe — read-only counts (recent user
            // corrections + dreaming activity) straight from the memory backend,
            // the in-core replacement for the loop-governance sqlite probes.
            // Shares the memory_trace_db handle (both gate on config.memory_db).
            "governance_metrics" => Box::pin(async move {
                let db = self.memory_trace_db.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "governance_metrics not available: no memory backend configured",
                    )
                })?;
                let tool = crate::builtin_tools::governance_metrics::GovernanceMetricsTool::new(
                    db.clone(),
                );
                tool.call_json(arguments).await
            }),

            // Curated hot memory write tool — resolves a per-agent
            // CuratedMemoryStore via MemoryContextProvider at call time
            // (mirrors the per-call construction pattern used by session_search).
            // Passes the BASE caller agent id — `get_or_load_curated_store`
            // resolves it through session scope itself (P1 curated per-scope
            // instancing — see `thinker::memory_context_provider::curated`).
            // Do NOT pre-resolve here: this crate's `session_write_id` is not
            // idempotent under re-application — feeding it an already-composed
            // id under an active personal scope would double-compose.
            "remember" => Box::pin(async move {
                let mcp = self.memory_context_provider.get().ok_or_else(|| {
                    AlephError::tool(
                        "remember not available: MemoryContextProvider not yet injected",
                    )
                })?;
                let base_agent = self.caller_agent_id("main");
                let store = mcp
                    .get_or_load_curated_store(&base_agent)
                    .await
                    .map_err(|e| AlephError::tool(format!("remember: {e}")))?;
                // The write-decision audit log shares the memory_trace_db handle
                // (both gate on config.memory_db, and `memory_trace` is what
                // reads these rows back). `None` before/without a memory
                // backend: the audit is a NO-OP there, never a write failure.
                let tool = crate::builtin_tools::RememberTool::new(store)
                    .with_decision_log(self.memory_trace_db.clone());
                tool.call_json(arguments).await
            }),

            // Cluster discovery tool — read-only projection of the same
            // injected NodeRegistry the invoke tools dispatch through.
            "node_list" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_list not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeListTool::new(reg.clone());
                tool.call_json(arguments).await
            }),

            // Cluster fan-out tool — resolves the gateway's NodeRegistry
            // (injected at boot via set_node_registry) and dispatches the
            // command to a connected node over reverse RPC (cluster Phase 0c).
            "node_invoke" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_invoke not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeInvokeTool::new(reg.clone());
                tool.call_json(arguments).await
            }),

            // Cluster file-transfer tool — same injected NodeRegistry as node_invoke.
            "node_file" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_file not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeFileTool::new(reg.clone());
                tool.call_json(arguments).await
            }),

            // Cluster tag fan-out — same injected NodeRegistry as node_invoke.
            "node_invoke_many" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_invoke_many not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeInvokeManyTool::new(reg.clone());
                tool.call_json(arguments).await
            }),

            // Cluster membership tool — the only cluster tool that also needs
            // the SecurityStore (the `role=node` device records are what makes a
            // deregister stick).
            "node_manage" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_manage not available: NodeRegistry not injected")
                })?;
                let store = self.node_security_store.get().ok_or_else(|| {
                    AlephError::tool("node_manage not available: SecurityStore not injected")
                })?;
                let tool = crate::builtin_tools::NodeManageTool::new(reg.clone(), store.clone());
                tool.call_json(arguments).await
            }),

            // Sessions tools for cross-session communication.
            // Caller identity is derived from session context (fallback "main")
            // so A2A policy filtering applies to the actual calling agent —
            // a hardcoded "main" would evaluate every caller as the most
            // privileged agent (same pattern as session_search below).
            "session_list" => Box::pin(async move {
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool("session_list not available: GatewayContext not yet injected")
                })?;
                let caller_id = self.caller_agent_id("main");
                let tool = SessionsListTool::new(Arc::clone(context), caller_id);
                tool.call_json(arguments).await
            }),
            "session_send" => Box::pin(async move {
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool("session_send not available: GatewayContext not yet injected")
                })?;
                let caller_id = self.caller_agent_id("main");
                let tool = SessionsSendTool::with_context((**context).clone(), caller_id);
                tool.call_json(arguments).await
            }),

            // Session search tool — uses GatewayContext for A2A policy filtering
            "session_search" => Box::pin(async move {
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool(
                        "session_search not available: GatewayContext not yet injected",
                    )
                })?;
                // Derive caller identity from session context. Falls back to
                // "main" when the handle is missing or the key fails to parse.
                let caller_id = self.caller_agent_id("main");
                // Assembler from MCP (None if not yet injected → call_impl falls back to raw FTS5).
                let assembler = self
                    .memory_context_provider
                    .get()
                    .map(|mcp| mcp.assembler());
                // SummarySynthesizer from SessionEndSummarizer global cell (None if no AiProvider).
                let synthesizer = crate::thinker::memory_context_provider::session_end_summarizer()
                    .map(|s| s.synthesizer.clone());
                let tool = crate::builtin_tools::SessionSearchTool::new(
                    Arc::clone(context),
                    caller_id,
                    assembler,
                    synthesizer,
                );
                tool.call_json(arguments).await
            }),

            // Browser tools
            "browser_open" => {
                Box::pin(async move { self.browser_open_tool.call_json(arguments).await })
            }
            "browser_click" => {
                Box::pin(async move { self.browser_click_tool.call_json(arguments).await })
            }
            "browser_type" => {
                Box::pin(async move { self.browser_type_tool.call_json(arguments).await })
            }
            "browser_screenshot" => {
                Box::pin(async move { self.browser_screenshot_tool.call_json(arguments).await })
            }
            "browser_snapshot" => {
                Box::pin(async move { self.browser_snapshot_tool.call_json(arguments).await })
            }
            "browser_navigate" => {
                Box::pin(async move { self.browser_navigate_tool.call_json(arguments).await })
            }
            "browser_tabs" => {
                Box::pin(async move { self.browser_tabs_tool.call_json(arguments).await })
            }
            "browser_select" => {
                Box::pin(async move { self.browser_select_tool.call_json(arguments).await })
            }
            "browser_evaluate" => {
                Box::pin(async move { self.browser_evaluate_tool.call_json(arguments).await })
            }
            "browser_fill_form" => {
                Box::pin(async move { self.browser_fill_form_tool.call_json(arguments).await })
            }
            "browser_press_key" => {
                Box::pin(async move { self.browser_press_key_tool.call_json(arguments).await })
            }
            "browser_wait_for" => {
                Box::pin(async move { self.browser_wait_for_tool.call_json(arguments).await })
            }
            "browser_batch" => {
                Box::pin(async move { self.browser_batch_tool.call_json(arguments).await })
            }
            "browser_console" => {
                Box::pin(async move { self.browser_console_tool.call_json(arguments).await })
            }
            "browser_hover" => {
                Box::pin(async move { self.browser_hover_tool.call_json(arguments).await })
            }
            "browser_scroll" => {
                Box::pin(async move { self.browser_scroll_tool.call_json(arguments).await })
            }
            "browser_pdf" => {
                Box::pin(async move { self.browser_pdf_tool.call_json(arguments).await })
            }
            "browser_network" => {
                Box::pin(async move { self.browser_network_tool.call_json(arguments).await })
            }
            "browser_dialog" => {
                Box::pin(async move { self.browser_dialog_tool.call_json(arguments).await })
            }
            "browser_drag" => {
                Box::pin(async move { self.browser_drag_tool.call_json(arguments).await })
            }
            "browser_upload" => {
                Box::pin(async move { self.browser_upload_tool.call_json(arguments).await })
            }
            "browser_resize" => {
                Box::pin(async move { self.browser_resize_tool.call_json(arguments).await })
            }
            "browser_emulate" => {
                Box::pin(async move { self.browser_emulate_tool.call_json(arguments).await })
            }
            "browser_cookies" => {
                Box::pin(async move { self.browser_cookies_tool.call_json(arguments).await })
            }
            "browser_session" => {
                Box::pin(async move { self.browser_session_tool.call_json(arguments).await })
            }
            "browser_profile" => {
                Box::pin(async move { self.browser_profile_tool.call_json(arguments).await })
            }

            // Session new tool — inject session key from session context
            "session_new" => {
                let arguments = {
                    let mut args = arguments;
                    // Prefer the race-free per-turn session key; the process-global
                    // session_context_handle is rewritten at every run start, so a
                    // concurrent run of another agent can swap it mid-turn and this
                    // tool would close/create the WRONG session (same rule
                    // recall_context and memory_search scope=current_session follow).
                    let session_key =
                        crate::tools::turn_context::current_session_key().or_else(|| {
                            self.session_context_handle
                                .as_ref()
                                .and_then(|h| h.try_read().ok())
                                .map(|ctx| ctx.session_key_str.clone())
                        });
                    if let (Some(session_key), Some(obj)) = (session_key, args.as_object_mut()) {
                        obj.insert(
                            "__session_key".into(),
                            serde_json::Value::String(session_key),
                        );
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.session_new_tool.as_ref().ok_or_else(|| {
                        AlephError::tool("session_new not available: no SessionManager configured")
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Session compact tool — inject session key from session context
            "session_compact" => {
                let arguments = {
                    let mut args = arguments;
                    // Prefer the race-free per-turn session key; see session_new above.
                    let session_key =
                        crate::tools::turn_context::current_session_key().or_else(|| {
                            self.session_context_handle
                                .as_ref()
                                .and_then(|h| h.try_read().ok())
                                .map(|ctx| ctx.session_key_str.clone())
                        });
                    if let (Some(session_key), Some(obj)) = (session_key, args.as_object_mut()) {
                        obj.insert(
                            "__session_key".into(),
                            serde_json::Value::String(session_key),
                        );
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.session_compact_tool.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "session_compact not available: no SessionManager configured",
                        )
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Session set-topic tool — inject session key from session context
            "session_rename" => {
                let arguments = {
                    let mut args = arguments;
                    // Prefer the race-free per-turn session key; see session_new above.
                    let session_key =
                        crate::tools::turn_context::current_session_key().or_else(|| {
                            self.session_context_handle
                                .as_ref()
                                .and_then(|h| h.try_read().ok())
                                .map(|ctx| ctx.session_key_str.clone())
                        });
                    if let (Some(session_key), Some(obj)) = (session_key, args.as_object_mut()) {
                        obj.insert(
                            "__session_key".into(),
                            serde_json::Value::String(session_key),
                        );
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.session_set_topic_tool.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "session_rename not available: no SessionManager configured",
                        )
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Session set-mode tool — inject session key from session context
            "session_set_mode" => {
                let arguments = {
                    let mut args = arguments;
                    // Prefer the race-free per-turn session key; see session_new above.
                    let session_key =
                        crate::tools::turn_context::current_session_key().or_else(|| {
                            self.session_context_handle
                                .as_ref()
                                .and_then(|h| h.try_read().ok())
                                .map(|ctx| ctx.session_key_str.clone())
                        });
                    if let (Some(session_key), Some(obj)) = (session_key, args.as_object_mut()) {
                        obj.insert(
                            "__session_key".into(),
                            serde_json::Value::String(session_key),
                        );
                    }
                    args
                };
                Box::pin(async move {
                    let tool = self.session_set_mode_tool.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "session_set_mode not available: no SessionManager configured",
                        )
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Cron management tool — inject session channel + conversation context so
            // created jobs know where to deliver results. Also inject current_time_ms
            // so the LLM has a reliable epoch reference for computing At timestamps.
            "cron_manage" => {
                let arguments = {
                    let mut args = arguments;
                    if let Some(obj) = args.as_object_mut() {
                        obj.insert(
                            "__current_time_ms".into(),
                            serde_json::Value::Number(chrono::Utc::now().timestamp_millis().into()),
                        );
                    }
                    self.inject_delivery_route(&mut args);
                    args
                };
                Box::pin(async move {
                    let tool = self.cron_manage_tool.as_ref().ok_or_else(|| {
                        AlephError::tool("cron_manage not available: cron service not configured")
                    })?;
                    tool.call_json(arguments).await
                })
            }

            // Heartbeat management tools
            "heartbeat_list" => Box::pin(async move {
                let tool = self.heartbeat_list_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_list not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            // Same delivery-route injection as `cron_manage`: a heartbeat with
            // no `delivery_config` runs its L2 turn and then drops the finding,
            // which is every heartbeat ever created before this was wired.
            "heartbeat_create" => {
                let arguments = {
                    let mut args = arguments;
                    self.inject_delivery_route(&mut args);
                    args
                };
                Box::pin(async move {
                    let tool = self.heartbeat_create_tool.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "heartbeat_create not available: heartbeat service not configured",
                        )
                    })?;
                    tool.call_json(arguments).await
                })
            }
            "heartbeat_update" => Box::pin(async move {
                let tool = self.heartbeat_update_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_update not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "heartbeat_delete" => Box::pin(async move {
                let tool = self.heartbeat_delete_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_delete not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "heartbeat_toggle" => Box::pin(async move {
                let tool = self.heartbeat_toggle_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "heartbeat_toggle not available: heartbeat service not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            // Heartbeat report tool — always available (used during L2 heartbeat execution)
            "heartbeat_report" => {
                Box::pin(async move { self.heartbeat_report_tool.call_json(arguments).await })
            }

            // Agent identity + signed operation ledger. Stateless: it reads the
            // process-global ledger installed at boot, so it needs no struct
            // field and no deferred injection — construct per call.
            "agent_identity" => Box::pin(async move {
                crate::builtin_tools::agent_identity::AgentIdentityTool::new()
                    .call_json(arguments)
                    .await
            }),

            // Workspace records (R8). Deliberately OUTSIDE the agent-management
            // arm below: that arm injects `__channel` for the channel→agent
            // binding verbs, and a workspace verb has no channel in it. Folding
            // it in there would also have made it unreachable — the arm is
            // guarded by an explicit name list.
            "workspace_manage" => Box::pin(async move {
                let tool = self.workspace_manage_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("workspace_manage not available: no AgentEnvStore configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Agent management tools — snapshot session context into arguments
            // to avoid race conditions from concurrent reads of the shared handle.
            "agent_create" | "agent_list" | "agent_delete" | "agent_switch" | "agent_unbind"
            | "agent_update" => {
                // Snapshot session context into tool arguments before async execution
                let arguments = {
                    let mut args = arguments;
                    // Prefer the race-free per-turn channel; the process-global
                    // session_context_handle is rewritten at every run start, so a
                    // concurrent run of another agent can swap it mid-turn and
                    // agent_switch would rebind the WRONG channel's active agent.
                    // Fall back to the mirror only outside a scoped turn.
                    let channel = match crate::tools::turn_context::current_turn_context() {
                        Some(t) => Some(t.channel_id.clone()),
                        None => self
                            .session_context_handle
                            .as_ref()
                            .and_then(|h| h.try_read().ok())
                            .map(|ctx| ctx.channel.clone()),
                    };
                    if let (Some(channel), Some(obj)) = (channel, args.as_object_mut()) {
                        obj.insert("__channel".into(), serde_json::Value::String(channel));
                    }
                    args
                };

                match tool_name {
                    "agent_create" => Box::pin(async move {
                        let tool = self.agent_create_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_create not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_list" => Box::pin(async move {
                        let tool = self.agent_list_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_list not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_delete" => Box::pin(async move {
                        let tool = self.agent_delete_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_delete not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_switch" => Box::pin(async move {
                        let tool = self.agent_switch_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_switch not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_unbind" => Box::pin(async move {
                        let tool = self.agent_unbind_tool.as_ref().ok_or_else(|| {
                            AlephError::tool("agent_unbind not available: no AgentRegistry/AgentEnvStore configured")
                        })?;
                        tool.call_json(arguments).await
                    }),
                    "agent_update" => Box::pin(async move {
                        let tool = self.agent_update_tool.as_ref().ok_or_else(|| {
                            AlephError::tool(
                                "agent_update not available: no AgentRegistry configured",
                            )
                        })?;
                        tool.call_json(arguments).await
                    }),
                    _ => {
                        let tool = tool_name.to_string();
                        Box::pin(async move {
                            Err(AlephError::tool(format!(
                                "Agent tool '{tool}' is not yet wired"
                            )))
                        })
                    }
                }
            }
            // agent_info is read-only and always available — no session-context
            // snapshot, no optional-dependency gate.
            "agent_info" => {
                Box::pin(async move { self.agent_info_tool.call_json(arguments).await })
            }

            // Task coordination tools
            "task_create" => Box::pin(async move {
                let tool = self.task_create_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_create not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_update" => Box::pin(async move {
                let tool = self.task_update_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_update not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_list" => Box::pin(async move {
                let tool = self.task_list_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_list not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_wait" => Box::pin(async move {
                let tool = self.task_wait_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_wait not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_comment" => Box::pin(async move {
                let tool = self.task_comment_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_comment not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_acp_member" => Box::pin(async move {
                let tool = self.team_acp_member_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_acp_member not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_workflow_canvas" => Box::pin(async move {
                let tool = self.team_workflow_canvas_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "team_workflow_canvas not available: no CoordTaskStore configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "workflow_step_review" => Box::pin(async move {
                let tool = self.workflow_step_review_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "workflow_step_review not available: no CoordTaskStore configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "workflow" => Box::pin(async move {
                let tool = self.workflow_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("workflow not available: no CoordTaskStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_task_control" => Box::pin(async move {
                let tool = self.team_task_control_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "team_task_control not available: no CoordTaskStore configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "task_exit_journal" => Box::pin(async move {
                let tool = self.task_exit_journal_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "task_exit_journal not available: no CoordTaskStore configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),

            // Team management tools
            "team_create" => Box::pin(async move {
                let tool = self.team_create_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_create not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_delegate" => Box::pin(async move {
                let tool = self.team_delegate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_delegate not available: no TeamStore configured")
                })?;
                // Inject GatewayContext from OnceCell (deferred — same pattern as session_send)
                let context = self.gateway_context.get().ok_or_else(|| {
                    AlephError::tool("team_delegate not available: GatewayContext not yet injected")
                })?;
                let mut delegate = tool.clone();
                delegate.set_context((**context).clone());
                delegate.call_json(arguments).await
            }),
            "team_status" => Box::pin(async move {
                let tool = self.team_status_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_status not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_disband" => Box::pin(async move {
                let tool = self.team_disband_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_disband not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_set_protocol" => Box::pin(async move {
                let tool = self.team_set_protocol_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_set_protocol not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_member_add" => Box::pin(async move {
                let tool = self.team_member_add_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_member_add not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_member_remove" => Box::pin(async move {
                let tool = self.team_member_remove_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_member_remove not available: no TeamStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_digest" => Box::pin(async move {
                let tool = self.team_digest_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_digest not available: no EventLogStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "team_from_template" => Box::pin(async move {
                let tool = self.team_from_template_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "team_from_template not available: TeamStore + CoordTaskStore required",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "team_snapshot" => Box::pin(async move {
                let tool = self.team_snapshot_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "team_snapshot not available: TeamStore + CoordTaskStore + SqliteSnapshotStore required",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "team_usage" => Box::pin(async move {
                let tool = self.team_usage_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("team_usage not available: TeamStore + StateDatabase required")
                })?;
                tool.call_json(arguments).await
            }),
            // Team messaging tools
            "message_send" => Box::pin(async move {
                let tool = self.message_send_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("message_send not available: no MessageRouter configured")
                })?;
                tool.call_json(arguments).await
            }),
            "inbox_read" => Box::pin(async move {
                let tool = self.inbox_read_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("inbox_read not available: no Inbox configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Plan approval tools
            "plan_submit" => Box::pin(async move {
                let tool = self.plan_submit_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("plan_submit not available: plan approval not configured")
                })?;
                tool.call_json(arguments).await
            }),
            "plan_resolve" => Box::pin(async move {
                let tool = self.plan_resolve_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("plan_resolve not available: plan approval not configured")
                })?;
                tool.call_json(arguments).await
            }),

            // Worker lifecycle tools
            "lifecycle_idle" => Box::pin(async move {
                let tool = self.lifecycle_idle_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "lifecycle_idle not available: MessageRouter / TeamStore not configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "lifecycle_request_shutdown" => Box::pin(async move {
                let tool = self
                    .lifecycle_request_shutdown_tool
                    .as_ref()
                    .ok_or_else(|| {
                        AlephError::tool(
                            "lifecycle_request_shutdown not available: MessageRouter / TeamStore not configured",
                        )
                    })?;
                tool.call_json(arguments).await
            }),
            "lifecycle_resolve_shutdown" => Box::pin(async move {
                let tool = self
                    .lifecycle_resolve_shutdown_tool
                    .as_ref()
                    .ok_or_else(|| {
                        AlephError::tool(
                            "lifecycle_resolve_shutdown not available: MessageRouter not configured",
                        )
                    })?;
                tool.call_json(arguments).await
            }),

            // Collaborative session tools
            "session_collaborate" => Box::pin(async move {
                let tool = self.session_collaborate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "session_collaborate not available: no SessionCoordinator configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "session_turn" => Box::pin(async move {
                let tool = self.session_turn_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("session_turn not available: no SessionCoordinator configured")
                })?;
                tool.call_json(arguments).await
            }),
            "session_read" => Box::pin(async move {
                let tool = self.session_read_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("session_read not available: no SessionStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            // Task artifact tools
            "task_submit" => Box::pin(async move {
                let tool = self.task_submit_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("task_submit not available: no ArtifactStore configured")
                })?;
                tool.call_json(arguments).await
            }),
            "task_read_artifact" => Box::pin(async move {
                let tool = self.task_read_artifact_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "task_read_artifact not available: no ArtifactStore configured",
                    )
                })?;
                tool.call_json(arguments).await
            }),
            "task_review" => Box::pin(async move {
                let tool = self.task_review_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "task_review not available: requires CoordTaskStore + TeamStore",
                    )
                })?;
                tool.call_json(arguments).await
            }),

            // Channel pairing tool (deferred — ChannelRegistry injected after construction)
            "channel_pairing" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "channel_pairing not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool =
                    crate::builtin_tools::channel_manage::ChannelPairingTool::new(Arc::clone(cr));
                tool.call_json(arguments).await
            }),

            // Channel message tool (deferred — ChannelRegistry injected after construction)
            "channel_message" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "channel_message not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool =
                    crate::builtin_tools::channel_message::ChannelMessageTool::new(Arc::clone(cr));
                tool.call_json(arguments).await
            }),

            // Channel directory tool (deferred — same ChannelRegistry cell)
            "channel_directory" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "channel_directory not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool = crate::builtin_tools::channel_directory::ChannelDirectoryTool::new(
                    Arc::clone(cr),
                );
                tool.call_json(arguments).await
            }),

            // Channel outbox tool (deferred — same ChannelRegistry cell)
            "channel_outbox" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "channel_outbox not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool =
                    crate::builtin_tools::channel_outbox::ChannelOutboxTool::new(Arc::clone(cr));
                tool.call_json(arguments).await
            }),

            // Google Meet tool — forwards the action to the configured
            // out-of-core transport bridge over JSON-RPC.
            "google_meet" => {
                let tool = self.google_meet_tool.clone();
                Box::pin(async move { tool.call_json(arguments).await })
            }

            // ask_user clarification tool (deferred — ChannelRegistry +
            // ClarificationManager injected after construction)
            "ask_user" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool("ask_user not available: ChannelRegistry not yet injected")
                })?;
                let cm = self.clarification_manager_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "ask_user not available: ClarificationManager not yet injected",
                    )
                })?;
                let tool = crate::builtin_tools::ask_user::AskUserTool::new(
                    Arc::clone(cm),
                    Arc::clone(cr),
                );
                tool.call_json(arguments).await
            }),

            // Voice mode tool (deferred — ChannelRegistry injected after construction)
            "voice_mode_set" => Box::pin(async move {
                let cr = self.channel_registry_cell.get().ok_or_else(|| {
                    AlephError::tool(
                        "voice_mode_set not available: ChannelRegistry not yet injected",
                    )
                })?;
                let tool = crate::builtin_tools::voice_tools::VoiceModeSetTool::new(Arc::clone(cr));
                tool.call_json(arguments).await
            }),

            // Local voice endpoint tool — status probe (R8). Needs the live
            // Config handle (same source as `config_audit`).
            "local_voice" => Box::pin(async move {
                let cfg = self.config.as_ref().ok_or_else(|| {
                    AlephError::tool("local_voice not available: no Config handle configured")
                })?;
                let tool = crate::builtin_tools::voice_tools::LocalVoiceTool::new(Arc::clone(cfg));
                tool.call_json(arguments).await
            }),

            "gateway_route" => {
                Box::pin(async move { self.gateway_route_tool.call_json(arguments).await })
            }

            // Media send tool — no dependencies, always available
            "media_send" => Box::pin(async move {
                crate::builtin_tools::media_send::MediaSendTool::new()
                    .call_json(arguments)
                    .await
            }),

            // Deliverable publisher — reads the session from the turn context,
            // so it needs nothing injected here either.
            "artifact_publish" => Box::pin(async move {
                crate::builtin_tools::artifact_publish::ArtifactPublishTool::new()
                    .call_json(arguments)
                    .await
            }),

            // ACP delegate tool (unified)
            "acp_delegate" => Box::pin(async move {
                let tool = self.acp_delegate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_delegate not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),
            "acp_switch" => Box::pin(async move {
                let tool = self.acp_switch_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_switch not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),
            "acp_session_control" => Box::pin(async move {
                let tool = self.acp_session_control_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("acp_session_control not available: ACP not configured")
                })?;
                tool.call_json(arguments).await
            }),

            // A2A outbound delegation tools (unified)
            "a2a_delegate" => Box::pin(async move {
                let tool = self.a2a_delegate_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("a2a_delegate not available: A2A subsystem not enabled")
                })?;
                tool.call_json(arguments).await
            }),
            "a2a_agents" => Box::pin(async move {
                let tool = self.a2a_agents_tool.as_ref().ok_or_else(|| {
                    AlephError::tool("a2a_agents not available: A2A subsystem not enabled")
                })?;
                tool.call_json(arguments).await
            }),

            // Skill management tools
            "skill_status" => {
                Box::pin(async move { self.skill_status_tool.call_json(arguments).await })
            }
            "skill_install" => {
                Box::pin(async move { self.skill_install_tool.call_json(arguments).await })
            }
            "skill_manage" => {
                Box::pin(async move { self.skill_manage_tool.call_json(arguments).await })
            }
            "note_manage" => {
                if let Some(ref tool) = self.note_manage_tool {
                    let tool = tool.clone();
                    Box::pin(async move { tool.call_json(arguments).await })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "note_manage tool is not available: memory backend not configured",
                        ))
                    })
                }
            }

            "session_complete" => {
                if let Some(ref tool) = self.session_complete_tool {
                    let tool = tool.clone();
                    Box::pin(async move { tool.call_json(arguments).await })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "session_complete tool is not available: memory backend not configured",
                        ))
                    })
                }
            }

            "memory_reflect" => {
                if let Some(ref tool) = self.memory_reflect_tool {
                    let tool = tool.clone();
                    Box::pin(async move { tool.call_json(arguments).await })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "memory_reflect tool is not available: MemoryReflector not wired (server builder needs to inject it)",
                        ))
                    })
                }
            }

            // Wiki orientation tools (Spec 5)
            "note_orient" => {
                // The caller's memory PARTITION, not the bare persona — the
                // note writers compose the session scope in, so a reader that
                // does not compose reads an empty `main` (see
                // `caller_memory_partition`).
                let agent_id = self.caller_memory_partition("default");
                if let Some(ref tool) = self.note_orient_tool {
                    let tool = tool.clone();
                    Box::pin(async move {
                        let args: crate::builtin_tools::note_orient::NoteOrientArgs =
                            serde_json::from_value(arguments).map_err(|e| {
                                AlephError::tool(format!("note_orient: bad args: {e}"))
                            })?;
                        let out = tool.call(&agent_id, args).await?;
                        serde_json::to_value(out)
                            .map_err(|e| AlephError::tool(format!("note_orient: serialize: {e}")))
                    })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "note_orient not available: NoteOrientation not wired at startup",
                        ))
                    })
                }
            }

            "note_schema" => {
                let agent_id = self.caller_memory_partition("default");
                if let Some(ref tool) = self.note_schema_tool {
                    let tool = tool.clone();
                    Box::pin(async move {
                        let args: crate::builtin_tools::note_schema::NoteSchemaArgs =
                            serde_json::from_value(arguments).map_err(|e| {
                                AlephError::tool(format!("note_schema: bad args: {e}"))
                            })?;
                        let out = tool.call(&agent_id, args).await?;
                        serde_json::to_value(out)
                            .map_err(|e| AlephError::tool(format!("note_schema: serialize: {e}")))
                    })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "note_schema not available: note_memory_dir not configured at startup",
                        ))
                    })
                }
            }

            // User profile tool (Spec 7 Task 9)
            "user_profile" => {
                // The one memory reader that must NOT fall back to the room's
                // partition: a room holds more than one human, so there is no
                // single profile to answer with. `profile_floor_id` returns
                // `None` there on purpose (`project_scope`'s doc), and the
                // honest answer is "no such thing here" rather than the room's
                // merged USER.md.
                let Some(agent_id) = self.caller_profile_partition("default") else {
                    return Box::pin(async move {
                        Err(AlephError::tool(
                            "user_profile is not available inside a project room: a room has more \
                             than one person in it, so there is no single profile to read. Ask in \
                             a personal session.",
                        ))
                    });
                };
                if let Some(ref tool) = self.user_profile_tool {
                    let tool = tool.clone();
                    Box::pin(async move {
                        let args: crate::builtin_tools::user_profile::UserProfileArgs =
                            serde_json::from_value(arguments).map_err(|e| {
                                AlephError::tool(format!("user_profile: bad args: {e}"))
                            })?;
                        let out = tool.call(&agent_id, args).await?;
                        serde_json::to_value(out)
                            .map_err(|e| AlephError::tool(format!("user_profile: serialize: {e}")))
                    })
                } else {
                    Box::pin(async move {
                        Err(AlephError::tool(
                            "user_profile not available: ProfileSynthesizer not wired at startup",
                        ))
                    })
                }
            }

            // Self-diagnosis / model-switch / config-audit tools.
            // These are listed in BUILTIN_TOOL_DEFINITIONS (hence advertised to
            // the LLM) but were never wired into dispatch, so a call fell through
            // to `_ =>` and returned "Unknown tool". select_model/doctor are
            // dependency-free unit structs; config_audit needs the live Config
            // handle (same source as `create_tool_boxed`). (logic-audit fix)
            "select_model" => Box::pin(async move {
                crate::builtin_tools::SelectModelTool
                    .call_json(arguments)
                    .await
            }),
            "doctor" => Box::pin(async move { self.doctor_tool.call_json(arguments).await }),
            "config_audit" => Box::pin(async move {
                let cfg = self.config.as_ref().ok_or_else(|| {
                    AlephError::tool("config_audit not available: no Config handle configured")
                })?;
                crate::builtin_tools::ConfigAuditTool::new(Arc::clone(cfg))
                    .call_json(arguments)
                    .await
            }),

            // Media understanding tools — require a MediaPipeline. Advertised via
            // BUILTIN_TOOL_DEFINITIONS but previously undispatchable in the loop.
            "media_understand" => Box::pin(async move {
                let mp = self.media_pipeline.as_ref().ok_or_else(|| {
                    AlephError::tool("media_understand not available: no media pipeline configured")
                })?;
                crate::builtin_tools::media_tools::MediaUnderstandTool::new(Arc::clone(mp))
                    .call_json(arguments)
                    .await
            }),
            "audio_transcribe" => Box::pin(async move {
                let mp = self.media_pipeline.as_ref().ok_or_else(|| {
                    AlephError::tool("audio_transcribe not available: no media pipeline configured")
                })?;
                crate::builtin_tools::media_tools::AudioTranscribeTool::new(Arc::clone(mp))
                    .call_json(arguments)
                    .await
            }),
            "document_extract" => Box::pin(async move {
                let mp = self.media_pipeline.as_ref().ok_or_else(|| {
                    AlephError::tool("document_extract not available: no media pipeline configured")
                })?;
                crate::builtin_tools::media_tools::DocumentExtractTool::new(Arc::clone(mp))
                    .call_json(arguments)
                    .await
            }),

            // Pre-compression context recovery — needs a memory backend plus the
            // active session id (resolved from the per-task turn context,
            // matching the session_key the compaction pipeline writes raw
            // chunks under). RecallContextTool predates AlephTool, so dispatch
            // via call_impl.
            "recall_context" => {
                // Per-task turn context first (race-free), then the
                // process-global session context mirror: the handle is
                // rewritten at every run start, so a concurrent run of
                // another agent can swap the session mid-turn and split it
                // from the agent id resolved below. Taking both from the same
                // TurnContext keeps the (agent, session) pair coherent — the
                // same rule memory_search scope=current_session follows.
                let session_id = crate::tools::turn_context::current_session_key().or_else(|| {
                    self.session_context_handle
                        .as_ref()
                        .and_then(|h| h.try_read().ok())
                        .map(|ctx| ctx.session_key_str.clone())
                });
                // Resolve the same (optionally project-scoped) agent id the
                // compaction pipeline writes raw chunks under. This arm was the
                // only one in this file that composed; it now shares the one
                // resolver with the six that did not.
                let agent_id = self.caller_memory_partition("default");
                Box::pin(async move {
                    let db = self.recall_context_db.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "recall_context not available: no memory backend configured",
                        )
                    })?;
                    let session_id = session_id.ok_or_else(|| {
                        AlephError::tool("recall_context not available: no active session context")
                    })?;
                    let args: crate::builtin_tools::recall_context::RecallContextArgs =
                        serde_json::from_value(arguments).map_err(|e| {
                            AlephError::tool(format!("recall_context: bad args: {e}"))
                        })?;
                    let tool = crate::builtin_tools::RecallContextTool::new(
                        db.clone(),
                        session_id,
                        agent_id,
                    );
                    let out = tool
                        .call_impl(args)
                        .await
                        .map_err(|e| AlephError::tool(format!("recall_context: {e}")))?;
                    serde_json::to_value(out)
                        .map_err(|e| AlephError::tool(format!("recall_context: serialize: {e}")))
                })
            }

            // Evidence-chain walk: profile section / note / raw id
            // → source notes → raw memories → original transcript text.
            "memory_trace" => {
                // `memory_trace`'s own DESCRIPTION promises one row per
                // `remember` / `flag_user_correction` write ATTEMPT, and tells
                // the model to answer "why didn't you remember that?" from
                // these rows rather than from recollection. Those rows are
                // written under the composed partition; reading the bare
                // persona answered "there are none" for every scoped run.
                let agent_id = self.caller_memory_partition("default");
                Box::pin(async move {
                    let db = self.memory_trace_db.as_ref().ok_or_else(|| {
                        AlephError::tool("memory_trace not available: no memory backend configured")
                    })?;
                    let note_memory_dir = crate::utils::paths::get_note_memory_dir()
                        .map_err(|e| AlephError::tool(format!("memory_trace: note dir: {e}")))?;
                    let args: crate::builtin_tools::memory_trace::MemoryTraceArgs =
                        serde_json::from_value(arguments).map_err(|e| {
                            AlephError::tool(format!("memory_trace: bad args: {e}"))
                        })?;
                    let tool = crate::builtin_tools::memory_trace::MemoryTraceTool::new(
                        db.clone(),
                        agent_id,
                        note_memory_dir,
                    );
                    let out = tool
                        .call_impl(args)
                        .await
                        .map_err(|e| AlephError::tool(format!("memory_trace: {e}")))?;
                    serde_json::to_value(out)
                        .map_err(|e| AlephError::tool(format!("memory_trace: serialize: {e}")))
                })
            }

            // Read-only knowledge-graph interrogation: schema / neighbors /
            // community / related over the note graph.
            "note_graph_query" => {
                let agent_id = self.caller_memory_partition("default");
                Box::pin(async move {
                    let db = self.memory_trace_db.as_ref().ok_or_else(|| {
                        AlephError::tool(
                            "note_graph_query not available: no memory backend configured",
                        )
                    })?;
                    let args: crate::builtin_tools::note_graph_query::NoteGraphQueryArgs =
                        serde_json::from_value(arguments).map_err(|e| {
                            AlephError::tool(format!("note_graph_query: bad args: {e}"))
                        })?;
                    let tool = crate::builtin_tools::note_graph_query::NoteGraphQueryTool::new(
                        db.clone(),
                        agent_id,
                    );
                    let out = tool
                        .call_impl(args)
                        .await
                        .map_err(|e| AlephError::tool(format!("note_graph_query: {e}")))?;
                    serde_json::to_value(out)
                        .map_err(|e| AlephError::tool(format!("note_graph_query: serialize: {e}")))
                })
            }

            _ => {
                if let Some((plugin_id, handler)) = self.resolve_plugin_handler(tool_name) {
                    let ext_mgr = self.extension_manager.clone();
                    return Box::pin(async move {
                        let ext_mgr = ext_mgr.ok_or_else(|| {
                            AlephError::tool(
                                "Plugin tool execution unavailable: extension manager not configured",
                            )
                        })?;
                        info!(plugin = %plugin_id, tool = %handler, "Executing plugin tool");
                        ext_mgr
                            .call_plugin_tool(&plugin_id, &handler, arguments)
                            .await
                            .map_err(|e| {
                                AlephError::tool(format!("Plugin tool '{handler}' failed: {e}"))
                            })
                    });
                }
                let tool = tool_name.to_string();
                error!(tool = %tool, "Unknown tool requested");
                Box::pin(async move { Err(AlephError::tool(format!("Unknown tool: {tool}"))) })
            }
        }
    }
}

#[cfg(test)]
mod recall_context_identity_tests {
    use super::*;
    use crate::builtin_tools::agent_manage::SessionContext;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    fn turn_ctx(agent: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main(agent),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
        }
    }

    /// Regression (agent/session identity split): the recall_context arm used
    /// to take the session from the process-global `session_context_handle`
    /// while the agent came from the per-task turn context — a concurrent run
    /// rewriting the handle mid-turn could split the (agent, session) pair
    /// across two runs. Both must resolve from the same TurnContext, so with a
    /// live turn scope the tool must read THIS turn's session even when the
    /// global mirror points at another run's session.
    #[tokio::test]
    async fn recall_context_reads_the_turn_session_not_the_global_mirror() {
        // `BuiltinToolRegistry::new` opens the goal store under whatever
        // `ALEPH_HOME` currently says, and that is process-global: without this
        // the test both touches the developer's real `~/.aleph/data` and races
        // any sibling holding an `IsolatedAlephHome` — both ended up opening the
        // *same* `goals.db` and one lost with "database is locked".
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let mut registry = BuiltinToolRegistry::new().await.unwrap();

        // Seed a raw chunk under this turn's (agent, session) pair.
        let turn_session = SessionKey::main("alice").to_key_string();
        let db: crate::memory::store::MemoryBackend =
            Arc::new(crate::memory::store::sqlite::SqliteMemoryBackend::in_memory().unwrap());
        let raw = RawMemory::new(
            "the alice chunk".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("alice")
        .with_session(turn_session.clone())
        .with_path(format!("aleph://session/{turn_session}/raw/0"));
        db.insert_raw_memory(&raw).await.unwrap();
        registry.recall_context_db = Some(db);

        // The process-global mirror points at ANOTHER run's session — exactly
        // what a concurrent run rewriting the handle mid-turn produces.
        registry.session_context_handle = Some(Arc::new(RwLock::new(SessionContext {
            session_key_str: SessionKey::main("bob").to_key_string(),
            ..Default::default()
        })));

        let out = TURN_CONTEXT
            .scope(turn_ctx("alice"), async {
                registry
                    .execute_tool("recall_context", serde_json::json!({ "query": "anything" }))
                    .await
            })
            .await
            .unwrap();

        let fragments = out["fragments"]
            .as_array()
            .expect("recall_context output carries a fragments array");
        assert_eq!(
            fragments.len(),
            1,
            "must recall under the turn-context session, not the global mirror's"
        );
        assert_eq!(fragments[0]["content"], "the alice chunk");
    }

    /// Outside a turn scope (direct calls, non-gateway paths) the global
    /// mirror is the only session source and must still be honored.
    #[tokio::test]
    async fn recall_context_falls_back_to_the_global_mirror_without_a_turn() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let mut registry = BuiltinToolRegistry::new().await.unwrap();

        let mirror_session = SessionKey::main("bob").to_key_string();
        let db: crate::memory::store::MemoryBackend =
            Arc::new(crate::memory::store::sqlite::SqliteMemoryBackend::in_memory().unwrap());
        let raw = RawMemory::new(
            "the bob chunk".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("bob")
        .with_session(mirror_session.clone())
        .with_path(format!("aleph://session/{mirror_session}/raw/0"));
        db.insert_raw_memory(&raw).await.unwrap();
        registry.recall_context_db = Some(db);
        // The mirror's key encodes agent "bob"; without a turn scope both the
        // session AND the agent (via caller_agent_id parsing the same key)
        // resolve from this one handle — the pair stays coherent.
        registry.session_context_handle = Some(Arc::new(RwLock::new(SessionContext {
            session_key_str: mirror_session,
            ..Default::default()
        })));

        let out = registry
            .execute_tool("recall_context", serde_json::json!({ "query": "anything" }))
            .await
            .unwrap();

        let fragments = out["fragments"]
            .as_array()
            .expect("recall_context output carries a fragments array");
        assert_eq!(fragments.len(), 1, "global mirror must remain the fallback");
        assert_eq!(fragments[0]["content"], "the bob chunk");
    }

    /// `caller_agent_id` prefers the per-turn `TURN_CONTEXT` over the shared,
    /// mutable `session_context_handle`: a concurrent run rewriting the mirror
    /// to another agent cannot steal THIS turn's identity mid-call.
    #[tokio::test]
    async fn caller_agent_id_prefers_the_turn_over_the_shared_handle() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let mut registry = BuiltinToolRegistry::new().await.unwrap();
        // Mirror points at "bob" — what a concurrent run's write leaves behind.
        registry.session_context_handle = Some(Arc::new(RwLock::new(SessionContext {
            session_key_str: SessionKey::main("bob").to_key_string(),
            ..Default::default()
        })));

        let out = TURN_CONTEXT
            .scope(turn_ctx("alice"), async {
                registry.caller_agent_id("fallback")
            })
            .await;
        assert_eq!(
            out, "alice",
            "the per-turn agent wins over the shared mirror"
        );
    }

    /// Outside a turn scope the shared handle stays the identity source, and its
    /// absence yields the fallback — unchanged pre-fix behaviour.
    #[tokio::test]
    async fn caller_agent_id_falls_back_to_handle_then_fallback_without_a_turn() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let mut registry = BuiltinToolRegistry::new().await.unwrap();
        // No handle, no turn scope → fallback.
        assert_eq!(registry.caller_agent_id("fallback"), "fallback");

        // Handle set, still no turn scope → parsed from the handle.
        registry.session_context_handle = Some(Arc::new(RwLock::new(SessionContext {
            session_key_str: SessionKey::main("bob").to_key_string(),
            ..Default::default()
        })));
        assert_eq!(registry.caller_agent_id("fallback"), "bob");
    }
}

#[cfg(test)]
mod channel_tool_dispatch_tests {
    use super::*;

    /// Every `channel_*` tool advertised to the model must have a dispatch arm.
    ///
    /// The failure this guards is silent and one-sided: `optional_tools.rs`
    /// registers the name + JSON schema (so the model sees the tool and calls
    /// it), while dispatch lives in a *different* file's `match`. Miss the arm
    /// and the call falls through to `_ =>` and returns "Unknown tool" — a
    /// capability that is advertised, tested at the tool level, and
    /// unreachable in production.
    ///
    /// Asserting on the *injection* error rather than success is deliberate:
    /// it is the proof that the arm ran. A bare "did not error" would also
    /// pass for a tool that never reached its arm at all.
    #[tokio::test]
    async fn advertised_channel_tools_reach_their_dispatch_arm() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = BuiltinToolRegistry::new().await.unwrap();

        for name in [
            "channel_pairing",
            "channel_message",
            "channel_directory",
            "channel_outbox",
        ] {
            let err = registry
                .execute_tool(name, serde_json::json!({}))
                .await
                .expect_err("no ChannelRegistry is injected in this test");
            let msg = err.to_string();
            assert!(
                !msg.contains("Unknown tool"),
                "{name} is advertised to the model but has no dispatch arm: {msg}"
            );
            assert!(
                msg.contains("ChannelRegistry not yet injected"),
                "{name} should report the missing injection, got: {msg}"
            );
        }
    }

    // -- read/write partition symmetry --------------------------------------

    /// Every dispatch arm that hands a memory/note tool an agent id must hand
    /// it the COMPOSED partition, because that is what the writers wrote to.
    ///
    /// Source-level, because the failure is silent in both directions: a reader
    /// pointed at the bare persona finds an empty directory and reports it as
    /// an empty directory, and a writer pointed at the bare persona pools every
    /// principal's rows into the one partition they can all read. No test that
    /// constructs a tool with a base id and asserts against that same base id
    /// can cross this seam.
    const MEMORY_ARMS_THAT_MUST_COMPOSE: &[&str] = &[
        "note_orient",
        "note_schema",
        "user_profile",
        "recall_context",
        "memory_trace",
        "note_graph_query",
        "flag_user_correction",
    ];

    #[test]
    fn every_memory_dispatch_arm_composes_the_partition() {
        let source = include_str!("tool_registry_impl.rs");
        let lines: Vec<&str> = source.lines().collect();
        let mut offenders = Vec::new();

        for name in MEMORY_ARMS_THAT_MUST_COMPOSE {
            let needle = format!("\"{name}\" =>");
            let Some(arm) = lines
                .iter()
                .position(|l| l.trim_start().starts_with(&needle))
            else {
                offenders.push(format!("{name}: no dispatch arm found at all"));
                continue;
            };
            // The resolution always happens in the arm's opening statements,
            // before the `Box::pin`; 20 lines is generous room for the comment
            // that explains why.
            let window = lines[arm..(arm + 20).min(lines.len())].join("\n");
            if !window.contains("caller_memory_partition")
                && !window.contains("caller_profile_partition")
            {
                offenders.push(format!(
                    "{name} (line {}): resolves an agent id without composing the session scope",
                    arm + 1
                ));
            }
        }

        assert!(
            offenders.is_empty(),
            "these memory/note dispatch arms read or write the bare persona (`main`) while their \
             counterparties use the composed partition (`main__u-alice` / `main__p-room`) — a \
             stock loopback Panel session is already `Personal(u-owner)`, so this is not a \
             multi-user-only defect. Resolve through \
             `BuiltinToolRegistry::caller_memory_partition` (or \
             `caller_profile_partition` for the one reader that must refuse inside a room):\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The helper itself: a scoped run resolves to its own partition, an
    /// unscoped one is byte-identical to the bare persona, and the profile
    /// twin refuses inside a room rather than answering with the room's.
    #[tokio::test]
    async fn caller_memory_partition_composes_the_ambient_scope() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = BuiltinToolRegistry::new().await.unwrap();

        // No scope at all (cron / tests / direct calls): unchanged.
        assert_eq!(registry.caller_memory_partition("main"), "main");
        assert_eq!(
            registry.caller_profile_partition("main").as_deref(),
            Some("main")
        );

        // A personal session writes and reads its own partition.
        let personal = crate::scope::ScopeAttribution::personal("u-alice");
        let (partition, profile) = crate::scope::with_scope(Some(personal), async {
            (
                registry.caller_memory_partition("main"),
                registry.caller_profile_partition("main"),
            )
        })
        .await;
        assert_eq!(partition, "main__u-alice");
        assert_eq!(profile.as_deref(), Some("main__u-alice"));

        // A room shares one partition — and has no single profile to read.
        let room = crate::scope::ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project("p-room".to_string()),
        };
        let (partition, profile) = crate::scope::with_scope(Some(room), async {
            (
                registry.caller_memory_partition("main"),
                registry.caller_profile_partition("main"),
            )
        })
        .await;
        assert_eq!(partition, "main__p-room");
        assert_eq!(
            profile, None,
            "a room holds more than one human, so `user_profile` must refuse rather than \
             answer with the room's merged profile"
        );
    }
}
