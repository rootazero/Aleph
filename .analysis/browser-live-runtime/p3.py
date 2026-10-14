from ed import apply
base='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/'
E=[('''    fn preflight(&self) -> Result<(), SandboxError> {
        self.os_driver.managed_spawn_preflight()
    }
''','''    fn preflight(&self) -> Result<(), SandboxError> {
        self.os_driver.managed_spawn_preflight()
    }
'''),
('''        if cmd.stdin.is_some() {
            return Err(SandboxError::Other(
                "managed spawn takes no pre-baked stdin: the owner writes the launch secret"
                    .into(),
            ));
        }
        let PreparedCommand {
            cmd,
            cwd,
            profile,
            timeout: _,
            active_proxy,
        } = self.prepare_command(cmd).await?;
        let spawned''','''        if cmd.stdin.is_some() {
            return Err(SandboxError::Other(
                "managed spawn takes no pre-baked stdin: the owner writes the launch secret"
                    .into(),
            ));
        }
        // Single-process contract: nothing here can prove that forked
        // descendants die with the service (no tree kill exists), so a
        // managed request that permits fork is refused BEFORE approval is
        // even asked, and again on the prepared command below.
        self.refuse_managed_fork(&cmd)?;
        let PreparedCommand {
            cmd,
            cwd,
            profile,
            timeout: _,
            active_proxy,
        } = self.prepare_command(cmd).await?;
        self.refuse_managed_fork(&cmd)?;
        let spawned'''),
('''#[async_trait]
impl crate::sandbox::managed::ManagedSandbox for WorkspaceSandbox {''','''impl WorkspaceSandbox {
    /// A managed service is a single process: only the child PID is
    /// supervised and reaped, so a policy that lets it fork (or spawn
    /// subprocesses) would leave descendants nobody can prove stopped.
    fn refuse_managed_fork(&self, cmd: &SandboxCommand) -> Result<(), SandboxError> {
        if cmd.capabilities.spawn_subprocess {
            return Err(SandboxError::UnsupportedPolicy {
                platform: self.os_driver.platform(),
                feature: "managed_spawn_fork".into(),
                reason: "a managed service must be a single process: the supervisor reaps \
                         only the service PID and has no process-tree kill, so a request \
                         that allows fork/subprocesses is refused"
                    .into(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl crate::sandbox::managed::ManagedSandbox for WorkspaceSandbox {'''),
]
apply(base+'src/sandbox/workspace/mod.rs',E)
