//! OpenAI-compatible data types for the Chat Completions API.

use serde::{Deserialize, Serialize};

/// A chat completion request mirroring the OpenAI API format.
#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default)]
    pub frequency_penalty: Option<f64>,
    #[serde(default)]
    pub presence_penalty: Option<f64>,
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// A chat completion response.
#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// A single choice in a chat completion response.
#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<Delta>,
}

/// A streaming delta update.
#[derive(Debug, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

/// A tool call delta in a streaming response.
#[derive(Debug, Serialize)]
pub struct DeltaToolCall {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<DeltaFunction>,
}

/// Function details within a tool call delta.
#[derive(Debug, Serialize)]
pub struct DeltaFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// A single choice in a streaming chunk response.
#[derive(Debug, Serialize)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// A streaming chat completion chunk.
#[derive(Debug, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<StreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Token usage statistics.
#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A single model object in the models listing.
#[derive(Debug, Serialize)]
pub struct ModelObject {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

/// A list of available models.
#[derive(Debug, Serialize)]
pub struct ModelList {
    pub object: String,
    pub data: Vec<ModelObject>,
}

// === Embedding types ===

#[derive(Debug, Deserialize)]
pub struct EmbeddingRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub input: EmbeddingInput,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    Single(String),
    Batch(Vec<String>),
}

#[derive(Debug, Serialize)]
pub struct EmbeddingResponse {
    pub object: String,
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingData {
    pub object: String,
    pub index: u32,
    pub embedding: Vec<f32>,
}

// === Responses API types ===

#[derive(Debug, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: ResponsesInput,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ResponsesInput {
    Text(String),
    Messages(Vec<ResponsesMessage>),
}

#[derive(Debug, Deserialize)]
pub struct ResponsesMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub status: String,
    pub model: String,
    pub output: Vec<serde_json::Value>,
    pub usage: ResponsesUsage,
}

#[derive(Debug, Serialize)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_chat_completion_request_deserializes() {
        let json_str = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello!"}
            ],
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 1024,
            "top_p": 0.9,
            "stop": ["\n"],
            "tools": [{"type": "function", "function": {"name": "get_weather"}}]
        });

        let req: ChatCompletionRequest = serde_json::from_value(json_str).unwrap();
        assert_eq!(req.model, "gpt-4");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(
            req.messages[0].content.as_deref(),
            Some("You are a helpful assistant.")
        );
        assert_eq!(req.messages[1].role, "user");
        assert_eq!(req.messages[1].content.as_deref(), Some("Hello!"));
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.top_p, Some(0.9));
        assert_eq!(req.stop.as_ref().unwrap(), &vec!["\n".to_string()]);
        assert!(req.tools.is_some());
        assert_eq!(req.tools.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_chat_completion_response_serializes() {
        let response = ChatCompletionResponse {
            id: "chatcmpl-abc123".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello! How can I help?".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
                delta: None,
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 8,
                total_tokens: 18,
            }),
        };

        let json_val = serde_json::to_value(&response).unwrap();
        assert_eq!(json_val["id"], "chatcmpl-abc123");
        assert_eq!(json_val["object"], "chat.completion");
        assert_eq!(json_val["created"], 1700000000_u64);
        assert_eq!(json_val["model"], "gpt-4");
        assert_eq!(json_val["choices"][0]["index"], 0);
        assert_eq!(
            json_val["choices"][0]["message"]["content"],
            "Hello! How can I help?"
        );
        assert_eq!(json_val["choices"][0]["finish_reason"], "stop");
        // delta should be absent (skip_serializing_if)
        assert!(json_val["choices"][0].get("delta").is_none());
        assert_eq!(json_val["usage"]["prompt_tokens"], 10);
        assert_eq!(json_val["usage"]["completion_tokens"], 8);
        assert_eq!(json_val["usage"]["total_tokens"], 18);
    }

    #[test]
    fn test_streaming_chunk_serializes() {
        let chunk = ChatCompletionResponse {
            id: "chatcmpl-abc123".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: None,
                delta: Some(Delta {
                    content: Some("Hello".to_string()),
                    role: None,
                    tool_calls: None,
                }),
            }],
            usage: None,
        };

        let json_val = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json_val["object"], "chat.completion.chunk");
        assert_eq!(json_val["choices"][0]["delta"]["content"], "Hello");
        // role in delta should be absent (skip_serializing_if)
        assert!(json_val["choices"][0]["delta"].get("role").is_none());
        // finish_reason should be absent
        assert!(json_val["choices"][0].get("finish_reason").is_none());
        // usage should be absent
        assert!(json_val.get("usage").is_none());
    }

    #[test]
    fn test_model_object_serializes() {
        let model = ModelObject {
            id: "gpt-4".to_string(),
            object: "model".to_string(),
            created: 1700000000,
            owned_by: "openai".to_string(),
        };

        let json_val = serde_json::to_value(&model).unwrap();
        assert_eq!(json_val["id"], "gpt-4");
        assert_eq!(json_val["object"], "model");
        assert_eq!(json_val["created"], 1700000000_u64);
        assert_eq!(json_val["owned_by"], "openai");
    }

    #[test]
    fn test_minimal_request_deserializes() {
        let json_str = json!({
            "model": "gpt-3.5-turbo",
            "messages": [
                {"role": "user", "content": "Hi"}
            ]
        });

        let req: ChatCompletionRequest = serde_json::from_value(json_str).unwrap();
        assert_eq!(req.model, "gpt-3.5-turbo");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.messages[0].content.as_deref(), Some("Hi"));
        assert!(req.stream.is_none());
        assert!(req.temperature.is_none());
        assert!(req.max_tokens.is_none());
        assert!(req.top_p.is_none());
        assert!(req.stop.is_none());
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert!(req.frequency_penalty.is_none());
        assert!(req.presence_penalty.is_none());
    }

    #[test]
    fn test_request_with_new_fields_deserializes() {
        let json_str = json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "Hello"}],
            "tool_choice": "auto",
            "frequency_penalty": 0.5,
            "presence_penalty": 0.3
        });

        let req: ChatCompletionRequest = serde_json::from_value(json_str).unwrap();
        assert_eq!(req.tool_choice, Some(json!("auto")));
        assert_eq!(req.frequency_penalty, Some(0.5));
        assert_eq!(req.presence_penalty, Some(0.3));
    }

    #[test]
    fn test_delta_with_tool_calls_serializes() {
        let delta = Delta {
            content: None,
            role: None,
            tool_calls: Some(vec![DeltaToolCall {
                index: 0,
                id: Some("call_abc".to_string()),
                r#type: Some("function".to_string()),
                function: Some(DeltaFunction {
                    name: Some("get_weather".to_string()),
                    arguments: Some("{\"city\":".to_string()),
                }),
            }]),
        };

        let json_val = serde_json::to_value(&delta).unwrap();
        assert!(json_val.get("content").is_none());
        assert!(json_val.get("role").is_none());
        let tc = &json_val["tool_calls"][0];
        assert_eq!(tc["index"], 0);
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "get_weather");
        assert_eq!(tc["function"]["arguments"], "{\"city\":");
    }

    #[test]
    fn test_delta_tool_call_partial_serializes() {
        // Subsequent chunks only carry argument fragments, no id/type/name
        let delta = Delta {
            content: None,
            role: None,
            tool_calls: Some(vec![DeltaToolCall {
                index: 0,
                id: None,
                r#type: None,
                function: Some(DeltaFunction {
                    name: None,
                    arguments: Some("\"NYC\"}".to_string()),
                }),
            }]),
        };

        let json_val = serde_json::to_value(&delta).unwrap();
        let tc = &json_val["tool_calls"][0];
        assert_eq!(tc["index"], 0);
        assert!(tc.get("id").is_none());
        assert!(tc.get("type").is_none());
        assert!(tc["function"].get("name").is_none());
        assert_eq!(tc["function"]["arguments"], "\"NYC\"}");
    }

    #[test]
    fn test_stream_choice_serializes() {
        let choice = StreamChoice {
            index: 0,
            delta: Delta {
                content: Some("Hi".to_string()),
                role: Some("assistant".to_string()),
                tool_calls: None,
            },
            finish_reason: None,
        };

        let json_val = serde_json::to_value(&choice).unwrap();
        assert_eq!(json_val["index"], 0);
        assert_eq!(json_val["delta"]["content"], "Hi");
        assert_eq!(json_val["delta"]["role"], "assistant");
        assert!(json_val.get("finish_reason").is_none());
    }

    #[test]
    fn test_chat_completion_chunk_serializes() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-chunk1".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![StreamChoice {
                index: 0,
                delta: Delta {
                    content: Some("Hello".to_string()),
                    role: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };

        let json_val = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json_val["id"], "chatcmpl-chunk1");
        assert_eq!(json_val["object"], "chat.completion.chunk");
        assert_eq!(json_val["created"], 1700000000_u64);
        assert_eq!(json_val["model"], "gpt-4");
        assert_eq!(json_val["choices"][0]["delta"]["content"], "Hello");
        assert!(json_val.get("usage").is_none());
    }

    #[test]
    fn test_chat_completion_chunk_with_usage() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-final".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![StreamChoice {
                index: 0,
                delta: Delta {
                    content: None,
                    role: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            }),
        };

        let json_val = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json_val["choices"][0]["finish_reason"], "stop");
        assert_eq!(json_val["usage"]["total_tokens"], 30);
    }

    #[test]
    fn test_embedding_input_single_deserializes() {
        let json_str = json!({
            "model": "text-embedding-ada-002",
            "input": "Hello world"
        });

        let req: EmbeddingRequest = serde_json::from_value(json_str).unwrap();
        assert_eq!(req.model, Some("text-embedding-ada-002".to_string()));
        match req.input {
            EmbeddingInput::Single(s) => assert_eq!(s, "Hello world"),
            _ => panic!("Expected Single variant"),
        }
    }

    #[test]
    fn test_embedding_input_batch_deserializes() {
        let json_str = json!({
            "input": ["Hello", "World"]
        });

        let req: EmbeddingRequest = serde_json::from_value(json_str).unwrap();
        assert!(req.model.is_none());
        match req.input {
            EmbeddingInput::Batch(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0], "Hello");
                assert_eq!(v[1], "World");
            }
            _ => panic!("Expected Batch variant"),
        }
    }

    #[test]
    fn test_embedding_response_serializes() {
        let response = EmbeddingResponse {
            object: "list".to_string(),
            data: vec![EmbeddingData {
                object: "embedding".to_string(),
                index: 0,
                embedding: vec![0.1, 0.2, 0.3],
            }],
            model: "text-embedding-ada-002".to_string(),
            usage: Usage {
                prompt_tokens: 5,
                completion_tokens: 0,
                total_tokens: 5,
            },
        };

        let json_val = serde_json::to_value(&response).unwrap();
        assert_eq!(json_val["object"], "list");
        assert_eq!(json_val["data"][0]["object"], "embedding");
        assert_eq!(json_val["data"][0]["index"], 0);
        let emb = json_val["data"][0]["embedding"].as_array().unwrap();
        assert_eq!(emb.len(), 3);
        assert_eq!(json_val["model"], "text-embedding-ada-002");
        assert_eq!(json_val["usage"]["prompt_tokens"], 5);
    }

    #[test]
    fn test_responses_input_text_deserializes() {
        let json_str = json!({
            "model": "gpt-4",
            "input": "Hello!"
        });

        let req: ResponsesRequest = serde_json::from_value(json_str).unwrap();
        assert_eq!(req.model, "gpt-4");
        match req.input {
            ResponsesInput::Text(s) => assert_eq!(s, "Hello!"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_responses_input_messages_deserializes() {
        let json_str = json!({
            "model": "gpt-4",
            "input": [
                {"role": "user", "content": "Hi there"}
            ],
            "instructions": "Be helpful",
            "temperature": 0.5
        });

        let req: ResponsesRequest = serde_json::from_value(json_str).unwrap();
        assert_eq!(req.model, "gpt-4");
        match req.input {
            ResponsesInput::Messages(msgs) => {
                assert_eq!(msgs.len(), 1);
                assert_eq!(msgs[0].role, "user");
            }
            _ => panic!("Expected Messages variant"),
        }
        assert_eq!(req.instructions.as_deref(), Some("Be helpful"));
        assert_eq!(req.temperature, Some(0.5));
    }

    #[test]
    fn test_tool_call_id_deserializes() {
        let json_str = json!({
            "role": "tool",
            "content": "42",
            "tool_call_id": "call_abc123"
        });

        let msg: ChatMessage = serde_json::from_value(json_str).unwrap();
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.tool_call_id, Some("call_abc123".to_string()));
    }

    #[test]
    fn test_tool_call_id_absent_by_default() {
        let json_str = json!({
            "role": "user",
            "content": "Hello"
        });

        let msg: ChatMessage = serde_json::from_value(json_str).unwrap();
        assert!(msg.tool_call_id.is_none());
    }
}
