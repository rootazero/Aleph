//! Messaging, plan-approval, lifecycle, artifact, collaborative-session,
//! skill, and note tool construction for `BuiltinToolRegistry`.
//!
//! Extracted from `constructor.rs` to keep file sizes manageable. Builds the
//! messaging (send/inbox), plan-approval, worker-lifecycle, task-artifact,
//! collaborative-session, Google Meet, skill-management, note-management,
//! session-complete, and memory-reflect tools, registering their parameter
//! schemas (and the `recall_context` schema) into the shared `tools` map.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tracing::{info, warn};

use super::{BuiltinToolConfig, BuiltinToolRegistry};
use crate::tool_metadata::{ToolSource, UnifiedTool};

#[allow(clippy::type_complexity)]
impl BuiltinToolRegistry {
    /// Build messaging/session/skill/note tools and register their schemas.
    pub(crate) fn build_collab_session_tools(
        config: &BuiltinToolConfig,
        tools: &mut HashMap<String, UnifiedTool>,
        current_agent_id: &str,
    ) -> (
        Option<crate::builtin_tools::team::MessageSendTool>,
        Option<crate::builtin_tools::team::InboxReadTool>,
        Option<crate::builtin_tools::team::PlanSubmitTool>,
        Option<crate::builtin_tools::team::PlanResolveTool>,
        Option<crate::builtin_tools::team::LifecycleIdleTool>,
        Option<crate::builtin_tools::team::LifecycleRequestShutdownTool>,
        Option<crate::builtin_tools::team::LifecycleResolveShutdownTool>,
        Option<crate::builtin_tools::team::TaskSubmitTool>,
        Option<crate::builtin_tools::team::TaskReadArtifactTool>,
        Option<crate::builtin_tools::team::TaskReviewTool>,
        Option<crate::builtin_tools::team::SessionCollaborateTool>,
        Option<crate::builtin_tools::team::SessionTurnTool>,
        Option<crate::builtin_tools::team::SessionReadTool>,
        crate::builtin_tools::google_meet::GoogleMeetTool,
        crate::builtin_tools::skill_status::SkillStatusTool,
        crate::builtin_tools::skill_install::SkillInstallTool,
        crate::builtin_tools::skill_manage::SkillManageTool,
        Option<crate::builtin_tools::note_manage::NoteManageTool>,
        Option<crate::builtin_tools::session_complete::SessionCompleteTool>,
        Option<crate::builtin_tools::memory_reflect::MemoryReflectTool>,
    ) {
        let current_agent_id = current_agent_id.to_string();
        // Add message_send + inbox_read tools (if MessageRouter / Inbox are available)
        let (message_send_tool, inbox_read_tool) = {
            let current_agent_id = current_agent_id.clone();
            let send = config.message_router.as_ref().and_then(|router| {
                let team_store = config.team_store.as_ref()?;
                use crate::builtin_tools::team::MessageSendTool;
                Some(MessageSendTool::new(
                    Arc::clone(router),
                    Arc::clone(team_store),
                    current_agent_id.clone(),
                ))
            });
            let read = config.inbox.as_ref().map(|inbox| {
                use crate::builtin_tools::team::InboxReadTool;
                InboxReadTool::new(Arc::clone(inbox), current_agent_id)
            });

            // Register parameter schemas
            {
                use crate::tools::AlephTool;
                let mut defs: Vec<crate::tool_metadata::ToolDefinition> = Vec::new();
                if let Some(ref s) = send {
                    defs.push(s.definition());
                }
                if let Some(ref r) = read {
                    defs.push(r.definition());
                }
                for td in &defs {
                    let mut ut = UnifiedTool::new(
                        format!("builtin:{}", td.name),
                        &td.name,
                        &td.description,
                        ToolSource::Builtin,
                    );
                    ut = ut.with_parameters_schema(td.parameters.clone());
                    tools.insert(td.name.clone(), ut);
                }
            }

            if send.is_some() || read.is_some() {
                info!(
                    "Registered team messaging tools (message_send={}, inbox_read={})",
                    send.is_some(),
                    read.is_some()
                );
            }
            (send, read)
        };

        // Add plan approval tools (require MessageRouter + ArtifactStore + EventLogStore)
        let (plan_submit_tool, plan_resolve_tool) = {
            let plan_manager = match (
                config.message_router.as_ref(),
                config.artifact_store.as_ref(),
                config.event_store.as_ref(),
            ) {
                (Some(mr), Some(a_s), Some(es)) => {
                    Some(Arc::new(crate::teams::plans::PlanManager::new(
                        Arc::clone(mr),
                        Arc::clone(a_s),
                        Arc::clone(es),
                    )))
                }
                _ => None,
            };
            let submit = match (plan_manager.as_ref(), config.team_store.as_ref()) {
                (Some(pm), Some(ts)) => {
                    use crate::builtin_tools::team::PlanSubmitTool;
                    Some(PlanSubmitTool::new(
                        Arc::clone(pm),
                        Arc::clone(ts),
                        current_agent_id.clone(),
                    ))
                }
                _ => None,
            };
            let resolve = plan_manager.as_ref().map(|pm| {
                use crate::builtin_tools::team::PlanResolveTool;
                PlanResolveTool::new(Arc::clone(pm), current_agent_id.clone())
            });

            // Register parameter schemas
            {
                use crate::tools::AlephTool;
                let mut defs: Vec<crate::tool_metadata::ToolDefinition> = Vec::new();
                if let Some(ref s) = submit {
                    defs.push(s.definition());
                }
                if let Some(ref r) = resolve {
                    defs.push(r.definition());
                }
                for td in &defs {
                    let mut ut = UnifiedTool::new(
                        format!("builtin:{}", td.name),
                        &td.name,
                        &td.description,
                        ToolSource::Builtin,
                    );
                    ut = ut.with_parameters_schema(td.parameters.clone());
                    tools.insert(td.name.clone(), ut);
                }
            }

            if submit.is_some() || resolve.is_some() {
                info!("Registered plan approval tools (plan_submit, plan_resolve)");
            }
            (submit, resolve)
        };

        // Add worker-lifecycle tools (idle / request-shutdown / resolve-shutdown).
        // Hermes/ClawTeam-inspired wiring: MessageType::Idle / ShutdownRequest /
        // ShutdownApproved / ShutdownRejected were already in the schema but no
        // tools exposed them — this completes the LLM-facing layer.
        let (lifecycle_idle_tool, lifecycle_request_shutdown_tool, lifecycle_resolve_shutdown_tool) = {
            let current = current_agent_id.clone();
            let mk = |router: &Arc<crate::teams::messages::router::MessageRouter>,
                      team_store: &Arc<dyn crate::teams::TeamStore>| {
                use crate::builtin_tools::team::{
                    LifecycleIdleTool, LifecycleRequestShutdownTool, LifecycleResolveShutdownTool,
                };
                let idle = LifecycleIdleTool::new(
                    Arc::clone(router),
                    Arc::clone(team_store),
                    current.clone(),
                );
                let request = LifecycleRequestShutdownTool::new(
                    Arc::clone(router),
                    Arc::clone(team_store),
                    current.clone(),
                );
                let resolve =
                    LifecycleResolveShutdownTool::new(Arc::clone(router), current.clone());
                (idle, request, resolve)
            };

            let triad = match (config.message_router.as_ref(), config.team_store.as_ref()) {
                (Some(router), Some(team_store)) => Some(mk(router, team_store)),
                _ => None,
            };

            // Register parameter schemas (same pattern as plan_submit/resolve above).
            if let Some((ref idle, ref request, ref resolve)) = triad {
                use crate::tools::AlephTool;
                let defs: [crate::tool_metadata::ToolDefinition; 3] = [
                    idle.definition(),
                    request.definition(),
                    resolve.definition(),
                ];
                for td in &defs {
                    let mut ut = UnifiedTool::new(
                        format!("builtin:{}", td.name),
                        &td.name,
                        &td.description,
                        ToolSource::Builtin,
                    );
                    ut = ut.with_parameters_schema(td.parameters.clone());
                    tools.insert(td.name.clone(), ut);
                }
                info!(
                    "Registered worker lifecycle tools (lifecycle_idle, \
                     lifecycle_request_shutdown, lifecycle_resolve_shutdown)"
                );
            }

            match triad {
                Some((i, r, x)) => (Some(i), Some(r), Some(x)),
                None => (None, None, None),
            }
        };

        // Add task artifact tools (if ArtifactStore is available)
        let (task_submit_tool, task_read_artifact_tool) =
            if let Some(ref artifact_store) = config.artifact_store {
                use crate::builtin_tools::team::{TaskReadArtifactTool, TaskSubmitTool};

                let current_agent_id = current_agent_id.clone();
                let submit = TaskSubmitTool::new(
                    Arc::clone(artifact_store),
                    config.coord_task_store.clone(),
                    current_agent_id,
                );
                let read = TaskReadArtifactTool::new(Arc::clone(artifact_store));

                // Register parameter schemas
                {
                    use crate::tools::AlephTool;
                    let tool_defs = [submit.definition(), read.definition()];
                    for td in &tool_defs {
                        let mut ut = UnifiedTool::new(
                            format!("builtin:{}", td.name),
                            &td.name,
                            &td.description,
                            ToolSource::Builtin,
                        );
                        ut = ut.with_parameters_schema(td.parameters.clone());
                        tools.insert(td.name.clone(), ut);
                    }
                }

                info!("Registered task artifact tools (task_submit, task_read_artifact)");
                (Some(submit), Some(read))
            } else {
                (None, None)
            };

        // Leader task-review tool (strategy round 2) — needs a CoordTaskStore
        // (to flip task status) AND a TeamStore (soft leader authz).
        let task_review_tool = if let (Some(coord_store), Some(team_store)) =
            (&config.coord_task_store, &config.team_store)
        {
            use crate::builtin_tools::team::TaskReviewTool;
            let tool = TaskReviewTool::new(
                Arc::clone(coord_store),
                Arc::clone(team_store),
                current_agent_id.to_string(),
            );
            {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered task_review tool (leader acceptance)");
            Some(tool)
        } else {
            None
        };

        // Add collaborative session tools (if SessionCoordinator / SessionStore are available)
        let (session_collaborate_tool, session_turn_tool, session_read_tool) = {
            let current_agent_id = current_agent_id.clone();

            let collaborate = config.session_coordinator.as_ref().map(|coord| {
                crate::builtin_tools::team::SessionCollaborateTool::new(
                    Arc::clone(coord),
                    current_agent_id.clone(),
                )
            });
            let turn = config.session_coordinator.as_ref().map(|coord| {
                crate::builtin_tools::team::SessionTurnTool::new(
                    Arc::clone(coord),
                    current_agent_id,
                )
            });
            let read = config
                .session_store
                .as_ref()
                .map(|store| crate::builtin_tools::team::SessionReadTool::new(Arc::clone(store)));

            // Register parameter schemas
            {
                use crate::tools::AlephTool;
                let mut defs: Vec<crate::tool_metadata::ToolDefinition> = Vec::new();
                if let Some(ref c) = collaborate {
                    defs.push(c.definition());
                }
                if let Some(ref t) = turn {
                    defs.push(t.definition());
                }
                if let Some(ref r) = read {
                    defs.push(r.definition());
                }
                for td in &defs {
                    let mut ut = UnifiedTool::new(
                        format!("builtin:{}", td.name),
                        &td.name,
                        &td.description,
                        ToolSource::Builtin,
                    );
                    ut = ut.with_parameters_schema(td.parameters.clone());
                    tools.insert(td.name.clone(), ut);
                }
            }

            if collaborate.is_some() || read.is_some() {
                info!(
                    "Registered collaborative session tools (session_collaborate={}, session_turn={}, session_read={})",
                    collaborate.is_some(), turn.is_some(), read.is_some()
                );
            }
            (collaborate, turn, read)
        };

        // Skill management tools — always available
        // Phase 2: use the process-wide shared SkillSystem instead of a
        // throwaway empty instance; skill_status previously always reported 0.
        // Google Meet tool — wraps the optional out-of-core transport bridge.
        let google_meet_tool = crate::builtin_tools::google_meet::GoogleMeetTool::new(
            config.google_meet_bridge.clone(),
        );

        let skill_system = crate::skill::shared_skill_system().clone();
        let skill_status_tool =
            crate::builtin_tools::skill_status::SkillStatusTool::new(skill_system.clone());
        let skill_install_tool =
            crate::builtin_tools::skill_install::SkillInstallTool::new(skill_system.clone());
        let skill_manage_tool =
            crate::builtin_tools::skill_manage::SkillManageTool::new(skill_system);

        // Register skill management tool schemas
        {
            use crate::tools::AlephTool;
            let tool_defs = [
                skill_status_tool.definition(),
                skill_install_tool.definition(),
                skill_manage_tool.definition(),
            ];
            for td in &tool_defs {
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
        }
        info!("Registered skill management tools (skill_status, skill_install, skill_manage)");

        // Note management tool — unified CRUD for all note categories
        let note_manage_tool = if let Some(ref db) = config.memory_db {
            let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                dirs::home_dir().map_or_else(
                    || {
                        std::env::temp_dir()
                            .join("aleph")
                            .join("memory")
                            .join("note")
                    },
                    |p| p.join(".aleph").join("memory").join("note"),
                )
            });
            let mut tool =
                crate::builtin_tools::note_manage::NoteManageTool::new(memory_dir, db.clone())
                    .with_project_scoping(config.memory_project_scoped);
            // Wire the orientation hook so LLM note writes refresh index.md /
            // log.md like the compression / dream write paths do. Without
            // this, notes created via note_manage never invalidate the
            // orientation snapshot the prompt layer reads.
            if let Some(ref wiki) = config.orientation {
                tool = tool.with_orientation(Arc::clone(wiki));
            }
            // Wire the embedder so `query` runs hybrid (vector + FTS) search;
            // without it the action silently degrades to FTS-only, which the
            // unicode61 tokenizer makes near-useless for CJK queries.
            if let Some(ref embedder) = config.embedder {
                tool = tool.with_embedder(Arc::clone(embedder));
            }
            // Wire the event-sourcing handler so note create/update/delete
            // actions feed the per-note event log that the memory_timeline
            // tool reads. Event-log only (no note_indexer) — note_manage owns
            // the notes-filesystem write path.
            if let Some(ref state_db) = config.state_db {
                let handler = Arc::new(crate::memory::events::handler::MemoryCommandHandler::new(
                    Arc::clone(state_db),
                ));
                tool = tool.with_command_handler(handler);
            }

            // Register note_manage tool schema
            {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered note_manage tool");
            Some(tool)
        } else {
            None
        };

        // Session-complete tool — requires memory_db (same guard as note_manage)
        let session_complete_tool = if let Some(ref db) = config.memory_db {
            let agent_id = config
                .current_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            let mut tool = crate::builtin_tools::session_complete::SessionCompleteTool::new(
                db.clone(),
                agent_id,
            );
            if let Some(ref reg) = config.capture_registry {
                tool = tool.with_capture_registry(Arc::clone(reg));
            }

            // Register tool schema
            {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered session_complete tool");
            Some(tool)
        } else {
            None
        };

        // Memory-reflect tool — always constructed; reflector injected later by Task 8.
        // Registration is gated on injection_mode (same rule as the other retrieval tools).
        let expose_retrieval_tools = matches!(
            config.injection_mode,
            crate::config::types::memory::MemoryInjectionMode::Tools
                | crate::config::types::memory::MemoryInjectionMode::Hybrid,
        );
        let memory_reflect_tool = {
            let agent_id = config
                .current_agent_id
                .clone()
                .unwrap_or_else(|| "main".to_string());
            let tool = crate::builtin_tools::memory_reflect::MemoryReflectTool::new(agent_id);

            // Register tool schema only when retrieval tools are exposed
            if expose_retrieval_tools {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
                info!("Registered memory_reflect tool");
            }
            Some(tool)
        };

        // recall_context tool — gated on injection_mode (retrieval tools only)
        if expose_retrieval_tools {
            use crate::builtin_tools::RecallContextTool;
            let schema = serde_json::to_value(schemars::schema_for!(
                crate::builtin_tools::recall_context::RecallContextArgs
            ))
            .unwrap_or_else(|e| {
                warn!("Failed to serialize schema for recall_context: {}", e);
                serde_json::Value::Object(Default::default())
            });
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", RecallContextTool::NAME),
                RecallContextTool::NAME,
                RecallContextTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(schema);
            tools.insert(RecallContextTool::NAME.to_string(), ut);
            info!("Registered recall_context tool");
        }

        // memory_trace tool — gated on injection_mode (retrieval tools only)
        if expose_retrieval_tools {
            use crate::builtin_tools::memory_trace::MemoryTraceTool;
            let schema = serde_json::to_value(schemars::schema_for!(
                crate::builtin_tools::memory_trace::MemoryTraceArgs
            ))
            .unwrap_or_else(|e| {
                warn!("Failed to serialize schema for memory_trace: {}", e);
                serde_json::Value::Object(Default::default())
            });
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", MemoryTraceTool::NAME),
                MemoryTraceTool::NAME,
                MemoryTraceTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(schema);
            tools.insert(MemoryTraceTool::NAME.to_string(), ut);
            info!("Registered memory_trace tool");
        }

        // note_graph_query tool — read-only graph interrogation (retrieval tools only)
        if expose_retrieval_tools {
            use crate::builtin_tools::note_graph_query::NoteGraphQueryTool;
            let schema = serde_json::to_value(schemars::schema_for!(
                crate::builtin_tools::note_graph_query::NoteGraphQueryArgs
            ))
            .unwrap_or_else(|e| {
                warn!("Failed to serialize schema for note_graph_query: {}", e);
                serde_json::Value::Object(Default::default())
            });
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", NoteGraphQueryTool::NAME),
                NoteGraphQueryTool::NAME,
                NoteGraphQueryTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(schema);
            tools.insert(NoteGraphQueryTool::NAME.to_string(), ut);
            info!("Registered note_graph_query tool");
        }

        (
            message_send_tool,
            inbox_read_tool,
            plan_submit_tool,
            plan_resolve_tool,
            lifecycle_idle_tool,
            lifecycle_request_shutdown_tool,
            lifecycle_resolve_shutdown_tool,
            task_submit_tool,
            task_read_artifact_tool,
            task_review_tool,
            session_collaborate_tool,
            session_turn_tool,
            session_read_tool,
            google_meet_tool,
            skill_status_tool,
            skill_install_tool,
            skill_manage_tool,
            note_manage_tool,
            session_complete_tool,
            memory_reflect_tool,
        )
    }
}
