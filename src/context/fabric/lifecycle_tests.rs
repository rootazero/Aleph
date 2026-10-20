use super::runtime_task3_tests::{caller, descriptor, handler, output, pressure, registry_with};
use super::*;
use crate::providers::message::UnifiedMessage;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn lifecycle_observer_receives_only_subscribed_metadata_and_cannot_transform() {
    let mut declaration = descriptor(
        "observe",
        ContextPhase::Lifecycle,
        ContextCapabilityMode::Observer,
    );
    declaration.lifecycle = vec![
        ContextLifecycleEvent::BeforePreflight,
        ContextLifecycleEvent::AfterPreflight,
    ];
    let registry = registry_with(vec![("observer", declaration)]).await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let fabric = ContextFabric::builder(registry)
        .handler(
            "observer:observe",
            handler(move |request| {
                captured.lock().unwrap().push(request);
                Ok(output(json!(["attempted replacement"])))
            }),
        )
        .build();
    let mut messages = vec![UnifiedMessage::user("private session content")];
    let before = serde_json::to_value(&messages).unwrap();
    fabric
        .preflight(&mut messages, &pressure(), 1, &caller(), None)
        .await
        .unwrap();
    fabric
        .notify_lifecycle(ContextLifecycleEvent::TurnEnd, &caller(), None)
        .await;
    assert_eq!(serde_json::to_value(messages).unwrap(), before);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for (request, event) in requests.iter().zip(["before_preflight", "after_preflight"]) {
        assert_eq!(request.operation, ContextOperation::Notify);
        let snapshot = request.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.payload, json!({"event": event}));
        assert!(!serde_json::to_string(request)
            .unwrap()
            .contains("private session content"));
    }
}

#[tokio::test]
async fn lifecycle_revocation_and_session_isolation_stop_delivery() {
    let mut declaration = descriptor(
        "observe",
        ContextPhase::Lifecycle,
        ContextCapabilityMode::Observer,
    );
    declaration.lifecycle = vec![ContextLifecycleEvent::TurnEnd];
    let registry = registry_with(vec![("observer", declaration)]).await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let fabric = ContextFabric::builder(registry.clone())
        .handler(
            "observer:observe",
            handler(move |_| {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(output(Value::Null))
            }),
        )
        .build();
    fabric
        .notify_lifecycle(ContextLifecycleEvent::TurnEnd, &caller(), None)
        .await;
    let other = AuthenticatedCaller::from_host_boundary("other-session").unwrap();
    fabric
        .notify_lifecycle(ContextLifecycleEvent::TurnEnd, &other, None)
        .await;
    registry
        .write()
        .await
        .revoke_context_grants("observer", caller().session_id());
    fabric
        .notify_lifecycle(ContextLifecycleEvent::TurnEnd, &caller(), None)
        .await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failing_lifecycle_observer_cannot_fail_native_preflight() {
    let mut declaration = descriptor(
        "observe",
        ContextPhase::Lifecycle,
        ContextCapabilityMode::Observer,
    );
    declaration.lifecycle = vec![
        ContextLifecycleEvent::BeforePreflight,
        ContextLifecycleEvent::AfterPreflight,
    ];
    let registry = registry_with(vec![("observer", declaration)]).await;
    let fabric = ContextFabric::builder(registry)
        .handler(
            "observer:observe",
            handler(|_| Err("observer unavailable".into())),
        )
        .build();
    let mut messages = vec![UnifiedMessage::user("keep")];
    fabric
        .preflight(&mut messages, &pressure(), 1, &caller(), None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].transcript_text(), "keep");
}

#[tokio::test]
async fn credential_snapshot_falls_back_without_calling_external_handler() {
    let declaration = descriptor(
        "admit",
        ContextPhase::Admission,
        ContextCapabilityMode::Replace,
    );
    let registry = registry_with(vec![("privacy", declaration)]).await;
    let fabric = ContextFabric::builder(registry)
        .handler(
            "privacy:admit",
            handler(|_| panic!("credential must not reach plugin")),
        )
        .build();
    let mut value =
        json!({"headers": {"Authorization": "Bearer sk-abc123def456ghi789jklmnopqrstuvwx"}});
    let result = fabric
        .admit_tool_output_with_context(
            "privacy:admit",
            "example",
            "call",
            &mut value,
            Some(4096),
            None,
            crate::tools::result_processing::RecoveryTools {
                ctx_search: false,
                file_read: false,
            },
            &caller(),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        result.owner,
        ContextOwner::BuiltinFallback {
            reason: DegradedReason::InvalidContract,
            ..
        }
    ));
}
