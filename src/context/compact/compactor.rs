//! LLM-based context compaction module.
//!
//! Replaces old conversation history with concise summaries via a side-channel
//! LLM call. Falls back to deterministic truncation when the LLM call fails.

use std::time::Duration;

use super::preserve::{
    is_summary_text, preserved_user_messages, PRESERVED_USER_TOKEN_BUDGET, SUMMARY_MARKER,
};
use super::summary_utils::{
    build_summary_update_prompt, build_window_summary_prompt, cap_transcript_text,
    latest_user_task, prepend_user_instructions, strip_analysis_block,
    SUMMARIZER_INPUT_TOKEN_BUDGET,
};
use crate::memory::session_compactor::summary_source::SessionSummarySource;
use crate::memory::store::MemoryBackend;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

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

/// What one bounded summarizer round-trip produced.
///
/// Three states, not two, because `Cancelled` must not be handled like
/// `Failed`: the failure arms fall through to deterministic truncation, which
/// splices a degraded summary AND caches it. Doing that for a turn the user
/// stopped is how a cancellation leaves a permanent mark on the session's
/// compaction state.
enum SummarizerOutcome {
    Summary(String),
    Failed,
    Cancelled,
}

impl From<Option<String>> for SummarizerOutcome {
    fn from(v: Option<String>) -> Self {
        match v {
            Some(s) => Self::Summary(s),
            None => Self::Failed,
        }
    }
}

/// The result a cancelled compaction reports: nothing moved, nothing cached.
fn cancelled_result(tokens_before: usize) -> CompactResult {
    CompactResult {
        tokens_before,
        tokens_after: tokens_before,
        strategy_used: CompactStrategy::Skipped {
            reason: "cancelled".to_string(),
        },
    }
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
    /// Token budget for a single summarizer-input call (window selection and
    /// extend-merge). Defaults to [`SUMMARIZER_INPUT_TOKEN_BUDGET`] (48k);
    /// production wiring derives it from the summarizer model's own window
    /// (`ContextBudgetConfig::summarizer_input_budget`), so a narrow-window
    /// cheap/aux model cannot be fed more transcript than it can hold.
    pub summarizer_input_budget: usize,
}

impl Default for CompactorConfig {
    fn default() -> Self {
        Self {
            fresh_tail: 6,
            target_ratio: 0.25,
            max_window: 40,
            timeout: Duration::from_secs(15),
            fallback_to_truncation: true,
            summarizer_input_budget: SUMMARIZER_INPUT_TOKEN_BUDGET,
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
    /// Per-run poison flag for the cheap tier (codex `compact_model_fallback`
    /// parity). Set the first time the cheap summarizer fails with a
    /// model-class error (`llm_retry::classify_exhausted` → `Fallback` minus
    /// the two transient-derived reasons) — the canonical shape being a
    /// third-party compatible proxy that does not serve the preset's
    /// `default_aux_model`, so EVERY summarization 404s. Once poisoned,
    /// `summarizer()` routes straight to the main provider for the rest of
    /// this compactor's life, so a misconfigured deployment pays one failed
    /// call + one fallback call per run boundary instead of two wasted calls
    /// per compaction. Deliberately NOT persisted across runs (the compactor
    /// is rebuilt per run): a transient outage must not mute the cheap tier
    /// forever, and a config fix takes effect on the next run without a
    /// restart.
    cheap_poisoned: AtomicBool,
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
    /// Watchdog scope for compaction resets — build it with
    /// [`cache_scope`](crate::thinker::prompt_builder::cache_monitor::cache_scope),
    /// which is `(agent, session)` because the provider's prompt-cache prefix
    /// is per conversation. A compaction here must reset only THIS
    /// conversation's streak, not mute every other one's watchdog. `None`
    /// (bare `new()`) falls back to the monitor's global reset.
    monitor_scope: Option<String>,
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
    /// This run's cancellation token, so a summarizer round-trip nobody will
    /// read can be abandoned (see [`Self::with_cancel`]).
    cancel: Option<CancellationToken>,
}

impl ContextCompactor {
    /// Create a new compactor with the given provider and configuration.
    pub fn new(provider: Arc<dyn AiProvider>, config: CompactorConfig) -> Self {
        Self {
            provider,
            config,
            summary_reuse: None,
            cheap_provider: None,
            cheap_poisoned: AtomicBool::new(false),
            cache: Mutex::new(None),
            monitor_scope: None,
            carryover_key: None,
            cancel: None,
        }
    }

    /// Race every summarizer round-trip against this run's cancellation token.
    ///
    /// The harness already threads `parent_cancel` into every LLM call it makes
    /// itself — `race_llm_call`, `stream_llm_call`, `RescueHost::call_llm`
    /// (whose contract is literally "raced against cancellation") — but the
    /// compaction step is awaited directly, bounded only by
    /// `CompactorConfig::timeout` (15 s by default). So pressing stop during
    /// step 2c burned up to fifteen seconds on a summary nobody would read, and
    /// the reactive rescue path can spend the same fifteen again.
    ///
    /// Abandoning the call is the smaller half. The larger one is that a
    /// compaction which runs to completion **commits**: it splices the summary
    /// and writes it into the fingerprint cache, which is seeded into the
    /// process-wide cross-run carry-over. A cancelled turn therefore left a
    /// permanent mark on that session's compaction state. Cancellation now
    /// returns `Skipped { reason: "cancelled" }` before either happens.
    ///
    /// Taken as a builder rather than a per-call argument because the compactor
    /// is constructed **once per run** (`harness_bridge::runner_impl`, beside
    /// `with_cache_carryover`), so the token's lifetime is exactly the lifetime
    /// of the work it bounds — the pairing that a shared, longer-lived
    /// compactor would get wrong.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// One summarizer round-trip, bounded by both the configured timeout and
    /// this run's cancellation token.
    ///
    /// The three call sites (window, extend-merge, slice) all go through here
    /// so the cancellation contract cannot hold on two of them and not the
    /// third — the shape that produced this gap in the first place.
    async fn summarize_bounded(&self, stage: &'static str, prompt: &str) -> SummarizerOutcome {
        let call = async {
            let llm_result = tokio::time::timeout(self.config.timeout, self.call_llm(prompt)).await;
            accept_summary(stage, self.config.timeout, llm_result)
        };
        let Some(cancel) = self.cancel.as_ref() else {
            return SummarizerOutcome::from(call.await);
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => SummarizerOutcome::Cancelled,
            out = call => SummarizerOutcome::from(out),
        }
    }

    /// Scope cache-watchdog compaction resets to `scope` (see
    /// [`CacheMonitor::notify_compaction`]). Callers build `scope` through
    /// [`cache_scope`](crate::thinker::prompt_builder::cache_monitor::cache_scope)
    /// so it matches the key `MeteringProvider` records under.
    ///
    /// [`CacheMonitor::notify_compaction`]: crate::thinker::prompt_builder::cache_monitor::CacheMonitor::notify_compaction
    pub fn with_monitor_scope(mut self, scope: impl Into<String>) -> Self {
        self.monitor_scope = Some(scope.into());
        self
    }

    /// The scope this compactor resets its watchdog under, if any.
    ///
    /// Test-only: it exists so a construction site can assert the scoping
    /// actually arrived, rather than asserting that `with_monitor_scope` was
    /// called. An unscoped reset is silent — it mutes the prefix watchdog for
    /// every other conversation in the process and nothing observable changes.
    #[cfg(test)]
    #[must_use]
    pub fn monitor_scope(&self) -> Option<&str> {
        self.monitor_scope.as_deref()
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

    /// Token budget for a single summarizer-input call — derived from the
    /// summarizer model's own window at startup (see
    /// [`CompactorConfig::summarizer_input_budget`]). Exposed so the OTHER
    /// drain sites that never see a `CompactorConfig` construction (manual
    /// `/compact`, session-split pre-tail) apply the same bound.
    pub(crate) fn summarizer_input_budget(&self) -> usize {
        self.config.summarizer_input_budget
    }

    /// Provider used for summarization — the cheap-tier override when set and
    /// not poisoned (see [`Self::cheap_poisoned`]), otherwise the main
    /// provider passed to `new()`. Internal accessor.
    fn summarizer(&self) -> &Arc<dyn AiProvider> {
        if self.cheap_poisoned.load(Ordering::Relaxed) {
            return &self.provider;
        }
        self.cheap_provider.as_ref().unwrap_or(&self.provider)
    }

    /// Name of the provider summarization would actually be billed to.
    ///
    /// Deliberately reports the resolved [`Self::summarizer`] rather than
    /// whether `cheap_provider` is `Some`: a construction site can only be
    /// proven correct by what the *routing* ends up being, not by observing
    /// that a builder was called.
    #[cfg(test)]
    #[must_use]
    pub fn summarizer_name(&self) -> &str {
        self.summarizer().name()
    }

    /// Compact older messages in the conversation history.
    ///
    /// The `fresh_tail` parameter overrides `config.fresh_tail` when larger.
    ///
    /// `transient_tail` is how many messages at the END of `messages` are
    /// synthetic and were never persisted — the `<system-reminder>` nudges
    /// `build_prompt` appends plus the per-run recall strand. It is **added**
    /// to the protected tail rather than max'd into it, and it is a separate
    /// required argument precisely so no call site can forget to answer the
    /// question.
    ///
    /// Why it cannot be folded into `fresh_tail`: `fresh_tail` is a count of
    /// *persisted* messages ("leave the last N turns of the conversation
    /// verbatim"), while the vector this runs on ends with up to five entries
    /// that are recomputed every Think and belong to no turn. Sharing one
    /// budget between them meant a run where all four nudges fired protected
    /// **one** real message out of a configured six. Three things broke at
    /// once, all silently: the model lost the verbatim tail it had just been
    /// shown; [`latest_user_task`] scanned a tail that was almost entirely
    /// scaffolding and returned `None`, dropping `<conversation_focus>`; and
    /// `cut_end` moved with the nudge count, so the fingerprint cache's
    /// `c.end <= cut_end` test failed on exactly the turns where nudges fired
    /// — purging the entry, re-paying the summarization call, and re-keying
    /// the provider's prefix cache, which is the thrash this cache exists to
    /// prevent. `PreflightPipeline` was taught the same lesson in §2.18; the
    /// compactor was not told.
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
        transient_tail: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<CompactResult> {
        let result = self
            .compact_inner(messages, fresh_tail, transient_tail, session_id)
            .await?;
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
                .notify_compaction(self.monitor_scope.as_deref());
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
        transient_tail: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<CompactResult> {
        // `max` picks the stricter of the two *persisted* tail requests; the
        // transient count is then ADDED, because those messages are not turns
        // and must not spend the turn budget. See `compact`'s doc.
        let effective_tail = fresh_tail
            .max(self.config.fresh_tail)
            .saturating_add(transient_tail);

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
            self.config.summarizer_input_budget,
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

        // Fingerprint of the window in rebuilt coordinates, captured before
        // any mutation below. Every success path stores it so the next turn's
        // rebuilt prompt hits the cache fast path instead of paying the
        // side-channel LLM call again.
        let window_hash = hash_window(&messages[window_start..window_end]);

        // Zero-LLM-cost fast paths: the validated fingerprint cache
        // (`reapply_cached`) and session-memory summary reuse (`try_reuse`).
        // `None` falls through to the side-channel summarizer below.
        if let Some(result) = self
            .try_zero_cost_compaction(
                messages,
                window_start,
                window_end,
                cut_end,
                window_hash,
                session_id,
            )
            .await?
        {
            return Ok(result);
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
        let summary = match self.summarize_bounded("window", &prompt).await {
            SummarizerOutcome::Summary(s) => Some(s),
            SummarizerOutcome::Failed => None,
            // Return BEFORE the splice and before `store_cache`: a stopped turn
            // must leave this session's compaction state exactly as it found it.
            SummarizerOutcome::Cancelled => return Ok(cancelled_result(tokens_before)),
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
                let summary_text = format!("{SUMMARY_MARKER}\n{summary}");
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
                    let summary_text = format!("{SUMMARY_MARKER}\n{truncated}");
                    let summary_msg = UnifiedMessage::user(summary_text.clone());

                    splice_preserved(messages, window_start..window_end, preserved, summary_msg);
                    // Spliced for THIS turn, deliberately not cached.
                    //
                    // `cache`'s own doc calls itself "the fingerprint cache of
                    // the last SUCCESSFUL compaction", and `store_cache` writes
                    // through to `COMPACTION_CARRYOVER`, a process-wide slot
                    // that `with_cache_carryover` seeds into every later run on
                    // this session key. Caching a truncation therefore turned
                    // one transient 502 (or one 15 s timeout) into a permanent
                    // verdict: the harness rebuilds `messages` from an
                    // append-only log, so `hash_window` matches forever, and
                    // every later turn takes `reapply_cached` and re-splices
                    // the degraded text without ever retrying the summarizer —
                    // silently, since `CacheReuse` is excluded from the
                    // degradation warning. The original turns are still in the
                    // session log, so what was discarded was recoverable.
                    //
                    // This is the same permanence `SummarizerOutcome::Cancelled`
                    // was split out to avoid; that split fixed the stopped-turn
                    // arm and left the failed-turn arm, which latches on its
                    // own. Not caching costs one summarizer attempt per
                    // high-pressure turn while the summarizer is down — a call
                    // that failed, against context that cannot be recovered any
                    // other way — and self-heals the moment it comes back.

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

    /// Zero-LLM-cost fast paths for [`compact_inner`](Self::compact_inner):
    /// the validated fingerprint cache (`reapply_cached`, zero cost) and
    /// session-memory summary reuse (`try_reuse`, zero API cost). Returns
    /// `Some(result)` when either handled this turn's compaction; `None`
    /// means fall through to the side-channel summarizer. `window_hash` is
    /// the fingerprint of `[window_start, window_end)` in rebuilt coordinates,
    /// captured by the caller before any mutation.
    async fn try_zero_cost_compaction(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        window_start: usize,
        window_end: usize,
        cut_end: usize,
        window_hash: u64,
        session_id: Option<&str>,
    ) -> anyhow::Result<Option<CompactResult>> {
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
                return self.reapply_cached(messages, c, cut_end).await.map(Some);
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

        // Fast path: reuse pre-existing hierarchical session summaries (zero
        // API cost). Active only when summary reuse is wired and the caller
        // supplied a session id; otherwise fall through to the LLM path.
        if let (Some(reuse), Some(sid)) = (self.summary_reuse.as_ref(), session_id) {
            let source =
                SessionSummarySource::new(reuse.backend.clone(), sid, reuse.agent_id.clone());
            // Captured before `try_reuse` drains the window out from under us —
            // it owns its own drain/insert and cannot be handed the preserved
            // turns after the fact. The carried artifacts (execution list, file
            // ledger) are captured here for the same reason: this is the FIFTH
            // drain site, and it used to re-attach the user's turns while
            // silently dropping everything `splice_preserved` carries — so the
            // zero-API-cost path was the one path where the model lost its own
            // checklist. Whatever a drain hands forward, every drain hands
            // forward.
            let preserved = preserved_user_messages(
                &messages[window_start..window_end],
                PRESERVED_USER_TOKEN_BUDGET,
            );
            let carriers = carried_artifacts(&messages[window_start..window_end]);
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
                // Re-attach around the summary `try_reuse` just inserted — after
                // `store_cache` has read it, since that read addresses the
                // summary by position. Carriers go in first, directly BELOW the
                // summary (they are live state the model acts on next turn);
                // the user's turns then go in ABOVE it. Doing it in this order
                // keeps both splice indices expressed against the summary's
                // known position instead of a running offset.
                let below = window_start + 1;
                messages.splice(below..below, carriers);
                messages.splice(window_start..window_start, preserved);
                tracing::info!(
                    tokens_before = reuse_result.tokens_before,
                    tokens_after = reuse_result.tokens_after,
                    "Compaction via session memory reuse (zero API cost)"
                );
                return Ok(Some(reuse_result));
            }
        }
        Ok(None)
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
            self.config.summarizer_input_budget,
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
        // Taken from the splice itself, never recomputed: the carriers it
        // appends below the summary are conditional, and a hand-rolled
        // `preserved.len() + 1` under-counts by one for every window that still
        // holds an unfinished execution list (or a file ledger). The gap
        // coordinates below are derived from this number, and an under-count
        // there is silent context loss — the last gap message never reaches the
        // summarizer while `store_cache` records a cover that includes it.
        let inserted = splice_preserved(
            messages,
            c.start..c.end,
            preserved,
            UnifiedMessage::user(c.summary.clone()),
        );
        // First message of the un-summarized gap in MUTATED coordinates. Derived
        // from `inserted` (which counts preserved turns + summary + carriers)
        // rather than from `summary_idx + 1`, so a carrier below the summary is
        // never mistaken for gap content and fed back to the summarizer.
        let gap_start = c.start + inserted;

        // Mutated coordinates: the gap between the reapplied summary and the
        // fresh tail.
        let cut_end_m = cut_end - replaced + inserted;
        let gap_msgs = cut_end_m - gap_start;
        let gap_text: String = messages[gap_start..cut_end_m]
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
                let gap_transcript = serialize_transcript(&messages[gap_start..cut_end_m]);
                build_summary_update_prompt(prior, &gap_transcript, token_budget, focus.as_deref())
            }
            None => build_window_summary_prompt(&transcript, token_budget, focus.as_deref()),
        };

        let merged = match self.summarize_bounded("merge", &prompt).await {
            SummarizerOutcome::Summary(s) => Some(s),
            SummarizerOutcome::Failed => None,
            // Same contract as the window path: the extend-merge arm also
            // splices and caches, so a cancelled turn must commit neither.
            SummarizerOutcome::Cancelled => return Ok(cancelled_result(tokens_before)),
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
                let gap = deterministic_truncation(&messages[gap_start..cut_end_m]);
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

        let summary_text = format!("{SUMMARY_MARKER}\n{body}");
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
        // Advance the cover only on a real merge — same reason the window path
        // does not cache its truncation. The sibling arm above ("merge failed
        // and truncation is disabled") already states the rule: *the cache
        // stays on its old, still-valid cover, so the next turn retries the
        // merge*. The truncation arm is that same failure wearing a fallback,
        // so caching under `extended_hash` would replay the gutted gap on every
        // later rebuild and — via `store_cache`'s write-through to the
        // process-global carry-over — on every later run too.
        if matches!(strategy, CompactStrategy::LlmSummary) {
            self.store_cache(c.start, cut_end, extended_hash, summary_text);
        }

        Ok(CompactResult {
            tokens_before,
            tokens_after,
            strategy_used: strategy,
        })
    }

    /// Summarize a slice of messages and return the raw summary string.
    ///
    /// Used by `session_split::summarize_pretail` (child-session seed) and by
    /// `manual::compact_session` (user-driven `/compact`) to produce a summary
    /// without running a full in-place `compact()`. Falls back to deterministic
    /// truncation when the LLM call fails (mirrors `compact`).
    ///
    /// `focus` is the user's active task (the most recent request preserved
    /// verbatim in the kept tail). Passing it anchors the summary to the live
    /// work — the heavy-compaction path where losing the task thread hurts
    /// most. `None` keeps the historical static prompt.
    ///
    /// `instructions` is the user's own `/compact <instructions>` directive
    /// (codex / pi / kimi-cli parity). It outranks `focus`: the anchor is a
    /// guess at what matters, this is the user saying so. `None` — the whole
    /// automatic path — leaves the prompt byte-identical.
    pub(crate) async fn summarize_slice(
        &self,
        messages: &[UnifiedMessage],
        focus: Option<&str>,
        instructions: Option<&str>,
    ) -> anyhow::Result<String> {
        if messages.is_empty() {
            return Ok(String::new());
        }

        let transcript = serialize_transcript(messages);
        let tokens_before = estimate_tokens(&transcript);
        let token_budget = (tokens_before as f32 * self.config.target_ratio) as usize;

        let prompt = prepend_user_instructions(
            &build_window_summary_prompt(&transcript, token_budget, focus),
            instructions,
        );

        // Strip before the emptiness check: an analysis-only response (no
        // <summary> block) strips to an empty string, which must fall back to
        // deterministic truncation rather than seed a child session with "".
        //
        // A cancelled slice falls back to truncation rather than erroring: this
        // function's two callers (the session-split child seed and manual
        // `/compact`) must return *something* — an `Err` here would leave the
        // split child with no seed at all. Truncation is deterministic and
        // commits nothing to the cache, so the stopped turn still leaves no
        // trace beyond the drain its caller was already performing.
        Ok(match self.summarize_bounded("slice", &prompt).await {
            SummarizerOutcome::Summary(s) => s,
            SummarizerOutcome::Failed | SummarizerOutcome::Cancelled => {
                deterministic_truncation(messages)
            }
        })
    }

    /// Side-channel LLM call for summarization. Routes to the cheap-tier
    /// provider when one is configured (Reasonix parity), otherwise reuses
    /// the main provider.
    ///
    /// Cheap-tier fallback (codex `compact_model_fallback` parity): when the
    /// cheap provider fails with an error the shared classifier reads as
    /// "switch model" (`RetryVerdict::Fallback` — 404 model-not-found being
    /// the canonical shape, see [`Self::cheap_poisoned`]) or "this input
    /// overflowed the cheap model's own window" (`CompactAndRetry` — the
    /// summarizer input budget is sized from the summarizer model's window,
    /// but an operator-pinned `summary_model` can outrun the catalog), the
    /// call is retried once on the main provider. Only the final outcome
    /// reaches `accept_summary`, so the observability contract (one warn per
    /// failure class) is unchanged. Model-class Fallbacks additionally poison
    /// the cheap tier for the rest of the run; the two transient-derived
    /// Fallback reasons (overload / network) retry without poisoning, so one
    /// blip does not mute the cheap tier for a long run.
    async fn call_llm(&self, prompt: &str) -> anyhow::Result<String> {
        let system =
            "You are a precise conversation summarizer. Output the analysis block followed by the summary block. No other text.";
        // `msgs` outlives both attempts: `RequestPayload` borrows it.
        let msgs = [UnifiedMessage::user(prompt)];
        let build_payload = || RequestPayload::new(&msgs).with_system(Some(system));
        let first = self.summarizer().clone();
        let tried_cheap =
            self.cheap_provider.is_some() && !self.cheap_poisoned.load(Ordering::Relaxed);
        let response: ProviderResponse = match first.process(build_payload()).await {
            Ok(r) => r,
            Err(e) => {
                let verdict = crate::providers::llm_retry::classify_exhausted(&e.to_string());
                // The fallback target exists only when the failed attempt ran
                // on the cheap tier — the main provider is the floor.
                let fallback_worthwhile = tried_cheap
                    && matches!(
                        verdict,
                        crate::providers::llm_retry::RetryVerdict::Fallback { .. }
                            | crate::providers::llm_retry::RetryVerdict::CompactAndRetry { .. }
                    );
                if !fallback_worthwhile {
                    return Err(e.into());
                }
                if let crate::providers::llm_retry::RetryVerdict::Fallback { ref reason } = verdict
                {
                    // Poison on every model-class failure (404 / auth /
                    // model-scoped quota): none of them heal within a run,
                    // and the per-run scope bounds the mute. The two
                    // transient-derived reasons (`classify_exhausted` wraps an
                    // exhausted in-place retry budget — the compactor's budget
                    // is one call, so they surface here) are excluded: one
                    // overload blip must not mute the cheap tier for a long run.
                    let is_transient_derived = reason.starts_with("provider overloaded")
                        || reason.starts_with("primary model unavailable");
                    if !is_transient_derived {
                        self.cheap_poisoned.store(true, Ordering::Relaxed);
                    }
                    tracing::warn!(
                        target: "context_budget",
                        cheap_provider = %first.name(),
                        %reason,
                        poisoned = self.cheap_poisoned.load(Ordering::Relaxed),
                        "cheap summarizer failed with a model-level error; retrying once on \
                         the main provider (check [context_budget] summary_model / the preset's \
                         aux model against what this provider actually serves)",
                    );
                } else {
                    tracing::warn!(
                        target: "context_budget",
                        cheap_provider = %first.name(),
                        "summarizer input overflowed the cheap model's window; retrying once \
                         on the main provider",
                    );
                }
                self.provider.process(build_payload()).await?
            }
        };
        Ok(response.text.unwrap_or_default())
    }
}

// === Helper functions ===

/// Replace `range` with `[preserved user turns…, summary, carried artifacts…]`
/// — the single shape every compaction drain site produces. The user's own
/// words stay verbatim and chronological ABOVE the summary that swallows
/// everything else, so a head-anchored window can no longer summarize the
/// original instruction away on its very first pass.
///
/// The carriers ([`carried_artifacts`]) ride *below* the summary because they
/// are live state the model acts on next turn, not history: they belong as
/// close to the read head as the drained region allows. Each is absent whenever
/// the drained range held nothing of its kind, which is the common case.
///
/// Returns **how many messages were actually inserted**. Callers that go on to
/// address the mutated list need that number, and only this function can know
/// it: the carriers below the summary are conditional, so any caller-side
/// arithmetic is a second — and eventually wrong — answer to the same question.
/// `reapply_cached` used to compute `preserved.len() + 1` by hand and was off
/// by one on every window that still held an unfinished execution list, which
/// silently dropped the newest message of the merged gap out of the summarizer
/// input while the cache entry claimed to cover it.
fn splice_preserved(
    messages: &mut Vec<UnifiedMessage>,
    range: std::ops::Range<usize>,
    preserved: Vec<UnifiedMessage>,
    summary: UnifiedMessage,
) -> usize {
    let carriers = carried_artifacts(&messages[range.start..range.end]);
    let inserted = preserved.len() + 1 + carriers.len();
    messages.splice(
        range,
        preserved
            .into_iter()
            .chain(std::iter::once(summary))
            .chain(carriers),
    );
    inserted
}

/// The deterministic artifacts a drained window must hand forward *below* the
/// summary — facts the model acts on next turn that a lossy prose summary is
/// not trusted to reproduce.
///
/// One function so every drain site carries the same set: the LLM-summary path,
/// the truncation fallback, both extend-merge legs, and the zero-cost
/// session-memory reuse path. Each entry is `None` in the common case, so calm
/// windows pay nothing.
fn carried_artifacts(window: &[UnifiedMessage]) -> Vec<UnifiedMessage> {
    [
        super::plan_carry::plan_carry_message(window),
        super::file_carry::file_carry_message(window),
        super::image_carry::image_carry_message(window),
    ]
    .into_iter()
    .flatten()
    .collect()
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

/// Serialize a slice of messages into a human-readable transcript, capping each
/// message body via [`cap_transcript_text`] so a few huge old tool results can
/// never blow up the side-channel summarizer prompt.
pub(crate) fn serialize_transcript(messages: &[UnifiedMessage]) -> String {
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

/// Accept a summarizer response, or say — out loud — why it was rejected.
///
/// Single source for all three side-channel summarization call sites (window
/// compaction, incremental merge, session-split slice). They were three copies
/// of `Ok(Err(_)) | Err(_) => None`, which discards the error **value**, and
/// this 2000-line file had exactly one `tracing::` call — on the zero-cost
/// cache-reuse path.
///
/// The cost of that silence is a whole deployment class: a third-party
/// Anthropic-compatible `base_url` (a first-class supported setup) plus tier-2
/// auto-routing clones the main provider config and swaps only the model to
/// that preset's `default_aux_model`. If the proxy does not serve that model,
/// every summarization 404s. Boot succeeds (the config is well-formed), and
/// this provider is not wrapped by `FailoverProvider` — so there is no
/// failover. (Its spend IS now metered: both compactor construction sites wrap
/// the cheap summarizer in `MeteringProvider` under `compactor:<agent>`, so the
/// failure at least shows up as zero-output spend in the billing view.)
/// Compaction silently degrades to
/// first-line truncation from day one, forever, with no log line, metric or
/// doctor check naming the summarizer. The compaction circuit breaker does
/// eventually trip, but it reports pressure, not cause.
///
/// Deliberately NOT surfaced to the model (A2): the model does not choose the
/// summarizer and cannot act on this. It is an operator fact, so it belongs on
/// the operator's channel.
fn accept_summary(
    stage: &'static str,
    timeout: std::time::Duration,
    llm_result: Result<anyhow::Result<String>, tokio::time::error::Elapsed>,
) -> Option<String> {
    match llm_result {
        Ok(Ok(raw)) => {
            let stripped = strip_analysis_block(&raw);
            if stripped.trim().is_empty() {
                tracing::warn!(
                    target: "context_budget",
                    stage,
                    "context summarizer returned no <summary> block; \
                     falling back to deterministic truncation"
                );
                return None;
            }
            Some(stripped)
        }
        Ok(Err(e)) => {
            tracing::warn!(
                target: "context_budget",
                stage,
                error = %e,
                "context summarizer call failed; falling back to deterministic truncation"
            );
            None
        }
        Err(_elapsed) => {
            tracing::warn!(
                target: "context_budget",
                stage,
                timeout_secs = timeout.as_secs(),
                "context summarizer timed out; falling back to deterministic truncation"
            );
            None
        }
    }
}

/// Deterministic truncation: keep only the first line of each message.
pub(crate) fn deterministic_truncation(messages: &[UnifiedMessage]) -> String {
    /// First line, then capped — because the first line is not a bound.
    ///
    /// "Keep one line" reduces nothing on the content type that dominates a
    /// long agent context: [`UnifiedMessage::text_content`] renders a
    /// `ContentBlock::Json` through serde_json's compact formatter, which
    /// emits no newlines at all (interior ones are escaped), and every
    /// `ToolResult` `build_prompt` constructs is exactly one such block. So
    /// `lines().next()` returned an 8 KB payload verbatim and uncapped, while
    /// deleting the assistant prose around it.
    ///
    /// [`cap_transcript_text`] is the same cap the summarizer's *input* path
    /// already applies to every message ([`serialize_transcript`]); the two
    /// paths disagreeing on whether a message has a size was the asymmetry.
    ///
    /// ## Where this runs — it is not only the summarizer-failed path
    ///
    /// An earlier version of this doc said the fallback "runs precisely when
    /// the summarizer failed, i.e. when losing the prose costs most". That
    /// sentence was true of the call site it was written next to and false of
    /// the function. Four reachings, three distinct triggers:
    ///
    /// * **Summarizer failed or timed out** — `compact_inner`'s window arm and
    ///   `reapply_cached`'s merge arm, both gated on `fallback_to_truncation`.
    ///   This is the case the sentence described.
    /// * **The turn was cancelled** — `summarize_slice` folds
    ///   `SummarizerOutcome::Cancelled` into the same arm on purpose, because
    ///   its two callers (the session-split child seed and manual `/compact`)
    ///   must return *something*.
    /// * **No summarizer provider is wired at all** — [`super::manual::compact_session`]
    ///   calls this directly on its `None` arm. Nothing failed there: the
    ///   truncation *is* the product of a user-typed `/compact`, not a
    ///   degradation from a better answer that did not arrive.
    ///
    /// The cap is wanted in all three, but for different reasons, and only the
    /// third is user-visible as itself. On that path the 2000-char ceiling is
    /// what a `/compact` on a provider-less deployment actually returns — and
    /// it is also what makes `manual`'s `tokens_after < tokens_before` refusal
    /// (a summary no smaller than what it replaces is pure loss) reachable at
    /// all on a span of long single-line turns.
    fn head(text: &str) -> std::borrow::Cow<'_, str> {
        cap_transcript_text(text.lines().next().unwrap_or(""))
    }

    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                let text = msg.text_content();
                lines.push(format!("tool_result({tool_name}): {}", head(&text)));
                continue;
            }
        };
        let text = msg.text_content();
        lines.push(format!("{role}: {}", head(&text)));
    }
    lines.join("\n")
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::super::summary_utils::TRANSCRIPT_MSG_MAX_CHARS;
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

    /// The truncation fallback must bound a JSON tool result.
    ///
    /// The unit has to match the shape of the content: a `ContentBlock::Json`
    /// renders through serde_json's compact formatter and contains no
    /// newlines, so "keep the first line" kept the entire payload — on exactly
    /// the message type that dominates a long agent context, and on exactly
    /// the path taken when the summarizer has already failed.
    ///
    /// Written over a payload with NO newline at all, because a fixture whose
    /// JSON happened to be pretty-printed would pass against the old code too.
    #[test]
    fn the_truncation_fallback_bounds_a_single_line_json_tool_result() {
        let payload = serde_json::json!({ "body": "x".repeat(50_000) });
        let rendered = payload.to_string();
        assert!(
            !rendered.contains('\n'),
            "fixture must be single-line or it cannot exercise the bug"
        );

        let out = deterministic_truncation(&[UnifiedMessage::tool_result_json(
            "call-1",
            "file_read",
            payload,
            false,
        )]);

        assert!(
            out.chars().count() < TRANSCRIPT_MSG_MAX_CHARS + 200,
            "fallback kept {} chars of a 50 KB single-line payload",
            out.chars().count()
        );
        assert!(out.starts_with("tool_result(file_read): "));
    }

    /// A cancelled compaction must change nothing — not the messages, and not
    /// the cache the next turn (and every later run) reads.
    ///
    /// The harness races every LLM call it makes itself against the run's
    /// cancellation token; the compaction step was awaited directly, bounded
    /// only by the 15 s summarizer timeout. Waiting that out is the visible
    /// half ("stop sometimes takes ten seconds"). The invisible half is that
    /// the compaction then *committed*: it spliced its summary and wrote it
    /// into the fingerprint cache, which `with_cache_carryover` seeds into
    /// every later run on that session key. So a stopped turn left a permanent
    /// mark on a conversation's compaction state.
    ///
    /// Asserted on the two EFFECTS rather than on "the token was consulted": a
    /// version that raced the token and then fell through to the truncation
    /// fallback would consult it and still commit, which is most of the damage.
    #[tokio::test]
    async fn a_cancelled_compaction_neither_splices_nor_caches() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        let compactor = ContextCompactor::new(
            Arc::new(MockProvider::new("Summary of earlier conversation.")),
            CompactorConfig {
                fresh_tail: 2,
                ..Default::default()
            },
        )
        .with_cancel(cancel);

        let original = make_messages(12);
        let mut messages = original.clone();
        let result = compactor
            .compact(&mut messages, 2, 0, None)
            .await
            .expect("a cancelled compaction reports, it does not error");

        assert!(
            matches!(&result.strategy_used, CompactStrategy::Skipped { reason } if reason == "cancelled"),
            "a cancelled compaction must say so, not report a summary or a \
             truncation: {:?}",
            result.strategy_used
        );
        assert_eq!(
            messages.len(),
            original.len(),
            "a cancelled compaction must not splice"
        );
        assert!(
            messages
                .iter()
                .all(|m| !m.text_content().starts_with(SUMMARY_MARKER)),
            "a cancelled compaction must not leave a summary behind"
        );
        assert!(
            compactor
                .cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "a cancelled compaction must not write the fingerprint cache — that \
             entry is seeded into every later run on this session key"
        );
    }

    /// The same call with no token wired is byte-identical to before: the
    /// subagent spawner and every test construct a compactor without one, and
    /// `None` must mean "no cancellation", never "cancelled".
    #[tokio::test]
    async fn a_compactor_without_a_token_still_compacts() {
        let compactor = ContextCompactor::new(
            Arc::new(MockProvider::new("Summary of earlier conversation.")),
            CompactorConfig {
                fresh_tail: 2,
                ..Default::default()
            },
        );
        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 2, 0, None).await.unwrap();
        assert!(
            matches!(result.strategy_used, CompactStrategy::LlmSummary),
            "an unwired compactor must behave exactly as it did before: {:?}",
            result.strategy_used
        );
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

    /// Summarizer that returns a fixed summary WITH usage, so the metering
    /// wrap has something to report (`MockProvider::text_only` carries
    /// `usage: None`, and `MeteringProvider` skips usage-less responses).
    struct UsageProvider;
    impl crate::providers::AiProvider for UsageProvider {
        fn process(
            &self,
            _payload: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                let mut resp = crate::providers::adapter::ProviderResponse::text_only(
                    "<summary>\ncompressed window\n</summary>".to_string(),
                );
                resp.usage = Some(crate::providers::adapter::TokenUsage {
                    input_tokens: 1200,
                    output_tokens: 80,
                    cache_read_tokens: Some(900),
                    cache_creation_tokens: Some(100),
                    thinking_tokens: None,
                    cost: None,
                });
                Ok(resp)
            })
        }
        fn name(&self) -> &str {
            "usage-mock"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    struct RecordingSink(crate::sync_primitives::Mutex<Vec<crate::harness::trace::LoopTraceEvent>>);
    impl crate::harness::TraceSink for RecordingSink {
        fn on_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn metered_cheap_summarizer_emits_provider_usage_labelled_compactor() {
        // Regression for the invisible-compaction-spend gap: the cheap
        // summarizer used to reach the compactor raw, so a compaction's token
        // and cache consumption never became a `ProviderUsage` row — absent
        // from the traces DB, the Panel Usage view and team rollups. Both
        // construction sites now wrap it in `MeteringProvider` under
        // `compactor:<agent>`; this test pins the composition the wiring
        // relies on.
        let sink = Arc::new(RecordingSink(
            crate::sync_primitives::Mutex::new(Vec::new()),
        ));
        let metered: Arc<dyn crate::providers::AiProvider> =
            Arc::new(crate::providers::MeteringProvider::new(
                Arc::new(UsageProvider),
                Some(sink.clone() as Arc<dyn crate::harness::TraceSink>),
                "compactor:test-agent",
            ));
        let compactor = ContextCompactor::new(
            Arc::new(MockProvider::new("main provider unused")),
            CompactorConfig::default(),
        )
        .with_cheap_provider(Some(metered));

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
        assert_eq!(result.strategy_used, CompactStrategy::LlmSummary);

        let events = sink.0.lock().unwrap_or_else(|e| e.into_inner());
        let usage_rows: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                crate::harness::trace::LoopTraceEvent::ProviderUsage {
                    agent_id,
                    cache_read_tokens,
                    ..
                } => Some((agent_id.clone(), *cache_read_tokens)),
                _ => None,
            })
            .collect();
        assert_eq!(
            usage_rows.len(),
            1,
            "one compaction through a metered summarizer = one ProviderUsage row"
        );
        assert_eq!(usage_rows[0].0, "compactor:test-agent");
        assert_eq!(usage_rows[0].1, Some(900));
    }

    #[tokio::test]
    async fn compacts_when_window_available() {
        let provider = Arc::new(MockProvider::new("Summary of earlier conversation."));
        let config = CompactorConfig::default();
        let compactor = ContextCompactor::new(provider, config);

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

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

        compactor.compact(&mut messages, 6, 0, None).await.unwrap();

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
        compactor.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert_eq!(turn1[0].text_content(), original);

        // Turn 2: rebuilt prompt (compaction is not persisted) + a new exchange.
        let mut turn2 = base.clone();
        turn2.push(UnifiedMessage::assistant("new assistant turn"));
        turn2.push(UnifiedMessage::user("new user turn"));
        let r2 = compactor.compact(&mut turn2, 6, 0, None).await.unwrap();

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
        c1.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        // Run 2: NEW compactor instance (run boundary) — seeds from the slot.
        let c2 = ContextCompactor::new(provider.clone(), CompactorConfig::default())
            .with_cache_carryover(key);
        let mut turn2 = base.clone();
        turn2.push(UnifiedMessage::assistant("new assistant turn"));
        turn2.push(UnifiedMessage::user("new user turn"));
        let r2 = c2.compact(&mut turn2, 6, 0, None).await.unwrap();
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
        c1.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert!(carryover_get(&COMPACTION_CARRYOVER, key).is_some());

        let c2 = ContextCompactor::new(provider.clone(), CompactorConfig::default())
            .with_cache_carryover(key);
        let mut rewritten = make_messages(12);
        rewritten[2] = UnifiedMessage::assistant("history rewritten between runs");
        let r2 = c2.compact(&mut rewritten, 6, 0, None).await.unwrap();
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

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
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
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

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
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );
        // 3 preserved user turns + 1 summary + 6 fresh = 10
        assert_eq!(messages.len(), 10);

        assert!(summary_text(&messages).starts_with("[Context Summary]"));
    }

    /// A truncation fallback serves this turn and commits to nothing.
    ///
    /// `store_cache` writes through to `COMPACTION_CARRYOVER`, a process-wide
    /// per-session slot that seeds every later run, and the harness rebuilds
    /// `messages` from an append-only log — so a cached truncation matches its
    /// own fingerprint forever and no later turn ever retries the summarizer.
    /// One transient 502 therefore became this conversation's permanent
    /// compaction state, silently: `CacheReuse` is excluded from the
    /// degradation warning. `SummarizerOutcome::Cancelled` was split out of
    /// `Failed` to stop exactly this permanence on the stopped-turn arm; this
    /// asserts the failed-turn arm no longer latches either.
    ///
    /// Asserted on both slots, because they fail differently: the local one
    /// keeps a broken run broken, the carry-over keeps every FUTURE run broken.
    #[tokio::test]
    async fn a_truncation_fallback_is_not_written_to_the_cache_or_the_carryover() {
        let key = "compaction-degradation-does-not-latch";
        carryover_remove(&COMPACTION_CARRYOVER, key);

        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let compactor = ContextCompactor::new(
            provider,
            CompactorConfig {
                fallback_to_truncation: true,
                ..Default::default()
            },
        )
        .with_cache_carryover(key);

        let mut messages = make_messages(12);
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation,
            "fixture must reach the fallback or it proves nothing"
        );
        assert!(
            summary_text(&messages).starts_with("[Context Summary]"),
            "the degraded summary must still serve THIS turn"
        );
        assert!(
            compactor
                .cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "a failed summarization must not become this run's cached cover"
        );
        assert!(
            carryover_get(&COMPACTION_CARRYOVER, key).is_none(),
            "a failed summarization must not be seeded into every later run on \
             this session key — the original turns are still in the session log"
        );

        carryover_remove(&COMPACTION_CARRYOVER, key);
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

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );

        let summary = summary_text(&messages);
        assert!(
            summary.contains("ORIGINAL_GOAL_MARKER") && summary.contains("step two"),
            "prior summary body must survive the truncation fallback verbatim; got:\n{summary}"
        );
        // This used to read the stored cache entry and assert the body was not
        // gutted there either. The fallback no longer stores one at all (see
        // `a_truncation_fallback_is_not_written_to_the_cache_or_the_carryover`),
        // which subsumes that check: there is no cover for a later rebuild to
        // reapply, so the next turn re-derives from the session log and retries
        // the summarizer. The spliced-summary assertion above is what carries
        // this test's own property.
        assert!(
            compactor
                .cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "a failed summarization must not become a cached cover"
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

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
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
            .compact(&mut messages, 0, 0, Some("test-session"))
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
        // (3) …and nothing survives the turn as a cover for `reapply_cached`.
        // This used to assert that the entry `store_cache` retained did not
        // contain the sentinel; the fallback path now writes no entry at all
        // (see `a_truncation_fallback_is_not_written_to_the_cache_or_the_carryover`),
        // which is the stronger form of the same statement — transient content
        // cannot outlive this turn through a cover that does not exist.
        // Assertions (1) and (2) are what still prove the window boundary,
        // which is this test's actual subject.
        assert!(
            compactor
                .cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "the truncation fallback must leave no cover for a future turn"
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
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

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
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

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

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
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

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
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

    /// Provider that always answers with a scripted outcome and counts calls —
    /// the witness for which provider a summarization actually ran on.
    struct ScriptedProvider {
        name: String,
        calls: Arc<crate::sync_primitives::Mutex<usize>>,
        outcome: std::result::Result<String, String>,
    }

    impl ScriptedProvider {
        fn ok(name: &str, text: &str) -> Self {
            Self {
                name: name.to_string(),
                calls: Arc::new(crate::sync_primitives::Mutex::new(0)),
                outcome: Ok(text.to_string()),
            }
        }
        fn failing(name: &str, raw_error: &str) -> Self {
            Self {
                name: name.to_string(),
                calls: Arc::new(crate::sync_primitives::Mutex::new(0)),
                outcome: Err(raw_error.to_string()),
            }
        }
        fn call_count(&self) -> usize {
            *self.calls.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    impl crate::providers::AiProvider for ScriptedProvider {
        fn process(
            &self,
            _payload: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + '_,
            >,
        > {
            *self.calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            let outcome = self.outcome.clone();
            Box::pin(async move {
                match outcome {
                    Ok(text) => Ok(crate::providers::adapter::ProviderResponse::text_only(text)),
                    Err(raw) => Err(crate::error::AlephError::provider(raw)),
                }
            })
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[tokio::test]
    async fn cheap_summarizer_model_not_found_falls_back_to_main_and_poisons() {
        // codex `compact_model_fallback` parity: the documented deployment
        // class is a third-party compatible proxy that does not serve the
        // preset's `default_aux_model`, so EVERY cheap summarization 404s.
        // The compactor must retry once on the main provider — and, a 404
        // being a config-level fact that cannot heal mid-run, route the rest
        // of the run straight to main instead of paying a 404 per compaction.
        let cheap = Arc::new(ScriptedProvider::failing(
            "cheap",
            "error 404: model 'aux-x' not found",
        ));
        let main = Arc::new(ScriptedProvider::ok("main", "main-summary"));
        let compactor = ContextCompactor::new(main.clone(), CompactorConfig::default())
            .with_cheap_provider(Some(cheap.clone()));

        let messages = make_messages(4);
        let s1 = compactor
            .summarize_slice(&messages, None, None)
            .await
            .unwrap();
        assert_eq!(
            s1, "main-summary",
            "the summary must come from the main provider"
        );
        assert_eq!(cheap.call_count(), 1, "the failed cheap attempt");
        assert_eq!(main.call_count(), 1, "the fallback attempt");

        // Poisoned: the next summarization goes straight to main.
        let s2 = compactor
            .summarize_slice(&messages, None, None)
            .await
            .unwrap();
        assert_eq!(s2, "main-summary");
        assert_eq!(
            cheap.call_count(),
            1,
            "a poisoned cheap tier must not be retried within the run"
        );
        assert_eq!(main.call_count(), 2);
    }

    #[tokio::test]
    async fn transient_cheap_failure_retries_main_without_poisoning() {
        // A network blip is worth one fallback attempt but must NOT poison:
        // the cheap tier is healthy again a turn later, and muting it for the
        // rest of the run would silently bill every later summarization to the
        // main model.
        let cheap = Arc::new(ScriptedProvider::failing(
            "cheap",
            "connection reset by peer",
        ));
        let main = Arc::new(ScriptedProvider::ok("main", "main-summary"));
        let compactor = ContextCompactor::new(main.clone(), CompactorConfig::default())
            .with_cheap_provider(Some(cheap.clone()));

        let messages = make_messages(4);
        let _ = compactor
            .summarize_slice(&messages, None, None)
            .await
            .unwrap();
        let _ = compactor
            .summarize_slice(&messages, None, None)
            .await
            .unwrap();
        assert_eq!(cheap.call_count(), 2, "transient failure must not poison");
        assert_eq!(main.call_count(), 2, "each failure earns one main retry");
    }

    #[tokio::test]
    async fn fatal_cheap_failure_does_not_fall_back() {
        // A 400-class error means the REQUEST is broken — retrying it verbatim
        // on the main provider would fail identically, so the compactor must
        // skip the fallback and take the deterministic-truncation path.
        let cheap = Arc::new(ScriptedProvider::failing(
            "cheap",
            "400 Bad Request: invalid request",
        ));
        let main = Arc::new(ScriptedProvider::ok("main", "main-summary"));
        let compactor = ContextCompactor::new(main.clone(), CompactorConfig::default())
            .with_cheap_provider(Some(cheap.clone()));

        let messages = make_messages(4);
        let seed = compactor
            .summarize_slice(&messages, None, None)
            .await
            .unwrap();
        assert_eq!(main.call_count(), 0, "fatal errors must not be retried");
        assert_eq!(cheap.call_count(), 1);
        assert!(
            seed.contains("User message 0"),
            "deterministic truncation fallback should still produce a summary, got: {seed}"
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
        let seed = compactor
            .summarize_slice(&messages, None, None)
            .await
            .unwrap();

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
        let r1 = compactor.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert_eq!(r1.strategy_used, CompactStrategy::LlmSummary);
        assert_eq!(provider.call_count(), 1);

        // Turn 2: rebuilt prompt = same history + one new exchange.
        let mut turn2 = base.clone();
        turn2.push(UnifiedMessage::assistant("new assistant turn"));
        turn2.push(UnifiedMessage::user("new user turn"));
        let r2 = compactor.compact(&mut turn2, 6, 0, None).await.unwrap();

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
        compactor.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        // Turn N: the un-summarized gap behind the summary has grown past the
        // extension threshold → exactly one merge call that feeds the previous
        // summary explicitly as prior state (incremental "update", not a fresh
        // re-summarization of the already-condensed head).
        let mut turn2 = base.clone();
        for i in 0..(CACHE_EXTEND_MIN_MESSAGES + 2) {
            turn2.push(UnifiedMessage::assistant(format!("extra turn {i}")));
        }
        let r2 = compactor.compact(&mut turn2, 6, 0, None).await.unwrap();

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
        let r3 = compactor.compact(&mut turn3, 6, 0, None).await.unwrap();
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
        compactor.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert_eq!(provider.call_count(), 1);

        // A preflight pass rewrote a message inside the covered range — the
        // fingerprint must miss and a full recompaction must run.
        let mut turn2 = base.clone();
        turn2[2] = UnifiedMessage::user("rewritten by a cheap pass");
        turn2.push(UnifiedMessage::assistant("another turn"));
        turn2.push(UnifiedMessage::user("another question"));
        let r2 = compactor.compact(&mut turn2, 6, 0, None).await.unwrap();

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
            .compact(&mut turn1, 6, 0, Some("sess-1"))
            .await
            .unwrap();
        assert_eq!(r1.strategy_used, CompactStrategy::SessionMemoryReuse);

        // Turn 2: the harness rebuilds the same history — the cached summary
        // must reapply via the zero-cost fast path, not recompact.
        let mut turn2 = base.clone();
        let r2 = compactor
            .compact(&mut turn2, 6, 0, Some("sess-1"))
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

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
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
        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();

        assert!(matches!(
            result.strategy_used,
            CompactStrategy::Skipped { reason } if reason.contains("already compacted")
        ));
        assert_eq!(messages.len(), original_len);
    }

    /// A `scratchpad` tool result carrying an unfinished execution list — the
    /// thing `plan_carry` re-emits below the summary, and therefore the reason
    /// a drain can insert more messages than `preserved.len() + 1`.
    fn unfinished_plan_result(call_id: &str) -> UnifiedMessage {
        UnifiedMessage::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: "scratchpad".into(),
            content: vec![ContentBlock::Json {
                value: serde_json::json!({
                    "snapshot": {
                        "objective": "ship the importer",
                        "items": [
                            {"text": "write the parser", "status": "completed"},
                            {"text": "wire the writer", "status": "pending"},
                        ]
                    }
                }),
            }],
            is_error: false,
        }
    }

    fn tool_call_msg(call_id: &str, name: &str, args: serde_json::Value) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                thought_signature: None,
                id: call_id.into(),
                name: name.into(),
                arguments: args,
            }],
        }
    }

    #[tokio::test]
    async fn the_protected_tail_counts_transient_scaffolding_on_top_of_the_conversation() {
        // §2.18 taught `PreflightPipeline` that the vector it rewrites ends with
        // up to five entries that were never persisted — four `<system-reminder>`
        // nudges plus the recall strand. The compactor was never told. With a
        // configured `fresh_tail` of 6 and all five firing, exactly ONE real turn
        // stayed verbatim and the summarizer swallowed the five the model had
        // just been shown.
        //
        // Forced-failure provider + truncation fallback, so the summary is a
        // deterministic function of the drained window: any sentinel that reaches
        // it was drained.
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let compactor = ContextCompactor::new(
            provider,
            CompactorConfig {
                fallback_to_truncation: true,
                ..Default::default() // fresh_tail: 6
            },
        );

        let mut messages: Vec<UnifiedMessage> = (0..12)
            .map(|i| UnifiedMessage::user(format!("PERSISTED_{i}")))
            .collect();
        for i in 0..5 {
            messages.push(UnifiedMessage::user(format!(
                "<system-reminder>\nsynthetic nudge {i}\n</system-reminder>"
            )));
        }
        let protected: Vec<String> = messages[6..]
            .iter()
            .map(UnifiedMessage::text_content)
            .collect();

        let result = compactor.compact(&mut messages, 0, 5, None).await.unwrap();
        assert_eq!(
            result.strategy_used,
            CompactStrategy::DeterministicTruncation
        );

        let kept: Vec<String> = messages[messages.len() - protected.len()..]
            .iter()
            .map(UnifiedMessage::text_content)
            .collect();
        assert_eq!(
            kept, protected,
            "six persisted turns AND five transient entries must survive verbatim"
        );
        let summary = summary_text(&messages);
        for i in 6..12 {
            assert!(
                !summary.contains(&format!("PERSISTED_{i}")),
                "the transient tail must not spend the conversation's protection budget; \
                 PERSISTED_{i} was drained into:\n{summary}"
            );
        }
    }

    #[tokio::test]
    async fn the_extend_merge_feeds_the_whole_gap_when_a_carrier_rides_below_the_summary() {
        // `reapply_cached` derived the gap's coordinates from a hand-rolled
        // `preserved.len() + 1`, but `splice_preserved` also inserts the carried
        // artifacts below the summary. Every window still holding an unfinished
        // execution list therefore shifted the gap by one: the NEWEST gap message
        // never reached the summarizer, while `store_cache` recorded a cover that
        // included it — so on the next rebuild the cached summary replaced a
        // message that no summary had ever seen. Silent, permanent context loss.
        let provider = Arc::new(CapturingProvider::new(
            "<summary>\n## Progress\nmerged\n</summary>",
        ));
        let compactor = ContextCompactor::new(provider.clone(), CompactorConfig::default());

        // Prefix the cached summary will cover: a user turn, a scratchpad call,
        // its unfinished-plan result, then filler.
        let prefix = |()| -> Vec<UnifiedMessage> {
            let mut v = vec![
                UnifiedMessage::user("ORIGINAL TASK: build the importer"),
                tool_call_msg(
                    "c0",
                    "scratchpad",
                    serde_json::json!({"action": "set_plan"}),
                ),
                unfinished_plan_result("c0"),
            ];
            for i in 0..7 {
                v.push(UnifiedMessage::assistant(format!("PREFIX_{i}")));
            }
            v
        };
        let tail = |()| -> Vec<UnifiedMessage> {
            (0..6)
                .map(|i| UnifiedMessage::user(format!("TAIL_{i}")))
                .collect()
        };

        // Turn 1 — full compaction, stores the cache over [0, 10).
        let mut turn1 = prefix(());
        turn1.extend(tail(()));
        let r1 = compactor.compact(&mut turn1, 6, 0, None).await.unwrap();
        assert_eq!(r1.strategy_used, CompactStrategy::LlmSummary);

        // Turn 2 — same prefix rebuilt from the log, plus ten new turns behind
        // the cached summary. That gap is over `CACHE_EXTEND_MIN_MESSAGES`, so
        // the extend-merge runs.
        let mut turn2 = prefix(());
        for i in 0..10 {
            turn2.push(UnifiedMessage::assistant(format!("GAP_{i}")));
        }
        turn2.extend(tail(()));
        let r2 = compactor.compact(&mut turn2, 6, 0, None).await.unwrap();
        assert_eq!(
            r2.strategy_used,
            CompactStrategy::LlmSummary,
            "the widened gap must trigger the extend-merge, not a bare cache reuse"
        );

        let prompt = provider.prompt();
        assert!(
            prompt.contains("GAP_9"),
            "the newest gap message must reach the summarizer; got:\n{prompt}"
        );
        assert!(
            !prompt.contains("[Execution list preserved across context compaction]"),
            "the carrier is not gap content and must not be re-summarized; got:\n{prompt}"
        );
        // And the carrier itself is still in the prompt the model will see.
        let carried = turn2.iter().any(|m| {
            m.text_content()
                .contains("[Execution list preserved across context compaction]")
        });
        assert!(
            carried,
            "the execution list must ride forward past the merge"
        );
    }

    #[tokio::test]
    async fn a_compaction_hands_the_file_ledger_forward() {
        // pi appends `<read-files>` / `<modified-files>` to the summary text.
        // Aleph carries them instead, so they survive the paths where no
        // summarizer ran at all — here the truncation fallback.
        let provider =
            Arc::new(MockProvider::new("ignored").with_error(MockError::Provider("fail".into())));
        let compactor = ContextCompactor::new(
            provider,
            CompactorConfig {
                fallback_to_truncation: true,
                ..Default::default()
            },
        );

        let mut messages = vec![
            UnifiedMessage::user("port the store"),
            tool_call_msg(
                "c1",
                "file_edit",
                serde_json::json!({"file_path": "src/store.rs"}),
            ),
            UnifiedMessage::tool_result("c1", "file_edit", "1 edit applied", false),
            tool_call_msg(
                "c2",
                "file_read",
                serde_json::json!({"path": "src/types.rs"}),
            ),
            UnifiedMessage::tool_result("c2", "file_read", "pub struct T;", false),
        ];
        for i in 0..8 {
            messages.push(UnifiedMessage::assistant(format!("step {i}")));
        }

        compactor.compact(&mut messages, 6, 0, None).await.unwrap();

        let ledger = messages
            .iter()
            .map(UnifiedMessage::text_content)
            .find(|t| t.contains("[Files touched, preserved across context compaction]"))
            .expect("the drain must hand the file ledger forward");
        assert!(ledger.contains("M src/store.rs"), "got:\n{ledger}");
        assert!(ledger.contains("R src/types.rs"), "got:\n{ledger}");
    }

    #[tokio::test]
    async fn the_zero_cost_reuse_path_carries_what_every_other_drain_carries() {
        // The session-memory reuse path is the FIFTH drain site. It re-attached
        // the user's verbatim turns and then dropped everything else the other
        // four hand forward — so the one path that costs nothing was the one path
        // where the model lost its own checklist and its file ledger.
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
        use crate::memory::store::sqlite::SqliteMemoryBackend;

        let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let raw = RawMemory::new(
            "Earlier turns: scaffolded the importer.".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_agent("agent-x")
        .with_session("sess-reuse")
        .with_path("aleph://session/sess-reuse/d0/0");
        backend.insert_raw_memory(&raw).await.unwrap();

        let provider = Arc::new(MockProvider::new("unused"));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default())
            .with_summary_reuse(backend, "agent-x");

        let mut messages = vec![
            UnifiedMessage::user("ORIGINAL TASK: build the importer"),
            tool_call_msg(
                "c0",
                "scratchpad",
                serde_json::json!({"action": "set_plan"}),
            ),
            unfinished_plan_result("c0"),
            tool_call_msg(
                "c1",
                "file_edit",
                serde_json::json!({"file_path": "src/store.rs"}),
            ),
            UnifiedMessage::tool_result("c1", "file_edit", "1 edit applied", false),
        ];
        for i in 0..7 {
            messages.push(UnifiedMessage::assistant(format!("PREFIX_{i}")));
        }
        for i in 0..6 {
            messages.push(UnifiedMessage::user(format!("TAIL_{i}")));
        }

        let result = compactor
            .compact(&mut messages, 6, 0, Some("sess-reuse"))
            .await
            .unwrap();
        assert_eq!(result.strategy_used, CompactStrategy::SessionMemoryReuse);

        let rendered: Vec<String> = messages.iter().map(UnifiedMessage::text_content).collect();
        let user_turn = rendered
            .iter()
            .position(|t| t.contains("ORIGINAL TASK"))
            .expect("the user's own turn is re-attached");
        let summary = rendered
            .iter()
            .position(|t| t.starts_with("[Context Summary (from session memory)]"))
            .expect("the reuse summary is inserted");
        let plan = rendered
            .iter()
            .position(|t| t.contains("[Execution list preserved across context compaction]"))
            .expect("the execution list must ride the zero-cost path too");
        let files = rendered
            .iter()
            .position(|t| t.contains("[Files touched, preserved across context compaction]"))
            .expect("the file ledger must ride the zero-cost path too");

        assert!(
            user_turn < summary && summary < plan && plan < files,
            "order must match every other drain: user turns, summary, then carriers \
             (got user={user_turn} summary={summary} plan={plan} files={files})"
        );
    }

    /// The image the preflight stage deliberately protects must survive the
    /// drain that runs immediately after it on the same vector.
    ///
    /// `HistoricalImageStrippingStage` keeps the newest screenshot and replaces
    /// the older ones with placeholders; compaction then drains a head-anchored
    /// window containing it, and nothing carried it out —
    /// `preserved_user_messages` is text-only by contract and `text_content`
    /// skips images. So a desktop run compacted *because of* its screenshots
    /// and lost every one of them, and the stripping stage's own test stayed
    /// green because it only ever measured its own stage.
    ///
    /// Asserted on the pixels reaching the post-compaction vector rather than
    /// on `image_carry_message` being called: a carrier wired into four of the
    /// five drain sites would satisfy the latter.
    #[tokio::test]
    async fn the_live_screenshot_survives_a_compaction() {
        let compactor = ContextCompactor::new(
            Arc::new(MockProvider::new("Summary of earlier conversation.")),
            CompactorConfig::default(),
        );

        let mut messages = vec![UnifiedMessage::User {
            content: vec![
                ContentBlock::Text {
                    text: "here is the screen".to_string(),
                    cache_control: None,
                },
                ContentBlock::Image {
                    data: "LIVE_PIXELS".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ],
        }];
        for i in 0..14 {
            messages.push(UnifiedMessage::assistant(format!("step {i}")));
        }

        let result = compactor.compact(&mut messages, 6, 0, None).await.unwrap();
        assert_eq!(
            result.strategy_used,
            CompactStrategy::LlmSummary,
            "fixture must take a real drain path or it proves nothing"
        );

        let survived = messages
            .iter()
            .flat_map(UnifiedMessage::content_blocks)
            .any(|b| matches!(b, ContentBlock::Image { data, .. } if data == "LIVE_PIXELS"));
        assert!(
            survived,
            "compaction deleted the screen the model is about to act on"
        );
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

    // `cap_transcript_text`'s own unit tests moved with it to `summary_utils`,
    // where the cap is now single-sourced for all three drain sites.

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

        compactor.compact(&mut messages, 2, 0, None).await.unwrap();

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
