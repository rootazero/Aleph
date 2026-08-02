//! Tool scheduling overhaul — integration coverage.
//!
//! This file's original subject — "an unhealthy probe strips the tool from the
//! dispatcher's smart-prompt schema" — is no longer reachable from here. The
//! health gate moved to the single enforcement point, `src/tools/scoped/`, and
//! `ToolCatalog::generate_smart_prompt` was cut with the layer it belonged to.
//! Its coverage moved with it and got stronger on the way: see
//! `scoped::tests::{list_strips_unhealthy_tools,
//! metadata_schema_strips_unhealthy_tools_and_invalidates_on_flip}` — the
//! latter also proves the schema cache rotates on a health-generation flip,
//! which the version that lived here could not observe.
//!
//! What remains below is the probe/cache contract seen from outside the crate.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use alephcore::tool_metadata::{HealthReason, ProbeResult, ToolHealthCache, ToolHealthProbe};

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
