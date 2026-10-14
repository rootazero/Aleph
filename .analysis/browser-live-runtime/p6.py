from ed import apply
base='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/'
apply(base+'src/builtin_tools/browser_tools/companion/process.rs',[('''    use super::*;

    /// A real child run''','''    use super::*;
    use crate::builtin_tools::browser_tools::companion::protocol::CanonicalEndpoint;

    /// A real child run''')])
apply(base+'src/sandbox/driver.rs',[('''    /// between (Windows `sandbox-init-windows`, Linux bwrap) must refuse.
    async fn spawn_managed(''','''    /// between (Windows `sandbox-init-windows`, Linux bwrap) must refuse.
    ///
    /// Single-process contract: the owner reaps the service PID only and no
    /// process-tree kill exists, so the profile reaching here has
    /// `process.allow_fork == false` (`ManagedSandbox::spawn_managed` refuses
    /// otherwise) and the driver must not let the service fork around it.
    async fn spawn_managed(''')])
