//! `MeteringProvider` — decorator that emits `LoopTraceEvent::ProviderUsage`
//! after each `process()` call (Stage J-pre cache observability pipeline).
//!
//! Decorator-only: no harness diff. Composes with any `AiProvider` (anthropic,
//! mock, failover, etc.). Non-Anthropic providers will populate `cache_*` as
//! `None` until their protocols extend.
//!
//! See: docs/superpowers/plans/2026-05-09-subagent-uplift-stage-j-pre-plan.md
//!
//! # The spend floor
//!
//! This is also the single funnel every LLM call in the process passes
//! through — `record_usage` is shared by `process` and
//! `execute_streaming_dyn`, and sub-agent/MoA-advisor/compactor spend each
//! wrap their own `MeteringProvider` rather than reuse the parent run's (the
//! source comment on those call sites says wrapping again "would
//! double-count"). That makes this the one place that sees *every* dollar,
//! including the three kinds of call the run-admission gate (Task 7 of the
//! per-principal spend plan) structurally cannot see: a sub-agent drives
//! `AgentHarness::run` directly without building a `FlowRequest`, and a MoA
//! advisor or a compactor call is never folded into the parent run's
//! `token_breakdown` at all.
//!
//! `enforce_spend_ceiling` runs first inside each arm's `Box::pin(async move
//! { .. })`, before `fut.await` — denying there means the request never
//! leaves the box. `record_usage` prices and records every response that
//! carries usage, via the same `spend::check`/ledger the admission arm
//! reads. See `spend::mod`'s doc for the ledger/policy process-global split
//! and why it is not threaded through [`MeteringProvider::new`].

use crate::error::Result;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

pub struct MeteringProvider {
    inner: Arc<dyn AiProvider>,
    sink: Option<Arc<dyn TraceSink>>,
    agent_id: String,
}

impl MeteringProvider {
    pub fn new(
        inner: Arc<dyn AiProvider>,
        sink: Option<Arc<dyn TraceSink>>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            sink,
            agent_id: agent_id.into(),
        }
    }
}

impl MeteringProvider {
    /// Emit the `ProviderUsage` trace event + cache-monitor feed + usage log for
    /// one response. Shared by the streaming and non-streaming paths so metering
    /// is byte-identical on both — the streaming path previously bypassed this
    /// decorator entirely, so streamed turns produced no `ProviderUsage` at all.
    /// The cache-watchdog key for a request: `(agent, session)`, because the
    /// provider's prompt-cache prefix is per conversation. `session_id` rides
    /// the payload metadata that `harness::agent::think::build_request_payload`
    /// stamps on every call, and it is the serialized `SessionKey` — the same
    /// string the compaction side resets under.
    fn cache_scope_of(payload: &RequestPayload<'_>, agent_id: &str) -> String {
        crate::thinker::prompt_builder::cache_monitor::cache_scope(
            agent_id,
            payload
                .metadata
                .as_ref()
                .and_then(|m| m.get("session_id"))
                .map(String::as_str),
        )
    }

    fn record_usage(
        resp: &ProviderResponse,
        sink: &Option<Arc<dyn TraceSink>>,
        agent_id: &str,
        provider_name: &str,
        cache_scope: &str,
        prefix_hash: Option<u64>,
        pricing_provider: &str,
        pricing_model: &str,
    ) {
        let Some(usage) = resp.usage.as_ref() else {
            return;
        };
        tracing::info!(
            target: "aleph::provider_usage",
            agent_id = %agent_id,
            provider = %provider_name,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_read_tokens = ?usage.cache_read_tokens,
            cache_creation_tokens = ?usage.cache_creation_tokens,
            cache_hit_ratio = ?usage.cache_hit_ratio(),
            thinking_tokens = ?usage.thinking_tokens,
            "LLM call completed"
        );
        // Cache-first observability: feed cache token counts into the
        // process-wide `CacheMonitor`, keyed per prompt-cache prefix. Three
        // consecutive misses (counted only once that prefix has seen real cache
        // activity) with more than three total calls triggers a warn — surfaces
        // accidental stable-prefix changes that would otherwise only show up on
        // the bill. On that rising edge the monitor hands back a report, which
        // becomes a `LoopTraceEvent::CacheHealthDegraded` on the same trace
        // stream as `ProviderUsage` — lifting the domain's only alarm out of
        // the log and onto the TUI / Panel / doctor surfaces.
        let degradation = crate::thinker::prompt_builder::cache_monitor::global_cache_monitor()
            .record_cache_usage(
                cache_scope,
                usage.cache_read_tokens,
                usage.cache_creation_tokens,
                prefix_hash,
            );
        if let Some(sink) = sink {
            sink.on_trace(&LoopTraceEvent::ProviderUsage {
                agent_id: agent_id.to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_creation_tokens: usage.cache_creation_tokens,
                thinking_tokens: usage.thinking_tokens,
            });
            if let Some(report) = degradation {
                sink.on_trace(&LoopTraceEvent::CacheHealthDegraded {
                    scope: report.scope,
                    streak: report.streak,
                    reads: report.reads,
                    writes: report.writes,
                    prefix_changed: report.prefix_changed,
                });
            }
        }
        Self::record_spend(usage, pricing_provider, pricing_model);
    }

    /// Deny before the call ever leaves the box. The first statement inside
    /// each arm's `Box::pin(async move { .. })`, before `fut.await` —
    /// `self.inner.process(req)` only *builds* the future; awaiting it is
    /// what makes the network call, so a denial here means the request is
    /// never sent. See this file's module doc for why this floor arm exists
    /// alongside run admission.
    ///
    /// Bounded overshoot by design: this checks BEFORE the call, and
    /// `record_usage` records AFTER it completes, so a principal can exceed
    /// the ceiling by at most the cost of calls already in flight at the
    /// moment they crossed it. The alternative — reserving an estimate up
    /// front — would need a refund on every error path, and a single missed
    /// refund silently shrinks someone's ceiling forever; a bounded
    /// overshoot is the cheaper failure to accept.
    fn enforce_spend_ceiling() -> Result<()> {
        let principal = crate::spend::ambient_principal();
        let now_ms = chrono::Utc::now().timestamp_millis();
        match crate::spend::check(&principal, now_ms) {
            crate::spend::Verdict::Allowed(_) => Ok(()),
            crate::spend::Verdict::Denied { limit, spent } => Err(crate::error::AlephError::provider(
                spend_denied_message(&limit, &spent),
            )),
        }
    }

    /// Price this call and fold it into the process-wide spend ledger.
    ///
    /// `pricing_provider`/`pricing_model` must be the caller's
    /// `serving_provider_hint()` / `serving_model_hint()` — never
    /// `provider_name` (`self.inner.name()`): every production `inner` is a
    /// `FailoverProvider` (optionally wrapped), so `name()` is the literal
    /// `"failover"`, a key the price table does not know. See
    /// `orchestrator::harness_bridge::runner_impl`'s `cost_provider` /
    /// `gauge_model` resolution, which paid for this exact trap once
    /// already for the turn-summary cost estimate.
    ///
    /// Reads the process-wide policy and returns without touching the
    /// ledger at all when it is disabled — an unconfigured box writes no
    /// ledger rows on the write side, exactly as `spend::check_with`
    /// already guarantees on the read side (see that function's
    /// `!policy.enabled()` arm).
    fn record_spend(usage: &crate::providers::adapter::TokenUsage, pricing_provider: &str, pricing_model: &str) {
        let policy = crate::spend::current_policy();
        if !policy.enabled() {
            return;
        }
        let principal = crate::spend::ambient_principal();
        if let Err(error) = Self::record_spend_with(
            usage,
            pricing_provider,
            pricing_model,
            &principal,
            &policy,
            &*crate::spend::global_ledger(),
        ) {
            tracing::error!(
                %error,
                principal = principal.as_key(),
                "MeteringProvider::record_spend: SpendLedger::record failed; this call's cost \
                 is not reflected in the spend ledger",
            );
        }
    }

    /// The actual `CostStatus` → `Delta` mapping and ledger write, with
    /// `principal`/`policy`/`ledger` taken as explicit parameters instead of
    /// read from the process globals — same split, for the same reason, as
    /// `spend::check`/`check_with`: `install_ledger`/`install_policy` are
    /// `OnceLock`s set once for the life of the process, and every unit test
    /// in this crate shares one process. Tests call this directly with a
    /// freshly-built `InMemorySpendLedger`, so nothing here races a sibling
    /// test's global install.
    fn record_spend_with(
        usage: &crate::providers::adapter::TokenUsage,
        pricing_provider: &str,
        pricing_model: &str,
        principal: &crate::spend::Principal,
        policy: &crate::config::types::policies::SpendPolicy,
        ledger: &dyn crate::spend::SpendLedger,
    ) -> anyhow::Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let period_start_ms = crate::spend::period::period_start_ms(now_ms, policy.period);
        let breakdown = crate::orchestrator::dispatch::TokenBreakdown::from(usage);
        let estimate = crate::pricing::estimate(pricing_provider, pricing_model, &breakdown);
        // A missing price never denies a call: `Unknown` counts the call
        // but leaves `usd` untouched, so it can never — by itself — push a
        // principal over a ceiling (G3). `PartialMissingPrice`'s figure is
        // a lower bound, still measured money, so it accumulates the same
        // way `Complete` does; it is only counted separately so a reader
        // can tell "priced" from "guessed" apart from "priced in full".
        let delta = match estimate.status {
            crate::pricing::CostStatus::Complete => crate::spend::Delta::Usd(estimate.usd),
            crate::pricing::CostStatus::PartialMissingPrice => crate::spend::Delta::Partial(estimate.usd),
            crate::pricing::CostStatus::Unknown => crate::spend::Delta::Unpriced,
        };
        ledger.record(principal, period_start_ms, delta)
    }
}

/// Render a `Verdict::Denied` into text a model can read and self-heal from
/// (A2): it names which ceiling fired and when it resets, so "what do I do
/// now" is answerable without a second round trip.
///
/// `Limit::Total` renders no numbers, by the shape of that variant: see its
/// doc for why it is deliberately fieldless — there is no field here it
/// could ride the machine-wide figure through without defeating the reason
/// `Limit::Total` carries none of its own.
fn spend_denied_message(limit: &crate::spend::Limit, spent: &crate::spend::Spent) -> String {
    let reset = spent
        .period_end_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "the next period".to_string());
    match limit {
        crate::spend::Limit::PerUser {
            spent: spent_usd,
            limit: limit_usd,
        } => format!(
            "Spend ceiling reached: ${spent_usd:.2} spent against a ${limit_usd:.2} per-user \
             limit for this period. Resets at {reset}."
        ),
        crate::spend::Limit::Total => format!(
            "Spend ceiling reached: this machine's shared spend limit for the period has been \
             reached (your own spend this period: ${:.2}). Resets at {reset}.",
            spent.usd
        ),
    }
}

impl AiProvider for MeteringProvider {
    fn process<'a>(
        &'a self,
        req: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let scope = Self::cache_scope_of(&req, &self.agent_id);
        let prefix_hash = crate::thinker::prompt_builder::cache_monitor::stable_prefix_hash(&req);
        let fut = self.inner.process(req);
        let sink = self.sink.clone();
        let agent_id = self.agent_id.clone();
        let provider_name = self.inner.name().to_string();
        let pricing_provider = self
            .inner
            .serving_provider_hint()
            .map_or_else(|| provider_name.clone(), Cow::into_owned);
        let pricing_model = self.inner.serving_model_hint().map_or_else(String::new, Cow::into_owned);
        Box::pin(async move {
            Self::enforce_spend_ceiling()?;
            let resp = fut.await?;
            Self::record_usage(
                &resp,
                &sink,
                &agent_id,
                &provider_name,
                &scope,
                prefix_hash,
                &pricing_provider,
                &pricing_model,
            );
            Ok(resp)
        })
    }

    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        stream_sink: &'a dyn crate::providers::DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Delegate the actual streaming to the inner provider, then meter its
        // assembled response identically to `process`. This is the fix for the
        // streaming metering gap: the same `ProviderUsage` pipeline now fires on
        // streamed turns instead of being skipped by the harness downcast.
        let scope = Self::cache_scope_of(&payload, &self.agent_id);
        let prefix_hash =
            crate::thinker::prompt_builder::cache_monitor::stable_prefix_hash(&payload);
        let fut = self.inner.execute_streaming_dyn(payload, stream_sink);
        let sink = self.sink.clone();
        let agent_id = self.agent_id.clone();
        let provider_name = self.inner.name().to_string();
        let pricing_provider = self
            .inner
            .serving_provider_hint()
            .map_or_else(|| provider_name.clone(), Cow::into_owned);
        let pricing_model = self.inner.serving_model_hint().map_or_else(String::new, Cow::into_owned);
        Box::pin(async move {
            Self::enforce_spend_ceiling()?;
            let resp = fut.await?;
            Self::record_usage(
                &resp,
                &sink,
                &agent_id,
                &provider_name,
                &scope,
                prefix_hash,
                &pricing_provider,
                &pricing_model,
            );
            Ok(resp)
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn protocol(&self) -> Cow<'_, str> {
        self.inner.protocol()
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.inner.model_behavior_override()
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.behavior_hint()
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.serving_model_hint()
    }

    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.serving_provider_hint()
    }

    fn as_http_provider(&self) -> Option<&crate::providers::http_provider::HttpProvider> {
        self.inner.as_http_provider()
    }

    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::TokenUsage;
    use crate::spend::SpendLedger as _;
    use crate::sync_primitives::Mutex;

    struct FakeProvider {
        usage: TokenUsage,
    }
    impl AiProvider for FakeProvider {
        fn process<'a>(
            &'a self,
            _req: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            let usage = self.usage.clone();
            Box::pin(async move {
                Ok(ProviderResponse {
                    usage: Some(usage),
                    ..Default::default()
                })
            })
        }
        fn name(&self) -> &str {
            "fake"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    struct CapturingSink(Mutex<Vec<LoopTraceEvent>>);
    impl TraceSink for CapturingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn emits_provider_usage_with_agent_id_and_full_token_split() {
        let inner = Arc::new(FakeProvider {
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 50,
                cache_read_tokens: Some(150),
                cache_creation_tokens: Some(20),
                thinking_tokens: None,
                cost: None,
            },
        });
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            inner,
            Some(sink.clone() as Arc<dyn TraceSink>),
            "subagent-test",
        );

        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let req = RequestPayload::new(&msgs);
        let _ = metering.process(req).await.expect("process");

        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            LoopTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                assert_eq!(agent_id, "subagent-test");
                assert_eq!(*input_tokens, 200);
                assert_eq!(*cache_read_tokens, Some(150));
                assert_eq!(*cache_creation_tokens, Some(20));
            }
            other => panic!("expected ProviderUsage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn degraded_streak_emits_cache_health_event_into_sink() {
        // The alarm-effect assertion: when the watchdog's streak crosses the
        // threshold, a `CacheHealthDegraded` event must actually reach the
        // trace sink (→ task_traces, wire, doctor) — not just a log line.
        // Rising edge only: 4 re-creating calls = 4 ProviderUsage + exactly 1
        // degraded event. Unique agent id: the monitor is a process-wide
        // singleton shared with every other test in this binary.
        let inner = Arc::new(FakeProvider {
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: Some(0),
                cache_creation_tokens: Some(1000),
                thinking_tokens: None,
                cost: None,
            },
        });
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            inner,
            Some(sink.clone() as Arc<dyn TraceSink>),
            "cache-degraded-sink-test",
        );
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        for _ in 0..4 {
            let _ = metering
                .process(RequestPayload::new(&msgs))
                .await
                .expect("process");
        }

        let events = sink.0.lock().unwrap();
        let degraded: Vec<&LoopTraceEvent> = events
            .iter()
            .filter(|e| matches!(e, LoopTraceEvent::CacheHealthDegraded { .. }))
            .collect();
        assert_eq!(
            degraded.len(),
            1,
            "rising edge only — total events: {}",
            events.len()
        );
        match degraded[0] {
            LoopTraceEvent::CacheHealthDegraded {
                scope,
                streak,
                writes,
                ..
            } => {
                assert_eq!(scope, "cache-degraded-sink-test");
                assert_eq!(*streak, 4);
                assert_eq!(*writes, 1000);
            }
            other => panic!("expected CacheHealthDegraded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_event_when_response_lacks_usage() {
        struct EmptyProvider;
        impl AiProvider for EmptyProvider {
            fn process<'a>(
                &'a self,
                _req: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::default()) })
            }
            fn name(&self) -> &str {
                "empty"
            }
            fn color(&self) -> &str {
                "#000"
            }
        }
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            Arc::new(EmptyProvider),
            Some(sink.clone() as Arc<dyn TraceSink>),
            "x",
        );
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let _ = metering.process(RequestPayload::new(&msgs)).await.unwrap();
        assert!(sink.0.lock().unwrap().is_empty());
    }

    // ========================================================================
    // Spend floor — Task 6 of the per-principal spend plan.
    //
    // `record_spend_with` and `spend::check_with` take `policy`/`ledger` as
    // explicit parameters instead of the process globals, so these tests
    // build their own fresh `InMemorySpendLedger` per test and never call
    // `spend::install_ledger`/`install_policy` — see `spend::check_with`'s
    // own doc for why racing a process-wide `OnceLock` across every test in
    // this binary is a hazard, not a convenience. G5 and the denial test
    // below are the exception: they must exercise the real
    // `process()`/`execute_streaming_dyn()` entry points to prove the
    // *wiring*, so they share one idempotently-installed ledger via
    // `install_test_spend_globals`, isolated per test by a distinct
    // `Principal` (see that function's doc for why sharing it is safe).
    // ========================================================================

    fn spend_test_usage(input: u32, output: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        }
    }

    /// G3 — a call whose provider/model miss the price table entirely
    /// (`CostStatus::Unknown`) leaves `usd` at exactly `0.0` and increments
    /// `unpriced_calls` — never the other way around, and never both.
    #[test]
    fn g3_unknown_estimate_leaves_usd_at_zero_and_counts_as_unpriced() {
        let ledger = crate::spend::InMemorySpendLedger::default();
        let policy = crate::config::types::policies::SpendPolicy {
            per_user_usd: Some(1.0),
            ..Default::default()
        };
        let principal = crate::spend::Principal::User("u-metering-g3".to_string());
        let usage = spend_test_usage(1_000, 1_000);

        MeteringProvider::record_spend_with(
            &usage,
            "totally-unknown-provider",
            "totally-unknown-model",
            &principal,
            &policy,
            &ledger,
        )
        .expect("record");

        let period_start_ms = crate::spend::period::period_start_ms(
            chrono::Utc::now().timestamp_millis(),
            policy.period,
        );
        let spent = ledger.spent_for(&principal, period_start_ms).unwrap();
        assert_eq!(spent.usd, 0.0, "an unpriced call must never move usd");
        assert_eq!(spent.unpriced_calls, 1);
        assert_eq!(spent.partial_calls, 0);
    }

    /// G3's structural twin. G3 above proves the *behaviour* for today's
    /// price table: an unpriced call currently happens to carry
    /// `estimate.usd == 0.0`, so `spent.usd == 0.0` after recording it
    /// proves nothing about the *mapping* — a future change to
    /// `pricing::estimate` that starts returning a nonzero "best guess" `usd`
    /// alongside `CostStatus::Unknown` would sail straight past G3 as long
    /// as `record_spend_with` still fed that guess into `Delta::Usd`.
    ///
    /// This pins the *source shape* instead: `CostStatus::Unknown` must map
    /// to the fieldless `Delta::Unpriced` variant at the one production
    /// site that performs this mapping — never `Delta::Usd`/`Delta::Partial`,
    /// which both carry a `usd` figure. Once that arm reads
    /// `Delta::Unpriced`, "an Unknown estimate never moves a dollar" stops
    /// being a fact about today's price table and becomes a fact the type
    /// checker enforces (`Delta::Unpriced` has no field to carry `usd` in —
    /// see its doc). That is the property `pricing.rs`'s module doc
    /// promises and the reason it is written the way it is.
    #[test]
    fn cost_status_unknown_has_no_source_path_to_a_priced_delta() {
        let src = include_str!("metering.rs").replace('\r', "");
        // CRLF-safe (this repo's Windows checkout uses \r\n) and unanchored
        // on purpose: an anchored "\n#[cfg(test)]" needle matches nothing
        // once \r is stripped from a line that had it, silently turning
        // "production" into the whole file — see CLAUDE.md §10 for the
        // documented failure this avoids.
        let production = src.split("#[cfg(test)]").next().unwrap_or(&src);
        assert!(
            production.len() < src.len(),
            "the #[cfg(test)] split matched nothing — this test would be \
             reading its own source, and the assertions below would be \
             checking their own doc comments instead of production code"
        );
        // Strip `//` line comments before scanning: this file's own doc
        // comments spell out `CostStatus::Unknown => crate::spend::Delta::Unpriced`
        // in prose (see `record_spend_with`'s doc), and a naive `contains`
        // would be satisfied by that sentence even if the match arm below
        // it read something else — the bug's own explanation becoming its
        // only search hit.
        let production: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            production.contains("CostStatus::Unknown => crate::spend::Delta::Unpriced"),
            "CostStatus::Unknown must map to the fieldless Delta::Unpriced \
             variant at record_spend_with's match — anything else gives an \
             unpriced call a `usd` figure to carry into the ledger, which \
             is exactly what pricing.rs's module doc says pricing must \
             never do once it feeds a spend ceiling"
        );
        assert!(
            !production.contains("CostStatus::Unknown => crate::spend::Delta::Usd")
                && !production.contains("CostStatus::Unknown => crate::spend::Delta::Partial"),
            "found a production mapping from CostStatus::Unknown to a \
             priced Delta variant (Usd/Partial) — an unpriced call must \
             never carry a usd figure into the spend ledger"
        );
    }

    /// The other two arms of the `CostStatus` → `Delta` mapping, pinned
    /// alongside G3 so the three-way match in `record_spend_with` cannot
    /// quietly collapse two arms into one: `Complete` moves `usd` and
    /// counts as neither partial nor unpriced.
    #[test]
    fn a_complete_estimate_moves_usd_and_is_not_counted_as_partial_or_unpriced() {
        let ledger = crate::spend::InMemorySpendLedger::default();
        let policy = crate::config::types::policies::SpendPolicy::default();
        let principal = crate::spend::Principal::User("u-metering-complete".to_string());
        // 1M input + 1M output against "anthropic"/"claude-sonnet-4-5" is a
        // known `CostStatus::Complete` entry — see
        // `pricing::tests::anthropic_sonnet_complete_estimate`.
        let usage = spend_test_usage(1_000_000, 1_000_000);

        MeteringProvider::record_spend_with(&usage, "anthropic", "claude-sonnet-4-5", &principal, &policy, &ledger)
            .expect("record");

        let period_start_ms = crate::spend::period::period_start_ms(
            chrono::Utc::now().timestamp_millis(),
            policy.period,
        );
        let spent = ledger.spent_for(&principal, period_start_ms).unwrap();
        assert!(spent.usd > 0.0, "a Complete estimate must move usd, got {}", spent.usd);
        assert_eq!(spent.unpriced_calls, 0);
        assert_eq!(spent.partial_calls, 0);
    }

    /// `PartialMissingPrice` moves `usd` (it is a measured lower bound, not
    /// a guess of zero) but is counted separately from `Complete`.
    #[test]
    fn a_partial_estimate_moves_usd_and_counts_as_partial_not_unpriced() {
        let ledger = crate::spend::InMemorySpendLedger::default();
        let policy = crate::config::types::policies::SpendPolicy::default();
        let principal = crate::spend::Principal::User("u-metering-partial".to_string());
        // "openai"/"o1" has no cache_creation rate — see
        // `pricing::tests::openai_o1_missing_cache_creation_is_partial_when_used`.
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_creation_tokens: Some(100),
            thinking_tokens: None,
            cost: None,
        };

        MeteringProvider::record_spend_with(&usage, "openai", "o1", &principal, &policy, &ledger)
            .expect("record");

        let period_start_ms = crate::spend::period::period_start_ms(
            chrono::Utc::now().timestamp_millis(),
            policy.period,
        );
        let spent = ledger.spent_for(&principal, period_start_ms).unwrap();
        assert!(spent.usd > 0.0, "a partial estimate is still measured money");
        assert_eq!(spent.unpriced_calls, 0);
        assert_eq!(spent.partial_calls, 1);
    }

    // `record_spend` (the thin process-global-reading wrapper around
    // `record_spend_with`) early-returns without touching the ledger at all
    // when `!policy.enabled()` — the write-side twin of `check_with`'s
    // already-proven `!policy.enabled()` arm (`spend::tests::
    // g8_disabled_policy_never_touches_the_ledger`). It is deliberately not
    // given its own `PanicOnAnyCall`-style regression test here: doing so
    // would require calling `spend::install_policy` with a *disabled*
    // policy, which — being a `OnceLock`, set once for the process — would
    // race every other test in this file that calls
    // `install_test_spend_globals` for an *enabled* one. The guard itself
    // is a single `if` composing three pieces each already tested on their
    // own: `SpendPolicy::enabled()` (`config::types::policies::spend::
    // tests`), `current_policy()`'s default-disabled fallback, and
    // `record_spend_with` (G3/G4/Complete/Partial above).

    /// G4 — recording an enormous run of `Unknown`-priced calls (so
    /// `unpriced_calls` grows large while `usd` never moves) still leaves
    /// `spend::check_with` reporting `Allowed`, even against a ceiling far
    /// below what an "unpriced_calls counts toward the ceiling" bug would
    /// have tripped.
    #[test]
    fn g4_enormous_unpriced_call_count_never_denies() {
        let ledger = crate::spend::InMemorySpendLedger::default();
        let policy = crate::config::types::policies::SpendPolicy {
            per_user_usd: Some(0.01),
            ..Default::default()
        };
        let principal = crate::spend::Principal::User("u-metering-g4".to_string());
        let usage = spend_test_usage(1_000, 1_000);

        for _ in 0..10_000 {
            MeteringProvider::record_spend_with(
                &usage,
                "totally-unknown-provider",
                "totally-unknown-model",
                &principal,
                &policy,
                &ledger,
            )
            .expect("record");
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        match crate::spend::check_with(&principal, now_ms, &policy, &ledger) {
            crate::spend::Verdict::Allowed(spent) => {
                assert_eq!(spent.usd, 0.0);
                assert_eq!(spent.unpriced_calls, 10_000);
            }
            crate::spend::Verdict::Denied { .. } => {
                panic!("10,000 unpriced calls (usd == 0.0) must never deny against any ceiling")
            }
        }
    }

    /// One-time, idempotent install of a process-wide spend ledger + policy
    /// for the tests below that must exercise the real
    /// `process()`/`execute_streaming_dyn()` entry points — proving the
    /// *wiring* is what matters there (the historical bug this guards
    /// against was streaming skipping `record_usage` entirely, not a bad
    /// `Delta` computation, which G3/G4 above already pin in isolation).
    ///
    /// The ceiling (`per_user_usd: $1,000,000`) is far above anything any
    /// test here accumulates through `Delta::Usd`/`Delta::Partial`, and
    /// every provider the tests in this file use ("fake"/"empty"/
    /// deliberately unknown ids) prices to `CostStatus::Unknown`, which
    /// never moves `usd` (G3). Each test also uses its own `Principal` (via
    /// `scope::with_scope`) so their ledger rows never collide even though
    /// the ledger is shared.
    ///
    /// ⚠️ That argument used to end "so no test **in this file** can deny
    /// another", and the two words in bold were the whole defect. It is an
    /// argument about `install_policy`, which is a `OnceLock` set — first
    /// writer wins, everyone else's identical value is a no-op, so sharing
    /// is safe as long as every writer writes the same thing. `config::
    /// live_apply` then grew tests that call `spend::update_policy`, which
    /// **overwrites** the same cell (that is the point of the live-apply
    /// round), from a different file — leaving this comment true as written
    /// and false as relied upon. `cargo test --lib` runs one process, so
    /// `--lib providers::metering::` passed 10/10 while
    /// `--lib providers::metering:: config::live_apply::` failed both
    /// wiring tests: a disabled policy makes `record_spend` early-return
    /// and the ledger stays empty.
    ///
    /// The fix is not a bigger ceiling — it is that every test which
    /// *reads* the process-wide policy through the real global belongs to
    /// one serial group with every test that *writes* it. Counting only the
    /// writers is what let this through: the writers were serialised
    /// against each other, and these two readers were not in the group.
    fn install_test_spend_globals() {
        let policy = crate::config::types::policies::SpendPolicy {
            per_user_usd: Some(1_000_000.0),
            total_usd: None,
            period: crate::config::types::policies::SpendPeriod::Month,
        };
        // Two calls, and the second is the one that matters. `install_policy`
        // creates the cell if nobody has yet; it is a `OnceLock` set, so on
        // every run after the first it is a silent no-op — which is exactly
        // how a value another test left behind survives into this one.
        // `update_policy` then *forces* the value this test needs. Ensure,
        // don't install: the serial group above stops two tests overlapping,
        // it does not undo what the previous one stored.
        crate::spend::install_policy(policy.clone());
        assert!(
            crate::spend::update_policy(policy),
            "install_policy must have created the handle, so update_policy cannot report false \
             here — if it does, the global was never installed and every assertion below would \
             be measuring the default-disabled policy instead of this one"
        );
        crate::spend::install_ledger(Arc::new(crate::spend::InMemorySpendLedger::default()));
    }

    /// G5 — the streaming arm records identically to the non-streaming one.
    /// Both entry points on the same principal must each contribute exactly
    /// one unpriced call — the streaming metering gap is a bug this file
    /// already shipped once (see `execute_streaming_dyn`'s doc comment).
    #[tokio::test]
    #[serial_test::serial(spend_global_policy)]
    async fn g5_streaming_and_non_streaming_arms_record_the_same_spend_delta() {
        install_test_spend_globals();
        let principal = crate::spend::Principal::User("u-metering-g5".to_string());
        let attr = crate::scope::ScopeAttribution::personal("u-metering-g5");
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let policy = crate::spend::current_policy();
        let period_start_ms = crate::spend::period::period_start_ms(
            chrono::Utc::now().timestamp_millis(),
            policy.period,
        );

        let before = crate::spend::global_ledger()
            .spent_for(&principal, period_start_ms)
            .unwrap();

        let non_streaming = MeteringProvider::new(
            Arc::new(FakeProvider {
                usage: spend_test_usage(10, 10),
            }),
            None,
            "g5-non-streaming",
        );
        crate::scope::with_scope(Some(attr.clone()), async {
            non_streaming
                .process(RequestPayload::new(&msgs))
                .await
                .expect("process");
        })
        .await;
        let after_non_streaming = crate::spend::global_ledger()
            .spent_for(&principal, period_start_ms)
            .unwrap();
        assert_eq!(
            after_non_streaming.unpriced_calls - before.unpriced_calls,
            1,
            "process() must record exactly one unpriced call"
        );

        let streaming = MeteringProvider::new(
            Arc::new(FakeProvider {
                usage: spend_test_usage(10, 10),
            }),
            None,
            "g5-streaming",
        );
        let sink = crate::providers::NoopSink;
        crate::scope::with_scope(Some(attr), async {
            streaming
                .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
                .await
                .expect("execute_streaming_dyn");
        })
        .await;
        let after_streaming = crate::spend::global_ledger()
            .spent_for(&principal, period_start_ms)
            .unwrap();
        assert_eq!(
            after_streaming.unpriced_calls - after_non_streaming.unpriced_calls,
            1,
            "execute_streaming_dyn() must record identically to process() — this is the \
             streaming metering gap this file shipped once before"
        );
    }

    /// The floor arm denies before the inner provider is ever invoked, and
    /// the returned error names the ceiling — "denies before any request
    /// leaves the box" is the load-bearing property Task 6 exists for.
    #[tokio::test]
    #[serial_test::serial(spend_global_policy)]
    async fn denied_verdict_returns_a_provider_error_before_the_inner_call_runs() {
        install_test_spend_globals();
        let principal = crate::spend::Principal::User("u-metering-deny".to_string());
        let attr = crate::scope::ScopeAttribution::personal("u-metering-deny");

        // Pre-seed this principal's own row past the shared $1,000,000
        // ceiling. The shared policy/ledger installed by
        // `install_test_spend_globals` is untouched — only this
        // principal's own row moves — so this cannot affect any other
        // test's principal.
        let policy = crate::spend::current_policy();
        let period_start_ms = crate::spend::period::period_start_ms(
            chrono::Utc::now().timestamp_millis(),
            policy.period,
        );
        crate::spend::global_ledger()
            .record(&principal, period_start_ms, crate::spend::Delta::Usd(2_000_000.0))
            .unwrap();

        struct CountingProvider {
            called: Arc<std::sync::atomic::AtomicBool>,
        }
        impl AiProvider for CountingProvider {
            fn process<'a>(
                &'a self,
                _req: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                // `self.inner.process(req)` only *builds* the future — the
                // synchronous part of `process()` corresponds to building a
                // request, never the network call itself (see this file's
                // module doc). So the flag must flip inside the returned
                // future's body, where polling it stands in for "the
                // request actually left the box" — flipping it here, in the
                // synchronous half, would make this test assert the wrong
                // thing: whether `process()` was *called*, not whether the
                // future it returned was ever *awaited*.
                let called = self.called.clone();
                Box::pin(async move {
                    called.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(ProviderResponse::default())
                })
            }
            fn name(&self) -> &str {
                "counting"
            }
            fn color(&self) -> &str {
                "#000"
            }
        }
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let metering = MeteringProvider::new(
            Arc::new(CountingProvider {
                called: called.clone(),
            }),
            None,
            "g-deny",
        );
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];

        let result = crate::scope::with_scope(Some(attr), async {
            metering.process(RequestPayload::new(&msgs)).await
        })
        .await;

        assert!(result.is_err(), "a principal already over ceiling must be denied");
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "the inner provider must never be invoked once denied — the request must never \
             leave the box"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("Spend ceiling reached"),
            "error text must name the ceiling, got: {message}"
        );
    }
}
