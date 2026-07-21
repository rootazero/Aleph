use std::collections::HashMap;

use tracing::info;

use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::builtin_tools::{
    ImageGenerateTool, MemoryBrowseTool, MemoryExploreTool, MemorySearchTool, VaultStoreTool,
};
use crate::sync_primitives::Arc;
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;

use super::{BuiltinToolConfig, BuiltinToolRegistry};

impl BuiltinToolRegistry {
    /// Register metadata for optional tools (only when their dependencies are available)
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register_optional_tools(
        tools: &mut HashMap<String, UnifiedTool>,
        memory_search_tool: &Option<MemorySearchTool>,
        memory_browse_tool: &Option<MemoryBrowseTool>,
        memory_explore_tool: &Option<MemoryExploreTool>,
        memory_timeline_tool: &Option<crate::builtin_tools::MemoryTimelineTool>,
        image_generate_tool: &Option<ImageGenerateTool>,
        vault_store_tool: &Option<VaultStoreTool>,
        config: &BuiltinToolConfig,
        injection_mode: crate::config::types::memory::MemoryInjectionMode,
        note_orient_tool: &Option<crate::builtin_tools::note_orient::NoteOrientTool>,
        note_schema_tool: &Option<crate::builtin_tools::note_schema::NoteSchemaTool>,
        user_profile_tool: &Option<crate::builtin_tools::user_profile::UserProfileTool>,
    ) {
        fn schema<T: schemars::JsonSchema>(name: &str) -> serde_json::Value {
            serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|e| {
                tracing::warn!("Failed to serialize schema for {}: {}", name, e);
                serde_json::Value::Object(Default::default())
            })
        }

        // Helper: register tool with schema from schemars
        fn reg(
            tools: &mut HashMap<String, UnifiedTool>,
            name: &str,
            desc: &str,
            schema: serde_json::Value,
        ) {
            let mut ut =
                UnifiedTool::new(format!("builtin:{name}"), name, desc, ToolSource::Builtin);
            ut.parameters_schema = Some(schema);
            tools.insert(name.to_string(), ut);
        }

        // Memory retrieval tools — only exposed in Tools / Hybrid mode.
        // In Context mode the LLM receives memory via auto-injected context messages instead.
        let expose_retrieval_tools = matches!(
            injection_mode,
            crate::config::types::memory::MemoryInjectionMode::Tools
                | crate::config::types::memory::MemoryInjectionMode::Hybrid,
        );

        if expose_retrieval_tools {
            if memory_search_tool.is_some() {
                reg(
                    tools,
                    "memory_search",
                    MemorySearchTool::DESCRIPTION,
                    schema::<crate::builtin_tools::memory_search::MemorySearchArgs>(
                        "memory_search",
                    ),
                );
                info!("Registered memory_search tool in BuiltinToolRegistry");
            }
            if memory_browse_tool.is_some() {
                reg(
                    tools,
                    "memory_browse",
                    MemoryBrowseTool::DESCRIPTION,
                    schema::<crate::builtin_tools::memory_browse::MemoryBrowseArgs>(
                        "memory_browse",
                    ),
                );
                info!("Registered memory.browse tool in BuiltinToolRegistry");
            }
            if memory_explore_tool.is_some() {
                reg(
                    tools,
                    "memory_explore",
                    MemoryExploreTool::DESCRIPTION,
                    schema::<crate::builtin_tools::memory_explore::MemoryExploreArgs>(
                        "memory_explore",
                    ),
                );
                info!("Registered memory_explore tool in BuiltinToolRegistry");
            }
            if memory_timeline_tool.is_some() {
                reg(
                    tools,
                    "memory_timeline",
                    crate::builtin_tools::MemoryTimelineTool::DESCRIPTION,
                    schema::<crate::builtin_tools::memory_timeline::MemoryTimelineArgs>(
                        "memory_timeline",
                    ),
                );
                info!("Registered memory_timeline tool in BuiltinToolRegistry");
            }
        }

        // Phase 3 Task 19 — flag_user_correction is exposed whenever a
        // memory backend exists (independent of retrieval policy: this is
        // a write-only signal, not a retrieval surface).
        if config.memory_db.is_some() {
            reg(
                tools,
                "flag_user_correction",
                crate::builtin_tools::FlagUserCorrectionTool::DESCRIPTION,
                schema::<crate::builtin_tools::flag_user_correction::FlagUserCorrectionArgs>(
                    "flag_user_correction",
                ),
            );
            info!("Registered flag_user_correction tool in BuiltinToolRegistry");

            // governance_metrics — read-only audit reality probe. Exposed
            // whenever a memory backend exists (independent of retrieval policy,
            // like flag_user_correction): the loop-governance audit ring runs in
            // a headless cron session and must reach it regardless of injection
            // mode.
            reg(
                tools,
                "governance_metrics",
                crate::builtin_tools::governance_metrics::GovernanceMetricsTool::DESCRIPTION,
                schema::<crate::builtin_tools::governance_metrics::GovernanceMetricsArgs>(
                    "governance_metrics",
                ),
            );
            info!("Registered governance_metrics tool in BuiltinToolRegistry");
        }

        // Spec A Task 17 — remember tool is always exposed; its execution
        // path resolves the per-agent CuratedMemoryStore via the deferred
        // MemoryContextProvider injection. Curated memory is independent of
        // the retrieval backend, so we don't gate registration on memory_db.
        reg(
            tools,
            "remember",
            crate::builtin_tools::RememberTool::DESCRIPTION,
            schema::<crate::builtin_tools::remember::RememberArgs>("remember"),
        );
        info!("Registered remember tool in BuiltinToolRegistry");

        // node_list — cluster discovery (the read half of node_invoke). Always
        // exposed; resolves the same deferred NodeRegistry at call time.
        reg(
            tools,
            "node_list",
            crate::builtin_tools::NodeListTool::DESCRIPTION,
            schema::<crate::builtin_tools::node_list::NodeListArgs>("node_list"),
        );
        info!("Registered node_list tool in BuiltinToolRegistry");

        // node_invoke — cluster fan-out tool. Always exposed; its execution
        // path resolves the gateway NodeRegistry via the deferred
        // set_node_registry injection (mirrors the remember tool's pattern).
        reg(
            tools,
            "node_invoke",
            crate::builtin_tools::NodeInvokeTool::DESCRIPTION,
            schema::<crate::builtin_tools::node_invoke::NodeInvokeArgs>("node_invoke"),
        );
        info!("Registered node_invoke tool in BuiltinToolRegistry");

        // node_file — cluster file transfer. Same deferred NodeRegistry as node_invoke.
        reg(
            tools,
            "node_file",
            crate::builtin_tools::NodeFileTool::DESCRIPTION,
            schema::<crate::builtin_tools::node_file::NodeFileArgs>("node_file"),
        );
        info!("Registered node_file tool in BuiltinToolRegistry");

        // node_invoke_many — cluster tag fan-out. Same deferred NodeRegistry.
        reg(
            tools,
            "node_invoke_many",
            crate::builtin_tools::NodeInvokeManyTool::DESCRIPTION,
            schema::<crate::builtin_tools::node_invoke_many::NodeInvokeManyArgs>(
                "node_invoke_many",
            ),
        );
        info!("Registered node_invoke_many tool in BuiltinToolRegistry");

        // Vault store tool
        if vault_store_tool.is_some() {
            reg(
                tools,
                "vault_store",
                VaultStoreTool::DESCRIPTION,
                schema::<crate::builtin_tools::vault_store::VaultStoreArgs>("vault_store"),
            );
            info!("Registered vault.store tool in BuiltinToolRegistry");
        }

        // Generation tools
        let generation_registry = config.generation_registry.clone();
        if let Some(ref registry) = generation_registry {
            {
                let reg_inner = registry.read().unwrap_or_else(|e| e.into_inner());
                use crate::generation::GenerationType;

                // image_generate must verify a backing Image provider exists,
                // exactly like Video/Audio/Speech below — otherwise the LLM is
                // advertised a tool with no provider to serve it.
                if image_generate_tool.is_some()
                    && reg_inner.first_for_type(GenerationType::Image).is_some()
                {
                    reg(
                        tools,
                        "image_generate",
                        ImageGenerateTool::DESCRIPTION,
                        schema::<crate::builtin_tools::ImageGenerateArgs>("image_generate"),
                    );
                    info!("Registered image.generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Video).is_some() {
                    reg(
                        tools,
                        "video_generate",
                        crate::builtin_tools::generation::VideoGenerateTool::DESCRIPTION,
                        schema::<crate::builtin_tools::generation::VideoGenerateArgs>(
                            "video_generate",
                        ),
                    );
                    info!("Registered video_generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Audio).is_some() {
                    reg(
                        tools,
                        "audio_generate",
                        crate::builtin_tools::generation::AudioGenerateTool::DESCRIPTION,
                        schema::<crate::builtin_tools::generation::AudioGenerateArgs>(
                            "audio_generate",
                        ),
                    );
                    info!("Registered audio_generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Speech).is_some() {
                    reg(
                        tools,
                        "speech_generate",
                        crate::builtin_tools::generation::SpeechGenerateTool::DESCRIPTION,
                        schema::<crate::builtin_tools::generation::SpeechGenerateArgs>(
                            "speech_generate",
                        ),
                    );
                    info!("Registered speech_generate tool in BuiltinToolRegistry");
                }
            }
        }

        // Cron management tool (requires SharedCronService)
        if let Some(ref cron_svc) = config.cron_service {
            use crate::builtin_tools::cron_manage::CronManageTool;
            let tmp_tool = CronManageTool::new(Arc::clone(cron_svc));
            let def = AlephTool::definition(&tmp_tool);
            reg(
                tools,
                "cron_manage",
                CronManageTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered cron.manage tool in BuiltinToolRegistry");
        }

        // Heartbeat management tools (require SharedHeartbeatService)
        if let Some(ref hb_svc) = config.heartbeat_service {
            use crate::builtin_tools::heartbeat_manage::{
                HeartbeatCreateTool, HeartbeatDeleteTool, HeartbeatListTool, HeartbeatReportTool,
                HeartbeatToggleTool, HeartbeatUpdateTool,
            };
            let list_tool = HeartbeatListTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&list_tool);
            reg(
                tools,
                "heartbeat_list",
                HeartbeatListTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let create_tool = HeartbeatCreateTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&create_tool);
            reg(
                tools,
                "heartbeat_create",
                HeartbeatCreateTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let update_tool = HeartbeatUpdateTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&update_tool);
            reg(
                tools,
                "heartbeat_update",
                HeartbeatUpdateTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let delete_tool = HeartbeatDeleteTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&delete_tool);
            reg(
                tools,
                "heartbeat_delete",
                HeartbeatDeleteTool::DESCRIPTION,
                def.parameters.clone(),
            );

            let toggle_tool = HeartbeatToggleTool::new(Arc::clone(hb_svc));
            let def = AlephTool::definition(&toggle_tool);
            reg(
                tools,
                "heartbeat_toggle",
                HeartbeatToggleTool::DESCRIPTION,
                def.parameters.clone(),
            );

            info!("Registered heartbeat management tools in BuiltinToolRegistry");

            // heartbeat_report is always registered (L2 output tool — no service dependency)
            let report_tool = HeartbeatReportTool;
            let def = AlephTool::definition(&report_tool);
            reg(
                tools,
                "heartbeat_report",
                HeartbeatReportTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered heartbeat_report tool in BuiltinToolRegistry");
        } else {
            // Register heartbeat_report even without the heartbeat service
            use crate::builtin_tools::heartbeat_manage::HeartbeatReportTool;
            let report_tool = HeartbeatReportTool;
            let def = AlephTool::definition(&report_tool);
            reg(
                tools,
                "heartbeat_report",
                HeartbeatReportTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered heartbeat_report tool (standalone) in BuiltinToolRegistry");
        }

        // Session tools (require SessionManager — from gateway_context or direct session_manager)
        let session_mgr = config
            .gateway_context
            .as_ref()
            .map(|ctx| Arc::clone(ctx.session_store()))
            .or_else(|| config.session_manager.clone());

        if let Some(ref sm) = session_mgr {
            use crate::builtin_tools::sessions::{
                SessionCompactTool, SessionNewTool, SessionSetModeTool, SessionSetTopicTool,
            };

            let tmp_new = SessionNewTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_new);
            reg(
                tools,
                "session_new",
                SessionNewTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered session.new tool in BuiltinToolRegistry");

            let tmp_compact = SessionCompactTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_compact);
            reg(
                tools,
                "session_compact",
                SessionCompactTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered session.compact tool in BuiltinToolRegistry");

            let tmp_topic = SessionSetTopicTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_topic);
            reg(
                tools,
                "session_rename",
                SessionSetTopicTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered session.rename tool in BuiltinToolRegistry");

            let tmp_mode = SessionSetModeTool::new(Arc::clone(sm));
            let def = AlephTool::definition(&tmp_mode);
            reg(
                tools,
                "session_set_mode",
                SessionSetModeTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered session.set_mode tool in BuiltinToolRegistry");

            // session_search — metadata only; tool is constructed on-the-fly from GatewayContext
            use crate::builtin_tools::SessionSearchTool;
            reg(
                tools,
                "session_search",
                SessionSearchTool::DESCRIPTION,
                schema::<crate::builtin_tools::session_search::SessionSearchArgs>("session_search"),
            );
            info!("Registered session_search tool in BuiltinToolRegistry");
        }

        // Channel pairing tool — always register metadata (ChannelRegistry injected later).
        // Uses deferred OnceCell injection, same pattern as GatewayContext.
        {
            use crate::builtin_tools::channel_manage::ChannelPairingTool;
            reg(
                tools,
                "channel_pairing",
                ChannelPairingTool::DESCRIPTION,
                schema::<crate::builtin_tools::channel_manage::ChannelPairingArgs>(
                    "channel_pairing",
                ),
            );
            info!("Registered channel_pairing tool in BuiltinToolRegistry");
        }

        // Channel message tool — always register metadata (ChannelRegistry injected later).
        // Deferred OnceCell injection, same pattern as channel_pairing.
        {
            use crate::builtin_tools::channel_message::ChannelMessageTool;
            reg(
                tools,
                "channel_message",
                ChannelMessageTool::DESCRIPTION,
                schema::<crate::builtin_tools::channel_message::ChannelMessageArgs>(
                    "channel_message",
                ),
            );
            info!("Registered channel_message tool in BuiltinToolRegistry");
        }

        // ask_user clarification tool — always register metadata
        // (ChannelRegistry + ClarificationManager injected later via deferred
        // OnceCell wiring). Execution checks both cells at call time.
        {
            use crate::builtin_tools::ask_user::AskUserTool;
            reg(
                tools,
                "ask_user",
                AskUserTool::DESCRIPTION,
                schema::<crate::builtin_tools::ask_user::AskUserArgs>("ask_user"),
            );
            info!("Registered ask_user tool in BuiltinToolRegistry");
        }

        // Sessions tools — always register metadata so LLM sees them.
        // GatewayContext may be injected later via set_gateway_context().
        // Execution checks OnceCell at call time.
        reg(
            tools,
            "session_list",
            SessionsListTool::DESCRIPTION,
            schema::<crate::builtin_tools::sessions::SessionsListArgs>("session_list"),
        );
        reg(
            tools,
            "session_send",
            SessionsSendTool::DESCRIPTION,
            schema::<crate::builtin_tools::sessions::SessionsSendArgs>("session_send"),
        );
        info!("Registered session.list + session.send in BuiltinToolRegistry");

        // Voice mode tool — toggle voice output on/off for a channel (R9)
        reg(
            tools,
            "voice_mode_set",
            "Enable or disable voice mode for a channel. When enabled, all replies will be converted to speech audio. Use when user says 'turn on voice mode', 'switch to voice', 'enable voice replies', etc.",
            schema::<crate::builtin_tools::voice_tools::VoiceModeSetArgs>("voice_mode_set"),
        );
        info!("Registered voice_mode_set tool in BuiltinToolRegistry");

        // Local voice endpoint tool — status probe for the BYO STT/TTS server (R8)
        reg(
            tools,
            "local_voice",
            "Check the local voice (BYO OpenAI-compatible STT/TTS endpoint) configuration and \
             reachability. Use when the user asks whether local voice is ready, configured, or \
             why voice requests fail.",
            schema::<crate::builtin_tools::voice_tools::LocalVoiceArgs>("local_voice"),
        );
        info!("Registered local_voice tool in BuiltinToolRegistry");

        // Wiki orientation tools (Spec 5 Task 12).
        // `note_schema` is always registered when a memory dir is available (LLM can always
        // read/write SCHEMA.md). `note_orient` is only exposed in Tools / Hybrid mode so the
        // LLM can call it on-demand; in Context mode orientation is injected automatically.
        if note_schema_tool.is_some() {
            reg(
                tools,
                "note_schema",
                "Read or write the SCHEMA.md file that describes the structure of the agent's \
                 long-term memory wiki. Use 'read' to inspect the current schema, 'write' to \
                 update it (include the expected_hash from your last read to prevent conflicts).",
                schema::<crate::builtin_tools::note_schema::NoteSchemaArgs>("note_schema"),
            );
            info!("Registered note_schema tool in BuiltinToolRegistry");
        }

        if expose_retrieval_tools && note_orient_tool.is_some() {
            reg(
                tools,
                "note_orient",
                "Fetch a compact orientation snapshot of the agent's memory wiki: SCHEMA, \
                 index, and recent log entries. Call this at the start of a task to understand \
                 what structured memory is available before searching or writing notes.",
                schema::<crate::builtin_tools::note_orient::NoteOrientArgs>("note_orient"),
            );
            info!("Registered note_orient tool in BuiltinToolRegistry");
        }

        // User profile tool (Spec 7 Task 9) — always exposed when synthesizer is available.
        if user_profile_tool.is_some() {
            reg(
                tools,
                "user_profile",
                "Read the current user profile (interests, preferences, context) or view \
                 its revision history. Use 'read' to get the latest profile, 'history' to \
                 inspect the revision log.",
                schema::<crate::builtin_tools::user_profile::UserProfileArgs>("user_profile"),
            );
            info!("Registered user_profile tool in BuiltinToolRegistry");
        }
    }
}
