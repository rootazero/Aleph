//! SandboxHooks — lifecycle hooks for sandbox-backed tool execution.
//!
//! Hooks fire before and after sandboxed execution, enabling per-tool
//! capability checks, audit logging, and environment preparation without
//! coupling to the Sandbox implementation.

use async_trait::async_trait;
use std::sync::Arc;

use crate::sandbox::command::SandboxCommand;

/// Result type for SandboxHook operations.
#[derive(Debug)]
pub enum SandboxHookResult {
    /// Allow execution to proceed.
    Allow,
    /// Veto execution with a reason message.
    Deny { reason: String },
}

/// Context passed to sandbox hooks.
#[derive(Debug)]
pub struct SandboxHookContext<'a> {
    /// The tool being executed.
    pub tool_name: &'a str,
    /// The sandbox command that would be executed.
    pub command: &'a SandboxCommand,
}

impl<'a> SandboxHookContext<'a> {
    pub fn new(tool_name: &'a str, command: &'a SandboxCommand) -> Self {
        Self { tool_name, command }
    }
}

/// Hook executed before sandboxed tool execution.
/// Returns `SandboxHookResult::Allow` to proceed or
/// `SandboxHookResult::Deny { reason }` to block.
#[async_trait]
pub trait SandboxBeforeHook: Send + Sync + 'static {
    /// Name identifying this hook for logging/debugging.
    fn name(&self) -> &'static str;

    /// Called before sandboxed execution proceeds.
    async fn before(&self, ctx: SandboxHookContext<'_>) -> SandboxHookResult;
}

/// Hook executed after sandboxed tool execution completes.
/// Fires regardless of success or failure; receives the outcome.
#[async_trait]
pub trait SandboxAfterHook: Send + Sync + 'static {
    /// Name identifying this hook for logging/debugging.
    fn name(&self) -> &'static str;

    /// Called after sandboxed execution finishes.
    async fn after(&self, ctx: SandboxHookContext<'_>, outcome: &Result<(), &str>);
}

/// Compose multiple before/after hooks into a pair for use in the sandbox
/// execution path.
#[derive(Default)]
pub struct SandboxHooks {
    pub before: Vec<Arc<dyn SandboxBeforeHook>>,
    pub after: Vec<Arc<dyn SandboxAfterHook>>,
}

impl SandboxHooks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_before(mut self, hook: Arc<dyn SandboxBeforeHook>) -> Self {
        self.before.push(hook);
        self
    }

    pub fn with_after(mut self, hook: Arc<dyn SandboxAfterHook>) -> Self {
        self.after.push(hook);
        self
    }

    /// Run all before hooks. Returns `SandboxHookResult::Deny` on first denial.
    pub async fn run_before(&self, ctx: &SandboxHookContext<'_>) -> SandboxHookResult {
        for hook in &self.before {
            let result = hook
                .before(SandboxHookContext::new(ctx.tool_name, ctx.command))
                .await;
            if !matches!(result, SandboxHookResult::Allow) {
                return result;
            }
        }
        SandboxHookResult::Allow
    }

    /// Run all after hooks.
    pub async fn run_after(&self, ctx: &SandboxHookContext<'_>, outcome: Result<(), &str>) {
        for hook in &self.after {
            hook.after(
                SandboxHookContext::new(ctx.tool_name, ctx.command),
                &outcome,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::capabilities::SandboxCapabilities;
    use crate::sandbox::command::SandboxCommand;

    struct TestBeforeHook(&'static str, bool);
    #[async_trait]
    impl SandboxBeforeHook for TestBeforeHook {
        fn name(&self) -> &'static str {
            self.0
        }
        async fn before(&self, _: SandboxHookContext<'_>) -> SandboxHookResult {
            if self.1 {
                SandboxHookResult::Allow
            } else {
                SandboxHookResult::Deny {
                    reason: "test deny".to_string(),
                }
            }
        }
    }

    struct TestAfterHook(&'static str);
    #[async_trait]
    impl SandboxAfterHook for TestAfterHook {
        fn name(&self) -> &'static str {
            self.0
        }
        async fn after(&self, _: SandboxHookContext<'_>, _: &Result<(), &str>) {}
    }

    #[tokio::test]
    async fn test_before_hook_allows() {
        use crate::routing::session_key::SessionKey;
        let hooks = SandboxHooks::new().with_before(Arc::new(TestBeforeHook("allow", true)));
        let cmd = SandboxCommand {
            session_id: SessionKey::ephemeral("test"),
            program: "echo".into(),
            args: vec!["hello".into()],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::default(),
            timeout: None,
        };
        let ctx = SandboxHookContext::new("test_tool", &cmd);
        assert!(matches!(
            hooks.run_before(&ctx).await,
            SandboxHookResult::Allow
        ));
    }

    #[tokio::test]
    async fn test_before_hook_denies() {
        use crate::routing::session_key::SessionKey;
        let hooks = SandboxHooks::new().with_before(Arc::new(TestBeforeHook("deny", false)));
        let cmd = SandboxCommand {
            session_id: SessionKey::ephemeral("test"),
            program: "echo".into(),
            args: vec!["hello".into()],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::default(),
            timeout: None,
        };
        let ctx = SandboxHookContext::new("test_tool", &cmd);
        let result = hooks.run_before(&ctx).await;
        assert!(matches!(result, SandboxHookResult::Deny { .. }));
    }

    #[tokio::test]
    async fn test_before_hook_stops_at_first_deny() {
        use crate::routing::session_key::SessionKey;
        use std::sync::atomic::{AtomicBool, Ordering};

        let second_ran = Arc::new(AtomicBool::new(false));
        let second_ran_clone = second_ran.clone();

        struct SecondBeforeHook {
            flag: Arc<AtomicBool>,
        }
        #[async_trait]
        impl SandboxBeforeHook for SecondBeforeHook {
            fn name(&self) -> &'static str {
                "second_hook"
            }
            async fn before(&self, _: SandboxHookContext<'_>) -> SandboxHookResult {
                self.flag.store(true, Ordering::SeqCst);
                SandboxHookResult::Allow
            }
        }

        let hooks = SandboxHooks::new()
            .with_before(Arc::new(TestBeforeHook("first_deny", false))) // denies
            .with_before(Arc::new(SecondBeforeHook {
                flag: second_ran_clone,
            })); // would set flag if called

        let cmd = SandboxCommand {
            session_id: SessionKey::ephemeral("test"),
            program: "echo".into(),
            args: vec!["hello".into()],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::default(),
            timeout: None,
        };
        let ctx = SandboxHookContext::new("test_tool", &cmd);
        let result = hooks.run_before(&ctx).await;
        assert!(matches!(result, SandboxHookResult::Deny { .. }));
        assert!(
            !second_ran.load(Ordering::SeqCst),
            "second hook should not have run after first hook denied"
        );
    }

    #[tokio::test]
    async fn test_after_hook_runs() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();

        struct CheckAfter {
            flag: Arc<AtomicBool>,
        }
        #[async_trait]
        impl SandboxAfterHook for CheckAfter {
            fn name(&self) -> &'static str {
                "check_after"
            }
            async fn after(&self, _: SandboxHookContext<'_>, _: &Result<(), &str>) {
                self.flag.store(true, Ordering::SeqCst);
            }
        }

        let hooks = SandboxHooks::new().with_after(Arc::new(CheckAfter { flag: ran_clone }));
        let cmd = SandboxCommand {
            session_id: crate::routing::session_key::SessionKey::ephemeral("test"),
            program: "echo".into(),
            args: vec!["hello".into()],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::default(),
            timeout: None,
        };
        let ctx = SandboxHookContext::new("test_tool", &cmd);
        hooks.run_after(&ctx, Ok(())).await;
        assert!(ran.load(Ordering::SeqCst));
    }
}
