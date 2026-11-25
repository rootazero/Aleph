use super::{Change, Transcript, TranscriptEntry};
use aleph_protocol::chat_transcript::{ChatToolChange, ChatToolDelta};
use aleph_protocol::StreamEvent;

fn chunk(transcript: &mut Transcript, text: &str) -> Vec<Change> {
    transcript.apply_live(
        &StreamEvent::ResponseChunk {
            run_id: "r".into(),
            seq: 1,
            content: text.into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        },
        10,
    )
}

fn tool(call: &str, change: ChatToolChange) -> ChatToolDelta {
    ChatToolDelta {
        tool_call_id: call.into(),
        change,
    }
}

fn start() -> ChatToolChange {
    ChatToolChange::Started {
        display_name: "Edit".into(),
        args_text: "src/lib.rs".into(),
    }
}

fn finish() -> ChatToolChange {
    ChatToolChange::Finished {
        success: true,
        duration_ms: 37,
        presentation: None,
        presentation_omitted: false,
    }
}

#[test]
fn v1_tool_boundaries_do_not_reorder_or_split_the_text_after_a_tool() {
    let mut fold = Transcript::new();
    fold.adopt_live_run("r", 1);
    chunk(&mut fold, "Before");
    fold.apply_chat_tool("r", &tool("call", start()), 2);
    chunk(&mut fold, "After");
    let changes = fold.apply_chat_tool("r", &tool("call", finish()), 3);
    assert_eq!(changes.len(), 1);
    let [TranscriptEntry::Step(before), TranscriptEntry::Tool(row), TranscriptEntry::Step(after)] =
        fold.as_slice()
    else {
        panic!("expected text, tool, text in source order: {fold:?}");
    };
    assert_eq!(before.text.as_deref(), Some("Before"));
    assert_eq!(row.summary.display_name, "Edit");
    assert_eq!(row.summary.args_text, "src/lib.rs");
    assert!(row.is_terminal());
    assert_eq!(after.text.as_deref(), Some("After"));
    assert_eq!(changes, vec![Change::Updated(row.id.clone())]);
    assert!(fold
        .apply_chat_tool("r", &tool("call", start()), 4)
        .is_empty());
    assert!(fold
        .apply_chat_tool("r", &tool("call", finish()), 5)
        .is_empty());
    chunk(&mut fold, " more");
    let Some(TranscriptEntry::Step(tail)) = fold.last() else {
        panic!("lost the text tail")
    };
    assert_eq!(tail.text.as_deref(), Some("After more"));
    assert_eq!(fold.len(), 3);
}

#[test]
fn v1_rejects_foreign_tools_and_restores_terminal_calls_without_a_start() {
    let mut fold = Transcript::new();
    fold.adopt_live_run("r", 1);
    assert!(fold
        .apply_chat_tool("old", &tool("call", finish()), 2)
        .is_empty());
    assert!(fold
        .apply_chat_tool("r", &tool("working-only", ChatToolChange::Working), 2)
        .is_empty());
    assert!(fold.is_empty());
    fold.apply_chat_tool("r", &tool("call", finish()), 3);
    let [TranscriptEntry::Tool(row)] = fold.as_slice() else {
        panic!("missing terminal call")
    };
    assert!(row.is_terminal());
    assert_eq!(row.summary.display_name, "Tool");
}
