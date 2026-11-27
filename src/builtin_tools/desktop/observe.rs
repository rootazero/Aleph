//! Opt-in observation of the subject actually reached by an action.

use std::time::Duration;

use aleph_desktop::DesktopPlatform;
use aleph_protocol::desktop_bridge::methods::ax::QueryFocusedParams;
use serde_json::{json, Value};

const SETTLE_MS: u64 = 300;

#[derive(Clone, Copy)]
pub(super) enum PostTarget {
    Global,
    Process { pid: i32, window_id: Option<u32> },
    Unavailable,
}

impl PostTarget {
    pub(super) fn process(pid: i32, window_id: Option<u32>) -> Self {
        if pid > 0 {
            Self::Process {
                pid,
                window_id: window_id.filter(|id| *id != 0),
            }
        } else {
            Self::Unavailable
        }
    }
}

/// Report observations separately from action success. An unavailable scope
/// never interrogates the system foreground or the screen.
pub(super) async fn gather_post_state(platform: &dyn DesktopPlatform, target: PostTarget) -> Value {
    let (mut state, pid) = match target {
        PostTarget::Global => (json!({"scope": "global"}), None),
        PostTarget::Process { pid, .. } => {
            (json!({"scope": "process", "target_pid": pid}), Some(pid))
        }
        PostTarget::Unavailable => {
            return json!({"scope": "unavailable", "focus_status": "unavailable"})
        }
    };

    tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
    if pid.is_none() {
        if let Some(system) = platform.system() {
            if let Ok(apps) = system.list_running_apps().await {
                if let Some(front) = apps.iter().find(|a| a.is_active) {
                    state["frontmost_app"] = json!(front.name);
                }
            }
        }
    }

    let focused = match platform.ax() {
        Some(ax) => ax.query_focused(QueryFocusedParams { pid }).await.ok(),
        None => None,
    };
    match focused {
        Some(Some(el)) if pid.is_none_or(|expected| el.pid == expected) => {
            state["focused_element"] = json!({
                "role": el.role,
                "title": el.title,
                "value": super::interactable::safe_value(&el)
                    .map(|value| value.chars().take(200).collect::<String>()),
            });
        }
        Some(None) => state["focus_status"] = json!("not_found"),
        None | Some(Some(_)) => state["focus_status"] = json!("unavailable"),
    }
    state
}
