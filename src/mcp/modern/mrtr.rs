//! Multi Round-Trip Requests (MRTR).
//!
//! `2026-07-28` removed server-initiated JSON-RPC requests. A server that needs
//! something from the client mid-call — an LLM completion, a prompt to the
//! user, the client's roots — no longer opens a request of its own on a
//! held-open stream. It answers the *original* request with an
//! `InputRequiredResult`, and the client retries that request carrying the
//! answers:
//!
//! ```text
//! client  tools/call (id: 1)                      ──▶ server
//! client  ◀── result{resultType: input_required,      inputRequests, requestState}
//! client  tools/call (id: 2, + inputResponses, requestState) ──▶ server
//! client  ◀── result{resultType: complete}
//! ```
//!
//! Two consequences worth stating plainly:
//!
//! - Each leg is an independent request, so the retry **must** use a fresh
//!   JSON-RPC id. Reusing the id is what a stateful client would do, and it is
//!   exactly what this revision removed.
//! - `requestState` is the server's own state, round-tripped through the
//!   client. It is opaque: the client echoes it byte-for-byte and never
//!   inspects, parses, or synthesizes one.
//!
//! This also fixes something that was broken before: server-driven sampling
//! used to require the SSE transport, because plain HTTP POST had no channel
//! for a server-initiated request. Under MRTR it is just a retry, so sampling
//! now works on every transport.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{AlephError, Result};
use crate::mcp::sampling::SamplingHandler;

use super::{result_kind, ResultKind};

/// Params member carrying the client's answers on a retry.
pub const INPUT_RESPONSES_FIELD: &str = "inputResponses";
/// Params member echoing the server's opaque state on a retry.
pub const REQUEST_STATE_FIELD: &str = "requestState";
/// Result member carrying the server's requests for more input.
const INPUT_REQUESTS_FIELD: &str = "inputRequests";

/// The sampling method, the one input request Aleph declares it can service.
pub const SAMPLING_METHOD: &str = "sampling/createMessage";

/// How many times a single call may be re-sent to satisfy input requests.
///
/// Servers are explicitly allowed to ask again on every attempt, so this bound
/// is what stops a buggy or hostile server from pinning a connection — and each
/// round costs an LLM call or a user prompt, not just a round-trip. Four leaves
/// room for a genuine multi-step interaction while keeping the worst case
/// finite.
pub const MAX_ROUNDS: usize = 4;

/// Whether a method may receive an `InputRequiredResult`.
///
/// The spec lists exactly three; a server must not send one anywhere else, and
/// treating an unexpected one as data rather than as an interim result would
/// hand the caller a body it cannot use.
#[must_use]
pub fn supports_input_required(method: &str) -> bool {
    matches!(method, "tools/call" | "resources/read" | "prompts/get")
}

/// One server-to-client request nested inside `inputRequests`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct InputRequest {
    /// The method the server is asking the client to perform.
    pub method: String,
    /// Its parameters, in the shape that method defines.
    #[serde(default)]
    pub params: Option<Value>,
}

/// A parsed `InputRequiredResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputRequired {
    /// Server-assigned key → the request to fulfill. Ordered so retries are
    /// deterministic and testable.
    pub requests: BTreeMap<String, InputRequest>,
    /// Opaque server state to echo back verbatim, when the server supplied one.
    pub request_state: Option<String>,
}

impl InputRequired {
    /// Parse an interim result, or `None` if this result is a final one.
    ///
    /// Relies on [`result_kind`], so a result from an earlier revision — which
    /// carries no `resultType` — can never be mistaken for an input request.
    #[must_use]
    pub fn from_result(result: &Value) -> Option<Self> {
        if result_kind(result) != ResultKind::InputRequired {
            return None;
        }

        let requests = result
            .get(INPUT_REQUESTS_FIELD)
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(key, raw)| {
                        match serde_json::from_value::<InputRequest>(raw.clone()) {
                            Ok(request) => Some((key.clone(), request)),
                            Err(e) => {
                                tracing::warn!(
                                    key = %key,
                                    error = %e,
                                    "MCP server sent a malformed entry in inputRequests; skipping"
                                );
                                None
                            }
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            requests,
            request_state: result
                .get(REQUEST_STATE_FIELD)
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// Whether the client may retry immediately without gathering anything.
    ///
    /// A server is allowed to return `requestState` alone — for instance to
    /// resume a long operation — and the spec then permits an immediate retry.
    #[must_use]
    pub fn needs_input(&self) -> bool {
        !self.requests.is_empty()
    }
}

/// Build the params for a retry leg.
///
/// The original params are carried over unchanged and joined by
/// `inputResponses` and — only when the server sent one — `requestState`. The
/// client must never invent a `requestState` the server did not supply, so
/// `None` means the member is absent rather than null.
#[must_use]
pub fn retry_params(
    original: &Value,
    responses: Map<String, Value>,
    request_state: Option<&str>,
) -> Value {
    let mut params = original.as_object().cloned().unwrap_or_default();

    params.insert(INPUT_RESPONSES_FIELD.to_string(), Value::Object(responses));
    match request_state {
        Some(state) => {
            params.insert(
                REQUEST_STATE_FIELD.to_string(),
                Value::String(state.to_string()),
            );
        }
        None => {
            params.remove(REQUEST_STATE_FIELD);
        }
    }

    Value::Object(params)
}

/// Produce the client's answer to one input request.
///
/// Only `sampling/createMessage` is serviced, because that is the only
/// capability Aleph declares (see
/// [`super::aleph_client_capabilities`]). A conformant server never asks for
/// anything else; one that does gets a precise error naming the method rather
/// than a stalled call, because the alternative — silently omitting the answer
/// — makes the server ask again and burns every remaining round.
pub async fn fulfill(
    key: &str,
    request: &InputRequest,
    sampling: &SamplingHandler,
    server_name: &str,
) -> Result<Value> {
    if request.method != SAMPLING_METHOD {
        return Err(AlephError::IoError(format!(
            "MCP server '{server_name}' asked for '{}' via inputRequests, \
             but Aleph only declares the '{SAMPLING_METHOD}' capability",
            request.method
        )));
    }

    // rust-doctor-disable-next-line excessive-clone
    let params = request.params.clone().unwrap_or(Value::Null);
    let response = sampling
        .handle_request(Value::String(key.to_string()), params, server_name)
        .await?;

    serde_json::to_value(&response)
        .map_err(|e| AlephError::IoError(format!("Failed to serialize sampling response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_three_methods_may_receive_an_interim_result() {
        assert!(supports_input_required("tools/call"));
        assert!(supports_input_required("resources/read"));
        assert!(supports_input_required("prompts/get"));

        assert!(!supports_input_required("tools/list"));
        assert!(!supports_input_required("server/discover"));
        assert!(!supports_input_required("resources/templates/list"));
    }

    #[test]
    fn parses_the_spec_example() {
        let result = json!({
            "resultType": "input_required",
            "inputRequests": {
                "github_login": {
                    "method": "elicitation/create",
                    "params": {"mode": "form", "message": "Please provide your GitHub username"}
                },
                "capital_of_france": {
                    "method": "sampling/createMessage",
                    "params": {"maxTokens": 100}
                }
            },
            "requestState": "AEAD-protected blob"
        });

        let parsed = InputRequired::from_result(&result).unwrap();
        assert_eq!(parsed.requests.len(), 2);
        assert_eq!(
            parsed.requests["capital_of_france"].method,
            "sampling/createMessage"
        );
        assert_eq!(parsed.request_state.as_deref(), Some("AEAD-protected blob"));
        assert!(parsed.needs_input());
    }

    #[test]
    fn final_results_are_not_interim_ones() {
        assert!(
            InputRequired::from_result(&json!({"resultType": "complete", "content": []})).is_none()
        );
        // A result from an earlier revision carries no resultType at all.
        assert!(InputRequired::from_result(&json!({"content": []})).is_none());
        // Not even one that happens to carry a lookalike member.
        assert!(InputRequired::from_result(&json!({"inputRequests": {}})).is_none());
    }

    #[test]
    fn request_state_alone_permits_an_immediate_retry() {
        let parsed = InputRequired::from_result(&json!({
            "resultType": "input_required",
            "requestState": "opaque"
        }))
        .unwrap();

        assert!(!parsed.needs_input());
        assert_eq!(parsed.request_state.as_deref(), Some("opaque"));
    }

    #[test]
    fn malformed_entries_are_skipped_not_fatal() {
        // One unusable entry must not discard the ones the client can answer.
        let parsed = InputRequired::from_result(&json!({
            "resultType": "input_required",
            "inputRequests": {
                "good": {"method": "sampling/createMessage"},
                "bad": {"no_method_here": true}
            }
        }))
        .unwrap();

        assert_eq!(parsed.requests.len(), 1);
        assert!(parsed.requests.contains_key("good"));
    }

    #[test]
    fn retry_carries_the_original_params_plus_answers() {
        let original = json!({"name": "get_weather", "arguments": {"city": "Seattle"}});
        let mut responses = Map::new();
        responses.insert("k".to_string(), json!({"action": "accept"}));

        let retry = retry_params(&original, responses, Some("opaque-state"));

        assert_eq!(retry["name"], "get_weather");
        assert_eq!(retry["arguments"]["city"], "Seattle");
        assert_eq!(retry[INPUT_RESPONSES_FIELD]["k"]["action"], "accept");
        assert_eq!(retry[REQUEST_STATE_FIELD], "opaque-state");
    }

    #[test]
    fn retry_omits_request_state_the_server_did_not_send() {
        // "If the InputRequiredResult does not contain a requestState field,
        // the client MUST NOT include one in the retry."
        let original = json!({"name": "t", "requestState": "stale-from-a-previous-leg"});

        let retry = retry_params(&original, Map::new(), None);

        assert!(retry.get(REQUEST_STATE_FIELD).is_none());
    }

    #[test]
    fn retry_echoes_request_state_verbatim() {
        // Opaque means opaque: no trimming, no re-encoding, no interpretation.
        let state = "  {\"not\":\"json we may touch\"}  \u{1f512}";
        let retry = retry_params(&json!({}), Map::new(), Some(state));

        assert_eq!(retry[REQUEST_STATE_FIELD], state);
    }

    #[tokio::test]
    async fn undeclared_capabilities_fail_fast_and_name_the_method() {
        let handler = SamplingHandler::new();
        let request = InputRequest {
            method: "elicitation/create".to_string(),
            params: Some(json!({"message": "who are you?"})),
        };

        let err = fulfill("k", &request, &handler, "srv").await.unwrap_err();
        let message = err.to_string();

        assert!(message.contains("elicitation/create"), "{message}");
        assert!(message.contains("srv"), "{message}");
    }

    #[tokio::test]
    async fn sampling_requests_run_through_the_existing_handler() {
        let handler = SamplingHandler::new();
        handler
            .set_callback(|_req| async {
                Ok(SamplingHandler::text_response(
                    "the capital of France is Paris",
                ))
            })
            .await;

        let request = InputRequest {
            method: SAMPLING_METHOD.to_string(),
            params: Some(json!({
                "messages": [{"role": "user", "content": {"type": "text", "text": "capital?"}}],
                "maxTokens": 100
            })),
        };

        let answer = fulfill("capital_of_france", &request, &handler, "srv")
            .await
            .unwrap();

        assert_eq!(answer["role"], "assistant");
        assert_eq!(answer["content"]["text"], "the capital of France is Paris");
    }
}
