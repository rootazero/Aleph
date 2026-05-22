use crate::context::compact::tool_aware_chunker::{
    parse_semantic_units, SemanticUnit, ToolAwareChunker,
};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use serde_json::json;

fn user_msg(text: &str) -> UnifiedMessage {
    UnifiedMessage::user(text)
}

fn assistant_msg(text: &str) -> UnifiedMessage {
    UnifiedMessage::assistant(text)
}

fn tool_use_msg(call_id: &str, tool_name: &str) -> UnifiedMessage {
    UnifiedMessage::Assistant {
        content: vec![ContentBlock::ToolCall {
            thought_signature: None,
            id: call_id.to_owned(),
            name: tool_name.to_owned(),
            arguments: json!({}),
        }],
    }
}

fn tool_result_msg(call_id: &str, tool_name: &str, output: &str) -> UnifiedMessage {
    UnifiedMessage::tool_result(call_id, tool_name, output, false)
}

#[test]
fn tool_use_and_result_form_single_unit() {
    let messages = vec![
        user_msg("Please search for something"),
        tool_use_msg("c1", "web_search"),
        tool_result_msg("c1", "web_search", "Found 10 results"),
        assistant_msg("Here are the results"),
    ];

    let units = parse_semantic_units(&messages);
    assert_eq!(units.len(), 2, "expected 2 semantic units, got {units:?}");
    assert!(matches!(units[0], SemanticUnit::UserMessage { index: 0 }));
    match &units[1] {
        SemanticUnit::ToolRound {
            tool_use_index,
            tool_result_index,
            follow_up_index,
            tool_name,
        } => {
            assert_eq!(*tool_use_index, 1);
            assert_eq!(*tool_result_index, 2);
            assert_eq!(*follow_up_index, Some(3));
            assert_eq!(tool_name, "web_search");
        }
        other => panic!("expected ToolRound, got {other:?}"),
    }
}

#[test]
fn tool_round_never_split_even_when_oversized() {
    let large_output = "y".repeat(500);
    let messages = vec![
        user_msg("Do something"),
        tool_use_msg("c2", "big_tool"),
        tool_result_msg("c2", "big_tool", &large_output),
        assistant_msg("Done."),
    ];

    let units = parse_semantic_units(&messages);
    let chunker = ToolAwareChunker::new(10, 4.0);
    let chunks = chunker.chunk(&units, &messages, messages.len());

    for chunk in &chunks {
        for unit in &chunk.units {
            if let SemanticUnit::ToolRound {
                tool_use_index,
                tool_result_index,
                follow_up_index,
                ..
            } = unit
            {
                let idx = chunk.message_indices();
                assert!(idx.contains(tool_use_index), "tool_use_index split");
                assert!(idx.contains(tool_result_index), "tool_result_index split");
                if let Some(fu) = follow_up_index {
                    assert!(idx.contains(fu), "follow_up_index split");
                }
            }
        }
    }
}

#[test]
fn pair_to_unified_message_conversion_preserves_structure() {
    let pairs: Vec<(String, String)> = vec![
        ("user".to_string(), "Hello".to_string()),
        ("assistant".to_string(), "Hi there".to_string()),
        ("user".to_string(), "Tell me more".to_string()),
    ];

    let unified: Vec<UnifiedMessage> = pairs
        .iter()
        .map(|(role, content)| {
            if role == "assistant" {
                UnifiedMessage::assistant(content)
            } else {
                UnifiedMessage::user(content)
            }
        })
        .collect();

    let units = parse_semantic_units(&unified);
    assert_eq!(units.len(), 3);
    assert!(matches!(units[0], SemanticUnit::UserMessage { .. }));
    assert!(matches!(units[1], SemanticUnit::AssistantText { .. }));
    assert!(matches!(units[2], SemanticUnit::UserMessage { .. }));
}

#[test]
fn chunk_indices_map_back_to_pairs() {
    let pairs: Vec<(String, String)> = (0..6)
        .map(|i| ("user".to_string(), format!("message {i}")))
        .collect();

    let unified: Vec<UnifiedMessage> = pairs.iter().map(|(_, c)| UnifiedMessage::user(c)).collect();

    let units = parse_semantic_units(&unified);
    let chunker = ToolAwareChunker::new(20, 4.0);
    let chunks = chunker.chunk(&units, &unified, unified.len());

    for chunk in &chunks {
        for idx in chunk.message_indices() {
            assert!(
                pairs.get(idx).is_some(),
                "chunk index {idx} out of bounds for pairs of len {}",
                pairs.len()
            );
        }
    }
}
