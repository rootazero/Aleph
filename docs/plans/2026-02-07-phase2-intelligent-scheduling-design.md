# Phase 2: Intelligent Scheduling Design

**Date**: 2026-02-07
**Status**: Design Complete
**Phase**: Phase 2 of Liquid Hub Implementation

## Overview

This document describes the design for Phase 2: Intelligent Scheduling, which implements priority-based task scheduling with conflict resolution for the Liquid Hub architecture.

## Goals

1. Implement three-tier priority queue system (User, Financial, Background)
2. Add routing matrix with conflict detection and resolution
3. Support task preemption with state freezing
4. Integrate with existing Dispatcher and BrowserPool
5. Prevent task starvation through dynamic priority boosting

## Architecture Principles

### Wrapper Pattern

The design follows the **Wrapper Pattern** to preserve orthogonality between:
- **DAG Scheduler**: Handles dependency resolution (what tasks *can* run)
- **Priority Scheduler**: Handles priority ordering (what tasks *should* run first)

This approach:
- ✅ Preserves existing tested DagScheduler logic (5800+ tests)
- ✅ Follows Open-Closed Principle (extend without modifying)
- ✅ Enables future extensions (ResourceAwareScheduler, FairnessScheduler)

## Part 1: Core Architecture

### 1.1 Component Hierarchy

```
┌─────────────────────────────────────────────────────┐
│              PriorityScheduler (New)                │
│  ┌───────────────────────────────────────────────┐  │
│  │  Three-Tier Priority Queue                    │  │
│  │  - Tier 0: User (Preemptive)                  │  │
│  │  - Tier 1: Financial (High Priority)          │  │
│  │  - Tier 2: Background (Normal)                │  │
│  └───────────────┬───────────────────────────────┘  │
│                  │                                   │
│                  ▼                                   │
│  ┌───────────────────────────────────────────────┐  │
│  │  DagScheduler (Existing)                      │  │
│  │  - Dependency resolution                      │  │
│  │  - Topological ordering                       │  │
│  └───────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

### 1.2 Key Data Structures

```rust
/// Task priority tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriorityTier {
    Background = 2,  // Normal background tasks
    Financial = 1,   // High-priority financial operations
    User = 0,        // Immediate user actions (preemptive)
}

/// Task metadata for priority scheduling
#[derive(Debug, Clone)]
pub struct TaskMetadata {
    /// Priority tier
    pub tier: PriorityTier,

    /// Task submission timestamp (for starvation prevention)
    pub submitted_at: SystemTime,

    /// Domain being accessed (for conflict detection)
    pub domain: Option<String>,

    /// Risk level (for routing decisions)
    pub risk_level: RiskLevel,

    /// Whether task can be preempted
    pub preemptible: bool,

    /// Effective priority (dynamic, calculated)
    pub effective_priority: f64,
}

/// Risk level for browser tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,      // Read-only operations
    Medium,   // Form filling, navigation
    High,     // Financial transactions, data modification
}
```

### 1.3 PriorityScheduler Interface

```rust
pub struct PriorityScheduler {
    /// Underlying DAG scheduler
    dag_scheduler: DagScheduler,

    /// Priority queues (one per tier)
    queues: [VecDeque<String>; 3],  // task_id queues

    /// Task metadata registry
    metadata: HashMap<String, TaskMetadata>,

    /// Currently executing tasks
    executing: HashSet<String>,

    /// Domain locks (for conflict prevention)
    domain_locks: HashMap<String, String>,  // domain -> task_id

    /// Starvation prevention threshold (seconds)
    starvation_threshold: u64,
}

impl PriorityScheduler {
    /// Submit task with metadata
    pub fn submit(&mut self, task_id: String, metadata: TaskMetadata);

    /// Get next task to execute (priority + dependency aware)
    pub fn next_ready(&mut self, graph: &TaskGraph) -> Option<String>;

    /// Calculate effective priority (with time-based boosting)
    fn calculate_effective_priority(&self, metadata: &TaskMetadata) -> f64;

    /// Check for domain conflicts
    fn has_domain_conflict(&self, task_id: &str) -> bool;

    /// Preempt lower priority task
    pub fn preempt(&mut self, task_id: &str) -> Option<String>;
}
```

## Part 2: Scheduling Algorithm & Conflict Resolution

### 2.1 Scheduling Decision Flow

```rust
pub fn next_ready(&mut self, graph: &TaskGraph) -> Option<String> {
    // Step 1: Update effective priorities (time-based boosting)
    self.update_effective_priorities();

    // Step 2: Iterate through tiers (Tier 0 → Tier 1 → Tier 2)
    for tier in [PriorityTier::User, PriorityTier::Financial, PriorityTier::Background] {
        let queue_idx = tier as usize;

        // Step 3: Check each task in current tier
        while let Some(task_id) = self.queues[queue_idx].front() {
            // Step 4: Verify DAG dependencies are satisfied
            if !self.dag_scheduler.is_ready(task_id, graph) {
                self.queues[queue_idx].pop_front();
                continue;
            }

            // Step 5: Check domain conflicts
            if self.has_domain_conflict(task_id) {
                // Try preemption if this is higher priority
                if tier == PriorityTier::User {
                    if let Some(preempted) = self.try_preempt_for(task_id) {
                        return Some(task_id.clone());
                    }
                }
                // Skip if can't preempt
                self.queues[queue_idx].pop_front();
                continue;
            }

            // Step 6: Task is ready to execute
            return self.queues[queue_idx].pop_front();
        }
    }

    None
}
```

### 2.2 Dynamic Priority Boosting (Starvation Prevention)

```rust
fn update_effective_priorities(&mut self) {
    let now = SystemTime::now();

    for (task_id, metadata) in &mut self.metadata {
        let wait_time = now.duration_since(metadata.submitted_at)
            .unwrap_or_default()
            .as_secs();

        // Base priority from tier
        let base_priority = match metadata.tier {
            PriorityTier::User => 1000.0,
            PriorityTier::Financial => 500.0,
            PriorityTier::Background => 100.0,
        };

        // Time-based boost: +1 priority per second waited
        let time_boost = wait_time as f64;

        // Risk penalty: High-risk tasks get slight penalty
        let risk_penalty = match metadata.risk_level {
            RiskLevel::High => -50.0,
            RiskLevel::Medium => -10.0,
            RiskLevel::Low => 0.0,
        };

        metadata.effective_priority = base_priority + time_boost + risk_penalty;

        // Promote to higher tier if waited too long
        if wait_time > self.starvation_threshold {
            metadata.tier = match metadata.tier {
                PriorityTier::Background => PriorityTier::Financial,
                PriorityTier::Financial => PriorityTier::User,
                PriorityTier::User => PriorityTier::User,
            };
        }
    }
}
```

### 2.3 Conflict Detection & Preemption

```rust
fn has_domain_conflict(&self, task_id: &str) -> bool {
    if let Some(metadata) = self.metadata.get(task_id) {
        if let Some(domain) = &metadata.domain {
            // Check if domain is locked by another task
            if let Some(locking_task) = self.domain_locks.get(domain) {
                return locking_task != task_id;
            }
        }
    }
    false
}

fn try_preempt_for(&mut self, high_priority_task: &str) -> Option<String> {
    let metadata = self.metadata.get(high_priority_task)?;
    let domain = metadata.domain.as_ref()?;

    // Find the task currently holding the domain lock
    let locking_task = self.domain_locks.get(domain)?.clone();
    let locking_metadata = self.metadata.get(&locking_task)?;

    // Can only preempt if:
    // 1. Locking task is preemptible
    // 2. High priority task has higher tier
    if locking_metadata.preemptible && metadata.tier < locking_metadata.tier {
        // Freeze the lower priority task
        self.freeze_task(&locking_task);

        // Release domain lock
        self.domain_locks.remove(domain);

        // Acquire lock for high priority task
        self.domain_locks.insert(domain.clone(), high_priority_task.to_string());

        return Some(locking_task);
    }

    None
}
```

### 2.4 State Freeze Mechanism

```rust
fn freeze_task(&mut self, task_id: &str) {
    // Mark task as frozen (will be resumed later)
    if let Some(metadata) = self.metadata.get_mut(task_id) {
        metadata.preemptible = false;  // Prevent re-preemption
    }

    // Move task back to its priority queue (front position)
    if let Some(metadata) = self.metadata.get(task_id) {
        let queue_idx = metadata.tier as usize;
        self.queues[queue_idx].push_front(task_id.to_string());
    }

    // Remove from executing set
    self.executing.remove(task_id);

    // TODO: Send CDP Debugger.pause command to browser
    // This will be implemented when integrating with BrowserPool
}

fn resume_task(&mut self, task_id: &str) {
    if let Some(metadata) = self.metadata.get_mut(task_id) {
        metadata.preemptible = true;  // Allow preemption again
    }

    // TODO: Send CDP Debugger.resume command to browser
}
```

## Part 3: Integration & State Management

### 3.1 Dispatcher Integration

```rust
pub struct DispatcherEngine {
    // Existing fields...
    tool_registry: ToolRegistry,
    model_router: ModelRouter,

    // New: Priority scheduler for browser tasks
    priority_scheduler: Option<PriorityScheduler>,
}

impl DispatcherEngine {
    /// Route task to appropriate scheduler
    fn schedule_task(&mut self, task: Task) -> Result<()> {
        // Check if task involves browser operations
        if self.is_browser_task(&task) {
            // Use priority scheduler
            if let Some(scheduler) = &mut self.priority_scheduler {
                let metadata = self.extract_task_metadata(&task);
                scheduler.submit(task.id.clone(), metadata);
            }
        } else {
            // Use existing DAG scheduler for non-browser tasks
            self.dag_scheduler.submit(task);
        }
        Ok(())
    }

    fn is_browser_task(&self, task: &Task) -> bool {
        // Check if task uses browser-related tools
        task.tool_name.starts_with("browser.") ||
        task.tool_name == "navigate" ||
        task.tool_name == "click" ||
        task.tool_name == "screenshot"
    }

    fn extract_task_metadata(&self, task: &Task) -> TaskMetadata {
        // Extract domain from task parameters
        let domain = self.extract_domain_from_task(task);

        // Determine priority tier based on task type
        let tier = if task.tags.contains("user_action") {
            PriorityTier::User
        } else if task.tags.contains("financial") {
            PriorityTier::Financial
        } else {
            PriorityTier::Background
        };

        // Determine risk level
        let risk_level = if task.tags.contains("high_risk") {
            RiskLevel::High
        } else if task.tags.contains("medium_risk") {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };

        TaskMetadata {
            tier,
            submitted_at: SystemTime::now(),
            domain,
            risk_level,
            preemptible: tier != PriorityTier::User,  // User tasks not preemptible
            effective_priority: 0.0,  // Will be calculated
        }
    }
}
```

### 3.2 BrowserPool Integration

```rust
impl BrowserPool {
    /// Execute task with priority awareness
    pub async fn execute_task_with_priority(
        &self,
        task_id: &str,
        metadata: &TaskMetadata,
    ) -> BrowserResult<()> {
        // Get appropriate context based on risk level
        let context = match metadata.risk_level {
            RiskLevel::High => {
                // Use dedicated isolated context for high-risk tasks
                self.create_ephemeral_context(task_id.to_string()).await?
            }
            RiskLevel::Medium | RiskLevel::Low => {
                // Use primary context for normal tasks
                self.get_primary_context().await?
            }
        };

        // Lock domain if specified
        if let Some(domain) = &metadata.domain {
            self.context_registry()
                .lock_domain(domain.clone(), task_id.to_string())
                .await
                .map_err(|e| BrowserError::Internal(e))?;
        }

        // Execute task...
        // (actual execution logic)

        Ok(())
    }

    /// Freeze browser context (for preemption)
    #[cfg(feature = "browser")]
    pub async fn freeze_context(&self, task_id: &str) -> BrowserResult<()> {
        if let Some(context) = self.get_ephemeral_context(&task_id.to_string()).await {
            // Send CDP Debugger.pause command
            // TODO: Implement CDP pause via chromiumoxide
            tracing::info!("Freezing context for task: {}", task_id);
        }
        Ok(())
    }

    /// Resume frozen browser context
    #[cfg(feature = "browser")]
    pub async fn resume_context(&self, task_id: &str) -> BrowserResult<()> {
        if let Some(context) = self.get_ephemeral_context(&task_id.to_string()).await {
            // Send CDP Debugger.resume command
            // TODO: Implement CDP resume via chromiumoxide
            tracing::info!("Resuming context for task: {}", task_id);
        }
        Ok(())
    }
}
```

### 3.3 Configuration

```rust
/// Configuration for priority scheduler
#[derive(Debug, Clone)]
pub struct PrioritySchedulerConfig {
    /// Starvation prevention threshold (seconds)
    pub starvation_threshold: u64,

    /// Maximum concurrent browser tasks
    pub max_concurrent_browser_tasks: usize,

    /// Enable dynamic priority boosting
    pub enable_priority_boosting: bool,

    /// Enable preemption
    pub enable_preemption: bool,
}

impl Default for PrioritySchedulerConfig {
    fn default() -> Self {
        Self {
            starvation_threshold: 300,  // 5 minutes
            max_concurrent_browser_tasks: 3,
            enable_priority_boosting: true,
            enable_preemption: true,
        }
    }
}
```

## Part 4: Testing Strategy & Success Criteria

### 4.1 Unit Tests

Key test scenarios:
- Priority tier ordering
- Priority queue ordering (User > Financial > Background)
- Starvation prevention (time-based promotion)
- Domain conflict detection
- Preemption logic
- State freeze and resume

### 4.2 Integration Tests

Key integration scenarios:
- Concurrent tasks on same domain
- User preempts background task
- Financial task serialization
- Recovery after preemption
- End-to-end scheduling flow

### 4.3 Success Criteria

Based on Phase 2 requirements:

1. **User actions immediately preempt background tasks**
   - ✅ Tier 0 (User) tasks can preempt Tier 1/2 tasks
   - ✅ Preemption latency < 100ms
   - ✅ Preempted tasks correctly frozen and resumable

2. **Financial operations automatically queue (no concurrency)**
   - ✅ Same-domain financial tasks strictly serialized
   - ✅ Domain locks prevent concurrent execution
   - ✅ High-risk tasks use isolated contexts

3. **System learns from conflicts and adjusts routing rules**
   - ✅ Dynamic priority boosting prevents starvation
   - ✅ Time-based weights automatically elevate waiting tasks
   - ✅ Automatic tier promotion after threshold exceeded

### 4.4 Performance Metrics

```rust
pub struct SchedulerMetrics {
    /// Total tasks scheduled
    pub total_scheduled: u64,

    /// Tasks preempted
    pub preemptions: u64,

    /// Average wait time per tier (seconds)
    pub avg_wait_time: [f64; 3],

    /// Domain conflicts detected
    pub conflicts_detected: u64,

    /// Starvation promotions
    pub starvation_promotions: u64,
}
```

## Future Enhancements

### Context-Aware Scheduling
- Dynamic priority based on user interaction state
- Browser tab visibility awareness
- UI-blocking task detection

### Resource-Constrained Concurrency
- Backpressure awareness for browser tasks
- Memory and CPU load monitoring
- Adaptive concurrency limits

### Predictive Pre-warming
- Historical execution pattern analysis
- DAG path prediction
- Proactive resource initialization

### Self-Adaptive Feedback Loop
- Execution time and resource tracking
- Automatic priority adjustment based on history
- Strategy optimization over time

## Implementation Plan

1. **Phase 2.1**: Core PriorityScheduler implementation
   - Data structures and interfaces
   - Basic priority queue logic
   - Unit tests

2. **Phase 2.2**: Conflict resolution and preemption
   - Domain locking mechanism
   - Preemption logic
   - State freeze/resume stubs

3. **Phase 2.3**: Integration
   - Dispatcher integration
   - BrowserPool integration
   - End-to-end tests

4. **Phase 2.4**: CDP integration
   - Implement actual freeze/resume with CDP
   - Browser context state management
   - Performance optimization

## References

- [Liquid Hub Architecture Design](./2026-02-07-liquid-hub-cross-platform-design.md)
- [BrowserPool Implementation](../ARCHITECTURE.md#browser-pool)
- [Dispatcher System](../ARCHITECTURE.md#dispatcher)
