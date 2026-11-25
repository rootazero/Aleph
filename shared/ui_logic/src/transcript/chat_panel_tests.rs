use super::ChatState;
use serde_json::{json, Value};

fn text(run: &str, seq: u64, content: &str) -> Value {
    json!({"topic":"stream.response_chunk", "nowMs":1, "data":{"run_id":run, "seq":seq, "content":content}})
}

fn tool(run: &str, seq: u64, change: Value) -> Value {
    json!({"topic":"stream.chat_tool", "nowMs":1, "data":{"type":"chat_tool", "run_id":run, "seq":seq, "tool_call_id":"edit-1", "change":change}})
}

fn end(run: &str, seq: u64) -> Value {
    json!({"topic":"stream.run_complete", "nowMs":1, "data":{"run_id":run, "seq":seq, "summary":{"tool_summaries":["must not appear"]}}})
}

fn history(active: Option<&str>, messages: Value) -> Value {
    json!({"session_key":"s-1", "active_run":active, "messages":messages})
}

fn paged_history(
    active: Option<&str>,
    messages: Value,
    requested: Option<&str>,
    next: Option<&str>,
    status: &str,
) -> Value {
    let mut page = history(active, messages);
    page["scrollback"] =
        json!({"requested_before": requested, "next_before": next, "status": status});
    page
}

#[test]
fn older_page_preserves_live_run_and_deduplicates_split_run() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    let tail = model.replace_history(
        paged_history(
            Some("live"),
            json!([
                {"message_id":"2", "role":"assistant", "content":"answer", "run_id":"finished"}
            ]),
            None,
            Some("boundary"),
            "ready",
        ),
        Value::Null,
    );
    assert_eq!(tail["nextBefore"], "boundary");
    model.accept(text("live", 1, "streaming"));
    let older = model.prepend_history(paged_history(Some("not-live"), json!([
        {"message_id":"1", "role":"user", "content":"older prompt", "run_id":"finished"},
        {"message_id":"2", "role":"assistant", "content":"overlapping copy", "run_id":"finished"}
    ]), Some("boundary"), None, "ready"), "boundary".into());
    let rows = older["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["text"], "older prompt");
    assert_eq!(rows[1]["key"], tail["rows"][0]["key"]);
    assert_eq!(rows[1]["text"], "answer");
    assert_eq!(rows[2]["text"], "streaming");
    assert_eq!(older["activeRun"], "live");
    assert_eq!(older["nextBefore"], Value::Null);
    let details = json!({"runs":[{"run_id":"finished", "limited":false, "tools":[
        {"tool_call_id":"edit-1", "change":start_tool()}, {"tool_call_id":"edit-1", "change":finish_tool()}
    ]}], "limited":false});
    model.apply_history_details(vec!["finished".into()], details.clone());
    let joined = model.apply_history_details(vec!["finished".into()], details);
    assert_eq!(joined["activeRun"], "live");
    assert_eq!(
        joined["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == "tool")
            .count(),
        1
    );
    assert_eq!(joined["rows"][2]["text"], "answer");
    let unavailable = model.apply_history_details(vec!["finished".into()], Value::Null);
    assert_eq!(unavailable["limited"], true);
    assert_eq!(unavailable["rows"][1]["text"], "answer");
    let none_admitted = model.apply_history_details(
        vec!["finished".into()],
        json!({"runs": [], "limited": false}),
    );
    assert_eq!(none_admitted["limited"], true);
    let confirmed_empty = model.apply_history_details(
        vec!["finished".into()],
        json!({"runs": [{"run_id": "finished", "tools": [], "limited": false}], "limited": false}),
    );
    assert_eq!(confirmed_empty["limited"], false);
    assert_eq!(confirmed_empty["rows"][1]["text"], "answer");
}

#[test]
fn tail_refresh_keeps_confirmed_prefix_or_reports_reset() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.replace_history(
        paged_history(
            None,
            json!([
                {"message_id":"2", "role":"assistant", "content":"second"}
            ]),
            None,
            Some("two"),
            "ready",
        ),
        Value::Null,
    );
    model.prepend_history(
        paged_history(
            None,
            json!([
                {"message_id":"1", "role":"assistant", "content":"first"}
            ]),
            Some("two"),
            Some("one"),
            "ready",
        ),
        "two".into(),
    );
    let refreshed = model.replace_history(
        paged_history(
            None,
            json!([
                {"message_id":"2", "role":"assistant", "content":"second"},
                {"message_id":"3", "role":"assistant", "content":"third"}
            ]),
            None,
            Some("two"),
            "ready",
        ),
        Value::Null,
    );
    assert_eq!(refreshed["rows"][0]["text"], "first");
    assert_eq!(refreshed["nextBefore"], "one");
    let reset = model.replace_history(
        paged_history(
            None,
            json!([
                {"message_id":"4", "role":"assistant", "content":"only this"}
            ]),
            None,
            Some("four"),
            "ready",
        ),
        Value::Null,
    );
    assert_eq!(reset["scrollbackReset"], true);
    assert_eq!(reset["rows"].as_array().unwrap().len(), 1);
    assert!(model
        .prepend_history(
            paged_history(None, json!([]), Some("one"), None, "ready"),
            "one".into()
        )
        .is_null());
}

#[test]
fn older_pages_require_support_exact_echo_and_stable_ids() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    let legacy = history(None, json!([{"role":"assistant", "content":"legacy"}]));
    assert!(model.replace_history(legacy.clone(), Value::Null)["nextBefore"].is_null());
    assert!(model.prepend_history(legacy, "two".into()).is_null());
    model.replace_history(
        paged_history(
            None,
            json!([
                {"message_id":"2", "role":"assistant", "content":"second"}
            ]),
            None,
            Some("two"),
            "ready",
        ),
        Value::Null,
    );
    assert!(model
        .prepend_history(
            paged_history(None, json!([]), Some("wrong"), None, "ready"),
            "two".into()
        )
        .is_null());
    assert!(model
        .prepend_history(
            paged_history(
                None,
                json!([
                    {"role":"assistant", "content":"ambiguous"}
                ]),
                Some("two"),
                None,
                "ready"
            ),
            "two".into()
        )
        .is_null());
    let unavailable = model.prepend_history(
        paged_history(
            None,
            json!([
                {"message_id":"1", "role":"assistant", "content":"first"}
            ]),
            Some("two"),
            None,
            "continuation_unavailable",
        ),
        "two".into(),
    );
    assert_eq!(unavailable["continuationUnavailable"], true);
    assert!(unavailable["nextBefore"].is_null());
    assert_eq!(unavailable["rows"][0]["text"], "first");
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
    assert_eq!(stale["rows"][0]["text"], "Unconfirmed turn");
    assert_eq!(stale["rows"][1]["text"], "live suffix");
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
fn a_history_read_before_execution_admission_keeps_stop_and_accepts_frames() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.begin_send("hello".into(), 0);
    model.resolve_send("s-1".into(), Some("r-1".into()), 0);
    let snapshot = model.replace_history(history(None, json!([])), Value::Null);
    assert_eq!(snapshot["activeRun"], "r-1");
    assert_eq!(
        model.accept(text("r-1", 1, "arrived"))["upsert"][0]["text"],
        "arrived"
    );
}

#[test]
fn continuity_loss_discards_held_terminal_and_keeps_unconfirmed_runs_separately() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.begin_send("first".into(), 0);
    model.accept(text("r-1", 1, "pre-interruption"));
    model.accept(end("r-1", 2));
    model.interrupt();
    model.accept(text("r-1", 3, "current suffix"));
    let ack = model.resolve_send("s-1".into(), Some("r-1".into()), 2);
    assert_eq!(ack["activeRun"], "r-1");
    assert_eq!(ack["upsert"][0]["text"], "current suffix");
    assert_eq!(ack["reconcile"], true);
    assert!(ack.get("terminalRun").is_none());
    let sealed = model.accept(end("r-1", 4));
    assert_eq!(sealed["terminalRun"], "r-1");
    assert_eq!(sealed["missingPrefix"], true);
    assert_eq!(sealed["unconfirmed"], true);

    let next = model.begin_send("second".into(), 0);
    assert_eq!(next["remove"], json!([]));
    model.resolve_send("s-1".into(), Some("r-2".into()), 0);
    model.accept(text("r-2", 1, "second suffix"));
    model.accept(end("r-2", 2));
    let snapshot = model.replace_history(history(None, json!([])), Value::Null);
    let texts = |snapshot: &Value| {
        snapshot["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        texts(&snapshot),
        [
            "Unconfirmed turn",
            "first",
            "current suffix",
            "Unconfirmed turn",
            "second",
            "second suffix"
        ]
    );
    let committed = model.replace_history(
        history(
            None,
            json!([
                {"role":"user", "content":"first committed prompt", "run_id":"r-1"},
                {"role":"tool", "content":"RAW TOOL BODY", "run_id":"r-1"},
                {"role":"assistant", "content":"first committed", "run_id":"r-1"}
            ]),
        ),
        Value::Null,
    );
    assert_eq!(committed["missingPrefix"], false);
    assert_eq!(committed["unconfirmed"], true);
    assert_eq!(
        texts(&committed),
        [
            "first committed prompt",
            "first committed",
            "Unconfirmed turn",
            "second",
            "second suffix"
        ]
    );
}

#[test]
fn graceful_final_response_seals_the_run_without_a_run_complete_frame() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.begin_send("hello".into(), 0);
    let mut intermediate = text("r-1", 1, "progress");
    intermediate["data"]["is_final"] = json!(true);
    intermediate["data"]["is_intermediate"] = json!(true);
    assert!(model.accept(intermediate).get("terminalRun").is_none());
    let mut final_chunk = text("r-1", 2, "Stopped by hook");
    final_chunk["data"]["is_final"] = json!(true);
    assert!(model.accept(final_chunk).get("terminalRun").is_none());
    let patch = model.resolve_send("s-1".into(), Some("r-1".into()), 0);
    assert!(patch["activeRun"].is_null());
    assert_eq!(patch["reconcile"], true);
    assert_eq!(patch["terminalRun"], "r-1");
    assert_eq!(patch["upsert"][0]["streaming"], false);
    assert!(patch["upsert"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Stopped by hook"));
    assert_eq!(model.accept(end("r-1", 3))["reconcile"], false);
}

#[test]
fn an_interrupted_unconfirmed_run_requires_an_idle_read_and_explicit_resume() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.begin_send("keep this prompt".into(), 0);
    model.resolve_send("s-1".into(), Some("r-1".into()), 0);
    assert_eq!(
        model.replace_history(history(None, json!([])), Value::Null)["canResume"],
        false
    );
    model.interrupt();
    assert_eq!(
        model.replace_history(history(Some("r-1"), json!([])), Value::Null)["canResume"],
        false
    );
    let idle = model.replace_history(history(None, json!([])), Value::Null);
    assert_eq!(idle["canResume"], true);
    assert_eq!(idle["activeRun"], "r-1");
    let late_text = model.accept(text("r-1", 1, "still arriving"));
    assert_eq!(late_text["canResume"], false);
    let mut stale = history(None, json!([]));
    stale["live_status_stale"] = json!(true);
    assert_eq!(
        model.replace_history(stale, Value::Null)["canResume"],
        false
    );
    assert_eq!(model.resume_composing()["activeRun"], "r-1");
    model.replace_history(history(None, json!([])), Value::Null);
    let preview = model.resume_composing();
    assert!(preview["activeRun"].is_null());
    assert_eq!(preview["missingPrefix"], true);
    assert_eq!(preview["unconfirmed"], true);
    assert!(preview["error"].is_null());
    let snapshot = model.replace_history(history(None, json!([])), Value::Null);
    assert_eq!(snapshot["rows"][0]["text"], "Unconfirmed turn");
    assert_eq!(snapshot["rows"][1]["text"], "keep this prompt");
    let detached = model.accept(text("r-1", 2, " later"));
    assert!(detached["activeRun"].is_null());
    assert_eq!(detached["upsert"][0]["text"], "still arriving later");
    let attached = model.replace_history(history(Some("r-1"), json!([])), Value::Null);
    assert_eq!(attached["activeRun"], "r-1");
    assert_eq!(attached["rows"][1]["text"], "still arriving later");
    assert_eq!(model.accept(end("r-1", 3))["reconcile"], true);
}

#[test]
fn detached_frames_patch_the_labelled_preview_while_a_new_run_is_active() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.begin_send("earlier prompt".into(), 0);
    model.resolve_send("s-1".into(), Some("r-1".into()), 0);
    model.interrupt();
    model.replace_history(history(None, json!([])), Value::Null);
    model.resume_composing();
    model.begin_send("new prompt".into(), 0);
    model.resolve_send("s-1".into(), Some("r-2".into()), 0);
    let snapshot = model.replace_history(history(Some("r-2"), json!([])), Value::Null);
    assert_eq!(snapshot["rows"][0]["text"], "Unconfirmed turn");
    assert_eq!(snapshot["rows"][1]["text"], "earlier prompt");
    assert_eq!(snapshot["rows"][2]["text"], "new prompt");
    let old = model.accept(text("r-1", 1, "late reply"));
    assert_eq!(old["activeRun"], "r-2");
    assert_eq!(old["insertAfter"], snapshot["rows"][1]["key"]);
    assert_eq!(old["upsert"][0]["text"], "late reply");
    assert_eq!(model.accept(end("r-1", 2))["activeRun"], "r-2");
    model.accept(end("r-2", 1));
    let snapshot = model.replace_history(
        history(
            None,
            json!([
                {"role":"assistant", "content":"new committed reply", "run_id":"r-2"}
            ]),
        ),
        Value::Null,
    );
    assert_eq!(snapshot["rows"][0]["text"], "new committed reply");
    assert_eq!(snapshot["rows"][1]["text"], "Unconfirmed turn");
    assert_eq!(snapshot["rows"][2]["text"], "earlier prompt");
    assert_eq!(snapshot["rows"][3]["text"], "late reply");
}

#[test]
fn resume_inserts_a_label_for_late_first_frame_without_a_history_reload() {
    let mut model = ChatState::default();
    model.reset("s-1".into());
    model.replace_history(history(Some("r-1"), json!([])), Value::Null);
    model.interrupt();
    model.replace_history(history(None, json!([])), Value::Null);
    let parked = model.resume_composing();
    assert_eq!(parked["upsert"][0]["text"], "Unconfirmed turn");
    assert_eq!(parked["appendOrder"], json!(["unconfirmed/r-1"]));
    model.begin_send("new prompt".into(), 0);
    model.resolve_send("s-1".into(), Some("r-2".into()), 0);
    let late = model.accept(text("r-1", 1, "late first reply"));
    assert_eq!(late["insertAfter"], "unconfirmed/r-1");
    assert_eq!(late["activeRun"], "r-2");
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
    let (small, large) = (patch_size(10), patch_size(500));
    println!("one upsert: {small} serialized bytes at 10 rows; {large} at 500 rows");
    assert_eq!(small, large);
}
