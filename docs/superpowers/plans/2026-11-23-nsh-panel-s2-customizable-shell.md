# S2 Customizable Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (native, chosen per user preference: implement after the plan lands, no extra gate). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One TSX Panel shell rendered by Desktop (embedded NSH `AppView`) and Web (NSH web package on `/v1/panel/`) from the same source, with persisted stable-ID layout, real teardown and stale-handle rejection.

**Architecture:** Runtime-neutral panel registry/projection moves into `shared/ui_logic` and is exposed to TSX by a small NSH UI set crate. TSX talks to Core only through `host.call("aleph.rpc")`; Desktop implements it over S1-B IPC, Web over S1-C `/v1/ws`. The Gateway owns one v1 method set and serves the web package from a configured directory.

**Tech Stack:** Rust 1.99.1, NSH 0.6.0 (`=0.6.0` pins), axum + tower-http 0.7 `ServeDir`, hmac 0.13 + sha2 0.11, Node 26 (`node --test`), Playwright + Chrome for web QA.

**Spec:** [docs/superpowers/specs/2026-11-23-nsh-panel-s2-customizable-shell-design.md](../specs/2026-11-23-nsh-panel-s2-customizable-shell-design.md)

## Global Constraints

- Work only in `.worktrees/nsh-panel-s2` on branch `nsh-panel-s2-spine`; never touch `main`.
- All `nsh-*` crates stay pinned `=0.6.0`; no NSH fork or upgrade.
- Default builds unchanged: `aleph-nsh-shell` without `tsx-panel` and `aleph-panel` (webchat) keep current behavior and tests.
- Moved files are deleted, never left as re-export shims. No dead code.
- Desktop TSX: local IPC target only; never falls back to WS. Remote target → `unavailable`.
- `/v1/panel/` only on the native TLS router, unmounted (404) when unconfigured, CSP `script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:; connect-src 'self'`.
- Layout key `aleph.panel.layout.v1:<endpoint>:<subject>`; layout holds only `PanelTabKey` and plain data.
- `panel.project.select` is not added to any v1 method set in S2.
- Memory-gate each cargo invocation per existing practice; Rust comments English; docs bilingual.

## Review Focus

1. A stored layout naming a tab that vanished from the snapshot → keeps an `unavailable` placeholder, never executes (Task 2 test `restore_keeps_missing_key_as_unavailable`).
2. Late RPC result after the module was hidden → dropped, no state write (Task 6 test `late_result_after_dispose_is_dropped`).
3. Web reload with an expired cookie → pairing form returns, no blank shell (Task 8 Chrome step).
4. Shared-token secret absent or rotated → hello omits `subject`, layout falls back to endpoint-only key without error (Task 4 test `hello_without_secret_omits_subject`).
5. Two `aleph/panel-surface` instances requested → second renders empty and logs, no nested `AppView` (Task 7 test `second_surface_is_refused`).

---

### Task 1: Move runtime-neutral panel model into `shared/ui_logic`

**Files:**
- Move: `interfaces/webchat/src/panel/{registry,projection,discovery,source}.rs` → `shared/ui_logic/src/panel/{registry,projection,discovery,source}.rs`
- Create: `shared/ui_logic/src/panel/mod.rs`
- Modify: `shared/ui_logic/Cargo.toml` (feature `panel = []`), `shared/ui_logic/src/lib.rs` (`#[cfg(feature = "panel")] pub mod panel;`), `interfaces/webchat/Cargo.toml` (enable `panel`), `interfaces/webchat/src/panel/mod.rs`, importers `interfaces/webchat/src/panel/{sdk,projection}.rs`, `interfaces/webchat/src/components/sidebar/projects.rs`, `interfaces/webchat/src/{app,context}.rs`

**Interfaces:** Produces `shared_ui_logic::panel::{registry::TabRegistry, projection, discovery, source}` with unchanged public names.

- [ ] Step 1: `git mv` the four files; inline tests move with them. Fix intra-crate `crate::panel::` paths to `crate::panel::` inside ui_logic and `shared_ui_logic::panel::` in webchat. `aleph-protocol` is already a ui_logic dependency.
- [ ] Step 2: Run `cargo test -p shared-ui-logic --features panel` → moved tests pass with the same count as before the move (record count from `cargo test -p aleph-panel panel::` first).
- [ ] Step 3: Run `cargo check -p aleph-panel --target wasm32-unknown-unknown` and `cargo test -p aleph-panel` → pass.
- [ ] Step 4: `rg -n "panel::(registry|projection|discovery|source)" interfaces/webchat/src` shows only `shared_ui_logic::` paths. Commit `refactor(panel): move the runtime-neutral panel model into shared-ui-logic`.

### Task 2: `PanelLayout` with restore and repair

**Files:** Create `shared/ui_logic/src/panel/layout.rs`.

**Interfaces:**
- Produces `pub struct PanelLayout { pub version: u32, pub order: Vec<PanelTabKey>, pub hidden: Vec<PanelTabKey>, pub active: Option<PanelTabKey> }` (serde), `pub const LAYOUT_VERSION: u32 = 1`, `pub fn layout_storage_key(endpoint: &str, subject: Option<&str>) -> String`, `pub fn restore(raw: Option<&str>, registry: &TabRegistry) -> RestoredLayout`, `pub struct RestoredLayout { pub layout: PanelLayout, pub slots: Vec<LayoutSlot>, pub repair: Option<LayoutRepair> }`, `pub enum LayoutSlot { Live(PanelTabKey), Unavailable(PanelTabKey) }`, `pub enum LayoutRepair { Corrupt, UnknownVersion(u32) }`, `impl PanelLayout { pub fn default_for(registry: &TabRegistry) -> Self; pub fn to_json(&self) -> String }`.

- [ ] Step 1: Tests: `restore_none_gives_default_order`; `restore_corrupt_json_resets_with_repair_corrupt`; `restore_unknown_version_resets_with_repair` (version 2 → `UnknownVersion(2)`); `restore_keeps_missing_key_as_unavailable`; `restore_appends_new_keys_at_end`; `hidden_keys_are_not_live_slots`; `active_pointing_at_hidden_or_missing_falls_back_to_first_live`; `storage_key_formats` (`("wss://h:1", Some("ab"))` → `"aleph.panel.layout.v1:wss://h:1:ab"`, `None` → `"aleph.panel.layout.v1:wss://h:1"`).
- [ ] Step 2: Run → fail (module missing). Implement. Run `cargo test -p shared-ui-logic --features panel layout` → pass. Commit `feat(panel): persistable stable-ID panel layout`.

### Task 3: One v1 method set with `panel.capabilities.snapshot`

**Files:** Modify `src/gateway/server/connection/mod.rs` (add `pub(super) const V1_METHODS: &[&str] = &["health", "sessions.list", "panel.capabilities.snapshot"];` and `pub(super) fn v1_mounted(method: &str) -> bool`), `src/gateway/server/connection/local_ipc.rs:272`, `src/gateway/server/connection/web_v1.rs:447`, tests in `src/gateway/server/connection/web_v1/tests.rs` and the local_ipc test module.

- [ ] Step 1: First confirm how `src/gateway/handlers/panel_capabilities.rs::handle` gets `PanelRequestContext` on the legacy path and whether the v1 executors provide it. If they don't, extend the v1 dispatch to provide the same caller snapshot (bound `CallerSnapshot`) — never a wider visibility. Record the finding in the commit message.
- [ ] Step 2: Tests: `v1_rejects_unmounted_method_on_both_entries` (`panel.project.select` → `METHOD_NOT_FOUND` on IPC and `/v1/ws`); `v1_capability_snapshot_matches_legacy_for_same_caller` (same descriptors/revision as `/ws` for the owner).
- [ ] Step 3: Run → fail. Replace both `matches!` with `v1_mounted`. Run `cargo test -p alephcore gateway::server::connection` → pass. Commit `feat(gateway): one v1 method set with the capability snapshot`.

### Task 4: Opaque `subject` in `HelloResult`

**Files:** Modify `shared/protocol/src/system_hello.rs:98` (`#[serde(default, skip_serializing_if = "Option::is_none")] pub subject: Option<String>`), `src/gateway/handlers/system_hello.rs` (`negotiate(request, connection_id, limits, subject: Option<String>)`; add `pub(crate) fn panel_subject(secret: &[u8; 32], user: &str) -> String`), call sites `src/gateway/handlers/system_hello.rs:22` (legacy: `None`), `src/gateway/server/connection/local_ipc.rs:124` (owner user), `src/gateway/server/connection/web_v1.rs:395` (binding user).

**Interfaces:** `panel_subject` = lowercase hex of the first 16 bytes of `HMAC-SHA256(secret, "aleph.panel.subject.v1\0" ‖ user)`; secret from `SecurityStore::get_shared_token_secret()`; `Ok(None)`/`Err` → `subject: None`.

- [ ] Step 1: Tests: protocol `hello_result_without_subject_round_trips_for_old_clients`; gateway `panel_subject_is_stable_and_user_scoped` (same inputs equal, different user differs, length 32, does not contain the user id); `hello_without_secret_omits_subject`.
- [ ] Step 2: Run → fail. Implement. Run `cargo test -p aleph-protocol system_hello`, `cargo test -p alephcore system_hello`, and `cargo test -p aleph-client` → pass. Commit `feat(protocol): opaque per-user subject in system.hello`.

### Task 5: Serve the web package on `/v1/panel/`

**Files:** Modify `src/gateway/server/mod.rs:299` (`pub panel_web_dir: Option<PathBuf>`), `src/gateway/config.rs:122` (`panel_web_dir` TOML key under `[gateway]`, default absent) and the binary wiring that maps it, `src/gateway/server/mod.rs:1355` `native_tls_router` (merge `connection::web_v1::panel_routes(dir)` only when `Some`), `src/gateway/server/connection/web_v1.rs` (`pub(in crate::gateway::server) fn panel_routes(dir: &Path) -> Router` = `nest_service("/v1/panel", ServeDir::new(dir))` + `SetResponseHeaderLayer` CSP, `X-Content-Type-Options: nosniff`).

- [ ] Step 1: Tests: `panel_route_absent_when_unconfigured` (GET `/v1/panel/index.html` → 404); `panel_route_serves_index_with_csp` (temp dir with `index.html` → 200 + exact CSP header); `panel_route_rejects_traversal` (`/v1/panel/../Cargo.toml` → not 200).
- [ ] Step 2: Run → fail. Implement. Run `cargo test -p alephcore web_v1` → pass. Commit `feat(gateway): serve the TSX panel web package on the TLS router`.

### Task 6: `interfaces/panel` — NSH mixed app, SDK and web host adapter

**Files:**
- Create `interfaces/panel/{nsh.json,package.json,tsconfig.json,App.tsx}`, `interfaces/panel/shell/{Shell.tsx,LayoutEditor.tsx,PairingForm.tsx,sdk.ts}`, `interfaces/panel/modules/{Overview.tsx,Sessions.tsx,Capabilities.tsx}`, `interfaces/panel/web/host.js`, `interfaces/panel/web/host.test.mjs`, `interfaces/panel/{Cargo.toml,src/lib.rs}` (crate `aleph-panel-ui`, own `[workspace]` like the spike, deps `nsh-core =0.6.0`, `shared-ui-logic` path with `default-features = false, features = ["panel"]`, `aleph-protocol` path)
- Create `shared/protocol/tests/fixtures/v1_frames.json` (golden vectors emitted by a new protocol test `v1_frame_golden_vectors_are_current`)
- Modify root `.gitignore` for `interfaces/panel/{.nsh,out-*,target}`

**Interfaces:**
- NSH UI set `"aleph/panel"`: `#[class] PanelModel` with `fromSnapshot(json: string)`, `tabs(): TabView[]`, `restoreLayout(raw: string | null): RestoredLayoutView`, `layoutKey(endpoint: string, subject: string | null): string`, `layoutJson(order, hidden, active): string` — thin wrappers over Task 1/2 Rust.
- `sdk.ts`: `createSdk(key: string, generation: number): PanelSdk` with `invoke(method, params)`, `subscribe(event, cb)`, `connection()`, `navigate(key)`, `theme()`, `locale()`, `dispose()`; invoke after dispose rejects `Error("disposed")`; results whose generation is not current are dropped.
- Host contract: `host.call("aleph.rpc", { method, params }) → { result } | { error: { code, message } }`; event `aleph.connection` `{ state: "connected"|"disconnected"|"unavailable", endpoint, subject }`.
- `web/host.js`: `createHost(player)` opens `wss://<origin>/v1/ws` subprotocol `aleph.protocol.v1`, 4-byte big-endian length-prefixed frames as in `shared/protocol` framing, first frame `system.hello`; pairing `fetch("/v1/session", { method: "POST", credentials: "same-origin" })` with the ticket body the existing `/v1/session` handler expects.

- [ ] Step 1: Node tests (`node --test interfaces/panel/web/host.test.mjs`): `encodes_and_decodes_golden_frames`, `correlates_out_of_order_responses`, `rejects_oversized_frame`, `first_frame_is_hello`. Run → fail; implement `host.js`; run → pass.
- [ ] Step 2: TSX tests via `nsh test interfaces/panel`: `late_result_after_dispose_is_dropped`, `invoke_after_dispose_rejects`, `hidden_module_releases_subscriptions`, `layout_round_trips_through_storage`, `missing_tab_renders_unavailable`, `dom_abi_community_tab_shows_unavailable`. Run → fail; implement shell, modules, SDK; run → pass.
- [ ] Step 3: `nsh build interfaces/panel -o interfaces/panel/out-bundle` and web package (env `AR_wasm32_unknown_unknown`, `CC_wasm32_unknown_unknown` per spike receipt) succeed. Commit `feat(panel): shared TSX shell with layout, SDK generations and web host`.

### Task 7: `nsh-shell` `tsx-panel` feature

**Files:** Modify `desktop/nsh-shell/Cargo.toml` (`tsx-panel = ["nsh-platform-macos/js", "nsh-platform-linux/js", "nsh-platform-windows/js", "dep:aleph-panel-ui"]` with target-scoped feature forms as cargo requires), `desktop/nsh-shell/src/main.rs` (select surface on `ALEPH_SHELL_PANEL=tsx` under `cfg(feature = "tsx-panel")`); Create `desktop/nsh-shell/src/panel_surface.rs`, `desktop/nsh-shell/src/panel_host.rs`, `desktop/nsh-shell/build.rs` (fail with the build command when `interfaces/panel/out-bundle/bundle.js` is missing and feature on); `justfile` target `shell-tsx-dev`.

**Interfaces:** `panel_surface::register()` installs `set_ui_sets` + `set_components` once; factory returns `PanelSurface { view: Rc<AppView>, host: panel_host::Handle }`, `Drop` → `stop()`, handler removal, IPC close; static `LIVE: AtomicBool` refuses a second surface. `panel_host::Handle::start(host: HostChannel, endpoint: PathBuf)` serves `aleph.rpc` via `AlephClient::connect_local` reusing `desktop/nsh-shell/src/ipc_status.rs:70` endpoint discovery; non-local target → `aleph.connection` `unavailable`.

- [ ] Step 1: Tests: `second_surface_is_refused`; `rpc_handler_maps_client_error_to_error_reply`; `remote_target_reports_unavailable`.
- [ ] Step 2: Run → fail. Implement. Run `cargo test -p aleph-nsh-shell --features tsx-panel` and `cargo test -p aleph-nsh-shell` (default) → both pass; `cargo tree -p aleph-nsh-shell -e features` default shows no `js`. Commit `feat(nsh-shell): opt-in embedded TSX panel surface`.

### Task 8: Real-machine QA and receipt

**Files:** Create `qa/nsh_s2_shell/` (driver reusing S1-B/S1-C isolated QA Gateway fixtures), `docs/superpowers/runs/2026-11-23-nsh-panel-s2-shell-receipt.md` + JSON; Delete `qa/nsh_s2_spike/` after the receipt references `ec2d264ac`.

- [ ] Step 1: macOS desktop: `tsx-panel` shell on isolated QA Gateway IPC — three modules show real data; reorder + hide, restart, layout restored; hiding a module zeroes its timers/subscriptions after quiescence; retained old SDK `invoke` rejected; threads return to baseline across 3 mount cycles.
- [ ] Step 2: Chrome web: TLS fixture, pairing form → `/v1/panel/` → same three modules; reload restores layout; CSP header present; expired cookie returns to pairing form.
- [ ] Step 3: Record `git ls-tree` hash of `interfaces/panel`, bundle and web package hashes, binary size delta of `tsx-panel`, versions. Linux/Windows `AppView` marked not verified. Commit `qa(nsh): S2 customizable shell receipt`.

### Task 9: Docs

**Files:** `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/DESKTOP_SHELL.md`, `docs/reference/SUBSYSTEM_ROUTING.md`, roadmap S2 row, and other references the change touches.

- [ ] Step 1: Add S2 entries (paths from Tasks 1–7), fix stale paths found by checking every linked path in those files with a script, keep current-vs-target wording consistent with code. Commit `docs: locate the S2 TSX panel shell`.
