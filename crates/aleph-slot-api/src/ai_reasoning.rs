//! Per-target reasoning replay: which persisted reasoning one target is sent.
//!
//! The message builder (`harness/agent/prompt.rs`) emits every persisted
//! thinking block as facts — its text, its signature, and whether it belongs to
//! an earlier user turn. The policy lives here, keyed on the concrete target:
//! one table ([`ReasoningReplay::for_target`]) and one pure projection
//! ([`ReasoningReplay::project_message`]). The wire (`transform_messages`,
//! called from `HttpProvider::execute` after failover picked the target) and
//! the context estimators apply the same projection, so the pressure gauge
//! counts exactly the reasoning the target receives.

use std::borrow::Cow;

use crate::ai_message::{ContentBlock, UnifiedMessage};

/// Who can verify a thinking block, derived from the signature's machine shape.
///
/// Never persisted: the shape already says it, for every log ever written. The
/// one thing it cannot say is which vendor produced *unsigned* reasoning, so a
/// session that switched vendors may replay one vendor's unsigned reasoning to
/// another's `reasoning_content`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingOrigin {
    /// An Anthropic Messages signed block. Anthropic-compatible hosts (Kimi,
    /// `MiniMax`) mint the same shape; this cannot tell them apart.
    AnthropicSigned,
    /// `OpenAI` Responses encrypted reasoning items, stored as NDJSON
    /// `{"id","ec"}` lines.
    ResponsesEncrypted,
    /// No signature: OpenAI-compatible `reasoning_content`, Gemini thought
    /// text, local models.
    Unsigned,
}

impl ThinkingOrigin {
    /// Classify a stored signature.
    #[must_use]
    pub fn of(signature: Option<&str>) -> Self {
        match signature {
            None => Self::Unsigned,
            Some(sig) if sig.trim_start().starts_with('{') => Self::ResponsesEncrypted,
            Some(_) => Self::AnthropicSigned,
        }
    }
}

/// What one target receives of the persisted reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningReplay {
    /// Anthropic Messages: Anthropic-signed thinking, on assistant turns that
    /// call tools. With `strip_earlier_turns` (models whose API drops
    /// prior-turn thinking itself) blocks from earlier user turns are not sent.
    AnthropicSigned { strip_earlier_turns: bool },
    /// `OpenAI` Responses: encrypted reasoning items, on assistant turns that
    /// call tools.
    ResponsesEncrypted,
    /// OpenAI-compatible `reasoning_content` on every assistant turn, from
    /// unsigned reasoning only. `fill_missing` gives a turn with none an empty
    /// value (for hosts that reject an assistant turn without the field).
    ReasoningContent { fill_missing: bool },
    /// Reasoning is never sent.
    Drop,
}

impl Default for ReasoningReplay {
    /// What the message builder hard-coded before this policy existed — signed
    /// thinking on tool turns only — so a consumer with no target counts the
    /// reasoning it always counted.
    fn default() -> Self {
        Self::AnthropicSigned {
            strip_earlier_turns: false,
        }
    }
}

impl ReasoningReplay {
    /// The one table: protocol × host × model → policy.
    ///
    /// `protocol` is the adapter's wire family
    /// (`ProtocolAdapter::wire_family` in Core),
    /// never a user-chosen protocol name — a YAML protocol that extends
    /// `anthropic` must resolve as `anthropic`. Hosts come from the protocols'
    /// own endpoint classifiers and model facts from the model catalog.
    /// Anything not named here keeps its protocol's pre-policy behaviour —
    /// never more reasoning, never less.
    #[must_use]
    pub fn for_target(protocol: &str, base_url: Option<&str>, model: &str) -> Self {
        match protocol {
            "anthropic" => Self::AnthropicSigned {
                strip_earlier_turns: strips_prior_turn_thinking(model),
            },
            "openai-responses" => Self::ResponsesEncrypted,
            "openai" => match detect_endpoint_class(base_url) {
                // With tools, every earlier turn's reasoning_content must come
                // back or the request is a 400 (api-docs.deepseek.com
                // /guides/thinking_mode). What to send for a turn that has none
                // is undocumented; an empty value at least satisfies presence.
                EndpointClass::DeepSeekNative => Self::ReasoningContent { fill_missing: true },
                // "Pass the complete assistant message returned by the API back
                // as-is" (platform.moonshot.ai, thinking models). No presence
                // rule is documented, so a turn without reasoning stays bare.
                EndpointClass::MoonshotNative => Self::ReasoningContent {
                    fill_missing: false,
                },
                _ => Self::Drop,
            },
            _ => Self::Drop,
        }
    }

    /// Apply the policy to one message; `None` when nothing of it is sent.
    ///
    /// Only assistant messages change. One whose content is nothing but
    /// reasoning is dropped whatever the policy — there is no answer to hang
    /// the reasoning on, and such turns were never replayed.
    #[must_use]
    pub fn project_message<'a>(
        &self,
        message: &'a UnifiedMessage,
    ) -> Option<Cow<'a, UnifiedMessage>> {
        let UnifiedMessage::Assistant { content } = message else {
            return Some(Cow::Borrowed(message));
        };
        if content
            .iter()
            .all(|b| matches!(b, ContentBlock::Thinking { .. }))
        {
            return None;
        }
        let tool_turn = content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolCall { .. }));
        let keep = |block: &ContentBlock| match block {
            ContentBlock::Thinking {
                thinking,
                signature,
                earlier_turn,
            } => self.keeps(thinking, signature.as_deref(), *earlier_turn, tool_turn),
            _ => true,
        };
        let fill = matches!(self, Self::ReasoningContent { fill_missing: true })
            && !content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { .. }) && keep(b));
        if !fill && content.iter().all(keep) {
            return Some(Cow::Borrowed(message));
        }
        let mut projected = Vec::with_capacity(content.len() + usize::from(fill));
        if fill {
            projected.push(ContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
                earlier_turn: false,
            });
        }
        projected.extend(content.iter().filter(|b| keep(b)).cloned());
        Some(Cow::Owned(UnifiedMessage::Assistant { content: projected }))
    }

    /// [`project_message`](Self::project_message) over a list, borrowing every
    /// message it leaves unchanged — the estimators' form.
    pub fn projected<'a>(
        &'a self,
        messages: &'a [UnifiedMessage],
    ) -> impl Iterator<Item = Cow<'a, UnifiedMessage>> + 'a {
        messages.iter().filter_map(|m| self.project_message(m))
    }

    /// [`project_message`](Self::project_message) over a list — the wire's form.
    #[must_use]
    pub fn project(&self, messages: &[UnifiedMessage]) -> Vec<UnifiedMessage> {
        self.projected(messages).map(Cow::into_owned).collect()
    }

    fn keeps(
        &self,
        thinking: &str,
        signature: Option<&str>,
        earlier_turn: bool,
        tool_turn: bool,
    ) -> bool {
        let origin = ThinkingOrigin::of(signature);
        match *self {
            Self::AnthropicSigned {
                strip_earlier_turns,
            } => {
                tool_turn
                    && !thinking.is_empty()
                    && origin == ThinkingOrigin::AnthropicSigned
                    && !(strip_earlier_turns && earlier_turn)
            }
            Self::ResponsesEncrypted => {
                // Encrypted reasoning can arrive without any summary text.
                tool_turn && origin == ThinkingOrigin::ResponsesEncrypted
            }
            Self::ReasoningContent { .. } => {
                !thinking.is_empty() && origin == ThinkingOrigin::Unsigned
            }
            Self::Drop => false,
        }
    }
}

// Pure model/endpoint leaves; Core re-exports these same implementations.

/// Provider/vendor tags stripped from the front of a model id during
/// canonicalisation. Lowercased. Scanned in order; nested tags
/// (`x-ai/openai/…`) peel one layer per matching entry.
const VENDOR_TAGS: &[&str] = &[
    "anthropic/",
    // Bedrock-style dot tag ("anthropic.claude-sonnet-4-6") — stripping it
    // lets the capability catalog resolve Bedrock-hosted Claude ids, and (since
    // `pricing::lookup_rates` gained its vendor-inferred fallback) also lets
    // `amazon-bedrock` be priced at Anthropic's rates instead of `Unknown`.
    // The estimate is flagged `RateBasis::VendorInferred` so the Bedrock
    // margin stays visible rather than being passed off as a quote.
    "anthropic.",
    "openai/",
    "google/",
    "models/",
    "deepseek/",
    "x-ai/",
    "xai/",
    "mistralai/",
    "mistral/",
    "moonshotai/",
    "moonshot/",
    "qwen/",
    "meta-llama/",
    "meta/",
    "z-ai/",
    "zai/",
];

/// Strip a leading vendor tag and a trailing `YYYYMMDD` date stamp from a
/// model id, lower-casing for case-insensitive prefix matching.
///
/// ```text
/// "anthropic/Claude-Sonnet-4-6-20250520" -> "claude-sonnet-4-6"
/// "gpt-4o-2024-11-20"                     -> "gpt-4o-2024-11-20" (non-8-digit tail kept)
/// "deepseek-ai/DeepSeek-V3"               -> "deepseek-v3"       (org path collapsed)
/// "accounts/fireworks/models/kimi-k2p6"   -> "kimi-k2.6"         (host path + `p` separator)
/// "llama3.3:70b"                          -> "llama3.3"          (ollama size tag)
/// ```
///
/// The result is a **table-lookup key only** — pricing, capabilities and
/// Core's `infer_vendor` all consume it, and none of them ever puts it back on the
/// wire. The outgoing request always carries the operator's raw model id, so
/// collapsing a host path here can never mis-address a provider.
#[doc(hidden)]
#[must_use]
pub fn canonicalize_model_id(model: &str) -> String {
    let mut m = model.trim().to_ascii_lowercase();
    // Peel vendor tags until none match. A single in-order pass would miss
    // nested aggregator tags whose inner tag precedes the outer in the table
    // (e.g. "x-ai/openai/…" — "openai/" is scanned before "x-ai/"). Each peel
    // strictly shortens the string, so the loop terminates.
    loop {
        let before = m.len();
        for tag in VENDOR_TAGS {
            if let Some(rest) = m.strip_prefix(tag) {
                m = rest.to_string();
            }
        }
        if m.len() == before {
            break;
        }
    }
    // Collapse any remaining host/org path to its last segment. [`VENDOR_TAGS`]
    // only knows the tags aggregators had when it was written; hosts keep
    // inventing new shapes ("deepseek-ai/…", "accounts/fireworks/models/…",
    // "@cf/meta/…"), and every unlisted shape used to miss BOTH the capability
    // table (→ conservative 128K window → premature compression) and the price
    // table (→ `CostStatus::Unknown` → `u64::MAX` under `cost_aware`). The
    // trailing segment is the vendor's own model id in every hosting scheme we
    // ship a preset for, so this generalises the fixed table instead of
    // chasing it.
    if let Some(idx) = m.rfind('/') {
        m = m[idx + 1..].to_string();
    }
    // Drop an Ollama-style `:tag` (size / quantisation variant): "llama3.3:70b",
    // "qwen3:8b-q4_K_M". The tag picks a *weight file*, not a different model
    // family, so it must not defeat the family lookup.
    if let Some(idx) = m.find(':') {
        m.truncate(idx);
    }
    // Drop a trailing 8-digit date stamp (e.g. "-20250520"). Arbitrary
    // dash-separated dates with non-8-digit tails are left intact.
    if let Some(idx) = m.rfind('-') {
        let tail = &m[idx + 1..];
        if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
            m.truncate(idx);
        }
    }
    // Fireworks writes the generation separator as `p` because its ids double as
    // URL path segments: `kimi-k2p6` is Kimi K2.6, `glm-5p2-fast` is GLM-5.2
    // Fast. Same class of fact as the host-path collapse above — how one host
    // spells an id, not a claim about the model — and the same blast radius,
    // since the result is a lookup key that never goes back on the wire.
    //
    // Without it, `glm-5p2-fast` fell past the `glm-5.2` rate to the GLM-4
    // family fallback (a third of the real price) and `kimi-k2p6` past
    // `kimi-k2.6` to the legacy Moonshot rate.
    m = restore_dotted_generation(&m);
    m
}

/// Rewrite `<digit>p<digit>` as `<digit>.<digit>` — Fireworks' URL-safe
/// spelling of a version separator.
///
/// Only fires between two ASCII digits, so ordinary words keep their `p`
/// (`llama-v3p3` → `llama-v3.3` is the intended case; `gpt-oss-120b` and
/// `deepseek-v4-pro` have no digit-`p`-digit run and are returned untouched).
fn restore_dotted_generation(id: &str) -> String {
    let bytes = id.as_bytes();
    let mut out = String::with_capacity(id.len());
    for (i, ch) in id.char_indices() {
        let is_separator = ch == 'p'
            && i > 0
            && bytes[i - 1].is_ascii_digit()
            && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
        out.push(if is_separator { '.' } else { ch });
    }
    out
}

/// Prefix match over a canonicalised model id, treating `.` and `-` as the
/// same separator.
///
/// Hosts disagree on how to spell a generation. Anthropic publishes
/// `claude-opus-4-8`; GitHub Copilot publishes `claude-opus-4.8` for the very
/// same model. Canonicalisation cannot fold one form into the other, because
/// the fold has no safe direction: rewriting `.`→`-` in the *id* alone breaks
/// every table prefix that is natively dotted (`gpt-5.6`, `kimi-k2.6`,
/// `glm-5.2`), and rewriting `-`→`.` corrupts ids where a dash between digits
/// is not a version separator at all (`llama-3-70b` is Llama 3 at 70B, not
/// Llama 3.70). Folding at *comparison* time sidesteps both: it is symmetric by
/// construction, needs no allocation, and lets every table keep the spelling
/// its vendor actually publishes — which is what makes the tables checkable
/// against vendor docs by eye.
///
/// Equating the two characters unconditionally (rather than only between
/// digits) is safe because a `.`/`-` swap at the same position has never named
/// a different model, and the tables are barred from relying on the
/// distinction: two rows that differ only by separator are fold-equal, so the
/// prefix-shadow guards — which use this same predicate — reject them.
///
/// This is the single matching predicate for all four prefix tables
/// (Core's `MODEL_VENDOR_PREFIXES`, the capability table, the lifecycle table, and
/// pricing's rate/tier tables). A guard that compares prefixes with anything
/// else goes blind to exactly the shadowing this introduces.
#[doc(hidden)]
#[must_use]
pub fn prefix_matches(canonical_id: &str, prefix: &str) -> bool {
    let (id, pre) = (canonical_id.as_bytes(), prefix.as_bytes());
    // `zip` stops at the shorter side, so the length check is what keeps a
    // prefix longer than the id from matching vacuously.
    pre.len() <= id.len()
        && id
            .iter()
            .zip(pre)
            .all(|(a, b)| fold_sep(*a) == fold_sep(*b))
}

/// Collapse the two interchangeable separator spellings onto one byte.
///
/// Byte-wise is sound for ids that are not pure ASCII: `.` and `-` cannot occur
/// inside a UTF-8 multi-byte sequence, so no continuation byte is ever rewritten
/// and a byte-prefix match remains a character-prefix match.
const fn fold_sep(b: u8) -> u8 {
    if b == b'.' {
        b'-'
    } else {
        b
    }
}

/// Canonical id prefixes of Claude models whose API strips thinking blocks from
/// turns before the current user turn by itself (Opus 4.1 and earlier, Sonnet
/// 4.5 and earlier, every Haiku through 4.5 — docs.claude.com context-editing,
/// "keep only the last turn's thinking" defaults). A closed list of shipped
/// models: anything newer or unknown preserves prior-turn thinking, which is
/// what the API does for Opus 4.5+, Sonnet 4.6+ and the Claude 5 family.
///
/// Membership only — order does not matter, so no shadow guard applies.
const STRIPS_PRIOR_TURN_THINKING: &[&str] = &[
    "claude-3",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
];

/// The 4.0 generation's canonical ids (`claude-opus-4-20250514` loses its date
/// stamp in canonicalisation). Matched exactly: as prefixes they would swallow
/// every later 4.x model, which preserves.
const STRIPS_PRIOR_TURN_THINKING_EXACT: &[&str] = &["claude-opus-4", "claude-sonnet-4"];

/// True when `model`'s API drops thinking from earlier user turns itself, so
/// sending it is wire weight the model never reads. Unknown models → `false`.
#[doc(hidden)]
#[must_use]
pub fn strips_prior_turn_thinking(model: &str) -> bool {
    let canon = canonicalize_model_id(model);
    STRIPS_PRIOR_TURN_THINKING_EXACT.contains(&canon.as_str())
        || STRIPS_PRIOR_TURN_THINKING
            .iter()
            .any(|prefix| prefix_matches(&canon, prefix))
}

/// Detected provider endpoint class.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointClass {
    /// Official `OpenAI` API (api.openai.com)
    OpenAiPublic,
    /// `OpenAI` Codex (chatgpt.com)
    OpenAiCodex,
    /// Azure `OpenAI` Service (*.openai.azure.com)
    AzureOpenAi,
    /// Anthropic public API (api.anthropic.com)
    AnthropicPublic,
    /// `DeepSeek` native API (api.deepseek.com)
    DeepSeekNative,
    /// Groq native API (api.groq.com)
    GroqNative,
    /// Mistral public API (api.mistral.ai)
    MistralPublic,
    /// Moonshot native API (api.moonshot.ai / api.moonshot.cn)
    MoonshotNative,
    /// Cerebras native API (api.cerebras.ai)
    CerebrasNative,
    /// X.AI / Grok native API (api.x.ai / api.grok.x.ai)
    XAiNative,
    /// `OpenRouter` (openrouter.ai)
    OpenRouter,
    /// Local endpoint (localhost / 127.0.0.1 / *.local)
    Local,
    /// Unknown / custom endpoint
    Custom,
}

/// Detect endpoint class from base URL.
#[doc(hidden)]
#[must_use]
pub fn detect_endpoint_class(base_url: Option<&str>) -> EndpointClass {
    let url = match base_url {
        None | Some("") => return EndpointClass::OpenAiPublic,
        Some(u) => u,
    };

    let host = match extract_hostname(url) {
        Some(h) => h.to_lowercase(),
        None => return EndpointClass::Custom,
    };

    match host.as_str() {
        "api.openai.com" => EndpointClass::OpenAiPublic,
        "chatgpt.com" => EndpointClass::OpenAiCodex,
        "api.anthropic.com" => EndpointClass::AnthropicPublic,
        "api.deepseek.com" => EndpointClass::DeepSeekNative,
        "api.groq.com" => EndpointClass::GroqNative,
        "api.mistral.ai" => EndpointClass::MistralPublic,
        "api.moonshot.ai" | "api.moonshot.cn" | "api.kimi.com" => EndpointClass::MoonshotNative,
        "api.cerebras.ai" => EndpointClass::CerebrasNative,
        "api.x.ai" | "api.grok.x.ai" => EndpointClass::XAiNative,
        _ => {
            if host.ends_with(".openai.azure.com") {
                EndpointClass::AzureOpenAi
            } else if host.ends_with("openrouter.ai") {
                EndpointClass::OpenRouter
            } else if is_local_host(&host) {
                EndpointClass::Local
            } else {
                EndpointClass::Custom
            }
        }
    }
}

fn extract_hostname(url: &str) -> Option<String> {
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };

    with_scheme
        .parse::<url::Url>()
        .ok()
        .map(|u| u.host_str().unwrap_or("").to_string())
}

fn is_local_host(host: &str) -> bool {
    const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];
    if LOOPBACK.iter().any(|l| l.eq_ignore_ascii_case(host)) {
        return true;
    }
    let lower = host.to_ascii_lowercase();
    lower.ends_with(".localhost") || lower.ends_with(".local")
}

#[cfg(test)]
mod tests {
    //! `ReasoningReplay`: the table, the origin derivation, and the projection
    //! matrix (policy × origin × turn). Wire-level effects are pinned in
    //! `http_provider/replay_tests.rs`.

    use std::borrow::Cow;

    use super::{ReasoningReplay, ThinkingOrigin};
    use crate::ai_message::{ContentBlock, UnifiedMessage};
    use serde_json::json;

    const NDJSON: &str = "{\"id\":\"rs_1\",\"ec\":\"gAAA\"}\n";

    fn thinking(text: &str, signature: Option<&str>, earlier_turn: bool) -> ContentBlock {
        ContentBlock::Thinking {
            thinking: text.into(),
            signature: signature.map(str::to_string),
            earlier_turn,
        }
    }

    fn tool() -> ContentBlock {
        ContentBlock::ToolCall {
            id: "c1".into(),
            name: "t".into(),
            arguments: json!({}),
            thought_signature: None,
        }
    }

    fn text() -> ContentBlock {
        ContentBlock::Text {
            text: "answer".into(),
            cache_control: None,
        }
    }

    /// Thinking texts left in `message` after projection (`None` = message dropped).
    fn kept(replay: ReasoningReplay, message: &UnifiedMessage) -> Option<Vec<String>> {
        replay.project_message(message).map(|m| {
            m.content_blocks()
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Thinking { thinking, .. } => Some(thinking.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    fn assistant(content: Vec<ContentBlock>) -> UnifiedMessage {
        UnifiedMessage::Assistant { content }
    }

    #[test]
    fn origin_is_read_off_the_signature_shape() {
        assert_eq!(ThinkingOrigin::of(None), ThinkingOrigin::Unsigned);
        assert_eq!(
            ThinkingOrigin::of(Some("EqQBCkgIBRABGAIiQL")),
            ThinkingOrigin::AnthropicSigned
        );
        assert_eq!(
            ThinkingOrigin::of(Some(NDJSON)),
            ThinkingOrigin::ResponsesEncrypted
        );
        assert_eq!(
            ThinkingOrigin::of(Some("  {\"id\":\"x\"}")),
            ThinkingOrigin::ResponsesEncrypted
        );
    }

    #[test]
    fn the_table_resolves_protocol_host_and_model() {
        use ReasoningReplay::*;
        let r = ReasoningReplay::for_target;
        assert_eq!(
            r("anthropic", None, "claude-opus-4-7"),
            AnthropicSigned {
                strip_earlier_turns: false
            }
        );
        assert_eq!(
            r("anthropic", None, "claude-sonnet-4-5"),
            AnthropicSigned {
                strip_earlier_turns: true
            }
        );
        assert_eq!(
            r(
                "anthropic",
                Some("https://api.moonshot.cn/anthropic"),
                "kimi-k2"
            ),
            AnthropicSigned {
                strip_earlier_turns: false
            }
        );
        assert_eq!(r("openai-responses", None, "gpt-5.6"), ResponsesEncrypted);
        assert_eq!(
            r(
                "openai",
                Some("https://api.deepseek.com"),
                "deepseek-v4-flash"
            ),
            ReasoningContent { fill_missing: true }
        );
        assert_eq!(
            r(
                "openai",
                Some("https://api.moonshot.ai/v1"),
                "kimi-k2-thinking"
            ),
            ReasoningContent {
                fill_missing: false
            }
        );
        for (protocol, base_url) in [
            ("openai", None),
            ("openai", Some("https://open.bigmodel.cn/api/paas/v4")),
            ("openai", Some("http://localhost:1234/v1")),
            ("gemini", None),
            ("ollama", None),
            ("unknown", None),
        ] {
            assert_eq!(r(protocol, base_url, "m"), Drop, "{protocol} {base_url:?}");
        }
    }

    /// Anthropic keeps exactly the old message-builder rule (signed, non-empty,
    /// tool turn) minus foreign signatures, minus earlier turns when stripping.
    #[test]
    fn anthropic_projection_matrix() {
        let keep = ReasoningReplay::AnthropicSigned {
            strip_earlier_turns: false,
        };
        let strip = ReasoningReplay::AnthropicSigned {
            strip_earlier_turns: true,
        };
        let signed_tool_now = assistant(vec![thinking("a", Some("sig"), false), tool()]);
        let signed_tool_earlier = assistant(vec![thinking("b", Some("sig"), true), tool()]);
        let signed_text = assistant(vec![thinking("c", Some("sig"), false), text()]);
        let unsigned_tool = assistant(vec![thinking("d", None, false), tool()]);
        let ndjson_tool = assistant(vec![thinking("e", Some(NDJSON), false), tool()]);
        let empty_signed_tool = assistant(vec![thinking("", Some("sig"), false), tool()]);

        assert_eq!(kept(keep, &signed_tool_now), Some(vec!["a".into()]));
        assert_eq!(kept(keep, &signed_tool_earlier), Some(vec!["b".into()]));
        assert_eq!(kept(strip, &signed_tool_now), Some(vec!["a".into()]));
        assert_eq!(kept(strip, &signed_tool_earlier), Some(vec![]));
        for (message, why) in [
            (&signed_text, "text-only turn"),
            (&unsigned_tool, "unsigned"),
            (&ndjson_tool, "Responses signature"),
            (&empty_signed_tool, "empty"),
        ] {
            assert_eq!(kept(keep, message), Some(vec![]), "{why}");
        }
    }

    #[test]
    fn responses_projection_keeps_its_own_signatures_on_tool_turns() {
        let r = ReasoningReplay::ResponsesEncrypted;
        assert_eq!(
            kept(
                r,
                &assistant(vec![thinking("a", Some(NDJSON), true), tool()])
            ),
            Some(vec!["a".into()])
        );
        assert_eq!(
            kept(
                r,
                &assistant(vec![thinking("b", Some(NDJSON), false), text()])
            ),
            Some(vec![])
        );
        assert_eq!(
            kept(
                r,
                &assistant(vec![thinking("c", Some("sig"), false), tool()])
            ),
            Some(vec![])
        );
    }

    #[test]
    fn responses_projection_preserves_signature_only_tool_turns() {
        for earlier_turn in [false, true] {
            let message = assistant(vec![thinking("", Some(NDJSON), earlier_turn), tool()]);
            let projected = ReasoningReplay::ResponsesEncrypted
                .project_message(&message)
                .expect("tool turn survives");
            assert_eq!(
                serde_json::to_value(projected.as_ref()).unwrap(),
                serde_json::to_value(&message).unwrap(),
            );
            assert!(matches!(
                &projected.content_blocks()[0],
                ContentBlock::Thinking { thinking, signature, .. }
                    if thinking.is_empty() && signature.as_deref() == Some(NDJSON)
            ));
            for policy in [
                ReasoningReplay::default(),
                ReasoningReplay::ReasoningContent {
                    fill_missing: false,
                },
                ReasoningReplay::Drop,
            ] {
                assert_eq!(
                    serde_json::to_value(policy.project(std::slice::from_ref(&message))).unwrap(),
                    serde_json::to_value(vec![assistant(vec![tool()])]).unwrap(),
                    "foreign signature must not cross policies: {policy:?}",
                );
            }
        }
    }

    #[test]
    fn signature_only_reasoning_requires_responses_origin_and_a_tool_turn() {
        let policy = ReasoningReplay::ResponsesEncrypted;
        for signature in [None, Some("ordinary-sig"), Some("")] {
            let message = assistant(vec![thinking("", signature, false), tool()]);
            assert_eq!(
                serde_json::to_value(policy.project(&[message])).unwrap(),
                serde_json::to_value(vec![assistant(vec![tool()])]).unwrap(),
            );
        }
        let text_turn = assistant(vec![thinking("", Some(NDJSON), false), text()]);
        assert_eq!(
            serde_json::to_value(policy.project(&[text_turn])).unwrap(),
            serde_json::to_value(vec![assistant(vec![text()])]).unwrap(),
        );
        let signature_only = assistant(vec![thinking("", Some(NDJSON), false)]);
        assert!(policy.project_message(&signature_only).is_none());
    }

    #[test]
    fn reasoning_content_projection_replays_unsigned_on_every_turn() {
        let fill = ReasoningReplay::ReasoningContent { fill_missing: true };
        let bare = ReasoningReplay::ReasoningContent {
            fill_missing: false,
        };
        let unsigned_text_earlier = assistant(vec![thinking("r", None, true), text()]);
        let no_reasoning = assistant(vec![text()]);
        let foreign = assistant(vec![thinking("s", Some("sig"), false), tool()]);

        assert_eq!(kept(fill, &unsigned_text_earlier), Some(vec!["r".into()]));
        assert_eq!(kept(bare, &unsigned_text_earlier), Some(vec!["r".into()]));
        assert_eq!(kept(fill, &no_reasoning), Some(vec![String::new()]));
        assert_eq!(kept(bare, &no_reasoning), Some(vec![]));
        assert_eq!(kept(fill, &foreign), Some(vec![String::new()]));
        assert_eq!(kept(bare, &foreign), Some(vec![]));
    }

    #[test]
    fn drop_projection_sends_no_reasoning() {
        let m = assistant(vec![thinking("x", None, false), tool()]);
        assert_eq!(kept(ReasoningReplay::Drop, &m), Some(vec![]));
    }

    /// A turn with nothing but reasoning has no answer to carry it — dropped under
    /// every policy, as the message builder always did.
    #[test]
    fn a_reasoning_only_turn_is_never_sent() {
        let m = assistant(vec![thinking("x", None, false)]);
        for replay in [
            ReasoningReplay::default(),
            ReasoningReplay::ResponsesEncrypted,
            ReasoningReplay::ReasoningContent { fill_missing: true },
            ReasoningReplay::Drop,
        ] {
            assert!(replay.project_message(&m).is_none(), "{replay:?}");
        }
    }

    /// The estimators run this over the whole history every turn: a message the
    /// policy leaves untouched must be borrowed, not copied.
    #[test]
    fn untouched_messages_are_borrowed() {
        let replay = ReasoningReplay::default();
        let user = UnifiedMessage::user("hi");
        let plain = assistant(vec![text(), tool()]);
        let kept_as_is = assistant(vec![thinking("a", Some("sig"), false), tool()]);
        for m in [&user, &plain, &kept_as_is] {
            assert!(
                matches!(replay.project_message(m), Some(Cow::Borrowed(_))),
                "{m:?}"
            );
        }
    }
}

#[cfg(test)]
mod pure_leaf_tests {
    use super::*;

    #[test]
    fn separator_spellings_are_interchangeable_in_a_prefix_match() {
        assert!(prefix_matches("claude-opus-4.8", "claude-opus-4-8"));
        assert!(prefix_matches("claude-opus-4-8", "claude-opus-4.8"));
        assert!(prefix_matches("llama-3-3-70b-instruct", "llama-3.3"));
        // A longer prefix must never match vacuously — `zip` alone would stop
        // at the shorter side and report success.
        assert!(!prefix_matches("gpt-5", "gpt-5.6"));
        // Folding must not reach across a differing digit.
        assert!(!prefix_matches("llama-3-70b", "llama-3.3"));
        assert!(!prefix_matches("grok-4-fast", "grok-4.3"));
    }

    #[test]
    fn canonicalize_strips_vendor_tag_and_date() {
        assert_eq!(
            canonicalize_model_id("anthropic/Claude-Sonnet-4-6-20250520"),
            "claude-sonnet-4-6"
        );
        assert_eq!(canonicalize_model_id("gpt-4o-20241120"), "gpt-4o");
        // Non-8-digit dash tail is left intact.
        assert_eq!(
            canonicalize_model_id("openai/gpt-4o-2024-11-20"),
            "gpt-4o-2024-11-20"
        );
    }

    #[test]
    fn canonicalize_peels_nested_aggregator_tags() {
        assert_eq!(canonicalize_model_id("x-ai/grok-4"), "grok-4");
    }

    #[test]
    fn canonicalize_collapses_unlisted_host_paths() {
        // Hosting schemes absent from VENDOR_TAGS used to miss both lookup
        // tables entirely; the trailing segment is the vendor id in all of them.
        assert_eq!(
            canonicalize_model_id("deepseek-ai/DeepSeek-V3"),
            "deepseek-v3"
        );
        assert_eq!(
            // `p` between digits is Fireworks' version separator; see
            // `canonicalize_restores_fireworks_p_separator`.
            canonicalize_model_id("accounts/fireworks/models/llama-v3p3-70b-instruct"),
            "llama-v3.3-70b-instruct"
        );
        assert_eq!(
            canonicalize_model_id("@cf/meta/llama-3.3-70b-instruct-fp8-fast"),
            "llama-3.3-70b-instruct-fp8-fast"
        );
        // Listed tags keep working (peeled before the collapse, same result).
        assert_eq!(canonicalize_model_id("openai/gpt-4o"), "gpt-4o");
    }

    #[test]
    fn canonicalize_restores_fireworks_p_separator() {
        // Both Fireworks defaults used to miss their curated rows entirely.
        assert_eq!(
            canonicalize_model_id("accounts/fireworks/models/kimi-k2p6"),
            "kimi-k2.6"
        );
        assert_eq!(
            canonicalize_model_id("accounts/fireworks/routers/glm-5p2-fast"),
            "glm-5.2-fast"
        );
        assert_eq!(
            canonicalize_model_id("accounts/fireworks/models/llama-v3p3-70b-instruct"),
            "llama-v3.3-70b-instruct"
        );
        // A `p` that is not between two digits is left alone — otherwise every
        // `-pro` / `gpt-oss` / `-preview` id would be mangled.
        assert_eq!(canonicalize_model_id("deepseek-v4-pro"), "deepseek-v4-pro");
        assert_eq!(canonicalize_model_id("gpt-oss-120b"), "gpt-oss-120b");
        assert_eq!(
            canonicalize_model_id("gemini-3.1-pro-preview"),
            "gemini-3.1-pro-preview"
        );
        assert_eq!(canonicalize_model_id("sonar-pro"), "sonar-pro");
    }

    #[test]
    fn canonicalize_strips_ollama_size_tag() {
        assert_eq!(canonicalize_model_id("llama3.3:70b"), "llama3.3");
        assert_eq!(canonicalize_model_id("qwen3:8b-q4_K_M"), "qwen3");
        // A colon-free id is untouched.
        assert_eq!(canonicalize_model_id("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn canonicalize_strips_bedrock_dot_tag() {
        assert_eq!(
            canonicalize_model_id("anthropic.claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn prior_turn_thinking_is_stripped_only_by_the_older_models() {
        for id in [
            "claude-opus-4-1-20250805",
            "claude-opus-4-20250514",
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-20250514",
            "claude-haiku-4-5",
            "claude-3-7-sonnet-20250219",
            "anthropic.claude-3-5-haiku-20241022-v1:0",
        ] {
            assert!(
                strips_prior_turn_thinking(id),
                "{id} strips prior-turn thinking"
            );
        }
        for id in [
            "claude-opus-4-5",
            "claude-opus-4-8",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
            "claude-opus-5-5",
            "claude-fable-5-1",
            "kimi-k2-thinking",
            "",
        ] {
            assert!(
                !strips_prior_turn_thinking(id),
                "{id} preserves prior-turn thinking"
            );
        }
    }

    #[test]
    fn test_detect_openai_public() {
        assert_eq!(
            detect_endpoint_class(Some("https://api.openai.com/v1")),
            EndpointClass::OpenAiPublic
        );
    }

    #[test]
    fn test_detect_deepseek() {
        assert_eq!(
            detect_endpoint_class(Some("https://api.deepseek.com")),
            EndpointClass::DeepSeekNative
        );
    }

    #[test]
    fn test_detect_localhost() {
        assert_eq!(
            detect_endpoint_class(Some("http://localhost:8080")),
            EndpointClass::Local
        );
    }

    #[test]
    fn test_detect_azure() {
        assert_eq!(
            detect_endpoint_class(Some("https://my-resource.openai.azure.com")),
            EndpointClass::AzureOpenAi
        );
    }

    #[test]
    fn test_detect_kimi() {
        assert_eq!(
            detect_endpoint_class(Some("https://api.kimi.com/coding/v1")),
            EndpointClass::MoonshotNative
        );
    }

    #[test]
    fn test_detect_openrouter() {
        assert_eq!(
            detect_endpoint_class(Some("https://openrouter.ai/api/v1")),
            EndpointClass::OpenRouter
        );
    }

    #[test]
    fn test_detect_default_is_openai() {
        assert_eq!(detect_endpoint_class(None), EndpointClass::OpenAiPublic);
        assert_eq!(detect_endpoint_class(Some("")), EndpointClass::OpenAiPublic);
    }

    #[test]
    fn test_custom_endpoint() {
        assert_eq!(
            detect_endpoint_class(Some("https://example.com")),
            EndpointClass::Custom
        );
    }
}
