//! Snapshot conversion from Chrome DevTools MCP tree format to Aleph's AriaSnapshot.

use super::error::BrowserError;
use super::types::{AriaElement, AriaSnapshot};

pub fn convert_chrome_mcp_snapshot(raw: &serde_json::Value) -> Result<AriaSnapshot, BrowserError> {
    let root = convert_node(raw);
    Ok(AriaSnapshot {
        elements: vec![root],
        page_title: raw.get("name").and_then(|v| v.as_str()).map(String::from),
        page_url: raw.get("url").and_then(|v| v.as_str()).map(String::from),
        focused_ref: None,
    })
}

fn convert_node(node: &serde_json::Value) -> AriaElement {
    let mut state = Vec::new();
    if node.get("focused").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("focused".to_string());
    }
    if node.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("disabled".to_string());
    }
    if node.get("expanded").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("expanded".to_string());
    }
    if node.get("checked").and_then(|v| v.as_bool()).unwrap_or(false) {
        state.push("checked".to_string());
    }

    let children = node
        .get("children")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(convert_node).collect())
        .unwrap_or_default();

    AriaElement {
        ref_id: node.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        role: node.get("role").and_then(|v| v.as_str()).unwrap_or("generic").to_string(),
        name: node.get("name").and_then(|v| v.as_str()).map(String::from),
        value: node.get("value").and_then(|v| v.as_str()).map(String::from),
        state,
        bounds: None,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_single_element() {
        let raw = json!({
            "role": "WebArea",
            "name": "Test Page",
            "children": [
                {
                    "role": "button",
                    "name": "Submit",
                    "id": "btn-1"
                }
            ]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        assert_eq!(snapshot.elements.len(), 1);
        assert_eq!(snapshot.elements[0].role, "WebArea");
        assert_eq!(snapshot.elements[0].children.len(), 1);
        assert_eq!(snapshot.elements[0].children[0].ref_id, "btn-1");
        assert_eq!(snapshot.elements[0].children[0].role, "button");
        assert_eq!(snapshot.elements[0].children[0].name.as_deref(), Some("Submit"));
    }

    #[test]
    fn test_convert_nested_tree() {
        let raw = json!({
            "role": "WebArea",
            "name": "Page",
            "children": [
                {
                    "role": "navigation",
                    "name": "Main",
                    "id": "nav-1",
                    "children": [
                        { "role": "link", "name": "Home", "id": "link-1" },
                        { "role": "link", "name": "About", "id": "link-2" }
                    ]
                },
                {
                    "role": "button",
                    "name": "Login",
                    "id": "btn-1"
                }
            ]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        let root = &snapshot.elements[0];
        assert_eq!(root.children.len(), 2);
        let nav = &root.children[0];
        assert_eq!(nav.ref_id, "nav-1");
        assert_eq!(nav.children.len(), 2);
        assert_eq!(nav.children[0].ref_id, "link-1");
        assert_eq!(nav.children[1].ref_id, "link-2");
    }

    #[test]
    fn test_convert_element_with_value_and_state() {
        let raw = json!({
            "role": "WebArea",
            "children": [{
                "role": "textbox",
                "name": "Email",
                "id": "input-1",
                "value": "user@example.com",
                "focused": true,
                "disabled": false
            }]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        let input = &snapshot.elements[0].children[0];
        assert_eq!(input.role, "textbox");
        assert_eq!(input.value.as_deref(), Some("user@example.com"));
        assert!(input.state.contains(&"focused".to_string()));
    }

    #[test]
    fn test_convert_empty_tree() {
        let raw = json!({
            "role": "WebArea",
            "name": "Empty Page"
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        assert_eq!(snapshot.elements.len(), 1);
        assert!(snapshot.elements[0].children.is_empty());
    }

    #[test]
    fn test_missing_id_uses_empty_string() {
        let raw = json!({
            "role": "WebArea",
            "children": [{
                "role": "paragraph",
                "name": "Some text"
            }]
        });

        let snapshot = convert_chrome_mcp_snapshot(&raw).unwrap();
        let para = &snapshot.elements[0].children[0];
        assert_eq!(para.ref_id, "");
    }
}
