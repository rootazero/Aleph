# Review-Results Follow-Up Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the 5 Windows/compile-verifiable review findings from `review-results/` (CMD injection, webview mic-origin, two R4 layering violations, one R3 dependency), with the macOS/Linux items deferred to their target OS.

**Architecture:** Structural fixes over sanitization: Windows shell-out replaced by `ShellExecuteW` (no cmd.exe); mic grant gated by the existing `external_link::is_internal` origin SSOT; pricing + plugin-source classification sink from the shells into the daemon; `shared/protocol` drops `uuid` for a process-local `AtomicU64` id.

**Tech Stack:** Rust (workspace crates `alephcore`, `aleph-protocol`, `aleph-tui`, `aleph-cli`, `aleph-desktop`), windows-rs 0.58, webview2-com, Tauri, serde_json.

## Global Constraints

- **Branch:** all work directly on `main` (single-branch dev mode). One commit per task.
- **Cargo economy (CLAUDE.md 极度节制):** run only **scoped** `cargo check -p <crate>` / `cargo test -p <crate> <filter> --lib` at task boundaries. No full-workspace test runs mid-flight. One final `cargo check -p alephcore` after all core-touching tasks.
- **alephcore builds are memory-heavy:** prefix heavy test compiles with `CARGO_PROFILE_TEST_DEBUG=line-tables-only` to avoid rustc OOM.
- **Redlines:** R1 — direct platform FFI is allowed only inside `desktop/*` (the limb crates). R3 — introduce no new heavy dependency. R4 — interface layer (`interfaces/*`, `desktop/shell`) stays pure I/O; business logic lives in the daemon. R7/P8 — no brittle pattern-matching for security.
- **Commit messages:** English, `<scope>: <description>`.
- **Deferred (NOT in this plan, see spec §延后):** #6 macOS `screen_record` region crop (`SCStreamConfiguration::setSourceRect`) — implement on macOS; #7 Linux `webview_perms` audio-only + origin gate — implement on Linux (won't even compile on Windows).

---

### Task 1: Drop `uuid` from `aleph-protocol` (R3)

**Files:**
- Create: `shared/protocol/src/ids.rs`
- Modify: `shared/protocol/src/lib.rs` (add module decl), `shared/protocol/src/jsonrpc.rs:74,302`, `shared/protocol/src/auth.rs:154,184,207`, `shared/protocol/Cargo.toml` (remove `uuid`)
- Test: `shared/protocol/src/ids.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `pub(crate) fn crate::ids::next_id() -> String` — process-local monotonic id string.

- [ ] **Step 1: Write the failing test**

Create `shared/protocol/src/ids.rs`:

```rust
//! Process-local monotonic id generation.
//!
//! JSON-RPC wire ids and `IdentityContext.request_id` only need to be unique
//! within the process for request/response correlation and audit tagging —
//! neither is a secret. An `AtomicU64` counter serves this without pulling
//! `uuid` (→ `rand`) into the protocol crate (R3).

use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return a fresh process-unique id, e.g. `"id-42"`. Monotonic; never repeats
/// within a process.
pub(crate) fn next_id() -> String {
    format!("id-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_monotonic() {
        let a = next_id();
        let b = next_id();
        assert_ne!(a, b);
        assert!(a.starts_with("id-"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-protocol ids_are_unique_and_monotonic --lib`
Expected: FAIL — `ids.rs` not yet declared as a module (`unresolved module` / test not found).

- [ ] **Step 3: Declare the module**

In `shared/protocol/src/lib.rs`, add after line 22 (`pub mod invitation;`):

```rust
mod ids;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-protocol ids_are_unique_and_monotonic --lib`
Expected: PASS.

- [ ] **Step 5: Replace `uuid` call sites**

In `shared/protocol/src/jsonrpc.rs`, change line 74 from:

```rust
            id: Some(Value::String(uuid_v4())),
```

to:

```rust
            id: Some(Value::String(crate::ids::next_id())),
```

and delete the now-unused helper (lines 302-304):

```rust
fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}
```

In `shared/protocol/src/auth.rs`, replace each of the three occurrences (lines 154, 184, 207) of:

```rust
            request_id: uuid::Uuid::new_v4().to_string(),
```

with:

```rust
            request_id: crate::ids::next_id(),
```

- [ ] **Step 6: Remove the dependency**

In `shared/protocol/Cargo.toml`, delete line 18:

```toml
uuid = { workspace = true, features = ["v4"] }
```

- [ ] **Step 7: Verify the crate compiles and existing tests pass**

Run: `cargo test -p aleph-protocol --lib`
Expected: PASS — including the pre-existing `test_request_creation` (asserts `req.id.is_some()`) and `test_notification_creation`.

- [ ] **Step 8: Commit**

```bash
git add shared/protocol/src/ids.rs shared/protocol/src/lib.rs shared/protocol/src/jsonrpc.rs shared/protocol/src/auth.rs shared/protocol/Cargo.toml
git commit -m "protocol: replace uuid wire/request ids with AtomicU64 counter (R3)"
```

---

### Task 2: Close Windows cmd.exe injection via `ShellExecuteW` (#1)

**Files:**
- Modify: `desktop/shared/src/action/open_path.rs:68-83`, `desktop/shared/src/action/app_launch.rs:68-86`, `desktop/shared/Cargo.toml:50-66` (add `Win32_UI_Shell` feature)
- Test: existing `rejects_empty_target` retained; verification is a Windows smoke run (ShellExecuteW launches real handlers — not unit-testable).

**Interfaces:**
- Consumes: nothing new.
- Produces: unchanged public signatures `open(target: &str) -> Result<()>`, `launch_app(app_name: &str) -> Result<()>`.

- [ ] **Step 1: Add the Shell feature**

In `desktop/shared/Cargo.toml`, inside the `cfg(target_os = "windows")` `windows` features list (lines 50-66), add:

```toml
    "Win32_UI_Shell",
```

- [ ] **Step 2: Replace the `open_path.rs` Windows block**

In `desktop/shared/src/action/open_path.rs`, replace the entire `#[cfg(target_os = "windows")]` block (lines 68-83) with:

```rust
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        // NUL-terminated wide buffers for the Win32 W API.
        let verb: Vec<u16> = std::ffi::OsStr::new("open")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let file: Vec<u16> = std::ffi::OsStr::new(target)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // ShellExecuteW hands `target` straight to the shell's association
        // resolver — it never spawns cmd.exe, so shell metacharacters in
        // `target` cannot inject a command. Returns HINSTANCE > 32 on success.
        // SAFETY: `verb`/`file` are valid NUL-terminated wide buffers that
        // outlive the call; the remaining pointer args are null as documented.
        let hinst = unsafe {
            ShellExecuteW(
                HWND(std::ptr::null_mut()),
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if hinst.0 as isize <= 32 {
            return Err(DesktopError::InputFailed(format!(
                "open: ShellExecuteW failed for '{target}' (code {})",
                hinst.0 as isize
            )));
        }
        info!(target, "Opened with default handler (Windows)");
        Ok(())
    }
```

- [ ] **Step 3: Replace the `app_launch.rs` Windows block**

In `desktop/shared/src/action/app_launch.rs`, replace the `launch_app` `#[cfg(target_os = "windows")]` block (lines 68-86) with:

```rust
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let verb: Vec<u16> = std::ffi::OsStr::new("open")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let app: Vec<u16> = std::ffi::OsStr::new(app_name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // ShellExecuteW resolves the app via file association / App Paths / PATH
        // without invoking cmd.exe, closing the metacharacter-injection vector.
        // SAFETY: `verb`/`app` are valid NUL-terminated wide buffers living past
        // the call; other pointers are null as documented.
        let hinst = unsafe {
            ShellExecuteW(
                HWND(std::ptr::null_mut()),
                PCWSTR(verb.as_ptr()),
                PCWSTR(app.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if hinst.0 as isize <= 32 {
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{app_name}' (ShellExecuteW code {})",
                hinst.0 as isize
            )));
        }
        info!(app_name, "App launched (Windows)");
        Ok(())
    }
```

- [ ] **Step 4: Compile-check the crate on Windows**

Run: `cargo check -p aleph-desktop`
Expected: PASS. If the `HWND(std::ptr::null_mut())` or `hinst.0 as isize` forms mismatch windows 0.58's handle types, adjust to the compiler's suggested form (e.g. `HWND::default()`) — the null-HWND + `>32` success check is the invariant to preserve.

- [ ] **Step 5: Run the retained unit test**

Run: `cargo test -p aleph-desktop rejects_empty_target --lib`
Expected: PASS (empty/whitespace target still rejected before the FFI call).

- [ ] **Step 6: Windows smoke verification**

Manually confirm on this Windows box (via a tiny throwaway binary or existing bridge call):
- `open("https://example.com/?a=1&b=2")` opens the default browser (the `&` is a URL query, not a shell operator).
- `open("C:/Windows/System32/notepad.exe")` and `launch_app("notepad")` launch Notepad.
- A target such as `foo & calc` yields a `ShellExecuteW failed` error and does **not** launch calc (injection closed).

- [ ] **Step 7: Commit**

```bash
git add desktop/shared/Cargo.toml desktop/shared/src/action/open_path.rs desktop/shared/src/action/app_launch.rs
git commit -m "desktop: use ShellExecuteW for open/launch to close cmd.exe injection"
```

---

### Task 3: Gate Windows webview mic grant to the Panel origin (#2)

**Files:**
- Modify: `desktop/shell/src/webview_perms.rs:65-97` (the `grant_windows` handler)
- Test: none unit-testable (COM callback); origin predicate `external_link::is_internal` is already unit-tested. Verification is a Windows smoke run.

**Interfaces:**
- Consumes: `crate::external_link::is_internal(&tauri::Url) -> bool` (existing SSOT for "is this the Panel origin").
- Produces: unchanged `pub fn grant_microphone(window: &WebviewWindow)`.

- [ ] **Step 1: Add the origin check inside the permission handler**

In `desktop/shell/src/webview_perms.rs`, in `grant_windows`, replace the handler closure (lines 84-92) so the mic grant is conditioned on the requesting origin being the Panel origin:

```rust
        let handler = PermissionRequestedEventHandler::create(Box::new(|_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            if kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                return Ok(());
            }
            // Only the Panel's own origin may auto-grant the mic. Reuse the
            // navigation SSOT so there is one definition of "Panel origin"
            // (loopback daemon / tauri.localhost / configured remote).
            let mut uri = windows::core::PWSTR::null();
            args.Uri(&mut uri)?;
            let origin_ok = (!uri.is_null())
                .then(|| unsafe { uri.to_string() }.ok())
                .flatten()
                .and_then(|s| tauri::Url::parse(&s).ok())
                .is_some_and(|u| crate::external_link::is_internal(&u));
            if origin_ok {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            }
            Ok(())
        }));
```

- [ ] **Step 2: Compile-check the shell crate on Windows**

Run: `cargo check -p aleph-desktop-shell`
Expected: PASS. If `args.Uri` needs a different out-param form under this webview2-com version, adjust to the crate's `get_Uri`/`Uri` signature — the invariant is "parse the request URI, allow only when `is_internal` is true".

- [ ] **Step 3: Windows smoke verification**

Launch the Panel, click the voice-input button, confirm `getUserMedia` still succeeds (mic granted for the Panel origin — no regression). The "foreign origin denied" branch is defense-in-depth behind `external_link::route` (which already pins the webview to the Panel origin) and is verified by code review, not a runtime step.

- [ ] **Step 4: Commit**

```bash
git add desktop/shell/src/webview_perms.rs
git commit -m "shell: restrict webview mic grant to the Panel origin (Windows)"
```

---

### Task 4: Compute session cost in the daemon (#3 server side)

**Files:**
- Modify: `src/gateway/handlers/session/db_handlers/query.rs:216-267` (add `session_usage_cost` helper + wire cost fields into the `session.usage` response)
- Test: `src/gateway/handlers/session/db_handlers/query.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `crate::pricing::estimate`, `crate::pricing::CostStatus`, `crate::orchestrator::dispatch::TokenBreakdown` (existing).
- Produces: `session.usage` JSON reply gains `cost_usd: Option<f64>` and `cost_status: "complete"|"partial_missing_price"|"unknown"`. `fn session_usage_cost(provider: Option<&str>, model: Option<&str>, input_tokens: u64, output_tokens: u64) -> (Option<f64>, &'static str)`.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` in `query.rs` (create the block if absent, `use super::*;`):

```rust
    #[test]
    fn session_usage_cost_prices_known_model() {
        let (usd, status) =
            session_usage_cost(Some("anthropic"), Some("claude-sonnet-4-6"), 1_000_000, 1_000_000);
        assert_eq!(status, "complete");
        assert!(usd.unwrap() > 0.0);
    }

    #[test]
    fn session_usage_cost_unknown_without_price() {
        assert_eq!(session_usage_cost(None, None, 100, 100), (None, "unknown"));
        assert_eq!(
            session_usage_cost(Some("anthropic"), Some("no-such-model"), 100, 100).1,
            "unknown"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore session_usage_cost --lib`
Expected: FAIL — `session_usage_cost` not defined.

- [ ] **Step 3: Add the helper**

Add above `handle_usage_db` in `query.rs`:

```rust
/// Best-effort USD cost for a session's token usage. `(None, "unknown")` when
/// the provider/model is unpriced; otherwise the total USD and serialized
/// status. Pure wrapper over `crate::pricing::estimate` so it is unit-testable
/// without a SessionStore, and so all pricing stays in core (R4 — the shells
/// no longer own a price table).
fn session_usage_cost(
    provider: Option<&str>,
    model: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
) -> (Option<f64>, &'static str) {
    let (Some(provider), Some(model)) = (provider, model) else {
        return (None, "unknown");
    };
    let breakdown = crate::orchestrator::dispatch::TokenBreakdown {
        input: u32::try_from(input_tokens).unwrap_or(u32::MAX),
        output: u32::try_from(output_tokens).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let est = crate::pricing::estimate(provider, model, &breakdown);
    match est.status {
        crate::pricing::CostStatus::Unknown => (None, "unknown"),
        crate::pricing::CostStatus::Complete => (Some(est.usd), "complete"),
        crate::pricing::CostStatus::PartialMissingPrice => {
            (Some(est.usd), "partial_missing_price")
        }
    }
}
```

- [ ] **Step 4: Wire cost into the response**

In `handle_usage_db`, replace the `JsonRpcResponse::success(...)` block (lines 248-259) with:

```rust
            let (cost_usd, cost_status) = session_usage_cost(
                session_meta.and_then(|s| s.model_provider.as_deref()),
                session_meta.and_then(|s| s.model.as_deref()),
                input_tokens,
                output_tokens,
            );

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key,
                    "tokens": total,
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "messages": message_count,
                    "created_at": created_at,
                    "last_active_at": last_active_at,
                    "cost_usd": cost_usd,
                    "cost_status": cost_status,
                }),
            )
```

- [ ] **Step 5: Run test to verify it passes**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore session_usage_cost --lib`
Expected: PASS (both cases).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/session/db_handlers/query.rs
git commit -m "gateway: compute session cost via core pricing in session.usage (R4)"
```

---

### Task 5: Render server-computed cost in the TUI; delete the shell price table (#3 client side)

**Files:**
- Delete: `interfaces/tui/src/tui/cost.rs`
- Modify: `interfaces/tui/src/tui/mod.rs:13` (remove `mod cost;`), `interfaces/tui/src/tui/commands.rs:214-252`
- Test: `interfaces/tui/src/tui/commands.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `session.usage` reply fields `cost_usd`, `cost_status` from Task 4.
- Produces: `fn cost_line(model: &str, cost_usd: Option<f64>) -> String`.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` in `commands.rs` (create if absent, `use super::*;`):

```rust
    #[test]
    fn cost_line_renders_amount_and_na() {
        assert_eq!(
            cost_line("claude-sonnet-4-6", Some(1.2345)),
            "Cost estimate (claude-sonnet-4-6): $1.2345"
        );
        assert!(cost_line("mystery-model", None).contains("n/a"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-tui cost_line_renders_amount_and_na --lib`
Expected: FAIL — `cost_line` not defined.

- [ ] **Step 3: Add `cost_line`, extend `UsageReply`, rewrite `format_usage`**

In `commands.rs`, extend the `UsageReply` struct (lines 214-223) with the amount field only — serde ignores the extra `cost_status` wire field the daemon sends, so the TUI need not carry it:

```rust
    #[serde(default)]
    cost_usd: Option<f64>,
```

Add the pure helper near `format_usage`:

```rust
/// Render the `/usage` cost line from the daemon-computed figure. Pure so it
/// is unit-testable; the TUI no longer owns any pricing (R4).
fn cost_line(model: &str, cost_usd: Option<f64>) -> String {
    match cost_usd {
        Some(usd) => format!("Cost estimate ({model}): ${usd:.4}"),
        None => format!("Cost: n/a (no pricing entry for {model})"),
    }
}
```

Replace `format_usage` (lines 236-252) with:

```rust
fn format_usage(state: &AppState, u: &UsageReply) -> String {
    vec![
        format!(
            "Session usage — messages: {}  input: {}  output: {}  total: {}",
            u.messages, u.input_tokens, u.output_tokens, u.tokens
        ),
        cost_line(&state.model_name, u.cost_usd),
    ]
    .join("\n")
}
```

- [ ] **Step 4: Remove the price table**

Delete the file `interfaces/tui/src/tui/cost.rs`, and remove line 13 (`mod cost;`) from `interfaces/tui/src/tui/mod.rs`. Delete the now-dead `use ...::cost` reference at the top of `commands.rs` (the `cost::estimate_cost` call is gone).

- [ ] **Step 5: Run test + compile-check**

Run: `cargo test -p aleph-tui cost_line_renders_amount_and_na --lib`
Expected: PASS.
Run: `cargo check -p aleph-tui`
Expected: PASS (no dangling `cost` module/import).

- [ ] **Step 6: Commit**

```bash
git add interfaces/tui/src/tui/mod.rs interfaces/tui/src/tui/commands.rs
git rm interfaces/tui/src/tui/cost.rs
git commit -m "tui: render daemon-computed cost, delete shell price table (R4)"
```

---

### Task 6: Classify plugin source in the daemon (#4 server side)

**Files:**
- Modify: `src/gateway/handlers/plugins/handlers/install.rs` (add `classify_plugin_source`, `handle_install_unified`), `src/gateway/handlers/plugins/handlers/mod.rs` (re-export `handle_install_unified` alongside `handle_install`), `src/gateway/handlers/mod.rs` (register `plugin.install`)
- Test: `src/gateway/handlers/plugins/handlers/install.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: existing `handle_install` (git clone) and `crate::gateway::handlers::plugins::handlers::marketplace::handle_marketplace_install` (marketplace-by-name).
- Produces: RPC method `plugin.install` accepting `{ source: String, scope: Option<String> }`; `fn classify_plugin_source(source: &str) -> PluginSourceKind` with `enum PluginSourceKind { Marketplace, GitUrl }`.

- [ ] **Step 1: Write the failing test**

Append to (or create) the `#[cfg(test)] mod tests` in `install.rs` (`use super::*;`):

```rust
    #[test]
    fn classify_bare_name_is_marketplace() {
        assert_eq!(classify_plugin_source("hello-world"), PluginSourceKind::Marketplace);
        assert_eq!(classify_plugin_source("my_plugin"), PluginSourceKind::Marketplace);
    }

    #[test]
    fn classify_urls_and_paths_are_git() {
        assert_eq!(
            classify_plugin_source("https://github.com/x/y"),
            PluginSourceKind::GitUrl
        );
        assert_eq!(classify_plugin_source("owner/repo"), PluginSourceKind::GitUrl);
        assert_eq!(classify_plugin_source("git@github.com:x/y.git"), PluginSourceKind::GitUrl);
        assert_eq!(classify_plugin_source("./local.thing"), PluginSourceKind::GitUrl);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore classify_ --lib`
Expected: FAIL — `classify_plugin_source` / `PluginSourceKind` not defined.

- [ ] **Step 3: Add the classifier + unified handler**

Add to `install.rs`:

```rust
/// How the daemon should install a raw `source` string. This is the R4-owned
/// classification that used to live in the CLI shell: a bare name is a
/// marketplace lookup; anything carrying a path/host/scheme separator is a
/// direct git source.
#[derive(Debug, PartialEq, Eq)]
pub enum PluginSourceKind {
    Marketplace,
    GitUrl,
}

/// Classify a plugin source. Mirrors the retired CLI heuristic verbatim:
/// only a bare identifier (no `/`, `.`, or `:`) routes to the marketplace.
pub fn classify_plugin_source(source: &str) -> PluginSourceKind {
    let bare = !source.contains('/') && !source.contains('.') && !source.contains(':');
    if bare {
        PluginSourceKind::Marketplace
    } else {
        PluginSourceKind::GitUrl
    }
}

/// Unified `plugin.install` entry: classify `source` server-side and dispatch
/// to the marketplace or git-clone installer. Keeps the shell a pure forwarder
/// (R4). Local `.zip` / `github:` sources stay client-side (they need local
/// file / GitHub I/O) and continue to use `plugins.installFromZip`.
pub async fn handle_install_unified(request: JsonRpcRequest) -> JsonRpcResponse {
    let source = request
        .params
        .as_ref()
        .and_then(|p| p.get("source"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let Some(source) = source else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing source");
    };
    let scope = request
        .params
        .as_ref()
        .and_then(|p| p.get("scope"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    match classify_plugin_source(&source) {
        PluginSourceKind::Marketplace => {
            let sub = JsonRpcRequest {
                jsonrpc: request.jsonrpc.clone(),
                method: "plugin.marketplace.install".to_string(),
                params: Some(json!({ "name": source, "scope": scope })),
                id: request.id.clone(),
            };
            super::marketplace::handle_marketplace_install(sub).await
        }
        PluginSourceKind::GitUrl => {
            let sub = JsonRpcRequest {
                jsonrpc: request.jsonrpc.clone(),
                method: "plugins.install".to_string(),
                params: Some(json!({ "url": source })),
                id: request.id.clone(),
            };
            handle_install(sub).await
        }
    }
}
```

- [ ] **Step 4: Re-export and register**

In `src/gateway/handlers/plugins/handlers/mod.rs`, add `handle_install_unified` to the same `pub use` line/list that exposes `handle_install` (so it is reachable as `plugins::handle_install_unified`, matching how `plugins::handle_install` resolves).

In `src/gateway/handlers/mod.rs`, after the `plugins.installFromZip` registration (line 299), add:

```rust
        registry.register("plugin.install", plugins::handle_install_unified);
```

- [ ] **Step 5: Run test to verify it passes**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore classify_ --lib`
Expected: PASS (both cases).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/plugins/handlers/install.rs src/gateway/handlers/plugins/handlers/mod.rs src/gateway/handlers/mod.rs
git commit -m "gateway: add plugin.install unified handler that classifies source (R4)"
```

---

### Task 7: CLI forwards to `plugin.install`; drop the shell heuristic (#4 client side)

**Files:**
- Modify: `interfaces/cli/src/main.rs:580-608` (the `PluginAction::Install` arm)
- Test: none new (RPC call path); verification is compile-check + a manual `plugin install` run.

**Interfaces:**
- Consumes: `plugin.install { source, scope }` from Task 6; existing `plugins_cmd::install` for the client-I/O cases (`github:`, local `.zip`).
- Produces: unchanged CLI surface.

- [ ] **Step 1: Replace the Install arm**

In `interfaces/cli/src/main.rs`, replace the whole `PluginAction::Install { source, scope } => { ... }` arm (lines 580-608, containing `looks_like_marketplace`) with:

```rust
        PluginAction::Install { source, scope } => {
            // Local-file / GitHub sources need client-side I/O (read the zip,
            // fetch the release) and stay here. Everything else is forwarded
            // raw to the daemon, which owns the marketplace-vs-git-url
            // classification (R4: the shell no longer decides).
            if source.starts_with("github:") || source.ends_with(".zip") {
                plugins_cmd::install(server_url, &source, json).await
            } else {
                let (client, _events) = AlephClient::connect(server_url).await?;
                let result: serde_json::Value = client
                    .call(
                        "plugin.install",
                        Some(serde_json::json!({ "source": source, "scope": scope })),
                    )
                    .await?;
                if json {
                    crate::output::print_json(&result);
                } else {
                    println!("{result}");
                }
                client.close().await?;
                Ok(())
            }
        }
```

- [ ] **Step 2: Compile-check the CLI**

Run: `cargo check -p aleph-cli`
Expected: PASS (the `looks_like_marketplace` binding is gone; `AlephClient`/`plugins_cmd` imports already present in `dispatch_plugin`).

- [ ] **Step 3: Manual verification**

Against a running daemon:
- `aleph plugin install some-name` → routed by the daemon to the marketplace.
- `aleph plugin install https://github.com/owner/repo` → routed to git clone.
- `aleph plugin install ./thing.zip` → still installs from the local zip (client I/O path unchanged).

- [ ] **Step 4: Commit**

```bash
git add interfaces/cli/src/main.rs
git commit -m "cli: forward plugin install source to daemon, drop routing heuristic (R4)"
```

---

### Final verification

- [ ] **Step 1: One consolidated core compile-check**

Run: `cargo check -p alephcore`
Expected: PASS (covers Tasks 4 and 6).

- [ ] **Step 2: Confirm the deferred items are recorded, not silently dropped**

Verify `docs/superpowers/specs/2026-07-20-review-followup-fixes-design.md` §延后 still documents #6 (macOS `setSourceRect` region crop) and #7 (Linux audio-only + origin gate) for implementation on their target OS. No code for these lands in this cycle.
