use super::ChatState;
use serde_json::{json, Value};

fn text(run: &str, seq: u64, content: &str) -> Value {
    json!({"topic":"stream.response_chunk", "nowMs":1, "data":{"run_id":run, "seq":seq, "content":content}})
}

fn tool(run: &str, seq: u64, change: Value) -> Value {
    json!({"topic":"stream.chat_tool", "nowMs":1, "data":{"run_id":run, "seq":seq, "delta":{"tool_call_id":"edit-1", "change":change}}})
}

fn end(run: &str, seq: u64) -> Value {
    json!({"topic":"stream.run_complete", "nowMs":1, "data":{"run_id":run, "seq":seq, "summary":{"tool_summaries":["must not appear"]}}})
}

fn history(active: Option<&str>, messages: Value) -> Value {
    json!({"session_key":"s-1", "active_run":active, "messages":messages})
}

fn start_tool() -> Value {
    json!({"kind":"started", "display_name":"Edit", "args_text":"src/lib.rs"})
}

fn finish_tool() -> Value {
    json!({"kind":"finished", "success":true, "duration_ms":16, "presentation_omitted":false})
}

#[test]
fn early_frames_keep_source_order_and_do_not_adopt_another_run() {
    let mut model = ChatState::default();
    model.begin_send("hello".into(), 0);
    model.accept(text("other", 1, "FOREIGN"));
    model.accept(text("r-1", 5, "Before"));
    model.accept(tool("r-1", 8, start_tool()));
    model.accept(text("r-1", 10, "After"));
    let patch = model.resolve_send("s-1".into(), Some("r-1".into()), 2);
    assert_eq!(
        patch["appendOrder"],
        json!(["text/r-1/5", "tool/r-1/edit-1", "text/r-1/10"])
    );
    assert!(!patch.to_string().contains("FOREIGN"));
    assert_eq!(patch["missingPrefix"], false);
    let patch = model.accept(tool("r-1", 13, finish_tool()));
    assert_eq!(patch["upsert"][0]["key"], "tool/r-1/edit-1");
    assert_eq!(patch["upsert"][0]["status"], "ok");
    assert_eq!(patch["appendOrder"], json!([]));
    assert_eq!(model.accept(text("r-1", 10, "After"))["upsert"], json!([]));
    let patch = model.accept(text("r-1", 21, " more"));
    assert_eq!(patch["upsert"][0]["text"], "After more");
    assert_eq!(patch["upsert"][0]["key"], "text/r-1/10");
}

#[test]
fn history_does_not_erase_the_live_suffix_and_replaces_it_only_when_committed() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    let initial = model.replace_history(history(Some("r-1"), json!([])), Value::Null);
    assert_eq!(initial["missingPrefix"], true);
    model.accept(text("r-1", 5, "live suffix"));
    let snapshot = model.replace_history(history(Some("r-1"), json!([])), Value::Null);
    assert_eq!(snapshot["rows"][0]["text"], "live suffix");
    let patch = model.accept(end("r-1", 6));
    assert_eq!(patch["reconcile"], true);
    assert_eq!(patch["activeRun"], Value::Null);
    assert_eq!(patch["upsert"][0]["streaming"], false);
    assert!(!patch.to_string().contains("must not appear"));
    let stale = model.replace_history(history(Some("r-1"), json!([])), Value::Null);
    assert_eq!(stale["activeRun"], Value::Null);
    assert_eq!(stale["rows"][0]["text"], "live suffix");
    let details = json!({"runs":[{"run_id":"r-1", "limited":false, "tools":[
        {"tool_call_id":"edit-1", "change":start_tool()}, {"tool_call_id":"edit-1", "change":finish_tool()}
    ]}], "limited":false});
    let committed = model.replace_history(
        history(
            None,
            json!([
                {"role":"user", "content":"hello", "timestamp":"t1", "run_id":"r-1"},
                {"role":"assistant", "content":"committed result", "timestamp":"t2", "run_id":"r-1"}
            ]),
        ),
        details,
    );
    let rows = committed["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["text"], "hello");
    assert_eq!(rows[1]["key"], "tool/r-1/edit-1");
    assert_eq!(rows[2]["text"], "committed result");
    assert_eq!(model.accept(text("r-1", 99, "late"))["upsert"], json!([]));
}

#[test]
fn early_event_capacity_and_age_mark_the_suffix_incomplete() {
    let mut crowded = ChatState::default();
    crowded.begin_send("hello".into(), 0);
    for seq in 1..=65 {
        crowded.accept(text("r-1", seq, if seq == 65 { "LOST" } else { "a" }));
    }
    let patch = crowded.resolve_send("s-1".into(), Some("r-1".into()), 2);
    assert_eq!(patch["upsert"][0]["text"], "a".repeat(64));
    assert!(!patch.to_string().contains("LOST"));
    assert_eq!(patch["missingPrefix"], true);
    assert_eq!(patch["reconcile"], true);

    let mut expired = ChatState::default();
    expired.begin_send("hello".into(), 0);
    expired.accept(text("r-2", 1, "OLD"));
    let patch = expired.resolve_send("s-1".into(), Some("r-2".into()), 10_001);
    assert_eq!(patch["upsert"], json!([]));
    assert_eq!(patch["missingPrefix"], true);
    assert_eq!(patch["reconcile"], true);
}

#[test]
fn a_foreign_snapshot_never_changes_the_live_fold() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.replace_history(history(Some("r-1"), json!([])), Value::Null);
    let mut foreign = history(Some("r-2"), json!([]));
    foreign["session_key"] = json!("s-2");
    assert_eq!(model.replace_history(foreign, Value::Null), Value::Null);
    assert_eq!(
        model.accept(text("r-2", 1, "not ours"))["upsert"],
        json!([])
    );
    assert_eq!(
        model.accept(text("r-1", 1, "ours"))["upsert"][0]["text"],
        "ours"
    );
}

#[test]
fn one_new_chunk_patch_has_constant_size_at_10_or_500_history_rows() {
    let patch_size = |num| {
        let mut model = ChatState::default();
        model.reset("s-1".into());
        let messages: Vec<_> = (0..num).map(|index| json!({"role":"assistant", "content":"a".repeat(100), "timestamp":index.to_string()})).collect();
        model.replace_history(history(Some("r-1"), json!(messages)), Value::Null);
        let patch = model.accept(text("r-1", 7, "chunk"));
        assert_eq!(patch["upsert"].as_array().unwrap().len(), 1);
        serde_json::to_vec(&patch).unwrap().len()
    };
    assert_eq!(patch_size(10), patch_size(500));
}
