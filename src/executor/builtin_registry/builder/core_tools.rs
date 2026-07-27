use std::collections::HashMap;

use crate::builtin_tools::skill_reader::{
    ListSkillsTool as SkillListTool, ReadSkillTool as SkillReadTool,
};
use crate::builtin_tools::{
    AutomationTool, CodeExecTool, CtxSearchTool, DesktopTool, FileEditTool, FileOpsTool,
    FileReadTool, FileWriteTool, MediaTool, PdfGenerateTool, PermissionTool, PimTool,
    ReadConfigGuideTool, RecallEventsTool, ScratchpadTool, SearchTool, SelfManageTool, SystemTool,
};
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;

use super::BuiltinToolRegistry;

impl BuiltinToolRegistry {
    /// Register always-available core tool metadata with JSON Schema parameters.
    pub(crate) fn register_core_tools(tools: &mut HashMap<String, UnifiedTool>) {
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

        reg(
            tools,
            "search",
            SearchTool::DESCRIPTION,
            schema::<crate::builtin_tools::search::SearchArgs>("search"),
        );
        reg(
            tools,
            "web_fetch",
            "Fetch and read content from a URL",
            schema::<crate::builtin_tools::web_fetch::WebFetchArgs>("web_fetch"),
        );
        reg(
            tools,
            "file_ops",
            FileOpsTool::DESCRIPTION,
            schema::<crate::builtin_tools::file_ops::FileOpsArgs>("file_ops"),
        );
        reg(
            tools,
            "file_read",
            FileReadTool::DESCRIPTION,
            schema::<crate::builtin_tools::file_ops::read::FileReadArgs>("file_read"),
        );
        reg(
            tools,
            "file_write",
            FileWriteTool::DESCRIPTION,
            schema::<crate::builtin_tools::file_ops::write::FileWriteArgs>("file_write"),
        );
        reg(
            tools,
            "file_edit",
            FileEditTool::DESCRIPTION,
            schema::<crate::builtin_tools::file_ops::edit::FileEditArgs>("file_edit"),
        );
        reg(
            tools,
            "bash",
            "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
            schema::<crate::builtin_tools::bash_exec::BashExecArgs>("bash"),
        );
        reg(
            tools,
            "code_exec",
            CodeExecTool::DESCRIPTION,
            schema::<crate::builtin_tools::code_exec::CodeExecArgs>("code_exec"),
        );
        reg(
            tools,
            "code_check",
            crate::builtin_tools::code_check::CodeCheckTool::DESCRIPTION,
            schema::<crate::builtin_tools::code_check::CodeCheckArgs>("code_check"),
        );
        reg(
            tools,
            "pdf_generate",
            PdfGenerateTool::DESCRIPTION,
            schema::<crate::builtin_tools::pdf_generate::PdfGenerateArgs>("pdf_generate"),
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
            schema::<crate::builtin_tools::skill_reader::ReadSkillArgs>("skill_read"),
        );
        reg(
            tools,
            "read_config_guide",
            ReadConfigGuideTool::DESCRIPTION,
            schema::<crate::builtin_tools::config_guide::ReadConfigGuideArgs>("read_config_guide"),
        );
        reg(
            tools,
            "ctx_search",
            CtxSearchTool::DESCRIPTION,
            schema::<crate::builtin_tools::ctx_search::CtxSearchArgs>("ctx_search"),
        );
        reg(
            tools,
            "recall_events",
            RecallEventsTool::DESCRIPTION,
            schema::<crate::builtin_tools::recall_events::RecallEventsArgs>("recall_events"),
        );
        reg(
            tools,
            "self_manage",
            SelfManageTool::DESCRIPTION,
            schema::<crate::builtin_tools::self_manage::SelfManageArgs>("self_manage"),
        );
        // Always-on: the hook registry it reads is the process-global
        // extension manager, so there is no service dependency to wait for.
        // It must stay reachable even when no hooks are registered — "nothing
        // is wired up" is itself the answer someone debugging a dead hook
        // needs, and an absent tool cannot give it.
        reg(
            tools,
            "hooks_manage",
            crate::builtin_tools::HooksManageTool::DESCRIPTION,
            schema::<crate::builtin_tools::hooks_manage::HooksManageArgs>("hooks_manage"),
        );
        // Always-on: the ledger it reads is a process global installed at boot,
        // so this tool has no service dependency to wait for. It must be
        // reachable even when the ledger is NOT installed — it then says so
        // explicitly, which is the whole point (an audit reader that silently
        // returns nothing is how the last audit surface came to lie).
        reg(
            tools,
            "agent_identity",
            crate::builtin_tools::agent_identity::AgentIdentityTool::DESCRIPTION,
            schema::<crate::builtin_tools::agent_identity::AgentIdentityArgs>("agent_identity"),
        );
        reg(
            tools,
            "self_config",
            crate::builtin_tools::self_config::SelfConfigTool::DESCRIPTION,
            schema::<crate::builtin_tools::self_config::SelfConfigArgs>("self_config"),
        );
        reg(
            tools,
            "list_models",
            crate::builtin_tools::list_models::ListModelsTool::DESCRIPTION,
            schema::<crate::builtin_tools::list_models::ListModelsArgs>("list_models"),
        );
        reg(
            tools,
            "moa",
            crate::builtin_tools::moa_manage::MoaManageTool::DESCRIPTION,
            schema::<crate::builtin_tools::moa_manage::MoaManageArgs>("moa"),
        );
        reg(
            tools,
            "desktop",
            DesktopTool::DESCRIPTION,
            schema::<crate::builtin_tools::desktop::DesktopArgs>("desktop"),
        );
        reg(
            tools,
            "pim",
            PimTool::DESCRIPTION,
            schema::<crate::builtin_tools::pim::PimArgs>("pim"),
        );
        reg(
            tools,
            "system",
            SystemTool::DESCRIPTION,
            schema::<crate::builtin_tools::system_tool::SystemArgs>("system"),
        );
        reg(
            tools,
            "automation",
            AutomationTool::DESCRIPTION,
            schema::<crate::builtin_tools::automation_tool::AutomationArgs>("automation"),
        );
        reg(
            tools,
            "permission",
            PermissionTool::DESCRIPTION,
            schema::<crate::builtin_tools::permission_tool::PermissionArgs>("permission"),
        );
        reg(
            tools,
            "media",
            MediaTool::DESCRIPTION,
            schema::<crate::builtin_tools::media_tool::MediaArgs>("media"),
        );
        reg(
            tools,
            "scratchpad",
            ScratchpadTool::DESCRIPTION,
            schema::<crate::builtin_tools::scratchpad::ScratchpadArgs>("scratchpad"),
        );
        reg(
            tools,
            "goal",
            crate::builtin_tools::GoalTool::DESCRIPTION,
            schema::<crate::builtin_tools::goal::GoalArgs>("goal"),
        );
        reg(
            tools,
            "loop",
            crate::builtin_tools::LoopTool::DESCRIPTION,
            schema::<crate::builtin_tools::loop_manage::LoopArgs>("loop"),
        );
        reg(
            tools,
            "loop_graph",
            crate::builtin_tools::LoopGraphTool::DESCRIPTION,
            schema::<crate::builtin_tools::loop_graph_manage::LoopGraphArgs>("loop_graph"),
        );
        reg(
            tools,
            "strategy",
            crate::builtin_tools::StrategyTool::DESCRIPTION,
            schema::<crate::builtin_tools::strategy_manage::StrategyArgs>("strategy"),
        );
        reg(
            tools,
            "clawhub",
            crate::builtin_tools::clawhub::ClawHubTool::DESCRIPTION,
            schema::<crate::builtin_tools::clawhub::ClawHubArgs>("clawhub"),
        );
        reg(
            tools,
            "media_send",
            crate::builtin_tools::media_send::MediaSendTool::DESCRIPTION,
            schema::<crate::builtin_tools::media_send::MediaSendArgs>("media_send"),
        );
        reg(
            tools,
            "artifact_publish",
            crate::builtin_tools::artifact_publish::ArtifactPublishTool::DESCRIPTION,
            schema::<crate::builtin_tools::artifact_publish::ArtifactPublishArgs>(
                "artifact_publish",
            ),
        );
    }
}
