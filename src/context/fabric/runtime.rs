//! Runtime facade for the context capability fabric.
//!
//! This module is the host policy seam: it admits a capability through the
//! registry, invokes the transport handler, revalidates the grant, and only
//! then commits a staged result. Native cheap passes, ingress, and compaction
//! remain the fallback paths owned by Core.

use super::{
    canonical_fingerprint, AuthenticatedCaller, ContextCapabilityDescriptor, ContextCapabilityMode,
    ContextCapabilityRequest, ContextContractError, ContextLifecycleEvent, ContextNamespace,
    ContextOperation, ContextPhase, ContextScope, ContextSnapshot, ContextTransformResult,
    DegradedReason, CONTEXT_CAPABILITY_API_VERSION,
};
use crate::context::budget::preflight::{
    NativePreflightRegistry, PreflightPipeline, PreflightStage,
};
use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::budget::ContextPressure;
use crate::context::compact::compactor::{CompactResult, CompactStrategy, ContextCompactor};
use crate::extension::registry::{ContextRegistryError, PluginRegistry};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// A transport or in-process plugin implementation of a context capability.
#[async_trait]
pub trait ContextHandler: Send + Sync {
    async fn invoke(
        &self,
        request: ContextCapabilityRequest,
    ) -> Result<ContextTransformResult, String>;

    /// The caller is supplied by the authenticated host, outside the wire DTO.
    /// Native handlers keep their existing API; transports may require this seam.
    async fn invoke_authenticated(
        &self,
        request: ContextCapabilityRequest,
        _caller: &AuthenticatedCaller,
    ) -> Result<ContextTransformResult, String> {
        self.invoke(request).await
    }
}

/// Who supplied the committed result. Task 5 can persist this owner beside
/// its fold/event without inspecting transport internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextOwner {
    Builtin,
    Plugin(String),
    BuiltinFallback {
        plugin: String,
        reason: DegradedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightOutcome {
    pub tokens_freed: usize,
    pub owner: ContextOwner,
    pub degraded: Option<DegradedReason>,
    pub skipped_observers: Vec<String>,
}

#[derive(Debug)]
pub struct CompactionOutcome {
    pub result: CompactResult,
    pub owner: ContextOwner,
    pub degraded: Option<DegradedReason>,
    pub skipped_observers: Vec<String>,
}

#[derive(Debug)]
pub struct AdmissionOutcome {
    pub result: crate::tools::result_processing::ProcessedResult,
    pub owner: ContextOwner,
    pub degraded: Option<DegradedReason>,
}

type HandlerResolver = Arc<dyn Fn(&str) -> Option<Arc<dyn ContextHandler>> + Send + Sync>;

/// The host-side context facade.
pub struct ContextFabric {
    registry: Arc<RwLock<PluginRegistry>>,
    preflight: PreflightPipeline,
    compactor: Option<Arc<ContextCompactor>>,
    handlers: HashMap<String, Arc<dyn ContextHandler>>,
    handler_resolver: Option<HandlerResolver>,
    timeout: Duration,
}

/// Builder for one run's context facade. The registry remains the only source
/// of descriptors and grants; handlers are implementations keyed by the
/// registry's namespaced component key.
pub struct ContextFabricBuilder {
    registry: Arc<RwLock<PluginRegistry>>,
    native_preflight: NativePreflightRegistry,
    native_pipeline: Option<PreflightPipeline>,
    compactor: Option<Arc<ContextCompactor>>,
    handlers: HashMap<String, Arc<dyn ContextHandler>>,
    handler_resolver: Option<HandlerResolver>,
    timeout: Duration,
}

impl ContextFabricBuilder {
    #[must_use]
    pub fn new(registry: Arc<RwLock<PluginRegistry>>) -> Self {
        Self {
            registry,
            native_preflight: NativePreflightRegistry::new(),
            native_pipeline: None,
            compactor: None,
            handlers: HashMap::new(),
            handler_resolver: None,
            timeout: Duration::from_secs(15),
        }
    }

    #[must_use]
    pub fn native_preflight_stage(mut self, stage: impl PreflightStage + 'static) -> Self {
        self.native_preflight.register(stage);
        self
    }

    #[must_use]
    pub fn native_preflight_registry(mut self, registry: NativePreflightRegistry) -> Self {
        self.native_preflight = registry;
        self
    }

    /// Supply the already-configured native pipeline, for example the output
    /// of `default_pipeline`. This preserves its pressure floor and cache
    /// stability policy instead of rebuilding an unconfigured registry.
    #[must_use]
    pub fn native_pipeline(mut self, pipeline: PreflightPipeline) -> Self {
        self.native_pipeline = Some(pipeline);
        self
    }

    #[must_use]
    pub fn compactor(mut self, compactor: Arc<ContextCompactor>) -> Self {
        self.compactor = Some(compactor);
        self
    }

    #[must_use]
    pub fn handler(mut self, key: impl Into<String>, handler: Arc<dyn ContextHandler>) -> Self {
        self.handlers.insert(key.into(), handler);
        self
    }

    /// Resolve handlers at invocation time so plugin unload/remount is visible
    /// without rebuilding this facade. The registry grant is still checked
    /// independently before and after every await.
    #[must_use]
    pub fn handler_resolver(mut self, resolver: HandlerResolver) -> Self {
        self.handler_resolver = Some(resolver);
        self
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn build(self) -> ContextFabric {
        ContextFabric {
            registry: self.registry,
            preflight: self
                .native_pipeline
                .unwrap_or_else(|| self.native_preflight.into_pipeline()),
            compactor: self.compactor,
            handlers: self.handlers,
            handler_resolver: self.handler_resolver,
            timeout: self.timeout,
        }
    }
}

impl ContextFabric {
    #[must_use]
    pub fn builder(registry: Arc<RwLock<PluginRegistry>>) -> ContextFabricBuilder {
        ContextFabricBuilder::new(registry)
    }

    /// Deliver bounded, read-only notifications through the existing transport.
    /// A notification carries only its event tag. Observer output is validated
    /// and discarded; delivery failure cannot change committed Core state.
    pub async fn notify_lifecycle(
        &self,
        event: ContextLifecycleEvent,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
    ) {
        self.notify_lifecycle_selected(event, caller, cancel, None)
            .await;
    }

    pub(crate) async fn notify_lifecycle_selected(
        &self,
        event: ContextLifecycleEvent,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
        selected_keys: Option<&[String]>,
    ) {
        if check_cancel(cancel).is_err() {
            return;
        }
        let Ok(entries) = self
            .phase_entries(ContextPhase::Lifecycle, caller.session_id())
            .await
        else {
            return;
        };
        let deliveries =
            futures::stream::iter(entries.into_iter().filter(|(key, descriptor, _)| {
                descriptor.lifecycle.contains(&event)
                    && selected_keys.is_none_or(|keys| keys.contains(key))
            }))
            .for_each_concurrent(4, |(key, descriptor, _)| async move {
                let Some(handler) = self.resolve_handler(&key) else {
                    return;
                };
                let payload = serde_json::json!({"event": event});
                let Ok(request) = make_payload_request(
                    &key,
                    &descriptor,
                    &payload,
                    ContextPhase::Lifecycle,
                    caller,
                ) else {
                    return;
                };
                let _ = self
                    .invoke_request(&key, &descriptor, request, caller, handler, cancel)
                    .await;
            });
        // One deadline bounds the whole observer batch, including a slow guest.
        let _ = tokio::time::timeout(self.timeout, deliveries).await;
    }

    /// Run one native candidate and the eligible external preflight owners.
    /// Replace/bundle owners get the first chance; Core runs native work only
    /// when no usable owner exists or a bounded external call falls back.
    pub async fn preflight(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail: usize,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
    ) -> Result<PreflightOutcome, ContextFabricError> {
        check_cancel(cancel)?;
        self.notify_lifecycle(ContextLifecycleEvent::BeforePreflight, caller, cancel)
            .await;
        check_cancel(cancel)?;
        let entries = self
            .phase_entries(ContextPhase::Preflight, caller.session_id())
            .await?;
        let skipped_observers = entries
            .iter()
            .filter(|(_, _, mode)| *mode == ContextCapabilityMode::Observer)
            .map(|(key, _, _)| key.clone())
            .collect::<Vec<_>>();
        let mut degraded = None;
        let original = messages.clone();
        let mut current = original.clone();
        let mut committed_owner = None;

        let replace_owner = entries.iter().find_map(|(key, descriptor, mode)| {
            if matches!(
                mode,
                ContextCapabilityMode::Replace | ContextCapabilityMode::Bundle
            ) {
                self.resolve_handler(key)
                    .map(|handler| (key.clone(), descriptor.clone(), handler))
            } else {
                None
            }
        });
        if let Some((key, descriptor, handler)) = replace_owner.as_ref() {
            match self
                .invoke_messages(
                    key,
                    descriptor,
                    &current,
                    fresh_tail,
                    0,
                    ContextPhase::Preflight,
                    caller,
                    handler.clone(),
                    cancel,
                    Some(pressure),
                )
                .await
            {
                Ok(candidate) => {
                    current = candidate;
                    committed_owner = Some(ContextOwner::Plugin(key.clone()));
                }
                Err(error) if error.is_cancelled() => return Err(error),
                Err(error) => degraded = Some(error.degraded_reason()),
            }
        }

        check_cancel(cancel)?;
        if committed_owner.is_none() {
            self.run_native(
                cancel,
                self.preflight.run(&mut current, pressure, fresh_tail),
            )
            .await?;
        }
        let mut owner = committed_owner.unwrap_or_else(|| {
            replace_owner
                .as_ref()
                .map(|(key, _, _)| ContextOwner::BuiltinFallback {
                    plugin: key.clone(),
                    reason: degraded.unwrap_or(DegradedReason::Unavailable),
                })
                .unwrap_or(ContextOwner::Builtin)
        });

        for (key, descriptor, _) in entries.iter().filter(|(_, _, mode)| {
            matches!(
                mode,
                ContextCapabilityMode::Compose | ContextCapabilityMode::Interceptor
            )
        }) {
            let Some(handler) = self.resolve_handler(key) else {
                degraded = Some(DegradedReason::Unavailable);
                continue;
            };
            match self
                .invoke_messages(
                    key,
                    descriptor,
                    &current,
                    fresh_tail,
                    0,
                    ContextPhase::Preflight,
                    caller,
                    handler,
                    cancel,
                    Some(pressure),
                )
                .await
            {
                Ok(candidate) => {
                    current = candidate;
                    owner = ContextOwner::Plugin(key.clone());
                }
                Err(error) if error.is_cancelled() => return Err(error),
                Err(error) => degraded = Some(error.degraded_reason()),
            }
        }
        check_cancel(cancel)?;
        let tokens_freed = freed_tokens(&original, &current);
        *messages = current;
        self.notify_lifecycle(ContextLifecycleEvent::AfterPreflight, caller, cancel)
            .await;
        Ok(PreflightOutcome {
            tokens_freed,
            owner,
            degraded,
            skipped_observers,
        })
    }

    /// Resolve the session's admitted output transformer in stable registry order.
    pub(crate) async fn admission_key(
        &self,
        caller: &AuthenticatedCaller,
    ) -> Result<Option<String>, ContextFabricError> {
        let entries = self
            .phase_entries(ContextPhase::Admission, caller.session_id())
            .await?;
        Ok(entries
            .iter()
            .find(|(_, _, mode)| {
                matches!(
                    mode,
                    ContextCapabilityMode::Replace | ContextCapabilityMode::Bundle
                )
            })
            .or_else(|| {
                entries.iter().find(|(_, _, mode)| {
                    matches!(
                        mode,
                        ContextCapabilityMode::Compose | ContextCapabilityMode::Interceptor
                    )
                })
            })
            .map(|(key, _, _)| key.clone()))
    }

    /// Existing synchronous native admission path. It performs no registry
    /// lookup, projection, serialization, or plugin hash work.
    pub fn admit_tool_output(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        value: &mut Value,
        budget: Option<usize>,
        store: Option<&crate::tools::result_store::ToolResultStore>,
        recovery: crate::tools::result_processing::RecoveryTools,
    ) -> crate::tools::result_processing::ProcessedResult {
        let outcome = crate::tool_output::ingress::clean_for_ingress_of(
            tool_name,
            value,
            budget,
            Some(tool_call_id),
        );
        crate::tools::result_processing::apply_result_budget(
            tool_call_id,
            tool_name,
            &outcome.model_facing,
            store,
            budget,
            outcome.reduced_from.as_deref(),
            recovery,
        )
    }

    /// Context-aware admission. The native path is selected immediately when
    /// no eligible external admission handler exists, preserving the hot path.
    pub async fn admit_tool_output_with_context(
        &self,
        capability_key: &str,
        tool_name: &str,
        tool_call_id: &str,
        value: &mut Value,
        budget: Option<usize>,
        store: Option<&crate::tools::result_store::ToolResultStore>,
        recovery: crate::tools::result_processing::RecoveryTools,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
    ) -> Result<AdmissionOutcome, ContextFabricError> {
        check_cancel(cancel)?;
        if budget.is_none() {
            return Ok(AdmissionOutcome {
                result: self.admit_tool_output(
                    tool_name,
                    tool_call_id,
                    value,
                    budget,
                    store,
                    recovery,
                ),
                owner: ContextOwner::Builtin,
                degraded: None,
            });
        }
        let Some((descriptor, mode)) = self
            .descriptor_for(capability_key, caller.session_id())
            .await?
        else {
            return Ok(AdmissionOutcome {
                result: self.admit_tool_output(
                    tool_name,
                    tool_call_id,
                    value,
                    budget,
                    store,
                    recovery,
                ),
                owner: ContextOwner::Builtin,
                degraded: None,
            });
        };
        if !descriptor.declares(ContextPhase::Admission)
            || matches!(
                mode,
                ContextCapabilityMode::Builtin | ContextCapabilityMode::Observer
            )
        {
            return Ok(AdmissionOutcome {
                result: self.admit_tool_output(
                    tool_name,
                    tool_call_id,
                    value,
                    budget,
                    store,
                    recovery,
                ),
                owner: ContextOwner::Builtin,
                degraded: None,
            });
        }
        let Some(handler) = self.resolve_handler(capability_key) else {
            return Ok(AdmissionOutcome {
                result: self.admit_tool_output(
                    tool_name,
                    tool_call_id,
                    value,
                    budget,
                    store,
                    recovery,
                ),
                owner: ContextOwner::BuiltinFallback {
                    plugin: capability_key.to_string(),
                    reason: DegradedReason::Unavailable,
                },
                degraded: Some(DegradedReason::Unavailable),
            });
        };
        let original = value.clone();
        let plugin_input = if matches!(
            mode,
            ContextCapabilityMode::Compose | ContextCapabilityMode::Interceptor
        ) {
            let mut native = original.clone();
            Value::String(
                crate::tool_output::ingress::clean_for_ingress_of(
                    tool_name,
                    &mut native,
                    budget,
                    Some(tool_call_id),
                )
                .model_facing,
            )
        } else {
            original.clone()
        };
        let request = match make_payload_request(
            capability_key,
            &descriptor,
            &plugin_input,
            ContextPhase::Admission,
            caller,
        ) {
            Ok(request) => request,
            Err(error) => {
                return Ok(AdmissionOutcome {
                    result: self.admit_tool_output(
                        tool_name,
                        tool_call_id,
                        value,
                        budget,
                        store,
                        recovery,
                    ),
                    owner: ContextOwner::BuiltinFallback {
                        plugin: capability_key.into(),
                        reason: error.degraded_reason(),
                    },
                    degraded: Some(error.degraded_reason()),
                })
            }
        };
        let response = match self
            .invoke_request(
                capability_key,
                &descriptor,
                request,
                caller,
                handler,
                cancel,
            )
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_cancelled() => return Err(error),
            Err(error) => {
                let reason = error.degraded_reason();
                return Ok(AdmissionOutcome {
                    result: self.admit_tool_output(
                        tool_name,
                        tool_call_id,
                        value,
                        budget,
                        store,
                        recovery,
                    ),
                    owner: ContextOwner::BuiltinFallback {
                        plugin: capability_key.to_string(),
                        reason,
                    },
                    degraded: Some(reason),
                });
            }
        };
        let mut candidate = response.output;
        let mut committed_key = capability_key.to_string();
        for (key, descriptor, mode) in self
            .phase_entries(ContextPhase::Admission, caller.session_id())
            .await?
        {
            if key == capability_key
                || !matches!(
                    mode,
                    ContextCapabilityMode::Compose | ContextCapabilityMode::Interceptor
                )
            {
                continue;
            }
            let Some(handler) = self.resolve_handler(&key) else {
                continue;
            };
            let request = make_payload_request(
                &key,
                &descriptor,
                &candidate,
                ContextPhase::Admission,
                caller,
            )?;
            match self
                .invoke_request(&key, &descriptor, request, caller, handler, cancel)
                .await
            {
                Ok(response) => {
                    candidate = response.output;
                    committed_key = key;
                }
                Err(ContextFabricError::Cancelled) => return Err(ContextFabricError::Cancelled),
                Err(_) => {}
            }
        }
        check_cancel(cancel)?;
        let original_text = serde_json::to_string(&original)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
        let output_changed = candidate != original;
        let outcome = crate::tool_output::ingress::clean_for_ingress_of(
            tool_name,
            &mut candidate,
            budget,
            Some(tool_call_id),
        );
        *value = candidate;
        let reduced_from = if output_changed {
            Some(original_text.as_str())
        } else {
            outcome.reduced_from.as_deref()
        };
        let result = crate::tools::result_processing::apply_result_budget(
            tool_call_id,
            tool_name,
            &outcome.model_facing,
            store,
            budget,
            reduced_from,
            recovery,
        );
        Ok(AdmissionOutcome {
            result,
            owner: ContextOwner::Plugin(committed_key),
            degraded: None,
        })
    }

    /// Alias kept explicit for adapters that model this as an async admission.
    pub async fn admit_tool_output_async(
        &self,
        capability_key: &str,
        tool_name: &str,
        tool_call_id: &str,
        value: &mut Value,
        budget: Option<usize>,
        store: Option<&crate::tools::result_store::ToolResultStore>,
        recovery: crate::tools::result_processing::RecoveryTools,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
    ) -> Result<AdmissionOutcome, ContextFabricError> {
        self.admit_tool_output_with_context(
            capability_key,
            tool_name,
            tool_call_id,
            value,
            budget,
            store,
            recovery,
            caller,
            cancel,
        )
        .await
    }

    /// Run the existing compactor only when no external replace/bundle owner
    /// can commit. Compose owners then run in stable registry order.
    pub async fn compact(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        fresh_tail: usize,
        transient_tail: usize,
        session_id: Option<&str>,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
    ) -> Result<CompactionOutcome, ContextFabricError> {
        check_cancel(cancel)?;
        self.notify_lifecycle(ContextLifecycleEvent::BeforeCompaction, caller, cancel)
            .await;
        check_cancel(cancel)?;
        let original = messages.clone();
        let entries = self
            .phase_entries(ContextPhase::Compaction, caller.session_id())
            .await?;
        let skipped_observers = entries
            .iter()
            .filter(|(_, _, mode)| *mode == ContextCapabilityMode::Observer)
            .map(|(key, _, _)| key.clone())
            .collect::<Vec<_>>();
        let mut degraded = None;
        let mut current = original.clone();
        let mut strategy = None;
        let mut selected_owner = None;

        for (key, descriptor, _) in entries.iter().filter(|(_, _, mode)| {
            matches!(
                mode,
                ContextCapabilityMode::Replace | ContextCapabilityMode::Bundle
            )
        }) {
            let Some(handler) = self.resolve_handler(key) else {
                degraded = Some(DegradedReason::Unavailable);
                continue;
            };
            match self
                .invoke_messages(
                    key,
                    descriptor,
                    &original,
                    fresh_tail,
                    transient_tail,
                    ContextPhase::Compaction,
                    caller,
                    handler,
                    cancel,
                    None,
                )
                .await
            {
                Ok(candidate) => {
                    current = candidate;
                    strategy = Some(CompactStrategy::Plugin {
                        capability: key.clone(),
                    });
                    selected_owner = Some(ContextOwner::Plugin(key.clone()));
                    break;
                }
                Err(error) if error.is_cancelled() => return Err(error),
                Err(error) => degraded = Some(error.degraded_reason()),
            }
        }

        check_cancel(cancel)?;
        if strategy.is_none() {
            let compactor = self
                .compactor
                .as_ref()
                .ok_or(ContextFabricError::BuiltinUnavailable("compactor"))?;
            let native_result = self
                .run_native(
                    cancel,
                    compactor.compact(&mut current, fresh_tail, transient_tail, session_id),
                )
                .await?
                .map_err(|error| ContextFabricError::Compaction(error.to_string()))?;
            strategy = Some(native_result.strategy_used);
        }
        let mut owner = selected_owner.unwrap_or_else(|| {
            if degraded.is_some() {
                ContextOwner::BuiltinFallback {
                    plugin: entries
                        .iter()
                        .find(|(_, _, mode)| {
                            matches!(
                                mode,
                                ContextCapabilityMode::Replace | ContextCapabilityMode::Bundle
                            )
                        })
                        .map(|(key, _, _)| key.clone())
                        .unwrap_or_default(),
                    reason: degraded.unwrap_or(DegradedReason::Unavailable),
                }
            } else {
                ContextOwner::Builtin
            }
        });
        for (key, descriptor, _) in entries.iter().filter(|(_, _, mode)| {
            matches!(
                mode,
                ContextCapabilityMode::Compose | ContextCapabilityMode::Interceptor
            )
        }) {
            let Some(handler) = self.resolve_handler(key) else {
                degraded = Some(DegradedReason::Unavailable);
                continue;
            };
            match self
                .invoke_messages(
                    key,
                    descriptor,
                    &current,
                    fresh_tail,
                    transient_tail,
                    ContextPhase::Compaction,
                    caller,
                    handler,
                    cancel,
                    None,
                )
                .await
            {
                Ok(candidate) => {
                    current = candidate;
                    owner = ContextOwner::Plugin(key.clone());
                }
                Err(error) if error.is_cancelled() => return Err(error),
                Err(error) => degraded = Some(error.degraded_reason()),
            }
        }
        check_cancel(cancel)?;
        *messages = current;
        let strategy = if let ContextOwner::Plugin(capability) = &owner {
            CompactStrategy::Plugin {
                capability: capability.clone(),
            }
        } else {
            strategy.expect("base strategy is selected before composition")
        };
        self.notify_lifecycle(ContextLifecycleEvent::AfterCompaction, caller, cancel)
            .await;
        Ok(CompactionOutcome {
            result: measured_compact_result(&original, messages, strategy),
            owner,
            degraded,
            skipped_observers,
        })
    }

    /// Summarize an event-log slice without changing manual compaction's cut,
    /// focus, user instructions, savings gate or atomic store transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn summarize_slice(
        &self,
        messages: &[UnifiedMessage],
        focus: Option<&str>,
        instructions: Option<&str>,
        native: Option<&ContextCompactor>,
        caller: &AuthenticatedCaller,
        cancel: Option<&CancellationToken>,
    ) -> Result<(String, ContextOwner), ContextFabricError> {
        check_cancel(cancel)?;
        self.notify_lifecycle(ContextLifecycleEvent::BeforeCompaction, caller, cancel)
            .await;
        check_cancel(cancel)?;
        let entries = self
            .phase_entries(ContextPhase::Compaction, caller.session_id())
            .await?;
        let mut summary: Option<String> = None;
        let mut owner = ContextOwner::Builtin;
        let mut failed_owner = None;
        for (key, descriptor, mode) in &entries {
            if !matches!(
                mode,
                ContextCapabilityMode::Replace | ContextCapabilityMode::Bundle
            ) {
                continue;
            }
            let Some(handler) = self.resolve_handler(key) else {
                failed_owner = Some(key.clone());
                continue;
            };
            let payload = serde_json::json!({
                "messages": safe_project(messages), "focus": focus,
                "instructions": instructions, "summary": null,
            });
            let response = match make_payload_request(
                key,
                descriptor,
                &payload,
                ContextPhase::Compaction,
                caller,
            ) {
                Ok(request) => {
                    self.invoke_request(key, descriptor, request, caller, handler, cancel)
                        .await
                }
                Err(error) => Err(error),
            };
            match response {
                Ok(result)
                    if result
                        .output
                        .as_str()
                        .is_some_and(|body| !body.trim().is_empty()) =>
                {
                    summary = result.output.as_str().map(str::to_owned);
                    owner = ContextOwner::Plugin(key.clone());
                    break;
                }
                Err(ContextFabricError::Cancelled) => return Err(ContextFabricError::Cancelled),
                _ => failed_owner = Some(key.clone()),
            }
        }
        if summary.is_none() {
            summary = Some(match native {
                Some(compactor) => self
                    .run_native(
                        cancel,
                        compactor.summarize_slice(messages, focus, instructions),
                    )
                    .await?
                    .map_err(|error| ContextFabricError::Compaction(error.to_string()))?,
                None => crate::context::compact::compactor::deterministic_truncation(messages),
            });
            if let Some(plugin) = failed_owner {
                owner = ContextOwner::BuiltinFallback {
                    plugin,
                    reason: DegradedReason::Unavailable,
                };
            }
        }
        let mut body = summary.expect("base summary selected");
        for (key, descriptor, mode) in &entries {
            if !matches!(
                mode,
                ContextCapabilityMode::Compose | ContextCapabilityMode::Interceptor
            ) {
                continue;
            }
            let Some(handler) = self.resolve_handler(key) else {
                continue;
            };
            let payload = serde_json::json!({
                "messages": safe_project(messages), "focus": focus,
                "instructions": instructions, "summary": body,
            });
            let response = match make_payload_request(
                key,
                descriptor,
                &payload,
                ContextPhase::Compaction,
                caller,
            ) {
                Ok(request) => {
                    self.invoke_request(key, descriptor, request, caller, handler, cancel)
                        .await
                }
                Err(error) => Err(error),
            };
            match response {
                Ok(result) => {
                    if let Some(candidate) = result.output.as_str().filter(|s| !s.trim().is_empty())
                    {
                        body = candidate.to_owned();
                        owner = ContextOwner::Plugin(key.clone());
                    }
                }
                Err(ContextFabricError::Cancelled) => return Err(ContextFabricError::Cancelled),
                Err(_) => {}
            }
        }
        check_cancel(cancel)?;
        self.notify_lifecycle(ContextLifecycleEvent::AfterCompaction, caller, cancel)
            .await;
        Ok((body, owner))
    }

    fn resolve_handler(&self, key: &str) -> Option<Arc<dyn ContextHandler>> {
        self.handler_resolver
            .as_ref()
            .and_then(|resolver| resolver(key))
            .or_else(|| self.handlers.get(key).cloned())
    }

    async fn eligible_keys(&self, session: &str) -> Result<Vec<String>, ContextFabricError> {
        let registry = self.registry.read().await;
        Ok(registry.eligible_context_keys(session, now_seconds()))
    }

    async fn phase_entries(
        &self,
        phase: ContextPhase,
        session: &str,
    ) -> Result<Vec<(String, ContextCapabilityDescriptor, ContextCapabilityMode)>, ContextFabricError>
    {
        let keys = self.eligible_keys(session).await?;
        let mut entries = Vec::new();
        for key in keys {
            if let Some((descriptor, mode)) = self.descriptor_for(&key, session).await? {
                if descriptor.declares(phase) {
                    entries.push((key, descriptor, mode));
                }
            }
        }
        Ok(entries)
    }

    async fn descriptor_for(
        &self,
        key: &str,
        session: &str,
    ) -> Result<Option<(ContextCapabilityDescriptor, ContextCapabilityMode)>, ContextFabricError>
    {
        let registry = self.registry.read().await;
        let Some(registration) = registry.resolve_context(key) else {
            return Ok(None);
        };
        if registration.grant_for_session(session).is_none() {
            return Ok(None);
        }
        Ok(Some((
            registration.descriptor.clone(),
            registration.owner_mode,
        )))
    }

    async fn invoke_messages(
        &self,
        key: &str,
        descriptor: &ContextCapabilityDescriptor,
        messages: &[UnifiedMessage],
        fresh_tail: usize,
        transient_tail: usize,
        phase: ContextPhase,
        caller: &AuthenticatedCaller,
        handler: Arc<dyn ContextHandler>,
        cancel: Option<&CancellationToken>,
        pressure: Option<&ContextPressure>,
    ) -> Result<Vec<UnifiedMessage>, ContextFabricError> {
        // Keep source coordinates separate from projection: reasoning removal can
        // delete entire messages, so projected indices cannot address native data.
        let window = if phase == ContextPhase::Preflight {
            0..self.preflight.external_cut(
                messages,
                pressure.ok_or(ContextFabricError::PreflightGated)?,
                fresh_tail,
            )
        } else {
            self.compactor
                .as_ref()
                .ok_or(ContextFabricError::BuiltinUnavailable("compactor"))?
                .compression_window(messages, fresh_tail, transient_tail)
                .ok_or(ContextFabricError::PreflightGated)?
        };
        if window.is_empty() {
            return Err(ContextFabricError::PreflightGated);
        }
        let projected = safe_project(&messages[window.clone()]);
        let public_original = serde_json::to_value(&projected)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
        let request = make_request_from_projected(key, descriptor, projected, phase, caller)?;
        let response = self
            .invoke_request(key, descriptor, request, caller, handler, cancel)
            .await?;
        let mut candidate: Vec<UnifiedMessage> = serde_json::from_value(response.output)
            .map_err(|e| ContextFabricError::MalformedOutput(e.to_string()))?;
        for message in &candidate {
            for block in message.content_blocks() {
                if matches!(
                    block,
                    ContentBlock::Thinking { .. }
                        | ContentBlock::ToolCall {
                            thought_signature: Some(_),
                            ..
                        }
                ) {
                    return Err(ContextFabricError::MalformedOutput(
                        "plugin inserted private protocol metadata".into(),
                    ));
                }
            }
        }
        let original_value = serde_json::to_value(&candidate)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
        crate::providers::message::normalize_tool_pairs(&mut candidate);
        let normalized_candidate = serde_json::to_value(&candidate)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
        if normalized_candidate != original_value {
            return Err(ContextFabricError::MalformedOutput(
                "tool-call/result pairing changed".into(),
            ));
        }
        if normalized_candidate == public_original {
            // Projection may have removed whole private-only messages. Preserve
            // the complete native frame when the plugin kept its public view.
            candidate = messages[window.clone()].to_vec();
        } else {
            super::protocol_frames::restore_private_frames(
                &messages[window.clone()],
                &mut candidate,
            )?;
        }
        let mut staged = messages[..window.start].to_vec();
        staged.extend(candidate);
        staged.extend_from_slice(&messages[window.end..]);
        let encoded = serde_json::to_value(&staged)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
        let mut paired = staged.clone();
        crate::providers::message::normalize_tool_pairs(&mut paired);
        if serde_json::to_value(&paired)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?
            != encoded
        {
            return Err(ContextFabricError::MalformedOutput(
                "stitched history contains orphaned tool messages".into(),
            ));
        }
        check_cancel(cancel)?;
        if let Some(pressure) = pressure {
            self.preflight
                .accept_external_candidate(messages, staged, pressure)
                .ok_or(ContextFabricError::PreflightGated)
        } else {
            Ok(staged)
        }
    }

    async fn invoke_request(
        &self,
        key: &str,
        descriptor: &ContextCapabilityDescriptor,
        request: ContextCapabilityRequest,
        caller: &AuthenticatedCaller,
        handler: Arc<dyn ContextHandler>,
        cancel: Option<&CancellationToken>,
    ) -> Result<ContextTransformResult, ContextFabricError> {
        let admitted_descriptor = self
            .validate_grant(key, descriptor, &request, caller)
            .await?;
        if request.snapshot.as_ref().is_some_and(|snapshot| {
            super::privacy::contains_recognised_credential(&snapshot.payload)
        }) {
            return Err(ContextFabricError::SensitiveInput);
        }
        let response = self
            .run_with_cancel(
                cancel,
                handler.invoke_authenticated(request.clone(), caller),
            )
            .await?
            .map_err(ContextFabricError::Handler)?;
        let post_descriptor = self
            .validate_grant(key, descriptor, &request, caller)
            .await?;
        if post_descriptor != admitted_descriptor || post_descriptor != *descriptor {
            return Err(ContextFabricError::Registry(
                ContextRegistryError::DescriptorMismatch(key.to_string()),
            ));
        }
        response
            .validate_for(
                &request
                    .admit(&post_descriptor, caller)
                    .map_err(ContextFabricError::Contract)?,
            )
            .map_err(ContextFabricError::Contract)?;
        let serialized = serde_json::to_vec(&response.output)
            .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
        let measured_tokens =
            estimate_tokens_smart(std::str::from_utf8(&serialized).unwrap_or_default()) as u64;
        if response.output_bytes != serialized.len() as u64
            || response.output_tokens != measured_tokens
        {
            return Err(ContextFabricError::MalformedOutput(
                "result measurements do not match serialized output".into(),
            ));
        }
        if response.output_bytes > post_descriptor.output_budget.max_bytes
            || response.output_tokens > post_descriptor.output_budget.max_tokens
        {
            return Err(ContextFabricError::OutputExceedsBudget);
        }
        Ok(response)
    }

    async fn validate_grant(
        &self,
        key: &str,
        descriptor: &ContextCapabilityDescriptor,
        request: &ContextCapabilityRequest,
        caller: &AuthenticatedCaller,
    ) -> Result<ContextCapabilityDescriptor, ContextFabricError> {
        let mut registry = self.registry.write().await;
        let admitted = registry
            .validate_context_grant(key, request, caller, now_seconds())
            .map_err(ContextFabricError::Registry)?;
        if admitted.descriptor() != descriptor {
            return Err(ContextFabricError::Registry(
                ContextRegistryError::DescriptorMismatch(key.to_string()),
            ));
        }
        Ok(admitted.descriptor().clone())
    }

    async fn run_native<F, T>(
        &self,
        cancel: Option<&CancellationToken>,
        future: F,
    ) -> Result<T, ContextFabricError>
    where
        F: std::future::Future<Output = T>,
    {
        check_cancel(cancel)?;
        let result = match cancel {
            Some(token) => {
                tokio::select! { biased; _ = token.cancelled() => return Err(ContextFabricError::Cancelled), result = future => result }
            }
            None => future.await,
        };
        check_cancel(cancel)?;
        Ok(result)
    }

    async fn run_with_cancel<F, T>(
        &self,
        cancel: Option<&CancellationToken>,
        future: F,
    ) -> Result<T, ContextFabricError>
    where
        F: std::future::Future<Output = T>,
    {
        let timed = tokio::time::timeout(self.timeout, future);
        match cancel {
            Some(token) => {
                tokio::select! { biased; _ = token.cancelled() => Err(ContextFabricError::Cancelled), result = timed => result.map_err(|_| ContextFabricError::Timeout) }
            }
            None => timed.await.map_err(|_| ContextFabricError::Timeout),
        }
    }
}

fn safe_project(messages: &[UnifiedMessage]) -> Vec<UnifiedMessage> {
    let mut projected = super::protocol_frames::public_messages(messages);
    crate::providers::message::normalize_tool_pairs(&mut projected);
    projected
}

fn make_request_from_projected(
    key: &str,
    descriptor: &ContextCapabilityDescriptor,
    projected: Vec<UnifiedMessage>,
    phase: ContextPhase,
    caller: &AuthenticatedCaller,
) -> Result<ContextCapabilityRequest, ContextFabricError> {
    let payload = serde_json::to_value(projected)
        .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
    make_payload_request(key, descriptor, &payload, phase, caller)
}

fn make_payload_request(
    _key: &str,
    descriptor: &ContextCapabilityDescriptor,
    payload: &Value,
    phase: ContextPhase,
    caller: &AuthenticatedCaller,
) -> Result<ContextCapabilityRequest, ContextFabricError> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|e| ContextFabricError::Serialization(e.to_string()))?;
    let fingerprint = canonical_fingerprint(payload);
    let snapshot = ContextSnapshot {
        snapshot_id: format!("context-fabric-{}", now_millis()),
        session_id: caller.session_id().to_string(),
        input_fingerprint: fingerprint.clone(),
        scope: ContextScope {
            namespace: ContextNamespace::Session,
            handles: Vec::new(),
        },
        token_count: estimate_tokens_smart(std::str::from_utf8(&encoded).unwrap_or_default())
            as u64,
        byte_count: encoded.len() as u64,
        payload: payload.clone(),
    };
    let operation = match phase {
        ContextPhase::Admission => ContextOperation::Admit,
        ContextPhase::Preflight => ContextOperation::Preflight,
        ContextPhase::Compaction => ContextOperation::Compact,
        ContextPhase::Recovery => ContextOperation::Restore,
        ContextPhase::Search => ContextOperation::Search,
        ContextPhase::Status => ContextOperation::Status,
        ContextPhase::Lifecycle => ContextOperation::Notify,
    };
    Ok(ContextCapabilityRequest {
        request_id: format!("context-fabric-{}", now_millis()),
        capability_id: descriptor.id.clone(),
        api_version: CONTEXT_CAPABILITY_API_VERSION.to_string(),
        phase,
        operation,
        budget: None,
        input_budget: Some(descriptor.input_budget),
        output_budget: Some(descriptor.output_budget),
        input_fingerprint: Some(fingerprint),
        snapshot: Some(snapshot),
    })
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn message_tokens(messages: &[UnifiedMessage]) -> usize {
    estimate_tokens_smart(&serde_json::to_string(messages).unwrap_or_default())
}

fn freed_tokens(before: &[UnifiedMessage], after: &[UnifiedMessage]) -> usize {
    message_tokens(before).saturating_sub(message_tokens(after))
}

fn measured_compact_result(
    before: &[UnifiedMessage],
    after: &[UnifiedMessage],
    strategy: CompactStrategy,
) -> CompactResult {
    CompactResult {
        tokens_before: estimate_tokens_smart(&serde_json::to_string(before).unwrap_or_default()),
        tokens_after: estimate_tokens_smart(&serde_json::to_string(after).unwrap_or_default()),
        strategy_used: strategy,
    }
}

fn check_cancel(cancel: Option<&CancellationToken>) -> Result<(), ContextFabricError> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        Err(ContextFabricError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ContextFabricError {
    #[error("context registry rejected call: {0}")]
    Registry(#[from] ContextRegistryError),
    #[error("context contract rejected call: {0}")]
    Contract(#[from] ContextContractError),
    #[error("context handler failed: {0}")]
    Handler(String),
    #[error("context handler timed out")]
    Timeout,
    #[error("context operation was cancelled")]
    Cancelled,
    #[error("context output is malformed: {0}")]
    MalformedOutput(String),
    #[error("protected context tail changed")]
    ProtectedTailChanged,
    #[error("context output serialization failed: {0}")]
    Serialization(String),
    #[error("builtin {0} is unavailable")]
    BuiltinUnavailable(&'static str),
    #[error("builtin compaction failed: {0}")]
    Compaction(String),
    #[error("external preflight candidate did not pass pressure/cache gates")]
    PreflightGated,
    #[error("context snapshot contains a recognised credential")]
    SensitiveInput,
    #[error("context result exceeds the descriptor output budget")]
    OutputExceedsBudget,
}

impl ContextFabricError {
    fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
    fn degraded_reason(&self) -> DegradedReason {
        match self {
            Self::Timeout => DegradedReason::Timeout,
            Self::Cancelled => DegradedReason::Timeout,
            Self::Contract(_)
            | Self::MalformedOutput(_)
            | Self::ProtectedTailChanged
            | Self::Serialization(_)
            | Self::PreflightGated
            | Self::SensitiveInput
            | Self::OutputExceedsBudget => DegradedReason::InvalidContract,
            Self::BuiltinUnavailable(_)
            | Self::Registry(_)
            | Self::Handler(_)
            | Self::Compaction(_) => DegradedReason::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_projection_drops_thinking_and_tool_signatures() {
        let messages = vec![UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "secret".into(),
                    signature: Some("sig".into()),
                    earlier_turn: false,
                },
                ContentBlock::ToolCall {
                    id: "c".into(),
                    name: "x".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: Some("tool-sig".into()),
                },
            ],
        }];
        let projected = safe_project(&messages);
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("tool-sig"));
    }
}
