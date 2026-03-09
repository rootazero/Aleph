//! Intent detection result types.
//!
//! These types represent the output of the intent detection pipeline,
//! replacing the older `ExecutionIntent` / `DecisionResult` types.

use serde::{Deserialize, Serialize};

/// Where a direct-tool invocation originated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectToolSource {
    SlashCommand,
    Skill,
    Mcp,
    Custom,
}

/// Which detection layer resolved the intent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionLayer {
    /// L0 — system-level / bypass
    L0,
    /// L1 — exact slash-command match
    L1,
    /// L2 — keyword / regex heuristics
    L2,
    /// L3 — lightweight classifier
    L3,
    /// L4 — fallback default (conversation)
    #[default]
    L4Default,
}

/// Metadata attached to an `Execute` intent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExecuteMetadata {
    pub detected_path: Option<String>,
    pub detected_url: Option<String>,
    pub context_hint: Option<String>,
    pub keyword_tag: Option<String>,
    pub layer: DetectionLayer,
}

impl ExecuteMetadata {
    /// Create metadata with only the layer set; all other fields `None`.
    pub fn default_with_layer(layer: DetectionLayer) -> Self {
        Self {
            layer,
            ..Default::default()
        }
    }
}

/// The resolved intent for a user message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IntentResult {
    /// A specific tool was identified directly (slash command, skill, MCP).
    DirectTool {
        tool_id: String,
        args: Option<String>,
        source: DirectToolSource,
    },
    /// The message should be executed (task / tool use).
    Execute {
        confidence: f32,
        metadata: ExecuteMetadata,
    },
    /// The message is conversational — no tool needed.
    Converse {
        confidence: f32,
    },
    /// The request was aborted (e.g. safety filter).
    Abort,
}

impl IntentResult {
    pub fn is_direct_tool(&self) -> bool {
        matches!(self, Self::DirectTool { .. })
    }

    pub fn is_execute(&self) -> bool {
        matches!(self, Self::Execute { .. })
    }

    pub fn is_converse(&self) -> bool {
        matches!(self, Self::Converse { .. })
    }

    pub fn is_abort(&self) -> bool {
        matches!(self, Self::Abort)
    }

    /// Return the confidence score. `DirectTool` and `Abort` are always 1.0.
    pub fn confidence(&self) -> f32 {
        match self {
            Self::DirectTool { .. } | Self::Abort => 1.0,
            Self::Execute { confidence, .. } | Self::Converse { confidence, .. } => *confidence,
        }
    }

    /// Return the detection layer, if available.
    pub fn layer(&self) -> Option<DetectionLayer> {
        match self {
            Self::Execute { metadata, .. } => Some(metadata.layer),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_result_direct_tool() {
        let result = IntentResult::DirectTool {
            tool_id: "read_file".to_string(),
            args: Some("path=/tmp".to_string()),
            source: DirectToolSource::SlashCommand,
        };
        assert!(result.is_direct_tool());
        assert!(!result.is_execute());
        assert!(!result.is_converse());
        assert!(!result.is_abort());
    }

    #[test]
    fn intent_result_execute_with_metadata() {
        let meta = ExecuteMetadata {
            detected_path: Some("/tmp/file.txt".to_string()),
            keyword_tag: Some("file-op".to_string()),
            layer: DetectionLayer::L2,
            ..Default::default()
        };
        let result = IntentResult::Execute {
            confidence: 0.85,
            metadata: meta,
        };
        assert!(result.is_execute());
        assert!((result.confidence() - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn intent_result_converse() {
        let result = IntentResult::Converse { confidence: 0.92 };
        assert!(result.is_converse());
        assert!((result.confidence() - 0.92).abs() < f32::EPSILON);
    }

    #[test]
    fn intent_result_abort() {
        let result = IntentResult::Abort;
        assert!(result.is_abort());
        assert!((result.confidence() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn execute_metadata_default() {
        let meta = ExecuteMetadata::default_with_layer(DetectionLayer::L3);
        assert_eq!(meta.layer, DetectionLayer::L3);
        assert!(meta.detected_path.is_none());
        assert!(meta.detected_url.is_none());
        assert!(meta.context_hint.is_none());
        assert!(meta.keyword_tag.is_none());
    }
}
