//! Request/response shapes for the BFL (Black Forest Labs) FLUX API.
//!
//! BFL splits each generation into two REST calls:
//!
//! 1. `POST /v1/<model>` returns a small `{ id, polling_url? }` envelope.
//! 2. `GET /v1/get_result?id=<id>` returns `{ id, status, result }`, where
//!    `status` walks through `Queued → Pending → Ready` (or terminal-error
//!    states), and `result.sample` is the signed image URL once ready.
//!
//! Status values per `docs.bfl.ai`:
//!
//! | status                 | terminal | meaning                                    |
//! |------------------------|----------|--------------------------------------------|
//! | `Queued`               | no       | submitted, awaiting GPU                    |
//! | `Pending`              | no       | running                                    |
//! | `Ready`                | yes      | success — `result.sample` is the image URL |
//! | `Error`                | yes      | render failed                              |
//! | `Request Moderated`    | yes      | prompt blocked by moderation               |
//! | `Content Moderated`    | yes      | output blocked by moderation               |
//! | `Task not found`       | yes      | id unknown (expired or invalid)            |

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `POST /v1/<model>` request body. We only serialize the fields that callers
/// actually set — `skip_serializing_if = "Option::is_none"` keeps the body
/// minimal so BFL's per-model defaults kick in.
#[derive(Debug, Clone, Serialize)]
pub struct BflGenerateRequest {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// 0-6 — BFL safety filter strictness (lower = stricter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_tolerance: Option<u32>,
    /// `"jpeg"` or `"png"`. BFL default is `"jpeg"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// Free-form passthrough for model-specific knobs (e.g. `aspect_ratio`
    /// for FLUX.1.1 Pro Ultra). Flattened into the request body.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extras: Option<Value>,
}

/// Initial `POST /v1/<model>` response. `polling_url` is the canonical
/// follow-up endpoint when present; otherwise we fall back to `get_result`.
#[derive(Debug, Clone, Deserialize)]
pub struct BflSubmitResponse {
    pub id: String,
    #[serde(default)]
    pub polling_url: Option<String>,
}

/// Polling response from `GET /v1/get_result?id=<id>`.
#[derive(Debug, Clone, Deserialize)]
pub struct BflPollResponse {
    #[serde(default)]
    #[allow(dead_code)]
    pub id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub result: Option<BflResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BflResult {
    #[serde(default)]
    pub sample: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub duration: Option<f32>,
    #[serde(default)]
    #[allow(dead_code)]
    pub start_time: Option<f64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub end_time: Option<f64>,
}

/// Best-effort error envelope. BFL sometimes returns `{ "error": "msg" }`,
/// sometimes `{ "detail": "..." }`, sometimes plain text.
#[derive(Debug, Clone, Deserialize)]
pub struct BflError {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub detail: Option<Value>,
    #[serde(default)]
    pub message: Option<String>,
}

impl BflError {
    #[must_use]
    pub fn best_message(&self) -> Option<String> {
        if let Some(e) = &self.error {
            return Some(e.clone());
        }
        if let Some(m) = &self.message {
            return Some(m.clone());
        }
        match &self.detail {
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => Some(other.to_string()),
            None => None,
        }
    }
}

/// Classify a BFL poll status. Terminal-error variants share their string so
/// the provider can surface a useful message to the caller.
#[must_use]
pub fn classify_status(status: &str) -> StatusClass {
    match status {
        "Ready" => StatusClass::Success,
        "Queued" | "Pending" => StatusClass::Pending,
        "Error" => StatusClass::Error,
        "Request Moderated" | "Content Moderated" => StatusClass::Moderated,
        "Task not found" => StatusClass::NotFound,
        _ => StatusClass::Unknown,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusClass {
    Success,
    Pending,
    Error,
    Moderated,
    NotFound,
    Unknown,
}
