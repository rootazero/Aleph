//! Integration tests for cross-session capability grants.
//!
//! These tests verify the complete grant lifecycle including:
//! - Grant creation and validation
//! - Delegation chains with depth limits
//! - Revocation cascades
//! - Session transitions and epoch handling
//! - Integration with OwnershipTree

#[cfg(test)]
mod tests {
    use crate::acp::manager::SessionKey;
    use crate::capability::descriptor::CapabilityId;
    use crate::capability::grant::{
        Grant, GrantError, GrantId, GrantMetadata, GrantRegistry,
    };
    use crate::capability::ownership::{
        LifetimeScope, OwnerGeneration, OwnerRef, OwnershipTree, VisibilityScope,
    };

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
        generation: OwnerGeneration,
    ) -> Grant {
        Grant {
            id: GrantId::new(id),
            capability: test_capability(capability),
            visibility: VisibilityScope::default(),
            grantor,
            grantee,
            generation,
            parent_grant: parent,
            metadata: GrantMetadata {
                delegation_depth: depth,
                ..Default::default()
            },
        }
    }

    #[test]
    fn grant_lifecycle_with_ownership_integration() {
        let ownership = OwnershipTree::new();
        let registry = GrantRegistry::default();

        let cap_id = test_capability("test-cap");
        let session1 = test_session(1);
        let session2 = test_session(2);

        // Session 1 registers a capability
        let result = ownership.register(
            &cap_id,
            &VisibilityScope::default(),
            OwnerRef::Session(session1.clone()),
            LifetimeScope::Session,
        );
        assert!(result.is_ok());

        // Get the generation
        let generation = ownership
            .generation(&cap_id, &VisibilityScope::default())
            .expect("capability should be registered");

        // Session 1 grants to Session 2
        let grant = test_grant(
            "grant-1",
            "test-cap",
            session1.clone(),
            session2.clone(),
            None,
            0,
            generation,
        );

        assert!(registry.create_grant(grant.clone()).is_ok());

        // Validate grant chain
        assert!(registry.validate_grant_chain(&grant.id, generation).is_ok());

        // Session 1 bumps its ownership (invalidates grant)
        assert!(ownership
            .bump(LifetimeScope::Session, OwnerRef::Session(session1.clone()))
            .is_ok());

        let new_generation = ownership
            .generation(&cap_id, &VisibilityScope::default())
            .expect("capability should still be registered");

        // Grant validation should fail with generation mismatch
        assert!(matches!(
            registry.validate_grant_chain(&grant.id, new_generation),
            Err(GrantError::GrantorInvalidated)
        ));
    }

    #[test]
    fn delegation_chain_enforces_depth_limit() {
        let registry = GrantRegistry::new(3);

        let session1 = test_session(1);
        let session2 = test_session(2);
        let session3 = test_session(3);
        let session4 = test_session(4);
        let session5 = test_session(5);

        // Create chain: 1 -> 2 -> 3 -> 4
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            session1,
            session2.clone(),
            None,
            0,
            OwnerGeneration(1),
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            session2,
            session3.clone(),
            Some(GrantId::new("grant-1")),
            1,
            OwnerGeneration(1),
        );
        let grant3 = test_grant(
            "grant-3",
            "cap-1",
            session3,
            session4.clone(),
            Some(GrantId::new("grant-2")),
            2,
            OwnerGeneration(1),
        );
        let grant4 = test_grant(
            "grant-4",
            "cap-1",
            session4,
            session5,
            Some(GrantId::new("grant-3")),
            3,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant1).is_ok());
        assert!(registry.create_grant(grant2).is_ok());
        assert!(registry.create_grant(grant3).is_ok());

        // Depth 3 should be accepted (at limit)
        assert!(registry.create_grant(grant4.clone()).is_ok());

        // Trying to extend further should fail
        let grant5 = test_grant(
            "grant-5",
            "cap-1",
            test_session(5),
            test_session(6),
            Some(GrantId::new("grant-4")),
            4, // exceeds max depth of 3
            OwnerGeneration(1),
        );

        assert!(matches!(
            registry.create_grant(grant5),
            Err(GrantError::ChainTooLong(3))
        ));
    }

    #[test]
    fn revocation_cascade_removes_all_descendants() {
        let registry = GrantRegistry::default();

        // Create a tree:
        //       grant1
        //      /      \
        //   grant2   grant3
        //     |
        //   grant4

        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
            OwnerGeneration(1),
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            test_session(2),
            test_session(3),
            Some(GrantId::new("grant-1")),
            1,
            OwnerGeneration(1),
        );
        let grant3 = test_grant(
            "grant-3",
            "cap-1",
            test_session(2),
            test_session(4),
            Some(GrantId::new("grant-1")),
            1,
            OwnerGeneration(1),
        );
        let grant4 = test_grant(
            "grant-4",
            "cap-1",
            test_session(3),
            test_session(5),
            Some(GrantId::new("grant-2")),
            2,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());
        assert!(registry.create_grant(grant3.clone()).is_ok());
        assert!(registry.create_grant(grant4.clone()).is_ok());

        // Subscribe to changes
        let mut changes = registry.subscribe();

        // Revoke grant1 - should cascade to all descendants
        assert!(registry.revoke_grant(&grant1.id).is_ok());

        // Verify all grants are gone
        assert!(registry.get_grant(&grant1.id).is_none());
        assert!(registry.get_grant(&grant2.id).is_none());
        assert!(registry.get_grant(&grant3.id).is_none());
        assert!(registry.get_grant(&grant4.id).is_none());

        // Verify all are marked as revoked
        assert!(registry.is_revoked(&grant1.id));
        assert!(registry.is_revoked(&grant2.id));
        assert!(registry.is_revoked(&grant3.id));
        assert!(registry.is_revoked(&grant4.id));

        // Verify revocation notification was sent
        let change = changes.try_recv().expect("should receive change");
        assert!(matches!(
            change,
            crate::capability::grant::GrantChange::Revoked { .. }
        ));

        // Verify revoked grant id cannot be reused
        let new_grant = test_grant(
            "grant-1",
            "cap-2",
            test_session(10),
            test_session(11),
            None,
            0,
            OwnerGeneration(1),
        );
        assert!(matches!(
            registry.create_grant(new_grant),
            Err(GrantError::Revoked(_))
        ));
    }

    #[test]
    fn session_migration_updates_grants() {
        let registry = GrantRegistry::default();
        let old_session = test_session(1);
        let new_session = test_session(99);

        // Create grants where old_session is grantor and grantee
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            old_session.clone(),
            test_session(2),
            None,
            0,
            OwnerGeneration(1),
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-2",
            test_session(3),
            old_session.clone(),
            None,
            0,
            OwnerGeneration(1),
        );
        let grant3 = test_grant(
            "grant-3",
            "cap-3",
            old_session.clone(),
            old_session.clone(),
            None,
            0,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());
        assert!(registry.create_grant(grant3.clone()).is_ok());

        // Migrate session
        registry.migrate_session(&old_session, new_session.clone(), OwnerGeneration(5));

        // Verify grants were updated
        let updated_grant1 = registry.get_grant(&grant1.id).unwrap();
        assert_eq!(updated_grant1.grantor, new_session);
        assert_eq!(updated_grant1.generation, OwnerGeneration(5));
        assert_eq!(updated_grant1.grantee, test_session(2)); // unchanged

        let updated_grant2 = registry.get_grant(&grant2.id).unwrap();
        assert_eq!(updated_grant2.grantor, test_session(3)); // unchanged
        assert_eq!(updated_grant2.grantee, new_session);

        let updated_grant3 = registry.get_grant(&grant3.id).unwrap();
        assert_eq!(updated_grant3.grantor, new_session);
        assert_eq!(updated_grant3.grantee, new_session);
        assert_eq!(updated_grant3.generation, OwnerGeneration(5));

        // Verify indexes were updated
        let grants_by_grantor = registry.grants_by_grantor(&new_session);
        assert_eq!(grants_by_grantor.len(), 2); // grant1 and grant3

        let grants_for_grantee = registry.grants_for_grantee(&new_session);
        assert_eq!(grants_for_grantee.len(), 2); // grant2 and grant3

        // Old session should have no grants
        assert!(registry.grants_by_grantor(&old_session).is_empty());
        assert!(registry.grants_for_grantee(&old_session).is_empty());
    }

    #[test]
    fn cycle_detection_prevents_circular_delegation() {
        let registry = GrantRegistry::default();

        // Create chain: 1 -> 2 -> 3
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
            OwnerGeneration(1),
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            test_session(2),
            test_session(3),
            Some(GrantId::new("grant-1")),
            1,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant1).is_ok());
        assert!(registry.create_grant(grant2).is_ok());

        // Try to create grant from 3 back to 1 (creates cycle)
        let grant3 = test_grant(
            "grant-3",
            "cap-1",
            test_session(3),
            test_session(1), // cycle!
            Some(GrantId::new("grant-2")),
            2,
            OwnerGeneration(1),
        );

        assert!(matches!(
            registry.create_grant(grant3),
            Err(GrantError::CycleDetected)
        ));

        // Try to create grant from 3 back to 2 (also a cycle)
        let grant4 = test_grant(
            "grant-4",
            "cap-1",
            test_session(3),
            test_session(2), // cycle!
            Some(GrantId::new("grant-2")),
            2,
            OwnerGeneration(1),
        );

        assert!(matches!(
            registry.create_grant(grant4),
            Err(GrantError::CycleDetected)
        ));
    }

    #[test]
    fn grant_validation_checks_entire_chain() {
        let registry = GrantRegistry::default();

        // Create chain: 1 -> 2 -> 3
        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
            OwnerGeneration(1),
        );
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            test_session(2),
            test_session(3),
            Some(GrantId::new("grant-1")),
            1,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.create_grant(grant2.clone()).is_ok());

        // Validate grant2 - should check entire chain
        assert!(registry
            .validate_grant_chain(&grant2.id, OwnerGeneration(1))
            .is_ok());

        // Revoke grant1 (parent)
        assert!(registry.revoke_grant(&grant1.id).is_ok());

        // Validation of grant2 should fail because parent is revoked
        // (even though grant2 itself would pass)
        assert!(matches!(
            registry.validate_grant_chain(&grant2.id, OwnerGeneration(1)),
            Err(GrantError::NotFound(_))
        ));
    }

    #[test]
    fn concurrent_grant_operations() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(GrantRegistry::default());
        let mut handles = vec![];

        // Spawn multiple threads creating grants concurrently
        for i in 0..10 {
            let registry = Arc::clone(&registry);
            let handle = thread::spawn(move || {
                let grant = test_grant(
                    &format!("grant-{i}"),
                    "cap-1",
                    test_session(i),
                    test_session(i + 100),
                    None,
                    0,
                    OwnerGeneration(1),
                );
                registry.create_grant(grant)
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            assert!(handle.join().is_ok());
        }

        // Verify all grants were created
        for i in 0..10 {
            let grant_id = GrantId::new(format!("grant-{i}"));
            assert!(registry.get_grant(&grant_id).is_some());
        }
    }

    #[test]
    fn grant_metadata_tracks_creation_time_and_depth() {
        let registry = GrantRegistry::default();

        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let grant = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant.clone()).is_ok());

        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let retrieved = registry.get_grant(&grant.id).unwrap();
        assert!(retrieved.metadata.created_at >= before);
        assert!(retrieved.metadata.created_at <= after);
        assert_eq!(retrieved.metadata.delegation_depth, 0);
    }

    #[test]
    fn revoked_parent_prevents_child_grant_creation() {
        let registry = GrantRegistry::default();

        let grant1 = test_grant(
            "grant-1",
            "cap-1",
            test_session(1),
            test_session(2),
            None,
            0,
            OwnerGeneration(1),
        );

        assert!(registry.create_grant(grant1.clone()).is_ok());
        assert!(registry.revoke_grant(&grant1.id).is_ok());

        // Try to create child of revoked parent
        let grant2 = test_grant(
            "grant-2",
            "cap-1",
            test_session(2),
            test_session(3),
            Some(GrantId::new("grant-1")),
            1,
            OwnerGeneration(1),
        );

        assert!(matches!(
            registry.create_grant(grant2),
            Err(GrantError::Revoked(_))
        ));
    }
}
