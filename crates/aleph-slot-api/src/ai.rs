//! AI provider request surface: the borrowed [`RequestPayload`], tool-choice
//! vocabulary, [`SystemPromptPart`], the [`ThinkLevel`] ladder, and the
//! object-safe [`AiProviderContract`] trait.
//!
//! Everything here is pure data + pure functions — no I/O, no HTTP, no
//! streaming machinery. The single authoritative definition of each item
//! lives here; `alephcore` re-exports the same types (one `TypeId`) and keeps
//! the runtime side (`HttpProvider`, the protocol registry, the failover
//! chain, `ProtocolAdapter`) to itself.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::ai_delta::DeltaSink;
use crate::ai_message::UnifiedMessage;
use crate::ai_reasoning::ReasoningReplay;
use crate::ai_response::ProviderResponse;
use crate::error::Result;
use crate::metadata::ToolDefinition;

// =============================================================================
// ToolChoice
// =============================================================================

/// Tool selection control for protocol adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    /// LLM decides whether to use tools (default)
    Auto,
    /// LLM MUST call at least one tool
    Required,
    /// LLM must call this specific tool by name
    Specific(String),
    /// Disable all tool use for this request
    None,
}

// =============================================================================
// SystemPromptPart
// =============================================================================

/// System prompt part with optional cache flag
///
/// When using Anthropic's prompt caching, static content can be cached
/// for improved performance. This struct allows splitting the system
/// prompt into cacheable and non-cacheable parts.
#[derive(Debug, Clone)]
pub struct SystemPromptPart {
    /// The content of this part
    pub content: String,
    /// Whether this part should be cached (for Anthropic)
    pub cache: bool,
}

// =============================================================================
// RequestPayload
// =============================================================================

/// Unified request payload for protocol adapters.
///
/// Protocol adapters translate this into provider-specific request formats.
#[derive(Debug)]
pub struct RequestPayload<'a> {
    /// Structured message list
    pub messages: &'a [UnifiedMessage],
    /// System prompt (handled differently per provider).
    ///
    /// Single-string form: kept for back-compat with every callsite that
    /// existed before cache-first wiring. When `system_blocks` is also set,
    /// adapters MUST prefer `system_blocks` (Anthropic prompt-cache splits
    /// the system into stable + dynamic so the breakpoint marker sits on
    /// the stable tail instead of the whole assembly).
    pub system_prompt: Option<&'a str>,
    /// Stable/dynamic split of the system prompt for cache-first providers.
    ///
    /// `None` (the default) preserves legacy behaviour — adapters fall back
    /// to `system_prompt`. When set, the first contiguous run of parts with
    /// `cache: true` forms the cacheable prefix; the rest is the per-turn
    /// dynamic tail. Currently consumed by the Anthropic adapter; other
    /// adapters concatenate transparently.
    pub system_blocks: Option<&'a [SystemPromptPart]>,
    /// Tool definitions for native `tool_use`
    pub tools: Option<&'a [ToolDefinition]>,
    /// Thinking/reasoning level
    pub think_level: Option<ThinkLevel>,
    /// Per-request temperature override
    pub temperature: Option<f32>,
    /// Per-request `max_tokens` override
    pub max_tokens: Option<u32>,
    /// Tool selection control (auto/required/specific/none)
    pub tool_choice: Option<ToolChoice>,
    /// Per-request model override (takes precedence over provider config)
    pub model: Option<String>,
    /// Metadata for provider-specific features (e.g. `user_id` for Anthropic)
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

#[allow(clippy::derivable_impls)]
impl<'a> Default for RequestPayload<'a> {
    fn default() -> Self {
        Self {
            messages: &[],
            system_prompt: None,
            system_blocks: None,
            tools: None,
            think_level: None,
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            model: None,
            metadata: None,
        }
    }
}

impl<'a> RequestPayload<'a> {
    /// Create payload from messages
    #[must_use]
    pub fn new(messages: &'a [UnifiedMessage]) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// Add system prompt (single-string form, legacy).
    #[must_use]
    pub const fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Attach the stable/dynamic split. Cache-first providers prefer this.
    #[must_use]
    pub const fn with_system_blocks(mut self, blocks: Option<&'a [SystemPromptPart]>) -> Self {
        self.system_blocks = blocks;
        self
    }

    /// Add tools
    #[must_use]
    pub const fn with_tools(mut self, tools: Option<&'a [ToolDefinition]>) -> Self {
        self.tools = tools;
        self
    }

    /// Set thinking level
    #[must_use]
    pub const fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set temperature
    #[must_use]
    pub const fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set `max_tokens`
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set tool choice
    #[must_use]
    pub fn with_tool_choice(mut self, choice: Option<ToolChoice>) -> Self {
        self.tool_choice = choice;
        self
    }

    /// Set model override
    #[must_use]
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// Set metadata for provider-specific features
    #[must_use]
    pub fn with_metadata(
        mut self,
        metadata: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        self.metadata = metadata;
        self
    }
}

// =============================================================================
// ThinkLevel
// =============================================================================

/// Thinking level for LLM reasoning depth control.
///
/// Six levels from no thinking to extended deep reasoning. Not every model
/// accepts every level; the per-protocol adapters clamp to the nearest value
/// the target model's family actually supports.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkLevel {
    /// No extended thinking, fastest response.
    ///
    /// NOTE for adapter authors: on several families `Off` is NOT expressible by
    /// omitting the field — Gemini 2.5 Flash and OpenAI reasoning models think
    /// by default, so omission silently buys (and bills) their default depth.
    /// `Off` must be sent as an explicit disable value on those families.
    Off,
    /// Minimal thinking, brief internal reasoning (default)
    #[default]
    Minimal,
    /// Low thinking depth
    Low,
    /// Medium thinking depth (balanced)
    Medium,
    /// High thinking depth, detailed reasoning
    High,
    /// Extended high thinking (xhigh), deepest reasoning.
    /// Only supported by specific models (e.g. GPT-5.2, Claude Opus).
    XHigh,
}

impl ThinkLevel {
    /// Canonical wire/storage id — what gets persisted in session metadata and
    /// accepted back by [`normalize_think_level`]. Mirrors `ExecTier::id()`.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }

    /// Every level, ordered shallow → deep.
    ///
    /// The order is the ladder itself, and it is what a picker renders top to
    /// bottom. Pinned against a new variant by `all_covers_every_variant`,
    /// which is an exhaustive `match` — adding a rung fails to compile there
    /// rather than silently shipping a surface that cannot offer it.
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
    ];

    /// Human-facing label for UI surfaces.
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "Extended",
        }
    }
}

impl std::fmt::Display for ThinkLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

impl std::str::FromStr for ThinkLevel {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        normalize_think_level(s).ok_or_else(|| format!("Unknown thinking level: '{s}'"))
    }
}

/// Normalize user-provided thinking level strings to the canonical enum.
///
/// Aliases exist because people type what they mean, not what the enum is
/// called:
/// - "think", "on", "enable" → Minimal
/// - "thinkhard" → Low
/// - "thinkharder", "harder" → Medium
/// - "ultrathink", "ultra", "max" → High
/// - "xhigh", "extended" → `XHigh`
///
/// Returns `None` for anything unrecognized — callers must REJECT rather than
/// default. Silently falling back would run the turn at a different depth (and
/// a different price) than the one the user believes they picked.
///
/// # Examples
///
/// ```rust
/// use aleph_slot_api::ai::{normalize_think_level, ThinkLevel};
///
/// assert_eq!(normalize_think_level("think"), Some(ThinkLevel::Minimal));
/// assert_eq!(normalize_think_level("ultrathink"), Some(ThinkLevel::High));
/// assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
/// assert_eq!(normalize_think_level("unknown"), None);
/// ```
#[must_use]
pub fn normalize_think_level(raw: &str) -> Option<ThinkLevel> {
    let key = raw.trim().to_lowercase();

    match key.as_str() {
        // Off
        "off" | "none" | "disable" | "disabled" | "0" | "false" | "no" => Some(ThinkLevel::Off),

        // Minimal
        "minimal" | "min" | "think" | "on" | "enable" | "enabled" | "1" | "true" | "yes" => {
            Some(ThinkLevel::Minimal)
        }

        // Low
        "low" | "thinkhard" | "think-hard" | "think_hard" | "2" => Some(ThinkLevel::Low),

        // Medium
        "medium" | "med" | "mid" | "thinkharder" | "think-harder" | "think_harder" | "harder"
        | "3" => Some(ThinkLevel::Medium),

        // High
        "high" | "ultra" | "ultrathink" | "ultra-think" | "ultra_think" | "thinkhardest"
        | "think-hardest" | "highest" | "max" | "4" => Some(ThinkLevel::High),

        // XHigh
        "xhigh" | "x-high" | "x_high" | "extended" | "ext" | "5" | "extreme" => {
            Some(ThinkLevel::XHigh)
        }

        _ => None,
    }
}

// =============================================================================
// Cross-crate match dispatch (doc-hidden)
// =============================================================================
//
// [`ThinkLevel`] is `#[non_exhaustive]`: only this crate (the owner) may match
// it exhaustively. External consumers — including `alephcore`'s protocol
// adapters — dispatch through these helpers, which keep the exhaustive match
// here. A new variant then fails to compile *here*, next to the enum, instead
// of silently taking an unknown-variant branch in a consumer's wildcard. There
// is deliberately no unknown/default arm anywhere: no consumer may answer for
// a variant it has never seen.

/// Dispatch on a [`ThinkLevel`] with one callback per variant.
///
/// The callbacks take `&mut C` so the caller can accumulate into (or branch
/// on) its own state — e.g. a per-model capability predicate — without the
/// helper knowing anything about the consumer's business policy. All policy
/// stays in the callbacks; this function only routes.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn dispatch_think_level<C, R>(
    level: &ThinkLevel,
    context: &mut C,
    on_off: impl FnOnce(&mut C) -> R,
    on_minimal: impl FnOnce(&mut C) -> R,
    on_low: impl FnOnce(&mut C) -> R,
    on_medium: impl FnOnce(&mut C) -> R,
    on_high: impl FnOnce(&mut C) -> R,
    on_xhigh: impl FnOnce(&mut C) -> R,
) -> R {
    match level {
        ThinkLevel::Off => on_off(context),
        ThinkLevel::Minimal => on_minimal(context),
        ThinkLevel::Low => on_low(context),
        ThinkLevel::Medium => on_medium(context),
        ThinkLevel::High => on_high(context),
        ThinkLevel::XHigh => on_xhigh(context),
    }
}

/// `const`-compatible [`ThinkLevel`] → value map for `Copy` types.
///
/// For consumers whose per-level mapping is a plain table (no per-model
/// predicate), so a `pub(super) const fn map_think_level(...)` can stay
/// `const` across the crate boundary. Same exhaustive match as
/// [`dispatch_think_level`]; a new variant fails to compile here.
#[doc(hidden)]
#[must_use]
pub const fn select_think_level_copy<T: Copy>(
    level: &ThinkLevel,
    off: T,
    minimal: T,
    low: T,
    medium: T,
    high: T,
    xhigh: T,
) -> T {
    match level {
        ThinkLevel::Off => off,
        ThinkLevel::Minimal => minimal,
        ThinkLevel::Low => low,
        ThinkLevel::Medium => medium,
        ThinkLevel::High => high,
        ThinkLevel::XHigh => xhigh,
    }
}

// =============================================================================
// AiProviderContract
// =============================================================================

/// Unified interface for AI providers — the pure contract half.
///
/// All AI backends (`OpenAI`, Claude, Ollama, etc.) implement this trait
/// to provide a consistent API for processing requests.
///
/// This is the pure-contract twin of `alephcore::providers::AiProvider`:
/// identical method surface minus `as_http_provider` (which names a Core
/// runtime type and therefore cannot live here). The old Core trait is NOT
/// replaced by this one — Core's runtime stack (`HttpProvider`, failover,
/// decorators) keeps implementing the Core trait; this contract exists so
/// non-Core consumers (panel/WASM, ffi, SDK stubs, tests) can speak to a
/// provider-shaped object without reaching for `alephcore`.
///
/// # Architecture
///
/// Single `process()` method accepts a `RequestPayload` containing structured
/// `UnifiedMessage` history. Protocol adapters convert these to native formats.
pub trait AiProviderContract: Send + Sync {
    /// Core method — process a request and return structured response
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>;

    /// Provider name
    fn name(&self) -> &str;

    /// Provider brand color
    fn color(&self) -> &str;

    /// Whether this provider supports native `tool_use`
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Protocol name for model behavior resolution.
    ///
    /// Returns the protocol identifier (e.g., "openai", "anthropic", "gemini", "ollama")
    /// used to select appropriate model behavior directives.
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed("unknown")
    }

    /// Model behavior override from provider config.
    ///
    /// When set, this takes precedence over the protocol-based auto-mapping.
    /// Used for providers like `OpenRouter` that use one protocol but route to
    /// a different model family.
    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Self-identified governance behavior name derived from the provider's
    /// own endpoint/model (e.g. Kimi/Minimax → `"strict"`). Sits ABOVE the
    /// protocol fallback but BELOW the explicit config `model_behavior`
    /// override in `resolve_behavior`. Default `None` = "no opinion, use the
    /// protocol default". Wrappers delegate; `HttpProvider` computes it.
    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Best-effort id of the model this provider would serve next. Used to key
    /// per-model lookups (context-window resolution for the occupancy gauge,
    /// pricing) when no explicit model directive (session pick / agent hint /
    /// strict brain pin) exists. Wrappers delegate to their live primary, like
    /// [`AiProviderContract::behavior_hint`]; `HttpProvider` reports its
    /// configured default model. Default `None` = "unknown", callers fall back
    /// to the provider name.
    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// The provider id actually serving this call — the twin of
    /// [`AiProviderContract::serving_model_hint`], and for the same reason.
    ///
    /// [`AiProviderContract::name`] is NOT this: on the production path
    /// `name()` walks the decorator stack down to the failover provider and
    /// returns the literal `"failover"`, which is not a provider id any lookup
    /// table knows. Pricing keyed on it silently reports unknown cost for
    /// every run. Wrappers delegate to their live primary; `HttpProvider`
    /// reports its configured key. Default `None` = "unknown".
    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// The reasoning this provider's next call would carry — for estimators
    /// that must count what the wire sends (Core's `HttpProvider::execute`
    /// applies the same policy to the real call). `model` is a per-call
    /// override (the payload's `model`); `None` = the provider's own serving
    /// model.
    ///
    /// The default resolves from [`AiProviderContract::protocol`] and the
    /// model hint, with no host facts. `HttpProvider` resolves fully;
    /// wrappers delegate to their live primary, like
    /// [`AiProviderContract::serving_model_hint`].
    fn reasoning_replay(&self, model: Option<&str>) -> ReasoningReplay {
        let hint = self.serving_model_hint();
        let model = model.or(hint.as_deref()).unwrap_or("");
        ReasoningReplay::for_target(&self.protocol(), None, model)
    }

    /// Whether this provider's calls ask the server to clear old tool results
    /// (Anthropic server-side context editing), so the local passes that
    /// rewrite old tool results must stand down for runs on it — one problem,
    /// one answer. `HttpProvider` answers from its adapter; wrappers delegate
    /// to their live primary, like [`AiProviderContract::reasoning_replay`].
    /// Default `false`.
    fn clears_tool_results_server_side(&self) -> bool {
        false
    }

    /// Whether calling [`execute_streaming_dyn`](AiProviderContract::execute_streaming_dyn)
    /// produces *live* deltas rather than one replayed batch at the end.
    ///
    /// Must be asked of the OUTERMOST provider: a decorator stack is only as
    /// streaming-capable as its weakest link, because a decorator that forgets
    /// to override `execute_streaming_dyn` collapses the call to `process` and
    /// the deltas arrive all at once. Default `false`; a provider opts in only
    /// by genuinely forwarding the sink, and a decorator by delegating.
    ///
    /// This is the honest replacement for gating streaming on a downcast to
    /// the concrete HTTP provider, which asked "are you literally the HTTP
    /// provider" — a question every real production stack answers "no" (the
    /// failover chain sits in the middle and is not one).
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Streaming twin of [`AiProviderContract::process`]: forward incremental
    /// deltas to `sink` while producing the same structured
    /// [`ProviderResponse`].
    ///
    /// Core's `HttpProvider` overrides this to actually stream; the per-run
    /// decorators (`ThinkLevelProvider`, `MeteringProvider`,
    /// `ModelOverrideProvider`) and the failover provider override it to apply
    /// their side effect / walk their chain and delegate, exactly as they do
    /// for `process`. Before this existed, the harness reached the raw inner
    /// HTTP provider via a downcast for streaming, silently skipping every
    /// decorator (dropping the declared `think_level`, never emitting
    /// `ProviderUsage`).
    ///
    /// The default calls `process` and then **replays** the finished response
    /// through `sink`. Replaying (rather than dropping `sink`) is what makes the
    /// contract usable as a contract: *whoever calls this always sees the
    /// response on the sink*, so a caller that suppresses its own once-per-turn
    /// emit — which is exactly what the harness does — can never end up
    /// delivering nothing because some link in the stack did not override this
    /// method. Overriding it upgrades the delivery from batched to live; it is
    /// not the difference between delivery and silence. There is no double-emit
    /// risk: this path hands `process` no sink, so nothing below ever sees one.
    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: &'a dyn DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.process(payload).await?;
            crate::ai_delta::replay_response_to_sink(&response, sink).await;
            Ok(response)
        })
    }
}

/// The canonical short name for [`AiProviderContract`].
///
/// Distinct from `alephcore::providers::AiProvider` (the Core runtime trait,
/// which additionally exposes `as_http_provider`); the two are deliberately
/// separate traits on the bridge boundary — see the trait docs.
pub use AiProviderContract as AiProvider;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_builder() {
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs)
            .with_system(Some("You are helpful"))
            .with_think_level(Some(ThinkLevel::Medium));

        assert_eq!(payload.messages.len(), 1);
        assert_eq!(payload.system_prompt, Some("You are helpful"));
        assert!(payload.think_level.is_some());
    }

    #[test]
    fn test_payload_default() {
        let msgs = [UnifiedMessage::user("Test")];
        let payload = RequestPayload::new(&msgs);
        assert_eq!(payload.messages.len(), 1);
        assert!(payload.system_prompt.is_none());
        assert!(payload.think_level.is_none());
        assert!(payload.temperature.is_none());
        assert!(payload.max_tokens.is_none());
    }

    #[test]
    fn test_payload_with_generation_overrides() {
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs)
            .with_temperature(Some(0.7))
            .with_max_tokens(Some(4096));

        assert_eq!(payload.temperature, Some(0.7));
        assert_eq!(payload.max_tokens, Some(4096));
    }

    #[test]
    fn test_request_payload_with_tools() {
        let msgs = [UnifiedMessage::user("test input")];
        let payload = RequestPayload::new(&msgs).with_tools(None);
        assert!(payload.tools.is_none());
    }

    #[test]
    fn request_payload_borrows_nonempty_metadata_tools() {
        let msgs = [UnifiedMessage::user("search")];
        let tools = [crate::metadata::ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
            crate::metadata::ToolCategory::Builtin,
        )];
        let payload = RequestPayload {
            messages: &msgs,
            system_prompt: None,
            system_blocks: None,
            tools: Some(&tools),
            think_level: None,
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            model: None,
            metadata: None,
        };
        let field_tools: &[crate::MetadataToolDefinition] = payload.tools.expect("tools present");
        assert!(std::ptr::eq(field_tools, tools.as_slice()));

        let payload = RequestPayload::new(&msgs).with_tools(Some(field_tools));
        let builder_tools: &[crate::metadata::ToolDefinition] =
            payload.tools.expect("tools present");
        assert!(std::ptr::eq(builder_tools, tools.as_slice()));
        assert_eq!(builder_tools.len(), 1);
        assert_eq!(builder_tools[0].name, "search");
        assert_eq!(
            builder_tools[0].parameters,
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            })
        );
    }

    #[test]
    fn test_tool_choice_enum() {
        assert_eq!(ToolChoice::Auto, ToolChoice::Auto);
        assert_ne!(ToolChoice::Auto, ToolChoice::Required);
        assert_eq!(
            ToolChoice::Specific("s".into()),
            ToolChoice::Specific("s".into())
        );
    }

    #[test]
    fn test_payload_with_tool_choice() {
        let msgs = [UnifiedMessage::user("test")];
        let payload = RequestPayload::new(&msgs).with_tool_choice(Some(ToolChoice::Required));
        assert_eq!(payload.tool_choice, Some(ToolChoice::Required));
    }

    // ---- ThinkLevel (moved from alephcore::agents::thinking) ----

    #[test]
    fn default_is_minimal() {
        assert_eq!(ThinkLevel::default(), ThinkLevel::Minimal);
    }

    #[test]
    fn normalizes_canonical_names() {
        assert_eq!(normalize_think_level("off"), Some(ThinkLevel::Off));
        assert_eq!(normalize_think_level("minimal"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("low"), Some(ThinkLevel::Low));
        assert_eq!(normalize_think_level("medium"), Some(ThinkLevel::Medium));
        assert_eq!(normalize_think_level("high"), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
    }

    #[test]
    fn normalizes_aliases_and_is_case_insensitive() {
        assert_eq!(
            normalize_think_level("  ULTRATHINK "),
            Some(ThinkLevel::High)
        );
        assert_eq!(normalize_think_level("Think"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("extended"), Some(ThinkLevel::XHigh));
    }

    #[test]
    fn unknown_input_is_rejected_not_defaulted() {
        assert_eq!(normalize_think_level("deeeep"), None);
        assert!("deeeep".parse::<ThinkLevel>().is_err());
    }

    /// A new rung has to reach [`ThinkLevel::ALL`], and the compiler is the
    /// only thing that can insist.
    ///
    /// The `match` is what does the work: adding a variant makes it
    /// non-exhaustive, which fails the build *here*, next to the list. The
    /// assertion then catches the other half — a variant added to the enum
    /// and to the match but forgotten in `ALL`.
    #[test]
    fn all_covers_every_variant() {
        for level in ThinkLevel::ALL {
            match level {
                ThinkLevel::Off
                | ThinkLevel::Minimal
                | ThinkLevel::Low
                | ThinkLevel::Medium
                | ThinkLevel::High
                | ThinkLevel::XHigh => {}
            }
        }
        assert_eq!(
            ThinkLevel::ALL.len(),
            6,
            "a rung was added to the enum without joining ALL"
        );
    }

    /// `id()` is the session-metadata storage form, so it MUST survive a
    /// write/read round-trip — otherwise a persisted choice silently decays to
    /// "unknown" on the next turn.
    #[test]
    fn id_round_trips_through_normalize() {
        for level in [
            ThinkLevel::Off,
            ThinkLevel::Minimal,
            ThinkLevel::Low,
            ThinkLevel::Medium,
            ThinkLevel::High,
            ThinkLevel::XHigh,
        ] {
            assert_eq!(
                normalize_think_level(level.id()),
                Some(level),
                "id() must parse back to itself for {level:?}"
            );
            assert_eq!(level.to_string(), level.id());
        }
    }

    // ---- dispatch helpers ----

    /// Every variant must route to its own callback, in the enum's order,
    /// with the context threaded through.
    #[test]
    fn dispatch_think_level_routes_every_variant() {
        for (level, expected) in [
            (ThinkLevel::Off, "off"),
            (ThinkLevel::Minimal, "minimal"),
            (ThinkLevel::Low, "low"),
            (ThinkLevel::Medium, "medium"),
            (ThinkLevel::High, "high"),
            (ThinkLevel::XHigh, "xhigh"),
        ] {
            let mut seen: Vec<&'static str> = Vec::new();
            let hit = dispatch_think_level(
                &level,
                &mut seen,
                |c| {
                    c.push("off");
                    "off"
                },
                |c| {
                    c.push("minimal");
                    "minimal"
                },
                |c| {
                    c.push("low");
                    "low"
                },
                |c| {
                    c.push("medium");
                    "medium"
                },
                |c| {
                    c.push("high");
                    "high"
                },
                |c| {
                    c.push("xhigh");
                    "xhigh"
                },
            );
            assert_eq!(hit, expected);
            assert_eq!(seen, [expected], "exactly one callback fired");
        }
    }

    /// The const selector must be usable in a const context and must map the
    /// ladder in order — this is what lets Core keep its
    /// `pub(super) const fn map_think_level` budget table `const`.
    #[test]
    fn select_think_level_copy_maps_the_ladder() {
        const fn budget(level: &ThinkLevel) -> Option<u32> {
            select_think_level_copy(
                level,
                None,
                Some(1024),
                Some(4096),
                Some(10000),
                Some(20000),
                Some(50000),
            )
        }
        const BUDGETS: [Option<u32>; 6] = [
            budget(&ThinkLevel::Off),
            budget(&ThinkLevel::Minimal),
            budget(&ThinkLevel::Low),
            budget(&ThinkLevel::Medium),
            budget(&ThinkLevel::High),
            budget(&ThinkLevel::XHigh),
        ];
        assert_eq!(
            BUDGETS,
            [
                None,
                Some(1024),
                Some(4096),
                Some(10000),
                Some(20000),
                Some(50000)
            ]
        );
    }
}
