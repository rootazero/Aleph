//! Whiteboard canvas wire-level integration suite (plan Task 20).
//!
//! Three suites, all driving the REAL production pieces from the crate's
//! published surface — an integration test compiles against exactly the view
//! a client sees, which is what makes its green mean something the in-crate
//! unit tests' green cannot (`cargo check` never builds this file; see the
//! §10 criteria about which commands compile which callers):
//!
//! 1. **Full-chain wire check** — call the real `canvas.*` handlers, parse
//!    every response with the `aleph_protocol::canvas` contract types, and
//!    assert key-set equality with the expectation DERIVED from the contract
//!    type itself (serialize the parsed value back). Parsing alone proves a
//!    superset — serde ignores unknown keys — and a literal key list is the
//!    same enumeration mistake one level up (§0).
//! 2. **AI-message-template tool-name resolution guard** — the Panel's
//!    `views/canvas/ai.rs` templates name tools in prose (`canvas`,
//!    `image_generate`). Prose naming a tool is a second copy of that tool's
//!    name with no compiler and no call site (§4.11 round-12): here — the
//!    only place both the templates and the real tool table are visible —
//!    every backtick-quoted name in the templates is resolved against
//!    `BUILTIN_TOOL_DEFINITIONS` ∪ the registry-only registrations, so a
//!    tool rename (or a template naming a tool that never existed) fails by
//!    name instead of costing the model a tool-not-found per generation.
//! 3. **Event visibility end-to-end** — a real `canvas.apply` through the
//!    real store broadcasts a real `CanvasUpdated` frame on a typed bus
//!    subscription; that produced frame (not a hand-built replica) is then
//!    pushed through `EventVisibilityIndex::event_admits_for` for the owner,
//!    a roster member of the linked room, and a stranger. The unit test in
//!    `event_visibility.rs` proves the classifier arm; this proves the frame
//!    the store actually emits carries the fields the classifier reads.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use aleph_protocol::canvas::{
    CanvasApplyParams, CanvasApplyResult, CanvasEnvelope, CanvasList, CanvasOp, CanvasUpdated,
    FracIndex, Shape, ShapeCommon, ShapeStyle, TOPIC,
};
use alephcore::canvas::CanvasStore;
use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
use alephcore::gateway::caller_identity::CALLER_USER;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::event_visibility::EventVisibilityIndex;
use alephcore::gateway::events::GatewayEventFrame;
use alephcore::gateway::handlers::canvas as canvas_rpc;
use alephcore::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use alephcore::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use alephcore::gateway::session_store::SessionStore;
use alephcore::projects::roster;

// ---------------------------------------------------------------------------
// Shared plumbing.
// ---------------------------------------------------------------------------

/// `roster::publish` REPLACES the process-global snapshot (its doc says so),
/// and the tests in this binary run in parallel. The lib's `TEST_GUARD` is
/// `#[cfg(test)]` and thus invisible to an integration test, so this binary
/// carries its own — same reason, same shape.
static ROSTER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn rpc(method: &str, params: Value) -> JsonRpcRequest {
    JsonRpcRequest::with_id(method, Some(params), json!(1))
}

async fn as_user<F: std::future::Future<Output = JsonRpcResponse>>(
    user: &str,
    fut: F,
) -> JsonRpcResponse {
    CALLER_USER.scope(Some(user.to_string()), fut).await
}

fn note_op(id: &str) -> CanvasOp {
    CanvasOp::UpsertShape {
        shape: Shape::Note {
            common: ShapeCommon {
                id: id.to_string(),
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 200.0,
                z: FracIndex::first(),
                parent_id: None,
            },
            style: ShapeStyle::default(),
            text: "wire".to_string(),
        },
    }
}

/// Key set a contract value serializes to — the expectation is derived from
/// the type, never written as a literal list.
fn keys_of<T: serde::Serialize>(v: &T) -> BTreeSet<String> {
    serde_json::to_value(v)
        .expect("contract types serialize")
        .as_object()
        .expect("contract projections are objects")
        .keys()
        .cloned()
        .collect()
}

fn emitted_keys(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .expect("response is an object")
        .keys()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Full-chain wire check.
// ---------------------------------------------------------------------------

/// Every `canvas.*` response, parsed by the client-side contract type and
/// compared key-for-key against that type's own serialization. Covers
/// create / get / list / apply / asset.put / asset.get in one chain so the
/// fixture is the real lifecycle, not six isolated calls.
#[tokio::test]
async fn the_wire_chain_round_trips_every_canvas_response_through_the_contract() {
    // Guard outlives every assertion — a dropped guard deletes the tree
    // under a live store (§0 scratch rule).
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(CanvasStore::new(dir.path().to_path_buf()));

    // --- create: a CanvasEnvelope, exactly -----------------------------
    let created = as_user(
        "u-wire",
        canvas_rpc::handle_create(
            rpc("canvas.create", json!({ "title": "wire" })),
            store.clone(),
        ),
    )
    .await
    .result
    .expect("create succeeds");
    let created_env: CanvasEnvelope =
        serde_json::from_value(created.clone()).expect("create parses as CanvasEnvelope");
    assert_eq!(
        emitted_keys(&created),
        keys_of(&created_env),
        "canvas.create must emit the CanvasEnvelope key set and nothing else"
    );
    assert_eq!(
        emitted_keys(&created["canvas"]),
        keys_of(&created_env.canvas),
        "the embedded document must emit the CanvasDoc key set and nothing else"
    );
    let id = created_env.canvas.id.clone();

    // --- apply: a CanvasApplyResult, exactly ---------------------------
    let apply_params = serde_json::to_value(CanvasApplyParams {
        canvas_id: id.clone(),
        base_revision: created_env.canvas.revision,
        ops: vec![note_op("n1")],
    })
    .expect("params serialize");
    let applied = as_user(
        "u-wire",
        canvas_rpc::handle_apply(rpc("canvas.apply", apply_params), store.clone()),
    )
    .await
    .result
    .expect("apply succeeds");
    let applied_parsed: CanvasApplyResult =
        serde_json::from_value(applied.clone()).expect("apply parses as CanvasApplyResult");
    assert_eq!(emitted_keys(&applied), keys_of(&applied_parsed));
    assert_eq!(applied_parsed.revision, created_env.canvas.revision + 1);

    // --- asset.put / asset.get: their results, exactly ------------------
    use base64::Engine as _;
    let png_bytes: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 9, 9, 9];
    let data = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    let put = as_user(
        "u-wire",
        canvas_rpc::handle_asset_put(
            rpc(
                "canvas.asset.put",
                json!({ "canvas_id": id, "mime_type": "image/png", "data": data }),
            ),
            store.clone(),
        ),
    )
    .await
    .result
    .expect("asset.put succeeds");
    let put_parsed: aleph_protocol::canvas::AssetPutResult =
        serde_json::from_value(put.clone()).expect("asset.put parses as AssetPutResult");
    assert_eq!(emitted_keys(&put), keys_of(&put_parsed));

    let got_asset = as_user(
        "u-wire",
        canvas_rpc::handle_asset_get(
            rpc(
                "canvas.asset.get",
                json!({ "canvas_id": id, "asset_id": put_parsed.asset_id }),
            ),
            store.clone(),
        ),
    )
    .await
    .result
    .expect("asset.get succeeds");
    let got_asset_parsed: aleph_protocol::canvas::AssetGetResult =
        serde_json::from_value(got_asset.clone()).expect("asset.get parses as AssetGetResult");
    assert_eq!(emitted_keys(&got_asset), keys_of(&got_asset_parsed));
    assert_eq!(got_asset_parsed.data, data, "asset bytes round-trip");

    // --- get: the envelope again, now with the capability asset base ----
    let got = as_user(
        "u-wire",
        canvas_rpc::handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
    )
    .await
    .result
    .expect("get succeeds");
    let got_env: CanvasEnvelope =
        serde_json::from_value(got.clone()).expect("get parses as CanvasEnvelope");
    assert_eq!(emitted_keys(&got), keys_of(&got_env));
    assert_eq!(emitted_keys(&got["canvas"]), keys_of(&got_env.canvas));
    // The Panel resolves `<image href>` against `{asset_base}/{asset_id}`,
    // so the base's shape is wire contract, not decoration.
    let base = got_env.asset_base.expect("canvas.get mints an asset base");
    assert!(
        base.starts_with("/canvas-asset/") && base.ends_with(&format!("/{id}")),
        "asset_base must be /canvas-asset/<cap>/<canvas_id>, got {base}"
    );

    // --- list: top level and the row, exactly ---------------------------
    let listed = as_user(
        "u-wire",
        canvas_rpc::handle_list(rpc("canvas.list", json!({})), store.clone()),
    )
    .await
    .result
    .expect("list succeeds");
    let listed_parsed: CanvasList =
        serde_json::from_value(listed.clone()).expect("list parses as CanvasList");
    assert_eq!(emitted_keys(&listed), keys_of(&listed_parsed));
    assert_eq!(
        emitted_keys(&listed["canvases"][0]),
        keys_of(&listed_parsed.canvases[0]),
        "canvas.list rows must emit the CanvasRow key set and nothing else"
    );

    drop(dir);
}

// ---------------------------------------------------------------------------
// 2. AI-message-template tool-name resolution guard.
// ---------------------------------------------------------------------------

fn repo_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Read a source file, normalize CRLF (§10: a `\n`-anchored separator never
/// matches on a CRLF checkout), and drop comment lines — the scanner judges
/// code; comments are documentation, and a doc mention of a tool name must
/// neither satisfy nor pollute the scan (§0 "扫描器判的是代码").
fn read_source_without_comments(rel: &str) -> String {
    let path = repo_path(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}) — if the file moved, this guard must move \
             with it, because it is the only place the Panel's tool-name \
             prose is resolved against the real tool table",
            path.display()
        )
    });
    src.replace('\r', "")
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Backtick-quoted tool-name-shaped tokens: split on backticks, keep the
/// inside segments, keep only `[a-z_][a-z0-9_]*` (registry names are snake
/// case — `[canvas]`-style doc links and type names never match).
fn backticked_tool_names(text: &str) -> BTreeSet<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
        .map(str::to_string)
        .collect()
}

/// The set of tool names that actually reach the model: the static catalog
/// plus every registry registration. The registry half is derived from the
/// registration sources exactly the way the in-crate census
/// (`every_registered_core_tool_is_accounted`) reads them — a hand-kept
/// second list here would be the very failure that census exists to prevent.
fn real_tool_names() -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BUILTIN_TOOL_DEFINITIONS
        .iter()
        .map(|d| d.name.to_string())
        .collect();

    // `reg(` registrations: the opener sits on its own line, the name
    // literal on the next (a rustfmt-stable format the census pins).
    for rel in [
        "src/executor/builtin_registry/builder/core_tools.rs",
        "src/executor/builtin_registry/builder/optional_tools.rs",
    ] {
        let src = read_source_without_comments(rel);
        let mut awaiting_name = false;
        for line in src.lines().map(str::trim) {
            if line == "reg(" {
                awaiting_name = true;
                continue;
            }
            if awaiting_name {
                if let Some(rest) = line.strip_prefix('"') {
                    if let Some(name) = rest.split('"').next() {
                        names.insert(name.to_string());
                        awaiting_name = false;
                    }
                }
            }
        }
    }

    // Constructor-direct `tools.insert("name"...)` sites (the third
    // registration shape the census covers via REG_INSERTED_NAMES).
    let constructor_dir = repo_path("src/executor/builtin_registry/builder/constructor");
    for entry in std::fs::read_dir(&constructor_dir)
        .unwrap_or_else(|e| panic!("cannot list {}: {e}", constructor_dir.display()))
    {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .replace('\r', "");
        for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
            if let Some(after) = line.split("tools.insert(\"").nth(1) {
                if let Some(name) = after.split('"').next() {
                    names.insert(name.to_string());
                }
            }
        }
    }

    assert!(
        names.len() > 30,
        "only {} tool names resolved from the catalog + registration sources \
         — the scan is looking at the wrong shape and certifies nothing",
        names.len()
    );
    names
}

/// Every tool the Panel's canvas AI templates name in prose resolves to a
/// real registered tool. The Panel-side unit tests pin the exact spelling;
/// this side pins that the spelling names something that exists.
#[test]
fn every_tool_the_panel_canvas_templates_name_resolves_in_the_real_tool_table() {
    let src =
        read_source_without_comments("interfaces/webchat/src/platform/wide/views/canvas/ai.rs");
    // Production prefix only: the `#[cfg(test)]` module's assertion strings
    // quote the same names, and scanning them would let the guard pass on
    // its own test fixtures after the templates stopped naming any tool.
    let production = src
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields a first segment");

    // Non-vacuity: the two template functions must still exist — if the AI
    // flow is redesigned, this guard has to follow it, not silently scan an
    // empty set.
    for template_fn in ["fn generation_message", "fn annotation_message"] {
        assert!(
            production.contains(template_fn),
            "{template_fn} is gone from ai.rs — move this guard to wherever \
             the model-facing canvas templates now live"
        );
    }

    let named = backticked_tool_names(production);
    assert!(
        named.contains("canvas") && named.contains("image_generate"),
        "the templates are contracted (ai.rs module doc) to name `canvas` \
         and `image_generate`; the scan extracted {named:?} — either the \
         templates changed or the scanner went blind"
    );

    let real = real_tool_names();
    let unresolved: Vec<&String> = named.iter().filter(|n| !real.contains(*n)).collect();
    assert!(
        unresolved.is_empty(),
        "the Panel's canvas templates tell the model to call {unresolved:?}, \
         but no tool with those names is cataloged or registered — every \
         generation would cost a tool-not-found round-trip. Fix the template \
         in interfaces/webchat/src/platform/wide/views/canvas/ai.rs or \
         register the tool."
    );
}

// ---------------------------------------------------------------------------
// 3. Event visibility end-to-end.
// ---------------------------------------------------------------------------

/// A real apply, through the real handler and store, broadcasts a
/// `CanvasUpdated` frame on the typed bus; the produced frame is then scoped
/// by the real delivery predicate: owner in, roster member in, stranger out
/// — and an operator-ROLE stranger out too (the predicate has no admin arm).
#[tokio::test]
async fn a_real_apply_broadcasts_a_frame_the_visibility_plane_scopes_correctly() {
    let _guard = ROSTER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    roster::publish(roster::RosterSnapshot::from_pairs([
        ("p-wire-room".to_string(), "u-alice".to_string()),
        ("p-wire-room".to_string(), "u-bob".to_string()),
    ]));

    let dir = tempfile::tempdir().expect("tempdir");
    let bus = Arc::new(GatewayEventBus::new());
    let mut rx = bus.subscribe_typed();
    let store = Arc::new(CanvasStore::new(dir.path().to_path_buf()).with_event_bus(bus.clone()));

    // Owner creates a room-linked canvas and applies one batch.
    let created = as_user(
        "u-alice",
        canvas_rpc::handle_create(
            rpc(
                "canvas.create",
                json!({ "title": "wire", "project_id": "p-wire-room" }),
            ),
            store.clone(),
        ),
    )
    .await
    .result
    .expect("create succeeds");
    let created_env: CanvasEnvelope = serde_json::from_value(created).expect("envelope");
    let id = created_env.canvas.id.clone();

    let apply_params = serde_json::to_value(CanvasApplyParams {
        canvas_id: id.clone(),
        base_revision: created_env.canvas.revision,
        ops: vec![note_op("n1")],
    })
    .expect("params serialize");
    let applied = as_user(
        "u-alice",
        canvas_rpc::handle_apply(rpc("canvas.apply", apply_params), store.clone()),
    )
    .await
    .result
    .expect("apply succeeds");
    let new_revision = applied["revision"].as_u64().expect("revision");

    // The frame is published inside the apply's critical section, so it is
    // already buffered by the time the handler returned — drain and find it.
    let mut canvas_frame = None;
    while let Ok(frame) = rx.try_recv() {
        if matches!(&frame, GatewayEventFrame::CanvasUpdated { canvas_id, .. } if *canvas_id == id)
        {
            canvas_frame = Some(frame);
        }
    }
    let frame = canvas_frame.expect("the apply must broadcast a CanvasUpdated frame");
    assert_eq!(frame.topic_name(), TOPIC);

    let GatewayEventFrame::CanvasUpdated {
        revision,
        actor,
        owner_user_id,
        project_id,
        ..
    } = &frame
    else {
        unreachable!("matched above");
    };
    assert_eq!(
        *revision, new_revision,
        "the frame carries the committed revision"
    );
    assert_eq!(actor.as_deref(), Some("u-alice"));
    // The self-report the classifier reads (§4.8 mine H: the frame carries
    // its own scope, no index seeding).
    assert_eq!(owner_user_id.as_deref(), Some("u-alice"));
    assert_eq!(project_id.as_deref(), Some("p-wire-room"));

    // The Panel parses the same payload as the protocol's CanvasUpdated,
    // tolerating the server-side extras.
    let data = serde_json::to_value(&frame).expect("frame serializes");
    let panel_view: CanvasUpdated =
        serde_json::from_value(data.clone()).expect("the Panel-side contract parses the frame");
    assert_eq!(panel_view.revision, new_revision);
    assert_eq!(panel_view.ops.len(), 1, "the ops ride the frame");

    // The delivery predicate over the PRODUCED frame — end to end, exactly
    // the arguments the delivery loop passes.
    let session_dir = tempfile::tempdir().expect("tempdir");
    let session_store: Arc<dyn SessionStore> = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: session_dir.path().to_path_buf(),
            ..Default::default()
        })
        .expect("session store"),
    );
    let index = EventVisibilityIndex::new();
    for (caller, role, admitted, why) in [
        ("u-alice", "member", true, "the owner"),
        (
            "u-bob",
            "member",
            true,
            "a roster member of the linked room",
        ),
        ("u-carol", "member", false, "a stranger"),
        (
            "u-carol",
            "operator",
            false,
            "an operator who is neither owner nor member",
        ),
    ] {
        assert_eq!(
            index
                .event_admits_for(
                    TOPIC,
                    Some(&data),
                    Some(caller),
                    Some(role),
                    &session_store,
                    None,
                )
                .await,
            admitted,
            "{why} ({caller}/{role})"
        );
    }

    drop(session_dir);
    drop(dir);
}
