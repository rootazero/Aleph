//! Public types for the `MemoryReflector` synthesis layer.

use crate::memory::namespace::NamespaceScope;
use serde::{Deserialize, Serialize};

/// A synthesised answer derived from the memory store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Synthesis {
    /// Natural-language answer composed by the LLM from retrieved notes.
    pub text: String,
    /// Notes the LLM cited. Titles are code-overlaid; LLM only emits path + relevance.
    pub sources: Vec<NoteRef>,
}

/// A single cited note.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteRef {
    /// `category/filename` path, e.g. `"reference/rust-ownership"`.
    pub path: String,
    /// Human-readable title, resolved from the note store (never from LLM).
    pub title: String,
    /// Rerank score carried over from the `HybridAssembler` packet, 0.0–1.0.
    pub relevance: f32,
}

/// Options passed to `MemoryReflector::reflect`. Tool-path callers fill this
/// from the current agent context; internal callers may set more fields.
#[derive(Debug, Clone)]
pub struct ReflectOpts {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    /// Optional token budget override for assembler.
    pub max_tokens: Option<usize>,
    /// Optional session id — threaded into `recall_signals`.
    pub session_id: Option<String>,
}

impl ReflectOpts {
    pub fn for_agent(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            namespace: NamespaceScope::Owner,
            max_tokens: None,
            session_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesis_round_trips_json() {
        let s = Synthesis {
            text: "Rust uses ownership.".into(),
            sources: vec![NoteRef {
                path: "reference/rust-ownership".into(),
                title: "Rust Ownership".into(),
                relevance: 0.91,
            }],
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: Synthesis = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn reflect_opts_for_agent_defaults_to_owner() {
        let o = ReflectOpts::for_agent("a1");
        assert_eq!(o.agent_id, "a1");
        assert!(matches!(o.namespace, NamespaceScope::Owner));
        assert!(o.max_tokens.is_none());
        assert!(o.session_id.is_none());
    }
}
