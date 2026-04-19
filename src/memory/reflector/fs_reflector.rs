//! MemoryReflector — orchestrates HybridAssembler + LLM synthesis.

use crate::error::AlephError;
use crate::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
use crate::memory::reflector::packet_adapter::{envelope_to_synthesis_context, SynthesisContext};
use crate::memory::reflector::prompts::PROMPT_SYNTHESIS;
use crate::memory::reflector::recall_signals::{record_reflect_signals, SignalRow};
use crate::memory::reflector::types::{NoteRef, ReflectOpts, Synthesis};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::utils::json_extract::extract_json_robust;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use tracing::warn;

/// Default token budget when the caller does not specify one.
const DEFAULT_BUDGET_TOKENS: u32 = 4096;

/// Async callable that takes a [`SignalRow`] and persists it. The reflector
/// stays decoupled from concrete storage via this type.
pub type RecallWriter = Arc<
    dyn Fn(SignalRow) -> Pin<Box<dyn Future<Output = Result<(), AlephError>> + Send>> + Send + Sync,
>;

// ---------------------------------------------------------------------------
// LLM response types (internal only)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LlmSourceRef {
    path: String,
    #[serde(default)]
    relevance: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct LlmSynthesis {
    #[serde(default)]
    text: String,
    #[serde(default)]
    sources: Vec<LlmSourceRef>,
}

// ---------------------------------------------------------------------------
// MemoryReflector
// ---------------------------------------------------------------------------

/// Orchestrates [`WorkingMemoryAssembler`] retrieval followed by LLM synthesis.
///
/// # Short-circuit
/// When the assembler returns an envelope with zero items across all slots,
/// `reflect` returns immediately with a stub `Synthesis` and never calls the LLM.
pub struct MemoryReflector {
    assembler: Arc<dyn WorkingMemoryAssembler>,
    provider: Arc<dyn AiProvider>,
    recall_writer: RecallWriter,
}

impl MemoryReflector {
    pub fn new(
        assembler: Arc<dyn WorkingMemoryAssembler>,
        provider: Arc<dyn AiProvider>,
        recall_writer: RecallWriter,
    ) -> Self {
        Self {
            assembler,
            provider,
            recall_writer,
        }
    }

    /// Retrieve memories relevant to `query` and synthesise a natural-language answer.
    ///
    /// Returns immediately with a stub when no relevant memories are found (empty-packet
    /// short-circuit). For non-empty envelopes the full LLM synthesis path runs.
    pub async fn reflect(&self, query: &str, opts: ReflectOpts) -> Result<Synthesis, AlephError> {
        let budget = AssemblyBudget {
            total_tokens: opts
                .max_tokens
                .map(|n| n as u32)
                .unwrap_or(DEFAULT_BUDGET_TOKENS),
        };

        let envelope = self
            .assembler
            .assemble(query, &opts.agent_id, opts.session_id.as_deref(), budget)
            .await?;

        // Check emptiness: all slots must have zero items.
        let is_empty = envelope.slots.iter().all(|slot| slot.items.is_empty());

        if is_empty {
            return Ok(Synthesis {
                text: "No relevant memories found.".to_string(),
                sources: Vec::new(),
            });
        }

        let ctx = envelope_to_synthesis_context(&envelope);
        let synthesis = Self::synthesise_from_context(&ctx, &self.provider).await?;

        // Side effect: log one recall_signal per note in the context.
        // Errors here must NOT fail the reflect() call — logging is best-effort.
        let writer = self.recall_writer.clone();
        let record = move |row: SignalRow| {
            let writer = writer.clone();
            async move { writer(row).await }
        };
        if let Err(e) = record_reflect_signals(record, query, &ctx.note_lookup, &opts).await {
            tracing::warn!("reflector: failed to write recall_signals: {e}");
        }

        Ok(synthesis)
    }

    /// Pure LLM-facing synthesis step — isolated for unit testing.
    ///
    /// Calls the LLM with [`PROMPT_SYNTHESIS`] as system prompt and the
    /// synthesis-context `user_prompt` as user message, parses the JSON
    /// response, overlays canonical titles from `note_lookup` (dropping any
    /// path the LLM fabricated), and degrades gracefully to a text-only
    /// `Synthesis` when JSON parsing fails.
    pub(crate) async fn synthesise_from_context(
        ctx: &SynthesisContext,
        provider: &Arc<dyn AiProvider>,
    ) -> Result<Synthesis, AlephError> {
        let msgs = [UnifiedMessage::user(&ctx.user_prompt)];
        let response = provider
            .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_SYNTHESIS)))
            .await
            .map_err(|e| AlephError::other(format!("Reflect LLM call failed: {e}")))?;
        let text = response.text_content();

        // Parse JSON → LlmSynthesis; on failure fall back to text-only.
        let parsed: Option<LlmSynthesis> =
            extract_json_robust(&text).and_then(|v| serde_json::from_value::<LlmSynthesis>(v).ok());

        let Some(llm) = parsed else {
            warn!("reflector: LLM response was not parseable JSON; returning text-only synthesis");
            return Ok(Synthesis {
                text: text.trim().to_string(),
                sources: Vec::new(),
            });
        };

        // Overlay canonical titles; drop any path the LLM fabricated.
        let sources: Vec<NoteRef> = llm
            .sources
            .into_iter()
            .filter_map(|s| {
                ctx.note_lookup.get(&s.path).map(|meta| NoteRef {
                    path: s.path,
                    title: meta.title.clone(),
                    relevance: s.relevance.unwrap_or(meta.relevance),
                })
            })
            .collect();

        Ok(Synthesis {
            text: llm.text,
            sources,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflector::types::Synthesis;

    /// Verifies the stub shape that the empty-packet short-circuit returns.
    /// The assembler + provider are not exercised here; the real short-circuit
    /// path end-to-end is covered by the Task 9 integration test.
    #[test]
    fn empty_stub_shape_matches_spec() {
        let stub = Synthesis {
            text: "No relevant memories found.".to_string(),
            sources: Vec::new(),
        };
        assert_eq!(stub.text, "No relevant memories found.");
        assert!(stub.sources.is_empty());
    }
}

#[cfg(test)]
mod llm_path_tests {
    use super::*;
    use crate::memory::reflector::packet_adapter::{NoteMeta, SynthesisContext};
    use crate::providers::recording_mock::RecordingMockProvider;
    use crate::sync_primitives::Arc;
    use std::collections::HashMap;

    fn ctx_with_lookup(entries: &[(&str, &str, f32)]) -> SynthesisContext {
        let mut lookup = HashMap::new();
        for (path, title, rel) in entries {
            lookup.insert(
                path.to_string(),
                NoteMeta {
                    title: title.to_string(),
                    relevance: *rel,
                },
            );
        }
        SynthesisContext {
            user_prompt: "QUESTION: q".into(),
            note_lookup: lookup,
        }
    }

    #[tokio::test]
    async fn synthesise_parses_llm_json_and_overlays_titles() {
        let canned = r#"{"text":"Rust enforces ownership.","sources":[{"path":"wiki/rust","relevance":0.91}]}"#;
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(RecordingMockProvider::new(canned.to_string()));
        let ctx = ctx_with_lookup(&[("wiki/rust", "Rust Lang", 0.91)]);

        let synthesis = MemoryReflector::synthesise_from_context(&ctx, &provider)
            .await
            .unwrap();

        assert_eq!(synthesis.text, "Rust enforces ownership.");
        assert_eq!(synthesis.sources.len(), 1);
        assert_eq!(synthesis.sources[0].path, "wiki/rust");
        assert_eq!(
            synthesis.sources[0].title, "Rust Lang",
            "title must be overlaid from lookup, not from LLM"
        );
        assert!((synthesis.sources[0].relevance - 0.91).abs() < 1e-6);
    }

    #[tokio::test]
    async fn unknown_path_from_llm_is_dropped() {
        let canned = r#"{"text":"x","sources":[{"path":"wiki/rust","relevance":0.5},{"path":"wiki/fake","relevance":0.99}]}"#;
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(RecordingMockProvider::new(canned.to_string()));
        let ctx = ctx_with_lookup(&[("wiki/rust", "Rust", 0.5)]);

        let synthesis = MemoryReflector::synthesise_from_context(&ctx, &provider)
            .await
            .unwrap();

        assert_eq!(synthesis.sources.len(), 1, "fake path must be dropped");
        assert_eq!(synthesis.sources[0].path, "wiki/rust");
    }

    #[tokio::test]
    async fn malformed_json_falls_back_to_text_only() {
        let canned = "The notes don't cover that topic.".to_string();
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(RecordingMockProvider::new(canned));
        let ctx = ctx_with_lookup(&[]);

        let synthesis = MemoryReflector::synthesise_from_context(&ctx, &provider)
            .await
            .unwrap();
        assert!(synthesis.text.contains("don't cover"));
        assert!(synthesis.sources.is_empty());
    }
}
