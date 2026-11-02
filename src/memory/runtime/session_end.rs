use std::path::Path;

use crate::error::AlephError;
use crate::memory::extensions::types::CaptureCtx;
use crate::memory::namespace::NamespaceScope;
use crate::memory::session_resume::{snapshot_partition, SnapshotWriter};
use crate::memory::session_search_summary::{retrieve_summary_fact, SessionEndSummarizer};
use crate::memory::store::raw_memory::{RawMemory, RawMemoryStore};
use crate::scope::ScopeAttribution;
use crate::sync_primitives::Arc;

use super::MemoryOperation;

impl MemoryOperation {
    /// Admission and readiness guards are established before session close returns.
    pub(crate) fn spawn_session_end(
        self,
        writer: Arc<dyn RawMemoryStore>,
        raw: RawMemory,
        scope: Option<ScopeAttribution>,
        resume: bool,
    ) -> Result<(), AlephError> {
        let services = self.services()?;
        let agent = raw.agent_id.clone();
        let session = raw
            .session_id
            .clone()
            .ok_or_else(|| AlephError::other("session-end capture lacks session identity"))?;
        let summary_guard = services
            .summarizer
            .as_ref()
            .map(|_| crate::memory::flush::global_registry().begin(&agent));
        let flush = services.compression.as_ref().map(|compression| {
            (
                crate::memory::flush::global_registry().begin(&agent),
                compression.clone(),
            )
        });
        let operation = self.clone();
        self.spawn(super::carry(crate::scope::with_scope(scope, async move {
            let archive = async {
                let context = CaptureCtx {
                    agent_id: agent.clone(),
                    namespace: NamespaceScope::Owner,
                    session_id: Some(session.clone()),
                    source_hint: "session_end".into(),
                };
                if let Err(error) = crate::memory::extensions::insert_with_capture_filter(
                    &writer,
                    &services.context.extensions(),
                    &context,
                    raw,
                )
                .await
                {
                    tracing::warn!("session_end raw_memory write failed: {error}");
                }
                // Keep the existing digest-before-flush ordering and backlog loop.
                if let Some((guard, compression)) = flush {
                    crate::memory::flush::flush_agent_memory(guard, agent.clone(), compression)
                        .await;
                }
            };
            let summarize = async {
                let _guard = summary_guard;
                let Some(summarizer) = &services.summarizer else {
                    return;
                };
                if let Err(error) = summarizer.produce(&agent, &session).await {
                    tracing::warn!(target: "spec_b.end_hook", %agent, %session, %error,
                        "session-end summarization failed (non-fatal)");
                    return;
                }
                if resume {
                    write_resume_snapshot(
                        &operation,
                        summarizer,
                        &agent,
                        &session,
                        &services.snapshot_dir,
                    )
                    .await;
                }
            };
            let reflect = async {
                if let Some(reflector) = &services.reflector {
                    if let Err(error) = reflector.reflect(&agent, &session).await {
                        tracing::warn!(target: "reflection.end_hook", %agent, %session, %error,
                            "session-end reflection failed (non-fatal)");
                    }
                }
            };
            tokio::join!(
                archive,
                summarize,
                reflect,
                services.context.invalidate_curated(&session),
            );
        })))
        .map(|_| ())
    }
}

async fn write_resume_snapshot(
    operation: &MemoryOperation,
    summarizer: &SessionEndSummarizer,
    agent: &str,
    session: &str,
    directory: &Path,
) {
    let summary = match retrieve_summary_fact(&summarizer.store, agent, session).await {
        Ok(Some(fact)) => fact.content,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(target: "session_resume.end_hook", %agent, %session, %error,
                "resume snapshot: /end-summary readback failed (non-fatal)");
            return;
        }
    };
    // Resolve persisted attribution before crossing the blocking boundary.
    let partition = snapshot_partition(agent);
    let writer = SnapshotWriter::new(directory.to_path_buf());
    let owned_agent = agent.to_owned();
    let owned_session = session.to_owned();
    if let Err(error) = operation
        .run_blocking(move || {
            writer
                .write_from_summary_in_partition(&owned_session, &summary, &owned_agent, partition)
                .map(|_| ())
                .map_err(|error| AlephError::other(error.to_string()))
        })
        .await
    {
        tracing::warn!(target: "session_resume.end_hook", %agent, %session, %error,
            "resume snapshot write failed (non-fatal)");
    }
}
