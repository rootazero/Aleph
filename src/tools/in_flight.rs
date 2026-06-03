//! In-flight tool-call registry — surfaces every running tool call to the
//! gateway so external callers (RPC, CLI, panel UI) can cancel a single call
//! without aborting the whole run.
//!
//! See Gap B follow-up: the harness Act phase forks a per-call child
//! `CancellationToken` from the run-level cancel. This registry holds those
//! tokens keyed by the LLM-issued `tool_call_id`, so the new
//! `tools.cancel_call` RPC can fire the right one without reaching into
//! harness internals.
//!
//! # Lifecycle
//!
//! * Register on entry to `act()` with [`InFlightToolCalls::register`] —
//!   returns a [`InFlightGuard`] RAII guard.
//! * The guard removes the entry on drop, even on panic or early `?` return.
//! * [`InFlightToolCalls::cancel`] fires the token; the registered call's
//!   `tokio::select!` (in adapters wired in Gap B) short-circuits, returns a
//!   cancelled `ToolError`, and the act-loop continues with the next call.
//!
//! # Concurrency
//!
//! Wraps a `Mutex<HashMap>` because lookups happen once per call
//! (registration + deregistration), not in any hot loop. Lock contention is
//! essentially nil at the steady-state scale of one harness per session.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::sync_primitives::Mutex;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

// =============================================================================
// Process-wide singleton
// =============================================================================
//
// Same pattern as `crate::tools::result_store::GLOBAL_STORE`: rather than
// thread the `Arc` through every constructor between server boot and the
// orchestrator's `HarnessDeps`, install one at startup via
// `set_global_in_flight_tool_calls` and pull it back via
// `global_in_flight_tool_calls`. The gateway's `tools.cancel_call` RPC reads
// the same singleton.

static GLOBAL_REGISTRY: OnceLock<InFlightToolCalls> = OnceLock::new();

/// Install the process-wide in-flight registry. Idempotent — subsequent calls
/// are silently ignored so multiple boot paths cannot stomp each other.
pub fn set_global_in_flight_tool_calls(reg: InFlightToolCalls) {
    let _ = GLOBAL_REGISTRY.set(reg);
}

/// Read the process-wide in-flight registry, if installed.
pub fn global_in_flight_tool_calls() -> Option<InFlightToolCalls> {
    GLOBAL_REGISTRY.get().cloned()
}

/// Registry of in-flight tool calls keyed by `tool_call_id`.
///
/// Cheap to clone — internally an `Arc<Mutex<...>>`. Holds `CancellationToken`
/// references handed in by the harness; firing one of those tokens triggers
/// the cooperative-cancel paths wired in Gap B (adapters, scoped service,
/// subagent runtimes).
#[derive(Debug, Default, Clone)]
pub struct InFlightToolCalls {
    inner: Arc<Mutex<HashMap<String, InFlightEntry>>>,
}

#[derive(Debug, Clone)]
struct InFlightEntry {
    tool_name: String,
    token: CancellationToken,
    started_at_ms: u64,
}

/// Public snapshot of one in-flight call, returned by [`InFlightToolCalls::list`].
#[derive(Debug, Clone, Serialize)]
pub struct InFlightSnapshot {
    pub call_id: String,
    pub tool_name: String,
    pub started_at_ms: u64,
}

impl InFlightToolCalls {
    /// Construct an empty registry. Production callers should share the same
    /// instance between the harness deps and the gateway handler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Track `call_id` as in-flight. The returned [`InFlightGuard`] removes
    /// the entry from the registry on drop — pair every `register` call with
    /// a `let _guard = ...;` binding so the cleanup runs on early-return too.
    ///
    /// A duplicate `call_id` overwrites the previous entry. In practice the
    /// LLM should never reuse a tool_call_id within a run, so a clash means
    /// either an upstream bug or a duplicate dispatch by the harness; the
    /// latest registration wins so the most recent cancel target stays valid.
    pub fn register(
        &self,
        call_id: &str,
        tool_name: &str,
        token: CancellationToken,
    ) -> InFlightGuard {
        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let entry = InFlightEntry {
            tool_name: tool_name.to_string(),
            token,
            started_at_ms,
        };
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(call_id.to_string(), entry);
        }
        InFlightGuard {
            registry: self.clone(),
            call_id: call_id.to_string(),
        }
    }

    /// Fire the cancellation token associated with `call_id`. Returns `true`
    /// if the call was found (the call's cooperative-cancel path will then
    /// short-circuit on its next await). Returns `false` if no such call is
    /// in flight — the LLM may have already completed it, or the ID is wrong.
    ///
    /// Removing the entry is left to the [`InFlightGuard`] drop on the
    /// harness side — that keeps registration and deregistration in one place
    /// and avoids a TOCTOU between cancel and natural completion.
    pub fn cancel(&self, call_id: &str) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        match guard.get(call_id) {
            Some(entry) => {
                entry.token.cancel();
                true
            }
            None => false,
        }
    }

    /// Snapshot every in-flight call. Useful for diagnostic `tools.in_flight`
    /// RPCs / CLI listings without exposing the raw token.
    pub fn list(&self) -> Vec<InFlightSnapshot> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        let mut out: Vec<InFlightSnapshot> = guard
            .iter()
            .map(|(id, entry)| InFlightSnapshot {
                call_id: id.clone(),
                tool_name: entry.tool_name.clone(),
                started_at_ms: entry.started_at_ms,
            })
            .collect();
        out.sort_by_key(|s| s.started_at_ms);
        out
    }

    /// Number of currently-in-flight calls.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// True when no calls are in flight.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// RAII guard returned by [`InFlightToolCalls::register`]. Drops the entry
/// from the registry on drop — works on normal completion, early `?` return,
/// and panic alike.
pub struct InFlightGuard {
    registry: InFlightToolCalls,
    call_id: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.registry.inner.lock() {
            guard.remove(&self.call_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_drop_removes_entry() {
        let reg = InFlightToolCalls::new();
        let token = CancellationToken::new();
        {
            let _guard = reg.register("call-1", "bash", token);
            assert_eq!(reg.len(), 1);
        }
        assert!(reg.is_empty(), "guard drop must remove the entry");
    }

    #[test]
    fn cancel_fires_token_when_call_id_known() {
        let reg = InFlightToolCalls::new();
        let token = CancellationToken::new();
        let _guard = reg.register("call-1", "bash", token.clone());

        assert!(reg.cancel("call-1"));
        assert!(
            token.is_cancelled(),
            "cancel() must fire the registered token",
        );
    }

    #[test]
    fn cancel_returns_false_for_unknown_call() {
        let reg = InFlightToolCalls::new();
        assert!(!reg.cancel("never-registered"));
    }

    #[test]
    fn list_sorts_by_started_at() {
        let reg = InFlightToolCalls::new();
        let t1 = CancellationToken::new();
        let _g1 = reg.register("a", "first", t1);
        // Ensure the second entry's timestamp is strictly later.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let t2 = CancellationToken::new();
        let _g2 = reg.register("b", "second", t2);

        let snaps = reg.list();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].call_id, "a");
        assert_eq!(snaps[1].call_id, "b");
        assert!(snaps[0].started_at_ms <= snaps[1].started_at_ms);
    }

    #[test]
    fn duplicate_register_replaces_token() {
        let reg = InFlightToolCalls::new();
        let stale = CancellationToken::new();
        let fresh = CancellationToken::new();
        let _g1 = reg.register("call-1", "bash", stale.clone());
        let _g2 = reg.register("call-1", "bash", fresh.clone());

        assert!(reg.cancel("call-1"));
        assert!(
            fresh.is_cancelled(),
            "cancel() must hit the latest registration",
        );
        assert!(
            !stale.is_cancelled(),
            "stale entry must not be fired — duplicate register replaced it",
        );
    }
}
