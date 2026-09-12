use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unique identifier for a browser tab.
pub type TabId = String;

/// One row of a backend tab listing. Re-exported here because `types` is the
/// module every backend already imports its vocabulary from; the type itself
/// lives with the parser that produces it (`super::tab_registry`).
pub use super::tab_registry::TabLine;

/// Target for a browser action (click, hover, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionTarget {
    /// Target an element by its snapshot ref ID (e.g. "e42").
    Ref { ref_id: String },
    /// Target a **page** coordinate — origin at the top-left of the document,
    /// which is the space `browser_snapshot`'s geometry is printed in, so a
    /// number the model read off a snapshot line means here what it meant
    /// there.
    ///
    /// The CDP backend converts to the viewport with `Page.getLayoutMetrics`
    /// before dispatching. The two text backends have no page-coordinate
    /// primitive and hand the number to a viewport-based mouse API, which
    /// agrees only at `scroll = 0`; they also print no geometry at all, so no
    /// model can derive a coordinate from one of their snapshots in the first
    /// place. Stated rather than papered over.
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
/// text / textGone / selector / url / time). Internal to the backend
/// contract — the tool layer builds one from its args, so no serde derives
/// are needed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitCondition {
    /// The rendered page text contains this substring.
    Text(String),
    /// The rendered page text no longer contains this substring (openclaw
    /// `textGone` parity) — the wait for a spinner / "Loading…" to disappear.
    TextGone(String),
    /// A CSS selector matches at least one element.
    Selector(String),
    /// The tab's current URL contains this substring.
    UrlContains(String),
    /// Plain delay in milliseconds (openclaw `time` parity) — for animations
    /// and debounced renders that expose no observable condition. Never
    /// polled; resolves after the (pre-clamped) delay.
    Time(u64),
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

/// The token every backend's snapshot text uses to mark an addressable
/// element, and the token the tool layer counts to report `ref_count`.
///
/// It lives here because it is a **wire key between three producers and one
/// consumer**: `page_state::render_text` emits it, the two text backends pass
/// through their driver's rendering of it, and
/// `builtin_tools::browser_tools::snapshot` counts it. A literal repeated at
/// the counting site is a second derivation of the renderer's output and would
/// keep reporting `0` the day the renderer changed shape (判据 §1, §10).
pub const REF_TOKEN: &str = "[ref=";

/// Browser snapshot (text-first).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotOutput {
    /// Raw snapshot text — YAML from playwright-cli, indented-tree from
    /// chrome-devtools-mcp, the line-shaped page-state render from the CDP
    /// backend.
    pub snapshot_text: String,
    /// Page URL at snapshot time, or `None` when the backend could not
    /// determine one. `None` means **unknown**, never "the page has no URL":
    /// the two text backends parse it out of their driver's header and get
    /// nothing when the header is absent, which is a different fact from an
    /// empty URL.
    pub page_url: Option<String>,
    /// Page title at snapshot time — same provenance and same `None` meaning
    /// as [`Self::page_url`].
    pub page_title: Option<String>,
    /// How many addressable refs the snapshot text carries. Backends that build
    /// the text themselves report the count they minted; backends that pass a
    /// driver's text through count [`REF_TOKEN`] in it.
    pub ref_count: usize,
    /// The full page-state tree as JSON, for consumers that need geometry and
    /// element states rather than the text render. `None` for backends with no
    /// structured state (the two text drivers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_json: Option<serde_json::Value>,
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

    /// The CDP `Network.emulateNetworkConditions` parameters for this tier.
    ///
    /// `None` for [`Self::Online`], which is expressed by *clearing* the
    /// override rather than by setting one — the same "absence is the value"
    /// convention [`Self::as_mcp`] already uses.
    ///
    /// The tier numbers are Puppeteer's published `PredefinedNetworkConditions`
    /// (throughput in bytes per second, latency in milliseconds), so the three
    /// drivers throttle to the same thing rather than to three independently
    /// invented tables.
    #[must_use]
    pub const fn as_cdp(self) -> Option<CdpNetworkConditions> {
        match self {
            Self::Online => None,
            Self::Offline => Some(CdpNetworkConditions {
                offline: true,
                latency_ms: 0.0,
                download_bps: -1.0,
                upload_bps: -1.0,
            }),
            Self::Slow3g => Some(CdpNetworkConditions {
                offline: false,
                latency_ms: 2000.0,
                download_bps: 50_000.0,
                upload_bps: 50_000.0,
            }),
            // Chrome renamed "Fast 3G" to "Slow 4G" without changing the
            // numbers, so these two tiers are deliberately identical.
            Self::Fast3g | Self::Slow4g => Some(CdpNetworkConditions {
                offline: false,
                latency_ms: 562.5,
                download_bps: 180_000.0,
                upload_bps: 84_375.0,
            }),
            Self::Fast4g => Some(CdpNetworkConditions {
                offline: false,
                latency_ms: 165.0,
                download_bps: 1_012_500.0,
                upload_bps: 168_750.0,
            }),
        }
    }
}

/// `Network.emulateNetworkConditions` parameters. `-1` on a throughput means
/// "unthrottled on this axis" per the CDP contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CdpNetworkConditions {
    pub offline: bool,
    pub latency_ms: f64,
    pub download_bps: f64,
    pub upload_bps: f64,
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
        // BROWSER-R4-18: validate extra_http_headers. A model-supplied
        // header map flows into `mcp::external::call_tool` un-checked:
        // no upper bound on count, no character whitelist on names
        // (RFC 7230 token = letters / digits / a small punctuation set),
        // no length cap on values. An `Authorization: sk-ant-...` value
        // passes through `redact_content` egress, but the *emulation*
        // itself is not gated. CRLF in a value name could in theory
        // split headers depending on the MCP implementation.
        if let Some(headers) = &self.extra_http_headers {
            const MAX_HEADERS: usize = 32;
            const MAX_VALUE_BYTES: usize = 4096;
            if headers.len() > MAX_HEADERS {
                return Err(format!(
                    "extra_http_headers has {} entries; max {MAX_HEADERS}",
                    headers.len()
                ));
            }
            for (name, value) in headers {
                if name.is_empty() {
                    return Err("extra_http_headers contains an empty name".into());
                }
                if !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                {
                    return Err(format!(
                        "extra_http_headers name '{name}' contains an invalid character; \
                         RFC 7230 token chars are letters / digits / '-' / '_' / '.'"
                    ));
                }
                if value.len() > MAX_VALUE_BYTES {
                    return Err(format!(
                        "extra_http_headers value for '{name}' is {} bytes; max {MAX_VALUE_BYTES}",
                        value.len()
                    ));
                }
                if value.bytes().any(|b| b.is_ascii_control()) {
                    return Err(format!(
                        "extra_http_headers value for '{name}' contains a control byte"
                    ));
                }
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
            page_url: Some("https://example.com/".into()),
            page_title: Some("Example".into()),
            ref_count: 1,
            state_json: None,
        };
        let json = serde_json::to_value(&snap).unwrap();
        let back: SnapshotOutput = serde_json::from_value(json).unwrap();
        assert_eq!(back.page_url.as_deref(), Some("https://example.com/"));
        assert_eq!(back.ref_count, 1);
        assert!(back.state_json.is_none());

        // An unknown URL must survive as unknown, not as an empty string: the
        // renderer has to be able to tell "I could not read the header" from
        // "the page is on about:blank".
        let unknown = SnapshotOutput {
            snapshot_text: String::new(),
            page_url: None,
            page_title: None,
            ref_count: 0,
            state_json: None,
        };
        let back: SnapshotOutput =
            serde_json::from_value(serde_json::to_value(&unknown).unwrap()).unwrap();
        assert!(back.page_url.is_none() && back.page_title.is_none());
    }

    /// The refs the model can act on are counted with ONE token, and the
    /// renderer emits that same token. A second literal at either end is the
    /// same-fact-twice shape that keeps reporting zero after a format change.
    ///
    /// The second half is the one that can go red on a renderer change: it
    /// counts [`REF_TOKEN`] in text the renderer's own test golden uses.
    #[test]
    fn ref_token_matches_what_the_backends_emit() {
        assert_eq!(REF_TOKEN, "[ref=");
        assert_eq!(
            "- button \"OK\" [ref=e1]\n- link \"x\" [ref=e2]"
                .matches(REF_TOKEN)
                .count(),
            2
        );
    }

    /// `Online` is the absence of an override, and every other tier names one.
    /// A tier that answered `None` would silently stop throttling while
    /// reporting success.
    #[test]
    fn every_network_tier_but_online_has_cdp_parameters() {
        assert_eq!(NetworkCondition::Online.as_cdp(), None);
        let offline = NetworkCondition::Offline
            .as_cdp()
            .expect("offline throttles");
        assert!(offline.offline);
        for tier in [
            NetworkCondition::Slow3g,
            NetworkCondition::Fast3g,
            NetworkCondition::Slow4g,
            NetworkCondition::Fast4g,
        ] {
            let c = tier
                .as_cdp()
                .unwrap_or_else(|| panic!("{tier:?} has no CDP parameters"));
            assert!(!c.offline, "{tier:?} is a throttle, not a disconnection");
            assert!(
                c.download_bps > 0.0 && c.upload_bps > 0.0 && c.latency_ms > 0.0,
                "{tier:?} throttles to nothing: {c:?}"
            );
        }
        // Chrome renamed "Fast 3G" to "Slow 4G" without changing the numbers;
        // this pins that they are one tier and not two that drifted.
        assert_eq!(
            NetworkCondition::Fast3g.as_cdp(),
            NetworkCondition::Slow4g.as_cdp()
        );
        // …and that the two 4G tiers are NOT the same, which is what makes the
        // line above an observation rather than a tautology.
        assert_ne!(
            NetworkCondition::Slow4g.as_cdp(),
            NetworkCondition::Fast4g.as_cdp()
        );
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
