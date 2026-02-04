# Thinking Levels System Design

> 参考实现: Moltbot `/src/auto-reply/thinking.ts` + `/src/agents/pi-embedded-helpers/thinking.ts`

## Overview

为 Aleph 实现完整的 Thinking Levels 系统，支持 6 级思考深度控制、多 Provider 适配、以及智能 Fallback 机制。

## Phase A: ThinkLevel 核心类型

### A.1 ThinkLevel 枚举

**文件**: `core/src/agents/thinking.rs` (新建)

```rust
use serde::{Deserialize, Serialize};

/// Thinking level for LLM reasoning depth control
///
/// Inspired by Moltbot's ThinkLevel system, provides 6 levels
/// of reasoning depth from no thinking to extended deep reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkLevel {
    /// No extended thinking, fastest response
    Off,
    /// Minimal thinking, brief internal reasoning
    #[default]
    Minimal,
    /// Low thinking depth
    Low,
    /// Medium thinking depth (balanced)
    Medium,
    /// High thinking depth, detailed reasoning
    High,
    /// Extended high thinking (xhigh), deepest reasoning
    /// Only supported by specific models (e.g., GPT-5.2, Claude Opus)
    XHigh,
}

impl ThinkLevel {
    /// All available thinking levels in order
    pub const ALL: &'static [ThinkLevel] = &[
        ThinkLevel::Off,
        ThinkLevel::Minimal,
        ThinkLevel::Low,
        ThinkLevel::Medium,
        ThinkLevel::High,
        ThinkLevel::XHigh,
    ];

    /// Get the next lower thinking level for fallback
    pub fn fallback(&self) -> Option<ThinkLevel> {
        match self {
            ThinkLevel::XHigh => Some(ThinkLevel::High),
            ThinkLevel::High => Some(ThinkLevel::Medium),
            ThinkLevel::Medium => Some(ThinkLevel::Low),
            ThinkLevel::Low => Some(ThinkLevel::Minimal),
            ThinkLevel::Minimal => Some(ThinkLevel::Off),
            ThinkLevel::Off => None,
        }
    }

    /// Get numeric weight for comparison (higher = more thinking)
    pub fn weight(&self) -> u8 {
        match self {
            ThinkLevel::Off => 0,
            ThinkLevel::Minimal => 1,
            ThinkLevel::Low => 2,
            ThinkLevel::Medium => 3,
            ThinkLevel::High => 4,
            ThinkLevel::XHigh => 5,
        }
    }

    /// Check if this level is higher than another
    pub fn is_higher_than(&self, other: &ThinkLevel) -> bool {
        self.weight() > other.weight()
    }

    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            ThinkLevel::Off => "Off",
            ThinkLevel::Minimal => "Minimal",
            ThinkLevel::Low => "Low",
            ThinkLevel::Medium => "Medium",
            ThinkLevel::High => "High",
            ThinkLevel::XHigh => "Extended",
        }
    }

    /// Get description for UI
    pub fn description(&self) -> &'static str {
        match self {
            ThinkLevel::Off => "No extended thinking, fastest responses",
            ThinkLevel::Minimal => "Brief internal reasoning",
            ThinkLevel::Low => "Basic thinking process",
            ThinkLevel::Medium => "Balanced thinking depth",
            ThinkLevel::High => "Detailed reasoning and analysis",
            ThinkLevel::XHigh => "Deep extended thinking (model-specific)",
        }
    }
}

impl std::fmt::Display for ThinkLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name().to_lowercase())
    }
}

impl std::str::FromStr for ThinkLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        normalize_think_level(s)
            .ok_or_else(|| format!("Unknown thinking level: '{}'", s))
    }
}
```

### A.2 用户输入归一化

```rust
/// Normalize user-provided thinking level strings to canonical enum
///
/// Supports various aliases:
/// - "think", "on", "enable" → Minimal
/// - "thinkhard", "think-hard" → Low
/// - "thinkharder", "harder" → Medium
/// - "ultrathink", "ultra", "max" → High
/// - "xhigh", "x-high" → XHigh
///
/// # Examples
/// ```
/// assert_eq!(normalize_think_level("think"), Some(ThinkLevel::Minimal));
/// assert_eq!(normalize_think_level("ultrathink"), Some(ThinkLevel::High));
/// assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
/// ```
pub fn normalize_think_level(raw: &str) -> Option<ThinkLevel> {
    let key = raw.trim().to_lowercase();

    match key.as_str() {
        // Off
        "off" | "none" | "disable" | "disabled" | "0" | "false" => Some(ThinkLevel::Off),

        // Minimal
        "minimal" | "min" | "think" | "on" | "enable" | "enabled" | "1" | "true" => {
            Some(ThinkLevel::Minimal)
        }

        // Low
        "low" | "thinkhard" | "think-hard" | "think_hard" | "2" => Some(ThinkLevel::Low),

        // Medium
        "medium" | "med" | "mid" | "thinkharder" | "think-harder" | "harder" | "3" => {
            Some(ThinkLevel::Medium)
        }

        // High
        "high" | "ultra" | "ultrathink" | "thinkhardest" | "highest" | "max" | "4" => {
            Some(ThinkLevel::High)
        }

        // XHigh
        "xhigh" | "x-high" | "x_high" | "extended" | "5" => Some(ThinkLevel::XHigh),

        _ => None,
    }
}

/// List available thinking level labels for a provider/model combination
pub fn list_thinking_level_labels(provider: &str, model: &str) -> Vec<&'static str> {
    if is_binary_thinking_provider(provider) {
        vec!["off", "on"]
    } else {
        let mut levels = vec!["off", "minimal", "low", "medium", "high"];
        if supports_xhigh_thinking(provider, model) {
            levels.push("xhigh");
        }
        levels
    }
}

/// Format thinking levels as comma-separated string
pub fn format_thinking_levels(provider: &str, model: &str) -> String {
    list_thinking_level_labels(provider, model).join(", ")
}
```

### A.3 模型能力矩阵

```rust
use std::collections::HashSet;
use once_cell::sync::Lazy;

/// Models that support xhigh (extended) thinking
static XHIGH_MODEL_REFS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // OpenAI models with extended thinking
        "openai/gpt-5.2",
        "openai/o1",
        "openai/o1-preview",
        "openai/o1-mini",
        // Anthropic models with extended thinking
        "claude/claude-opus-4-5-20251101",
        "claude/claude-3-opus-20240229",
    ]
    .into_iter()
    .collect()
});

/// Model IDs (without provider prefix) that support xhigh
static XHIGH_MODEL_IDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "gpt-5.2",
        "o1",
        "o1-preview",
        "o1-mini",
        "claude-opus-4-5-20251101",
        "claude-3-opus-20240229",
    ]
    .into_iter()
    .collect()
});

/// Providers that only support binary thinking (on/off)
static BINARY_THINKING_PROVIDERS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    ["z.ai", "zai", "z-ai"].into_iter().collect()
});

/// Check if provider only supports binary thinking (on/off)
pub fn is_binary_thinking_provider(provider: &str) -> bool {
    let normalized = provider.trim().to_lowercase();
    BINARY_THINKING_PROVIDERS.contains(normalized.as_str())
}

/// Check if model supports xhigh (extended) thinking
pub fn supports_xhigh_thinking(provider: &str, model: &str) -> bool {
    let model_key = model.trim().to_lowercase();
    let provider_key = provider.trim().to_lowercase();

    // Check full reference (provider/model)
    let full_ref = format!("{}/{}", provider_key, model_key);
    if XHIGH_MODEL_REFS.contains(full_ref.as_str()) {
        return true;
    }

    // Check model ID only
    XHIGH_MODEL_IDS.contains(model_key.as_str())
}

/// Get supported thinking levels for a provider/model combination
pub fn get_supported_levels(provider: &str, model: &str) -> Vec<ThinkLevel> {
    if is_binary_thinking_provider(provider) {
        vec![ThinkLevel::Off, ThinkLevel::Minimal]
    } else {
        let mut levels = vec![
            ThinkLevel::Off,
            ThinkLevel::Minimal,
            ThinkLevel::Low,
            ThinkLevel::Medium,
            ThinkLevel::High,
        ];
        if supports_xhigh_thinking(provider, model) {
            levels.push(ThinkLevel::XHigh);
        }
        levels
    }
}

/// Check if a thinking level is supported by provider/model
pub fn is_level_supported(level: ThinkLevel, provider: &str, model: &str) -> bool {
    get_supported_levels(provider, model).contains(&level)
}
```

---

## Phase B: Provider 适配

### B.1 ThinkingConfig 结构

**文件**: `core/src/agents/thinking.rs` (续)

```rust
/// Configuration for thinking level in LLM requests
#[derive(Debug, Clone)]
pub struct ThinkingConfig {
    /// Requested thinking level
    pub level: ThinkLevel,
    /// Provider name (for capability checking)
    pub provider: String,
    /// Model name (for capability checking)
    pub model: String,
}

impl ThinkingConfig {
    pub fn new(level: ThinkLevel, provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            level,
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Get the effective level (capped by model capability)
    pub fn effective_level(&self) -> ThinkLevel {
        let supported = get_supported_levels(&self.provider, &self.model);
        if supported.contains(&self.level) {
            self.level
        } else {
            // Find highest supported level <= requested
            supported
                .into_iter()
                .filter(|l| l.weight() <= self.level.weight())
                .max_by_key(|l| l.weight())
                .unwrap_or(ThinkLevel::Off)
        }
    }
}
```

### B.2 Provider API 参数映射

**文件**: `core/src/agents/thinking_adapter.rs` (新建)

```rust
use super::thinking::{ThinkLevel, ThinkingConfig};
use serde_json::{json, Value};

/// Adapter for converting ThinkLevel to provider-specific API parameters
pub struct ThinkingAdapter;

impl ThinkingAdapter {
    /// Convert thinking config to Anthropic API parameters
    ///
    /// Anthropic uses:
    /// - `thinking` block with `type: "enabled"` and `budget_tokens`
    /// - or prefill-based thinking prompts
    pub fn to_anthropic_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 1024
                }
            })),
            ThinkLevel::Low => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 2048
                }
            })),
            ThinkLevel::Medium => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 4096
                }
            })),
            ThinkLevel::High => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 8192
                }
            })),
            ThinkLevel::XHigh => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 16384
                }
            })),
        }
    }

    /// Convert thinking config to OpenAI API parameters
    ///
    /// OpenAI uses:
    /// - `reasoning_effort`: "low" | "medium" | "high" (for o1 models)
    /// - or model-specific parameters
    pub fn to_openai_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        // Check if model supports reasoning_effort (o1 family)
        let model_lower = config.model.to_lowercase();
        let supports_reasoning_effort = model_lower.contains("o1")
            || model_lower.contains("gpt-5");

        if !supports_reasoning_effort {
            return None;
        }

        match level {
            ThinkLevel::Off | ThinkLevel::Minimal => None,
            ThinkLevel::Low => Some(json!({
                "reasoning_effort": "low"
            })),
            ThinkLevel::Medium => Some(json!({
                "reasoning_effort": "medium"
            })),
            ThinkLevel::High | ThinkLevel::XHigh => Some(json!({
                "reasoning_effort": "high"
            })),
        }
    }

    /// Convert thinking config to Gemini API parameters
    ///
    /// Gemini uses:
    /// - `thinking_config.thinking_budget`: token budget
    /// - or legacy `thinking_level`: "LOW" | "HIGH"
    pub fn to_gemini_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        // Check if model supports new thinking_config (Gemini 2.5+)
        let model_lower = config.model.to_lowercase();
        let supports_budget = model_lower.contains("2.5")
            || model_lower.contains("3.0")
            || model_lower.contains("3-");

        if supports_budget {
            match level {
                ThinkLevel::Off => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 0
                    }
                })),
                ThinkLevel::Minimal => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 1024
                    }
                })),
                ThinkLevel::Low => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 2048
                    }
                })),
                ThinkLevel::Medium => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 4096
                    }
                })),
                ThinkLevel::High => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 8192
                    }
                })),
                ThinkLevel::XHigh => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 16384
                    }
                })),
            }
        } else {
            // Legacy Gemini 3 models use LOW/HIGH
            match level {
                ThinkLevel::Off | ThinkLevel::Minimal | ThinkLevel::Low => {
                    Some(json!({ "thinking_level": "LOW" }))
                }
                _ => Some(json!({ "thinking_level": "HIGH" })),
            }
        }
    }

    /// Convert thinking config to DeepSeek API parameters
    ///
    /// DeepSeek uses `enable_thinking` boolean
    pub fn to_deepseek_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        if level == ThinkLevel::Off {
            Some(json!({ "enable_thinking": false }))
        } else {
            Some(json!({ "enable_thinking": true }))
        }
    }

    /// Get provider-specific parameters based on provider type
    pub fn to_provider_params(config: &ThinkingConfig) -> Option<Value> {
        let provider_lower = config.provider.to_lowercase();

        match provider_lower.as_str() {
            "claude" | "anthropic" => Self::to_anthropic_params(config),
            "openai" => Self::to_openai_params(config),
            "gemini" | "google" => Self::to_gemini_params(config),
            "deepseek" => Self::to_deepseek_params(config),
            _ => None, // Unknown provider, no thinking params
        }
    }
}
```

### B.3 Provider 集成

**修改**: `core/src/providers/claude.rs`

```rust
// 在 ClaudeProvider 中添加 thinking_config 支持

impl ClaudeProvider {
    pub fn with_thinking_level(mut self, level: ThinkLevel) -> Self {
        self.thinking_level = Some(level);
        self
    }

    fn build_request_body(&self, messages: &[Message], system: Option<&str>) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": messages,
        });

        if let Some(sys) = system {
            body["system"] = json!(sys);
        }

        // Add thinking config if set
        if let Some(level) = self.thinking_level {
            let config = ThinkingConfig::new(level, "claude", &self.model);
            if let Some(thinking_params) = ThinkingAdapter::to_anthropic_params(&config) {
                if let Some(obj) = body.as_object_mut() {
                    for (k, v) in thinking_params.as_object().unwrap() {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        body
    }
}
```

---

## Phase C: Fallback 机制

### C.1 错误消息解析

**文件**: `core/src/agents/thinking_fallback.rs` (新建)

```rust
use super::thinking::{normalize_think_level, ThinkLevel};
use std::collections::HashSet;
use regex::Regex;
use once_cell::sync::Lazy;

/// Extract supported thinking levels from error message
///
/// Parses error messages like:
/// - "supported values are: 'off', 'minimal', 'low'"
/// - "Supported values: off, low, medium"
fn extract_supported_values(message: &str) -> Vec<String> {
    static PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"supported values(?: are)?:\s*([^\n.]+)"#).unwrap()
    });

    if let Some(captures) = PATTERN.captures(message) {
        if let Some(fragment) = captures.get(1) {
            let text = fragment.as_str();

            // Try to extract quoted values first
            static QUOTED: Lazy<Regex> = Lazy::new(|| {
                Regex::new(r#"['"]([^'"]+)['"]"#).unwrap()
            });

            let quoted: Vec<String> = QUOTED
                .captures_iter(text)
                .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
                .collect();

            if !quoted.is_empty() {
                return quoted;
            }

            // Fall back to comma/and separated values
            return text
                .split(|c| c == ',' || c == ' ')
                .map(|s| s.trim().trim_matches(|c| !char::is_alphabetic(c)))
                .filter(|s| !s.is_empty() && *s != "and")
                .map(|s| s.to_string())
                .collect();
        }
    }

    Vec::new()
}

/// Pick a fallback thinking level based on error message
///
/// Parses the error message to find supported levels, then returns
/// the first supported level that hasn't been attempted yet.
///
/// # Arguments
/// * `message` - Error message from the provider
/// * `attempted` - Set of already-attempted thinking levels
///
/// # Returns
/// * `Some(ThinkLevel)` - A fallback level to try
/// * `None` - No fallback available (all levels exhausted)
pub fn pick_fallback_thinking_level(
    message: Option<&str>,
    attempted: &HashSet<ThinkLevel>,
) -> Option<ThinkLevel> {
    let message = message?.trim();
    if message.is_empty() {
        return None;
    }

    let supported = extract_supported_values(message);
    if supported.is_empty() {
        return None;
    }

    for entry in supported {
        if let Some(level) = normalize_think_level(&entry) {
            if !attempted.contains(&level) {
                return Some(level);
            }
        }
    }

    None
}

/// Detect if error is related to unsupported thinking level
pub fn is_thinking_level_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("thinking") && (
        lower.contains("unsupported") ||
        lower.contains("not supported") ||
        lower.contains("invalid") ||
        lower.contains("supported values")
    )
}

/// ThinkingFallbackState tracks retry attempts for thinking level fallback
#[derive(Debug, Default)]
pub struct ThinkingFallbackState {
    /// Set of already-attempted thinking levels
    pub attempted: HashSet<ThinkLevel>,
    /// Current thinking level
    pub current: ThinkLevel,
    /// Number of fallback attempts
    pub attempts: u32,
}

impl ThinkingFallbackState {
    pub fn new(initial: ThinkLevel) -> Self {
        let mut attempted = HashSet::new();
        attempted.insert(initial);
        Self {
            attempted,
            current: initial,
            attempts: 0,
        }
    }

    /// Try to fallback to a lower thinking level
    ///
    /// Returns the new level if fallback is possible, None otherwise.
    pub fn try_fallback(&mut self, error_message: Option<&str>) -> Option<ThinkLevel> {
        // First try to parse supported levels from error message
        if let Some(level) = pick_fallback_thinking_level(error_message, &self.attempted) {
            self.attempted.insert(level);
            self.current = level;
            self.attempts += 1;
            return Some(level);
        }

        // Fall back to next lower level
        if let Some(lower) = self.current.fallback() {
            if !self.attempted.contains(&lower) {
                self.attempted.insert(lower);
                self.current = lower;
                self.attempts += 1;
                return Some(lower);
            }
        }

        None
    }

    /// Check if we've exhausted all fallback options
    pub fn is_exhausted(&self) -> bool {
        self.current == ThinkLevel::Off || self.attempts >= 5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_supported_values_quoted() {
        let msg = "Error: unsupported thinking level. Supported values are: 'off', 'minimal', 'low'";
        let values = extract_supported_values(msg);
        assert_eq!(values, vec!["off", "minimal", "low"]);
    }

    #[test]
    fn test_extract_supported_values_unquoted() {
        let msg = "Invalid thinking level. Supported values: off, low, medium, high";
        let values = extract_supported_values(msg);
        assert!(values.contains(&"off".to_string()));
        assert!(values.contains(&"low".to_string()));
    }

    #[test]
    fn test_pick_fallback() {
        let msg = "Supported values are: 'off', 'minimal', 'low'";
        let mut attempted = HashSet::new();
        attempted.insert(ThinkLevel::High);

        let fallback = pick_fallback_thinking_level(Some(msg), &attempted);
        assert!(fallback.is_some());
        assert!(fallback.unwrap().weight() < ThinkLevel::High.weight());
    }

    #[test]
    fn test_fallback_state() {
        let mut state = ThinkingFallbackState::new(ThinkLevel::High);

        // First fallback
        let level1 = state.try_fallback(Some("Supported values: off, low"));
        assert_eq!(level1, Some(ThinkLevel::Low));

        // Second fallback (low already attempted)
        let level2 = state.try_fallback(Some("Supported values: off, low"));
        assert_eq!(level2, Some(ThinkLevel::Off));
    }
}
```

### C.2 Agent Loop 集成

**修改**: `core/src/agent_loop/agent_loop.rs`

```rust
use crate::agents::thinking::{ThinkLevel, ThinkingFallbackState};

impl AgentLoop {
    /// Run the agent loop with thinking level fallback support
    pub async fn run_with_thinking(&mut self, think_level: ThinkLevel) -> Result<LoopResult> {
        let mut fallback_state = ThinkingFallbackState::new(think_level);

        loop {
            // Set current thinking level
            self.config.think_level = fallback_state.current;

            match self.run_iteration().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let error_msg = e.to_string();

                    // Check if error is thinking-level related
                    if is_thinking_level_error(&error_msg) {
                        if let Some(fallback) = fallback_state.try_fallback(Some(&error_msg)) {
                            tracing::warn!(
                                current = %fallback_state.current,
                                fallback = %fallback,
                                "Thinking level unsupported, falling back"
                            );
                            continue;
                        }
                    }

                    // Not a thinking error or fallback exhausted
                    return Err(e);
                }
            }
        }
    }
}
```

---

## Integration Points

### 1. LoopConfig 扩展

**修改**: `core/src/agent_loop/config.rs`

```rust
use crate::agents::thinking::ThinkLevel;

pub struct LoopConfig {
    // ... existing fields ...

    /// Default thinking level for LLM calls
    #[serde(default)]
    pub think_level: ThinkLevel,

    /// Whether to enable thinking level fallback on errors
    #[serde(default = "default_true")]
    pub enable_thinking_fallback: bool,
}
```

### 2. Gateway Protocol 扩展

**修改**: `core/src/gateway/protocol.rs`

```rust
/// Chat request with thinking level support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub session_id: Option<String>,
    /// Thinking level: "off", "minimal", "low", "medium", "high", "xhigh"
    #[serde(default)]
    pub thinking: Option<String>,
    // ... other fields
}
```

### 3. ThinkerConfig 扩展

**修改**: `core/src/thinker/mod.rs`

```rust
use crate::agents::thinking::ThinkLevel;

pub struct ThinkerConfig {
    // ... existing fields ...

    /// Thinking level for LLM reasoning
    pub think_level: ThinkLevel,
}
```

---

## File Structure

```
core/src/agents/
├── mod.rs              # Add: pub mod thinking;
├── thinking.rs         # NEW: ThinkLevel enum, normalize, capabilities
├── thinking_adapter.rs # NEW: Provider-specific parameter mapping
└── thinking_fallback.rs # NEW: Fallback mechanism
```

---

## Implementation Order

1. **Phase A.1**: Create `thinking.rs` with ThinkLevel enum
2. **Phase A.2**: Add normalize functions and model capability matrix
3. **Phase A.3**: Add tests for Phase A
4. **Phase B.1**: Create `thinking_adapter.rs` with provider mapping
5. **Phase B.2**: Integrate with ClaudeProvider
6. **Phase B.3**: Integrate with OpenAiProvider, GeminiProvider
7. **Phase C.1**: Create `thinking_fallback.rs`
8. **Phase C.2**: Integrate fallback into agent_loop
9. **Integration**: Update LoopConfig, Gateway protocol, ThinkerConfig
10. **Testing**: End-to-end tests with mock providers

---

## Testing Strategy

### Unit Tests
- ThinkLevel parsing and normalization
- Model capability detection
- Provider parameter mapping
- Fallback error parsing

### Integration Tests
- Agent loop with thinking level
- Gateway chat request with thinking parameter
- Fallback retry flow

### Manual Testing
- Test with real Anthropic API (Claude)
- Test with real OpenAI API (GPT-4, o1)
- Test with real Gemini API

---

## Migration Notes

1. **Backward Compatibility**: Default ThinkLevel is `Minimal`, matching current behavior
2. **Config Migration**: Existing configs without `think_level` will use default
3. **Provider Migration**: Existing `thinking_level` in ProviderConfig maps to new system
