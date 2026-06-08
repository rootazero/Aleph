//! 节点侧反向 RPC 分发（执行臂）。
//!
//! 收到中心发来的 `tool.call` 请求 → 查命令表（allowlist = 表的 keys，节点侧
//! 权威闸门）→ 命中则跑该命令 → 回 `Result<Value, String>`（节点 loop 据此
//! 构造带 id 的 JsonRpcResponse）。
//!
//! 红线：确定性查表，无 LLM 推理（R7）；不进 `src/harness/`（R10）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::cluster::CommandDescriptor;

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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, name: impl Into<String>, cmd: Arc<dyn NodeCommand>) {
        self.commands.insert(name.into(), cmd);
    }

    /// 节点 connect 时声明给中心的命令目录。
    pub fn descriptors(&self) -> Vec<CommandDescriptor> {
        self.commands.values().map(|c| c.descriptor()).collect()
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
            CommandDescriptor { name: "echo".to_string(), schema: json!({"type": "object"}) }
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
            .dispatch("tool.call", &json!({"tool": "echo", "args": {"boom": true}}))
            .await
            .expect_err("command error surfaces");
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn descriptors_list_registered_commands() {
        let d = table().descriptors();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, "echo");
    }
}
