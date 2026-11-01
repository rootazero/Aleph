use super::{SessionManager, SessionState};
use crate::memory::store::raw_memory::{RawMemory, RawMemoryStore};
#[cfg(test)]
use crate::memory::store::raw_memory::{RawMemorySource, SessionEndReason};
use crate::scope::ScopeAttribution;
use crate::sync_primitives::Arc;

impl SessionManager {
    pub(super) fn emit_session_updated(&self, session_key: &str) {
        if let Some(bus) = &self.event_bus {
            // Session mutations are independent of any active execution run.
            let frame = crate::gateway::events::GatewayEventFrame::SessionUpdated {
                session_key: session_key.to_string(),
                origin_channel: None,
                origin_run_id: None,
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

    /// Raw persistence uses the session writer even when derived admission is closed.
    pub(crate) fn capture_session_end(
        &self,
        writer: Arc<dyn RawMemoryStore>,
        raw: RawMemory,
        scope: Option<ScopeAttribution>,
        resume_snapshot: bool,
    ) {
        capture_session_end(
            self.memory_runtime
                .as_ref()
                .and_then(std::sync::Weak::upgrade),
            writer,
            raw,
            scope,
            resume_snapshot,
        );
    }
}

pub(crate) fn capture_session_end(
    runtime: Option<Arc<crate::memory::runtime::MemoryRuntime>>,
    writer: Arc<dyn RawMemoryStore>,
    raw: RawMemory,
    scope: Option<ScopeAttribution>,
    resume_snapshot: bool,
) {
    if raw.content.is_empty() {
        return;
    }
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::warn!("no tokio runtime for session_end emit; skipping");
        return;
    }
    if let Some(runtime) = runtime {
        if let Ok(operation) = runtime.operation() {
            match operation.spawn_session_end(writer.clone(), raw.clone(), scope, resume_snapshot) {
                Ok(()) => return,
                Err(error) => tracing::warn!("session-end derived scheduling failed: {error}"),
            }
        }
    }
    emit_raw(writer, raw);
}

/// Standalone raw fixture entry; derived services require explicit runtime injection.
#[cfg(test)]
pub(crate) fn emit_session_end_raw(
    writer: Arc<dyn RawMemoryStore>,
    agent_id: String,
    session_id: String,
    tail: String,
    reason: SessionEndReason,
    _owner_user_id: Option<String>,
    _scope_id: Option<String>,
) {
    if tail.is_empty() {
        return;
    }
    emit_raw(
        writer,
        RawMemory::new(tail, RawMemorySource::SessionEnd { reason })
            .with_agent(agent_id)
            .with_session(session_id),
    );
}

fn emit_raw(writer: Arc<dyn RawMemoryStore>, raw: RawMemory) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            if let Err(error) = writer.insert_raw_memory(&raw).await {
                tracing::warn!("session_end raw_memory write failed: {error}");
            }
        });
        return;
    }
    tracing::warn!("no tokio runtime for session_end emit; skipping");
}
