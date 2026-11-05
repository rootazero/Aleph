//! Cross-session capability grants — delegation, revocation cascades, and
//! session transitions.
//!
//! A **grant** enables one session to delegate a capability to another session.
//! The grantee receives a transitive capability reference that respects:
//!
//! - **Authority limits**: A grant cannot exceed the grantor's own authority.
//! - **Delegation chains**: Tracked for length validation and cycle detection.
//! - **Revocation cascades**: Revoking a grant also revokes all downstream grants.
//! - **Session transitions**: Grants persist across session epochs with proper
//!   migration logic.
//!
//! ## Architecture
//!
//! Grants are stored in the [`GrantRegistry`], which maintains:
//! - Active grants indexed by grantee session
//! - Delegation chain metadata for cycle/length validation
//! - Revocation state for fail-closed enforcement
//!
//! Grant validation happens at resolve time through [`OwnershipTree`], ensuring
//! that a grantee's lease is valid only when:
//! 1. The grant exists and is active
//! 2. The grantor's own binding remains valid
//! 3. No ancestor grant in the chain has been revoked
//! 4. Chain length does not exceed the configured limit

use crate::acp::manager::SessionKey;
use crate::capability::descriptor::CapabilityId;
use crate::capability::ownership::{OwnerGeneration, OwnerRef, VisibilityScope};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// A unique identifier for a capability grant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GrantId(String);

impl GrantId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A capability grant: permission for one session to use a capability owned by
/// another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// Unique identifier for this grant.
    pub id: GrantId,
    /// The capability being granted.
    pub capability: CapabilityId,
    /// Visibility scope of the grant (which contexts can see it).
    pub visibility: VisibilityScope,
    /// The session that granted this capability (the owner).
    pub grantor: SessionKey,
    /// The session receiving the grant.
    pub grantee: SessionKey,
    /// The generation at which this grant was issued. Used to detect when the
    /// grantor's own binding has been invalidated.
    pub generation: OwnerGeneration,
    /// The grant this one was derived from, if any (delegation chain parent).
    pub parent_grant: Option<GrantId>,
    /// Metadata about the grant (creation time, delegation depth, etc.).
    pub metadata: GrantMetadata,
}

/// Metadata attached to a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantMetadata {
    /// When the grant was created (Unix timestamp in seconds).
    pub created_at: u64,
    /// Depth in the delegation chain (0 = direct grant from owner).
    pub delegation_depth: u32,
    /// Optional expiration time (Unix timestamp in seconds).
    pub expires_at: Option<u64>,
    /// Optional human-readable label or purpose.
    pub label: Option<String>,
}

/// Why a grant operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    /// The grant already exists with the same id.
    #[error("grant {0} already exists")]
    AlreadyExists(String),
    /// The grant does not exist.
    #[error("grant {0} not found")]
    NotFound(String),
    /// The grantor does not have authority to grant this capability.
    #[error("grantor lacks authority to grant capability {0}")]
    NoAuthority(String),
    /// The delegation chain would exceed the maximum allowed depth.
    #[error("delegation chain exceeds maximum depth {0}")]
    ChainTooLong(u32),
    /// Creating this grant would create a cycle in the delegation chain.
    #[error("grant would create a delegation cycle")]
    CycleDetected,
    /// The grantor's own binding has been invalidated (generation mismatch).
    #[error("grantor binding invalidated (generation mismatch)")]
    GrantorInvalidated,
    /// The grant has been revoked.
    #[error("grant {0} has been revoked")]
    Revoked(String),
}

/// A grant change notification emitted by [`GrantRegistry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantChange {
    /// A new grant was created.
    Created { grant: Grant },
    /// A grant was revoked (and all its descendants).
    Revoked { grant_id: GrantId },
}

/// Maximum delegation chain depth (configurable, default 5).
const MAX_DELEGATION_DEPTH: u32 = 5;

/// Internal state of the grant registry.
#[derive(Debug)]
struct GrantRegistryInner {
    /// All active grants indexed by grant id.
    grants: HashMap<GrantId, Grant>,
    /// Grants indexed by grantee session for fast lookup.
    by_grantee: HashMap<SessionKey, HashSet<GrantId>>,
    /// Grants indexed by grantor session.
    by_grantor: HashMap<SessionKey, HashSet<GrantId>>,
    /// Child grants for each parent (delegation chain structure).
    children: HashMap<GrantId, HashSet<GrantId>>,
    /// Revoked grant ids (immutable once revoked).
    revoked: HashSet<GrantId>,
    /// Change notification broadcast.
    changes: broadcast::Sender<GrantChange>,
}

/// Registry for cross-session capability grants.
///
/// Maintains grant state, delegation chains, and enforces revocation cascades.
#[derive(Debug)]
pub struct GrantRegistry {
    inner: Mutex<GrantRegistryInner>,
    max_depth: u32,
}

impl Default for GrantRegistry {
    fn default() -> Self {
        Self::new(MAX_DELEGATION_DEPTH)
    }
}

impl GrantRegistry {
    /// Create a new grant registry with the specified maximum delegation depth.
    #[must_use]
    pub fn new(max_depth: u32) -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            inner: Mutex::new(GrantRegistryInner {
                grants: HashMap::new(),
                by_grantee: HashMap::new(),
                by_grantor: HashMap::new(),
                children: HashMap::new(),
                revoked: HashSet::new(),
                changes,
            }),
            max_depth,
        }
    }

    /// Subscribe to grant change notifications.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<GrantChange> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .changes
            .subscribe()
    }

    /// Create a new grant.
    ///
    /// Validates:
    /// - Grant id is unique
    /// - Delegation depth does not exceed max
    /// - No cycles would be created
    /// - Parent grant exists if specified
    pub fn create_grant(&self, grant: Grant) -> Result<(), GrantError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Check if already exists
        if inner.grants.contains_key(&grant.id) {
            return Err(GrantError::AlreadyExists(grant.id.0.clone()));
        }

        // Check if already revoked (permanent state)
        if inner.revoked.contains(&grant.id) {
            return Err(GrantError::Revoked(grant.id.0.clone()));
        }

        // Validate delegation depth
        if grant.metadata.delegation_depth > self.max_depth {
            return Err(GrantError::ChainTooLong(self.max_depth));
        }

        // Validate parent grant exists if specified
        if let Some(ref parent_id) = grant.parent_grant {
            if !inner.grants.contains_key(parent_id) {
                return Err(GrantError::NotFound(parent_id.0.clone()));
            }
            // Check parent is not revoked
            if inner.revoked.contains(parent_id) {
                return Err(GrantError::Revoked(parent_id.0.clone()));
            }
        }

        // Detect cycles: check if grantee already appears in the ancestor chain
        if let Some(ref parent_id) = grant.parent_grant {
            if self.would_create_cycle(&inner, parent_id, &grant.grantee) {
                return Err(GrantError::CycleDetected);
            }
        }

        // Insert grant
        let grant_id = grant.id.clone();
        let grantee = grant.grantee.clone();
        let grantor = grant.grantor.clone();
        let parent = grant.parent_grant.clone();

        inner.grants.insert(grant_id.clone(), grant.clone());
        inner
            .by_grantee
            .entry(grantee)
            .or_default()
            .insert(grant_id.clone());
        inner
            .by_grantor
            .entry(grantor)
            .or_default()
            .insert(grant_id.clone());

        // Record parent-child relationship
        if let Some(parent_id) = parent {
            inner
                .children
                .entry(parent_id)
                .or_default()
                .insert(grant_id.clone());
        }

        // Emit change notification
        let _ = inner.changes.send(GrantChange::Created { grant });

        Ok(())
    }

    /// Check if adding a grant from `parent_id` to `new_grantee` would create
    /// a cycle in the delegation chain.
    fn would_create_cycle(
        &self,
        inner: &GrantRegistryInner,
        parent_id: &GrantId,
        new_grantee: &SessionKey,
    ) -> bool {
        // Walk up the ancestor chain and check if new_grantee appears as a grantor
        let mut current = Some(parent_id);
        let mut visited = HashSet::new();

        while let Some(grant_id) = current {
            if !visited.insert(grant_id) {
                // Already visited this node (shouldn't happen in valid state)
                return true;
            }

            if let Some(grant) = inner.grants.get(grant_id) {
                // If the grantor of this ancestor is the new grantee, we have a cycle
                if &grant.grantor == new_grantee {
                    return true;
                }
                // Move up to the parent
                current = grant.parent_grant.as_ref();
            } else {
                break;
            }
        }

        false
    }

    /// Revoke a grant and all its descendants (cascade).
    ///
    /// Once revoked, a grant id cannot be reused (immutable revocation state).
    pub fn revoke_grant(&self, grant_id: &GrantId) -> Result<(), GrantError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Check grant exists
        let grant = inner
            .grants
            .get(grant_id)
            .ok_or_else(|| GrantError::NotFound(grant_id.0.clone()))?
            .clone();

        // Collect all descendants (BFS)
        let mut to_revoke = VecDeque::new();
        to_revoke.push_back(grant_id.clone());
        let mut all_revoked = HashSet::new();

        while let Some(current_id) = to_revoke.pop_front() {
            if !all_revoked.insert(current_id.clone()) {
                continue; // Already processed
            }

            // Add children to the queue
            if let Some(children) = inner.children.get(&current_id) {
                for child_id in children {
                    to_revoke.push_back(child_id.clone());
                }
            }
        }

        // Remove all grants from the registry and mark as revoked
        for id in &all_revoked {
            if let Some(revoked_grant) = inner.grants.remove(id) {
                // Remove from indexes
                if let Some(grantee_set) = inner.by_grantee.get_mut(&revoked_grant.grantee) {
                    grantee_set.remove(id);
                }
                if let Some(grantor_set) = inner.by_grantor.get_mut(&revoked_grant.grantor) {
                    grantor_set.remove(id);
                }
                if let Some(parent_id) = &revoked_grant.parent_grant {
                    if let Some(children) = inner.children.get_mut(parent_id) {
                        children.remove(id);
                    }
                }
                inner.children.remove(id);
            }
            // Mark as permanently revoked
            inner.revoked.insert(id.clone());
        }

        // Emit change notification for the root grant
        let _ = inner.changes.send(GrantChange::Revoked {
            grant_id: grant_id.clone(),
        });

        Ok(())
    }

    /// Get a grant by id.
    #[must_use]
    pub fn get_grant(&self, grant_id: &GrantId) -> Option<Grant> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grants
            .get(grant_id)
            .cloned()
    }

    /// Get all active grants for a grantee session.
    #[must_use]
    pub fn grants_for_grantee(&self, grantee: &SessionKey) -> Vec<Grant> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        inner
            .by_grantee
            .get(grantee)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| inner.grants.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all active grants where the session is the grantor.
    #[must_use]
    pub fn grants_by_grantor(&self, grantor: &SessionKey) -> Vec<Grant> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        inner
            .by_grantor
            .get(grantor)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| inner.grants.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if a grant id has been revoked (permanently).
    #[must_use]
    pub fn is_revoked(&self, grant_id: &GrantId) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revoked
            .contains(grant_id)
    }

    /// Validate that a grant chain is still valid given the current ownership
    /// state.
    ///
    /// Returns `Ok(())` if:
    /// - The grant exists and is not revoked
    /// - All ancestor grants exist and are not revoked
    /// - The grantor's generation matches the grant's recorded generation
    pub fn validate_grant_chain(
        &self,
        grant_id: &GrantId,
        current_generation: OwnerGeneration,
    ) -> Result<(), GrantError> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Check grant exists
        let grant = inner
            .grants
            .get(grant_id)
            .ok_or_else(|| GrantError::NotFound(grant_id.0.clone()))?;

        // Check not revoked
        if inner.revoked.contains(grant_id) {
            return Err(GrantError::Revoked(grant_id.0.clone()));
        }

        // Check generation matches
        if grant.generation != current_generation {
            return Err(GrantError::GrantorInvalidated);
        }

        // Validate ancestor chain
        let mut current = grant.parent_grant.as_ref();
        while let Some(parent_id) = current {
            if inner.revoked.contains(parent_id) {
                return Err(GrantError::Revoked(parent_id.0.clone()));
            }
            let parent = inner
                .grants
                .get(parent_id)
                .ok_or_else(|| GrantError::NotFound(parent_id.0.clone()))?;
            current = parent.parent_grant.as_ref();
        }

        Ok(())
    }

    /// Migrate grants when a session transitions to a new epoch.
    ///
    /// Updates all grants where the session is the grantor or grantee to use
    /// the new session key and generation.
    pub fn migrate_session(
        &self,
        old_session: &SessionKey,
        new_session: SessionKey,
        new_generation: OwnerGeneration,
    ) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut updated_grants = Vec::new();

        // Find all grants involving the old session
        let grant_ids: HashSet<GrantId> = inner
            .by_grantee
            .get(old_session)
            .cloned()
            .unwrap_or_default()
            .union(
                &inner
                    .by_grantor
                    .get(old_session)
                    .cloned()
                    .unwrap_or_default(),
            )
            .cloned()
            .collect();

        // Update each grant
        for grant_id in grant_ids {
            if let Some(mut grant) = inner.grants.remove(&grant_id) {
                let old_grantee = grant.grantee.clone();
                let old_grantor = grant.grantor.clone();

                // Update session keys
                if &grant.grantee == old_session {
                    grant.grantee = new_session.clone();
                }
                if &grant.grantor == old_session {
                    grant.grantor = new_session.clone();
                    grant.generation = new_generation;
                }

                // Update indexes
                if old_grantee == *old_session {
                    if let Some(set) = inner.by_grantee.get_mut(&old_grantee) {
                        set.remove(&grant_id);
                    }
                    inner
                        .by_grantee
                        .entry(grant.grantee.clone())
                        .or_default()
                        .insert(grant_id.clone());
                }
                if old_grantor == *old_session {
                    if let Some(set) = inner.by_grantor.get_mut(&old_grantor) {
                        set.remove(&grant_id);
                    }
                    inner
                        .by_grantor
                        .entry(grant.grantor.clone())
                        .or_default()
                        .insert(grant_id.clone());
                }

                updated_grants.push(grant.clone());
                inner.grants.insert(grant_id, grant);
            }
        }
    }
}

impl Default for GrantMetadata {
    fn default() -> Self {
        Self {
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            delegation_depth: 0,
            expires_at: None,
            label: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session(id: u64) -> SessionKey {
        SessionKey::new(format!("session-{id}"))
    }

    fn test_capability(name: &str) -> CapabilityId {
        CapabilityId::new(name.to_string())
    }

    fn test_grant(
        id: &str,
        capability: &str,
        grantor: SessionKey,
        grantee: SessionKey,
        parent: Option<GrantId>,
        depth: u32,
    ) -> Grant {
        Grant {
            id: GrantId::new(id),
            capability: test_capability(capability),
            visibility: VisibilityScope::default(),
            grantor,
            grantee,
            generation: OwnerGeneration(1),
            parent_grant: parent,
            metadata: GrantMetadata {
                delegation_depth: depth,
                ..Default::default()
            },
        }
    }

    #[test]
    fn creates_grant_successfully() {
        let registry = GrantRegistry::default();
        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );

        assert!(registry.create_grant(grant.clone()).is_ok());
        assert_eq!(registry.get_grant(&grant.id), Some(grant));
    }

    #[test]
    fn rejects_duplicate_grant_id() {
        let registry = GrantRegistry::default();
        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );

        assert!(registry.create_grant(grant.clone()).is_ok());
        assert!(matches!(
            registry.create_grant(grant),
            Err(GrantError::AlreadyExists(_))
        ));
    }

    #[test]
    fn rejects_grant_exceeding_max_depth() {
        let registry = GrantRegistry::new(2);
        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            3, // exceeds max depth of 2
        );

        assert!(matches!(
            registry.create_grant(grant),
            Err(GrantError::ChainTooLong(2))
        ));
    }

    #[test]
    fn detects_delegation_cycles() {
        let registry = GrantRegistry::default();

        // Session 1 grants to Session 2
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );
        assert!(registry.create_grant(grant1).is_ok());

        // Session 2 grants to Session 3
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            test_session(2),
            test_session(3),
            Some(GrantId::new("grant-1")),
            1,
        );
        assert!(registry.create_grant(grant2).is_ok());

        // Session 3 trying to grant back to Session 1 would create a cycle
        let grant3 = test_grant(
            "grant-3",
            "cap-1",
            test_session(3),
            test_session(1), // cycle here
            Some(GrantId::new("grant-2")),
            2,
        );
        assert!(matches!(
            registry.create_grant(grant3),
            Err(GrantError::CycleDetected)
        ));
    }

    #[test]
    fn revokes_grant_and_descendants() {
        let registry = GrantRegistry::default();

        // Create chain: grant1 -> grant2 -> grant3
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            test_session(2),
            test_session(3),
            Some(GrantId::new("grant-1")),
            1,
        );
        let grant3 = test_grant(
            "grant-3",
            "cap-1",
            test_session(3),
            test_session(4),
            Some(GrantId::new("grant-2")),
            2,
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());
        assert!(registry.create_grant(grant3.clone()).is_ok());

        // Revoke grant1 — should cascade to grant2 and grant3
        assert!(registry.revoke_grant(&grant1.id).is_ok());

        // All should be gone
        assert!(registry.get_grant(&grant1.id).is_none());
        assert!(registry.get_grant(&grant2.id).is_none());
        assert!(registry.get_grant(&grant3.id).is_none());

        // All should be marked as revoked
        assert!(registry.is_revoked(&grant1.id));
        assert!(registry.is_revoked(&grant2.id));
        assert!(registry.is_revoked(&grant3.id));
    }

    #[test]
    fn revoked_grant_id_cannot_be_reused() {
        let registry = GrantRegistry::default();
        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );

        assert!(registry.create_grant(grant.clone()).is_ok());
        assert!(registry.revoke_grant(&grant.id).is_ok());

        // Try to create a new grant with the same id
        let new_grant = test_grant(
            "grant-1",
            "cap-2",
            test_session(3),
            test_session(4),
            None,
            0,
        );
        assert!(matches!(
            registry.create_grant(new_grant),
            Err(GrantError::Revoked(_))
        ));
    }

    #[test]
    fn lists_grants_by_grantee() {
        let registry = GrantRegistry::default();
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-2",
            test_session(3),
            test_session(2),
            None,
            0,
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());

        let grants = registry.grants_for_grantee(&test_session(2));
        assert_eq!(grants.len(), 2);
        assert!(grants.contains(&grant1));
        assert!(grants.contains(&grant2));
    }

    #[test]
    fn lists_grants_by_grantor() {
        let registry = GrantRegistry::default();
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-2",
            test_session(1),
            test_session(3),
            None,
            0,
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());

        let grants = registry.grants_by_grantor(&test_session(1));
        assert_eq!(grants.len(), 2);
        assert!(grants.contains(&grant1));
        assert!(grants.contains(&grant2));
    }

    #[test]
    fn validates_grant_chain_with_matching_generation() {
        let registry = GrantRegistry::default();
        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );

        assert!(registry.create_grant(grant.clone()).is_ok());
        assert!(registry
            .validate_grant_chain(&grant.id, OwnerGeneration(1))
            .is_ok());
    }

    #[test]
    fn rejects_grant_with_mismatched_generation() {
        let registry = GrantRegistry::default();
        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
        );

        assert!(registry.create_grant(grant.clone()).is_ok());
        assert!(matches!(
            registry.validate_grant_chain(&grant.id, OwnerGeneration(2)),
            Err(GrantError::GrantorInvalidated)
        ));
    }

    #[test]
    fn migrates_session_grants() {
        let registry = GrantRegistry::default();
        let old_session = test_session(1);
        let new_session = test_session(99);

        // Create grants with old session as grantor and grantee
        let grant1 = test_grant("grant-1", "cap-1", old_session.clone(), test_session(2), None, 0);
        let grant2 = test_grant("grant-2", "cap-2", test_session(3), old_session.clone(), None, 0);

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());

        // Migrate session
        registry.migrate_session(&old_session, new_session.clone(), OwnerGeneration(5));

        // Check grants were updated
        let updated_grant1 = registry.get_grant(&grant1.id).unwrap();
        assert_eq!(updated_grant1.grantor, new_session);
        assert_eq!(updated_grant1.generation, OwnerGeneration(5));

        let updated_grant2 = registry.get_grant(&grant2.id).unwrap();
        assert_eq!(updated_grant2.grantee, new_session);

        // Check indexes were updated
        assert_eq!(registry.grants_by_grantor(&new_session).len(), 1);
        assert_eq!(registry.grants_for_grantee(&new_session).len(), 1);
        assert!(registry.grants_by_grantor(&old_session).is_empty());
        assert!(registry.grants_for_grantee(&old_session).is_empty());
    }
}
