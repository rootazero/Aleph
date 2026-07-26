//! LLM-based context compaction module.
//!
//! Replaces old conversation history with concise summaries via a side-channel
//! LLM call. Falls back to deterministic truncation when the LLM call fails.

use std::borrow::Cow;
use std::time::Duration;

use super::preserve::{is_summary_text, preserved_user_messages, PRESERVED_USER_TOKEN_BUDGET};
use super::summary_utils::{
    build_summary_update_prompt, build_window_summary_prompt, latest_user_task,
    strip_analysis_block,
};
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

/// Summarizer-input token budget for a single compaction call. The window is
/// anchored at the oldest compressible message and extended forward until the
/// serialized (per-message-capped) transcript reaches this many estimated
/// tokens — bounding the side-channel summarization call so a long compressible
/// span cannot overflow the (possibly flash-tier) summarizer's own context
/// window. Chosen well below common flash-tier windows (64k+) to leave room for
/// the prompt scaffold and the summary output; spans larger than this fold into
/// the running summary incrementally across turns via the cache extend-merge in
/// [`ContextCompactor::reapply_cached`].
const SUMMARIZER_INPUT_TOKEN_BUDGET: usize = 48_000;

/// Bound on cross-run carry-over slots. Sessions beyond the cap evict the
/// least-recently-WRITTEN entry (every `carryover_put` moves its key to the
/// back) — a long-lived interactive session that keeps compacting stays hot
/// even while daemon/cron fires churn one-shot session keys through the
/// front. A linear-scan `Vec` is fine at this size.
const CARRYOVER_MAX_SESSIONS: usize = 16;

/// Cross-run fingerprint-cache carry-over, keyed by session key.
///
/// The compactor is constructed fresh per run (`runner_impl`), so without
/// this slot the fingerprint cache dies at every run boundary and a long
/// high-pressure conversation re-pays the side-channel summarization call —
/// with freshly-worded summary text that re-keys the provider prompt cache —
/// at the start of every run. Same shape as the runner's
/// `CALIBRATION_CARRYOVER`: process-wide because the bridge is a boot-time
/// singleton; never persisted to disk; safe because every read is
/// hash-validated against the rebuilt history before reuse (a stale entry
/// misses and is purged).
static COMPACTION_CARRYOVER: Mutex<Vec<(String, CompactionCache)>> = Mutex::new(Vec::new());

/// Read the carried-over cache entry for `key`, if present. Slot-parametric
/// so tests can exercise eviction/purge without touching the process-global.
fn carryover_get(
    slot: &Mutex<Vec<(String, CompactionCache)>>,
    key: &str,
) -> Option<CompactionCache> {
    let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, entry)| entry.clone())
}

/// Store `entry` under `key`. Re-writing an existing key moves it to the
/// back (LRU-on-write); when the slot is full the least-recently-written
/// entry at the front is evicted. FIFO-by-first-insertion would evict the
/// feature's primary beneficiary first: the long-lived session inserted
/// earliest and updated most often.
fn carryover_put(slot: &Mutex<Vec<(String, CompactionCache)>>, key: &str, entry: CompactionCache) {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pos) = guard.iter().position(|(k, _)| k == key) {
        guard.remove(pos);
    } else if guard.len() >= CARRYOVER_MAX_SESSIONS {
        guard.remove(0);
    }
    guard.push((key.to_string(), entry));
}

/// Drop the entry for `key` (no-op when absent) — called when a hash
/// validation fails so the next run does not re-seed a dead entry.
fn carryover_remove(slot: &Mutex<Vec<(String, CompactionCache)>>, key: &str) {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|(k, _)| k != key);
}

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
    /// Agent id for scoping cache-watchdog resets (`CacheMonitor` keys its
    /// consecutive-miss counters per agent; a compaction here must reset only
    /// THIS agent's streak, not mute every other agent's watchdog). `None`
    /// (bare `new()`) falls back to the monitor's global reset.
    monitor_agent: Option<String>,
    /// Cross-run carry-over key (the session key). The compactor itself is
    /// constructed fresh per run, which used to discard the fingerprint cache
    /// at every run boundary — a long high-pressure conversation then paid a
    /// fresh summarization call at the start of every run, and the re-worded
    /// summary re-keyed the provider's message-prefix cache (the exact thrash
    /// the cache exists to prevent). When set, the cache seeds from and
    /// writes through to a process-wide per-session slot (same shape as the
    /// runner's `CALIBRATION_CARRYOVER`). Safe by construction: the entry is
    /// hash-validated against the rebuilt history each turn, so a stale
    /// carry-over simply misses and falls through to a full recompaction.
    carryover_key: Option<String>,
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
            monitor_agent: None,
            carryover_key: None,
        }
    }

    /// Scope cache-watchdog compaction resets to `agent_id` (see
    /// [`CacheMonitor::notify_compaction`]).
    ///
    /// [`CacheMonitor::notify_compaction`]: crate::thinker::prompt_builder::cache_monitor::CacheMonitor::notify_compaction
    pub fn with_monitor_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.monitor_agent = Some(agent_id.into());
        self
    }

    /// Enable cross-run fingerprint-cache carry-over keyed by `session_key`.
    ///
    /// Seeds the fingerprint cache from the process-wide per-session slot
    /// (populated by the previous run's compactions) and write-through-updates
    /// the slot on every `store_cache`. See the `carryover_key` field docs for
    /// why this is safe (hash-validated) and what it saves (one summarization
    /// call + one provider prefix re-key per run boundary).
    pub fn with_cache_carryover(mut self, session_key: impl Into<String>) -> Self {
        let key = session_key.into();
        if let Some(entry) = carryover_get(&COMPACTION_CARRYOVER, &key) {
            *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry);
        }
        self.carryover_key = Some(key);
        self
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
        let result = self.compact_inner(messages, fresh_tail, session_id).await?;
        // A compaction that actually rewrote the message list legitimately
        // breaks the provider prompt cache. Tell the process-wide monitor so
        // its consecutive-miss warning doesn't fire spuriously on the next
        // few (expectedly cold) calls. Two outcomes leave the provider-visible
        // prefix byte-identical to the previous turn and must NOT reset the
        // watchdog: `Skipped` (messages untouched) and `CacheReuse` (the SAME
        // summary text re-spliced at the same coordinates — the steady state
        // of a long high-pressure run, where a provider cache miss is exactly
        // the stable-prefix bug the watchdog exists to catch; resetting on it
        // would mute the warning for the entire steady state).
        if !matches!(
            result.strategy_used,
            CompactStrategy::Skipped { .. } | CompactStrategy::CacheReuse
        ) {
            crate::thinker::prompt_builder::cache_monitor::global_cache_monitor()
                .notify_compaction(self.monitor_agent.as_deref());
        }
        Ok(result)
    }

    /// Body of [`compact`](Self::compact) — all strategy selection and early
    /// `Skipped` exits live here so the wrapper above can observe the final
    /// outcome once.
    async fn compact_inner(
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

        // Step 2: idempotency check — skip if already compacted and window is
        // small. `is_summary_text` recognises both marker flavours — plain
        // `[Context Summary]` and the reuse path's `(from session memory)`.
        if let Some(first_text) = first_message_text(&messages[0]) {
            if is_summary_text(first_text) && cut_end <= 2 {
                return Ok(CompactResult {
                    tokens_before: 0,
                    tokens_after: 0,
                    strategy_used: CompactStrategy::Skipped {
                        reason: "already compacted with small window".into(),
                    },
                });
            }
        }

        // Step 3: select the compaction window. Anchor it at the OLDEST
        // compressible message (the head), not a fixed slice before the fresh
        // tail: the previous tail-anchored `max_window` window summarized only
        // the newest pre-tail messages and left the oldest history raw, re-sent
        // uncompressed on every turn until an escalation to session-split. The
        // window now extends forward from the head until a summarizer-input
        // token budget (or the `max_window` message ceiling) is reached, so the
        // prefix that actually bloats the context is compressed first. Any span
        // beyond the budget rides raw for one turn and folds into the running
        // summary incrementally via `reapply_cached`'s bounded extend-merge.
        //
        // `snap_boundary_forward` on both ends preserves tool-call/result pairs:
        // the head snap avoids draining a result whose call stays in the (empty)
        // kept head, and `select_window_end` snaps its end past any tool-result
        // run so the kept region never begins on an orphan.
        let window_start = snap_boundary_forward(messages.as_slice(), 0);
        let window_end = select_window_end(
            messages.as_slice(),
            window_start,
            cut_end,
            self.config.max_window,
            SUMMARIZER_INPUT_TOKEN_BUDGET,
        );

        // Guard: a zero-width window means there is nothing to compress.
        if window_start >= window_end {
            return Ok(CompactResult {
                tokens_before: 0,
                tokens_after: 0,
                strategy_used: CompactStrategy::Skipped {
                    reason: "compression window is empty".into(),
                },
            });
        }

        // Fingerprint-cache fast path (openteams compression-cache parity).
        // The harness rebuilds `messages` from the session log every turn, so
        // the previous turn's in-place compaction is gone by the time we run
        // again. If the last compaction's covered range still hashes to the
        // same fingerprint in this rebuild, reapply the cached summary with
        // zero API cost. When the un-summarized gap behind the summary has
        // grown past the extension threshold, run one LLM merge that feeds the
        // cached summary explicitly as prior state and folds only the new gap
        // into it (openclaw "merge prior summaries", done incrementally) — and
        // refresh the cache to cover the wider range.
        let cached = self.cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(c) = cached {
            let fits = c.start < c.end && c.end <= cut_end;
            if fits && hash_window(&messages[c.start..c.end]) == c.hash {
                return self.reapply_cached(messages, c, cut_end).await;
            }
            // Stale fingerprint (prefix changed under a preflight pass, or the
            // window shrank): drop the entry and fall through to a full
            // recompaction, which refreshes the cache. Purge the carry-over
            // slot too so the next run does not re-seed the same dead entry.
            *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Some(key) = self.carryover_key.as_deref() {
                carryover_remove(&COMPACTION_CARRYOVER, key);
            }
        }

        // Fingerprint of the window in rebuilt coordinates, captured before
        // any mutation below. Every success path stores it so the next turn's
        // rebuilt prompt hits the cache fast path above instead of paying the
        // side-channel LLM call again.
        let window_hash = hash_window(&messages[window_start..window_end]);

        // Fast path: reuse pre-existing hierarchical session summaries (zero
        // API cost). Active only when summary reuse is wired and the caller
        // supplied a session id; otherwise fall through to the LLM path.
        if let (Some(reuse), Some(sid)) = (self.summary_reuse.as_ref(), session_id) {
            let source =
                SessionSummarySource::new(reuse.backend.clone(), sid, reuse.agent_id.clone());
            // Captured before `try_reuse` drains the window out from under us —
            // it owns its own drain/insert and cannot be handed the preserved
            // turns after the fact.
            let preserved = preserved_user_messages(
                &messages[window_start..window_end],
                PRESERVED_USER_TOKEN_BUDGET,
            );
            if let Some(reuse_result) = source.try_reuse(messages, window_start, window_end).await {
                if let Some(text) = first_message_text(&messages[window_start]) {
                    // The cache cover must be the hashed+drained range
                    // [window_start, window_end) — `cut_end` can exceed
                    // `window_end` when `select_window_end` clipped (the
                    // long-history case), and mismatched coordinates would
                    // make next turn's validation hash a different range and
                    // miss on every rebuild.
                    self.store_cache(window_start, window_end, window_hash, text.to_string());
                }
                // Re-attach ABOVE the summary `try_reuse` just inserted — after
                // `store_cache` has read it, since that read addresses the
                // summary by position.
                messages.splice(window_start..window_start, preserved);
                tracing::info!(
                    tokens_before = reuse_result.tokens_before,
                    tokens_after = reuse_result.tokens_after,
                    "Compaction via session memory reuse (zero API cost)"
                );
                return Ok(reuse_result);
            }
        }

        let window = &messages[window_start..window_end];
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
        // Incremental inheritance: when the window carries a prior
        // `[Context Summary]` (e.g. a persisted child-session seed being
        // re-compacted, or a second compaction inside one turn), fold the new
        // turns into it via the "update" prompt instead of re-condensing the
        // already-condensed head from scratch — preserving structure and
        // avoiding paraphrase-decay across cycles. The prior summary is located
        // by scan, not at `window_start`: preservation re-attaches the user's
        // verbatim turns ABOVE it, so it no longer necessarily opens the window.
        // Everything before it is a verbatim copy of turns it already covers, so
        // the "new turns" are exactly what follows it.
        let prior_summary = messages[window_start..window_end]
            .iter()
            .enumerate()
            .find_map(|(i, m)| {
                first_message_text(m)
                    .and_then(strip_context_summary_prefix)
                    .map(|prior| (window_start + i, prior))
            });
        let prompt = match prior_summary {
            Some((idx, prior)) if idx + 1 < window_end => {
                let new_transcript = serialize_transcript(&messages[idx + 1..window_end]);
                build_summary_update_prompt(prior, &new_transcript, token_budget, focus.as_deref())
            }
            _ => build_window_summary_prompt(&transcript, token_budget, focus.as_deref()),
        };

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

        // The user's own turns come back verbatim above whichever summary the
        // window collapses into (B13) — computed once here because both arms
        // below drain the same window.
        let preserved = preserved_user_messages(
            &messages[window_start..window_end],
            PRESERVED_USER_TOKEN_BUDGET,
        );

        match summary {
            Some(summary) => {
                // Success: drain old window and insert the stripped summary.
                let summary_text = format!("[Context Summary]\n{summary}");
                let summary_msg = UnifiedMessage::user(summary_text.clone());
                let tokens_after = estimate_tokens(&summary);

                // Remove the compressed window and insert [user turns…, summary]
                // at window_start.
                splice_preserved(messages, window_start..window_end, preserved, summary_msg);
                self.store_cache(window_start, window_end, window_hash, summary_text);

                Ok(CompactResult {
                    tokens_before,
                    tokens_after,
                    strategy_used: CompactStrategy::LlmSummary,
                })
            }
            None => {
                // LLM failed or produced no usable summary — try fallback.
                // A prior running summary in the window must ride forward
                // verbatim: first-line truncation would collapse it to its
                // bare marker line, gutting the running summary (and the
                // cache entry stored below would replay the loss on every
                // rebuild). Only the raw turns after it are truncated —
                // everything before it is a verbatim copy of turns the
                // summary already covers.
                if self.config.fallback_to_truncation {
                    let truncated = match prior_summary {
                        Some((idx, prior)) if idx + 1 < window_end => format!(
                            "{prior}\n{}",
                            deterministic_truncation(&messages[idx + 1..window_end])
                        ),
                        Some((_, prior)) => prior.to_string(),
                        None => deterministic_truncation(window),
                    };
                    let tokens_after = estimate_tokens(&truncated);
                    let summary_text = format!("[Context Summary]\n{truncated}");
                    let summary_msg = UnifiedMessage::user(summary_text.clone());

                    splice_preserved(messages, window_start..window_end, preserved, summary_msg);
                    self.store_cache(window_start, window_end, window_hash, summary_text);

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
        let entry = CompactionCache {
            start,
            end,
            hash,
            summary,
        };
        // Write through to the cross-run carry-over slot so the next run on
        // this session seeds from it instead of recompacting from scratch.
        if let Some(key) = self.carryover_key.as_deref() {
            carryover_put(&COMPACTION_CARRYOVER, key, entry.clone());
        }
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry);
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
        // Bound how far this merge extends the cover. A head-anchored initial
        // window can leave a large un-summarized gap between the cached summary
        // (ending at `c.end`) and the fresh-tail boundary; folding all of it in
        // one call could overflow the side-channel summarizer. Cap the extend
        // target to one summarizer-input budget forward of `c.end`, so the
        // summary advances by at most a budget-worth per turn and the remainder
        // folds in on later turns. When the gap already fits, this is a no-op
        // (returns the original `cut_end`); when there is no new growth it
        // collapses to `c.end`, routing to the zero-LLM cache-reuse path below.
        let cut_end = select_window_end(
            messages,
            c.end,
            cut_end,
            self.config.max_window,
            SUMMARIZER_INPUT_TOKEN_BUDGET,
        );
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

        // The cache-reuse round must re-attach the user's turns exactly like the
        // LLM path does (B13). CacheReuse is the STEADY state — preserving only
        // on the summarization path would make the user's own words blink out of
        // the prompt on every cache hit. Preserved from the region the cached
        // summary replaces; the gap behind it still carries its user turns raw,
        // so preserving over the gap too would duplicate them.
        let preserved =
            preserved_user_messages(&messages[c.start..c.end], PRESERVED_USER_TOKEN_BUDGET);
        let summary_idx = c.start + preserved.len();
        let inserted = preserved.len() + 1;
        splice_preserved(
            messages,
            c.start..c.end,
            preserved,
            UnifiedMessage::user(c.summary.clone()),
        );

        // Mutated coordinates: the gap between the reapplied summary and the
        // fresh tail.
        let cut_end_m = cut_end - replaced + inserted;
        let gap_msgs = cut_end_m - (summary_idx + 1);
        let gap_text: String = messages[summary_idx + 1..cut_end_m]
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

        // Extension merge: one LLM call that updates the cached summary with the
        // new gap (see the incremental-inheritance note below for how the prior
        // summary is fed). Deterministic truncation mirrors the main path's
        // failure handling. The merge window is small (1 summary + gap), so no
        // max_window re-cap is needed here.
        let merge_window = &messages[c.start..cut_end_m];
        let transcript = serialize_transcript(merge_window);
        let merge_tokens = estimate_tokens(&transcript);
        let token_budget = (merge_tokens as f32 * self.config.target_ratio) as usize;
        let focus = latest_user_task(&messages[cut_end_m..]);
        // Incremental inheritance: feed the cached summary explicitly as the
        // prior state and fold only the new gap into it, rather than serializing
        // [summary + gap] together and re-summarizing. Preserves the running
        // summary's structure and avoids paraphrase-decay on each extension
        // (hermes `previousSummary` / pi `UPDATE_SUMMARIZATION_PROMPT` parity).
        let prompt = match strip_context_summary_prefix(&c.summary) {
            Some(prior) => {
                let gap_transcript = serialize_transcript(&messages[summary_idx + 1..cut_end_m]);
                build_summary_update_prompt(prior, &gap_transcript, token_budget, focus.as_deref())
            }
            None => build_window_summary_prompt(&transcript, token_budget, focus.as_deref()),
        };

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
            None if self.config.fallback_to_truncation => {
                // Carry the running summary forward verbatim — first-line
                // truncating the merge window (which INCLUDES the reapplied
                // summary message) would collapse it to its bare marker line
                // and cache the gutted result under `extended_hash`, replaying
                // the loss on every future rebuild. Only the raw gap behind
                // the summary is truncated; the preserved user turns ahead of
                // it are re-attached below anyway.
                let prior = strip_context_summary_prefix(&c.summary).unwrap_or(&c.summary);
                let gap = deterministic_truncation(&messages[summary_idx + 1..cut_end_m]);
                (
                    format!("{prior}\n{gap}"),
                    CompactStrategy::DeterministicTruncation,
                )
            }
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

        let summary_text = format!("[Context Summary]\n{body}");
        let tokens_after = estimate_tokens(&summary_text);
        // Re-preserve over the MERGED cover: the merge folds the gap into the
        // summary too, so the gap's own user turns must come back verbatim
        // alongside the ones already re-attached above (they are all inside
        // `[c.start, cut_end_m)` in mutated coordinates, and the summary sitting
        // among them is skipped by the preserver).
        let merged_preserved =
            preserved_user_messages(&messages[c.start..cut_end_m], PRESERVED_USER_TOKEN_BUDGET);
        splice_preserved(
            messages,
            c.start..cut_end_m,
            merged_preserved,
            UnifiedMessage::user(summary_text.clone()),
        );
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

/// Replace `range` with `[preserved user turns…, summary, execution list?]` —
/// the single shape every compaction drain site produces. The user's own words
/// stay verbatim and chronological ABOVE the summary that swallows everything
/// else, so a head-anchored window can no longer summarize the original
/// instruction away on its very first pass.
///
/// The execution list rides *below* the summary because it is live state the
/// model acts on next turn, not history: it belongs as close to the read head
/// as the drained region allows. It is `None` whenever the drained range held
/// no unfinished plan, which is the common case.
fn splice_preserved(
    messages: &mut Vec<UnifiedMessage>,
    range: std::ops::Range<usize>,
    preserved: Vec<UnifiedMessage>,
    summary: UnifiedMessage,
) {
    let carry = super::plan_carry::plan_carry_message(&messages[range.clone()]);
    messages.splice(
        range,
        preserved
            .into_iter()
            .chain(std::iter::once(summary))
            .chain(carry),
    );
}

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

/// Select the exclusive end of a compaction window that starts at `start`.
///
/// Walks forward from `start`, accumulating each message's *capped* transcript
/// token estimate (matching what [`serialize_transcript`] actually sends the
/// summarizer, via [`cap_transcript_text`]), and stops once `budget_tokens` is
/// reached or `max_messages` messages have been taken — whichever binds first.
/// The end is then snapped forward past any tool-result run (so the kept region
/// never begins on an orphaned result whose call was drained into the summary)
/// and clamped to `[start + 1, hard_end]`, so the window is always non-empty and
/// never spills past the fresh-tail boundary `hard_end`.
///
/// This is the single source of window bounding shared by the from-scratch
/// window selection in [`ContextCompactor::compact_inner`] and the extend-merge
/// in [`ContextCompactor::reapply_cached`], keeping both calls within the same
/// summarizer-input budget.
fn select_window_end(
    messages: &[UnifiedMessage],
    start: usize,
    hard_end: usize,
    max_messages: usize,
    budget_tokens: usize,
) -> usize {
    if start >= hard_end {
        return hard_end;
    }
    let msg_ceiling = start.saturating_add(max_messages.max(1));
    let mut acc = 0usize;
    let mut end = start;
    while end < hard_end && end < msg_ceiling {
        let text = messages[end].text_content();
        let capped = cap_transcript_text(&text);
        acc = acc.saturating_add(estimate_tokens(capped.as_ref()));
        end += 1;
        if acc >= budget_tokens {
            break;
        }
    }
    // Snap past any tool-result run so the kept region [end..] never begins on an
    // orphan, then clamp: never past the fresh-tail boundary, always ≥ 1 message.
    snap_boundary_forward(messages, end)
        .min(hard_end)
        .max(start + 1)
        .min(hard_end)
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

/// If `text` opens with a compaction-summary marker line — `[Context Summary]`
/// (LLM / truncation paths) or `[Context Summary (from session memory)]` (the
/// reuse path) — return the body after that line; `None` for raw turns. A
/// window carrying one holds a *prior* summary being folded into a wider one,
/// which routes summarization to the incremental "update" prompt instead of
/// re-summarizing the already-condensed text from scratch. Both markers share
/// the `[Context Summary` head recognised by [`is_summary_text`]; requiring
/// the marker line to close with `]` keeps a raw turn that merely opens with
/// those words from matching. `'\n'` is ASCII, so the byte split lands on a
/// UTF-8 boundary.
fn strip_context_summary_prefix(text: &str) -> Option<&str> {
    let line_end = text.find('\n').unwrap_or(text.len());
    let marker = &text[..line_end];
    if !is_summary_text(marker) || !marker.trim_end().ends_with(']') {
        return None;
    }
    Some(text[line_end..].trim_start_matches('\n'))
}

/// Per-message character cap applied to the summarizer transcript.
///
/// Old tool results and pasted blobs in the compaction window can each be many
/// KB; an un-capped transcript can blow past the side-channel summarizer's own
/// context window, failing the LLM call and forcing the lossy truncation
/// fallback. openclaw caps tool-result serialization at 2000 chars for the same
/// reason. This bounds the summarizer INPUT only — the stored message log and
/// the fingerprint cache hash are computed from `messages`, never the
/// transcript, so capping here cannot affect cache validity or what the model
/// finally sees in context.
const TRANSCRIPT_MSG_MAX_CHARS: usize = 2000;

/// Cap `text` to [`TRANSCRIPT_MSG_MAX_CHARS`] Unicode scalar values on a char
/// boundary (P7 UTF-8 safety), appending an elision marker when cut. The head
/// carries the actionable signal (what the tool did / the turn's intent); the
/// tail of a long old result is rarely load-bearing in a summary.
fn cap_transcript_text(text: &str) -> Cow<'_, str> {
    let count = text.chars().count();
    if count <= TRANSCRIPT_MSG_MAX_CHARS {
        return Cow::Borrowed(text);
    }
    let head: String = text.chars().take(TRANSCRIPT_MSG_MAX_CHARS).collect();
    let dropped = count - TRANSCRIPT_MSG_MAX_CHARS;
    Cow::Owned(format!("{head}… [+{dropped} chars elided]"))
}

/// Serialize a slice of messages into a human-readable transcript, capping each
/// message body via [`cap_transcript_text`] so a few huge old tool results can
/// never blow up the side-channel summarizer prompt.
fn serialize_transcript(messages: &[UnifiedMessage]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let text = msg.text_content();
        let capped = cap_transcript_text(&text);
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                lines.push(format!("tool_result({tool_name}): {capped}"));
                continue;
            }
        };
        lines.push(format!("{role}: {capped}"));
    }
    lines.join("\n")
}

/// Estimate token count using content-aware ratio detection.
///
/// Thin alias for [`pressure::estimate_tokens_smart`] — the single source of
/// truth for the prose-anchored, CJK/code-aware char→token estimate (which now
/// blends mixed content proportionally). Kept as a local name so the ten call
/// sites below read clearly.
fn estimate_tokens(text: &str) -> usize {
    crate::context::budget::pressure::estimate_tokens_smart(text)
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
                lines.push(format!("tool_result({tool_name}): {first_line}"));
                continue;
            }
        };
        let text = msg.text_content();
        let first_line = text.lines().next().unwrap_or("");
        lines.push(format!("{role}: {first_line}"));
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

    /// The single `[Context Summary]` a compaction inserts. Since B13 the
    /// summary no longer sits at index 0 — the user's preserved turns are spliced
    /// in above it — so tests locate it by marker, not by position.
    fn summary_text(messages: &[UnifiedMessage]) -> String {
        messages
            .iter()
            .map(UnifiedMessage::text_content)
            .find(|t| t.starts_with("[Context Summary]"))
            .expect("a successful compaction inserts a summary message")
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
        // Original: 12 messages. Window = first 6 (indices 0..6), of which 3 are
        // user turns (0/2/4). After: 3 preserved + 1 summary + 6 fresh = 10.
        assert_eq!(messages.len(), 10);

        // The user's own turns come back verbatim, in order, ABOVE the summary.
        let head: Vec<String> = messages[..4]
            .iter()
            .map(UnifiedMessage::text_content)
            .collect();
        assert_eq!(head[0], "User message 0");
        assert_eq!(head[1], "User message 2");
        assert_eq!(head[2], "User message 4");
        assert!(head[3].starts_with("[Context Summary]"));
    }

    #[tokio::test]
    async fn original_instruction_survives_the_head_anchored_window_verbatim() {
        // B13 regression: the compaction window is head-anchored, so the FIRST
        // thing summarized away used to be the task the user actually asked for
        // — surviving only as a ≤600-char focus hint derived from the tail.
        let provider = Arc::new(MockProvider::new(
            "<summary>\n## Primary Request\nsomething vague\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let original = "ORIGINAL: migrate the vector store to sqlite-vec, keep the API stable";
        let mut messages = make_messages(12);
        messages[0] = UnifiedMessage::user(original);

        compactor.compact(&mut messages, 6, None).await.unwrap();

        assert_eq!(
            messages[0].text_content(),
            original,
            "the user's first instruction must survive compaction verbatim"
        );
        let summary_pos = messages
            .iter()
            .position(|m| m.text_content().starts_with("[Context Summary]"))
            .expect("summary present");
        assert!(
            summary_pos > 0,
            "preserved user turns are emitted chronologically, above the summary"
        );
    }

    #[tokio::test]
    async fn preserved_user_turns_do_not_flicker_out_on_cache_reuse_rounds() {
        // CacheReuse is the STEADY state (the harness rebuilds the prompt every
        // turn and the fingerprint hits). Preserving only on the summarization
        // path would make the user's own words blink out of the prompt on every
        // cache-hit turn.
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nS1\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        let original = "ORIGINAL: migrate the vector store, keep the API stable";
        let mut base = make_messages(12);
        base[0] = UnifiedMessage::user(original);

        let mut turn1 = base.clone();
        compactor.compact(&mut turn1, 6, None).await.unwrap();
        assert_eq!(turn1[0].text_content(), original);

        // Turn 2: rebuilt prompt (compaction is not persisted) + a new exchange.
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
        assert_eq!(
            turn2[0].text_content(),
            original,
            "the preserved instruction must survive the cache-reuse round too"
        );
        assert!(summary_text(&turn2).contains("S1"));
    }

    #[tokio::test]
    async fn carryover_seeds_fresh_compactor_across_runs() {
        // The compactor is constructed fresh per run; the carry-over slot must
        // hand the fingerprint cache across that boundary so run 2 reuses the
        // summary with zero LLM calls (and byte-identical summary text — the
        // provider prompt cache survives the run boundary).
        let key = "carryover-test-run-boundary";
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nS1\n</summary>",
        ));
        let base = make_messages(12);

        // Run 1: full LLM compaction, write-through to the slot.
        let c1 = ContextCompactor::new(provider.clone(), CompactorConfig::default())
            .with_cache_carryover(key);
        let mut turn1 = base.clone();
        c1.compact(&mut turn1, 6, None).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        // Run 2: NEW compactor instance (run boundary) — seeds from the slot.
        let c2 = ContextCompactor::new(provider.clone(), CompactorConfig::default())
            .with_cache_carryover(key);
        let mut turn2 = base.clone();
        turn2.push(UnifiedMessage::assistant("new assistant turn"));
        turn2.push(UnifiedMessage::user("new user turn"));
        let r2 = c2.compact(&mut turn2, 6, None).await.unwrap();
        assert_eq!(r2.strategy_used, CompactStrategy::CacheReuse);
        assert_eq!(
            provider.call_count(),
            1,
            "run-2 reuse must not pay a second summarization call"
        );
        carryover_remove(&COMPACTION_CARRYOVER, key);
    }

    #[tokio::test]
    async fn carryover_stale_entry_misses_and_recompacts() {
        // A history rewritten between runs (post-turn compression, splits)
        // must hash-miss the carried entry and fall through to a full
        // recompaction — the carry-over can never replay a summary over
        // history it does not cover.
        let key = "carryover-test-stale-purge";
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nS1\n</summary>",
        ));
        let c1 = ContextCompactor::new(provider.clone(), CompactorConfig::default())
            .with_cache_carryover(key);
        let mut turn1 = make_messages(12);
        c1.compact(&mut turn1, 6, None).await.unwrap();
        assert!(carryover_get(&COMPACTION_CARRYOVER, key).is_some());

        let c2 = ContextCompactor::new(provider.clone(), CompactorConfig::default())
            .with_cache_carryover(key);
        let mut rewritten = make_messages(12);
        rewritten[2] = UnifiedMessage::assistant("history rewritten between runs");
        let r2 = c2.compact(&mut rewritten, 6, None).await.unwrap();
        assert_eq!(r2.strategy_used, CompactStrategy::LlmSummary);
        assert_eq!(
            provider.call_count(),
            2,
            "stale carry-over must recompact, not replay"
        );
        carryover_remove(&COMPACTION_CARRYOVER, key);
    }

    #[test]
    fn carryover_slot_bounded_updatable_and_removable() {
        let slot: Mutex<Vec<(String, CompactionCache)>> = Mutex::new(Vec::new());
        let entry = |h: u64| CompactionCache {
            start: 0,
            end: 2,
            hash: h,
            summary: "s".into(),
        };
        for i in 0..(CARRYOVER_MAX_SESSIONS + 3) {
            carryover_put(&slot, &format!("k{i}"), entry(i as u64));
        }
        assert!(slot.lock().unwrap().len() <= CARRYOVER_MAX_SESSIONS);
        assert!(
            carryover_get(&slot, "k0").is_none(),
            "least-recently-written entry evicted at cap"
        );
        carryover_put(&slot, "k5", entry(999));
        assert_eq!(carryover_get(&slot, "k5").expect("updated entry").hash, 999);
        carryover_remove(&slot, "k5");
        assert!(carryover_get(&slot, "k5").is_none());
    }

    #[test]
    fn carryover_rewrite_refreshes_recency_against_eviction() {
        // LRU-on-write: a hot session that keeps compacting must survive a
        // churn of one-shot daemon/cron session keys — FIFO-by-first-insert
        // would evict the feature's primary beneficiary first.
        let slot: Mutex<Vec<(String, CompactionCache)>> = Mutex::new(Vec::new());
        let entry = |h: u64| CompactionCache {
            start: 0,
            end: 2,
            hash: h,
            summary: "s".into(),
        };
        carryover_put(&slot, "hot-session", entry(1));
        // Fill the slot with cold one-shots, re-writing the hot key mid-churn.
        for i in 0..(CARRYOVER_MAX_SESSIONS - 1) {
            carryover_put(&slot, &format!("cron-{i}"), entry(10 + i as u64));
        }
        carryover_put(&slot, "hot-session", entry(2)); // refreshes recency
        for i in 0..(CARRYOVER_MAX_SESSIONS - 1) {
            carryover_put(&slot, &format!("cron-late-{i}"), entry(100 + i as u64));
        }
        assert_eq!(
            carryover_get(&slot, "hot-session")
                .expect("hot key survives")
                .hash,
            2,
            "re-written key must outlive older one-shot entries"
        );
    }

    #[tokio::test]
    async fn head_anchored_window_compresses_oldest_first() {
        // A long conversation: the compaction window must anchor at the OLDEST
        // message (shed the prefix that bloats every turn), not a fixed slice
        // before the fresh tail. With default max_window=40 the oldest 40
        // messages are summarized while the newer pre-tail gap rides raw — the
        // opposite of the old tail-anchored window, which left the oldest
        // history uncompressed and re-sent it verbatim every turn.
        let provider = Arc::new(MockProvider::new("Summary of earlier conversation."));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        // 60 messages, fresh_tail 6 → cut_end = 54. Oldest at index 1 (an
        // ASSISTANT turn — user turns are re-attached verbatim by B13 and so are
        // never proof of what the window swallowed), a newer pre-tail turn at
        // index 45 (inside the [40..54] raw gap).
        let mut messages = make_messages(60);
        messages[1] = UnifiedMessage::assistant("OLDEST_MARKER first task");
        messages[45] = UnifiedMessage::user("MIDGAP_MARKER newer pre-tail turn");

        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);

        let joined: String = messages
            .iter()
            .map(UnifiedMessage::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        // The oldest message was drained into the summary…
        assert!(
            !joined.contains("OLDEST_MARKER"),
            "oldest message must be compressed away, not left raw"
        );
        // …while the newer pre-tail gap message survives verbatim — proving the
        // window is anchored at the head, not the tail.
        assert!(
            joined.contains("MIDGAP_MARKER"),
            "newer pre-tail message must ride raw (oldest-first anchoring)"
        );
        assert!(summary_text(&messages).starts_with("[Context Summary]"));
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
        // 3 preserved user turns + 1 summary + 6 fresh = 10
        assert_eq!(messages.len(), 10);

        assert!(summary_text(&messages).starts_with("[Context Summary]"));
    }

    #[tokio::test]
    async fn truncation_fallback_carries_a_prior_summary_body_forward() {
        // CTX-02 regression: when the LLM call fails over a window that
        // carries a prior running summary (e.g. a persisted child-session
        // seed), deterministic truncation used to keep only the FIRST LINE of
        // each message — collapsing the summary to its bare marker line and
        // gutting the running state. The prior body must ride forward
        // verbatim; only the raw turns after it are truncated.
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let mut messages = vec![UnifiedMessage::user(
            "[Context Summary]\n## Primary Request\nORIGINAL_GOAL_MARKER\n## Pending\nstep two",
        )];
        for i in 0..13 {
            messages.push(UnifiedMessage::assistant(format!("turn {i}")));
        }

        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );

        let summary = summary_text(&messages);
        assert!(
            summary.contains("ORIGINAL_GOAL_MARKER") && summary.contains("step two"),
            "prior summary body must survive the truncation fallback verbatim; got:\n{summary}"
        );
        // The stored cache entry is what every future rebuild reapplies — it
        // must carry the full body too, not the gutted marker line.
        let cached = compactor
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("fallback compaction stores a cache entry");
        assert!(
            cached.summary.contains("ORIGINAL_GOAL_MARKER"),
            "cache must not retain a gutted summary"
        );
    }

    #[tokio::test]
    async fn cache_extend_merge_failure_keeps_the_running_summary_body() {
        // CTX-02 regression (reapply path): when the extension merge fails,
        // the truncation fallback used to first-line the merge window — which
        // INCLUDES the reapplied summary message — collapsing the running
        // summary to "[Context Summary]" and caching the gutted result under
        // the extended hash, so every future rebuild reapplied the loss.
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        // Hand-craft a valid cache entry over the first 8 messages, carrying a
        // distinctive multi-line running summary body. The 16-message gap up
        // to the fresh tail exceeds CACHE_EXTEND_MIN_MESSAGES, forcing the
        // extension merge (which the provider then fails).
        let mut messages = make_messages(30);
        let hash = hash_window(&messages[0..8]);
        compactor.store_cache(
            0,
            8,
            hash,
            "[Context Summary]\n## Primary Request\nRUNNING_BODY_MARKER\n## Pending\nstep two"
                .to_string(),
        );

        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );

        // The full body survives in the message list…
        let summary = summary_text(&messages);
        assert!(
            summary.contains("RUNNING_BODY_MARKER") && summary.contains("step two"),
            "merge-failure fallback must not gut the running summary; got:\n{summary}"
        );
        // …and in the refreshed cache entry (reapplied on every rebuild).
        let cached = compactor
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("extension fallback refreshes the cache");
        assert!(
            cached.summary.contains("RUNNING_BODY_MARKER"),
            "extended cache must not retain a gutted summary"
        );
    }

    #[tokio::test]
    async fn trailing_transient_message_survives_compact_to_fit() {
        // Spec 5-2 reproduction (Task 11): the harness think loop pushes
        // `deps.recall_context` as a transient trailing user message BEFORE
        // pressure measurement and compaction run. If the compaction window
        // could ever reach that tail message, it would be folded into a
        // summary and cached via `store_cache` — violating the "never
        // persisted" contract of the recall context.
        //
        // Shape: the provider is forced to fail with fallback_to_truncation
        // enabled, so the produced summary derives DETERMINISTICALLY from the
        // window content (`deterministic_truncation` keeps each window
        // message's first line) — if the window included the sentinel, it
        // would appear verbatim in the summary. `fresh_tail = 0` is the
        // harshest caller-side configuration (the CompactAndContinue directive
        // passes 0; `compact_to_fit` passes `budget.fresh_tail_count()`):
        // protection then rests solely on the compactor's own
        // `effective_tail = fresh_tail.max(config.fresh_tail)` clamp.
        let sentinel = "RECALL_TRANSIENT_SENTINEL_e7a1";
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let config = CompactorConfig {
            fallback_to_truncation: true,
            ..Default::default()
        };
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(20);
        messages.push(UnifiedMessage::user(sentinel));

        let result = compactor
            .compact(&mut messages, 0, Some("test-session"))
            .await
            .unwrap();
        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );

        // (1) The transient tail message survives verbatim as the LAST message.
        let last = messages
            .last()
            .map(UnifiedMessage::text_content)
            .unwrap_or_default();
        assert_eq!(
            last, sentinel,
            "transient recall tail must survive compaction verbatim"
        );

        // (2) No generated summary contains the sentinel — neither the
        // in-list summary message…
        assert!(
            !summary_text(&messages).contains(sentinel),
            "summary must not swallow the transient recall tail"
        );
        // …nor the fingerprint-cache entry `store_cache` retained (the cache
        // re-injects its summary into future turns via `reapply_cached`, so
        // transient content here would outlive the turn that pushed it).
        let cached = compactor
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("a successful compaction stores a cache entry");
        assert!(
            !cached.summary.contains(sentinel),
            "store_cache must never retain transient recall content"
        );
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
        // 3 preserved user turns + 1 summary + 6 fresh = 10
        assert_eq!(messages.len(), 10);

        // The inserted summary must carry the truncated window, never be empty.
        let summary = summary_text(&messages);
        assert!(
            !summary
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
    async fn re_compaction_over_a_prior_summary_uses_the_update_prompt() {
        // A window that opens with a persisted prior `[Context Summary]` (e.g. a
        // child-session seed being re-compacted) must fold the new turns into it
        // via the incremental update prompt, not re-condense the already-
        // condensed head from scratch.
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nrevised\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        // index 0 = prior summary; 1..8 = new turns; tail = last 6. total 14,
        // fresh_tail 6 → cut_end = 8 (> 2, so the idempotency skip never fires),
        // window_start = 0.
        let mut messages = vec![UnifiedMessage::user(
            "[Context Summary]\n## Primary Request\noriginal goal\n## Pending\nstep two",
        )];
        for i in 0..13 {
            messages.push(UnifiedMessage::assistant(format!("turn {i}")));
        }

        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);

        let prompt = provider.prompt();
        assert!(
            prompt.contains("UPDATING an existing running summary"),
            "must route to the incremental update prompt; got:\n{prompt}"
        );
        // Prior summary fenced as authoritative state, marker stripped.
        assert!(prompt.contains(
            "<current_summary>\n## Primary Request\noriginal goal\n## Pending\nstep two\n</current_summary>"
        ));
        // New turns ride under the NEW TURNS marker, not the from-scratch one.
        assert!(prompt.contains("---NEW TURNS---"));
        assert!(
            !prompt.contains("---TRANSCRIPT---"),
            "incremental path must not use the from-scratch transcript marker"
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
        assert!(summary_text(&turn2).contains("S1"));
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
        // extension threshold → exactly one merge call that feeds the previous
        // summary explicitly as prior state (incremental "update", not a fresh
        // re-summarization of the already-condensed head).
        let mut turn2 = base.clone();
        for i in 0..(CACHE_EXTEND_MIN_MESSAGES + 2) {
            turn2.push(UnifiedMessage::assistant(format!("extra turn {i}")));
        }
        let r2 = compactor.compact(&mut turn2, 6, None).await.unwrap();

        assert_eq!(r2.strategy_used, CompactStrategy::LlmSummary);
        assert_eq!(provider.call_count(), 2);
        let merge_prompt = provider.prompt();
        assert!(
            merge_prompt.contains("UPDATING an existing running summary")
                && merge_prompt.contains("## Primary Request\nmerged"),
            "merge must feed the previous summary explicitly via the update prompt; got:\n{merge_prompt}"
        );
        assert!(summary_text(&turn2).contains("merged"));

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

    #[tokio::test]
    async fn reuse_path_cache_survives_a_clipped_window() {
        // CTX-06 regression: the reuse path stored its cache entry as
        // [window_start, cut_end) while the fingerprint was hashed over
        // [window_start, window_end). Whenever `select_window_end` clipped the
        // window (window_end < cut_end — the long-history case), the next
        // turn's validation hashed a different range, missed every time, and
        // recompacted via session memory on every single turn.
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
        use crate::memory::store::sqlite::SqliteMemoryBackend;

        let backend: crate::memory::store::MemoryBackend =
            Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let raw = RawMemory::new(
            "Earlier turns: set up the project and ran the tests.".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-x")
        .with_session("sess-1")
        .with_path("aleph://session/sess-1/d0/0");
        backend.insert_raw_memory(&raw).await.unwrap();

        let provider = Arc::new(CapturingProvider::new("unused"));
        let config = CompactorConfig {
            // Force window_end (8) < cut_end (10): the clipped case.
            max_window: 8,
            ..CompactorConfig::default()
        };
        let compactor =
            ContextCompactor::new(provider.clone(), config).with_summary_reuse(backend, "agent-x");

        let base = make_messages(16);
        let mut turn1 = base.clone();
        let r1 = compactor
            .compact(&mut turn1, 6, Some("sess-1"))
            .await
            .unwrap();
        assert_eq!(r1.strategy_used, CompactStrategy::SessionMemoryReuse);

        // Turn 2: the harness rebuilds the same history — the cached summary
        // must reapply via the zero-cost fast path, not recompact.
        let mut turn2 = base.clone();
        let r2 = compactor
            .compact(&mut turn2, 6, Some("sess-1"))
            .await
            .unwrap();
        assert_eq!(
            r2.strategy_used,
            CompactStrategy::CacheReuse,
            "clipped-window reuse cache must hit on the rebuilt prompt"
        );
        assert_eq!(provider.call_count(), 0, "neither turn may call the LLM");
    }

    #[tokio::test]
    async fn session_memory_summary_routes_to_the_update_prompt() {
        // CTX-06 regression: the reuse path's marker is
        // "[Context Summary (from session memory)]", which the exact-prefix
        // strip used to miss — the already-condensed reuse summary was then
        // re-summarized from scratch (paraphrase decay) instead of folded via
        // the incremental update prompt.
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Primary Request\nrevised\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        let mut messages = vec![UnifiedMessage::user(
            "[Context Summary (from session memory)]\nearlier condensed work",
        )];
        for i in 0..13 {
            messages.push(UnifiedMessage::assistant(format!("turn {i}")));
        }

        let result = compactor.compact(&mut messages, 6, None).await.unwrap();
        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);

        let prompt = provider.prompt();
        assert!(
            prompt.contains("UPDATING an existing running summary"),
            "session-memory summary must route to the update prompt; got:\n{prompt}"
        );
        // Marker line stripped; body fenced as the authoritative prior state.
        assert!(prompt.contains("<current_summary>\nearlier condensed work\n</current_summary>"));
    }

    #[tokio::test]
    async fn idempotency_recognizes_the_session_memory_marker() {
        // CTX-06 regression: the Step-2 idempotency check matched only the
        // exact "[Context Summary]" prefix, so a small window opening with the
        // session-memory flavour was needlessly re-compacted.
        let provider = Arc::new(MockProvider::new("ignored"));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let mut messages = vec![
            UnifiedMessage::user("[Context Summary (from session memory)]\nPrevious work."),
            UnifiedMessage::assistant("Continuing from summary."),
        ];
        for i in 0..6 {
            messages.push(UnifiedMessage::user(format!("Fresh message {}", i)));
        }
        let original_len = messages.len();
        let result = compactor.compact(&mut messages, 6, None).await.unwrap();

        assert!(matches!(
            result.strategy_used,
            CompactStrategy::Skipped { reason } if reason.contains("already compacted")
        ));
        assert_eq!(messages.len(), original_len);
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

    #[test]
    fn select_window_end_bounds_by_message_ceiling() {
        // Small messages + generous token budget → the max_messages ceiling
        // binds, measured forward from the head.
        let messages = make_messages(50);
        let end = select_window_end(&messages, 0, 44, 40, SUMMARIZER_INPUT_TOKEN_BUDGET);
        assert_eq!(end, 40, "window bounded to max_messages from the head");
    }

    #[test]
    fn select_window_end_bounds_by_token_budget() {
        // A tiny budget binds before the message ceiling: the window stops as
        // soon as the accumulated capped transcript exceeds the budget.
        let messages = make_messages(50);
        let end = select_window_end(&messages, 0, 44, 40, 5);
        assert!(
            (1..40).contains(&end),
            "token budget must bind before the message ceiling, got {end}"
        );
    }

    #[test]
    fn select_window_end_clamps_to_hard_end_and_min_one() {
        let messages = make_messages(10);
        // hard_end below the ceiling → clamp to hard_end.
        assert_eq!(
            select_window_end(&messages, 0, 4, 40, SUMMARIZER_INPUT_TOKEN_BUDGET),
            4
        );
        // start == hard_end → empty, returns hard_end (caller guards).
        assert_eq!(
            select_window_end(&messages, 4, 4, 40, SUMMARIZER_INPUT_TOKEN_BUDGET),
            4
        );
    }

    #[test]
    fn select_window_end_snaps_past_trailing_tool_result() {
        // A boundary landing on a tool-result run must snap forward so the kept
        // region [end..] never begins on an orphaned result.
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
            UnifiedMessage::user("next"),
        ];
        // max_messages=2 lands the raw end on index 2 (the tool_result) → snap to 3.
        let end = select_window_end(&messages, 0, 4, 2, SUMMARIZER_INPUT_TOKEN_BUDGET);
        assert_eq!(
            end, 3,
            "end snaps past the tool_result so [end..] has no orphan"
        );
    }

    #[test]
    fn cap_transcript_text_passes_short_text_through_borrowed() {
        // Below the cap the text is returned untouched and borrowed (no alloc).
        let short = "a short tool result";
        let capped = cap_transcript_text(short);
        assert!(matches!(capped, Cow::Borrowed(_)));
        assert_eq!(capped, short);
    }

    #[test]
    fn cap_transcript_text_truncates_on_char_boundary_with_marker() {
        // A multibyte body over the cap must truncate without panicking and carry
        // the elision marker — never slice mid-codepoint (P7 UTF-8 safety).
        let long = "本".repeat(TRANSCRIPT_MSG_MAX_CHARS + 500);
        let capped = cap_transcript_text(&long);
        assert!(matches!(capped, Cow::Owned(_)));
        assert!(capped.contains("chars elided]"));
        // Head is bounded to the cap; the original is far longer.
        assert!(capped.chars().count() < long.chars().count());
        assert!(capped.starts_with('本'));
    }

    #[test]
    fn serialize_transcript_bounds_huge_tool_results() {
        // A giant old tool result must not be serialized verbatim into the
        // summarizer prompt (openclaw TOOL_RESULT_MAX_CHARS parity): the rendered
        // line is bounded, while the role/tool framing is preserved.
        let big = "Z".repeat(TRANSCRIPT_MSG_MAX_CHARS * 4);
        let messages = vec![
            UnifiedMessage::user("ask"),
            UnifiedMessage::tool_result("c1", "file_read", &big, false),
        ];
        let transcript = serialize_transcript(&messages);
        assert!(transcript.contains("user: ask"));
        assert!(transcript.contains("tool_result(file_read):"));
        assert!(transcript.contains("chars elided]"));
        // The transcript is far shorter than the raw body it summarizes.
        assert!(transcript.chars().count() < big.chars().count());
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
