# Anthropic Protocol Cycle 4 — Parity & Capability Matrix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire missing `ProviderConfig` fields into Anthropic's `MessagesRequest`, add a parallel capability matrix (Official + Custom) so per-host gating lives in one place, and add two new Anthropic-only fields (`effort`, `metadata_user_id`).

**Architecture:** New sibling module `src/providers/protocols/anthropic/provider_policy.rs` mirrors `openai_common/provider_policy.rs`. Capability bits resolve from `config.base_url → AnthropicEndpointClass → AnthropicCapabilities`. `build_request` populates every field unconditionally, then `AnthropicPolicy::apply(&mut serde_json::Value)` strips fields whose capability bit is `false`. `cache_control` is gated at injection time, not stripped post-hoc.

**Tech Stack:** Rust 2024 edition, `serde` + `serde_json`, `reqwest`, `url` crate for hostname extraction. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-05-12-anthropic-protocol-cycle4-parity-and-capabilities-design.md`](../specs/2026-05-12-anthropic-protocol-cycle4-parity-and-capabilities-design.md)

---

## File Structure

**New files:**
- `src/providers/protocols/anthropic/provider_policy.rs` — the new module (~250 lines incl. tests)

**Modified files:**
- `src/providers/protocols/anthropic.rs` — register the new submodule; extend `mod tests` with integration tests
- `src/providers/protocols/anthropic/adapter.rs` — wire fields in `build_request`; simplify `effective_cache_retention`; gate `cache_control` injection on capability bit
- `src/providers/anthropic/types.rs` — add 4 fields to `MessagesRequest`; add `Metadata` struct
- `src/config/types/provider.rs` — add `effort` and `metadata_user_id`; update `test_config()` and 4 other struct-literal sites
- `CHANGELOG.md` — Cycle 4 retrospective entry (T10, last task)

Each task produces a self-contained commit that compiles and passes its own tests. Tasks are ordered so dependencies resolve top-down: T1-T3 build the policy module bottom-up; T4 extends the wire struct; T5 extends config; T6-T8 wire `build_request`; T9 refactors caching; T10 docs.

---

## Task 1: `AnthropicEndpointClass` + `detect_anthropic_endpoint_class`

**Files:**
- Create: `src/providers/protocols/anthropic/provider_policy.rs`
- Modify: `src/providers/protocols/anthropic.rs:65-67` (register new module)

- [ ] **Step 1: Register the new submodule**

Add to `src/providers/protocols/anthropic.rs` after line 67 (the existing `mod sse;` line):

```rust
mod proto_impl;
mod adapter;
mod sse;
pub mod provider_policy;  // NEW
```

- [ ] **Step 2: Create the new file with failing tests**

Create `src/providers/protocols/anthropic/provider_policy.rs` with:

```rust
//! Anthropic-protocol provider policy.
//!
//! Mirrors `openai_common::provider_policy` but for the Anthropic Messages
//! API. Endpoint classification → capability bits → JSON body mutation.

// =============================================================================
// AnthropicEndpointClass
// =============================================================================

/// Detected Anthropic-protocol endpoint class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnthropicEndpointClass {
    /// Official Anthropic API: host == api.anthropic.com
    Official,
    /// Everything else: kimi-for-coding, MiniMax-anthropic-mode, Bedrock,
    /// Vertex, OpenRouter-anthropic, etc.
    Custom,
}

// =============================================================================
// Endpoint Detection
// =============================================================================

/// Detect Anthropic-protocol endpoint class from base URL.
///
/// `None` / `Some("")` → `Official` (matches `build_endpoint` default).
/// Unparseable URLs → `Custom` (conservative).
pub fn detect_anthropic_endpoint_class(base_url: Option<&str>) -> AnthropicEndpointClass {
    let url = match base_url {
        None | Some("") => return AnthropicEndpointClass::Official,
        Some(u) => u,
    };
    let host = extract_hostname(url).unwrap_or_default().to_lowercase();
    if host == "api.anthropic.com" {
        AnthropicEndpointClass::Official
    } else {
        AnthropicEndpointClass::Custom
    }
}

fn extract_hostname(url: &str) -> Option<String> {
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    };
    with_scheme
        .parse::<url::Url>()
        .ok()
        .map(|u| u.host_str().unwrap_or("").to_string())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_official_when_base_url_is_anthropic_host() {
        assert_eq!(
            detect_anthropic_endpoint_class(Some("https://api.anthropic.com/v1/messages")),
            AnthropicEndpointClass::Official
        );
    }

    #[test]
    fn detect_official_when_base_url_is_none() {
        assert_eq!(
            detect_anthropic_endpoint_class(None),
            AnthropicEndpointClass::Official
        );
    }

    #[test]
    fn detect_official_when_base_url_is_empty_string() {
        assert_eq!(
            detect_anthropic_endpoint_class(Some("")),
            AnthropicEndpointClass::Official
        );
    }

    #[test]
    fn detect_custom_for_third_party_hosts() {
        assert_eq!(
            detect_anthropic_endpoint_class(Some("https://kimi-for-coding.example.com/v1/messages")),
            AnthropicEndpointClass::Custom
        );
        assert_eq!(
            detect_anthropic_endpoint_class(Some("https://api.moonshot.cn/anthropic")),
            AnthropicEndpointClass::Custom
        );
    }

    #[test]
    fn detect_custom_for_unparseable_url() {
        // Garbage that even after `https://` prefix won't parse as a URL
        assert_eq!(
            detect_anthropic_endpoint_class(Some("not a url at all !!!")),
            AnthropicEndpointClass::Custom
        );
    }
}
```

- [ ] **Step 3: Run the new tests to confirm GREEN**

Run: `cargo test -p alephcore --lib providers::protocols::anthropic::provider_policy::tests::detect -- --nocapture`

Expected: 5 tests pass.

Note: Step 2 wrote both the type/function AND its tests in one go. The "RED" phase for this small task is mental — there's no API surface to test in isolation. The acceptance is that the 5 tests above pass on first run.

- [ ] **Step 4: Run full lib build to confirm no regressions**

Run: `cargo check -p alephcore`

Expected: clean compile (warnings allowed; no errors).

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/anthropic.rs src/providers/protocols/anthropic/provider_policy.rs
git commit -m "anthropic: add AnthropicEndpointClass + detect_anthropic_endpoint_class

Cycle 4 T1. New sibling module to openai_common's provider_policy.
Two variants: Official (api.anthropic.com) and Custom (everything
else). Conservative URL-parse fallback returns Custom.

5 unit tests cover Official/None/empty/third-party/garbage cases."
```

---

## Task 2: `AnthropicCapabilities` + `resolve_anthropic_capabilities`

**Files:**
- Modify: `src/providers/protocols/anthropic/provider_policy.rs` (append new types + function + tests)

- [ ] **Step 1: Add the struct and resolver above the existing `Tests` section**

Insert after the `extract_hostname` function (before the `// === Tests ===` divider) in `src/providers/protocols/anthropic/provider_policy.rs`:

```rust
// =============================================================================
// AnthropicCapabilities
// =============================================================================

/// Per-class capability flags for Anthropic-protocol endpoints.
///
/// Bits resolve solely from endpoint class (single source of truth).
/// There is no `ProviderConfig` override for any bit.
#[derive(Debug, Clone, Default)]
pub struct AnthropicCapabilities {
    /// `cache_control` breakpoints in `system` / `messages`.
    /// Gates `effective_cache_retention` injection in `build_request`.
    pub supports_cache_control: bool,
    /// `service_tier` field ("auto" / "standard_only").
    pub supports_service_tier: bool,
    /// `metadata.user_id` field.
    pub supports_metadata_user_id: bool,
    /// `output_config.effort` field ("low"/"medium"/"high"/"max").
    pub supports_output_config_effort: bool,
    /// `top_k` sampling field.
    pub supports_top_k: bool,
    /// `top_p` sampling field.
    pub supports_top_p: bool,
    /// `stop_sequences` field.
    pub supports_stop_sequences: bool,
}

// =============================================================================
// Capability Resolution
// =============================================================================

/// Resolve capabilities for a given Anthropic endpoint class.
pub fn resolve_anthropic_capabilities(class: AnthropicEndpointClass) -> AnthropicCapabilities {
    match class {
        AnthropicEndpointClass::Official => AnthropicCapabilities {
            supports_cache_control: true,
            supports_service_tier: true,
            supports_metadata_user_id: true,
            supports_output_config_effort: true,
            supports_top_k: true,
            supports_top_p: true,
            supports_stop_sequences: true,
        },
        AnthropicEndpointClass::Custom => AnthropicCapabilities {
            // Conservative: only the universally-portable Anthropic-protocol bits.
            // Premium/Anthropic-only features default OFF — third-party gateways
            // (kimi-for-coding, bedrock, vertex, etc.) reject them with 400 errors.
            supports_cache_control: false,
            supports_service_tier: false,
            supports_metadata_user_id: false,
            supports_output_config_effort: false,
            supports_top_k: true,
            supports_top_p: true,
            supports_stop_sequences: true,
        },
    }
}
```

- [ ] **Step 2: Append two new tests in the existing `tests` module**

Insert these tests inside the existing `#[cfg(test)] mod tests { ... }` block, after the `detect_custom_for_unparseable_url` test:

```rust
    #[test]
    fn official_capabilities_have_all_bits_true() {
        let caps = resolve_anthropic_capabilities(AnthropicEndpointClass::Official);
        assert!(caps.supports_cache_control);
        assert!(caps.supports_service_tier);
        assert!(caps.supports_metadata_user_id);
        assert!(caps.supports_output_config_effort);
        assert!(caps.supports_top_k);
        assert!(caps.supports_top_p);
        assert!(caps.supports_stop_sequences);
    }

    #[test]
    fn custom_capabilities_keep_protocol_standard_bits_only() {
        let caps = resolve_anthropic_capabilities(AnthropicEndpointClass::Custom);
        // Anthropic-only bits OFF
        assert!(!caps.supports_cache_control);
        assert!(!caps.supports_service_tier);
        assert!(!caps.supports_metadata_user_id);
        assert!(!caps.supports_output_config_effort);
        // Protocol-standard bits ON
        assert!(caps.supports_top_k);
        assert!(caps.supports_top_p);
        assert!(caps.supports_stop_sequences);
    }
```

- [ ] **Step 3: Run the new tests**

Run: `cargo test -p alephcore --lib providers::protocols::anthropic::provider_policy::tests::official_capabilities providers::protocols::anthropic::provider_policy::tests::custom_capabilities`

Expected: 2 tests pass.

- [ ] **Step 4: Run module tests fully**

Run: `cargo test -p alephcore --lib providers::protocols::anthropic::provider_policy::`

Expected: 7 tests pass (5 from T1 + 2 new).

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/anthropic/provider_policy.rs
git commit -m "anthropic: add AnthropicCapabilities + 7-bit profile per class

Cycle 4 T2. Capability matrix: Official has all 7 bits on; Custom
keeps the 3 protocol-standard bits (top_k/top_p/stop_sequences) and
drops the 4 Anthropic-only bits (cache_control/service_tier/
metadata_user_id/output_config_effort).

Single source of truth: bits flow only from endpoint class. No
ProviderConfig override (per feedback_no_user_capability_override.md)."
```

---

## Task 3: `AnthropicPolicy` + `apply()` strip logic + `build_anthropic_policy`

**Files:**
- Modify: `src/providers/protocols/anthropic/provider_policy.rs`

- [ ] **Step 1: Add the policy struct and helpers**

Insert after the `resolve_anthropic_capabilities` function (still before the test module):

```rust
// =============================================================================
// AnthropicPolicy
// =============================================================================

/// Resolved Anthropic-protocol policy for a given config.
#[derive(Debug, Clone)]
pub struct AnthropicPolicy {
    pub class: AnthropicEndpointClass,
    pub capabilities: AnthropicCapabilities,
}

impl AnthropicPolicy {
    /// Strip fields from the serialized request body whose capability bit is
    /// `false`. Called once at the end of `build_request`, after
    /// `serde_json::to_value(&request_body)`.
    ///
    /// `cache_control` is NOT stripped here — it's gated at injection time in
    /// `build_request` (see `policy.capabilities.supports_cache_control` check).
    pub fn apply(&self, body: &mut serde_json::Value) {
        let Some(obj) = body.as_object_mut() else { return };
        let caps = &self.capabilities;
        if !caps.supports_service_tier {
            obj.remove("service_tier");
        }
        if !caps.supports_metadata_user_id {
            strip_metadata_user_id(obj);
        }
        if !caps.supports_output_config_effort {
            strip_output_config_effort(obj);
        }
        if !caps.supports_top_k {
            obj.remove("top_k");
        }
        if !caps.supports_top_p {
            obj.remove("top_p");
        }
        if !caps.supports_stop_sequences {
            obj.remove("stop_sequences");
        }
    }
}

/// Remove `metadata.user_id`. If `metadata` becomes empty afterward, remove it too.
fn strip_metadata_user_id(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(metadata) = obj.get_mut("metadata") else { return };
    let Some(map) = metadata.as_object_mut() else { return };
    map.remove("user_id");
    if map.is_empty() {
        obj.remove("metadata");
    }
}

/// Remove `output_config.effort`. If `output_config` becomes empty afterward, remove it too.
fn strip_output_config_effort(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(output_config) = obj.get_mut("output_config") else { return };
    let Some(map) = output_config.as_object_mut() else { return };
    map.remove("effort");
    if map.is_empty() {
        obj.remove("output_config");
    }
}

// =============================================================================
// Policy Builder
// =============================================================================

/// Build a complete Anthropic policy from configuration.
pub fn build_anthropic_policy(base_url: Option<&str>) -> AnthropicPolicy {
    let class = detect_anthropic_endpoint_class(base_url);
    let capabilities = resolve_anthropic_capabilities(class);
    AnthropicPolicy { class, capabilities }
}
```

- [ ] **Step 2: Append the apply() matrix tests**

Insert into the existing `tests` module after the cap-resolution tests:

```rust
    fn body_with_all_anthropic_fields() -> serde_json::Value {
        serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [],
            "max_tokens": 1024,
            "temperature": 0.7,
            "top_p": 0.9,
            "top_k": 40,
            "stop_sequences": ["END", "STOP"],
            "service_tier": "auto",
            "metadata": {"user_id": "u_42"},
            "output_config": {"effort": "high"}
        })
    }

    #[test]
    fn apply_on_custom_strips_service_tier() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert!(body.get("service_tier").is_none());
    }

    #[test]
    fn apply_on_custom_strips_metadata_object_when_user_id_only_field() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert!(body.get("metadata").is_none(),
            "metadata object should be removed when only field (user_id) is stripped");
    }

    #[test]
    fn apply_on_custom_strips_output_config_when_effort_only_field() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn apply_on_custom_keeps_top_k_top_p_stop_sequences() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["stop_sequences"], serde_json::json!(["END", "STOP"]));
    }

    #[test]
    fn apply_on_official_keeps_all_fields() {
        let policy = build_anthropic_policy(Some("https://api.anthropic.com/v1/messages"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert_eq!(body["service_tier"], "auto");
        assert_eq!(body["metadata"]["user_id"], "u_42");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn apply_on_custom_never_touches_unrelated_fields() {
        let policy = build_anthropic_policy(Some("https://custom.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert_eq!(body["model"], "claude-3-5-sonnet");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["temperature"], 0.7);
        assert!(body.get("messages").is_some());
    }

    #[test]
    fn apply_metadata_with_other_keys_keeps_metadata_object() {
        let policy = build_anthropic_policy(Some("https://custom.example.com"));
        let mut body = serde_json::json!({
            "metadata": {"user_id": "u_42", "future_field": "preserved"}
        });
        policy.apply(&mut body);
        // user_id removed, but future_field keeps the object alive
        assert!(body.get("metadata").is_some());
        assert!(body["metadata"].get("user_id").is_none());
        assert_eq!(body["metadata"]["future_field"], "preserved");
    }

    #[test]
    fn apply_on_non_object_body_is_no_op() {
        let policy = build_anthropic_policy(None);
        let mut body = serde_json::json!("not an object");
        policy.apply(&mut body);
        assert_eq!(body, serde_json::json!("not an object"));
    }
```

- [ ] **Step 3: Run the matrix tests**

Run: `cargo test -p alephcore --lib providers::protocols::anthropic::provider_policy::tests::apply`

Expected: 8 tests pass.

- [ ] **Step 4: Run full provider_policy module tests**

Run: `cargo test -p alephcore --lib providers::protocols::anthropic::provider_policy::`

Expected: 15 tests pass (5 detect + 2 caps + 8 apply).

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/anthropic/provider_policy.rs
git commit -m "anthropic: add AnthropicPolicy::apply + build_anthropic_policy

Cycle 4 T3. Single mutation point for capability-gated field stripping.
- service_tier removed when capability off
- metadata.user_id removed + parent object removed if becomes empty
- output_config.effort removed + parent object removed if becomes empty
- top_k/top_p/stop_sequences stripped only when capability off (Custom keeps them)
- cache_control NOT stripped here (gated at injection time)

8 matrix tests cover Custom strips, Official keeps, no-op on
non-object body, and preservation of unrelated fields."
```

---

## Task 4: `MessagesRequest` adds `top_p` / `top_k` / `stop_sequences` / `metadata`

**Files:**
- Modify: `src/providers/anthropic/types.rs:14-34` (extend struct + add Metadata)

- [ ] **Step 1: Write the wire-shape test FIRST (RED phase)**

Add to `src/providers/anthropic/types.rs` — append a `#[cfg(test)] mod tests` block at the end of the file (the file currently has no test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_request_serializes_with_all_cycle4_fields() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet".to_string(),
            messages: vec![],
            max_tokens: 1024,
            system: None,
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            stop_sequences: Some(vec!["END".to_string(), "STOP".to_string()]),
            stream: Some(true),
            thinking: None,
            tools: None,
            service_tier: Some("auto".to_string()),
            metadata: Some(Metadata { user_id: Some("u_42".to_string()) }),
            output_config: Some(OutputConfig { effort: Some("high".to_string()) }),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-3-5-sonnet");
        assert_eq!(json["max_tokens"], 1024);
        assert_eq!(json["temperature"], 0.7);
        assert_eq!(json["top_p"], 0.9);
        assert_eq!(json["top_k"], 40);
        assert_eq!(json["stop_sequences"], serde_json::json!(["END", "STOP"]));
        assert_eq!(json["stream"], true);
        assert_eq!(json["service_tier"], "auto");
        assert_eq!(json["metadata"]["user_id"], "u_42");
        assert_eq!(json["output_config"]["effort"], "high");
    }

    #[test]
    fn messages_request_omits_none_cycle4_fields_on_wire() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet".to_string(),
            messages: vec![],
            max_tokens: 1024,
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            thinking: None,
            tools: None,
            service_tier: None,
            metadata: None,
            output_config: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("top_p").is_none());
        assert!(json.get("top_k").is_none());
        assert!(json.get("stop_sequences").is_none());
        assert!(json.get("metadata").is_none());
        assert!(json.get("service_tier").is_none());
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn metadata_omits_user_id_when_none() {
        let m = Metadata { user_id: None };
        let json = serde_json::to_value(&m).unwrap();
        // Object exists but has no fields
        assert_eq!(json, serde_json::json!({}));
    }
}
```

- [ ] **Step 2: Run the test to confirm RED**

Run: `cargo test -p alephcore --lib providers::anthropic::types::tests::messages_request 2>&1 | tail -20`

Expected: compile error — `MessagesRequest` does not have fields `top_p`, `top_k`, `stop_sequences`, `metadata`; struct `Metadata` does not exist.

- [ ] **Step 3: Extend `MessagesRequest` and add `Metadata`**

Replace `src/providers/anthropic/types.rs:14-34` (the `pub struct MessagesRequest { ... }` block) with:

```rust
/// Request body for Claude Messages API
#[derive(Debug, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// `top_p` nucleus sampling. Capability-gated (`supports_top_p`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// `top_k` sampling. Capability-gated (`supports_top_k`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Anthropic `stop_sequences` (up to 4 sequences). Capability-gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
    /// Service tier for priority or batch processing (e.g. "auto", "flex")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Anthropic abuse-detection / rate-limit metadata.
    /// Capability-gated (`supports_metadata_user_id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// Output configuration (effort level, structured output format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

/// Anthropic abuse-detection / rate-limit metadata.
/// Spec: https://docs.anthropic.com/en/api/messages#body-metadata
#[derive(Debug, Serialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}
```

- [ ] **Step 4: Run the tests to confirm GREEN**

Run: `cargo test -p alephcore --lib providers::anthropic::types::tests::`

Expected: 3 tests pass.

Then run `cargo check -p alephcore` and verify it compiles. If `build_request` in `adapter.rs:229-240` is now a compile error because the struct literal is missing the 4 new fields, that's expected — Task 6/7/8 fixes that. **Provisional fix to keep T4 commit isolated:** add the four missing fields to the literal as `None`:

In `src/providers/protocols/anthropic/adapter.rs`, edit the `MessagesRequest { ... }` literal starting at line 229. The current literal is:

```rust
let request_body = MessagesRequest {
    model: actual_model.to_string(),
    messages,
    max_tokens,
    system,
    temperature,
    stream: Some(true), // always streaming (stream-first architecture)
    thinking,
    tools,
    service_tier: None,
    output_config: None,
};
```

Replace with:

```rust
let request_body = MessagesRequest {
    model: actual_model.to_string(),
    messages,
    max_tokens,
    system,
    temperature,
    top_p: None,             // wired in T6
    top_k: None,             // wired in T6
    stop_sequences: None,    // wired in T6
    stream: Some(true), // always streaming (stream-first architecture)
    thinking,
    tools,
    service_tier: None,      // un-hardcoded in T7
    metadata: None,          // wired in T8
    output_config: None,     // wired in T8
};
```

- [ ] **Step 5: Verify full build and commit**

Run: `cargo check -p alephcore`

Expected: clean compile.

Run: `cargo test -p alephcore --lib providers::anthropic::types::`

Expected: tests pass.

```bash
git add src/providers/anthropic/types.rs src/providers/protocols/anthropic/adapter.rs
git commit -m "anthropic: add top_p/top_k/stop_sequences/metadata to MessagesRequest

Cycle 4 T4. Extend the wire struct with 4 Option<...> fields, all
skip_if_none. New Metadata struct (Anthropic abuse-detection /
rate-limit bucketing). Field order matches Anthropic API reference.

build_request literal updated with all-None placeholders; real wiring
follows in T6/T7/T8. Backward compatible: absent fields = old wire shape.

3 serialization tests cover full-populated round-trip, all-None
omission, and Metadata's user_id-only shape."
```

---

## Task 5: `ProviderConfig` adds `metadata_user_id` + `effort`

**Files:**
- Modify: `src/config/types/provider.rs:74-214` (extend `ProviderConfig`)
- Modify: `src/config/types/provider.rs:261` (extend `test_config()`)
- Modify: `src/config/types/provider.rs:314-343` (struct-literal site #1)
- Modify: `src/config/types/provider.rs:349-378` (struct-literal site #2)

- [ ] **Step 1: Write failing tests in the existing tests module**

Append these tests to the `#[cfg(test)] mod tests` block in `src/config/types/provider.rs` (after the existing `cycle3_fields_default_when_toml_omits_them` test at line ~415):

```rust
    #[test]
    fn cycle4_anthropic_fields_default_to_none() {
        let config = ProviderConfig::test_config("claude-3-5-sonnet");
        assert!(config.metadata_user_id.is_none());
        assert!(config.effort.is_none());
    }

    #[test]
    fn cycle4_anthropic_fields_deserialize_from_toml() {
        let toml_str = r#"
            protocol = "anthropic"
            models = ["claude-3-5-sonnet"]
            metadata_user_id = "u_42"
            effort = "high"
        "#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
        assert_eq!(cfg.metadata_user_id, Some("u_42".to_string()));
        assert_eq!(cfg.effort, Some("high".to_string()));
    }

    #[test]
    fn cycle4_anthropic_fields_omit_in_toml_yields_none() {
        let toml_str = r#"
            protocol = "anthropic"
            models = ["claude-3-5-sonnet"]
        "#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
        assert!(cfg.metadata_user_id.is_none());
        assert!(cfg.effort.is_none());
    }
```

- [ ] **Step 2: Run tests to confirm RED**

Run: `cargo test -p alephcore --lib config::types::provider::tests::cycle4 2>&1 | tail -20`

Expected: compile error — `metadata_user_id` and `effort` not found on `ProviderConfig`.

- [ ] **Step 3: Add the two new fields to `ProviderConfig`**

In `src/config/types/provider.rs`, find the last field of `ProviderConfig` (`pub top_logprobs: Option<u8>` at line ~213) and add these two fields immediately after, before the closing `}` of the struct:

```rust
    // Anthropic-specific parameters (Cycle 4)
    /// Anthropic `metadata.user_id`. Opaque string passed to Anthropic's
    /// abuse-detection / rate-limit-bucketing system.
    /// Capability-gated; silently dropped on non-Official Anthropic endpoints.
    #[serde(default)]
    pub metadata_user_id: Option<String>,

    /// Anthropic `output_config.effort`. Maps to `output_config.effort` on
    /// MessagesRequest. Accepted values: "low", "medium", "high", "max".
    /// Capability-gated; silently dropped on non-Official Anthropic endpoints.
    #[serde(default)]
    pub effort: Option<String>,
```

- [ ] **Step 4: Update `test_config()` and the two test struct-literals**

In `src/config/types/provider.rs`, find the `test_config` function around line 261. After the existing `top_logprobs: None,` line, add:

```rust
            metadata_user_id: None,
            effort: None,
```

Then find the two test fixtures at lines ~314-343 (`test_protocol_without_provider_type`) and ~349-378 (`test_protocol_defaults_to_openai`). Each has a `ProviderConfig { ... }` literal. Add the same two lines (`metadata_user_id: None, effort: None,`) before the closing `};` of each literal.

- [ ] **Step 5: Run all provider tests and commit**

Run: `cargo test -p alephcore --lib config::types::provider::`

Expected: All existing tests + 3 new Cycle 4 tests pass.

Run: `cargo check -p alephcore`

Expected: clean compile (any other call site constructing `ProviderConfig` with struct-literal syntax may fail — search for them with `grep -rn "ProviderConfig {" src --include='*.rs' | grep -v 'test'` and add `metadata_user_id: None, effort: None,` to each).

```bash
git add src/config/types/provider.rs
git commit -m "config: add metadata_user_id + effort to ProviderConfig (Cycle 4)

Two new Option<String> fields, both with #[serde(default)]. Old
config.toml deserializes unchanged (missing fields become None).
Wired into MessagesRequest in T7/T8.

test_config() + two test fixture literals updated. 3 new tests
cover defaults, TOML deser, and TOML-omits-them paths."
```

---

## Task 6: Wire `top_p` / `top_k` / `stop_sequences` in `build_request`

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (top of `build_request`, line ~163; the `MessagesRequest` literal at line 229)
- Modify: `src/providers/protocols/anthropic.rs` (mod tests, append integration tests)

- [ ] **Step 1: Write failing integration tests**

Append to the existing `#[cfg(test)] mod tests { ... }` block in `src/providers/protocols/anthropic.rs` (the module starting at line 71, just before its closing `}`). Add these helpers + tests:

```rust
    fn body_of(request: reqwest::RequestBuilder) -> serde_json::Value {
        let built = request.build().unwrap();
        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        serde_json::from_slice(body_bytes).unwrap()
    }

    #[test]
    fn build_request_wires_top_p_and_top_k_from_config() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.top_p = Some(0.9);
        config.top_k = Some(40);

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["top_k"], 40);
    }

    #[test]
    fn build_request_wires_stop_sequences_csv_from_config() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.stop_sequences = Some("END, STOP, DONE".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["stop_sequences"], serde_json::json!(["END", "STOP", "DONE"]));
    }

    #[test]
    fn build_request_drops_empty_stop_sequences() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.stop_sequences = Some("".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("stop_sequences").is_none(), "empty CSV should produce no field");
    }

    #[test]
    fn build_request_drops_whitespace_only_stop_sequences() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.stop_sequences = Some(" , ,  ".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("stop_sequences").is_none());
    }
```

- [ ] **Step 2: Run tests to confirm RED**

Run: `cargo test -p alephcore --lib build_request_wires_top_p build_request_wires_stop_sequences build_request_drops_empty_stop_sequences build_request_drops_whitespace 2>&1 | tail -25`

Expected: tests compile but FAIL — `top_p` / `top_k` / `stop_sequences` not present in body (still `None` from T4 placeholders).

- [ ] **Step 3: Add the CSV parser and wire the three fields**

In `src/providers/protocols/anthropic/adapter.rs`, add this helper near the top of the file (after the existing top-level `use` statements and before `fn effective_cache_retention`):

```rust
/// Parse a comma-separated stop-sequences string into a Vec<String>.
/// Splits on `,`, trims each element, and filters out empties.
fn parse_stop_sequences(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}
```

Then in the same file, locate the `MessagesRequest` literal construction (line ~229). Just before the `let request_body = MessagesRequest { ... }` line, add field extractions:

```rust
        // Cycle 4: wire sampling fields from config
        let top_p = config.top_p;
        let top_k = config.top_k;
        let stop_sequences = config
            .stop_sequences
            .as_deref()
            .map(parse_stop_sequences)
            .filter(|v| !v.is_empty());
```

Then in the `MessagesRequest { ... }` literal, replace the three placeholder lines added in T4:

```rust
            top_p: None,             // wired in T6
            top_k: None,             // wired in T6
            stop_sequences: None,    // wired in T6
```

with:

```rust
            top_p,
            top_k,
            stop_sequences,
```

- [ ] **Step 4: Run tests to confirm GREEN**

Run: `cargo test -p alephcore --lib build_request_wires_top_p build_request_wires_stop_sequences build_request_drops_empty_stop_sequences build_request_drops_whitespace`

Expected: 4 tests pass.

Run also the existing Anthropic test suite to confirm no regressions: `cargo test -p alephcore --lib providers::protocols::anthropic`

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/anthropic/adapter.rs src/providers/protocols/anthropic.rs
git commit -m "anthropic: wire top_p/top_k/stop_sequences from ProviderConfig

Cycle 4 T6. Adapter now consumes the three sampling fields from
ProviderConfig and writes them into MessagesRequest. New
parse_stop_sequences helper splits the CSV format, trims, and filters
empties (matches the OpenAI side's existing convention).

4 build_request integration tests cover top_p/top_k wiring, CSV
parsing, empty input, and whitespace-only input."
```

---

## Task 7: Wire `service_tier` from `ProviderConfig` (un-hardcode `None`)

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (the literal at line ~229)
- Modify: `src/providers/protocols/anthropic.rs` (mod tests)

- [ ] **Step 1: Write failing integration tests**

Append to the `mod tests` block in `src/providers/protocols/anthropic.rs`:

```rust
    #[test]
    fn build_request_wires_service_tier_on_official() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.service_tier = Some("auto".to_string());
        // base_url left None → resolves to Official

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["service_tier"], "auto");
    }

    #[test]
    fn build_request_strips_service_tier_on_custom_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.service_tier = Some("auto".to_string());
        config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(
            body.get("service_tier").is_none(),
            "service_tier must be stripped on Custom endpoint"
        );
    }
```

- [ ] **Step 2: Run tests to confirm RED**

Run: `cargo test -p alephcore --lib build_request_wires_service_tier build_request_strips_service_tier 2>&1 | tail -20`

Expected: tests fail — `service_tier` is hardcoded `None` in the request body.

- [ ] **Step 3: Wire service_tier and add policy.apply() call**

In `src/providers/protocols/anthropic/adapter.rs`:

(a) After the existing `self.stream_idle_timeout_secs.store(...)` block at the top of `build_request` (line ~155), add the policy resolution:

```rust
        // Cycle 4: resolve capability policy once at the top of build_request.
        let policy = crate::providers::protocols::anthropic::provider_policy::build_anthropic_policy(
            config.base_url.as_deref(),
        );
```

(b) Replace this placeholder line in the `MessagesRequest` literal:

```rust
            service_tier: None,      // un-hardcoded in T7
```

with:

```rust
            service_tier: config.service_tier.clone(),
```

(c) At the very end of `build_request`, just before the final `Ok(self.client.post(...)...)` builder chain, add the single capability-gate call. Locate the spot where `body` is fully constructed (after the `tool_choice` handling and the cache-control injection block — currently around line 292). Insert:

```rust
        // Cycle 4: strip capability-gated fields one last time.
        policy.apply(&mut body);
```

Place this **after** the cache_control injection block (lines 280-292) so all body mutation completes first.

- [ ] **Step 4: Run tests to confirm GREEN**

Run: `cargo test -p alephcore --lib build_request_wires_service_tier build_request_strips_service_tier`

Expected: 2 tests pass.

Run the full Anthropic suite: `cargo test -p alephcore --lib providers::protocols::anthropic`

Expected: all green (no regressions in existing tests).

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/anthropic/adapter.rs src/providers/protocols/anthropic.rs
git commit -m "anthropic: wire service_tier + add policy.apply gate to build_request

Cycle 4 T7. ProviderConfig.service_tier now flows into MessagesRequest
instead of being hardcoded None. AnthropicPolicy resolved at the top
of build_request, applied as the last body mutation step.

2 tests lock the behavior: Official wires service_tier=\"auto\";
Custom (kimi-for-coding.example.com) strips it via policy.apply."
```

---

## Task 8: Wire `metadata_user_id` + `effort`

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (the literal at line ~229)
- Modify: `src/providers/protocols/anthropic.rs` (mod tests)

- [ ] **Step 1: Write failing integration tests**

Append to the `mod tests` block in `src/providers/protocols/anthropic.rs`:

```rust
    #[test]
    fn build_request_wires_metadata_user_id_on_official() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.metadata_user_id = Some("u_cycle4".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["metadata"]["user_id"], "u_cycle4");
    }

    #[test]
    fn build_request_strips_metadata_on_custom_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.metadata_user_id = Some("u_cycle4".to_string());
        config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("metadata").is_none(), "metadata must be stripped on Custom");
    }

    #[test]
    fn build_request_wires_effort_on_official() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.effort = Some("high".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["output_config"]["effort"], "high");
    }

    #[test]
    fn build_request_strips_output_config_on_custom_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.effort = Some("high".to_string());
        config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("output_config").is_none(), "output_config must be stripped on Custom");
    }
```

- [ ] **Step 2: Run tests to confirm RED**

Run: `cargo test -p alephcore --lib build_request_wires_metadata build_request_strips_metadata build_request_wires_effort build_request_strips_output_config 2>&1 | tail -25`

Expected: 4 tests fail — fields still hardcoded `None` in the literal.

- [ ] **Step 3: Wire the two fields**

In `src/providers/protocols/anthropic/adapter.rs`:

(a) Add the `use` import at the top of the file if not already there:

```rust
use crate::providers::anthropic::types::{Metadata, OutputConfig};
```

(Note: `OutputConfig` may already be imported via the existing `use crate::providers::anthropic::types::*;` style; check first and only add what's missing.)

(b) Before the `let request_body = MessagesRequest {` literal at line ~229, after the `top_p`/`top_k`/`stop_sequences` extraction block from T6, add:

```rust
        // Cycle 4: wire metadata + effort from config
        let metadata = config
            .metadata_user_id
            .as_ref()
            .map(|uid| Metadata { user_id: Some(uid.clone()) });
        let output_config = config
            .effort
            .as_ref()
            .map(|e| OutputConfig { effort: Some(e.clone()) });
```

(c) In the `MessagesRequest { ... }` literal, replace these two placeholder lines:

```rust
            metadata: None,          // wired in T8
            output_config: None,     // wired in T8
```

with:

```rust
            metadata,
            output_config,
```

- [ ] **Step 4: Run tests to confirm GREEN**

Run: `cargo test -p alephcore --lib build_request_wires_metadata build_request_strips_metadata build_request_wires_effort build_request_strips_output_config`

Expected: 4 tests pass.

Run the full Anthropic suite to confirm: `cargo test -p alephcore --lib providers::protocols::anthropic`

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/anthropic/adapter.rs src/providers/protocols/anthropic.rs
git commit -m "anthropic: wire metadata.user_id + output_config.effort

Cycle 4 T8. ProviderConfig.metadata_user_id flows into
MessagesRequest.metadata.user_id; ProviderConfig.effort flows into
MessagesRequest.output_config.effort. policy.apply (from T7) strips
both nested objects when capability bit is off.

4 tests cover: Official wires both, Custom strips both."
```

---

## Task 9: Refactor `effective_cache_retention` + gate cache_control on capability

**Files:**
- Modify: `src/providers/protocols/anthropic/adapter.rs` (`effective_cache_retention` at line 32; the `if retention != CacheRetention::Off { ... }` block at line ~282)
- Modify: existing tests in `adapter.rs:515-547`
- Modify: `src/providers/protocols/anthropic.rs` (add one new end-to-end test)

- [ ] **Step 1: Update existing `effective_cache_retention` tests to new semantics**

In `src/providers/protocols/anthropic/adapter.rs`, locate the four existing tests around lines 515-547:

(a) `effective_retention_official_unset_defaults_short` (line ~515) — **keep as-is**, semantics unchanged.

(b) `effective_retention_third_party_unset_defaults_off` (line ~524) — **rename and update expectation**. Replace:

```rust
    #[test]
    fn effective_retention_third_party_unset_defaults_off() {
        let config = ProviderConfig::test_config("claude-3-5-sonnet");
        // cache_retention is None by default in test_config
        let retention =
            effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Off);
    }
```

with:

```rust
    #[test]
    fn effective_retention_unset_always_defaults_short_after_cycle4() {
        // Cycle 4: host gate moved to policy.capabilities.supports_cache_control.
        // effective_cache_retention only resolves None → Short.
        let config = ProviderConfig::test_config("claude-3-5-sonnet");
        let retention =
            effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Short);
    }
```

(c) `effective_retention_explicit_long_on_third_party_respected` (line ~532) — **keep as-is**, semantics unchanged (explicit value still respected at this layer; host gate happens downstream).

(d) `effective_retention_explicit_off_always_off` (line ~541) — **keep as-is**.

- [ ] **Step 2: Simplify `effective_cache_retention`**

Replace the `fn effective_cache_retention` body (starting at line 32) with:

```rust
fn effective_cache_retention(config: &ProviderConfig, endpoint: &str) -> CacheRetention {
    match config.cache_retention {
        Some(CacheRetention::Long) if !endpoint.contains("api.anthropic.com") => {
            // Keep the existing warning that surfaces long-TTL misuse on
            // third-party hosts. Physical injection is blocked downstream
            // by policy.capabilities.supports_cache_control, but the user
            // signal that they explicitly asked for Long is still useful.
            tracing::warn!(
                endpoint = %endpoint,
                "cache_retention = long on non-official Anthropic host; \
                 cache_control will not be injected because the endpoint \
                 capability is disabled."
            );
            CacheRetention::Long
        }
        Some(r) => r,
        None => CacheRetention::Short,
    }
}
```

(Confirm the `tracing` use is already present at the top of `adapter.rs`; the existing file uses `warn!` so the import is there.)

- [ ] **Step 3: Gate the cache_control injection on capability**

Locate the cache injection block in `build_request` (around line 279-292):

```rust
        // Inject prompt-cache breakpoints if retention is not Off.
        let retention = effective_cache_retention(config, &endpoint);
        let extended_cache_ttl = matches!(retention, CacheRetention::Long);
        if retention != CacheRetention::Off {
            let cc = CacheControl::Ephemeral {
                ttl: if extended_cache_ttl {
                    Some(EphemeralTtl::OneHour)
                } else {
                    None
                },
            };
            inject_cache_control_into_system_array(&mut body, cc);
            inject_cache_control_into_last_user_message(&mut body, cc);
        }
```

Replace with:

```rust
        // Inject prompt-cache breakpoints only when the endpoint supports it
        // (cf. policy.capabilities.supports_cache_control). Cycle 4 moved
        // the host-level gate here from effective_cache_retention.
        let extended_cache_ttl = if policy.capabilities.supports_cache_control {
            let retention = effective_cache_retention(config, &endpoint);
            let ext = matches!(retention, CacheRetention::Long);
            if retention != CacheRetention::Off {
                let cc = CacheControl::Ephemeral {
                    ttl: if ext {
                        Some(EphemeralTtl::OneHour)
                    } else {
                        None
                    },
                };
                inject_cache_control_into_system_array(&mut body, cc);
                inject_cache_control_into_last_user_message(&mut body, cc);
            }
            ext
        } else {
            false
        };
```

The `extended_cache_ttl` variable is consumed downstream in `Self::build_beta_headers(actual_model, Some(api_key), extended_cache_ttl)`. With the gated form, Custom hosts always pass `false`, so the long-TTL beta header is never emitted there.

- [ ] **Step 4: Add the end-to-end Official-vs-Custom comparison test**

Append to the `mod tests` block in `src/providers/protocols/anthropic.rs`:

```rust
    #[test]
    fn build_request_injects_cache_control_only_on_official_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("Be helpful."));

        // Official path: cache_control present on system block
        let mut official = ProviderConfig::test_config("claude-3-5-sonnet");
        official.api_key = Some("test-key".to_string());
        let official_body = body_of(protocol.build_request(&payload, &official).unwrap());
        assert!(
            official_body["system"][0]["cache_control"].is_object(),
            "Official endpoint should inject cache_control on system block"
        );

        // Custom path: cache_control absent on system block
        let mut custom = ProviderConfig::test_config("claude-3-5-sonnet");
        custom.api_key = Some("test-key".to_string());
        custom.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());
        let custom_body = body_of(protocol.build_request(&payload, &custom).unwrap());
        // system serializes to array of blocks; cache_control must be absent
        let custom_system_block = &custom_body["system"][0];
        assert!(
            custom_system_block.get("cache_control").is_none(),
            "Custom endpoint must NOT inject cache_control on system block, got: {:?}",
            custom_system_block
        );
    }
```

- [ ] **Step 5: Run tests + commit**

Run the four updated retention tests + the new end-to-end test:

```bash
cargo test -p alephcore --lib effective_retention build_request_injects_cache_control_only_on_official
```

Expected: 5 tests pass.

Then full Anthropic suite: `cargo test -p alephcore --lib providers::protocols::anthropic`

Expected: all green.

Then a full lib regression: `cargo test -p alephcore --lib`

Expected: same baseline failures as pre-Cycle 4 (test_apply_policy_strips_fields is the known pre-existing failure documented in spec §13.6; no new failures should appear).

```bash
git add src/providers/protocols/anthropic/adapter.rs src/providers/protocols/anthropic.rs
git commit -m "anthropic: gate cache_control injection on capability bit

Cycle 4 T9. effective_cache_retention simplified: None defaults to
Short regardless of host. Host-level gate moves to
policy.capabilities.supports_cache_control wrapping the whole
injection + extended_cache_ttl block.

Existing warning for cache_retention=long on third-party hosts is
preserved (signals user intent even though injection is blocked).

Four existing retention tests updated; one new end-to-end test
locks Official-vs-Custom wire-level behavior."
```

---

## Task 10: CHANGELOG.md retrospective entry

**Files:**
- Modify: `CHANGELOG.md` (Unreleased section)

- [ ] **Step 1: Confirm Unreleased section structure**

Run: `head -30 CHANGELOG.md`

Expected: see an `## [Unreleased]` heading followed by `### Added` / `### Fixed` sub-sections from prior cycles.

- [ ] **Step 2: Append Cycle 4 entries to the Unreleased section**

Edit `CHANGELOG.md` Unreleased section. Add this block, keeping it consistent with the existing Cycle 2/3 entry style (English, bullet-point, scoped by feature):

```markdown
## Cycle 4 — Anthropic Protocol Parity & Capability Matrix (2026-05-12)

### Added

- New sibling module `src/providers/protocols/anthropic/provider_policy.rs` exposing `AnthropicEndpointClass` (Official + Custom), `AnthropicCapabilities` (7-bit profile per class), `AnthropicPolicy::apply` (single mutation gate over the JSON body), and `build_anthropic_policy` (one-shot builder).
- `ProviderConfig.metadata_user_id: Option<String>` — wired into `MessagesRequest.metadata.user_id` on Official, silently stripped on Custom. Anthropic abuse-detection / rate-limit bucketing field.
- `ProviderConfig.effort: Option<String>` — wired into `MessagesRequest.output_config.effort` on Official, silently stripped on Custom. Accepted values: `"low"`, `"medium"`, `"high"`, `"max"`.
- `MessagesRequest.top_p` / `top_k` / `stop_sequences` / `metadata` fields — added to the wire struct with `skip_if_none`; backward compatible.
- New `Metadata` struct in `providers::anthropic::types` with a single optional `user_id` field; future-proof shape for additional metadata keys.

### Changed

- `MessagesRequest.service_tier` no longer hardcoded `None` in `build_request`. Now wired from `ProviderConfig.service_tier` and capability-gated.
- `MessagesRequest.top_p`, `top_k`, `stop_sequences` previously ignored by the Anthropic adapter — now wired from existing `ProviderConfig` fields (capability-gated via `supports_top_p` / `top_k` / `stop_sequences`; Custom keeps these on, but the gate is in place for future variants).
- `effective_cache_retention` simplified: only resolves `None → Short`. The host-level gate is now `policy.capabilities.supports_cache_control` wrapping the whole `cache_control` injection block in `build_request`. Wire-level behavior is identical to pre-Cycle 4 (Custom hosts never receive `cache_control`).

### Discipline (no change but worth noting)

- Capability bits remain a one-way flow: `base_url → AnthropicEndpointClass → AnthropicCapabilities`. No `ProviderConfig.supports_*` override fields. Future deployment-specific feature tuning goes through a new `AnthropicEndpointClass` variant (e.g., Bedrock, Vertex), not a config flag — per the `feedback_no_user_capability_override.md` redline.
```

- [ ] **Step 3: Verify markdown lints cleanly**

If the repo has any CHANGELOG validator (e.g., `just changelog-check` or `cargo run -p changelog-validator`), run it. Otherwise visually inspect that the new block follows the same heading depth and bullet structure as prior cycles.

- [ ] **Step 4: Run final regression sweep**

Run: `cargo test -p alephcore --lib`

Expected: only the documented pre-existing `test_apply_policy_strips_fields` failure (OpenAI-protocol bug, not Anthropic).

Run: `cargo clippy -p alephcore -- -D warnings`

Expected: zero new clippy warnings introduced by Cycle 4 code (pre-existing warnings in unrelated files allowed).

- [ ] **Step 5: Commit + close**

```bash
git add CHANGELOG.md
git commit -m "changelog: document Cycle 4 — Anthropic protocol parity & cap matrix

Final commit of Cycle 4. Documents new provider_policy module
(AnthropicEndpointClass + AnthropicCapabilities + AnthropicPolicy),
the 4 newly-wired fields (top_p, top_k, stop_sequences, service_tier),
the 2 new config fields (metadata_user_id, effort), and the
effective_cache_retention refactor (host gate moved to policy).

Manual real-machine regressions (Official + kimi-for-coding) tracked
separately per spec §13."
```

---

## Post-Implementation Manual Regressions (Spec §13)

After T10 commits, run the two manual real-machine checks before declaring Cycle 4 shipped:

1. **Official Anthropic endpoint** — configure a provider with `base_url = "https://api.anthropic.com/v1"`, `service_tier = "auto"`, `metadata_user_id = "cycle4-test"`, `effort = "medium"`. Send one chat request through the gateway. Confirm: Anthropic accepts, response streams normally, no 400.

2. **kimi-for-coding Custom endpoint** — configure a provider with kimi-for-coding `base_url` and the same three fields populated. Send one chat request. Confirm: request accepted (because those three fields were stripped at the wire); response streams normally.

Record outcomes in the commit message of an optional T11 follow-up commit (if any tweaks needed) or in a memory file `project_cycle4_anthropic_shipped.md`.

---

## Self-Review

**Spec coverage** — Mapped each spec section to tasks:

- §1 Scope (4 bundles A/B/C/D) → C4-A (T6+T7), C4-B (T1+T2+T3+T9), C4-C (T5+T8), C4-D (T5+T8)
- §2 Module layout → T1 creates `provider_policy.rs`, registered in T1 step 1
- §3 New types (EndpointClass, Capabilities, Policy, builder) → T1, T2, T3
- §4 ProviderConfig fields → T5
- §5 MessagesRequest evolution → T4
- §6 build_request wiring + parse_stop_sequences + effective_cache_retention simplification → T6, T7, T8, T9
- §7 Testing strategy (5 detect + 2 caps + 8 apply unit tests; ~10 integration tests; wire-shape test; existing test updates; 1 end-to-end) → covered T1-T9
- §8 TDD task decomposition → tasks T1-T10
- §9 Error handling → no panics; URL parse fallback to Custom (T1); body-not-object early return (T3); CSV parser edge cases (T6)
- §10 Backward compat → all new fields `Option<T>`/`#[serde(default)]`; struct-literal updates in T4 (placeholder), T5 (config), T6/T7/T8 (real wiring); existing `effective_cache_retention` callers untouched (signature stable)
- §11 Risk mitigation → 8 matrix tests + end-to-end comparison test + manual regressions
- §12 Rollback → each task = independent commit
- §13 Acceptance criteria → all met through T1-T10 + post-implementation manual regression

**Placeholder scan** — No "TBD", no "implement later", no "similar to Task N" (each task is self-contained), no abstract "add appropriate validation". Every step has either exact code, exact filename + line ref, or exact command + expected output.

**Type consistency** — Spot-checked:
- `AnthropicEndpointClass::Official` / `Custom` (T1) ↔ `match class { Official => ..., Custom => ... }` (T2) — consistent
- `AnthropicCapabilities { supports_cache_control, supports_service_tier, ... }` (T2) ↔ `policy.capabilities.supports_cache_control` (T9) — consistent
- `build_anthropic_policy(base_url: Option<&str>)` (T3) ↔ `build_anthropic_policy(config.base_url.as_deref())` (T7) — consistent
- `Metadata { user_id: Option<String> }` (T4) ↔ `Metadata { user_id: Some(uid.clone()) }` (T8) — consistent
- `OutputConfig { effort: Option<String> }` (exists pre-Cycle 4, T4 references it) ↔ `OutputConfig { effort: Some(e.clone()) }` (T8) — consistent
- `parse_stop_sequences(csv: &str) -> Vec<String>` (T6) ↔ `config.stop_sequences.as_deref().map(parse_stop_sequences)` (T6) — consistent

Plan ready for execution.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-12-anthropic-protocol-cycle4-parity-and-capabilities.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Matches the Cycle 1-3 execution pattern memory.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
