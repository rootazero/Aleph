//! `ExecutionAdapter` trait implementation for `ExecutionEngine`.

use crate::sync_primitives::Arc;
use async_trait::async_trait;

use super::{engine::ExecutionEngine, ExecutionError, RunRequest, RunStatus};
use crate::executor::ToolRegistry;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

/// Implement `ExecutionAdapter` for the `ExecutionEngine`.
///
/// This allows `InboundMessageRouter` to use `ExecutionEngine` via a trait object,
/// enabling routing without being generic over provider and tool registry types.
#[async_trait]
impl<P, R> ExecutionAdapter for ExecutionEngine<P, R>
where
    P: ThinkerProviderRegistry + Send + Sync + 'static,
    R: ToolRegistry + Send + Sync + 'static,
{
    async fn execute(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        // Wrap the dyn trait object in DynEventEmitter to make it Sized,
        // then delegate to the existing generic execute method
        let wrapper = Arc::new(DynEventEmitter::new(emitter));
        Self::execute(self, request, agent, wrapper).await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Self::cancel(self, run_id).await
    }

    async fn cancel_session(
        &self,
        session_key: &crate::routing::session_key::SessionKey,
    ) -> Result<Option<String>, ExecutionError> {
        Self::cancel_session(self, session_key).await
    }

    async fn session_of_run(
        &self,
        run_id: &str,
    ) -> Option<crate::routing::session_key::SessionKey> {
        Self::session_of_run(self, run_id).await
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        Self::get_status(self, run_id).await
    }

    async fn active_run_count(&self) -> usize {
        Self::active_run_count(self).await
    }

    fn concurrency_snapshot(&self) -> super::ConcurrencySnapshot {
        Self::concurrency_snapshot(self)
    }

    fn running_sessions(&self) -> Vec<String> {
        Self::running_sessions(self)
    }

    fn active_run_for_session(&self, session_key: &str) -> Option<String> {
        Self::active_run_for_session(self, session_key)
    }
}
