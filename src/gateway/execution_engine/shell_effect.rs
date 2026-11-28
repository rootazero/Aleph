//! Private authority composition for a paired Shell input attempt.

use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::approval::operator_requester::OperatorApprovalRequester;
use crate::builtin_tools::process_registry::{
    encode_session_label, process_registry, RegisterOutcome,
};
use crate::builtin_tools::{code_exec, process_completion};
use crate::capability::facade::BackendLease;
use crate::capability::zahir_facade::ZahirFacade;
use crate::config::types::{agents_def::agent_admits_user, policies::EXEC_TIER_SESSION_KEY};
use crate::extension::PermissionAction;
use crate::gateway::agent_instance::{AgentInstance, AgentRegistry};
use crate::gateway::busy_queue::paired_shell_owners_for_run;
use crate::gateway::router::SessionKey;
use crate::gateway::server::connection::dispatch::PairedShellOrigin;
use crate::gateway::session_store::file_backend::{ShellSessionGuard, ShellSessionIdentity};
use crate::gateway::session_store::SessionStore;
use crate::sandbox::context::LIVE_TAIL;
use crate::sandbox::live_tail::LiveTail;
use crate::sandbox::shell_input::{
    ShellEffectDenied, ShellEffectResult, ShellInputAction, ShellPipeAttempt, JSONL_ARGS,
    JSONL_PROGRAM,
};
use crate::sandbox::shell_lifecycle::ShellLifecycle;
use crate::sandbox::shell_policy::ShellPolicy;
use crate::sandbox::shell_workspace::ShellWorkspace;
use crate::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};
use crate::sync_primitives::Arc;
use crate::tools::scoped::ScopedToolService;

use super::{
    is_unattended, slash_skill_scope, turn_permissions, RunRequest, CHANNEL_TOOL_PERMISSIONS_KEY,
    RESUME_TIER_CEILING_KEY,
};

/// This is a binding to live owners, not a snapshot granting permission.
/// In particular no role, session tier, or merged tool permission is cached.
#[derive(Clone)]
pub(crate) struct PairedShellRoute {
    origin: PairedShellOrigin,
    agents: Arc<AgentRegistry>,
    agent: Arc<AgentInstance>,
    config: Arc<RwLock<crate::Config>>,
    agent_manager: Arc<crate::AgentManager>,
    store: Arc<dyn SessionStore>,
    sandbox: Arc<dyn Sandbox>,
    session: SessionKey,
}

impl PairedShellRoute {
    pub(super) fn for_turn(request: &RunRequest, agent: Arc<AgentInstance>) -> Option<Self> {
        let (origin, owners) = paired_shell_owners_for_run(request)?;
        // Overlays and project overrides have additional mutable owners.
        // An otherwise valid origin does not establish authority over them.
        if request.is_resume()
            || is_unattended(&request.metadata)
            || request.telecom_public.is_some()
            || request.telecom_authority.is_some()
            || request.workspace_override.is_some()
            || request.metadata.contains_key(EXEC_TIER_SESSION_KEY)
            || request.metadata.contains_key(RESUME_TIER_CEILING_KEY)
            || request.metadata.contains_key(CHANNEL_TOOL_PERMISSIONS_KEY)
            || request
                .metadata
                .contains_key(crate::gateway::btw::BTW_METADATA_KEY)
            || !slash_skill_scope::pregrant_from_metadata(&request.metadata).is_empty()
            || !matches!(request.session_key, SessionKey::Main { .. })
            || request.session_key.agent_id() != agent.id()
            || super::run_loop::request_scope(request)
                != Some(crate::scope::ScopeAttribution::personal(origin.user_id()))
            || !Arc::ptr_eq(&owners.sessions, &agent.session_store())
        {
            return None;
        }
        Some(Self {
            origin,
            agents: owners.agents,
            agent,
            config: owners.config,
            agent_manager: owners.agent_manager,
            store: owners.sessions,
            sandbox: owners.sandbox,
            session: request.session_key.clone(),
        })
    }

    pub(crate) fn can_offer(&self) -> bool {
        let Ok(config) = self.config.try_read() else {
            return false;
        };
        self.agent.is_tool_allowed("bash")
            && self
                .sandbox
                .as_workspace()
                .is_some_and(|workspace| workspace.can_run_fixed_jsonl(&config))
            && self.agents.try_shell_agent_guard(&self.agent).is_ok()
            && self
                .origin
                .try_effect_guard()
                .is_ok_and(|guard| guard.role_at_effect().is_some())
    }

    pub(crate) async fn launch(
        &self,
        service: &ScopedToolService,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<u64, ShellEffectDenied> {
        if timeout.is_zero() || timeout > Duration::from_secs(4 * 60 * 60) {
            return Err(ShellEffectDenied::Expired);
        }
        let job = Arc::new(ShellLifecycle::default());
        let deadline = Instant::now() + timeout;
        let path = self.agent.workspace().to_path_buf();
        let workspace = tokio::task::spawn_blocking(move || ShellWorkspace::pin(&path))
            .await
            .map_err(|_| ShellEffectDenied::Stale)?
            .map_err(|_| ShellEffectDenied::Stale)?;
        let launch = SandboxCommand {
            session_id: self.session.clone(),
            tool_name: "bash".into(),
            program: JSONL_PROGRAM.into(),
            args: JSONL_ARGS.into_iter().map(str::to_owned).collect(),
            cwd: Some(workspace.path().to_path_buf()),
            env: Default::default(),
            capabilities: crate::sandbox::SandboxCapabilities {
                max_memory_mb: Some(256),
                ..Default::default()
            },
            stdin: None,
            timeout: Some(timeout),
        };
        let authority = Arc::new(
            ShellInputAuthority::bind(
                self.clone(),
                launch,
                service,
                workspace,
                job.clone(),
                deadline,
            )
            .await?,
        );
        authority.check_launch().await?;
        if cancel.is_cancelled() || Instant::now() >= deadline {
            return Err(ShellEffectDenied::Expired);
        }
        let registry = process_registry();
        let label = "jq (jsonl_inspect)";
        let id = match registry.reserve_shell(
            label,
            Some(encode_session_label(&self.session)),
            job.clone(),
        ) {
            RegisterOutcome::Registered(id) => id,
            RegisterOutcome::TooManyRunning { .. } | RegisterOutcome::AdmissionClosed => {
                return Err(ShellEffectDenied::Busy);
            }
        };
        let live = Arc::new(LiveTail::for_shell());
        registry.attach_live(id, live.clone());
        if !registry.attach_shell_input(id, &job, authority.clone()) {
            job.request_cancel();
            let _ = job.cancel_before_spawn();
            let output = code_exec::sandbox_result_to_output(
                Err(SandboxError::ExecutionFailed(
                    "fixed JSONL reservation lost".into(),
                )),
                "bash".into(),
                timeout.as_secs(),
            );
            registry.complete(id, output);
            return Err(ShellEffectDenied::Stale);
        }
        let identity = crate::approval::current_call_identity();
        let turn = crate::tools::turn_context::current_turn_context();
        let originator = crate::tools::turn_context::current_originator();
        let session = self.session.clone();
        tokio::spawn(async move {
            use crate::sandbox::context::{with_exec_workspace_binding, ExecWorkspace, SESSION_ID};
            use crate::tools::turn_context::{with_originator, TURN_CONTEXT};

            let workspace = ExecWorkspace::Directory(authority.workspace.path().to_path_buf());
            let execute = SESSION_ID.scope(
                session.clone(),
                LIVE_TAIL.scope(
                    live,
                    with_exec_workspace_binding(Some(workspace), authority.run()),
                ),
            );
            let execute =
                crate::approval::with_call_identity(identity, with_originator(originator, execute));
            let result = match turn {
                Some(turn) => TURN_CONTEXT.scope(turn, execute).await,
                None => execute.await,
            };
            let output =
                code_exec::sandbox_result_to_output(result, "bash".into(), timeout.as_secs());
            let event = process_completion::completion_event(
                id,
                label,
                output.exit_code,
                &output.stdout,
                &output.stderr,
            );
            if registry.complete(id, output) {
                process_completion::broadcast(&session, event).await;
            }
        });
        Ok(id)
    }

    pub(crate) async fn input(
        &self,
        service: &ScopedToolService,
        approval: &OperatorApprovalRequester,
        id: u64,
        cancel: &CancellationToken,
        action: ShellInputAction,
    ) -> Result<ShellEffectResult, ShellEffectDenied> {
        let authority = process_registry()
            .shell_input(id, &self.session, matches!(action, ShellInputAction::Close))
            .ok_or(ShellEffectDenied::Stale)?;
        authority
            .apply(
                self,
                service,
                approval,
                id,
                authority.deadline,
                cancel,
                action,
            )
            .await
    }

    fn same_owners(&self, current: &Self) -> bool {
        self.origin.same_connection(&current.origin)
            && Arc::ptr_eq(&self.agents, &current.agents)
            && Arc::ptr_eq(&self.agent, &current.agent)
            && Arc::ptr_eq(&self.config, &current.config)
            && Arc::ptr_eq(&self.agent_manager, &current.agent_manager)
            && Arc::ptr_eq(&self.store, &current.store)
            && Arc::ptr_eq(&self.sandbox, &current.sandbox)
            && self.session == current.session
    }

    async fn session_guard(
        &self,
        expected: Option<&ShellSessionIdentity>,
    ) -> Result<ShellSessionGuard, ShellEffectDenied> {
        self.store
            .prepare_shell_session(&self.session, expected)
            .await
            .map_err(|_| ShellEffectDenied::Stale)?
            .ok_or(ShellEffectDenied::Stale)
    }

    fn check_session<'a>(
        &self,
        session: &'a ShellSessionGuard,
    ) -> Result<&'a crate::gateway::session_store::types::SessionMetadata, ShellEffectDenied> {
        let metadata = session.metadata().ok_or(ShellEffectDenied::Stale)?;
        let user = self.origin.user_id();
        let scope = crate::scope::ScopeAttribution::from_persisted(
            metadata.owner_user_id.as_deref(),
            metadata.scope_id.as_deref(),
        );
        if metadata.agent_id != self.agent.id()
            || scope != Some(crate::scope::ScopeAttribution::personal(user))
            || !crate::gateway::visibility::session_visible_to(metadata, user)
            || !agent_admits_user(self.agent.config().allowed_users.as_deref(), Some(user))
            || metadata
                .identity_meta
                .as_ref()
                .is_some_and(|identity| identity.custom.contains_key("project_root"))
        {
            return Err(ShellEffectDenied::Stale);
        }
        Ok(metadata)
    }
}

pub(crate) struct ShellInputAuthority {
    route: PairedShellRoute,
    session: ShellSessionIdentity,
    workspace: ShellWorkspace,
    sandbox: Arc<dyn Sandbox>,
    policy: Arc<ShellPolicy>,
    launch: SandboxCommand,
    facade: Arc<ZahirFacade>,
    lease: BackendLease,
    job: Arc<ShellLifecycle>,
    deadline: Instant,
}

/// Stop the stream if a worker can no longer hand its receipt to the caller.
/// Created inside the worker so dropping a queued, unexecuted attempt does not
/// stop an otherwise healthy job.
struct UnobservedDelivery(Option<Arc<ShellLifecycle>>);

impl UnobservedDelivery {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for UnobservedDelivery {
    fn drop(&mut self) {
        if let Some(job) = &self.0 {
            job.request_cancel();
        }
    }
}

impl ShellInputAuthority {
    /// Bootstrap from the admitted launch, before exposing its input handle.
    #[allow(clippy::too_many_arguments)]
    async fn bind(
        route: PairedShellRoute,
        launch: SandboxCommand,
        service: &ScopedToolService,
        workspace: ShellWorkspace,
        job: Arc<ShellLifecycle>,
        deadline: Instant,
    ) -> Result<Self, ShellEffectDenied> {
        if launch.session_id != route.session
            || launch.tool_name != "bash"
            || launch.program != JSONL_PROGRAM
            || !launch.args.iter().map(String::as_str).eq(JSONL_ARGS)
            || !launch.env.is_empty()
            || launch.stdin.is_some()
        {
            return Err(ShellEffectDenied::Stale);
        }
        let (facade, lease) = service.capture_paired_shell()?;
        let sandbox = route.sandbox.clone();
        let policy = sandbox
            .as_workspace()
            .and_then(|workspace| workspace.fixed_jsonl_policy())
            .ok_or(ShellEffectDenied::UnverifiedPolicy)?
            .clone();
        let session = route.session_guard(None).await?;
        route.check_session(&session)?;
        let identity = session.identity().clone();
        drop(session);
        if launch.cwd.as_deref() != Some(workspace.path()) {
            return Err(ShellEffectDenied::Stale);
        }
        Ok(Self {
            route,
            session: identity,
            workspace,
            sandbox,
            policy,
            launch,
            facade,
            lease,
            job,
            deadline,
        })
    }

    async fn check_launch(&self) -> Result<(), ShellEffectDenied> {
        let session = self.route.session_guard(Some(&self.session)).await?;
        let metadata = self.route.check_session(&session)?;
        let config = self
            .route
            .config
            .try_read()
            .map_err(|_| ShellEffectDenied::Busy)?;
        let _agent = self.route.agents.try_shell_agent_guard(&self.route.agent)?;
        if !self.policy.matches_config(&config) {
            return Err(ShellEffectDenied::UnverifiedPolicy);
        }
        let account = self.route.origin.try_effect_guard()?;
        let role = account.role_at_effect().ok_or(ShellEffectDenied::Expired)?;
        if self.check_permission(&config, metadata, role)? != PermissionAction::Allow {
            return Err(ShellEffectDenied::ApprovalRequired);
        }
        self.facade
            .validate_completion(&self.lease)
            .map_err(|_| ShellEffectDenied::Stale)
    }

    fn check_permission(
        &self,
        config: &crate::Config,
        metadata: &crate::gateway::session_store::types::SessionMetadata,
        role: crate::gateway::security::store::UserRole,
    ) -> Result<PermissionAction, ShellEffectDenied> {
        let permission =
            turn_permissions::paired_shell_permission(config, metadata, &self.route.agent, role)?;
        if !self.route.agent.is_tool_allowed("bash") || permission == PermissionAction::Deny {
            return Err(ShellEffectDenied::Stale);
        }
        Ok(permission)
    }

    /// Run the native child using the exact workspace and policy-bearing
    /// sandbox captured for subsequent approved input effects.
    pub(super) async fn run(&self) -> Result<SandboxOutput, SandboxError> {
        let sandbox =
            self.sandbox
                .as_workspace()
                .ok_or_else(|| SandboxError::CapabilityDenied {
                    reason: "fixed JSONL sandbox is unavailable".into(),
                })?;
        sandbox
            .execute_fixed_jsonl(
                self.launch.clone(),
                &self.workspace,
                &self.job,
                self.deadline.into(),
            )
            .await
    }

    pub(super) async fn apply(
        self: &Arc<Self>,
        current: &PairedShellRoute,
        service: &ScopedToolService,
        approval: &OperatorApprovalRequester,
        job_id: u64,
        deadline: Instant,
        cancel: &CancellationToken,
        action: ShellInputAction,
    ) -> Result<ShellEffectResult, ShellEffectDenied> {
        if !self.route.same_owners(current) {
            return Err(ShellEffectDenied::Stale);
        }
        let (facade, lease) = service.capture_paired_shell()?;
        if !Arc::ptr_eq(&self.facade, &facade) || self.lease != lease {
            return Err(ShellEffectDenied::Stale);
        }
        let attempt_cancel = cancel.child_token();
        // Also stop pending work if the enclosing future is abandoned before
        // its blocking task reaches the job fence.
        let _cancel_on_drop = attempt_cancel.clone().drop_guard();
        let attempt = ShellPipeAttempt::prepare(
            self.job.clone(),
            job_id,
            &self.policy,
            &self.launch,
            approval,
            deadline.min(self.deadline),
            attempt_cancel,
            action,
        )
        .await?;
        let session = self.route.session_guard(Some(&self.session)).await?;
        let authority = self.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let mut delivery = UnobservedDelivery(Some(authority.job.clone()));
            let result = authority.commit(session, attempt);
            // Classification is exhaustive: any future effect must decide
            // whether losing its receipt makes further input unsafe.
            match &result {
                Err(_) | Ok(ShellEffectResult::AlreadyClosed) => delivery.disarm(),
                Ok(
                    ShellEffectResult::Written(_)
                    | ShellEffectResult::Closed
                    | ShellEffectResult::Ambiguous,
                ) => {}
            }
            (result, delivery)
        });
        match worker.await {
            Ok((result, mut delivery)) => {
                delivery.disarm();
                result
            }
            Err(error) if error.is_cancelled() => Err(ShellEffectDenied::Expired),
            // The worker's guard also stops the job if commit unwinds.
            Err(_) => Ok(ShellEffectResult::Ambiguous),
        }
    }

    fn commit(
        &self,
        session: ShellSessionGuard,
        attempt: ShellPipeAttempt,
    ) -> Result<ShellEffectResult, ShellEffectDenied> {
        // Session acquisition reads metadata before entering the blocking
        // operation. Every other lock here is try-only. Device revocation
        // already orders connection before the SQLite store.
        let metadata = self.route.check_session(&session)?;
        let _archive = self
            .route
            .agent_manager
            .try_workspace_archive_guard()
            .map_err(|error| {
                if crate::utils::atomic_io::is_lock_contended(&error) {
                    ShellEffectDenied::Busy
                } else {
                    ShellEffectDenied::Stale
                }
            })?;
        let config = self
            .route
            .config
            .try_read()
            .map_err(|_| ShellEffectDenied::Busy)?;
        let _agent = self.route.agents.try_shell_agent_guard(&self.route.agent)?;
        if !self.policy.matches_config(&config) {
            return Err(ShellEffectDenied::UnverifiedPolicy);
        }
        if !self
            .workspace
            .is_current(self.route.agent.workspace())
            .unwrap_or(false)
        {
            return Err(ShellEffectDenied::Stale);
        }
        let account = self.route.origin.try_effect_guard()?;
        let role = account.role_at_effect().ok_or(ShellEffectDenied::Expired)?;
        self.check_permission(&config, metadata, role)?;
        self.facade
            .commit_shell_pipe_attempt(&self.lease, account.bound_attempt(attempt)?)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn shell_workspace_witness_rejects_replacement_at_the_same_path() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let witness = ShellWorkspace::pin(&workspace).unwrap();
        assert!(witness.is_current(&workspace).unwrap());

        std::fs::rename(&workspace, root.path().join("archived")).unwrap();
        assert!(witness.is_current(&workspace).is_err());
        std::fs::create_dir(&workspace).unwrap();
        assert!(!witness.is_current(&workspace).unwrap());
    }

    #[tokio::test]
    async fn shell_unobserved_commit_closes_input_and_stops() {
        use crate::sandbox::shell_input::{nonblocking_test_pipe, JsonlFrame};
        use std::io::{ErrorKind, Read};
        use std::time::Duration;

        let (mut reader, writer) = nonblocking_test_pipe();
        let job = Arc::new(ShellLifecycle::default());
        job.spawn(|| Ok(())).unwrap();
        job.ready_with_input(writer).unwrap();
        let attempt = ShellPipeAttempt::new_for_test(
            job.clone(),
            0,
            Instant::now() + Duration::from_secs(5),
            ShellInputAction::Write(JsonlFrame::from_record(&serde_json::Value::Null).unwrap()),
        );
        let (committed, committed_rx) = tokio::sync::oneshot::channel();
        let worker_job = job.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let delivery = UnobservedDelivery(Some(worker_job));
            assert_eq!(attempt.consume(), Ok(ShellEffectResult::Written(5)));
            committed.send(()).unwrap();
            delivery
        });
        committed_rx.await.unwrap();
        // Dropping a JoinHandle detaches its worker and discards its receipt.
        drop(worker);
        tokio::time::timeout(Duration::from_secs(5), job.cancelled())
            .await
            .expect("unclaimed receipt must stop the stream");
        let mut delivered = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match reader.read_to_end(&mut delivered) {
                    Ok(_) => break,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        tokio::task::yield_now().await;
                    }
                    Err(error) => panic!("could not observe closed input: {error}"),
                }
            }
        })
        .await
        .expect("the writer must be closed");
        assert_eq!(delivered, b"null\n");
    }

    /// The Panel RPC fixture supplies the origin; the sandbox factory supplies
    /// the policy. The native variant also owns a real Workspace child.
    pub(crate) async fn assert_paired_shell_effect_order(
        origin: PairedShellOrigin,
        account_store: &crate::gateway::security::SecurityStore,
        native: bool,
    ) {
        use crate::capability::ownership::OwnershipTree;
        use crate::config::types::policies::ExecTier;
        use crate::exec::{socket::ApprovalDecisionType, ExecApprovalManager};
        use crate::gateway::agent_instance::AgentInstanceConfig;
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::events::GatewayEventFrame;
        use crate::gateway::security::store::UserStatus;
        use crate::gateway::session_store::file_backend::{
            FileSessionStore, FileSessionStoreConfig,
        };
        use crate::gateway::session_store::types::SessionPatch;
        use crate::sandbox::exec_approval::gate::ApprovalGate;
        use crate::sandbox::platforms::linux::bwrap::{BubblewrapDriver, LinuxSandboxOptions};
        use crate::sandbox::shell_input::{nonblocking_test_pipe, JsonlFrame};
        use crate::tools::adapters::mcp_adapter::McpRegistryTool;
        use crate::tools::descriptor::ToolCapabilityDescriptor;
        use crate::tools::handlers::{builtin::BuiltinHandler, ToolHandler};
        use crate::tools::registry::ToolHandlerRegistry;
        use crate::tools::runtime::LoopToolRegistry;
        use std::io::{ErrorKind, Read};
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("aleph.toml");
        let workspace_root = root.path().join("workspaces");
        let workspace = workspace_root.join("main");
        let agents_root = root.path().join("agents");
        let agent_manager = Arc::new(crate::AgentManager::new(
            config_path.clone(),
            workspace_root,
            agents_root.clone(),
            root.path().join("trash"),
        ));
        crate::utils::atomic_io::with_file_lock(&config_path.with_extension("toml.lock"), |_| {
            Ok(())
        })
        .unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: root.path().join("sessions"),
                ..Default::default()
            })
            .unwrap(),
        );
        let session = SessionKey::main("main");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(origin.user_id())),
            store.get_or_create(&session),
        )
        .await
        .unwrap();
        let agents = Arc::new(AgentRegistry::new());
        agents
            .register(
                AgentInstance::new(
                    AgentInstanceConfig {
                        workspace: workspace.clone(),
                        agent_dir: agents_root.join("main"),
                        ..Default::default()
                    },
                    store.clone(),
                )
                .unwrap(),
            )
            .await;
        let agent = agents.get("main").await.unwrap();
        let mut config = crate::Config::default();
        config.security.enable_custom_patterns = false;
        config
            .policies
            .tool_permissions
            .overrides
            .insert("bash".into(), PermissionAction::Allow);
        config.sandbox.enabled = true;
        config.sandbox.workspace_root = root.path().join("sandbox");
        let sandbox = crate::sandbox::build_sandbox(
            &config.sandbox,
            Arc::new(BubblewrapDriver::with_options(LinuxSandboxOptions {
                cpu_quota_percent: Some(50),
                max_pids: Some(16),
                ..Default::default()
            })),
            Arc::new(ApprovalGate::new(None)),
            Default::default(),
            &config.security,
        );
        let mut metadata = std::collections::HashMap::new();
        crate::scope::stamp_metadata(
            &mut metadata,
            &crate::scope::ScopeAttribution::personal(origin.user_id()),
        );
        let request = RunRequest {
            run_id: "shell-fixture".into(),
            input: String::new(),
            session_key: session.clone(),
            timeout_secs: None,
            metadata,
            attachments: Vec::new(),
            pending_media: Default::default(),
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
            telecom_public: None,
            telecom_authority: None,
        };
        let owners = crate::gateway::busy_queue::PanelShellOwners {
            agents,
            config: Arc::new(RwLock::new(config)),
            agent_manager,
            sessions: store,
            sandbox: sandbox.clone(),
        };
        assert!(PairedShellRoute::for_turn(&request, agent.clone()).is_none());
        let current =
            crate::gateway::busy_queue::with_paired_shell_worker_fixture(&request, owners, async {
                let mut wrong_run = request.clone();
                wrong_run.run_id.push_str("-nested");
                assert!(PairedShellRoute::for_turn(&wrong_run, agent.clone()).is_none());

                let mut wrong_session = request.clone();
                wrong_session.session_key = SessionKey::main("other");
                assert!(paired_shell_owners_for_run(&wrong_session).is_none());

                let mut wrong_scope = request.clone();
                crate::scope::stamp_metadata(
                    &mut wrong_scope.metadata,
                    &crate::scope::ScopeAttribution::personal("other-user"),
                );
                assert!(PairedShellRoute::for_turn(&wrong_scope, agent.clone()).is_none());

                PairedShellRoute::for_turn(&request, agent).unwrap()
            })
            .await;
        let registry = Arc::new(ToolHandlerRegistry::new());
        let handler = Arc::new(BuiltinHandler::new(
            "bash".into(),
            Arc::new(crate::builtin_tools::BashExecTool::new()),
        ));
        let descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
        registry
            .register(descriptor.clone(), handler.clone())
            .unwrap();
        let facade = ZahirFacade::new(registry.as_ref().clone(), Arc::new(OwnershipTree::new()));
        let mut projection = LoopToolRegistry::new();
        projection.bind_canonical_registry(registry, Arc::new(|_| false));
        assert!(projection.bind_canonical_facade(facade));
        projection.register(Box::new(McpRegistryTool::from_registry_entry(
            handler,
            &descriptor,
        )));
        let projection = Arc::new(projection);
        let filtered = ScopedToolService::new(projection.clone(), ["other".into()].into());
        assert!(matches!(
            filtered.capture_paired_shell(),
            Err(ShellEffectDenied::Stale)
        ));
        let bus = Arc::new(GatewayEventBus::new());
        let mut events = bus.subscribe_typed();
        let approvals = Arc::new(ExecApprovalManager::new());
        let requester = Arc::new(OperatorApprovalRequester::for_config_tier(
            approvals.clone(),
            bus,
        ));
        let service = ScopedToolService::new(projection, ["bash".into()].into())
            .with_paired_shell(Some(current.clone()))
            .with_operator_approval(requester.clone())
            .with_turn_context(crate::tools::turn_context::TurnContext {
                session_key: request.session_key.clone(),
                run_id: request.run_id.clone(),
                channel_id: String::new(),
                conversation_id: String::new(),
                caller_role: Some("member".into()),
                channel_tool_permissions: None,
                unattended: false,
                plan_gate: None,
                side_question: false,
                telecom_authority: None,
            });
        let deadline = Instant::now() + Duration::from_secs(10);
        let cancel = CancellationToken::new();
        let set_status = |status| {
            account_store
                .update_user(current.origin.user_id(), None, None, Some(status))
                .unwrap();
        };

        #[derive(Clone, Copy, Debug)]
        enum Mutation {
            Account,
            ToolPermission,
            SessionTier,
            MountedPolicy,
        }
        let mutate = async |mutation, revoked| match mutation {
            Mutation::Account => set_status(if revoked {
                UserStatus::Deactivated
            } else {
                UserStatus::Active
            }),
            Mutation::ToolPermission => {
                current
                    .config
                    .write()
                    .await
                    .policies
                    .tool_permissions
                    .overrides
                    .insert(
                        "bash".into(),
                        if revoked {
                            PermissionAction::Deny
                        } else {
                            PermissionAction::Allow
                        },
                    );
            }
            Mutation::SessionTier => {
                let tier = if revoked {
                    ExecTier::Plan
                } else {
                    ExecTier::Ask
                };
                current
                    .store
                    .patch_session(
                        &current.session,
                        &SessionPatch {
                            metadata: Some(serde_json::json!({ EXEC_TIER_SESSION_KEY: tier.id() })),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            }
            Mutation::MountedPolicy => {
                current.config.write().await.security.enable_custom_patterns = revoked;
            }
        };
        let write = |value| ShellInputAction::Write(JsonlFrame::from_record(&value).unwrap());
        let approve = |events: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>| {
            let GatewayEventFrame::ApprovalRequested { approval_id, .. } =
                events.try_recv().unwrap()
            else {
                panic!("expected operator approval");
            };
            assert!(approvals.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None));
        };
        let check_resolved = |events: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>| {
            assert!(matches!(
                events.try_recv(),
                Ok(GatewayEventFrame::ApprovalResolved { .. })
            ));
        };

        if native {
            use crate::builtin_tools::process_registry::{ReadOutcome, WaitOutcome};

            let id = service
                .launch_paired_shell(Duration::from_secs(15), &cancel)
                .await
                .expect("reserve fixed jq through the scoped dispatcher");
            let registry = process_registry();
            let caller = encode_session_label(&current.session);
            assert!(registry
                .shell_input(id, &SessionKey::main("other"), false)
                .is_none());
            let authority = registry.shell_input(id, &current.session, false).unwrap();
            let job = authority.job.clone();
            tokio::time::timeout_at(deadline.into(), async {
                while job.input_progress_for_test().is_err() {
                    assert!(registry.shell_input(id, &current.session, false).is_some());
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("fixed consumer never attached");

            let apply = |action| Box::pin(service.input_paired_shell(id, &cancel, action));
            for (mutation, error) in [
                (Mutation::Account, ShellEffectDenied::Stale),
                (Mutation::ToolPermission, ShellEffectDenied::Stale),
                (Mutation::SessionTier, ShellEffectDenied::Stale),
                (Mutation::MountedPolicy, ShellEffectDenied::UnverifiedPolicy),
            ] {
                // Both effects must leave the *same live stream* untouched.
                for action in [write(serde_json::json!([1, 2, 3])), ShellInputAction::Close] {
                    let mut denied = apply(action);
                    assert!(futures::poll!(denied.as_mut()).is_pending());
                    approve(&mut events);
                    mutate(mutation, true).await;
                    assert_eq!(denied.await.as_ref(), Err(&error), "{mutation:?}");
                    assert_eq!(job.input_progress_for_test().unwrap(), (0, false));
                    check_resolved(&mut events);
                    mutate(mutation, false).await;
                }
            }

            let mut busy = apply(write(serde_json::json!([1, 2, 3])));
            assert!(futures::poll!(busy.as_mut()).is_pending());
            approve(&mut events);
            let config_writer = current.config.write().await;
            assert_eq!(busy.await, Err(ShellEffectDenied::Busy));
            check_resolved(&mut events);
            drop(config_writer);

            let pending = CancellationToken::new();
            let mut abandoned = Box::pin(service.input_paired_shell(
                id,
                &pending,
                write(serde_json::json!([1, 2, 3])),
            ));
            assert!(futures::poll!(abandoned.as_mut()).is_pending());
            approve(&mut events);
            pending.cancel();
            assert_eq!(abandoned.await, Err(ShellEffectDenied::Expired));
            assert!(matches!(
                events.try_recv(),
                Ok(GatewayEventFrame::ApprovalExpired { .. })
            ));
            assert_eq!(job.input_progress_for_test().unwrap(), (0, false));

            let mut accepted = apply(write(serde_json::json!([1, 2])));
            assert!(futures::poll!(accepted.as_mut()).is_pending());
            approve(&mut events);
            assert_eq!(accepted.await, Ok(ShellEffectResult::Written(6)));
            set_status(UserStatus::Deactivated);
            let cursor = tokio::time::timeout_at(deadline.into(), async {
                loop {
                    let ReadOutcome::View {
                        stdout,
                        next_cursor,
                        ..
                    } = registry.read(id, &current.session, None)
                    else {
                        panic!("fixed consumer output is unavailable");
                    };
                    if !stdout.text.is_empty() {
                        assert_eq!(stdout.text.trim_end(), r#"{"kind":"array","count":2}"#);
                        break next_cursor;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("fixed consumer should emit before EOF");
            assert!(matches!(
                events.try_recv(),
                Ok(GatewayEventFrame::ApprovalResolved { .. })
            ));
            for action in [write(serde_json::json!([0])), ShellInputAction::Close] {
                let mut revoked = apply(action);
                assert!(futures::poll!(revoked.as_mut()).is_pending());
                approve(&mut events);
                assert_eq!(revoked.await, Err(ShellEffectDenied::Stale));
                assert!(matches!(
                    events.try_recv(),
                    Ok(GatewayEventFrame::ApprovalResolved { .. })
                ));
            }
            assert_eq!(job.input_progress_for_test().unwrap(), (6, false));
            set_status(UserStatus::Active);

            let mut another = apply(write(serde_json::json!({"private_marker": 42})));
            assert!(futures::poll!(another.as_mut()).is_pending());
            approve(&mut events);
            assert_eq!(another.await, Ok(ShellEffectResult::Written(22)));
            check_resolved(&mut events);
            let next = tokio::time::timeout_at(deadline.into(), async {
                loop {
                    let ReadOutcome::View {
                        stdout,
                        next_cursor,
                        ..
                    } = registry.read(id, &current.session, Some(&cursor))
                    else {
                        panic!("second fixed consumer record is unavailable");
                    };
                    if !stdout.text.is_empty() {
                        assert_eq!(stdout.text.trim_end(), r#"{"kind":"object","count":1}"#);
                        break next_cursor;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("second record should be processed before EOF");

            let mut eof = apply(ShellInputAction::Close);
            assert!(futures::poll!(eof.as_mut()).is_pending());
            approve(&mut events);
            assert_eq!(eof.await, Ok(ShellEffectResult::Closed));
            let WaitOutcome::Done(output) = registry
                .wait(id, Some(&caller), Duration::from_secs(4))
                .await
            else {
                panic!("native child should settle after EOF");
            };
            assert_eq!(output.exit_code, 0);
            assert_eq!(
                output.stdout.lines().collect::<Vec<_>>(),
                [
                    r#"{"kind":"array","count":2}"#,
                    r#"{"kind":"object","count":1}"#,
                ]
            );
            assert!(output.stderr.is_empty());
            assert!(
                matches!(registry.read(id, &current.session, Some(&next)), ReadOutcome::View { stdout, .. } if stdout.text.is_empty())
            );
            assert!(registry.shell_input(id, &current.session, false).is_none());
            return;
        }

        let workspace = ShellWorkspace::pin(&workspace).unwrap();
        let launch = SandboxCommand {
            session_id: session,
            tool_name: "bash".into(),
            program: JSONL_PROGRAM.into(),
            args: JSONL_ARGS.into_iter().map(str::to_owned).collect(),
            cwd: Some(workspace.path().to_path_buf()),
            env: Default::default(),
            capabilities: crate::sandbox::SandboxCapabilities {
                max_memory_mb: Some(256),
                ..Default::default()
            },
            stdin: None,
            timeout: Some(Duration::from_secs(15)),
        };
        let job = Arc::new(ShellLifecycle::default());
        let authority = Arc::new(
            ShellInputAuthority::bind(
                current.clone(),
                launch,
                &service,
                workspace,
                job.clone(),
                deadline,
            )
            .await
            .unwrap(),
        );
        let (mut reader, writer) = nonblocking_test_pipe();
        job.spawn(|| Ok(())).unwrap();
        job.ready_with_input(writer).unwrap();
        // Revocation wins each independent authority before either write or
        // close. Run the successful write and close last on the same stream.
        let orders = [
            (Some(Mutation::Account), false),
            (Some(Mutation::Account), true),
            (Some(Mutation::ToolPermission), false),
            (Some(Mutation::ToolPermission), true),
            (Some(Mutation::SessionTier), false),
            (Some(Mutation::SessionTier), true),
            (None, false),
            (None, true),
        ];
        for (mutate_first, close) in orders {
            let action = if close {
                ShellInputAction::Close
            } else {
                ShellInputAction::Write(JsonlFrame::from_record(&serde_json::Value::Null).unwrap())
            };
            let mut effect = Box::pin(
                authority.apply(&current, &service, &requester, 1, deadline, &cancel, action),
            );
            assert!(futures::poll!(effect.as_mut()).is_pending());
            let GatewayEventFrame::ApprovalRequested { approval_id, .. } =
                events.try_recv().unwrap()
            else {
                panic!("expected operator approval");
            };
            assert!(approvals.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None));
            if let Some(mutation) = mutate_first {
                mutate(mutation, true).await;
                assert_eq!(effect.await, Err(ShellEffectDenied::Stale));
                assert_eq!(
                    reader.read(&mut [0]).unwrap_err().kind(),
                    ErrorKind::WouldBlock
                );
                mutate(mutation, false).await;
            } else if close {
                assert_eq!(effect.await, Ok(ShellEffectResult::Closed));
                set_status(UserStatus::Deactivated);
                assert_eq!(reader.read(&mut [0]).unwrap(), 0);
            } else {
                assert_eq!(effect.await, Ok(ShellEffectResult::Written(5)));
                set_status(UserStatus::Deactivated);
                let mut bytes = [0; 5];
                reader.read_exact(&mut bytes).unwrap();
                assert_eq!(&bytes, b"null\n");
            }
            set_status(UserStatus::Active);
            assert!(matches!(
                events.try_recv(),
                Ok(GatewayEventFrame::ApprovalResolved { .. })
            ));
        }
        let repeat_eof = || {
            authority.apply(
                &current,
                &service,
                &requester,
                1,
                deadline,
                &cancel,
                ShellInputAction::Close,
            )
        };
        assert_eq!(repeat_eof().await, Ok(ShellEffectResult::AlreadyClosed));
        set_status(UserStatus::Deactivated);
        assert_eq!(repeat_eof().await, Err(ShellEffectDenied::Stale));
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        set_status(UserStatus::Active);
    }
}
