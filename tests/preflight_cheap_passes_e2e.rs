//! End-to-end test: the `PreflightPipeline` saves tokens independently of
//! whether the `ContextCompactor` is available.
//!
//! Mirrors the runtime wiring in `harness::agent::think` — the pipeline runs
//! BEFORE the compactor, so cheap-pass savings should be observable with no
//! LLM provider present.

use std::sync::Arc;

use alephcore::context::budget::cheap_passes::{
    HistoricalImageStrippingStage, ToolResultPruningStage,
};
use alephcore::context::budget::preflight::{PreflightPipeline, PreflightStage};
use alephcore::context::budget::ContextPressure;
use alephcore::providers::message::{ContentBlock, UnifiedMessage};

fn fresh_pressure() -> ContextPressure {
    ContextPressure {
        used_tokens: 0,
        budget_tokens: 0,
        ratio: 1.0,
        overhead_tokens: 0,
        available_for_messages: 0,
    }
}

fn make_pipeline() -> PreflightPipeline {
    let stages: Vec<Box<dyn PreflightStage>> = vec![
        Box::new(ToolResultPruningStage::default()),
        Box::new(HistoricalImageStrippingStage),
    ];
    PreflightPipeline::new(stages)
}

#[tokio::test]
async fn cheap_passes_save_tokens_with_no_compactor() {
    // History that simulates a real long-running session:
    // - one large stale tool_result (will be pruned)
    // - one old image-bearing turn (will be stripped)
    // - one newest image-bearing turn (must survive)
    // - six "fresh tail" turns (must all survive)
    let huge_tool_output = "y".repeat(4000); // > 200-token threshold
    let mut messages = vec![
        UnifiedMessage::tool_result("call-a", "Bash", huge_tool_output, false),
        UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "old_image".into(),
                mime_type: "image/png".into(),
            },
            ContentBlock::Text {
                text: "old image turn".into(),
                cache_control: None,
            },
        ]),
        UnifiedMessage::user("middle turn"),
        UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "newest_image".into(),
                mime_type: "image/png".into(),
            },
            ContentBlock::Text {
                text: "newest image turn".into(),
                cache_control: None,
            },
        ]),
        UnifiedMessage::user("recent 1"),
        UnifiedMessage::user("recent 2"),
        UnifiedMessage::user("recent 3"),
        UnifiedMessage::user("recent 4"),
        UnifiedMessage::user("recent 5"),
        UnifiedMessage::user("recent 6"),
    ];
    let original_len = messages.len();

    // No compactor — pipeline runs alone (this is the "LLM unavailable" scenario)
    let pipeline = Arc::new(make_pipeline());
    let freed = pipeline.run(&mut messages, &fresh_pressure(), 6).await;

    assert!(
        freed > 1500,
        "expected substantial savings (tool_result prune > image strip ~= 1500); got {freed}"
    );
    assert_eq!(
        messages.len(),
        original_len,
        "pipeline must not change message count, only content",
    );

    // Verify the prune happened
    let (_name, pruned_text) = messages[0]
        .tool_result_info()
        .expect("first message still a ToolResult");
    assert!(
        pruned_text.starts_with("[pruned tool_result"),
        "expected pruned placeholder, got: {pruned_text}"
    );

    // Verify old image gone, newest image still present
    let old_image_msg_has_image = messages[1]
        .content_blocks()
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    let newest_image_msg_has_image = messages[3]
        .content_blocks()
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    assert!(!old_image_msg_has_image, "old image must be stripped");
    assert!(newest_image_msg_has_image, "newest image must survive");
}

#[tokio::test]
async fn pipeline_no_op_on_clean_history() {
    // History that has nothing to prune or strip
    let mut messages = vec![
        UnifiedMessage::user("hello"),
        UnifiedMessage::assistant("hi there"),
        UnifiedMessage::user("how are you"),
    ];
    let original = format!("{messages:?}");

    let pipeline = make_pipeline();
    let freed = pipeline.run(&mut messages, &fresh_pressure(), 6).await;

    assert_eq!(freed, 0, "nothing to save on clean history");
    assert_eq!(format!("{messages:?}"), original, "messages must be untouched");
}

#[tokio::test]
async fn pipeline_protects_short_histories_entirely() {
    // History shorter than fresh_tail — everything is in the protected zone
    let mut messages = vec![
        UnifiedMessage::tool_result("c1", "Read", "x".repeat(5000), false),
        UnifiedMessage::user_with_content(vec![ContentBlock::Image {
            data: "img".into(),
            mime_type: "image/png".into(),
        }]),
        UnifiedMessage::user("recent"),
    ];

    let pipeline = make_pipeline();
    let freed = pipeline.run(&mut messages, &fresh_pressure(), 6).await;

    assert_eq!(
        freed, 0,
        "fresh tail of 6 must protect everything in a 3-message history"
    );
}
