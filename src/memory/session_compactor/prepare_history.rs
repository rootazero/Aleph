use super::{extract_depth, session_message_to_unified, SessionCompactor};
use crate::extension::hooks::{HookContext, HookExecutor};
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::router::SessionKey;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::message::UnifiedMessage;
use crate::thinker::xml_util::escape_xml_attr;
use tracing::{debug, warn};

impl SessionCompactor {
    /// Assemble compressed history for a new agent loop turn.
    ///
    /// `hooks` is the per-run extension `HookExecutor` snapshot. When history
    /// is actually compacted (summaries replace raw turns), `BeforeCompaction`
    /// fires just before the summaries are assembled and `AfterCompaction`
    /// fires once the compacted history is ready. `AfterCompaction` is
    /// observer-only; `BeforeCompaction` fires observers (fire-and-forget,
    /// back-compat) *and* Interceptor-kind hooks, whose `context:` lines are
    /// pinned into the compacted history so operator-defined facts survive
    /// every compaction (mirrors opencode's `experimental.session.compacting`).
    /// Turns that fall through without compaction (compaction disabled, short
    /// history, summary-fetch failure) fire nothing.
    pub async fn prepare_history(
        &self,
        agent: &AgentInstance,
        session_key: &SessionKey,
        _current_input: &str,
        token_budget: u64,
        hooks: Option<&HookExecutor>,
    ) -> Vec<UnifiedMessage> {
        tracing::info!(target: "session_compactor", "prepare");

        if !self.config.enabled {
            let raw = agent.get_history(session_key, None).await;
            return raw
                .into_iter()
                .map(|m| session_message_to_unified(&m))
                .collect();
        }

        let raw_messages = agent.get_history(session_key, None).await;
        if raw_messages.len() <= self.config.fresh_tail_count {
            return raw_messages
                .iter()
                .map(session_message_to_unified)
                .collect();
        }

        let session_id = session_key.to_key_string();
        // Resolve the storage agent id the same way `post_turn_compress` does
        // when it writes the d0/d1/d2 summaries. Reading under the unscoped base
        // id while writes are project-scoped (or vice versa) would match nothing
        // and silently skip every prior summary.
        let agent_id = crate::memory::project_scope::scoped_or_base(
            agent.id(),
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        );
        let path_prefix = format!("aleph://session/{session_id}/");

        let mut summaries = match self
            .database
            .get_raw_by_path_prefix(&path_prefix, &agent_id, 200)
            .await
        {
            Ok(raws) => raws,
            Err(e) => {
                warn!(
                    error = %e,
                    session = %session_id,
                    "Failed to fetch session summaries, falling back to raw history"
                );
                let raw = agent.get_history(session_key, None).await;
                return raw
                    .into_iter()
                    .map(|m| session_message_to_unified(&m))
                    .collect();
            }
        };

        // Compaction is now committed: raw history exceeds the fresh tail and
        // the summaries are in hand. Announce it before assembling the result.
        // Interceptor-kind `BeforeCompaction` hooks may pin steering context
        // that must survive this (and every future) compaction.
        let pinned_contexts = fire_before_compaction(
            hooks,
            &session_id,
            vec![
                ("COMPACTION_RAW_MESSAGES", raw_messages.len().to_string()),
                (
                    "COMPACTION_FRESH_TAIL",
                    self.config.fresh_tail_count.to_string(),
                ),
                ("COMPACTION_TOKEN_BUDGET", token_budget.to_string()),
                ("COMPACTION_SUMMARY_CANDIDATES", summaries.len().to_string()),
            ],
        )
        .await;

        summaries.sort_by(|a, b| {
            let pa = a.path.as_deref().unwrap_or("");
            let pb = b.path.as_deref().unwrap_or("");
            let da = extract_depth(pa);
            let db = extract_depth(pb);
            // Within a depth, order chronologically. The path holds a decimal
            // seq, so a lexicographic compare alone misorders seq >= 10
            // ("d0/10" < "d0/2"); created_at preserves the true sequence.
            db.cmp(&da)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| pa.cmp(pb))
        });

        let tail_start = if raw_messages.len() > self.config.fresh_tail_count {
            raw_messages.len() - self.config.fresh_tail_count
        } else {
            0
        };
        let fresh_tail: Vec<UnifiedMessage> = raw_messages[tail_start..]
            .iter()
            .map(session_message_to_unified)
            .collect();

        let mut result: Vec<UnifiedMessage> = Vec::new();
        let ratio = self.config.token_estimate_ratio;
        let budget = token_budget as usize;
        let mut used_tokens: usize = 0;

        // Operator-pinned context from `BeforeCompaction` interceptor hooks
        // leads the compacted history (ahead of the depth-ranked summaries) so
        // persistent facts — deployment targets, user preferences, project
        // invariants — survive every compaction. Counted against the same
        // budget so a verbose hook can't blow it out; the summary loop below
        // sees the reduced `used_tokens`.
        for ctx_text in &pinned_contexts {
            let escaped = escape_xml_attr(ctx_text);
            let xml = format!("<session_context depth=\"hook\">\n{escaped}\n</session_context>");
            used_tokens += super::context_window::estimate_tokens(&xml, ratio);
            result.push(UnifiedMessage::user(xml));
        }

        let tail_tokens: usize = fresh_tail
            .iter()
            .map(|m| super::context_window::estimate_tokens(&m.text_content(), ratio))
            .sum();

        let summary_budget = budget.saturating_sub(tail_tokens);

        for fact in &summaries {
            let path = fact.path.as_deref().unwrap_or("");
            let depth = extract_depth(path);
            let summary_tokens = super::context_window::estimate_tokens(&fact.content, ratio);

            if used_tokens + summary_tokens > summary_budget {
                debug!(
                    session = %session_id,
                    used_tokens,
                    summary_budget,
                    "Summary token budget exhausted, evicting remaining summaries"
                );
                break;
            }

            let xml_content = format!(
                "<session_context depth=\"d{depth}\">\n{}\n</session_context>",
                escape_xml_attr(&fact.content)
            );
            result.push(UnifiedMessage::user(xml_content));
            used_tokens += summary_tokens;
        }

        result.extend(fresh_tail);

        let tail_len = raw_messages[tail_start..].len();
        let injected = result.len().saturating_sub(tail_len);

        debug!(
            session = %session_id,
            summary_count = summaries.len(),
            injected,
            tail_count = tail_len,
            total_tokens_est = used_tokens + tail_tokens,
            "Prepared session history"
        );

        fire_compaction_hook(
            hooks,
            HookEvent::AfterCompaction,
            &session_id,
            vec![
                ("COMPACTION_RAW_MESSAGES", raw_messages.len().to_string()),
                ("COMPACTION_SUMMARIES_INJECTED", injected.to_string()),
                ("COMPACTION_MESSAGES_AFTER", result.len().to_string()),
                (
                    "COMPACTION_TOKENS_AFTER",
                    (used_tokens + tail_tokens).to_string(),
                ),
            ],
        )
        .await;

        result
    }
}

/// Fire a compaction lifecycle hook as an observer. No-op when no executor is
/// wired or it carries no hooks — compaction stats are surfaced via env vars.
async fn fire_compaction_hook(
    hooks: Option<&HookExecutor>,
    event: HookEvent,
    session_id: &str,
    stats: Vec<(&'static str, String)>,
) {
    let Some(executor) = hooks else {
        return;
    };
    if executor.hook_count() == 0 {
        return;
    }
    let mut ctx = HookContext::new(session_id);
    for (key, value) in stats {
        ctx = ctx.with_env(key, value);
    }
    executor.execute_observers(event, &ctx).await;
}

/// Fire `BeforeCompaction` hooks and harvest any context they pin into the
/// compacted history.
///
/// Dual-dispatch mirrors the tool-call seam in `tools::scoped::dispatch`:
/// Observer-kind hooks run in parallel, fire-and-forget (unchanged
/// back-compat behaviour — observers can never steer compaction); then
/// Interceptor-kind hooks run sequentially in priority order and their
/// `context:` lines / `additionalContext` JSON are returned so the caller can
/// pin them ahead of the summaries. This is the Rust analogue of opencode's
/// `experimental.session.compacting` (`context: string[]`).
///
/// A no-op returning an empty `Vec` when no executor is wired or it carries no
/// hooks. Interceptor failures degrade to no pinned context (compaction still
/// proceeds) rather than aborting the turn.
async fn fire_before_compaction(
    hooks: Option<&HookExecutor>,
    session_id: &str,
    stats: Vec<(&'static str, String)>,
) -> Vec<String> {
    let Some(executor) = hooks else {
        return Vec::new();
    };
    if executor.hook_count() == 0 {
        return Vec::new();
    }
    let mut ctx = HookContext::new(session_id);
    for (key, value) in stats {
        ctx = ctx.with_env(key, value);
    }
    // Observers: unchanged fire-and-forget behaviour (back-compat).
    executor
        .execute_observers(HookEvent::BeforeCompaction, &ctx)
        .await;
    // Interceptors: new — harvest pinned context for the compacted history.
    match executor
        .execute_interceptors(HookEvent::BeforeCompaction, ctx)
        .await
    {
        // Pinned context survives compaction by definition, so an unbounded
        // block here is the most expensive of all four hook-context seams —
        // it outlives the very history the compactor is shrinking. Same
        // budget, same spill-to-disk recovery path.
        Ok((_ctx, hook_result)) => {
            crate::extension::hooks::budget_hook_contexts(
                session_id,
                hook_result.additional_contexts,
            )
            .await
        }
        Err(e) => {
            warn!(error = %e, "BeforeCompaction interceptor hooks failed; no context pinned");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{HookAction, HookConfig, HookKind, HookPriority};

    #[cfg(unix)]
    fn observer_touch_hook(event: HookEvent, sentinel: &std::path::Path) -> HookConfig {
        let cmd = if cfg!(windows) {
            format!("type nul > \"{}\"", sentinel.display())
        } else {
            format!("touch '{}'", sentinel.display())
        };
        HookConfig {
            event,
            kind: HookKind::Observer,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command { command: cmd }],
            plugin_name: "compaction-test".to_string(),
            plugin_root: std::env::temp_dir(),
            handler: None,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn fire_compaction_hook_no_executor_is_noop() {
        // Must not panic when no executor is wired.
        fire_compaction_hook(None, HookEvent::BeforeCompaction, "s", vec![]).await;
    }

    #[tokio::test]
    async fn fire_compaction_hook_empty_executor_is_noop() {
        let executor = HookExecutor::new(vec![]);
        fire_compaction_hook(Some(&executor), HookEvent::AfterCompaction, "s", vec![]).await;
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell observer hook uses sh / touch fixtures
    async fn fire_compaction_hook_runs_only_the_matching_observer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().join("after.flag");
        let executor = HookExecutor::new(vec![observer_touch_hook(
            HookEvent::AfterCompaction,
            &sentinel,
        )]);

        // A mismatched event must not trigger the AfterCompaction hook.
        fire_compaction_hook(Some(&executor), HookEvent::BeforeCompaction, "s", vec![]).await;
        assert!(!sentinel.exists(), "wrong-event hook must not run");

        // The matching event runs the observer command.
        fire_compaction_hook(
            Some(&executor),
            HookEvent::AfterCompaction,
            "s",
            vec![("COMPACTION_RAW_MESSAGES", "42".to_string())],
        )
        .await;
        assert!(sentinel.exists(), "AfterCompaction observer must run");
    }

    /// An Interceptor-kind `BeforeCompaction` hook pins its `context` into the
    /// compacted history (opencode `experimental.session.compacting` parity).
    /// A `Prompt` action's resolved text lands in `additional_contexts` — the
    /// same harvest the production seam injects ahead of the summaries.
    #[tokio::test]
    async fn before_compaction_interceptor_pins_context() {
        let hook = HookConfig {
            event: HookEvent::BeforeCompaction,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Prompt {
                prompt: "remember: deploy target is AWS us-east-1".to_string(),
            }],
            plugin_name: "compaction-pin".to_string(),
            plugin_root: std::env::temp_dir(),
            handler: None,
            timeout_secs: None,
        };
        let executor = HookExecutor::new(vec![hook]);
        let pinned = fire_before_compaction(Some(&executor), "s", vec![]).await;
        assert_eq!(
            pinned,
            vec!["remember: deploy target is AWS us-east-1".to_string()]
        );
    }

    /// Back-compat: an Observer-kind `BeforeCompaction` hook still fires
    /// (fire-and-forget) but pins NOTHING — observers can never steer
    /// compaction, so the compacted history stays byte-identical.
    #[tokio::test]
    #[cfg(unix)] // POSIX-only: shell observer hook uses sh / touch fixtures
    async fn before_compaction_observer_only_pins_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().join("before.flag");
        let executor = HookExecutor::new(vec![observer_touch_hook(
            HookEvent::BeforeCompaction,
            &sentinel,
        )]);

        let pinned = fire_before_compaction(Some(&executor), "s", vec![]).await;

        assert!(pinned.is_empty(), "observer hooks must not pin context");
        assert!(
            sentinel.exists(),
            "BeforeCompaction observer must still run"
        );
    }

    #[tokio::test]
    async fn fire_before_compaction_no_executor_is_noop() {
        assert!(fire_before_compaction(None, "s", vec![]).await.is_empty());
    }

    #[tokio::test]
    async fn fire_before_compaction_empty_executor_is_noop() {
        let executor = HookExecutor::new(vec![]);
        assert!(fire_before_compaction(Some(&executor), "s", vec![])
            .await
            .is_empty());
    }
}
