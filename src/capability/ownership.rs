//! Capability ownership base types — the minimal vocabulary
//! [`crate::capability::descriptor`] needs to compile.
//!
//! These are pure data types with **no behaviour**. The ownership *tree* and
//! its fencing/revocation semantics are deliberately deferred to Task 2; this
//! file only establishes the types so a descriptor can name its owner, its
//! generation, its lifetime and its visibility. Keeping them behaviour-free is
//! load-bearing: `descriptor.rs` must compile against these without pulling in
//! an ownership engine that does not exist yet.

use crate::acp::manager::SessionKey;
use crate::capability::descriptor::CapabilityId;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Monotonic owner generation counter.
///
/// Bumped whenever ownership is re-issued, so a stale lease can be told apart
/// from a live one: a lease whose recorded generation is below the current one
/// is invalid. `u64` matches the registry's own monotonic revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerGeneration(pub u64);

/// How long a capability's owner binding lives, from the longest to the
/// shortest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifetimeScope {
    /// Bound to the process — lives for the whole runtime.
    Runtime,
    /// Bound to one session.
    Session,
    /// Bound to one run.
    Run,
    /// Bound to one task.
    Task,
    /// Owned externally, outside this process's lifetime model.
    External,
}

/// A run identifier.
///
/// Defined here as a plain newtype because the codebase has no single
/// canonical run-id type yet (the only `RunId` in-tree is a deserialization
/// params struct in `gateway::handlers::agent`). Align this to an existing
/// type the moment one is introduced — see the module-level note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

/// A task identifier.
///
/// Same reasoning as [`RunId`]: no canonical task-id type exists yet (the
/// domain-layer `TaskId` lives behind `#[cfg(test)]`), so this is a plain
/// newtype until one is introduced.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub String);

/// Who owns a capability binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerRef {
    /// Owned by the runtime itself (process-global, unowned-by-any-scope).
    Runtime,
    /// Owned by a specific session.
    Session(SessionKey),
    /// Owned by a specific run.
    Run(RunId),
    /// Owned by a specific task.
    Task(TaskId),
}

/// The visibility envelope a capability may be described under.
///
/// Every field is `Option`; `None` means "unrestricted" on that dimension, so
/// `VisibilityScope::default()` (all `None`) is "visible everywhere". The
/// "allowed kinds/ids" dimensions are spelled as `String` on purpose: this
/// module must not depend on [`crate::capability::descriptor::CapabilityKind`]
/// (that would make `descriptor` ↔ `ownership` a cycle), and the strings are
/// compared against the kind/name spellings at describe time, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibilityScope {
    /// The principal / agent allowed. `None` = unrestricted.
    pub principal: Option<String>,
    /// The workspace or channel allowed. `None` = unrestricted.
    pub workspace: Option<String>,
    /// The session allowed. `None` = unrestricted.
    pub session: Option<SessionKey>,
    /// Allowed capability kinds, by name. `None` = unrestricted.
    pub allowed_kinds: Option<Vec<String>>,
    /// Allowed capability ids, by name. `None` = unrestricted.
    pub allowed_ids: Option<Vec<String>>,
    /// Permission / approval context required to see this. `None` = unrestricted.
    pub approval: Option<String>,
}

/// Opaque fence token, issued by the owner layer.
///
/// The field is deliberately private and `Copy`: a fence is an unforgeable
/// marker, and every `claim` mints a fresh one so a stale lease can never
/// present an old fence as live. Only the owner layer constructs these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FencingToken(u64);

impl FencingToken {
    /// Mint a fresh token from the owner layer's monotonic nonce. This is the
    /// *only* constructor; the nonce's monotonicity is the owner layer's
    /// contract, not this type's.
    #[must_use]
    pub fn new(nonce: u64) -> Self {
        Self(nonce)
    }
}

/// A recorded effect claim: one request's ownership assertion over a fence.
///
/// Pure data — the claim's enforcement (who may revoke, what a stale fence
/// means) lives in Task 2's ownership tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaim {
    pub request_id: String,
    pub fence: FencingToken,
    pub owner: OwnerRef,
}

// =============================================================================
// Task 2 (Gate B): OwnershipTree behaviour — bump / revoke / dispose.
// =============================================================================
//
// The pure types above were Task 1's deliverable. Task 2 layers a five-deep
// tree (Runtime → Session → Run → Task → EffectClaim) on top, with these
// orthogonal axes:
//
//   * LifetimeScope  — the RAII axis: how long an owner binding lives.
//   * VisibilityScope — the ACL axis: who can see the binding.
//
// `bump` raises the owner nonce once and rewrites the generation of every binding
// at `scope` and below; every active EffectClaim attached to an affected
// binding is reclassified `Unknown`. `revoke` / `dispose` are irreversible —
// once flipped they cannot be undone, and any subsequent `resolve` / `claim`
// is rejected.

/// Lifetime ordering: Runtime is the outermost / longest-lived container;
/// Task is innermost / shortest-lived; External sits outside the process model.
///
/// `bump(scope)` rewrites the generation of every binding whose `lifetime` is
/// at-or-below `scope` in this order, so a child binding's recorded
/// generation is older than its parent's and any claim made against it is
/// `Unknown`.
impl PartialOrd for LifetimeScope {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LifetimeScope {
    fn cmp(&self, other: &Self) -> Ordering {
        rank(*self).cmp(&rank(*other))
    }
}

fn rank(scope: LifetimeScope) -> u8 {
    match scope {
        LifetimeScope::External => 0,
        LifetimeScope::Task => 1,
        LifetimeScope::Run => 2,
        LifetimeScope::Session => 3,
        LifetimeScope::Runtime => 4,
    }
}

/// Observable state of an `EffectClaim`.
///
/// `Active` means the binding the claim was made against is live, not revoked,
/// not disposed, and has not been re-issued since the claim. `Unknown` covers
/// every reason a claim may stop being authoritative: revoked id, disposed
/// lifetime, parent-bump invalidation, or no such claim exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    Active,
    Unknown,
}

/// Why `register` refused to install a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    /// The id has been revoked in this tree; new bindings for it are refused
    /// permanently.
    Revoked,
    /// The requested lifetime has been disposed; new bindings at it (or below)
    /// are refused permanently.
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BindingKey {
    capability: CapabilityId,
    visibility: String,
}

#[derive(Debug)]
struct Binding {
    owner: OwnerRef,
    lifetime: LifetimeScope,
    generation: OwnerGeneration,
    claims: HashMap<FencingToken, (OwnerGeneration, OwnerRef)>,
}

#[derive(Debug)]
struct OwnershipInner {
    nonce: u64,
    revoked: HashSet<CapabilityId>,
    disposed: HashSet<LifetimeScope>,
    bindings: HashMap<BindingKey, Binding>,
}

#[derive(Debug)]
pub struct OwnershipTree {
    inner: Mutex<OwnershipInner>,
}

impl OwnershipTree {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(OwnershipInner {
                nonce: 0,
                revoked: HashSet::new(),
                disposed: HashSet::new(),
                bindings: HashMap::new(),
            }),
        }
    }

    fn key(capability: &CapabilityId, visibility: &VisibilityScope) -> BindingKey {
        BindingKey {
            capability: capability.clone(),
            visibility: format!("{visibility:?}"),
        }
    }

    fn lifetime_disposed(inner: &OwnershipInner, lifetime: LifetimeScope) -> bool {
        inner.disposed.iter().any(|scope| rank(lifetime) <= rank(*scope))
    }

    pub fn register(
        &self,
        capability: CapabilityId,
        owner: OwnerRef,
        lifetime: LifetimeScope,
        visibility: VisibilityScope,
    ) -> Result<(), RegisterError> {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        if inner.revoked.contains(&capability) {
            return Err(RegisterError::Revoked);
        }
        if Self::lifetime_disposed(&inner, lifetime) {
            return Err(RegisterError::Disposed);
        }
        let key = Self::key(&capability, &visibility);
        let generation = OwnerGeneration(inner.nonce);
        inner.bindings.insert(
            key,
            Binding {
                owner,
                lifetime,
                generation,
                claims: HashMap::new(),
            },
        );
        Ok(())
    }

    pub fn resolve(&self, capability: &CapabilityId, visibility: &VisibilityScope) -> Option<()> {
        let inner = self.inner.lock().expect("ownership mutex poisoned");
        let key = Self::key(capability, visibility);
        inner.bindings.get(&key).map(|_| ())
    }

    /// Read the current owner generation of one `(CapabilityId, VisibilityScope)`
    /// binding without mutating it.
    ///
    /// Returns `None` when no such binding exists. Never bumps the nonce, never
    /// touches claims, never registers a binding. Generation is per-binding
    /// (`(CapabilityId, VisibilityScope)`-level), NOT a global registry revision.
    #[must_use]
    pub fn generation(
        &self,
        capability: &CapabilityId,
        visibility: &VisibilityScope,
    ) -> Option<OwnerGeneration> {
        let inner = self.inner.lock().expect("ownership mutex poisoned");
        let key = Self::key(capability, visibility);
        inner.bindings.get(&key).map(|binding| binding.generation)
    }

    pub fn claim(
        &self,
        capability: &CapabilityId,
        visibility: &VisibilityScope,
        owner: OwnerRef,
        request_id: String,
    ) -> Option<EffectClaim> {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        if inner.revoked.contains(capability) {
            return None;
        }
        let key = Self::key(capability, visibility);
        let lifetime = inner.bindings.get(&key)?.lifetime;
        if Self::lifetime_disposed(&inner, lifetime) {
            return None;
        }
        if inner.bindings.get(&key)?.owner != owner {
            return None;
        }
        inner.nonce = inner.nonce.checked_add(1)?;
        let fence = FencingToken::new(inner.nonce);
        let binding = inner.bindings.get_mut(&key)?;
        binding
            .claims
            .insert(fence, (binding.generation, binding.owner.clone()));
        Some(EffectClaim { request_id, fence, owner })
    }

    pub fn claim_state(
        &self,
        claim: &EffectClaim,
        capability: &CapabilityId,
        visibility: &VisibilityScope,
    ) -> ClaimState {
        let inner = self.inner.lock().expect("ownership mutex poisoned");
        let Some(binding) = inner.bindings.get(&Self::key(capability, visibility)) else {
            return ClaimState::Unknown;
        };
        if inner.revoked.contains(capability)
            || Self::lifetime_disposed(&inner, binding.lifetime)
            || binding
                .claims
                .get(&claim.fence)
                .is_none_or(|(generation, owner)| {
                    *generation != binding.generation || *owner != claim.owner
                })
        {
            ClaimState::Unknown
        } else {
            ClaimState::Active
        }
    }

    pub fn is_revoked(&self, capability: &CapabilityId) -> bool {
        self.inner.lock().expect("ownership mutex poisoned").revoked.contains(capability)
    }

    pub fn is_disposed(&self, scope: LifetimeScope) -> bool {
        self.inner.lock().expect("ownership mutex poisoned").disposed.contains(&scope)
    }

    pub fn bump(&self, scope: LifetimeScope) -> OwnerGeneration {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        inner.nonce = inner.nonce.saturating_add(1);
        let generation = OwnerGeneration(inner.nonce);
        for binding in inner.bindings.values_mut() {
            if rank(binding.lifetime) <= rank(scope) {
                binding.generation = generation;
                binding.claims.clear();
            }
        }
        generation
    }

    pub fn revoke(&self, capability: &CapabilityId) -> bool {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        let before = inner.bindings.len();
        inner.bindings.retain(|key, _| &key.capability != capability);
        inner.revoked.insert(capability.clone());
        before != inner.bindings.len()
    }

    pub fn dispose(&self, scope: LifetimeScope) -> bool {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        let before = inner.bindings.len();
        inner.bindings.retain(|_, binding| rank(binding.lifetime) > rank(scope));
        inner.disposed.insert(scope);
        before != inner.bindings.len()
    }
}

impl Default for OwnershipTree {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    //! Behaviour tests for `OwnershipTree`.
    //!
    //! The three tests pinned by Gate B:
    //!
    //! 1. `bump_invalidates_child_claims` — bumping a parent scope rewrites
    //!    the generation of every child binding, and any claim made against
    //!    such a binding transitions to `Unknown`.
    //! 2. `same_id_two_scopes_coexist` — the same `CapabilityId` may be
    //!    registered under two distinct `VisibilityScope`s; both resolve
    //!    independently.
    //! 3. `revoke_and_dispose_are_irreversible` — revoke disposes of every
    //!    binding for an id, makes its active claims `Unknown`, and refuses
    //!    subsequent claims; dispose wipes a lifetime and everything below it.

    use super::*;

    fn cap_id(name: &str) -> CapabilityId {
        CapabilityId {
            namespace: "aleph/test".to_string(),
            name: name.to_string(),
        }
    }

    fn full_vis() -> VisibilityScope {
        VisibilityScope::default()
    }

    fn restricted_vis(workspace: &str) -> VisibilityScope {
        VisibilityScope {
            workspace: Some(workspace.to_string()),
            ..VisibilityScope::default()
        }
    }

    #[test]
    fn bump_invalidates_child_claims() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        tree.register(
            cap.clone(),
            OwnerRef::Task(TaskId("t1".into())),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        let claim = tree
            .claim(
                &cap,
                &vis,
                OwnerRef::Task(TaskId("t1".into())),
                "r1".to_string(),
            )
            .expect("claim succeeds");
        assert_eq!(
            tree.claim_state(&claim, &cap, &vis),
            ClaimState::Active,
            "freshly-minted claim is Active"
        );

        let new_gen = tree.bump(LifetimeScope::Session);
        assert!(
            new_gen.0 > 0,
            "bump returns the monotonic generation it just minted"
        );

        assert_eq!(
            tree.claim_state(&claim, &cap, &vis),
            ClaimState::Unknown,
            "bump(Session) invalidates the Task-bounded child binding"
        );
    }

    #[test]
    fn same_id_two_scopes_coexist() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis_a = restricted_vis("ws-a");
        let vis_b = restricted_vis("ws-b");

        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_a.clone(),
        )
        .expect("register under vis_a");
        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_b.clone(),
        )
        .expect("register under vis_b");

        assert!(
            tree.resolve(&cap, &vis_a).is_some(),
            "vis_a binding resolves"
        );
        assert!(
            tree.resolve(&cap, &vis_b).is_some(),
            "vis_b binding resolves"
        );
    }

    #[test]
    fn register_after_revoke_is_rejected() {
        let tree = OwnershipTree::new();
        let cap = cap_id("revoke");
        let vis = full_vis();
        tree.register(cap.clone(), OwnerRef::Runtime, LifetimeScope::Runtime, vis.clone())
            .expect("register");
        tree.revoke(&cap);
        assert_eq!(
            tree.register(cap, OwnerRef::Runtime, LifetimeScope::Runtime, vis),
            Err(RegisterError::Revoked)
        );
    }

    #[test]
    fn register_after_dispose_is_rejected() {
        let tree = OwnershipTree::new();
        let cap = cap_id("dispose");
        let vis = full_vis();
        tree.dispose(LifetimeScope::Run);
        assert_eq!(
            tree.register(cap, OwnerRef::Runtime, LifetimeScope::Run, vis),
            Err(RegisterError::Disposed)
        );
    }

    #[test]
    fn bump_task_does_not_affect_parent_claim() {
        let tree = OwnershipTree::new();
        let cap = cap_id("parent");
        let vis = full_vis();
        tree.register(cap.clone(), OwnerRef::Runtime, LifetimeScope::Runtime, vis.clone())
            .expect("register");
        let claim = tree.claim(&cap, &vis, OwnerRef::Runtime, "parent".into()).expect("claim");
        tree.bump(LifetimeScope::Task);
        assert_eq!(tree.claim_state(&claim, &cap, &vis), ClaimState::Active);
    }

    #[test]
    fn bump_runtime_invalidates_child_claim() {
        let tree = OwnershipTree::new();
        let cap = cap_id("child");
        let vis = full_vis();
        tree.register(cap.clone(), OwnerRef::Task(TaskId("t".into())), LifetimeScope::Task, vis.clone())
            .expect("register");
        let claim = tree.claim(&cap, &vis, OwnerRef::Task(TaskId("t".into())), "child".into()).expect("claim");
        tree.bump(LifetimeScope::Runtime);
        assert_eq!(tree.claim_state(&claim, &cap, &vis), ClaimState::Unknown);
    }

    #[test]
    fn revoke_and_dispose_are_irreversible() {
        // ── revoke path ───────────────────────────────────────────────────
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis.clone(),
        )
        .expect("register");
        let claim = tree
            .claim(&cap, &vis, OwnerRef::Runtime, "r1".to_string())
            .expect("claim");

        assert!(tree.revoke(&cap), "first revoke actually removes the binding");
        assert!(
            tree.resolve(&cap, &vis).is_none(),
            "resolve returns None after revoke"
        );
        assert_eq!(
            tree.claim_state(&claim, &cap, &vis),
            ClaimState::Unknown,
            "active claim transitions to Unknown on revoke"
        );
        assert!(
            tree.is_revoked(&cap),
            "id is marked revoked (irreversible)"
        );

        // Second revoke: still unsuccessful at removing (nothing left to drop),
        // and the irreversibility is preserved.
        assert!(
            !tree.revoke(&cap),
            "second revoke is a no-op on the bindings"
        );
        assert!(
            tree.is_revoked(&cap),
            "id stays revoked after a second revoke"
        );

        // New claim after revoke is rejected.
        assert!(
            tree.claim(&cap, &vis, OwnerRef::Runtime, "r2".to_string())
                .is_none(),
            "no new claim against a revoked id"
        );

        // ── dispose path ──────────────────────────────────────────────────
        let tree2 = OwnershipTree::new();
        tree2
            .register(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Run,
                vis.clone(),
            )
            .expect("register on tree2");

        assert!(
            tree2.dispose(LifetimeScope::Run),
            "first dispose actually releases bindings"
        );
        assert!(
            tree2.is_disposed(LifetimeScope::Run),
            "scope is marked disposed (irreversible)"
        );
        assert!(
            tree2.claim(&cap, &vis, OwnerRef::Runtime, "r3".to_string())
                .is_none(),
            "no new claim against a disposed scope"
        );
        assert!(
            !tree2.dispose(LifetimeScope::Run),
            "second dispose is a no-op"
        );
    }

    #[test]
    fn generation_reads_binding_and_tracks_bump() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        assert_eq!(tree.generation(&cap, &vis), None);
        tree.register(
            cap.clone(),
            OwnerRef::Task(TaskId("t1".into())),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));
        let bumped = tree.bump(LifetimeScope::Task);
        assert_eq!(bumped, OwnerGeneration(1));
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(1)));
    }

    #[test]
    fn generation_is_none_after_revoke_or_dispose() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));
        assert!(tree.revoke(&cap));
        assert_eq!(tree.generation(&cap, &vis), None);

        let cap2 = cap_id("bar");
        tree.register(
            cap2.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        assert!(tree.dispose(LifetimeScope::Task));
        assert_eq!(tree.generation(&cap2, &vis), None);
    }

    #[test]
    fn generation_does_not_mutate() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis.clone(),
        )
        .expect("register succeeds");
        let before = tree.generation(&cap, &vis);
        let again = tree.generation(&cap, &vis);
        assert_eq!(before, again);
        assert_eq!(tree.resolve(&cap, &vis), Some(()));
    }
}
