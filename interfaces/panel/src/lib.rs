//! NSH UI set `aleph/panel` for the shared TSX Panel shell.
//!
//! A thin binding over the runtime-neutral Panel model in `shared-ui-logic`
//! (registry, snapshot projection, layout restore and edits). The TSX shell
//! calls these functions and never re-implements those rules, so Desktop (the
//! embedded AppView) and Web (the wasm package) read one implementation.
//!
//! Values cross into TSX as JSON (`unknown` in TS) so the surface stays a few
//! functions rather than a mirrored type per Rust struct.

use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiCapabilityDeclaration,
    PanelUiCapabilityOrigin, PanelUiCapabilitySnapshot, PanelUiTrust, PANEL_UI_API_MAJOR,
};
use nsh_core::prelude::*;
use nsh_core::JsonValue;
use serde_json::json;
use shared_ui_logic::panel::layout::{
    layout_storage_key, restore, LayoutRepair, LayoutSlot, PanelLayout,
};
use shared_ui_logic::panel::projection::PanelProjection;
use shared_ui_logic::panel::registry::BUILTIN_NAMESPACE;
use shared_ui_logic::prefs::appearance::{theme_tokens, FontScale};
use shared_ui_logic::prefs::commands::{
    rank, resolve_bindings, CommandKey, KeyProblem, KeySource, RankItem,
};
use shared_ui_logic::prefs::{self, Binding, PanelPrefs, PrefsRepair};
use shared_ui_logic::transcript::chat_panel::ChatState;

#[native_module("aleph/panel")]
fn native(registry: &mut Registry) {
    registry.module::<PanelModel>();
    registry.module::<NebulaModel>();
    registry.class::<ChatModel>();
}

/// The transcript state for one mounted Chat tab.
#[class]
pub struct ChatModel {
    state: ChatState,
}

#[class]
impl ChatModel {
    /// Constructs an independent transcript for the mounted tab.
    pub fn new() -> Self {
        Self {
            state: ChatState::default(),
        }
    }

    /// Selects a session and clears all provisional rows.
    pub fn reset(&mut self, session: String) {
        self.state.reset(session);
    }

    /// Adds the optimistic user row and opens the early-event buffer.
    pub fn begin_send(&mut self, text: String, now_ms: u64) -> JsonValue {
        self.state.begin_send(text, now_ms)
    }

    /// Reconciles the optimistic row with the acknowledged run, if any.
    pub fn resolve_send(&mut self, session: String, run: Option<String>, now_ms: u64) -> JsonValue {
        self.state.resolve_send(session, run, now_ms)
    }

    /// Applies one typed event and returns only the affected rows.
    pub fn accept(&mut self, event: JsonValue) -> JsonValue {
        self.state.accept(event)
    }

    /// Marks the live suffix incomplete when the host reports an interruption.
    pub fn interrupt(&mut self) -> JsonValue {
        self.state.interrupt()
    }

    /// Allows composing after an interrupted run without claiming it completed.
    pub fn resume_composing(&mut self) -> JsonValue {
        self.state.resume_composing()
    }

    /// Returns the visible snapshot after reconciling committed history.
    pub fn replace_history(&mut self, history: JsonValue, details: JsonValue) -> JsonValue {
        self.state.replace_history(history, details)
    }

    /// Joins an older window without changing the live run.
    pub fn prepend_history(&mut self, history: JsonValue, requested_before: String) -> JsonValue {
        self.state.prepend_history(history, requested_before)
    }

    /// Updates the details of already represented runs, independently of history reads.
    pub fn apply_history_details(&mut self, run_ids: Vec<String>, details: JsonValue) -> JsonValue {
        self.state.apply_history_details(run_ids, details)
    }

    /// Show the last measured request without imputing provider usage.
    pub fn context_view(&self, breakdown: JsonValue) -> JsonValue {
        serde_json::from_value(breakdown).map_or(JsonValue::Null, |report| {
            shared_ui_logic::transcript::panel_context_view(&report)
        })
    }

    /// Layout only the bounded, inert flowchart subset.
    pub fn mermaid_view(&self, source: String) -> JsonValue {
        use shared_ui_logic::transcript::mermaid::{layout_flowchart, FlowchartFallback};
        match layout_flowchart(&source) {
            Ok(scene) => json!({"scene": scene}),
            Err(reason) => json!({"reason": match reason {
                FlowchartFallback::SourceLimit | FlowchartFallback::NodeLimit |
                FlowchartFallback::EdgeLimit | FlowchartFallback::LabelLimit |
                FlowchartFallback::CanvasLimit => "diagram exceeds limits",
                _ => "unsupported diagram",
            }}),
        }
    }

    /// Visible line numbers and word spans from Aleph's recorded change.
    pub fn diff_view(&self, key: String, expanded: bool) -> JsonValue {
        use shared_ui_logic::transcript::{diff_rows, stats_label, DiffView};

        let changes: Vec<_> = self.state.diff_body(&key).unwrap_or_default().iter().map(|change| {
            let label = stats_label(change);
            match diff_rows(change, expanded) {
                DiffView::Unavailable(_) => {
                    let (label, reason) = if label.starts_with('+') {
                        (label, "Diff unavailable: too large to show".to_owned())
                    } else {
                        (String::new(), label)
                    };
                    json!({"path": change.path, "label": label, "unavailable": reason, "lines": [], "hidden": 0})
                }
                DiffView::Rows(body) => {
                    let lines: Vec<_> = body.rows.iter().map(|line| {
                        let spans: Vec<_> = line.spans.iter().map(|span| {
                            json!({"text": span.text, "emphasis": span.emphasis})
                        }).collect();
                        json!({"old": line.old_no, "new": line.new_no, "tag": line.tag, "spans": spans})
                    }).collect();
                    json!({"path": change.path, "label": label, "unavailable": null, "lines": lines, "hidden": body.hidden_rows})
                }
            }
        }).collect();
        json!(changes)
    }
}

/// One built-in module the TSX shell renders itself.
struct ShellModule {
    tab_id: &'static str,
    label: &'static str,
    icon: &'static str,
    order: i32,
}

/// Tabs this renderer implements in TSX (`interfaces/panel/modules/`). Every
/// other tab in a snapshot targets the DOM module ABI of the Leptos Panel and
/// is shown as unavailable here.
const SHELL_MODULES: [ShellModule; 4] = [
    ShellModule {
        tab_id: "overview",
        label: "Overview",
        icon: "layout-dashboard",
        order: -30,
    },
    ShellModule {
        tab_id: "sessions",
        label: "Sessions",
        icon: "messages-square",
        order: -20,
    },
    ShellModule {
        tab_id: "chat",
        label: "Chat",
        icon: "message-circle",
        order: -15,
    },
    ShellModule {
        tab_id: "capabilities",
        label: "Capabilities",
        icon: "blocks",
        order: -10,
    },
];

const SHELL_PUBLIC_BASE: &str = "shell/";

fn is_shell_module(key: &PanelTabKey) -> bool {
    key.plugin_id == BUILTIN_NAMESPACE
        && (key.tab_id == aleph_protocol::memory_graph::NEBULA_TAB_ID
            || SHELL_MODULES
                .iter()
                .any(|module| module.tab_id == key.tab_id))
}

/// Add the shell's own modules to a Core snapshot as built-in declarations,
/// so they pass the same projection validation as Core's. A key Core already
/// declares keeps Core's descriptor; this renderer still draws it in TSX.
fn with_shell_modules(snapshot: PanelUiCapabilitySnapshot) -> PanelUiCapabilitySnapshot {
    let revision = snapshot.revision;
    let mut declarations = snapshot.declarations;
    for module in &SHELL_MODULES {
        let key = PanelTabKey {
            plugin_id: BUILTIN_NAMESPACE.into(),
            tab_id: module.tab_id.into(),
        };
        if declarations
            .iter()
            .any(|declaration| declaration.descriptor.key == key)
        {
            continue;
        }
        declarations.push(PanelUiCapabilityDeclaration {
            descriptor: PanelTabDescriptor {
                key,
                label: module.label.into(),
                icon: module.icon.into(),
                order: module.order,
                entry: format!("{SHELL_PUBLIC_BASE}{}", module.tab_id),
                api_major: PANEL_UI_API_MAJOR,
                trust: PanelUiTrust::Verified,
                permissions: Vec::new(),
                status: PanelTabStatus::Enabled,
                status_detail: None,
            },
            origin: PanelUiCapabilityOrigin::BuiltIn,
            plugin_version: env!("CARGO_PKG_VERSION").into(),
            asset_integrity: None,
            public_base: SHELL_PUBLIC_BASE.into(),
            source_revision: revision,
        });
    }
    PanelUiCapabilitySnapshot::new(revision, declarations)
}

/// Project a `panel.capabilities.snapshot` result. A missing, malformed or
/// rejected snapshot leaves only the shell's modules and reports why; it
/// never mounts part of a rejected snapshot.
fn project(snapshot: JsonValue) -> (PanelProjection, Option<&'static str>) {
    let empty = || PanelUiCapabilitySnapshot::new(0, Vec::new());
    let parsed = if snapshot.is_null() {
        Ok(empty())
    } else {
        serde_json::from_value::<PanelUiCapabilitySnapshot>(snapshot)
            .map_err(|_| "invalid_snapshot")
    };
    let projected = parsed.and_then(|snapshot| {
        PanelProjection::from_snapshot(with_shell_modules(snapshot))
            .map_err(|error| error.reason_code())
    });
    match projected {
        Ok(projection) => (projection, None),
        Err(reason) => (
            PanelProjection::from_snapshot(with_shell_modules(empty()))
                .expect("the shell's own modules are valid built-ins"),
            Some(reason),
        ),
    }
}

fn status_name(status: &PanelTabStatus) -> &'static str {
    match status {
        PanelTabStatus::Enabled => "enabled",
        PanelTabStatus::Disabled => "disabled",
        PanelTabStatus::Incompatible => "incompatible",
        PanelTabStatus::Failed => "failed",
    }
}

fn renderer(key: &PanelTabKey) -> &'static str {
    if is_shell_module(key) {
        "tsx"
    } else {
        "dom"
    }
}

fn layout_json(layout: &PanelLayout) -> JsonValue {
    let qualified =
        |keys: &[PanelTabKey]| -> Vec<String> { keys.iter().map(PanelTabKey::qualified).collect() };
    json!({
        "order": qualified(&layout.order),
        "hidden": qualified(&layout.hidden),
        "active": layout.active.as_ref().map(PanelTabKey::qualified),
    })
}

/// Parse a qualified `plugin:tab` key; identity segments never contain `:`.
fn parse_key(qualified: &str) -> Option<PanelTabKey> {
    let (plugin_id, tab_id) = qualified.split_once(':')?;
    Some(PanelTabKey {
        plugin_id: plugin_id.into(),
        tab_id: tab_id.into(),
    })
}

/// Apply one edit to a stored layout the shell got from `view`. Returns the
/// new stored form, or `None` when the stored form or the key is not valid.
fn edit(
    stored: &str,
    key: &str,
    apply: impl FnOnce(&mut PanelLayout, &PanelTabKey) -> bool,
) -> Option<String> {
    let mut layout: PanelLayout = serde_json::from_str(stored).ok()?;
    let key = parse_key(key)?;
    apply(&mut layout, &key).then(|| layout.to_json())
}

/// Stored preferences as `restore` reads them, ignoring any repair: the shell
/// only edits the `stored` form it got back from `prefs`.
fn stored_prefs(stored: &str) -> PanelPrefs {
    prefs::restore(Some(stored)).prefs
}

/// The canonical keystroke text, or `None` when `text` is not a keystroke.
fn normalize(text: &str, apple: bool) -> Option<String> {
    nsh_core::keymap::Keystroke::parse_with(text, apple)
        .ok()
        .map(|keystroke| keystroke.to_string())
}

/// A string field of a JSON object, or `""`.
fn text_field<'a>(value: &'a JsonValue, name: &str) -> &'a str {
    value
        .get(name)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
}

/// The Panel model the TSX shell reads: projection, layout and its edits.
#[module]
pub struct PanelModel;

#[module]
impl PanelModel {
    /// The shell's whole view model for one snapshot and one stored layout:
    /// `{ revision, tabs, slots, layout, stored, repair, projectionError }`.
    /// `stored` is what the shell writes back to storage.
    pub fn view(snapshot: JsonValue, stored_layout: Option<String>) -> JsonValue {
        let (projection, projection_error) = project(snapshot);
        let registry = projection.registry();
        let restored = restore(stored_layout.as_deref(), &registry);

        let tabs: Vec<JsonValue> = registry
            .rows()
            .iter()
            .map(|descriptor| {
                json!({
                    "key": descriptor.key.qualified(),
                    "label": descriptor.label,
                    "icon": descriptor.icon,
                    "status": status_name(&descriptor.status),
                    "statusDetail": descriptor.status_detail,
                    "renderer": renderer(&descriptor.key),
                })
            })
            .collect();
        let slots: Vec<JsonValue> = restored
            .slots
            .iter()
            .map(|slot| match slot {
                LayoutSlot::Live(key) => json!({
                    "key": key.qualified(),
                    "state": "live",
                    "renderer": renderer(key),
                }),
                LayoutSlot::Unavailable(key) => json!({
                    "key": key.qualified(),
                    "state": "unavailable",
                    "renderer": renderer(key),
                }),
            })
            .collect();
        let repair = restored.repair.as_ref().map(|repair| match repair {
            LayoutRepair::Corrupt => "corrupt",
            LayoutRepair::UnknownVersion(_) => "unknown_version",
        });

        json!({
            "revision": registry.source_revision(),
            "tabs": tabs,
            "slots": slots,
            "layout": layout_json(&restored.layout),
            "stored": restored.layout.to_json(),
            "repair": repair,
            "projectionError": projection_error,
        })
    }

    /// Storage key for one endpoint and, when the Gateway sent one, the
    /// opaque `subject` from `system.hello`.
    pub fn layout_key(endpoint: String, subject: Option<String>) -> String {
        layout_storage_key(&endpoint, subject.as_deref())
    }

    /// Move a tab one step towards the front (`forward`) or the back.
    pub fn move_tab(stored: String, key: String, forward: bool) -> Option<String> {
        edit(&stored, &key, |layout, key| layout.move_one(key, forward))
    }

    /// Hide or show a tab; hiding the active tab leaves no tab active.
    pub fn set_hidden(stored: String, key: String, hidden: bool) -> Option<String> {
        edit(&stored, &key, |layout, key| layout.set_hidden(key, hidden))
    }

    /// Make a listed, visible tab the active one.
    pub fn select(stored: String, key: String) -> Option<String> {
        edit(&stored, &key, |layout, key| layout.select(key))
    }

    /// Personal preferences read from storage:
    /// `{ appearance, keybindings, stored, repair, theme, fontFactor, density }`.
    /// `stored` is what the shell writes back; `theme` is the `setTheme`
    /// payload and `fontFactor` the host text-size multiplier (`null` for
    /// the default size, which follows the system setting).
    pub fn prefs(stored: Option<String>) -> JsonValue {
        let restored = prefs::restore(stored.as_deref());
        let prefs = &restored.prefs;
        // The default text size follows the system setting, so it sends no
        // factor rather than 1.0 (which would override that setting).
        let font_scale = prefs.appearance.font_scale;
        let font_factor = (font_scale != FontScale::Default).then(|| font_scale.factor());
        let repair = restored.repair.map(|repair| match repair {
            PrefsRepair::Corrupt => "corrupt",
            PrefsRepair::UnknownVersion(_) => "unknown_version",
        });
        json!({
            "appearance": prefs.appearance,
            "keybindings": prefs.keybindings,
            "stored": prefs.to_json(),
            "repair": repair,
            "theme": theme_tokens(&prefs.appearance),
            "fontFactor": font_factor,
            "density": prefs.appearance.density,
        })
    }

    /// Set one appearance axis (`mode`, `accent`, `fontScale`, `roundness`,
    /// `density`). Returns the new stored form, or `None` for an unknown axis
    /// or value.
    pub fn set_appearance(stored: String, axis: String, value: String) -> Option<String> {
        let mut prefs = stored_prefs(&stored);
        prefs::set_appearance(&mut prefs, &axis, &value).then(|| prefs.to_json())
    }

    /// Set one command's key: `null` restores the default, `"cleared"`
    /// removes the key, `{ "key": "<text>" }` sets it. Returns the new stored
    /// form, or `None` when `binding` has none of those shapes. Key text is
    /// not checked here; `resolveBindings` reports invalid keys.
    pub fn set_binding(stored: String, command: String, binding: JsonValue) -> Option<String> {
        let binding = if binding.is_null() {
            None
        } else {
            Some(serde_json::from_value::<Binding>(binding).ok()?)
        };
        let mut prefs = stored_prefs(&stored);
        prefs::set_binding(&mut prefs, &command, binding);
        Some(prefs.to_json())
    }

    /// The canonical form of a keystroke (`ctrl+alt+shift+cmd+<key>`), or
    /// `None` when the text is not one. `apple` makes `mod` mean Command.
    pub fn normalize_keystroke(text: String, apple: bool) -> Option<String> {
        normalize(&text, apple)
    }

    /// Each command's effective key, in input order:
    /// `[{ id, key, source, problem }]` with `source` one of
    /// `default|user|cleared` and `problem` `null`, `"invalid"` or
    /// `{ "conflict": <earlier id> }`. A key with a problem does not apply.
    /// `commands` is `[{ id, defaultKey }]`.
    pub fn resolve_bindings(commands: JsonValue, stored: String, apple: bool) -> JsonValue {
        let listed = commands.as_array().cloned().unwrap_or_default();
        let keys: Vec<CommandKey<'_>> = listed
            .iter()
            .map(|command| CommandKey {
                id: text_field(command, "id"),
                default_key: command.get("defaultKey").and_then(JsonValue::as_str),
            })
            .collect();
        let prefs = stored_prefs(&stored);
        let resolved = resolve_bindings(&keys, &prefs.keybindings, |text| normalize(text, apple));
        let rows: Vec<JsonValue> = resolved
            .into_iter()
            .map(|entry| {
                let source = match entry.source {
                    KeySource::Default => "default",
                    KeySource::User => "user",
                    KeySource::Cleared => "cleared",
                };
                let problem = match entry.problem {
                    None => JsonValue::Null,
                    Some(KeyProblem::Invalid) => json!("invalid"),
                    Some(KeyProblem::Conflict(owner)) => json!({ "conflict": owner }),
                };
                json!({ "id": entry.id, "key": entry.key, "source": source, "problem": problem })
            })
            .collect();
        JsonValue::Array(rows)
    }

    /// Ids of the palette items matching `query`, best first.
    /// `items` is `[{ id, label, keywords? }]`.
    pub fn rank_commands(query: String, items: JsonValue) -> Vec<String> {
        let listed = items.as_array().cloned().unwrap_or_default();
        let rows: Vec<RankItem<'_>> = listed
            .iter()
            .map(|item| RankItem {
                id: text_field(item, "id"),
                label: text_field(item, "label"),
                keywords: item
                    .get("keywords")
                    .and_then(JsonValue::as_array)
                    .map(|words| words.iter().filter_map(JsonValue::as_str).collect())
                    .unwrap_or_default(),
            })
            .collect();
        rank(&query, &rows).into_iter().map(str::to_owned).collect()
    }
}

#[module]
/// Builds non-authoritative graph scene data for the shared NSH renderer.
pub struct NebulaModel;

#[module]
impl NebulaModel {
    /// The graph already came through Core; this is presentation data only.
    pub fn scene(graph: JsonValue) -> JsonValue {
        use aleph_protocol::memory_graph::GraphQueryResponse;
        use shared_ui_logic::nebula::{ForceLayout, NebulaIndex};

        let Ok(graph) = serde_json::from_value::<GraphQueryResponse>(graph) else {
            return JsonValue::Null;
        };
        let index = NebulaIndex::new(&graph.nodes, &graph.edges);
        let ids: Vec<_> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let communities: Vec<_> = graph.nodes.iter().map(|node| node.community_id).collect();
        let mut layout = ForceLayout::new(ids.len(), &index.springs, &communities);
        let mut positions = layout.seed(&ids);
        // The NSH read window is 100 notes. Keep the work bounded per refresh;
        // the classic renderer still animates the full 3D settling process.
        for _ in 0..8 {
            layout.step(&mut positions);
        }
        let points: Vec<_> = positions.iter().map(|point| [point.x, point.y]).collect();
        let edges: Vec<_> = index
            .relations
            .iter()
            .map(|edge| json!({ "from": edge.from, "to": edge.to, "kind": edge.row.kind }))
            .collect();
        json!({"points": points, "edges": edges})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nebula_scene_preserves_similarity_edge_kind() {
        let node =
            |id| json!({"id": id, "path": id, "name": id, "category": "concept", "link_count": 0});
        let scene = NebulaModel::scene(json!({
            "nodes": [node("a"), node("b")],
            "edges": [
                {"from": "a", "to": "b", "kind": "refers"},
                {"from": "b", "to": "a", "kind": "related_similarity"},
                {"from": "missing", "to": "a", "kind": "refers"}
            ]
        }));
        assert_eq!(scene["points"].as_array().unwrap().len(), 2);
        assert_eq!(
            scene["edges"],
            json!([
                {"from": 0, "to": 1, "kind": "refers"},
                {"from": 1, "to": 0, "kind": "related_similarity"}
            ])
        );
    }

    fn community(revision: u64) -> JsonValue {
        json!({
            "revision": revision,
            "declarations": [{
                "descriptor": {
                    "key": { "plugin_id": "community.fixture", "tab_id": "notes" },
                    "label": "Notes",
                    "icon": "note",
                    "order": 5,
                    "entry": "./tabs/community.fixture/1.0.0/notes.js",
                    "api_major": PANEL_UI_API_MAJOR,
                    "trust": "trusted",
                    "permissions": [],
                    "status": "enabled",
                    "status_detail": null
                },
                "origin": "community",
                "plugin_version": "1.0.0",
                "asset_integrity": format!("sha256-{}", "A".repeat(43) + "="),
                "public_base": "./tabs/",
                "source_revision": revision
            }],
            "project_context": { "project_id": null, "generation": 0 }
        })
    }

    #[test]
    fn shell_modules_lead_and_community_tabs_use_the_dom_renderer() {
        let view = PanelModel::view(community(3), None);
        assert_eq!(view["projectionError"], JsonValue::Null);
        assert_eq!(view["revision"], 3);
        let slots: Vec<(String, String)> = view["slots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|slot| {
                (
                    slot["key"].as_str().unwrap().to_owned(),
                    slot["renderer"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            slots,
            vec![
                ("aleph:overview".to_owned(), "tsx".to_owned()),
                ("aleph:sessions".to_owned(), "tsx".to_owned()),
                ("aleph:chat".to_owned(), "tsx".to_owned()),
                ("aleph:capabilities".to_owned(), "tsx".to_owned()),
                ("community.fixture:notes".to_owned(), "dom".to_owned()),
            ]
        );
        assert_eq!(view["layout"]["active"], "aleph:overview");
    }

    #[test]
    fn only_an_enabled_and_valid_nebula_declaration_mounts() {
        let mut snapshot = community(3);
        let declaration = &mut snapshot["declarations"][0];
        declaration["origin"] = json!("builtin");
        declaration["public_base"] = json!("shell/");
        declaration["asset_integrity"] = JsonValue::Null;
        declaration["descriptor"]["key"] =
            json!({ "plugin_id": "aleph", "tab_id": "memory-nebula" });
        declaration["descriptor"]["entry"] = json!("shell/memory-nebula");
        declaration["descriptor"]["trust"] = json!("verified");
        let has_nebula = |view: &JsonValue| {
            view["slots"].as_array().unwrap().iter().any(|slot| {
                slot["key"] == "aleph:memory-nebula"
                    && slot["state"] == "live"
                    && slot["renderer"] == "tsx"
            })
        };
        let view = PanelModel::view(snapshot.clone(), None);
        assert_eq!(view["projectionError"], JsonValue::Null);
        assert!(has_nebula(&view));
        let stored = view["stored"].as_str().unwrap().to_owned();
        assert!(!has_nebula(&PanelModel::view(
            JsonValue::Null,
            Some(stored.clone())
        )));
        snapshot["declarations"][0]["descriptor"]["status"] = json!("disabled");
        let view = PanelModel::view(snapshot.clone(), Some(stored.clone()));
        assert_eq!(view["projectionError"], JsonValue::Null);
        assert!(!has_nebula(&view));
        snapshot["declarations"][0]["descriptor"]["trust"] = json!("trusted");
        let view = PanelModel::view(snapshot, Some(stored));
        assert_eq!(view["projectionError"], "unacceptable_trust");
        assert!(!has_nebula(&view));
    }

    #[test]
    fn rejected_snapshot_keeps_only_the_shell_and_reports_why() {
        let mut bad = community(3);
        bad["declarations"][0]["source_revision"] = json!(2);
        let view = PanelModel::view(bad, None);
        assert_eq!(view["projectionError"], "revision_mismatch");
        assert_eq!(view["slots"].as_array().unwrap().len(), 4);

        let view = PanelModel::view(json!({ "nope": true }), None);
        assert_eq!(view["projectionError"], "invalid_snapshot");
    }

    #[test]
    fn a_stored_tab_that_disappears_stays_unavailable() {
        let first = PanelModel::view(community(3), None);
        let stored = first["stored"].as_str().unwrap().to_owned();
        let view = PanelModel::view(JsonValue::Null, Some(stored));
        let notes = view["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["key"] == "community.fixture:notes")
            .unwrap()
            .clone();
        assert_eq!(notes["state"], "unavailable");
    }

    #[test]
    fn edits_round_trip_through_the_stored_form() {
        let view = PanelModel::view(JsonValue::Null, None);
        let stored = view["stored"].as_str().unwrap().to_owned();
        let moved =
            PanelModel::move_tab(stored.clone(), "aleph:capabilities".into(), true).unwrap();
        let hidden = PanelModel::set_hidden(moved, "aleph:overview".into(), true).unwrap();
        let view = PanelModel::view(JsonValue::Null, Some(hidden.clone()));
        assert_eq!(
            view["layout"]["order"],
            json!([
                "aleph:overview",
                "aleph:sessions",
                "aleph:capabilities",
                "aleph:chat"
            ])
        );
        assert_eq!(view["layout"]["hidden"], json!(["aleph:overview"]));
        assert_eq!(view["layout"]["active"], "aleph:sessions");

        assert_eq!(
            PanelModel::select(hidden.clone(), "aleph:overview".into()),
            None
        );
        assert_eq!(PanelModel::select(hidden.clone(), "no-colon".into()), None);
        assert_eq!(
            PanelModel::select("{".into(), "aleph:sessions".into()),
            None
        );
    }

    #[test]
    fn layout_key_includes_the_subject_only_when_present() {
        assert_eq!(
            PanelModel::layout_key("local".into(), Some("ab".into())),
            "aleph.panel.layout.v1:local:ab"
        );
        assert_eq!(
            PanelModel::layout_key("local".into(), None),
            "aleph.panel.layout.v1:local"
        );
    }

    fn default_stored() -> String {
        PanelPrefs::default().to_json()
    }

    #[test]
    fn prefs_of_nothing_is_default() {
        let prefs = PanelModel::prefs(None);
        assert_eq!(prefs["repair"], JsonValue::Null);
        assert_eq!(prefs["stored"], default_stored());
        assert_eq!(prefs["appearance"]["accent"], "mauve");
        assert_eq!(prefs["fontFactor"], JsonValue::Null);
        assert_eq!(prefs["density"], "compact");
        assert!(prefs["theme"]["cssVars"]["dark"]["primary"].is_string());

        let corrupt = PanelModel::prefs(Some("{".into()));
        assert_eq!(corrupt["repair"], "corrupt");
        assert_eq!(corrupt["stored"], default_stored());
    }

    #[test]
    fn set_appearance_rejects_unknown_axis() {
        let stored = PanelModel::set_appearance(default_stored(), "accent".into(), "ocean".into())
            .expect("known axis");
        assert_eq!(
            PanelModel::prefs(Some(stored.clone()))["appearance"]["accent"],
            "ocean"
        );
        assert_eq!(
            PanelModel::set_appearance(stored.clone(), "material".into(), "liquid".into()),
            None
        );
        assert_eq!(
            PanelModel::set_appearance(stored, "fontScale".into(), "huge".into()),
            None
        );
    }

    #[test]
    fn set_binding_null_restores_default() {
        let cleared =
            PanelModel::set_binding(default_stored(), "shell.palette".into(), json!("cleared"))
                .expect("cleared is a binding");
        assert_ne!(cleared, default_stored());
        let keyed =
            PanelModel::set_binding(cleared, "shell.palette".into(), json!({ "key": "mod+j" }))
                .expect("a key is a binding");
        assert_eq!(
            PanelModel::prefs(Some(keyed.clone()))["keybindings"]["shell.palette"]["key"],
            "mod+j"
        );
        let restored =
            PanelModel::set_binding(keyed.clone(), "shell.palette".into(), JsonValue::Null);
        assert_eq!(restored.as_deref(), Some(default_stored().as_str()));
        assert_eq!(
            PanelModel::set_binding(keyed, "shell.palette".into(), json!(42)),
            None
        );
    }

    #[test]
    fn normalize_treats_mod_as_cmd_on_apple() {
        let apple = PanelModel::normalize_keystroke("mod+K".into(), true);
        assert!(apple.is_some());
        assert_eq!(apple, PanelModel::normalize_keystroke("cmd+k".into(), true));
        assert_ne!(
            apple,
            PanelModel::normalize_keystroke("mod+k".into(), false)
        );
        assert_eq!(PanelModel::normalize_keystroke("mod+".into(), true), None);
    }

    #[test]
    fn resolve_reports_conflict_and_invalid() {
        let commands = json!([
            { "id": "shell.palette", "defaultKey": "mod+k" },
            { "id": "shell.settings", "defaultKey": "mod+," },
            { "id": "shell.reconnect" },
        ]);
        let stored = PanelModel::set_binding(
            default_stored(),
            "shell.settings".into(),
            json!({ "key": "cmd+K" }),
        )
        .and_then(|stored| {
            PanelModel::set_binding(
                stored,
                "shell.reconnect".into(),
                json!({ "key": "not a key" }),
            )
        })
        .expect("bindings apply");
        let rows = PanelModel::resolve_bindings(commands, stored, true);
        assert_eq!(rows[0]["key"], "cmd+k");
        assert_eq!(rows[0]["source"], "default");
        assert_eq!(rows[0]["problem"], JsonValue::Null);
        assert_eq!(rows[1]["source"], "user");
        assert_eq!(rows[1]["problem"], json!({ "conflict": "shell.palette" }));
        assert_eq!(rows[2]["key"], JsonValue::Null);
        assert_eq!(rows[2]["problem"], "invalid");
    }

    #[test]
    fn rank_commands_filters() {
        let items = json!([
            { "id": "open:chat", "label": "Open Chat" },
            { "id": "shell.settings", "label": "Settings", "keywords": ["preferences"] },
        ]);
        assert_eq!(
            PanelModel::rank_commands("pref".into(), items.clone()),
            vec!["shell.settings".to_owned()]
        );
        assert_eq!(PanelModel::rank_commands(String::new(), items).len(), 2);
    }
}
