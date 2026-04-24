use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub owner: String,
    pub pid: u64,
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
            }],
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: ListResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.windows.len(), 1);
        assert_eq!(back.windows[0].id, 42);
        assert_eq!(back.windows[0].title, "Foo");
        assert_eq!(back.windows[0].owner, "Bar");
        assert_eq!(back.windows[0].pid, 100);
    }

    #[test]
    fn focus_params_roundtrip() {
        let p = FocusParams { window_id: 42 };
        let j = serde_json::to_string(&p).unwrap();
        let back: FocusParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.window_id, 42);
    }
}
