//! Tool category definitions for Panel UI display.
//!
//! Categories are display-only metadata — they don't affect tool filtering.
//! TOML config uses individual tool names/globs, not category IDs.

use serde::Serialize;

/// A logical group of tools for UI display
#[derive(Debug, Clone, Serialize)]
pub struct ToolCategory {
    /// Group identifier (e.g., "`search_web`")
    pub id: &'static str,
    /// Human-readable group name
    pub name: &'static str,
    /// Tool names belonging to this group
    pub tools: &'static [&'static str],
}

/// All tool categories (ordered for UI display)
pub static TOOL_CATEGORIES: &[ToolCategory] = &[
    ToolCategory {
        id: "search_web",
        name: "搜索与网络",
        tools: &["search", "web_fetch"],
    },
    ToolCategory {
        id: "file_code",
        name: "文件与代码",
        tools: &[
            "file_ops",
            "file_read",
            "file_write",
            "file_edit",
            "apply_patch",
            "bash",
            "code_exec",
            "code_check",
            "pdf_generate",
        ],
    },
    ToolCategory {
        id: "memory_knowledge",
        name: "记忆与知识",
        tools: &[
            "memory_search",
            "memory_browse",
            "memory_explore",
            "memory_timeline",
            "memory_reflect",
            "memory_trace",
            "recall_context",
            "recall_events",
            "governance_metrics",
            "ctx_search",
            "remember",
            "note_manage",
            "note_orient",
            "note_schema",
            "note_graph_query",
            "user_profile",
            "session_complete",
            "flag_user_correction",
            "session_search",
            "skill_list",
            "skill_read",
            "scratchpad",
        ],
    },
    ToolCategory {
        id: "content_gen",
        name: "内容生成",
        tools: &[
            "image_generate",
            "video_generate",
            "audio_generate",
            "speech_generate",
            "media_send",
            "artifact_publish",
        ],
    },
    ToolCategory {
        id: "system_config",
        name: "系统与配置",
        tools: &[
            "desktop",
            "desktop_ax_query_focused",
            "desktop_ax_query_tree",
            "desktop_ax_query_by_role",
            "desktop_ax_snapshot",
            "desktop_check_permissions",
            "desktop_gui_locate",
            "desktop_som",
            "self_manage",
            "hooks_manage",
            "self_config",
            "read_config_guide",
            "config_audit",
            "doctor",
            "select_model",
            "list_models",
            "moa",
            "vault_store",
            "channel_pairing",
            "google_meet",
            "voice_mode_set",
            "local_voice",
            "ask_user",
            "skill_status",
            "skill_install",
            "skill_manage",
            "system",
            "pim",
            "permission",
            "automation",
            "media",
            "goal",
            "loop",
            "loop_graph",
            "strategy",
        ],
    },
    ToolCategory {
        id: "browser",
        name: "浏览器",
        tools: &[
            "browser_open",
            "browser_click",
            "browser_type",
            "browser_screenshot",
            "browser_snapshot",
            "browser_navigate",
            "browser_tabs",
            "browser_select",
            "browser_evaluate",
            "browser_fill_form",
            "browser_press_key",
            "browser_wait_for",
            "browser_console",
            "browser_hover",
            "browser_scroll",
            "browser_pdf",
            "browser_network",
            "browser_dialog",
            "browser_drag",
            "browser_upload",
            "browser_resize",
            "browser_emulate",
            "browser_cookies",
            "browser_session",
            "browser_profile",
        ],
    },
    ToolCategory {
        id: "media",
        name: "媒体理解",
        tools: &["media_understand", "audio_transcribe", "document_extract"],
    },
    // -- Multi-agent collaboration modes --
    ToolCategory {
        id: "delegate",
        name: "Agent 间通信",
        tools: &[
            "session_send",
            "session_list",
            "gateway_route",
            "channel_message",
            "channel_directory",
            "channel_outbox",
        ],
    },
    ToolCategory {
        id: "team",
        name: "团队协调",
        tools: &[
            "team_create",
            "team_from_template",
            "team_snapshot",
            "team_usage",
            "team_workflow_canvas",
            "team_delegate",
            "team_status",
            "team_disband",
            "team_set_protocol",
            "team_member_add",
            "team_member_remove",
            "team_digest",
            "message_send",
            "inbox_read",
            "plan_submit",
            "plan_resolve",
            "lifecycle_idle",
            "lifecycle_request_shutdown",
            "lifecycle_resolve_shutdown",
            "task_create",
            "task_update",
            "task_list",
            "task_wait",
            "task_comment",
            "team_acp_member",
            "workflow_step_review",
            "workflow",
            "team_task_control",
            "task_exit_journal",
            "task_submit",
            "task_review",
            "task_read_artifact",
            "session_collaborate",
            "session_turn",
            "session_read",
        ],
    },
    // -- Infrastructure --
    ToolCategory {
        id: "agent_mgmt",
        name: "Agent 管理",
        tools: &[
            "agent_create",
            "agent_list",
            "agent_switch",
            "agent_unbind",
            "agent_delete",
            "agent_update",
            "agent_info",
            "agent_identity",
        ],
    },
    ToolCategory {
        id: "session_mgmt",
        name: "会话管理",
        tools: &[
            "session_new",
            "session_compact",
            "session_rename",
            "session_set_mode",
        ],
    },
    ToolCategory {
        id: "automation",
        name: "自动化",
        tools: &["cron_manage"],
    },
    ToolCategory {
        id: "extensions_store",
        name: "扩展商店",
        tools: &[
            "hub_catalog_search",
            "hub_catalog_sync",
            "hub_resolve_spec",
            "hub_fetch_docs",
            "hub_install_run",
            "hub_install_verify",
        ],
    },
    ToolCategory {
        id: "acp",
        name: "外部代码 Agent",
        tools: &["acp_delegate", "acp_switch", "acp_session_control"],
    },
    ToolCategory {
        id: "a2a",
        name: "远程 A2A Agent",
        tools: &["a2a_delegate", "a2a_agents"],
    },
    ToolCategory {
        id: "heartbeat",
        name: "心跳监控",
        tools: &[
            "heartbeat_list",
            "heartbeat_create",
            "heartbeat_update",
            "heartbeat_delete",
            "heartbeat_toggle",
            "heartbeat_report",
        ],
    },
    ToolCategory {
        id: "cluster",
        name: "集群节点",
        tools: &[
            "node_list",
            "node_invoke",
            "node_invoke_many",
            "node_file",
            "node_manage",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::builtin_registry::BUILTIN_TOOL_DEFINITIONS;

    #[test]
    fn test_all_builtin_tools_have_a_group() {
        let grouped: Vec<&str> = TOOL_CATEGORIES
            .iter()
            .flat_map(|g| g.tools.iter().copied())
            .collect();

        for def in BUILTIN_TOOL_DEFINITIONS.iter() {
            assert!(
                grouped.contains(&def.name),
                "Builtin tool '{}' is not in any group",
                def.name
            );
        }
    }

    /// The reverse direction of `test_all_builtin_tools_have_a_group`, scoped to
    /// the one group other code derives an invariant from: `agents::registry`
    /// reads `extensions_store` to prove the read-only verifier denies every Hub
    /// tool, so a typo'd or stale name here would make that check assert about
    /// nothing.
    ///
    /// Deliberately **not** applied to every group: builtins reach the registry by
    /// two paths — `BUILTIN_TOOL_DEFINITIONS` (config-gated tools) and
    /// `builder/core_tools.rs` (always-on core tools, registered by inline
    /// `reg("name", …)` calls with no list to join against). `scratchpad` is a
    /// live tool that legitimately appears in a group and not in the definitions
    /// table. Widening this assertion means first giving the core path a data
    /// source; until then it would fail on working tools.
    #[test]
    fn extensions_store_group_names_only_defined_tools() {
        let defined: Vec<&str> = BUILTIN_TOOL_DEFINITIONS.iter().map(|d| d.name).collect();
        let family = TOOL_CATEGORIES
            .iter()
            .find(|c| c.id == "extensions_store")
            .expect("extensions_store tool category");
        for tool in family.tools {
            assert!(
                defined.contains(tool),
                "extensions_store lists '{tool}', which is not in BUILTIN_TOOL_DEFINITIONS"
            );
        }
    }

    #[test]
    fn test_no_duplicate_tools_across_groups() {
        let mut seen = std::collections::HashSet::new();
        for group in TOOL_CATEGORIES {
            for tool in group.tools {
                assert!(
                    seen.insert(tool),
                    "Tool '{}' appears in multiple groups",
                    tool
                );
            }
        }
    }
}
