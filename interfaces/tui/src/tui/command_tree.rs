// Hierarchical command tree for the command palette.
//
// Represents gateway commands as a tree structure where namespaces
// contain child commands. Supports browsing into namespaces and
// filtering at any level.

/// A single entry in the command tree. May be a namespace (has children)
/// or a leaf command.
#[derive(Clone, Debug)]
pub struct CommandEntry {
    pub name: String,
    pub hint: String,
    pub is_namespace: bool,
    pub param_hint: Option<String>,
    pub children: Vec<CommandEntry>,
}

/// A flattened display item for the command palette, produced from
/// browsing a specific level of the command tree.
#[derive(Clone, Debug)]
pub struct DisplayEntry {
    /// The display label (e.g. "session" or "new [topic]")
    pub label: String,
    /// Description hint
    pub hint: String,
    /// Whether this entry is a namespace that can be drilled into
    pub is_namespace: bool,
    /// The full command string to insert (e.g. "/session new ")
    pub full_command: String,
}

impl CommandEntry {
    /// Parse a list of CommandEntry from a JSON value (the "commands" array
    /// returned by the `commands.list` RPC).
    pub fn parse_from_json(value: &serde_json::Value) -> Vec<CommandEntry> {
        let commands = value
            .get("commands")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        commands.iter().filter_map(Self::parse_one).collect()
    }

    fn parse_one(val: &serde_json::Value) -> Option<CommandEntry> {
        let name = val.get("name").and_then(|v| v.as_str())
            // Fallback: old format uses "key" field
            .or_else(|| val.get("key").and_then(|v| v.as_str()))?;
        let hint = val
            .get("hint")
            .or_else(|| val.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_namespace = val
            .get("is_namespace")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let param_hint = val
            .get("param_hint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let children = val
            .get("children")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(Self::parse_one).collect())
            .unwrap_or_default();

        Some(CommandEntry {
            name: name.to_string(),
            hint,
            is_namespace,
            param_hint,
            children,
        })
    }

    /// Flatten the root level into display entries.
    pub fn root_display_entries(entries: &[CommandEntry]) -> Vec<DisplayEntry> {
        entries
            .iter()
            .map(|e| {
                let label = if e.is_namespace {
                    format!("{}", e.name)
                } else if let Some(ref ph) = e.param_hint {
                    format!("{} {}", e.name, ph)
                } else {
                    e.name.clone()
                };
                DisplayEntry {
                    label,
                    hint: e.hint.clone(),
                    is_namespace: e.is_namespace,
                    full_command: if e.is_namespace {
                        // Selecting a namespace enters it (don't append space yet)
                        format!("/{}", e.name)
                    } else {
                        format!("/{} ", e.name)
                    },
                }
            })
            .collect()
    }

    /// Find a namespace by name at the root level.
    pub fn find_namespace<'a>(entries: &'a [CommandEntry], name: &str) -> Option<&'a CommandEntry> {
        entries
            .iter()
            .find(|e| e.is_namespace && e.name.eq_ignore_ascii_case(name))
    }

    /// Flatten children of a namespace into display entries.
    pub fn namespace_display_entries(
        namespace: &CommandEntry,
        namespace_path: &str,
    ) -> Vec<DisplayEntry> {
        namespace
            .children
            .iter()
            .map(|child| {
                let label = if let Some(ref ph) = child.param_hint {
                    format!("{} {}", child.name, ph)
                } else {
                    child.name.clone()
                };
                DisplayEntry {
                    label,
                    hint: child.hint.clone(),
                    is_namespace: child.is_namespace,
                    full_command: format!("/{} {} ", namespace_path, child.name),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_tree_structure() {
        let val = json!({
            "commands": [
                {
                    "name": "session",
                    "is_namespace": true,
                    "hint": "Session management",
                    "children": [
                        { "name": "new", "hint": "Start new session", "param_hint": "[topic]" },
                        { "name": "list", "hint": "List all sessions" }
                    ]
                },
                {
                    "name": "search",
                    "is_namespace": false,
                    "hint": "Web search",
                    "param_hint": "<query>"
                }
            ]
        });

        let entries = CommandEntry::parse_from_json(&val);
        assert_eq!(entries.len(), 2);

        assert!(entries[0].is_namespace);
        assert_eq!(entries[0].name, "session");
        assert_eq!(entries[0].children.len(), 2);
        assert_eq!(entries[0].children[0].name, "new");
        assert_eq!(entries[0].children[0].param_hint, Some("[topic]".to_string()));

        assert!(!entries[1].is_namespace);
        assert_eq!(entries[1].name, "search");
    }

    #[test]
    fn parse_old_format_fallback() {
        let val = json!({
            "commands": [
                { "key": "new", "description": "Start new session" }
            ]
        });

        let entries = CommandEntry::parse_from_json(&val);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "new");
        assert_eq!(entries[0].hint, "Start new session");
    }

    #[test]
    fn root_display_entries_correct() {
        let entries = vec![
            CommandEntry {
                name: "session".into(),
                hint: "Session management".into(),
                is_namespace: true,
                param_hint: None,
                children: vec![],
            },
            CommandEntry {
                name: "search".into(),
                hint: "Web search".into(),
                is_namespace: false,
                param_hint: Some("<query>".into()),
                children: vec![],
            },
        ];

        let display = CommandEntry::root_display_entries(&entries);
        assert_eq!(display.len(), 2);
        assert_eq!(display[0].label, "session");
        assert!(display[0].is_namespace);
        assert_eq!(display[0].full_command, "/session");
        assert_eq!(display[1].label, "search <query>");
        assert!(!display[1].is_namespace);
        assert_eq!(display[1].full_command, "/search ");
    }

    #[test]
    fn find_namespace_works() {
        let entries = vec![
            CommandEntry {
                name: "session".into(),
                hint: "".into(),
                is_namespace: true,
                param_hint: None,
                children: vec![],
            },
            CommandEntry {
                name: "search".into(),
                hint: "".into(),
                is_namespace: false,
                param_hint: None,
                children: vec![],
            },
        ];

        assert!(CommandEntry::find_namespace(&entries, "session").is_some());
        assert!(CommandEntry::find_namespace(&entries, "SESSION").is_some());
        assert!(CommandEntry::find_namespace(&entries, "search").is_none()); // not a namespace
        assert!(CommandEntry::find_namespace(&entries, "missing").is_none());
    }

    #[test]
    fn namespace_display_entries_correct() {
        let ns = CommandEntry {
            name: "session".into(),
            hint: "Session management".into(),
            is_namespace: true,
            param_hint: None,
            children: vec![
                CommandEntry {
                    name: "new".into(),
                    hint: "Start new session".into(),
                    is_namespace: false,
                    param_hint: Some("[topic]".into()),
                    children: vec![],
                },
                CommandEntry {
                    name: "list".into(),
                    hint: "List all sessions".into(),
                    is_namespace: false,
                    param_hint: None,
                    children: vec![],
                },
            ],
        };

        let display = CommandEntry::namespace_display_entries(&ns, "session");
        assert_eq!(display.len(), 2);
        assert_eq!(display[0].label, "new [topic]");
        assert_eq!(display[0].full_command, "/session new ");
        assert_eq!(display[1].label, "list");
        assert_eq!(display[1].full_command, "/session list ");
    }
}
