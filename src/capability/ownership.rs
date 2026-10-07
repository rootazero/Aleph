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
use tokio::sync::broadcast;

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

/// An invalidation notification emitted by [`OwnershipTree`] under its own
/// mutex, in the same critical section that mutates ownership state.
///
/// This enum is the *only* observer-visible side of [`OwnershipTree`]'s
/// mutation methods. Each variant identifies which operation committed, and
/// carries the minimal payload needed by downstream consumers
/// (e.g. [`crate::capability::projection_host`]) to reconcile their view:
///
/// - [`OwnershipChange::Bumped`]: the owner nonce was bumped; every binding
///   at or below `scope` was rewritten to `generation` and its claims were
///   cleared.
/// - [`OwnershipChange::Revoked`]: every binding for `capability` was
///   removed and the id was added to the irreversibly-revoked set.
/// - [`OwnershipChange::Disposed`]: every binding at or below `scope` was
///   removed and the scope was added to the irreversibly-disposed set.
///
/// The sender lives entirely inside `OwnershipInner`; the change stream does
/// not route through any global bus, session event store, or approval
/// authority. Observers subscribe through [`OwnershipTree::subscribe_changes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipChange {
    /// The owner nonce was bumped.
    Bumped {
        generation: OwnerGeneration,
        scope: LifetimeScope,
    },
    /// A capability id was irreversibly revoked.
    Revoked { capability: CapabilityId },
    /// A lifetime scope was irreversibly disposed.
    Disposed { scope: LifetimeScope },
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
    /// Local invalidation broadcast: every committed mutation sends under
    /// the same mutex that performed the mutation, so subscribers only ever
    /// observe post-mutation state.
    changes: broadcast::Sender<OwnershipChange>,
}

#[derive(Debug)]
pub struct OwnershipTree {
    inner: Mutex<OwnershipInner>,
}

impl OwnershipTree {
    #[must_use]
    pub fn new() -> Self {
        // Capacity 8 mirrors the codebase's other authority broadcasts
        // (`extension::lifecycle`, `tools::registry`); bumpers are infrequent
        // so lag is rare in practice.
        let (changes, _) = broadcast::channel(8);
        Self {
            inner: Mutex::new(OwnershipInner {
                nonce: 0,
                revoked: HashSet::new(),
                disposed: HashSet::new(),
                bindings: HashMap::new(),
                changes,
            }),
        }
    }

    /// Subscribe to ownership invalidation events.
    ///
    /// Each subscriber receives every subsequent [`OwnershipChange`] emitted
    /// by [`bump`](Self::bump), [`revoke`](Self::revoke), or
    /// [`dispose`](Self::dispose). Subscribing is pure observation: it does
    /// not mutate state, does not advance the owner nonce, and does not
    /// register a binding. The receiver may lag if it falls behind the
    /// channel capacity; the lagged count is reported so a consumer can
    /// reconcile against a fresh snapshot.
    #[must_use]
    pub fn subscribe_changes(&self) -> broadcast::Receiver<OwnershipChange> {
        self.inner
            .lock()
            .expect("ownership mutex poisoned")
            .changes
            .subscribe()
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

    /// Shared authority for the two refusal gates: a revoked id or a disposed
    /// lifetime is refused identically by `register` and `register_if_absent`.
    fn guard(
        inner: &OwnershipInner,
        capability: &CapabilityId,
        lifetime: LifetimeScope,
    ) -> Result<(), RegisterError> {
        if inner.revoked.contains(capability) {
            return Err(RegisterError::Revoked);
        }
        if Self::lifetime_disposed(inner, lifetime) {
            return Err(RegisterError::Disposed);
        }
        Ok(())
    }

    pub fn register(
        &self,
        capability: CapabilityId,
        owner: OwnerRef,
        lifetime: LifetimeScope,
        visibility: VisibilityScope,
    ) -> Result<(), RegisterError> {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        Self::guard(&inner, &capability, lifetime)?;
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

    /// Insert a binding only when none exists for the `(capability, visibility)`
    /// key, preserving any incumbent's owner / lifetime / generation / claims.
    pub(crate) fn register_if_absent(
        &self,
        capability: CapabilityId,
        owner: OwnerRef,
        lifetime: LifetimeScope,
        visibility: VisibilityScope,
    ) -> Result<(), RegisterError> {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        Self::guard(&inner, &capability, lifetime)?;
        let key = Self::key(&capability, &visibility);
        let generation = OwnerGeneration(inner.nonce);
        inner.bindings.entry(key).or_insert_with(|| Binding {
            owner,
            lifetime,
            generation,
            claims: HashMap::new(),
        });
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
        // Emit AFTER the state mutation, still under the same mutex: a
        // subscriber that observes the event therefore reads the post-bump
        // state on its next lock acquisition.
        let _ = inner.changes.send(OwnershipChange::Bumped { generation, scope });
        generation
    }

    pub fn revoke(&self, capability: &CapabilityId) -> bool {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        let before = inner.bindings.len();
        inner.bindings.retain(|key, _| &key.capability != capability);
        inner.revoked.insert(capability.clone());
        let did_transition = before != inner.bindings.len();
        if did_transition {
            // Emit only when the existing method reports a real state
            // transition (a binding was actually removed).
            let _ = inner.changes.send(OwnershipChange::Revoked {
                capability: capability.clone(),
            });
        }
        did_transition
    }

    pub fn dispose(&self, scope: LifetimeScope) -> bool {
        let mut inner = self.inner.lock().expect("ownership mutex poisoned");
        let before = inner.bindings.len();
        inner.bindings.retain(|_, binding| rank(binding.lifetime) > rank(scope));
        inner.disposed.insert(scope);
        let did_dispose = before != inner.bindings.len();
        if did_dispose {
            // Emit only when the existing method reports a real disposal
            // (a binding was actually released).
            let _ = inner.changes.send(OwnershipChange::Disposed { scope });
        }
        did_dispose
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

    #[test]
    fn register_if_absent_creates_missing_binding() {
        let tree = OwnershipTree::new();
        let cap = cap_id("absent");
        let vis = full_vis();
        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis.clone(),
            ),
            Ok(())
        );
        // A newly-inserted binding is usable: it resolves and reads gen 0.
        assert_eq!(tree.resolve(&cap, &vis), Some(()));
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));
        // The inserted owner is usable: claiming as Runtime succeeds.
        assert!(tree
            .claim(&cap, &vis, OwnerRef::Runtime, "r".to_string())
            .is_some());
    }

    #[test]
    fn register_if_absent_preserves_incumbent_owner_generation_claim_lifetime() {
        let tree = OwnershipTree::new();
        let cap = cap_id("incumbent");
        let vis = full_vis();
        let incumbent = OwnerRef::Task(TaskId("t1".into()));
        tree.register(
            cap.clone(),
            incumbent.clone(),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register incumbent");
        // `claim` bumps the nonce to 1 but leaves the binding generation at 0.
        let claim = tree
            .claim(&cap, &vis, incumbent.clone(), "r1".to_string())
            .expect("claim succeeds");
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));

        // Re-insert-if-absent with a different owner/lifetime must be a no-op.
        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis.clone(),
            ),
            Ok(())
        );

        // Generation preserved as 0, NOT reset to the bumped nonce (1).
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));
        // Active claim preserved.
        assert_eq!(tree.claim_state(&claim, &cap, &vis), ClaimState::Active);
        // Incumbent owner preserved: Task can still claim, Runtime cannot.
        assert!(tree
            .claim(&cap, &vis, incumbent, "r2".to_string())
            .is_some());
        assert!(tree
            .claim(&cap, &vis, OwnerRef::Runtime, "r3".to_string())
            .is_none());
        // Lifetime preserved as Task: dispose(Run) removes a Task binding
        // (rank 1 <= 2). A clobbering replace to Runtime (rank 4) would survive.
        assert!(tree.dispose(LifetimeScope::Run));
        assert_eq!(tree.generation(&cap, &vis), None);
    }

    #[test]
    fn register_if_absent_rejects_revoked_and_disposed_without_resurrecting() {
        // Revoked: refused, and no binding is resurrected.
        let tree = OwnershipTree::new();
        let cap = cap_id("revoked");
        let vis = full_vis();
        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis.clone(),
        )
        .expect("register");
        assert!(tree.revoke(&cap));
        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis.clone(),
            ),
            Err(RegisterError::Revoked)
        );
        assert!(tree.is_revoked(&cap));
        assert_eq!(tree.resolve(&cap, &vis), None);

        // Disposed: refused for the disposed scope (and below).
        let tree2 = OwnershipTree::new();
        let cap2 = cap_id("disposed");
        let vis2 = full_vis();
        tree2.dispose(LifetimeScope::Run);
        assert_eq!(
            tree2.register_if_absent(
                cap2.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Run,
                vis2.clone(),
            ),
            Err(RegisterError::Disposed)
        );
        assert!(tree2.is_disposed(LifetimeScope::Run));
        assert_eq!(tree2.resolve(&cap2, &vis2), None);
    }

    #[test]
    fn register_if_absent_inserts_independently_per_visibility() {
        let tree = OwnershipTree::new();
        let cap = cap_id("samecap");
        let vis_a = restricted_vis("ws-a");
        let vis_b = restricted_vis("ws-b");

        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis_a.clone(),
            ),
            Ok(())
        );
        // Same capability under a different visibility is a distinct key: it
        // inserts independently.
        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis_b.clone(),
            ),
            Ok(())
        );
        assert!(tree.resolve(&cap, &vis_a).is_some());
        assert!(tree.resolve(&cap, &vis_b).is_some());

        // Re-insert under vis_a is a no-op and leaves both generations intact.
        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis_a.clone(),
            ),
            Ok(())
        );
        assert_eq!(tree.generation(&cap, &vis_a), Some(OwnerGeneration(0)));
        assert_eq!(tree.generation(&cap, &vis_b), Some(OwnerGeneration(0)));
    }

    #[test]
    fn register_replaces_owner_and_invalidates_old_claim() {
        let tree = OwnershipTree::new();
        let cap = cap_id("replace");
        let vis = full_vis();
        let old_owner = OwnerRef::Task(TaskId("old".into()));
        tree.register(
            cap.clone(),
            old_owner.clone(),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register old owner");
        let old_claim = tree
            .claim(&cap, &vis, old_owner.clone(), "old-claim".to_string())
            .expect("claim old owner");

        // Explicit `register` still REPLACES: the new owner takes over.
        let new_owner = OwnerRef::Runtime;
        tree.register(
            cap.clone(),
            new_owner.clone(),
            LifetimeScope::Runtime,
            vis.clone(),
        )
        .expect("register new owner");

        // Old owner can no longer claim; the new owner can.
        assert!(tree.claim(&cap, &vis, old_owner, "x".to_string()).is_none());
        assert!(tree
            .claim(&cap, &vis, new_owner.clone(), "new-claim".to_string())
            .is_some());
        // The old claim is invalidated by replacement (claims were reset).
        assert_eq!(
            tree.claim_state(&old_claim, &cap, &vis),
            ClaimState::Unknown
        );
    }

    #[test]
    fn register_if_absent_is_atomic_under_stale_resolve_observation() {
        use std::sync::{mpsc, Arc};
        use std::thread;

        let tree = Arc::new(OwnershipTree::new());
        let cap = cap_id("race");
        let vis = full_vis();
        let incumbent = OwnerRef::Task(TaskId("worker".into()));

        // Main observes the binding absent BEFORE the worker registers it —
        // the stale observation the old resolve-then-register path acts on.
        assert!(tree.resolve(&cap, &vis).is_none());

        let worker_tree = Arc::clone(&tree);
        let worker_cap = cap.clone();
        let worker_vis = vis.clone();
        let worker_owner = incumbent.clone();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            worker_tree
                .register(
                    worker_cap.clone(),
                    worker_owner.clone(),
                    LifetimeScope::Task,
                    worker_vis.clone(),
                )
                .expect("worker registers incumbent");
            let claim = worker_tree
                .claim(
                    &worker_cap,
                    &worker_vis,
                    worker_owner,
                    "worker-claim".to_string(),
                )
                .expect("worker claims");
            tx.send(claim).expect("worker signals main");
        });

        // Main acts on its stale observation only after the incumbent + claim
        // are observable.
        let claim = rx.recv().expect("receive worker claim");
        worker.join().expect("worker joins");

        // Atomic insert-if-absent must not clobber the incumbent.
        assert_eq!(
            tree.register_if_absent(
                cap.clone(),
                OwnerRef::Runtime,
                LifetimeScope::Runtime,
                vis.clone(),
            ),
            Ok(())
        );

        assert_eq!(tree.claim_state(&claim, &cap, &vis), ClaimState::Active);
        assert!(tree
            .claim(&cap, &vis, incumbent, "post".to_string())
            .is_some());
        assert!(tree
            .claim(&cap, &vis, OwnerRef::Runtime, "racer".to_string())
            .is_none());
    }

    // ── Task 1 (H-pre) tests: atomic ownership invalidation notifications ──
    //
    // These exercise the `OwnershipChange` broadcast channel and
    // `OwnershipTree::subscribe_changes`. They are the RED phase of TDD:
    // before implementation, the type names referenced here do not exist.

    fn wait_for_change(
        rx: &mut tokio::sync::broadcast::Receiver<OwnershipChange>,
    ) -> OwnershipChange {
        use std::thread;
        use tokio::sync::broadcast::error::TryRecvError;
        loop {
            match rx.try_recv() {
                Ok(c) => return c,
                Err(TryRecvError::Empty) => thread::yield_now(),
                Err(TryRecvError::Lagged(_)) => {
                    panic!("observer lagged behind broadcast")
                }
                Err(TryRecvError::Closed) => {
                    panic!("broadcast closed unexpectedly")
                }
            }
        }
    }

    #[test]
    fn ownership_changes_emit_inside_mutation_lock() {
        use std::sync::{mpsc, Arc};

        let tree = Arc::new(OwnershipTree::new());
        let cap_a = cap_id("a");
        let cap_b = cap_id("b");
        let vis = full_vis();
        tree.register(
            cap_a.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis.clone(),
        )
        .expect("register a");
        tree.register(
            cap_b.clone(),
            OwnerRef::Task(TaskId("t".into())),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register b");

        // ── bump ────────────────────────────────────────────────────────
        let mut rx = tree.subscribe_changes();
        let tree_obs = Arc::clone(&tree);
        let cap_a_obs = cap_a.clone();
        let cap_b_obs = cap_b.clone();
        let vis_obs = vis.clone();
        let (tx, rx_chan) =
            mpsc::channel::<(OwnershipChange, Option<OwnerGeneration>, Option<OwnerGeneration>)>();
        let observer = std::thread::spawn(move || {
            let change = wait_for_change(&mut rx);
            // After the event is delivered, read the protected state from
            // a fresh lock acquisition. Because `send` completes inside the
            // same critical section that performed the mutation, the
            // post-lock state must reflect what the event claims.
            let gen_a = tree_obs.generation(&cap_a_obs, &vis_obs);
            let gen_b = tree_obs.generation(&cap_b_obs, &vis_obs);
            tx.send((change, gen_a, gen_b)).expect("send to main");
        });

        let new_gen = tree.bump(LifetimeScope::Runtime);

        let (change, observed_gen_a, observed_gen_b) =
            rx_chan.recv().expect("observer delivers observation");
        observer.join().expect("observer joins");

        match change {
            OwnershipChange::Bumped { generation, scope } => {
                assert_eq!(
                    generation, new_gen,
                    "event reports the generation bump produced"
                );
                assert_eq!(
                    scope, LifetimeScope::Runtime,
                    "event reports the scope bumped"
                );
                assert_eq!(
                    observed_gen_a,
                    Some(new_gen),
                    "subscriber observes post-bump generation for cap_a (proves send-after-mutation)"
                );
                assert_eq!(
                    observed_gen_b,
                    Some(new_gen),
                    "subscriber observes post-bump generation for cap_b"
                );
            }
            other => panic!("expected OwnershipChange::Bumped, got {other:?}"),
        }

        // ── revoke ──────────────────────────────────────────────────────
        let mut rx2 = tree.subscribe_changes();
        let tree_obs2 = Arc::clone(&tree);
        let cap_a_obs2 = cap_a.clone();
        let vis_obs2 = vis.clone();
        let (tx2, rx_chan2) = mpsc::channel::<(OwnershipChange, bool, bool)>();
        let observer2 = std::thread::spawn(move || {
            let change = wait_for_change(&mut rx2);
            let resolved = tree_obs2.resolve(&cap_a_obs2, &vis_obs2).is_some();
            let revoked = tree_obs2.is_revoked(&cap_a_obs2);
            tx2.send((change, resolved, revoked)).expect("send to main");
        });

        assert!(
            tree.revoke(&cap_a),
            "revoke returns true on a real state transition"
        );

        let (change2, resolved_after, revoked_after) =
            rx_chan2.recv().expect("observer delivers observation");
        observer2.join().expect("observer joins");

        match change2 {
            OwnershipChange::Revoked { capability } => {
                assert_eq!(
                    capability, cap_a,
                    "event identifies the revoked capability id"
                );
                assert!(
                    !resolved_after,
                    "subscriber observes cap_a binding gone after event"
                );
                assert!(
                    revoked_after,
                    "subscriber observes cap_a marked revoked after event"
                );
            }
            other => panic!("expected OwnershipChange::Revoked, got {other:?}"),
        }

        // ── dispose ─────────────────────────────────────────────────────
        let mut rx3 = tree.subscribe_changes();
        let tree_obs3 = Arc::clone(&tree);
        let (tx3, rx_chan3) = mpsc::channel::<(OwnershipChange, bool)>();
        let observer3 = std::thread::spawn(move || {
            let change = wait_for_change(&mut rx3);
            let disposed = tree_obs3.is_disposed(LifetimeScope::Task);
            tx3.send((change, disposed)).expect("send to main");
        });

        assert!(
            tree.dispose(LifetimeScope::Task),
            "dispose returns true on a real disposal"
        );

        let (change3, disposed_after) = rx_chan3.recv().expect("observer delivers observation");
        observer3.join().expect("observer joins");

        match change3 {
            OwnershipChange::Disposed { scope } => {
                assert_eq!(
                    scope, LifetimeScope::Task,
                    "event identifies disposed lifetime scope"
                );
                assert!(
                    disposed_after,
                    "subscriber observes Task marked disposed after event"
                );
            }
            other => panic!("expected OwnershipChange::Disposed, got {other:?}"),
        }
    }

    #[test]
    fn revoke_and_dispose_notifications_are_specific() {
        let tree = OwnershipTree::new();
        let cap_alpha = cap_id("alpha");
        let cap_beta = cap_id("beta");
        let vis_alpha = restricted_vis("ws-alpha");
        let vis_beta = restricted_vis("ws-beta");

        tree.register(
            cap_alpha.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_alpha.clone(),
        )
        .expect("register alpha");
        tree.register(
            cap_beta.clone(),
            OwnerRef::Task(TaskId("t".into())),
            LifetimeScope::Task,
            vis_beta.clone(),
        )
        .expect("register beta");

        let mut rx = tree.subscribe_changes();

        // Revoke alpha: only alpha's binding is removed.
        assert!(
            tree.revoke(&cap_alpha),
            "first revoke of alpha returns true"
        );
        let change1 = wait_for_change(&mut rx);
        match change1 {
            OwnershipChange::Revoked { capability } => {
                assert_eq!(
                    capability, cap_alpha,
                    "revoke event names the exact capability id"
                );
                assert_eq!(
                    capability, cap_alpha,
                    "the revoked id is exactly the one passed to revoke()"
                );
            }
            other => panic!("expected Revoked(alpha), got {other:?}"),
        }
        assert!(
            tree.resolve(&cap_beta, &vis_beta).is_some(),
            "beta's binding is unaffected by alpha's revoke"
        );

        // Dispose Task: only the Task binding (beta) is removed.
        assert!(
            tree.dispose(LifetimeScope::Task),
            "first dispose of Task returns true"
        );
        let change2 = wait_for_change(&mut rx);
        match change2 {
            OwnershipChange::Disposed { scope } => {
                assert_eq!(
                    scope, LifetimeScope::Task,
                    "dispose event names the exact lifetime scope"
                );
                assert_ne!(
                    scope,
                    LifetimeScope::Runtime,
                    "the disposed scope is exactly Task, not Runtime"
                );
            }
            other => panic!("expected Disposed(Task), got {other:?}"),
        }
        assert!(
            tree.resolve(&cap_beta, &vis_beta).is_none(),
            "beta's binding is gone after Task dispose"
        );

        // Bump: the event carries the bumped generation and the scope bumped.
        let new_gen = tree.bump(LifetimeScope::Runtime);
        let change3 = wait_for_change(&mut rx);
        match change3 {
            OwnershipChange::Bumped { generation, scope } => {
                assert_eq!(
                    generation, new_gen,
                    "bumped event carries the generation just minted"
                );
                assert_eq!(
                    scope, LifetimeScope::Runtime,
                    "bumped event names the scope that was bumped"
                );
            }
            other => panic!("expected Bumped, got {other:?}"),
        }
    }

    #[test]
    fn ownership_notification_does_not_change_generation_or_session_seq() {
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

        // Subscription is a pure observation: generation must not change.
        let pre_gen = tree.generation(&cap, &vis);
        let _sub1 = tree.subscribe_changes();
        let _sub2 = tree.subscribe_changes();
        let _sub3 = tree.subscribe_changes();
        let post_gen = tree.generation(&cap, &vis);
        assert_eq!(
            pre_gen, post_gen,
            "subscription does not change binding generation"
        );

        // The authority stays local: two independent subscribers both observe
        // the same event from one mutation, and the post-mutation generation
        // matches the generation carried in the event payload.
        let mut rx1 = tree.subscribe_changes();
        let mut rx2 = tree.subscribe_changes();
        let new_gen = tree.bump(LifetimeScope::Runtime);

        let change1 = match rx1.try_recv() {
            Ok(c) => c,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                panic!("rx1 received no event after bump")
            }
            Err(e) => panic!("rx1: {e:?}"),
        };
        let change2 = match rx2.try_recv() {
            Ok(c) => c,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                panic!("rx2 received no event after bump")
            }
            Err(e) => panic!("rx2: {e:?}"),
        };
        assert_eq!(
            change1, change2,
            "all subscribers see the same event from one mutation"
        );
        match change1 {
            OwnershipChange::Bumped { generation, scope } => {
                assert_eq!(generation, new_gen);
                assert_eq!(scope, LifetimeScope::Runtime);
            }
            other => panic!("expected Bumped, got {other:?}"),
        }

        // The post-bump generation is what the event reports — subscription
        // never side-effected the nonce.
        let post_bump_gen = tree.generation(&cap, &vis);
        assert_eq!(
            post_bump_gen,
            Some(new_gen),
            "bump emitted the generation it claimed; subscription never side-effected"
        );

        // The 'no session event appended' half is verified by static
        // inspection: the `OwnershipChange` broadcast sender lives entirely
        // inside `OwnershipTree` and does not route through any session
        // event store, global bus, or approval authority. The authority is
        // disconnected from the session layer by construction.
    }
}
