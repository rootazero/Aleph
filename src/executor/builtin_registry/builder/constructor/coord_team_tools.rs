//! Task-coordination and team-management tool construction for
//! `BuiltinToolRegistry`.
//!
//! Extracted from `constructor.rs` to keep file sizes manageable. Builds the
//! optional task-coordination tools (create/update/list/wait/comment) and the
//! full team-management suite (create/delegate/status/disband/protocol/members/
//! templates/snapshot/usage/acp-member/workflow-canvas/step-review/workflow/
//! task-control/exit-journal/digest), registering their parameter schemas into
//! the shared `tools` map.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tracing::{info, warn};

use super::{BuiltinToolConfig, BuiltinToolRegistry};
use crate::tool_metadata::{ToolSource, UnifiedTool};

#[allow(clippy::type_complexity)]
impl BuiltinToolRegistry {
    /// Build task-coordination and team-management tools and register schemas.
    pub(crate) fn build_coord_team_tools(
        config: &BuiltinToolConfig,
        tools: &mut HashMap<String, UnifiedTool>,
        boot_fallback_agent_id: &str,
    ) -> (
        Option<crate::builtin_tools::task_manage::TaskCreateTool>,
        Option<crate::builtin_tools::task_manage::TaskUpdateTool>,
        Option<crate::builtin_tools::task_manage::TaskListTool>,
        Option<crate::builtin_tools::task_manage::TaskWaitTool>,
        Option<crate::builtin_tools::team::TaskCommentTool>,
        Option<crate::builtin_tools::team::TeamCreateTool>,
        Option<crate::builtin_tools::team::TeamDelegateTool>,
        Option<crate::builtin_tools::team::TeamStatusTool>,
        Option<crate::builtin_tools::team::TeamDisbandTool>,
        Option<crate::builtin_tools::team::TeamSetProtocolTool>,
        Option<crate::builtin_tools::team::TeamMemberAddTool>,
        Option<crate::builtin_tools::team::TeamMemberRemoveTool>,
        Option<crate::builtin_tools::team::TeamFromTemplateTool>,
        Option<crate::builtin_tools::team::TeamSnapshotTool>,
        Option<crate::builtin_tools::team::TeamUsageTool>,
        Option<crate::builtin_tools::team::TeamAcpMemberTool>,
        Option<crate::builtin_tools::team::TeamWorkflowCanvasTool>,
        Option<crate::builtin_tools::team::WorkflowStepReviewTool>,
        Option<crate::builtin_tools::workflow_tool::WorkflowTool>,
        Option<crate::builtin_tools::team::TeamTaskControlTool>,
        Option<crate::builtin_tools::team::TaskExitJournalTool>,
        Option<crate::builtin_tools::team::TeamDigestTool>,
    ) {
        // NOT the acting identity. Every tool below re-resolves its actor per
        // call through `builtin_tools::acting_agent::acting_agent_id`, which
        // reads the running turn's `TURN_CONTEXT`. What is baked in here is
        // only the fallback for calls made outside a turn scope (direct
        // construction in tests, background paths). Boot cannot know who will
        // be running in an hour, and it used to answer anyway — always with
        // the literal "main".
        let current_agent_id = boot_fallback_agent_id.to_string();
        // Add task coordination tools (if CoordTaskStore is available)
        let (task_create_tool, task_update_tool, task_list_tool, task_wait_tool, task_comment_tool) =
            if let Some(ref store) = config.coord_task_store {
                use crate::builtin_tools::task_manage::{
                    TaskCreateTool, TaskListTool, TaskUpdateTool, TaskWaitTool,
                };
                use crate::builtin_tools::team::TaskCommentTool;

                let create = TaskCreateTool::new(Arc::clone(store), config.dispatch_signal.clone())
                    .with_team_store(config.team_store.clone());
                // The LIST surface owes the same gate the addressed siblings
                // below owe, as a retain rather than a refusal — `teams/scoped.rs`'s
                // census names `task_list` by name. Without it, `task_list {}`
                // returned every coord task in the process: another principal's
                // team ids, task ids, owners and task SUBJECTS.
                let list =
                    TaskListTool::new(Arc::clone(store)).with_team_store(config.team_store.clone());
                // task_comment is unconditional once the coord store exists —
                // it doesn't depend on the agent message bus.
                let comment = TaskCommentTool::new(
                    Arc::clone(store),
                    config
                        .current_agent_id
                        .clone()
                        .unwrap_or_else(|| "main".to_string()),
                )
                .with_team_store(config.team_store.clone());

                // TaskUpdateTool and TaskWaitTool derive purely from the coord
                // store; the store's own GlobalBus broadcast is what wakes
                // `task_wait`, so neither tool needs an event bus injected.
                //
                // Both DO need the team store, for the same reason
                // `TaskCommentTool` three lines above does: a coord task is
                // addressed by a bare id and lives in a different database from
                // the teams the `ScopedTeamStore` decorator wraps, so the
                // decorator cannot see either call. The omission was accidental
                // rather than reasoned — `config.team_store` was already in
                // scope here, and six sibling tools were already passing it.
                let update = Some(
                    TaskUpdateTool::new(Arc::clone(store))
                        .with_team_store(config.team_store.clone()),
                );
                let wait = Some(
                    TaskWaitTool::new(Arc::clone(store)).with_team_store(config.team_store.clone()),
                );

                // Register parameter schemas for task tools
                {
                    use crate::tools::AlephTool;
                    let mut defs: Vec<crate::tool_metadata::ToolDefinition> =
                        vec![create.definition(), list.definition(), comment.definition()];
                    if let Some(ref u) = update {
                        defs.push(u.definition());
                    }
                    if let Some(ref w) = wait {
                        defs.push(w.definition());
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

                info!("Registered task coordination tools (incl. task_comment)");
                (Some(create), update, Some(list), wait, Some(comment))
            } else {
                (None, None, None, None, None)
            };
        // Add team management tools (if TeamStore + CoordTaskStore are available)
        let (
            team_create_tool,
            team_delegate_tool,
            team_status_tool,
            team_disband_tool,
            team_set_protocol_tool,
            team_member_add_tool,
            team_member_remove_tool,
            team_from_template_tool,
        ) = if let (Some(ref store), Some(ref coord_store)) =
            (&config.team_store, &config.coord_task_store)
        {
            use crate::builtin_tools::team::{
                TeamCreateTool, TeamDelegateTool, TeamDisbandTool, TeamFromTemplateTool,
                TeamMemberAddTool, TeamMemberRemoveTool, TeamSetProtocolTool, TeamStatusTool,
            };

            let agent_registry = config
                .agent_registry
                .clone()
                .unwrap_or_else(|| Arc::new(crate::gateway::agent_instance::AgentRegistry::new()));

            let sm_for_teams = config
                .gateway_context
                .as_ref()
                .map(|ctx| Arc::clone(ctx.session_store()))
                .or_else(|| config.session_manager.clone())
                .or_else(|| match crate::gateway::SessionManager::with_defaults() {
                    Ok(sm) => Some(Arc::new(sm)),
                    Err(e) => {
                        warn!(
                            "Failed to create fallback SessionManager for team tools: {}",
                            e
                        );
                        None
                    }
                });

            if sm_for_teams.is_none() {
                warn!("Team management tools disabled: SessionManager not available");
            }

            let create = sm_for_teams.as_ref().map(|sm| {
                TeamCreateTool::new(
                    Arc::clone(store),
                    Arc::clone(&agent_registry),
                    config.agent_manager.clone(),
                    Arc::clone(sm),
                    current_agent_id.clone(),
                )
            });
            let delegate = TeamDelegateTool::new(
                Arc::clone(store),
                Arc::clone(coord_store),
                config.artifact_store.clone(),
            );
            let status = TeamStatusTool::new(Arc::clone(store), Arc::clone(coord_store));
            let disband = TeamDisbandTool::new(Arc::clone(store)).with_cleanup_stores(
                config.message_store.clone(),
                config.session_store.clone(),
                config.event_store.clone(),
            );
            let set_protocol = TeamSetProtocolTool::new(Arc::clone(store));
            let member_add = TeamMemberAddTool::new(
                Arc::clone(store),
                Arc::clone(&agent_registry),
                current_agent_id.clone(),
            );
            let member_remove =
                TeamMemberRemoveTool::new(Arc::clone(store), current_agent_id.clone());

            // team_from_template reuses the same agent_registry + session_store
            // wiring that team_create depends on, so we construct it here once
            // the TeamStore + CoordTaskStore guard is satisfied.
            let from_template = sm_for_teams.as_ref().map(|sm| {
                TeamFromTemplateTool::new(
                    Arc::clone(store),
                    Arc::clone(coord_store),
                    Arc::clone(&agent_registry),
                    config.agent_manager.clone(),
                    Arc::clone(sm),
                    current_agent_id.clone(),
                )
            });

            // Register parameter schemas for team tools
            {
                use crate::tools::AlephTool;
                let mut tool_defs: Vec<crate::tool_metadata::ToolDefinition> = vec![
                    delegate.definition(),
                    status.definition(),
                    disband.definition(),
                    set_protocol.definition(),
                    member_add.definition(),
                    member_remove.definition(),
                ];
                if let Some(ref c) = create {
                    tool_defs.push(c.definition());
                }
                if let Some(ref ft) = from_template {
                    tool_defs.push(ft.definition());
                }
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

            info!("Registered team management tools (team_create={:?}, team_delegate, team_status, team_disband, team_member_add, team_member_remove, team_from_template={:?})", create.is_some(), from_template.is_some());
            (
                create,
                Some(delegate),
                Some(status),
                Some(disband),
                Some(set_protocol),
                Some(member_add),
                Some(member_remove),
                from_template,
            )
        } else {
            (None, None, None, None, None, None, None, None)
        };

        // Build team_snapshot when TeamStore + CoordTaskStore + SqliteSnapshotStore
        // are all present. The snapshot store is constructed alongside coord
        // in the boot path so they share one Connection (see config.rs comment).
        let team_snapshot_tool =
            if let (Some(ref team_store), Some(ref coord_store), Some(ref snap_store)) = (
                &config.team_store,
                &config.coord_task_store,
                &config.snapshot_store,
            ) {
                use crate::builtin_tools::team::TeamSnapshotTool;
                let tool = TeamSnapshotTool::new(
                    Arc::clone(team_store),
                    Arc::clone(coord_store),
                    Arc::clone(snap_store),
                    current_agent_id.clone(),
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
                info!("Registered team_snapshot tool");
                Some(tool)
            } else {
                None
            };

        // Build team_usage when TeamStore + StateDatabase are both present.
        // The state.db lives in ~/.aleph/data and holds the task_traces rows
        // that the aggregator scans; without it the tool returns NotAvailable.
        let team_usage_tool = if let (Some(ref team_store), Some(ref state_db)) =
            (&config.team_store, &config.state_db)
        {
            use crate::builtin_tools::team::TeamUsageTool;
            let tool = TeamUsageTool::new(
                Arc::clone(team_store),
                Arc::clone(state_db),
                current_agent_id.clone(),
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
            info!("Registered team_usage tool");
            Some(tool)
        } else {
            None
        };

        // Build team_acp_member when TeamStore is present. The tool itself
        // does not need the ACP manager — it only mutates membership; the
        // dispatcher is the one that consumes the routing fields at task
        // run time.
        let team_acp_member_tool = if let Some(ref team_store) = config.team_store {
            use crate::builtin_tools::team::TeamAcpMemberTool;
            let tool = TeamAcpMemberTool::new(Arc::clone(team_store), current_agent_id.clone());
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
            info!("Registered team_acp_member tool");
            Some(tool)
        } else {
            None
        };

        // Build team_workflow_canvas when CoordTaskStore is present. Powers
        // the DAG ↔ JSON Canvas bridge so the LLM can produce / read plan
        // diagrams without leaving conversation (R8).
        let team_workflow_canvas_tool = if let Some(ref coord_store) = config.coord_task_store {
            use crate::builtin_tools::team::TeamWorkflowCanvasTool;
            let tool = TeamWorkflowCanvasTool::new(Arc::clone(coord_store))
                .with_team_store(config.team_store.clone());
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
            info!("Registered team_workflow_canvas tool");
            Some(tool)
        } else {
            None
        };

        // Build workflow_step_review when CoordTaskStore is present. Lets
        // a lead agent (or panel user via RPC) approve / reject / retry /
        // skip individual workflow steps without touching the dispatcher
        // mid-flight.
        let workflow_step_review_tool = if let Some(ref coord_store) = config.coord_task_store {
            use crate::builtin_tools::team::WorkflowStepReviewTool;
            let tool =
                WorkflowStepReviewTool::new(Arc::clone(coord_store), current_agent_id.clone())
                    .with_team_store(config.team_store.clone());
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
            info!("Registered workflow_step_review tool");
            Some(tool)
        } else {
            None
        };

        // Build the workflow-template tool when CoordTaskStore is present.
        // `run` materialises a saved template into the coord-task DAG; the
        // dispatch signal wakes the team dispatcher so it starts promptly.
        let workflow_tool = if let Some(ref coord_store) = config.coord_task_store {
            use crate::builtin_tools::workflow_tool::WorkflowTool;
            let tool = WorkflowTool::new(Arc::clone(coord_store), config.dispatch_signal.clone())
                .with_team_store(config.team_store.clone())
                .with_planner_provider(config.planner_provider.clone());
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
            info!("Registered workflow template tool");
            Some(tool)
        } else {
            None
        };

        // Build team_task_control when CoordTaskStore is present. Admin-
        // context complement of workflow_step_review (pause/resume/retry/skip
        // without requiring a finished run). R3 — ClawTeam task-control parity.
        let team_task_control_tool = if let Some(ref coord_store) = config.coord_task_store {
            use crate::builtin_tools::team::TeamTaskControlTool;
            let tool = TeamTaskControlTool::new(Arc::clone(coord_store))
                .with_team_store(config.team_store.clone());
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
            info!("Registered team_task_control tool");
            Some(tool)
        } else {
            None
        };

        // Build task_exit_journal when CoordTaskStore is present. R3 —
        // ClawTeam parity. Agent self-call on task wrap-up; output feeds
        // trace + replay UI.
        let task_exit_journal_tool = if let Some(ref coord_store) = config.coord_task_store {
            use crate::builtin_tools::team::TaskExitJournalTool;
            let tool = TaskExitJournalTool::new(Arc::clone(coord_store), current_agent_id.clone())
                .with_team_store(config.team_store.clone());
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
            info!("Registered task_exit_journal tool");
            Some(tool)
        } else {
            None
        };

        // Add team_digest tool (if EventLogStore + TeamStore are available)
        let team_digest_tool = if let (Some(ref event_store), Some(ref team_store)) =
            (&config.event_store, &config.team_store)
        {
            use crate::builtin_tools::team::TeamDigestTool;

            let current_agent_id = current_agent_id.clone();
            let digest = TeamDigestTool::new(
                Arc::clone(team_store),
                Arc::clone(event_store),
                current_agent_id,
            );

            // Register parameter schema
            {
                use crate::tools::AlephTool;
                let td = digest.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }

            info!("Registered team_digest tool");
            Some(digest)
        } else {
            None
        };

        (
            task_create_tool,
            task_update_tool,
            task_list_tool,
            task_wait_tool,
            task_comment_tool,
            team_create_tool,
            team_delegate_tool,
            team_status_tool,
            team_disband_tool,
            team_set_protocol_tool,
            team_member_add_tool,
            team_member_remove_tool,
            team_from_template_tool,
            team_snapshot_tool,
            team_usage_tool,
            team_acp_member_tool,
            team_workflow_canvas_tool,
            workflow_step_review_tool,
            workflow_tool,
            team_task_control_tool,
            task_exit_journal_tool,
            team_digest_tool,
        )
    }
}
