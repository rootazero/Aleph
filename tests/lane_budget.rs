//! Stage C integration: verify lane budget enforcement for subagent spawns.
//! - 4 concurrent subagent spawns succeed.
//! - 5th spawn fails with "subagent lane budget exhausted" error string.
//! - After one completes, the 5th can succeed.

use std::sync::Arc;

use alephcore::scheduler::{Lane, LaneConfig, LaneScheduler};

#[tokio::test]
async fn try_reserve_4_ok_5th_busy_then_recover() {
    let scheduler = Arc::new(LaneScheduler::new(LaneConfig::default()));

    // Reserve all 4 Subagent slots.
    let mut held = vec![];
    for i in 0..4 {
        let guard = scheduler
            .try_reserve(format!("sub-{i}"), Lane::Subagent)
            .await
            .expect("first 4 reserves should succeed");
        held.push(guard);
    }

    // 5th must fail.
    let err = scheduler
        .try_reserve("sub-5".to_string(), Lane::Subagent)
        .await
        .expect_err("5th reserve should fail");
    assert!(
        format!("{err}").contains("budget exhausted"),
        "error message should mention 'budget exhausted': {err}"
    );

    // Release one slot via on_run_complete (simulates spawn exit).
    let released_guard = held.pop().unwrap();
    scheduler
        .on_run_complete("sub-3", Lane::Subagent, Some(released_guard))
        .await;

    // 5th can now succeed.
    let _guard = scheduler
        .try_reserve("sub-5".to_string(), Lane::Subagent)
        .await
        .expect("after release, reserve should succeed");
}
