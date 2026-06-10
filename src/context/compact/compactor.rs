//! LLM-based context compaction module.
//!
//! Replaces old conversation history with concise summaries via a side-channel
//! LLM call. Falls back to deterministic truncation when the LLM call fails.

use std::time::Duration;

use super::summary_utils::{build_window_summary_prompt, latest_user_task, strip_analysis_block};
use crate::memory::session_compactor::summary_source::SessionSummarySource;
use crate::memory::store::MemoryBackend;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::{Arc, Mutex};

/// Strategy used during compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactStrategy {
    /// Successfully summarized via a side-channel LLM call.
    LlmSummary,
    /// LLM call failed; fell back to keeping only the first line of each message.
    DeterministicTruncation,
    /// Reused existing session summaries — zero API cost.
    SessionMemoryReuse,
    /// Reapplied this run's cached compaction over an unchanged window
    /// fingerprint — zero API cost.
    CacheReuse,
    /// Compaction was skipped entirely.
    Skipped { reason: String },
}

/// Result of a compaction attempt.
#[derive(Debug, Clone)]
pub struct CompactResult {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub strategy_used: CompactStrategy,
}

/// Configuration for [`ContextCompactor`].
#[derive(Debug, Clone)]
pub struct CompactorConfig {
    /// Number of recent messages to keep untouched (default: 6).
    pub fresh_tail: usize,
    /// Target compression ratio (default: 0.25).
    pub target_ratio: f32,
    /// Maximum messages in the compression window (default: 40).
    pub max_window: usize,
    /// Timeout for the side-channel LLM call (default: 15 s).
    pub timeout: Duration,
    /// Whether to fall back to deterministic truncation on LLM failure (default: true).
    pub fallback_to_truncation: bool,
}

impl Default for CompactorConfig {
    fn default() -> Self {
        Self {
            fresh_tail: 6,
            target_ratio: 0.25,
            max_window: 40,
            timeout: Duration::from_secs(15),
            fallback_to_truncation: true,
        }
    }
}

/// Wiring for the zero-API-cost session-summary reuse path: the memory backend
/// holding the d0/d1/d2 summaries plus the agent id they were written under.
/// Both are required together — `get_raw_by_path_prefix` filters by agent id.
struct SummaryReuse {
    backend: MemoryBackend,
    agent_id: String,
}

/// Cached result of the last successful compaction, expressed in coordinates
/// of the *rebuilt* (uncompacted) message list: `[start, end)` is the covered
/// range, `hash` fingerprints the covered messages, and `summary` is the full
/// `[Context Summary]…` text that replaces them.
#[derive(Clone)]
struct CompactionCache {
    start: usize,
    end: usize,
    hash: u64,
    summary: String,
}

/// Un-summarized pre-tail growth beyond the cached summary that triggers an
/// incremental LLM merge instead of a pure cache reapply. Below both
/// thresholds the gap rides along uncompacted (it is recent, small, and will
/// be folded into the summary once it crosses either bound).
const CACHE_EXTEND_MIN_MESSAGES: usize = 8;
const CACHE_EXTEND_MIN_TOKENS: usize = 4096;

/// LLM-based context compactor.
///
/// Compresses older conversation history into a concise summary, keeping
/// recent messages intact. Uses a side-channel LLM call for summarization
/// and falls back to deterministic truncation when the call fails.
pub struct ContextCompactor {
    provider: Arc<dyn AiProvider>,
    config: CompactorConfig,
    /// When set, `compact()` first tries to reuse the hierarchical session
    /// summaries written by `SessionCompactor` (zero API cost) before falling
    /// back to a side-channel LLM call.
    summary_reuse: Option<SummaryReuse>,
    /// Cheap-tier provider override (Reasonix parity — `summaryModel = "deepseek-v4-flash"`).
    /// When set, summarization calls go to this provider instead of `provider`.
    /// `None` (default) preserves legacy behavior of reusing the main LLM.
    /// Summarization is read-and-condense work where the strongest model is
    /// almost never required; routing it to a flash-tier provider yields a
    /// 10–20× per-token cost reduction without measurable quality regression.
    cheap_provider: Option<Arc<dyn AiProvider>>,
    /// Fingerprint cache of the last successful compaction (openteams
    /// compression-cache parity). The harness rebuilds the message list from
    /// the session log every turn, discarding the previous turn's in-place
    /// compaction — without this cache a high-pressure run pays a fresh
    /// side-channel summarization call for essentially the same window on
    /// every Think turn, and the changing summary text thrashes the provider
    /// prompt cache. Validated by content hash, so any prefix change (e.g.
    /// preflight passes pruning differently) is a miss that falls through to
    /// a full recompaction.
    cache: Mutex<Option<CompactionCache>>,
}

impl ContextCompactor {
    /// Create a new compactor with the given provider and configuration.
    pub fn new(provider: Arc<dyn AiProvider>, config: CompactorConfig) -> Self {
        Self {
            provider,
            config,
            summary_reuse: None,
            cheap_provider: None,
            cache: Mutex::new(None),
        }
    }

    /// Enable the zero-API-cost session-summary reuse path. `backend` holds the
    /// d0/d1/d2 facts; `agent_id` is the owning agent they were written under.
    pub fn with_summary_reuse(
        mut self,
        backend: MemoryBackend,
        agent_id: impl Into<String>,
    ) -> Self {
        self.summary_reuse = Some(SummaryReuse {
            backend,
            agent_id: agent_id.into(),
        });
        self
    }

    /// Route summarization calls to a cheap-tier provider (Reasonix parity —
    /// `summaryModel = "deepseek-v4-flash"`). `None` clears the override.
    pub fn with_cheap_provider(mut self, cheap: Option<Arc<dyn AiProvider>>) -> Self {
        self.cheap_provider = cheap;
        self
    }

    /// Provider used for summarization — cheap-tier override if set, otherwise
    /// the main provider passed to `new()`. Internal accessor.
    fn summarizer(&self) -> &Arc<dyn AiProvider> {
        self.cheap_provider.as_ref().unwrap_or(&self.provider)
    }

    /// Compact older messages in the conversation history.
    ///
    /// The `fresh_tail` parameter overrides `config.fresh_tail` when larger.
    ///
    /// # Flow
    ///
    /// 1. Determine the compression window (everything before the fresh tail).
    /// 2. Skip if the window is too small or already compacted.
    /// 3. Serialize the window into a transcript and estimate tokens.
    /// 4. Call the LLM with a summarization prompt (with timeout).
    /// 5. On success: replace the window with a single summary message.
    /// 6. On failure + fallback enabled: deterministic truncation.
    /// 7. On failure + fallback disabled: skip.
    ///
    /// `session_id` enables the zero-API-cost session-summary reuse path when a
    /// memory backend is also wired (see [`ContextCompactor::with_summary_reuse`]).
    pub async fn compact(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        fresh_tail: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<CompactResult> {
        let effective_tail = fresh_tail.max(self.config.fresh_tail);

        // Step 1: determine compression window
        if messages.len() <= effective_tail {
            return Ok(CompactResult {
                tokens_before: 0,
                tokens_after: 0,
                strategy_used: CompactStrategy::Skipped {
                    reason: "not enough messages to compact".into(),
                },
            });
        }

        // Snap the tail boundary forward past any tool-result run so the kept
        // fresh tail never *starts* with a `ToolResult` whose `ToolCall` we are
        // about to drain into the summary. Without this the boundary can land
        // mid-pair, orphaning the result and getting the next provider call
        // rejected (Anthropic: `tool_result` without a preceding `tool_use`).
        let cut_end = snap_boundary_forward(messages.as_slice(), messages.len() - effective_tail);
        if cut_end <= 1 {
            return Ok(CompactResult {
                tokens_before: 0,
                tokens_after: 0,
                strategy_used: CompactStrategy::Skipped {
                    reason: "compression window too small".into(),
                },
            });
        }

        // Step 2: idempotency check — skip if already compacted and window is small
        if let Some(first_text) = first_message_text(&messages[0]) {
            if first_text.starts_with("[Context Summary]") && cut_end <= 2 {
                return Ok(CompactResult {
                    tokens_before: 0,
                    tokens_after: 0,
                    strategy_used: CompactStrategy::Skipped {
                        reason: "already compacted with small window".into(),
                    },
                });
            }
        }

        // Step 3: limit window and serialize. Snap the head boundary forward too:
        // the summary is inserted at `window_start`, so if that index sits on a
        // `ToolResult` whose `ToolCall` stays in the kept head, draining the
        // result would orphan the call. Advancing past the result keeps the pair
        // intact in the head.
        let window_start = snap_boundary_forward(
            messages.as_slice(),
            cut_end.saturating_sub(self.config.max_window),
        );

        // Guard: a zero-width window means there is nothing to compress.
        if window_start >= cut_end {
            return Ok(CompactResult {
                tokens_before: 0,
                tokens_after: 0,
                strategy_used: CompactStrategy::Skipped {
                    reason: "compression window is empty (max_window may be zero)".into(),
                },
            });
        }

        // Fingerprint-cache fast path (openteams compression-cache parity).
        // The harness rebuilds `messages` from the session log every turn, so
        // the previous turn's in-place compaction is gone by the time we run
        // again. If the last compaction's covered range still hashes to the
        // same fingerprint in this rebuild, reapply the cached summary with
        // zero API cost. When the un-summarized gap behind the summary has
        // grown past the extension threshold, run one LLM merge over
        // [summary + gap] — the transcript carries the old summary, so the
        // new one absorbs it (openclaw "merge prior summaries") — and refresh
        // the cache to cover the wider range.
        let cached = self.cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(c) = cached {
            let fits = c.start < c.end && c.end <= cut_end;
            if fits && hash_window(&messages[c.start..c.end]) == c.hash {
                return self.reapply_cached(messages, c, cut_end).await;
            }
            // Stale fingerprint (prefix changed under a preflight pass, or the
            // window shrank): drop the entry and fall through to a full
            // recompaction, which refreshes the cache.
            *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }

        // Fingerprint of the window in rebuilt coordinates, captured before
        // any mutation below. Every success path stores it so the next turn's
        // rebuilt prompt hits the cache fast path above instead of paying the
        // side-channel LLM call again.
        let window_hash = hash_window(&messages[window_start..cut_end]);

        // Fast path: reuse pre-existing hierarchical session summaries (zero
        // API cost). Active only when summary reuse is wired and the caller
        // supplied a session id; otherwise fall through to the LLM path.
        if let (Some(reuse), Some(sid)) = (self.summary_reuse.as_ref(), session_id) {
            let source =
                SessionSummarySource::new(reuse.backend.clone(), sid, reuse.agent_id.clone());
            if let Some(reuse_result) = source.try_reuse(messages, window_start, cut_end).await {
                if let Some(text) = first_message_text(&messages[window_start]) {
                    self.store_cache(window_start, cut_end, window_hash, text.to_string());
                }
                tracing::info!(
                    tokens_before = reuse_result.tokens_before,
                    tokens_after = reuse_result.tokens_after,
                    "Compaction via session memory reuse (zero API cost)"
                );
                return Ok(reuse_result);
            }
        }

        let window = &messages[window_start..cut_end];
        let transcript = serialize_transcript(window);
        let tokens_before = estimate_tokens(&transcript);

        // Step 4: build prompt with token budget, anchored to the live task.
        // The user's current request lives in the fresh tail we are about to
        // keep (`messages[cut_end..]`); deriving the focus from it here means
        // task-anchoring needs no extra plumbing from the harness — the live
        // task is already in the message list the compactor owns. Biasing the
        // summary toward the active task is the convergent gap vs hermes
        // ("Active Task") / openclaw ("last thing the user requested").
        let token_budget = (tokens_before as f32 * self.config.target_ratio) as usize;
        let focus = latest_user_task(&messages[cut_end..]);
        let prompt = build_window_summary_prompt(&transcript, token_budget, focus.as_deref());

        // Step 5–7: attempt LLM call with timeout. The emptiness check runs on
        // the *stripped* output, not the raw LLM text: a model (especially the
        // flash-tier cheap provider) can emit only an <analysis> scratchpad with
        // no <summary> block, leaving an empty string after stripping. Checking
        // the raw text would treat that as a successful summary and drain the
        // entire window into an empty "[Context Summary]" — permanent context
        // loss reported as a successful LlmSummary. Stripping first routes the
        // degenerate case to the deterministic-truncation fallback below.
        let llm_result = tokio::time::timeout(self.config.timeout, self.call_llm(&prompt)).await;
        let summary = match llm_result {
            Ok(Ok(raw)) => {
                let stripped = strip_analysis_block(&raw);
                (!stripped.trim().is_empty()).then_some(stripped)
            }
            Ok(Err(_)) | Err(_) => None,
        };

        match summary {
            Some(summary) => {
                // Success: drain old window and insert the stripped summary.
                let summary_text = format!("[Context Summary]\n{}", summary);
                let summary_msg = UnifiedMessage::user(summary_text.clone());
                let tokens_after = estimate_tokens(&summary);

                // Remove the compressed window and insert the summary at window_start
                messages.drain(window_start..cut_end);
                messages.insert(window_start, summary_msg);
                self.store_cache(window_start, cut_end, window_hash, summary_text);

                Ok(CompactResult {
                    tokens_before,
                    tokens_after,
                    strategy_used: CompactStrategy::LlmSummary,
                })
            }
            None => {
                // LLM failed or produced no usable summary — try fallback
                if self.config.fallback_to_truncation {
                    let truncated = deterministic_truncation(window);
                    let tokens_after = estimate_tokens(&truncated);
                    let summary_text = format!("[Context Summary]\n{}", truncated);
                    let summary_msg = UnifiedMessage::user(summary_text.clone());

                    messages.drain(window_start..cut_end);
                    messages.insert(window_start, summary_msg);
                    self.store_cache(window_start, cut_end, window_hash, summary_text);

                    Ok(CompactResult {
                        tokens_before,
                        tokens_after,
                        strategy_used: CompactStrategy::DeterministicTruncation,
                    })
                } else {
                    Ok(CompactResult {
                        tokens_before,
                        tokens_after: tokens_before,
                        strategy_used: CompactStrategy::Skipped {
                            reason: "LLM call failed and fallback disabled".into(),
                        },
                    })
                }
            }
        }
    }

    /// Store a fresh cache entry covering `[start, end)` of the rebuilt
    /// message list. `summary` is the full `[Context Summary]…` text.
    fn store_cache(&self, start: usize, end: usize, hash: u64, summary: String) {
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(CompactionCache {
            start,
            end,
            hash,
            summary,
        });
    }

    /// Reapply a validated cache entry to the rebuilt message list, extending
    /// it with one LLM merge when the un-summarized gap behind the summary has
    /// grown past the extension threshold.
    ///
    /// Precondition (checked by the caller): `c.start < c.end <= cut_end` and
    /// `hash_window(&messages[c.start..c.end]) == c.hash`.
    async fn reapply_cached(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        c: CompactionCache,
        cut_end: usize,
    ) -> anyhow::Result<CompactResult> {
        // Hash over the wider range BEFORE mutating: this is the fingerprint a
        // future rebuild will present for the extended cover.
        let extended_hash = hash_window(&messages[c.start..cut_end]);
        let replaced = c.end - c.start;
        let window_text: String = messages[c.start..cut_end]
            .iter()
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");
        let tokens_before = estimate_tokens(&window_text);

        messages.drain(c.start..c.end);
        messages.insert(c.start, UnifiedMessage::user(c.summary.clone()));

        // Mutated coordinates: the gap between the reapplied summary and the
        // fresh tail.
        let cut_end_m = cut_end - replaced + 1;
        let gap_msgs = cut_end_m - (c.start + 1);
        let gap_text: String = messages[c.start + 1..cut_end_m]
            .iter()
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");
        let gap_tokens = estimate_tokens(&gap_text);

        if gap_msgs < CACHE_EXTEND_MIN_MESSAGES && gap_tokens < CACHE_EXTEND_MIN_TOKENS {
            return Ok(CompactResult {
                tokens_before,
                tokens_after: estimate_tokens(&c.summary) + gap_tokens,
                strategy_used: CompactStrategy::CacheReuse,
            });
        }

        // Extension merge: one LLM call over [cached summary + gap]; the old
        // summary is part of the transcript, so the merged summary absorbs it.
        // Deterministic truncation mirrors the main path's failure handling.
        // The merge window is small (1 summary + gap), so no max_window re-cap
        // is needed here.
        let merge_window = &messages[c.start..cut_end_m];
        let transcript = serialize_transcript(merge_window);
        let merge_tokens = estimate_tokens(&transcript);
        let token_budget = (merge_tokens as f32 * self.config.target_ratio) as usize;
        let focus = latest_user_task(&messages[cut_end_m..]);
        let prompt = build_window_summary_prompt(&transcript, token_budget, focus.as_deref());

        let llm_result = tokio::time::timeout(self.config.timeout, self.call_llm(&prompt)).await;
        let merged = match llm_result {
            Ok(Ok(raw)) => {
                let stripped = strip_analysis_block(&raw);
                (!stripped.trim().is_empty()).then_some(stripped)
            }
            Ok(Err(_)) | Err(_) => None,
        };
        let (body, strategy) = match merged {
            Some(s) => (s, CompactStrategy::LlmSummary),
            None if self.config.fallback_to_truncation => (
                deterministic_truncation(merge_window),
                CompactStrategy::DeterministicTruncation,
            ),
            None => {
                // Merge failed and truncation is disabled: keep the reapplied
                // summary + raw gap. The cache stays on its old (still valid)
                // cover, so the next turn retries the merge.
                return Ok(CompactResult {
                    tokens_before,
                    tokens_after: estimate_tokens(&c.summary) + gap_tokens,
                    strategy_used: CompactStrategy::CacheReuse,
                });
            }
        };

        let summary_text = format!("[Context Summary]\n{}", body);
        let tokens_after = estimate_tokens(&summary_text);
        messages.drain(c.start..cut_end_m);
        messages.insert(c.start, UnifiedMessage::user(summary_text.clone()));
        self.store_cache(c.start, cut_end, extended_hash, summary_text);

        Ok(CompactResult {
            tokens_before,
            tokens_after,
            strategy_used: strategy,
        })
    }

    /// Summarize a slice of messages and return the raw summary string.
    ///
    /// Used by `session_split::summarize_pretail` to produce the seed text for
    /// a child session without running a full `compact()` in-place.  Falls back
    /// to deterministic truncation when the LLM call fails (mirrors `compact`).
    ///
    /// `focus` is the user's active task (the most recent request preserved
    /// verbatim in the child's fresh tail). Passing it anchors the pre-tail
    /// summary to the live work — the heavy-compaction path where losing the
    /// task thread hurts most. `None` keeps the historical static prompt.
    pub(crate) async fn summarize_slice(
        &self,
        messages: &[UnifiedMessage],
        focus: Option<&str>,
    ) -> anyhow::Result<String> {
        if messages.is_empty() {
            return Ok(String::new());
        }

        let transcript = serialize_transcript(messages);
        let tokens_before = estimate_tokens(&transcript);
        let token_budget = (tokens_before as f32 * self.config.target_ratio) as usize;

        let prompt = build_window_summary_prompt(&transcript, token_budget, focus);

        let llm_result = tokio::time::timeout(self.config.timeout, self.call_llm(&prompt)).await;

        // Strip before the emptiness check: an analysis-only response (no
        // <summary> block) strips to an empty string, which must fall back to
        // deterministic truncation rather than seed a child session with "".
        let stripped = match llm_result {
            Ok(Ok(raw)) => {
                let s = strip_analysis_block(&raw);
                (!s.trim().is_empty()).then_some(s)
            }
            _ => None,
        };
        Ok(stripped.unwrap_or_else(|| deterministic_truncation(messages)))
    }

    /// Side-channel LLM call for summarization. Routes to the cheap-tier
    /// provider when one is configured (Reasonix parity), otherwise reuses
    /// the main provider.
    async fn call_llm(&self, prompt: &str) -> anyhow::Result<String> {
        let msgs = [UnifiedMessage::user(prompt)];
        let system =
            "You are a precise conversation summarizer. Output the analysis block followed by the summary block. No other text.";
        let payload = RequestPayload::new(&msgs).with_system(Some(system));
        let response: ProviderResponse = self.summarizer().process(payload).await?;
        Ok(response.text.unwrap_or_default())
    }
}

// === Helper functions ===

/// Advance a proposed cut index forward past any contiguous run of `ToolResult`
/// messages so the cut never falls *between* a `ToolCall` and the result that
/// answers it. Tool results immediately follow their call, so the only mid-pair
/// position is one where `messages[idx]` is a `ToolResult`; skipping the whole
/// run lands the boundary on a clean message (or at `messages.len()`).
///
/// Used for both compaction boundaries (`window_start`, `cut_end`) so the
/// compactor preserves the call/result pairing invariant at the source, rather
/// than relying solely on the wire-level repair in
/// [`crate::providers::message::normalize_tool_pairs`].
fn snap_boundary_forward(messages: &[UnifiedMessage], idx: usize) -> usize {
    let mut i = idx;
    while i < messages.len() && matches!(messages[i], UnifiedMessage::ToolResult { .. }) {
        i += 1;
    }
    i
}

/// Content fingerprint of a message window: role discriminant + text content
/// per message. Deterministic across turns because the prompt builder and the
/// preflight cheap passes are deterministic functions of an append-only
/// session log — when a pass *does* change an old message (e.g. a new file op
/// supersedes an earlier result), the hash misses and the compactor falls
/// back to a full recompaction.
fn hash_window(messages: &[UnifiedMessage]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in messages {
        std::mem::discriminant(m).hash(&mut h);
        m.text_content().hash(&mut h);
    }
    h.finish()
}

/// Extract the text content of the first content block in a message (if Text).
fn first_message_text(msg: &UnifiedMessage) -> Option<&str> {
    msg.content_blocks().first().and_then(|b| b.as_text())
}

/// Serialize a slice of messages into a human-readable transcript.
fn serialize_transcript(messages: &[UnifiedMessage]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                lines.push(format!(
                    "tool_result({}): {}",
                    tool_name,
                    msg.text_content()
                ));
                continue;
            }
        };
        lines.push(format!("{}: {}", role, msg.text_content()));
    }
    lines.join("\n")
}

/// Estimate token count using content-aware ratio detection.
fn estimate_tokens(text: &str) -> usize {
    let ratio = crate::context::budget::pressure::detect_content_ratio(text);
    let char_count = text.chars().count();
    (char_count as f64 / ratio).ceil() as usize
}

/// Deterministic truncation: keep only the first line of each message.
fn deterministic_truncation(messages: &[UnifiedMessage]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                let text = msg.text_content();
                let first_line = text.lines().next().unwrap_or("");
                lines.push(format!("tool_result({}): {}", tool_name, first_line));
                continue;
            }
        };
        let text = msg.text_content();
        let first_line = text.lines().next().unwrap_or("");
        lines.push(format!("{}: {}", role, first_line));
    }
    lines.join("\n")
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::ContentBlock;
    use crate::providers::mock::MockProvider;
    use crate::providers::MockError;

    fn make_messages(count: usize) -> Vec<UnifiedMessage> {
        let mut msgs = Vec::with_capacity(count);
        for i in 0..count {
            if i % 2 == 0 {
                msgs.push(UnifiedMessage::user(format!("User message {}", i)));
            } else {
                msgs.push(UnifiedMessage::assistant(format!(
                    "Assistant response {}",
                    i
                )));
            }
        }
        msgs
    }

    #[tokio::test]
    async fn compacts_when_window_available() {
        let provider = Arc::new(MockProvider::new("Summary of earlier conversation."));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);
        assert!(result.tokens_after < result.tokens_before);
        // Original: 12 messages. Window = first 6 (indices 0..6).
        // After: 1 summary + 6 fresh = 7 messages.
        assert_eq!(messages.len(), 7);

        // First message should be the summary
        let first_text = first_message_text(&messages[0]).unwrap();
        assert!(first_text.starts_with("[Context Summary]"));
    }

    #[tokio::test]
    async fn skips_when_window_too_small() {
        let provider = Arc::new(MockProvider::new("ignored"));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(4);
        let original_len = messages.len();
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert!(matches!(
            result.strategy_used,
            CompactStrategy::Skipped { .. }
        ));
        assert_eq!(messages.len(), original_len);
    }

    #[tokio::test]
    async fn falls_back_to_truncation_on_provider_failure() {
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let config = CompactorConfig {
            fallback_to_truncation: true,
            ..Default::default()
        };
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );
        // 1 summary + 6 fresh = 7
        assert_eq!(messages.len(), 7);

        let first_text = first_message_text(&messages[0]).unwrap();
        assert!(first_text.starts_with("[Context Summary]"));
    }

    #[tokio::test]
    async fn idempotent_on_already_compacted() {
        let provider = Arc::new(MockProvider::new("ignored"));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        // Simulate already-compacted state: summary + a few messages
        let mut messages = vec![
            UnifiedMessage::user("[Context Summary]\nPrevious conversation summary."),
            UnifiedMessage::assistant("Continuing from summary."),
        ];
        // Add fresh tail
        for i in 0..6 {
            messages.push(UnifiedMessage::user(format!("Fresh message {}", i)));
        }
        // Total: 8 messages. cut_end = 8 - 6 = 2. First starts with [Context Summary] and cut_end <= 2.
        let original_len = messages.len();
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert!(matches!(
            result.strategy_used,
            CompactStrategy::Skipped { reason } if reason.contains("already compacted")
        ));
        assert_eq!(messages.len(), original_len);
    }

    #[tokio::test]
    async fn recovers_when_summary_is_only_analysis_block() {
        // A weak/flash-tier model emits the <analysis> scratchpad but omits the
        // <summary> block. After stripping, the summary is empty — the window
        // must NOT be drained into an empty "[Context Summary]". Instead, fall
        // back to deterministic truncation so the context is preserved in
        // condensed form (regression for the "recover empty compaction" bug).
        let provider = Arc::new(MockProvider::new(
            "<analysis>\nlots of reasoning but no summary block\n</analysis>",
        ));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        // Must recover via truncation, not report a phantom LlmSummary success.
        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );
        assert_eq!(messages.len(), 7);

        // The inserted summary must carry the truncated window, never be empty.
        let first_text = first_message_text(&messages[0]).unwrap();
        assert!(first_text.starts_with("[Context Summary]"));
        assert!(
            !first_text
                .trim_start_matches("[Context Summary]")
                .trim()
                .is_empty(),
            "summary body must not be empty after analysis-only output"
        );
    }

    /// Provider that records the prompt text of the last `process()` call so a
    /// test can assert what actually reached the summarizer.
    #[derive(Clone)]
    struct CapturingProvider {
        last_prompt: Arc<crate::sync_primitives::Mutex<String>>,
        calls: Arc<crate::sync_primitives::Mutex<usize>>,
        response: String,
    }

    impl CapturingProvider {
        fn new(response: impl Into<String>) -> Self {
            Self {
                last_prompt: Arc::new(crate::sync_primitives::Mutex::new(String::new())),
                calls: Arc::new(crate::sync_primitives::Mutex::new(0)),
                response: response.into(),
            }
        }
        fn prompt(&self) -> String {
            self.last_prompt
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn call_count(&self) -> usize {
            *self.calls.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    impl crate::providers::AiProvider for CapturingProvider {
        fn process(
            &self,
            payload: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + '_,
            >,
        > {
            if let Some(first) = payload.messages.first() {
                *self.last_prompt.lock().unwrap_or_else(|e| e.into_inner()) = first.text_content();
            }
            *self.calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            let resp = self.response.clone();
            Box::pin(
                async move { Ok(crate::providers::adapter::ProviderResponse::text_only(resp)) },
            )
        }
        fn name(&self) -> &str {
            "capturing"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[tokio::test]
    async fn compaction_anchors_summary_to_live_task_in_tail() {
        // The user's current request lives in the kept fresh tail. The compactor
        // must derive it as the focus and inject it into the summarizer prompt so
        // the summary of older turns is biased toward the active task (hermes /
        // openclaw task-anchoring parity).
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nok\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        // 12 messages, fresh_tail 6 → window = [0..6], tail = [6..12]. Make the
        // last user turn a distinctive live task.
        let mut messages = make_messages(12);
        messages[10] = UnifiedMessage::user("LIVE_TASK: migrate the vector store");

        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);

        let prompt = provider.prompt();
        assert!(
            prompt.contains("<conversation_focus>")
                && prompt.contains("LIVE_TASK: migrate the vector store"),
            "summarizer prompt must carry the live task as focus; got:\n{prompt}"
        );
    }

    #[tokio::test]
    async fn summarize_slice_recovers_on_analysis_only_output() {
        // Same degenerate case for the session-split seed path: an analysis-only
        // response strips to "" and must fall back to deterministic truncation
        // rather than seed a child session with an empty string.
        let provider = Arc::new(MockProvider::new(
            "<analysis>\nreasoning only, no summary\n</analysis>",
        ));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let messages = make_messages(6);
        let seed = compactor.summarize_slice(&messages, None).await.unwrap();

        assert!(
            !seed.trim().is_empty(),
            "seed must not be empty after analysis-only output"
        );
        assert!(seed.contains("User message 0"));
    }

    #[tokio::test]
    async fn cache_reapplies_summary_across_prompt_rebuilds_without_llm() {
        // The harness rebuilds the message list from the session log every
        // turn. The second compact() call sees the same (grown) history and
        // must reapply the cached summary with zero LLM calls.
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nS1\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        let base = make_messages(12);
        let mut turn1 = base.clone();
        let r1 = compactor.compact(&mut turn1, 6, None).await.unwrap();
        assert_eq!(r1.strategy_used, CompactStrategy::LlmSummary);
        assert_eq!(provider.call_count(), 1);

        // Turn 2: rebuilt prompt = same history + one new exchange.
        let mut turn2 = base.clone();
        turn2.push(UnifiedMessage::assistant("new assistant turn"));
        turn2.push(UnifiedMessage::user("new user turn"));
        let r2 = compactor.compact(&mut turn2, 6, None).await.unwrap();

        assert_eq!(r2.strategy_used, CompactStrategy::CacheReuse);
        assert_eq!(
            provider.call_count(),
            1,
            "cache reapply must not call the LLM"
        );
        let first_text = first_message_text(&turn2[0]).unwrap();
        assert!(first_text.starts_with("[Context Summary]"));
        assert!(first_text.contains("S1"));
    }

    #[tokio::test]
    async fn cache_extends_with_one_llm_merge_when_gap_grows() {
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nmerged\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        let base = make_messages(12);
        let mut turn1 = base.clone();
        compactor.compact(&mut turn1, 6, None).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        // Turn N: the un-summarized gap behind the summary has grown past the
        // extension threshold → exactly one merge call whose transcript
        // carries the previous summary (openclaw "merge prior summaries").
        let mut turn2 = base.clone();
        for i in 0..(CACHE_EXTEND_MIN_MESSAGES + 2) {
            turn2.push(UnifiedMessage::assistant(format!("extra turn {i}")));
        }
        let r2 = compactor.compact(&mut turn2, 6, None).await.unwrap();

        assert_eq!(r2.strategy_used, CompactStrategy::LlmSummary);
        assert_eq!(provider.call_count(), 2);
        assert!(
            provider.prompt().contains("[Context Summary]"),
            "merge transcript must include the previous summary"
        );
        let first_text = first_message_text(&turn2[0]).unwrap();
        assert!(first_text.contains("merged"));

        // Turn N+1: the merged cover reapplies with zero further LLM calls.
        let mut turn3 = base.clone();
        for i in 0..(CACHE_EXTEND_MIN_MESSAGES + 2) {
            turn3.push(UnifiedMessage::assistant(format!("extra turn {i}")));
        }
        turn3.push(UnifiedMessage::user("fresh question"));
        let r3 = compactor.compact(&mut turn3, 6, None).await.unwrap();
        assert_eq!(r3.strategy_used, CompactStrategy::CacheReuse);
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn cache_invalidates_when_covered_prefix_changes() {
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nS\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        let base = make_messages(12);
        let mut turn1 = base.clone();
        compactor.compact(&mut turn1, 6, None).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        // A preflight pass rewrote a message inside the covered range — the
        // fingerprint must miss and a full recompaction must run.
        let mut turn2 = base.clone();
        turn2[2] = UnifiedMessage::user("rewritten by a cheap pass");
        turn2.push(UnifiedMessage::assistant("another turn"));
        turn2.push(UnifiedMessage::user("another question"));
        let r2 = compactor.compact(&mut turn2, 6, None).await.unwrap();

        assert_eq!(r2.strategy_used, CompactStrategy::LlmSummary);
        assert_eq!(provider.call_count(), 2, "stale fingerprint must recompact");
    }

    #[test]
    fn snap_boundary_forward_skips_tool_result_run() {
        // A boundary landing on a tool-result run must advance past the whole run
        // so it never separates a call from the result(s) that answer it.
        let messages = vec![
            UnifiedMessage::user("ask"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: serde_json::json!({}),
                }],
            },
            UnifiedMessage::tool_result("c1", "search", "r1", false),
            UnifiedMessage::tool_result("c2", "search", "r2", false),
            UnifiedMessage::user("next"),
        ];
        // Index 2 sits on the first result → snap to 4 (past both results).
        assert_eq!(snap_boundary_forward(&messages, 2), 4);
        // A clean index is returned unchanged.
        assert_eq!(snap_boundary_forward(&messages, 1), 1);
        assert_eq!(snap_boundary_forward(&messages, 4), 4);
        // Out-of-range / end index is preserved.
        assert_eq!(snap_boundary_forward(&messages, 5), 5);
    }

    #[tokio::test]
    async fn compaction_does_not_orphan_a_tool_pair_at_the_tail_boundary() {
        // Build a history where the default fresh-tail boundary would split a
        // tool call (kept-side) from its result (tail-side). After compaction the
        // kept tail must NOT begin with an orphan ToolResult.
        let provider = Arc::new(MockProvider::new(
            "<summary>\n## Primary Request\nstuff\n</summary>",
        ));
        let config = CompactorConfig {
            fresh_tail: 2,
            max_window: 8,
            ..CompactorConfig::default()
        };
        let compactor = ContextCompactor::new(provider, config);

        // 10 messages; with fresh_tail=2 the naive cut_end = 8. Place a ToolCall
        // at index 7 and its ToolResult at index 8 (straddling the boundary).
        let mut messages = vec![UnifiedMessage::user("start")];
        for i in 0..6 {
            messages.push(UnifiedMessage::assistant(format!("turn {i}")));
        }
        messages.push(UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                thought_signature: None,
                id: "pair".into(),
                name: "search".into(),
                arguments: serde_json::json!({}),
            }],
        }); // index 7
        messages.push(UnifiedMessage::tool_result(
            "pair", "search", "answer", false,
        )); // index 8
        messages.push(UnifiedMessage::user("end")); // index 9

        compactor.compact(&mut messages, 2, None).await.unwrap();

        // Whatever the boundary, no ToolResult may exist without its ToolCall.
        let call_ids: std::collections::HashSet<String> = messages
            .iter()
            .flat_map(|m| match m {
                UnifiedMessage::Assistant { content } => content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall { id, .. } => Some(id.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect();
        for m in &messages {
            if let UnifiedMessage::ToolResult { tool_call_id, .. } = m {
                assert!(
                    call_ids.contains(tool_call_id),
                    "compaction left an orphan ToolResult ({tool_call_id}) — boundary split a pair"
                );
            }
        }
    }
}
