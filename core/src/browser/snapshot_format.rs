//! AI-friendly text-tree formatter for ARIA snapshots.
//!
//! Converts an [`AriaSnapshot`] into a compact, indented text tree optimized for
//! LLM consumption. Supports role-based filtering, depth limits, and character
//! truncation to control token usage.

use super::types::{content_roles, interactive_roles, structural_roles, AriaElement, AriaSnapshot};

/// Options controlling how the snapshot is formatted.
#[derive(Debug, Clone)]
pub struct SnapshotFormatOptions {
    /// Only include interactive elements (buttons, inputs, links, etc.).
    pub interactive_only: bool,
    /// Skip unnamed structural/container elements (default: true).
    pub compact: bool,
    /// Maximum output characters before truncation.
    pub max_chars: Option<usize>,
    /// Maximum tree depth to include.
    pub max_depth: Option<usize>,
}

impl Default for SnapshotFormatOptions {
    fn default() -> Self {
        Self {
            interactive_only: false,
            compact: true,
            max_chars: Some(30_000),
            max_depth: None,
        }
    }
}

/// Result of formatting a snapshot.
pub struct SnapshotFormatResult {
    /// The formatted text tree.
    pub text: String,
    /// Whether the output was truncated due to max_chars.
    pub truncated: bool,
    /// Number of element refs included.
    pub ref_count: usize,
}

/// Format an [`AriaSnapshot`] as an AI-friendly text tree.
pub fn format_snapshot(
    snapshot: &AriaSnapshot,
    opts: &SnapshotFormatOptions,
) -> SnapshotFormatResult {
    let mut lines = Vec::new();
    let mut ref_count = 0;

    // Add page context header
    if let Some(title) = &snapshot.page_title {
        if let Some(url) = &snapshot.page_url {
            lines.push(format!("[{title}] ({url})"));
        } else {
            lines.push(format!("[{title}]"));
        }
    } else if let Some(url) = &snapshot.page_url {
        lines.push(format!("({url})"));
    }

    if let Some(focused) = &snapshot.focused_ref {
        lines.push(format!("focused: {focused}"));
    }

    for element in &snapshot.elements {
        visit_element(element, 0, opts, &mut lines, &mut ref_count);
    }

    let mut text = lines.join("\n");
    let truncated = if let Some(max) = opts.max_chars {
        if text.len() > max {
            // Truncate at char boundary
            let truncate_at = text.floor_char_boundary(max);
            text.truncate(truncate_at);
            text.push_str("\n\n[...TRUNCATED - page too large]");
            true
        } else {
            false
        }
    } else {
        false
    };

    SnapshotFormatResult {
        text,
        truncated,
        ref_count,
    }
}

fn should_include(role: &str, name: &Option<String>, opts: &SnapshotFormatOptions) -> bool {
    if opts.interactive_only && !interactive_roles().contains(role) {
        return false;
    }
    if opts.compact && structural_roles().contains(role) && name.is_none() {
        return false;
    }
    true
}

fn should_create_ref(role: &str, name: &Option<String>) -> bool {
    interactive_roles().contains(role) || (content_roles().contains(role) && name.is_some())
}

fn visit_element(
    el: &AriaElement,
    depth: usize,
    opts: &SnapshotFormatOptions,
    lines: &mut Vec<String>,
    ref_count: &mut usize,
) {
    if let Some(max_depth) = opts.max_depth {
        if depth > max_depth {
            return;
        }
    }

    let include = should_include(&el.role, &el.name, opts);

    if include {
        let indent = "  ".repeat(depth);
        let mut line = format!("{indent}- {}", el.role);

        if let Some(name) = &el.name {
            let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
            line.push_str(&format!(" \"{escaped}\""));
        }

        if should_create_ref(&el.role, &el.name) && !el.ref_id.is_empty() {
            line.push_str(&format!(" [ref={}]", el.ref_id));
            *ref_count += 1;
        }

        if let Some(value) = &el.value {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            line.push_str(&format!(" value=\"{escaped}\""));
        }

        if !el.state.is_empty() {
            line.push_str(&format!(" ({})", el.state.join(", ")));
        }

        lines.push(line);
    }

    // Visit children at same depth if this node was filtered out, deeper if included
    let child_depth = if include { depth + 1 } else { depth };
    for child in &el.children {
        visit_element(child, child_depth, opts, lines, ref_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::types::{AriaElement, AriaSnapshot};

    fn el(ref_id: &str, role: &str, name: Option<&str>) -> AriaElement {
        AriaElement {
            ref_id: ref_id.to_string(),
            role: role.to_string(),
            name: name.map(|s| s.to_string()),
            value: None,
            state: vec![],
            bounds: None,
            children: vec![],
        }
    }

    fn el_with_children(
        ref_id: &str,
        role: &str,
        name: Option<&str>,
        children: Vec<AriaElement>,
    ) -> AriaElement {
        AriaElement {
            ref_id: ref_id.to_string(),
            role: role.to_string(),
            name: name.map(|s| s.to_string()),
            value: None,
            state: vec![],
            bounds: None,
            children,
        }
    }

    fn snap(elements: Vec<AriaElement>) -> AriaSnapshot {
        AriaSnapshot {
            elements,
            page_title: Some("Test Page".to_string()),
            page_url: Some("https://example.com".to_string()),
            focused_ref: None,
        }
    }

    #[test]
    fn test_basic_format() {
        let snapshot = snap(vec![
            el("btn-1", "button", Some("Submit")),
            el("link-1", "link", Some("Home")),
        ]);

        let result = format_snapshot(&snapshot, &SnapshotFormatOptions::default());
        assert!(result.text.contains("[Test Page] (https://example.com)"));
        assert!(result.text.contains("- button \"Submit\" [ref=btn-1]"));
        assert!(result.text.contains("- link \"Home\" [ref=link-1]"));
        assert_eq!(result.ref_count, 2);
        assert!(!result.truncated);
    }

    #[test]
    fn test_nested_format() {
        let snapshot = snap(vec![el_with_children(
            "nav-1",
            "navigation",
            Some("Main"),
            vec![
                el("link-1", "link", Some("Home")),
                el("link-2", "link", Some("About")),
            ],
        )]);

        let result = format_snapshot(&snapshot, &SnapshotFormatOptions::default());
        assert!(result.text.contains("- navigation \"Main\" [ref=nav-1]"));
        assert!(result.text.contains("  - link \"Home\" [ref=link-1]"));
        assert!(result.text.contains("  - link \"About\" [ref=link-2]"));
        assert_eq!(result.ref_count, 3);
    }

    #[test]
    fn test_interactive_only() {
        let snapshot = snap(vec![
            el_with_children(
                "nav-1",
                "navigation",
                Some("Main"),
                vec![
                    el("link-1", "link", Some("Home")),
                    el("h1", "heading", Some("Title")),
                ],
            ),
            el("btn-1", "button", Some("Click")),
        ]);

        let opts = SnapshotFormatOptions {
            interactive_only: true,
            ..Default::default()
        };
        let result = format_snapshot(&snapshot, &opts);
        assert!(result.text.contains("- link \"Home\""));
        assert!(result.text.contains("- button \"Click\""));
        // navigation and heading are not interactive
        assert!(!result.text.contains("- navigation"));
        assert!(!result.text.contains("- heading"));
    }

    #[test]
    fn test_compact_skips_unnamed_structural() {
        let snapshot = snap(vec![
            el_with_children("", "group", None, vec![el("btn-1", "button", Some("OK"))]),
            el_with_children(
                "",
                "list",
                None,
                vec![el("item-1", "listitem", Some("Item 1"))],
            ),
        ]);

        let opts = SnapshotFormatOptions {
            compact: true,
            ..Default::default()
        };
        let result = format_snapshot(&snapshot, &opts);
        // group and list are structural and unnamed, should be skipped
        assert!(!result.text.contains("- group"));
        assert!(!result.text.contains("- list "));
        assert!(!result.text.contains("- list\n"));
        // children should bubble up to same depth
        assert!(result.text.contains("- button \"OK\""));
        assert!(result.text.contains("- listitem \"Item 1\""));
    }

    #[test]
    fn test_truncation() {
        let snapshot = snap(vec![
            el(
                "btn-1",
                "button",
                Some("A very long button name that takes up space"),
            ),
            el("btn-2", "button", Some("Another button with a long name")),
        ]);

        let opts = SnapshotFormatOptions {
            max_chars: Some(50),
            ..Default::default()
        };
        let result = format_snapshot(&snapshot, &opts);
        assert!(result.truncated);
        assert!(result.text.contains("[...TRUNCATED"));
    }

    #[test]
    fn test_max_depth() {
        let snapshot = snap(vec![el_with_children(
            "nav-1",
            "navigation",
            Some("Main"),
            vec![el_with_children(
                "list-1",
                "list",
                Some("Links"),
                vec![el("link-1", "link", Some("Deep"))],
            )],
        )]);

        let opts = SnapshotFormatOptions {
            max_depth: Some(1),
            compact: false,
            ..Default::default()
        };
        let result = format_snapshot(&snapshot, &opts);
        assert!(result.text.contains("- navigation"));
        assert!(result.text.contains("- list"));
        assert!(!result.text.contains("- link \"Deep\""));
    }

    #[test]
    fn test_value_and_state() {
        let snapshot = snap(vec![AriaElement {
            ref_id: "input-1".to_string(),
            role: "textbox".to_string(),
            name: Some("Email".to_string()),
            value: Some("user@test.com".to_string()),
            state: vec!["focused".to_string(), "required".to_string()],
            bounds: None,
            children: vec![],
        }]);

        let result = format_snapshot(&snapshot, &SnapshotFormatOptions::default());
        assert!(result.text.contains(
            "- textbox \"Email\" [ref=input-1] value=\"user@test.com\" (focused, required)"
        ));
    }

    #[test]
    fn test_empty_snapshot() {
        let snapshot = AriaSnapshot {
            elements: vec![],
            page_title: None,
            page_url: None,
            focused_ref: None,
        };

        let result = format_snapshot(&snapshot, &SnapshotFormatOptions::default());
        assert!(result.text.is_empty());
        assert_eq!(result.ref_count, 0);
    }
}
