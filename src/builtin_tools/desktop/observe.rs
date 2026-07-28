//! Post-action observation — closes the act→observe loop in one tool call.
//!
//! After a successful mutating action the model usually needs to see what
//! happened before it can plan the next step. Without this it burns a whole
//! extra round-trip on `screenshot` / `ax_snapshot`. orca returns a fresh
//! snapshot from every action; UI-TARS re-screenshots after every action.
//! Aleph makes it opt-in per call via `observe: "state" | "screenshot"`.

use std::time::Duration;

use aleph_protocol::desktop_bridge::methods::ax::QueryFocusedParams;
use serde_json::json;

use crate::sync_primitives::Arc;

/// Settle delay before observing — UI needs a beat to react (UI-TARS
/// `loopIntervalInMs` parity).
const SETTLE_MS: u64 = 300;

/// Gather a lightweight textual post-action state: frontmost app and the
/// focused element. Every part is best-effort — a missing capability or a
/// query error just omits that field (this must never fail the action that
/// already succeeded).
pub(super) async fn gather_post_state(
    platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
) -> serde_json::Value {
    tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;

    let mut state = serde_json::Map::new();

    if let Some(system) = platform.system() {
        if let Ok(apps) = system.list_running_apps().await {
            if let Some(front) = apps.iter().find(|a| a.is_active) {
                state.insert("frontmost_app".into(), json!(front.name));
            }
        }
    }

    if let Some(ax) = platform.ax() {
        if let Ok(Some(el)) = ax.query_focused(QueryFocusedParams::default()).await {
            state.insert(
                "focused_element".into(),
                json!({
                    "role": el.role,
                    "title": el.title,
                    // Through `safe_value`, not `el.value`: post-action state is
                    // reported for whatever holds focus, which may well be the
                    // password box the user just filled.
                    "value": super::interactable::safe_value(&el).map(|v| {
                        v.chars().take(200).collect::<String>()
                    }),
                }),
            );
        }
    }

    serde_json::Value::Object(state)
}
