//! Trace API — wraps trace.list / trace.get RPC methods.

use crate::context::DashboardState;
use aleph_protocol::AgentTraceReplay;

pub struct TraceApi;

impl TraceApi {
    // `TraceApi::list` was CUT on 2026-08-29. `trace.list` is admin-gated and
    // exists for the two operator debugging surfaces (`aleph trace list`, the
    // TUI's `/replay list`); the Panel calls `get` and `by_runs` only, and a
    // repo-wide grep found zero call sites for `list`. It also carried the
    // CLI's parse bug verbatim — a zero-consumer wrapper around a broken
    // contract is the R10 case exactly: cutting it is cheaper than repairing
    // a caller that does not exist. Re-add it against `AgentTraceListPage` on
    // the day a Panel view actually needs the listing.

    /// Fetch a full trace replay for a given task.
    pub async fn get(state: &DashboardState, task_id: &str) -> Result<AgentTraceReplay, String> {
        let result = state
            .rpc_call("trace.get", serde_json::json!({ "task_id": task_id }))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse trace: {e}"))
    }

    /// Fetch persisted agent-trace events for the given `run_ids`, grouped by
    /// `run_id` (= `task_id`). Used to rehydrate the chat step strip + workspace
    /// panel after reload / session switch. Unknown runs map to empty vecs.
    ///
    /// `session_key` is the session those runs belong to and is REQUIRED: a
    /// trace is a full transcript and the trace store records no owner, so
    /// the server resolves ownership through the session instead — it refuses
    /// a session the caller does not own, and serves only the runs that
    /// session's own transcript claims (P1). A run outside that set comes
    /// back as an empty vec, exactly like an unknown one.
    pub async fn by_runs(
        state: &DashboardState,
        session_key: &str,
        run_ids: Vec<String>,
    ) -> Result<std::collections::HashMap<String, Vec<serde_json::Value>>, String> {
        let result = state
            .rpc_call(
                "trace.by_runs",
                serde_json::json!({ "session_key": session_key, "run_ids": run_ids }),
            )
            .await?;
        let runs = result
            .get("runs")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));
        serde_json::from_value(runs).map_err(|e| format!("Failed to parse trace.by_runs: {e}"))
    }
}
