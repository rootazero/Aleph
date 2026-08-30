//! The `commands.list` tree contract, shared by the gateway handler that
//! produces it and every client that renders it.
//!
//! # Why this type exists
//!
//! The tree nodes were private `Serialize`-only structs inside
//! `gateway/handlers/commands.rs`, so their field names were a contract with no
//! compiler behind it. The CLI declared its own `struct Command { key: String,
//! description: String, … }` — two REQUIRED fields, **neither of which the
//! server has ever emitted** (it sends `name` and `hint`). Two consequences,
//! and the second is worse than the first:
//!
//! * `aleph tools` / `aleph tools list` died with `Invalid response: missing
//!   field 'key'`, which at least reads as a bug.
//! * `aleph tools describe <name>` matched raw JSON on `item["key"]`, a
//!   comparison that can never be true, so it returned the fabricated
//!   `tool 'X' not found in commands.list` — a *wrong answer that reads as a
//!   fact about the server*, and it fired ahead of the `--json` branch, so that
//!   escape hatch was dead too.
//!
//! The TUI reads the same RPC by `name` and was correct all along, which is why
//! the family never looked broken.
//!
//! Moving the nodes here makes a rename a compile error on the server and a
//! loud parse error at the client, instead of an empty listing.
//!
//! # Why the `skip_serializing_if` attributes are preserved verbatim
//!
//! They are the existing wire shape: a namespace node omits `param_hint` /
//! `source_type` / `internal_id`, a leaf node omits `children`. Panel and TUI
//! both tolerate absence. Making them unconditional would be a wire change with
//! no reader asking for it, so the DTO keeps the omissions and every optional
//! field carries `#[serde(default)]` on the way back in — the one place a
//! default is correct, because absence here is a real state rather than a
//! server that forgot to send something.

use serde::{Deserialize, Serialize};

/// A child command within a namespace (`session` → `new`, `list`, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildCommandNode {
    /// Subcommand name (e.g. `"new"`), NOT the canonical tool name.
    pub name: String,
    /// Human-readable description.
    pub hint: String,
    /// Parameter hint (e.g. `"[topic]"`, `"<query>"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_hint: Option<String>,
    /// Source type (`"builtin"`, `"mcp"`, `"skill"`, …), lowercased.
    pub source_type: String,
    /// Canonical tool id — the name `tools.invoke` takes.
    pub internal_id: String,
}

/// A top-level entry in the command tree: either a namespace with children, or
/// a standalone command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandTreeNode {
    /// Command or namespace name.
    pub name: String,
    /// Whether this is a namespace (its `children` carry the real commands).
    pub is_namespace: bool,
    /// Human-readable hint/description.
    pub hint: String,
    /// Parameter hint — standalone commands only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_hint: Option<String>,
    /// Source type — standalone commands only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    /// Canonical tool id — standalone commands only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal_id: Option<String>,
    /// Children — namespaces only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ChildCommandNode>,
}

impl CommandTreeNode {
    /// Find the node named `name`, looking through namespaces at
    /// `"<namespace>_<action>"` and `"<namespace> <action>"` as well as at the
    /// child's own short name.
    ///
    /// Namespaced tools are the majority of the catalogue (`session_new`,
    /// `cron_manage`, …) and they are *not* top-level nodes, so a client that
    /// scans only the roots reports "not found" for most of the tools that
    /// exist. Resolving here rather than in each client keeps one answer to
    /// "which node did the user mean".
    #[must_use]
    pub fn find<'a>(nodes: &'a [Self], name: &str) -> Option<CommandMatch<'a>> {
        let wanted = name.trim().trim_start_matches('/');
        for node in nodes {
            if node.name == wanted {
                return Some(CommandMatch::Top(node));
            }
        }
        for node in nodes {
            for child in &node.children {
                let joined_underscore = format!("{}_{}", node.name, child.name);
                let joined_space = format!("{} {}", node.name, child.name);
                if child.internal_id == wanted
                    || joined_underscore == wanted
                    || joined_space == wanted
                    || child.name == wanted
                {
                    return Some(CommandMatch::Child {
                        namespace: &node.name,
                        child,
                    });
                }
            }
        }
        None
    }
}

/// Where [`CommandTreeNode::find`] found the requested command.
#[derive(Debug, Clone, Copy)]
pub enum CommandMatch<'a> {
    Top(&'a CommandTreeNode),
    Child {
        namespace: &'a str,
        child: &'a ChildCommandNode,
    },
}

impl CommandMatch<'_> {
    /// Human-readable description, whichever node matched.
    #[must_use]
    pub fn hint(&self) -> &str {
        match self {
            Self::Top(node) => &node.hint,
            Self::Child { child, .. } => &child.hint,
        }
    }

    /// Source type, whichever node matched. Empty when a namespace matched —
    /// a namespace has no single source.
    #[must_use]
    pub fn source_type(&self) -> &str {
        match self {
            Self::Top(node) => node.source_type.as_deref().unwrap_or_default(),
            Self::Child { child, .. } => &child.source_type,
        }
    }

    /// The canonical tool id `tools.invoke` takes, when the match has one.
    #[must_use]
    pub fn internal_id(&self) -> Option<&str> {
        match self {
            Self::Top(node) => node.internal_id.as_deref(),
            Self::Child { child, .. } => Some(&child.internal_id),
        }
    }

    #[must_use]
    pub fn param_hint(&self) -> Option<&str> {
        match self {
            Self::Top(node) => node.param_hint.as_deref(),
            Self::Child { child, .. } => child.param_hint.as_deref(),
        }
    }
}

/// The `commands.list` response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandListResponse {
    pub commands: Vec<CommandTreeNode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Vec<CommandTreeNode> {
        vec![
            CommandTreeNode {
                name: "session".to_string(),
                is_namespace: true,
                hint: "Session commands".to_string(),
                param_hint: None,
                source_type: None,
                internal_id: None,
                children: vec![ChildCommandNode {
                    name: "new".to_string(),
                    hint: "Start a new session".to_string(),
                    param_hint: Some("[topic]".to_string()),
                    source_type: "builtin".to_string(),
                    internal_id: "session_new".to_string(),
                }],
            },
            CommandTreeNode {
                name: "search".to_string(),
                is_namespace: false,
                hint: "Web search".to_string(),
                param_hint: Some("<query>".to_string()),
                source_type: Some("builtin".to_string()),
                internal_id: Some("search".to_string()),
                children: Vec::new(),
            },
        ]
    }

    /// The keys a client must read are the keys the server writes.
    ///
    /// This pins the two names the CLI had guessed wrong — a `key` and a
    /// `description` that never existed — so the guess cannot come back.
    #[test]
    fn the_wire_keys_are_name_and_hint_not_key_and_description() {
        let value = serde_json::to_value(&tree()[1]).expect("serialize");
        let object = value.as_object().expect("node is an object");
        assert!(object.contains_key("name"), "the identity key is `name`");
        assert!(object.contains_key("hint"), "the description key is `hint`");
        assert!(
            !object.contains_key("key") && !object.contains_key("description"),
            "`key` / `description` never existed on this wire; a client that \
             reads them gets an empty listing and a fabricated 'not found'"
        );
    }

    /// A namespaced tool must be reachable by its canonical name.
    ///
    /// `session_new` is not a top-level node — it lives under the `session`
    /// namespace as the child `new` — so a lookup that scans only the roots
    /// answers "not found" for most of the catalogue.
    #[test]
    fn find_reaches_namespaced_children_by_canonical_name() {
        let nodes = tree();
        assert!(matches!(
            CommandTreeNode::find(&nodes, "search"),
            Some(CommandMatch::Top(_))
        ));
        for spelling in ["session_new", "session new", "new", "/session_new"] {
            let found = CommandTreeNode::find(&nodes, spelling)
                .unwrap_or_else(|| panic!("`{spelling}` should resolve"));
            assert_eq!(found.internal_id(), Some("session_new"));
            assert_eq!(found.hint(), "Start a new session");
        }
        assert!(CommandTreeNode::find(&nodes, "no_such_tool").is_none());
    }

    /// A namespace node omits the three leaf-only keys and a leaf omits
    /// `children`; both must still parse back.
    #[test]
    fn the_omitted_keys_round_trip() {
        let json = serde_json::to_value(CommandListResponse { commands: tree() }).expect("ser");
        let namespace = json["commands"][0].as_object().expect("object");
        assert!(!namespace.contains_key("param_hint"));
        assert!(!namespace.contains_key("source_type"));
        assert!(!namespace.contains_key("internal_id"));
        assert!(!json["commands"][1]
            .as_object()
            .expect("object")
            .contains_key("children"));

        let back: CommandListResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.commands.len(), 2);
        assert_eq!(back.commands[0].children.len(), 1);
    }
}
