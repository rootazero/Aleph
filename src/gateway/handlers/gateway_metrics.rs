//! `gateway.metrics.lanes` — live per-lane occupancy gauge for diagnostics.
//!
//! Returns the snapshot produced by [`LaneManager::snapshot`] as a JSON
//! array, suitable for ops dashboards / panel UIs to detect saturation
//! before it manifests as user-visible timeouts.
//!
//! Lives on the Query lane (registered in `Lane::override_for`).

use crate::sync_primitives::Arc;

use serde_json::json;

use super::super::lane::LaneManager;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Handle `gateway.metrics.lanes`. Returns the live occupancy snapshot of
/// every lane in a fixed order (Query / Execute / Mutate / System).
pub async fn handle_gateway_metrics_lanes(
    request: JsonRpcRequest,
    lane_manager: Arc<LaneManager>,
) -> JsonRpcResponse {
    let lanes = lane_manager.snapshot();
    JsonRpcResponse::success(request.id, json!({ "lanes": lanes }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::lane::LaneConfig;
    use serde_json::json;

    #[tokio::test]
    async fn returns_one_entry_per_lane_in_fixed_order() {
        let manager = Arc::new(LaneManager::new(LaneConfig::default()));
        let req = JsonRpcRequest::with_id("gateway.metrics.lanes", None, json!(1));
        let resp = handle_gateway_metrics_lanes(req, manager).await;

        assert!(resp.is_success());
        let result = resp.result.unwrap();
        let lanes = result["lanes"].as_array().expect("lanes must be array");
        assert_eq!(lanes.len(), 4);
        assert_eq!(lanes[0]["lane"], "Query");
        assert_eq!(lanes[1]["lane"], "Execute");
        assert_eq!(lanes[2]["lane"], "Mutate");
        assert_eq!(lanes[3]["lane"], "System");

        // Single-pool lanes omit the desktop split (None → JSON null).
        assert!(lanes[0]["desktop_total"].is_null());
        assert!(lanes[0]["desktop_available"].is_null());

        // Channel-class-split lanes carry both pool sizes.
        assert!(lanes[1]["desktop_total"].as_u64().is_some());
        assert!(lanes[1]["shared_total"].as_u64().is_some());
    }
}
