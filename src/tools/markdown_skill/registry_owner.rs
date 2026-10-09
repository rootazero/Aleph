//! Markdown skill `ToolHandlerRegistry` ownership.
//!
//! This module owns the *writer half* of the capability registry for tools
//! loaded from SKILL.md files. Every tool the markdown-skill loader produces
//! is published into [`ToolHandlerRegistry`] through
//! [`MarkdownSkillRegistryOwner`]; the owner is the single writer for its
//! slots, so a foreign registration (e.g. an MCP server registering under
//! the same name) is reported as [`MarkdownRegistryError::ForeignCollision`]
//! rather than silently clobbered.
//!
//! ## Design constraints
//!
//! - **Generation-safe disposal.** Every entry we install gets a
//!   [`RegistrationHandle`]; removal always goes through that handle so the
//!   registry's monotonic-revision guard prevents a stale handle from
//!   tearing down a replacement installed under the same name.
//! - **Fail-closed before configuration.** Without a configured registry,
//!   every install fails with [`MarkdownRegistryError::RegistryNotConfigured`]
//!   — the owner cannot accidentally publish into a registry the gateway
//!   has not yet installed.
//! - **No unguarded `unregister`.** [`ToolHandlerRegistry::unregister`] is
//!   the registry's teardown path; we never call it. Every removal uses the
//!   owner's own handle so a stale-state misstep is reported rather than
//!   acted on.
//!
//! ## Threading
//!
//! All methods on [`MarkdownSkillRegistryOwner`] take `&mut self`; the
//! caller is expected to serialize access. The process-wide static
//! [`markdown_skill_registry_owner`] wraps the owner in a
//! [`crate::sync_primitives::Mutex`] for callers that reach it through the
//! static path (skill loaders, capability refresh hooks, tests).

use std::collections::HashMap;

use thiserror::Error;

use crate::sync_primitives::{Arc, Mutex, OnceLock};
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::builtin::BuiltinHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::{RegistrationHandle, ToolHandlerRegistry};
use crate::tools::service::ToolError;
use crate::tools::AlephToolDyn;
use crate::tools::ToolRegistrationScope;

/// Owner label reported by the scope's [`ToolDisposeReport`] at teardown.
const SCOPE_OWNER: &str = "capability:markdown-skills";

/// The single writer for markdown-skill-owned slots in the capability
/// registry.
///
/// Holds an [`Arc`] to the live [`ToolHandlerRegistry`], a per-name
/// [`RegistrationHandle`] for every slot this owner installed, and a
/// [`ToolRegistrationScope`] that tracks a disposer for *every* handle it ever
/// issued (including superseded generations). The
/// `Arc<ToolHandlerRegistry>` is `None` until the gateway installs one via
/// [`Self::set_registry`] (typically during bootstrap); every install /
/// replace / remove operation requires the registry to be configured first.
pub struct MarkdownSkillRegistryOwner {
    registry: Option<Arc<ToolHandlerRegistry>>,
    handles: HashMap<String, RegistrationHandle>,
    /// Ordered disposers for every handle issued. Teardown consumes this
    /// scope (see [`Self::take_shutdown`]) so the registry is drained in
    /// reverse registration order without consulting the handle map.
    scope: ToolRegistrationScope,
}

impl Default for MarkdownSkillRegistryOwner {
    fn default() -> Self {
        Self {
            registry: None,
            handles: HashMap::new(),
            scope: ToolRegistrationScope::new(SCOPE_OWNER),
        }
    }
}

impl MarkdownSkillRegistryOwner {
    /// Construct an empty owner with no registry installed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install (or refresh) the registry this owner writes through.
    ///
    /// Existing handles remain in the local map but refer to a registry the
    /// owner no longer holds an `Arc` to. The caller is expected to do this
    /// exactly once during bootstrap; replacing the registry under live
    /// handles is undefined behavior in this module.
    pub fn set_registry(&mut self, registry: Arc<ToolHandlerRegistry>) {
        self.registry = Some(registry);
    }

    /// Whether a registry has been installed.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.registry.is_some()
    }

    /// The number of slots this owner currently holds a handle for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Whether this owner holds no slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Install or replace a tool in the capability registry.
    ///
    /// Behaviour:
    /// - **No registry configured** → [`MarkdownRegistryError::RegistryNotConfigured`]
    ///   (fail closed; never publish into the void).
    /// - **Foreign collision** (the registry already holds `name` under a
    ///   handle that is not ours) → [`MarkdownRegistryError::ForeignCollision`].
    ///   Silently overwriting someone else's tool would let an MCP/extension
    ///   capability be clobbered by a markdown skill.
    /// - **Already owned** → [`ToolHandlerRegistry::replace`].
    /// - **Fresh install** → [`ToolHandlerRegistry::register`].
    ///
    /// The returned `bool` is `true` for a replacement, `false` for a fresh
    /// install.
    ///
    /// # Generation safety
    ///
    /// The new handle is inserted into the local map *before* the old one
    /// (if any) is disposed. By the time `dispose` runs on the old handle
    /// the registry has already advanced its revision past the old
    /// handle's, so `dispose` reports `false` and emits no further event.
    /// A panic between the insert and the dispose leaves the new entry in
    /// place under the new handle — the registry stays consistent.
    pub fn install_or_replace(
        &mut self,
        tool: Arc<dyn AlephToolDyn>,
    ) -> Result<bool, MarkdownRegistryError> {
        let name = tool.name().to_string();
        let registry = self
            .registry
            .clone()
            .ok_or(MarkdownRegistryError::RegistryNotConfigured)?;

        let owned_before = self.handles.contains_key(&name);
        if !owned_before && registry.resolve(&name).is_some() {
            return Err(MarkdownRegistryError::ForeignCollision { name });
        }

        // Wrap once: the handler we hand to the registry must keep the same
        // `Arc<dyn AlephToolDyn>` alive that the caller owns, so future
        // `install_or_replace` calls observe the same definition.
        let handler = Arc::new(BuiltinHandler::new(name.clone(), Arc::clone(&tool)));
        // `from_definition` builds the descriptor from the handler's own
        // `service::ToolDefinition`, pinning `source = Builtin` and carrying
        // `requires_confirmation` / `idempotent` / `max_duration_ms`
        // through `BuiltinHandler::definition`. The revision we pass (0)
        // gets overwritten by `register` / `replace` — the registry owns it.
        let descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);

        let new_handle = if owned_before {
            registry.replace(descriptor, handler)?
        } else {
            registry.register(descriptor, handler)?
        };

        // Track the handle in the scope *before* the map insert: the scope
        // must hold a disposer for every generation, including the one this
        // replacement supersedes, so teardown can drain the registry in
        // reverse registration order. The clone shares the same
        // generation guard, so a disposer for a superseded handle is a
        // harmless no-op when it finally runs.
        self.scope.track(new_handle.clone());

        // Insert BEFORE disposing the old handle: if `owned_before` was true,
        // the registry has already advanced past the old handle's revision,
        // and the old `dispose()` is a guaranteed no-op. The new handle is
        // in place first, so the map can never be observed empty for a name
        // the registry still holds.
        let old_handle = self.handles.insert(name, new_handle);
        if let Some(prev) = old_handle {
            let _ = prev.dispose();
        }

        Ok(owned_before)
    }

    /// Remove a tool this owner installed. Returns `true` if the handle was
    /// present and its `dispose` succeeded (i.e. the entry was actually
    /// removed); `false` if the name was never installed through this owner.
    ///
    /// Never reaches into the registry directly: every removal uses the
    /// generation-guarded `dispose`, so a foreign entry that happens to
    /// share the name is left untouched.
    pub fn remove(&mut self, name: &str) -> bool {
        let Some(handle) = self.handles.remove(name) else {
            return false;
        };
        handle.dispose()
    }

    /// Tear down every slot this owner installed and drop the registry link.
    ///
    /// Consumes the tracked [`ToolRegistrationScope`] and returns it, leaving
    /// the owner back at its default state (no registry, no handles, a fresh
    /// empty scope). The caller drives `scope.dispose().await` — outside the
    /// owner lock — to actually run the disposers and inspect the resulting
    /// [`ToolDisposeReport`]. Because `dispose` is generation-guarded and
    /// idempotent, a stale handle left by an earlier replacement is a
    /// harmless no-op. Repeated calls return an empty scope (idempotent).
    pub fn take_shutdown(&mut self) -> ToolRegistrationScope {
        self.handles.clear();
        self.registry = None;
        std::mem::replace(&mut self.scope, ToolRegistrationScope::new(SCOPE_OWNER))
    }
}

/// Errors that can surface from [`MarkdownSkillRegistryOwner::install_or_replace`].
#[derive(Debug, Error)]
pub enum MarkdownRegistryError {
    /// `install_or_replace` was called before [`MarkdownSkillRegistryOwner::set_registry`].
    /// The owner cannot publish into a registry it does not hold an `Arc` to.
    #[error("markdown skill registry owner is not configured with a ToolHandlerRegistry")]
    RegistryNotConfigured,

    /// The registry already holds `name` under a handle this owner did not
    /// install. Refused rather than silently clobbering a foreign capability.
    #[error(
        "tool name {name:?} is owned by another subsystem and cannot be replaced by markdown skill"
    )]
    ForeignCollision { name: String },

    /// The underlying [`ToolHandlerRegistry`] rejected the operation. The
    /// registry owns validation, descriptor/handler mismatch, duplicate
    /// registration, and the closed-registry check; this variant forwards
    /// its verdict verbatim.
    #[error("capability registry rejected the markdown skill tool: {0}")]
    Registry(#[from] ToolError),
}

// ---------------------------------------------------------------------------
// Process-wide owner.
//
// The gateway installs the registry exactly once during bootstrap; skill
// loaders and capability refresh hooks reach the owner through
// `markdown_skill_registry_owner()` and acquire the per-call lock to install
// or remove. `OnceLock` keeps this initialisation lazy and one-shot, so the
// owner is constructed the first time it is observed regardless of call
// order.
// ---------------------------------------------------------------------------

static REGISTRY_OWNER: OnceLock<Mutex<MarkdownSkillRegistryOwner>> = OnceLock::new();

/// Access the process-wide [`MarkdownSkillRegistryOwner`].
///
/// The first call constructs the owner (with no registry installed); every
/// subsequent call returns the same `Mutex`. Callers acquire the lock to
/// install, replace, or remove slots.
#[must_use]
pub fn markdown_skill_registry_owner() -> &'static Mutex<MarkdownSkillRegistryOwner> {
    REGISTRY_OWNER.get_or_init(|| Mutex::new(MarkdownSkillRegistryOwner::new()))
}

/// Install the [`ToolHandlerRegistry`] into the process-wide owner.
///
/// The intended call site is exactly one: the gateway bootstrap. Calling
/// this multiple times replaces the registry reference on the owner; any
/// handles issued under the previous registry remain in the owner's local
/// map but refer to a registry the owner no longer holds — those handles
/// become stale bookkeeping and the caller is expected to drive the owner
/// to `shutdown()` first.
pub fn set_markdown_skill_registry(registry: Arc<ToolHandlerRegistry>) {
    let mut guard = markdown_skill_registry_owner()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.set_registry(registry);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::tool_metadata::{ToolCategory, ToolDefinition as MetaDefinition};
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;

    /// Minimal `AlephToolDyn` for owner tests. The owner never invokes the
    /// tool — it only reads `name()` and wraps the value in `BuiltinHandler`,
    /// which only reads `definition()` — so the `call` impl is a noop
    /// future that should never be polled under normal use.
    struct FakeTool {
        name: String,
    }

    impl FakeTool {
        fn boxed(name: &str) -> Arc<dyn AlephToolDyn> {
            Arc::new(Self {
                name: name.to_string(),
            })
        }
    }

    impl AlephToolDyn for FakeTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn definition(&self) -> MetaDefinition {
            MetaDefinition::new(
                self.name.clone(),
                "",
                serde_json::json!({"type": "object"}),
                ToolCategory::Builtin,
            )
        }

        fn call(&self, _args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
            Box::pin(async { Ok(Value::Null) })
        }
    }

    fn fresh_registry() -> Arc<ToolHandlerRegistry> {
        Arc::new(ToolHandlerRegistry::new())
    }

    #[test]
    fn unset_owner_fails_closed_on_install() {
        let mut owner = MarkdownSkillRegistryOwner::new();
        assert!(!owner.is_configured());

        let err = owner
            .install_or_replace(FakeTool::boxed("alpha"))
            .expect_err("install without a configured registry must fail closed");
        assert!(matches!(err, MarkdownRegistryError::RegistryNotConfigured));
        assert!(owner.is_empty(), "no handle must have been created");
    }

    #[test]
    fn install_then_resolve_via_registry() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        let replaced = owner
            .install_or_replace(FakeTool::boxed("alpha"))
            .expect("first install must succeed");
        assert!(!replaced, "first install is not a replacement");
        assert_eq!(owner.len(), 1);

        // The handler is resolvable through the underlying registry; the
        // owner wrote it under the same name the FakeTool advertised.
        let resolved = registry
            .resolve("alpha")
            .expect("registry must resolve the freshly installed tool");
        assert_eq!(resolved.definition().name, "alpha");
        assert_eq!(registry.snapshot().len(), 1);
    }

    #[test]
    fn replace_returns_true_and_keeps_single_entry() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        owner
            .install_or_replace(FakeTool::boxed("beta"))
            .expect("first install");
        let snapshot_revision = registry.revision();
        let snapshot_len = registry.snapshot().len();

        let replaced = owner
            .install_or_replace(FakeTool::boxed("beta"))
            .expect("replace must succeed for an owned name");
        assert!(
            replaced,
            "second install for an owned name is a replacement"
        );

        // The registry still has exactly one entry, and the revision has
        // advanced — the new handler is observable through `resolve`.
        assert_eq!(registry.snapshot().len(), snapshot_len);
        assert!(
            registry.revision() > snapshot_revision,
            "replacement must advance the registry's monotonic revision"
        );
        let resolved = registry.resolve("beta").expect("replacement is resolvable");
        assert_eq!(resolved.definition().name, "beta");
    }

    #[test]
    fn replace_keeps_entry_and_makes_old_handle_generation_stale() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        owner
            .install_or_replace(FakeTool::boxed("gamma"))
            .expect("first install");
        let revision_after_install = registry.revision();
        assert_eq!(owner.len(), 1);

        // Second install for an owned name must take the `replace` path.
        // The owner's old handle is disposed inside `install_or_replace`
        // after the new one is inserted; because the registry has already
        // advanced past the old revision by then, that `dispose` is a
        // guaranteed no-op. The externally-observable consequences are:
        //  (a) the entry survives (still resolvable), and
        //  (b) the registry's monotonic revision advanced, so any external
        //      observer holding the old handle would see it as stale.
        let replaced = owner
            .install_or_replace(FakeTool::boxed("gamma"))
            .expect("replace");
        assert!(
            replaced,
            "second install for an owned name is a replacement"
        );
        assert!(
            registry.resolve("gamma").is_some(),
            "replacement must keep the entry alive"
        );
        assert!(
            registry.revision() > revision_after_install,
            "replacement must advance the registry's monotonic revision"
        );
        // The owner still tracks exactly one handle for `gamma` — the
        // disposed-old-handle bookkeeping did not leak.
        assert_eq!(owner.len(), 1);
    }

    #[test]
    fn foreign_collision_is_refused() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        // A foreign writer (here: the registry itself) installs a tool
        // under `foreign_name` directly, bypassing the owner.
        let foreign_descriptor = ToolCapabilityDescriptor::from_definition(
            &crate::tools::handlers::builtin::BuiltinHandler::new(
                "foreign_name".to_string(),
                FakeTool::boxed("foreign_name"),
            )
            .definition(),
            0,
        );
        let foreign_handler = Arc::new(BuiltinHandler::new(
            "foreign_name".to_string(),
            FakeTool::boxed("foreign_name"),
        ));
        registry
            .register(foreign_descriptor, foreign_handler)
            .expect("foreign install through the registry must succeed");

        // The owner must NOT silently clobber it.
        let err = owner
            .install_or_replace(FakeTool::boxed("foreign_name"))
            .expect_err("foreign collision must be reported, not overwritten");
        assert!(
            matches!(err, MarkdownRegistryError::ForeignCollision { ref name } if name == "foreign_name"),
            "got {err:?}"
        );
        assert!(
            owner.is_empty(),
            "the owner must not hold a handle for a refused name"
        );
        // The foreign entry survives — sanity-check.
        assert!(registry.resolve("foreign_name").is_some());
    }

    #[test]
    fn remove_disposes_and_clears_handle() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        owner
            .install_or_replace(FakeTool::boxed("delta"))
            .expect("install");
        assert_eq!(owner.len(), 1);
        assert!(registry.resolve("delta").is_some());

        let removed = owner.remove("delta");
        assert!(
            removed,
            "remove must report success when the handle existed"
        );
        assert_eq!(owner.len(), 0);
        assert!(
            registry.resolve("delta").is_none(),
            "registry must no longer resolve a removed name"
        );

        // A second remove for the same name is a clean no-op.
        assert!(!owner.remove("delta"), "second remove must report no-op");
    }

    #[tokio::test]
    async fn take_shutdown_disposes_every_handle_and_drops_registry_link() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        owner
            .install_or_replace(FakeTool::boxed("a"))
            .expect("install a");
        owner
            .install_or_replace(FakeTool::boxed("b"))
            .expect("install b");
        assert_eq!(owner.len(), 2);
        let rev_before = registry.revision();

        let scope = owner.take_shutdown();
        assert_eq!(scope.len(), 2, "scope must track every installed handle");
        let report = scope.dispose().await;
        assert!(
            report.all_ok(),
            "every tracked disposer must succeed: {:?}",
            report.failures().collect::<Vec<_>>()
        );
        assert!(owner.is_empty());
        assert!(
            !owner.is_configured(),
            "take_shutdown must drop the registry link"
        );
        assert_eq!(
            registry.revision(),
            rev_before + 2,
            "both entries must have been removed from the registry"
        );
        assert!(registry.resolve("a").is_none());
        assert!(registry.resolve("b").is_none());
    }

    #[tokio::test]
    async fn scope_tracks_stale_generations_and_take_shutdown_is_idempotent() {
        let registry = fresh_registry();
        let mut owner = MarkdownSkillRegistryOwner::new();
        owner.set_registry(Arc::clone(&registry));

        owner
            .install_or_replace(FakeTool::boxed("z"))
            .expect("install z");
        owner
            .install_or_replace(FakeTool::boxed("z"))
            .expect("replace z");

        // The scope must retain BOTH the superseded and current generation;
        // reverse-order disposal leaves the entry gone and reports no errors
        // (the stale disposer is a harmless no-op, not a failure).
        let scope = owner.take_shutdown();
        assert_eq!(
            scope.len(),
            2,
            "scope must retain the superseded generation"
        );
        let report = scope.dispose().await;
        assert!(
            report.all_ok(),
            "stale disposer must be a no-op, not an error: {:?}",
            report.failures().collect::<Vec<_>>()
        );
        assert!(
            registry.resolve("z").is_none(),
            "the current generation must be disposed"
        );

        // A second shutdown is a clean no-op: no handles, no registry, empty
        // scope.
        let scope2 = owner.take_shutdown();
        assert!(scope2.is_empty());
        let report2 = scope2.dispose().await;
        assert!(report2.all_ok());
    }

    #[test]
    fn static_owner_installs_registry_lazily() {
        // We can't easily reset the process-wide `OnceLock`, so we only
        // verify that the static accessor returns the same `Mutex` on
        // repeated calls. This is the only test that touches the global;
        // structural tests above exercise the owner in isolation.
        let a = markdown_skill_registry_owner();
        let b = markdown_skill_registry_owner();
        assert!(
            std::ptr::eq(a, b),
            "the static owner must be constructed exactly once"
        );
    }
}
