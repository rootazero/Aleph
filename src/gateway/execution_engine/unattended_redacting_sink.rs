//! `UnattendedRedactingSink` — secret redaction for unattended autonomous runs.
//!
//! Round 2 made unattended runs fail closed on tool confirmation. This closes
//! the observability side: when no human is watching, model-authored trace text
//! (which could echo a secret the loop just read) is run through `SecretMasker`
//! before it reaches persistence, the channel progress push, or the WebSocket
//! stream. Attended runs are never wrapped, so their trace path is unchanged.
//!
//! Lives in `src/gateway/` (a `TraceSink` consumer), not `src/harness/` (R10).

use std::sync::Arc;

use crate::exec::masker::{mask_json_strings, SecretMasker};
use crate::harness::trace::{LoopTraceEvent, ToolCallEndEvent, ToolCallStartEvent};
use crate::harness::TraceSink;

/// Mask every text leaf of `event` in place; `true` when anything changed.
///
/// # Why an exhaustive match and not a whitelist
///
/// This used to be three arms (`TextEmitted`, `SessionCompleted.final_text`,
/// `ToolCallCompleted`) plus `other => forward`. The wildcard was written for
/// `#[non_exhaustive]` forward-compatibility, which is precisely what made it
/// a hole: `ToolCallStarted` carries *the same* `call.input` bytes the
/// completed event carries (`harness/agent/act.rs` builds both from
/// `call.arguments`), it is forwarded to the WebSocket by
/// [`super::agent_trace_emit_sink::is_step_event`], and it is emitted
/// *seconds earlier* — so an unattended run put the credential on the wire
/// before the masked copy arrived, and the two events in one trace disagreed
/// about the same string. `VerifierVeto.reason` (the scratchpad pending-item
/// list) and `MoaAdvisor.text` (advisor model output) were unmasked for the
/// same reason.
///
/// So there is no whitelist any more and no judgement about which field is
/// "free text": **every** text leaf of **every** variant is masked, and every
/// arm destructures its variant *without* `..`, so both a new variant and a
/// new field on an existing variant are compile errors here rather than a
/// silent hole. Identifier-shaped fields (`agent_id`, advisor `label`, cache
/// `scope`) cost one regex pass that will not match — cheaper than a rule that
/// needs a person to re-classify each field correctly.
///
/// Every event reaching this function is on its way to at least one of
/// persistence (`task_traces`), the channel progress push, or the WS
/// `agent_trace` mirror, so "does this variant reach a user surface" is not a
/// question this function asks.
pub(crate) fn mask_trace_event(masker: &SecretMasker, event: &mut LoopTraceEvent) -> bool {
    match event {
        LoopTraceEvent::TextEmitted {
            iteration: _,
            stream: _,
            text,
        } => mask_in_place(masker, text),
        LoopTraceEvent::ToolCallStarted {
            iteration: _,
            call:
                ToolCallStartEvent {
                    tool_id,
                    tool_name,
                    input,
                },
        } => {
            let mut changed = mask_json_strings(masker, input);
            changed |= mask_in_place(masker, tool_id);
            changed |= mask_in_place(masker, tool_name);
            changed
        }
        LoopTraceEvent::ToolCallCompleted {
            iteration: _,
            call:
                ToolCallEndEvent {
                    tool_id,
                    tool_name,
                    input,
                    duration_ms: _,
                },
            result,
        } => {
            // Tool results are the highest-bandwidth secret channel in an
            // unattended run: a tool that read a credential echoes it in
            // `result` (which the scratchpad progress push sends to the bound
            // channel and `AgentTraceEmitSink` puts on the WS), and the model
            // can echo one into `call.input` (a scratchpad objective/plan).
            let mut changed = mask_json_strings(masker, input);
            changed |= mask_in_place(masker, tool_id);
            changed |= mask_in_place(masker, tool_name);
            match result {
                crate::tools::runtime::ToolResult::Success { output } => {
                    changed |= mask_json_strings(masker, output);
                }
                crate::tools::runtime::ToolResult::Error { error, .. } => {
                    changed |= mask_in_place(masker, error);
                }
            }
            changed
        }
        LoopTraceEvent::TurnStarted { iteration: _ } => false,
        LoopTraceEvent::TurnStateEntered {
            iteration: _,
            state: _,
        } => false,
        LoopTraceEvent::TurnCompleted {
            iteration: _,
            outcome: _,
            metrics: _,
        } => false,
        LoopTraceEvent::SessionCompleted {
            outcome: _,
            iterations: _,
            tool_calls_made: _,
            total_tokens: _,
            hit_limit: _,
            final_text,
            terminate_reason: _,
            duration_ms: _,
            token_breakdown: _,
            tool_timeline: _,
        } => final_text
            .as_mut()
            .is_some_and(|t| mask_in_place(masker, t)),
        LoopTraceEvent::WorktreeCreated { path } => mask_path_in_place(masker, path),
        LoopTraceEvent::WorktreeCleanedUp { path, leaked: _ } => {
            mask_path_in_place(masker, path)
        }
        LoopTraceEvent::McpScopeAttached {
            agent_id,
            references,
            inline_count: _,
        } => {
            let mut changed = mask_in_place(masker, agent_id);
            for r in references.iter_mut() {
                changed |= mask_in_place(masker, r);
            }
            changed
        }
        LoopTraceEvent::McpScopeCleaned {
            agent_id,
            leaked: _,
        } => mask_in_place(masker, agent_id),
        LoopTraceEvent::ProviderUsage {
            agent_id,
            input_tokens: _,
            output_tokens: _,
            cache_read_tokens: _,
            cache_creation_tokens: _,
            thinking_tokens: _,
        } => mask_in_place(masker, agent_id),
        LoopTraceEvent::ReactiveCompactionAttempted {
            token_gap: _,
            succeeded: _,
        } => false,
        LoopTraceEvent::VerifierVeto {
            iteration: _,
            reason,
        } => mask_in_place(masker, reason),
        LoopTraceEvent::MoaAdvisor {
            index: _,
            count: _,
            label,
            text,
            error,
        } => {
            let mut changed = mask_in_place(masker, label);
            changed |= mask_in_place(masker, text);
            if let Some(e) = error.as_mut() {
                changed |= mask_in_place(masker, e);
            }
            changed
        }
        LoopTraceEvent::MoaAggregating {
            aggregator,
            advisor_count: _,
            cached: _,
        } => mask_in_place(masker, aggregator),
        LoopTraceEvent::MoaAdvisorSpend {
            advisor_count: _,
            billed_count: _,
            input_tokens: _,
            output_tokens: _,
            cost_usd: _,
        } => false,
        LoopTraceEvent::MoaTurnTrace { preset, payload } => {
            let mut changed = mask_in_place(masker, preset);
            changed |= mask_json_strings(masker, payload);
            changed
        }
        LoopTraceEvent::CacheHealthDegraded {
            scope,
            streak: _,
            reads: _,
            writes: _,
            prefix_changed: _,
        } => mask_in_place(masker, scope),
    }
}

/// Mask `text` in place; `true` when it changed.
fn mask_in_place(masker: &SecretMasker, text: &mut String) -> bool {
    let masked = masker.mask(text);
    if masked == *text {
        false
    } else {
        *text = masked;
        true
    }
}

/// Same, for a path. Only rewritten when a secret actually matched, so a path
/// that is not valid UTF-8 is never round-tripped through `to_string_lossy`
/// on the overwhelmingly common clean path.
fn mask_path_in_place(masker: &SecretMasker, path: &mut std::path::PathBuf) -> bool {
    let text = path.to_string_lossy();
    let masked = masker.mask(&text);
    if masked == text {
        false
    } else {
        *path = std::path::PathBuf::from(masked);
        true
    }
}

/// Wraps an inner `TraceSink`, masking every text leaf of every
/// `LoopTraceEvent` before it reaches persistence, the channel progress push,
/// or the WebSocket `agent_trace` mirror. See [`mask_trace_event`] for why the
/// coverage is exhaustive rather than a whitelist.
pub struct UnattendedRedactingSink {
    inner: Arc<dyn TraceSink>,
    masker: SecretMasker,
}

impl UnattendedRedactingSink {
    #[must_use]
    pub fn new(inner: Arc<dyn TraceSink>) -> Self {
        Self {
            inner,
            masker: SecretMasker::new(),
        }
    }
}

impl TraceSink for UnattendedRedactingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        let mut ev = event.clone();
        if mask_trace_event(&self.masker, &mut ev) {
            self.inner.on_trace(&ev);
        } else {
            // Nothing matched: forward the original so the clean path is
            // byte-identical to an unwrapped sink.
            self.inner.on_trace(event);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::{LoopTraceSessionOutcome, LoopTraceTextKind};
    use std::sync::Mutex;

    #[derive(Default)]
    struct CaptureSink {
        events: Mutex<Vec<LoopTraceEvent>>,
    }
    impl TraceSink for CaptureSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[test]
    fn redacts_secret_in_text_emitted() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "the key is sk-ant-api03-AAAABBBBCCCCDDDD".into(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::TextEmitted { text, .. } => {
                assert!(
                    !text.contains("sk-ant-api03-AAAABBBBCCCCDDDD"),
                    "secret leaked: {text}"
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn passes_clean_text_through_unchanged() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "just a normal sentence".into(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::TextEmitted { text, .. } => assert_eq!(text, "just a normal sentence"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn redacts_secret_in_tool_call_completed_result_and_input() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: crate::harness::trace::ToolCallEndEvent {
                tool_id: "t1".into(),
                tool_name: "scratchpad".into(),
                input: serde_json::json!({
                    "action": "set_objective",
                    "objective": "rotate key sk-ant-api03-AAAABBBBCCCCDDDD"
                }),
                duration_ms: 5,
            },
            result: crate::tools::runtime::ToolResult::Success {
                output: serde_json::json!({
                    "content": "- [ ] use AKIAIOSFODNN7EXAMPLE to sign"
                }),
            },
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::ToolCallCompleted { call, result, .. } => {
                let input = call.input.to_string();
                assert!(
                    !input.contains("sk-ant-api03-AAAABBBBCCCCDDDD"),
                    "input leaked"
                );
                let crate::tools::runtime::ToolResult::Success { output } = result else {
                    panic!("expected success result");
                };
                assert!(
                    !output.to_string().contains("AKIAIOSFODNN7EXAMPLE"),
                    "result leaked: {output}"
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn clean_tool_call_completed_forwards_unchanged() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 2,
            call: crate::harness::trace::ToolCallEndEvent {
                tool_id: "t2".into(),
                tool_name: "read_file".into(),
                input: serde_json::json!({"path": "README.md"}),
                duration_ms: 3,
            },
            result: crate::tools::runtime::ToolResult::Success {
                output: serde_json::json!({"content": "plain text"}),
            },
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::ToolCallCompleted { call, .. } => {
                assert_eq!(call.input["path"], "README.md");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn redacts_secret_in_session_completed_final_text() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 1,
            tool_calls_made: 0,
            total_tokens: 0,
            hit_limit: false,
            final_text: Some("done, token AKIAIOSFODNN7EXAMPLE used".into()),
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::SessionCompleted { final_text, .. } => {
                assert!(!final_text
                    .as_ref()
                    .unwrap()
                    .contains("AKIAIOSFODNN7EXAMPLE"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// A real AWS key shape, so `SecretMasker`'s own pattern decides which
    /// bytes are a secret — not a pattern this test invented.
    const KEY: &str = "AKIAIOSFODNN7EXAMPLE";

    /// One instance of EVERY `LoopTraceEvent` variant, with [`KEY`] planted in
    /// every text leaf it has.
    ///
    /// Completeness is not trusted to this list — it is checked against the
    /// enum's own source by `every_trace_variant_has_a_fixture`, so a new
    /// variant fails by name here instead of silently escaping the masker.
    fn every_variant_with_a_planted_secret() -> Vec<LoopTraceEvent> {
        use crate::harness::trace::{LoopTraceState, LoopTraceTurnMetrics, LoopTraceTurnOutcome};
        vec![
            LoopTraceEvent::TextEmitted {
                iteration: 1,
                stream: LoopTraceTextKind::Final,
                text: format!("the key is {KEY}"),
            },
            LoopTraceEvent::ToolCallStarted {
                iteration: 1,
                call: ToolCallStartEvent {
                    tool_id: format!("t-{KEY}"),
                    tool_name: format!("n-{KEY}"),
                    input: serde_json::json!({ "objective": format!("rotate {KEY}") }),
                },
            },
            LoopTraceEvent::ToolCallCompleted {
                iteration: 1,
                call: ToolCallEndEvent {
                    tool_id: format!("t-{KEY}"),
                    tool_name: format!("n-{KEY}"),
                    input: serde_json::json!({ "objective": format!("rotate {KEY}") }),
                    duration_ms: 5,
                },
                result: crate::tools::runtime::ToolResult::Error {
                    error: format!("failed using {KEY}"),
                    retryable: false,
                },
            },
            LoopTraceEvent::TurnStarted { iteration: 1 },
            LoopTraceEvent::TurnStateEntered {
                iteration: 1,
                state: LoopTraceState::Think,
            },
            LoopTraceEvent::TurnCompleted {
                iteration: 1,
                outcome: LoopTraceTurnOutcome::Continue,
                metrics: LoopTraceTurnMetrics {
                    requested_tool_calls: 0,
                    executed_tool_calls: 0,
                    productive: true,
                    total_tokens: 0,
                },
            },
            LoopTraceEvent::SessionCompleted {
                outcome: LoopTraceSessionOutcome::Completed,
                iterations: 1,
                tool_calls_made: 0,
                total_tokens: 0,
                hit_limit: false,
                final_text: Some(format!("done, token {KEY} used")),
                terminate_reason: None,
                duration_ms: None,
                token_breakdown: None,
                tool_timeline: Vec::new(),
            },
            LoopTraceEvent::WorktreeCreated {
                path: std::path::PathBuf::from(format!("/tmp/{KEY}")),
            },
            LoopTraceEvent::WorktreeCleanedUp {
                path: std::path::PathBuf::from(format!("/tmp/{KEY}")),
                leaked: false,
            },
            LoopTraceEvent::McpScopeAttached {
                agent_id: format!("a-{KEY}"),
                references: vec![format!("r-{KEY}")],
                inline_count: 0,
            },
            LoopTraceEvent::McpScopeCleaned {
                agent_id: format!("a-{KEY}"),
                leaked: false,
            },
            LoopTraceEvent::ProviderUsage {
                agent_id: format!("a-{KEY}"),
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                thinking_tokens: None,
            },
            LoopTraceEvent::ReactiveCompactionAttempted {
                token_gap: None,
                succeeded: true,
            },
            LoopTraceEvent::VerifierVeto {
                iteration: 1,
                reason: format!("- [ ] rotate {KEY}"),
            },
            LoopTraceEvent::MoaAdvisor {
                index: 0,
                count: 1,
                label: format!("p:{KEY}"),
                text: format!("advice mentioning {KEY}"),
                error: Some(format!("boom {KEY}")),
            },
            LoopTraceEvent::MoaAggregating {
                aggregator: format!("agg-{KEY}"),
                advisor_count: 1,
                cached: false,
            },
            LoopTraceEvent::MoaAdvisorSpend {
                advisor_count: 1,
                billed_count: 1,
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: None,
            },
            LoopTraceEvent::MoaTurnTrace {
                preset: format!("preset-{KEY}"),
                payload: serde_json::json!({ "advice": format!("uses {KEY}") }),
            },
            LoopTraceEvent::CacheHealthDegraded {
                scope: format!("scope-{KEY}"),
                streak: 1,
                reads: 0,
                writes: 1,
                prefix_changed: None,
            },
        ]
    }

    /// `PascalCase` -> `snake_case`, matching what `#[serde(rename_all =
    /// "snake_case")]` puts in the `type` tag.
    fn to_snake(name: &str) -> String {
        let mut out = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i != 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Variant names read off `harness/trace.rs`'s own enum body, so the
    /// fixture's completeness is a property of the code and not of a list a
    /// person maintains.
    fn variant_names_from_source() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/harness/trace.rs");
        let src = crate::utils::source_scan::code_text(
            &std::fs::read_to_string(&path).expect("harness/trace.rs"),
        );
        let start = src
            .find("pub enum LoopTraceEvent")
            .expect("LoopTraceEvent enum declaration");
        let open = src[start..].find('{').expect("enum body") + start + 1;
        let body = &src[open..];

        let mut names = Vec::new();
        let mut depth = 1usize;
        let mut token = String::new();
        for c in body.chars() {
            match c {
                '{' | '(' | '[' => {
                    if depth == 1 && !token.is_empty() {
                        names.push(std::mem::take(&mut token));
                    }
                    token.clear();
                    depth += 1;
                }
                '}' | ')' | ']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    token.clear();
                }
                c if c.is_ascii_alphanumeric() || c == '_' => {
                    if depth == 1 {
                        token.push(c);
                    }
                }
                c if c.is_whitespace() => {}
                _ => token.clear(),
            }
        }
        names
    }

    /// The fixture above must name every variant the enum declares.
    ///
    /// Self-guard first: if the source walk stops finding variants, its green
    /// would mean nothing.
    #[test]
    fn every_trace_variant_has_a_fixture() {
        let declared = variant_names_from_source();
        assert!(
            declared.len() >= 19,
            "the enum-body walk found only {declared:?} — it stopped matching \
             `harness/trace.rs`, so this guard would pass vacuously"
        );

        let covered: std::collections::BTreeSet<String> = every_variant_with_a_planted_secret()
            .iter()
            .map(|e| {
                serde_json::to_value(e).expect("trace event serializes")["type"]
                    .as_str()
                    .expect("serde tag")
                    .to_string()
            })
            .collect();

        let missing: Vec<&String> = declared
            .iter()
            .filter(|v| !covered.contains(&to_snake(v)))
            .collect();
        assert!(
            missing.is_empty(),
            "these LoopTraceEvent variants have no redaction fixture, so nothing \
             proves the unattended sink masks them: {missing:?}"
        );
    }

    /// No text leaf of any trace event survives the unattended sink unmasked.
    ///
    /// This is the whole contract. It replaced a three-variant whitelist whose
    /// `other => forward` arm shipped `ToolCallStarted.call.input` — the same
    /// bytes the masked `ToolCallCompleted` carries, emitted seconds earlier —
    /// verbatim to the WS `agent_trace` mirror and the `task_traces` table.
    #[test]
    fn no_trace_event_reaches_the_inner_sink_with_an_unmasked_secret() {
        for event in every_variant_with_a_planted_secret() {
            let cap = Arc::new(CaptureSink::default());
            let sink = UnattendedRedactingSink::new(cap.clone());
            sink.on_trace(&event);
            let captured = cap.events.lock().unwrap_or_else(|e| e.into_inner());
            let rendered = serde_json::to_string(&captured[0]).expect("captured serializes");
            let tag = serde_json::to_value(&event).expect("tag")["type"].clone();
            assert!(
                !rendered.contains(KEY),
                "an unattended run forwarded a credential verbatim on {tag}: {rendered}"
            );
        }
    }
}
