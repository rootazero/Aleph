//! Keep Core-owned protocol fragments out of plugins without deleting them
//! from retained, unchanged native assistant frames.

use super::{canonical_fingerprint, ContextFabricError};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::reasoning_replay::ReasoningReplay;
use std::collections::HashMap;

pub(super) fn public_messages(messages: &[UnifiedMessage]) -> Vec<UnifiedMessage> {
    let mut projected = ReasoningReplay::Drop.project(messages);
    for message in &mut projected {
        for block in message.content_blocks_mut() {
            if let ContentBlock::ToolCall {
                thought_signature, ..
            } = block
            {
                *thought_signature = None;
            }
        }
    }
    projected
}

pub(super) fn restore_private_frames(
    original: &[UnifiedMessage],
    candidate: &mut [UnifiedMessage],
) -> Result<(), ContextFabricError> {
    let mut by_public: HashMap<String, Option<usize>> = HashMap::new();
    let mut by_call: HashMap<&str, Option<usize>> = HashMap::new();
    let mut fingerprints = HashMap::new();
    for (index, message) in original.iter().enumerate() {
        if !message.content_blocks().iter().any(|block| {
            matches!(
                block,
                ContentBlock::Thinking { .. }
                    | ContentBlock::ToolCall {
                        thought_signature: Some(_),
                        ..
                    }
            )
        }) {
            continue;
        }
        let visible = public_messages(std::slice::from_ref(message));
        let Some(visible) = visible.first() else {
            continue;
        };
        let fingerprint = fingerprint(visible)?;
        by_public
            .entry(fingerprint.clone())
            .and_modify(|source| *source = None)
            .or_insert(Some(index));
        fingerprints.insert(index, fingerprint);
        for block in message.content_blocks() {
            if let ContentBlock::ToolCall { id, .. } = block {
                by_call
                    .entry(id.as_str())
                    .and_modify(|source| *source = None)
                    .or_insert(Some(index));
            }
        }
    }
    for message in candidate {
        let mut required_source = None;
        for block in message.content_blocks() {
            if let ContentBlock::ToolCall { id, .. } = block {
                if let Some(source) = by_call.get(id.as_str()) {
                    let source = source.ok_or_else(invalid_frame)?;
                    if required_source.is_some_and(|previous| previous != source) {
                        return Err(invalid_frame());
                    }
                    required_source = Some(source);
                }
            }
        }
        let fingerprint = fingerprint(message)?;
        let source = if let Some(source) = required_source {
            if fingerprints.get(&source) != Some(&fingerprint) {
                return Err(invalid_frame());
            }
            Some(source)
        } else {
            match by_public.get(&fingerprint) {
                Some(Some(source)) => Some(*source),
                Some(None) => return Err(invalid_frame()),
                None => None,
            }
        };
        if let Some(source) = source {
            *message = original[source].clone();
        }
    }
    Ok(())
}

fn fingerprint(message: &UnifiedMessage) -> Result<String, ContextFabricError> {
    serde_json::to_value(message)
        .map(|value| canonical_fingerprint(&value))
        .map_err(|error| ContextFabricError::Serialization(error.to_string()))
}

fn invalid_frame() -> ContextFabricError {
    ContextFabricError::MalformedOutput(
        "plugin changed or ambiguously retained a private protocol frame".into(),
    )
}
