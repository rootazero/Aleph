use aleph_protocol::memory_trace::{TraceKind, TraceRequest, TraceResult};
use serde_json::json;

#[test]
fn trace_response_preserves_legacy_shape() {
    let legacy: TraceResult =
        serde_json::from_value(json!({ "notes": [], "evidence": [] })).unwrap();
    assert_eq!(legacy, TraceResult::default());
    assert_eq!(
        serde_json::to_value(TraceResult {
            target: "missing".into(),
            ..TraceResult::default()
        })
        .unwrap(),
        json!({ "target": "missing", "notes": [], "evidence": [] }),
    );

    let payload = json!({
        "target": "habits/exercise",
        "notes": ["habits/exercise"],
        "evidence": [{"raw_id":"raw-1", "via_note":null, "via_session":null,
                      "content":null, "pruned":true}],
        "facts": [{"note_path":"habits/exercise", "fact_idx":0, "text":"Runs daily",
                   "origin":"raw_source", "inferred":false, "source_id":"raw-1"}],
        "write_decisions": [{"action":"add", "reason":"duplicate", "subject":"Runs daily",
                             "created_at":42}],
        "note_sources": [
            {"note_path":"habits/exercise", "source_path":"reference/exercise", "status":"available"},
            {"note_path":"habits/exercise", "source_path":"reference/removed", "status":"unavailable"},
            {"note_path":"habits/exercise", "source_path":"reference/queued", "status":"not_visited"}
        ],
        "incomplete": true,
        "truncated": true
    });
    let result: TraceResult = serde_json::from_value(payload.clone()).unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), payload);
}

#[test]
fn trace_request_preserves_kind_tokens_and_defaults_to_legacy_addressing() {
    for (kind, token) in [
        (TraceKind::Note, "note"),
        (TraceKind::Raw, "raw"),
        (TraceKind::ProfileSection, "profile_section"),
        (TraceKind::WriteDecision, "write_decision"),
    ] {
        let request: TraceRequest =
            serde_json::from_value(json!({"target": "", "kind": token})).unwrap();
        assert_eq!(request.kind, kind);
        assert!(!request.exact_partition);
        assert_eq!(request.agent_id, None);
        assert_eq!(request.max_results, None);
        assert_eq!(serde_json::to_value(request).unwrap()["kind"], token);
    }
}
