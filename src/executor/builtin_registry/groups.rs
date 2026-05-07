//! Tool category definitions for Panel UI display.
//!
//! Categories are display-only metadata — they don't affect tool filtering.
//! TOML config uses individual tool names/globs, not category IDs.

use serde::Serialize;

/// A logical group of tools for UI display
#[derive(Debug, Clone, Serialize)]
pub struct ToolCategory {
    /// Group identifier (e.g., "search_web")
    pub id: &'static str,
    /// Human-readable group name
    pub name: &'static str,
    /// Tool names belonging to this group
    pub tools: &'static [&'static str],
}

/// All tool categorys (ordered for UI display)
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
            "bash",
            "code_exec",
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
            "remember",
            "note_manage",
            "session_search",
            "skill_list",
            "skill_read",
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
            "desktop_check_permissions",
            "self_manage",
            "read_config_guide",
            "vault_store",
            "channel_pairing",
            "voice_mode_set",
            "skill_status",
            "skill_install",
            "skill_manage",
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
        id: "spawn",
        name: "子 Agent 派发",
        tools: &["subagent_spawn", "subagent_steer", "subagent_kill"],
    },
    ToolCategory {
        id: "delegate",
        name: "Agent 间通信",
        tools: &["session_send", "session_list", "gateway_route"],
    },
    ToolCategory {
        id: "team",
        name: "团队协调",
        tools: &[
            "team_create",
            "team_delegate",
            "team_status",
            "team_disband",
            "team_member_remove",
            "team_digest",
            "message_send",
            "inbox_read",
            "task_create",
            "task_update",
            "task_list",
            "task_wait",
            "task_submit",
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
        tools: &["agent_create", "agent_list", "agent_delete"],
    },
    ToolCategory {
        id: "session_mgmt",
        name: "会话管理",
        tools: &["session_new", "session_rename"],
    },
    ToolCategory {
        id: "automation",
        name: "自动化",
        tools: &["cron_manage", "clawhub"],
    },
    ToolCategory {
        id: "acp",
        name: "外部代码 Agent",
        tools: &["acp_delegate", "acp_switch"],
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
