//! Accessibility tree query schemas for the desktop bridge.
//!
//! Exposes three read-only AX operations:
//! - `ax.query_focused` — element currently holding keyboard focus
//! - `ax.query_tree`    — full subtree rooted at a given process (or the
//!   frontmost app if `pid` is omitted)
//! - `ax.query_by_role` — collect all elements matching an AX role string

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::screen::Region;

// ── Method name constants ────────────────────────────────────────────────────

pub const METHOD_QUERY_FOCUSED: &str = "ax.query_focused";
pub const METHOD_QUERY_TREE: &str = "ax.query_tree";
pub const METHOD_QUERY_BY_ROLE: &str = "ax.query_by_role";
pub const METHOD_SET_VALUE: &str = "ax.set_value";
pub const METHOD_PERFORM_ACTION: &str = "ax.perform_action";
pub const NOTIFY_MUTATION: &str = "ax.mutation";
pub const SUGGESTED_TIMEOUT_MS: u64 = 3_000;

// ── Request params ───────────────────────────────────────────────────────────

/// Params for `ax.query_focused` — no inputs required.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryFocusedParams {}

/// Params for `ax.query_tree`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryTreeParams {
    /// pid of the target application; `null` means "use the frontmost app".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Maximum depth of the returned subtree (default 6 to bound response size).
    #[serde(default = "default_depth")]
    pub max_depth: u32,
}

const fn default_depth() -> u32 {
    6
}

/// Params for `ax.query_by_role`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryByRoleParams {
    /// AX role string to match, e.g. `"AXButton"`.
    pub role: String,
    /// pid of the target application; `null` means "use the frontmost app".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
}

/// Stateless element locator for `ax.set_value` / `ax.perform_action`.
///
/// The bridge re-walks the AX tree on every call and picks the best match:
/// role filter → title match (exact beats contains, case-insensitive) →
/// nearest `center` tiebreak. No element handles cross the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxLocator {
    /// pid of the target application; `null` means "use the frontmost app".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// AX role filter, e.g. `"AXTextField"`. Optional but recommended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Title/label to match (exact beats contains, case-insensitive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Global screen-point `[x, y]` used as a nearest-center tiebreak.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
}

/// Params for `ax.set_value`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetValueParams {
    pub locator: AxLocator,
    /// New value written to the element's `AXValue` attribute.
    pub value: String,
}

/// Params for `ax.perform_action`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PerformActionParams {
    pub locator: AxLocator,
    /// AX action name passed through verbatim, e.g. `"AXPress"`.
    pub action: String,
}

/// Post-write verification outcome.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxVerification {
    /// `"verified"` when the read-back value matches the written value,
    /// `"unverified"` otherwise (see `reason`).
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// First 200 chars of the value read back after the write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_preview: Option<String>,
}

/// Result for `ax.set_value` and `ax.perform_action`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxActionResult {
    /// Whether the native AX call was issued successfully.
    pub performed: bool,
    /// Always `"accessibility"` — mirrors orca's action-path metadata.
    pub path: String,
    /// The element acted on (children pruned), for model visibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<AxElement>,
    /// Present for `set_value`; absent for `perform_action`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<AxVerification>,
}

// ── Response types ───────────────────────────────────────────────────────────

/// A node in the AX element tree.
///
/// `children` is empty when the subtree has been pruned at the requested depth.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AxElement {
    /// AX role identifier, e.g. `"AXWindow"`, `"AXButton"`.
    pub role: String,
    /// Human-readable title / label (may be absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// String value of the element (may be absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Bounding rectangle in screen-point coordinates, top-left origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Region>,
    /// Process ID of the owning application.
    pub pid: i32,
    /// Child elements (empty when depth limit reached).
    #[serde(default)]
    pub children: Vec<Self>,
}

/// Result for `ax.query_focused` and `ax.query_tree`.
///
/// `element` is `null` when no element was found (e.g. no focused element,
/// or the process is not accessible).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<AxElement>,
}

/// Result for `ax.query_by_role`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryListResult {
    pub elements: Vec<AxElement>,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_tree_default_depth() {
        let p: QueryTreeParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p.max_depth, 6);
    }

    #[test]
    fn query_focused_params_roundtrip() {
        let p = QueryFocusedParams {};
        let json = serde_json::to_string(&p).unwrap();
        let _back: QueryFocusedParams = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn query_by_role_params_roundtrip() {
        let p = QueryByRoleParams {
            role: "AXButton".to_string(),
            pid: Some(1234),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: QueryByRoleParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "AXButton");
        assert_eq!(back.pid, Some(1234));
    }

    #[test]
    fn ax_element_children_default_empty() {
        let json = r#"{"role":"AXWindow","pid":42}"#;
        let el: AxElement = serde_json::from_str(json).unwrap();
        assert_eq!(el.role, "AXWindow");
        assert!(el.children.is_empty());
    }

    #[test]
    fn query_result_element_null() {
        let json = r#"{}"#;
        let r: QueryResult = serde_json::from_str(json).unwrap();
        assert!(r.element.is_none());
    }

    #[test]
    fn query_list_result_roundtrip() {
        let r = QueryListResult {
            elements: vec![AxElement {
                role: "AXButton".to_string(),
                title: Some("OK".to_string()),
                value: None,
                bounds: None,
                pid: 99,
                children: vec![],
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: QueryListResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.elements.len(), 1);
        assert_eq!(back.elements[0].role, "AXButton");
    }

    #[test]
    fn set_value_params_roundtrip() {
        let p = SetValueParams {
            locator: AxLocator {
                pid: None,
                role: Some("AXTextField".into()),
                title: Some("Email".into()),
                center: Some([100.0, 200.0]),
            },
            value: "a@b.c".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: SetValueParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.locator.role.as_deref(), Some("AXTextField"));
        assert_eq!(back.value, "a@b.c");
    }

    #[test]
    fn ax_action_result_verification_optional() {
        let json = r#"{"performed":true,"path":"accessibility"}"#;
        let r: AxActionResult = serde_json::from_str(json).unwrap();
        assert!(r.performed);
        assert!(r.verification.is_none());
    }
}
