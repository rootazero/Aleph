//! Tool scheduling overhaul — integration coverage.
//!
//! Commit 1 (ToolHealthGate): verifies that an unhealthy probe strips a
//! tool from the dispatcher's emitted prompt + smart-prompt schema, and
//! that invalidation flips the result back the next turn.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use alephcore::dispatcher::{HealthReason, ProbeResult, ToolHealthCache, ToolHealthProbe};

/// A probe that always reports the same canned result.
struct CannedProbe(ProbeResult);

#[async_trait::async_trait]
impl ToolHealthProbe for CannedProbe {
    async fn probe(&self) -> ProbeResult {
        self.0.clone()
    }
    fn ttl(&self) -> Duration {
        // Short TTL so tests don't drift on repeated runs in the same
        // process; the 200ms guard inside the cache still applies.
        Duration::from_secs(5)
    }
}

#[tokio::test]
async fn missing_probe_reports_healthy() {
    let cache = ToolHealthCache::new();
    let snap = cache.snapshot();
    assert!(snap.is_healthy("ghost_tool"));
    assert!(snap.reason("ghost_tool").is_none());
}

#[tokio::test]
async fn dead_probe_makes_tool_unhealthy_until_invalidation() {
    let cache = ToolHealthCache::new();
    let dead = Arc::new(CannedProbe(ProbeResult::Unhealthy {
        reason: HealthReason::DependencyDown(Cow::Borrowed("fixture")),
        retry_after: None,
    }));
    cache.register_probe("dead_tool", dead);
    assert!(cache.needs_refresh("dead_tool"));
    cache.refresh("dead_tool").await;
    let snap = cache.snapshot();
    assert!(!snap.is_healthy("dead_tool"));
    let reason = snap.reason("dead_tool").expect("reason cached");
    assert_eq!(reason.short_label(), "fixture");

    // Invalidate: tool is healthy again (membership change semantics).
    cache.invalidate_all();
    assert!(cache.snapshot().is_healthy("dead_tool"));
}

#[tokio::test]
async fn dispatcher_smart_prompt_filters_unhealthy_tools() {
    use alephcore::dispatcher::{ToolRegistry, UnifiedTool};

    let registry = ToolRegistry::new();

    // Register two builtin-shaped tools by way of the refresh path.
    let tools = vec![
        UnifiedTool::builtin("alive").with_description("a"),
        UnifiedTool::builtin("dead").with_description("b"),
    ];
    registry.refresh_atomic(tools).await;

    // Probe "dead" as unhealthy.
    registry.register_health_probe(
        "dead",
        Arc::new(CannedProbe(ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed("test offline")),
            retry_after: None,
        })),
    );
    // Force a synchronous refresh so the probe result is in the snapshot
    // before we query — the live `trigger_health_refresh` fires
    // `tokio::spawn` which is racy in a single-tick test.
    registry.health().refresh("dead").await;

    let (full, _index) = registry
        .generate_smart_prompt(&["alive", "dead"], &[])
        .await;
    let names: Vec<&str> = full.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"alive"), "alive tool should remain visible");
    assert!(
        !names.contains(&"dead"),
        "dead tool must be stripped from the schema; got: {names:?}"
    );

    // After invalidation the cache forgets the unhealthy entry → tool
    // reappears (until the next refresh repopulates the probe result).
    registry.health().invalidate_all();
    let (full2, _) = registry
        .generate_smart_prompt(&["alive", "dead"], &[])
        .await;
    let names2: Vec<&str> = full2.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names2.contains(&"dead"),
        "after invalidation, dead reappears until next probe re-fires; got: {names2:?}"
    );
}
