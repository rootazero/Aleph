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
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
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
                },
                finish_reason: None,
                delta: Some(Delta {
                    content: Some("Hello".to_string()),
                    role: None,
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
    }
}
