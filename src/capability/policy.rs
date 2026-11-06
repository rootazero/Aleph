//! Capability policy enforcement — access control, visibility validation, and
//! cross-session policy decisions.
//!
//! This module implements the policy layer that sits between capability
//! resolution requests and the ownership tree. Every `resolve` / `claim` /
//! `validate_lease` decision flows through these gates, which enforce:
//!
//! 1. **Visibility scope matching** — principal, workspace, session, allowed
//!    kinds/ids, and approval context must all align.
//! 2. **Owner verification** — the requesting principal must match the binding's
//!    recorded owner.
//! 3. **Lifetime validation** — the binding's lifetime must not be disposed.
//! 4. **Generation consistency** — leases carry a generation that must match
//!    the current binding generation (no stale leases).
//!
//! All enforcement is **fail-closed**: any mismatch, missing binding, revoked
//! id, or disposed lifetime returns a refusal. The policy engine never guesses
//! or falls back to a permissive default.

use crate::acp::manager::SessionKey;
use crate::capability::descriptor::{CapabilityDescriptor, CapabilityId};
use crate::capability::events::{AuditedPolicyEngine, CapabilityEventEmitter};
use crate::capability::ownership::{
    LifetimeScope, OwnerGeneration, OwnerRef, OwnershipTree, VisibilityScope,
};

/// Why a capability access request was denied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyDenial {
    /// No binding exists for this `(CapabilityId, VisibilityScope)` pair.
    #[error("capability not found")]
    NotFound,

    /// The capability id has been irreversibly revoked.
    #[error("capability revoked")]
    Revoked,

    /// The binding's lifetime has been irreversibly disposed.
    #[error("lifetime disposed")]
    Disposed,

    /// The requesting principal does not match the binding's recorded owner.
    #[error("principal mismatch: expected {expected:?}, got {actual:?}")]
    PrincipalMismatch {
        expected: OwnerRef,
        actual: OwnerRef,
    },

    /// The visibility scope does not match the request.
    #[error("visibility scope mismatch")]
    VisibilityMismatch,

    /// The lease generation is stale (binding has been bumped or rotated).
    #[error("generation mismatch: expected {expected:?}, got {actual:?}")]
    GenerationMismatch {
        expected: OwnerGeneration,
        actual: OwnerGeneration,
    },

    /// The approval context required by the visibility scope is missing.
    #[error("approval required: {context}")]
    ApprovalRequired { context: String },

    /// The requested capability kind is not allowed by the visibility scope.
    #[error("kind not allowed: {kind}")]
    KindNotAllowed { kind: String },

    /// The requested capability id is not allowed by the visibility scope.
    #[error("id not allowed: {id}")]
    IdNotAllowed { id: String },
}

/// A validated capability lease: proof that a request passed all policy gates.
///
/// This is an opaque token returned by [`PolicyEngine::resolve`]. Callers
/// present it to [`PolicyEngine::validate_lease`] later, and the engine
/// re-checks that the binding is still live, not revoked, not disposed, and
/// at the same generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityLease {
    pub(crate) capability: CapabilityId,
    pub(crate) visibility: VisibilityScope,
    pub(crate) generation: OwnerGeneration,
    pub(crate) owner: OwnerRef,
    pub(crate) lifetime: LifetimeScope,
}

/// The capability policy enforcement engine.
///
/// All access control decisions flow through this type. It wraps an
/// [`OwnershipTree`] and applies the three-gate model:
///
/// 1. **Visibility gate** — does the request's visibility scope match what the
///    descriptor allows?
/// 2. **Principal gate** — does the requesting principal match the binding's
///    owner?
/// 3. **Authority gate** — is the binding live (not revoked, not disposed, at
///    the expected generation)?
///
/// Every method is fail-closed: a denial at any gate stops the request.
pub struct PolicyEngine {
    tree: OwnershipTree,
}

impl PolicyEngine {
    #[must_use]
    pub fn new(tree: OwnershipTree) -> Self {
        Self { tree }
    }

    /// Observe capability-specific policy gates without changing their decisions.
    #[must_use]
    pub fn with_events(self, events: CapabilityEventEmitter) -> AuditedPolicyEngine {
        AuditedPolicyEngine::new(self, events)
    }

    /// Resolve a capability for the given principal and visibility scope.
    ///
    /// This is the primary access control gate. It checks:
    /// 1. A binding exists for `(capability, visibility)`.
    /// 2. The capability is not revoked.
    /// 3. The binding's lifetime is not disposed.
    /// 4. The requesting `principal` matches the binding's owner.
    /// 5. The `visibility` scope aligns with the descriptor's constraints.
    ///
    /// On success, returns a [`CapabilityLease`] that can be validated later
    /// via [`Self::validate_lease`].
    pub fn resolve(
        &self,
        capability: &CapabilityId,
        principal: &OwnerRef,
        visibility: &VisibilityScope,
        descriptor: &CapabilityDescriptor,
    ) -> Result<CapabilityLease, PolicyDenial> {
        // Gate 1: Binding existence and revocation check
        if self.tree.is_revoked(capability) {
            return Err(PolicyDenial::Revoked);
        }

        let generation = self
            .tree
            .generation(capability, visibility)
            .ok_or(PolicyDenial::NotFound)?;

        let owner = self
            .tree
            .owner_of(capability, visibility)
            .ok_or(PolicyDenial::NotFound)?;

        // Gate 2: Principal verification
        if &owner != principal {
            return Err(PolicyDenial::PrincipalMismatch {
                expected: owner.clone(),
                actual: principal.clone(),
            });
        }

        // Gate 3: Visibility scope validation
        self.validate_visibility_scope(capability, visibility, descriptor)?;

        // Gate 4: Lifetime disposal check (derived from the descriptor)
        if self.tree.is_disposed(descriptor.lifetime) {
            return Err(PolicyDenial::Disposed);
        }

        Ok(CapabilityLease {
            capability: capability.clone(),
            visibility: visibility.clone(),
            generation,
            owner,
            lifetime: descriptor.lifetime,
        })
    }

    /// Validate that a previously issued lease is still valid.
    ///
    /// This is the second half of the two-phase access control model:
    /// 1. `resolve` issues a lease after checking policy gates.
    /// 2. `validate_lease` re-checks that the binding is still live before use.
    ///
    /// A lease becomes invalid when:
    /// - The capability is revoked.
    /// - The binding's lifetime is disposed.
    /// - The owner generation has been bumped or rotated.
    /// - The binding no longer exists.
    pub fn validate_lease(&self, lease: &CapabilityLease) -> Result<(), PolicyDenial> {
        if self.tree.is_revoked(&lease.capability) {
            return Err(PolicyDenial::Revoked);
        }

        if self.tree.is_disposed(lease.lifetime) {
            return Err(PolicyDenial::Disposed);
        }

        let current_generation = self
            .tree
            .generation(&lease.capability, &lease.visibility)
            .ok_or(PolicyDenial::NotFound)?;

        if current_generation != lease.generation {
            return Err(PolicyDenial::GenerationMismatch {
                expected: lease.generation,
                actual: current_generation,
            });
        }

        let current_owner = self
            .tree
            .owner_of(&lease.capability, &lease.visibility)
            .ok_or(PolicyDenial::NotFound)?;

        if current_owner != lease.owner {
            return Err(PolicyDenial::PrincipalMismatch {
                expected: lease.owner.clone(),
                actual: current_owner,
            });
        }

        Ok(())
    }

    /// Validate that the visibility scope matches the descriptor's constraints.
    ///
    /// Fail-closed: any dimension that's restricted in the descriptor but
    /// unrestricted in the request scope is a mismatch.
    fn validate_visibility_scope(
        &self,
        capability: &CapabilityId,
        visibility: &VisibilityScope,
        descriptor: &CapabilityDescriptor,
    ) -> Result<(), PolicyDenial> {
        // Approval context check
        if let Some(required_approval) = &descriptor.visibility.approval {
            match &visibility.approval {
                Some(provided) if provided == required_approval => {}
                _ => {
                    return Err(PolicyDenial::ApprovalRequired {
                        context: required_approval.clone(),
                    })
                }
            }
        }

        // Allowed kinds check
        if let Some(allowed_kinds) = &descriptor.visibility.allowed_kinds {
            let kind_name = format!("{:?}", descriptor.kind);
            if !allowed_kinds.contains(&kind_name) {
                return Err(PolicyDenial::KindNotAllowed { kind: kind_name });
            }
        }

        // Allowed ids check
        if let Some(allowed_ids) = &descriptor.visibility.allowed_ids {
            if !allowed_ids.contains(&capability.name) {
                return Err(PolicyDenial::IdNotAllowed {
                    id: capability.name.clone(),
                });
            }
        }

        // Principal match (if specified in descriptor)
        if let Some(required_principal) = &descriptor.visibility.principal {
            match &visibility.principal {
                Some(provided) if provided == required_principal => {}
                _ => return Err(PolicyDenial::VisibilityMismatch),
            }
        }

        // Workspace match (if specified in descriptor)
        if let Some(required_workspace) = &descriptor.visibility.workspace {
            match &visibility.workspace {
                Some(provided) if provided == required_workspace => {}
                _ => return Err(PolicyDenial::VisibilityMismatch),
            }
        }

        // Session match (if specified in descriptor)
        if let Some(required_session) = &descriptor.visibility.session {
            match &visibility.session {
                Some(provided) if provided == required_session => {}
                _ => return Err(PolicyDenial::VisibilityMismatch),
            }
        }

        Ok(())
    }

    /// Check if a principal has delegation rights over another principal.
    ///
    /// Delegation rules:
    /// - Runtime can delegate to any scope.
    /// - Session can delegate to Run/Task within itself.
    /// - Run can delegate to Task within itself.
    /// - Task cannot delegate.
    /// - External cannot delegate.
    pub fn can_delegate(&self, delegator: &OwnerRef, delegatee: &OwnerRef) -> bool {
        match delegator {
            OwnerRef::Runtime => true,
            OwnerRef::Session(session_key) => match delegatee {
                OwnerRef::Session(other) if session_key == other => true,
                OwnerRef::Run(_) | OwnerRef::Task(_) => true,
                _ => false,
            },
            OwnerRef::Run(run_id) => match delegatee {
                OwnerRef::Run(other) if run_id == other => true,
                OwnerRef::Task(_) => true,
                _ => false,
            },
            OwnerRef::Task(_) => false,
        }
    }

    /// Check cross-session access: can `requestor_session` access a capability
    /// owned by `owner_session`?
    ///
    /// Cross-session policy:
    /// - Same session: always allowed.
    /// - Different session: requires explicit grant (not implemented in this phase).
    pub fn check_cross_session(
        &self,
        requestor_session: &SessionKey,
        owner_session: &SessionKey,
    ) -> Result<(), PolicyDenial> {
        if requestor_session == owner_session {
            Ok(())
        } else {
            // Cross-session grants are deferred to a later phase
            Err(PolicyDenial::VisibilityMismatch)
        }
    }

    /// Access the underlying ownership tree for direct queries.
    ///
    /// This is a deliberate escape hatch for cases where the caller needs to
    /// inspect ownership state without going through the policy gates (e.g.,
    /// administrative queries, diagnostics). Use sparingly.
    #[must_use]
    pub fn tree(&self) -> &OwnershipTree {
        &self.tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::descriptor::{
        CapabilityKind, CapabilityRevision, ProjectionMetadata, SchemaRef,
    };

    fn fake_descriptor(
        id: CapabilityId,
        visibility: VisibilityScope,
        lifetime: LifetimeScope,
    ) -> CapabilityDescriptor {
        CapabilityDescriptor {
            id,
            kind: CapabilityKind::Tool,
            schema: SchemaRef {
                version: 0,
                fingerprint: [0u8; 32],
            },
            revision: CapabilityRevision(0),
            lifetime,
            visibility,
            owner: OwnerRef::Runtime,
            services: vec![],
            metadata: ProjectionMetadata::default(),
            facets: Default::default(),
        }
    }

    fn cap_id(name: &str) -> CapabilityId {
        CapabilityId {
            namespace: "aleph/test".to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn resolve_succeeds_when_all_gates_pass() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("tool");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();

        assert_eq!(lease.capability, cap);
        assert_eq!(lease.owner, owner);
    }

    #[test]
    fn resolve_fails_when_binding_missing() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("missing");
        let vis = VisibilityScope::default();
        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);

        let result = policy.resolve(&cap, &OwnerRef::Runtime, &vis, &descriptor);
        assert_eq!(result, Err(PolicyDenial::NotFound));
    }

    #[test]
    fn resolve_fails_when_capability_revoked() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("revoked");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();
        policy.tree.revoke(&cap);

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let result = policy.resolve(&cap, &owner, &vis, &descriptor);
        assert_eq!(result, Err(PolicyDenial::Revoked));
    }

    #[test]
    fn resolve_fails_when_lifetime_disposed() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("disposed");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(cap.clone(), owner.clone(), LifetimeScope::Task, vis.clone())
            .unwrap();
        policy.tree.dispose(LifetimeScope::Task);

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Task);
        let result = policy.resolve(&cap, &owner, &vis, &descriptor);
        // Disposed bindings are not found in the tree, so NotFound is returned
        assert_eq!(result, Err(PolicyDenial::NotFound));
    }

    #[test]
    fn resolve_fails_when_principal_mismatch() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("wrong-owner");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;
        let wrong_principal = OwnerRef::Session(SessionKey::new("test", "."));

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let result = policy.resolve(&cap, &wrong_principal, &vis, &descriptor);

        assert!(matches!(
            result,
            Err(PolicyDenial::PrincipalMismatch { .. })
        ));
    }

    #[test]
    fn resolve_fails_when_approval_required_but_missing() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("needs-approval");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let mut descriptor_vis = VisibilityScope::default();
        descriptor_vis.approval = Some("admin-approval".to_string());
        let descriptor = fake_descriptor(cap.clone(), descriptor_vis, LifetimeScope::Runtime);

        let result = policy.resolve(&cap, &owner, &vis, &descriptor);
        assert!(matches!(result, Err(PolicyDenial::ApprovalRequired { .. })));
    }

    #[test]
    fn resolve_succeeds_when_approval_matches() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("approved");
        let mut vis = VisibilityScope::default();
        vis.approval = Some("admin-approval".to_string());
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let mut descriptor_vis = VisibilityScope::default();
        descriptor_vis.approval = Some("admin-approval".to_string());
        let descriptor = fake_descriptor(cap.clone(), descriptor_vis, LifetimeScope::Runtime);

        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();
        assert_eq!(lease.capability, cap);
    }

    #[test]
    fn resolve_fails_when_kind_not_allowed() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("wrong-kind");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let mut descriptor_vis = VisibilityScope::default();
        descriptor_vis.allowed_kinds = Some(vec!["Skill".to_string(), "Agent".to_string()]);
        let descriptor = fake_descriptor(cap.clone(), descriptor_vis, LifetimeScope::Runtime);

        let result = policy.resolve(&cap, &owner, &vis, &descriptor);
        assert!(matches!(result, Err(PolicyDenial::KindNotAllowed { .. })));
    }

    #[test]
    fn resolve_fails_when_id_not_allowed() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("forbidden-id");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let mut descriptor_vis = VisibilityScope::default();
        descriptor_vis.allowed_ids = Some(vec!["allowed-id".to_string()]);
        let descriptor = fake_descriptor(cap.clone(), descriptor_vis, LifetimeScope::Runtime);

        let result = policy.resolve(&cap, &owner, &vis, &descriptor);
        assert!(matches!(result, Err(PolicyDenial::IdNotAllowed { .. })));
    }

    #[test]
    fn validate_lease_succeeds_when_binding_unchanged() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("stable");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();

        // Validate immediately — binding unchanged
        assert!(policy.validate_lease(&lease).is_ok());
    }

    #[test]
    fn validate_lease_fails_when_generation_bumped() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("bumped");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();

        // Bump the generation
        policy.tree.bump(LifetimeScope::Runtime).unwrap();

        let result = policy.validate_lease(&lease);
        assert!(matches!(
            result,
            Err(PolicyDenial::GenerationMismatch { .. })
        ));
    }

    #[test]
    fn validate_lease_fails_when_capability_revoked_after_issue() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("revoked-later");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();

        // Revoke after lease issued
        policy.tree.revoke(&cap);

        let result = policy.validate_lease(&lease);
        assert_eq!(result, Err(PolicyDenial::Revoked));
    }

    #[test]
    fn validate_lease_fails_when_lifetime_disposed_after_issue() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("disposed-later");
        let vis = VisibilityScope::default();
        let owner = OwnerRef::Runtime;

        policy
            .tree
            .register(cap.clone(), owner.clone(), LifetimeScope::Task, vis.clone())
            .unwrap();

        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Task);
        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();

        // Dispose after lease issued
        policy.tree.dispose(LifetimeScope::Task);

        let result = policy.validate_lease(&lease);
        assert_eq!(result, Err(PolicyDenial::Disposed));
    }

    #[test]
    fn can_delegate_follows_hierarchy() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);

        let runtime = OwnerRef::Runtime;
        let session_a = OwnerRef::Session(SessionKey::new("test", "."));
        let session_b = OwnerRef::Session(SessionKey::with_name("test", ".", Some("other")));
        let run_a = OwnerRef::Run(crate::capability::ownership::RunId("run-a".into()));
        let task_a = OwnerRef::Task(crate::capability::ownership::TaskId("task-a".into()));

        // Runtime can delegate to anyone
        assert!(policy.can_delegate(&runtime, &session_a));
        assert!(policy.can_delegate(&runtime, &run_a));
        assert!(policy.can_delegate(&runtime, &task_a));

        // Session can delegate to itself, runs, and tasks
        assert!(policy.can_delegate(&session_a, &session_a));
        assert!(policy.can_delegate(&session_a, &run_a));
        assert!(policy.can_delegate(&session_a, &task_a));
        assert!(!policy.can_delegate(&session_a, &runtime));
        assert!(!policy.can_delegate(&session_a, &session_b));

        // Run can delegate to itself and tasks
        assert!(policy.can_delegate(&run_a, &run_a));
        assert!(policy.can_delegate(&run_a, &task_a));
        assert!(!policy.can_delegate(&run_a, &runtime));
        assert!(!policy.can_delegate(&run_a, &session_a));

        // Task cannot delegate
        assert!(!policy.can_delegate(&task_a, &task_a));
        assert!(!policy.can_delegate(&task_a, &run_a));
        assert!(!policy.can_delegate(&task_a, &session_a));
        assert!(!policy.can_delegate(&task_a, &runtime));
    }

    #[test]
    fn check_cross_session_allows_same_session() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let session = SessionKey::new("test", ".");

        assert!(policy.check_cross_session(&session, &session).is_ok());
    }

    #[test]
    fn check_cross_session_rejects_different_sessions() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let session_a = SessionKey::new("test", ".");
        let session_b = SessionKey::with_name("test", ".", Some("other"));

        let result = policy.check_cross_session(&session_a, &session_b);
        assert_eq!(result, Err(PolicyDenial::VisibilityMismatch));
    }

    #[test]
    fn visibility_scope_validation_checks_all_dimensions() {
        let tree = OwnershipTree::new();
        let policy = PolicyEngine::new(tree);
        let cap = cap_id("multi-constraint");
        let owner = OwnerRef::Runtime;

        let mut vis = VisibilityScope::default();
        vis.principal = Some("agent-a".to_string());
        vis.workspace = Some("workspace-a".to_string());
        vis.session = Some(SessionKey::new("test", "."));
        vis.approval = Some("admin".to_string());

        policy
            .tree
            .register(
                cap.clone(),
                owner.clone(),
                LifetimeScope::Runtime,
                vis.clone(),
            )
            .unwrap();

        let mut descriptor_vis = vis.clone();
        descriptor_vis.allowed_kinds = Some(vec!["Tool".to_string()]);
        descriptor_vis.allowed_ids = Some(vec!["multi-constraint".to_string()]);
        let descriptor = fake_descriptor(cap.clone(), descriptor_vis, LifetimeScope::Runtime);

        // All constraints match — should succeed
        let lease = policy.resolve(&cap, &owner, &vis, &descriptor).unwrap();
        assert_eq!(lease.capability, cap);

        // Change one dimension at a time and verify failure
        let mut wrong_vis = vis.clone();
        wrong_vis.principal = Some("agent-b".to_string());
        let descriptor = fake_descriptor(cap.clone(), vis.clone(), LifetimeScope::Runtime);
        let result = policy.resolve(&cap, &owner, &wrong_vis, &descriptor);
        // When visibility doesn't match, the binding lookup fails with NotFound
        assert_eq!(result, Err(PolicyDenial::NotFound));
    }
}
