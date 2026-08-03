//! `MoaProvider` — the AiProvider facade that runs the MoA turn shape:
//! flatten conversation → parallel advisor fan-out (per-advisor timeout,
//! fail-soft) → inject guidance at prompt tail → call the aggregator, which
//! IS the acting model. The harness sees one provider (R10).

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::json;

use crate::config::{MoaFanout, MoaToml};
use crate::error::Result;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{ProviderResponse, RequestPayload, TokenUsage};
use crate::providers::message::UnifiedMessage;
use crate::providers::session_moa_handle::SessionMoaPref;
use crate::providers::{AiProvider, DeltaSink, MeteringProvider, ModelOverrideProvider};
use crate::sync_primitives::{Arc, Mutex};

use super::advisor_health::AdvisorHealth;
use super::advisory_view::{
    apply_view_budget, build_advisory_view, mark_cache_breakpoints, view_budget_chars,
    view_signature,
};
use super::prompts::{advisor_system_prompt, attach_guidance, build_guidance, AdvisorOutcome};
use crate::providers::model_catalog::{resolve_context_window, CONSERVATIVE_CONTEXT_WINDOW};

/// One resolved advisor: label + provider chain + identity for pricing.
pub(crate) struct AdvisorSlot {
    pub(crate) label: String,
    pub(crate) provider_key: String,
    pub(crate) model: String,
    pub(crate) chain: Arc<dyn AiProvider>,
}

/// The run's fan-out memory: the last advice produced, plus enough state to
/// answer "should this iteration re-consult?" for every [`MoaFanout`].
struct FanoutState {
    /// Signature of the most recent advisory view this run has seen — updated
    /// on every turn, fan-out or reuse. Distinguishes "the task advanced" from
    /// "the harness re-issued an identical request", which is the difference
    /// between consuming a cadence slot and not.
    last_seen_signature: u64,
    /// State advances observed this run; the first is `1`. Only
    /// [`MoaFanout::EveryN`] reads it.
    advances: u32,
    outcomes: Vec<AdvisorOutcome>,
}

pub struct MoaProvider {
    // Kept (not removed): `AiProvider::name(&self) -> &str` must return a
    // borrow, and a computed/derived form (e.g. `format!` on every call)
    // cannot satisfy that signature. Spec §7's cleanup item for this field
    // was evaluated during round-2 planning and rejected (R1).
    display_name: String,
    preset_name: String,
    advisors: Vec<AdvisorSlot>,
    aggregator: Arc<dyn AiProvider>,
    aggregator_label: String,
    fanout: MoaFanout,
    advisor_timeout: Duration,
    advisor_max_tokens: Option<u32>,
    advisor_temperature: Option<f32>,
    aggregator_temperature: Option<f32>,
    /// Smallest context window (tokens) among the advisor slots, from the
    /// model catalogue. One advisory view is built per turn and shared by the
    /// whole fan-out, so it must fit the weakest link (round-8).
    advisor_window_tokens: u32,
    save_traces: bool,
    sink: Option<Arc<dyn TraceSink>>,
    /// Fan-out cache + cadence bookkeeping. INVARIANT: a MoaProvider is
    /// run-scoped and the Think loop drives `process()` strictly
    /// sequentially, so the read (cadence decision) and write (post-fan-out)
    /// never race. If an instance were ever shared across concurrent calls,
    /// two MISSes could both fan out (duplicate advisor spend,
    /// last-writer-wins) — the check-then-act gap is deliberate, not an
    /// oversight (round-2 R3).
    cache: Mutex<Option<FanoutState>>,
    /// Run-scoped per-advisor circuit breaker (round-6 G1). Same run-scoped,
    /// strictly-sequential invariant as `cache` above.
    health: Mutex<AdvisorHealth>,
}

/// Build a run-scoped `MoaProvider` from the session pref + live config.
/// Errors are human-readable reasons — the runner logs and falls back to
/// the normal provider chain (fail-soft; the conversation never breaks).
pub fn try_build_for_run(
    pref: &SessionMoaPref,
    moa_cfg: Option<&MoaToml>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
    sink: Option<Arc<dyn TraceSink>>,
) -> std::result::Result<MoaProvider, String> {
    let cfg = moa_cfg.ok_or("no [moa] section configured")?;
    let (preset_name, preset) = cfg.resolve_preset(pref.preset.as_deref()).ok_or_else(|| {
        format!(
            "MoA preset '{}' not found (configure [moa.presets.*] or ask me to set one up)",
            pref.preset.as_deref().unwrap_or("<default>")
        )
    })?;
    // Validate ONLY the preset this run resolved — against a scratch config,
    // the exact mirror of `MoaPresetStore::save_preset`. Validating the whole
    // [moa] table here meant one broken/unrelated preset (even a disabled
    // one), or a dangling default_preset, poisoned EVERY activation: arm-time
    // reported success and then every turn silently fell back to the normal
    // provider chain.
    let errs = {
        let mut scratch = MoaToml::default();
        scratch.presets.insert(preset_name.clone(), preset.clone());
        scratch.validation_errors()
    };
    if !errs.is_empty() {
        return Err(format!(
            "[moa] preset '{preset_name}' invalid: {}",
            errs.join("; ")
        ));
    }

    let resolve_slot = |slot: &crate::config::types::moa::MoaSlot| {
        // Runtime recursion guard (layer 3) — config validation already
        // rejects this, but presets can arrive through raw TOML edits.
        if slot.provider.trim().eq_ignore_ascii_case("moa") {
            return Err(format!(
                "slot {}:{} is recursive",
                slot.provider, slot.model
            ));
        }
        let base = named
            .get(&slot.provider)
            .cloned()
            .ok_or_else(|| format!("provider '{}' is not configured/keyed", slot.provider))?;
        // rust-doctor-disable-next-line excessive-clone
        Ok(Arc::new(ModelOverrideProvider::new(base, slot.model.clone())) as Arc<dyn AiProvider>)
    };

    let mut advisors = Vec::new();
    if preset.enabled {
        for (idx, slot) in preset.advisors.iter().enumerate() {
            let chain = resolve_slot(slot)?;
            let label = format!("{}:{}", slot.provider, slot.model);
            // Per-advisor metering: usage lands as ProviderUsage events
            // labelled "moa:<i>:<provider>:<model>", priced per advisor.
            let metered = Arc::new(MeteringProvider::new(
                chain,
                // rust-doctor-disable-next-line excessive-clone
                sink.clone(),
                format!("moa:{idx}:{label}"),
            )) as Arc<dyn AiProvider>;
            advisors.push(AdvisorSlot {
                label,
                // rust-doctor-disable-next-line excessive-clone
                provider_key: slot.provider.clone(),
                // rust-doctor-disable-next-line excessive-clone
                model: slot.model.clone(),
                chain: metered,
            });
        }
    }
    let aggregator = resolve_slot(&preset.aggregator)?;
    let aggregator_label = format!("{}:{}", preset.aggregator.provider, preset.aggregator.model);

    // The advisory view is shared by the whole fan-out, so its budget is set
    // by the SMALLEST advisor window — a 262 K advisor next to a 1 M one still
    // 4xx's on a view sized for the 1 M. Derived from the slots we actually
    // consult (not `preset.advisors`) so the two can never diverge.
    let advisor_window_tokens = advisors
        .iter()
        .map(|a| resolve_context_window(&a.model))
        .min()
        .unwrap_or(CONSERVATIVE_CONTEXT_WINDOW);

    Ok(MoaProvider {
        display_name: format!("moa:{preset_name}"),
        preset_name,
        health: Mutex::new(AdvisorHealth::new(advisors.len())),
        advisors,
        advisor_window_tokens,
        aggregator,
        aggregator_label,
        fanout: preset.fanout,
        advisor_timeout: Duration::from_secs(preset.advisor_timeout_secs.max(1)),
        advisor_max_tokens: preset.advisor_max_tokens,
        advisor_temperature: preset.advisor_temperature,
        aggregator_temperature: preset.aggregator_temperature,
        save_traces: cfg.save_traces,
        sink,
        cache: Mutex::new(None),
    })
}

impl MoaProvider {
    fn emit(&self, event: LoopTraceEvent) {
        if let Some(sink) = &self.sink {
            sink.on_trace(&event);
        }
    }

    /// `(provider, model)` of the aggregator slot — run-level attribution
    /// (VESR must record the ACTING model, not the pre-MoA directive; the
    /// gauge fold at runner_impl.rs already does the equivalent via
    /// serving_model_hint). Split at the FIRST ':' — provider keys never
    /// contain one; model ids may.
    #[must_use]
    pub fn aggregator_identity(&self) -> (String, String) {
        match self.aggregator_label.split_once(':') {
            Some((p, m)) => (p.to_string(), m.to_string()),
            // rust-doctor-disable-next-line excessive-clone
            None => (String::new(), self.aggregator_label.clone()),
        }
    }

    /// Sum advisor usages + per-advisor own-rate pricing for the spend event.
    /// `consulted` = advisors fanned out; `usages` = those that returned usage.
    fn spend_event(&self, consulted: usize, usages: &[(usize, TokenUsage)]) -> LoopTraceEvent {
        let mut input = 0u32;
        let mut output = 0u32;
        let mut cost: Option<f64> = None;
        for (idx, usage) in usages {
            input = input.saturating_add(usage.input_tokens);
            output = output.saturating_add(usage.output_tokens);
            let slot = &self.advisors[*idx];
            // `TokenBreakdown` fields are `u32` (matches `TokenUsage`'s own
            // token fields) — no widening conversion needed here.
            let breakdown = crate::orchestrator::dispatch::TokenBreakdown {
                input: usage.input_tokens,
                output: usage.output_tokens,
                cache_read: usage.cache_read_tokens.unwrap_or(0),
                cache_creation: usage.cache_creation_tokens.unwrap_or(0),
                reasoning: usage.thinking_tokens.unwrap_or(0),
            };
            let est = crate::pricing::estimate(&slot.provider_key, &slot.model, &breakdown);
            // `CostEstimate.usd` is a plain `f64` (0.0 when `status` is
            // `Unknown`, per its own doc comment) — gate on `status` rather
            // than treating it as an `Option`.
            if est.status != crate::pricing::CostStatus::Unknown {
                cost = Some(cost.unwrap_or(0.0) + est.usd);
            }
        }
        LoopTraceEvent::MoaAdvisorSpend {
            advisor_count: consulted,
            billed_count: usages.len(),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
        }
    }

    /// The cadence gate: `Some(advice)` reuses this run's last fan-out,
    /// `None` means consult the advisors now.
    ///
    /// Called exactly once per turn and it MUTATES the run's cadence
    /// bookkeeping — a step, not a query.
    ///
    /// * [`MoaFanout::PerIteration`] — consult whenever the view changed.
    /// * [`MoaFanout::UserTurn`] — consult once, then reuse for the run.
    /// * [`MoaFanout::EveryN`] — consult on state advance 1, then every
    ///   `n`-th after it; the iterations between reuse the last advice, so the
    ///   aggregator is never advice-less, it just is not refreshed against the
    ///   very latest tool result.
    ///
    /// A turn whose view signature is UNCHANGED is a re-issue of the same
    /// request, not task progress: it always reuses, and never consumes a
    /// cadence slot. Without that distinction one internal retry would shift
    /// every later cadence position for the rest of the run.
    fn reuse_cached(&self, sig: u64) -> Option<Vec<AdvisorOutcome>> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        // No entry = nothing consulted yet this run, so nothing to reuse.
        let state = guard.as_mut()?;
        let advanced = state.last_seen_signature != sig;
        if advanced {
            state.last_seen_signature = sig;
            state.advances = state.advances.saturating_add(1);
        } else {
            // rust-doctor-disable-next-line excessive-clone
            return Some(state.outcomes.clone());
        }
        match self.fanout {
            // rust-doctor-disable-next-line excessive-clone
            MoaFanout::UserTurn => Some(state.outcomes.clone()),
            MoaFanout::PerIteration => None,
            MoaFanout::EveryN(n) => {
                // `n >= 2` is guaranteed by `MoaFanout::from_str` and by
                // `MoaToml::validation_errors` (which `try_build_for_run`
                // runs on the resolved preset). Clamp anyway: the cost of
                // being wrong here is a divide-by-zero panic that takes the
                // turn down, and the clamp is free (P7).
                let n = n.max(2);
                // Advance 1 is what created this entry, so re-consult on
                // advances 1, 1+n, 1+2n, …
                // rust-doctor-disable-next-line excessive-clone
                ((state.advances - 1) % n != 0).then(|| state.outcomes.clone())
            }
        }
    }
}

impl MoaProvider {
    /// The MoA turn, shared by the batched and streaming entry points.
    ///
    /// `sink` is threaded to the AGGREGATOR call only — the advisors are a
    /// side channel whose text never reaches the user, and their fan-out
    /// finishes before the acting model is dialled at all. With `Some(sink)`
    /// the aggregator streams its deltas live exactly as it would without the
    /// facade in front of it; with `None` this is the historical one-shot path,
    /// byte-identical.
    fn run_turn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: Option<&'a dyn DeltaSink>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Own the borrowed payload fields (FailoverProvider pattern) so the
        // async block can rebuild sub-request payloads freely.
        let messages: Vec<UnifiedMessage> = payload.messages.to_vec();
        let system_prompt = payload.system_prompt.map(str::to_string);
        let system_blocks = payload.system_blocks.map(<[_]>::to_vec);
        let tools = payload.tools.map(<[_]>::to_vec);
        let think_level = payload.think_level;
        let caller_temperature = payload.temperature;
        let max_tokens = payload.max_tokens;
        // rust-doctor-disable-next-line excessive-clone
        let tool_choice = payload.tool_choice.clone();
        // rust-doctor-disable-next-line excessive-clone
        let metadata = payload.metadata.clone();

        Box::pin(async move {
            // 1. Advisory view + whole-view budget + signature. The budget is
            //    derived from the weakest advisor's real context window
            //    (round-8) and runs BEFORE the signature so the cache key
            //    describes what is actually sent (round-6 G3).
            let mut view = build_advisory_view(&messages);
            let budget =
                view_budget_chars(&view, self.advisor_window_tokens, self.advisor_max_tokens);
            apply_view_budget(&mut view, budget);
            let sig = view_signature(&view);

            // 1b. Prompt-cache breakpoints (round-2 E1) — AFTER the signature
            //     (which ignores marks) so the cache key is never perturbed.
            mark_cache_breakpoints(&mut view);

            // 2. Cadence decision — see `reuse_cached`.
            let cached: Option<Vec<AdvisorOutcome>> = self.reuse_cached(sig);

            // Round-2 B3: pending turn-trace payload, filled in on a MISS
            // (below) and emitted only after the aggregator returns — see
            // the comment at the aggregator call site.
            let mut pending_trace: Option<serde_json::Value> = None;

            let outcomes: Vec<AdvisorOutcome> = if let Some(hit) = cached {
                // Cache HIT: the aggregator still runs on the reused advice —
                // emit the lightweight aggregating moment so multi-iteration
                // user_turn runs don't go dark on the panel (round-2 B4).
                if !hit.is_empty() {
                    self.emit(LoopTraceEvent::MoaAggregating {
                        // rust-doctor-disable-next-line excessive-clone
                        aggregator: self.aggregator_label.clone(),
                        advisor_count: hit.len(),
                        cached: true,
                    });
                }
                hit
            } else if self.advisors.is_empty() {
                Vec::new()
            } else {
                // 3. Parallel fan-out (extracted: fan_out.rs). The breaker
                //    mask is read here and folded back below — both under the
                //    same run-scoped sequential invariant as `cache`, and
                //    never held across the fan-out `.await`.
                let skip_reasons: Vec<Option<String>> = {
                    let guard = self.health.lock().unwrap_or_else(|e| e.into_inner());
                    (0..self.advisors.len())
                        .map(|idx| guard.skip_reason(idx))
                        .collect()
                };
                let results = super::fan_out::run_fan_out(
                    &self.advisors,
                    &view,
                    // Round-6 G2: advisors see the acting agent's tool roster
                    // so their "tool-use strategy" advice names real tools.
                    &advisor_system_prompt(tools.as_deref()),
                    &skip_reasons,
                    self.advisor_timeout,
                    self.advisor_temperature,
                    self.advisor_max_tokens,
                )
                .await;
                {
                    let signals: Vec<_> = results.iter().map(|r| r.health.clone()).collect();
                    self.health
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .record(&signals);
                }

                let usages: Vec<(usize, TokenUsage)> = results
                    .iter()
                    .enumerate()
                    // rust-doctor-disable-next-line excessive-clone
                    .filter_map(|(idx, r)| r.usage.clone().map(|u| (idx, u)))
                    .collect();
                let outcomes: Vec<AdvisorOutcome> =
                    // rust-doctor-disable-next-line excessive-clone
                    results.iter().map(|r| r.outcome.clone()).collect();

                // 4. Display + accounting + heavy trace events (MISS only;
                //    per-advisor + aggregating emission lives in fan_out.rs).
                super::fan_out::emit_fanout_events(&self.sink, &results, &self.aggregator_label);
                if !usages.is_empty() {
                    // `advisor_count` is documented as CONSULTED — breaker
                    // skips don't count (the display `i/n` in
                    // `emit_fanout_events` stays the total slot count).
                    let consulted = results.iter().filter(|r| r.consulted()).count();
                    let spend = self.spend_event(consulted, &usages);
                    self.emit(spend);
                }
                if self.save_traces {
                    pending_trace = Some(json!({
                        "aggregator": self.aggregator_label,
                        "view_signature": sig,
                        "advisors": outcomes
                            .iter()
                            .map(|o| json!({ "label": o.label, "output": o.text }))
                            .collect::<Vec<_>>(),
                    }));
                }

                {
                    let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                    match guard.as_mut() {
                        Some(state) => {
                            // R3 invariant guard: `reuse_cached` stamped
                            // `last_seen_signature = sig` before this fan-out
                            // started, and the lock was NOT held across the
                            // `.await` (see the invariant doc on `cache`).
                            // Finding a different signature here means another
                            // `process()` call advanced the run's state while
                            // this one was consulting — the run-scoped,
                            // strictly sequential invariant was violated.
                            debug_assert_eq!(
                                state.last_seen_signature, sig,
                                "MoaProvider cache invariant violated: a concurrent \
                                 process() call advanced the fan-out state while \
                                 this one was consulting advisors"
                            );
                            // rust-doctor-disable-next-line excessive-clone
                            state.outcomes = outcomes.clone();
                        }
                        None => {
                            *guard = Some(FanoutState {
                                last_seen_signature: sig,
                                advances: 1,
                                // rust-doctor-disable-next-line excessive-clone
                                outcomes: outcomes.clone(),
                            });
                        }
                    }
                }
                outcomes
            };

            // 5. Guidance injection at the prompt tail (cache-stable prefix).
            let mut agg_messages = messages;
            if !outcomes.is_empty() {
                let guidance = build_guidance(&self.preset_name, &self.aggregator_label, &outcomes);
                attach_guidance(&mut agg_messages, &guidance);
            }

            // 6. Aggregator = acting model: full payload passthrough. Its
            //    ProviderResponse (tool_calls/thinking/usage) returns as-is —
            //    advisor usage is deliberately NOT merged in (gauge honesty).
            let agg_payload = RequestPayload::new(&agg_messages)
                .with_system(system_prompt.as_deref())
                .with_system_blocks(system_blocks.as_deref())
                .with_tools(tools.as_deref())
                .with_think_level(think_level)
                .with_temperature(self.aggregator_temperature.or(caller_temperature))
                .with_max_tokens(max_tokens)
                .with_tool_choice(tool_choice)
                .with_metadata(metadata);
            let agg_result = match sink {
                // Live deltas: the aggregator IS the acting model, so its
                // stream is the user-visible answer. Delegating (rather than
                // exposing `as_http_provider`) keeps the fan-out in the path —
                // forwarding the inner HttpProvider would let the caller stream
                // AROUND the facade and the advisors would never run.
                Some(sink) => {
                    self.aggregator
                        .execute_streaming_dyn(agg_payload, sink)
                        .await
                }
                None => self.aggregator.process(agg_payload).await,
            };

            // Round-2 B3: the heavy turn trace fires AFTER the aggregator so
            // it records the full turn (hermes parity: advisor I/O + the
            // aggregator's actual output). Fires on error too — advisors ran
            // and were billed, the audit record must say so. A cancelled
            // future drops the pending trace (advisor spend is already on the
            // per-advisor MeteringProvider events). Note: `pending_trace` is
            // populated only on a cache MISS, so a cache-HIT iteration emits just
            // `MoaAggregating{cached:true}` and reuses the MISS turn's record —
            // its (fresh) aggregator output is intentionally NOT re-traced, to
            // keep the HIT path a single event (see
            // `cache_hit_emits_cached_aggregating_only`).
            if let Some(mut payload) = pending_trace {
                let (output, status) = match &agg_result {
                    // rust-doctor-disable-next-line excessive-clone
                    Ok(resp) => (resp.text.clone().unwrap_or_default(), "ok".to_string()),
                    Err(e) => (String::new(), format!("error: {e}")),
                };
                payload["aggregator_output"] = json!(output);
                payload["aggregator_status"] = json!(status);
                self.emit(LoopTraceEvent::MoaTurnTrace {
                    // rust-doctor-disable-next-line excessive-clone
                    preset: self.preset_name.clone(),
                    payload,
                });
            }
            agg_result
        })
    }
}

impl AiProvider for MoaProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        self.run_turn(payload, None)
    }

    /// Same turn, with the aggregator's deltas forwarded live.
    ///
    /// Without this override the facade fell to the trait default (call
    /// `process`, then replay the finished response), so **turning MoA on
    /// silently turned live streaming off** — the user went from watching the
    /// answer type to one batch dump at the end of the turn, with nothing
    /// anywhere saying why.
    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: &'a dyn DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        self.run_turn(payload, Some(sink))
    }

    /// A decorator stack is only as streaming-capable as its weakest link, and
    /// the aggregator is the only link here that talks to a model. Advisors do
    /// not stream by construction — their text is a side channel.
    fn supports_streaming(&self) -> bool {
        self.aggregator.supports_streaming()
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn color(&self) -> &str {
        "#8b5cf6"
    }

    // Identity surfaces all delegate to the aggregator — it IS the acting
    // model: prompt behavior family, tool extraction, gauge window, pricing.
    fn supports_native_tools(&self) -> bool {
        self.aggregator.supports_native_tools()
    }

    fn protocol(&self) -> Cow<'_, str> {
        self.aggregator.protocol()
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.aggregator.model_behavior_override()
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.aggregator.behavior_hint()
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.aggregator.serving_model_hint()
    }

    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        self.aggregator.serving_provider_hint()
    }

    // `as_http_provider` stays the default `None` — forwarding the aggregator's
    // HttpProvider would let a caller stream AROUND the facade and the advisors
    // would never run. Streaming is served by `execute_streaming_dyn` above,
    // which keeps the fan-out in the path and hands the sink to the aggregator.
}

#[cfg(test)]
mod tests {
    use super::super::advisory_view::ADVISORY_VIEW_BUDGET;
    use super::*;
    use crate::providers::mock::MockProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counting stub: fixed text, optional delay, call counter.
    struct CountingProvider {
        text: String,
        delay: Option<Duration>,
        calls: Arc<AtomicUsize>,
    }
    impl CountingProvider {
        /// Fixed text, no delay, fresh call counter.
        fn new(text: impl Into<String>) -> Self {
            Self {
                text: text.into(),
                delay: None,
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }
    impl AiProvider for CountingProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let text = self.text.clone();
            let delay = self.delay;
            Box::pin(async move {
                if let Some(d) = delay {
                    tokio::time::sleep(d).await;
                }
                // Carries a (zeroed) usage so spend/billed_count tests can
                // distinguish "returned usage" from "errored, no usage" —
                // real advisor providers always populate `usage` on success.
                Ok(ProviderResponse {
                    usage: Some(TokenUsage::default()),
                    ..ProviderResponse::text_only(text)
                })
            })
        }
        fn name(&self) -> &str {
            "counting"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    fn make_provider(
        advisors: Vec<(Arc<dyn AiProvider>, &str)>,
        aggregator: Arc<dyn AiProvider>,
        fanout: MoaFanout,
        timeout_secs: u64,
    ) -> MoaProvider {
        let advisors: Vec<AdvisorSlot> = advisors
            .into_iter()
            .enumerate()
            .map(|(i, (chain, label))| AdvisorSlot {
                label: label.to_string(),
                provider_key: "mock".into(),
                model: format!("m{i}"),
                chain,
            })
            .collect();
        MoaProvider {
            display_name: "moa:test".into(),
            preset_name: "test".into(),
            health: Mutex::new(AdvisorHealth::new(advisors.len())),
            advisors,
            aggregator,
            aggregator_label: "mock:agg".into(),
            fanout,
            advisor_timeout: Duration::from_secs(timeout_secs),
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
            advisor_window_tokens: CONSERVATIVE_CONTEXT_WINDOW,
            save_traces: false,
            sink: None,
            cache: Mutex::new(None),
        }
    }

    /// Captures every emitted `LoopTraceEvent` for wire-shape/lock assertions
    /// (round-2 B1/B2/B4 tests).
    struct RecordingSink(std::sync::Mutex<Vec<LoopTraceEvent>>);
    impl RecordingSink {
        fn new() -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(Vec::new())))
        }
        fn events(&self) -> Vec<LoopTraceEvent> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }
    impl crate::harness::TraceSink for RecordingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event.clone());
        }
        fn flush(&self) {}
    }

    fn make_provider_sinked(
        advisors: Vec<(Arc<dyn AiProvider>, &str)>,
        aggregator: Arc<dyn AiProvider>,
        fanout: MoaFanout,
        sink: Arc<RecordingSink>,
    ) -> MoaProvider {
        let mut p = make_provider(advisors, aggregator, fanout, 5);
        p.sink = Some(sink);
        p
    }

    fn user_msgs(text: &str) -> Vec<UnifiedMessage> {
        vec![UnifiedMessage::user(text)]
    }

    /// Records the joined text of every message it is handed — used as the
    /// aggregator to assert on the injected guidance block.
    struct CapturingAggregator(Arc<Mutex<String>>);
    impl AiProvider for CapturingAggregator {
        fn process<'a>(
            &'a self,
            p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            let joined = p
                .messages
                .iter()
                .flat_map(UnifiedMessage::content_blocks)
                .filter_map(|b| match b {
                    crate::providers::message::ContentBlock::Text { text, .. } => {
                        Some(text.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = joined;
            Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str {
            "capture"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    /// Records the SYSTEM prompt it is handed — used as an advisor to assert
    /// on the tool roster (round-6 G2).
    struct SystemCapture(Arc<Mutex<String>>);
    impl AiProvider for SystemCapture {
        fn process<'a>(
            &'a self,
            p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) =
                p.system_prompt.unwrap_or_default().to_string();
            Box::pin(async { Ok(ProviderResponse::text_only("advice".into())) })
        }
        fn name(&self) -> &str {
            "system-capture"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[tokio::test]
    async fn advisors_run_in_parallel_and_aggregator_answers() {
        let start = std::time::Instant::now();
        let calls = Arc::new(AtomicUsize::new(0));
        let slow = |t: &str| -> Arc<dyn AiProvider> {
            Arc::new(CountingProvider {
                text: t.into(),
                delay: Some(Duration::from_millis(150)),
                calls: calls.clone(),
            })
        };
        let p = make_provider(
            vec![(slow("advice-1"), "a:1"), (slow("advice-2"), "a:2")],
            Arc::new(MockProvider::new("final answer")),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("go");
        let resp = p.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "final answer");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Parallel: two 150ms advisors must not take 300ms serially.
        assert!(start.elapsed() < Duration::from_millis(280));
    }

    /// A streaming-capable aggregator whose `execute_streaming_dyn` emits the
    /// answer one word at a time, so the facade's delegation is observable.
    struct StreamingAggregator;
    impl AiProvider for StreamingAggregator {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async { Ok(ProviderResponse::text_only("final answer".into())) })
        }
        fn execute_streaming_dyn<'a>(
            &'a self,
            _p: RequestPayload<'a>,
            sink: &'a dyn DeltaSink,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                sink.on_delta(&crate::providers::ProviderDelta::TextDelta("final ".into()))
                    .await;
                sink.on_delta(&crate::providers::ProviderDelta::TextDelta("answer".into()))
                    .await;
                Ok(ProviderResponse::text_only("final answer".into()))
            })
        }
        fn supports_streaming(&self) -> bool {
            true
        }
        fn name(&self) -> &str {
            "streaming-aggregator"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[derive(Default)]
    struct CollectingSink(Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl DeltaSink for CollectingSink {
        async fn on_delta(&self, delta: &crate::providers::ProviderDelta) {
            if let crate::providers::ProviderDelta::TextDelta(t) = delta {
                self.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(t.clone());
            }
        }
    }

    /// Turning MoA on must not silently turn live streaming off. Before the
    /// facade overrode the streaming seam it fell to the trait default (call
    /// `process`, replay at the end), so a MoA session went from watching the
    /// answer type to one batch dump — with nothing saying why. The fan-out
    /// still runs: the sink reaches the AGGREGATOR, not around the facade.
    #[tokio::test]
    async fn streaming_delegates_to_the_aggregator_with_the_fanout_intact() {
        let calls = Arc::new(AtomicUsize::new(0));
        let advisor: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(advisor, "a:1")],
            Arc::new(StreamingAggregator),
            MoaFanout::PerIteration,
            30,
        );
        assert!(
            p.supports_streaming(),
            "the facade must report the aggregator's streaming capability",
        );
        let msgs = user_msgs("go");
        let sink = CollectingSink::default();
        let resp = p
            .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
            .await
            .unwrap();
        assert_eq!(resp.text_content(), "final answer");
        assert_eq!(
            sink.0.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec!["final ".to_string(), "answer".to_string()],
            "aggregator deltas must reach the caller's sink live",
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the advisor fan-out must still run on the streaming path",
        );
    }

    /// The capability bit is a promise about the OUTERMOST provider: a
    /// non-streaming aggregator must not be advertised as streaming.
    #[tokio::test]
    async fn non_streaming_aggregator_is_not_advertised_as_streaming() {
        let p = make_provider(
            vec![],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        assert!(!p.supports_streaming());
    }

    #[tokio::test]
    async fn advisor_failure_and_timeout_degrade_to_notes() {
        use crate::providers::mock::MockError;
        let failing: Arc<dyn AiProvider> =
            Arc::new(MockProvider::new("x").with_error(MockError::Network("down".into())));
        let sleepy: Arc<dyn AiProvider> =
            Arc::new(MockProvider::new("late").with_delay(Duration::from_secs(5)));
        let seen = Arc::new(Mutex::new(String::new()));
        let p = make_provider(
            vec![(failing, "f:1"), (sleepy, "s:2")],
            Arc::new(CapturingAggregator(seen.clone())),
            MoaFanout::PerIteration,
            1, // 1s timeout < 5s delay
        );
        let msgs = user_msgs("go");
        let resp = p.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "ok");
        let guidance = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(guidance.contains("[failed:"));
        assert!(guidance.contains("[timeout after 1s]"));
        // Order stable: advisor 1 note appears before advisor 2 note.
        assert!(guidance.find("f:1").unwrap() < guidance.find("s:2").unwrap());
    }

    #[tokio::test]
    async fn per_iteration_cache_dedupes_identical_state() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counting: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(counting, "c:1")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("same state");
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        p.process(RequestPayload::new(&msgs)).await.unwrap(); // identical → HIT
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Changed state → MISS.
        let msgs2 = vec![
            UnifiedMessage::user("same state"),
            UnifiedMessage::assistant("did something"),
        ];
        p.process(RequestPayload::new(&msgs2)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn user_turn_cache_survives_state_growth() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counting: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(counting, "c:1")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::UserTurn,
            30,
        );
        let msgs = user_msgs("go");
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        let grown = vec![
            UnifiedMessage::user("go"),
            UnifiedMessage::assistant("step"),
        ];
        p.process(RequestPayload::new(&grown)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1); // run-scoped: once only
    }

    #[tokio::test]
    async fn every_n_consults_on_the_first_advance_then_every_nth() {
        // The middle ground between per_iteration (advisor spend x tool-loop
        // depth) and user_turn (advice frozen at the top of the run).
        let calls = Arc::new(AtomicUsize::new(0));
        let counting: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(counting, "c:1")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::EveryN(3),
            30,
        );
        // 7 state advances -> fan out on advances 1, 4, 7.
        for step in 0..7 {
            let msgs = iteration_msgs(step);
            p.process(RequestPayload::new(&msgs)).await.unwrap();
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "every_n:3 over 7 advances must consult on 1, 4 and 7"
        );
    }

    #[tokio::test]
    async fn every_n_does_not_let_an_identical_reissue_consume_a_cadence_slot() {
        // A repeat `process()` with a byte-identical view is the harness
        // re-issuing the same request, not the task advancing. Counting it
        // would shift every later cadence position for the rest of the run.
        let calls = Arc::new(AtomicUsize::new(0));
        let counting: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(counting, "c:1")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::EveryN(2),
            30,
        );
        let first = iteration_msgs(0);
        // Advance 1: consults.
        p.process(RequestPayload::new(&first)).await.unwrap();
        // Three identical re-issues: all reuse, none advance the cadence.
        for _ in 0..3 {
            p.process(RequestPayload::new(&first)).await.unwrap();
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an identical re-issue must reuse the run's advice"
        );
        // Advance 2 (off-cadence for n=2) still reuses...
        let second = iteration_msgs(1);
        p.process(RequestPayload::new(&second)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // ...and advance 3 is on-cadence again. Had the re-issues consumed
        // slots, this would already have fired earlier.
        let third = iteration_msgs(2);
        p.process(RequestPayload::new(&third)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn all_advisors_down_drops_the_use_these_framing() {
        // Wrapping a wall of failure notes in "use the advisor responses
        // below as private context" told the acting model to consult advice
        // that does not exist — on EVERY iteration, since the breaker keeps
        // the slots retired and the wall is reprinted verbatim each time.
        let seen = Arc::new(Mutex::new(String::new()));
        let dead_a: Arc<dyn AiProvider> = Arc::new(FailingCounter(Arc::new(AtomicUsize::new(0))));
        let dead_b: Arc<dyn AiProvider> = Arc::new(FailingCounter(Arc::new(AtomicUsize::new(0))));
        let p = make_provider(
            vec![(dead_a, "mock:a"), (dead_b, "mock:b")],
            Arc::new(CapturingAggregator(seen.clone())),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("go");
        p.process(RequestPayload::new(&msgs)).await.unwrap();

        let guidance = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(
            !guidance.contains("Use the advisor responses below"),
            "{guidance}"
        );
        assert!(
            guidance.contains("No advisor returned usable guidance"),
            "{guidance}"
        );
        // Both slots are still disclosed, so the aggregator can say it ran
        // degraded rather than silently pretending it had advice.
        assert!(guidance.contains("mock:a [failed:"), "{guidance}");
        assert!(guidance.contains("mock:b [failed:"), "{guidance}");
    }

    /// An advisor that answers, recording the number of view characters it saw.
    struct ViewSizeProbe(Arc<Mutex<usize>>);
    impl AiProvider for ViewSizeProbe {
        fn process<'a>(
            &'a self,
            p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            let chars: usize = p
                .messages
                .iter()
                .flat_map(UnifiedMessage::content_blocks)
                .filter_map(|b| match b {
                    crate::providers::message::ContentBlock::Text { text, .. } => {
                        Some(text.chars().count())
                    }
                    _ => None,
                })
                .sum();
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = chars;
            Box::pin(async { Ok(ProviderResponse::text_only("advice".into())) })
        }
        fn name(&self) -> &str {
            "view-size-probe"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[tokio::test]
    async fn advisory_view_is_sized_by_the_smallest_advisor_window() {
        // One view is shared by the whole fan-out, so a 1 M-window aggregator
        // sitting next to a small-window advisor must not size it for the
        // aggregator — that is a hard 4xx on the advisor, every iteration,
        // in exactly the long-run scenario MoA exists for.
        let seen = Arc::new(Mutex::new(0usize));
        let probe: Arc<dyn AiProvider> = Arc::new(ViewSizeProbe(seen.clone()));
        let mut p = make_provider(
            vec![(probe, "small:advisor")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        p.advisor_window_tokens = 16_000;
        // Far more prose than a 16 K window can hold.
        let msgs = vec![UnifiedMessage::user("word ".repeat(200_000))];
        p.process(RequestPayload::new(&msgs)).await.unwrap();

        let chars = *seen.lock().unwrap_or_else(|e| e.into_inner());
        assert!(chars > 0, "the advisor must still receive a usable view");
        // 16 K tokens of prose is well under the 120 K-char ceiling the flat
        // constant would have handed it.
        assert!(
            chars < ADVISORY_VIEW_BUDGET,
            "a 16 K-window advisor got {chars} chars — the window was ignored"
        );
    }

    #[tokio::test]
    async fn no_advisors_means_bare_aggregator() {
        let p = make_provider(
            vec![],
            Arc::new(MockProvider::new("solo")),
            MoaFanout::PerIteration,
            30,
        );
        let msgs = user_msgs("go");
        let resp = p.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "solo");
    }

    #[test]
    fn identity_delegates_to_aggregator_and_no_http_downcast() {
        let p = make_provider(
            vec![],
            Arc::new(ModelOverrideProvider::new(
                Arc::new(MockProvider::new("x")),
                "agg-model",
            )),
            MoaFanout::PerIteration,
            30,
        );
        assert_eq!(p.serving_model_hint().unwrap(), "agg-model");
        assert_eq!(p.name(), "moa:test");
        assert!(p.as_http_provider().is_none());
    }

    #[test]
    fn aggregator_identity_splits_label() {
        let agg = Arc::new(CountingProvider::new("x"));
        let p = make_provider(vec![], agg, MoaFanout::PerIteration, 5);
        // make_provider sets aggregator_label = "mock:agg".
        assert_eq!(
            p.aggregator_identity(),
            ("mock".to_string(), "agg".to_string())
        );
    }

    #[test]
    fn parse_one_shot_command_semantics() {
        use super::super::parse_one_shot_command;
        assert_eq!(
            parse_one_shot_command("/moa write a poem"),
            Some("write a poem")
        );
        // Arg equal to a preset name is STILL a prompt (hermes-pinned).
        assert_eq!(parse_one_shot_command("/moa default"), Some("default"));
        assert_eq!(parse_one_shot_command("/moa"), None);
        assert_eq!(parse_one_shot_command("/moa   "), None);
        assert_eq!(parse_one_shot_command("hello"), None);
        assert_eq!(parse_one_shot_command("/moab x"), None);
        // Nested slash command as the remainder: still returned as the prompt
        // (the caller decides not to arm MoA for it — see the guard in
        // execute.rs / slash_command.rs that checks `starts_with('/')`).
        assert_eq!(parse_one_shot_command("/moa /help"), Some("/help"));
    }

    #[test]
    fn try_build_for_run_errors() {
        use crate::providers::session_moa_handle::SessionMoaPref;
        let named: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        let pref = SessionMoaPref {
            preset: None,
            one_shot: false,
            restore: None,
        };
        // No config at all.
        assert!(try_build_for_run(&pref, None, &named, None).is_err());
        // Preset references an unconfigured provider.
        let cfg: MoaToml = toml::from_str(
            r#"
[presets.p]
advisors = [{ provider = "ghost", model = "m" }]
aggregator = { provider = "ghost", model = "n" }
"#,
        )
        .unwrap();
        let err = try_build_for_run(
            &SessionMoaPref {
                preset: Some("p".into()),
                one_shot: false,
                restore: None,
            },
            Some(&cfg),
            &named,
            None,
        )
        .err()
        .unwrap();
        assert!(err.contains("ghost"));
    }

    #[tokio::test]
    async fn events_carry_error_cached_and_billed_count() {
        let sink = RecordingSink::new();
        let ok = Arc::new(CountingProvider::new("advice"));
        let bad: Arc<dyn AiProvider> = Arc::new(
            MockProvider::new("bad")
                .with_error(crate::providers::mock::MockError::Network("boom".into())),
        );
        let agg = Arc::new(CountingProvider::new("final"));
        let p = make_provider_sinked(
            vec![(ok, "mock:ok"), (bad, "mock:bad")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.process(RequestPayload::new(&user_msgs("go")))
            .await
            .unwrap();

        let events = sink.events();
        let advisors: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, LoopTraceEvent::MoaAdvisor { .. }))
            .collect();
        assert_eq!(advisors.len(), 2);
        // B2: success carries error=None, failure carries the structural reason.
        let LoopTraceEvent::MoaAdvisor { error: e0, .. } = advisors[0] else {
            panic!()
        };
        let LoopTraceEvent::MoaAdvisor { error: e1, .. } = advisors[1] else {
            panic!()
        };
        assert!(e0.is_none());
        assert!(e1.as_deref().is_some_and(|e| e.contains("boom")));
        // B4: MISS aggregating is cached=false.
        assert!(events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::MoaAggregating { cached: false, .. })));
        // B1: spend advisor_count = consulted (2), billed_count = with-usage (1).
        let spend = events
            .iter()
            .find(|e| matches!(e, LoopTraceEvent::MoaAdvisorSpend { .. }))
            .expect("spend event");
        let LoopTraceEvent::MoaAdvisorSpend {
            advisor_count,
            billed_count,
            ..
        } = spend
        else {
            panic!()
        };
        assert_eq!(*advisor_count, 2);
        assert_eq!(*billed_count, 1);
    }

    #[tokio::test]
    async fn cache_hit_emits_cached_aggregating_only() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg = Arc::new(CountingProvider::new("final"));
        let p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::UserTurn,
            sink.clone(),
        );
        let msgs = user_msgs("go");
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        let miss_events = sink.events().len();
        p.process(RequestPayload::new(&msgs)).await.unwrap();
        let all = sink.events();
        let hit_events = &all[miss_events..];
        // HIT: exactly one new event — MoaAggregating { cached: true }; no
        // advisor re-emission, no spend re-emission.
        assert_eq!(hit_events.len(), 1);
        assert!(matches!(
            hit_events[0],
            LoopTraceEvent::MoaAggregating { cached: true, .. }
        ));
    }

    #[tokio::test]
    async fn save_traces_gate_controls_turn_trace() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg = Arc::new(CountingProvider::new("final"));
        let mut p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.save_traces = false;
        p.process(RequestPayload::new(&user_msgs("go")))
            .await
            .unwrap();
        assert!(!sink
            .events()
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::MoaTurnTrace { .. })));
        // Flip the gate on a fresh provider: trace fires.
        let sink2 = RecordingSink::new();
        let adv2 = Arc::new(CountingProvider::new("advice"));
        let agg2 = Arc::new(CountingProvider::new("final"));
        let mut p2 = make_provider_sinked(
            vec![(adv2, "mock:a")],
            agg2,
            MoaFanout::PerIteration,
            sink2.clone(),
        );
        p2.save_traces = true;
        p2.process(RequestPayload::new(&user_msgs("go")))
            .await
            .unwrap();
        assert!(sink2
            .events()
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::MoaTurnTrace { .. })));
    }

    #[tokio::test]
    async fn turn_trace_carries_aggregator_output_and_fires_after_it() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg = Arc::new(CountingProvider::new("final answer"));
        let mut p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.save_traces = true;
        p.process(RequestPayload::new(&user_msgs("go")))
            .await
            .unwrap();
        let events = sink.events();
        let trace = events
            .iter()
            .find_map(|e| match e {
                LoopTraceEvent::MoaTurnTrace { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .expect("turn trace");
        assert_eq!(trace["aggregator_status"], "ok");
        assert_eq!(trace["aggregator_output"], "final answer");
        // Ordering: the turn trace must be the LAST event (after aggregating).
        assert!(matches!(
            events.last().unwrap(),
            LoopTraceEvent::MoaTurnTrace { .. }
        ));
    }

    /// Always fails, and counts how many times it was actually reached —
    /// the breaker's whole point is that this number stops growing.
    struct FailingCounter(Arc<AtomicUsize>);
    impl AiProvider for FailingCounter {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Err(crate::error::AlephError::NetworkError {
                    message: "advisor down".to_string(),
                    suggestion: None,
                })
            })
        }
        fn name(&self) -> &str {
            "failing"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    /// Distinct messages per iteration so every `process()` is a cache MISS —
    /// exactly the `per_iteration` shape where a dead advisor used to re-pay
    /// its full timeout budget on every tool step.
    fn iteration_msgs(step: usize) -> Vec<UnifiedMessage> {
        vec![
            UnifiedMessage::user("go"),
            UnifiedMessage::assistant(format!("step {step}")),
        ]
    }

    #[tokio::test]
    async fn dead_advisor_is_retired_after_consecutive_failures() {
        // Round-6 G1: without the breaker this advisor is called once per
        // iteration forever (and in production burns advisor_timeout_secs
        // each time). It must stop being reached after the trip threshold.
        let calls = Arc::new(AtomicUsize::new(0));
        let dead: Arc<dyn AiProvider> = Arc::new(FailingCounter(calls.clone()));
        let p = make_provider(
            vec![(dead, "mock:dead")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        for step in 0..8 {
            let msgs = iteration_msgs(step);
            p.process(RequestPayload::new(&msgs)).await.unwrap();
        }
        let reached = calls.load(Ordering::SeqCst);
        assert!(
            reached <= 3,
            "advisor was reached {reached} times across 8 iterations — breaker never tripped"
        );
        assert!(reached >= 1, "the breaker must not skip a healthy advisor");
    }

    #[tokio::test]
    async fn retired_advisor_keeps_its_slot_and_says_why() {
        // The aggregator must still see the slot: "one advisor configured" and
        // "three configured, two down" are different situations, and advisor
        // numbering must not shift mid-run.
        let calls = Arc::new(AtomicUsize::new(0));
        let dead: Arc<dyn AiProvider> = Arc::new(FailingCounter(calls.clone()));
        let live = Arc::new(CountingProvider::new("real advice"));
        let seen = Arc::new(Mutex::new(String::new()));
        let p = make_provider(
            vec![(dead, "mock:dead"), (live, "mock:live")],
            Arc::new(CapturingAggregator(seen.clone())),
            MoaFanout::PerIteration,
            30,
        );
        for step in 0..6 {
            let msgs = iteration_msgs(step);
            p.process(RequestPayload::new(&msgs)).await.unwrap();
        }
        let guidance = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // The slot is still disclosed, with its reason — in the roster line,
        // which is where round-8 moved every non-advising slot.
        assert!(guidance.contains("[skipped:"), "{guidance}");
        assert!(guidance.contains("retired for this run"), "{guidance}");
        assert!(
            guidance.contains("Advisors: mock:dead [skipped:"),
            "the dead slot must stay visible to the aggregator:\n{guidance}"
        );
        // Slot order intact: the survivor is still Advisor 2, NOT renumbered
        // to 1. This is the anti-drift property round-6 G1 副则 exists for.
        assert!(guidance.contains("Advisor 2 — mock:live"), "{guidance}");
        assert!(guidance.contains("real advice"), "{guidance}");
        // ...and a dead slot is not numbered as if it were advice to read.
        assert!(
            !guidance.contains("Advisor 1 —"),
            "a retired slot must not be presented as a numbered response:\n{guidance}"
        );
    }

    #[tokio::test]
    async fn spend_counts_consulted_slots_while_display_counts_all() {
        // `MoaAdvisorSpend.advisor_count` is documented as CONSULTED, so a
        // breaker skip must not inflate it; the `i/n` display count stays the
        // total so advisor numbering is stable.
        let sink = RecordingSink::new();
        let dead: Arc<dyn AiProvider> = Arc::new(FailingCounter(Arc::new(AtomicUsize::new(0))));
        let live = Arc::new(CountingProvider::new("advice"));
        let p = make_provider_sinked(
            vec![(dead, "mock:dead"), (live, "mock:live")],
            Arc::new(CountingProvider::new("final")),
            MoaFanout::PerIteration,
            sink.clone(),
        );
        for step in 0..6 {
            let msgs = iteration_msgs(step);
            p.process(RequestPayload::new(&msgs)).await.unwrap();
        }
        let events = sink.events();
        let last_spend = events
            .iter()
            .rev()
            .find_map(|e| match e {
                LoopTraceEvent::MoaAdvisorSpend { advisor_count, .. } => Some(*advisor_count),
                _ => None,
            })
            .expect("spend event");
        assert_eq!(last_spend, 1, "skipped slot must not count as consulted");
        let last_aggregating = events
            .iter()
            .rev()
            .find_map(|e| match e {
                LoopTraceEvent::MoaAggregating { advisor_count, .. } => Some(*advisor_count),
                _ => None,
            })
            .expect("aggregating event");
        assert_eq!(last_aggregating, 2, "display count stays the total slots");
    }

    #[tokio::test]
    async fn healthy_advisors_are_never_skipped() {
        // Guard against the breaker mis-firing on a working preset: N
        // iterations must produce N consultations.
        let calls = Arc::new(AtomicUsize::new(0));
        let live: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            text: "advice".into(),
            delay: None,
            calls: calls.clone(),
        });
        let p = make_provider(
            vec![(live, "mock:live")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        for step in 0..5 {
            let msgs = iteration_msgs(step);
            p.process(RequestPayload::new(&msgs)).await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn advisors_receive_the_acting_agents_tool_roster() {
        // Round-6 G2: `payload.tools` was owned and dropped on the advisor
        // path, so advisors could only learn a tool existed after it had been
        // called — and invented names for the rest.
        let seen = Arc::new(Mutex::new(String::new()));
        let advisor: Arc<dyn AiProvider> = Arc::new(SystemCapture(seen.clone()));
        let p = make_provider(
            vec![(advisor, "mock:a")],
            Arc::new(MockProvider::new("ok")),
            MoaFanout::PerIteration,
            30,
        );
        let tools = vec![crate::tool_metadata::ToolDefinition::new(
            "web_search",
            "Search the web.",
            serde_json::json!({}),
            crate::tool_metadata::ToolCategory::Builtin,
        )];
        let msgs = user_msgs("go");
        p.process(RequestPayload::new(&msgs).with_tools(Some(&tools)))
            .await
            .unwrap();
        let system = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(system.contains("- web_search: Search the web"), "{system}");
    }

    #[tokio::test]
    async fn turn_trace_fires_with_error_status_when_aggregator_fails() {
        let sink = RecordingSink::new();
        let adv = Arc::new(CountingProvider::new("advice"));
        let agg: Arc<dyn AiProvider> = Arc::new(
            crate::providers::mock::MockProvider::new("unused").with_error(
                crate::providers::mock::MockError::Network("agg down".into()),
            ),
        );
        let mut p = make_provider_sinked(
            vec![(adv, "mock:a")],
            agg,
            MoaFanout::PerIteration,
            sink.clone(),
        );
        p.save_traces = true;
        let result = p.process(RequestPayload::new(&user_msgs("go"))).await;
        assert!(result.is_err());
        let events = sink.events();
        let trace = events
            .iter()
            .find_map(|e| match e {
                LoopTraceEvent::MoaTurnTrace { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .expect("turn trace fires even on aggregator error — advisors were billed");
        assert!(trace["aggregator_status"]
            .as_str()
            .unwrap()
            .starts_with("error:"));
    }
}
