//! Accessibility tree query schemas for the desktop bridge.
//!
//! Exposes three read-only AX operations:
//! - `ax.query_focused` — element currently holding keyboard focus
//! - `ax.query_tree`    — full subtree rooted at a given process (or the
//!                        frontmost app if `pid` is omitted)
//! - `ax.query_by_role` — collect all elements matching an AX role string

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::screen::Region;

// ── Method name constants ────────────────────────────────────────────────────

pub const METHOD_QUERY_FOCUSED: &str = "ax.query_focused";
pub const METHOD_QUERY_TREE: &str = "ax.query_tree";
pub const METHOD_QUERY_BY_ROLE: &str = "ax.query_by_role";
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

fn default_depth() -> u32 {
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
    pub children: Vec<AxElement>,
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
}
