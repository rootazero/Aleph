//! Session Scheduler
//!
//! Enforces per-session serial execution: messages to the same session are queued
//! and processed one at a time, while different sessions run in parallel.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::sync_primitives::Arc;

use super::agent_instance::AgentRegistry;
use super::channel_registry::ChannelRegistry;
use super::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use super::execution_adapter::ExecutionAdapter;
use super::execution_engine::RunRequest;
use super::pipeline::EnrichedMessage;
use super::reply_emitter::ReplyEmitter;

/// Maximum age for a queued task before it is dropped (5 minutes).
const MAX_QUEUE_AGE: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// SessionQueue (private)
// ---------------------------------------------------------------------------

/// Per-session FIFO queue with tracking of the currently active run.
struct SessionQueue {
    pending: VecDeque<QueuedTask>,
    active_run_id: Option<String>,
}

impl SessionQueue {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            active_run_id: None,
        }
    }

    /// Whether the session is idle (no active run).
    fn is_idle(&self) -> bool {
        self.active_run_id.is_none()
    }
}

// ---------------------------------------------------------------------------
// QueuedTask (private)
// ---------------------------------------------------------------------------

/// A message waiting in the session queue.
struct QueuedTask {
    enriched: EnrichedMessage,
    enqueued_at: Instant,
}

// ---------------------------------------------------------------------------
// SessionScheduler
// ---------------------------------------------------------------------------

/// Enforces per-session serial execution.
///
/// Same session queues; different sessions run in parallel.
pub struct SessionScheduler {
    queues: Arc<Mutex<HashMap<String, SessionQueue>>>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry: Arc<ChannelRegistry>,
}

impl SessionScheduler {
    /// Create a new session scheduler.
    pub fn new(
        execution_adapter: Arc<dyn ExecutionAdapter>,
        agent_registry: Arc<AgentRegistry>,
        channel_registry: Arc<ChannelRegistry>,
    ) -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            execution_adapter,
            agent_registry,
            channel_registry,
        }
    }

    /// Enqueue a message for execution.
    ///
    /// If the session is idle the message executes immediately; otherwise it is
    /// appended to the session's queue and will run once the current run completes.
    pub async fn enqueue(&self, enriched: EnrichedMessage) {
        let session_key_str = enriched.merged.primary_context.session_key.to_key_string();

        let mut queues = self.queues.lock().await;
        let queue = queues
            .entry(session_key_str.clone())
            .or_insert_with(SessionQueue::new);

        if queue.is_idle() {
            // Execute immediately — drop the lock first.
            drop(queues);
            self.execute_enriched(enriched, &session_key_str).await;
        } else {
            debug!(
                session = %session_key_str,
                depth = queue.pending.len() + 1,
                "Session busy — queuing message"
            );
            queue.pending.push_back(QueuedTask {
                enriched,
                enqueued_at: Instant::now(),
            });
        }
    }

    /// Return the number of pending (not yet executing) tasks for a session.
    pub fn queue_depth<'a>(&'a self, session_key: &str) -> QueueDepthFuture<'a> {
        QueueDepthFuture {
            queues: &self.queues,
            session_key: session_key.to_string(),
        }
    }

    /// Execute an enriched message on its session.
    async fn execute_enriched(&self, enriched: EnrichedMessage, session_key_str: &str) {
        let ctx = &enriched.merged.primary_context;
        let agent_id = ctx.session_key.agent_id().to_string();
        let run_id = Uuid::new_v4().to_string();

        // Set active run id
        {
            let mut queues = self.queues.lock().await;
            let queue = queues
                .entry(session_key_str.to_string())
                .or_insert_with(SessionQueue::new);
            queue.active_run_id = Some(run_id.clone());
        }

        // Resolve agent
        let agent = match self.agent_registry.get(&agent_id).await {
            Some(a) => a,
            None => {
                error!(agent_id = %agent_id, "Agent not found — dropping message");
                // Clear active run
                let mut queues = self.queues.lock().await;
                if let Some(queue) = queues.get_mut(session_key_str) {
                    queue.active_run_id = None;
                }
                return;
            }
        };

        // Build reply emitter
        let reply_emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(ReplyEmitter::new(
            self.channel_registry.clone(),
            enriched.merged.primary_context.reply_route.clone(),
            run_id.clone(),
        ));

        // Wrap with scheduler event listener
        let listener: Arc<dyn EventEmitter + Send + Sync> =
            Arc::new(SchedulerEventListener {
                inner: reply_emitter,
                queues: Arc::clone(&self.queues),
                session_key: session_key_str.to_string(),
                execution_adapter: Arc::clone(&self.execution_adapter),
                agent_registry: Arc::clone(&self.agent_registry),
                channel_registry: Arc::clone(&self.channel_registry),
            });

        // Build metadata
        let mut metadata = HashMap::new();
        metadata.insert(
            "channel_id".to_string(),
            ctx.message.channel_id.as_str().to_string(),
        );
        metadata.insert(
            "sender_id".to_string(),
            ctx.message.sender_id.as_str().to_string(),
        );
        metadata.insert("is_group".to_string(), ctx.message.is_group.to_string());
        metadata.insert("is_mentioned".to_string(), ctx.is_mentioned.to_string());

        let request = RunRequest {
            run_id: run_id.clone(),
            input: enriched.enriched_text.clone(),
            session_key: enriched.merged.primary_context.session_key.clone(),
            timeout_secs: None,
            metadata,
        };

        info!(
            run_id = %run_id,
            session = %session_key_str,
            agent = %agent_id,
            "Spawning execution"
        );

        let adapter = Arc::clone(&self.execution_adapter);
        tokio::spawn(async move {
            if let Err(e) = adapter.execute(request, agent, listener).await {
                error!(run_id = %run_id, error = %e, "Execution failed");
            }
        });
    }
}

/// Future wrapper for `queue_depth` so it can be called without `.await` in sync
/// contexts while still taking the async Mutex lock.
pub struct QueueDepthFuture<'a> {
    queues: &'a Mutex<HashMap<String, SessionQueue>>,
    session_key: String,
}

impl<'a> QueueDepthFuture<'a> {
    /// Await the queue depth.
    pub async fn get(self) -> usize {
        let queues = self.queues.lock().await;
        queues
            .get(&self.session_key)
            .map(|q| q.pending.len())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// SchedulerEventListener (private)
// ---------------------------------------------------------------------------

/// Wraps an inner `EventEmitter` and triggers the next queued task on run completion.
struct SchedulerEventListener {
    inner: Arc<dyn EventEmitter + Send + Sync>,
    queues: Arc<Mutex<HashMap<String, SessionQueue>>>,
    session_key: String,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry: Arc<ChannelRegistry>,
}

impl SchedulerEventListener {
    /// Called when a run completes or errors — drains expired tasks and starts
    /// the next one if available.
    async fn on_run_finished(&self) {
        let next_task = {
            let mut queues = self.queues.lock().await;
            if let Some(queue) = queues.get_mut(&self.session_key) {
                queue.active_run_id = None;

                // Drop expired tasks
                let before = queue.pending.len();
                queue
                    .pending
                    .retain(|t| t.enqueued_at.elapsed() < MAX_QUEUE_AGE);
                let dropped = before - queue.pending.len();
                if dropped > 0 {
                    warn!(
                        session = %self.session_key,
                        dropped,
                        "Dropped expired queued tasks"
                    );
                }

                queue.pending.pop_front()
            } else {
                None
            }
        };

        if let Some(task) = next_task {
            debug!(
                session = %self.session_key,
                "Dequeuing next task for session"
            );
            // We need to execute the next task. Build a temporary scheduler-like
            // context inline to avoid circular references.
            execute_next(
                task.enriched,
                &self.session_key,
                Arc::clone(&self.queues),
                Arc::clone(&self.execution_adapter),
                Arc::clone(&self.agent_registry),
                Arc::clone(&self.channel_registry),
            )
            .await;
        }
    }
}

#[async_trait]
impl EventEmitter for SchedulerEventListener {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Always forward to inner first
        let result = self.inner.emit(event.clone()).await;

        // On terminal events, trigger the next queued task
        match &event {
            StreamEvent::RunComplete { .. } | StreamEvent::RunError { .. } => {
                self.on_run_finished().await;
            }
            _ => {}
        }

        result
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}

// ---------------------------------------------------------------------------
// Standalone helper (avoids circular reference through SessionScheduler)
// ---------------------------------------------------------------------------

/// Execute the next enriched message for a session.
///
/// This is a free function so that `SchedulerEventListener` can trigger
/// execution without holding a reference to `SessionScheduler`.
async fn execute_next(
    enriched: EnrichedMessage,
    session_key_str: &str,
    queues: Arc<Mutex<HashMap<String, SessionQueue>>>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    channel_registry: Arc<ChannelRegistry>,
) {
    let ctx = &enriched.merged.primary_context;
    let agent_id = ctx.session_key.agent_id().to_string();
    let run_id = Uuid::new_v4().to_string();

    // Set active run id
    {
        let mut qs = queues.lock().await;
        let queue = qs
            .entry(session_key_str.to_string())
            .or_insert_with(SessionQueue::new);
        queue.active_run_id = Some(run_id.clone());
    }

    // Resolve agent
    let agent = match agent_registry.get(&agent_id).await {
        Some(a) => a,
        None => {
            error!(agent_id = %agent_id, "Agent not found — dropping queued message");
            let mut qs = queues.lock().await;
            if let Some(queue) = qs.get_mut(session_key_str) {
                queue.active_run_id = None;
            }
            return;
        }
    };

    // Build reply emitter
    let reply_emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(ReplyEmitter::new(
        channel_registry.clone(),
        enriched.merged.primary_context.reply_route.clone(),
        run_id.clone(),
    ));

    // Wrap with a new listener for the next run
    let listener: Arc<dyn EventEmitter + Send + Sync> =
        Arc::new(SchedulerEventListener {
            inner: reply_emitter,
            queues: Arc::clone(&queues),
            session_key: session_key_str.to_string(),
            execution_adapter: Arc::clone(&execution_adapter),
            agent_registry: Arc::clone(&agent_registry),
            channel_registry: Arc::clone(&channel_registry),
        });

    // Build metadata
    let mut metadata = HashMap::new();
    metadata.insert(
        "channel_id".to_string(),
        ctx.message.channel_id.as_str().to_string(),
    );
    metadata.insert(
        "sender_id".to_string(),
        ctx.message.sender_id.as_str().to_string(),
    );
    metadata.insert("is_group".to_string(), ctx.message.is_group.to_string());
    metadata.insert("is_mentioned".to_string(), ctx.is_mentioned.to_string());

    let request = RunRequest {
        run_id: run_id.clone(),
        input: enriched.enriched_text.clone(),
        session_key: enriched.merged.primary_context.session_key.clone(),
        timeout_secs: None,
        metadata,
    };

    info!(
        run_id = %run_id,
        session = %session_key_str,
        agent = %agent_id,
        "Spawning queued execution"
    );

    let adapter = execution_adapter;
    tokio::spawn(async move {
        if let Err(e) = adapter.execute(request, agent, listener).await {
            error!(run_id = %run_id, error = %e, "Queued execution failed");
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_queue_new() {
        let queue = SessionQueue::new();
        assert!(queue.is_idle());
        assert!(queue.pending.is_empty());
        assert!(queue.active_run_id.is_none());
    }

    #[tokio::test]
    async fn test_queue_depth_empty() {
        use crate::gateway::event_emitter::NoOpEventEmitter;
        use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunStatus};
        use crate::gateway::agent_instance::AgentInstance;

        // Minimal mock adapter
        struct NoOpAdapter;

        #[async_trait]
        impl ExecutionAdapter for NoOpAdapter {
            async fn execute(
                &self,
                _request: RunRequest,
                _agent: Arc<AgentInstance>,
                _emitter: Arc<dyn EventEmitter + Send + Sync>,
            ) -> Result<(), ExecutionError> {
                Ok(())
            }
            async fn cancel(&self, _run_id: &str) -> Result<(), ExecutionError> {
                Ok(())
            }
            async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
                None
            }
        }

        let scheduler = SessionScheduler::new(
            Arc::new(NoOpAdapter),
            Arc::new(AgentRegistry::new()),
            Arc::new(ChannelRegistry::new()),
        );

        // Nonexistent session should return 0
        let depth = scheduler.queue_depth("nonexistent").get().await;
        assert_eq!(depth, 0);
    }
}
