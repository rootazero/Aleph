//! Golden compatibility tests for `aleph-slot-api` — Task 2 closure
//! (tool contracts, classification taxonomy, capability descriptor).
//!
//! Same discipline as `error_compat.rs`: these tests pin behaviour that
//! alephcore relies on — typed↔Display classifier agreement, retry/ladder
//! predicates, stable labels, descriptor validation/fingerprint semantics.
//! Any change to the pinned values is a deliberate API break and belongs in
//! a separate, explicitly-named proposal — not in this closure.
//!
//! P0 Task 2 ruling pinned here: never fabricate `revision = 1` to project
//! metadata — every descriptor in this file is built with an explicit,
//! meaningful revision.

use aleph_slot_api::descriptor::{
    DescriptorError, ImplementationContract, ReplayContractFingerprint, ReplayPolicy,
    ToolCapabilityDescriptor, SCHEMA_VERSION,
};
use aleph_slot_api::error_kind::{
    classify_error_str, classify_tool_error, ToolErrorKind, CROSS_BATCH_REFUSED_CAUSE,
};
use aleph_slot_api::tool::{RefusedBy, ToolDefinition, ToolDefinitionMetadata, ToolError};

// =========================================================================
// 1. RefusedBy — 4 variants, exact heads, hints never override the head.
// =========================================================================

#[test]
fn refused_by_all_lists_four_variants_in_declaration_order() {
    assert_eq!(
        RefusedBy::ALL,
        [
            RefusedBy::Hook,
            RefusedBy::HookFailed,
            RefusedBy::Person,
            RefusedBy::NobodyAsked,
        ]
    );
}

#[test]
fn refused_by_heads_are_pinned_verbatim() {
    assert_eq!(RefusedBy::Hook.head(), "was refused by a policy hook");
    assert_eq!(
        RefusedBy::HookFailed.head(),
        "was blocked because a policy hook failed to run (fail-closed)"
    );
    assert_eq!(RefusedBy::Person.head(), "was declined");
    assert_eq!(RefusedBy::NobodyAsked.head(), "was not authorized");
}

/// A refusal whose `reason` carries a `401` / `429` / timeout hint must
/// still classify as `Permission`: the fixed head wins over any content.
/// The matrix is every `RefusedBy` variant (three faces of refusal) crossed
/// with each hinted reason: 4 × 3 = 12 explicitly-counted cases (S2 repair).
#[test]
fn refused_reason_hints_never_override_fixed_head() {
    let hinted_reasons = [
        "hook vetoed after HTTP 401 from upstream",
        "hook saw 429 too many requests",
        "user wrote: timed out after 30s, denied",
    ];
    let mut cases = 0usize;
    for by in RefusedBy::ALL {
        for reason in hinted_reasons {
            cases += 1;
            let err = ToolError::Refused {
                name: "search".to_string(),
                by,
                reason: reason.to_string(),
            };
            // Typed face.
            assert_eq!(
                err.kind(),
                ToolErrorKind::Permission,
                "RefusedBy::{by:?} with hint {reason:?} must stay Permission"
            );
            // Display face starts with the fixed head.
            let rendered = err.to_string();
            assert!(
                rendered.starts_with(&format!("tool search {}: ", by.head())),
                "unexpected rendering: {rendered}"
            );
            // Read-back face agrees (判据 §12): the classifier reading the
            // rendered string reaches the same kind as the typed error.
            assert_eq!(classify_error_str(&rendered), ToolErrorKind::Permission);
            assert!(!ToolErrorKind::Permission.admits_ladder());
        }
    }
    assert_eq!(
        cases,
        RefusedBy::ALL.len() * hinted_reasons.len(),
        "the refusal matrix must cover every RefusedBy x hinted-reason pair"
    );
}

// =========================================================================
// 2. Retry / ladder matrix — Repeated and Cancelled never admit a ladder;
//    ApprovalExpired and Cancelled are retryable and transient.
// =========================================================================

#[test]
fn repeated_never_admits_ladder() {
    let err = ToolError::Execution {
        name: "search".to_string(),
        cause: CROSS_BATCH_REFUSED_CAUSE.to_string(),
    };
    assert_eq!(err.kind(), ToolErrorKind::Repeated);
    assert_eq!(
        classify_error_str(&err.to_string()),
        ToolErrorKind::Repeated
    );
    assert!(!ToolErrorKind::Repeated.admits_ladder());
    // A refused repeat is not itself retryable — the earlier error is the
    // one that means something.
    assert!(!err.is_retryable());
    assert!(!ToolErrorKind::Repeated.is_transient());
}

#[test]
fn cancelled_never_admits_ladder_but_is_retryable_and_transient() {
    let err = ToolError::Cancelled {
        name: "search".to_string(),
    };
    assert!(err.is_retryable());
    assert_eq!(err.kind(), ToolErrorKind::Cancelled);
    assert!(ToolErrorKind::Cancelled.is_transient());
    assert!(!ToolErrorKind::Cancelled.admits_ladder());
    assert_eq!(
        classify_error_str(&err.to_string()),
        ToolErrorKind::Cancelled
    );
}

#[test]
fn approval_expired_is_retryable_and_transient() {
    let err = ToolError::ApprovalExpired {
        name: "search".to_string(),
        waited_ms: 60_000,
    };
    assert!(err.is_retryable());
    let kind = err.kind();
    assert!(
        kind.is_transient(),
        "ApprovalExpired kind {kind:?} must be transient"
    );
    // Read-back face agrees with the typed face.
    assert_eq!(classify_error_str(&err.to_string()), kind);
}

/// The retry gate is structural, the taxonomy a superset: every retryable
/// variant classifies to a transient kind.
#[test]
fn every_retryable_variant_is_also_transient() {
    let retryables = [
        ToolError::Timeout {
            name: "t".into(),
            elapsed_ms: 1,
        },
        ToolError::Transport {
            name: "t".into(),
            cause: "io".into(),
        },
        ToolError::ApprovalExpired {
            name: "t".into(),
            waited_ms: 1,
        },
        ToolError::Cancelled { name: "t".into() },
    ];
    for err in retryables {
        assert!(err.is_retryable());
        assert!(err.kind().is_transient(), "{} must classify transient", err);
    }
}

// =========================================================================
// 3. Kind labels — the LLM-facing wire surface, pinned verbatim.
// =========================================================================

#[test]
fn tool_error_kind_labels_are_pinned_verbatim() {
    let cases: [(ToolErrorKind, &str); 15] = [
        (ToolErrorKind::Unauthorized, "unauthorized"),
        (ToolErrorKind::RateLimited, "rate_limited"),
        (ToolErrorKind::UpstreamNotFound, "upstream_not_found"),
        (ToolErrorKind::UpstreamServerError, "upstream_server_error"),
        (ToolErrorKind::BlockedByPolicy, "blocked_by_policy"),
        (ToolErrorKind::EmptyResult, "empty_result"),
        (ToolErrorKind::Timeout, "timeout"),
        (ToolErrorKind::Transport, "transport"),
        (ToolErrorKind::Validation, "validation"),
        (ToolErrorKind::Permission, "permission"),
        (ToolErrorKind::ToolNotFound, "tool_not_found"),
        (ToolErrorKind::Duplicate, "duplicate"),
        (ToolErrorKind::Cancelled, "cancelled"),
        (ToolErrorKind::Repeated, "repeated"),
        (ToolErrorKind::Execution, "execution"),
    ];
    for (kind, label) in cases {
        assert_eq!(kind.label(), label);
    }
}

/// classify_tool_error on the typed value and classify_error_str on its
/// rendering must agree for every structural variant (判据 §12, one
/// derivation on every face).
#[test]
fn typed_and_rendered_classification_agree_on_all_variants() {
    let errors = [
        ToolError::NotFound { name: "t".into() },
        ToolError::PermissionDenied {
            name: "t".into(),
            reason: "nope".into(),
        },
        ToolError::ValidationFailed {
            name: "t".into(),
            cause: "bad".into(),
        },
        ToolError::Execution {
            name: "t".into(),
            cause: "boom".into(),
        },
        ToolError::Refused {
            name: "t".into(),
            by: RefusedBy::Hook,
            reason: "policy".into(),
        },
        ToolError::Timeout {
            name: "t".into(),
            elapsed_ms: 5,
        },
        ToolError::ApprovalExpired {
            name: "t".into(),
            waited_ms: 5,
        },
        ToolError::Transport {
            name: "t".into(),
            cause: "io".into(),
        },
        ToolError::Cancelled { name: "t".into() },
        ToolError::Duplicate { name: "t".into() },
        ToolError::InvalidDescriptor {
            name: "t".into(),
            reason: "bad".into(),
        },
        ToolError::DescriptorMismatch {
            name: "t".into(),
            reason: "bad".into(),
        },
        ToolError::RegistryClosed { name: "t".into() },
        ToolError::UnknownRevision {
            name: "t".into(),
            revision: 9,
        },
        ToolError::Execution {
            name: "t".into(),
            cause: CROSS_BATCH_REFUSED_CAUSE.into(),
        },
    ];
    for err in errors {
        assert_eq!(
            classify_tool_error(&err),
            classify_error_str(&err.to_string()),
            "typed/rendered disagreement for {err:?}"
        );
    }
}

// =========================================================================
// 4. Descriptor goldens — roundtrip, mismatch rejection, fingerprint rules.
// =========================================================================

fn sample_definition() -> ToolDefinition {
    ToolDefinition {
        name: "web_search".to_string(),
        description: "Search the web".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
        source: aleph_slot_api::descriptor::ToolSource::Builtin,
        metadata: ToolDefinitionMetadata::default(),
    }
}

/// A descriptor built from a definition validates, matches that definition,
/// and carries the explicit revision it was given — never a fabricated one.
#[test]
fn descriptor_from_definition_roundtrips_with_explicit_revision() {
    let definition = sample_definition();
    let descriptor = ToolCapabilityDescriptor::from_definition(&definition, 7);
    assert_eq!(descriptor.revision, 7);
    assert_eq!(descriptor.schema_version, SCHEMA_VERSION);
    assert!(descriptor.validate().is_ok());
    assert!(descriptor.matches_definition(&definition));

    // Metadata projection agrees field-for-field.
    let metadata = descriptor.to_metadata_definition();
    assert_eq!(metadata.name, definition.name);
    assert_eq!(metadata.description, definition.description);
    assert_eq!(metadata.parameters, definition.input_schema);
    assert_eq!(
        metadata.requires_confirmation,
        definition.metadata.requires_approval
    );
}

/// Every projected field participates in the pairing check: changing any of
/// name / description / schema / safety flags breaks `matches_definition`.
#[test]
fn descriptor_rejects_field_schema_and_safety_mismatches() {
    let definition = sample_definition();
    let descriptor = ToolCapabilityDescriptor::from_definition(&definition, 3);

    let mut renamed = definition.clone();
    renamed.name = "other".to_string();
    assert!(!descriptor.matches_definition(&renamed));

    let mut redescribed = definition.clone();
    redescribed.description = "different".to_string();
    assert!(!descriptor.matches_definition(&redescribed));

    let mut reschema = definition.clone();
    reschema.input_schema = serde_json::json!({"type": "object"});
    assert!(!descriptor.matches_definition(&reschema));

    let mut source_changed = definition.clone();
    source_changed.source = aleph_slot_api::descriptor::ToolSource::Mcp {
        server_id: "srv".to_string(),
    };
    assert!(!descriptor.matches_definition(&source_changed));

    // Safety-flag projection: each flag flip is a mismatch.
    let mut approval = definition.clone();
    approval.metadata.requires_approval = true;
    assert!(!descriptor.matches_definition(&approval));

    let mut idempotent = definition.clone();
    idempotent.metadata.idempotent = true;
    assert!(!descriptor.matches_definition(&idempotent));

    let mut concurrent = definition.clone();
    concurrent.metadata.concurrent_safe = true;
    assert!(!descriptor.matches_definition(&concurrent));

    let mut budgeted = definition.clone();
    budgeted.metadata.max_duration_ms = Some(5_000);
    assert!(!descriptor.matches_definition(&budgeted));
}

/// `validate` fail-closed cases: empty name, overlong name, non-object
/// schema, zero schema_version, zero revision.
#[test]
fn descriptor_validate_rejects_invalid_shapes() {
    let definition = sample_definition();

    let mut d = ToolCapabilityDescriptor::from_definition(&definition, 1);
    d.name = String::new();
    assert!(matches!(d.validate(), Err(DescriptorError::EmptyName)));
    assert_eq!(
        d.validate().unwrap_err().to_string(),
        "tool capability name is empty"
    );

    let mut d = ToolCapabilityDescriptor::from_definition(&definition, 1);
    d.name = "x".repeat(65);
    assert!(matches!(
        d.validate(),
        Err(DescriptorError::NameTooLong { max: 64, .. })
    ));

    let mut d = ToolCapabilityDescriptor::from_definition(&definition, 1);
    d.input_schema = serde_json::json!(["not", "an", "object"]);
    assert!(matches!(
        d.validate(),
        Err(DescriptorError::NonObjectInputSchema)
    ));

    let mut d = ToolCapabilityDescriptor::from_definition(&definition, 1);
    d.schema_version = 0;
    assert!(matches!(
        d.validate(),
        Err(DescriptorError::InvalidSchemaVersion(0))
    ));

    let d = ToolCapabilityDescriptor::from_definition(&definition, 0);
    assert!(matches!(
        d.validate(),
        Err(DescriptorError::InvalidRevision(0))
    ));
}

/// A Safe descriptor without a valid audited implementation contract is
/// rejected and has no fingerprint.
#[test]
fn safe_replay_requires_implementation_contract() {
    let definition = sample_definition();
    let mut descriptor = ToolCapabilityDescriptor::from_definition(&definition, 2);
    descriptor.replay_policy = ReplayPolicy::Safe;

    // Missing contract: rejected, no fingerprint.
    assert!(matches!(
        descriptor.validate(),
        Err(DescriptorError::SafeReplayRequiresImplementationContract { .. })
    ));
    assert_eq!(
        descriptor.validate().unwrap_err().to_string(),
        "tool capability \"web_search\" declares Safe replay but has no valid audited \
         implementation contract (id and version are required)"
    );
    assert_eq!(descriptor.replay_contract_fingerprint(), None);

    // Invalid contract (empty version): same refusal.
    descriptor.implementation_contract = Some(ImplementationContract {
        id: "web-search-impl".to_string(),
        version: String::new(),
    });
    assert!(matches!(
        descriptor.validate(),
        Err(DescriptorError::SafeReplayRequiresImplementationContract { .. })
    ));
    assert_eq!(descriptor.replay_contract_fingerprint(), None);

    // Unsafe descriptors legitimately carry no contract.
    let unsafe_descriptor = ToolCapabilityDescriptor::from_definition(&definition, 2);
    assert!(unsafe_descriptor.validate().is_ok());
    assert_eq!(unsafe_descriptor.replay_contract_fingerprint(), None);
}

/// Fingerprint goldens: deterministic for identical inputs, stable under
/// JSON key reordering (canonical preimage), sensitive to any contract or
/// descriptor change, and roundtripping through the `v{n}:{hex}` wire form.
#[test]
fn replay_contract_fingerprint_golden_behaviour() {
    let definition = sample_definition();
    let contract = ImplementationContract {
        id: "web-search-impl".to_string(),
        version: "2026-10-a".to_string(),
    };
    let mut descriptor = ToolCapabilityDescriptor::from_definition(&definition, 4);
    descriptor.replay_policy = ReplayPolicy::Safe;
    descriptor.implementation_contract = Some(contract.clone());
    assert!(descriptor.validate().is_ok());

    let fp = descriptor
        .replay_contract_fingerprint()
        .expect("valid Safe descriptor must fingerprint");
    assert_eq!(fp.version(), 1);

    // Literal golden (Q1 repair): SHA-256 over the 244-byte canonical
    // replay-contract preimage of this exact sample descriptor. Independently
    // reconstructed from the HEAD preimage algorithm and confirmed to match;
    // any drift in the preimage encoding breaks this pin loudly. The
    // algorithm is never edited to fit the golden.
    assert_eq!(
        fp.to_wire_string(),
        "v1:9a13f48190624a3e752ea15c53e12f1b13f6454530f6f6031ca2a57e0a1d7d4f"
    );

    // Deterministic: an identical second descriptor fingerprints the same.
    let mut twin = ToolCapabilityDescriptor::from_definition(&definition, 4);
    twin.replay_policy = ReplayPolicy::Safe;
    twin.implementation_contract = Some(contract.clone());
    assert_eq!(twin.replay_contract_fingerprint(), Some(fp));

    // Canonicalization: reordering schema keys does not change the digest.
    let mut reordered = definition.clone();
    reordered.input_schema = serde_json::json!({
        "required": ["query"],
        "properties": {"query": {"type": "string"}},
        "type": "object"
    });
    let mut twin2 = ToolCapabilityDescriptor::from_definition(&reordered, 4);
    twin2.replay_policy = ReplayPolicy::Safe;
    twin2.implementation_contract = Some(contract.clone());
    assert_eq!(twin2.replay_contract_fingerprint(), Some(fp));

    // Sensitivity: bumping the contract version changes the digest (an old
    // token can never be reused after a handler change).
    let mut bumped = descriptor.clone();
    bumped.implementation_contract = Some(ImplementationContract {
        id: "web-search-impl".to_string(),
        version: "2026-10-b".to_string(),
    });
    assert_ne!(bumped.replay_contract_fingerprint(), Some(fp));

    // Wire form: `v1:` + 64 lowercase hex chars, parse roundtrips exactly.
    let wire = fp.to_wire_string();
    assert!(wire.starts_with("v1:"), "unexpected wire form: {wire}");
    assert_eq!(wire.len(), 3 + 64);
    assert!(wire[3..]
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(ReplayContractFingerprint::parse(&wire), Ok(fp));
}

/// Unknown or malformed fingerprint versions are rejected fail-closed —
/// never read back as a valid fingerprint.
#[test]
fn unknown_fingerprint_version_is_rejected() {
    let valid = ReplayContractFingerprint::parse(&format!("v1:{}", "ab".repeat(32)))
        .expect("v1 with 64 lowercase hex must parse");

    // Unknown version.
    let v2 = format!("v2:{}", "ab".repeat(32));
    assert!(ReplayContractFingerprint::parse(&v2).is_err());

    // Missing `v` prefix.
    assert!(ReplayContractFingerprint::parse(&format!("1:{}", "ab".repeat(32))).is_err());

    // Missing colon / wrong digest length / uppercase hex.
    assert!(ReplayContractFingerprint::parse("v1").is_err());
    assert!(ReplayContractFingerprint::parse("v1:ab").is_err());
    assert!(ReplayContractFingerprint::parse(&format!("v1:{}", "AB".repeat(32))).is_err());

    // Serde face: a rejected string fails to deserialize (fail-closed).
    let bad_json = serde_json::json!(v2);
    assert!(serde_json::from_value::<ReplayContractFingerprint>(bad_json).is_err());
    let good_json = serde_json::json!(valid.to_wire_string());
    assert_eq!(
        serde_json::from_value::<ReplayContractFingerprint>(good_json).unwrap(),
        valid
    );
}
