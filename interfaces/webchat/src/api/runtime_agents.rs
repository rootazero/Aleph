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

use crate::context::{DashboardState, RpcFailure};

pub struct RuntimeAgentsApi;

impl RuntimeAgentsApi {
    /// Keeps the JSON-RPC error **code** (`rpc_call_with_code`, not
    /// `rpc_call`). The panel splits "the operator gate refused" from
    /// "the call did not come back" on that code and on nothing else —
    /// classifying by words in the message is what P8 forbids, and the
    /// projection to `String` throws away the only field that can answer
    /// the question honestly.
    ///
    /// A decode failure is minted here with `code: None`, the same shape
    /// `RpcFailure` uses for every locally-produced failure: this client
    /// deciding it cannot read a response is not a server verdict, and must
    /// never be able to impersonate one.
    pub async fn list(state: &DashboardState) -> Result<RuntimeAgentsListResponse, RpcFailure> {
        let result = state
            .rpc_call_with_code(RUNTIME_AGENTS_LIST_METHOD, Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| RpcFailure {
            code: None,
            message: format!("Failed to parse runtime agents list: {e}"),
        })
    }
}
