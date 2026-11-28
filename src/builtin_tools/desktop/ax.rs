//! Accessibility (AX) query tools — LLM-callable wrappers over
//! `DesktopPlatform::ax()` for the three read-only AX operations.
//!
//! Each tool is a separate [`AlephTool`] so the LLM sees a small, focused
//! surface (`desktop_ax_query_focused`, `desktop_ax_query_tree`,
//! `desktop_ax_query_by_role`) rather than a catch-all `desktop_ax` verb.
//!
//! All tools degrade gracefully when no `DesktopPlatform` is configured
//! (e.g. headless server builds) or when the platform does not implement
//! `AccessibilityCapability` — they return a structured
//! `DesktopOutput { success: false, message: "..." }` instead of erroring.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aleph_protocol::desktop_bridge::methods::ax::{
    clamp_max_nodes, AxElement, AxReferenceSnapshot, AxSnapshotRequest, QueryByRoleParams,
    QueryFocusedParams, QueryTreeParams, DEFAULT_MAX_NODES, MAX_AX_REFERENCE_ROWS,
};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use crate::utils::text_format::truncate_with_marker;

use super::interactable::{affordance_fields, redact_secure_values, safe_value, usable_bounds};
use super::types::DesktopOutput;

// ── Argument types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxQueryFocusedArgs {
    /// Ask this process which of *its own* elements has focus, instead of
    /// asking the system which element has focus overall.
    ///
    /// Use it whenever the app you care about is not the one in front of the
    /// user — which, on the targeted input rail, is the normal case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxQueryTreeArgs {
    /// Target process ID. Omit to query the frontmost application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Maximum subtree depth (default 6).  Larger values produce more JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum number of nodes to return (default 1500, ceiling 10000).
    ///
    /// Depth does not bound a wide tree. If the response comes back with
    /// `truncated: true`, narrow the query (a `pid`, a smaller `max_depth`, or
    /// `ax_query_by_role`) rather than simply asking for more nodes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxQueryByRoleArgs {
    /// AX role string to collect, e.g. `"AXButton"`, `"AXTextField"`.
    pub role: String,
    /// Target process ID. Omit to query the frontmost application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Node budget for the search (default 1500, ceiling 10000). Bounds how much
    /// of the tree is searched, not how many matches are returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<u32>,
}

// ── Shared helpers ───────────────────────────────────────────────────────────

fn no_platform_output() -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(
            "Desktop platform capability is not configured for this server build.".to_string(),
        ),
    }
}

fn no_ax_capability_output() -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some("Accessibility (AX) capability is not available on this platform.".into()),
    }
}

fn bridge_err_output(err: impl std::fmt::Display) -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(super::recovery::with_hint(format!(
            "AX query failed: {err}"
        ))),
    }
}

// ── desktop_ax_query_focused ────────────────────────────────────────────────

/// LLM tool: return the currently focused AX element.
#[derive(Clone, Default)]
pub struct DesktopAxQueryFocused {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryFocused {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxQueryFocused {
    // Platform availability is stated once for the whole AX family, on
    // `desktop_ax_snapshot`. Four copies used to ship in every request that
    // discloses the desktop family; `no_sentence_is_stated_twice` counts what
    // ships, not what is written. A call where no AX layer exists returns
    // `no_ax_capability_output()` — a runtime signal, which is the better home
    // for it anyway (R9).
    const NAME: &'static str = "desktop_ax_query_focused";
    const DESCRIPTION: &'static str =
        "Return the UI element currently holding keyboard focus via the OS accessibility API. \
         Response contains an `element` field (null if no focused element). The element carries \
         its own affordances: `actions` (the exact AX action names it supports — pass one \
         verbatim to `ax_action` instead of guessing), `enabled:false` when greyed out, \
         `settable:false` when its value cannot be written, `secure:true` for a password field \
         (whose value is never returned).";

    type Args = DesktopAxQueryFocusedArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => return Ok(no_ax_capability_output()),
        };
        match ax.query_focused(QueryFocusedParams { pid: args.pid }).await {
            // The wire type is the response here — there is no projection to
            // redact in, so the tree is scrubbed before it is serialized.
            Ok(element) => Ok(DesktopOutput {
                success: true,
                data: Some(json!({ "element": element.map(redact_secure_values) })),
                message: None,
            }),
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── desktop_ax_query_tree ───────────────────────────────────────────────────

/// LLM tool: return the full AX subtree rooted at a process.
#[derive(Clone, Default)]
pub struct DesktopAxQueryTree {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxQueryTree {
    // Platform availability is stated once for the whole AX family, on
    // `desktop_ax_snapshot`. Four copies used to ship in every request that
    // discloses the desktop family; `no_sentence_is_stated_twice` counts what
    // ships, not what is written. A call where no AX layer exists returns
    // `no_ax_capability_output()` — a runtime signal, which is the better home
    // for it anyway (R9).
    const NAME: &'static str = "desktop_ax_query_tree";
    const DESCRIPTION: &'static str =
        "Return the AX element tree for a process (the frontmost app if `pid` is omitted). \
         Bounded by `max_depth` (default 6). Response contains an `element` field \
         with nested `children`; pure layout wrappers (AXGroup/AXUnknown carrying no \
         label, value, actions or affordance flags) are flattened away — their children \
         are kept, the wrappers are counted in `elided`. Each node carries its own \
         affordances: `actions` (the exact \
         AX action names that node supports — pass one verbatim to `ax_action` instead of \
         guessing), `enabled` (false = greyed out), `settable` (false = value not writable), \
         `secure` (true = password field; its value is never returned).";

    type Args = DesktopAxQueryTreeArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => return Ok(no_ax_capability_output()),
        };
        let params = QueryTreeParams {
            pid: args.pid,
            max_depth: args.max_depth.unwrap_or(6),
            max_nodes: clamp_max_nodes(args.max_nodes.unwrap_or(DEFAULT_MAX_NODES)),
        };
        match ax.query_tree(params).await {
            Ok(result) => {
                // Render-only compression: pure layout wrappers (AXGroup/
                // AXUnknown with no label/value/actions/affordances) are
                // elided and their children rehomed, so the node budget is
                // spent on real controls. Matching paths (verify_state,
                // gui_locate, set_of_marks) still see the raw tree.
                let mut element = result.element.map(redact_secure_values);
                let elided = element
                    .as_mut()
                    .map(super::ax_compress::elide_wrapper_nodes)
                    .unwrap_or(0);
                Ok(DesktopOutput {
                    success: true,
                    // `truncated` travels with the tree, never silently: a model
                    // handed a clipped subtree with no marker concludes the control
                    // it is looking for is absent.
                    data: Some(json!({
                        "element": element,
                        "node_count": result.node_count,
                        "elided": elided,
                        "truncated": result.truncated,
                    })),
                    message: result.truncated.then(|| {
                        "Tree truncated at the node budget — this is not the whole app. Narrow it \
                         with `pid` / a smaller `max_depth`, or use ax_query_by_role."
                            .to_string()
                    }),
                })
            }
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── desktop_ax_query_by_role ────────────────────────────────────────────────

/// LLM tool: collect all AX elements matching a given role.
#[derive(Clone, Default)]
pub struct DesktopAxQueryByRole {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxQueryByRole {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxQueryByRole {
    // Platform availability is stated once for the whole AX family, on
    // `desktop_ax_snapshot`. Four copies used to ship in every request that
    // discloses the desktop family; `no_sentence_is_stated_twice` counts what
    // ships, not what is written. A call where no AX layer exists returns
    // `no_ax_capability_output()` — a runtime signal, which is the better home
    // for it anyway (R9).
    const NAME: &'static str = "desktop_ax_query_by_role";
    const DESCRIPTION: &'static str =
        "Collect all AX elements whose role matches `role` (e.g. \"AXButton\") in a process. \
         If `pid` is omitted, the frontmost application is queried. Response contains an \
         `elements` array; each element carries its own `actions` (the exact AX action names it \
         supports — pass one verbatim to `ax_action` instead of guessing), plus `enabled` \
         (false = greyed out), `settable` (false = value not writable) and `secure` (true = \
         password field; its value is never returned).";

    type Args = DesktopAxQueryByRoleArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => return Ok(no_ax_capability_output()),
        };
        let params = QueryByRoleParams {
            role: args.role,
            pid: args.pid,
            max_nodes: clamp_max_nodes(args.max_nodes.unwrap_or(DEFAULT_MAX_NODES)),
        };
        match ax.query_by_role(params).await {
            Ok(result) => Ok(DesktopOutput {
                success: true,
                data: Some(json!({
                    "elements": result
                        .elements
                        .into_iter()
                        .map(redact_secure_values)
                        .collect::<Vec<_>>(),
                    "node_count": result.node_count,
                    "truncated": result.truncated,
                })),
                message: result.truncated.then(|| {
                    "Search stopped at the node budget — there may be further matches beyond \
                     these. Narrow it with `pid`."
                        .to_string()
                }),
            }),
            Err(e) => Ok(bridge_err_output(e)),
        }
    }
}

// ── desktop_ax_snapshot ─────────────────────────────────────────────────────

const SNAPSHOT_DEFAULT_DEPTH: u32 = 16;
const SNAPSHOT_MAX_DEPTH: u32 = 32;
const SNAPSHOT_DEFAULT_ELEMENTS: usize = 200;
const SNAPSHOT_TEXT_CAP: usize = 120;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopAxSnapshotArgs {
    /// Target PID. Defaults to the frontmost app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Exact window ID where supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u64>,
    /// Tree depth (default 16, max 32).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Rows returned (default 200, max 500).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_elements: Option<usize>,
    /// Nodes examined (default 1500, max 10000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<u32>,
}

/// Round to one decimal place — click targets do not need sub-pixel precision
/// and shorter numbers keep the response compact.
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Render AX semantics and include a center only when bounds are available.
///
/// `value` goes through [`safe_value`] — a secure field's contents must not
/// appear here — and the element's own affordances (`actions` / `enabled` /
/// `settable` / `secure`) are appended by [`affordance_fields`], which omits
/// everything the model already assumes.
pub(super) fn element_to_json(index: usize, el: &AxElement) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("index".into(), json!(index));
    map.insert("role".into(), json!(el.role));
    if let Some((x, y, width, height)) = usable_bounds(el) {
        map.insert(
            "center".into(),
            json!([round1(x + width / 2.0), round1(y + height / 2.0)]),
        );
    }
    if let Some(name) = el.title.as_deref().filter(|s| !s.trim().is_empty()) {
        map.insert(
            "name".into(),
            json!(truncate_with_marker(name, SNAPSHOT_TEXT_CAP, "…")),
        );
    }
    if let Some(value) = safe_value(el) {
        map.insert(
            "value".into(),
            json!(truncate_with_marker(value, SNAPSHOT_TEXT_CAP, "…")),
        );
    }
    map.extend(affordance_fields(el));
    serde_json::Value::Object(map)
}

fn render_reference_snapshot(
    snapshot: AxReferenceSnapshot,
    read: &super::element_ref::SnapshotRead,
    session: &str,
) -> DesktopOutput {
    use super::element_ref::{self, ElementRecord};

    let mut records = Vec::new();
    let mut elements = Vec::with_capacity(snapshot.elements.len());
    for (index, row) in snapshot.elements.into_iter().enumerate() {
        let projected = element_to_json(index, &row.element);
        let token_index = row.reference.map(|reference| {
            let token_index = records.len();
            records.push(ElementRecord {
                pid: snapshot.app_pid,
                window_id: snapshot.window_id,
                reference,
            });
            token_index
        });
        elements.push((projected, token_index));
    }
    let generation = if snapshot.app_pid > 0 {
        match read.publish(session, snapshot.app_pid, records) {
            Ok(id) => Some(id),
            Err(error) => return element_ref::token_error_output(&error),
        }
    } else {
        None
    };
    let elements: Vec<_> = elements
        .into_iter()
        .map(|(mut projected, token_index)| {
            if let Some((id, index)) = generation.zip(token_index) {
                projected["token"] = json!(element_ref::render(id, index));
            }
            projected
        })
        .collect();
    let focused = snapshot
        .focused
        .map(|element| json!({ "role": element.role, "title": element.title }));
    let mut data = json!({
        "app_pid": snapshot.app_pid,
        "count": elements.len(),
        "truncated": snapshot.truncated,
        "tree_truncated": snapshot.tree_truncated,
        "nodes_walked": snapshot.nodes_walked,
        "total_interactable": snapshot.total_interactable,
        "focused": focused,
        "elements": elements,
    });
    if let Some(window_id) = snapshot.window_id {
        data["window_id"] = json!(window_id);
    }
    let message = if !snapshot.tree_available {
        Some("No accessible UI tree for the target application.".into())
    } else if snapshot.tree_truncated {
        Some(
            "The accessibility walk hit its budget. Narrow the target or increase the node budget."
                .into(),
        )
    } else {
        None
    };
    DesktopOutput {
        success: true,
        data: Some(data),
        message,
    }
}

/// Observe accessibility state and issue session-bound native element references.
#[derive(Clone, Default)]
pub struct DesktopAxSnapshot {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopAxSnapshot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopAxSnapshot {
    const NAME: &'static str = "desktop_ax_snapshot";
    const DESCRIPTION: &'static str =
        "Read an application's accessibility UI as JSON. Use a row's token as desktop.element \
         with ax_action or set_value. Pass an action from the row's actions list verbatim. \
         A token is valid only for the current session and observation. Read a new snapshot \
         of the same application or window after each action to verify the result. \
         Coordinates are optional. Disabled or secure controls can restrict actions. \
         Omit pid and window_id for the frontmost application.";

    type Args = DesktopAxSnapshotArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use super::element_ref::{self, SnapshotRead};

        let Some(platform) = self.platform.as_ref() else {
            return Ok(no_platform_output());
        };
        let Some(ax) = platform.ax() else {
            return Ok(no_ax_capability_output());
        };
        let read = match SnapshotRead::begin(args.pid) {
            Ok(read) => read,
            Err(error) => return Ok(element_ref::token_error_output(&error)),
        };
        let session = super::held_inputs::current_session_id();
        let request = AxSnapshotRequest {
            pid: args.pid,
            window_id: args.window_id,
            max_depth: args
                .max_depth
                .unwrap_or(SNAPSHOT_DEFAULT_DEPTH)
                .min(SNAPSHOT_MAX_DEPTH),
            max_nodes: clamp_max_nodes(args.max_nodes.unwrap_or(DEFAULT_MAX_NODES)),
            max_elements: args
                .max_elements
                .unwrap_or(SNAPSHOT_DEFAULT_ELEMENTS)
                .min(MAX_AX_REFERENCE_ROWS),
        };
        let snapshot = match ax.snapshot_references(request).await {
            Ok(snapshot) => snapshot,
            Err(error) => return Ok(element_ref::driver_error_output(error)),
        };
        if args.pid.is_some_and(|pid| pid != snapshot.app_pid)
            || args
                .window_id
                .is_some_and(|id| Some(id) != snapshot.window_id)
            || snapshot
                .elements
                .iter()
                .any(|row| row.element.pid != snapshot.app_pid)
        {
            return Ok(DesktopOutput::failed(
                "The accessibility provider returned a different target.",
            ));
        }
        Ok(render_reference_snapshot(snapshot, &read, &session))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::desktop_bridge::methods::screen::Region;

    fn leaf(role: &str, title: Option<&str>, bounds: Option<Region>) -> AxElement {
        AxElement {
            role: role.to_string(),
            title: title.map(str::to_string),
            bounds,
            pid: 1,
            ..Default::default()
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    #[tokio::test]
    async fn query_focused_without_platform_returns_message() {
        let tool = DesktopAxQueryFocused::new();
        let out = tool
            .call(DesktopAxQueryFocusedArgs { pid: None })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[tokio::test]
    async fn query_tree_without_platform_returns_message() {
        let tool = DesktopAxQueryTree::new();
        let out = tool
            .call(DesktopAxQueryTreeArgs {
                pid: None,
                max_depth: None,
                max_nodes: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[tokio::test]
    async fn query_by_role_without_platform_returns_message() {
        let tool = DesktopAxQueryByRole::new();
        let out = tool
            .call(DesktopAxQueryByRoleArgs {
                role: "AXButton".to_string(),
                pid: None,
                max_nodes: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[tokio::test]
    async fn snapshot_without_platform_returns_message() {
        let tool = DesktopAxSnapshot::new();
        let out = tool
            .call(DesktopAxSnapshotArgs {
                pid: None,
                window_id: None,
                max_depth: None,
                max_elements: None,
                max_nodes: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }

    #[test]
    fn snapshot_element_carries_its_own_actions_and_greyed_state() {
        let mut disabled = leaf("AXButton", Some("Submit"), Some(rect(0.0, 0.0, 10.0, 10.0)));
        disabled.enabled = Some(false);
        disabled.actions = Some(vec!["AXPress".into(), "AXShowMenu".into()]);

        let mut healthy = leaf(
            "AXButton",
            Some("Cancel"),
            Some(rect(20.0, 0.0, 10.0, 10.0)),
        );
        healthy.enabled = Some(true);

        let elements = [element_to_json(0, &disabled), element_to_json(1, &healthy)];

        // The greyed button says so, and lists the action names it accepts —
        // `ax_action` no longer has to guess one.
        assert_eq!(elements[0]["enabled"], json!(false));
        assert_eq!(elements[0]["actions"], json!(["AXPress", "AXShowMenu"]));
        // The healthy one stays compact: no key for a state already assumed.
        assert!(elements[1].get("enabled").is_none());
        assert!(elements[1].get("actions").is_none());
    }

    #[test]
    fn redaction_walks_the_whole_tree() {
        let mut secret = leaf(
            "AXTextField",
            Some("Password"),
            Some(rect(0.0, 0.0, 9.0, 9.0)),
        );
        secret.value = Some("hunter2".into());
        secret.secure = Some(true);
        let mut normal = leaf("AXTextField", Some("Email"), Some(rect(0.0, 0.0, 9.0, 9.0)));
        normal.value = Some("a@b.c".into());

        let tree = AxElement {
            role: "AXWindow".to_string(),
            pid: 1,
            children: vec![AxElement {
                role: "AXGroup".to_string(),
                pid: 1,
                children: vec![secret, normal],
                ..Default::default()
            }],
            ..Default::default()
        };

        let scrubbed = redact_secure_values(tree);
        let json = serde_json::to_string(&scrubbed).unwrap();
        assert!(!json.contains("hunter2"), "secure value survived: {json}");
        assert!(json.contains("a@b.c"), "non-secure value must survive");
    }
}
