use alephcore::capability::projection_host::ProjectionHost;
use alephcore::error::AlephError;
use alephcore::memory::runtime::MemoryRuntime;
use alephcore::sync_primitives::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Keeps acquired startup resources reachable until common cleanup completes.
pub(in crate::commands::start) struct StartupResources {
    memory: Option<Arc<MemoryRuntime>>,
    projection_host: Option<Arc<ProjectionHost>>,
    cleanup_called: AtomicBool,
}

impl StartupResources {
    pub(in crate::commands::start) fn new() -> Self {
        Self {
            memory: None,
            projection_host: None,
            cleanup_called: AtomicBool::new(false),
        }
    }

    pub(in crate::commands::start) fn install_memory(&mut self, runtime: Arc<MemoryRuntime>) {
        self.memory = Some(runtime);
    }

    pub(in crate::commands::start) fn install_projection_host(
        &mut self,
        host: Arc<ProjectionHost>,
    ) {
        self.projection_host = Some(host);
    }

    pub(in crate::commands::start) async fn cleanup(&mut self) -> Result<(), AlephError> {
        if self.cleanup_called.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already cleaned up
        }
        let failure = match &self.memory {
            Some(memory) => memory.stop().await.err(),
            None => None,
        };
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
        match failure.or(projection_failure) {
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

    #[tokio::test]
    async fn early_startup_cleanup_observes_original_blocking_work() {
        tokio::time::timeout(Duration::from_secs(10), async {
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
