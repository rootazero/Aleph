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
        memory_search_tool: &Option<crate::builtin_tools::runtime::RuntimeTool<MemorySearchTool>>,
        memory_browse_tool: &Option<crate::builtin_tools::runtime::RuntimeTool<MemoryBrowseTool>>,
        memory_explore_tool: &Option<crate::builtin_tools::runtime::RuntimeTool<MemoryExploreTool>>,
        memory_timeline_tool: &Option<crate::builtin_tools::MemoryTimelineTool>,
        image_generate_tool: &Option<ImageGenerateTool>,
        vault_store_tool: &Option<VaultStoreTool>,
        config: &BuiltinToolConfig,
        note_orient_tool: &Option<
            crate::builtin_tools::runtime::RuntimeTool<
                crate::builtin_tools::note_orient::NoteOrientTool,
            >,
        >,
        note_schema_tool: &Option<
            crate::builtin_tools::runtime::RuntimeTool<
                crate::builtin_tools::note_schema::NoteSchemaTool,
            >,
        >,
        user_profile_tool: &Option<
            crate::builtin_tools::runtime::RuntimeTool<
                crate::builtin_tools::user_profile::UserProfileTool,
            >,
        >,
    ) {
        // Helper: register tool with schema from schemars
        fn reg<'a>(
            tools: &'a mut HashMap<String, UnifiedTool>,
            name: &str,
            desc: &str,
            schema: serde_json::Value,
        ) -> &'a mut UnifiedTool {
            let mut ut =
                UnifiedTool::new(format!("builtin:{name}"), name, desc, ToolSource::Builtin);
            ut.parameters_schema = Some(schema);
            tools.entry(name.to_owned()).insert_entry(ut).into_mut()
        }

        // Memory retrieval tools — only exposed in Tools / Hybrid mode.
        // In Context mode the LLM receives memory via auto-injected context messages instead.
        // The mode is read from the config, the single source of truth for it: a
        // separate parameter could disagree with the config the rest of the
        // system reads, and the gate would then use a different value.
        let expose_retrieval_tools = matches!(
            config.injection_mode,
            crate::config::types::memory::MemoryInjectionMode::Tools
                | crate::config::types::memory::MemoryInjectionMode::Hybrid,
        );

        if expose_retrieval_tools {
            if memory_search_tool.is_some() {
                reg(
                    tools,
                    "memory_search",
                    MemorySearchTool::DESCRIPTION,
                    super::core_tools::tool_schema::<
                        crate::builtin_tools::memory_search::MemorySearchArgs,
                    >("memory_search"),
                )
                .services = MemorySearchTool::services();
                info!("Registered memory_search tool in BuiltinToolRegistry");
            }
            if memory_browse_tool.is_some() {
                reg(
                    tools,
                    "memory_browse",
                    MemoryBrowseTool::DESCRIPTION,
                    super::core_tools::tool_schema::<
                        crate::builtin_tools::memory_browse::MemoryBrowseArgs,
                    >("memory_browse"),
                )
                .services = MemoryBrowseTool::services();
                info!("Registered memory.browse tool in BuiltinToolRegistry");
            }
            if memory_explore_tool.is_some() {
                reg(
                    tools,
                    "memory_explore",
                    MemoryExploreTool::DESCRIPTION,
                    super::core_tools::tool_schema::<
                        crate::builtin_tools::memory_explore::MemoryExploreArgs,
                    >("memory_explore"),
                )
                .services = MemoryExploreTool::services();
                info!("Registered memory_explore tool in BuiltinToolRegistry");
            }
            if memory_timeline_tool.is_some() {
                reg(
                    tools,
                    "memory_timeline",
                    crate::builtin_tools::MemoryTimelineTool::DESCRIPTION,
                    super::core_tools::tool_schema::<
                        crate::builtin_tools::memory_timeline::MemoryTimelineArgs,
                    >("memory_timeline"),
                )
                .services = crate::builtin_tools::MemoryTimelineTool::services();
                info!("Registered memory_timeline tool in BuiltinToolRegistry");
            }
        }

        // Phase 3 Task 19 — flag_user_correction is exposed whenever a
        // memory backend exists (independent of retrieval policy: this is
        // a write-only signal, not a retrieval surface).
        //
        // Both handlers need the memory backend AND the raw memory store: the
        // correction is written to the raw store and the audit counts are read
        // back from it. Advertising the tools without the raw store produced a
        // fully-schemaed tool whose every call failed with
        // "no raw memory store configured".
        if config.memory_db.is_some() && config.raw_memory_store.is_some() {
            reg(
                tools,
                "flag_user_correction",
                crate::builtin_tools::FlagUserCorrectionTool::DESCRIPTION,
                super::core_tools::tool_schema::<
                    crate::builtin_tools::flag_user_correction::FlagUserCorrectionArgs,
                >("flag_user_correction"),
            )
            .services = crate::builtin_tools::FlagUserCorrectionTool::services();
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
                super::core_tools::tool_schema::<
                    crate::builtin_tools::governance_metrics::GovernanceMetricsArgs,
                >("governance_metrics"),
            )
            .services = crate::builtin_tools::governance_metrics::GovernanceMetricsTool::services();
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
            super::core_tools::tool_schema::<crate::builtin_tools::remember::RememberArgs>(
                "remember",
            ),
        )
        .services = crate::builtin_tools::RememberTool::services();
        info!("Registered remember tool in BuiltinToolRegistry");

        // node_list — cluster discovery (the read half of node_invoke). Always
        // exposed; resolves the same deferred NodeRegistry at call time.
        reg(
            tools,
            "node_list",
            crate::builtin_tools::NodeListTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::node_list::NodeListArgs>(
                "node_list",
            ),
        );
        info!("Registered node_list tool in BuiltinToolRegistry");

        // node_invoke — cluster fan-out tool. Always exposed; its execution
        // path resolves the gateway NodeRegistry via the deferred
        // set_node_registry injection (mirrors the remember tool's pattern).
        reg(
            tools,
            "node_invoke",
            crate::builtin_tools::NodeInvokeTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::node_invoke::NodeInvokeArgs>(
                "node_invoke",
            ),
        );
        info!("Registered node_invoke tool in BuiltinToolRegistry");

        // node_file — cluster file transfer. Same deferred NodeRegistry as node_invoke.
        reg(
            tools,
            "node_file",
            crate::builtin_tools::NodeFileTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::node_file::NodeFileArgs>(
                "node_file",
            ),
        );
        info!("Registered node_file tool in BuiltinToolRegistry");

        // node_invoke_many — cluster tag fan-out. Same deferred NodeRegistry.
        reg(
            tools,
            "node_invoke_many",
            crate::builtin_tools::NodeInvokeManyTool::DESCRIPTION,
            super::core_tools::tool_schema::<
                crate::builtin_tools::node_invoke_many::NodeInvokeManyArgs,
            >("node_invoke_many"),
        );
        info!("Registered node_invoke_many tool in BuiltinToolRegistry");

        // node_manage — cluster membership (R8 write half of node_list). Needs
        // the NodeRegistry AND the SecurityStore, both deferred-injected; the
        // dispatch arm reports which one is missing.
        reg(
            tools,
            "node_manage",
            crate::builtin_tools::NodeManageTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::node_manage::NodeManageArgs>(
                "node_manage",
            ),
        );
        info!("Registered node_manage tool in BuiltinToolRegistry");

        // workspace_manage — the conversational face of `workspace.*` (R8).
        // Gated on the same handle the constructor builds the tool from, so
        // the schema and the tool appear or stay absent together: a schema
        // registered without a tool is a name the model can call and nothing
        // can answer.
        if config.workspace_manager.is_some() {
            reg(
                tools,
                "workspace_manage",
                crate::builtin_tools::WorkspaceManageTool::DESCRIPTION,
                super::core_tools::tool_schema::<
                    crate::builtin_tools::workspace_manage::WorkspaceManageArgs,
                >("workspace_manage"),
            );
            info!("Registered workspace_manage tool in BuiltinToolRegistry");
        }

        // canvas — the model's face of the whiteboard (R8). Gated on the same
        // handle the constructor builds the tool from, so schema and tool
        // appear or stay absent together (the workspace_manage rule).
        if config.canvas_store.is_some() {
            reg(
                tools,
                "canvas",
                crate::builtin_tools::CanvasTool::DESCRIPTION,
                super::core_tools::tool_schema::<crate::builtin_tools::canvas::CanvasToolArgs>(
                    "canvas",
                ),
            );
            info!("Registered canvas tool in BuiltinToolRegistry");
        }

        // Vault store tool
        if vault_store_tool.is_some() {
            reg(
                tools,
                "vault_store",
                VaultStoreTool::DESCRIPTION,
                super::core_tools::tool_schema::<crate::builtin_tools::vault_store::VaultStoreArgs>(
                    "vault_store",
                ),
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
                        super::core_tools::tool_schema::<crate::builtin_tools::ImageGenerateArgs>(
                            "image_generate",
                        ),
                    );
                    info!("Registered image.generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Video).is_some() {
                    reg(
                        tools,
                        "video_generate",
                        crate::builtin_tools::generation::VideoGenerateTool::DESCRIPTION,
                        super::core_tools::tool_schema::<
                            crate::builtin_tools::generation::VideoGenerateArgs,
                        >("video_generate"),
                    );
                    info!("Registered video_generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Audio).is_some() {
                    reg(
                        tools,
                        "audio_generate",
                        crate::builtin_tools::generation::AudioGenerateTool::DESCRIPTION,
                        super::core_tools::tool_schema::<
                            crate::builtin_tools::generation::AudioGenerateArgs,
                        >("audio_generate"),
                    );
                    info!("Registered audio_generate tool in BuiltinToolRegistry");
                }

                if reg_inner.first_for_type(GenerationType::Speech).is_some() {
                    reg(
                        tools,
                        "speech_generate",
                        crate::builtin_tools::generation::SpeechGenerateTool::DESCRIPTION,
                        super::core_tools::tool_schema::<
                            crate::builtin_tools::generation::SpeechGenerateArgs,
                        >("speech_generate"),
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
                HeartbeatCreateTool, HeartbeatDeleteTool, HeartbeatListTool, HeartbeatToggleTool,
                HeartbeatUpdateTool,
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
        }

        // heartbeat_report is always registered (L2 output tool — no service
        // dependency). It is registered once, outside the service block: the
        // two branches used to register it twice, so the second registration
        // silently overwrote the first.
        {
            use crate::builtin_tools::heartbeat_manage::HeartbeatReportTool;
            let report_tool = HeartbeatReportTool;
            let def = AlephTool::definition(&report_tool);
            reg(
                tools,
                "heartbeat_report",
                HeartbeatReportTool::DESCRIPTION,
                def.parameters.clone(),
            );
            info!("Registered heartbeat_report tool in BuiltinToolRegistry");
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

            let tmp_compact = SessionCompactTool::new();
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
        }

        // session_search — metadata only; tool is constructed on-the-fly from
        // the GatewayContext. The handler needs that context, so the tool is
        // registered only when one exists: otherwise it was advertised with a
        // full schema and every call failed with
        // "GatewayContext not yet injected".
        if config.gateway_context.is_some() {
            use crate::builtin_tools::SessionSearchTool;
            reg(
                tools,
                "session_search",
                SessionSearchTool::DESCRIPTION,
                super::core_tools::tool_schema::<
                    crate::builtin_tools::session_search::SessionSearchArgs,
                >("session_search"),
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
                super::core_tools::tool_schema::<
                    crate::builtin_tools::channel_manage::ChannelPairingArgs,
                >("channel_pairing"),
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
                super::core_tools::tool_schema::<
                    crate::builtin_tools::channel_message::ChannelMessageArgs,
                >("channel_message"),
            );
            info!("Registered channel_message tool in BuiltinToolRegistry");
        }

        // Channel directory tool — the read half of channel addressing.
        // Same deferred-injection pattern as channel_message above.
        {
            use crate::builtin_tools::channel_directory::ChannelDirectoryTool;
            reg(
                tools,
                "channel_directory",
                ChannelDirectoryTool::DESCRIPTION,
                super::core_tools::tool_schema::<
                    crate::builtin_tools::channel_directory::ChannelDirectoryArgs,
                >("channel_directory"),
            );
            info!("Registered channel_directory tool in BuiltinToolRegistry");
        }

        // Channel outbox tool — the recovery half of the delivery queue, and
        // the only production consumer of the dead-letter trail. Same deferred
        // ChannelRegistry injection as the two above.
        {
            use crate::builtin_tools::channel_outbox::ChannelOutboxTool;
            reg(
                tools,
                "channel_outbox",
                ChannelOutboxTool::DESCRIPTION,
                super::core_tools::tool_schema::<
                    crate::builtin_tools::channel_outbox::ChannelOutboxArgs,
                >("channel_outbox"),
            );
            info!("Registered channel_outbox tool in BuiltinToolRegistry");
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
                super::core_tools::tool_schema::<crate::builtin_tools::ask_user::AskUserArgs>(
                    "ask_user",
                ),
            );
            info!("Registered ask_user tool in BuiltinToolRegistry");
        }

        // Sessions tools — always register metadata so LLM sees them.
        // GatewayContext may be injected later via gateway_context_cell().
        // Execution checks OnceCell at call time.
        reg(
            tools,
            "session_list",
            SessionsListTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::sessions::SessionsListArgs>(
                "session_list",
            ),
        );
        reg(
            tools,
            "session_send",
            SessionsSendTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::sessions::SessionsSendArgs>(
                "session_send",
            ),
        );
        info!("Registered session.list + session.send in BuiltinToolRegistry");

        // Voice mode tool — toggle voice output on/off for a channel (R9)
        reg(
            tools,
            "voice_mode_set",
            crate::builtin_tools::voice_tools::VoiceModeSetTool::DESCRIPTION,
            super::core_tools::tool_schema::<crate::builtin_tools::voice_tools::VoiceModeSetArgs>(
                "voice_mode_set",
            ),
        );
        info!("Registered voice_mode_set tool in BuiltinToolRegistry");

        // Local voice endpoint tool — status probe for the BYO STT/TTS server (R8).
        // The handler needs the live Config handle, so the tool is registered
        // only when one is configured — the same rule the sibling
        // `config_audit` tool follows.
        if config.config.is_some() {
            reg(
                tools,
                "local_voice",
                crate::builtin_tools::voice_tools::LocalVoiceTool::DESCRIPTION,
                super::core_tools::tool_schema::<crate::builtin_tools::voice_tools::LocalVoiceArgs>(
                    "local_voice",
                ),
            );
            info!("Registered local_voice tool in BuiltinToolRegistry");
        }

        // Wiki orientation tools (Spec 5 Task 12).
        // `note_schema` is always registered when a memory dir is available (LLM can always
        // read/write SCHEMA.md). `note_orient` is only exposed in Tools / Hybrid mode so the
        // LLM can call it on-demand; in Context mode orientation is injected automatically.
        if note_schema_tool.is_some() {
            reg(
                tools,
                "note_schema",
                crate::builtin_tools::note_schema::NoteSchemaTool::DESCRIPTION,
                super::core_tools::tool_schema::<crate::builtin_tools::note_schema::NoteSchemaArgs>(
                    "note_schema",
                ),
            )
            .services = crate::builtin_tools::note_schema::NoteSchemaTool::services();
            info!("Registered note_schema tool in BuiltinToolRegistry");
        }

        if expose_retrieval_tools && note_orient_tool.is_some() {
            reg(
                tools,
                "note_orient",
                crate::builtin_tools::note_orient::NoteOrientTool::DESCRIPTION,
                super::core_tools::tool_schema::<crate::builtin_tools::note_orient::NoteOrientArgs>(
                    "note_orient",
                ),
            )
            .services = crate::builtin_tools::note_orient::NoteOrientTool::services();
            info!("Registered note_orient tool in BuiltinToolRegistry");
        }

        // User profile tool (Spec 7 Task 9) — always exposed when synthesizer is available.
        if user_profile_tool.is_some() {
            reg(
                tools,
                "user_profile",
                crate::builtin_tools::user_profile::UserProfileTool::DESCRIPTION,
                super::core_tools::tool_schema::<crate::builtin_tools::user_profile::UserProfileArgs>(
                    "user_profile",
                ),
            ).services = crate::builtin_tools::user_profile::UserProfileTool::services();
            info!("Registered user_profile tool in BuiltinToolRegistry");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{BuiltinToolConfig, BuiltinToolRegistry};
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::raw_memory::RawMemoryStore;
    use crate::sync_primitives::Arc;

    fn memory_backend(dir: &std::path::Path) -> crate::memory::store::MemoryBackend {
        Arc::new(crate::memory::store::SqliteMemoryBackend::new(dir).unwrap())
    }

    async fn raw_memory_store(dir: &std::path::Path) -> Arc<dyn RawMemoryStore> {
        Arc::new(
            crate::memory::store::markdown::MarkdownSessionStore::new(dir)
                .await
                .unwrap(),
        )
    }

    fn stub_embedder() -> Arc<dyn crate::memory::EmbeddingProvider> {
        Arc::new(MockEmbeddingProvider::new(1024, "mock"))
    }

    /// The corrections are written to the raw store and the audit counts are
    /// read back from it, so a memory backend on its own cannot serve either
    /// tool — it used to be advertised anyway and every call then failed with
    /// "no raw memory store configured".
    #[tokio::test]
    async fn flag_user_correction_and_governance_metrics_need_the_raw_memory_store() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = memory_backend(tmp.path());
        let raw_store = raw_memory_store(tmp.path()).await;

        let mut without_raw_store = HashMap::new();
        BuiltinToolRegistry::register_optional_tools(
            &mut without_raw_store,
            &None,
            &None,
            &None,
            &None,
            &None,
            &None,
            &BuiltinToolConfig {
                memory_db: Some(backend.clone()),
                raw_memory_store: None,
                ..Default::default()
            },
            &None,
            &None,
            &None,
        );
        assert!(!without_raw_store.contains_key("flag_user_correction"));
        assert!(!without_raw_store.contains_key("governance_metrics"));

        let mut with_raw_store = HashMap::new();
        BuiltinToolRegistry::register_optional_tools(
            &mut with_raw_store,
            &None,
            &None,
            &None,
            &None,
            &None,
            &None,
            &BuiltinToolConfig {
                memory_db: Some(backend),
                raw_memory_store: Some(raw_store),
                ..Default::default()
            },
            &None,
            &None,
            &None,
        );
        assert!(with_raw_store.contains_key("flag_user_correction"));
        assert!(with_raw_store.contains_key("governance_metrics"));
    }

    /// The local_voice handler reads the live Config handle, so without that
    /// handle every call fails. The tool must not be advertised in that case
    /// — the sibling `config_audit` tool already follows that rule.
    #[test]
    fn local_voice_requires_the_config_handle() {
        let mut tools = HashMap::new();
        BuiltinToolRegistry::register_optional_tools(
            &mut tools,
            &None,
            &None,
            &None,
            &None,
            &None,
            &None,
            &BuiltinToolConfig::default(),
            &None,
            &None,
            &None,
        );
        assert!(!tools.contains_key("local_voice"));
    }

    /// The session_search handler is built from the GatewayContext. A session
    /// store alone is not enough, and advertising the tool anyway produced a
    /// fully-schemaed tool whose every call failed with
    /// "GatewayContext not yet injected".
    #[tokio::test]
    async fn session_search_requires_the_gateway_context() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let session_manager: Arc<dyn crate::gateway::session_store::SessionStore> =
            Arc::new(crate::gateway::SessionManager::with_defaults().unwrap());

        let mut tools = HashMap::new();
        BuiltinToolRegistry::register_optional_tools(
            &mut tools,
            &None,
            &None,
            &None,
            &None,
            &None,
            &None,
            &BuiltinToolConfig {
                session_manager: Some(session_manager),
                ..Default::default()
            },
            &None,
            &None,
            &None,
        );
        assert!(tools.contains_key("session_new"));
        assert!(!tools.contains_key("session_search"));
    }

    /// Context mode injects memory as context messages instead of exposing
    /// retrieval tools. The tools must not exist at all there: a constructed
    /// tool still answers dispatch calls, which would serve retrieval the
    /// model was never given a schema for.
    #[tokio::test]
    async fn context_mode_does_not_build_the_retrieval_tools() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let tmp = tempfile::tempdir().unwrap();
        let backend = memory_backend(tmp.path());
        let raw_store = raw_memory_store(tmp.path()).await;
        let embedder = stub_embedder();

        let context_registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
            memory_db: Some(backend.clone()),
            raw_memory_store: Some(raw_store.clone()),
            embedder: Some(embedder.clone()),
            injection_mode: crate::config::types::memory::MemoryInjectionMode::Context,
            ..Default::default()
        })
        .await
        .unwrap();

        assert!(!context_registry.has_tool("memory_search"));
        assert!(!context_registry.has_tool("memory_browse"));
        assert!(!context_registry.has_tool("memory_explore"));
        assert!(!context_registry.has_tool("memory_timeline"));

        let tools_registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
            memory_db: Some(backend),
            raw_memory_store: Some(raw_store),
            embedder: Some(embedder),
            injection_mode: crate::config::types::memory::MemoryInjectionMode::Tools,
            ..Default::default()
        })
        .await
        .unwrap();

        assert!(tools_registry.has_tool("memory_search"));
        assert!(tools_registry.has_tool("memory_browse"));
        assert!(tools_registry.has_tool("memory_explore"));
    }
}
