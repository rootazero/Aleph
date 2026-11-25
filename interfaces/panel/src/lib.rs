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

    /// Returns the visible snapshot after reconciling committed history.
    pub fn replace_history(&mut self, history: JsonValue, details: JsonValue) -> JsonValue {
        self.state.replace_history(history, details)
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
}
