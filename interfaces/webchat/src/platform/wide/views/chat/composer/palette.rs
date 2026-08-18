//! Slash-command palette — types, pure entry builder, and the popup view.
//!
//! Decoupled from the composer orchestrator so the data plane (palette
//! entries) and the render plane (the popup `<Show>` block) live next to
//! the types they operate on. The pure `build_palette_entries` is free of
//! i18n macro expansion (labels travel in via [`PaletteLabels`]) so it
//! can be unit-tested without a Leptos runtime.

use leptos::prelude::*;

/// A single command entry parsed from the Gateway `commands.list` reply.
///
/// Supports a tree shape: a *namespace* command (e.g. `/session`) carries
/// `children`, each of which is a leaf or a nested namespace.
#[derive(Clone, Debug)]
pub(super) struct CommandInfo {
    pub key: String,
    pub description: String,
    pub is_namespace: bool,
    pub param_hint: Option<String>,
    pub children: Vec<Self>,
}

/// Parse a single command from the JSON reply (recursive for children).
///
/// Accepts both the modern `{name,hint,children}` shape and the legacy
/// flat `{key,description}` shape — the daemon used to emit the latter
/// before the namespace tree landed.
pub(super) fn parse_command_info(item: &serde_json::Value) -> Option<CommandInfo> {
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("key").and_then(|v| v.as_str()))?;
    let hint = item
        .get("hint")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("description").and_then(|v| v.as_str()))
        .unwrap_or("");
    let is_namespace = item
        .get("is_namespace")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let param_hint = item
        .get("param_hint")
        .and_then(|v| v.as_str())
        .map(String::from);
    let children = item
        .get("children")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_command_info).collect())
        .unwrap_or_default();
    Some(CommandInfo {
        key: name.to_string(),
        description: hint.to_string(),
        is_namespace,
        param_hint,
        children,
    })
}

/// A single row rendered in the palette popup.
///
/// `is_back` and `is_namespace` are mutually exclusive sentinels — the
/// row is rendered differently for each (back arrow, leaf, ▸ namespace).
#[derive(Clone, Debug)]
pub(super) struct PaletteEntry {
    pub label: String,
    pub full_command: String,
    pub description: String,
    pub is_namespace: bool,
    pub is_back: bool,
}

/// i18n labels for the "back" pseudo-entry, threaded into the pure
/// builder so it stays free of macro expansion (and unit-testable).
#[derive(Clone, Debug)]
pub(super) struct PaletteLabels {
    pub back: String,
    pub back_desc: String,
}

/// Build the visible palette entries from the (possibly nested) command
/// catalogue, the current namespace context, and a typed filter query.
///
/// Pure — no signals, no i18n macros, no DOM. Suitable for unit tests.
pub(super) fn build_palette_entries(
    cmds: &[CommandInfo],
    namespace: &Option<String>,
    query: &str,
    labels: &PaletteLabels,
) -> Vec<PaletteEntry> {
    let mut entries = Vec::new();
    if let Some(ns) = namespace {
        // Inside a namespace — show "back" + children.
        entries.push(PaletteEntry {
            label: labels.back.clone(),
            full_command: "/".to_string(),
            description: labels.back_desc.clone(),
            is_namespace: false,
            is_back: true,
        });
        if let Some(parent) = cmds.iter().find(|c| &c.key == ns) {
            let q = query.to_lowercase();
            for child in &parent.children {
                let full_cmd = format!("/{} {} ", ns, child.key);
                let desc = if let Some(ref ph) = child.param_hint {
                    format!("{} {}", child.description, ph)
                } else {
                    child.description.clone()
                };
                if query.is_empty()
                    || child.key.to_lowercase().contains(&q)
                    || desc.to_lowercase().contains(&q)
                {
                    entries.push(PaletteEntry {
                        label: child.key.clone(),
                        full_command: full_cmd,
                        description: desc,
                        is_namespace: false,
                        is_back: false,
                    });
                }
            }
        }
        return entries;
    }
    // Root level — show namespaces and independent commands.
    let q = query.to_lowercase();
    for cmd in cmds {
        let desc = if let Some(ref ph) = cmd.param_hint {
            format!("{} {}", cmd.description, ph)
        } else {
            cmd.description.clone()
        };
        if !query.is_empty()
            && !cmd.key.to_lowercase().contains(&q)
            && !desc.to_lowercase().contains(&q)
        {
            continue;
        }
        let indicator = if cmd.is_namespace { " \u{25B8}" } else { "" };
        entries.push(PaletteEntry {
            label: cmd.key.clone(),
            full_command: if cmd.is_namespace {
                String::new() // namespaces drill in; we never insert the bare prefix
            } else {
                format!("/{} ", cmd.key)
            },
            description: format!("{desc}{indicator}"),
            is_namespace: cmd.is_namespace,
            is_back: false,
        });
    }
    entries
}

/// Slash-command palette popup. Lives above the composer textarea and
/// is dismissed by Escape, leaf selection, or text divergence.
#[component]
pub(super) fn SlashPaletteView(
    show: RwSignal<bool>,
    palette_entries: RwSignal<Vec<PaletteEntry>>,
    selected_index: RwSignal<usize>,
    current_namespace: RwSignal<Option<String>>,
    on_select: Callback<PaletteEntry>,
) -> impl IntoView {
    // Follow ↑/↓ selection: scroll the highlighted row into view when it
    // leaves the capped viewport.
    let list_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let idx = selected_index.get();
        let _ = palette_entries.get(); // re-run when entries change (filter / drill resets idx)
        if let Some(el) = list_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            crate::views::chat::list_scroll::reveal_selected_row(el, idx);
        }
    });

    view! {
        <Show when=move || show.get() && !palette_entries.get().is_empty()>
            <div
                node_ref=list_ref
                class="glass animate-pop-in absolute bottom-full inset-x-0 mb-2 z-50
                       rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                       max-h-[200px] overflow-y-auto"
            >
                // Namespace breadcrumb header.
                <Show when=move || current_namespace.get().is_some()>
                    <div class="px-3 py-1.5 text-xs text-text-tertiary border-b border-border">
                        "/" {move || current_namespace.get().unwrap_or_default()} " >"
                    </div>
                </Show>
                <For
                    each=move || {
                        palette_entries.get().into_iter().enumerate().collect::<Vec<_>>()
                    }
                    key=|(_, entry)| format!("{}:{}", entry.label, entry.is_back)
                    children=move |(idx, entry)| {
                        let label = entry.label.clone();
                        let desc = entry.description.clone();
                        let entry_for_click = entry.clone();
                        let is_back = entry.is_back;
                        let is_ns = entry.is_namespace;
                        let ns_ctx = current_namespace.get_untracked();
                        let pidx = idx.to_string();
                        view! {
                            <button
                                data-pidx=pidx
                                class=move || {
                                    let base = "w-full text-left px-3 py-2 flex items-baseline gap-2 text-sm transition-colors";
                                    if selected_index.get() == idx {
                                        format!("{base} bg-primary/10 text-primary")
                                    } else {
                                        format!("{base} hover:bg-surface-sunken text-text-primary")
                                    }
                                }
                                on:mousedown=move |ev| {
                                    ev.prevent_default(); // keep textarea focus
                                    on_select.run(entry_for_click.clone());
                                }
                            >
                                {if is_back {
                                    view! {
                                        <span class="text-text-tertiary shrink-0">{label}</span>
                                    }.into_any()
                                } else if ns_ctx.is_some() {
                                    view! {
                                        <span class="font-medium shrink-0">{label}</span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <span class="font-medium shrink-0">"/" {label}</span>
                                    }.into_any()
                                }}
                                <span class="text-text-secondary text-xs truncate">{desc}</span>
                                {if is_ns {
                                    Some(view! {
                                        <span class="ml-auto text-text-tertiary text-xs shrink-0">"\u{25B8}"</span>
                                    })
                                } else {
                                    None
                                }}
                            </button>
                        }
                    }
                />
            </div>
        </Show>
    }
}

/// Is `text` exactly the bare `/doctor` command (trimmed, case-insensitive)?
///
/// A predicate, not an expansion: the prompt it stands for is copy
/// (`chat.doctor_detect_prompt`, the read-only mirror of the `f`-hotkey
/// `chat.doctor_repair_prompt`), and copy is resolved by the caller, which has
/// the i18n context this pure fn does not. The prompt itself goes through the
/// normal LLM pipeline rather than the literal slash command, which would hit
/// the deterministic fast path and never reach a model. Args are not supported
/// in v1 — `/doctor <anything>` does not match.
pub(super) fn is_doctor_command(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("/doctor")
}

/// Static palette entry so `/doctor` is discoverable when the user types `/`,
/// even though it is intercepted client-side (not a Gateway tool-backed
/// command). Merged into the fetched catalogue by `composer::fetch_commands`.
pub(super) fn doctor_command_info(description: String) -> CommandInfo {
    CommandInfo {
        key: "doctor".to_string(),
        description,
        is_namespace: false,
        param_hint: None,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_labels() -> PaletteLabels {
        PaletteLabels {
            back: "back".into(),
            back_desc: "to top".into(),
        }
    }

    fn leaf(key: &str, desc: &str) -> CommandInfo {
        CommandInfo {
            key: key.into(),
            description: desc.into(),
            is_namespace: false,
            param_hint: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn root_lists_all_when_query_empty() {
        let cmds = vec![leaf("clear", "wipe"), leaf("help", "show help")];
        let entries = build_palette_entries(&cmds, &None, "", &mk_labels());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].full_command, "/clear ");
        assert!(!entries.iter().any(|e| e.is_back));
    }

    #[test]
    fn root_filters_by_substring() {
        let cmds = vec![leaf("clear", "wipe"), leaf("help", "show help")];
        let entries = build_palette_entries(&cmds, &None, "hel", &mk_labels());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "help");
    }

    #[test]
    fn namespace_emits_back_row_first() {
        let parent = CommandInfo {
            key: "session".into(),
            description: "manage sessions".into(),
            is_namespace: true,
            param_hint: None,
            children: vec![leaf("new", "start a new session")],
        };
        let entries = build_palette_entries(&[parent], &Some("session".into()), "", &mk_labels());
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_back);
        assert_eq!(entries[1].label, "new");
        assert_eq!(entries[1].full_command, "/session new ");
    }

    #[test]
    fn namespace_filters_children_by_query() {
        let parent = CommandInfo {
            key: "session".into(),
            description: "manage".into(),
            is_namespace: true,
            param_hint: None,
            children: vec![leaf("new", "start"), leaf("list", "enumerate")],
        };
        let entries =
            build_palette_entries(&[parent], &Some("session".into()), "lis", &mk_labels());
        assert_eq!(entries.len(), 2); // back + list
        assert_eq!(entries[1].label, "list");
    }

    #[test]
    fn namespace_emits_only_back_when_query_matches_nothing() {
        let parent = CommandInfo {
            key: "session".into(),
            description: "manage".into(),
            is_namespace: true,
            param_hint: None,
            children: vec![leaf("new", "start")],
        };
        let entries = build_palette_entries(
            &[parent],
            &Some("session".into()),
            "zzznotfound",
            &mk_labels(),
        );
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_back);
    }

    #[test]
    fn namespace_drilldown_marker_present_at_root() {
        let parent = CommandInfo {
            key: "session".into(),
            description: "manage".into(),
            is_namespace: true,
            param_hint: None,
            children: vec![leaf("new", "start")],
        };
        let entries = build_palette_entries(&[parent], &None, "", &mk_labels());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_namespace);
        assert_eq!(entries[0].full_command, ""); // empty — caller drills instead of inserting
        assert!(entries[0].description.ends_with('\u{25B8}'));
    }

    #[test]
    fn param_hint_is_appended_to_description() {
        let cmds = vec![CommandInfo {
            key: "rename".into(),
            description: "rename session".into(),
            is_namespace: false,
            param_hint: Some("<new-name>".into()),
            children: Vec::new(),
        }];
        let entries = build_palette_entries(&cmds, &None, "", &mk_labels());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].description.contains("<new-name>"));
    }

    #[test]
    fn parse_handles_legacy_flat_keys() {
        let v = serde_json::json!({"key": "clear", "description": "wipe"});
        let cmd = parse_command_info(&v).unwrap();
        assert_eq!(cmd.key, "clear");
        assert_eq!(cmd.description, "wipe");
        assert!(!cmd.is_namespace);
        assert!(cmd.children.is_empty());
    }

    #[test]
    fn parse_handles_modern_shape_with_children() {
        let v = serde_json::json!({
            "name": "session",
            "hint": "manage",
            "is_namespace": true,
            "children": [{"name": "new", "hint": "start"}]
        });
        let cmd = parse_command_info(&v).unwrap();
        assert_eq!(cmd.key, "session");
        assert!(cmd.is_namespace);
        assert_eq!(cmd.children.len(), 1);
        assert_eq!(cmd.children[0].key, "new");
    }

    #[test]
    fn expand_doctor_matches_only_bare_command() {
        assert!(is_doctor_command("/doctor"));
        assert!(is_doctor_command("/Doctor")); // case-insensitive
        assert!(is_doctor_command("  /doctor  ")); // trimmed
    }

    #[test]
    fn expand_doctor_rejects_non_matches() {
        assert!(!is_doctor_command("/doctorx"));
        assert!(!is_doctor_command("/doctor now")); // args not supported in v1
        assert!(!is_doctor_command("/doc"));
        assert!(!is_doctor_command("hello"));
        assert!(!is_doctor_command(""));
    }

    #[test]
    fn doctor_command_info_is_a_leaf() {
        let info = doctor_command_info("read-only health check".to_string());
        assert_eq!(info.key, "doctor");
        assert!(!info.is_namespace);
        assert!(info.children.is_empty());
        assert!(!info.description.is_empty());
    }
}
