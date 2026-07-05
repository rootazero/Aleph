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
use crate::providers::{AiProvider, MeteringProvider, ModelOverrideProvider};
use crate::sync_primitives::{Arc, Mutex};

use super::advisory_view::{build_advisory_view, view_signature};
use super::prompts::{attach_guidance, build_guidance, AdvisorOutcome, ADVISOR_SYSTEM_PROMPT};

/// One resolved advisor: label + provider chain + identity for pricing.
pub(crate) struct AdvisorSlot {
    label: String,
    provider_key: String,
    model: String,
    chain: Arc<dyn AiProvider>,
}

struct AdvisorCache {
    signature: u64,
    outcomes: Vec<AdvisorOutcome>,
}

pub struct MoaProvider {
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
    save_traces: bool,
    sink: Option<Arc<dyn TraceSink>>,
    cache: Mutex<Option<AdvisorCache>>,
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
    let (preset_name, preset) = cfg
        .resolve_preset(pref.preset.as_deref())
        .ok_or_else(|| {
            format!(
                "MoA preset '{}' not found (configure [moa.presets.*] or ask me to set one up)",
                pref.preset.as_deref().unwrap_or("<default>")
            )
        })?;
    let errs = cfg.validation_errors();
    if !errs.is_empty() {
        return Err(format!("[moa] config invalid: {}", errs.join("; ")));
    }

    let resolve_slot = |slot: &crate::config::types::moa::MoaSlot| {
        // Runtime recursion guard (layer 3) — config validation already
        // rejects this, but presets can arrive through raw TOML edits.
        if slot.provider.trim().eq_ignore_ascii_case("moa") {
            return Err(format!("slot {}:{} is recursive", slot.provider, slot.model));
        }
        let base = named.get(&slot.provider).cloned().ok_or_else(|| {
            format!("provider '{}' is not configured/keyed", slot.provider)
        })?;
        Ok(Arc::new(ModelOverrideProvider::new(base, slot.model.clone()))
            as Arc<dyn AiProvider>)
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
                sink.clone(),
                format!("moa:{idx}:{label}"),
            )) as Arc<dyn AiProvider>;
            advisors.push(AdvisorSlot {
                label,
                provider_key: slot.provider.clone(),
                model: slot.model.clone(),
                chain: metered,
            });
        }
    }
    let aggregator = resolve_slot(&preset.aggregator)?;
    let aggregator_label = format!(
        "{}:{}",
        preset.aggregator.provider, preset.aggregator.model
    );

    Ok(MoaProvider {
        display_name: format!("moa:{preset_name}"),
        preset_name,
        advisors,
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

    /// Sum advisor usages + per-advisor own-rate pricing for the spend event.
    fn spend_event(&self, usages: &[(usize, TokenUsage)]) -> LoopTraceEvent {
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
            advisor_count: usages.len(),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
        }
    }
}

impl AiProvider for MoaProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
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
        let tool_choice = payload.tool_choice.clone();
        let metadata = payload.metadata.clone();

        Box::pin(async move {
            // 1. Advisory view + signature.
            let view = build_advisory_view(&messages);
            let sig = view_signature(&view);

            // 2. Cache decision (per_iteration: same-signature repeat calls
            //    — harness internal retries — are HITs; user_turn: any
            //    existing cache is a HIT for the rest of this run).
            let cached: Option<Vec<AdvisorOutcome>> = {
                let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                guard.as_ref().and_then(|c| match self.fanout {
                    MoaFanout::UserTurn => Some(c.outcomes.clone()),
                    MoaFanout::PerIteration => {
                        (c.signature == sig).then(|| c.outcomes.clone())
                    }
                })
            };

            let outcomes: Vec<AdvisorOutcome> = if let Some(hit) = cached {
                hit
            } else if self.advisors.is_empty() {
                Vec::new()
            } else {
                // 3. Parallel fan-out, per-advisor timeout, fail-soft.
                let timeout = self.advisor_timeout;
                let futures = self.advisors.iter().map(|slot| {
                    let view = &view;
                    async move {
                        let advisor_payload = RequestPayload::new(view)
                            .with_system(Some(ADVISOR_SYSTEM_PROMPT))
                            .with_temperature(self.advisor_temperature)
                            .with_max_tokens(self.advisor_max_tokens);
                        match tokio::time::timeout(timeout, slot.chain.process(advisor_payload))
                            .await
                        {
                            Ok(Ok(resp)) => {
                                let text = resp
                                    .text
                                    .clone()
                                    .filter(|t| !t.trim().is_empty())
                                    .unwrap_or_else(|| "(empty response)".to_string());
                                (text, resp.usage, None::<String>)
                            }
                            Ok(Err(e)) => (format!("[failed: {e}]"), None, Some(e.to_string())),
                            Err(_) => (
                                format!("[timeout after {}s]", timeout.as_secs()),
                                None,
                                Some("timeout".to_string()),
                            ),
                        }
                    }
                });
                let results = futures::future::join_all(futures).await;

                let mut outcomes = Vec::with_capacity(results.len());
                let mut usages: Vec<(usize, TokenUsage)> = Vec::new();
                for (idx, (text, usage, _err)) in results.into_iter().enumerate() {
                    if let Some(u) = usage {
                        usages.push((idx, u));
                    }
                    outcomes.push(AdvisorOutcome {
                        label: self.advisors[idx].label.clone(),
                        text,
                    });
                }

                // 4. Display + accounting + heavy trace events (MISS only).
                let count = outcomes.len();
                for (idx, o) in outcomes.iter().enumerate() {
                    self.emit(LoopTraceEvent::MoaAdvisor {
                        index: idx + 1,
                        count,
                        label: o.label.clone(),
                        text: o.text.clone(),
                    });
                }
                self.emit(LoopTraceEvent::MoaAggregating {
                    aggregator: self.aggregator_label.clone(),
                    advisor_count: count,
                });
                if !usages.is_empty() {
                    let spend = self.spend_event(&usages);
                    self.emit(spend);
                }
                if self.save_traces {
                    self.emit(LoopTraceEvent::MoaTurnTrace {
                        preset: self.preset_name.clone(),
                        payload: json!({
                            "aggregator": self.aggregator_label,
                            "view_signature": sig,
                            "advisors": outcomes
                                .iter()
                                .map(|o| json!({ "label": o.label, "output": o.text }))
                                .collect::<Vec<_>>(),
                        }),
                    });
                }

                *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(AdvisorCache {
                    signature: sig,
                    outcomes: outcomes.clone(),
                });
                outcomes
            };

            // 5. Guidance injection at the prompt tail (cache-stable prefix).
            let mut agg_messages = messages;
            if !outcomes.is_empty() {
                let guidance =
                    build_guidance(&self.preset_name, &self.aggregator_label, &outcomes);
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
            self.aggregator.process(agg_payload).await
        })
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

    // as_http_provider stays the default `None` — forwarding the aggregator's
    // HttpProvider would let think.rs stream AROUND the facade and advisors
    // would never run. The production failover path is `None` today anyway.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock::MockProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counting stub: fixed text, optional delay, call counter.
    struct CountingProvider {
        text: String,
        delay: Option<Duration>,
        calls: Arc<AtomicUsize>,
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
                Ok(ProviderResponse::text_only(text))
            })
        }
        fn name(&self) -> &str { "counting" }
        fn color(&self) -> &str { "#000" }
    }

    fn make_provider(
        advisors: Vec<(Arc<dyn AiProvider>, &str)>,
        aggregator: Arc<dyn AiProvider>,
        fanout: MoaFanout,
        timeout_secs: u64,
    ) -> MoaProvider {
        MoaProvider {
            display_name: "moa:test".into(),
            preset_name: "test".into(),
            advisors: advisors
                .into_iter()
                .enumerate()
                .map(|(i, (chain, label))| AdvisorSlot {
                    label: label.to_string(),
                    provider_key: "mock".into(),
                    model: format!("m{i}"),
                    chain,
                })
                .collect(),
            aggregator,
            aggregator_label: "mock:agg".into(),
            fanout,
            advisor_timeout: Duration::from_secs(timeout_secs),
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
            save_traces: false,
            sink: None,
            cache: Mutex::new(None),
        }
    }

    fn user_msgs(text: &str) -> Vec<UnifiedMessage> {
        vec![UnifiedMessage::user(text)]
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

    #[tokio::test]
    async fn advisor_failure_and_timeout_degrade_to_notes() {
        use crate::providers::mock::MockError;
        let failing: Arc<dyn AiProvider> =
            Arc::new(MockProvider::new("x").with_error(MockError::Network("down".into())));
        let sleepy: Arc<dyn AiProvider> = Arc::new(
            MockProvider::new("late").with_delay(Duration::from_secs(5)),
        );
        // Aggregator records what it saw via the guidance in its messages —
        // use a capturing stub.
        struct Capture(Arc<Mutex<String>>);
        impl AiProvider for Capture {
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
            fn name(&self) -> &str { "capture" }
            fn color(&self) -> &str { "#000" }
        }
        let seen = Arc::new(Mutex::new(String::new()));
        let p = make_provider(
            vec![(failing, "f:1"), (sleepy, "s:2")],
            Arc::new(Capture(seen.clone())),
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
        let grown = vec![UnifiedMessage::user("go"), UnifiedMessage::assistant("step")];
        p.process(RequestPayload::new(&grown)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1); // run-scoped: once only
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
    fn parse_one_shot_command_semantics() {
        use super::super::parse_one_shot_command;
        assert_eq!(parse_one_shot_command("/moa write a poem"), Some("write a poem"));
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
        let pref = SessionMoaPref { preset: None, one_shot: false };
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
            &SessionMoaPref { preset: Some("p".into()), one_shot: false },
            Some(&cfg),
            &named,
            None,
        )
        .err().unwrap();
        assert!(err.contains("ghost"));
    }
}
