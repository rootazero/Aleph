//! Stage 5a — noop perf observation. Asserts that an empty registry has
//! near-zero overhead (master spec acceptance: noop path zero-allocation /
//! zero-await in steady state).
//!
//! This is a smoke benchmark, not a strict gate. The threshold is loose
//! enough to run cleanly on CI under load while still flagging an obvious
//! regression (e.g. accidental synchronous lock under the hot path).

use crate::guardrails::registry::GuardrailRegistry;

#[tokio::test]
async fn noop_input_evaluation_is_fast() {
    let r = GuardrailRegistry::builder().build();
    let n = 10_000usize;
    let start = std::time::Instant::now();
    for _ in 0..n {
        let _ = r.evaluate_input("benign").await;
    }
    let elapsed = start.elapsed();
    // This is a regression smoke test, not a microbenchmark. The bound must be
    // generous enough to survive a CI runner that is CPU-starved by 12k other
    // tests running in parallel (a tight 100ms bound flaked there). A real
    // hot-path regression (sync lock / per-iter allocation) is orders of
    // magnitude slower than this, so 2s still flags it.
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "noop evaluation slow: {:?} for {} iters",
        elapsed,
        n
    );
}

#[tokio::test]
async fn noop_output_evaluation_is_fast() {
    let r = GuardrailRegistry::builder().build();
    let n = 10_000usize;
    let start = std::time::Instant::now();
    for _ in 0..n {
        let _ = r.evaluate_output("benign").await;
    }
    // Generous bound for the same reason as noop_input_evaluation_is_fast:
    // survives a CPU-starved CI runner while still flagging a real regression.
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

#[tokio::test]
async fn disable_all_idempotent_under_repeated_calls() {
    let r = GuardrailRegistry::builder().build();
    for _ in 0..1_000 {
        r.disable_all();
        r.enable_all();
    }
    assert!(r.is_enabled());
}
