//! References to settled, displayable provider thought blocks.
//!
//! A reference conveys identity only. Core must authorize the addressed session
//! and run before resolving the already masked persisted source.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Opaque identity of one persisted thought block; clients must not interpret it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ThoughtBlockId(pub String);

/// An addressed block, never a caller-supplied translation source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ThoughtBlockRef {
    pub session_key: String,
    pub run_id: String,
    pub block_id: ThoughtBlockId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranslationEngine {
    Machine,
    Ai,
}

/// Locales currently shipped by the Panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranslationTarget {
    En,
    Zh,
}

/// An explicit request to translate one already-visible block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThoughtTranslationRequest {
    pub block: ThoughtBlockRef,
    pub engine: TranslationEngine,
    pub target: TranslationTarget,
    pub expected_destination: String,
    pub request_id: String,
}

/// Destinations the connected caller can explicitly approve per block.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThoughtTranslationEngines {
    pub machine: Option<String>,
    pub ai: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThoughtTranslationCancel {
    pub request_id: String,
}

pub const THOUGHT_TRANSLATION_TOPIC: &str = "thought.translation.update";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThoughtTranslationEvent {
    pub request_id: String,
    pub update: ThoughtTranslationUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThoughtTranslationUpdate {
    Delta { text: String },
    Completed { text: String },
    Failed { code: ThoughtTranslationFailure },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThoughtTranslationFailure {
    Unavailable,
    Denied,
    Cancelled,
    Timeout,
    TooLarge,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thought_reference_round_trips_without_source_text() {
        let reference = ThoughtBlockRef {
            session_key: "session".into(),
            run_id: "run".into(),
            block_id: ThoughtBlockId("opaque-block".into()),
        };
        let json = serde_json::to_value(&reference).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"session_key":"session", "run_id":"run", "block_id":"opaque-block"})
        );
        assert_eq!(reference, serde_json::from_value(json).unwrap());
    }

    #[test]
    fn translation_options_use_closed_wire_values() {
        for (engine, wire) in [
            (TranslationEngine::Machine, "machine"),
            (TranslationEngine::Ai, "ai"),
        ] {
            assert_eq!(serde_json::to_value(engine).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<TranslationEngine>(wire.into()).unwrap(),
                engine
            );
        }
        for (target, wire) in [(TranslationTarget::En, "en"), (TranslationTarget::Zh, "zh")] {
            assert_eq!(serde_json::to_value(target).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<TranslationTarget>(wire.into()).unwrap(),
                target
            );
        }
        assert!(serde_json::from_str::<TranslationEngine>("\"other\"").is_err());
        assert!(serde_json::from_str::<TranslationTarget>("\"fr\"").is_err());
    }
}
