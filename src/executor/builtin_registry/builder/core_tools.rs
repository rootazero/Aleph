use std::collections::HashMap;

use crate::builtin_tools::skill_reader::{
    ListSkillsTool as SkillListTool, ReadSkillTool as SkillReadTool,
};
use crate::builtin_tools::{
    AutomationTool, CodeExecTool, CtxSearchTool, DesktopTool, FileEditTool, FileReadTool,
    FileWriteTool, MediaTool, PdfGenerateTool, PermissionTool, PimTool, ReadConfigGuideTool,
    RecallEventsTool, ScratchpadTool, SearchTool, SelfManageTool, SystemTool,
};
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;

use super::BuiltinToolRegistry;

impl BuiltinToolRegistry {
    /// Register always-available core tool metadata with JSON Schema parameters.
    pub(crate) fn register_core_tools(tools: &mut HashMap<String, UnifiedTool>) {
        use schemars::schema_for;

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

        reg(
            tools,
            "search",
            SearchTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::search::SearchArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "web_fetch",
            "Fetch and read content from a URL",
            serde_json::to_value(schema_for!(crate::builtin_tools::web_fetch::WebFetchArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "file_ops",
            "File system operations - list, move, copy, delete, mkdir, search, batch_move, organize",
            serde_json::to_value(schema_for!(crate::builtin_tools::file_ops::FileOpsArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "file_read",
            FileReadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::file_ops::read::FileReadArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "file_write",
            FileWriteTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::file_ops::write::FileWriteArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "file_edit",
            FileEditTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::file_ops::edit::FileEditArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "bash",
            "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
            serde_json::to_value(schema_for!(crate::builtin_tools::bash_exec::BashExecArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "code_exec",
            CodeExecTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::code_exec::CodeExecArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "code_check",
            crate::builtin_tools::code_check::CodeCheckTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::code_check::CodeCheckArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "pdf_generate",
            PdfGenerateTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::pdf_generate::PdfGenerateArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "skill_list",
            SkillListTool::DESCRIPTION,
            serde_json::json!({"type": "object", "properties": {}, "required": []}),
        );
        reg(
            tools,
            "skill_read",
            SkillReadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::skill_reader::ReadSkillArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "read_config_guide",
            ReadConfigGuideTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::config_guide::ReadConfigGuideArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "ctx_search",
            CtxSearchTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::ctx_search::CtxSearchArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "recall_events",
            RecallEventsTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::recall_events::RecallEventsArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "self_manage",
            SelfManageTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::self_manage::SelfManageArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "self_config",
            crate::builtin_tools::self_config::SelfConfigTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::self_config::SelfConfigArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "desktop",
            DesktopTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::desktop::DesktopArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "pim",
            PimTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::pim::PimArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "system",
            SystemTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::system_tool::SystemArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "automation",
            AutomationTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::automation_tool::AutomationArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "permission",
            PermissionTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::permission_tool::PermissionArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "media",
            MediaTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::media_tool::MediaArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "scratchpad",
            ScratchpadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(
                crate::builtin_tools::scratchpad::ScratchpadArgs
            ))
            .unwrap_or_default(),
        );
        reg(
            tools,
            "clawhub",
            crate::builtin_tools::clawhub::ClawHubTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::clawhub::ClawHubArgs))
                .unwrap_or_default(),
        );
        reg(
            tools,
            "media_send",
            crate::builtin_tools::media_send::MediaSendTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::media_send::MediaSendArgs))
                .unwrap_or_default(),
        );
    }
}
