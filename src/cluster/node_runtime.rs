//! 节点侧反向 RPC 分发（执行臂）。
//!
//! 收到中心发来的 `tool.call` 请求 → 查命令表（allowlist = 表的 keys，节点侧
//! 权威闸门）→ 命中则跑该命令 → 回 `Result<Value, String>`（节点 loop 据此
//! 构造带 id 的 `JsonRpcResponse`）。
//!
//! 红线：确定性查表，无 LLM 推理（R7）；不进 `src/harness/`（R10）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::builtin_tools::BashExecTool;
use crate::cluster::CommandDescriptor;
use crate::routing::session_key::SessionKey;
use crate::sandbox::context::SESSION_ID;
use crate::tools::AlephTool;

/// 节点可执行的一个命令。`run` 返回 `Ok(payload)` 或 `Err(message)`。
#[async_trait]
pub trait NodeCommand: Send + Sync {
    async fn run(&self, args: Value) -> Result<Value, String>;
    fn descriptor(&self) -> CommandDescriptor;
}

/// 节点命令表。keys 即 allowlist（节点侧权威）。
#[derive(Default)]
pub struct CommandTable {
    commands: HashMap<String, Arc<dyn NodeCommand>>,
}

impl CommandTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, name: impl Into<String>, cmd: Arc<dyn NodeCommand>) {
        self.commands.insert(name.into(), cmd);
    }

    /// 节点 connect 时声明给中心的命令目录。**按名排序**——backing store 是
    /// `HashMap`，迭代序每次进程启动都不同，会让节点每次 connect 上报的目录顺序
    /// 抖动，进而抖动 `environments.list` 与模型可见的 `node_list` 输出。
    #[must_use]
    pub fn descriptors(&self) -> Vec<CommandDescriptor> {
        let mut out: Vec<CommandDescriptor> =
            self.commands.values().map(|c| c.descriptor()).collect();
        out.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// 分发一帧反向 RPC 请求体。`method` 必须是 `"tool.call"`；`params` 形如
    /// `{"tool": "<name>", "args": {...}}`。allowlist 权威：tool 不在表中即拒，
    /// 无论中心发什么。返回 `Ok(payload)` / `Err(message)`，由调用方包成
    /// 带 id 的响应。
    pub async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, String> {
        if method != "tool.call" {
            return Err(format!("unknown method '{method}' (expected tool.call)"));
        }
        let tool = params
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "tool.call: missing string field `tool`".to_string())?;
        let Some(cmd) = self.commands.get(tool) else {
            return Err(format!("command '{tool}' not permitted on this node"));
        };
        let args = params.get("args").cloned().unwrap_or(Value::Null);
        cmd.run(args).await
    }
}

/// `bash` 作为节点命令：在固定 session 作用域下委托 `BashExecTool`。
pub struct BashNodeCommand {
    bash: BashExecTool,
    session: SessionKey,
}

impl BashNodeCommand {
    pub const fn new(bash: BashExecTool, session: SessionKey) -> Self {
        Self { bash, session }
    }
}

#[async_trait]
impl NodeCommand for BashNodeCommand {
    async fn run(&self, args: Value) -> Result<Value, String> {
        SESSION_ID
            .scope(self.session.clone(), self.bash.call_json(args))
            .await
            .map_err(|e| e.to_string())
    }
    fn descriptor(&self) -> CommandDescriptor {
        CommandDescriptor {
            name: "bash".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

impl CommandTable {
    /// 便捷构造：注册唯一的 `bash` 命令（0c 节点的全部能力）。
    #[must_use]
    pub fn with_bash(bash: BashExecTool, session: SessionKey) -> Self {
        let mut t = Self::new();
        t.register("bash", Arc::new(BashNodeCommand::new(bash, session)));
        t
    }

    /// 在已有命令之外注册 `file.read` / `file.write`，两者共享同一 jail 根
    /// （应传入节点 bash 的同一 session workspace 目录）。
    pub fn register_file_commands(&mut self, workspace_dir: std::path::PathBuf) {
        use crate::cluster::{FileReadCommand, FileWriteCommand};
        self.register(
            "file.read",
            Arc::new(FileReadCommand::new(workspace_dir.clone())),
        );
        self.register("file.write", Arc::new(FileWriteCommand::new(workspace_dir)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoCmd;

    #[async_trait]
    impl NodeCommand for EchoCmd {
        async fn run(&self, args: Value) -> Result<Value, String> {
            if args.get("boom").is_some() {
                return Err("echo: boom".to_string());
            }
            Ok(json!({"echoed": args}))
        }
        fn descriptor(&self) -> CommandDescriptor {
            CommandDescriptor {
                name: "echo".to_string(),
                schema: json!({"type": "object"}),
            }
        }
    }

    fn table() -> CommandTable {
        let mut t = CommandTable::new();
        t.register("echo", Arc::new(EchoCmd));
        t
    }

    #[tokio::test]
    async fn dispatch_runs_registered_command() {
        let out = table()
            .dispatch("tool.call", &json!({"tool": "echo", "args": {"x": 1}}))
            .await
            .expect("registered command runs");
        assert_eq!(out["echoed"]["x"], 1);
    }

    #[tokio::test]
    async fn dispatch_rejects_unlisted_command() {
        let err = table()
            .dispatch("tool.call", &json!({"tool": "rm", "args": {}}))
            .await
            .expect_err("allowlist denies");
        assert!(err.contains("not permitted"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_rejects_unknown_method() {
        let err = table()
            .dispatch("evil.method", &json!({"tool": "echo"}))
            .await
            .expect_err("only tool.call");
        assert!(err.contains("unknown method"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_passes_through_command_error() {
        let err = table()
            .dispatch(
                "tool.call",
                &json!({"tool": "echo", "args": {"boom": true}}),
            )
            .await
            .expect_err("command error surfaces");
        assert!(err.contains("boom"), "{err}");
    }

    #[tokio::test]
    async fn bash_command_runs_under_sandbox() {
        use crate::routing::session_key::SessionKey;
        use crate::sandbox::test_util::MockSandbox;
        use crate::sandbox::SandboxOutput;

        let sandbox = MockSandbox::new(SandboxOutput {
            stdout: b"hi\n".to_vec(),
            exit_code: Some(0),
            duration_ms: 1,
            ..Default::default()
        });
        let bash = BashExecTool::new().with_sandbox(sandbox);
        let session = SessionKey::ephemeral("node-test");
        let table = CommandTable::with_bash(bash, session);

        let out = table
            .dispatch(
                "tool.call",
                &json!({"tool": "bash", "args": {"cmd": "echo hi"}}),
            )
            .await
            .expect("bash runs under sandbox");
        // MockSandbox returns a structured CodeExecOutput; assert the envelope shape.
        assert!(
            out.get("exit_code").is_some(),
            "bash output envelope: {out}"
        );

        // allowlist still authoritative: bash table denies a non-bash tool.
        let err = table
            .dispatch("tool.call", &json!({"tool": "python", "args": {}}))
            .await
            .expect_err("only bash permitted");
        assert!(err.contains("not permitted"), "{err}");
    }

    #[test]
    fn descriptors_list_registered_commands() {
        let d = table().descriptors();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, "echo");
    }

    /// A command that reports whatever name it was built with, so the sort can
    /// actually be observed (EchoCmd hardcodes "echo").
    struct NamedCmd(&'static str);

    #[async_trait]
    impl NodeCommand for NamedCmd {
        async fn run(&self, _args: Value) -> Result<Value, String> {
            Ok(Value::Null)
        }
        fn descriptor(&self) -> CommandDescriptor {
            CommandDescriptor {
                name: self.0.to_string(),
                schema: json!({"type": "object"}),
            }
        }
    }

    #[test]
    fn descriptors_are_sorted_by_name() {
        // The HashMap backing store iterates in a per-process-random order; the
        // declared catalog must not inherit that jitter (it lands verbatim in
        // environments.list and in what the model sees via node_list).
        let mut t = CommandTable::new();
        for name in ["file.write", "bash", "file.read"] {
            t.register(name, Arc::new(NamedCmd(name)));
        }
        let names: Vec<String> = t.descriptors().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["bash", "file.read", "file.write"]);
    }
}
