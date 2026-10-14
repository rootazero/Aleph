from ed import apply
base='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/'
E=[('''    /// A sandbox without a managed-spawn capability (or a driver that refuses,''','''    /// The managed command asks for ONE process: no fork/subprocess grant, so
    /// the policy handed to the driver has `allow_fork == false`, and the
    /// workspace seam refuses anything else (`managed_spawn_fork`).
    #[test]
    fn launcher_capabilities_never_grant_fork() {
        let req = LaunchRequest {
            program: "dev-server".into(),
            args: vec![],
            cwd: None,
            endpoint: CanonicalEndpoint::parse("http://127.0.0.1:5173/app").expect("canonical"),
            ready_timeout: Duration::from_secs(5),
            lifetime: Duration::from_secs(60),
        };
        let caps = capabilities(&req);
        assert!(!caps.spawn_subprocess);
        assert!(!crate::sandbox::policy::SandboxPolicy::from(&caps).process.allow_fork);
    }

    /// A sandbox without a managed-spawn capability (or a driver that refuses,'''),]
apply(base+'src/builtin_tools/browser_tools/companion/process.rs',E)
E=[('''//! * a driver whose wrapper''','''//! * a managed service is ONE process. The supervisor kills and waits for the
//!   service PID only; there is no process-tree kill, so descendants of a
//!   forked child could outlive a "confirmed" stop. `spawn_managed` therefore
//!   refuses any command whose capabilities allow fork/subprocesses
//!   (`UnsupportedPolicy { feature: "managed_spawn_fork" }`), before approval
//!   and again on the prepared command. Denying fork at the OS level is the
//!   driver's job (macOS seatbelt emits `(deny process-fork)` when
//!   `allow_fork == false`; that denial has not been verified on a real macOS
//!   host from this change). Linux and Windows refuse managed spawns anyway;
//!   this is a single-process contract, not a tree-kill guarantee;
//! * a driver whose wrapper'''),]
apply(base+'src/sandbox/managed.rs',E)
