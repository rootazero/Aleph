//! Task Observer with Gap-Fill
//!
//! Implements real-time event observation with self-healing capability.
//! Detects sequence gaps and backfills from database.

use crate::error::AlephError;
use crate::resilience::AgentEvent;
use crate::resilience::database::StateDatabase;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Gap detection and fill result
#[derive(Debug, Clone)]
pub struct GapFillResult {
    /// Number of gaps detected
    pub gaps_detected: usize,

    /// Number of events recovered from database
    pub events_recovered: usize,

    /// Any gaps that could not be filled
    pub unfilled_gaps: Vec<(u64, u64)>,
}

/// Task observation state
#[derive(Debug)]
struct TaskState {
    /// Last seen sequence number
    last_seq: u64,

    /// Events received out of order (pending reorder)
    pending: HashMap<u64, AgentEvent>,

    /// Callback for event delivery
    callback_tx: mpsc::Sender<AgentEvent>,
}

/// Task Observer for real-time event monitoring with gap-fill
pub struct TaskObserver {
    db: Arc<StateDatabase>,

    /// Task states indexed by task_id
    states: RwLock<HashMap<String, TaskState>>,

    /// Maximum gap size before triggering fill
    gap_threshold: u64,
}

impl TaskObserver {
    /// Create a new Task Observer
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self {
            db,
            states: RwLock::new(HashMap::new()),
            gap_threshold: 10,
        }
    }

    /// Create observer with custom gap threshold
    pub fn with_threshold(db: Arc<StateDatabase>, gap_threshold: u64) -> Self {
        Self {
            db,
            states: RwLock::new(HashMap::new()),
            gap_threshold,
        }
    }

    /// Subscribe to events for a task
    ///
    /// Returns a channel receiver for ordered events
    pub async fn subscribe(&self, task_id: &str) -> mpsc::Receiver<AgentEvent> {
        let (tx, rx) = mpsc::channel(100);

        let state = TaskState {
            last_seq: 0,
            pending: HashMap::new(),
            callback_tx: tx,
        };

        let mut states = self.states.write().await;
        states.insert(task_id.to_string(), state);

        info!(task_id = %task_id, "Task observer subscribed");
        rx
    }

    /// Unsubscribe from task events
    pub async fn unsubscribe(&self, task_id: &str) {
        let mut states = self.states.write().await;
        if states.remove(task_id).is_some() {
            info!(task_id = %task_id, "Task observer unsubscribed");
        }
    }

    /// Process an incoming event
    ///
    /// Detects gaps and delivers events in order
    pub async fn on_event(&self, event: AgentEvent) -> Result<(), AlephError> {
        let mut states = self.states.write().await;

        let state = match states.get_mut(&event.task_id) {
            Some(s) => s,
            None => {
                debug!(
                    task_id = %event.task_id,
                    "No subscriber for task, ignoring event"
                );
                return Ok(());
            }
        };

        let expected_seq = state.last_seq + 1;
        let event_seq = event.seq;

        // Case 1: Event is next in sequence
        if event_seq == expected_seq {
            self.deliver_event(state, event).await?;
            self.deliver_pending(state).await?;
            return Ok(());
        }

        // Case 2: Event is ahead (gap detected)
        if event_seq > expected_seq {
            let gap_size = event_seq - expected_seq;

            if gap_size <= self.gap_threshold {
                // Small gap: buffer and wait
                debug!(
                    task_id = %event.task_id,
                    expected = expected_seq,
                    actual = event_seq,
                    "Small gap detected, buffering"
                );
                state.pending.insert(event_seq, event);
            } else {
                // Large gap: trigger fill from database
                warn!(
                    task_id = %event.task_id,
                    expected = expected_seq,
                    actual = event_seq,
                    gap_size = gap_size,
                    "Large gap detected, triggering fill"
                );

                // Store this event in pending
                state.pending.insert(event_seq, event.clone());

                // Fill will be done asynchronously
                let task_id = event.task_id.clone();
                let db = self.db.clone();
                let start_seq = expected_seq;
                let end_seq = event_seq - 1;

                // Spawn background fill task
                tokio::spawn(async move {
                    match db.get_events_in_range(&task_id, start_seq, end_seq).await {
                        Ok(events) => {
                            info!(
                                task_id = %task_id,
                                recovered = events.len(),
                                "Gap-fill recovered events"
                            );
                            // Events will be processed via normal flow
                            // when they're re-emitted or via separate handling
                        }
                        Err(e) => {
                            warn!(
                                task_id = %task_id,
                                error = %e,
                                "Gap-fill failed"
                            );
                        }
                    }
                });
            }
            return Ok(());
        }

        // Case 3: Event is behind (duplicate or late)
        if event_seq <= state.last_seq {
            debug!(
                task_id = %event.task_id,
                seq = event_seq,
                last_seq = state.last_seq,
                "Duplicate or late event, ignoring"
            );
            return Ok(());
        }

        Ok(())
    }

    /// Fill gaps for a task from database
    pub async fn fill_gaps(&self, task_id: &str) -> Result<GapFillResult, AlephError> {
        let states = self.states.read().await;

        let state = match states.get(task_id) {
            Some(s) => s,
            None => {
                return Ok(GapFillResult {
                    gaps_detected: 0,
                    events_recovered: 0,
                    unfilled_gaps: Vec::new(),
                });
            }
        };

        // Find gaps in pending events
        let mut pending_seqs: Vec<u64> = state.pending.keys().copied().collect();
        pending_seqs.sort();

        let mut gaps = Vec::new();
        let mut expected = state.last_seq + 1;

        for seq in &pending_seqs {
            if *seq > expected {
                gaps.push((expected, seq - 1));
            }
            expected = seq + 1;
        }

        if gaps.is_empty() {
            return Ok(GapFillResult {
                gaps_detected: 0,
                events_recovered: 0,
                unfilled_gaps: Vec::new(),
            });
        }

        drop(states); // Release read lock before async operations

        let mut total_recovered = 0;
        let mut unfilled = Vec::new();

        for (start, end) in &gaps {
            match self.db.get_events_in_range(task_id, *start, *end).await {
                Ok(events) => {
                    for event in events {
                        self.on_event(event).await?;
                        total_recovered += 1;
                    }
                }
                Err(e) => {
                    warn!(
                        task_id = %task_id,
                        start = start,
                        end = end,
                        error = %e,
                        "Failed to fill gap"
                    );
                    unfilled.push((*start, *end));
                }
            }
        }

        Ok(GapFillResult {
            gaps_detected: gaps.len(),
            events_recovered: total_recovered,
            unfilled_gaps: unfilled,
        })
    }

    /// Get current sequence number for a task
    pub async fn get_last_seq(&self, task_id: &str) -> Option<u64> {
        let states = self.states.read().await;
        states.get(task_id).map(|s| s.last_seq)
    }

    /// Deliver an event to the subscriber
    async fn deliver_event(&self, state: &mut TaskState, event: AgentEvent) -> Result<(), AlephError> {
        state.last_seq = event.seq;

        if let Err(e) = state.callback_tx.send(event).await {
            warn!(error = %e, "Failed to deliver event, subscriber may have dropped");
        }

        Ok(())
    }

    /// Deliver any pending events that are now in sequence
    async fn deliver_pending(&self, state: &mut TaskState) -> Result<(), AlephError> {
        loop {
            let next_seq = state.last_seq + 1;

            if let Some(event) = state.pending.remove(&next_seq) {
                self.deliver_event(state, event).await?;
            } else {
                break;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gap_fill_result() {
        let result = GapFillResult {
            gaps_detected: 2,
            events_recovered: 5,
            unfilled_gaps: vec![(10, 12)],
        };

        assert_eq!(result.gaps_detected, 2);
        assert_eq!(result.events_recovered, 5);
        assert_eq!(result.unfilled_gaps.len(), 1);
    }
}
