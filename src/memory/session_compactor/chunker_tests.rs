use crate::context::compact::tool_aware_chunker::{
    parse_semantic_units, SemanticUnit, ToolAwareChunker,
};
use crate::providers::message::UnifiedMessage;

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
    let chunks = chunker.chunk(&units, &unified);

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
