//! sway / wlroots window management over the i3 IPC (`swaymsg`).
//!
//! `swaymsg` ships with sway itself, so "sway is running" implies it is
//! available. Everything here is split into a pure argv/parse half (unit-tested
//! on any host) and a thin executor.
//!
//! Window identity is sway's `con_id`, a monotonically increasing integer that
//! is stable for the lifetime of the window — it maps onto the cross-platform
//! `WindowInfo.id: u64` without a handle table.

use serde_json::Value;

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, WindowInfo};

/// `swaymsg -t get_tree` — the whole container tree as JSON.
#[must_use]
pub fn list_args() -> Vec<String> {
    vec!["-t".into(), "get_tree".into()]
}

/// `swaymsg '[con_id=N] focus'`.
///
/// The criteria and the command are one argument, not two: sway parses the
/// bracketed criteria as a prefix of the command string.
#[must_use]
pub fn focus_args(con_id: u64) -> Vec<String> {
    vec![format!("[con_id={con_id}] focus")]
}

/// `swaymsg '[con_id=N] move absolute position X Y'`.
///
/// Absolute positioning only applies to floating containers; a tiled window's
/// geometry is owned by the layout. sway reports that refusal in its JSON
/// reply, which [`check_reply`] turns into an error rather than a false success.
#[must_use]
pub fn move_args(con_id: u64, x: i32, y: i32) -> Vec<String> {
    vec![format!("[con_id={con_id}] move absolute position {x} {y}")]
}

/// `swaymsg '[con_id=N] resize set width W px height H px'`.
#[must_use]
pub fn resize_args(con_id: u64, width: u32, height: u32) -> Vec<String> {
    vec![format!(
        "[con_id={con_id}] resize set width {width} px height {height} px"
    )]
}

/// `swaymsg '[con_id=N] kill'` — sway's `kill` sends the window a close
/// request (`xdg_toplevel.close` / `WM_DELETE_WINDOW`), i.e. the polite close,
/// not a signal.
#[must_use]
pub fn close_args(con_id: u64) -> Vec<String> {
    vec![format!("[con_id={con_id}] kill")]
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// Collect the application windows out of a `get_tree` reply.
///
/// A node is a window when it carries a `pid` — sway sets that only on real
/// toplevels, never on workspaces, outputs or split containers — and has no
/// children of its own.
#[must_use]
pub fn parse_tree(json: &Value) -> Vec<WindowInfo> {
    let mut out = Vec::new();
    collect(json, &mut out);
    out
}

fn collect(node: &Value, out: &mut Vec<WindowInfo>) {
    if let Some(info) = node_to_window(node) {
        out.push(info);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                collect(child, out);
            }
        }
    }
}

fn node_to_window(node: &Value) -> Option<WindowInfo> {
    let pid = node.get("pid").and_then(Value::as_u64)?;
    let id = node.get("id").and_then(Value::as_u64)?;

    // A container that still has children is a split/tabbed container that
    // happens to inherit a pid; only leaves are windows.
    let has_children = ["nodes", "floating_nodes"].iter().any(|k| {
        node.get(*k)
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    });
    if has_children {
        return None;
    }

    let title = node
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Native Wayland toplevels carry `app_id`; XWayland ones carry the X11
    // class under `window_properties`.
    let owner = node
        .get("app_id")
        .and_then(Value::as_str)
        .or_else(|| {
            node.get("window_properties")
                .and_then(|p| p.get("class"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string();

    let bounds = node.get("rect").and_then(|r| {
        Some(BoundingBox {
            x: r.get("x").and_then(Value::as_f64)?,
            y: r.get("y").and_then(Value::as_f64)?,
            w: r.get("width").and_then(Value::as_f64)?,
            h: r.get("height").and_then(Value::as_f64)?,
        })
    });

    Some(WindowInfo {
        id,
        title,
        owner,
        pid,
        bounds,
        // sway reports no stacking level; not told is not zero.
        layer: None,
        on_screen: node.get("visible").and_then(Value::as_bool),
        })
}

/// The focused window's `con_id`, if any node in the tree claims focus.
#[must_use]
pub fn parse_focused(json: &Value) -> Option<u64> {
    if node_to_window(json).is_some() && json.get("focused").and_then(Value::as_bool) == Some(true) {
        return json.get("id").and_then(Value::as_u64);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = json.get(key).and_then(Value::as_array) {
            for child in children {
                if let Some(found) = parse_focused(child) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Turn a `swaymsg` command reply into a `Result`.
///
/// A command reply is an array of `{"success": bool, "error": "…"}`. sway exits
/// 0 even when it refuses the command (moving a tiled window, an unmatched
/// criteria), so the exit status alone would report a no-op as a success.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] when any entry reports failure, quoting
/// sway's own explanation.
pub fn check_reply(stdout: &str) -> Result<()> {
    let parsed: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        DesktopError::WindowFailed(format!("swaymsg returned unparseable JSON: {e}"))
    })?;
    let entries = parsed
        .as_array()
        .map_or_else(|| vec![parsed.clone()], Clone::clone);

    if entries.is_empty() {
        return Err(DesktopError::WindowFailed(
            "swaymsg matched no window (the con_id is stale — re-run window_list)".into(),
        ));
    }

    for entry in &entries {
        if entry.get("success").and_then(Value::as_bool) == Some(false) {
            let reason = entry
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("sway refused the command");
            return Err(DesktopError::WindowFailed(format!("swaymsg: {reason}")));
        }
    }
    Ok(())
}

// ── Runtime ──────────────────────────────────────────────────────────────────

fn run(args: Vec<String>) -> Result<String> {
    let out = std::process::Command::new("swaymsg")
        .args(&args)
        .output()
        .map_err(|e| DesktopError::WindowFailed(format!("Failed to run swaymsg: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "swaymsg failed: {}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_command(args: Vec<String>) -> Result<()> {
    check_reply(&run(args)?)
}

fn tree() -> Result<Value> {
    let stdout = run(list_args())?;
    serde_json::from_str(&stdout)
        .map_err(|e| DesktopError::WindowFailed(format!("swaymsg get_tree returned bad JSON: {e}")))
}

/// List sway's application windows.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if `swaymsg` cannot run or returns bad JSON.
pub fn window_list() -> Result<Vec<WindowInfo>> {
    Ok(parse_tree(&tree()?))
}

/// Focus a window by `con_id`.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if sway refuses or the id is stale.
pub fn focus_window(con_id: u64) -> Result<()> {
    run_command(focus_args(con_id))
}

/// Move a floating window to an absolute position.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] — notably when the window is tiled, where
/// sway owns the geometry and refuses.
pub fn move_window(con_id: u64, x: i32, y: i32) -> Result<()> {
    run_command(move_args(con_id, x, y))
}

/// Resize a window.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if sway refuses or the id is stale.
pub fn resize_window(con_id: u64, width: u32, height: u32) -> Result<()> {
    run_command(resize_args(con_id, width, height))
}

/// Ask a window to close.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if sway refuses or the id is stale.
pub fn close_window(con_id: u64) -> Result<()> {
    run_command(close_args(con_id))
}

/// The focused window's `con_id`.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if `swaymsg` cannot run or returns bad JSON.
pub fn active_window() -> Result<Option<u64>> {
    Ok(parse_focused(&tree()?))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tree() -> Value {
        json!({
            "id": 1, "type": "root", "name": "root",
            "nodes": [{
                "id": 2, "type": "output", "name": "HDMI-A-1",
                "nodes": [{
                    "id": 3, "type": "workspace", "name": "1",
                    "nodes": [
                        {
                            "id": 7, "type": "con", "name": "Firefox — Wikipedia",
                            "pid": 4242, "app_id": "firefox", "focused": true, "visible": true,
                            "rect": {"x": 0, "y": 27, "width": 1920, "height": 1053},
                            "nodes": [], "floating_nodes": []
                        },
                        {
                            // A split container that inherits a pid must not be
                            // mistaken for a window.
                            "id": 8, "type": "con", "name": null, "pid": 4242,
                            "rect": {"x": 0, "y": 0, "width": 960, "height": 1080},
                            "nodes": [{
                                "id": 9, "type": "con", "name": "xterm", "pid": 5150,
                                "focused": false, "visible": true,
                                "window_properties": {"class": "XTerm"},
                                "rect": {"x": 960, "y": 27, "width": 960, "height": 1053},
                                "nodes": [], "floating_nodes": []
                            }],
                            "floating_nodes": []
                        }
                    ],
                    "floating_nodes": []
                }],
                "floating_nodes": []
            }],
            "floating_nodes": []
        })
    }

    #[test]
    fn parses_only_leaf_windows() {
        let windows = parse_tree(&sample_tree());
        let ids: Vec<u64> = windows.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![7, 9], "container 8 is not a window");
    }

    #[test]
    fn carries_title_owner_pid_and_geometry() {
        let windows = parse_tree(&sample_tree());
        let fx = &windows[0];
        assert_eq!(fx.title, "Firefox — Wikipedia");
        assert_eq!(fx.owner, "firefox");
        assert_eq!(fx.pid, 4242);
        let b = fx.bounds.expect("geometry must survive");
        assert!((b.x - 0.0).abs() < f64::EPSILON);
        assert!((b.y - 27.0).abs() < f64::EPSILON);
        assert!((b.w - 1920.0).abs() < f64::EPSILON);
        assert_eq!(fx.on_screen, Some(true));
        // Not reported by sway — must stay unknown rather than become 0.
        assert_eq!(fx.layer, None);
    }

    #[test]
    fn xwayland_windows_fall_back_to_the_x11_class() {
        let windows = parse_tree(&sample_tree());
        let xterm = windows.iter().find(|w| w.id == 9).unwrap();
        assert_eq!(xterm.owner, "XTerm");
    }

    #[test]
    fn finds_the_focused_window() {
        assert_eq!(parse_focused(&sample_tree()), Some(7));
    }

    #[test]
    fn no_focus_is_none_not_an_error() {
        let mut tree = sample_tree();
        tree["nodes"][0]["nodes"][0]["nodes"][0]["focused"] = json!(false);
        assert_eq!(parse_focused(&tree), None);
    }

    #[test]
    fn empty_tree_yields_no_windows() {
        assert!(parse_tree(&json!({"id": 1, "type": "root"})).is_empty());
    }

    #[test]
    fn criteria_and_command_are_a_single_argument() {
        // Two arguments would make sway parse "[con_id=7]" as the whole command.
        assert_eq!(focus_args(7), vec!["[con_id=7] focus"]);
        assert_eq!(close_args(7), vec!["[con_id=7] kill"]);
        assert_eq!(
            move_args(7, 100, -50),
            vec!["[con_id=7] move absolute position 100 -50"]
        );
        assert_eq!(
            resize_args(7, 800, 600),
            vec!["[con_id=7] resize set width 800 px height 600 px"]
        );
    }

    #[test]
    fn a_refused_command_is_an_error_not_a_silent_no_op() {
        // sway exits 0 here; only the JSON says it refused.
        let err = check_reply(r#"[{"success": false, "error": "Cannot move tiled container"}]"#)
            .unwrap_err();
        assert!(err.to_string().contains("Cannot move tiled"), "{err}");
    }

    #[test]
    fn a_successful_command_passes() {
        check_reply(r#"[{"success": true}]"#).unwrap();
    }

    #[test]
    fn an_empty_reply_means_the_criteria_matched_nothing() {
        let err = check_reply("[]").unwrap_err();
        assert!(err.to_string().contains("stale"), "{err}");
    }

    #[test]
    fn unparseable_reply_is_an_error() {
        assert!(check_reply("not json").is_err());
    }
}
