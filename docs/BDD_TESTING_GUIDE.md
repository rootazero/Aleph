# BDD Testing Guide

> **Audience**: Developers writing BDD tests for Aleph using cucumber-rs
> **Last Updated**: 2026-02-06

This guide provides best practices, patterns, and anti-patterns for writing high-quality BDD tests in the Aleph project.

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Writing Features](#writing-features)
4. [Writing Step Definitions](#writing-step-definitions)
5. [Context Management](#context-management)
6. [Common Patterns](#common-patterns)
7. [Anti-Patterns](#anti-patterns)
8. [Testing Async Code](#testing-async-code)
9. [Testing Concurrency](#testing-concurrency)
10. [Code Quality Checklist](#code-quality-checklist)

---

## Overview

Aleph uses **cucumber-rs** for BDD (Behavior-Driven Development) testing. BDD tests are written in Gherkin syntax and organized into:

- **Features** (`.feature` files): Human-readable test scenarios
- **Steps** (Rust code): Implementation of Given/When/Then steps
- **World** (Rust structs): Shared state between steps

### Benefits

- ✅ Executable specifications
- ✅ Clear separation of "what" (feature) and "how" (steps)
- ✅ Reusable step definitions
- ✅ Living documentation

### Tech Stack

- **cucumber**: 0.21 (BDD framework)
- **tokio**: Async runtime
- **tempfile**: Test isolation

---

## Architecture

### Directory Structure

```
core/tests/
├── cucumber.rs              # Test runner
├── features/                # Gherkin feature files
│   ├── scheduler/
│   │   ├── lane_state.feature
│   │   └── lane_scheduler.feature
│   ├── memory/
│   └── ...
├── steps/                   # Step definitions
│   ├── mod.rs
│   ├── common.rs
│   ├── scheduler_steps.rs
│   └── ...
└── world/                   # Context structs
    ├── mod.rs
    ├── scheduler_ctx.rs
    └── ...
```

### World Design

The `AlephWorld` struct uses a **composite pattern** with optional sub-contexts:

```rust
#[derive(Debug, Default, World)]
pub struct AlephWorld {
    pub scheduler: Option<SchedulerContext>,
    pub memory: Option<MemoryContext>,
    pub gateway: Option<GatewayContext>,
    // ... more contexts
}
```

**Benefits**:
- Lazy initialization (only create what you need)
- Clear separation of concerns
- Easy to extend

---

## Writing Features

### Feature File Structure

```gherkin
Feature: Brief description of the feature

  Background: (optional)
    Given common setup for all scenarios

  @tag1 @tag2
  Scenario: Descriptive scenario name
    Given initial state
    When action is performed
    Then expected outcome
    And additional assertion

  Scenario Outline: Parameterized scenario
    Given a value of <input>
    When I process it
    Then I should get <output>

    Examples:
      | input | output |
      | 1     | 2      |
      | 2     | 4      |
```

### Best Practices

#### ✅ DO: Use descriptive scenario names

```gherkin
# Good
Scenario: Enqueue and schedule runs by priority

# Bad
Scenario: Test 1
```

#### ✅ DO: Focus on behavior, not implementation

```gherkin
# Good
When I enqueue run "run-1" to lane "Main"

# Bad
When I call scheduler.enqueue("run-1", Lane::Main)
```

#### ✅ DO: Use tags for organization

```gherkin
@lane_scheduler @priority
Scenario: Priority-based scheduling
```

#### ✅ DO: Keep scenarios independent

Each scenario should be runnable in isolation without depending on other scenarios.

#### ❌ DON'T: Test implementation details

```gherkin
# Bad - tests internal structure
Then the queue should be a VecDeque with 3 elements

# Good - tests observable behavior
Then the queue should have 3 runs
```

---

## Writing Step Definitions

### Basic Structure

```rust
use cucumber::{given, when, then};
use crate::world::AlephWorld;

#[given(expr = "a scheduler with {int} lanes")]
async fn given_scheduler_with_lanes(w: &mut AlephWorld, count: usize) {
    let ctx = w.scheduler.get_or_insert_with(SchedulerContext::default);
    ctx.create_scheduler(count);
}

#[when(expr = "I enqueue run {string}")]
async fn when_enqueue_run(w: &mut AlephWorld, run_id: String) {
    let ctx = w.scheduler.as_mut().expect("Scheduler not initialized");
    ctx.scheduler.enqueue(run_id).await;
}

#[then(expr = "the queue should have {int} runs")]
async fn then_queue_should_have_runs(w: &mut AlephWorld, expected: usize) {
    let ctx = w.scheduler.as_ref().expect("Scheduler not initialized");
    let actual = ctx.scheduler.queue_len().await;
    assert_eq!(
        actual, expected,
        "Expected {} runs in queue, but found {}",
        expected, actual
    );
}
```

### Best Practices

#### ✅ DO: Use clear assertion messages

```rust
// Good
assert_eq!(
    actual, expected,
    "Expected {} runs in queue, but found {}",
    expected, actual
);

// Bad
assert_eq!(actual, expected);
```

#### ✅ DO: Handle errors gracefully

```rust
// Good
let ctx = w.scheduler.as_mut()
    .expect("Scheduler context not initialized - did you forget Given step?");

// Bad
let ctx = w.scheduler.as_mut().unwrap();
```

#### ✅ DO: Store results for later assertions

```rust
#[when("I try to dequeue a run")]
async fn when_try_dequeue(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_mut().expect("...");
    ctx.last_result = ctx.scheduler.try_dequeue().await;
}

#[then("the dequeue should succeed")]
async fn then_dequeue_should_succeed(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_ref().expect("...");
    assert!(
        ctx.last_result.is_some(),
        "Expected dequeue to succeed, but it failed"
    );
}
```

#### ❌ DON'T: Perform assertions in When steps

```rust
// Bad - When steps should only perform actions
#[when("I enqueue a run")]
async fn when_enqueue(w: &mut AlephWorld) {
    ctx.scheduler.enqueue("run-1").await;
    assert_eq!(ctx.scheduler.queue_len().await, 1); // ❌ Don't assert here
}

// Good - separate action and assertion
#[when("I enqueue a run")]
async fn when_enqueue(w: &mut AlephWorld) {
    ctx.scheduler.enqueue("run-1").await;
}

#[then("the queue should have 1 run")]
async fn then_queue_has_one_run(w: &mut AlephWorld) {
    assert_eq!(ctx.scheduler.queue_len().await, 1);
}
```

---

## Context Management

### Context Struct Design

```rust
#[derive(Default)]
pub struct SchedulerContext {
    // Core components
    pub scheduler: Option<Arc<LaneScheduler>>,

    // Test state
    pub last_scheduled: Option<(String, Lane)>,
    pub last_result: Option<Result<(), String>>,

    // Helpers
    pub run_counter: usize,
}

impl SchedulerContext {
    pub fn create_scheduler(&mut self) {
        self.scheduler = Some(Arc::new(LaneScheduler::new(config)));
    }

    pub fn generate_run_id(&mut self) -> String {
        self.run_counter += 1;
        format!("run-{}", self.run_counter)
    }
}
```

### Best Practices

#### ✅ DO: Use Option for lazy initialization

```rust
pub struct Context {
    pub scheduler: Option<Arc<Scheduler>>,  // ✅ Lazy
}
```

#### ✅ DO: Provide helper methods

```rust
impl Context {
    pub fn generate_run_id(&mut self) -> String {
        self.run_counter += 1;
        format!("run-{}", self.run_counter)
    }
}
```

#### ✅ DO: Clean up unused fields

Run `cargo fix` regularly to catch unused field warnings.

#### ❌ DON'T: Store raw values when Arc is needed

```rust
// Bad - can't share across async boundaries
pub scheduler: Scheduler,

// Good - can be cloned and shared
pub scheduler: Arc<Scheduler>,
```

---

## Common Patterns

### Pattern 1: Parameterized Steps

```rust
#[given(expr = "a scheduler with {int} lanes")]
async fn given_scheduler(w: &mut AlephWorld, count: usize) {
    // ...
}

#[when(expr = "I enqueue run {string} to lane {string}")]
async fn when_enqueue(w: &mut AlephWorld, run_id: String, lane: String) {
    // ...
}
```

### Pattern 2: Multiple Assertions

```rust
#[then(expr = "the scheduler should have {int} queued runs")]
#[then(expr = "the scheduler should have {int} queued run")]
async fn then_queued_runs(w: &mut AlephWorld, count: usize) {
    // Handles both singular and plural
}
```

### Pattern 3: Result Storage

```rust
#[when("I try to spawn a child")]
async fn when_try_spawn(w: &mut AlephWorld) {
    let result = ctx.scheduler.try_spawn().await;
    ctx.last_result = Some(result.map_err(|e| e.to_string()));
}

#[then("the spawn should fail")]
async fn then_spawn_should_fail(w: &mut AlephWorld) {
    assert!(ctx.last_result.as_ref().unwrap().is_err());
}
```

### Pattern 4: Enum Parsing

```rust
impl Context {
    pub fn parse_lane(s: &str) -> Lane {
        match s {
            "Main" => Lane::Main,
            "Subagent" => Lane::Subagent,
            _ => panic!("Unknown lane: {}", s),
        }
    }
}
```

---

## Anti-Patterns

### ❌ Anti-Pattern 1: Placeholder Implementations

```rust
// BAD - doesn't actually test anything
#[when("I wait for timeout")]
async fn when_wait(_w: &mut AlephWorld) {
    // TODO: Implement waiting
}
```

**Fix**: Implement actual behavior or remove the step.

### ❌ Anti-Pattern 2: Memory Leaks

```rust
// BAD - permit is forgotten without cleanup
if let Some(permit) = semaphore.try_acquire() {
    std::mem::forget(permit);  // ❌ Memory leak
}
```

**Fix**: Provide explicit cleanup in a corresponding step.

### ❌ Anti-Pattern 3: Unsafe Lifetime Extension

```rust
// BAD - unsafe transmute can cause stack overflow
let static_permit = unsafe { std::mem::transmute(permit) };
ctx.permits.push(static_permit);  // ❌ Dangerous
```

**Fix**: Use proper lifetime management or redesign the test.

### ❌ Anti-Pattern 4: Hardcoded Values

```rust
// BAD - hardcoded threshold
tokio::time::sleep(Duration::from_secs(30)).await;
```

**Fix**: Read from configuration.

```rust
// GOOD
let threshold = ctx.scheduler.config().threshold_ms;
tokio::time::sleep(Duration::from_millis(threshold + 100)).await;
```

---

## Testing Async Code

### Use tokio::time for delays

```rust
// ✅ Good - async-aware
tokio::time::sleep(Duration::from_millis(100)).await;

// ❌ Bad - blocks the executor
std::thread::sleep(Duration::from_millis(100));
```

### Use tokio::spawn for background tasks

```rust
#[when("I start a background task")]
async fn when_start_background(w: &mut AlephWorld) {
    let handle = tokio::spawn(async {
        // Background work
    });
    ctx.background_task = Some(handle);
}

#[then("the background task should complete")]
async fn then_background_completes(w: &mut AlephWorld) {
    let handle = ctx.background_task.take().unwrap();
    handle.await.unwrap();
}
```

### Mock time for faster tests

```rust
#[tokio::test]
async fn test_with_mocked_time() {
    tokio::time::pause();  // Pause time

    let start = tokio::time::Instant::now();
    tokio::time::advance(Duration::from_secs(60)).await;  // Fast-forward

    assert_eq!(start.elapsed(), Duration::from_secs(60));
}
```

---

## Testing Concurrency

### Pattern: Semaphore Testing

When testing low-level concurrency primitives, you may need to hold resources across step boundaries.

```rust
#[when("I acquire a permit")]
async fn when_acquire_permit(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_mut().expect("...");
    let lane_state = ctx.lane_state.as_ref().expect("...");

    if let Some(permit) = lane_state.try_acquire_permit() {
        // For testing: intentionally forget permit to simulate holding it
        // IMPORTANT: Must provide cleanup in corresponding step
        std::mem::forget(permit);
        ctx.last_result = Some(Ok(()));
    } else {
        ctx.last_result = Some(Err("No permits available".to_string()));
    }
}

#[when("I release the permit")]
async fn when_release_permit(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_mut().expect("...");
    let lane_state = ctx.lane_state.as_ref().expect("...");

    // Explicit cleanup: add permit back to semaphore
    lane_state.semaphore().add_permits(1);
}
```

**Guidelines**:
- Document intentional `std::mem::forget` usage
- Always provide explicit cleanup
- Consider if higher-level API testing can avoid this pattern
- Never use unsafe code for lifetime extension

### Pattern: Time-Based Anti-Starvation

```rust
#[when("I wait for anti-starvation conditions")]
async fn when_wait_for_anti_starvation(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_ref().expect("...");
    let scheduler = ctx.lane_scheduler.as_ref().expect("...");

    // Read threshold from config
    let threshold_ms = scheduler.config().anti_starvation_threshold_ms;

    // Wait for slightly more than threshold
    let wait_duration = Duration::from_millis(threshold_ms + 100);
    tokio::time::sleep(wait_duration).await;
}
```

### Pattern: Validation Before State Change

```rust
#[when(expr = "I spawn child {string} from parent {string}")]
async fn when_spawn_child(w: &mut AlephWorld, child: String, parent: String) {
    let ctx = w.scheduler.as_mut().expect("...");
    let scheduler = ctx.lane_scheduler.as_ref().expect("...");

    // Validate before executing
    let check_result = scheduler.check_recursion_depth(&parent).await;

    if check_result.is_ok() {
        scheduler.record_spawn(&parent, &child).await;
        ctx.recursion_check_result = Some(Ok(()));
    } else {
        ctx.recursion_check_result = Some(check_result.map_err(|e| e.to_string()));
    }
}
```

---

## Code Quality Checklist

Before submitting BDD tests for review, verify:

### Feature Files

- [ ] Scenarios have descriptive names
- [ ] Steps focus on behavior, not implementation
- [ ] Scenarios are independent
- [ ] Tags are used for organization
- [ ] Examples are provided for Scenario Outlines

### Step Definitions

- [ ] All steps are async
- [ ] Assertions have clear error messages
- [ ] Results are stored for later assertions
- [ ] No assertions in When steps
- [ ] Error handling uses `.expect()` with context

### Context Management

- [ ] Unused fields are removed
- [ ] Arc is used for shared state
- [ ] Helper methods are provided
- [ ] Default implementation exists

### Concurrency Testing

- [ ] No memory leaks from forgotten resources
- [ ] Explicit cleanup for intentional `std::mem::forget`
- [ ] No unsafe lifetime extension
- [ ] Time-based tests use actual delays

### General

- [ ] All tests pass
- [ ] No compiler warnings
- [ ] Code is formatted (`cargo fmt`)
- [ ] Documentation is updated

---

## Examples

### Complete Example: Scheduler Tests

See `core/tests/features/scheduler/lane_scheduler.feature` and `core/tests/steps/scheduler_steps.rs` for a comprehensive example of:

- Priority-based scheduling
- Concurrency limits
- Anti-starvation logic
- Recursion depth tracking

### Key Commits

- `fb901b14`: Fix critical issues in BDD tests (Phase 2, Task 5)
- Shows proper permit management, time-based testing, and validation patterns

---

## References

- [Cucumber Best Practices](https://cucumber.io/docs/bdd/better-gherkin/)
- [cucumber-rs Documentation](https://docs.rs/cucumber/)
- [Tokio Testing Guide](https://tokio.rs/tokio/topics/testing)
- [BDD Migration Plan](./plans/2026-02-04-bdd-migration-implementation.md)
- [Phase 2 Implementation](./plans/2026-02-05-multi-agent-2.0-phase2-impl.md)

---

## Getting Help

- Check existing feature files for patterns
- Review step definitions in `core/tests/steps/`
- Consult this guide for best practices
- Ask in team chat for clarification
