use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A rectangular region on screen (pixels).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Position and size of a canvas overlay window (pixels).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<ScreenRegion> for CanvasPosition {
    fn from(r: ScreenRegion) -> Self {
        CanvasPosition { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

impl From<CanvasPosition> for ScreenRegion {
    fn from(p: CanvasPosition) -> Self {
        ScreenRegion { x: p.x, y: p.y, width: p.width, height: p.height }
    }
}

/// Element reference ID (e.g. "e1", "e12").
pub type RefId = String;

/// A resolved UI element from a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedElement {
    pub ref_id: RefId,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub frame: ScreenRegion,
}

/// Statistics about a UI snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotStats {
    pub total_elements: u32,
    pub interactive: u32,
    pub max_depth: u32,
}

/// Desktop Bridge request variants.
///
/// Wire serialization is performed manually in `client::request_to_jsonrpc()`.
/// These types are NOT serialized via serde directly — they exist for type-safe
/// request construction on the Rust side.
#[derive(Debug, Clone)]
pub enum DesktopRequest {
    // Perception (existing)
    Screenshot { region: Option<ScreenRegion> },
    Ocr { image_base64: Option<String> },
    AxTree { app_bundle_id: Option<String> },

    // Perception (new)
    Snapshot {
        app_bundle_id: Option<String>,
        max_depth: Option<u32>,
        include_non_interactive: Option<bool>,
    },

    // Action (existing — upgraded with ref support)
    Click {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        button: MouseButton,
    },
    TypeText {
        ref_id: Option<String>,
        text: String,
    },
    KeyCombo { keys: Vec<String> },
    LaunchApp { bundle_id: String },
    WindowList,
    FocusWindow { window_id: u32 },

    // Action (new)
    Scroll {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        delta_x: f64,
        delta_y: f64,
    },
    DoubleClick {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        button: MouseButton,
    },
    Drag {
        start_ref: Option<String>,
        start_x: Option<f64>,
        start_y: Option<f64>,
        end_ref: Option<String>,
        end_x: Option<f64>,
        end_y: Option<f64>,
        duration_ms: Option<u64>,
    },
    Hover {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
    },
    Paste { text: String },

    // Canvas (unchanged)
    CanvasShow { html: String, position: CanvasPosition },
    CanvasHide,
    CanvasUpdate { patch: serde_json::Value },

    // Internal
    Ping,

    // ========= PIM (Personal Information Management) =========

    // Calendar
    PimCalendarList { from: String, to: String, calendar_id: Option<String> },
    PimCalendarGet { id: String },
    PimCalendarCreate {
        title: String, start: String, end: String,
        calendar_id: Option<String>, location: Option<String>,
        notes: Option<String>, all_day: Option<bool>,
    },
    PimCalendarUpdate {
        id: String, title: Option<String>, start: Option<String>,
        end: Option<String>, location: Option<String>, notes: Option<String>,
    },
    PimCalendarDelete { id: String },
    PimCalendarCalendars,

    // Reminders
    PimRemindersList { list_id: Option<String>, include_completed: Option<bool> },
    PimRemindersGet { id: String },
    PimRemindersCreate {
        title: String, list_id: Option<String>,
        due_date: Option<String>, priority: Option<i32>, notes: Option<String>,
    },
    PimRemindersComplete { id: String, completed: bool },
    PimRemindersDelete { id: String },
    PimRemindersLists,

    // Notes
    PimNotesList { folder: Option<String> },
    PimNotesGet { id: String },
    PimNotesCreate { title: String, body: Option<String>, folder: Option<String> },
    PimNotesUpdate { id: String, title: Option<String>, body: Option<String> },
    PimNotesDelete { id: String },
    PimNotesFolders,

    // Contacts
    PimContactsSearch { query: String },
    PimContactsGet { id: String },
    PimContactsCreate {
        given_name: String, family_name: Option<String>,
        organization: Option<String>, notes: Option<String>,
        phone_numbers: Option<Vec<String>>, emails: Option<Vec<String>>,
    },
    PimContactsUpdate {
        id: String, given_name: Option<String>, family_name: Option<String>,
        organization: Option<String>, notes: Option<String>,
        phone_numbers: Option<Vec<String>>, emails: Option<Vec<String>>,
    },
    PimContactsDelete { id: String },
    PimContactsGroups,
}

/// Desktop Bridge response (parsed manually in client, not via serde).
#[derive(Debug, Clone)]
pub enum DesktopResponse {
    Success(serde_json::Value),
    Error { code: i32, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopRpcError {
    pub code: i32,
    pub message: String,
}
