//! Canonical special-tool adapters over the mounted typed domain owners.

use super::descriptor::{
    CapabilityDescriptor, CapabilityFacets, CapabilityId, CapabilityKind, CapabilityRevision,
    ProjectionMetadata, SchemaRef,
};
use super::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use super::provider::{ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport};
use super::special::{
    CapabilityDispatchError, CapabilityHandler, CapabilityHandlerCall, CapabilityHandlerRegistry,
    CapabilityHandlerSourceSlot, CapabilityRegistryError, SpecialRegistration,
};
use super::special_dispatch::CapabilityInvocationError;
use crate::sync_primitives::Arc;
use crate::tools::descriptor::{ReplayPolicy, ToolCapabilityDescriptor, ToolKind, ToolSource};
use crate::tools::registry::{RegistrationHandle, ToolHandlerRegistry};
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolOutput};
use aleph_protocol::capability::{PtyInvokeParams, PtySpawnParams, PtyToolParams, UiInvokeParams};
use async_trait::async_trait;
use base64::Engine;
use schemars::schema_for;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex as StdMutex, OnceLock, Weak,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub const TERMINAL_ADAPTER_ID: &str = "aleph/tools/terminal_capability";
pub const UI_ADAPTER_ID: &str = "aleph/tools/ui_capability";

tokio::task_local! {
    static TERMINAL_SOURCE_CALL: crate::capability::policy_runtime::CapabilityCallContext;
}

/// The mounted PTY domain owner behind the pre-catalog terminal adapter.
///
/// This is an assembly handle only: the manager and ownership tree remain the
/// authorities for session state and admission. The source does not cache a
/// second session table or descriptor map.
pub struct TerminalCapabilitySource {
    specials: CapabilityHandlerRegistry,
    _policy: Arc<crate::capability::policy::PolicyEngine>,
    facade: OnceLock<Weak<crate::capability::zahir_facade::ZahirFacade>>,
    config: Arc<RwLock<crate::Config>>,
    provider: OnceLock<ProviderHandle>,
    #[cfg(test)]
    fail_after_binding: AtomicBool,
    #[cfg(test)]
    last_registration: StdMutex<Option<String>>,
}

impl TerminalCapabilitySource {
    #[must_use]
    pub fn new(
        specials: CapabilityHandlerRegistry,
        policy: Arc<crate::capability::policy::PolicyEngine>,
        facade: Weak<crate::capability::zahir_facade::ZahirFacade>,
        config: Arc<RwLock<crate::Config>>,
    ) -> Self {
        let facade_slot = OnceLock::new();
        let _ = facade_slot.set(facade);
        Self {
            specials,
            _policy: policy,
            facade: facade_slot,
            config,
            provider: OnceLock::new(),
            #[cfg(test)]
            fail_after_binding: AtomicBool::new(false),
            #[cfg(test)]
            last_registration: StdMutex::new(None),
        }
    }

    pub(crate) fn bind_facade(
        &self,
        facade: Weak<crate::capability::zahir_facade::ZahirFacade>,
    ) -> Result<(), CapabilityInvocationError> {
        self.facade
            .set(facade)
            .map_err(|_| CapabilityInvocationError::Unavailable)
    }

    fn facade(&self) -> Option<Arc<crate::capability::zahir_facade::ZahirFacade>> {
        self.facade.get().and_then(Weak::upgrade)
    }

    pub(crate) fn bind_manager_handle(
        &self,
        provider: ProviderHandle,
    ) -> Result<(), crate::gateway::handlers::pty::PtyCapabilityError> {
        if let Some(existing) = self.provider.get() {
            return if existing.same_instance(&provider) {
                Ok(())
            } else {
                Err(crate::gateway::handlers::pty::PtyCapabilityError::Unavailable)
            };
        }
        match self.provider.set(provider.clone()) {
            Ok(()) => Ok(()),
            Err(_) => self
                .provider
                .get()
                .filter(|existing| existing.same_instance(&provider))
                .map(|_| ())
                .ok_or(crate::gateway::handlers::pty::PtyCapabilityError::Unavailable),
        }
    }

    pub(crate) fn policy_snapshot(
        &self,
    ) -> Result<(ProviderHandle, u64, usize), CapabilityDispatchError> {
        let provider = self
            .provider
            .get()
            .ok_or(CapabilityDispatchError::Unavailable)?;
        let manager =
            crate::gateway::pty::try_manager().ok_or(CapabilityDispatchError::Unavailable)?;
        if !provider.same_instance(&manager.provider_handle()) {
            return Err(CapabilityDispatchError::Unavailable);
        }
        let generation = manager
            .ready_generation()
            .map_err(|_| CapabilityDispatchError::Unavailable)?;
        let occupied = manager
            .occupied_sessions_at(generation)
            .map_err(|_| CapabilityDispatchError::Unavailable)?;
        Ok((provider.clone(), generation, occupied))
    }

    pub fn bind_handler(self: &Arc<Self>) -> Result<(), CapabilityInvocationError> {
        let source: Arc<dyn CapabilityHandler> = self.clone();
        self.specials
            .terminal_source_slot()
            .bind(Arc::downgrade(&source))
            .map_err(Into::into)
    }

    pub(crate) fn register_session(
        self: &Arc<Self>,
        session_id: &str,
        created_by: Option<&str>,
    ) -> Result<(), crate::gateway::handlers::pty::PtyCapabilityError> {
        let provider = self
            .provider
            .get()
            .cloned()
            .ok_or(crate::gateway::handlers::pty::PtyCapabilityError::Unavailable)?;
        let schema = schema(CapabilityKind::Terminal);
        let id = CapabilityId {
            namespace: "aleph/terminal".into(),
            name: format!("session-{session_id}"),
        };
        let visibility = VisibilityScope {
            principal: created_by.map(str::to_owned),
            ..VisibilityScope::default()
        };
        let descriptor = descriptor_with_visibility(
            CapabilityKind::Terminal,
            &schema,
            id.clone(),
            LifetimeScope::Session,
            OwnerRef::Runtime,
            visibility.clone(),
        );
        self.specials
            .register(
                SpecialRegistration::new(descriptor, schema, self.clone(), provider)
                    .with_deferred_source_commit(TERMINAL_SOURCE_CALL.try_with(Clone::clone).ok()),
            )
            .map_err(|error| {
                crate::gateway::handlers::pty::PtyCapabilityError::Io(error.to_string())
            })?;
        let facade = match self.facade() {
            Some(facade) => facade,
            None => {
                let _ = self.specials.unregister(&id);
                return Err(crate::gateway::handlers::pty::PtyCapabilityError::Unavailable);
            }
        };
        let registration = match facade.bind_terminal_session(id.clone(), visibility) {
            Ok(registration) => registration,
            Err(error) => {
                let _ = self.specials.unregister(&id);
                return Err(crate::gateway::handlers::pty::PtyCapabilityError::Io(
                    format!("{error:?}"),
                ));
            }
        };
        #[cfg(test)]
        if self.fail_after_binding.swap(false, Ordering::SeqCst) {
            *self
                .last_registration
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(session_id.to_owned());
            if let Some(token) = registration.as_ref() {
                debug_assert!(facade.rollback_terminal_session(token));
            }
            let _ = self.specials.unregister(&id);
            return Err(crate::gateway::handlers::pty::PtyCapabilityError::Io(
                "injected post-publication registration failure".into(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_registration_after_binding(&self) {
        self.fail_after_binding.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn last_registration_id(&self) -> Option<String> {
        self.last_registration
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) fn commit_session<R>(
        &self,
        session_id: &str,
        commit: impl FnOnce() -> R,
    ) -> Result<(R, crate::capability::events::SourceObservationReceipt), String> {
        let id = CapabilityId {
            namespace: "aleph/terminal".into(),
            name: format!("session-{session_id}"),
        };
        self.facade()
            .ok_or_else(|| "terminal source unavailable".to_owned())?
            .commit_terminal_session(&id, commit)
            .map_err(|_| "terminal source publication stopped".to_owned())
    }

    pub(crate) fn withdraw_session_deferred(
        &self,
        session_id: &str,
    ) -> crate::capability::events::SourceObservationReceipt {
        let id = CapabilityId {
            namespace: "aleph/terminal".into(),
            name: format!("session-{session_id}"),
        };
        // A delayed callback from an old source cannot unregister a replacement
        // handler. The removed immutable record owns its already-captured fact key.
        let (removed, observation) = self
            .specials
            .unregister_owned_deferred(&id, self)
            .unwrap_or((false, None));
        if removed {
            if let Some(facade) = self.facade() {
                facade.withdraw_terminal_session(&id);
            }
        }
        crate::capability::events::SourceObservationReceipt::new(observation, None)
    }

    pub(crate) fn withdraw_session(&self, session_id: &str) {
        self.withdraw_session_deferred(session_id).flush();
    }

    pub(crate) async fn invoke_params(
        &self,
        params: aleph_protocol::capability::PtyInvokeParams,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        let owned = crate::gateway::handlers::pty::require_owned_session(&params.session_id)
            .map_err(|error| self.map_owned_error(error, &params.session_id))?;
        self.invoke_params_with_owner(params, cancel, owned).await
    }

    fn map_owned_error(
        &self,
        error: crate::gateway::handlers::pty::PtyCapabilityError,
        session_id: &str,
    ) -> CapabilityDispatchError {
        match error {
            crate::gateway::handlers::pty::PtyCapabilityError::NotFound => {
                CapabilityDispatchError::NotFound(CapabilityId {
                    namespace: "aleph/terminal".into(),
                    name: format!("session-{session_id}"),
                })
            }
            other => map_pty_error(other),
        }
    }

    pub(crate) async fn invoke_params_with_owner(
        &self,
        params: aleph_protocol::capability::PtyInvokeParams,
        cancel: CancellationToken,
        owned: crate::gateway::handlers::pty::OwnedPtySession,
    ) -> Result<Value, CapabilityDispatchError> {
        params
            .validate()
            .map_err(|error| CapabilityDispatchError::InvalidInput(error.into()))?;
        if owned.session_id() != params.session_id {
            return Err(CapabilityDispatchError::NotFound(CapabilityId {
                namespace: "aleph/terminal".into(),
                name: format!("session-{}", params.session_id),
            }));
        }
        if cancel.is_cancelled() {
            return Err(CapabilityDispatchError::Cancelled);
        }
        if !self.config.read().await.policies.terminal.enabled {
            return Err(CapabilityDispatchError::Unavailable);
        }
        let manager =
            crate::gateway::pty::try_manager().ok_or(CapabilityDispatchError::Unavailable)?;
        let session_id = owned.session_id();
        let result = match params.operation {
            aleph_protocol::capability::PtyOperation::Input { data, base64 } => {
                let bytes = if base64 {
                    base64::engine::general_purpose::STANDARD
                        .decode(data.as_bytes())
                        .map_err(|error| {
                            CapabilityDispatchError::InvalidInput(format!(
                                "invalid base64: {error}"
                            ))
                        })?
                } else {
                    data.into_bytes()
                };
                manager
                    .write_at(owned.generation(), session_id, &bytes)
                    .map(|()| json!({"ok": true}))
            }
            aleph_protocol::capability::PtyOperation::Attach => manager
                .attach_snapshot_at(owned.generation(), session_id)
                .and_then(|snapshot| {
                    serde_json::to_value(snapshot).map_err(|error| error.to_string())
                }),
            aleph_protocol::capability::PtyOperation::Resize { rows, cols } => {
                let connection = TERMINAL_SOURCE_CALL
                    .try_with(|call| call.connection_id().map(str::to_owned))
                    .ok()
                    .flatten();
                let applied = match connection {
                    Some(connection) => manager.note_viewport_at(
                        owned.generation(),
                        session_id,
                        &connection,
                        rows,
                        cols,
                    ),
                    None => {
                        // A model without a viewport cannot override attached clients.
                        let (rows, cols) =
                            manager.effective_size(session_id).unwrap_or((rows, cols));
                        manager.resize_at(owned.generation(), session_id, rows, cols)
                    }
                };
                applied.map(|()| json!({"ok": true}))
            }
            aleph_protocol::capability::PtyOperation::Close => manager
                .close_at(owned.generation(), session_id)
                .map(|()| json!({"ok": true})),
        };
        result.map_err(|error| CapabilityDispatchError::Execution(error.to_string()))
    }

    /// Route a legacy gateway face through the mounted facade. The optional
    /// capture is the handler's existing ownership gate; the facade then
    /// performs the authoritative lease/policy admission before effects.
    pub(crate) async fn invoke_legacy_gateway(
        &self,
        params: PtyToolParams,
        call_id: String,
        cancel: CancellationToken,
        owned: Option<crate::gateway::handlers::pty::OwnedPtySession>,
    ) -> Result<Value, CapabilityInvocationError> {
        if let Some(owned) = &owned {
            let session_id = match &params {
                PtyToolParams::Input { session_id, .. }
                | PtyToolParams::Attach { session_id }
                | PtyToolParams::Resize { session_id, .. }
                | PtyToolParams::Close { session_id } => Some(session_id),
                PtyToolParams::Spawn { .. } | PtyToolParams::List {} => None,
            };
            if let Some(session_id) = session_id {
                if session_id != owned.session_id() {
                    return Err(CapabilityInvocationError::Handler(
                        CapabilityDispatchError::NotFound(CapabilityId {
                            namespace: "aleph/terminal".into(),
                            name: format!("session-{session_id}"),
                        }),
                    ));
                }
            }
        }
        let facade = self
            .facade()
            .ok_or(CapabilityInvocationError::Unavailable)?;
        facade
            .invoke_terminal_tool_gateway(params, call_id, cancel, owned)
            .await
    }

    pub(crate) async fn invoke_tool_params(
        &self,
        params: PtyToolParams,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        match params {
            PtyToolParams::Spawn { params } => self.invoke_spawn(params, cancel).await,
            PtyToolParams::List {} => self.invoke_list().await,
            PtyToolParams::Input {
                session_id,
                data,
                base64,
            } => {
                self.invoke_params(
                    PtyInvokeParams {
                        session_id,
                        operation: aleph_protocol::capability::PtyOperation::Input { data, base64 },
                    },
                    cancel,
                )
                .await
            }
            PtyToolParams::Attach { session_id } => {
                self.invoke_params(
                    PtyInvokeParams {
                        session_id,
                        operation: aleph_protocol::capability::PtyOperation::Attach,
                    },
                    cancel,
                )
                .await
            }
            PtyToolParams::Resize {
                session_id,
                rows,
                cols,
            } => {
                self.invoke_params(
                    PtyInvokeParams {
                        session_id,
                        operation: aleph_protocol::capability::PtyOperation::Resize { rows, cols },
                    },
                    cancel,
                )
                .await
            }
            PtyToolParams::Close { session_id } => {
                self.invoke_params(
                    PtyInvokeParams {
                        session_id,
                        operation: aleph_protocol::capability::PtyOperation::Close,
                    },
                    cancel,
                )
                .await
            }
        }
    }

    async fn invoke_spawn(
        &self,
        params: PtySpawnParams,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        crate::gateway::handlers::pty::check_dimensions(params.rows, params.cols)
            .map_err(CapabilityDispatchError::InvalidInput)?;
        if cancel.is_cancelled() {
            return Err(CapabilityDispatchError::Cancelled);
        }
        let cfg = self.config.read().await;
        if !cfg.policies.terminal.enabled {
            return Err(CapabilityDispatchError::Unavailable);
        }
        let roots = crate::gateway::pty::workspace_roots(&cfg.agents.defaults);
        let terminal = cfg.policies.terminal.clone();
        let cwd = crate::gateway::pty::jail::resolve_spawn_cwd(params.cwd.as_deref(), &roots)
            .map_err(CapabilityDispatchError::InvalidInput)?;
        let generation = crate::gateway::pty::try_manager()
            .and_then(|manager| manager.ready_generation().ok())
            .ok_or(CapabilityDispatchError::Unavailable)?;
        let actor = super::special_dispatch::DOMAIN_TOOL_CALL
            .try_with(|call| call.owner_actor.clone())
            .unwrap_or_else(|_| crate::gateway::visibility::ambient_actor());
        let options = crate::gateway::pty::SpawnOptions {
            command: params.command,
            args: params.args,
            cwd: Some(crate::utils::paths::display_string(&cwd)),
            env: params.env.into_iter().collect(),
            rows: params.rows,
            cols: params.cols,
            scrollback_lines: Some(terminal.scrollback_lines as usize),
            created_by: actor,
        };
        let manager =
            crate::gateway::pty::try_manager().ok_or(CapabilityDispatchError::Unavailable)?;
        manager.set_max_sessions(terminal.max_sessions);
        drop(cfg);
        let result = manager
            .spawn_at(generation, &options)
            .map_err(CapabilityDispatchError::Execution)?;
        let snapshot = manager
            .attach_snapshot_at(generation, &result.session_id)
            .map_err(CapabilityDispatchError::Execution)?;
        serde_json::to_value(aleph_protocol::pty::PtySpawnResponse {
            session_id: result.session_id,
            shell: result.shell,
            seq: snapshot.seq,
            rows: snapshot.rows,
            cols: snapshot.cols,
        })
        .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))
    }

    async fn invoke_list(&self) -> Result<Value, CapabilityDispatchError> {
        let actor = super::special_dispatch::DOMAIN_TOOL_CALL
            .try_with(|call| call.owner_actor.clone())
            .unwrap_or_else(|_| crate::gateway::visibility::ambient_actor());
        let sessions = crate::gateway::pty::try_manager()
            .map(|manager| manager.list())
            .unwrap_or_default()
            .into_iter()
            .filter(|session| {
                crate::gateway::pty::owner_admits(session.created_by.as_deref(), actor.as_deref())
            })
            .map(|info| aleph_protocol::pty::PtySessionInfo::from(&info))
            .collect::<Vec<_>>();
        serde_json::to_value(aleph_protocol::pty::PtyListResponse { sessions })
            .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))
    }

    pub(crate) async fn invoke_tool(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let params: PtyToolParams =
            serde_json::from_value(input).map_err(|error| ToolError::ValidationFailed {
                name: "terminal_capability".into(),
                cause: error.to_string(),
            })?;
        let call = super::special_dispatch::DOMAIN_TOOL_CALL
            .try_with(Clone::clone)
            .map_err(|_| ToolError::Execution {
                name: "terminal_capability".into(),
                cause: "trusted canonical domain call is unavailable".into(),
            })?;
        let value = self
            .invoke_legacy_gateway(params, call.context.call_id().to_owned(), call.cancel, None)
            .await
            .map_err(|error| map_domain_tool_error("terminal_capability", error))?;
        Ok(ToolOutput {
            value,
            metadata: Default::default(),
        })
    }
}

fn map_domain_tool_error(name: &str, error: CapabilityInvocationError) -> ToolError {
    match error {
        CapabilityInvocationError::Policy(error) => ToolError::PermissionDenied {
            name: name.into(),
            reason: error.to_string(),
        },
        error => ToolError::Execution {
            name: name.into(),
            cause: error.to_string(),
        },
    }
}

fn map_pty_error(
    error: crate::gateway::handlers::pty::PtyCapabilityError,
) -> CapabilityDispatchError {
    match error {
        crate::gateway::handlers::pty::PtyCapabilityError::InvalidInput(error) => {
            CapabilityDispatchError::InvalidInput(error)
        }
        crate::gateway::handlers::pty::PtyCapabilityError::NotFound
        | crate::gateway::handlers::pty::PtyCapabilityError::Unavailable => {
            CapabilityDispatchError::Unavailable
        }
        crate::gateway::handlers::pty::PtyCapabilityError::Io(error) => {
            CapabilityDispatchError::Execution(error)
        }
    }
}

#[async_trait]
impl CapabilityHandler for TerminalCapabilitySource {
    async fn invoke(
        &self,
        input: Value,
        call: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        let owner = call.context().clone();
        let result = TERMINAL_SOURCE_CALL
            .scope(owner, async {
                match serde_json::from_value::<PtyToolParams>(input.clone()) {
                    Ok(params) => self.invoke_tool_params(params, cancel).await,
                    Err(_) => {
                        let params =
                            serde_json::from_value::<PtyInvokeParams>(input).map_err(|error| {
                                CapabilityDispatchError::InvalidInput(error.to_string())
                            })?;
                        self.invoke_params(params, cancel).await
                    }
                }
            })
            .await;
        drop(call);
        result
    }
}

fn descriptor_with_visibility(
    kind: CapabilityKind,
    schema: &Value,
    id: CapabilityId,
    lifetime: LifetimeScope,
    owner: OwnerRef,
    visibility: VisibilityScope,
) -> CapabilityDescriptor {
    let mut descriptor = descriptor(kind, schema, "aleph/tools/terminal_capability");
    descriptor.id = id;
    descriptor.lifetime = lifetime;
    descriptor.owner = owner;
    descriptor.visibility = visibility;
    descriptor
}

struct UnavailableHandler {
    source: Option<Arc<CapabilityHandlerSourceSlot>>,
}

#[async_trait]
impl CapabilityHandler for UnavailableHandler {
    async fn invoke(
        &self,
        input: Value,
        call: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        match &self.source {
            Some(source) => source.invoke(input, call, cancel).await,
            None => Err(CapabilityDispatchError::Unavailable),
        }
    }
}

/// Canonical projection only; the mounted typed seam owns admission and effects.
struct CanonicalToolAdapter {
    definition: ToolDefinition,
    placement: crate::tools::registry::DomainPolicyPlacement,
    terminal_source: Option<Arc<TerminalCapabilitySource>>,
    facade: Arc<OnceLock<Weak<super::zahir_facade::ZahirFacade>>>,
}

#[async_trait]
impl crate::tools::handlers::ToolHandler for CanonicalToolAdapter {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        use crate::tools::registry::DomainPolicyPlacement;
        match self.placement {
            DomainPolicyPlacement::Terminal => match &self.terminal_source {
                Some(source) => source.invoke_tool(input).await,
                None => Err(ToolError::Execution {
                    name: self.definition.name.clone(),
                    cause: "capability source unavailable".into(),
                }),
            },
            DomainPolicyPlacement::Ui => {
                let call = super::special_dispatch::DOMAIN_TOOL_CALL
                    .try_with(Clone::clone)
                    .map_err(|_| ToolError::Execution {
                        name: self.definition.name.clone(),
                        cause: "capability source unavailable".into(),
                    })?;
                let facade = self.facade.get().and_then(Weak::upgrade).ok_or_else(|| {
                    ToolError::Execution {
                        name: self.definition.name.clone(),
                        cause: "capability source unavailable".into(),
                    }
                })?;
                let params: UiInvokeParams =
                    serde_json::from_value(input).map_err(|error| ToolError::ValidationFailed {
                        name: self.definition.name.clone(),
                        cause: error.to_string(),
                    })?;
                facade
                    .invoke_ui_gateway(
                        params,
                        call.context,
                        super::special_dispatch::CapabilityInvocationOrigin::CanonicalIngress,
                        call.cancel,
                    )
                    .await
                    .map(|value| ToolOutput {
                        value,
                        metadata: Default::default(),
                    })
                    .map_err(|error| map_domain_tool_error(&self.definition.name, error))
            }
        }
    }

    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
}

fn id(value: &str) -> CapabilityId {
    let (namespace, name) = value
        .rsplit_once('/')
        .expect("canonical adapter ids contain a namespace and name");
    CapabilityId {
        namespace: namespace.to_owned(),
        name: name.to_owned(),
    }
}

fn schema(kind: CapabilityKind) -> Value {
    let schema = match kind {
        CapabilityKind::Terminal => schema_for!(PtyInvokeParams),
        CapabilityKind::Ui => schema_for!(UiInvokeParams),
        _ => unreachable!("only terminal and UI adapters are canonical"),
    };
    serde_json::to_value(schema).unwrap_or_else(|_| json!({"type": "object"}))
}

fn tool_schema(kind: CapabilityKind) -> Value {
    let schema = match kind {
        CapabilityKind::Terminal => schema_for!(PtyToolParams),
        CapabilityKind::Ui => schema_for!(UiInvokeParams),
        _ => unreachable!("only terminal and UI adapters are canonical"),
    };
    serde_json::to_value(schema).unwrap_or_else(|_| json!({"type": "object"}))
}

fn tool_definition(name: &str, schema: Value, requires_approval: bool) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: format!("Canonical {name} capability adapter"),
        input_schema: schema,
        source: ToolSource::Builtin,
        metadata: ToolDefinitionMetadata {
            idempotent: false,
            requires_approval,
            ..ToolDefinitionMetadata::default()
        },
    }
}

fn tool_descriptor(definition: &ToolDefinition) -> ToolCapabilityDescriptor {
    ToolCapabilityDescriptor {
        name: definition.name.clone(),
        kind: ToolKind::Tool,
        schema_version: crate::tools::descriptor::SCHEMA_VERSION,
        description: definition.description.clone(),
        input_schema: definition.input_schema.clone(),
        source: ToolSource::Builtin,
        services: Vec::new(),
        replay_policy: ReplayPolicy::Unsafe,
        requires_confirmation: definition.metadata.requires_approval,
        idempotent: false,
        concurrent_safe: false,
        max_duration_ms: None,
        revision: 0,
        implementation_contract: None,
    }
}

fn descriptor(kind: CapabilityKind, schema: &Value, value: &str) -> CapabilityDescriptor {
    let canonical = super::special::canonical_schema_bytes(schema);
    let mut fingerprint = [0_u8; 32];
    fingerprint.copy_from_slice(Sha256::digest(canonical).as_slice());
    CapabilityDescriptor {
        id: id(value),
        kind,
        schema: SchemaRef {
            version: 1,
            fingerprint,
        },
        revision: CapabilityRevision(0),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: Vec::new(),
        metadata: ProjectionMetadata::default(),
        facets: CapabilityFacets::new().with(super::descriptor::CapabilityFacet::Callable),
    }
}

/// The exact registration handles for one canonical adapter publication.
pub(crate) struct SpecialToolRegistrationBundle {
    pub(crate) ids: Vec<CapabilityId>,
    special: Vec<super::special::SpecialRegistrationHandle>,
    tools: Vec<RegistrationHandle>,
}

impl SpecialToolRegistrationBundle {
    pub(crate) fn dispose(self) {
        for handle in self.tools.into_iter().rev() {
            let _ = handle.dispose();
        }
        for handle in self.special.into_iter().rev() {
            let _ = handle.dispose();
        }
    }
}

/// Register both canonical adapter records in the sole special registry.
pub fn register_canonical_adapters(
    registry: &CapabilityHandlerRegistry,
) -> Result<Vec<CapabilityId>, CapabilityRegistryError> {
    register_canonical_adapters_with_tools_and_source(registry, None, None)
}

/// Register the canonical special records and their Tool/catalog projections as
/// one logical operation. If the Tool publication fails, exact handles dispose
/// only the incarnations created by this operation.
pub fn register_canonical_adapters_with_tools(
    registry: &CapabilityHandlerRegistry,
    tools: Option<&ToolHandlerRegistry>,
) -> Result<Vec<CapabilityId>, CapabilityRegistryError> {
    register_canonical_adapters_with_tools_and_source(registry, tools, None)
}

pub fn register_canonical_adapters_with_tools_and_source(
    registry: &CapabilityHandlerRegistry,
    tools: Option<&ToolHandlerRegistry>,
    terminal_source: Option<&Arc<TerminalCapabilitySource>>,
) -> Result<Vec<CapabilityId>, CapabilityRegistryError> {
    Ok(register_canonical_adapters_with_tools_bundle(registry, tools, terminal_source)?.ids)
}

pub(crate) fn register_canonical_adapters_with_tools_bundle(
    registry: &CapabilityHandlerRegistry,
    tools: Option<&ToolHandlerRegistry>,
    terminal_source: Option<&Arc<TerminalCapabilitySource>>,
) -> Result<SpecialToolRegistrationBundle, CapabilityRegistryError> {
    let provider = ProviderHandle::new(ProviderLifecycle::ready(
        ProviderId::new("canonical-special-adapters").expect("static provider id"),
        ProviderTransport::InProcess,
    ));
    let mut ids = Vec::new();
    let mut special = Vec::new();
    let mut tool_handles = Vec::new();
    for (kind, canonical_id) in [
        (CapabilityKind::Terminal, TERMINAL_ADAPTER_ID),
        (CapabilityKind::Ui, UI_ADAPTER_ID),
    ] {
        let input_schema = schema(kind);
        let handle = match registry.register_with_handle(SpecialRegistration::new(
            descriptor(kind, &input_schema, canonical_id),
            input_schema.clone(),
            Arc::new(UnavailableHandler {
                source: (kind == CapabilityKind::Terminal).then(|| registry.terminal_source_slot()),
            }),
            provider.clone(),
        )) {
            Ok(handle) => handle,
            Err(error) => {
                SpecialToolRegistrationBundle {
                    ids,
                    special,
                    tools: tool_handles,
                }
                .dispose();
                return Err(error);
            }
        };
        ids.push(handle.id().clone());
        special.push(handle);
        let Some(tools) = tools else { continue };
        let name = canonical_id.rsplit('/').next().unwrap_or(canonical_id);
        let placement = match kind {
            CapabilityKind::Terminal => crate::tools::registry::DomainPolicyPlacement::Terminal,
            CapabilityKind::Ui => crate::tools::registry::DomainPolicyPlacement::Ui,
            _ => unreachable!("only canonical domain adapters"),
        };
        let definition = tool_definition(name, tool_schema(kind), kind == CapabilityKind::Terminal);
        let tool_handle = match tools.register_domain_adapter(
            tool_descriptor(&definition),
            Arc::new(CanonicalToolAdapter {
                definition,
                placement,
                terminal_source: (kind == CapabilityKind::Terminal)
                    .then(|| terminal_source.cloned())
                    .flatten(),
                facade: registry.domain_facade_slot(),
            }),
            placement,
        ) {
            Ok(handle) => handle,
            Err(error) => {
                SpecialToolRegistrationBundle {
                    ids,
                    special,
                    tools: tool_handles,
                }
                .dispose();
                return Err(CapabilityRegistryError::ToolRegistration(error.to_string()));
            }
        };
        tool_handles.push(tool_handle);
    }
    Ok(SpecialToolRegistrationBundle {
        ids,
        special,
        tools: tool_handles,
    })
}
