//! R7-safe capability floor for failover candidate selection.
//!
//! A prompt-blind, deterministic filter that drops candidate models which
//! *structurally cannot serve* a request — the request carries an image but
//! the model has no vision, supplies tool definitions but the model has no
//! tool-calling, or the estimated input exceeds the model's context window.
//! These are facts about the request's SHAPE (does it contain an image block?
//! a tools array? roughly how many input tokens?), never about its semantic
//! intent. Choosing "which models are even *capable*" is infrastructure
//! feasibility (R7 infrastructure layer), not the forbidden "classify the task to pick the
//! best model" router — the LLM still owns model choice; this only removes
//! candidates that would deterministically fail.
//!
//! Like [`route_policy`](super::route_policy), it shapes the candidate SET
//! handed to the existing failover engine; the harness never learns of it. It
//! is modelled on `LiteLLM`'s `_pre_call_checks` and is **fail-open**:
//!
//! * a model with unknown capabilities ([`capabilities_for`] = `None`) is kept
//!   — the static table is best-effort, not authoritative;
//! * if filtering would drop *every* model, the original list is returned
//!   unchanged so an over-eager gate (or a stale table) never empties the
//!   failover chain.

use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::{capabilities_for, ModelCapabilities};

/// Structural requirements derived from a request — never from prompt meaning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestRequirements {
    /// The request carries at least one image block (needs a vision model).
    pub needs_vision: bool,
    /// The request supplies tool definitions (needs a tool-calling model).
    pub needs_tools: bool,
    /// Estimated input tokens. `Some` gates against the model's context
    /// window; `None` skips that dimension.
    pub input_tokens: Option<u32>,
}

impl RequestRequirements {
    /// No constraint at all — the gate is a no-op (byte-identical passthrough).
    #[must_use]
    pub const fn is_unconstrained(&self) -> bool {
        !self.needs_vision && !self.needs_tools && self.input_tokens.is_none()
    }

    /// Derive requirements from the structural shape of a request.
    ///
    /// * `needs_vision` ← any message content block is an [`ContentBlock::Image`].
    /// * `needs_tools`  ← `has_tools` (the caller passes a non-empty tools array).
    /// * `input_tokens` ← a conservative `chars / 4` estimate over all message
    ///   text (the same ~4-chars-per-token heuristic Aleph uses elsewhere).
    ///
    /// The estimate is intentionally rough: a request only loses a candidate
    /// when it *clearly* overflows that model's window, and fail-open restores
    /// everything if the gate would otherwise empty the chain.
    pub fn from_request(messages: &[UnifiedMessage], has_tools: bool) -> Self {
        let needs_vision = messages
            .iter()
            .flat_map(UnifiedMessage::content_blocks)
            .any(|b| matches!(b, ContentBlock::Image { .. }));
        let chars: usize = messages
            .iter()
            .map(|m| m.text_content().chars().count())
            .sum();
        let input_tokens = u32::try_from(chars / 4).ok();
        Self {
            needs_vision,
            needs_tools: has_tools,
            input_tokens,
        }
    }
}

/// Why the gate dropped a candidate model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateReason {
    /// Estimated input exceeds the model's context window.
    ContextWindow,
    /// Request needs vision; model has none.
    NoVision,
    /// Request needs tool calling; model has none.
    NoTools,
}

/// The gate's verdict for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// The model can serve the request.
    Keep,
    /// The model would deterministically fail; drop it (with the reason).
    Drop(GateReason),
}

/// Classify one model against the request. Pure total function.
///
/// Any single unmet requirement is conclusive; the first failure wins (the
/// reason is for logging only, so order among failures is immaterial).
#[must_use]
pub const fn capability_gate(req: &RequestRequirements, caps: &ModelCapabilities) -> GateOutcome {
    if let Some(tokens) = req.input_tokens {
        if tokens > caps.context_window {
            return GateOutcome::Drop(GateReason::ContextWindow);
        }
    }
    if req.needs_vision && !caps.supports_vision {
        return GateOutcome::Drop(GateReason::NoVision);
    }
    if req.needs_tools && !caps.supports_tools {
        return GateOutcome::Drop(GateReason::NoTools);
    }
    GateOutcome::Keep
}

/// Filter a candidate's model list to those that can serve the request.
///
/// Fail-open: unknown-capability models are kept, and if every model is
/// dropped the original list is restored (an incomplete static table must
/// never hard-fail a request). When `req.is_unconstrained()` the input is
/// returned verbatim. Dropped models are logged for observability.
pub fn retain_capable_models(models: Vec<String>, req: &RequestRequirements) -> Vec<String> {
    if req.is_unconstrained() {
        return models;
    }
    let mut kept: Vec<String> = Vec::with_capacity(models.len());
    let mut dropped: Vec<(String, GateReason)> = Vec::new();
    for m in models {
        match capabilities_for(&m) {
            Some(caps) => match capability_gate(req, &caps) {
                GateOutcome::Keep => kept.push(m),
                GateOutcome::Drop(reason) => dropped.push((m, reason)),
            },
            None => kept.push(m), // unknown caps → fail-open keep
        }
    }
    if kept.is_empty() {
        let restored: Vec<String> = dropped.into_iter().map(|(m, _)| m).collect();
        tracing::warn!(
            count = restored.len(),
            "capability gate dropped every candidate model; failing open (keeping all)"
        );
        return restored;
    }
    for (m, reason) in &dropped {
        tracing::debug!(model = %m, ?reason, "capability gate dropped model");
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    // ── capability_gate: per-model verdict ───────────────────────────────

    fn caps(vision: bool, tools: bool, window: u32) -> ModelCapabilities {
        ModelCapabilities {
            context_window: window,
            max_output_tokens: 4096,
            supports_vision: vision,
            supports_tools: tools,
            supports_reasoning: false,
        }
    }

    #[test]
    fn unconstrained_request_keeps_any_model() {
        let req = RequestRequirements::default();
        assert!(req.is_unconstrained());
        assert_eq!(
            capability_gate(&req, &caps(false, false, 1000)),
            GateOutcome::Keep
        );
    }

    #[test]
    fn vision_request_drops_blind_model_keeps_seeing_model() {
        let req = RequestRequirements {
            needs_vision: true,
            ..Default::default()
        };
        assert_eq!(
            capability_gate(&req, &caps(false, true, 200_000)),
            GateOutcome::Drop(GateReason::NoVision)
        );
        assert_eq!(
            capability_gate(&req, &caps(true, true, 200_000)),
            GateOutcome::Keep
        );
    }

    #[test]
    fn tools_request_drops_model_without_tool_calling() {
        let req = RequestRequirements {
            needs_tools: true,
            ..Default::default()
        };
        assert_eq!(
            capability_gate(&req, &caps(true, false, 200_000)),
            GateOutcome::Drop(GateReason::NoTools)
        );
    }

    #[test]
    fn input_over_context_window_drops_model() {
        let req = RequestRequirements {
            input_tokens: Some(150_000),
            ..Default::default()
        };
        assert_eq!(
            capability_gate(&req, &caps(true, true, 128_000)),
            GateOutcome::Drop(GateReason::ContextWindow)
        );
        // Fits → kept.
        assert_eq!(
            capability_gate(&req, &caps(true, true, 200_000)),
            GateOutcome::Keep
        );
    }

    // ── retain_capable_models: list filtering + fail-open ─────────────────

    #[test]
    fn unconstrained_returns_list_verbatim() {
        let models = vec!["claude-sonnet-4".to_string(), "o1-mini".to_string()];
        let out = retain_capable_models(models.clone(), &RequestRequirements::default());
        assert_eq!(out, models);
    }

    #[test]
    fn vision_request_filters_out_blind_models() {
        // o1-mini has no vision; gpt-4o does.
        let models = vec!["o1-mini".to_string(), "gpt-4o".to_string()];
        let req = RequestRequirements {
            needs_vision: true,
            ..Default::default()
        };
        let out = retain_capable_models(models, &req);
        assert_eq!(out, vec!["gpt-4o".to_string()]);
    }

    #[test]
    fn unknown_capability_model_is_kept_fail_open() {
        let models = vec!["totally-made-up-model".to_string()];
        let req = RequestRequirements {
            needs_vision: true,
            ..Default::default()
        };
        let out = retain_capable_models(models.clone(), &req);
        assert_eq!(out, models);
    }

    #[test]
    fn dropping_every_model_fails_open_to_original() {
        // Both are blind to vision; a vision request would empty the list, so
        // the gate restores everything rather than hard-fail the request.
        let models = vec!["o1-mini".to_string(), "deepseek-chat".to_string()];
        let req = RequestRequirements {
            needs_vision: true,
            ..Default::default()
        };
        let out = retain_capable_models(models.clone(), &req);
        assert_eq!(out.len(), models.len());
        assert!(out.contains(&"o1-mini".to_string()));
        assert!(out.contains(&"deepseek-chat".to_string()));
    }

    // ── from_request: structural derivation ───────────────────────────────

    #[test]
    fn from_request_detects_image_block() {
        let msg = UnifiedMessage::user_with_content(vec![ContentBlock::Image {
            data: "base64".to_string(),
            mime_type: "image/png".to_string(),
        }]);
        let req = RequestRequirements::from_request(&[msg], false);
        assert!(req.needs_vision);
        assert!(!req.needs_tools);
    }

    #[test]
    fn from_request_text_only_no_tools_is_vision_and_tool_free() {
        let msg = UnifiedMessage::user("just text, no image");
        let req = RequestRequirements::from_request(&[msg], false);
        assert!(!req.needs_vision);
        assert!(!req.needs_tools);
        // input_tokens is always estimated (Some), so it is *not* unconstrained
        // — context-window gating still applies to plain text requests.
        assert!(req.input_tokens.is_some());
    }

    #[test]
    fn from_request_threads_has_tools_flag() {
        let msg = UnifiedMessage::user("hi");
        assert!(RequestRequirements::from_request(&[msg], true).needs_tools);
    }
}
