//! Embedded PTY terminal handlers.
//!
//! JSON-RPC surface over the [`crate::gateway::pty`] subsystem. Most of these
//! handlers are stateless — they reach the process-global [`pty::manager`]
//! accessor (the same pattern user-hooks admin uses) so no boot-time closure
//! wiring is needed. The gateway event bus is attached to the manager once in
//! `GatewayServer::build_router`; live output is streamed to subscribers on the
//! `pty.screen` / `pty.exit` topics (subscribe via `events.subscribe` with
//! pattern `pty.*`).
//!
//! [`handle_spawn`] is the one exception: it needs the live `Config` to read
//! `[policies.terminal]` (the session gate, and the scrollback/session-cap
//! values), so it takes an `Arc<RwLock<Config>>` and is registered
//! separately from its siblings — see its own doc, and
//! `register_pty_handlers` in
//! `src/bin/aleph-server/commands/start/builder/handlers/system.rs`.
//!
//! ## Operator-only, on BOTH faces
//!
//! A PTY is a raw shell: the command policy does not see it and the exec tier
//! does not gate it. `"pty."` is therefore in
//! [`ADMIN_PREFIXES`](crate::gateway::method_admin), and — since 2026-08-08 —
//! also in [`EventScopeGuard::default_rules`](crate::gateway::event_scope::EventScopeGuard::default_rules).
//!
//! The second half was missing for a whole multi-user arc, and this paragraph
//! is why the omission survived: it used to read *"under the LAN-trust model
//! every connection is the implicit owner/operator, so the `pty.*` surface is
//! open to all connections."* That was true when it was written and became
//! false the day roles landed, but it went on describing the subscribe face
//! accurately — because nobody had changed the subscribe face. **A sentence
//! about who may reach a surface has a copy on every face that surface has;
//! closing one face and leaving the sentence is how the other face stays open.**

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::config::Config;
use crate::gateway::pty::{self, SpawnOptions};
use crate::sync_primitives::Arc;

/// The largest terminal geometry `pty.spawn` / `pty.resize` will accept, per
/// axis.
///
/// `Grid` allocates `rows * cols` cells of 16 bytes each in a single `vec!`,
/// twice over on a resize (a whole second grid is built before the swap), and
/// `rows`/`cols` arrive on the wire as bare `u16`. At the type's ceiling that
/// is 65535 x 65535 x 16 B ~= 68 GB in one allocation — and a Rust allocation
/// failure calls `handle_alloc_error`, which **aborts the process**. It does
/// not unwind, so there is no `catch_unwind` and no failed-request arm: the
/// whole daemon goes, including the vault it holds. `pty.resize` is the
/// cheaper way to reach it, because unlike `pty.spawn` it never consults
/// `[policies.terminal] enabled`.
///
/// 1000 because 1000 x 1000 = 10^6 cells = 16 MB, which is a grid a host can
/// lose without noticing, and because a 1000-COLUMN terminal needs a window
/// roughly 7000 px wide at any legible monospace advance — wider than an 8K
/// display, let alone a Panel pane inside one. No real terminal has ever
/// asked for it. The number is deliberately far below "the largest value that
/// would still work": this is a floor against a malformed request, not a
/// capacity plan, and the failure it prevents is unrecoverable while the
/// failure it causes is a legible error message.
///
/// **Refused, never silently clamped.** A client that sends nonsense has to
/// learn it did; a clamp would leave it rendering against dimensions the
/// server never agreed to. Note what the shipped client does and do not copy
/// it: `render::viewport_cells` clamps its own measurement to
/// `f64::from(u16::MAX)` — the largest value this server will *parse*, not one
/// it survives. Client-side clamping is not the enforcement point and cannot
/// be: the bound has to hold for a caller that never ran our JavaScript.
pub const MAX_TERMINAL_DIMENSION: u16 = 1000;

/// The one geometry predicate, shared by `pty.spawn` and `pty.resize`.
///
/// One body rather than a check at each call site: these are the two faces of
/// the same question, and a bound enforced on one face is not a bound. `0` is
/// admitted on purpose — it is the wire's "unset", which `PtySession::spawn`
/// substitutes 24/80 for and `note_viewport` raises with `.max(1)`; refusing
/// it here would break every client that omits the fields.
fn check_dimensions(rows: u16, cols: u16) -> Result<(), String> {
    if rows > MAX_TERMINAL_DIMENSION || cols > MAX_TERMINAL_DIMENSION {
        return Err(format!(
            "terminal geometry {rows}x{cols} exceeds the maximum of \
             {MAX_TERMINAL_DIMENSION}x{MAX_TERMINAL_DIMENSION} (rows x cols)"
        ));
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
pub struct SpawnParams {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra env vars layered on the inherited environment.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
}

#[derive(Debug, Deserialize)]
pub struct InputParams {
    pub session_id: String,
    /// Bytes to send to the child's stdin.
    pub data: String,
    /// When true, `data` is base64-decoded before writing (binary-safe paste);
    /// otherwise it is sent as raw UTF-8 (the common keystroke case).
    #[serde(default)]
    pub base64: bool,
}

#[derive(Debug, Deserialize)]
pub struct ResizeParams {
    pub session_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Deserialize)]
pub struct CloseParams {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AttachParams {
    pub session_id: String,
}

/// `pty.spawn` — open a new terminal session, returning its id.
///
/// `config` is read exactly once, into `cfg` below, and every downstream
/// decision in this call — the `[policies.terminal] enabled` gate, the
/// workspace-root jail, and the `scrollback_lines`/`max_sessions` values
/// handed to the manager — comes from that one snapshot. Two `Config`
/// answers three lines apart (one live, one re-read from disk) is exactly
/// how a live-patched `enabled = false` and a stale on-disk workspace root
/// could disagree within a single spawn; see `jail.rs`'s module doc on why a
/// fallback that answers a different question than the one asked is a lie,
/// not a default.
pub async fn handle_spawn(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: SpawnParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(id, INVALID_PARAMS, format!("invalid params: {e}"))
            }
        },
        // Empty params → default shell.
        None => SpawnParams::default(),
    };

    // Before the config read and before any allocation: a geometry this large
    // is refused on the cheapest possible path, and the refusal does not
    // depend on any policy being loaded.
    if let Err(e) = check_dimensions(params.rows, params.cols) {
        return JsonRpcResponse::error(id, INVALID_PARAMS, e);
    }

    // Read fresh on every spawn (not cached at boot) so a `[policies.terminal]`
    // patch — the gate below, or a workspace root registered after
    // start-up — is usable on the very next call.
    let cfg = config.read().await;
    if !cfg.policies.terminal.enabled {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "the embedded terminal is disabled: set `[policies.terminal] enabled = true` \
             in config.toml to turn it on"
                .to_string(),
        );
    }
    let roots = pty::workspace_roots(&cfg.agents.defaults);
    let terminal = cfg.policies.terminal.clone();
    drop(cfg); // don't hold the read lock across the spawn below

    // The client's cwd is a request, not an authorisation: resolve it
    // against the operator-registered workspace roots before it ever reaches
    // the child process.
    let cwd = match pty::jail::resolve_spawn_cwd(params.cwd.as_deref(), &roots) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(id, INVALID_PARAMS, e),
    };

    let actor = crate::gateway::visibility::ambient_actor();
    let opts = SpawnOptions {
        command: params.command,
        args: params.args,
        // Converted only now, at the boundary where the canonical path
        // leaves our own comparison logic and is handed to `portable_pty`'s
        // `CommandBuilder` for the OS to consume directly — never inside the
        // jail's `starts_with` check itself (`jail::canonical`'s doc). On
        // Windows this strips the `\\?\` extended-length prefix that
        // `std::fs::canonicalize` returns, which legacy shells (`cmd.exe`)
        // are known to mishandle as a working directory.
        cwd: Some(crate::utils::paths::display_string(&cwd)),
        env: params.env.into_iter().collect(),
        rows: params.rows,
        cols: params.cols,
        // Carried in the options rather than applied afterwards — see
        // `SpawnOptions::scrollback_lines` for the race a post-hoc setter opens.
        scrollback_lines: Some(terminal.scrollback_lines as usize),
        created_by: actor.clone(),
    };

    pty::manager().set_max_sessions(terminal.max_sessions);
    match pty::manager().spawn(&opts) {
        Ok(res) => {
            // The session is already registered by the time `spawn` returns
            // (manager.rs inserts under the same lock it builds the result
            // from), so this snapshot is real, not a guess — but fall back to
            // the requested dimensions if it somehow can't be found, rather
            // than failing a spawn that already succeeded.
            let snapshot = pty::manager()
                .attach_snapshot(&res.session_id)
                .unwrap_or_else(|_| aleph_protocol::pty::PtyAttachResponse {
                    seq: 0,
                    rows: if params.rows == 0 { 24 } else { params.rows },
                    cols: if params.cols == 0 { 80 } else { params.cols },
                    patch: aleph_protocol::pty::PtyScreenPatch::default(),
                    scrollback_len: 0,
                });
            let body = aleph_protocol::pty::PtySpawnResponse {
                session_id: res.session_id,
                shell: res.shell,
                seq: snapshot.seq,
                rows: snapshot.rows,
                cols: snapshot.cols,
            };
            match serde_json::to_value(&body) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, format!("encode failed: {e}")),
            }
        }
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.attach` — one snapshot of a session's screen plus the seq it was
/// taken at. One call, not two: split across two round trips this opens a
/// window where the client holds a screen and a different cursor.
pub async fn handle_attach(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: AttachParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_owned(&request, &params.session_id) {
        return resp;
    }
    match pty::manager().attach_snapshot(&params.session_id) {
        Ok(snapshot) => match serde_json::to_value(&snapshot) {
            Ok(v) => JsonRpcResponse::success(id, v),
            Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, format!("encode failed: {e}")),
        },
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.input` — write keystrokes/bytes to a session's stdin.
pub async fn handle_input(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: InputParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_owned(&request, &params.session_id) {
        return resp;
    }
    let bytes = if params.base64 {
        match BASE64.decode(params.data.as_bytes()) {
            Ok(b) => b,
            Err(e) => {
                return JsonRpcResponse::error(id, INVALID_PARAMS, format!("invalid base64: {e}"))
            }
        }
    } else {
        params.data.into_bytes()
    };
    match pty::manager().write(&params.session_id, &bytes) {
        Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.resize` — record this connection's terminal viewport and apply the
/// smallest viewport across every client attached to the session.
///
/// A server-held screen makes multi-client sharing free, and the moment a
/// second client attaches, something has to decide the one size the PTY
/// actually gets — see [`PtyManager::note_viewport`]. The viewport is keyed
/// on the real transport-level connection id
/// ([`caller_identity::CALLER_CONN_ID`](crate::gateway::caller_identity::CALLER_CONN_ID)),
/// never a caller-supplied one: `client_id` is a value the caller picks, and
/// keying the constraint table on it would let the axis be chosen by the
/// party being graded. A caller with no such id (no gateway dispatch scope —
/// cron, internal, a bare test) is refused rather than silently attributed a
/// made-up connection.
pub async fn handle_resize(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: ResizeParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // The same bound `pty.spawn` applies, and the reason it must be repeated
    // here rather than left to spawn: `note_viewport` -> `apply_effective_size`
    // -> `Grid::resize` reaches the same single allocation, and this face never
    // consults `[policies.terminal] enabled`, so it is the cheaper of the two
    // ways in.
    if let Err(e) = check_dimensions(params.rows, params.cols) {
        return JsonRpcResponse::error(id, INVALID_PARAMS, e);
    }

    if let Err(resp) = require_owned(&request, &params.session_id) {
        return resp;
    }

    let Some(conn_id) = crate::gateway::caller_identity::current_caller_conn_id() else {
        return JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            "pty.resize requires a gateway connection",
        );
    };

    // `note_viewport` itself checks for an unknown session under the same
    // lock it uses to record the viewport — no separate `list()` round trip
    // here, which would both TOCTOU-race a concurrent close/remove and
    // full-clone every live session's SessionInfo on every resize (a resize
    // can fire on every frame of a window drag).
    match pty::manager().note_viewport(&params.session_id, &conn_id, params.rows, params.cols) {
        Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.close` — terminate a session.
pub async fn handle_close(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let params: CloseParams = match parse(&request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_owned(&request, &params.session_id) {
        return resp;
    }
    match pty::manager().close(&params.session_id) {
        Ok(()) => JsonRpcResponse::success(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(id, INVALID_PARAMS, e),
    }
}

/// `pty.list` — enumerate THIS CALLER'S active sessions.
///
/// The filter is the reason `SessionInfo::created_by` exists, and it is not
/// cosmetic: this list is where a client gets the ids the four addressed
/// methods take, and the shipped Panel adopts the first live entry it finds
/// as its own view's session. Unfiltered, a second operator's terminal view
/// silently joined the first one's shell — scrollback restored, keystrokes
/// delivered. The addressed methods carry the same predicate ([`require_owned`])
/// rather than trusting this one, because a list that hides an id is not a
/// gate on the id.
///
/// The trust model makes operators permission-equivalent, which is why this
/// is scoping and not a privilege fix: it is what justifies GRANTING access
/// on request, not handing it over by default. Terminal bytes also bypass the
/// secret masker, so a key typed at one prompt reached every attached
/// operator connection.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let actor = crate::gateway::visibility::ambient_actor();
    let sessions: Vec<_> = pty::manager()
        .list()
        .into_iter()
        .filter(|s| pty::owner_admits(s.created_by.as_deref(), actor.as_deref()))
        .collect();
    JsonRpcResponse::success(id, json!({ "sessions": sessions }))
}

/// The ownership gate every session-ADDRESSED `pty.*` method passes through.
///
/// One body, one predicate ([`pty::owner_admits`], also used by
/// [`handle_list`]'s filter and by `event_visibility`'s `pty.screen` /
/// `pty.exit` arm), because a rule enforced on some of a verb's faces is not
/// a rule. Pinned by `every_addressed_pty_handler_checks_ownership`, which
/// derives its membership from the source rather than listing the four
/// handlers — the fifth one is the one that would be missed.
///
/// # Refused as "no such session", byte for byte
///
/// The message is exactly what the manager returns for an id that does not
/// exist, on purpose: a distinct "not yours" would turn every addressed
/// method into an oracle for enumerating other operators' session ids, which
/// is the very thing scoping `pty.list` just took away. Callers lose nothing
/// — a session you may not address is one you have no legitimate use for.
///
/// # No TOCTOU worth closing
///
/// This resolves the owner and the addressed call resolves the session, two
/// lock acquisitions apart. A session that dies in between makes the call
/// fail with the same not-found it would have failed with anyway, and the id
/// is a v4 UUID that is never reused — so there is no window in which the
/// answer this returns can become wrong for the id it was asked about.
#[allow(clippy::result_large_err)]
fn require_owned(request: &JsonRpcRequest, session_id: &str) -> Result<(), JsonRpcResponse> {
    let actor = crate::gateway::visibility::ambient_actor();
    if pty::manager().owner_of(session_id).admits(actor.as_deref()) {
        return Ok(());
    }
    Err(JsonRpcResponse::error(
        request.id.clone(),
        INVALID_PARAMS,
        format!("no such session: {session_id}"),
    ))
}

/// Parse typed params, mapping a missing/invalid body to `INVALID_PARAMS`.
#[allow(clippy::result_large_err)]
fn parse<T: serde::de::DeserializeOwned>(request: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "missing params",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn req(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    /// A config handle whose `[agents.defaults] workspace_root` points at a
    /// private tempdir this call owns — deliberately NOT `Config::load()`.
    ///
    /// `Config::load()` (and `workspace_root_for`'s `default_workspace_root`
    /// fallback it hits when unset) reads `ALEPH_HOME`/`$HOME`, which are
    /// process-global. libtest runs this module's tests on parallel threads
    /// alongside sibling tests elsewhere in this binary that isolate their
    /// own `ALEPH_HOME` via `AlephHomeEnvGuard`/`IsolatedAlephHome` — so a
    /// handler that re-read the real env on every spawn was racing whichever
    /// sibling happened to be swapping that variable at the same instant.
    /// Measured: `cargo test -p alephcore --lib` intermittently failed two
    /// of this file's own tests on that exact symptom (two different
    /// resolved roots, one per test) before `handle_spawn` took its config
    /// as a parameter instead of loading it itself (Task 12).
    ///
    /// The caller must keep the returned `TempDir` alive for as long as the
    /// config handle is used — dropping it deletes the directory out from
    /// under a still-running child.
    fn isolated_config() -> (Arc<RwLock<Config>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = Config::default();
        cfg.agents.defaults.workspace_root = Some(dir.path().to_path_buf());
        (Arc::new(RwLock::new(cfg)), dir)
    }

    /// The geometry ceiling, on the face that reaches the allocation without
    /// consulting any policy.
    ///
    /// `pty.resize` is the cheaper of the two ways to `Grid::resize`'s single
    /// `vec![Cell; rows * cols]` — it never reads `[policies.terminal]`. The
    /// session id is deliberately a ghost: the refusal has to land BEFORE the
    /// lookup, so this passes for the right reason only if the message names
    /// the ceiling rather than the missing session.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn resize_refuses_a_geometry_that_would_allocate_the_host() {
        let resp = handle_resize(req(
            "pty.resize",
            json!({ "session_id": "ghost", "rows": 65535, "cols": 65535 }),
        ))
        .await;
        let msg = resp
            .error
            .expect("oversized resize must be refused")
            .message;
        assert!(
            msg.contains("exceeds the maximum"),
            "must refuse on geometry, not on the unknown session: {msg}"
        );
        assert!(
            msg.contains(&MAX_TERMINAL_DIMENSION.to_string()),
            "the refusal must name the ceiling so a client can correct itself: {msg}"
        );
    }

    /// Refused, not silently clamped — and refused on `spawn` too, because a
    /// bound enforced on one of two faces is not a bound.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn spawn_refuses_a_geometry_that_would_allocate_the_host() {
        let (config, _dir) = isolated_config();
        let resp = handle_spawn(
            req("pty.spawn", json!({ "rows": 65535, "cols": 65535 })),
            config,
        )
        .await;
        let msg = resp.error.expect("oversized spawn must be refused").message;
        assert!(
            msg.contains("exceeds the maximum"),
            "spawn must refuse oversized geometry: {msg}"
        );
        assert!(
            resp.result.is_none(),
            "a refused spawn must not return a session"
        );
    }

    /// One axis is enough. Written because the natural way to get this wrong
    /// is `rows > MAX && cols > MAX` — which admits 24 x 65535, whose product
    /// is the same order of magnitude as the case above.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_single_oversized_axis_is_refused() {
        assert!(
            check_dimensions(24, 65535).is_err(),
            "wide grid must refuse"
        );
        assert!(
            check_dimensions(65535, 80).is_err(),
            "tall grid must refuse"
        );
        assert!(
            check_dimensions(MAX_TERMINAL_DIMENSION, MAX_TERMINAL_DIMENSION).is_ok(),
            "the ceiling itself is admissible"
        );
        assert!(
            check_dimensions(MAX_TERMINAL_DIMENSION + 1, 80).is_err(),
            "one past the ceiling is not"
        );
    }

    /// `0` means "unset" on this wire, not "a zero-sized terminal": every
    /// client that omits the fields sends it, `PtySession::spawn` substitutes
    /// 24/80 and `note_viewport` raises it with `.max(1)`. A ceiling check
    /// that also refused the floor would break the common case.
    #[test]
    #[serial_test::parallel(pty_global_manager)]
    fn zero_is_the_wires_unset_and_stays_admissible() {
        assert!(check_dimensions(0, 0).is_ok());
        assert!(check_dimensions(24, 80).is_ok());
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn input_unknown_session_is_error_not_panic() {
        let resp = handle_input(req(
            "pty.input",
            json!({ "session_id": "ghost", "data": "x" }),
        ))
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn input_rejects_bad_base64() {
        let resp = handle_input(req(
            "pty.input",
            json!({ "session_id": "ghost", "data": "!!!not base64!!!", "base64": true }),
        ))
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn list_returns_sessions_array() {
        let resp = handle_list(req("pty.list", json!({}))).await;
        let result = resp.result.expect("list always succeeds");
        assert!(result.get("sessions").and_then(|s| s.as_array()).is_some());
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn attach_returns_a_snapshot_with_its_seq() {
        let (config, _tmp) = isolated_config();
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 8, "cols": 30 })), config).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = handle_attach(req("pty.attach", json!({ "session_id": sid }))).await;
        let value = resp.result.expect("attach must succeed");
        let parsed: aleph_protocol::pty::PtyAttachResponse =
            serde_json::from_value(value.clone()).expect("attach response must match the contract");
        assert_eq!(parsed.rows, 8);
        assert_eq!(parsed.cols, 30);
        assert_eq!(parsed.patch.rows.len(), 8, "a snapshot carries every row");

        // The contract is the key set, not a subset: a parse-only assertion
        // is blind to over-sending because serde ignores unknown keys.
        let keys: std::collections::BTreeSet<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["cols", "patch", "rows", "scrollback_len", "seq"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn attach_on_an_unknown_session_is_an_error_not_an_empty_screen() {
        let resp = handle_attach(req("pty.attach", json!({ "session_id": "ghost" }))).await;
        assert!(
            resp.result.is_none(),
            "an unknown session must not read as a blank screen"
        );
        assert!(resp.error.is_some());
    }

    /// A caller with no gateway connection scope (cron, internal, a bare
    /// test) must be refused, never attributed a made-up viewport owner —
    /// see `caller_identity::CALLER_CONN_ID`'s module doc. This is exercised
    /// with no `CALLER_CONN_ID` scope at all, matching how a non-gateway
    /// caller actually looks.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn resize_without_conn_id_is_refused_not_applied() {
        let (config, _tmp) = isolated_config();
        let spawn = handle_spawn(req("pty.spawn", json!({ "rows": 24, "cols": 80 })), config).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = handle_resize(req(
            "pty.resize",
            json!({ "session_id": sid, "rows": 10, "cols": 40 }),
        ))
        .await;
        assert!(
            resp.error.is_some(),
            "resize without a connection id must be refused"
        );

        // Refusing must not have recorded a viewport under some fallback id.
        assert_eq!(pty::manager().effective_size(&sid), None);

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// The happy path: a real gateway connection resizes and its viewport is
    /// recorded and applied (single attached client ⇒ its own request wins).
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn resize_with_conn_id_records_viewport_and_applies_it() {
        let (config, _tmp) = isolated_config();
        let spawn =
            handle_spawn(req("pty.spawn", json!({ "rows": 40, "cols": 120 })), config).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resp = crate::gateway::caller_identity::CALLER_CONN_ID
            .scope(Some("conn-a".to_string()), async {
                handle_resize(req(
                    "pty.resize",
                    json!({ "session_id": sid, "rows": 24, "cols": 80 }),
                ))
                .await
            })
            .await;
        assert!(
            resp.error.is_none(),
            "resize with a connection id must succeed: {resp:?}"
        );
        assert_eq!(pty::manager().effective_size(&sid), Some((24, 80)));

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// Two connections attached to the same session share one PTY size —
    /// the smallest wins, and the JSON-RPC surface must produce the same
    /// behavior `PtyManager::note_viewport`'s own unit tests establish.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn two_conn_ids_share_smallest_wins_through_the_handler() {
        let (config, _tmp) = isolated_config();
        let spawn =
            handle_spawn(req("pty.spawn", json!({ "rows": 40, "cols": 120 })), config).await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let resize = |conn: &'static str, rows: u16, cols: u16, sid: String| async move {
            crate::gateway::caller_identity::CALLER_CONN_ID
                .scope(Some(conn.to_string()), async {
                    handle_resize(req(
                        "pty.resize",
                        json!({ "session_id": sid, "rows": rows, "cols": cols }),
                    ))
                    .await
                })
                .await
        };

        let a = resize("conn-a", 40, 120, sid.clone()).await;
        assert!(a.error.is_none());
        let b = resize("conn-b", 24, 80, sid.clone()).await;
        assert!(b.error.is_none());

        assert_eq!(pty::manager().effective_size(&sid), Some((24, 80)));

        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// `pty.resize` on an unknown session id must still be an error, even
    /// with a valid connection id — the existing "unknown session is an
    /// error, never a silent no-op" contract other pty.* handlers hold.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn resize_unknown_session_is_still_an_error() {
        let resp = crate::gateway::caller_identity::CALLER_CONN_ID
            .scope(Some("conn-a".to_string()), async {
                handle_resize(req(
                    "pty.resize",
                    json!({ "session_id": "ghost", "rows": 10, "cols": 40 }),
                ))
                .await
            })
            .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn spawn_response_matches_the_contract_key_for_key() {
        let (config, _tmp) = isolated_config();
        let resp = handle_spawn(req("pty.spawn", json!({ "rows": 4, "cols": 12 })), config).await;
        let value = resp.result.expect("spawned");
        let keys: std::collections::BTreeSet<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["cols", "rows", "seq", "session_id", "shell"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        let sid = value["session_id"].as_str().expect("id").to_string();
        let _ = handle_close(req("pty.close", json!({ "session_id": sid }))).await;
    }

    /// A response-only assertion (`resp.error.is_some()`) would still pass
    /// if the handler returned the jail's error *and also* called
    /// `manager().spawn()` before, or without, honoring it — e.g. a future
    /// refactor that moves the spawn call ahead of the jail check but
    /// leaves the error response construction intact. This asserts the
    /// effect, not just the reply: the session table must not gain an
    /// entry as a result of the call. (Review round 1, Major finding.)
    ///
    /// This does NOT compare `pty::manager().list()` before and after the
    /// call — a round-1 version of this test did, and it was wrong. The
    /// manager is a process-global singleton shared with every other test
    /// in this binary, libtest runs tests on parallel threads, and a
    /// before/after set difference picks up *any* sibling's spawn/close
    /// landing inside this call's window: measured (fix-round-1
    /// re-review) at 10/12 to 14/14 failures under filters that include
    /// the heaviest sibling
    /// (`pty::tests::a_write_reaches_a_real_subscriber_over_the_pty_screen_topic`,
    /// which holds a session open across its own 100 x 50ms polling loop).
    /// Every one of those failures reported a sibling's id as evidence of
    /// a jail bypass that never happened — the expensive direction, since
    /// the next reader goes looking for a security bug that is not there.
    ///
    /// Instead this names *this call's own* session by giving it a program
    /// label no other spawn anywhere in this test binary uses, then checks
    /// whether any session carrying that label exists — a predicate a
    /// sibling's activity cannot satisfy no matter how the scheduler
    /// interleaves it. `SessionInfo.shell` is that label verbatim:
    /// `session.rs`'s spawn does
    /// `Some(prog) => (CommandBuilder::new(prog), prog.clone())`, so
    /// whatever string is passed as `command` is exactly what shows up
    /// here. On Unix, `"/bin/sh"` is the same real binary every other
    /// spawn in this crate reaches via the bare name `"sh"`
    /// (`session.rs:295`, `manager.rs:331`) or via the omitted-command
    /// fallback `default_shell_label()` (which reports `$SHELL` — by
    /// convention a user's interactive login shell such as bash or zsh,
    /// not `/bin/sh` verbatim) — so the full path is a distinct string
    /// from every label those two paths produce. On Windows,
    /// `"powershell.exe"` resolves via `PATH` the same way the bare
    /// `"cmd.exe"` used by those same two call sites does, and is distinct
    /// both from that literal and from the omitted-command fallback's
    /// `%COMSPEC%` (a stock install sets that to a *full path* to
    /// `cmd.exe`, not the bare name `"powershell.exe"`).
    ///
    /// The probe must stay alive for this to mean anything: if the jail
    /// were bypassed, the session is inserted synchronously, but
    /// `spawn_reader` calls `manager().remove()` the instant the child
    /// hits EOF, so a short-lived probe could be reaped before this looks
    /// for it and the assertion would pass vacuously on a real bypass.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn spawn_with_a_cwd_outside_every_root_creates_no_session() {
        // See the doc comment above for why this exact pair of labels was
        // chosen, and why a before/after id snapshot is not used instead.
        let (probe_command, probe_args): (&str, Vec<String>) = if cfg!(windows) {
            (
                "powershell.exe",
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "Start-Sleep -Seconds 30".to_string(),
                ],
            )
        } else {
            ("/bin/sh", vec!["-c".to_string(), "sleep 30".to_string()])
        };

        // "/" is outside every registered workspace root on every platform
        // this repo supports: `resolve_spawn_cwd` rejects any `asked` that
        // is an *ancestor* of a root (jail.rs's module doc, "One direction,
        // not two"), and no configured or defaulted workspace root is the
        // filesystem root itself.
        let (config, _tmp) = isolated_config();
        let resp = handle_spawn(
            req(
                "pty.spawn",
                json!({
                    "cwd": "/",
                    "command": probe_command,
                    "args": probe_args,
                    "rows": 24,
                    "cols": 80
                }),
            ),
            config,
        )
        .await;

        assert!(
            resp.result.is_none(),
            "a refused spawn must not report success"
        );
        let err = resp
            .error
            .expect("a cwd outside every root must be refused");
        assert_eq!(err.code, INVALID_PARAMS);

        let leaked: Vec<pty::SessionInfo> = pty::manager()
            .list()
            .into_iter()
            .filter(|s| s.shell == probe_command)
            .collect();
        // A bypass would leave the probe alive (it sleeps 30s); close it
        // first so a failing run does not leak a process into the rest of
        // this binary's life.
        for session in &leaked {
            pty::manager().close(&session.session_id).ok();
        }
        assert!(
            leaked.is_empty(),
            "a refused spawn must not have created a session, but found one \
             carrying this test's probe label {probe_command:?}: {leaked:?}"
        );
    }

    /// The security-relevant claim is not "the handler returns a session
    /// id" — it is "the process the OS actually execs inherits the
    /// *jailed* cwd, not whatever the client asked for verbatim". A
    /// response-only assertion would pass unchanged if `handle_spawn`
    /// resolved the jail and then spawned with `params.cwd` anyway. So
    /// this asks the real child process what its own cwd is, over the real
    /// PTY screen — the same idiom
    /// `session::tests::a_child_write_reaches_the_server_held_screen`
    /// already uses to verify real output, not a handler's return value.
    /// (Review round 1, Major finding.)
    ///
    /// This exercises the omitted-`cwd` arm rather than an explicit
    /// in-root path: on a machine with no symlinks anywhere under the
    /// workspace root (true here — confirmed `~/.aleph` is a real
    /// directory, not a link), an explicit request's literal string and
    /// its canonicalised form are byte-identical, so a handler that skips
    /// `resolve_spawn_cwd` and passes `params.cwd` straight through would
    /// land the child in exactly the same place as the correctly-jailed
    /// one — `chdir` + the shell's own `getcwd`-backed `pwd` always reports
    /// the canonical location regardless of which spelling was used to get
    /// there, so that shape cannot discriminate "resolved" from "passed
    /// through" here. The omitted-`cwd` arm can: `session.rs`'s spawn only
    /// calls `CommandBuilder::cwd()` `if let Some(cwd) = &opts.cwd`, so a
    /// handler that lets `params.cwd` (`None`) through unresolved never
    /// calls it at all.
    ///
    /// What the child inherits then is **not** the daemon's own process
    /// cwd — an earlier version of this comment, and of the failure
    /// message below, both said that, and both were wrong (fix-round-1
    /// re-review, New Minor). `portable-pty` 0.8.1's `CommandBuilder`
    /// falls back to the home directory it reads from the child's own
    /// environment (`cmdbuilder.rs`'s `get_home_dir`/`current_directory`:
    /// `$HOME` on Unix, `USERPROFILE` on Windows), inherited straight from
    /// this test binary's own process environment for an unmodified
    /// spawn. Measured directly under both mutations in the fix-round-1
    /// re-review: the screen held `/Users/…` (the account's `$HOME`),
    /// while the test binary's own cwd was the worktree. So the real
    /// discriminating margin is "resolved workspace root" vs.
    /// "`$HOME`/`USERPROFILE`", not vs. "wherever the daemon process
    /// happens to be running from" — and that margin is machine-dependent
    /// in a way an earlier version of this doc denied: on an install whose
    /// `[agents.defaults] workspace_root` resolves to `$HOME`, or to any
    /// string `$HOME` contains as a substring (e.g. `/Users`, or `/`), an
    /// unjailed child's screen would *also* satisfy `contains(expected)`,
    /// and this test would go green with the jail bypassed. The guard
    /// below turns that into a loud failure instead of a silent pass.
    ///
    /// Task 12 gave `handle_spawn` a real config-injection point: an
    /// `Arc<RwLock<Config>>` parameter, read once inside the handler. This
    /// test uses it via `isolated_config()`, deriving `expected` from the
    /// exact same config handle passed to `handle_spawn` — not from
    /// `Config::load()`/`ALEPH_HOME`, which is process-global and was
    /// racing every sibling test in this binary that isolates its own
    /// `ALEPH_HOME` (see `isolated_config`'s doc; that race was the "two
    /// different resolved roots" flake Task 12 measured and fixed by
    /// injecting config instead of loading it inside the handler). The
    /// only workspace root this test relies on is whatever
    /// `workspace_roots()` resolves against that one owned config — the
    /// exact function, and the exact config, `handle_spawn` itself
    /// resolves against — with the guard below covering the one case (an
    /// owned tempdir root that happens to be a substring of `$HOME`) that
    /// would make this an insufficient discriminator on its own.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn spawn_with_no_cwd_actually_chdirs_the_child_into_the_first_root() {
        // The poll loop below drains its budget at ITERATIONS x
        // INTERVAL_MS; both platforms' child processes are bounded to a
        // fixed multiple of that budget (not a bare guess), so they
        // comfortably outlive every iteration on a slow runner while
        // still being bounded rather than open-ended.
        const POLL_ITERATIONS: u32 = 100;
        const POLL_INTERVAL_MS: u64 = 50;
        const CHILD_LIFETIME_SECS: u64 = POLL_ITERATIONS as u64 * POLL_INTERVAL_MS / 1000 * 6;

        let (config, _tmp) = isolated_config();
        let roots = {
            let cfg = config.read().await;
            pty::workspace_roots(&cfg.agents.defaults)
        };
        let root = roots
            .first()
            .cloned()
            .expect("workspace_roots always returns one root");

        // What the handler should have handed the child: the canonical,
        // display-boundary-converted form of the root — computed
        // independently here (not by calling `resolve_spawn_cwd` and
        // trusting its own answer), so this cannot pass by construction
        // against that function. (It is not independent of
        // `workspace_roots` itself, which both this test and the handler
        // call — see the doc comment above.)
        let expected =
            crate::utils::paths::display_string(&std::fs::canonicalize(&root).expect("canonical"));

        // If the unjailed `cwd: None` fallback (`$HOME`/`USERPROFILE`)
        // already contains `expected` as a substring, a bypassed
        // handler's screen would satisfy `contains(expected)` too, and
        // the loop below could not tell "jailed" from "bypassed" on this
        // machine. Fail loudly here instead of passing vacuously on a
        // real bypass.
        let home_fallback = if cfg!(windows) {
            std::env::var("USERPROFILE")
        } else {
            std::env::var("HOME")
        }
        .expect("this test needs $HOME/USERPROFILE set to know what it must discriminate against");
        assert!(
            !home_fallback.contains(expected.as_str()),
            "this machine cannot discriminate a jailed spawn from a bypassed \
             one: the resolved workspace root {expected:?} is a substring of \
             the unjailed cwd:-None fallback ({home_fallback:?}, which is \
             what portable-pty's CommandBuilder falls back to when no cwd is \
             set). Configure a workspace root outside {home_fallback:?} to \
             run this test meaningfully."
        );

        // The child must OUTLIVE the assertion, which is why it sleeps
        // rather than just printing. `spawn_reader` calls
        // `manager().remove(&id)` the moment the child hits EOF
        // (session.rs, end of the reader thread), so a one-shot `pwd`
        // races its own reaper: the first `attach_snapshot` can land
        // before the reader has fed the screen, and every later one
        // returns `Err` because the session is already gone from the map.
        // The symptom is a blank screen and a five-second timeout, which
        // reads exactly like "the cwd was wrong". It is not: the session
        // was reaped.
        let (command, args) = if cfg!(windows) {
            // `/K` never terminates on its own — a panic between spawn and
            // `close()` would leave the shell running with no bound at
            // all. `/C` runs the given command line and exits once it
            // finishes; `ping -n N 127.0.0.1` is the traditional
            // dependency-free sleep on Windows (no `sleep.exe`/`timeout`
            // required to be present, and no console-input assumptions
            // the way `timeout` makes), and `N` pings take about `N - 1`
            // seconds, hence the `+ 1` below.
            (
                "cmd.exe",
                vec![
                    "/C".to_string(),
                    format!("cd & ping -n {} 127.0.0.1 > NUL", CHILD_LIFETIME_SECS + 1),
                ],
            )
        } else {
            (
                "sh",
                vec![
                    "-c".to_string(),
                    format!("pwd; sleep {CHILD_LIFETIME_SECS}"),
                ],
            )
        };
        // Deliberately no `cwd` field at all — the omitted-cwd request the
        // jail must resolve to the first registered root.
        let spawn = handle_spawn(
            req(
                "pty.spawn",
                json!({ "command": command, "args": args, "rows": 24, "cols": 240 }),
            ),
            config,
        )
        .await;
        let sid = spawn
            .result
            .as_ref()
            .expect("spawn with an omitted cwd must succeed")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let mut found = false;
        let mut last_seen = String::from("<no snapshot ever returned Ok>");
        for _ in 0..POLL_ITERATIONS {
            if let Ok(snap) = pty::manager().attach_snapshot(&sid) {
                // Concatenate the whole screen, not just one row's runs: a
                // long path can hard-wrap across a row boundary, and a
                // per-row `contains` check would miss it there regardless
                // of how wide the grid is.
                let screen_text: String = snap
                    .patch
                    .rows
                    .iter()
                    .map(|r| {
                        r.runs
                            .iter()
                            .map(|run| run.text.as_str())
                            .collect::<String>()
                    })
                    .collect();
                last_seen = screen_text.clone();
                if screen_text.contains(expected.as_str()) {
                    found = true;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }

        pty::manager().close(&sid).ok();
        assert!(
            found,
            "an omitted cwd must chdir the child into the resolved first workspace root \
             {expected}, not the unjailed $HOME/USERPROFILE fallback; screen held: {last_seen:?}"
        );
    }

    /// The handler is the only place both fields have to be filled in, and it
    /// is the one place neither task's own test looks: Task 12's constructs
    /// `SpawnOptions` by hand and asserts the scrollback field reaches the
    /// grid; a `created_by` test of the same shape would assert the same
    /// thing about the other field. A `handle_spawn` that filled exactly one
    /// of them passes both.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_spawn_through_the_handler_carries_both_the_actor_and_the_scrollback() {
        let (config, _tmp) = isolated_config();
        config.write().await.policies.terminal.scrollback_lines = 7;

        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_spawn(req("pty.spawn", json!({})), config),
            )
            .await;
        let sid = resp.result.as_ref().expect("spawn should succeed")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        // Never `pty::manager().list()[0]` -- this binary's tests share one
        // process-global manager, so `[0]` can be a sibling's session.
        let info = pty::manager()
            .list()
            .into_iter()
            .find(|s| s.session_id == sid)
            .expect("the session we just spawned must be listed");
        assert_eq!(
            info.created_by.as_deref(),
            Some("u-alice"),
            "the actor must reach the session"
        );
        assert_eq!(
            pty::manager().scrollback_limit_of(&sid),
            Some(7),
            "and so must the configured scrollback -- a handler that fills one \
             SpawnOptions field and not the other passes every other test in both tasks"
        );
        pty::manager().close(&sid).expect("close");
    }

    /// The whole of F2 in one run: a second operator can neither SEE nor
    /// ADDRESS a session it did not create, on every face that takes or hands
    /// out a session id.
    ///
    /// Both halves matter and neither implies the other. Hiding the id in
    /// `pty.list` without gating the addressed methods leaves a guessable
    /// handle; gating the methods without filtering the list leaves the
    /// shipped Panel adopting a stranger's shell as its own view (it takes
    /// the first live entry) and then failing every call against it.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_second_operator_can_neither_see_nor_address_another_operators_session() {
        let (config, _tmp) = isolated_config();
        let spawn = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_spawn(req("pty.spawn", json!({})), config),
            )
            .await;
        let sid = spawn.result.as_ref().expect("spawned")["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        // --- the enumeration face ---
        let bob_list = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_list(req("pty.list", json!({}))),
            )
            .await;
        let listed = |resp: &JsonRpcResponse| -> bool {
            resp.result
                .as_ref()
                .and_then(|v| v.get("sessions"))
                .and_then(|v| v.as_array())
                .expect("sessions array")
                .iter()
                .any(|s| s.get("session_id").and_then(Value::as_str) == Some(sid.as_str()))
        };
        assert!(!listed(&bob_list), "bob must not be handed alice's id");

        let alice_list = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_list(req("pty.list", json!({}))),
            )
            .await;
        assert!(
            listed(&alice_list),
            "and the filter must not swallow the owner's own session — a \
             scoping that hides it from everyone is not a fix"
        );

        // --- the four addressed faces, with the id supplied out of band ---
        for (name, resp) in [
            (
                "pty.attach",
                crate::gateway::caller_identity::CALLER_USER
                    .scope(
                        Some("u-bob".to_string()),
                        handle_attach(req("pty.attach", json!({ "session_id": sid }))),
                    )
                    .await,
            ),
            (
                "pty.input",
                crate::gateway::caller_identity::CALLER_USER
                    .scope(
                        Some("u-bob".to_string()),
                        handle_input(req("pty.input", json!({ "session_id": sid, "data": "id\n" }))),
                    )
                    .await,
            ),
            (
                "pty.resize",
                crate::gateway::caller_identity::CALLER_USER
                    .scope(
                        Some("u-bob".to_string()),
                        crate::gateway::caller_identity::CALLER_CONN_ID.scope(
                            Some("conn-bob".to_string()),
                            handle_resize(req(
                                "pty.resize",
                                json!({ "session_id": sid, "rows": 24, "cols": 80 }),
                            )),
                        ),
                    )
                    .await,
            ),
            (
                "pty.close",
                crate::gateway::caller_identity::CALLER_USER
                    .scope(
                        Some("u-bob".to_string()),
                        handle_close(req("pty.close", json!({ "session_id": sid }))),
                    )
                    .await,
            ),
        ] {
            let err = resp
                .error
                .unwrap_or_else(|| panic!("{name} must refuse a session bob does not own"));
            assert_eq!(
                err.message,
                format!("no such session: {sid}"),
                "{name} must refuse in the same words an unknown id gets — a \
                 distinguishable refusal is an oracle for enumerating other \
                 operators' session ids, which is what scoping pty.list just \
                 took away"
            );
        }

        // Alice's own session is untouched by all of that — in particular
        // bob's `pty.close` must not have killed it.
        let alice_attach = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_attach(req("pty.attach", json!({ "session_id": sid }))),
            )
            .await;
        assert!(
            alice_attach.result.is_some(),
            "the owner must still be able to attach: {:?}",
            alice_attach.error
        );

        pty::manager().close(&sid).expect("close");
    }

    /// Membership derived from the source, not listed here: every
    /// session-ADDRESSED handler in this file must carry the ownership gate.
    ///
    /// The enumeration this replaces would name `attach`/`input`/`resize`/
    /// `close` — and the handler that matters is the fifth one, written later
    /// by someone who read the four and copied the shape without the check.
    /// So the question asked is "does this handler body reach for
    /// `params.session_id`", which is exactly what makes a handler addressed;
    /// `handle_spawn` mints an id rather than accepting one and is excluded
    /// by the same rule that includes the others, not by an exemption.
    ///
    /// [`code_text`](crate::utils::source_scan::code_text) over the
    /// production prefix, because this guard's needles appear in its own
    /// doc comment and assertion strings — blanking comments and literal
    /// payloads deletes the self-match problem instead of exempting this file.
    #[test]
    fn every_addressed_pty_handler_checks_ownership() {
        use crate::utils::source_scan::{code_text, production_prefix};

        const GATE: &str = "require_owned(";
        const ADDRESSED: &str = "params.session_id";

        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gateway/handlers/pty.rs"),
        )
        .expect("this file must be readable");
        let code = code_text(&production_prefix(&src));
        let lines: Vec<&str> = code.lines().collect();

        let mut addressed: Vec<String> = Vec::new();
        let mut ungated: Vec<String> = Vec::new();
        let mut i = 0usize;
        while i < lines.len() {
            if !lines[i].trim_start().starts_with("pub async fn handle_") {
                i += 1;
                continue;
            }
            let name = lines[i].trim().to_string();
            let (mut depth, mut opened, mut end) = (0i32, false, i);
            for (k, l) in lines.iter().enumerate().skip(i) {
                depth += i32::try_from(l.matches('{').count()).unwrap_or(0);
                depth -= i32::try_from(l.matches('}').count()).unwrap_or(0);
                opened |= l.contains('{');
                end = k;
                if opened && depth <= 0 {
                    break;
                }
            }
            let body = lines[i..=end].join("\n");
            if body.contains(ADDRESSED) {
                addressed.push(name.clone());
                if !body.contains(GATE) {
                    ungated.push(name);
                }
            }
            i = end + 1;
        }

        assert!(
            ungated.is_empty(),
            "these `pty.*` handlers address a caller-supplied session id without \
             passing `{GATE}`. Every face of the ownership rule has to carry it: \
             a session hidden from `pty.list` is still reachable by id.\n  {}",
            ungated.join("\n  ")
        );
        assert!(
            addressed.len() >= 4,
            "the scan found only {} addressed handlers ({addressed:?}); \
             attach/input/resize/close existed when this census was written, so \
             a smaller number means the scanner stopped working and this guard \
             is passing vacuously",
            addressed.len()
        );
    }

    /// Every test in this crate that reaches the process-global `PtyManager`
    /// carries one of the two `pty_global_manager` `serial_test` keys.
    ///
    /// `config::live_apply`'s
    /// `disabling_the_terminal_live_kills_sessions_through_apply_live_sections`
    /// holds `#[serial_test::serial(pty_global_manager)]` and calls
    /// `PtyManager::close_all()`, which kills EVERY live session in the
    /// process. Any test that spawns one and is still asserting about it must
    /// hold `#[serial_test::parallel(pty_global_manager)]` — still parallel
    /// with its siblings; the key only excludes the serial one.
    ///
    /// # Why the question is "every test that calls `manager()`" and not
    /// "every test in this file"
    ///
    /// The census this replaces read `include_str!("pty.rs")` — its own
    /// module's source — and required the tag on every test attribute in it.
    /// That is a guard whose corpus is a FILE, so it was structurally blind to
    /// a user of the singleton living anywhere else, which is the only place
    /// the next one was ever going to appear (§0 "一条守卫如果按名字列举它要
    /// 检查的成员，成员集合增长时它不会知道" — here the enumeration was of
    /// files rather than of members, with the same result). It duly missed
    /// `gateway::pty::tests::a_write_reaches_a_real_subscriber_over_the_pty_screen_topic`,
    /// which drives the singleton deliberately: it exists to prove the real
    /// `attach_event_bus` wire, so a local `PtyManager` would defeat its
    /// purpose. Measured before the tag was added,
    /// `cargo test -p alephcore --lib -- --test-threads=16 gateway::pty
    /// config::live_apply` failed **3 times in 8 runs**, each failing run
    /// taking 5.24 s — that test's 100 x 50 ms poll timing out — against
    /// 0.07 s when it passed. A default-threaded `--lib` run was green, which
    /// is why the whole branch was.
    ///
    /// So the corpus is the whole crate source and membership is derived from
    /// the CALL. `super::manager()` and a bare `manager()` are only this
    /// singleton inside `src/gateway/pty/`; elsewhere they are some other
    /// module's function of the same name, so the unqualified spellings are
    /// only needles for files under that directory.
    ///
    /// # Attribution, and the one case it refuses to guess
    ///
    /// A hit is charged to the brace-matched body of the `#[test]` /
    /// `#[tokio::test]` function containing it. A hit that lands in NO test
    /// body — a shared helper in the test module — fails too, naming itself:
    /// the guard cannot tell which tests call a helper, and silently charging
    /// it to whichever test happens to precede it in the file would be a
    /// verdict about the wrong function. Failing is the honest answer;
    /// tagging every test in that file is the fix.
    ///
    /// [`code_text`](crate::utils::source_scan::code_text) rather than
    /// `strip_comment_lines`: this guard's own file is inside the corpus it
    /// scans, and `code_text` blanks string-literal payloads as well as
    /// comments, so the needles below cannot match themselves. That deletes
    /// the self-match problem instead of growing an exemption for this one
    /// file — and an exemption is what later hides a real hit.
    #[test]
    #[serial_test::parallel(pty_global_manager)]
    fn every_test_that_reaches_the_global_pty_manager_is_tagged() {
        use crate::utils::source_scan::{cfg_test_portion, code_text, rust_sources_under};

        const PARALLEL_TAG: &str = "#[serial_test::parallel(pty_global_manager)]";
        const SERIAL_TAG: &str = "#[serial_test::serial(pty_global_manager)]";
        const QUALIFIED: &str = "pty::manager()";
        const VIA_SUPER: &str = "super::manager()";
        const BARE: &str = "manager()";

        // The three files that reached the singleton the day this census was
        // written. Asserted PRESENT, never asserted to be the whole set: a new
        // file is allowed to reach the singleton, it just has to be tagged.
        // The membership assertion is here so that a scan which silently stops
        // finding anything — a moved directory, a broken lexer, a test binary
        // built from another worktree — fails loudly instead of passing
        // vacuously.
        const KNOWN_REACHERS: [&str; 3] = [
            "src/gateway/handlers/pty.rs",
            "src/gateway/pty/mod.rs",
            "src/config/live_apply.rs",
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut reaching: Vec<String> = Vec::new();
        let mut checked_tests = 0usize;
        let mut violations: Vec<String> = Vec::new();

        for (path, src) in rust_sources_under(&root) {
            // Cheap pre-filter: every spelling above contains `manager()`.
            if !src.contains(BARE) {
                continue;
            }
            let in_pty_module = path.contains("/gateway/pty/");
            let code = code_text(&cfg_test_portion(&src));
            let lines: Vec<&str> = code.lines().collect();
            let reaches = |l: &str| {
                l.contains(QUALIFIED)
                    || (in_pty_module && (l.contains(VIA_SUPER) || l.contains(BARE)))
            };
            if !lines.iter().any(|l| reaches(l)) {
                continue;
            }
            reaching.push(path.clone());

            let mut charged = vec![false; lines.len()];
            let mut i = 0usize;
            while i < lines.len() {
                let attr = lines[i].trim();
                if !(attr.starts_with("#[tokio::test") || attr == "#[test]") {
                    i += 1;
                    continue;
                }
                // The rest of the attribute block, then the `fn` line.
                let mut j = i + 1;
                let mut tagged = false;
                while j < lines.len() && lines[j].trim().starts_with("#[") {
                    let a = lines[j].trim();
                    tagged |= a == PARALLEL_TAG || a == SERIAL_TAG;
                    j += 1;
                }
                if j >= lines.len() {
                    break;
                }
                let name = lines[j].trim().to_string();

                // Brace-match the body. Literal payloads are already blanked,
                // so a `{` inside a string cannot desynchronise this.
                let (mut depth, mut opened, mut end) = (0i32, false, j);
                for (k, l) in lines.iter().enumerate().skip(j) {
                    depth += i32::try_from(l.matches('{').count()).unwrap_or(0);
                    depth -= i32::try_from(l.matches('}').count()).unwrap_or(0);
                    opened |= l.contains('{');
                    end = k;
                    if opened && depth <= 0 {
                        break;
                    }
                }

                checked_tests += 1;
                let body_reaches = lines[j..=end].iter().any(|l| reaches(l));
                for c in charged.iter_mut().take(end + 1).skip(j) {
                    *c = true;
                }
                if body_reaches && !tagged {
                    violations.push(format!(
                        "{path}: `{name}` reaches the process-global PtyManager but carries \
                         neither {PARALLEL_TAG} nor {SERIAL_TAG}"
                    ));
                }
                i = end + 1;
            }

            for (k, l) in lines.iter().enumerate() {
                if !charged[k] && reaches(l) {
                    violations.push(format!(
                        "{path}:{}: `{}` reaches the process-global PtyManager outside any \
                         #[test] body (a shared helper?). This guard will not guess which \
                         tests call it — move the call into the tests, or tag every test in \
                         that file with {PARALLEL_TAG}",
                        k + 1,
                        l.trim()
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "untagged users of the process-global PtyManager. Without the key, \
             config::live_apply's close_all test can kill a session mid-assertion; \
             measured 3 failures in 8 runs of `cargo test -p alephcore --lib -- \
             --test-threads=16 gateway::pty config::live_apply` before the tag on \
             `a_write_reaches_a_real_subscriber_over_the_pty_screen_topic` was added.\n  {}",
            violations.join("\n  ")
        );
        for known in KNOWN_REACHERS {
            assert!(
                reaching.iter().any(|p| p.ends_with(known)),
                "the scan no longer sees {known} reaching the global PtyManager. Either the \
                 call moved (fine — update this list) or the scanner stopped working (not \
                 fine: a census that finds nothing passes vacuously). Files it did find: {reaching:?}"
            );
        }
        assert!(
            checked_tests >= 12,
            "the scanner charged only {checked_tests} test functions across {} files that \
             reach the singleton — fewer than the 12 that existed when this census was \
             written. A scanner that finds nothing passes vacuously.",
            reaching.len()
        );
    }
}
