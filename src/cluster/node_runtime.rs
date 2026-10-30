//! Node-side reverse RPC dispatch (execution arm).
//!
//! Receives a `tool.call` request from the center → looks up the command table
//! (allowlist = table keys, node-side authoritative gate) → on hit, runs the
//! command → returns `Result<Value, String>` (the node loop constructs a
//! `JsonRpcResponse` with an id from this).
//!
//! Redline: deterministic table lookup, no LLM reasoning (R7); does not enter
//! `src/harness/` (R10).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::builtin_tools::BashExecTool;
use crate::cluster::CommandDescriptor;
use crate::routing::session_key::SessionKey;
use crate::sandbox::context::SESSION_ID;
use crate::tools::AlephTool;

/// A command executable by a node. `run` returns `Ok(payload)` or `Err(message)`.
#[async_trait]
pub trait NodeCommand: Send + Sync {
    async fn run(&self, args: Value) -> Result<Value, String>;
    fn descriptor(&self) -> CommandDescriptor;
}

/// Node command table. Keys = allowlist (node-side authoritative).
#[derive(Default)]
pub struct CommandTable {
    commands: HashMap<String, Arc<dyn NodeCommand>>,
}

impl CommandTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command under the name it **declares** to the center.
    ///
    /// The table key and the declared descriptor name used to be two
    /// independent namespaces: the key was whatever the caller passed, the
    /// declared name was `cmd.descriptor().name`, and nothing asserted that
    /// they agree. A mismatch makes the node advertise a command it then
    /// refuses ("command 'X' not permitted on this node"), or hide an
    /// executable command from the center's admission check.
    ///
    /// The descriptor name is the single source of truth: it is the name the
    /// node declares AND the name the center's admission check compares
    /// against, so a mismatch can only hide or refuse a capability, never
    /// expose an undeclared one. The caller-supplied `name` stays in the
    /// signature (it is `pub` and re-exported) and is used to report the
    /// mismatch.
    pub fn register(&mut self, name: impl Into<String>, cmd: Arc<dyn NodeCommand>) {
        let requested = name.into();
        let key = cmd.descriptor().name;
        if key != requested {
            tracing::error!(
                requested_name = %requested,
                descriptor_name = %key,
                "CommandTable::register: the registered name does not match the command's \
                 descriptor name; the descriptor name wins (it is the name the node declares \
                 to the center and the name the center's admission check compares against)"
            );
        }
        if self.commands.contains_key(&key) {
            // A second `register` for the same name shadows the previous
            // command. With `register_file_commands` and `with_bash` both
            // callable from the same setup path, a misordered init could
            // bind `file.read` / `file.write` to a different jail root than
            // the one an operator has already pointed at; warn so that
            // shows up in logs instead of being silently overwritten.
            tracing::warn!(
                command = %key,
                "CommandTable::register overwriting an existing entry"
            );
        }
        self.commands.insert(key, cmd);
    }

    /// Command catalog declared to the center on node connect. **Sorted by
    /// name** — the backing store is a `HashMap` whose iteration order differs
    /// every process start, which would cause the declared catalog to jitter on
    /// each connect, in turn jittering `environments.list` and the
    /// model-visible `node_list` output. The table key is the secondary sort
    /// key: since the key IS the descriptor name (see [`register`]), no two
    /// entries can share a name — but the tie-break keeps the order total even
    /// if that ever changes, so the emitted order can never depend on
    /// HashMap iteration order.
    #[must_use]
    pub fn descriptors(&self) -> Vec<CommandDescriptor> {
        let mut out: Vec<(String, CommandDescriptor)> = self
            .commands
            .iter()
            .map(|(key, command)| (key.clone(), command.descriptor()))
            .collect();
        out.sort_unstable_by(|a, b| a.1.name.cmp(&b.1.name).then_with(|| a.0.cmp(&b.0)));
        out.into_iter().map(|(_, descriptor)| descriptor).collect()
    }

    /// Dispatch a reverse RPC request body. `method` must be `"tool.call"`;
    /// `params` is like `{"tool": "<name>", "args": {...}}`. The allowlist is
    /// authoritative: a tool not in the table is rejected regardless of what the
    /// center sends. Returns `Ok(payload)` / `Err(message)`, to be wrapped into
    /// an id-bearing response by the caller.
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

/// `bash` as a node command: delegates to `BashExecTool` under a fixed session scope.
/// `pub(crate)` because `node_runtime` is a private module and the type is not
/// re-exported from `cluster/mod.rs`; widening to `pub` would expose it across
/// the crate boundary with no consumer.
pub(crate) struct BashNodeCommand {
    bash: BashExecTool,
    session: SessionKey,
}

impl BashNodeCommand {
    pub(crate) const fn new(bash: BashExecTool, session: SessionKey) -> Self {
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
    /// Convenience constructor: registers the single `bash` command (the full
    /// capability of a 0c node).
    #[must_use]
    pub fn with_bash(bash: BashExecTool, session: SessionKey) -> Self {
        let mut t = Self::new();
        t.register("bash", Arc::new(BashNodeCommand::new(bash, session)));
        t
    }

    /// Register `file.read` / `file.write` on top of existing commands. Both
    /// share the same jail root (should be the node's bash session workspace
    /// directory).
    ///
    /// Prefer [`register_file_commands_for_session`](Self::register_file_commands_for_session)
    /// at an assembly site: it derives the jail root from the same session key
    /// the bash command runs in, so the two cannot drift apart.
    pub fn register_file_commands(&mut self, workspace_dir: std::path::PathBuf) {
        use crate::cluster::{FileReadCommand, FileWriteCommand};
        self.register(
            "file.read",
            Arc::new(FileReadCommand::new(workspace_dir.clone())),
        );
        self.register("file.write", Arc::new(FileWriteCommand::new(workspace_dir)));
    }

    /// Register `file.read` / `file.write` with the jail root derived from the
    /// node's session key — the same derivation the bash sandbox uses for its
    /// working directory.
    ///
    /// The file-command jail root and the bash session directory used to be
    /// two independent inputs that were equal only by convention at the
    /// call site: a drifted pair writes a pushed file into one directory while
    /// bash executes in another, and both report success, silently breaking
    /// the documented "a pushed script can be run by bash" workflow. Deriving
    /// both from one `(workspace_root, session)` pair makes the equality a
    /// fact of the API instead of a convention.
    pub fn register_file_commands_for_session(
        &mut self,
        workspace_root: &std::path::Path,
        session: &SessionKey,
    ) {
        let workspace_dir =
            crate::sandbox::workspace::session_workspace_dir(workspace_root, session);
        self.register_file_commands(workspace_dir);
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

    /// The allowlist key and the name the node declares to the center are ONE
    /// name. A mismatch used to make the node advertise a command it then
    /// refuses ("not permitted on this node"), or hide an executable command
    /// from the center's admission check.
    #[tokio::test]
    async fn register_keys_the_table_by_the_declared_descriptor_name() {
        let mut t = CommandTable::new();
        // The caller asks for "echo"; the command declares itself "echo2".
        t.register("echo", Arc::new(NamedCmd("echo2")));
        assert_eq!(
            t.descriptors()
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>(),
            vec!["echo2".to_string()],
            "the declared descriptor name is the single source of truth"
        );
        let err = t
            .dispatch("tool.call", &json!({"tool": "echo", "args": {}}))
            .await
            .expect_err("the caller's name is not the declared one");
        assert!(err.contains("not permitted"), "{err}");
        t.dispatch("tool.call", &json!({"tool": "echo2", "args": {}}))
            .await
            .expect("the declared name dispatches");
    }

    /// Two commands that declare the same name now collide on one table key:
    /// the later registration wins (and the duplicate-registration warning
    /// fires).
    #[test]
    fn registering_two_commands_with_the_same_declared_name_keeps_one() {
        let mut t = CommandTable::new();
        t.register("a", Arc::new(NamedCmd("dup")));
        t.register("b", Arc::new(NamedCmd("dup")));
        assert_eq!(t.descriptors().len(), 1);
        assert_eq!(t.descriptors()[0].name, "dup");
    }
}
