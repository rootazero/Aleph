use super::*;
use crate::context::fabric::{
    canonical_fingerprint, ContextBudget, ContextOperation, ContextPersistencePolicy, ContextPhase,
    ContextScope, ContextSideEffectPermissions, ContextSnapshot, CONTEXT_CAPABILITY_API_VERSION,
};
use crate::extension::capability::{ContextGrant, ContextGrantApproval};
use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

const NOW: u64 = 100;

fn caller(session: &str) -> AuthenticatedCaller {
    AuthenticatedCaller::from_host_boundary(session).unwrap()
}

fn descriptor(
    id: &str,
    phase: ContextPhase,
    mode: ContextCapabilityMode,
) -> ContextCapabilityDescriptor {
    ContextCapabilityDescriptor {
        id: id.into(),
        api_version: CONTEXT_CAPABILITY_API_VERSION.into(),
        implementation_version: "1.0".into(),
        phases: vec![phase],
        mode,
        scope: ContextScope {
            namespace: ContextNamespace::Session,
            handles: vec![],
        },
        input_budget: ContextBudget::new(100, 1000),
        output_budget: ContextBudget::new(100, 1000),
        persistence: ContextPersistencePolicy::default(),
        permissions: ContextSideEffectPermissions::default(),
        lifecycle: vec![],
        bundle_id: None,
    }
}

fn approval() -> ContextGrantApproval {
    ContextGrantApproval {
        expires_at: 200,
        audit_reason: "operator approved context for session a".into(),
        ..Default::default()
    }
}

fn registration_with(
    owner: &str,
    descriptor: ContextCapabilityDescriptor,
    approval: ContextGrantApproval,
) -> ContextRegistration {
    let grant = ContextGrant::host_issue(&descriptor, owner, &caller("a"), approval, NOW).unwrap();
    ContextRegistration::new(descriptor, grant, (!owner.is_empty()).then(|| owner.into()))
}

fn registration(
    owner: &str,
    id: &str,
    phase: ContextPhase,
    mode: ContextCapabilityMode,
) -> ContextRegistration {
    registration_with(owner, descriptor(id, phase, mode), approval())
}

fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    for id in ["one", "two"] {
        registry.register_plugin(PluginRecord::new(
            id.into(),
            id.into(),
            PluginKind::Wasm,
            PluginOrigin::Config,
        ));
    }
    registry
}

fn request(id: &str, source: &str) -> ContextCapabilityRequest {
    let payload = serde_json::json!({"messages": ["hello"]});
    let fingerprint = canonical_fingerprint(&payload);
    ContextCapabilityRequest {
        request_id: "call-1".into(),
        capability_id: id.into(),
        api_version: CONTEXT_CAPABILITY_API_VERSION.into(),
        phase: ContextPhase::Compaction,
        operation: ContextOperation::Compact,
        budget: Some(ContextBudget::new(100, 1000)),
        input_budget: None,
        output_budget: None,
        input_fingerprint: Some(fingerprint.clone()),
        snapshot: Some(ContextSnapshot {
            snapshot_id: "snapshot-1".into(),
            session_id: source.into(),
            input_fingerprint: fingerprint,
            scope: ContextScope {
                namespace: ContextNamespace::Session,
                handles: vec![],
            },
            token_count: 10,
            byte_count: 30,
            payload,
        }),
    }
}

fn bundle(owner: &str, bundle_id: &str) -> Vec<ContextRegistration> {
    [
        ("compact", ContextPhase::Compaction),
        ("restore", ContextPhase::Recovery),
    ]
    .into_iter()
    .map(|(id, phase)| {
        let mut d = descriptor(id, phase, ContextCapabilityMode::Bundle);
        d.bundle_id = Some(bundle_id.into());
        registration_with(owner, d, approval())
    })
    .collect()
}

fn for_session(registration: ContextRegistration, session: &str) -> ContextRegistration {
    let grant = ContextGrant::host_issue(
        &registration.descriptor,
        &registration.plugin_id,
        &caller(session),
        approval(),
        NOW,
    )
    .unwrap();
    ContextRegistration::new(
        registration.descriptor,
        grant,
        registration.lifecycle_handle,
    )
}

#[test]
fn one_descriptor_serves_two_sessions_and_revocation_is_isolated() {
    let mut registry = registry();
    let row = registration(
        "one",
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    registry.register_context(row.clone(), NOW).unwrap();
    registry
        .register_context(for_session(row, "b"), NOW)
        .unwrap();
    assert_eq!(registry.context_stats().registrations, 1);
    assert_eq!(registry.list_context_descriptors().len(), 1);
    for session in ["a", "b"] {
        assert!(registry
            .validate_context_grant(
                "one:compact",
                &request("compact", session),
                &caller(session),
                NOW
            )
            .is_ok());
        assert_eq!(
            registry.eligible_context_keys(session, NOW),
            ["one:compact"]
        );
    }
    assert_eq!(registry.revoke_context_grants("one", "a"), 1);
    assert!(registry.eligible_context_keys("a", NOW).is_empty());
    assert!(matches!(
        registry.validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), NOW),
        Err(ContextRegistryError::Grant(
            ContextGrantError::SessionDenied
        ))
    ));
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "b"), &caller("b"), NOW)
        .is_ok());
    registry.unregister_plugin("one");
    assert!(registry.resolve_context("one:compact").is_none());
}

#[test]
fn phase_conflicts_are_scoped_to_the_invoking_session() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "one",
                "compact",
                ContextPhase::Compaction,
                ContextCapabilityMode::Replace,
            ),
            NOW,
        )
        .unwrap();
    registry
        .register_context(
            for_session(
                registration(
                    "two",
                    "compact",
                    ContextPhase::Compaction,
                    ContextCapabilityMode::Replace,
                ),
                "b",
            ),
            NOW,
        )
        .unwrap();
    assert!(matches!(
        registry.register_context(
            for_session(
                registration(
                    "one",
                    "conflict",
                    ContextPhase::Compaction,
                    ContextCapabilityMode::Replace
                ),
                "b"
            ),
            NOW
        ),
        Err(ContextRegistryError::OwnerConflict { .. })
    ));
    assert_eq!(registry.eligible_context_keys("a", NOW), ["one:compact"]);
    assert_eq!(registry.eligible_context_keys("b", NOW), ["two:compact"]);
}

#[test]
fn bundles_can_be_shared_and_revoked_per_session_atomically() {
    let mut registry = registry();
    registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .unwrap();
    let second = bundle("one", "all")
        .into_iter()
        .map(|row| for_session(row, "b"))
        .collect();
    registry.register_context_bundle(second, NOW).unwrap();
    assert_eq!(registry.context_stats().registrations, 2);
    assert_eq!(
        registry.unregister_context_bundle_for_session("one", "all", "a"),
        2
    );
    assert!(registry.eligible_context_keys("a", NOW).is_empty());
    assert_eq!(
        registry.eligible_context_keys("b", NOW),
        ["one:compact", "one:restore"]
    );
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "b"), &caller("b"), NOW)
        .is_ok());
    assert_eq!(
        registry.unregister_context_bundle_for_session("one", "all", "b"),
        2
    );
    assert_eq!(registry.context_stats().registrations, 0);
}

#[test]
fn audit_retention_is_bounded_but_total_and_last_decision_are_preserved() {
    let mut registry = registry();
    for _ in 0..(CONTEXT_AUDIT_CAP + 17) {
        assert!(matches!(
            registry.validate_context_grant(
                "one:missing",
                &request("missing", "a"),
                &caller("a"),
                NOW
            ),
            Err(ContextRegistryError::NotFound(_))
        ));
    }
    let stats = registry.context_stats();
    assert_eq!(stats.audit_retained_events, CONTEXT_AUDIT_CAP);
    assert_eq!(stats.audit_total_events, (CONTEXT_AUDIT_CAP + 17) as u64);
    let audit = registry.context_audit();
    assert_eq!(audit.len(), CONTEXT_AUDIT_CAP);
    let last = audit.last().unwrap();
    assert!(!last.allowed);
    assert_eq!(last.component_key, "one:missing");
    assert_eq!(last.session_id.as_deref(), Some("a"));
    assert_eq!(last.action, "invoke");
    registry.clear();
    assert_eq!(registry.context_stats().audit_total_events, 0);
    assert!(registry.context_audit().is_empty());
}

#[test]
fn default_approval_has_no_valid_grant_or_elevated_permissions() {
    let d = descriptor(
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    let denied = ContextGrantApproval::default();
    assert_eq!(denied.scope.namespace, ContextNamespace::Session);
    assert!(denied.scope.handles.is_empty());
    assert!(!denied.cross_session && !denied.egress && !denied.project_write);
    assert_eq!(
        ContextGrant::host_issue(&d, "one", &caller("a"), denied, NOW),
        Err(ContextGrantError::MissingAuditReason)
    );
    let missing_expiry = ContextGrantApproval {
        audit_reason: "approved".into(),
        ..Default::default()
    };
    assert!(matches!(
        ContextGrant::host_issue(&d, "one", &caller("a"), missing_expiry, NOW),
        Err(ContextGrantError::Expired { expires_at: 0 })
    ));
}

#[test]
fn descriptor_cannot_replace_host_grant_or_change_after_approval() {
    let mut registry = registry();
    let mut r = registration(
        "one",
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    r.descriptor.phases.push(ContextPhase::Recovery);
    assert_eq!(
        registry.register_context(r, NOW),
        Err(ContextRegistryError::Grant(
            ContextGrantError::DescriptorChanged
        ))
    );
    assert_eq!(registry.context_stats().registrations, 0);
}

#[test]
fn default_grant_binds_authenticated_session_even_if_snapshot_claims_own_session() {
    let mut registry = registry();
    let r = registration(
        "one",
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    assert_eq!(r.key(), "one:compact");
    registry.register_context(r, NOW).unwrap();
    assert!(registry.resolve_context("compact").is_none());
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), NOW)
        .is_ok());
    assert!(matches!(
        registry.validate_context_grant("one:compact", &request("compact", "b"), &caller("b"), NOW),
        Err(ContextRegistryError::Grant(
            ContextGrantError::SessionDenied
        ))
    ));
    assert!(matches!(
        registry.validate_context_grant("one:compact", &request("compact", "b"), &caller("a"), NOW),
        Err(ContextRegistryError::Grant(
            ContextGrantError::CrossSessionDenied
        ))
    ));
    let audit_events = registry.context_audit();
    let audit = audit_events.last().unwrap();
    assert!(!audit.allowed);
    assert_eq!(audit.action, "invoke");
    assert_eq!(
        audit.audit_reason.as_deref(),
        Some("operator approved context for session a")
    );
}

#[test]
fn project_scope_and_handles_require_explicit_host_approval() {
    let mut registry = registry();
    let mut d = descriptor(
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    d.scope = ContextScope {
        namespace: ContextNamespace::Project,
        handles: vec!["project-x".into(), "project-y".into()],
    };
    // Declaration alone cannot widen the default host grant.
    registry
        .register_context(registration_with("one", d.clone(), approval()), NOW)
        .unwrap();
    let mut call = request("compact", "a");
    call.snapshot.as_mut().unwrap().scope = ContextScope {
        namespace: ContextNamespace::Project,
        handles: vec!["project-x".into()],
    };
    assert!(matches!(
        registry.validate_context_grant("one:compact", &call, &caller("a"), NOW),
        Err(ContextRegistryError::Grant(
            ContextGrantError::ScopeEscalation
        ))
    ));
    registry.unregister_plugin("one");
    registry.register_plugin(PluginRecord::new(
        "one".into(),
        "one".into(),
        PluginKind::Wasm,
        PluginOrigin::Config,
    ));
    let approved = ContextGrantApproval {
        scope: call.snapshot.as_ref().unwrap().scope.clone(),
        ..approval()
    };
    registry
        .register_context(registration_with("one", d, approved), NOW)
        .unwrap();
    assert!(registry
        .validate_context_grant("one:compact", &call, &caller("a"), NOW)
        .is_ok());
    call.snapshot.as_mut().unwrap().scope.handles = vec!["project-y".into()];
    assert!(registry
        .validate_context_grant("one:compact", &call, &caller("a"), NOW)
        .is_err());
    call.snapshot.as_mut().unwrap().scope.handles.clear();
    assert!(registry
        .validate_context_grant("one:compact", &call, &caller("a"), NOW)
        .is_err());
}

#[test]
fn cross_session_requires_declaration_host_flag_and_exact_target_handle() {
    let mut registry = registry();
    let mut d = descriptor(
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    d.scope = ContextScope {
        namespace: ContextNamespace::CrossSession,
        handles: vec!["b".into(), "c".into()],
    };
    d.permissions.cross_session_read = true;
    let r = registration_with("one", d.clone(), approval());
    assert!(matches!(
        registry.register_context(r, NOW),
        Err(ContextRegistryError::Grant(
            ContextGrantError::CrossSessionDenied
        ))
    ));
    let approved = ContextGrantApproval {
        scope: ContextScope {
            namespace: ContextNamespace::CrossSession,
            handles: vec!["b".into()],
        },
        cross_session: true,
        ..approval()
    };
    registry
        .register_context(registration_with("one", d, approved), NOW)
        .unwrap();
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "b"), &caller("a"), NOW)
        .is_ok());
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "c"), &caller("a"), NOW)
        .is_err());
}

#[test]
fn side_effects_are_denied_and_audited_before_registration() {
    for network in [true, false] {
        let mut registry = registry();
        let mut d = descriptor(
            "compact",
            ContextPhase::Compaction,
            ContextCapabilityMode::Replace,
        );
        d.permissions.network_egress = network;
        d.permissions.project_write = !network;
        let denied =
            registry.register_context(registration_with("one", d.clone(), approval()), NOW);
        assert_eq!(
            denied,
            Err(ContextRegistryError::Grant(if network {
                ContextGrantError::EgressDenied
            } else {
                ContextGrantError::ProjectWriteDenied
            }))
        );
        assert!(registry.resolve_context("one:compact").is_none());
        let audit_events = registry.context_audit();
        let audit = audit_events.last().unwrap();
        assert!(!audit.allowed);
        assert!(audit
            .reason
            .contains(if network { "egress" } else { "project write" }));
        assert!(audit.audit_reason.is_some());
        let approved = ContextGrantApproval {
            egress: network,
            project_write: !network,
            ..approval()
        };
        registry
            .register_context(registration_with("one", d, approved), NOW)
            .unwrap();
        assert!(registry
            .validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), NOW)
            .is_ok());
    }
}

#[test]
fn undeclared_side_effects_or_cross_session_cannot_be_granted() {
    let d = descriptor(
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    for approved in [
        ContextGrantApproval {
            egress: true,
            ..approval()
        },
        ContextGrantApproval {
            project_write: true,
            ..approval()
        },
        ContextGrantApproval {
            cross_session: true,
            ..approval()
        },
    ] {
        assert!(ContextGrant::host_issue(&d, "one", &caller("a"), approved, NOW).is_err());
    }
}

#[test]
fn expiry_is_checked_at_invocation_and_expired_registration_is_atomic() {
    let mut registry = registry();
    let r = registration(
        "one",
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    registry.register_context(r.clone(), NOW).unwrap();
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), 199)
        .is_ok());
    assert!(matches!(
        registry.validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), 200),
        Err(ContextRegistryError::Grant(ContextGrantError::Expired {
            expires_at: 200
        }))
    ));
    let mut empty = PluginRegistry::new();
    empty.register_plugin(PluginRecord::new(
        "one".into(),
        "one".into(),
        PluginKind::Wasm,
        PluginOrigin::Config,
    ));
    assert!(empty.register_context(r, 200).is_err());
    assert_eq!(empty.context_stats().registrations, 0);
}

#[test]
fn wire_validation_is_part_of_grant_gate_and_no_denied_call_reaches_adapter() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "one",
                "compact",
                ContextPhase::Compaction,
                ContextCapabilityMode::Replace,
            ),
            NOW,
        )
        .unwrap();
    let mut invalid = request("compact", "a");
    invalid.snapshot.as_mut().unwrap().input_fingerprint = "wrong".into();
    let mut invoked = 0;
    if registry
        .validate_context_grant("one:compact", &invalid, &caller("a"), NOW)
        .is_ok()
    {
        invoked += 1;
    }
    if registry
        .validate_context_grant("one:missing", &request("compact", "a"), &caller("a"), NOW)
        .is_ok()
    {
        invoked += 1;
    }
    assert_eq!(invoked, 0);
    assert_eq!(
        registry.context_audit().last().unwrap().component_key,
        "one:missing"
    );
    assert!(!registry.context_audit().last().unwrap().allowed);
}

#[test]
fn duplicate_final_owners_and_builtin_owners_reject_before_invocation() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "",
                "builtin",
                ContextPhase::Compaction,
                ContextCapabilityMode::Builtin,
            ),
            NOW,
        )
        .unwrap();
    registry
        .register_context(
            registration(
                "one",
                "compact",
                ContextPhase::Compaction,
                ContextCapabilityMode::Replace,
            ),
            NOW,
        )
        .unwrap();
    assert!(matches!(
        registry.register_context(
            registration(
                "two",
                "compact",
                ContextPhase::Compaction,
                ContextCapabilityMode::Replace
            ),
            NOW
        ),
        Err(ContextRegistryError::OwnerConflict {
            phase: ContextPhase::Compaction,
            ..
        })
    ));
    assert!(matches!(
        registry.register_context(
            registration(
                "",
                "duplicate",
                ContextPhase::Compaction,
                ContextCapabilityMode::Builtin
            ),
            NOW
        ),
        Err(ContextRegistryError::OwnerConflict { .. })
    ));
    registry
        .register_context(
            registration(
                "two",
                "observer",
                ContextPhase::Compaction,
                ContextCapabilityMode::Observer,
            ),
            NOW,
        )
        .unwrap();
    assert!(registry.resolve_context("two:compact").is_none());
    assert_eq!(registry.context_stats().registrations, 3);
}

#[test]
fn duplicate_key_or_forged_owner_metadata_is_rejected() {
    let mut registry = registry();
    let r = registration(
        "one",
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    registry.register_context(r.clone(), NOW).unwrap();
    assert_eq!(
        registry.register_context(r.clone(), NOW),
        Err(ContextRegistryError::DuplicateKey("one:compact".into()))
    );
    let mut mode = r.clone();
    mode.owner_mode = ContextCapabilityMode::Builtin;
    let mut lifecycle = r.clone();
    lifecycle.lifecycle_handle = Some("two".into());
    let mut bundle = r;
    bundle.bundle_id = Some("forged".into());
    for forged in [mode, lifecycle, bundle] {
        assert_eq!(
            registry.register_context(forged, NOW),
            Err(ContextRegistryError::InvalidOwnership)
        );
    }
    let forged = registration(
        "one",
        "builtin",
        ContextPhase::Recovery,
        ContextCapabilityMode::Builtin,
    );
    assert_eq!(
        registry.register_context(forged, NOW),
        Err(ContextRegistryError::InvalidOwnership)
    );
    let forged = registration(
        "",
        "external",
        ContextPhase::Recovery,
        ContextCapabilityMode::Replace,
    );
    assert_eq!(
        registry.register_context(forged, NOW),
        Err(ContextRegistryError::InvalidOwnership)
    );
}

#[test]
fn bundle_failure_leaves_no_partial_owner_and_overlap_rejects() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "two",
                "restore",
                ContextPhase::Recovery,
                ContextCapabilityMode::Replace,
            ),
            NOW,
        )
        .unwrap();
    assert!(matches!(
        registry.register_context_bundle(bundle("one", "all"), NOW),
        Err(ContextRegistryError::OwnerConflict {
            phase: ContextPhase::Recovery,
            ..
        })
    ));
    assert!(registry.resolve_context("one:compact").is_none());
    assert_eq!(registry.context_stats().registrations, 1);
    registry.unregister_plugin("two");
    registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .unwrap();
    assert_eq!(registry.context_stats().registrations, 2);
    assert_eq!(registry.context_stats().bundles, 1);
    assert_eq!(registry.unregister_context_bundle("two", "all").len(), 0);
    assert_eq!(registry.unregister_context_bundle("one", "all").len(), 2);
    assert_eq!(registry.context_stats().registrations, 0);
}

#[test]
fn bundle_rejects_duplicate_members_mixed_owners_and_incremental_install() {
    let mut registry = registry();
    assert_eq!(
        registry.register_context_bundle(vec![], NOW),
        Err(ContextRegistryError::EmptyBundle)
    );
    let mut duplicate = bundle("one", "all");
    duplicate[1] = duplicate[0].clone();
    assert!(registry.register_context_bundle(duplicate, NOW).is_err());
    let mixed = vec![
        bundle("one", "all").remove(0),
        bundle("two", "all").remove(1),
    ];
    assert_eq!(
        registry.register_context_bundle(mixed, NOW),
        Err(ContextRegistryError::InvalidBundle)
    );
    assert_eq!(registry.context_stats().registrations, 0);
    registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .unwrap();
    let mut additional = descriptor(
        "search",
        ContextPhase::Search,
        ContextCapabilityMode::Bundle,
    );
    additional.bundle_id = Some("all".into());
    assert_eq!(
        registry.register_context(registration_with("one", additional, approval()), NOW),
        Err(ContextRegistryError::InvalidBundle)
    );
    assert_eq!(registry.context_stats().registrations, 2);
}

#[test]
fn internal_bundle_owner_overlap_rejects_entire_install() {
    let mut registry = registry();
    let mut rows = bundle("one", "all");
    let mut overlap = descriptor(
        "restore",
        ContextPhase::Compaction,
        ContextCapabilityMode::Bundle,
    );
    overlap.bundle_id = Some("all".into());
    rows[1] = registration_with("one", overlap, approval());
    assert!(matches!(
        registry.register_context_bundle(rows, NOW),
        Err(ContextRegistryError::OwnerConflict { .. })
    ));
    assert_eq!(registry.context_stats().registrations, 0);
}

#[test]
fn bundle_identity_is_namespaced_and_uninstall_preserves_builtin() {
    let mut registry = registry();
    let mut builtin = descriptor(
        "builtin",
        ContextPhase::Compaction,
        ContextCapabilityMode::Builtin,
    );
    builtin.bundle_id = Some("all".into());
    registry
        .register_context(registration_with("", builtin, approval()), NOW)
        .unwrap();
    registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .unwrap();
    assert_eq!(registry.context_stats().bundles, 2);
    assert_eq!(registry.unregister_context_bundle("one", "all").len(), 2);
    assert!(registry.resolve_context("builtin").is_some());
    assert_eq!(registry.context_stats().builtin_registrations, 1);
}

#[test]
fn disabled_unregistered_and_remounted_plugins_lose_contexts_but_builtin_survives() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "",
                "builtin",
                ContextPhase::Compaction,
                ContextCapabilityMode::Builtin,
            ),
            NOW,
        )
        .unwrap();
    registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .unwrap();
    assert!(registry.disable_plugin("one"));
    assert_eq!(registry.context_stats().registrations, 1);
    assert!(registry.resolve_context("builtin").is_some());
    assert!(registry.resolve_context("one:compact").is_none());
    assert!(registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .is_err());
    registry
        .register_context_bundle(bundle("two", "all"), NOW)
        .unwrap();
    registry.register_plugin(PluginRecord::new(
        "two".into(),
        "new-two".into(),
        PluginKind::Wasm,
        PluginOrigin::Config,
    ));
    assert_eq!(registry.context_stats().registrations, 1);
    registry
        .register_context_bundle(bundle("two", "all"), NOW)
        .unwrap();
    registry.unregister_plugin("two");
    assert_eq!(registry.context_stats().registrations, 1);
    assert!(registry
        .validate_context_grant("builtin", &request("builtin", "a"), &caller("a"), NOW)
        .is_ok());
}

#[test]
fn inactive_or_absent_plugin_rows_cannot_issue_executable_registration() {
    let mut registry = registry();
    registry.unregister_plugin("one");
    let r = registration(
        "one",
        "compact",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    assert_eq!(
        registry.register_context(r, NOW),
        Err(ContextRegistryError::PluginUnavailable("one".into()))
    );
    registry
        .register_context(
            registration(
                "two",
                "compact",
                ContextPhase::Compaction,
                ContextCapabilityMode::Replace,
            ),
            NOW,
        )
        .unwrap();
    registry.get_plugin_mut("two").unwrap().status = PluginStatus::Disabled;
    assert!(registry.resolve_context("two:compact").is_none());
    assert!(registry.list_context_descriptors().is_empty());
    assert!(matches!(
        registry.validate_context_grant("two:compact", &request("compact", "a"), &caller("a"), NOW),
        Err(ContextRegistryError::PluginUnavailable(_))
    ));
}

#[test]
fn context_stats_and_clear_cover_real_registration_and_audit_state() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "",
                "builtin",
                ContextPhase::Compaction,
                ContextCapabilityMode::Builtin,
            ),
            NOW,
        )
        .unwrap();
    registry
        .register_context_bundle(bundle("one", "all"), NOW)
        .unwrap();
    assert_eq!(registry.list_context_descriptors().len(), 3);
    let stats = registry.context_stats();
    assert_eq!(stats.registrations, 3);
    assert_eq!(stats.builtin_registrations, 1);
    assert_eq!(stats.plugin_registrations, 2);
    assert_eq!(stats.bundles, 1);
    assert_eq!(stats.active_plugins, 2);
    assert_eq!(stats.audit_retained_events, 3);
    assert_eq!(stats.audit_total_events, 3);
    assert_eq!(registry.stats().contexts, 3);
    registry.clear();
    assert_eq!(registry.context_stats(), ContextStats::default());
    assert!(registry.context_audit().is_empty());
}

#[test]
fn bundle_expiry_disables_every_phase_and_mixed_sessions_cannot_install() {
    let mut registry = registry();
    let mut rows = bundle("one", "all");
    let second = rows[1].descriptor.clone();
    rows[1] = registration_with(
        "one",
        second.clone(),
        ContextGrantApproval {
            expires_at: 150,
            ..approval()
        },
    );
    registry.register_context_bundle(rows, NOW).unwrap();
    assert!(registry
        .validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), 149)
        .is_ok());
    assert!(matches!(
        registry.validate_context_grant("one:compact", &request("compact", "a"), &caller("a"), 150),
        Err(ContextRegistryError::Grant(ContextGrantError::Expired {
            expires_at: 150
        }))
    ));
    registry.unregister_context_bundle("one", "all");
    let mut mixed = bundle("one", "all");
    let grant = ContextGrant::host_issue(&second, "one", &caller("b"), approval(), NOW).unwrap();
    mixed[1] = ContextRegistration::new(second, grant, Some("one".into()));
    assert_eq!(
        registry.register_context_bundle(mixed, NOW),
        Err(ContextRegistryError::InvalidBundle)
    );
    assert_eq!(registry.context_stats().registrations, 0);
}

#[test]
fn plugin_cleanup_with_empty_id_preserves_core_context_owner() {
    let mut registry = registry();
    registry
        .register_context(
            registration(
                "",
                "builtin",
                ContextPhase::Compaction,
                ContextCapabilityMode::Builtin,
            ),
            NOW,
        )
        .unwrap();
    registry.unregister_plugin("");
    assert!(registry.resolve_context("builtin").is_some());
}

#[test]
fn observer_can_inspect_metadata_but_cannot_receive_transform_admission() {
    for operation in ContextOperation::ALL {
        let mut registry = registry();
        let phase = operation.required_phase().unwrap_or(ContextPhase::Status);
        let d = descriptor("observer", phase, ContextCapabilityMode::Observer);
        registry
            .register_context(registration_with("one", d, approval()), NOW)
            .unwrap();
        let mut call = request("observer", "a");
        call.operation = operation;
        call.phase = phase;
        if !operation.requires_snapshot() {
            call.snapshot = None;
            call.budget = None;
            call.input_fingerprint = None;
        }
        let authenticated = caller("a");
        let admitted = registry.validate_context_grant("one:observer", &call, &authenticated, NOW);
        if matches!(
            operation,
            ContextOperation::Describe | ContextOperation::Status | ContextOperation::Shutdown
        ) {
            assert!(admitted.is_ok(), "{operation:?}");
        } else {
            assert!(
                admitted.is_err(),
                "observer must not transform via {operation:?}"
            );
        }
    }
}

#[test]
fn namespace_handle_names_cannot_authorize_a_different_kind_of_resource() {
    let mut registry = registry();
    let mut project = descriptor(
        "project",
        ContextPhase::Compaction,
        ContextCapabilityMode::Replace,
    );
    project.scope = ContextScope {
        namespace: ContextNamespace::Project,
        handles: vec!["b".into()],
    };
    let approved = ContextGrantApproval {
        scope: project.scope.clone(),
        ..approval()
    };
    registry
        .register_context(registration_with("one", project, approved), NOW)
        .unwrap();
    let mut call = request("project", "a");
    call.snapshot.as_mut().unwrap().scope.handles = vec!["b".into()];
    assert!(registry
        .validate_context_grant("one:project", &call, &caller("a"), NOW)
        .is_err());

    let mut cross = descriptor(
        "cross",
        ContextPhase::Compaction,
        ContextCapabilityMode::Compose,
    );
    cross.scope = ContextScope {
        namespace: ContextNamespace::CrossSession,
        handles: vec!["b".into()],
    };
    cross.permissions.cross_session_read = true;
    let approved = ContextGrantApproval {
        scope: cross.scope.clone(),
        cross_session: true,
        ..approval()
    };
    registry
        .register_context(registration_with("two", cross.clone(), approved), NOW)
        .unwrap();
    let mut call = request("cross", "a");
    call.snapshot.as_mut().unwrap().scope = ContextScope {
        namespace: ContextNamespace::Project,
        handles: vec!["b".into()],
    };
    assert!(registry
        .validate_context_grant("two:cross", &call, &caller("a"), NOW)
        .is_err());
    let wrong_namespace = ContextGrantApproval {
        scope: call.snapshot.unwrap().scope,
        cross_session: true,
        ..approval()
    };
    assert!(ContextGrant::host_issue(&cross, "two", &caller("a"), wrong_namespace, NOW).is_err());
}
