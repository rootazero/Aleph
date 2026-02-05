//! Step definitions for Scheduler features

use cucumber::{given, when, then};

use crate::world::{AlephWorld, SchedulerContext};

// =============================================================================
// LaneState: Given Steps
// =============================================================================

#[given(expr = "a LaneState with max_concurrent {int}")]
async fn given_lane_state_with_max_concurrent(w: &mut AlephWorld, max_concurrent: usize) {
    let ctx = w.scheduler.get_or_insert_with(SchedulerContext::default);
    ctx.create_lane_state(max_concurrent);
}

#[given(expr = "I enqueue run {string}")]
async fn given_enqueue_run(w: &mut AlephWorld, run_id: String) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    lane_state.enqueue(run_id).await;
}

#[given(expr = "I enqueue run {string} at timestamp {int}")]
async fn given_enqueue_run_at_timestamp(w: &mut AlephWorld, run_id: String, timestamp: i64) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    lane_state.enqueue_at(run_id, timestamp).await;
}

// =============================================================================
// LaneState: When Steps
// =============================================================================

#[when(expr = "I enqueue run {string}")]
async fn when_enqueue_run(w: &mut AlephWorld, run_id: String) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    lane_state.enqueue(run_id).await;
}

#[when(expr = "I enqueue run {string} at timestamp {int}")]
async fn when_enqueue_run_at_timestamp(w: &mut AlephWorld, run_id: String, timestamp: i64) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    lane_state.enqueue_at(run_id, timestamp).await;
}

#[when("I try to dequeue a run")]
async fn when_try_dequeue_run(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    ctx.dequeued_run_id = lane_state.try_dequeue().await;
    ctx.last_dequeue_result = Some(ctx.dequeued_run_id.is_some());
}

#[when("I try to dequeue a run with semaphore")]
async fn when_try_dequeue_run_with_semaphore(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");

    // Try to acquire a permit and dequeue
    if let Some(_permit) = lane_state.try_acquire_permit() {
        if let Some(run_id) = lane_state.try_dequeue().await {
            // Mark as running
            lane_state.mark_running(run_id.clone()).await;
            ctx.dequeued_run_id = Some(run_id);
            ctx.last_dequeue_result = Some(true);
            ctx.held_permits_count += 1;

            // Forget the permit so it stays acquired
            std::mem::forget(_permit);
        } else {
            ctx.last_dequeue_result = Some(false);
        }
    } else {
        ctx.last_dequeue_result = Some(false);
    }
}

#[when(expr = "I complete run {string}")]
async fn when_complete_run(w: &mut AlephWorld, run_id: String) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");

    // Mark as completed (removes from running set)
    lane_state.complete(&run_id).await;

    // Decrement the held permits count
    if ctx.held_permits_count > 0 {
        ctx.held_permits_count -= 1;
    }

    // Add a permit back to the semaphore by acquiring and immediately dropping
    // This simulates releasing a permit that was forgotten
    lane_state.semaphore().add_permits(1);
}

#[when(expr = "I calculate priority boost for {string} at timestamp {int}")]
async fn when_calculate_priority_boost(w: &mut AlephWorld, run_id: String, current_time: i64) {
    let ctx = w.scheduler.as_mut().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    ctx.priority_boost = Some(lane_state.calculate_priority_boost(&run_id, current_time).await);
}

// =============================================================================
// LaneState: Then Steps
// =============================================================================

#[then(expr = "the queue should have {int} runs")]
async fn then_queue_should_have_runs(w: &mut AlephWorld, expected_count: usize) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    let actual_count = lane_state.queue_len().await;
    assert_eq!(
        actual_count, expected_count,
        "Expected queue to have {} runs, but found {}",
        expected_count, actual_count
    );
}

#[then(expr = "the dequeued run should be {string}")]
async fn then_dequeued_run_should_be(w: &mut AlephWorld, expected_run_id: String) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let actual_run_id = ctx.dequeued_run_id.as_ref().expect("No run was dequeued");
    assert_eq!(
        actual_run_id, &expected_run_id,
        "Expected dequeued run to be '{}', but got '{}'",
        expected_run_id, actual_run_id
    );
}

#[then("the dequeue should succeed")]
async fn then_dequeue_should_succeed(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let result = ctx.last_dequeue_result.expect("No dequeue attempt was made");
    assert!(result, "Expected dequeue to succeed, but it failed");
}

#[then("the dequeue should fail due to semaphore limit")]
async fn then_dequeue_should_fail_due_to_semaphore(w: &mut AlephWorld) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let result = ctx.last_dequeue_result.expect("No dequeue attempt was made");
    assert!(!result, "Expected dequeue to fail due to semaphore limit, but it succeeded");
}

#[then(expr = "there should be {int} running run")]
#[then(expr = "there should be {int} running runs")]
async fn then_there_should_be_running_runs(w: &mut AlephWorld, expected_count: usize) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    let actual_count = lane_state.running_count().await;
    assert_eq!(
        actual_count, expected_count,
        "Expected {} running runs, but found {}",
        expected_count, actual_count
    );
}

#[then(expr = "run {string} should be running")]
async fn then_run_should_be_running(w: &mut AlephWorld, run_id: String) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let lane_state = ctx.lane_state.as_ref().expect("LaneState not created");
    let is_running = lane_state.is_running(&run_id).await;
    assert!(is_running, "Expected run '{}' to be running, but it is not", run_id);
}

#[then(expr = "the priority boost should be at least {int}")]
async fn then_priority_boost_should_be_at_least(w: &mut AlephWorld, min_boost: i8) {
    let ctx = w.scheduler.as_ref().expect("Scheduler context not initialized");
    let boost = ctx.priority_boost.expect("No priority boost was calculated");
    assert!(
        boost >= min_boost,
        "Expected priority boost to be at least {}, but got {}",
        min_boost, boost
    );
}
