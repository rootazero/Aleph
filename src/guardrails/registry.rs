//! `GuardrailRegistry` — aggregates all three guardrail surfaces behind
//! a single `Arc`-shareable handle. Constructed once at startup, held by
//! `HarnessDeps` as `Option<Arc<GuardrailRegistry>>`.
//!
//! Sequential evaluation per surface: stops at the first `Block` or
//! `Sanitize`. `Warn`s are collected along the way and the last one is
//! returned when nothing stronger fires. `disable_all()` flips an
//! `AtomicBool` so every evaluation short-circuits to `Allow` — the
//! high-risk runtime rollback knob from master spec § Stage 5.

use crate::sync_primitives::{Arc, AtomicBool, Ordering};

use serde_json::Value;

use crate::guardrails::decision::GuardrailDecision;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::thinker::nudges::REDACTED_USER_MESSAGE;

/// Outcome of [`GuardrailRegistry::screen_session_input`].
pub enum SessionInputScreen {
    /// The events the prompt is built from — an in-memory clone whose user
    /// text may have been rewritten. The persisted log is never touched, so
    /// the audit trail keeps the original.
    Pass(Vec<SessionEventRecord>),
    /// The message this turn is answering was blocked; the caller ends the
    /// turn without calling the provider.
    Blocked(String),
}

pub struct GuardrailRegistry {
    input: Vec<Arc<dyn InputGuardrail>>,
    output: Vec<Arc<dyn OutputGuardrail>>,
    tool_call: Vec<Arc<dyn ToolCallGuardrail>>,
    enabled: AtomicBool,
}

impl GuardrailRegistry {
    #[must_use]
    pub fn builder() -> GuardrailRegistryBuilder {
        GuardrailRegistryBuilder::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Runtime kill-switch — flips `enabled` to false. All three `evaluate_*`
    /// methods short-circuit to `Allow` until `enable_all()` is called.
    ///
    /// Emits an audit-level log entry on every state change. Anyone with a
    /// handle to `GuardrailRegistry` (sub-modules, callbacks, plugins) can
    /// call this — the log is what makes it possible to tell an operator
    /// action from a hostile one after the fact.
    pub fn disable_all(&self) {
        self.enabled.store(false, Ordering::Release);
        tracing::warn!(
            actor = crate::scope::current_scope()
                .map(|s| s.owner_user_id)
                .unwrap_or_else(|| "unknown".into()),
            "guardrails disabled (runtime kill-switch) — all evaluations now Allow"
        );
    }

    pub fn enable_all(&self) {
        self.enabled.store(true, Ordering::Release);
        tracing::warn!(
            actor = crate::scope::current_scope()
                .map(|s| s.owner_user_id)
                .unwrap_or_else(|| "unknown".into()),
            "guardrails re-enabled"
        );
    }

    pub fn input_count(&self) -> usize {
        self.input.len()
    }
    pub fn output_count(&self) -> usize {
        self.output.len()
    }
    pub fn tool_call_count(&self) -> usize {
        self.tool_call.len()
    }

    pub async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if !self.is_enabled() || self.input.is_empty() {
            return GuardrailDecision::Allow;
        }
        // Aggregate Warn reasons from ALL guardrails instead of keeping only
        // the last one. The previous `last_warn: Option<Warn>` silently
        // dropped every earlier warning, so operators saw at most one reason
        // even when three guardrails each fired (H2 in
        // review/guardrails-statics). Block / Sanitize still short-circuit.
        let mut warns: Vec<String> = Vec::new();
        for g in &self.input {
            let d = g.evaluate_input(text).await;
            match d {
                GuardrailDecision::Allow => continue,
                GuardrailDecision::Warn { reason } => warns.push(reason),
                _ => return d,
            }
        }
        if warns.is_empty() {
            GuardrailDecision::Allow
        } else {
            GuardrailDecision::Warn {
                reason: warns.join("; "),
            }
        }
    }

    /// Screen the user input the harness is about to replay into a prompt.
    /// EVERY non-synthetic `UserMessage` in the log is evaluated, not only the
    /// one that opened this turn: the harness rebuilds each prompt from the
    /// full raw log, so a rewrite that covered only the tail sent the original
    /// text to the provider again from turn 2 onwards. `SystemMessage` is
    /// screened on the same footing — it is compacted user content, and a
    /// summary that outlives the message it summarises would otherwise carry
    /// the redacted secret back onto the wire.
    ///
    /// `Block` is deliberately asymmetric. Only the tail's own user message —
    /// the one this turn is answering — ends the turn. A `Block` landing on an
    /// earlier message degrades to redaction, because session events are
    /// immutable and replayed forever: re-blocking on every rebuild would end
    /// every subsequent turn and brick the session with no way out, and the
    /// PII guardrail is fail-closed, so a transient secret-resolution error
    /// blocks too.
    ///
    /// Rewrites land on the returned clone only.
    pub async fn screen_session_input(
        &self,
        events: Vec<SessionEventRecord>,
        tail_start: usize,
    ) -> SessionInputScreen {
        let blocking = events[tail_start.min(events.len())..]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(rel, r)| user_text(&r.event).map(|_| tail_start + rel));
        let (events, blocked) = self.screen_user_messages(events, blocking).await;
        match blocked {
            Some(reason) => SessionInputScreen::Blocked(reason),
            None => SessionInputScreen::Pass(events),
        }
    }

    /// The salvage face of [`Self::screen_session_input`]: the boundary grace
    /// turn re-reads the RAW log and rebuilds the prompt from it, so it needs
    /// the same redaction. Nothing can end that turn (the run is already
    /// terminating and the grace call is the last chance at terminal text), so
    /// every `Block` degrades to redaction here.
    pub async fn redact_session_input(
        &self,
        events: Vec<SessionEventRecord>,
    ) -> Vec<SessionEventRecord> {
        self.screen_user_messages(events, None).await.0
    }

    /// Shared core. `blocking` is the one index whose `Block` ends the turn;
    /// a `Block` on any other message is redacted in place.
    async fn screen_user_messages(
        &self,
        mut events: Vec<SessionEventRecord>,
        blocking: Option<usize>,
    ) -> (Vec<SessionEventRecord>, Option<String>) {
        if !self.is_enabled() || self.input.is_empty() {
            return (events, None);
        }
        let mut blocked = None;
        for (idx, record) in events.iter_mut().enumerate() {
            let Some(text) = screenable_text(&record.event).map(str::to_string) else {
                continue;
            };
            match self.evaluate_input(&text).await {
                GuardrailDecision::Allow => {}
                GuardrailDecision::Warn { reason } => {
                    tracing::warn!(reason = %reason, "input guardrail warned");
                }
                GuardrailDecision::Sanitize(rep) => set_screened_text(&mut record.event, rep.text),
                GuardrailDecision::Block { reason, .. } if blocking == Some(idx) => {
                    blocked = Some(reason);
                    break;
                }
                GuardrailDecision::Block { reason, .. } => {
                    tracing::warn!(
                        seq = record.seq,
                        reason = %reason,
                        "input guardrail blocked a replayed user message; redacting it",
                    );
                    // Tag the redaction with the event's `seq` so the audit
                    // trail can tell two redacted messages apart. The previous
                    // single constant (`REDACTED_USER_MESSAGE`) made every
                    // redaction indistinguishable, so if user A pasted user
                    // B's secret and user C later pasted their own, both
                    // looked identical in the log — there was no way to
                    // correlate a redaction with the session seq that held
                    // the original.
                    set_screened_text(
                        &mut record.event,
                        format!("{REDACTED_USER_MESSAGE} [redacted:seq={}]", record.seq),
                    );
                }
            }
        }
        (events, blocked)
    }

    pub async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        if !self.is_enabled() || self.output.is_empty() {
            return GuardrailDecision::Allow;
        }
        let mut warns: Vec<String> = Vec::new();
        for g in &self.output {
            let d = g.evaluate_output(text).await;
            match d {
                GuardrailDecision::Allow => continue,
                GuardrailDecision::Warn { reason } => warns.push(reason),
                _ => return d,
            }
        }
        if warns.is_empty() {
            GuardrailDecision::Allow
        } else {
            GuardrailDecision::Warn {
                reason: warns.join("; "),
            }
        }
    }

    pub async fn evaluate_tool_call(&self, tool_name: &str, args: &Value) -> GuardrailDecision {
        if !self.is_enabled() || self.tool_call.is_empty() {
            return GuardrailDecision::Allow;
        }
        let mut warns: Vec<String> = Vec::new();
        for g in &self.tool_call {
            let d = g.evaluate_tool_call(tool_name, args).await;
            match d {
                GuardrailDecision::Allow => continue,
                GuardrailDecision::Warn { reason } => warns.push(reason),
                _ => return d,
            }
        }
        if warns.is_empty() {
            GuardrailDecision::Allow
        } else {
            GuardrailDecision::Warn {
                reason: warns.join("; "),
            }
        }
    }
}

/// Text of a real user message. Synthetic entries are the harness's own copy
/// (grace nudges, verifier vetoes, `MAX_STEPS` hints) — screening them would
/// let a fail-closed guardrail block the loop on text the loop itself wrote.
///
/// This is also the *blocking* candidate set: only a real user message can be
/// "the message this turn is answering", so only it may end a turn.
fn user_text(event: &SessionEvent) -> Option<&str> {
    match event {
        SessionEvent::UserMessage {
            content, synthetic, ..
        } if !*synthetic => Some(content.text.as_str()),
        _ => None,
    }
}

/// Everything the prompt builder replays as *user-authored* content. A
/// `SystemMessage` is the compaction summary a split child is rebuilt from —
/// it is a lossy restatement of earlier user messages and tool output, so a
/// secret redacted in the original survives in the summary unless it is
/// screened here too. It can never be the blocking message (see [`user_text`]),
/// so a `Block` on it always degrades to redaction.
fn screenable_text(event: &SessionEvent) -> Option<&str> {
    match event {
        SessionEvent::SystemMessage { content, .. } => Some(content.as_str()),
        _ => user_text(event),
    }
}

fn set_screened_text(event: &mut SessionEvent, text: String) {
    match event {
        SessionEvent::UserMessage { content, .. } => content.text = text,
        SessionEvent::SystemMessage { content, .. } => *content = text,
        _ => {}
    }
}

#[derive(Default)]
pub struct GuardrailRegistryBuilder {
    input: Vec<Arc<dyn InputGuardrail>>,
    output: Vec<Arc<dyn OutputGuardrail>>,
    tool_call: Vec<Arc<dyn ToolCallGuardrail>>,
}

impl GuardrailRegistryBuilder {
    pub fn with_input(mut self, g: Arc<dyn InputGuardrail>) -> Self {
        self.input.push(g);
        self
    }
    pub fn with_output(mut self, g: Arc<dyn OutputGuardrail>) -> Self {
        self.output.push(g);
        self
    }
    pub fn with_tool_call(mut self, g: Arc<dyn ToolCallGuardrail>) -> Self {
        self.tool_call.push(g);
        self
    }
    #[must_use]
    pub fn build(self) -> GuardrailRegistry {
        GuardrailRegistry {
            input: self.input,
            output: self.output,
            tool_call: self.tool_call,
            enabled: AtomicBool::new(true),
        }
    }
}

#[cfg(test)]
fn _assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<GuardrailRegistry>();
    check::<Arc<GuardrailRegistry>>();
}
