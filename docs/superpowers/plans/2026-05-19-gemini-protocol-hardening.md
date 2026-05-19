# Gemini Protocol Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 6 defect classes (G1, G2a, G3, G4, G5, G6, G7) in Aleph's Google Gemini wire protocol — dropped image input, asymmetric tool-call IDs, unparsed errors, silent prompt-block turns, lossy finish-reason and usage mapping, and double-encoded structured tool output.

**Architecture:** Every change is confined to `src/providers/gemini/**` and `src/providers/protocols/gemini/**`. No shared type is modified, so there is zero merge-conflict surface with the four active sibling worktrees. Each gap is one TDD task with its own commit.

**Tech Stack:** Rust, `cargo test`, `serde_json`, `reqwest`, `async-trait`, `futures`.

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-gemini-wt` (branch `gemini-protocol-opt`). All paths below are relative to that root. Run all `cargo`/`git` commands from there.

**Reference spec:** `docs/superpowers/specs/2026-05-19-gemini-protocol-hardening-design.md`

---

## File Structure

| File | Responsibility | Touched by |
|------|----------------|------------|
| `src/providers/gemini/types.rs` | Gemini API type definitions | T3 (one-line `serde(rename)` fix) |
| `src/providers/protocols/gemini/sse.rs` | SSE / response-body parsing | T1, T2, T6, T7 |
| `src/providers/protocols/gemini/adapter.rs` | `ProtocolAdapter` impl: request build + stream | T1, T6 |
| `src/providers/protocols/gemini/proto_impl.rs` | Request helpers: `convert_messages` etc. | T3, T4, T5 |
| `src/providers/protocols/gemini/tests.rs` | Unit tests | T1–T7 (append tests; T6 also edits the import on line 11) |

Tasks T1, T2, T6, T7 all edit different regions of the single function `parse_gemini_sse_chunk` in `sse.rs` and do not overlap. Execute tasks in numeric order.

---

## Task 1: G6 — usage cache tokens + `top_k`

**Files:**
- Modify: `src/providers/protocols/gemini/sse.rs` (the `usageMetadata` block, ~line 139-149)
- Modify: `src/providers/protocols/gemini/adapter.rs:51`
- Test: `src/providers/protocols/gemini/tests.rs` (append)

- [ ] **Step 1: Write the failing tests**

Append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_parse_sse_cached_content_tokens() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":200,"candidatesTokenCount":10,"cachedContentTokenCount":150}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    let usage = out
        .iter()
        .find_map(|d| match d {
            Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
            _ => None,
        })
        .expect("Usage event not found");
    assert_eq!(usage.cache_read_tokens, Some(150));
}

#[test]
fn test_build_request_includes_top_k() {
    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());
    config.top_k = Some(40);

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);

    let req = protocol
        .build_request(&payload, &config)
        .expect("build_request")
        .build()
        .expect("build");
    let body_bytes = req
        .body()
        .expect("body present")
        .as_bytes()
        .expect("in-memory body");
    let body: serde_json::Value = serde_json::from_slice(body_bytes).expect("json body");
    assert_eq!(body["generationConfig"]["topK"], 40);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib test_parse_sse_cached_content_tokens test_build_request_includes_top_k`
Expected: both FAIL — `test_parse_sse_cached_content_tokens` panics with `Some(150)` vs `None`; `test_build_request_includes_top_k` panics with `Null` vs `40`.

- [ ] **Step 3: Implement**

In `src/providers/protocols/gemini/sse.rs`, inside the `if let Some(usage) = json.get("usageMetadata")` block, replace the line `cache_read_tokens: None,` with:

```rust
            cache_read_tokens: usage
                .get("cachedContentTokenCount")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
```

In `src/providers/protocols/gemini/adapter.rs`, in `build_request`, inside the `GenerationConfig { ... }` literal, replace `top_k: None,` with:

```rust
            top_k: config.top_k,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib test_parse_sse_cached_content_tokens test_build_request_includes_top_k`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/gemini/sse.rs src/providers/protocols/gemini/adapter.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: map cachedContentTokenCount to cache tokens and wire top_k"
```

---

## Task 2: G5 — `finishReason` SAFETY / RECITATION mapping

**Files:**
- Modify: `src/providers/protocols/gemini/sse.rs` (the `stop_reason` match, ~line 98-115)
- Test: `src/providers/protocols/gemini/tests.rs` (append)

- [ ] **Step 1: Write the failing tests**

Append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_parse_sse_finish_reason_safety() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::Refusal)))),
        "SAFETY should map to Done(Refusal), got {:?}",
        out
    );
}

#[test]
fn test_parse_sse_finish_reason_recitation() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"RECITATION"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::Sensitive)))),
        "RECITATION should map to Done(Sensitive), got {:?}",
        out
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib test_parse_sse_finish_reason_safety test_parse_sse_finish_reason_recitation`
Expected: both FAIL — current code maps both reasons to `StopReason::Unknown`.

- [ ] **Step 3: Implement**

In `src/providers/protocols/gemini/sse.rs`, replace the whole `let stop_reason = match finish_reason { ... };` expression with:

```rust
        let stop_reason = match finish_reason {
            Some("STOP") => Some(StopReason::EndTurn),
            Some("MAX_TOKENS") => Some(StopReason::MaxTokens),
            Some("FUNCTION_CALL") => Some(StopReason::ToolUse),
            Some("SAFETY") | Some("BLOCKLIST") | Some("PROHIBITED_CONTENT")
            | Some("SPII") => Some(StopReason::Refusal),
            Some("RECITATION") => Some(StopReason::Sensitive),
            Some(other) if !other.is_empty() => {
                // If we emitted tool calls in this same chunk, treat as ToolUse
                if has_tool_calls {
                    Some(StopReason::ToolUse)
                } else {
                    Some(StopReason::Unknown)
                }
            }
            _ => {
                // No finish reason in this chunk — check if we saw tool calls
                // without an explicit reason (some Gemini variants omit the field)
                None
            }
        };
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib test_parse_sse_finish_reason`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/gemini/sse.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: map SAFETY/RECITATION finish reasons to Refusal/Sensitive"
```

---

## Task 3: G1 — multimodal image input

**Files:**
- Modify: `src/providers/gemini/types.rs` (the `Part::InlineData` variant, ~line 72-73)
- Modify: `src/providers/protocols/gemini/proto_impl.rs` (the `convert_messages` `User` arm, ~line 47-57)
- Test: `src/providers/protocols/gemini/tests.rs` (append)

- [ ] **Step 1: Write the failing test**

Append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_convert_user_with_image() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::user_with_content(vec![
        CB::Text {
            text: "look at this".to_string(),
            cache_control: None,
        },
        CB::Image {
            data: "QUJD".to_string(),
            mime_type: "image/png".to_string(),
        },
    ])];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].parts.len(), 2, "text + image = two parts");
    match &result[0].parts[0] {
        Part::Text { text } => assert_eq!(text, "look at this"),
        other => panic!("expected Text part, got {:?}", other),
    }
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(json["parts"][1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(json["parts"][1]["inlineData"]["data"], "QUJD");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib test_convert_user_with_image`
Expected: FAIL — current `convert_messages` drops the image; `result[0].parts.len()` is 1, not 2.

- [ ] **Step 3a: Fix the `Part::InlineData` serde rename**

In `src/providers/gemini/types.rs`, in the `pub enum Part` definition, replace the `InlineData` variant:

```rust
    /// Inline image data part
    InlineData { inline_data: InlineData },
```

with:

```rust
    /// Inline image data part
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
```

- [ ] **Step 3b: Build image parts in `convert_messages`**

In `src/providers/protocols/gemini/proto_impl.rs`, replace the entire `UnifiedMessage::User { content } => { ... }` arm with:

```rust
                UnifiedMessage::User { content } => {
                    let mut parts = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                parts.push(Part::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::Image { data, mime_type } => {
                                parts.push(Part::InlineData {
                                    inline_data: crate::providers::gemini::InlineData {
                                        mime_type: mime_type.clone(),
                                        data: data.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    // Keep the request valid even if the message carried no
                    // text/image blocks (e.g. only unsupported block kinds).
                    if parts.is_empty() {
                        parts.push(Part::Text {
                            text: String::new(),
                        });
                    }
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib test_convert_user_with_image`
Expected: PASS.

Then confirm no regression in the existing convert tests:
Run: `cargo test -p alephcore --lib test_convert`
Expected: all `test_convert_*` PASS (single-text-block user messages still yield exactly one `Part::Text`).

- [ ] **Step 5: Commit**

```bash
git add src/providers/gemini/types.rs src/providers/protocols/gemini/proto_impl.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: convert image content blocks to inlineData parts"
```

---

## Task 4: G2a — tool-call ID passthrough on replay

**Files:**
- Modify: `src/providers/protocols/gemini/proto_impl.rs` (the `convert_messages` `Assistant` arm's `ToolCall` block, ~line 65-77)
- Test: `src/providers/protocols/gemini/tests.rs` (append)

- [ ] **Step 1: Write the failing test**

Append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_convert_assistant_tool_call_preserves_id() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            id: "call_xyz".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
        }],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(
        json["parts"][0]["functionCall"]["id"], "call_xyz",
        "replayed functionCall must carry the tool-call id"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib test_convert_assistant_tool_call_preserves_id`
Expected: FAIL — current code sets `id: None`, so `functionCall.id` is absent (`Null`).

- [ ] **Step 3: Implement**

In `src/providers/protocols/gemini/proto_impl.rs`, in the `UnifiedMessage::Assistant` arm, replace the `ContentBlock::ToolCall` match arm:

```rust
                            crate::providers::message::ContentBlock::ToolCall {
                                name,
                                arguments,
                                ..
                            } => {
                                parts.push(Part::FunctionCall {
                                    function_call: crate::providers::gemini::GeminiFunctionCall {
                                        name: name.clone(),
                                        args: arguments.clone(),
                                        id: None,
                                    },
                                });
                            }
```

with:

```rust
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                parts.push(Part::FunctionCall {
                                    function_call: crate::providers::gemini::GeminiFunctionCall {
                                        name: name.clone(),
                                        args: arguments.clone(),
                                        // Replay the id so the assistant's functionCall
                                        // and the matching functionResponse stay paired
                                        // (required for Gemini 3 native tool-call ids).
                                        id: Some(id.clone()),
                                    },
                                });
                            }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib test_convert_assistant_tool_call_preserves_id`
Expected: PASS. Also run `cargo test -p alephcore --lib test_convert_s3 test_convert_s5 test_convert_s8` — Expected: PASS (unchanged assertions).

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/gemini/proto_impl.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: preserve tool-call id when replaying assistant functionCall"
```

---

## Task 5: G7 — structured tool-result passthrough

**Files:**
- Modify: `src/providers/protocols/gemini/proto_impl.rs` (the `convert_messages` `ToolResult` arm, ~line 91-120)
- Test: `src/providers/protocols/gemini/tests.rs` (append)

- [ ] **Step 1: Write the failing test**

Append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_convert_tool_result_json_object_passthrough() {
    let msgs = [UnifiedMessage::tool_result_json(
        "call_1",
        "get_weather",
        serde_json::json!({"temp": 20, "unit": "C"}),
        false,
    )];
    let result = GeminiProtocol::convert_messages(&msgs);
    let json = serde_json::to_value(&result[0]).unwrap();
    let response = &json["parts"][0]["functionResponse"]["response"];
    assert_eq!(response["temp"], 20);
    assert_eq!(response["unit"], "C");
    assert!(
        response.get("result").is_none(),
        "a structured object payload must pass through unwrapped"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib test_convert_tool_result_json_object_passthrough`
Expected: FAIL — current code stringifies the JSON and wraps it as `{"result":"{\"temp\":20,...}"}`, so `response["temp"]` is `Null`.

- [ ] **Step 3: Implement**

In `src/providers/protocols/gemini/proto_impl.rs`, replace the entire `UnifiedMessage::ToolResult { ... } => { ... }` arm with:

```rust
                UnifiedMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    ..
                } => {
                    // A lone structured-JSON object passes through directly as the
                    // functionResponse payload; text / mixed content is wrapped in
                    // `{"result": ...}` so `response` is always a JSON object.
                    let response = match content.as_slice() {
                        [crate::providers::message::ContentBlock::Json { value }]
                            if value.is_object() =>
                        {
                            value.clone()
                        }
                        _ => {
                            let output = content
                                .iter()
                                .map(|b| match b {
                                    crate::providers::message::ContentBlock::Text {
                                        text,
                                        ..
                                    } => text.clone(),
                                    crate::providers::message::ContentBlock::Json { value } => {
                                        serde_json::to_string(value).unwrap_or_default()
                                    }
                                    _ => String::new(),
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            serde_json::json!({ "result": output })
                        }
                    };
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts: vec![Part::FunctionResponse {
                            function_response: crate::providers::gemini::GeminiFunctionResponse {
                                name: tool_name.clone(),
                                response,
                                id: Some(tool_call_id.clone()),
                            },
                        }],
                    });
                }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib test_convert_tool_result_json_object_passthrough`
Expected: PASS. Also run `cargo test -p alephcore --lib test_convert_s4 test_convert_s6` — Expected: PASS (text tool results still produce `{"result":"..."}`).

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/gemini/proto_impl.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: pass structured JSON tool results through without re-encoding"
```

---

## Task 6: G3 — error response parsing

**Files:**
- Modify: `src/providers/protocols/gemini/sse.rs` (add `parse_gemini_error_body` helper + mid-stream error check; add a `use`)
- Modify: `src/providers/protocols/gemini/adapter.rs` (non-2xx branch in `stream_deltas`; add to the `sse` import)
- Modify: `src/providers/protocols/gemini/tests.rs` (line 11 import + append tests)

- [ ] **Step 1: Write the failing tests**

In `src/providers/protocols/gemini/tests.rs`, replace the import line 11:

```rust
use crate::providers::protocols::gemini::sse::parse_gemini_sse_chunk;
```

with:

```rust
use crate::providers::protocols::gemini::sse::{parse_gemini_error_body, parse_gemini_sse_chunk};
```

Then append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_parse_gemini_error_body_object_form() {
    let body = r#"{"error":{"code":400,"message":"Invalid argument","status":"INVALID_ARGUMENT"}}"#;
    let err = parse_gemini_error_body(body).expect("envelope parsed");
    assert_eq!(err.code, 400);
    assert_eq!(err.message, "Invalid argument");
    assert_eq!(err.status, "INVALID_ARGUMENT");
}

#[test]
fn test_parse_gemini_error_body_array_form() {
    let body = r#"[{"error":{"code":500,"message":"Internal error","status":"INTERNAL"}}]"#;
    let err = parse_gemini_error_body(body).expect("envelope parsed");
    assert_eq!(err.code, 500);
    assert_eq!(err.status, "INTERNAL");
}

#[test]
fn test_parse_gemini_error_body_not_an_envelope() {
    assert!(parse_gemini_error_body("plain text").is_none());
    assert!(parse_gemini_error_body(r#"{"candidates":[]}"#).is_none());
}

#[test]
fn test_parse_sse_mid_stream_error_frame() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"error":{"code":500,"message":"boom","status":"INTERNAL"}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert_eq!(out.len(), 1, "error frame yields exactly one event");
    match out.front() {
        Some(Err(e)) => assert!(
            format!("{e}").contains("boom"),
            "error should carry the message: {e}"
        ),
        other => panic!("expected a fatal Err, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib test_parse_gemini_error_body test_parse_sse_mid_stream_error_frame`
Expected: FAIL to compile — `parse_gemini_error_body` does not exist yet (`cannot find function`).

- [ ] **Step 3a: Add the error-body helper and mid-stream check in `sse.rs`**

In `src/providers/protocols/gemini/sse.rs`, add this `use` after the existing `use` lines (after `use std::collections::VecDeque;`):

```rust
use crate::providers::gemini::GeminiError;
```

Add this function at the end of the file (after `parse_gemini_sse_chunk`):

```rust
/// Extract a Gemini error envelope from an HTTP error body.
///
/// Handles both the object form `{"error": {...}}` and the streaming array
/// form `[{"error": {...}}]`. Returns `None` when no envelope can be parsed.
pub(crate) fn parse_gemini_error_body(body: &str) -> Option<GeminiError> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let error_obj = match &value {
        serde_json::Value::Array(items) => items.first()?.get("error")?,
        serde_json::Value::Object(_) => value.get("error")?,
        _ => return None,
    };
    serde_json::from_value(error_obj.clone()).ok()
}
```

In `parse_gemini_sse_chunk`, immediately after the `let json: serde_json::Value = match serde_json::from_str(data) { ... };` block and before `let candidate = ...`, insert:

```rust
    // Mid-stream error frame: Gemini may deliver `{"error": {...}}` as a data
    // chunk. Surface it as a fatal stream error (matches the parse-error path).
    if let Some(err) = json.get("error") {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        let status = err.get("status").and_then(|s| s.as_str()).unwrap_or("");
        out.push_back(Err(AlephError::provider(format!(
            "Gemini stream error: {} ({})",
            message, status
        ))));
        return;
    }
```

- [ ] **Step 3b: Use the helper in `adapter.rs`**

In `src/providers/protocols/gemini/adapter.rs`, change the import line:

```rust
use super::sse::parse_gemini_sse_chunk;
```

to:

```rust
use super::sse::{parse_gemini_error_body, parse_gemini_sse_chunk};
```

In `stream_deltas`, replace the entire non-2xx block (from `if !status.is_success() {` through its closing `}`) with:

```rust
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let error_text = response.text().await.unwrap_or_default();
            // Parse Gemini's error envelope for a clean message; fall back to raw text.
            let detail = parse_gemini_error_body(&error_text)
                .map(|e| format!("{} ({})", e.message, e.status))
                .unwrap_or_else(|| error_text.clone());
            if status.as_u16() == 429 {
                let suggestion = retry_after
                    .as_ref()
                    .map(|ra| format!("Rate limited. Retry after {ra} seconds."))
                    .unwrap_or_else(|| {
                        "Rate limited. Wait before retrying or upgrade your API plan.".to_string()
                    });
                return Err(AlephError::RateLimitError {
                    message: format!("Gemini API rate limited (429): {}", detail),
                    suggestion: Some(suggestion),
                });
            }
            return Err(AlephError::provider(format!(
                "Gemini API error ({}): {}",
                status, detail
            )));
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib test_parse_gemini_error_body test_parse_sse_mid_stream_error_frame`
Expected: all four PASS.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/gemini/sse.rs src/providers/protocols/gemini/adapter.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: parse error envelope from HTTP body and mid-stream frames"
```

---

## Task 7: G4 — `promptFeedback` block detection

**Files:**
- Modify: `src/providers/protocols/gemini/sse.rs` (`parse_gemini_sse_chunk` — add a check after the Task 6 mid-stream error block)
- Test: `src/providers/protocols/gemini/tests.rs` (append)

- [ ] **Step 1: Write the failing tests**

Append to the end of `src/providers/protocols/gemini/tests.rs`:

```rust
#[test]
fn test_parse_sse_prompt_blocked() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert_eq!(out.len(), 1, "a blocked prompt yields exactly one event");
    match out.front() {
        Some(Err(e)) => assert!(
            format!("{e}").contains("SAFETY"),
            "error must name the block reason: {e}"
        ),
        other => panic!("expected Err naming SAFETY, got {:?}", other),
    }
}

#[test]
fn test_parse_sse_prompt_feedback_without_block_is_ignored() {
    // promptFeedback with only safetyRatings (no blockReason) appears on
    // successful responses and must not be treated as an error.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}],"promptFeedback":{"safetyRatings":[]}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter().all(|d| d.is_ok()),
        "no error expected, got {:?}",
        out
    );
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::TextDelta(t)) if t == "ok")),
        "text delta should still be emitted"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib test_parse_sse_prompt_blocked test_parse_sse_prompt_feedback_without_block_is_ignored`
Expected: `test_parse_sse_prompt_blocked` FAILS (`out` is empty — `promptFeedback` is ignored, `out.len()` is 0); `test_parse_sse_prompt_feedback_without_block_is_ignored` PASSES already (sanity check that the next change does not regress it).

- [ ] **Step 3: Implement**

In `src/providers/protocols/gemini/sse.rs`, in `parse_gemini_sse_chunk`, immediately after the mid-stream error block added in Task 6 (the `if let Some(err) = json.get("error") { ... }` block) and before `let candidate = ...`, insert:

```rust
    // Prompt-level block: a blocked prompt returns `promptFeedback.blockReason`
    // and no candidates. Surface it as a fatal error instead of an empty turn.
    if let Some(block_reason) = json
        .get("promptFeedback")
        .and_then(|pf| pf.get("blockReason"))
        .and_then(|r| r.as_str())
        .filter(|r| !r.is_empty())
    {
        out.push_back(Err(AlephError::provider(format!(
            "Gemini blocked the prompt (blockReason={})",
            block_reason
        ))));
        return;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib test_parse_sse_prompt_blocked test_parse_sse_prompt_feedback_without_block_is_ignored`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/gemini/sse.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: detect promptFeedback block reason instead of empty turn"
```

---

## Task 8: Final verification

**Files:** none modified unless a check fails.

- [ ] **Step 1: Run the full library test suite**

Run: `cargo test -p alephcore --lib`
Expected: PASS. Pre-existing unrelated failures may exist on `main` (see project memory "Baseline Test Failures"); confirm every `gemini`-named test and every `test_convert_*` / `test_parse_sse_*` / `test_build_request_*` test passes, and that no test that passed before this branch now fails.

- [ ] **Step 2: Run clippy on the crate**

Run: `cargo clippy -p alephcore --lib 2>&1 | grep -E "gemini|warning: unused" || echo "no gemini warnings"`
Expected: no warnings referencing files under `src/providers/gemini/` or `src/providers/protocols/gemini/`. (The crate as a whole has pre-existing warnings per project memory — only the touched files must be clean.)

- [ ] **Step 3: Format the touched files individually**

Do NOT run a project-wide `cargo fmt` (the tree is not fmt-clean — see project memory). Format only the touched files:

```bash
rustfmt src/providers/gemini/types.rs src/providers/protocols/gemini/sse.rs src/providers/protocols/gemini/adapter.rs src/providers/protocols/gemini/proto_impl.rs src/providers/protocols/gemini/tests.rs
```

- [ ] **Step 4: Commit any formatting changes**

```bash
git add src/providers/gemini/types.rs src/providers/protocols/gemini/sse.rs src/providers/protocols/gemini/adapter.rs src/providers/protocols/gemini/proto_impl.rs src/providers/protocols/gemini/tests.rs
git commit -m "gemini: rustfmt touched protocol files" || echo "nothing to format"
```

- [ ] **Step 5: Verify the branch is clean**

Run: `git status --short`
Expected: empty working tree (all changes committed).

---

## Self-Review Notes

- **Spec coverage:** G1 → T3; G2a → T4; G3 → T6; G4 → T7; G5 → T2; G6 → T1; G7 → T5; verification → T8. G2b is deferred by the spec (§11) — no task, by design.
- **Type consistency:** `parse_gemini_error_body` returns `Option<GeminiError>`; `GeminiError` fields `code: i32`, `message: String`, `status: String` (defined in `gemini/types.rs`). `parse_gemini_sse_chunk(&str, &mut u64, &mut VecDeque<Result<ProviderDelta>>)` signature is unchanged. `StopReason::Refusal` / `StopReason::Sensitive` exist in `providers/adapter.rs`. `Part::InlineData { inline_data: InlineData }` and `InlineData { mime_type, data }` are in `gemini/types.rs`.
- **Out of scope (do not touch):** `ContentBlock`, `NativeToolCall`, `ProviderDelta`, `ProviderResponse`, and any file outside the two Gemini directories. The vestigial `extract_provider_response` test scaffold in `tests.rs` is left as-is (spec §9 observation).
