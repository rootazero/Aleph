pub mod actions;
pub mod discovery;
pub mod error;
pub mod manager;
pub mod network_policy;
pub mod profile;
pub mod runtime;
pub mod snapshot;
pub mod types;

pub use discovery::find_chromium;
pub use error::BrowserError;
pub use runtime::BrowserRuntime;
pub use snapshot::{resolve_ref_to_point, take_aria_snapshot};
pub use types::{
    ActionTarget, AriaElement, AriaSnapshot, BrowserConfig, ElementRect, LaunchMode,
    ScreenshotOpts, ScreenshotResult, ScrollDirection, StorageKind, TabId, TabInfo,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert_eq!(config.cdp_port, 9222);
        assert!(!config.headless);
        assert!(config.extra_args.is_empty());
        assert!(config.user_data_dir.is_none());
        assert!(matches!(config.mode, LaunchMode::Auto));
    }

    #[test]
    fn test_action_target_serialization() {
        // Ref variant
        let target = ActionTarget::Ref { ref_id: "e42".to_string() };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "ref");
        assert_eq!(json["ref_id"], "e42");

        // Selector variant
        let target = ActionTarget::Selector { css: "button.submit".to_string() };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "selector");
        assert_eq!(json["css"], "button.submit");

        // Coordinates variant
        let target = ActionTarget::Coordinates { x: 100.0, y: 200.0 };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "coordinates");
        assert_eq!(json["x"], 100.0);
        assert_eq!(json["y"], 200.0);

        // Round-trip deserialization
        let round_trip: ActionTarget = serde_json::from_value(json).unwrap();
        assert!(matches!(round_trip, ActionTarget::Coordinates { x, y } if x == 100.0 && y == 200.0));
    }

    #[test]
    fn test_aria_element_serialization() {
        let element = AriaElement {
            ref_id: "e1".to_string(),
            role: "button".to_string(),
            name: Some("Submit".to_string()),
            value: None,
            state: vec!["focused".to_string()],
            bounds: Some(ElementRect { x: 10.0, y: 20.0, width: 100.0, height: 40.0 }),
            children: vec![],
        };

        let json = serde_json::to_value(&element).unwrap();
        assert_eq!(json["ref_id"], "e1");
        assert_eq!(json["role"], "button");
        assert_eq!(json["name"], "Submit");
        assert!(json["value"].is_null());
        assert_eq!(json["state"][0], "focused");
        assert_eq!(json["bounds"]["x"], 10.0);
        assert!(json["children"].as_array().unwrap().is_empty());

        // Round-trip
        let round_trip: AriaElement = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.ref_id, "e1");
        assert_eq!(round_trip.role, "button");
        assert_eq!(round_trip.name.unwrap(), "Submit");
        assert!(round_trip.value.is_none());
        assert_eq!(round_trip.state, vec!["focused"]);
        assert!(round_trip.bounds.is_some());
    }

    #[test]
    fn test_launch_mode_tagged_enum_serialization() {
        // Auto
        let mode = LaunchMode::Auto;
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "auto");

        // Connect
        let mode = LaunchMode::Connect { endpoint: "ws://127.0.0.1:9222".to_string() };
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "connect");
        assert_eq!(json["endpoint"], "ws://127.0.0.1:9222");

        // Binary
        let mode = LaunchMode::Binary { path: "/usr/bin/chromium".to_string() };
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "binary");
        assert_eq!(json["path"], "/usr/bin/chromium");

        // Round-trip deserialization
        let round_trip: LaunchMode = serde_json::from_value(json).unwrap();
        assert!(matches!(round_trip, LaunchMode::Binary { path } if path == "/usr/bin/chromium"));
    }
}
