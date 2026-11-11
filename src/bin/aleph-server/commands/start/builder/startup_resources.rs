use alephcore::capability::projection_host::ProjectionHost;
use alephcore::error::AlephError;
use alephcore::memory::runtime::MemoryRuntime;
use alephcore::sync_primitives::Arc;

/// Keeps acquired startup resources reachable until common cleanup completes.
pub(in crate::commands::start) struct StartupResources {
    memory: Option<Arc<MemoryRuntime>>,
    projection_host: Option<Arc<ProjectionHost>>,
    ownership_tree: Option<Arc<alephcore::capability::ownership::OwnershipTree>>,
    runtime_withdrawn: bool,
    policy: Option<Arc<alephcore::capability::policy::PolicyEngine>>,
    events: Option<Arc<alephcore::capability::events::InMemoryEventStream>>,
    bridge: Option<alephcore::gateway::capability_events::CapabilityEventBridge>,
    pty: Option<&'static alephcore::gateway::pty::PtyManager>,
}

impl StartupResources {
    pub(in crate::commands::start) fn new() -> Self {
        Self {
            memory: None,
            projection_host: None,
            ownership_tree: None,
            runtime_withdrawn: false,
            policy: None,
            events: None,
            bridge: None,
            pty: None,
        }
    }

    pub(in crate::commands::start) fn install_memory(&mut self, runtime: Arc<MemoryRuntime>) {
        self.memory = Some(runtime);
    }

    pub(in crate::commands::start) fn install_ownership_tree(
        &mut self,
        tree: Arc<alephcore::capability::ownership::OwnershipTree>,
    ) {
        self.ownership_tree = Some(tree);
    }

    pub(in crate::commands::start) fn install_projection_host(
        &mut self,
        host: Arc<ProjectionHost>,
    ) {
        self.projection_host = Some(host);
    }

    pub(in crate::commands::start) fn install_capability_events(
        &mut self,
        policy: Arc<alephcore::capability::policy::PolicyEngine>,
        events: Arc<alephcore::capability::events::InMemoryEventStream>,
        bridge: alephcore::gateway::capability_events::CapabilityEventBridge,
    ) {
        self.policy = Some(policy);
        self.events = Some(events);
        self.bridge = Some(bridge);
    }

    pub(in crate::commands::start) fn install_pty(
        &mut self,
        manager: &'static alephcore::gateway::pty::PtyManager,
    ) {
        self.pty = Some(manager);
    }

    pub(in crate::commands::start) async fn cleanup(&mut self) -> Result<(), AlephError> {
        // STOP linearizes with both no-rule admissions and source publication.
        // UI waits must be released before waiting on their operation guards.
        let policy = self.policy.clone().or_else(|| {
            self.projection_host
                .as_ref()
                .map(|host| host.facade().policy())
        });
        if let Some(policy) = &policy {
            policy.stop_admission();
        }
        let mut observation_failure = None;
        if let Some(host) = &self.projection_host {
            if host.specials().close_ui_surfaces().await.is_err() {
                observation_failure = Some(AlephError::other("UI source cleanup failed"));
            }
        }
        if let Some(manager) = self.pty {
            if manager.deactivate().await.is_err() && observation_failure.is_none() {
                observation_failure = Some(AlephError::other("terminal source cleanup failed"));
            }
        }
        let failure = match &self.memory {
            Some(memory) => memory.stop().await.err(),
            None => None,
        };
        if let Some(policy) = &policy {
            policy.wait_for_idle().await;
        }
        if !self.runtime_withdrawn {
            if let Some(host) = &self.projection_host {
                if let Some(tree) = &self.ownership_tree {
                    match alephcore::capability::diagnostic_control::DiagnosticControl::new(
                        host.clone(),
                        tree.clone(),
                    ) {
                        Ok(control) => {
                            // Whole-runtime authority ends while the ledger is
                            // open; physical registration scopes stay post-host.
                            let result = control.dispose_runtime();
                            self.runtime_withdrawn = result.is_ok()
                                || tree.is_disposed(
                                    alephcore::capability::ownership::LifetimeScope::Runtime,
                                );
                            if result.is_err() && observation_failure.is_none() {
                                observation_failure =
                                    Some(AlephError::other("runtime capability withdrawal failed"));
                            }
                        }
                        Err(_) if observation_failure.is_none() => {
                            observation_failure =
                                Some(AlephError::other("runtime capability ownership mismatch"));
                        }
                        Err(_) => {}
                    }
                } else if observation_failure.is_none() {
                    observation_failure = Some(AlephError::other(
                        "runtime capability ownership unavailable",
                    ));
                }
            }
        }
        if let Some(host) = &self.projection_host {
            if host.specials().close().is_err() && observation_failure.is_none() {
                observation_failure = Some(AlephError::other("special source close failed"));
            }
        }
        // Emitters may retain Arcs: explicit close drains queued/lagged records
        // and wakes even an idle bridge without waiting for client acknowledgements.
        if let Some(events) = &self.events {
            if events.close().is_err() && observation_failure.is_none() {
                observation_failure = Some(AlephError::other("capability event close failed"));
            }
        }
        if let Some(bridge) = &self.bridge {
            if bridge.clone().join().await.is_err() && observation_failure.is_none() {
                observation_failure =
                    Some(AlephError::other("capability event bridge join failed"));
            }
        }
        let projection_failure = match &self.projection_host {
            Some(host) => {
                let outcome = host.clone().close_and_await().await;
                if outcome.source_joined && outcome.applier_joined {
                    tracing::debug!("projection host drained before registry-scope dispose");
                    None
                } else {
                    Some(AlephError::other(format!(
                        "projection host did not quiesce: source_joined={}, applier_joined={}, source_failed={}, applier_failed={}",
                        outcome.source_joined, outcome.applier_joined, outcome.source_failed, outcome.applier_failed
                    )))
                }
            }
            None => None,
        };
        match failure.or(observation_failure).or(projection_failure) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(in crate::commands::start) async fn finish<T, E>(
        &mut self,
        result: Result<T, E>,
    ) -> Result<T, E> {
        if let Err(error) = self.cleanup().await {
            tracing::warn!(%error, "Startup resource cleanup failed");
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use alephcore::capability::descriptor::CapabilityId;
    use alephcore::capability::facade::Reference;
    use alephcore::capability::ownership::{OwnershipTree, VisibilityScope};
    use alephcore::error::AlephError;
    use alephcore::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryState};
    use alephcore::sync_primitives::Arc;
    use futures::FutureExt;

    use super::StartupResources;

    struct ShutdownTool(alephcore::tools::service::ToolDefinition);

    #[async_trait::async_trait]
    impl alephcore::tools::handlers::ToolHandler for ShutdownTool {
        fn definition(&self) -> alephcore::tools::service::ToolDefinition {
            self.0.clone()
        }

        async fn invoke(
            &self,
            _: serde_json::Value,
        ) -> Result<alephcore::tools::service::ToolOutput, alephcore::tools::service::ToolError>
        {
            Ok(alephcore::tools::service::ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            })
        }
    }

    #[derive(Default)]
    struct ShutdownAudit {
        records: std::sync::Mutex<Vec<alephcore::capability::events::CapabilityEventRecord>>,
        close_on_revoke: std::sync::OnceLock<
            std::sync::Weak<alephcore::capability::projection_host::ProjectionHost>,
        >,
    }

    impl alephcore::capability::events::EventPersistence for ShutdownAudit {
        fn persist(
            &self,
            record: &alephcore::capability::events::CapabilityEventRecord,
        ) -> Result<(), String> {
            self.records.lock().unwrap().push(record.clone());
            if matches!(record.event(), alephcore::capability::events::CapabilityEvent::CapabilityRevoked { id, .. } if id.name == "retained_shutdown_tool")
            {
                if let Some(host) = self
                    .close_on_revoke
                    .get()
                    .and_then(std::sync::Weak::upgrade)
                {
                    // Exercise the real post-mutation Closed check after all
                    // source/tree/STOP locks have been released by observation.
                    let _ = host.close_and_await().now_or_never();
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn runtime_withdrawal_is_observed_before_seal_while_registration_scope_lives() {
        use alephcore::capability::events::{
            CapabilityEvent, CapabilityEventStream, EventError, InMemoryEventStream,
        };
        use alephcore::capability::facade::Zahir;
        use alephcore::capability::ownership::LifetimeScope;
        use alephcore::capability::projection_host::ProjectionHost;
        use alephcore::tools::descriptor::ToolCapabilityDescriptor;
        use alephcore::tools::registry::ToolHandlerRegistry;
        use alephcore::tools::ToolRegistrationScope;

        tokio::time::timeout(Duration::from_secs(10), async {
            for close_after_mutation in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let tree = Arc::new(OwnershipTree::new());
                let memory = MemoryRuntime::new(MemoryRuntimeConfig {
                    metadata_path: root.path().join("memory.db"),
                    note_memory_dir: root.path().join("notes"),
                }, tree.clone());
                let policy = crate::commands::start::capability_init::build_capability_policy(&alephcore::Config::default(), tree.clone()).unwrap();
                let audit = Arc::new(ShutdownAudit::default());
                let events = Arc::new(InMemoryEventStream::with_persistence(64, Some(audit.clone())).unwrap());
                let mut typed = events.subscribe(Default::default());
                let bus = Arc::new(alephcore::gateway::event_bus::GatewayEventBus::new());
                let mut forwarded = bus.subscribe();
                let bridge = crate::commands::start::capability_init::start_capability_event_bridge(&events, bus);
                let registry = ToolHandlerRegistry::new();
                let host = ProjectionHost::mount_with_runtime_and_events(registry.clone(), tree.clone(), memory.clone(), alephcore::capability::special::CapabilityHandlerRegistry::new(), policy.clone(), events.clone(), false);
                assert!(host.wait_until_ready().await);
                let definition: alephcore::tools::service::ToolDefinition = serde_json::from_value(serde_json::json!({
                    "name": "retained_shutdown_tool", "description": "Actual retained scope", "input_schema": {"type": "object"}, "source": alephcore::tools::descriptor::ToolSource::Builtin
                })).unwrap();
                let registration = registry.register(ToolCapabilityDescriptor::from_definition(&definition, 0), Arc::new(ShutdownTool(definition))).unwrap();
                let mut scope = ToolRegistrationScope::new("task7-retained-shutdown");
                scope.track(registration);
                let reference = Reference {
                    id: CapabilityId { namespace: "aleph/tools".into(), name: "retained_shutdown_tool".into() },
                    visibility: VisibilityScope::default(),
                };
                host.facade().resolve(reference.clone()).unwrap();
                assert!(tree.generation(&reference.id, &reference.visibility).is_some());
                if close_after_mutation {
                    audit.close_on_revoke.set(Arc::downgrade(&host)).unwrap();
                }
                let mut owner = StartupResources::new();
                owner.install_memory(memory);
                owner.install_ownership_tree(tree.clone());
                owner.install_projection_host(host.clone());
                owner.install_capability_events(policy, events.clone(), bridge);
                let result = owner.cleanup().await;
                if close_after_mutation {
                    assert!(result.is_err(), "a genuine post-mutation Closed error must not be hidden");
                } else {
                    result.unwrap();
                }
                assert!(tree.is_disposed(LifetimeScope::Runtime), "cleanup must truly withdraw Runtime authority while the scope is still alive");
                assert!(tree.generation(&reference.id, &reference.visibility).is_none());
                assert!(registry.resolve_entry("retained_shutdown_tool").is_some(), "physical registration must remain until after host join");
                assert_eq!(scope.len(), 1);
                assert_eq!(host.diagnostic_status().lifecycle, alephcore::capability::diagnostic_control::DiagnosticLifecycle::Closed);
                owner.bridge.as_ref().unwrap().clone().join().await.unwrap();
                let records = events.snapshot().unwrap().records().to_vec();
                assert_eq!(records.iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityRevoked { id, .. } if id == &reference.id)).count(), 1);
                let audit_records = audit.records.lock().unwrap().clone();
                assert_eq!(serde_json::to_value(&records).unwrap(), serde_json::to_value(&audit_records).unwrap());
                let mut live = Vec::new();
                while let Ok(frame) = forwarded.try_recv() {
                    let frame: serde_json::Value = serde_json::from_str(&frame).unwrap();
                    if frame["topic"] == "capability.event" { live.push(frame["data"].clone()); }
                }
                assert_eq!(serde_json::Value::Array(live), serde_json::to_value(&records).unwrap());
                let mut typed_records = Vec::new();
                while let Ok(record) = typed.recv().await { typed_records.push(record); }
                assert_eq!(serde_json::to_value(&typed_records).unwrap(), serde_json::to_value(&records).unwrap());
                assert!(matches!(typed.recv().await, Err(EventError::Closed)));
                let report = scope.dispose().await;
                assert!(report.steps.iter().all(|(_, result)| result.is_ok()));
                assert!(registry.resolve_entry("retained_shutdown_tool").is_none());
                assert_eq!(serde_json::to_value(events.snapshot().unwrap().records()).unwrap(), serde_json::to_value(&records).unwrap(), "late physical scope disposal must neither add nor lose a terminal observation");
                assert_eq!(audit.records.lock().unwrap().len(), records.len());
                owner.cleanup().await.unwrap();
                let mut drained = events.subscribe(Default::default());
                while drained.recv().await.is_ok() {}
                assert!(matches!(drained.recv().await, Err(EventError::Closed)));
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn early_startup_cleanup_observes_original_blocking_work() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let root = tempfile::tempdir().unwrap();
            let tree = Arc::new(OwnershipTree::new());
            let runtime = MemoryRuntime::new(
                MemoryRuntimeConfig {
                    metadata_path: root.path().join("memory.db"),
                    note_memory_dir: root.path().join("notes"),
                },
                tree.clone(),
            );
            let policy = alephcore::capability::production_init::build_capability_policy(&alephcore::Config::default(), tree.clone()).unwrap();
            let events = Arc::new(alephcore::capability::events::InMemoryEventStream::default());
            let bus = Arc::new(alephcore::gateway::event_bus::GatewayEventBus::new());
            let mut forwarded = bus.subscribe();
            let bridge = crate::commands::start::capability_init::start_capability_event_bridge(&events, bus);
            let mut owner = StartupResources::new();
            owner.install_memory(runtime.clone());
            owner.install_ownership_tree(tree.clone());
            owner.install_capability_events(policy.clone(), events.clone(), bridge);
            let host = alephcore::capability::projection_host::ProjectionHost::mount_with_runtime_and_events(alephcore::tools::registry::ToolHandlerRegistry::new(), tree, runtime.clone(), alephcore::capability::special::CapabilityHandlerRegistry::new(), policy.clone(), events.clone(), false);
            assert!(host.wait_until_ready().await);
            owner.install_projection_host(host);
            runtime.start().await.unwrap();
            let operation = runtime
                .capture(&Reference {
                    id: CapabilityId {
                        namespace: "aleph/resources".into(),
                        name: "memory.runtime".into(),
                    },
                    visibility: VisibilityScope::default(),
                })
                .unwrap()
                .admit()
                .unwrap();
            let (entered, started) = tokio::sync::oneshot::channel();
            let (release, wait) = mpsc::channel();
            let effect = root.path().join("completed");
            let path = effect.clone();
            let mut work = Box::pin(operation.run_blocking(move || {
                let _ = entered.send(());
                wait.recv()
                    .map_err(|error| AlephError::other(error.to_string()))?;
                std::fs::write(path, "real blocking work completed")
                    .map_err(|error| AlephError::other(error.to_string()))
            }));
            assert!(work.as_mut().now_or_never().is_none());
            started.await.unwrap();
            drop(work);
            drop(operation);
            let mut cleanup = Box::pin(owner.cleanup());
            assert!(
                cleanup.as_mut().now_or_never().is_none(),
                "startup cleanup must observe the original pending blocking job"
            );
            drop(cleanup);
            assert!(!effect.exists());
            release.send(()).unwrap();
            owner.cleanup().await.unwrap();
            assert_eq!(runtime.status().state, MemoryState::Off);
            // The original bridge remains joinable after a cancelled cleanup;
            // producer withdrawal was forwarded before the explicit hub seal.
            owner.bridge.as_ref().unwrap().clone().join().await.unwrap();
            let mut revoked = 0;
            while let Ok(frame) = forwarded.try_recv() {
                let frame: serde_json::Value = serde_json::from_str(&frame).unwrap();
                if frame["topic"] == "capability.event" && frame["data"]["event"]["type"] == "capability_revoked" { revoked += 1; }
            }
            assert_eq!(revoked, 1, "the real Memory source final withdrawal drains through the retained bridge");
            owner.cleanup().await.unwrap();
            assert_eq!(
                std::fs::read_to_string(effect).unwrap(),
                "real blocking work completed"
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn early_registration_error_drains_memory_and_its_projection_host() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let root = tempfile::tempdir().unwrap();
            let tree = Arc::new(OwnershipTree::new());
            let runtime = MemoryRuntime::new(
                MemoryRuntimeConfig {
                    metadata_path: root.path().join("memory.db"),
                    note_memory_dir: root.path().join("notes"),
                },
                tree.clone(),
            );
            let mut owner = StartupResources::new();
            owner.install_memory(runtime.clone());
            owner.install_ownership_tree(tree.clone());
            let registry = alephcore::tools::registry::ToolHandlerRegistry::new();
            let host = alephcore::capability::projection_host::ProjectionHost::mount_with_memory(
                registry,
                tree,
                runtime.clone(),
                false,
            );
            owner.install_projection_host(host.clone());
            runtime.start().await.unwrap();
            let result = owner
                .finish(Err::<(), _>(AlephError::other("registration rejected")))
                .await;
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("registration rejected"));
            assert_eq!(runtime.status().state, MemoryState::Off);
            let status = host.diagnostic_status();
            assert_eq!(
                status.lifecycle,
                alephcore::capability::diagnostic_control::DiagnosticLifecycle::Closed
            );
            owner.cleanup().await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn early_startup_failure_preserves_original_error_after_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: root.path().join("memory.db"),
                note_memory_dir: root.path().join("notes"),
            },
            Arc::new(OwnershipTree::new()),
        );
        let mut owner = StartupResources::new();
        owner.install_memory(runtime.clone());
        runtime.start().await.unwrap();
        let result = owner
            .finish(Err::<(), _>(AlephError::other("original startup failure")))
            .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("original startup failure"));
        assert_eq!(
            runtime.status().state,
            MemoryState::Off,
            "an early error must drain the acquired source"
        );
    }
}
