//! Performance benchmarks for Markdown projection system.
//!
//! These benchmarks measure the performance of the Markdown projection pipeline:
//! - Event throughput: Target 1000 events/sec
//! - Projection latency: Target <10ms per event
//! - Memory usage: Target <500MB for 1000 sessions

use alephcore::memory::store::markdown::MarkdownSessionStore;
use alephcore::session::events::{
    MessageContent, RunOutcome, SessionEvent, SessionEventRecord, ToolOutput, ToolOutputMetadata,
    TurnTrigger,
};
use alephcore::session::markdown_projector::MarkdownProjector;
use alephcore::session::observer::SessionEventObserver;
use alephcore::session::service::SessionId;
// criterion::black_box is deprecated; use std::hint::black_box directly (same
// signature, drop-in replacement in std since 1.51).
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::hint::black_box;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

/// Helper to create a message content structure.
fn make_content(text: &str) -> MessageContent {
    MessageContent {
        text: text.to_string(),
        blocks: Vec::new(),
        thinking: None,
        thinking_signature: None,
    }
}

/// Helper to create a session event record.
fn make_record(seq: u64, event: SessionEvent) -> SessionEventRecord {
    SessionEventRecord {
        seq,
        event,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    }
}

/// Create a typical conversation event sequence.
/// Returns a vector of events representing a realistic session.
fn create_event_sequence(
    run_id: String,
    turn_id: Uuid,
    call_id: String,
) -> Vec<SessionEventRecord> {
    let mut events = Vec::new();
    let mut seq = 0;

    // 1. RunStarted
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::RunStarted {
            run_id: run_id.clone(),
            at: chrono::Utc::now().timestamp(),
            project_root: Some("/tmp/bench_project".to_string()),
            envelope: None,
            telecom_provenance: Default::default(),
        },
    ));

    // 2. Turn started
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::TurnStarted {
            turn_id,
            trigger: TurnTrigger::UserMessage,
            at: chrono::Utc::now().timestamp(),
        },
    ));

    // 3. User message
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::UserMessage {
            turn_id,
            content: make_content("Can you help me with a coding task?"),
            at: chrono::Utc::now().timestamp(),
            synthetic: false,
            author_user_id: None,
        },
    ));

    // 4. Tool call requested
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::ToolCallRequested {
            turn_id,
            call_id: call_id.clone(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls -la"}),
            identity: None,
            at: chrono::Utc::now().timestamp(),
        },
    ));

    // 5. Tool result
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::ToolResult {
            turn_id,
            call_id: call_id.clone(),
            output: ToolOutput {
                value: serde_json::json!("total 8\ndrwxr-xr-x  2 user user 4096 Oct 19 00:00 ."),
                metadata: ToolOutputMetadata::default(),
            },
            at: chrono::Utc::now().timestamp(),
        },
    ));

    // 6. Assistant message
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::AssistantMessage {
            turn_id,
            content: make_content("Here are the files in the current directory."),
            usage: None,
            at: chrono::Utc::now().timestamp(),
            model: None,
            model_provider: None,
            model_generation: None,
        },
    ));

    // 7. RunFinished
    seq += 1;
    events.push(make_record(
        seq,
        SessionEvent::RunFinished {
            run_id: run_id.clone(),
            outcome: RunOutcome::Completed,
            at: chrono::Utc::now().timestamp(),
        },
    ));

    events
}

/// Benchmark 1: Event Throughput
/// Target: 1000 events/sec
/// Measures time to process 10,000 events across multiple sessions.
fn benchmark_event_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_throughput");

    // Create runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for event_count in [1000, 5000, 10000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(event_count),
            event_count,
            |b, &event_count| {
                b.iter(|| {
                    runtime.block_on(async {
                        // Setup: Create temp store and projector
                        let temp_dir = TempDir::new().unwrap();
                        let store = Arc::new(
                            MarkdownSessionStore::new(temp_dir.path())
                                .await
                                .expect("Failed to create store"),
                        );
                        let projector = MarkdownProjector::new(store.clone());

                        // Generate events
                        let session_id = SessionId::ephemeral("bench");
                        let events_per_session = 7; // From create_event_sequence
                        let num_sessions = event_count / events_per_session;

                        for _ in 0..num_sessions {
                            let run_id = format!("run_{}", Uuid::new_v4());
                            let turn_id = Uuid::new_v4();
                            let call_id = format!("call_{}", Uuid::new_v4());
                            let events = create_event_sequence(run_id, turn_id, call_id);

                            // Process all events
                            for event in events {
                                projector.on_appended(&session_id, &event);
                            }
                        }

                        // Wait for background processing to complete
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                        black_box(projector);
                    });
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 2: Projection Latency (Per Event Type)
/// Target: <10ms per event
/// Measures latency for individual event types.
fn benchmark_projection_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_latency");

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Benchmark UserMessage event
    group.bench_function("user_message", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let temp_dir = TempDir::new().unwrap();
                let store = Arc::new(
                    MarkdownSessionStore::new(temp_dir.path())
                        .await
                        .expect("Failed to create store"),
                );
                let projector = MarkdownProjector::new(store.clone());
                let session_id = SessionId::ephemeral("bench");
                let turn_id = Uuid::new_v4();

                let event = make_record(
                    1,
                    SessionEvent::UserMessage {
                        turn_id,
                        content: make_content("This is a test message for latency measurement."),
                        at: chrono::Utc::now().timestamp(),
                        synthetic: false,
                        author_user_id: None,
                    },
                );

                let start = std::time::Instant::now();
                projector.on_appended(&session_id, &event);
                // Wait for background write
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let elapsed = start.elapsed();

                black_box(elapsed);
            });
        });
    });

    // Benchmark AssistantMessage event
    group.bench_function("assistant_message", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let temp_dir = TempDir::new().unwrap();
                let store = Arc::new(
                    MarkdownSessionStore::new(temp_dir.path())
                        .await
                        .expect("Failed to create store"),
                );
                let projector = MarkdownProjector::new(store.clone());
                let session_id = SessionId::ephemeral("bench");
                let turn_id = Uuid::new_v4();

                let event = make_record(
                    1,
                    SessionEvent::AssistantMessage {
                        turn_id,
                        content: make_content(
                            "This is an assistant response for latency measurement.",
                        ),
                        usage: None,
                        at: chrono::Utc::now().timestamp(),
                        model: None,
                        model_provider: None,
                        model_generation: None,
                    },
                );

                let start = std::time::Instant::now();
                projector.on_appended(&session_id, &event);
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let elapsed = start.elapsed();

                black_box(elapsed);
            });
        });
    });

    // Benchmark ToolCallRequested event
    group.bench_function("tool_call_requested", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let temp_dir = TempDir::new().unwrap();
                let store = Arc::new(
                    MarkdownSessionStore::new(temp_dir.path())
                        .await
                        .expect("Failed to create store"),
                );
                let projector = MarkdownProjector::new(store.clone());
                let session_id = SessionId::ephemeral("bench");
                let turn_id = Uuid::new_v4();
                let call_id = format!("call_{}", Uuid::new_v4());

                let event = make_record(
                    1,
                    SessionEvent::ToolCallRequested {
                        turn_id,
                        call_id,
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo 'benchmark'"}),
                        identity: None,
                        at: chrono::Utc::now().timestamp(),
                    },
                );

                let start = std::time::Instant::now();
                projector.on_appended(&session_id, &event);
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let elapsed = start.elapsed();

                black_box(elapsed);
            });
        });
    });

    // Benchmark ToolResult event
    group.bench_function("tool_result", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let temp_dir = TempDir::new().unwrap();
                let store = Arc::new(
                    MarkdownSessionStore::new(temp_dir.path())
                        .await
                        .expect("Failed to create store"),
                );
                let projector = MarkdownProjector::new(store.clone());
                let session_id = SessionId::ephemeral("bench");
                let turn_id = Uuid::new_v4();
                let call_id = format!("call_{}", Uuid::new_v4());

                let event = make_record(
                    1,
                    SessionEvent::ToolResult {
                        turn_id,
                        call_id,
                        output: ToolOutput {
                            value: serde_json::json!("benchmark output"),
                            metadata: ToolOutputMetadata::default(),
                        },
                        at: chrono::Utc::now().timestamp(),
                    },
                );

                let start = std::time::Instant::now();
                projector.on_appended(&session_id, &event);
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let elapsed = start.elapsed();

                black_box(elapsed);
            });
        });
    });

    group.finish();
}

/// Benchmark 3: Memory Usage
/// Target: <500MB for 1000 sessions
/// Note: This benchmark measures memory indirectly through allocation patterns.
/// For precise memory measurement, use external profiling tools.
fn benchmark_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");

    let runtime = tokio::runtime::Runtime::new().unwrap();

    for session_count in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(session_count),
            session_count,
            |b, &session_count| {
                b.iter(|| {
                    runtime.block_on(async {
                        let temp_dir = TempDir::new().unwrap();
                        let store = Arc::new(
                            MarkdownSessionStore::new(temp_dir.path())
                                .await
                                .expect("Failed to create store"),
                        );
                        let projector = MarkdownProjector::new(store.clone());

                        // Create multiple sessions with 100 events each
                        for _ in 0..session_count {
                            let session_id = SessionId::ephemeral("bench");

                            // Generate 100 events (approximately 14 complete sequences)
                            for _ in 0..14 {
                                let run_id = format!("run_{}", Uuid::new_v4());
                                let turn_id = Uuid::new_v4();
                                let call_id = format!("call_{}", Uuid::new_v4());
                                let events = create_event_sequence(run_id, turn_id, call_id);

                                for event in events {
                                    projector.on_appended(&session_id, &event);
                                }
                            }
                        }

                        // Wait for all background processing
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                        black_box((projector, store));
                    });
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    benchmark_event_throughput,
    benchmark_projection_latency,
    benchmark_memory_usage
);

criterion_main!(benches);
