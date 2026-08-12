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
    pub children: Vec<Self>,
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

/// Split palette input into the part that narrows the list and the part that
/// is arguments for whichever command is finally chosen.
///
/// # Why the palette has to carry arguments at all
///
/// It is the only way in. `/` on an empty composer opens this palette rather
/// than typing a `/`, so a slash command's argument cannot be typed into the
/// composer at all (short of typing a space first, which nothing tells anyone
/// to do). Confirming an entry runs its `full_command` — the bare `/think` —
/// so before this, every command that takes a value could only ever be
/// invoked without one: `/tier`, `/mode`, `/think`, `/memory-mode` and
/// `/tools` printed their current value and their usage line and nothing else,
/// and `/compact <instructions>` could not be steered. The four session knobs
/// shipped in that state — readable from the TUI, not settable from it.
///
/// The first run of whitespace splits it. Everything before narrows the list;
/// everything after is passed through verbatim. When the head alone matches
/// nothing, the caller falls back to narrowing on the whole string with no
/// arguments — so a multi-word search over the entries' description text keeps
/// working exactly as it did, and this is never a worse candidate list than
/// before.
pub fn split_palette_input(input: &str) -> (&str, &str) {
    input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(head, rest)| (head, rest.trim_start()))
}

/// The bare word an entry is addressed by: its label without the leading `/`
/// and without any parameter hint — `/think` → `think`, `new [topic]` → `new`.
#[must_use]
pub fn entry_command_word(label: &str) -> &str {
    label
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("")
}

/// Ranking for a filtered palette list: an entry whose command word IS the
/// filter comes first, then entries whose word starts with it, then the rest
/// (matched only through their description text).
///
/// # Why this is not cosmetic
///
/// The filter matches an entry's hint as well as its label, and confirming
/// runs whatever is selected — index 0 by default. Typing `mode` therefore
/// selected `/tools`, whose hint reads "Tool progress mode: …", ahead of
/// `/mode` itself. That was merely annoying while the palette could only run
/// a command bare; once it carries arguments
/// ([`split_palette_input`]), `mode chat` would hand `chat` to `/tools`. An
/// exact name has to outrank a mention.
#[must_use]
pub fn filter_rank(label: &str, filter: &str) -> u8 {
    let word = entry_command_word(label).to_lowercase();
    let filter = filter.trim_start_matches('/').to_lowercase();
    if word == filter {
        0
    } else if word.starts_with(&filter) {
        1
    } else {
        2
    }
}

impl CommandEntry {
    /// Parse a list of `CommandEntry` from a JSON value (the "commands" array
    /// returned by the `commands.list` RPC).
    pub fn parse_from_json(value: &serde_json::Value) -> Vec<Self> {
        let commands = value
            .get("commands")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        commands.iter().filter_map(Self::parse_one).collect()
    }

    fn parse_one(val: &serde_json::Value) -> Option<Self> {
        let name = val
            .get("name")
            .and_then(|v| v.as_str())
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
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let param_hint = val
            .get("param_hint")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);

        let children = val
            .get("children")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(Self::parse_one).collect())
            .unwrap_or_default();

        Some(Self {
            name: name.to_string(),
            hint,
            is_namespace,
            param_hint,
            children,
        })
    }

    /// Flatten the root level into display entries.
    pub fn root_display_entries(entries: &[Self]) -> Vec<DisplayEntry> {
        entries
            .iter()
            .map(|e| {
                let label = if e.is_namespace {
                    e.name.clone()
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
    pub fn find_namespace<'a>(entries: &'a [Self], name: &str) -> Option<&'a Self> {
        entries
            .iter()
            .find(|e| e.is_namespace && e.name.eq_ignore_ascii_case(name))
    }

    /// Flatten children of a namespace into display entries.
    pub fn namespace_display_entries(namespace: &Self, namespace_path: &str) -> Vec<DisplayEntry> {
        namespace
            .children
            .iter()
            .map(|child| {
                let label = child
                    .param_hint
                    .as_ref()
                    .map_or_else(|| child.name.clone(), |ph| format!("{} {}", child.name, ph));
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
    fn an_exact_command_name_outranks_a_mention_in_a_description() {
        // `/tools`' hint contains the word "mode"; `/mode` is the command.
        assert!(filter_rank("/mode", "mode") < filter_rank("/tools", "mode"));
        // Prefix beats description-only.
        assert!(filter_rank("/memory-mode", "mem") < filter_rank("/tools", "mem"));
        // A namespace child is addressed by its own word, hint and all.
        assert_eq!(filter_rank("new [topic]", "new"), 0);
        assert_eq!(entry_command_word("new [topic]"), "new");
    }

    #[test]
    fn palette_input_splits_a_command_from_its_argument() {
        assert_eq!(split_palette_input("think high"), ("think", "high"));
        assert_eq!(split_palette_input("think"), ("think", ""));
        assert_eq!(split_palette_input(""), ("", ""));
        // Extra spaces belong to neither half.
        assert_eq!(
            split_palette_input("compact   keep the api notes"),
            ("compact", "keep the api notes")
        );
        // A trailing space is not an argument.
        assert_eq!(split_palette_input("think "), ("think", ""));
    }

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
        assert_eq!(
            entries[0].children[0].param_hint,
            Some("[topic]".to_string())
        );

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
                hint: String::new(),
                is_namespace: true,
                param_hint: None,
                children: vec![],
            },
            CommandEntry {
                name: "search".into(),
                hint: String::new(),
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
