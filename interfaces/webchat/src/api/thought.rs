//! Typed client for explicit, connection-scoped thought translation.

use aleph_protocol::thought::{
    ThoughtTranslationCancel, ThoughtTranslationEngines, ThoughtTranslationRequest,
};
use serde_json::json;

use crate::context::DashboardState;

pub(crate) struct ThoughtApi;

impl ThoughtApi {
    pub(crate) async fn engines(dash: DashboardState) -> ThoughtTranslationEngines {
        dash.rpc_call("thought.translation.engines", json!({}))
            .await
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    pub(crate) async fn start(
        dash: DashboardState,
        epoch: u64,
        request: ThoughtTranslationRequest,
    ) -> Result<(), String> {
        let params = serde_json::to_value(request).map_err(|error| error.to_string())?;
        dash.rpc_call_at_epoch(epoch, "thought.translation.start", params)
            .await
            .map(|_| ())
    }

    pub(crate) async fn cancel(dash: DashboardState, epoch: u64, request_id: String) {
        if let Ok(params) = serde_json::to_value(ThoughtTranslationCancel { request_id }) {
            let _ = dash
                .rpc_call_at_epoch(epoch, "thought.translation.cancel", params)
                .await;
        }
    }
}
