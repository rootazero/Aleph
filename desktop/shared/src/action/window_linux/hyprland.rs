//! Hyprland window management over `hyprctl`'s JSON interface.
//!
//! `hyprctl` ships with Hyprland, so "Hyprland is running" implies it is
//! available. As with the sway backend, argv building and reply parsing are
//! pure and unit-tested; only the executor touches the process boundary.
//!
//! Window identity is Hyprland's client `address`, a pointer-shaped hex string
//! (`0x55f8a1b2c3d4`). It parses cleanly into the cross-platform
//! `WindowInfo.id: u64` and formats back to the same text, so no handle table
//! is needed — the round-trip is exact.

use serde_json::Value;

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, WindowInfo};

/// Format a window id back into Hyprland's `address:0x…` selector.
///
/// Lower-case hex with an `0x` prefix, matching what `hyprctl clients` prints.
#[must_use]
pub fn address_of(window_id: u64) -> String {
    format!("0x{window_id:x}")
}

/// `hyprctl -j clients` — every mapped client as a flat JSON array.
#[must_use]
pub fn list_args() -> Vec<String> {
    vec!["-j".into(), "clients".into()]
}

/// `hyprctl -j activewindow`.
#[must_use]
pub fn active_args() -> Vec<String> {
    vec!["-j".into(), "activewindow".into()]
}

/// `hyprctl dispatch focuswindow address:0x…`.
#[must_use]
pub fn focus_args(window_id: u64) -> Vec<String> {
    vec![
        "dispatch".into(),
        "focuswindow".into(),
        format!("address:{}", address_of(window_id)),
    ]
}

/// `hyprctl dispatch movewindowpixel exact X Y,address:0x…`.
///
/// The dispatcher takes one comma-joined argument: the geometry and the window
/// selector. Splitting it into two would make Hyprland read `exact X Y` as the
/// whole argument and ignore the selector, moving whatever happens to be
/// focused.
#[must_use]
pub fn move_args(window_id: u64, x: i32, y: i32) -> Vec<String> {
    vec![
        "dispatch".into(),
        "movewindowpixel".into(),
        format!("exact {x} {y},address:{}", address_of(window_id)),
    ]
}

/// `hyprctl dispatch resizewindowpixel exact W H,address:0x…`.
#[must_use]
pub fn resize_args(window_id: u64, width: u32, height: u32) -> Vec<String> {
    vec![
        "dispatch".into(),
        "resizewindowpixel".into(),
        format!("exact {width} {height},address:{}", address_of(window_id)),
    ]
}

/// `hyprctl dispatch closewindow address:0x…` — a polite close request, not a
/// signal.
#[must_use]
pub fn close_args(window_id: u64) -> Vec<String> {
    vec![
        "dispatch".into(),
        "closewindow".into(),
        format!("address:{}", address_of(window_id)),
    ]
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// Parse a `0x…` address into a window id.
#[must_use]
pub fn parse_address(addr: &str) -> Option<u64> {
    let hex = addr
        .trim()
        .strip_prefix("0x")
        .or_else(|| addr.trim().strip_prefix("0X"))
        .unwrap_or_else(|| addr.trim());
    u64::from_str_radix(hex, 16).ok()
}

/// Parse `hyprctl -j clients` into window records.
#[must_use]
pub fn parse_clients(json: &Value) -> Vec<WindowInfo> {
    json.as_array()
        .map(|clients| clients.iter().filter_map(client_to_window).collect())
        .unwrap_or_default()
}

fn client_to_window(client: &Value) -> Option<WindowInfo> {
    let id = parse_address(client.get("address").and_then(Value::as_str)?)?;

    let at = client.get("at").and_then(Value::as_array);
    let size = client.get("size").and_then(Value::as_array);
    let bounds = match (at, size) {
        (Some(at), Some(size)) if at.len() >= 2 && size.len() >= 2 => Some(BoundingBox {
            x: at[0].as_f64()?,
            y: at[1].as_f64()?,
            w: size[0].as_f64()?,
            h: size[1].as_f64()?,
        }),
        _ => None,
    };

    // `mapped` and `hidden` are both reported; a window is on screen when it is
    // mapped and not hidden. If neither field is present, say nothing rather
    // than guess.
    let on_screen = match (
        client.get("mapped").and_then(Value::as_bool),
        client.get("hidden").and_then(Value::as_bool),
    ) {
        (None, None) => None,
        (mapped, hidden) => Some(mapped.unwrap_or(true) && !hidden.unwrap_or(false)),
    };

    Some(WindowInfo {
        id,
        title: client
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        owner: client
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pid: client
            .get("pid")
            .and_then(Value::as_i64)
            .and_then(|p| u64::try_from(p).ok())
            .unwrap_or(0),
        bounds,
        // Hyprland reports no stacking level.
        layer: None,
        on_screen,
    })
}

/// Parse `hyprctl -j activewindow` into a window id.
///
/// Hyprland answers with an empty object (or an all-zero address) when nothing
/// is focused, which must read as `None`, not as window 0.
#[must_use]
pub fn parse_active(json: &Value) -> Option<u64> {
    let id = parse_address(json.get("address").and_then(Value::as_str)?)?;
    (id != 0).then_some(id)
}

/// Turn a `hyprctl dispatch` reply into a `Result`.
///
/// `hyprctl` prints `ok` on success and an error sentence otherwise, while
/// exiting 0 either way — so the exit status alone would report a refused
/// dispatch as a success.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] quoting Hyprland's own message.
pub fn check_reply(stdout: &str) -> Result<()> {
    let trimmed = stdout.trim();
    if trimmed.eq_ignore_ascii_case("ok") {
        return Ok(());
    }
    Err(DesktopError::WindowFailed(format!(
        "hyprctl refused the command: {}",
        if trimmed.is_empty() {
            "no reply (the address is stale — re-run window_list)"
        } else {
            trimmed
        }
    )))
}

// ── Runtime ──────────────────────────────────────────────────────────────────

fn run(args: Vec<String>) -> Result<String> {
    let out = std::process::Command::new("hyprctl")
        .args(&args)
        .output()
        .map_err(|e| DesktopError::WindowFailed(format!("Failed to run hyprctl: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "hyprctl failed: {}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_json(args: Vec<String>) -> Result<Value> {
    let stdout = run(args)?;
    serde_json::from_str(&stdout)
        .map_err(|e| DesktopError::WindowFailed(format!("hyprctl returned bad JSON: {e}")))
}

/// List Hyprland's clients.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if `hyprctl` cannot run or returns bad JSON.
pub fn window_list() -> Result<Vec<WindowInfo>> {
    Ok(parse_clients(&run_json(list_args())?))
}

/// Focus a window by address.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if Hyprland refuses or the address is stale.
pub fn focus_window(window_id: u64) -> Result<()> {
    check_reply(&run(focus_args(window_id))?)
}

/// Move a window to an absolute pixel position.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if Hyprland refuses or the address is stale.
pub fn move_window(window_id: u64, x: i32, y: i32) -> Result<()> {
    check_reply(&run(move_args(window_id, x, y))?)
}

/// Resize a window in pixels.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if Hyprland refuses or the address is stale.
pub fn resize_window(window_id: u64, width: u32, height: u32) -> Result<()> {
    check_reply(&run(resize_args(window_id, width, height))?)
}

/// Ask a window to close.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if Hyprland refuses or the address is stale.
pub fn close_window(window_id: u64) -> Result<()> {
    check_reply(&run(close_args(window_id))?)
}

/// The focused window's address.
///
/// # Errors
///
/// [`DesktopError::WindowFailed`] if `hyprctl` cannot run or returns bad JSON.
pub fn active_window() -> Result<Option<u64>> {
    Ok(parse_active(&run_json(active_args())?))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn clients() -> Value {
        json!([
            {
                "address": "0x55f8a1b2c3d4",
                "mapped": true, "hidden": false,
                "at": [100, 40], "size": [1280, 720],
                "pid": 4242, "title": "Firefox — Wikipedia", "class": "firefox"
            },
            {
                "address": "0x55f8deadbeef",
                "mapped": true, "hidden": true,
                "at": [0, 0], "size": [800, 600],
                "pid": 5150, "title": "scratchpad", "class": "kitty"
            }
        ])
    }

    #[test]
    fn address_round_trips_exactly() {
        let addr = "0x55f8a1b2c3d4";
        let id = parse_address(addr).unwrap();
        assert_eq!(address_of(id), addr, "id must format back to its address");
    }

    #[test]
    fn address_parsing_tolerates_case_and_whitespace() {
        assert_eq!(parse_address("  0X1F  "), Some(31));
        assert_eq!(parse_address("1f"), Some(31));
        assert_eq!(parse_address("nonsense"), None);
    }

    #[test]
    fn parses_clients_with_geometry_and_visibility() {
        let windows = parse_clients(&clients());
        assert_eq!(windows.len(), 2);

        let fx = &windows[0];
        assert_eq!(fx.title, "Firefox — Wikipedia");
        assert_eq!(fx.owner, "firefox");
        assert_eq!(fx.pid, 4242);
        assert_eq!(fx.on_screen, Some(true));
        let b = fx.bounds.unwrap();
        assert!((b.x - 100.0).abs() < f64::EPSILON);
        assert!((b.h - 720.0).abs() < f64::EPSILON);

        // hidden: true must not read as on screen.
        assert_eq!(windows[1].on_screen, Some(false));
    }

    #[test]
    fn missing_visibility_fields_stay_unknown() {
        let json = json!([{"address": "0x1", "pid": 1, "title": "t", "class": "c"}]);
        assert_eq!(parse_clients(&json)[0].on_screen, None);
        assert!(parse_clients(&json)[0].bounds.is_none());
    }

    #[test]
    fn a_client_without_an_address_is_skipped_not_defaulted_to_zero() {
        let json = json!([{"pid": 1, "title": "no address"}]);
        assert!(parse_clients(&json).is_empty());
    }

    #[test]
    fn non_array_client_payload_yields_nothing() {
        assert!(parse_clients(&json!({"address": "0x1"})).is_empty());
    }

    #[test]
    fn active_window_is_none_when_nothing_is_focused() {
        assert_eq!(parse_active(&json!({})), None);
        assert_eq!(parse_active(&json!({"address": "0x0"})), None);
        assert_eq!(parse_active(&json!({"address": "0x2a"})), Some(42));
    }

    #[test]
    fn dispatch_geometry_and_selector_are_one_comma_joined_argument() {
        // Two arguments would drop the selector and move the focused window.
        assert_eq!(
            move_args(0x2a, 10, 20),
            vec!["dispatch", "movewindowpixel", "exact 10 20,address:0x2a"]
        );
        assert_eq!(
            resize_args(0x2a, 800, 600),
            vec!["dispatch", "resizewindowpixel", "exact 800 600,address:0x2a"]
        );
        assert_eq!(
            focus_args(0x2a),
            vec!["dispatch", "focuswindow", "address:0x2a"]
        );
        assert_eq!(
            close_args(0x2a),
            vec!["dispatch", "closewindow", "address:0x2a"]
        );
    }

    #[test]
    fn ok_is_success_anything_else_is_not() {
        check_reply("ok").unwrap();
        check_reply(" OK \n").unwrap();
        let err = check_reply("Invalid dispatcher").unwrap_err();
        assert!(err.to_string().contains("Invalid dispatcher"), "{err}");
    }

    #[test]
    fn an_empty_reply_points_at_a_stale_address() {
        let err = check_reply("").unwrap_err();
        assert!(err.to_string().contains("stale"), "{err}");
    }
}
