//! `convert_messages` tests — covers every shape the adapter must lift into
//! Anthropic's `messages` array (text, tool-call, tool-result, multi-turn,
//! image, signed/unsigned thinking) plus `ThinkingBlock` serialization.

use super::super::AnthropicProtocol;
use crate::providers::anthropic::types::{ContentBlock, MessageContent};
use crate::providers::message::UnifiedMessage;

#[test]
fn test_convert_s1_pure_text_user() {
    let msgs = [UnifiedMessage::user("Hello, Claude!")];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    // Single text user message uses Text variant (not Multimodal)
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "Hello, Claude!");
}

#[test]
fn test_convert_s2_multi_turn() {
    let msgs = [
        UnifiedMessage::user("What is Rust?"),
        UnifiedMessage::assistant("Rust is a systems programming language."),
        UnifiedMessage::user("Tell me more."),
    ];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 3);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[1].role, "assistant");
    assert_eq!(result[2].role, "user");
}

#[test]
fn test_convert_s3_assistant_text_and_tool_call() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            CB::Text {
                text: "Let me search for that.".to_string(),
                cache_control: None,
            },
            CB::ToolCall {
                thought_signature: None,
                id: "toolu_123".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "rust"}),
            },
        ],
    }];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "assistant");
    let json = serde_json::to_value(&result[0]).unwrap();
    let content = json["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Let me search for that.");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(content[1]["name"], "search");
    assert_eq!(content[1]["id"], "toolu_123");
    assert_eq!(content[1]["input"]["query"], "rust");
}

#[test]
fn test_convert_s4_tool_result() {
    let msgs = [UnifiedMessage::tool_result(
        "toolu_123",
        "search",
        "Found 3 results about Rust.",
        false,
    )];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    let json = serde_json::to_value(&result[0]).unwrap();
    let content = json["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "toolu_123");
    assert_eq!(content[0]["content"], "Found 3 results about Rust.");
    // is_error is false, should be omitted (skip_serializing_if)
    assert!(content[0].get("is_error").is_none());
}

#[test]
fn test_convert_s5_full_cycle() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [
        UnifiedMessage::user("Search for Rust"),
        UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                thought_signature: None,
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "Rust"}),
            }],
        },
        UnifiedMessage::tool_result("call_1", "search", "Rust is great", false),
        UnifiedMessage::assistant("Based on the results, Rust is great!"),
    ];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[1].role, "assistant");
    assert_eq!(result[2].role, "user"); // tool_result wrapped as user
    assert_eq!(result[3].role, "assistant");
}

#[test]
fn test_convert_s6_multiple_tool_calls() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            CB::ToolCall {
                thought_signature: None,
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust"}),
            },
            CB::ToolCall {
                thought_signature: None,
                id: "call_2".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/a.rs"}),
            },
        ],
    }];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    let json = serde_json::to_value(&result[0]).unwrap();
    let content = json["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["name"], "search");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(content[1]["name"], "read_file");
}

#[test]
fn test_convert_s7_consecutive_tool_results_merge() {
    let msgs = [
        UnifiedMessage::tool_result("call_1", "search", "result 1", false),
        UnifiedMessage::tool_result("call_2", "read_file", "result 2", false),
    ];
    let result = AnthropicProtocol::convert_messages(&msgs);

    // Consecutive ToolResults should merge into ONE user message
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    let json = serde_json::to_value(&result[0]).unwrap();
    let content = json["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "call_1");
    assert_eq!(content[1]["type"], "tool_result");
    assert_eq!(content[1]["tool_use_id"], "call_2");
}

#[test]
fn test_convert_s8_error_tool_result() {
    let msgs = [UnifiedMessage::tool_result(
        "call_err",
        "search",
        "Connection timed out",
        true,
    )];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    let json = serde_json::to_value(&result[0]).unwrap();
    let content = json["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["content"], "Connection timed out");
    assert_eq!(content[0]["is_error"], true);
}

#[test]
fn test_convert_s9_tool_id_sanitization() {
    use crate::providers::message::ContentBlock as CB;
    let long_special_id = "call/foo@bar#1!!!!".to_string();
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            thought_signature: None,
            id: long_special_id,
            name: "test".to_string(),
            arguments: serde_json::json!({}),
        }],
    }];
    let result = AnthropicProtocol::convert_messages(&msgs);

    let json = serde_json::to_value(&result[0]).unwrap();
    let content = json["content"].as_array().unwrap();
    let id = content[0]["id"].as_str().unwrap();
    // Special chars replaced with '_'
    assert_eq!(id, "call_foo_bar_1____");
    // No special chars remain
    assert!(id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));

    // Also test max 64 char truncation
    let long_id = "a".repeat(100);
    let msgs2 = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            thought_signature: None,
            id: long_id,
            name: "test".to_string(),
            arguments: serde_json::json!({}),
        }],
    }];
    let result2 = AnthropicProtocol::convert_messages(&msgs2);
    let json2 = serde_json::to_value(&result2[0]).unwrap();
    let content2 = json2["content"].as_array().unwrap();
    let id2 = content2[0]["id"].as_str().unwrap();
    assert_eq!(id2.len(), 64);
}

#[test]
fn test_convert_s10_image_content() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::User {
        content: vec![
            CB::Text {
                text: "What is in this image?".to_string(),
                cache_control: None,
            },
            CB::Image {
                data: "aGVsbG8=".to_string(),
                mime_type: "image/png".to_string(),
            },
        ],
    }];
    let result = AnthropicProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    let json = serde_json::to_value(&result[0]).unwrap();
    // With multiple blocks, should be Multimodal (array content)
    let content = json["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "What is in this image?");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert_eq!(content[1]["source"]["data"], "aGVsbG8=");
}

#[test]
fn test_thinking_block_enabled_serialization() {
    use crate::providers::anthropic::types::ThinkingBlock;
    let block = ThinkingBlock {
        thinking_type: "enabled".to_string(),
        budget_tokens: Some(10000),
        display: None,
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "enabled");
    assert_eq!(json["budget_tokens"], 10000);
    assert!(json.get("display").is_none());
}

#[test]
fn test_thinking_block_adaptive_serialization() {
    use crate::providers::anthropic::types::ThinkingBlock;
    let block = ThinkingBlock {
        thinking_type: "adaptive".to_string(),
        budget_tokens: None,
        display: Some("summarized".to_string()),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "adaptive");
    assert!(json.get("budget_tokens").is_none());
    assert_eq!(json["display"], "summarized");
}

#[test]
fn test_convert_assistant_with_signed_thinking_and_tool_use() {
    use crate::providers::message::{ContentBlock as UContentBlock, UnifiedMessage};
    let messages = vec![UnifiedMessage::Assistant {
        content: vec![
            UContentBlock::Thinking {
                thinking: "Let me think...".to_string(),
                signature: Some("sig_abc123".to_string()),
            },
            UContentBlock::ToolCall {
                thought_signature: None,
                id: "toolu_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust"}),
            },
        ],
    }];
    let converted = AnthropicProtocol::convert_messages(&messages);
    assert_eq!(converted.len(), 1);
    let blocks = match &converted[0].content {
        MessageContent::Multimodal { content } => content,
        _ => panic!("expected multimodal assistant message"),
    };
    // First block: Thinking with signature
    match &blocks[0] {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "Let me think...");
            assert_eq!(signature, "sig_abc123");
        }
        other => panic!("expected Thinking block first, got {:?}", other),
    }
    // Second block: ToolUse
    assert!(matches!(blocks[1], ContentBlock::ToolUse { .. }));
}

#[test]
fn test_convert_assistant_drops_unsigned_thinking() {
    use crate::providers::message::{ContentBlock as UContentBlock, UnifiedMessage};
    let messages = vec![UnifiedMessage::Assistant {
        content: vec![
            UContentBlock::Thinking {
                thinking: "unsigned reasoning".to_string(),
                signature: None,
            },
            UContentBlock::Text {
                text: "answer".to_string(),
                cache_control: None,
            },
        ],
    }];
    let converted = AnthropicProtocol::convert_messages(&messages);
    let blocks = match &converted[0].content {
        MessageContent::Multimodal { content } => content,
        MessageContent::Text { .. } => return, // collapsed-text path is also acceptable
    };
    // Unsigned thinking must be dropped (would be rejected by Anthropic API)
    assert!(!blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Thinking { .. })));
}
