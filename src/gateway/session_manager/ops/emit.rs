use super::{SessionManager, SessionState};

impl SessionManager {
    pub(super) fn emit_session_updated(&self, session_key: &str) {
        if let Some(bus) = &self.event_bus {
            let frame = crate::gateway::events::GatewayEventFrame::SessionUpdated {
                session_key: session_key.to_string(),
                // Topic/title/state updates have no triggering channel.
                origin_channel: None,
            };
            let _ = bus.publish_frame(&frame);
        }
    }

    pub(super) fn emit_session_lifecycle_changed(
        &self,
        session_key: &str,
        old_state: Option<&SessionState>,
        new_state: &str,
        reason: Option<&str>,
    ) {
        if let Some(bus) = &self.event_bus {
            let frame = crate::gateway::events::GatewayEventFrame::SessionLifecycleChanged {
                session_key: session_key.to_string(),
                old_state: old_state.map(|s| s.to_string()),
                new_state: new_state.to_string(),
                reason: reason.map(|s| s.to_string()),
            };
            let _ = bus.publish_frame(&frame);
        }
    }
}

/// Spec 1 G3-A: fire-and-forget emission of a `SessionEnd` raw-memory row.
///
/// Spawns a tokio task so `close_session` latency is unaffected.
/// Skips silently when `tail` is empty (nothing to digest).
///
/// Capture filters (Spec 4 Task 6): the `MemoryExtensionRegistry` is resolved
/// from the startup-registered `SESSION_END_MCP` cell rather than threaded as
/// a parameter — none of the three `close_session` call sites ever supplied
/// one, so session-close `SessionEnd` rows silently bypassed the `on_capture`
/// pipeline the `session_complete` tool path honours. Resolving it here closes
/// that gap with zero caller churn; no registered MCP (tests / early boot)
/// degrades to a direct insert, exactly the old behaviour.
/// `owner_user_id`/`scope_id` are the session row's persisted P1 columns
/// (`SessionMetadata`) — review-fix follow-up: the session-end reflector
/// (`SessionReflector::reflect`, which writes OPEN_LOOPS.md) runs in a
/// spawned task on a path that may be outside any run's task tree, so the
/// ambient `crate::scope::current_scope()` task-local is not reliable here.
/// Callers pass the persisted columns straight through; `emit_session_end`
/// re-derives the `ScopeAttribution` from them (never guesses).
pub(crate) fn emit_session_end_raw(
    writer: std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    agent_id: String,
    session_id: String,
    tail: String,
    reason: crate::memory::store::raw_memory::SessionEndReason,
    owner_user_id: Option<String>,
    scope_id: Option<String>,
) {
    emit_session_end(
        writer,
        agent_id,
        session_id,
        tail,
        reason,
        true,
        owner_user_id,
        scope_id,
    );
}

/// `sessions.delete` variant: identical capture, but it does NOT materialize a
/// `resume.json` snapshot.
///
/// The delete path used to manufacture one for the very conversation the user
/// just deleted — an LLM-authored summary of it written to disk and then
/// injected into the NEXT session's system prompt as "previous session"
/// context. The handler purges the session's existing snapshot directory, but a
/// purge alone cannot cover this one: the summarizer is an LLM call spawned
/// here and lands seconds AFTER the handler has finished, so the only way not
/// to leak the deleted conversation is not to write it. The `/end-summary` raw
/// row itself is unaffected — capturing the dying session's tail into
/// `raw_memories` is the delete handler's deliberate, documented behaviour.
pub(crate) fn emit_session_end_raw_without_resume(
    writer: std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    agent_id: String,
    session_id: String,
    tail: String,
    reason: crate::memory::store::raw_memory::SessionEndReason,
    owner_user_id: Option<String>,
    scope_id: Option<String>,
) {
    emit_session_end(
        writer,
        agent_id,
        session_id,
        tail,
        reason,
        false,
        owner_user_id,
        scope_id,
    );
}

/// Shared body of the two entry points above. `resume_snapshot` is the only
/// difference: whether the session-end summary is also persisted as this
/// session's `resume.json`.
fn emit_session_end(
    writer: std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    agent_id: String,
    session_id: String,
    tail: String,
    reason: crate::memory::store::raw_memory::SessionEndReason,
    resume_snapshot: bool,
    owner_user_id: Option<String>,
    scope_id: Option<String>,
) {
    // Re-derived once, cloned into whichever spawned task needs ambient
    // scope re-established (`crate::scope::with_scope` — task-locals don't
    // cross a `tokio::spawn` boundary). `None` for legacy/pre-P1/org rows —
    // those spawns proceed unscoped, exactly the pre-P1 behaviour.
    let scope_attr = crate::scope::ScopeAttribution::from_persisted(
        owner_user_id.as_deref(),
        scope_id.as_deref(),
    );
    if tail.is_empty() {
        return;
    }
    let raw = crate::memory::store::raw_memory::RawMemory::new(
        tail,
        crate::memory::store::raw_memory::RawMemorySource::SessionEnd { reason },
    )
    .with_agent(agent_id.clone())
    .with_session(session_id.clone());

    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let mcp = crate::thinker::memory_context_provider::session_end_mcp();
        let registry = mcp.as_ref().map(|m| m.extensions());
        if let Some(mcp) = mcp {
            let session_key = session_id.clone();
            rt.spawn(async move {
                mcp.invalidate_curated(&session_key).await;
            });
        }

        if let Some(summarizer) = crate::thinker::memory_context_provider::session_end_summarizer()
        {
            let b_agent_id = agent_id.clone();
            let b_session_id = session_id.clone();
            // Readiness gate, second producer. `HybridAssembler::assemble` opens
            // with `flush::global_registry().await_ready(agent_id, 2s)`, and the
            // ONLY thing that ever registered there was the Pillar-2 flush below
            // — while THIS task is the one that writes `resume.json`. So a user
            // who hit New chat and typed immediately got `await_ready` returning
            // at once and the prompt assembled before the previous session's
            // snapshot existed: the resume chain silently did nothing exactly
            // when it mattered most. Registering here makes the gate see it.
            //
            // Acquired SYNCHRONOUSLY, for the same reason spelled out at the
            // flush guard below: `rt.spawn` returns before the task is first
            // polled, so taking the guard inside the task races the very waiter
            // it exists for.
            //
            // This NARROWS the race, it does not close it: `produce` is an LLM
            // synthesis and will usually outrun the assembler's 2 s ceiling.
            // Raising that ceiling is a separate decision (it would make every
            // follow-on session's first turn wait longer) and is deliberately
            // not taken here.
            let summary_guard = crate::memory::flush::global_registry().begin(&b_agent_id);
            rt.spawn(async move {
                let _summary_guard = summary_guard;
                if let Err(e) = summarizer.produce(&b_agent_id, &b_session_id).await {
                    tracing::warn!(
                        target: "spec_b.end_hook",
                        agent_id = %b_agent_id,
                        session_id = %b_session_id,
                        error = %e,
                        "session-end summarization failed (non-fatal)"
                    );
                    return;
                }
                // Session-resume producer: materialize the /end-summary the
                // call above just guaranteed into `resume.json`, so the read
                // chain (`SnapshotReader` → `HybridAssembler::fetch_snapshot`
                // → memory envelope injected by the assembler) is no longer
                // starved. Reuses the summarizer's stored output — zero extra
                // LLM calls. Same fire-and-forget warn-on-error posture as
                // the sibling hooks. Skipped for a DELETED session — see
                // `emit_session_end_raw_without_resume`.
                if resume_snapshot {
                    write_resume_snapshot(&summarizer, &b_agent_id, &b_session_id).await;
                }
            });
        }

        // Batch 2 — session-end reflection (lessons learned). Only present when
        // [memory.reflection] enabled; the reflector self-gates on substance +
        // cooldown. Fire-and-forget, never blocks close_session.
        if let Some(reflector) = crate::thinker::memory_context_provider::session_reflector() {
            let r_agent_id = agent_id.clone();
            let r_session_id = session_id.clone();
            // Review fix: re-establish the session's scope inside the
            // spawned task (it does not cross the `tokio::spawn` boundary
            // otherwise), from the row's persisted columns, not a guess —
            // `SessionReflector::reflect` writes OPEN_LOOPS.md through
            // `session_write_id`, which needs the SAME active scope the
            // curated-envelope reader resolves, or a personal-scoped
            // owner's open loops go permanently stale after adoption while
            // a member reads a permanently empty block.
            let r_scope_attr = scope_attr.clone();
            rt.spawn(async move {
                let reflect = crate::scope::with_scope(
                    r_scope_attr,
                    reflector.reflect(&r_agent_id, &r_session_id),
                );
                if let Err(e) = reflect.await {
                    tracing::warn!(
                        target: "reflection.end_hook",
                        agent_id = %r_agent_id,
                        session_id = %r_session_id,
                        error = %e,
                        "session-end reflection failed (non-fatal)"
                    );
                }
            });
        }

        // Real-time session-end flush (Pillar 2). Two ordering hazards the old
        // shape got wrong, both fixed here:
        //   1. TOCTOU — the FlushGuard is acquired SYNCHRONOUSLY now (not inside
        //      the spawned task), so the registry entry is observable the instant
        //      close_session returns. Acquiring it in the task raced a back-to-back
        //      follow-on `await_ready`, which then saw an empty registry and
        //      no-op'd the gate (`rt.spawn` returns before the task is polled).
        //   2. Insert race — the flush runs in the TAIL of the raw-insert task
        //      below, AFTER this session's SessionEnd digest row is committed,
        //      so the digest is already queued when the drain starts (previously
        //      the two were unordered spawns and the freshest signal was usually
        //      excluded). Ordering alone is NOT sufficient: one
        //      `compress_to_notes` call takes the 50 OLDEST rows, while the
        //      digest is the newest, so `flush_agent_memory` loops until the
        //      backlog stops shrinking. Do not collapse that loop back to a
        //      single call — it silently reinstates the bug this ordering
        //      exists to fix.
        // Fire-and-forget — best-effort consolidation, never gates close_session.
        let flush = crate::thinker::memory_context_provider::session_end_compression().map(|cs| {
            let reg = crate::memory::flush::global_registry();
            let flush_agent = agent_id.clone();
            let guard = reg.begin(&flush_agent);
            (guard, flush_agent, cs)
        });

        rt.spawn(async move {
            if let Some(reg) = registry {
                let ctx = crate::memory::extensions::types::CaptureCtx {
                    agent_id,
                    namespace: crate::memory::namespace::NamespaceScope::Owner,
                    session_id: Some(session_id),
                    source_hint: "session_end".into(),
                };
                if let Err(e) =
                    crate::memory::extensions::insert_with_capture_filter(&writer, &reg, &ctx, raw)
                        .await
                {
                    tracing::warn!("session_end raw_memory write failed: {e}");
                }
            } else if let Err(e) = writer.insert_raw_memory(&raw).await {
                tracing::warn!("session_end raw_memory write failed: {e}");
            }

            // Pillar 2 flush runs only after the SessionEnd digest is committed
            // above, holding the synchronously-acquired guard (dropped at task
            // end → wakes any `await_ready` waiter for this agent).
            if let Some((guard, flush_agent, cs)) = flush {
                crate::memory::flush::flush_agent_memory(guard, flush_agent, cs).await;
            }
        });
    } else {
        tracing::warn!("no tokio runtime for session_end emit; skipping");
    }
}

/// Producer half of the session-resume chain: read back the `/end-summary`
/// row `SessionEndSummarizer::produce` just wrote and persist it as this
/// session's `resume.json` snapshot. Best-effort — every failure path is a
/// warn, never an error (P7: memory is degradable).
async fn write_resume_snapshot(
    summarizer: &crate::memory::session_search_summary::SessionEndSummarizer,
    agent_id: &str,
    session_id: &str,
) {
    let summary = match crate::memory::session_search_summary::retrieve_summary_fact(
        &summarizer.store,
        agent_id,
        session_id,
    )
    .await
    {
        Ok(Some(fact)) => fact.content,
        // `produce` can legitimately write nothing (e.g. empty transcript) —
        // then there is nothing to snapshot either.
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                target: "session_resume.end_hook",
                agent_id,
                session_id,
                error = %e,
                "resume snapshot: /end-summary readback failed (non-fatal)"
            );
            return;
        }
    };
    let Some(writer) = crate::memory::session_resume::SnapshotWriter::default_path() else {
        return;
    };
    // Off the async worker: `write_from_summary` is blocking file work — a
    // `create_dir_all`, an atomic write (temp + fsync + rename), and a
    // retention sweep that opens every snapshot directory and can
    // `remove_dir_all` several of them. Running that inline parked a tokio
    // worker for the whole sweep.
    //
    // `agent_id` is the fallback owner for session ids that don't parse as
    // gateway keys; the writer prefers the id embedded in the key itself.
    let owned_session = session_id.to_string();
    let owned_agent = agent_id.to_string();
    let written = tokio::task::spawn_blocking(move || {
        writer.write_from_summary(&owned_session, &summary, &owned_agent)
    })
    .await;
    let error: Option<String> = match written {
        Ok(Ok(_)) => None,
        Ok(Err(e)) => Some(e.to_string()),
        // The blocking task panicked or the runtime is shutting down; either
        // way no snapshot exists and the next session silently loses its
        // resume context, so say so.
        Err(e) => Some(e.to_string()),
    };
    if let Some(error) = error {
        tracing::warn!(
            target: "session_resume.end_hook",
            agent_id,
            session_id,
            error = %error,
            "resume snapshot write failed (non-fatal)"
        );
    }
}
