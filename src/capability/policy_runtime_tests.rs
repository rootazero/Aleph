//! Focused tests for source-owned startup restrictions, not production mounting.
use super::constraints::{ConstraintContext, ConstraintViolation};
use super::descriptor::{
    CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilityRevision, ProjectionMetadata,
    SchemaRef,
};
use super::facade::{BackendLease, BackendSourceEvidence};
use super::ownership::{LifetimeScope, OwnerRef, OwnershipTree, RunId, VisibilityScope};
use super::policy::{PolicyDenial, PolicyEngine};
use super::policy_runtime::{
    CapabilityCallContext, ConstraintFactsSource, PolicyFactSourceKey, ResourceUse,
};
use super::provider::{ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport};
use crate::config::types::policies::capability_constraints::{
    CapabilityConstraintsConfig, ResourceUnit,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Facts {
    key: PolicyFactSourceKey,
    context: Mutex<Result<ConstraintContext, ConstraintViolation>>,
    reads: AtomicUsize,
    conditions: &'static [&'static str],
    units: &'static [ResourceUnit],
}
impl ConstraintFactsSource for Facts {
    fn source_key(&self) -> PolicyFactSourceKey {
        self.key.clone()
    }
    fn supported_conditions(&self) -> &'static [&'static str] {
        self.conditions
    }
    fn supported_units(&self) -> &'static [ResourceUnit] {
        self.units
    }
    fn snapshot(
        &self,
        _call: &CapabilityCallContext,
        unit: Option<ResourceUnit>,
        other_active_calls: usize,
    ) -> Result<ConstraintContext, ConstraintViolation> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.context.lock().unwrap().clone().map(|context| {
            if unit == Some(ResourceUnit::ActiveCalls) {
                context.with_active_resources(other_active_calls)
            } else {
                context
            }
        })
    }
}
fn context(hour: u8, ready: Option<bool>, occupied: usize) -> ConstraintContext {
    let context = ConstraintContext::at_hour(hour)
        .unwrap()
        .with_active_resources(occupied);
    match ready {
        Some(ready) => context
            .with_condition("provider_ready", ready)
            .with_condition("attachment_ready", ready),
        None => context,
    }
}
fn call() -> CapabilityCallContext {
    CapabilityCallContext::from_trusted_parts(
        OwnerRef::Runtime,
        Some("user-a".into()),
        None,
        None,
        Some("connection-a".into()),
        "call-a".into(),
    )
}
fn rules(body: &str) -> CapabilityConstraintsConfig {
    toml::from_str(body).unwrap()
}
fn provider() -> ProviderHandle {
    ProviderHandle::new(ProviderLifecycle::ready(
        ProviderId::new("runtime/test").unwrap(),
        ProviderTransport::InProcess,
    ))
}
fn fixture(body: &str) -> (PolicyEngine, BackendLease, ProviderHandle) {
    let tree = Arc::new(OwnershipTree::new());
    let id = CapabilityId {
        namespace: "aleph/tools".into(),
        name: "ui".into(),
    };
    tree.register(
        id.clone(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .unwrap();
    let generation = tree.generation(&id, &VisibilityScope::default()).unwrap();
    let provider = provider();
    let descriptor = CapabilityDescriptor {
        id,
        kind: CapabilityKind::Tool,
        schema: SchemaRef {
            version: 1,
            fingerprint: [0; 32],
        },
        revision: CapabilityRevision(1),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: vec![],
        metadata: ProjectionMetadata::default(),
        facets: Default::default(),
    };
    let lease = BackendLease {
        descriptor,
        owner_generation: generation,
        owner: OwnerRef::Runtime,
        source: BackendSourceEvidence::Provider {
            provider_id: provider.provider_id(),
            provider_generation: provider.generation(),
        },
        required_resources: vec![],
    };
    (
        PolicyEngine::with_shared_tree(tree, rules(body)).unwrap(),
        lease,
        provider,
    )
}
fn facts(
    provider: &ProviderHandle,
    lease: &BackendLease,
    incarnation: u64,
    current: ConstraintContext,
) -> Arc<Facts> {
    Arc::new(Facts {
        key: PolicyFactSourceKey::for_source(provider, &lease.descriptor.id, incarnation),
        context: Mutex::new(Ok(current)),
        reads: AtomicUsize::new(0),
        conditions: &["provider_ready", "attachment_ready"],
        units: &[ResourceUnit::ActiveCalls, ResourceUnit::PtySessions],
    })
}
const CALL_LIMIT: &str = r#"
["aleph/tools/ui".resource]
unit = "active_calls"
max_active = 2
"#;

#[test]
fn configured_pending_and_replacement_never_fall_back_to_allow() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let pending = policy.admit_runtime(&lease, None, &call(), ResourceUse::Allocate);
    assert!(matches!(
        pending,
        Err(PolicyDenial::AttachmentUnavailable { .. })
    ));
    let facts = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let old = policy.prepare_attachment(&lease, facts.clone()).unwrap();
    drop(
        policy
            .admit_runtime(&lease, Some(&old), &call(), ResourceUse::Allocate)
            .unwrap(),
    );
    let replacement = BackendLease {
        descriptor: CapabilityDescriptor {
            revision: CapabilityRevision(2),
            ..lease.descriptor.clone()
        },
        ..lease.clone()
    };
    let new = policy.prepare_attachment(&replacement, facts).unwrap();
    let mut denied_effect_writes = 0;
    if policy
        .admit_runtime(&lease, Some(&old), &call(), ResourceUse::Allocate)
        .is_ok()
    {
        denied_effect_writes += 1;
    }
    assert_eq!(denied_effect_writes, 0);
    assert!(policy
        .admit_runtime(&replacement, Some(&new), &call(), ResourceUse::Allocate)
        .is_ok());
    policy
        .tree()
        .register(
            lease.descriptor.id.clone(),
            OwnerRef::Run(RunId("new-owner".into())),
            LifetimeScope::Runtime,
            VisibilityScope::default(),
        )
        .unwrap();
    assert!(matches!(
        policy.admit_runtime(&replacement, Some(&new), &call(), ResourceUse::Allocate),
        Err(PolicyDenial::PrincipalMismatch { .. })
    ));
    policy.withdraw_attachment(&new);
    assert!(policy
        .admit_runtime(&replacement, Some(&new), &call(), ResourceUse::Allocate)
        .is_err());
}

#[test]
fn strict_below_excludes_candidate_and_keeps_other_calls() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let source = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let attachment = policy.prepare_attachment(&lease, source).unwrap();
    let first = policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .unwrap();
    let below_limit =
        policy.admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate);
    assert!(below_limit.is_ok());
    let second = below_limit.unwrap();
    let at_limit = policy.admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate);
    assert!(at_limit.is_err());
    drop(first);
    let restored = policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .unwrap();
    drop(second);
    drop(restored);
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_ok());
}

#[test]
fn utc_missing_condition_and_release_boundary() {
    let (policy, lease, provider) = fixture(
        r#"
["aleph/tools/ui".time]
start_hour = 9
end_hour = 17
["aleph/tools/ui".resource]
unit = "pty_sessions"
max_active = 2
["aleph/tools/ui".required_conditions]
provider_ready = true
"#,
    );
    let source = facts(&provider, &lease, 1, context(8, Some(true), 0));
    let attachment = policy.prepare_attachment(&lease, source.clone()).unwrap();
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_err());
    for (hour, allowed) in [(9, true), (16, true), (17, false)] {
        *source.context.lock().unwrap() = Ok(context(hour, Some(true), 0));
        assert_eq!(
            policy
                .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
                .is_ok(),
            allowed
        );
    }
    for ready in [None, Some(false)] {
        *source.context.lock().unwrap() = Ok(context(12, ready, 0));
        assert!(policy
            .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
            .is_err());
    }
    *source.context.lock().unwrap() = Err(ConstraintViolation::ClockUnavailable);
    let error = policy.admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Release);
    assert!(matches!(error, Err(PolicyDenial::ConstraintDenied { .. })));
    *source.context.lock().unwrap() = Ok(context(12, Some(true), 2));
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_err());
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Existing)
        .is_ok());
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Release)
        .is_ok());
    *source.context.lock().unwrap() = Ok(context(17, Some(true), 2));
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Release)
        .is_err());
    *source.context.lock().unwrap() = Ok(context(12, Some(false), 2));
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Release)
        .is_err());
    policy.withdraw_attachment(&attachment);
    *source.context.lock().unwrap() = Ok(context(12, Some(true), 0));
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Release)
        .is_err());

    let (night, lease, provider) = fixture(
        r#"["aleph/tools/ui".time]
start_hour = 22
end_hour = 6"#,
    );
    let source = facts(&provider, &lease, 2, context(22, None, 0));
    let attachment = night.prepare_attachment(&lease, source.clone()).unwrap();
    for (hour, allowed) in [
        (22, true),
        (23, true),
        (0, true),
        (5, true),
        (6, false),
        (21, false),
    ] {
        *source.context.lock().unwrap() = Ok(context(hour, None, 0));
        assert_eq!(
            night
                .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
                .is_ok(),
            allowed
        );
    }
}

#[test]
fn two_live_source_fact_bindings_do_not_replace_each_other() {
    let (policy, lease, provider) = fixture(
        r#"
["aleph/tools/ui".resource]
unit = "active_calls"
max_active = 1
["aleph/tools/ui".required_conditions]
attachment_ready = true
"#,
    );
    let registry = crate::tools::registry::ToolHandlerRegistry::new();
    let handler: Arc<dyn crate::tools::handlers::ToolHandler> = Arc::new(UiIngress);
    let descriptor = crate::tools::descriptor::ToolCapabilityDescriptor::from_definition(
        &handler.definition(),
        0,
    );
    let _registration = registry.register(descriptor, handler).unwrap();
    let lease = BackendLease {
        source: BackendSourceEvidence::Tool {
            canonical_entry: registry.capture_entry("ui").unwrap(),
            provider_generation: registry.provider().generation(),
        },
        ..lease
    };
    let source_a = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let a = policy.prepare_attachment(&lease, source_a.clone()).unwrap();
    let running = policy
        .admit_runtime(&lease, Some(&a), &call(), ResourceUse::Allocate)
        .unwrap();
    let source_b = facts(&provider, &lease, 2, context(12, Some(false), 0));
    let b = policy.prepare_attachment(&lease, source_b).unwrap();
    assert!(
        matches!(
            policy.admit_runtime(&lease, Some(&b), &call(), ResourceUse::Allocate),
            Err(PolicyDenial::ConstraintDenied { .. })
        ),
        "B must retain its own false attachment facts"
    );
    assert!(
        matches!(
            policy.admit_runtime(&lease, Some(&a), &call(), ResourceUse::Allocate),
            Err(PolicyDenial::ConstraintDenied { .. })
        ),
        "B must neither replace A nor reset A's running usage"
    );
    drop(running);
    let recovered = policy
        .admit_runtime(&lease, Some(&a), &call(), ResourceUse::Allocate)
        .expect("A must remain live after B attaches and recover when its running guard drops");
    drop(recovered);
    assert!(
        matches!(
            policy.admit_runtime(&lease, Some(&b), &call(), ResourceUse::Allocate),
            Err(PolicyDenial::ConstraintDenied { .. })
        ),
        "B must still fail its own facts after A releases its usage"
    );
    policy.withdraw_fact_source(&source_a.source_key());
    assert!(
        matches!(
            policy.admit_runtime(&lease, Some(&a), &call(), ResourceUse::Allocate),
            Err(PolicyDenial::AttachmentUnavailable { .. })
        ),
        "A must become unavailable after its own source is withdrawn"
    );
    assert!(
        matches!(
            policy.admit_runtime(&lease, Some(&b), &call(), ResourceUse::Allocate),
            Err(PolicyDenial::ConstraintDenied { .. })
        ),
        "withdrawing A must not withdraw B"
    );
}

#[test]
fn unsupported_facts_rejected_at_attachment_and_no_rule_reads_nothing() {
    let (policy, lease, provider) = fixture(
        r#"["aleph/tools/ui".required_conditions]
unknown_fact = true"#,
    );
    let source = facts(&provider, &lease, 1, context(12, Some(true), 0));
    assert!(matches!(
        policy.prepare_attachment(&lease, source.clone()),
        Err(PolicyDenial::InvalidConstraints { .. })
    ));
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    let (no_rule, lease, provider) = fixture("");
    let source = facts(&provider, &lease, 1, context(12, None, 0));
    *source.context.lock().unwrap() = Err(ConstraintViolation::ClockUnavailable);
    let attachment = no_rule.prepare_attachment(&lease, source.clone()).unwrap();
    assert!(no_rule
        .admit_runtime(&lease, None, &call(), ResourceUse::Allocate)
        .is_ok());
    assert!(no_rule
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_ok());
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    assert!(!no_rule.is_configured(&lease.descriptor.id));
}

#[test]
fn unsupported_unit_and_stale_attachment_never_admit() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    assert_eq!(
        policy.resource_unit(&lease.descriptor.id),
        Some(ResourceUnit::ActiveCalls)
    );
    let mut source = facts(&provider, &lease, 1, context(12, None, 0));
    Arc::get_mut(&mut source).unwrap().units = &[];
    assert!(matches!(
        policy.prepare_attachment(&lease, source),
        Err(PolicyDenial::InvalidConstraints { .. })
    ));
    let source = facts(&provider, &lease, 1, context(12, None, 0));
    let attachment = policy.prepare_attachment(&lease, source).unwrap();
    let stale = BackendLease {
        owner_generation: super::ownership::OwnerGeneration(999),
        ..lease.clone()
    };
    assert!(policy
        .admit_runtime(&stale, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_err());
    let wrong_owner = OwnerRef::Run(RunId("not-the-captured-owner".into()));
    let forged = BackendLease {
        owner: wrong_owner.clone(),
        ..lease.clone()
    };
    let matching_actor = CapabilityCallContext::from_trusted_parts(
        wrong_owner,
        None,
        None,
        None,
        None,
        "other".into(),
    );
    assert_eq!(matching_actor.actor(), &forged.owner);
    assert!(policy
        .admit_runtime(
            &forged,
            Some(&attachment),
            &matching_actor,
            ResourceUse::Release,
        )
        .is_err());
}

#[test]
fn trusted_run_actor_does_not_become_the_runtime_tool_owner() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let registry = crate::tools::registry::ToolHandlerRegistry::new();
    let handler: Arc<dyn crate::tools::handlers::ToolHandler> = Arc::new(UiIngress);
    let descriptor = crate::tools::descriptor::ToolCapabilityDescriptor::from_definition(
        &handler.definition(),
        0,
    );
    let _registration = registry.register(descriptor, handler).unwrap();
    let lease = BackendLease {
        source: BackendSourceEvidence::Tool {
            canonical_entry: registry.capture_entry("ui").unwrap(),
            provider_generation: registry.provider().generation(),
        },
        ..lease
    };
    let source = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let attachment = policy.prepare_attachment(&lease, source).unwrap();
    let actor = OwnerRef::Run(RunId("trusted-ingress-run".into()));
    let call = CapabilityCallContext::from_trusted_parts(
        actor.clone(),
        Some("authenticated-user-a".into()),
        None,
        Some("workspace-a".into()),
        Some("connection-a".into()),
        "call-a".into(),
    );
    assert_ne!(call.actor(), &lease.owner);
    assert_eq!(call.principal(), Some("authenticated-user-a"));
    let first = policy
        .admit_runtime(&lease, Some(&attachment), &call, ResourceUse::Allocate)
        .expect("trusted actor attribution is not an additional owner-equality authorization gate");
    let second = policy
        .admit_runtime(&lease, Some(&attachment), &call, ResourceUse::Allocate)
        .unwrap();
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call, ResourceUse::Allocate)
        .is_err());
    drop(first);
    drop(
        policy
            .admit_runtime(&lease, Some(&attachment), &call, ResourceUse::Allocate)
            .unwrap(),
    );
    assert_eq!(call.actor(), &actor);
    drop(second);
}

#[test]
fn captured_owner_does_not_follow_an_unrelated_tree_rebind() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let source = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let attachment = policy.prepare_attachment(&lease, source).unwrap();
    let new_owner = OwnerRef::Run(RunId("next-owner".into()));
    policy
        .tree()
        .register(
            lease.descriptor.id.clone(),
            new_owner.clone(),
            LifetimeScope::Runtime,
            lease.descriptor.visibility.clone(),
        )
        .unwrap();
    assert_eq!(
        policy
            .tree()
            .generation(&lease.descriptor.id, &lease.descriptor.visibility),
        Some(lease.owner_generation)
    );
    let rebound = BackendLease {
        descriptor: CapabilityDescriptor {
            owner: new_owner.clone(),
            ..lease.descriptor.clone()
        },
        owner: new_owner.clone(),
        ..lease
    };
    let matching_actor = CapabilityCallContext::from_trusted_parts(
        new_owner,
        None,
        None,
        None,
        None,
        "next-owner-call".into(),
    );
    assert_eq!(matching_actor.actor(), &rebound.owner);
    assert!(
        policy
            .admit_runtime(
                &rebound,
                Some(&attachment),
                &matching_actor,
                ResourceUse::Allocate
            )
            .is_err(),
        "a matching caller cannot transfer a captured restriction attachment"
    );
}

struct UiIngress;
#[async_trait::async_trait]
impl crate::tools::handlers::ToolHandler for UiIngress {
    fn definition(&self) -> crate::tools::service::ToolDefinition {
        crate::tools::service::ToolDefinition {
            name: "ui".into(),
            description: "Policy unit fixture".into(),
            input_schema: serde_json::json!({"type": "object"}),
            source: crate::tools::service::ToolSource::Builtin,
            metadata: Default::default(),
        }
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
        panic!("the policy-only fixture must not invoke an effect");
    }
}

#[test]
fn repeated_attachment_preserves_reader_and_replacement_keeps_running_usage() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let original = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let attachment = policy.prepare_attachment(&lease, original.clone()).unwrap();
    let first = policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .unwrap();
    let second = policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .unwrap();
    let alternate = facts(&provider, &lease, 1, context(12, Some(false), 0));
    let repeated = policy
        .prepare_attachment(&lease, alternate.clone())
        .unwrap();
    assert!(policy
        .admit_runtime(&lease, Some(&repeated), &call(), ResourceUse::Allocate)
        .is_err());
    assert_eq!(alternate.reads.load(Ordering::SeqCst), 0);
    let replacement = BackendLease {
        descriptor: CapabilityDescriptor {
            revision: CapabilityRevision(2),
            ..lease.descriptor.clone()
        },
        ..lease.clone()
    };
    let new = policy.prepare_attachment(&replacement, original).unwrap();
    assert!(policy.prepare_attachment(&lease, alternate).is_err());
    assert!(policy
        .admit_runtime(&replacement, Some(&new), &call(), ResourceUse::Allocate)
        .is_err());
    drop(first);
    assert!(policy
        .admit_runtime(&replacement, Some(&new), &call(), ResourceUse::Allocate)
        .is_ok());
    drop(second);
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_err());
}

struct WithdrawingFacts {
    facts: Arc<Facts>,
    policy: std::sync::Weak<PolicyEngine>,
}
impl ConstraintFactsSource for WithdrawingFacts {
    fn source_key(&self) -> PolicyFactSourceKey {
        self.facts.source_key()
    }
    fn supported_conditions(&self) -> &'static [&'static str] {
        self.facts.supported_conditions()
    }
    fn supported_units(&self) -> &'static [ResourceUnit] {
        self.facts.supported_units()
    }
    fn snapshot(
        &self,
        call: &CapabilityCallContext,
        unit: Option<ResourceUnit>,
        count: usize,
    ) -> Result<ConstraintContext, ConstraintViolation> {
        self.policy
            .upgrade()
            .unwrap()
            .withdraw_fact_source(&self.facts.source_key());
        self.facts.snapshot(call, unit, count)
    }
}

#[test]
fn source_callback_runs_outside_locks_and_binding_is_rechecked_after_it() {
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let policy = Arc::new(policy);
    let source = Arc::new(WithdrawingFacts {
        facts: facts(&provider, &lease, 1, context(12, Some(true), 0)),
        policy: Arc::downgrade(&policy),
    });
    let attachment = policy.prepare_attachment(&lease, source).unwrap();
    let admitted = policy.admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate);
    assert!(matches!(
        admitted,
        Err(PolicyDenial::AttachmentUnavailable { .. })
    ));
}

#[test]
fn startup_constraints_change_reports_restart() {
    use crate::config::reload_impact::ReloadImpact;
    for path in [
        "policies",
        "policies.capability_constraints",
        "policies.capability_constraints.aleph/tools/ui.resource.max_active",
    ] {
        assert_eq!(ReloadImpact::classify(path), ReloadImpact::Restart);
    }
    let (policy, lease, provider) = fixture(CALL_LIMIT);
    let source = facts(&provider, &lease, 1, context(12, Some(true), 0));
    let attachment = policy.prepare_attachment(&lease, source).unwrap();
    let mut changed = rules(CALL_LIMIT);
    changed
        .get_mut("aleph/tools/ui")
        .unwrap()
        .resource
        .as_mut()
        .unwrap()
        .max_active = 1;
    let updated = PolicyEngine::with_shared_tree(Arc::new(OwnershipTree::new()), changed).unwrap();
    assert!(updated.is_configured(&lease.descriptor.id));
    let first = policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .unwrap();
    let second = policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .unwrap();
    assert!(policy
        .admit_runtime(&lease, Some(&attachment), &call(), ResourceUse::Allocate)
        .is_err());
    drop((first, second));
}
