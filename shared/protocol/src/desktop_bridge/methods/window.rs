use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::screen::Region;

pub const METHOD_LIST: &str = "window.list";
pub const METHOD_FOCUS: &str = "window.focus";
pub const METHOD_LAUNCH_APP: &str = "window.launch_app";
pub const METHOD_QUIT_APP: &str = "window.quit_app";
pub const SUGGESTED_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListParams {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListResult {
    pub windows: Vec<WindowInfo>,
}

/// A window as reported by `window.list`.
///
/// The three trailing fields are `Option` on purpose: `None` means *"the
/// platform query did not tell us"*, which is not the same as "zero" or
/// "false". A helper that predates them omits them from the wire, so every
/// consumer must treat unknown as unknown.
///
/// `Default` exists so a limb can spell only the fields its platform query
/// actually yields via `..Default::default()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub owner: String,
    pub pid: u64,
    /// Window frame in the GLOBAL screen POINT space, top-left origin — the
    /// same space clicks are issued in. Required to crop a screenshot to this
    /// window and map its pixels back to click coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Region>,
    /// Window level. `0` is a normal application window; menu extras, docks and
    /// overlays sit above it. Lets a caller mechanically skip chrome without
    /// guessing from the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<i32>,
    /// Whether the window is currently on screen (not minimized / not on
    /// another Space).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_screen: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FocusParams {
    pub window_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FocusResult {
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LaunchAppParams {
    pub app_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LaunchAppResult {
    pub launched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuitAppParams {
    pub app_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuitAppResult {
    pub quit: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_result_roundtrip() {
        let r = ListResult {
            windows: vec![WindowInfo {
                id: 42,
                title: "Foo".into(),
                owner: "Bar".into(),
                pid: 100,
                ..Default::default()
            }],
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: ListResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.windows.len(), 1);
        assert_eq!(back.windows[0].id, 42);
        assert_eq!(back.windows[0].title, "Foo");
        assert_eq!(back.windows[0].owner, "Bar");
        assert_eq!(back.windows[0].pid, 100);
        // Not told ≠ zero: a limb that cannot report geometry says nothing.
        assert!(back.windows[0].bounds.is_none());
        assert!(back.windows[0].layer.is_none());
        assert!(back.windows[0].on_screen.is_none());
    }

    #[test]
    fn window_info_carries_bounds_layer_on_screen() {
        let j = r#"{"id":1,"title":"T","owner":"O","pid":9,
                    "bounds":{"x":10.0,"y":20.0,"width":800.0,"height":600.0},
                    "layer":0,"on_screen":true}"#;
        let w: WindowInfo = serde_json::from_str(j).unwrap();
        let b = w.bounds.expect("bounds");
        assert_eq!(b.x, 10.0);
        assert_eq!(b.height, 600.0);
        assert_eq!(w.layer, Some(0));
        assert_eq!(w.on_screen, Some(true));
    }

    #[test]
    fn focus_params_roundtrip() {
        let p = FocusParams { window_id: 42 };
        let j = serde_json::to_string(&p).unwrap();
        let back: FocusParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.window_id, 42);
    }
}
