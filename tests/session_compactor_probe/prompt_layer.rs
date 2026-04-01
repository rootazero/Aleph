//! Prompt layer scenario tests.
//!
//! Tests that summaries are injected as `<session_context>` XML blocks
//! and that token budgets are respected during history assembly.

use alephcore::thinker::layers::SessionContextGuideLayer;
use alephcore::thinker::prompt_builder::PromptConfig;
use alephcore::thinker::prompt_layer::{LayerInput, PromptLayer};

// ---------------------------------------------------------------------------
// Scenario 7 — SessionContextGuideLayer
// ---------------------------------------------------------------------------

/// P7: The guide is injected when `has_session_summaries` is true.
///
/// The output must contain the section header "Session Context Notes",
/// a reference to the `memory_search` tool, and the scope keyword
/// `current_session`.
#[test]
fn p7_guide_injected_when_summaries_present() {
    let layer = SessionContextGuideLayer;
    let config = PromptConfig::default();
    let input = LayerInput::basic(&config, &[]).with_session_summaries(true);

    let mut output = String::new();
    layer.inject(&mut output, &input);

    assert!(
        output.contains("Session Context Notes"),
        "output must contain section header; got: {output:.120}"
    );
    assert!(
        output.contains("memory_search"),
        "output must mention the memory_search tool; got: {output:.120}"
    );
    assert!(
        output.contains("current_session"),
        "output must mention current_session scope; got: {output:.120}"
    );
}

/// P7: The guide is skipped when `has_session_summaries` is false (default).
///
/// `LayerInput::basic` sets `has_session_summaries = false`, so the layer
/// must produce no output at all.
#[test]
fn p7_guide_skipped_when_no_summaries() {
    let layer = SessionContextGuideLayer;
    let config = PromptConfig::default();
    let input = LayerInput::basic(&config, &[]); // has_session_summaries defaults to false

    let mut output = String::new();
    layer.inject(&mut output, &input);

    assert!(
        output.is_empty(),
        "output must be empty when has_session_summaries is false; got: {output:.120}"
    );
}
