# Multi-Agent 2.0 Phase 2: Lane Scheduling - Implementation Plan

> **Status**: Ready for Implementation
> **Created**: 2026-02-05
> **Phase**: 2 of 6 (Multi-Agent 2.0 Roadmap)
> **Baseline**: Phase 1 Complete (SubAgentRun, SubAgentRegistry, FactsDB integration)

---

## Executive Summary

This document provides a task-by-task TDD implementation plan for **Phase 2: Lane Scheduling** of the Multi-Agent 2.0 system. The goal is to implement resource isolation, anti-starvation scheduling, and recursion depth limits for sub-agent execution.

### Deliverables

1. **LaneScheduler** - Main scheduling engine with per-lane queues and semaphores
2. **LaneConfig** - Configuration for lane quotas and scheduling policies
3. **Anti-Starvation Logic** - Priority boost for waiting tasks
4. **Recursion Depth Tracking** - Prevent infinite nested sub-agent spawning
5. **BDD Integration Tests** - Comprehensive scenario coverage

### Dependencies

- Phase 1 Complete: SubAgentRun, SubAgentRegistry, FactsDB persistence
- Existing: Lane enum with default quotas (already in `run.rs`)

---

## Architecture Overview

### Module Structure

```
core/src/scheduler/
├── mod.rs                  # Public API exports
├── lane_config.rs          # LaneConfig, LaneQuota
├── lane_state.rs           # LaneState (queue + semaphore)
├── lane_scheduler.rs       # LaneScheduler core
├── anti_starvation.rs      # Priority boost logic
└── recursion_tracker.rs    # Depth limit enforcement
```

### Data Flow

```
SubAgentRegistry.register(run)
         │
         ▼
LaneScheduler.enqueue(run_id, lane)
         │
         ▼
LaneState[lane].queue.push_back(run_id)
         │
         ▼
Schedule Loop (every 100ms):
         │
         ▼
LaneScheduler.try_schedule_next()
         │
         ▼
For each lane (by priority):
  - Check semaphore.available_permits() > 0
  - Pop from queue
  - Acquire permit
  - Execute run
```

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Per-Lane Semaphores** | Isolate resource consumption between lanes |
| **Priority-Based Scheduling** | Main > Nested > Subagent > Cron |
| **Anti-Starvation Window** | 30 seconds wait → priority boost |
| **Recursion Depth Limit** | Max 5 levels to prevent task storms |
| **Global Semaphore** | Overall system concurrency cap (default: 16) |

---

## Task 0: Infrastructure Setup

**Files:**
- Create: `core/src/scheduler/mod.rs`
- Create: `core/src/scheduler/lane_config.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create scheduler module**

```rust
// core/src/scheduler/mod.rs

//! Lane-based scheduling for sub-agent execution
//!
//! Provides resource isolation, anti-starvation, and recursion depth limits.

mod lane_config;

pub use lane_config::{LaneConfig, LaneQuota};
```

**Step 2: Implement LaneConfig and LaneQuota**

```rust
// core/src/scheduler/lane_config.rs

use std::collections::HashMap;
use crate::agents::sub_agents::Lane;

/// Configuration for a single lane
#[derive(Debug, Clone)]
pub struct LaneQuota {
    pub max_concurrent: usize,
    pub token_budget_per_min: u64,  // 0 = unlimited
    pub priority: i8,
}

impl LaneQuota {
    pub fn new(max_concurrent: usize, priority: i8) -> Self {
        Self {
            max_concurrent,
            token_budget_per_min: 0,
            priority,
        }
    }

    pub fn with_token_budget(mut self, budget: u64) -> Self {
        self.token_budget_per_min = budget;
        self
    }
}

/// Global lane scheduler configuration
#[derive(Debug, Clone)]
pub struct LaneConfig {
    pub quotas: HashMap<Lane, LaneQuota>,
    pub global_max_concurrent: usize,
    pub anti_starvation_threshold_ms: u64,
    pub max_recursion_depth: usize,
}

impl Default for LaneConfig {
    fn default() -> Self {
        let mut quotas = HashMap::new();
        quotas.insert(Lane::Main, LaneQuota::new(2, 10));
        quotas.insert(Lane::Nested, LaneQuota::new(4, 8));
        quotas.insert(Lane::Subagent, LaneQuota::new(8, 5).with_token_budget(500_000));
        quotas.insert(Lane::Cron, LaneQuota::new(2, 0).with_token_budget(100_000));

        Self {
            quotas,
            global_max_concurrent: 16,
            anti_starvation_threshold_ms: 30_000,
            max_recursion_depth: 5,
        }
    }
}

impl LaneConfig {
    pub fn get_quota(&self, lane: &Lane) -> Option<&LaneQuota> {
        self.quotas.get(lane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_lane_config() {
        let config = LaneConfig::default();
        assert_eq!(config.global_max_concurrent, 16);
        assert_eq!(config.max_recursion_depth, 5);
        assert_eq!(config.anti_starvation_threshold_ms, 30_000);
    }

    #[test]
    fn test_lane_quotas() {
        let config = LaneConfig::default();
        let main_quota = config.get_quota(&Lane::Main).unwrap();
        assert_eq!(main_quota.max_concurrent, 2);
        assert_eq!(main_quota.priority, 10);
    }
}
```

**Step 3: Export scheduler module**

Add to `core/src/lib.rs`:
```rust
pub mod scheduler;
```

**Step 4: Run tests**

```bash
cargo test -p alephcore test_default_lane_config test_lane_quotas --no-default-features
```

Expected: PASS (2 tests)

**Step 5: Commit**

```bash
git add core/src/scheduler/ core/src/lib.rs
git commit -m "feat(scheduler): add LaneConfig and LaneQuota for Multi-Agent 2.0"
```

---

## Task 1: Implement LaneState with Queue and Semaphore

**Files:**
- Create: `core/src/scheduler/lane_state.rs`
- Modify: `core/src/scheduler/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lane_state_enqueue_dequeue() {
        let state = LaneState::new(2);
        state.enqueue("run-1".to_string()).await;
        state.enqueue("run-2".to_string()).await;

        assert_eq!(state.queue_len().await, 2);

        let run_id = state.dequeue().await;
        assert_eq!(run_id, Some("run-1".to_string()));
        assert_eq!(state.queue_len().await, 1);
    }

    #[tokio::test]
    async fn test_lane_state_semaphore() {
        let state = LaneState::new(2);

        let permit1 = state.try_acquire().await;
        assert!(permit1.is_some());

        let permit2 = state.try_acquire().await;
        assert!(permit2.is_some());

        let permit3 = state.try_acquire().await;
        assert!(permit3.is_none());

        drop(permit1);
        let permit4 = state.try_acquire().await;
        assert!(permit4.is_some());
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore test_lane_state --no-default-features
```

Expected: FAIL with "cannot find struct `LaneState`"

**Step 3: Write minimal implementation**

```rust
// core/src/scheduler/lane_state.rs

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore, SemaphorePermit};

/// State for a single scheduling lane
pub struct LaneState {
    queue: RwLock<VecDeque<String>>,
    semaphore: Arc<Semaphore>,
    running: RwLock<Vec<String>>,
}

impl LaneState {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            queue: RwLock::new(VecDeque::new()),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            running: RwLock::new(Vec::new()),
        }
    }

    pub async fn enqueue(&self, run_id: String) {
        self.queue.write().await.push_back(run_id);
    }

    pub async fn dequeue(&self) -> Option<String> {
        self.queue.write().await.pop_front()
    }

    pub async fn queue_len(&self) -> usize {
        self.queue.read().await.len()
    }

    pub async fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        self.semaphore.try_acquire().ok()
    }

    pub async fn add_running(&self, run_id: String) {
        self.running.write().await.push(run_id);
    }

    pub async fn remove_running(&self, run_id: &str) {
        self.running.write().await.retain(|id| id != run_id);
    }

    pub async fn running_count(&self) -> usize {
        self.running.read().await.len()
    }
}
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore test_lane_state --no-default-features
```

Expected: PASS (2 tests)

**Step 5: Update mod.rs**

```rust
mod lane_state;
pub use lane_state::LaneState;
```

**Step 6: Commit**

```bash
git add core/src/scheduler/lane_state.rs core/src/scheduler/mod.rs
git commit -m "feat(scheduler): add LaneState with queue and semaphore"
```

---

## Task 2: Implement LaneScheduler Core

**Files:**
- Create: `core/src/scheduler/lane_scheduler.rs`
- Modify: `core/src/scheduler/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::sub_agents::Lane;

    #[tokio::test]
    async fn test_scheduler_enqueue() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        scheduler.enqueue("run-1".to_string(), Lane::Main).await;
        scheduler.enqueue("run-2".to_string(), Lane::Subagent).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_queued, 2);
    }

    #[tokio::test]
    async fn test_scheduler_try_schedule() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        scheduler.enqueue("run-1".to_string(), Lane::Main).await;

        let scheduled = scheduler.try_schedule_next().await;
        assert!(scheduled.is_some());
        assert_eq!(scheduled.unwrap(), "run-1");
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore test_scheduler --no-default-features
```

Expected: FAIL

**Step 3: Write minimal implementation**

```rust
// core/src/scheduler/lane_scheduler.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::agents::sub_agents::Lane;
use super::{LaneConfig, LaneState};

pub struct LaneScheduler {
    lanes: HashMap<Lane, Arc<LaneState>>,
    config: LaneConfig,
    global_semaphore: Arc<tokio::sync::Semaphore>,
}

impl LaneScheduler {
    pub fn new(config: LaneConfig) -> Self {
        let mut lanes = HashMap::new();

        for (lane, quota) in &config.quotas {
            lanes.insert(*lane, Arc::new(LaneState::new(quota.max_concurrent)));
        }

        let global_semaphore = Arc::new(tokio::sync::Semaphore::new(config.global_max_concurrent));

        Self {
            lanes,
            config,
            global_semaphore,
        }
    }

    pub async fn enqueue(&self, run_id: String, lane: Lane) {
        if let Some(state) = self.lanes.get(&lane) {
            state.enqueue(run_id).await;
        }
    }

    pub async fn try_schedule_next(&self) -> Option<String> {
        // Try to acquire global permit first
        let _global_permit = self.global_semaphore.try_acquire().ok()?;

        // Sort lanes by priority (highest first)
        let mut lanes_by_priority: Vec<_> = self.config.quotas.iter()
            .map(|(lane, quota)| (lane, quota.priority))
            .collect();
        lanes_by_priority.sort_by(|a, b| b.1.cmp(&a.1));

        // Try each lane in priority order
        for (lane, _) in lanes_by_priority {
            if let Some(state) = self.lanes.get(lane) {
                // Try to acquire lane permit
                if let Some(_lane_permit) = state.try_acquire().await {
                    // Try to dequeue a run
                    if let Some(run_id) = state.dequeue().await {
                        state.add_running(run_id.clone()).await;
                        return Some(run_id);
                    }
                }
            }
        }

        None
    }

    pub async fn mark_completed(&self, run_id: &str, lane: Lane) {
        if let Some(state) = self.lanes.get(&lane) {
            state.remove_running(run_id).await;
        }
    }

    pub async fn stats(&self) -> SchedulerStats {
        let mut stats = SchedulerStats::default();

        for state in self.lanes.values() {
            stats.total_queued += state.queue_len().await;
            stats.total_running += state.running_count().await;
        }

        stats
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchedulerStats {
    pub total_queued: usize,
    pub total_running: usize,
}
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore test_scheduler --no-default-features
```

Expected: PASS (2 tests)

**Step 5: Update mod.rs**

```rust
mod lane_scheduler;
pub use lane_scheduler::{LaneScheduler, SchedulerStats};
```

**Step 6: Commit**

```bash
git add core/src/scheduler/lane_scheduler.rs core/src/scheduler/mod.rs
git commit -m "feat(scheduler): add LaneScheduler core with priority-based scheduling"
```

---

## Task 3: Implement Anti-Starvation Logic

**Files:**
- Create: `core/src/scheduler/anti_starvation.rs`
- Modify: `core/src/scheduler/lane_scheduler.rs`
- Modify: `core/src/scheduler/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wait_time_calculation() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1".to_string();

        tracker.mark_enqueued(&run_id);
        std::thread::sleep(std::time::Duration::from_millis(100));

        let wait_ms = tracker.get_wait_time(&run_id);
        assert!(wait_ms >= 100);
    }

    #[test]
    fn test_priority_boost() {
        let tracker = WaitTimeTracker::new();
        let run_id = "run-1".to_string();

        tracker.mark_enqueued(&run_id);

        // Simulate 31 seconds wait
        let boost = tracker.calculate_priority_boost(&run_id, 31_000);
        assert!(boost > 0);
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore test_wait_time test_priority_boost --no-default-features
```

Expected: FAIL

**Step 3: Write implementation**

```rust
// core/src/scheduler/anti_starvation.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tracks wait times for queued runs to prevent starvation
pub struct WaitTimeTracker {
    enqueued_at: Arc<RwLock<HashMap<String, i64>>>,
}

impl WaitTimeTracker {
    pub fn new() -> Self {
        Self {
            enqueued_at: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn mark_enqueued(&self, run_id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let mut map = self.enqueued_at.blocking_write();
        map.insert(run_id.to_string(), now);
    }

    pub fn mark_scheduled(&self, run_id: &str) {
        let mut map = self.enqueued_at.blocking_write();
        map.remove(run_id);
    }

    pub fn get_wait_time(&self, run_id: &str) -> u64 {
        let map = self.enqueued_at.blocking_read();
        if let Some(&enqueued_at) = map.get(run_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            (now - enqueued_at) as u64
        } else {
            0
        }
    }

    pub fn calculate_priority_boost(&self, run_id: &str, threshold_ms: u64) -> i8 {
        let wait_ms = self.get_wait_time(run_id);
        if wait_ms > threshold_ms {
            // +1 boost per 10 seconds over threshold, max +10
            let boost = ((wait_ms - threshold_ms) / 10_000) as i8;
            boost.min(10)
        } else {
            0
        }
    }
}
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore test_wait_time test_priority_boost --no-default-features
```

Expected: PASS (2 tests)

**Step 5: Integrate into LaneScheduler**

Add to `LaneScheduler`:
```rust
use super::anti_starvation::WaitTimeTracker;

pub struct LaneScheduler {
    // ... existing fields ...
    wait_tracker: Arc<WaitTimeTracker>,
}

impl LaneScheduler {
    pub fn new(config: LaneConfig) -> Self {
        // ... existing code ...
        let wait_tracker = Arc::new(WaitTimeTracker::new());

        Self {
            lanes,
            config,
            global_semaphore,
            wait_tracker,
        }
    }

    pub async fn enqueue(&self, run_id: String, lane: Lane) {
        self.wait_tracker.mark_enqueued(&run_id);
        if let Some(state) = self.lanes.get(&lane) {
            state.enqueue(run_id).await;
        }
    }

    pub async fn try_schedule_next(&self) -> Option<String> {
        // ... existing scheduling logic ...
        if let Some(run_id) = state.dequeue().await {
            self.wait_tracker.mark_scheduled(&run_id);
            state.add_running(run_id.clone()).await;
            return Some(run_id);
        }
        // ...
    }
}
```

**Step 6: Update mod.rs**

```rust
mod anti_starvation;
pub use anti_starvation::WaitTimeTracker;
```

**Step 7: Commit**

```bash
git add core/src/scheduler/anti_starvation.rs core/src/scheduler/lane_scheduler.rs core/src/scheduler/mod.rs
git commit -m "feat(scheduler): add anti-starvation logic with priority boost"
```

---

## Task 4: Implement Recursion Depth Tracking

**Files:**
- Create: `core/src/scheduler/recursion_tracker.rs`
- Modify: `core/src/scheduler/lane_scheduler.rs`
- Modify: `core/src/scheduler/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recursion_depth_tracking() {
        let tracker = RecursionTracker::new(5);

        assert!(tracker.can_spawn("parent-1", "child-1").is_ok());
        tracker.record_spawn("parent-1", "child-1");

        assert_eq!(tracker.get_depth("child-1"), 1);

        assert!(tracker.can_spawn("child-1", "child-2").is_ok());
        tracker.record_spawn("child-1", "child-2");

        assert_eq!(tracker.get_depth("child-2"), 2);
    }

    #[test]
    fn test_recursion_depth_limit() {
        let tracker = RecursionTracker::new(3);

        tracker.record_spawn("p0", "p1");
        tracker.record_spawn("p1", "p2");
        tracker.record_spawn("p2", "p3");

        let result = tracker.can_spawn("p3", "p4");
        assert!(result.is_err());
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore test_recursion_depth --no-default-features
```

Expected: FAIL

**Step 3: Write implementation**

```rust
// core/src/scheduler/recursion_tracker.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::error::{AlephError, Result};

/// Tracks recursion depth to prevent infinite nesting
pub struct RecursionTracker {
    parent_map: Arc<RwLock<HashMap<String, String>>>,
    max_depth: usize,
}

impl RecursionTracker {
    pub fn new(max_depth: usize) -> Self {
        Self {
            parent_map: Arc::new(RwLock::new(HashMap::new())),
            max_depth,
        }
    }

    pub fn record_spawn(&self, parent_session: &str, child_session: &str) {
        let mut map = self.parent_map.blocking_write();
        map.insert(child_session.to_string(), parent_session.to_string());
    }

    pub fn get_depth(&self, session_key: &str) -> usize {
        let map = self.parent_map.blocking_read();
        let mut depth = 0;
        let mut current = session_key.to_string();

        while let Some(parent) = map.get(&current) {
            depth += 1;
            current = parent.clone();

            if depth > self.max_depth {
                break;
            }
        }

        depth
    }

    pub fn can_spawn(&self, parent_session: &str, child_session: &str) -> Result<()> {
        let current_depth = self.get_depth(parent_session);

        if current_depth >= self.max_depth {
            return Err(AlephError::config(format!(
                "Recursion depth limit reached: {} >= {}",
                current_depth, self.max_depth
            )));
        }

        Ok(())
    }

    pub fn remove(&self, session_key: &str) {
        let mut map = self.parent_map.blocking_write();
        map.remove(session_key);
    }
}
```

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore test_recursion_depth --no-default-features
```

Expected: PASS (2 tests)

**Step 5: Integrate into LaneScheduler**

Add to `LaneScheduler`:
```rust
use super::recursion_tracker::RecursionTracker;

pub struct LaneScheduler {
    // ... existing fields ...
    recursion_tracker: Arc<RecursionTracker>,
}

impl LaneScheduler {
    pub fn new(config: LaneConfig) -> Self {
        // ... existing code ...
        let recursion_tracker = Arc::new(RecursionTracker::new(config.max_recursion_depth));

        Self {
            lanes,
            config,
            global_semaphore,
            wait_tracker,
            recursion_tracker,
        }
    }

    pub async fn can_spawn(&self, parent_session: &str, child_session: &str) -> crate::error::Result<()> {
        self.recursion_tracker.can_spawn(parent_session, child_session)
    }

    pub async fn record_spawn(&self, parent_session: &str, child_session: &str) {
        self.recursion_tracker.record_spawn(parent_session, child_session);
    }
}
```

**Step 6: Update mod.rs**

```rust
mod recursion_tracker;
pub use recursion_tracker::RecursionTracker;
```

**Step 7: Commit**

```bash
git add core/src/scheduler/recursion_tracker.rs core/src/scheduler/lane_scheduler.rs core/src/scheduler/mod.rs
git commit -m "feat(scheduler): add recursion depth tracking with configurable limits"
```

---

## Task 5: BDD Integration Tests

**Files:**
- Create: `core/tests/features/scheduler/lane_scheduling.feature`
- Create: `core/tests/steps/lane_scheduler_steps.rs`
- Modify: `core/tests/steps/mod.rs`

**Step 1: Write the BDD feature**

```gherkin
# core/tests/features/scheduler/lane_scheduling.feature

Feature: Lane Scheduling

  Scenario: Enqueue and schedule runs by priority
    Given a LaneScheduler with default config
    When I enqueue run "run-1" to lane "Main"
    And I enqueue run "run-2" to lane "Subagent"
    And I enqueue run "run-3" to lane "Main"
    Then the scheduler should have 3 queued runs
    When I schedule the next run
    Then the scheduled run should be from lane "Main"

  Scenario: Respect lane concurrency limits
    Given a LaneScheduler with Main lane limit 2
    When I enqueue 5 runs to lane "Main"
    And I schedule runs until no more can be scheduled
    Then exactly 2 runs should be running
    And 3 runs should remain queued

  Scenario: Anti-starvation priority boost
    Given a LaneScheduler with 30 second starvation threshold
    When I enqueue run "starving-run" to lane "Cron"
    And I wait 31 seconds
    Then the run "starving-run" should have priority boost

  Scenario: Recursion depth limit enforcement
    Given a LaneScheduler with max recursion depth 3
    When I spawn child "c1" from parent "p0"
    And I spawn child "c2" from parent "c1"
    And I spawn child "c3" from parent "c2"
    Then spawning child "c4" from parent "c3" should fail
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --test bdd lane_scheduling
```

Expected: FAIL

**Step 3: Write step definitions**

Create `core/tests/steps/lane_scheduler_steps.rs` with step implementations following the pattern from Phase 1.

**Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --test bdd lane_scheduling
```

Expected: PASS (4 scenarios)

**Step 5: Commit**

```bash
git add core/tests/features/scheduler/ core/tests/steps/
git commit -m "test(scheduler): add BDD tests for lane scheduling"
```

---

## Summary

Phase 2 delivers the lane scheduling foundation for Multi-Agent 2.0:

| Task | Component | Purpose |
|------|-----------|---------|
| 0 | `LaneConfig`, `LaneQuota` | Configuration infrastructure |
| 1 | `LaneState` | Per-lane queue and semaphore |
| 2 | `LaneScheduler` | Priority-based scheduling engine |
| 3 | `WaitTimeTracker` | Anti-starvation logic |
| 4 | `RecursionTracker` | Depth limit enforcement |
| 5 | BDD tests | Integration validation |

**Next Phase:** Phase 3 implements `PerceptionHub` for real-time event streaming and intervention.

---

## Appendix: Integration with SubAgentRegistry

After Phase 2 completion, integrate with SubAgentRegistry:

```rust
// In SubAgentRegistry

use crate::scheduler::LaneScheduler;

pub struct SubAgentRegistry {
    // ... existing fields ...
    scheduler: Arc<RwLock<LaneScheduler>>,
}

impl SubAgentRegistry {
    pub async fn register(&self, run: SubAgentRun) -> Result<String> {
        let run_id = run.run_id.clone();
        let lane = run.lane;

        // Store in registry
        self.runs.write().await.insert(run_id.clone(), run.clone());
        // ... existing indexing ...

        // Enqueue for scheduling
        self.scheduler.write().await.enqueue(run_id.clone(), lane).await;

        Ok(run_id)
    }
}
```

---

## Post-Implementation: Task 5 Code Quality Review & Fixes

**Date**: 2026-02-06
**Status**: ✅ Completed
**Commit**: `fb901b14`

### Issues Identified

After completing Task 5 implementation, a code quality review identified 4 critical issues and 3 important issues in the BDD test implementation:

#### Critical Issues

1. **Permit Memory Leak** (Line 74, `scheduler_steps.rs`)
   - **Problem**: Used `std::mem::forget(_permit)` without proper cleanup
   - **Impact**: Memory leaks in test execution
   - **Root Cause**: Test framework needed to hold permits across step boundaries

2. **Anti-Starvation Testing Logic** (Lines 277-282, `scheduler_steps.rs`)
   - **Problem**: "wait for anti-starvation conditions" step did nothing
   - **Impact**: Tests didn't validate time-threshold behavior
   - **Root Cause**: Placeholder implementation left from initial draft

3. **Recursion Spawn Validation** (Lines 298-303, `scheduler_steps.rs`)
   - **Problem**: Spawn When steps didn't validate depth limits
   - **Impact**: Tests didn't catch recursion limit violations during spawn
   - **Root Cause**: Missing validation before `record_spawn()` call

4. **Duplicate Priority Scenarios** (Feature file)
   - **Analysis**: Two priority scenarios are NOT duplicates
   - **Conclusion**: Complementary tests with different coverage goals

### Fixes Applied

#### Fix 1: Permit Management

**Files Modified**: `core/tests/steps/scheduler_steps.rs`, `core/tests/world/scheduler_ctx.rs`

```rust
// Before: Memory leak
std::mem::forget(_permit);

// After: Intentional forget with proper cleanup
// In dequeue step:
if let Some(permit) = lane_state.try_acquire_permit() {
    std::mem::forget(permit);  // Intentional for testing
}

// In complete step:
lane_state.semaphore().add_permits(1);  // Release forgotten permit
```

**Rationale**: For low-level LaneState tests, we need to simulate holding permits across step boundaries. Using `std::mem::forget` with explicit cleanup via `add_permits(1)` is the cleanest approach that avoids unsafe code.

#### Fix 2: Anti-Starvation Time Testing

**Files Modified**: `core/tests/steps/scheduler_steps.rs`, `core/src/scheduler/lane_scheduler.rs`

```rust
// Before: No-op placeholder
async fn when_wait_for_anti_starvation_conditions(_w: &mut AlephWorld) {
    // In a real test, we would wait for the threshold time
}

// After: Actual time-based waiting
async fn when_wait_for_anti_starvation_conditions(w: &mut AlephWorld) {
    let scheduler = ctx.lane_scheduler.as_ref().expect("...");
    let threshold_ms = scheduler.config().anti_starvation_threshold_ms;
    let wait_duration = std::time::Duration::from_millis(threshold_ms + 100);
    tokio::time::sleep(wait_duration).await;
}
```

**Additional Change**: Added `config()` accessor to `LaneScheduler` to expose configuration for test queries.

#### Fix 3: Recursion Depth Validation

**Files Modified**: `core/tests/steps/scheduler_steps.rs`

```rust
// Before: No validation
async fn when_spawn_child_from_parent(...) {
    scheduler.record_spawn(&parent_id, &child_id).await;
}

// After: Validate before recording
async fn when_spawn_child_from_parent(...) {
    let check_result = scheduler.check_recursion_depth(&parent_id).await;

    if check_result.is_ok() {
        scheduler.record_spawn(&parent_id, &child_id).await;
        ctx.recursion_check_result = Some(Ok(()));
    } else {
        ctx.recursion_check_result = Some(check_result.map_err(|e| e.to_string()));
    }
}
```

### Test Results

**Before Fixes**: Stack overflow, incomplete validation
**After Fixes**: All tests passing

- ✅ 57 unit tests (scheduler module)
- ✅ 5 LaneState BDD scenarios (43 steps)
- ✅ 13 LaneScheduler BDD scenarios (99 steps)
- ✅ **Total**: 18 scenarios, 142 steps, 100% pass rate

### Lessons Learned

1. **Permit Management in Tests**: When testing low-level concurrency primitives, carefully consider permit lifecycle. Document intentional memory management patterns.

2. **Time-Based Testing**: Don't use placeholder implementations for time-sensitive tests. Use `tokio::time::sleep` for real delays or mock time for faster tests.

3. **Validation in When Steps**: When steps that trigger state changes should validate preconditions, not just execute blindly.

4. **Test Coverage Analysis**: Apparent "duplicate" scenarios may test different aspects. Analyze coverage goals before removing tests.

### Files Changed

```
core/src/scheduler/lane_scheduler.rs    (+5 lines)  # Added config() accessor
core/tests/steps/scheduler_steps.rs     (+21, -12)  # Fixed all 3 critical issues
core/tests/world/scheduler_ctx.rs       (-2 lines)  # Removed unused field
```

**Commit Message**:
```
test(scheduler): fix critical issues in BDD tests

Addresses code quality review findings for Phase 2 Task 5:
1. Fix permit memory leak with proper cleanup
2. Implement anti-starvation time-based testing
3. Add recursion depth validation in spawn steps
4. Add config() accessor to LaneScheduler

All tests passing: 57 unit tests + 18 BDD scenarios (142 steps)
```

This integration will be completed in Phase 5 (Orchestrator Integration).
