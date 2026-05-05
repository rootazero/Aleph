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
    let r = GuardrailRegistry::empty();
    let n = 10_000usize;
    let start = std::time::Instant::now();
    for _ in 0..n {
        let _ = r.evaluate_input("benign").await;
    }
    let elapsed = start.elapsed();
    // 10k noop evaluations should comfortably finish under 100ms even on a
    // CI runner under heavy load. If this fires, suspect that a hot-path
    // allocation or lock crept in.
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "noop evaluation slow: {:?} for {} iters",
        elapsed,
        n
    );
}

#[tokio::test]
async fn noop_output_evaluation_is_fast() {
    let r = GuardrailRegistry::empty();
    let n = 10_000usize;
    let start = std::time::Instant::now();
    for _ in 0..n {
        let _ = r.evaluate_output("benign").await;
    }
    assert!(start.elapsed() < std::time::Duration::from_millis(100));
}

#[tokio::test]
async fn disable_all_idempotent_under_repeated_calls() {
    let r = GuardrailRegistry::empty();
    for _ in 0..1_000 {
        r.disable_all();
        r.enable_all();
    }
    assert!(r.is_enabled());
}
