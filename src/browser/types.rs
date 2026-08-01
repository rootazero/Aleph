use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unique identifier for a browser tab.
pub type TabId = String;

/// Target for a browser action (click, hover, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionTarget {
    /// Target an element by its snapshot ref ID (e.g. "e42").
    Ref { ref_id: String },
    /// Target a viewport coordinate.
    Coordinates { x: f64, y: f64 },
}

/// Direction for scrolling.
/// Lateral/vertical scroll step in CSS pixels, used when a backend has no
/// page-key primitive and must fall back to a wheel/`scrollBy` delta.
/// Single source of truth shared by both backends.
pub const SCROLL_STEP_PX: i32 = 400;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    /// `(dx, dy)` wheel delta in CSS pixels for this direction, using
    /// [`SCROLL_STEP_PX`] as the step. Down/Right are positive, Up/Left
    /// negative — matching wheel/`scrollBy` axis conventions.
    pub const fn wheel_delta(self) -> (i32, i32) {
        match self {
            ScrollDirection::Up => (0, -SCROLL_STEP_PX),
            ScrollDirection::Down => (0, SCROLL_STEP_PX),
            ScrollDirection::Left => (-SCROLL_STEP_PX, 0),
            ScrollDirection::Right => (SCROLL_STEP_PX, 0),
        }
    }
}

/// History-style navigation on the current tab (back / forward / refresh).
/// Internal to the backend contract — the tool layer maps its own serde enum
/// onto this, so no serde derives are needed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryNav {
    Back,
    Forward,
    Refresh,
}

/// Condition a `wait_for` call polls for on a tab (openclaw parity:
/// text / selector / url). Internal to the backend contract — the tool layer
/// builds one from its args, so no serde derives are needed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitCondition {
    /// The rendered page text contains this substring.
    Text(String),
    /// A CSS selector matches at least one element.
    Selector(String),
    /// The tab's current URL contains this substring.
    UrlContains(String),
}

/// Options for taking a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotOpts {
    /// Capture the full scrollable page instead of just the viewport.
    #[serde(default)]
    pub full_page: bool,

    /// Image format ("png" or "jpeg"). Consumed only by the Playwright
    /// backend as a file-extension hint; the Chrome MCP backend always writes
    /// PNG. Not exposed to the LLM via `browser_screenshot` Args.
    #[serde(default = "default_screenshot_format")]
    pub format: String,
}

fn default_screenshot_format() -> String {
    "png".to_string()
}

impl Default for ScreenshotOpts {
    fn default() -> Self {
        Self {
            full_page: false,
            format: default_screenshot_format(),
        }
    }
}

/// Browser snapshot (text-first).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotOutput {
    /// Raw snapshot text — YAML from playwright-cli, indented-tree from chrome-devtools-mcp.
    pub snapshot_text: String,
    /// Page URL at snapshot time.
    pub page_url: String,
    /// Page title at snapshot time.
    pub page_title: String,
}

/// Screenshot output (raw PNG bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotOutput {
    pub png_bytes: Vec<u8>,
}

/// Emulated color scheme (CSS `prefers-color-scheme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    Dark,
    Light,
    /// Reset to the system default.
    Auto,
}

impl ColorScheme {
    /// Value accepted by chrome-devtools-mcp's `emulate.colorScheme`.
    #[must_use]
    pub const fn as_mcp(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Auto => "auto",
        }
    }
}

/// Emulated network condition.
///
/// `Offline` / `Online` are supported by both backends; the throttled tiers are
/// chrome-devtools-mcp only (the managed Playwright CLI only toggles offline/online).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCondition {
    Offline,
    Online,
    Slow3g,
    Fast3g,
    Slow4g,
    Fast4g,
}

impl NetworkCondition {
    /// Value for chrome-devtools-mcp's `emulate.networkConditions`, or `None`
    /// when the condition is expressed by *omitting* the field (`Online` =
    /// no throttling per the MCP contract).
    #[must_use]
    pub const fn as_mcp(self) -> Option<&'static str> {
        match self {
            Self::Offline => Some("Offline"),
            Self::Slow3g => Some("Slow 3G"),
            Self::Fast3g => Some("Fast 3G"),
            Self::Slow4g => Some("Slow 4G"),
            Self::Fast4g => Some("Fast 4G"),
            Self::Online => None,
        }
    }

    /// `playwright-cli network-state-set` argument for the conditions the
    /// managed backend can express natively; `None` for throttled tiers it cannot.
    #[must_use]
    pub const fn as_playwright_state(self) -> Option<&'static str> {
        match self {
            Self::Offline => Some("offline"),
            Self::Online => Some("online"),
            _ => None,
        }
    }
}

/// A geographic coordinate to emulate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Geolocation {
    /// Latitude, between -90 and 90.
    pub latitude: f64,
    /// Longitude, between -180 and 180.
    pub longitude: f64,
}

/// Environment/device emulation overrides applied to a tab.
///
/// Every field is optional; only the set fields are applied. This collapses
/// what other tools spread across many setters (geolocation, color scheme,
/// network throttling, CPU throttling, HTTP headers, user-agent) into one
/// type-checked request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EmulateOptions {
    /// Emulate dark / light / auto color scheme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<ColorScheme>,
    /// Override the geolocation reported to the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geolocation: Option<Geolocation>,
    /// Throttle (or disable) the network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_condition: Option<NetworkCondition>,
    /// CPU slowdown factor (1.0 = none, up to 20.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_throttle: Option<f64>,
    /// Extra HTTP headers added to every request from the page (e.g. an
    /// `Authorization` bearer token). An empty map clears previously-set headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_http_headers: Option<std::collections::BTreeMap<String, String>>,
    /// Override the `User-Agent` string. Empty string clears the override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

impl EmulateOptions {
    /// Whether no override at all was requested.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.color_scheme.is_none()
            && self.geolocation.is_none()
            && self.network_condition.is_none()
            && self.cpu_throttle.is_none()
            && self.extra_http_headers.is_none()
            && self.user_agent.is_none()
    }

    /// Validate field ranges at the system boundary. Returns a human-readable
    /// reason string on the first violation.
    pub fn validate(&self) -> Result<(), String> {
        if self.is_empty() {
            return Err("emulate requires at least one option to apply".into());
        }
        if let Some(geo) = &self.geolocation {
            if !(-90.0..=90.0).contains(&geo.latitude) {
                return Err(format!("latitude {} out of range [-90, 90]", geo.latitude));
            }
            if !(-180.0..=180.0).contains(&geo.longitude) {
                return Err(format!(
                    "longitude {} out of range [-180, 180]",
                    geo.longitude
                ));
            }
        }
        if let Some(rate) = self.cpu_throttle {
            if !(1.0..=20.0).contains(&rate) {
                return Err(format!("cpu_throttle {rate} out of range [1, 20]"));
            }
        }
        Ok(())
    }
}

/// `SameSite` attribute applied when setting a cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    /// Canonical value accepted by `playwright-cli cookie-set --sameSite`.
    #[must_use]
    pub const fn as_cli(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

/// A cookie-management operation against the live managed browser session.
///
/// Modeled directly on the `playwright-cli cookie-*` verbs. This is a
/// backend-internal contract constructed by the tool layer from validated
/// arguments, so it carries no serde derive — only the leaf [`SameSite`] is
/// part of any tool's JSON schema.
#[derive(Debug, Clone)]
pub enum CookieOp {
    /// List all cookies, optionally filtered by domain and/or path.
    List {
        domain: Option<String>,
        path: Option<String>,
    },
    /// Get a single cookie by name.
    Get { name: String },
    /// Set a cookie with optional attributes.
    Set {
        name: String,
        value: String,
        domain: Option<String>,
        path: Option<String>,
        /// Expiration as a unix timestamp (seconds).
        expires: Option<i64>,
        http_only: Option<bool>,
        secure: Option<bool>,
        same_site: Option<SameSite>,
    },
    /// Delete a single cookie by name.
    Delete { name: String },
    /// Clear all cookies.
    Clear,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_target_serialization() {
        // Ref variant
        let target = ActionTarget::Ref {
            ref_id: "e42".to_string(),
        };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "ref");
        assert_eq!(json["ref_id"], "e42");

        // Coordinates variant
        let target = ActionTarget::Coordinates { x: 100.0, y: 200.0 };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "coordinates");
        assert_eq!(json["x"], 100.0);
        assert_eq!(json["y"], 200.0);

        // Round-trip deserialization
        let round_trip: ActionTarget = serde_json::from_value(json).unwrap();
        assert!(
            matches!(round_trip, ActionTarget::Coordinates { x, y } if x == 100.0 && y == 200.0)
        );
    }

    #[test]
    fn test_snapshot_output_serde_roundtrip() {
        let snap = SnapshotOutput {
            snapshot_text: "- button \"OK\" [ref=e1]".into(),
            page_url: "https://example.com/".into(),
            page_title: "Example".into(),
        };
        let json = serde_json::to_value(&snap).unwrap();
        let back: SnapshotOutput = serde_json::from_value(json).unwrap();
        assert_eq!(back.page_url, "https://example.com/");
    }

    #[test]
    fn test_action_target_no_selector_variant() {
        let json = serde_json::json!({"type": "selector", "css": ".foo"});
        let parsed: Result<ActionTarget, _> = serde_json::from_value(json);
        assert!(parsed.is_err());
    }

    #[test]
    fn test_same_site_cli_values() {
        assert_eq!(SameSite::Strict.as_cli(), "Strict");
        assert_eq!(SameSite::Lax.as_cli(), "Lax");
        assert_eq!(SameSite::None.as_cli(), "None");
    }

    #[test]
    fn test_color_scheme_mcp_values() {
        assert_eq!(ColorScheme::Dark.as_mcp(), "dark");
        assert_eq!(ColorScheme::Light.as_mcp(), "light");
        assert_eq!(ColorScheme::Auto.as_mcp(), "auto");
    }

    #[test]
    fn test_network_condition_backend_mappings() {
        // MCP: Online omits the field; tiers map to spaced labels.
        assert_eq!(NetworkCondition::Offline.as_mcp(), Some("Offline"));
        assert_eq!(NetworkCondition::Slow3g.as_mcp(), Some("Slow 3G"));
        assert_eq!(NetworkCondition::Fast4g.as_mcp(), Some("Fast 4G"));
        assert_eq!(NetworkCondition::Online.as_mcp(), None);
        // Playwright: only offline/online expressible; tiers are None.
        assert_eq!(
            NetworkCondition::Offline.as_playwright_state(),
            Some("offline")
        );
        assert_eq!(
            NetworkCondition::Online.as_playwright_state(),
            Some("online")
        );
        assert_eq!(NetworkCondition::Slow3g.as_playwright_state(), None);
    }

    #[test]
    fn test_emulate_options_validate() {
        // Empty → rejected.
        assert!(EmulateOptions::default().validate().is_err());

        // Valid single field.
        let ok = EmulateOptions {
            color_scheme: Some(ColorScheme::Dark),
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
        assert!(!ok.is_empty());

        // Out-of-range latitude / longitude / cpu.
        let bad_lat = EmulateOptions {
            geolocation: Some(Geolocation {
                latitude: 200.0,
                longitude: 0.0,
            }),
            ..Default::default()
        };
        assert!(bad_lat.validate().is_err());

        let bad_cpu = EmulateOptions {
            cpu_throttle: Some(0.5),
            ..Default::default()
        };
        assert!(bad_cpu.validate().is_err());
    }
}
