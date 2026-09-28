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

use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::model_catalog::strips_prior_turn_thinking;
use crate::providers::protocols::openai_common::provider_policy::{
    detect_endpoint_class, EndpointClass,
};

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
    /// ([`ProtocolAdapter::wire_family`](crate::providers::adapter::ProtocolAdapter::wire_family)),
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
                tool_turn && !thinking.is_empty() && origin == ThinkingOrigin::ResponsesEncrypted
            }
            Self::ReasoningContent { .. } => {
                !thinking.is_empty() && origin == ThinkingOrigin::Unsigned
            }
            Self::Drop => false,
        }
    }
}

#[cfg(test)]
mod tests;
