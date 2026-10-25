//! Layered Hook publication over the shared immutable registry primitive.
//!
//! The fair async lock is only a compatibility/publication gate. Registration
//! facts, user cursor, executor and diagnostics live in one generic snapshot.

use std::ops::Deref;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(test)]
use tokio::sync::{broadcast, Notify, TryLockError};
use tokio::sync::{RwLock, RwLockReadGuard};

use super::{HookExecutor, HookInventoryEntry};
use crate::capability::descriptor::CapabilityId;
use crate::capability::ownership::{
    LifetimeScope, OwnerRef, OwnershipTree, RegisterError, VisibilityScope,
};
use crate::capability::versioned_registry::{VersionedRegistry, VersionedSnapshot};
use crate::extension::types::{HookConfig, HookPriority, PluginRecord};
use crate::extension::visibility::{canonical_root, ScopeKey};
use crate::sync_primitives::Arc;

/// Logical content identity, independent of registration order or multiplicity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookId(String);

/// Trusted producer identity. Never deserialized from hook/plugin config.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum HookOrigin {
    User {
        config_path: PathBuf,
        scope: ScopeKey,
    },
    Plugin {
        plugin_id: String,
        scope: ScopeKey,
        plugin_root: PathBuf,
    },
    /// Reserved for a future Core producer; no installed hook uses this today.
    #[allow(dead_code)] // Reserved for future use
    Builtin { name: String },
}

/// Unadmitted declaration paired with facts captured by its real producer.
#[derive(Clone)]
pub(crate) struct HookInput {
    config: HookConfig,
    origin: HookOrigin,
}

impl HookInput {
    pub(super) fn user(mut config: HookConfig, path: &Path, scope: &ScopeKey) -> Self {
        let scope = canonical_scope(scope);
        config.scope_key = scope.clone();
        Self {
            config,
            origin: HookOrigin::User {
                config_path: canonical_root(path),
                scope,
            },
        }
    }

    pub(super) fn into_config(self) -> HookConfig {
        self.config
    }

    /// Called while reading the active owning row, never from a display label.
    pub(crate) fn plugin(mut config: HookConfig, row: &PluginRecord) -> Self {
        let scope = canonical_scope(&row.scope_key);
        config.scope_key = scope.clone();
        Self {
            config,
            origin: HookOrigin::Plugin {
                plugin_id: row.id.clone(),
                scope,
                plugin_root: canonical_root(&row.root_dir),
            },
        }
    }
}

fn canonical_scope(scope: &ScopeKey) -> ScopeKey {
    match scope {
        ScopeKey::Global => ScopeKey::Global,
        ScopeKey::Project(root) => ScopeKey::project(root),
    }
}

#[derive(Clone, Copy)]
enum OwnershipRefusal {
    Revoked,
    Disposed,
    GenerationExhausted,
}

/// Immutable lookup reference; the tree remains the sole owner authority.
#[derive(Clone)]
struct HookBinding {
    tree: Arc<OwnershipTree>,
    capability: CapabilityId,
    visibility: VisibilityScope,
}

impl HookBinding {
    fn unavailable_reason(&self) -> Option<String> {
        if self
            .tree
            .resolve(&self.capability, &self.visibility)
            .is_some()
        {
            return None;
        }
        let cause = if self.tree.is_revoked(&self.capability) {
            "revoked"
        } else {
            "unavailable (disposed lifetime or missing binding)"
        };
        Some(format!(
            "Hook ownership {cause}: hook/{}",
            self.capability.name
        ))
    }
}

#[derive(Clone)]
pub(super) struct HookMetadata {
    hook_id: String,
    source_identity: serde_json::Value,
    requested_priority: String,
    effective_priority: Option<String>,
    admission: &'static str,
    admission_reason: Option<String>,
    binding: Option<HookBinding>,
    ownership_refusal: Option<OwnershipRefusal>,
}

impl HookMetadata {
    pub(super) fn ownership_unavailable_reason(&self) -> Option<String> {
        if let Some(reason) = self
            .binding
            .as_ref()
            .and_then(HookBinding::unavailable_reason)
        {
            return Some(reason);
        }
        match self.ownership_refusal {
            Some(OwnershipRefusal::Revoked) => Some("Hook ownership revoked".into()),
            Some(OwnershipRefusal::Disposed) => Some("Hook ownership disposed lifetime".into()),
            Some(OwnershipRefusal::GenerationExhausted) => {
                Some("Hook ownership generation exhausted".into())
            }
            None => None,
        }
    }

    pub(super) fn decorate(&self, row: &mut HookInventoryEntry) {
        row.hook_id = Some(self.hook_id.clone());
        row.source_identity = Some(self.source_identity.clone());
        row.requested_priority = Some(self.requested_priority.clone());
        row.effective_priority = self.effective_priority.clone();
        row.admission = Some(self.admission.to_string());
        row.admission_reason = self.admission_reason.clone();
        if self.admission == "rejected" {
            row.reachable = false;
            row.issue = self.admission_reason.clone();
        } else if let Some(reason) = self.ownership_unavailable_reason() {
            row.reachable = false;
            row.issue = Some(reason);
        }
    }
}

#[derive(Clone)]
struct Registration {
    config: HookConfig,
    metadata: HookMetadata,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum HookRegistryError {
    #[error("hook registry is closed")]
    Closed,
    #[error("hook identity cannot be serialized: {0}")]
    Identity(#[from] serde_json::Error),
    #[error("hook user-layer cursor exhausted")]
    CursorExhausted,
}

/// All execution-bearing declaration facts, including serde-skipped root.
/// Explicit destructuring below forces new HookConfig fields to be considered.
#[derive(Serialize)]
struct Declaration<'a> {
    event: &'a crate::extension::types::HookEvent,
    kind: &'a crate::extension::types::HookKind,
    requested_priority: &'a HookPriority,
    matcher: &'a Option<String>,
    actions: &'a [crate::extension::types::HookAction],
    plugin_root: PathBuf,
    handler: &'a Option<String>,
    timeout_secs: &'a Option<u64>,
    declared_event: &'a Option<String>,
    scope: &'a ScopeKey,
}

fn identity(
    config: &HookConfig,
    origin: &HookOrigin,
) -> Result<(HookId, serde_json::Value), HookRegistryError> {
    let HookConfig {
        event,
        kind,
        priority,
        matcher,
        actions,
        plugin_name: _,
        plugin_root,
        handler,
        timeout_secs,
        declared_event,
        scope_key,
    } = config;
    let declaration = Declaration {
        event,
        kind,
        requested_priority: priority,
        matcher,
        actions,
        plugin_root: canonical_root(plugin_root),
        handler,
        timeout_secs,
        declared_event,
        scope: scope_key,
    };
    let source = serde_json::to_value(origin)?;
    #[derive(Serialize)]
    struct Identity<'a> {
        version: u8,
        origin: &'a serde_json::Value,
        declaration: Declaration<'a>,
    }
    let value = serde_json::to_value(Identity {
        version: 1,
        origin: &source,
        declaration,
    })?;
    let mut bytes = Vec::new();
    aleph_slot_api::descriptor::canonicalize_json(&value, &mut bytes);
    Ok((HookId(hex::encode(Sha256::digest(&bytes))), source))
}

fn priority_name(priority: HookPriority) -> &'static str {
    match priority {
        HookPriority::System => "system",
        HookPriority::High => "high",
        HookPriority::Normal => "normal",
        HookPriority::Low => "low",
    }
}

/// Admission runs once for incoming facts, inside their publication transaction.
fn admit(input: HookInput) -> Result<Registration, HookRegistryError> {
    let HookInput { mut config, origin } = input;
    let requested = config.priority;
    let (id, source_identity) = identity(&config, &origin)?;
    let (effective, admission, reason) = match (&origin, requested) {
        (HookOrigin::User { .. } | HookOrigin::Plugin { .. }, HookPriority::System) => (
            None,
            "rejected",
            Some("System priority is reserved for trusted Core builtin hooks".to_string()),
        ),
        (HookOrigin::Plugin { .. }, HookPriority::High) => (
            Some(HookPriority::Normal),
            "downgraded",
            Some(
                "Plugin High priority requires a trusted grant; no grant is available".to_string(),
            ),
        ),
        _ => (Some(requested), "accepted", None),
    };
    if let Some(priority) = effective {
        config.priority = priority;
    }
    Ok(Registration {
        config,
        metadata: HookMetadata {
            hook_id: id.0,
            source_identity,
            requested_priority: priority_name(requested).to_string(),
            effective_priority: effective.map(|p| priority_name(p).to_string()),
            admission,
            admission_reason: reason,
            binding: None,
            ownership_refusal: None,
        },
    })
}

fn bind(
    registrations: &mut [Registration],
    tree: &Arc<OwnershipTree>,
) -> Result<(), HookRegistryError> {
    // Validate every visibility before performing any binding side effects.
    let visibility = registrations
        .iter()
        .map(|registration| {
            let workspace = match &registration.config.scope_key {
                ScopeKey::Global => None,
                ScopeKey::Project(root) => Some(serde_json::from_value::<String>(
                    serde_json::to_value(root)?,
                )?),
            };
            Ok(VisibilityScope {
                workspace,
                ..VisibilityScope::default()
            })
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    for (registration, visibility) in registrations.iter_mut().zip(visibility) {
        if registration.metadata.effective_priority.is_none() {
            continue;
        }
        let capability = CapabilityId {
            namespace: "hook".into(),
            name: registration.metadata.hook_id.clone(),
        };
        match tree.register_if_absent(
            capability.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            visibility.clone(),
        ) {
            Ok(()) => {
                registration.metadata.binding = Some(HookBinding {
                    tree: Arc::clone(tree),
                    capability,
                    visibility,
                })
            }
            Err(error) => {
                let cause = match error {
                    RegisterError::Revoked => "revoked capability",
                    RegisterError::Disposed => "disposed lifetime",
                    RegisterError::GenerationExhausted => "ownership generation exhausted",
                };
                registration.metadata.effective_priority = None;
                registration.metadata.admission = "rejected";
                registration.metadata.admission_reason =
                    Some(format!("Hook ownership registration refused: {cause}"));
                registration.metadata.ownership_refusal = Some(match error {
                    RegisterError::Revoked => OwnershipRefusal::Revoked,
                    RegisterError::Disposed => OwnershipRefusal::Disposed,
                    RegisterError::GenerationExhausted => OwnershipRefusal::GenerationExhausted,
                });
            }
        }
    }
    Ok(())
}

/// Ordered facts and their same-generation projection. Ownership is a separate
/// authority; publication and lazy Runtime binding are not one transaction.
#[derive(Clone)]
pub(crate) struct HookState {
    plugins: Vec<Registration>,
    users: Vec<Registration>,
    user_cursor: u64,
    pub(crate) executor: HookExecutor,
}

impl HookState {
    fn rebuilt(
        &self,
        plugins: Vec<Registration>,
        users: Vec<Registration>,
        user_cursor: u64,
    ) -> Self {
        let mut admitted = Vec::new();
        let mut metadata = Vec::new();
        let mut rejected = Vec::new();
        for registration in plugins.iter().chain(&users) {
            // Retain ownership refusals only as denial guards. Source-policy
            // rejections remain excluded and cannot deny unrelated calls.
            if registration.metadata.effective_priority.is_some()
                || registration.metadata.ownership_refusal.is_some()
            {
                admitted.push(registration.config.clone());
                metadata.push(Some(registration.metadata.clone()));
            } else {
                rejected.push((registration.config.clone(), registration.metadata.clone()));
            }
        }
        Self {
            executor: self.executor.rebuilt(admitted, metadata, rejected),
            plugins,
            users,
            user_cursor,
        }
    }
}

#[derive(Clone)]
pub(crate) struct HookRegistryChange {
    pub(crate) revision: u64,
}

impl std::fmt::Debug for HookRegistryChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRegistryChange")
            .field("revision", &self.revision)
            .finish()
    }
}

pub(crate) struct HookRegistry {
    registry: VersionedRegistry<HookState, HookRegistryChange>,
    gate: RwLock<()>,
    ownership_tree: Arc<OwnershipTree>,
}

/// Own both the fair compatibility permit and the immutable captured generation.
pub(crate) struct HookReadGuard<'a> {
    _gate: RwLockReadGuard<'a, ()>,
    snapshot: Arc<VersionedSnapshot<HookState>>,
}

impl Deref for HookReadGuard<'_> {
    type Target = HookExecutor;

    fn deref(&self) -> &Self::Target {
        &self.snapshot.value.executor
    }
}

impl HookRegistry {
    #[cfg(test)]
    pub(crate) fn new(template: HookExecutor) -> Self {
        Self::new_with_ownership_tree(template, Arc::new(OwnershipTree::new()))
    }

    pub(crate) fn new_with_ownership_tree(
        template: HookExecutor,
        tree: Arc<OwnershipTree>,
    ) -> Self {
        Self {
            registry: VersionedRegistry::new(HookState {
                plugins: Vec::new(),
                users: Vec::new(),
                user_cursor: 0,
                executor: template.rebuilt(Vec::new(), Vec::new(), Vec::new()),
            }),
            gate: RwLock::new(()),
            ownership_tree: tree,
        }
    }

    pub(crate) async fn read(&self) -> HookReadGuard<'_> {
        let gate = self.gate.read().await;
        HookReadGuard {
            _gate: gate,
            snapshot: self.snapshot(),
        }
    }

    #[cfg(test)]
    pub(crate) fn try_read(&self) -> Result<HookReadGuard<'_>, TryLockError> {
        let gate = self.gate.try_read()?;
        Ok(HookReadGuard {
            _gate: gate,
            snapshot: self.snapshot(),
        })
    }

    pub(crate) fn snapshot(&self) -> Arc<VersionedSnapshot<HookState>> {
        self.registry.snapshot()
    }
    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.registry.revision()
    }
    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.registry.is_closed()
    }
    #[cfg(test)]
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<HookRegistryChange> {
        self.registry.subscribe()
    }
    #[cfg(test)]
    pub(crate) fn close_signal(&self) -> Arc<Notify> {
        self.registry.close_signal()
    }
    #[cfg(test)]
    pub(crate) fn close(&self) {
        self.registry.close();
    }

    /// Capture before any off-lock producer loading starts.
    pub(crate) fn begin_transition(&self) -> u64 {
        self.snapshot().value.user_cursor
    }

    pub(crate) async fn publish_transition(
        &self,
        observed_user_cursor: u64,
        plugin_inputs: Vec<HookInput>,
        freshly_loaded_user_inputs: Vec<HookInput>,
    ) -> Result<(), HookRegistryError> {
        let _gate = self.gate.write().await;
        self.registry.transact(|current, revision| {
            if current.closed {
                return Err(HookRegistryError::Closed);
            }
            let mut plugins = plugin_inputs
                .into_iter()
                .map(admit)
                .collect::<Result<Vec<_>, _>>()?;
            let (mut users, cursor) = if current.value.user_cursor == observed_user_cursor {
                let cursor = next_cursor(current.value.user_cursor)?;
                (
                    freshly_loaded_user_inputs
                        .into_iter()
                        .map(admit)
                        .collect::<Result<Vec<_>, _>>()?,
                    cursor,
                )
            } else {
                (current.value.users.clone(), current.value.user_cursor)
            };
            // All incoming identities above are validated before owner mutation.
            bind(&mut plugins, &self.ownership_tree)?;
            bind(&mut users, &self.ownership_tree)?;
            let next = current.value.rebuilt(plugins, users, cursor);
            Ok(Some((next, HookRegistryChange { revision }, ())))
        })?;
        Ok(())
    }

    pub(crate) async fn replace_users(
        &self,
        user_inputs: Vec<HookInput>,
    ) -> Result<(), HookRegistryError> {
        let _gate = self.gate.write().await;
        self.registry.transact(|current, revision| {
            if current.closed {
                return Err(HookRegistryError::Closed);
            }
            let cursor = next_cursor(current.value.user_cursor)?;
            let mut users = user_inputs
                .into_iter()
                .map(admit)
                .collect::<Result<Vec<_>, _>>()?;
            bind(&mut users, &self.ownership_tree)?;
            let next = current
                .value
                .rebuilt(current.value.plugins.clone(), users, cursor);
            Ok(Some((next, HookRegistryChange { revision }, ())))
        })?;
        Ok(())
    }
}

fn next_cursor(cursor: u64) -> Result<u64, HookRegistryError> {
    cursor
        .checked_add(1)
        .ok_or(HookRegistryError::CursorExhausted)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ownership_tests;
