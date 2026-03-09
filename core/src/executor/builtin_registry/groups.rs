//! Tool group definitions for Panel UI display.
//!
//! Groups are display-only metadata — they don't affect tool filtering.
//! TOML config uses individual tool names/globs, not group IDs.

use serde::Serialize;

/// A logical group of tools for UI display
#[derive(Debug, Clone, Serialize)]
pub struct ToolGroup {
    /// Group identifier (e.g., "search_web")
    pub id: &'static str,
    /// Human-readable group name
    pub name: &'static str,
    /// Tool names belonging to this group
    pub tools: &'static [&'static str],
}

/// All tool groups (ordered for UI display)
pub static TOOL_GROUPS: &[ToolGroup] = &[
    ToolGroup {
        id: "search_web",
        name: "搜索与网络",
        tools: &["search", "web_fetch"],
    },
    ToolGroup {
        id: "file_code",
        name: "文件与代码",
        tools: &["file_ops", "bash", "code_exec", "pdf_generate"],
    },
    ToolGroup {
        id: "memory_knowledge",
        name: "记忆与知识",
        tools: &["memory_search", "memory_browse", "read_skill", "list_skills"],
    },
    ToolGroup {
        id: "content_gen",
        name: "内容生成",
        tools: &["generate_image"],
    },
    ToolGroup {
        id: "system_config",
        name: "系统与配置",
        tools: &["desktop", "config_read", "config_update"],
    },
    ToolGroup {
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
            "browser_profile",
        ],
    },
    ToolGroup {
        id: "media",
        name: "媒体理解",
        tools: &["media_understand", "audio_transcribe", "document_extract"],
    },
    ToolGroup {
        id: "agent_mgmt",
        name: "Agent 管理",
        tools: &[
            "agent_create",
            "agent_switch",
            "agent_list",
            "agent_delete",
            "sessions_list",
            "sessions_send",
            "subagent_spawn",
            "subagent_steer",
            "subagent_kill",
            "escalate_task",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::builtin_registry::BUILTIN_TOOL_DEFINITIONS;

    #[test]
    fn test_all_builtin_tools_have_a_group() {
        let grouped: Vec<&str> = TOOL_GROUPS
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
        for group in TOOL_GROUPS {
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
