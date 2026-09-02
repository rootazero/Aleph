//! RPC client for the `runtime.agents.*` gateway surface — the agent panel's
//! snapshot fetch. Shape copied from `api/acp.rs`'s `AcpApi::list` (a thin
//! `rpc_call` + `serde_json::from_value` wrapper, nothing else).
//!
//! Named `runtime_agents`, not `runtime`, because `api::runtimes` already
//! exists and means something unrelated: runtime *installers* (node/python
//! capability install), rendered by `platform/wide/views/runtimes.rs`. Two
//! files one character apart with different meanings would be a wrong label
//! (判据 §17), so this one spells out what it actually is.

use aleph_protocol::runtime::{RuntimeAgentsListResponse, RUNTIME_AGENTS_LIST_METHOD};
use serde_json::Value;

use crate::context::DashboardState;

pub struct RuntimeAgentsApi;

impl RuntimeAgentsApi {
    pub async fn list(state: &DashboardState) -> Result<RuntimeAgentsListResponse, String> {
        let result = state.rpc_call(RUNTIME_AGENTS_LIST_METHOD, Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse runtime agents list: {e}"))
    }
}
