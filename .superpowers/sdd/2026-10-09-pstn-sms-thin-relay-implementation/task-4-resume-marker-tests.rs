//! Tests for Task 4: server-authored telecom provenance marker for resume
//!
//! These tests are written FIRST (TDD) to define the behavior before implementation.

#[cfg(test)]
mod telecom_marker_tests {
    use crate::gateway::resume_coordinator::{plan_resume, ResumeRefusal};
    use crate::gateway::telecom::persisted_marker::{MarkerRestore, PersistedTelecomMarker};
    use crate::gateway::telecom::public_context::{
        TelecomChannel, TelecomProvenance, TelecomPublicContext,
    };
    use crate::session::events::{RunEnvelopeSnapshot, SessionEvent};
    use crate::session::reduction::RunStartFacts;

    // ========================================================================
    // Test 1: SessionEvent::RunStarted serialization with telecom_provenance
    // ========================================================================

    #[test]
    fn run_started_with_ordinary_marker_round_trips() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-ord".into(),
            at: 1_700_000_000_000,
            project_root: None,
            envelope: None,
            telecom_provenance: PersistedTelecomMarker::Ordinary,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        match back {
            SessionEvent::RunStarted {
                telecom_provenance,
                ..
            } => {
                assert_eq!(telecom_provenance, PersistedTelecomMarker::Ordinary);
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
        // Marker present on wire
        assert!(json.contains("telecom_provenance"));
    }

    #[test]
    fn run_started_with_public_marker_round_trips() {
        let ctx = TelecomPublicContext::new(TelecomProvenance::new(
            TelecomChannel::Sms,
            "us-west".to_string(),
            "binding-123".to_string(),
            42,
        ));
        let marker = PersistedTelecomMarker::capture(Some(&ctx));
        let ev = SessionEvent::RunStarted {
            run_id: "run-pub".into(),
            at: 1_700_000_000_000,
            project_root: None,
            envelope: None,
            telecom_provenance: marker.clone(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        match back {
            SessionEvent::RunStarted {
                telecom_provenance,
                ..
            } => match telecom_provenance {
                PersistedTelecomMarker::Public(ref p) => {
                    assert_eq!(p.channel(), TelecomChannel::Sms);
                    assert_eq!(p.region(), "us-west");
                    assert_eq!(p.binding_ref(), "binding-123");
                    assert_eq!(p.connection_generation(), 42);
                }
                other => panic!("expected Public marker, got {other:?}"),
            },
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn run_started_legacy_json_deserializes_to_legacy_marker() {
        // Old JSON with no telecom_provenance field
        let legacy = r#"{"type":"run_started","run_id":"old","at":1700000000000}"#;
        let back: SessionEvent = serde_json::from_str(legacy).unwrap();
        match back {
            SessionEvent::RunStarted {
                telecom_provenance,
                ..
            } => {
                assert_eq!(telecom_provenance, PersistedTelecomMarker::Legacy);
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn run_started_marker_skipped_on_wire_when_legacy() {
        let ev = SessionEvent::RunStarted {
            run_id: "run-leg".into(),
            at: 1_700_000_000_000,
            project_root: None,
            envelope: None,
            telecom_provenance: PersistedTelecomMarker::Legacy,
        };
        let json = serde_json::to_string(&ev).unwrap();
        // Legacy should NOT appear on wire (skip_serializing_if)
        assert!(!json.contains("telecom_provenance"));
    }

    // ========================================================================
    // Test 2: RunStartFacts carries marker
    // ========================================================================

    #[test]
    fn run_start_facts_carries_telecom_marker() {
        let marker = PersistedTelecomMarker::Ordinary;
        let facts = RunStartFacts {
            seq: 42,
            run_id: "run-1".into(),
            project_root: None,
            envelope: None,
            telecom_provenance: marker.clone(),
        };
        assert_eq!(facts.telecom_provenance, marker);
    }

    // ========================================================================
    // Test 3: Reduction extracts marker from RunStarted
    // ========================================================================

    #[test]
    fn reduction_extracts_ordinary_marker_from_run_started() {
        use crate::session::events::SessionEventRecord;
        use crate::session::reduction::reduce_run;

        let events = vec![SessionEventRecord {
            seq: 1,
            created_at_ms: 1_700_000_000_000,
            event: SessionEvent::RunStarted {
                run_id: "run-ord".into(),
                at: 1_700_000_000_000,
                project_root: None,
                envelope: None,
                telecom_provenance: PersistedTelecomMarker::Ordinary,
            },
        }];

        let reduction = reduce_run(&events).unwrap();
        assert!(reduction.open_run.is_some());
        let facts = reduction.open_run.unwrap();
        assert_eq!(facts.telecom_provenance, PersistedTelecomMarker::Ordinary);
    }

    #[test]
    fn reduction_extracts_public_marker_from_run_started() {
        use crate::session::events::SessionEventRecord;
        use crate::session::reduction::reduce_run;

        let ctx = TelecomPublicContext::new(TelecomProvenance::new(
            TelecomChannel::Voice,
            "eu-central".to_string(),
            "bind-456".to_string(),
            7,
        ));
        let marker = PersistedTelecomMarker::capture(Some(&ctx));

        let events = vec![SessionEventRecord {
            seq: 1,
            created_at_ms: 1_700_000_000_000,
            event: SessionEvent::RunStarted {
                run_id: "run-pub".into(),
                at: 1_700_000_000_000,
                project_root: None,
                envelope: None,
                telecom_provenance: marker,
            },
        }];

        let reduction = reduce_run(&events).unwrap();
        let facts = reduction.open_run.unwrap();
        match facts.telecom_provenance {
            PersistedTelecomMarker::Public(p) => {
                assert_eq!(p.channel(), TelecomChannel::Voice);
                assert_eq!(p.region(), "eu-central");
            }
            other => panic!("expected Public marker, got {other:?}"),
        }
    }

    // ========================================================================
    // Test 4: plan_resume carries marker restore result
    // ========================================================================

    #[test]
    fn plan_resume_ordinary_marker_yields_none_telecom() {
        let facts = RunStartFacts {
            seq: 1,
            run_id: "r1".into(),
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
            telecom_provenance: PersistedTelecomMarker::Ordinary,
        };

        let plan = plan_resume(Some(&facts), None, &|_| true);
        // Ordinary → restore should yield None for telecom_public
        match plan.telecom_restore.as_ref().unwrap() {
            MarkerRestore::Ordinary => {}
            other => panic!("expected Ordinary restore, got {other:?}"),
        }
    }

    #[test]
    fn plan_resume_public_marker_restores_context() {
        let ctx = TelecomPublicContext::new(TelecomProvenance::new(
            TelecomChannel::Sms,
            "ap-south".to_string(),
            "bind-789".to_string(),
            13,
        ));
        let marker = PersistedTelecomMarker::capture(Some(&ctx));

        let facts = RunStartFacts {
            seq: 1,
            run_id: "r1".into(),
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
            telecom_provenance: marker,
        };

        let plan = plan_resume(Some(&facts), None, &|_| true);
        match plan.telecom_restore.as_ref().unwrap() {
            MarkerRestore::Public(restored) => {
                let prov = restored.provenance();
                assert_eq!(prov.channel(), TelecomChannel::Sms);
                assert_eq!(prov.region(), "ap-south");
                assert_eq!(prov.binding_ref(), "bind-789");
                assert_eq!(prov.connection_generation(), 13);
            }
            other => panic!("expected Public restore, got {other:?}"),
        }
    }

    #[test]
    fn plan_resume_legacy_marker_yields_unproven() {
        let facts = RunStartFacts {
            seq: 1,
            run_id: "r1".into(),
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
            telecom_provenance: PersistedTelecomMarker::Legacy,
        };

        let plan = plan_resume(Some(&facts), None, &|_| true);
        match plan.telecom_restore.as_ref().unwrap() {
            MarkerRestore::LegacyUnproven => {}
            other => panic!("expected LegacyUnproven, got {other:?}"),
        }
    }

    #[test]
    fn plan_resume_unknown_marker_yields_refusal() {
        use serde_json::json;
        let facts = RunStartFacts {
            seq: 1,
            run_id: "r1".into(),
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
            telecom_provenance: PersistedTelecomMarker::Unknown(json!({"version": 999})),
        };

        let plan = plan_resume(Some(&facts), None, &|_| true);
        match plan.telecom_restore.as_ref().unwrap() {
            MarkerRestore::Refuse(reason) => {
                assert!(reason.contains("unrecognised"));
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn plan_resume_corrupt_marker_yields_refusal() {
        use serde_json::json;
        let facts = RunStartFacts {
            seq: 1,
            run_id: "r1".into(),
            project_root: None,
            envelope: Some(RunEnvelopeSnapshot::default()),
            telecom_provenance: PersistedTelecomMarker::Corrupt(json!("not an object")),
        };

        let plan = plan_resume(Some(&facts), None, &|_| true);
        match plan.telecom_restore.as_ref().unwrap() {
            MarkerRestore::Refuse(reason) => {
                assert!(reason.contains("unreadable"));
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    // ========================================================================
    // Test 5: admit_resume enforces fail-closed on Unknown/Corrupt/Legacy
    // ========================================================================

    // These tests would check that admit_resume returns ResumeRefusal for
    // Legacy/Unknown/Corrupt when attempting public resume. Since admit_resume
    // is complex and uses many dependencies, we focus on the plan_resume tests
    // above which define the contract.

    // ========================================================================
    // Test 6: Writer path captures marker correctly
    // ========================================================================

    // This would test that when RunStarted is written in runner_impl.rs,
    // it captures PersistedTelecomMarker::capture(telecom_public.as_ref()).
    // We cannot easily test the full runner here, but the pattern is:
    // - telecom_public: None → Ordinary
    // - telecom_public: Some(ctx) → Public(...)
}
