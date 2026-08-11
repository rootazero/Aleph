// Tool-approval overlay controller (Ask exec tier).
//
// A parked server run in Ask tier waits on `exec.approval.resolve` for a human
// decision. The thin TUI gets no approval StreamEvent to react to, so it polls
// `exec.approvals.pending` from the main-loop tick (only while a run is active)
// and resolves the user's choice.
//
// The session filter in `select_session_approval` is load-bearing for a UX
// reason, not a security one: the TUI must never surface, let alone resolve, an
// approval raised for a different session than the one on screen.
//
// It used to be the ONLY thing standing there — this comment read
// "`exec.approvals.pending` is GLOBAL across every session and
// `exec.approval.resolve` has no server-side ownership check". Both halves have
// been false since 2026-08-08: both methods are owner-scoped server-side
// (`gateway::handlers::exec_approvals`). Left in place because a client-side
// filter is still the right shape for "which card belongs on THIS screen".

use serde_json::{json, Value};

use aleph_client::AlephClient;

use super::app::{self, AppState, Focus};

/// A pending approval that belongs to this session, projected to the fields the
/// overlay renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingApprovalView {
    pub id: String,
    pub command: String,
    pub reason: Option<String>,
    /// The decision tiers the server raised this card with. Absent from an
    /// older core, where the historical three are the right reading —
    /// `offered_decisions` supplies them.
    pub decisions: Vec<(&'static str, &'static str)>,
}

/// Pick the oldest pending approval that belongs to `session_key` from an
/// `exec.approvals.pending` response
/// (`{ "pending": [ { "record": { .. }, "remaining_ms": .. } ] }`, oldest
/// first). Records whose `session_key` does not match — or which lack an id —
/// are skipped. This is the load-bearing safety filter (see module docs).
pub(super) fn select_session_approval(
    resp: &Value,
    session_key: &str,
) -> Option<PendingApprovalView> {
    let pending = resp.get("pending")?.as_array()?;
    for item in pending {
        let Some(record) = item.get("record") else {
            continue;
        };
        if record.get("session_key").and_then(Value::as_str) != Some(session_key) {
            continue;
        }
        let Some(id) = record.get("id").and_then(Value::as_str) else {
            continue;
        };
        let command = record
            .get("command")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("(tool execution)")
            .to_string();
        let reason = record
            .get("reason")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let allowed: Vec<String> = record
            .get("allowed_decisions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        return Some(PendingApprovalView {
            id: id.to_string(),
            command,
            reason,
            decisions: app::offered_decisions(&allowed),
        });
    }
    None
}

/// Is `id` still present anywhere in the pending list? Used to retract a showing
/// overlay when its approval was resolved on another surface (Panel / channel)
/// or expired server-side.
pub(super) fn approval_id_present(resp: &Value, id: &str) -> bool {
    resp.get("pending")
        .and_then(Value::as_array)
        .is_some_and(|pending| {
            pending.iter().any(|item| {
                item.get("record")
                    .and_then(|r| r.get("id"))
                    .and_then(Value::as_str)
                    == Some(id)
            })
        })
}

/// Poll `exec.approvals.pending` and reconcile the overlay. Called from the
/// main-loop tick at ~1s cadence while a run is active. Opens the overlay for a
/// newly-pending, session-owned approval and retracts a showing one that has
/// since vanished. A transient RPC error is swallowed — the next tick retries.
pub(super) async fn poll_approvals(state: &mut AppState, client: &AlephClient) {
    let Ok(resp) = client
        .call::<_, Value>("exec.approvals.pending", None::<()>)
        .await
    else {
        return;
    };

    // One prompt at a time: if an overlay is already up, only check whether its
    // approval is still live (resolved elsewhere / expired → retract it).
    if let Some(current) = &state.approval {
        if !approval_id_present(&resp, &current.id) {
            state.close_approval();
        }
        return;
    }

    // Don't yank the user out of another overlay (palette / dialog / picker);
    // the approval stays pending server-side and a later idle tick surfaces it.
    if !matches!(state.focus, Focus::Input | Focus::Chat) {
        return;
    }

    if let Some(view) = select_session_approval(&resp, &state.session_key) {
        state.open_approval(view.id, view.command, view.reason, view.decisions);
    }
}

/// Resolve the showing approval by decision index and tell the server. On
/// failure the overlay is left closed — the approval is still pending, so the
/// next poll re-surfaces it.
pub(super) async fn resolve_approval(state: &mut AppState, client: &AlephClient, index: usize) {
    let Some(approval) = state.approval.take() else {
        return;
    };
    state.focus = Focus::Input;

    // Index into the decisions THIS card offered, not a global list: the two
    // differ whenever the server narrowed the set, and indexing the wrong one
    // sends a decision the user did not pick.
    let Some((label, decision)) = approval.decisions.get(index).copied() else {
        return;
    };
    let params = json!({
        "id": approval.id,
        "decision": decision,
        "resolved_by": "TUI",
    });
    match client
        .call::<_, Value>("exec.approval.resolve", Some(params))
        .await
    {
        Ok(_) => {
            state.add_system_message(format!("Tool approval — {label}: {}", approval.command));
        }
        Err(e) => state.add_system_message(format!("Approval resolve error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(session: &str, id: &str) -> Value {
        json!({
            "record": {
                "id": id,
                "session_key": session,
                "command": format!("cmd-{id}"),
                "reason": "destructive",
            },
            "remaining_ms": 90_000,
        })
    }

    /// The overlay renders what the SERVER said this card may offer. A record
    /// carrying the persistent tier gets four options; one without the field
    /// (an older core) falls back to the historical three and — crucially —
    /// never invents `allow-always`.
    #[test]
    fn the_overlay_offers_exactly_what_the_record_allows() {
        let mut with_always = record("s1", "a1");
        with_always["record"]["allowed_decisions"] =
            json!(["allow-once", "allow-session", "allow-always", "deny"]);
        let resp = json!({ "pending": [with_always] });
        let view = select_session_approval(&resp, "s1").expect("view");
        let wires: Vec<&str> = view.decisions.iter().map(|(_, w)| *w).collect();
        assert_eq!(
            wires,
            vec!["allow-once", "allow-session", "allow-always", "deny"]
        );

        let resp = json!({ "pending": [record("s1", "a1")] });
        let view = select_session_approval(&resp, "s1").expect("view");
        let wires: Vec<&str> = view.decisions.iter().map(|(_, w)| *w).collect();
        assert_eq!(
            wires,
            vec!["allow-once", "allow-session", "deny"],
            "a missing field may narrow what a client offers, never widen it"
        );
    }

    /// A narrowed card is the case the index bug lives in: option 2 of a
    /// two-option card is `deny`, not the third entry of a global list.
    #[test]
    fn a_narrowed_card_indexes_against_its_own_options() {
        let mut narrow = record("s1", "a1");
        narrow["record"]["allowed_decisions"] = json!(["allow-once", "deny"]);
        let resp = json!({ "pending": [narrow] });
        let view = select_session_approval(&resp, "s1").expect("view");
        assert_eq!(view.decisions.len(), 2);
        assert_eq!(view.decisions[1].1, "deny");
    }

    #[test]
    fn selects_only_our_session_oldest_first() {
        let resp = json!({
            "pending": [
                record("other", "a"),
                record("mine", "b"),
                record("mine", "c"),
            ]
        });
        let picked = select_session_approval(&resp, "mine").expect("a mine-owned approval");
        assert_eq!(picked.id, "b"); // oldest of ours; the "other" one is skipped
        assert_eq!(picked.command, "cmd-b");
        assert_eq!(picked.reason.as_deref(), Some("destructive"));
    }

    #[test]
    fn ignores_foreign_sessions_entirely() {
        // Safety: an approval for another session must never be selectable.
        let resp = json!({ "pending": [ record("panel", "x"), record("other", "y") ] });
        assert!(select_session_approval(&resp, "mine").is_none());
    }

    #[test]
    fn empty_or_malformed_yields_none() {
        assert!(select_session_approval(&json!({ "pending": [] }), "mine").is_none());
        assert!(select_session_approval(&json!({}), "mine").is_none());
        // A record missing its id is skipped, not selected.
        let no_id = json!({ "pending": [ { "record": { "session_key": "mine" } } ] });
        assert!(select_session_approval(&no_id, "mine").is_none());
    }

    #[test]
    fn command_falls_back_when_absent() {
        let resp = json!({ "pending": [ { "record": { "id": "z", "session_key": "mine" } } ] });
        let picked = select_session_approval(&resp, "mine").unwrap();
        assert_eq!(picked.command, "(tool execution)");
        assert_eq!(picked.reason, None);
    }

    #[test]
    fn id_presence_tracks_resolution() {
        let resp = json!({ "pending": [ record("mine", "b") ] });
        assert!(approval_id_present(&resp, "b"));
        assert!(!approval_id_present(&resp, "gone"));
        assert!(!approval_id_present(&json!({ "pending": [] }), "b"));
    }
}
