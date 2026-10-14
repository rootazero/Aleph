from ed import apply
base='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/'
E=[('''    /// A driver that cannot report the service PID (default / Windows wrapper)
    /// refuses at preflight and at spawn; nothing runs.''','''    /// A managed service is a single process (no tree kill exists): a request
    /// that allows fork/subprocesses is refused before approval and nothing
    /// is spawned, while the same request without it still works.
    #[tokio::test]
    async fn managed_spawn_refuses_a_forking_policy_and_spawns_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let spawned = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(ManagedFakeDriver {
            spawned: Arc::clone(&spawned),
        });
        let sandbox = build_sandbox(
            &tmp,
            driver,
            build_gate_with(ApprovalOutcome::Approved),
            SandboxHooks::new(),
        );
        let managed = Sandbox::managed(&sandbox).expect("managed-capable");
        let forking = SandboxCapabilities {
            spawn_subprocess: true,
            ..SandboxCapabilities::strict()
        };
        let err = managed
            .spawn_managed(managed_command(forking))
            .await
            .map(|_| ())
            .expect_err("fork must be refused");
        assert!(
            matches!(&err, SandboxError::UnsupportedPolicy { feature, .. } if feature == "managed_spawn_fork"),
            "{err:?}"
        );
        assert_eq!(spawned.load(std::sync::atomic::Ordering::SeqCst), 0);
        let mut ok = managed
            .spawn_managed(managed_command(SandboxCapabilities::strict()))
            .await
            .expect("single-process request is admitted");
        assert_eq!(spawned.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = ok.child.wait().await;
    }

    /// A driver that cannot report the service PID (default / Windows wrapper)
    /// refuses at preflight and at spawn; nothing runs.'''),]
apply(base+'src/sandbox/workspace/mod.rs',E)
