# Gateway Auth UX — Phase 1: Silent Shell Bootstrap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate "find and paste gateway token" friction during first install of the Tauri desktop app. Token becomes invisible — the Panel inside the desktop window auto-authenticates with zero user action.

**Architecture:** Reuse the existing `?token=` URL-param auto-login path that `interfaces/webchat/src/context.rs:284-313` already implements. Daemon ships a new `aleph-server bootstrap-token` subcommand that reads the shared token from `~/.aleph/data/security.db` (same-UID filesystem gate, identical threat model to `aleph secret list`). The Tauri shell spawns the subcommand on startup, captures the token from stdout, and appends `?token=…` to the navigation URL once. The Panel's existing code stores it in `localStorage` and immediately replaces the address bar so the token never appears to the user. The stderr token banner that previously leaked the token into log captures and screen-share recordings is removed.

**Tech Stack:** Rust 1.x (alephcore + aleph-server binary), Tauri 2.x (desktop shell), Leptos WASM (Panel UI), clap (CLI), `url` crate for query manipulation, `tempfile` + `tokio::process` for subcommand tests.

**Out of scope for this phase (Phase 2+):**
- Bootstrap nonce HTTP endpoint (`/auth/bootstrap?nonce=…`) and `gateway.bootstrap.issue` RPC
- `aleph open` CLI / "Open in Browser" Tauri menu
- Replacing `/login` HTML form
- Browser pairing UX for cold-visit and remote scenarios
- Deprecating `aleph auth show-token`

These are intentionally deferred — Phase 1 ships a working, testable improvement for the most common case (first install via desktop app) without touching the gateway HTTP surface.

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/bin/aleph-server/cli.rs` | Modify | Add `BootstrapToken` variant to `Command` enum |
| `src/bin/aleph-server/commands/mod.rs` | Modify | Re-export new `bootstrap_token` module |
| `src/bin/aleph-server/commands/bootstrap_token.rs` | **Create** | Handler: read shared token from DB, print to stdout |
| `src/bin/aleph-server/main.rs` | Modify | Dispatch `Command::BootstrapToken` |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs:143-164` | Modify | Remove stderr token banner; replace with single quiet `info!` line |
| `desktop/shell/src/daemon.rs` | Modify | New `load_bootstrap_token()` helper that spawns `aleph-server bootstrap-token` and captures stdout; modify `navigate_to_panel()` to accept optional token and build URL with `?token=` |
| `desktop/shell/src/main.rs` | Modify | Wire bootstrap token through `spawn_background` → `reveal_panel` → `navigate_to_panel` |

**No changes** to `interfaces/webchat/src/context.rs` — its existing `?token=` handler at lines 284-313 already covers Phase 1's needs.

**Test files:**
- `src/bin/aleph-server/commands/bootstrap_token.rs` — embedded `#[cfg(test)]` mod with `tempfile`-backed `SecurityStore`
- `tests/bootstrap_token_subprocess.rs` (**create**) — top-level integration test that runs the binary as a subprocess
- `desktop/shell/src/daemon.rs` — embedded `#[cfg(test)]` mod for `navigate_to_panel` URL building (pure helper test)

---

## Task 1: Add `BootstrapToken` subcommand variant

**Files:**
- Modify: `src/bin/aleph-server/cli.rs:69` (Command enum)

- [ ] **Step 1: Write the failing test (CLI parses new subcommand)**

Append to `src/bin/aleph-server/cli.rs`'s existing `#[cfg(test)] mod tests` (or create if none — verify with `grep -n "#\[cfg(test)\]" src/bin/aleph-server/cli.rs`).

```rust
#[test]
fn parses_bootstrap_token_subcommand() {
    use clap::Parser;
    let args = Args::try_parse_from(["aleph-server", "bootstrap-token"]).unwrap();
    assert!(matches!(args.command, Some(Command::BootstrapToken)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin aleph-server -- parses_bootstrap_token_subcommand`
Expected: FAIL with `error[E0599]: no variant or associated item named BootstrapToken`

- [ ] **Step 3: Add the variant**

In `src/bin/aleph-server/cli.rs` `pub enum Command { … }` (line 69), add:

```rust
    /// Print the auto-provisioned shared token to stdout (one line, no banner).
    ///
    /// Used by the desktop shell to silently bootstrap the embedded Panel —
    /// reads `~/.aleph/data/security.db` directly (same-UID gate). Exits with
    /// code 64 (EX_USAGE) and a stderr message if no token has been provisioned
    /// yet (i.e. the server has never started).
    BootstrapToken,
```

Place it alphabetically near `Audit` / `BootstrapRuntime` if the file sorts variants; otherwise append at the end of the enum just before its closing brace.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin aleph-server -- parses_bootstrap_token_subcommand`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/cli.rs
git commit -m "aleph-server: add BootstrapToken CLI variant"
```

---

## Task 2: Implement `bootstrap_token` handler with unit test

**Files:**
- Create: `src/bin/aleph-server/commands/bootstrap_token.rs`
- Modify: `src/bin/aleph-server/commands/mod.rs:1-15`

- [ ] **Step 1: Write the failing unit test**

Create `src/bin/aleph-server/commands/bootstrap_token.rs` with **only** the test module first:

```rust
//! `bootstrap-token` subcommand — prints the auto-provisioned shared token
//! so the desktop shell can silently authenticate the embedded Panel.
//!
//! Same threat model as `secret list`: reads `~/.aleph/data/security.db`
//! directly (file mode 0600 enforced by SQLite + OS), no daemon required.

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn returns_existing_token_when_db_has_one() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("security.db");
        let vault_path = dir.path().join("secrets.vault");

        let store = Arc::new(SecurityStore::open(&db_path).expect("open store"));
        let mgr = SharedTokenManager::new(store, vault_path);
        let expected = mgr.generate_token().expect("generate");

        let out = read_token_from_db(&db_path, dir.path()).expect("read");
        assert_eq!(out, expected);
        assert!(out.starts_with("aleph-"), "expected aleph-<uuid> format");
    }

    #[test]
    fn returns_none_when_db_empty() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("security.db");
        // Create the DB but never call generate_token().
        let store = Arc::new(SecurityStore::open(&db_path).expect("open store"));
        let _ = SharedTokenManager::new(store, dir.path().join("secrets.vault"));

        assert!(read_token_from_db(&db_path, dir.path()).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails (function does not exist)**

Run: `cargo test --bin aleph-server commands::bootstrap_token`
Expected: FAIL with `error[E0425]: cannot find function read_token_from_db`

- [ ] **Step 3: Implement the helper + dispatch function**

Add the production code above the `#[cfg(test)]` block in `src/bin/aleph-server/commands/bootstrap_token.rs`:

```rust
use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
use std::error::Error;
use std::io::Write;
use std::sync::Arc;

/// Read the shared token from `db_path` if one has been provisioned.
/// `data_dir` is used to locate the secrets vault (its existence is not
/// required for token retrieval, but `SharedTokenManager::new` opens it).
///
/// Returns `None` when the DB has no plaintext token (first-run state).
pub fn read_token_from_db(db_path: &Path, data_dir: &Path) -> Option<String> {
    let store = Arc::new(SecurityStore::open(db_path).ok()?);
    let vault_path = data_dir.join("secrets.vault");
    let mgr = SharedTokenManager::new(store, vault_path);
    mgr.try_load_token_from_db()
}

/// Handle the `aleph-server bootstrap-token` subcommand.
///
/// Resolves the standard `~/.aleph/data/` paths via `alephcore::utils::paths`,
/// then prints the token to stdout followed by a single newline (no banner,
/// no decoration — the shell parses it). Exits with EX_USAGE (64) and a
/// stderr message when no token exists.
pub fn handle_bootstrap_token() -> Result<(), Box<dyn Error>> {
    use alephcore::utils::paths;

    let db_path = paths::get_security_db_path()
        .map_err(|e| format!("resolve security DB path: {e}"))?;
    let data_dir = db_path
        .parent()
        .ok_or("security DB has no parent directory")?
        .to_path_buf();

    match read_token_from_db(&db_path, &data_dir) {
        Some(token) => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            writeln!(handle, "{token}")?;
            Ok(())
        }
        None => {
            eprintln!(
                "aleph-server: no shared token provisioned yet — start the \
                 server once (`aleph-server start`) to generate one."
            );
            std::process::exit(64); // EX_USAGE per sysexits.h
        }
    }
}
```

- [ ] **Step 4: Wire module into `commands/mod.rs`**

Edit `src/bin/aleph-server/commands/mod.rs` — add `pub mod bootstrap_token;` after `pub mod bootstrap_runtime;` (line 6), and add `pub use bootstrap_token::handle_bootstrap_token;` to the re-exports section (after line 23):

```rust
pub mod audit;
pub mod bootstrap_runtime;
pub mod bootstrap_token;
pub mod devices;
// ...
pub use audit::*;
pub use bootstrap_token::handle_bootstrap_token;
pub use devices::*;
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test --bin aleph-server commands::bootstrap_token`
Expected: PASS (2 tests)

- [ ] **Step 6: Commit**

```bash
git add src/bin/aleph-server/commands/bootstrap_token.rs src/bin/aleph-server/commands/mod.rs
git commit -m "aleph-server: bootstrap-token command — read DB token for shell"
```

---

## Task 3: Dispatch `Command::BootstrapToken` in `main.rs`

**Files:**
- Modify: `src/bin/aleph-server/main.rs:120-148` (the synchronous early-dispatch block)

- [ ] **Step 1: Write the failing integration test (subprocess execution)**

Create `tests/bootstrap_token_subprocess.rs`:

```rust
//! End-to-end smoke test for `aleph-server bootstrap-token`.
//!
//! Runs the binary in a subprocess against a tempdir-redirected `$HOME` so
//! we don't touch the user's real `~/.aleph/data/`.

use std::process::Command;
use tempfile::tempdir;

fn aleph_server_bin() -> String {
    // CARGO_BIN_EXE_<name> is populated for any binary in the package tests.
    env!("CARGO_BIN_EXE_aleph-server").to_string()
}

#[test]
fn bootstrap_token_exits_64_when_no_token_provisioned() {
    let home = tempdir().expect("tempdir");
    let output = Command::new(aleph_server_bin())
        .arg("bootstrap-token")
        .env("HOME", home.path())
        .env_remove("ALEPH_HOME")
        .output()
        .expect("spawn aleph-server bootstrap-token");

    assert_eq!(output.status.code(), Some(64), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty(), "stdout should be empty on EX_USAGE");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no shared token provisioned"),
        "stderr should mention provisioning: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
```

(We do not test the success path in this integration test because seeding the DB requires running the full `start` flow once; that's covered by the unit tests in Task 2.)

- [ ] **Step 2: Run test to verify it fails (unknown subcommand)**

Run: `cargo test --test bootstrap_token_subprocess`
Expected: FAIL — subcommand parses but main.rs falls through with `unreachable!()` or returns an error, exit code != 64.

- [ ] **Step 3: Add the dispatch arm**

In `src/bin/aleph-server/main.rs`, locate the **synchronous** early-dispatch block (currently ends around line 148 with `SandboxInitWindows`). Add a new arm BEFORE the daemonize check at line 153:

```rust
        Some(Command::BootstrapToken) => return commands::handle_bootstrap_token(),
```

Also append `Command::BootstrapToken` to the `unreachable!()` arm at line 242-248 so the async dispatcher doesn't see it again:

```rust
        Some(Command::Stop)
        | Some(Command::Secret { .. })
        | Some(Command::Status { .. })
        | Some(Command::Devices { .. })
        | Some(Command::Hooks { .. })
        | Some(Command::BootstrapToken)
        | Some(Command::SandboxInit { .. })
        | Some(Command::SandboxInitWindows { .. }) => unreachable!(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test bootstrap_token_subprocess`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/main.rs tests/bootstrap_token_subprocess.rs
git commit -m "aleph-server: dispatch BootstrapToken in main, add subprocess test"
```

---

## Task 4: Remove stderr token banner

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs:143-164`

- [ ] **Step 1: Identify all banner lines and the surrounding logic**

Read `src/bin/aleph-server/commands/start/builder/subsystems.rs:143-164`. Confirm the structure matches:

```rust
    if auth_mode.is_auth_required() {
        match shared_token_mgr.try_load_token_from_db() {
            Some(token) => {
                info!("========================================");
                info!("  Access token (existing): {}", token);
                info!("========================================");
            }
            None => {
                match shared_token_mgr.generate_token() {
                    Ok(token) => {
                        info!("========================================");
                        info!("  Access token (new): {}", token);
                        info!("========================================");
                    }
                    Err(e) => { warn!("Failed to generate shared token: {}", e); }
                }
            }
        }
    }
```

- [ ] **Step 2: Replace banner with silent provisioning + one quiet info line**

Replace lines 143-164 with:

```rust
    if auth_mode.is_auth_required() {
        match shared_token_mgr.try_load_token_from_db() {
            Some(_) => {
                info!(
                    "auth token ready (loaded from DB) — desktop app auto-injects; \
                     CLI users: run `aleph-server bootstrap-token` to retrieve"
                );
            }
            None => match shared_token_mgr.generate_token() {
                Ok(_) => {
                    info!(
                        "auth token provisioned (first start) — desktop app \
                         auto-injects; CLI users: run `aleph-server bootstrap-token`"
                    );
                }
                Err(e) => {
                    warn!("Failed to generate shared token: {}", e);
                }
            },
        }
    }
```

Rationale: the banner printed the plaintext token to journald / Console.app / screen captures during demos. The new lines never disclose the token; users learn the retrieval path instead.

- [ ] **Step 3: Run existing tests to confirm no regression**

Run: `cargo check -p alephcore && cargo test --bin aleph-server -- --skip bootstrap_token_subprocess`
Expected: PASS (no logic change beyond log strings)

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/subsystems.rs
git commit -m "aleph-server: remove stderr token banner (token leaks in logs/screencast)"
```

---

## Task 5: Shell — load bootstrap token via subprocess

**Files:**
- Modify: `desktop/shell/src/daemon.rs` (add helper + tests)
- Modify: `desktop/shell/Cargo.toml` if `url` crate not already a dependency (verify with `grep '^url' desktop/shell/Cargo.toml`)

- [ ] **Step 1: Verify `url` crate availability**

Run: `grep -n "^url " desktop/shell/Cargo.toml || grep -n '"url"' desktop/shell/Cargo.toml`

If absent, add `url = "2"` under `[dependencies]` in `desktop/shell/Cargo.toml`. If `tauri` already pulls it transitively (likely), you still need it as a direct dep for explicit imports.

- [ ] **Step 2: Write failing test for URL helper**

Add to `desktop/shell/src/daemon.rs` (append at end of file, or wherever `#[cfg(test)]` block lives):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_panel_url_appends_token_query() {
        let url = build_panel_url(Some("aleph-deadbeef")).expect("build url");
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(18790));
        let token_param = url
            .query_pairs()
            .find(|(k, _)| k == "token")
            .map(|(_, v)| v.into_owned());
        assert_eq!(token_param.as_deref(), Some("aleph-deadbeef"));
    }

    #[test]
    fn build_panel_url_without_token_has_no_query() {
        let url = build_panel_url(None).expect("build url");
        assert!(url.query().is_none(), "expected no query, got {:?}", url.query());
    }

    #[test]
    fn build_panel_url_token_is_url_encoded() {
        // Even though our tokens are aleph-<uuid> and never need escaping,
        // we should not silently double-encode if a future token format
        // contains reserved characters.
        let url = build_panel_url(Some("with space&amp;")).expect("build url");
        let token = url
            .query_pairs()
            .find(|(k, _)| k == "token")
            .map(|(_, v)| v.into_owned())
            .expect("token present");
        // query_pairs() decodes, so we get the original value back.
        assert_eq!(token, "with space&amp;");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p aleph-desktop-shell daemon::tests`
(If the package name differs, run `grep -n '^name' desktop/shell/Cargo.toml` to find it.)
Expected: FAIL — `build_panel_url` does not exist.

- [ ] **Step 4: Implement `build_panel_url` and `load_bootstrap_token`**

Add to `desktop/shell/src/daemon.rs` (alongside existing helpers like `resolve_daemon_binary`):

```rust
use url::Url;

/// Construct the Panel URL, optionally with a one-time `?token=` query
/// param that the Panel consumes on first load and immediately removes
/// from the address bar via `history.replaceState` (see
/// `interfaces/webchat/src/context.rs:284-313`).
pub(crate) fn build_panel_url(token: Option<&str>) -> Result<Url, url::ParseError> {
    let mut url: Url = super::PANEL_URL.parse()?;
    if let Some(t) = token {
        url.query_pairs_mut().append_pair("token", t);
    }
    Ok(url)
}

/// Spawn `aleph-server bootstrap-token` and read the shared token from
/// stdout. Returns `None` on any failure (binary missing, no token
/// provisioned yet, parse error) so the shell can still boot and fall
/// back to the manual pairing modal.
pub(crate) fn load_bootstrap_token() -> Option<String> {
    let bin = resolve_daemon_binary()?;
    let output = std::process::Command::new(&bin)
        .arg("bootstrap-token")
        .output()
        .ok()?;
    if !output.status.success() {
        // Exit 64 = no token yet (first install, never started). That is
        // fine — the Panel will show the pairing modal as today.
        tracing::info!(
            status = ?output.status.code(),
            "bootstrap-token returned non-zero — falling back to pairing"
        );
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let token = raw.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p aleph-desktop-shell daemon::tests`
Expected: PASS (3 tests)

- [ ] **Step 6: Commit**

```bash
git add desktop/shell/src/daemon.rs desktop/shell/Cargo.toml
git commit -m "shell: load_bootstrap_token + build_panel_url with ?token= query"
```

---

## Task 6: Wire bootstrap token into navigation flow

**Files:**
- Modify: `desktop/shell/src/main.rs` (around `PANEL_URL`, `navigate_to_panel`, `reveal_panel`)
- Modify: `desktop/shell/src/daemon.rs` — accept token in `navigate_to_panel` rewrite

- [ ] **Step 1: Locate the current navigation chain**

Re-read `desktop/shell/src/main.rs:29` (`const PANEL_URL`), `desktop/shell/src/daemon.rs:258-271` (`navigate_to_panel`), and `desktop/shell/src/main.rs:287-290` (`reveal_panel`). Confirm the flow is: `spawn_background` → `daemon::ensure_ready().await` → `reveal_panel(&handle)` → `navigate_to_panel(handle)` → `window.navigate(PANEL_URL.parse()?)`.

- [ ] **Step 2: Change `navigate_to_panel` signature to accept optional token**

In `desktop/shell/src/daemon.rs:258`, change:

```rust
fn navigate_to_panel(handle: &tauri::AppHandle) {
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot reach the Panel");
        return;
    };
    match PANEL_URL.parse() {
        Ok(url) => {
            if let Err(e) = window.navigate(url) {
                tracing::error!("failed to navigate to the Panel: {e}");
            }
        }
        Err(e) => tracing::error!("invalid Panel URL: {e}"),
    }
}
```

to:

```rust
pub(crate) fn navigate_to_panel(handle: &tauri::AppHandle, token: Option<&str>) {
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot reach the Panel");
        return;
    };
    match build_panel_url(token) {
        Ok(url) => {
            if let Err(e) = window.navigate(url) {
                tracing::error!("failed to navigate to the Panel: {e}");
            }
        }
        Err(e) => tracing::error!("invalid Panel URL: {e}"),
    }
}
```

- [ ] **Step 3: Update `reveal_panel` to fetch and pass the token**

In `desktop/shell/src/main.rs:287-290`:

```rust
fn reveal_panel(handle: &tauri::AppHandle) {
    let token = daemon::load_bootstrap_token();
    daemon::navigate_to_panel(handle, token.as_deref());
    focus_window(handle);
}
```

- [ ] **Step 4: Update all other `navigate_to_panel` callers**

Run: `grep -rn "navigate_to_panel" desktop/shell/src/`

For each remaining caller (likely `supervise_daemon` recovery reload, possibly tray "Show Panel" action), decide whether the token is needed:
- **First reveal (`reveal_panel`)**: yes — token may be needed (Panel has no localStorage yet).
- **Reload after daemon recovery**: pass `None` — the Panel webview retains localStorage across navigations within the same window lifetime, so the device token from the previous successful auth is still there.
- **Tray "Show Panel" / re-focus**: pass `None` for the same reason.

Apply the corresponding `None` arguments so compilation succeeds. Add a short `// token only on first reveal — Panel keeps localStorage thereafter` comment at each `None` call site.

- [ ] **Step 5: Compile check**

Run: `cargo check -p aleph-desktop-shell`
Expected: PASS — no `expected 1 argument, found 0` errors.

- [ ] **Step 6: Re-run shell tests**

Run: `cargo test -p aleph-desktop-shell`
Expected: PASS (all existing + the 3 from Task 5).

- [ ] **Step 7: Commit**

```bash
git add desktop/shell/src/main.rs desktop/shell/src/daemon.rs
git commit -m "shell: inject ?token= on first Panel reveal for silent bootstrap"
```

---

## Task 7: End-to-end manual verification

**Files:** none modified — this is a runtime verification step.

- [ ] **Step 1: Clean state — wipe local data dir (USE A TEMP HOME OR BACKUP FIRST)**

```bash
# WARNING: this destroys your local Aleph state. Back up ~/.aleph first if you
# have anything you want to keep (memories, secrets, pairings).
mv ~/.aleph ~/.aleph.backup-$(date +%s) 2>/dev/null || true
```

- [ ] **Step 2: Build the desktop app in dev mode**

Run: `just shell-dev`
Expected: Tauri window opens. With clean state, this is "first install" behavior.

- [ ] **Step 3: Verify zero-friction Panel auth**

In the Tauri window:
- The splash should appear briefly, then transition to the Panel.
- The Panel header / connection chip should turn green ("Connected") **without** showing the PairingModal.
- Open the Tauri webview devtools (Cmd+Option+I on macOS dev build) → Application → Local Storage → `http://127.0.0.1:18790` → confirm `aleph_shared_token` and `aleph_device_token` are populated.

- [ ] **Step 4: Verify token is NOT in any visible log**

Run: `just shell-dev 2>&1 | grep -iE "(access token|aleph-[0-9a-f]{8})" || echo "OK: no token in logs"`
Expected: `OK: no token in logs` — the new info lines do not contain the token plaintext.

- [ ] **Step 5: Verify `bootstrap-token` works manually**

Run: `./target/debug/aleph-server bootstrap-token`
Expected: Prints `aleph-<uuid>` followed by a newline. Exit code 0.

- [ ] **Step 6: Verify the CLI fallback path still works**

Run: `cargo run --bin aleph-cli -- auth show-token`
Expected: Returns the same token (RPC route still alive — Phase 4 will deprecate it).

- [ ] **Step 7: Restore your data dir**

```bash
rm -rf ~/.aleph
mv ~/.aleph.backup-* ~/.aleph 2>/dev/null || true
```

- [ ] **Step 8: Commit verification notes (optional, no code changes)**

If anything in steps 3–6 misbehaved, return to the earlier task and fix. Otherwise no commit needed — verification is complete.

---

## Self-Review Checklist

After implementing all tasks, verify:

1. **Goal achieved?** Fresh install of the desktop app reaches a "connected" Panel state with zero user-visible token interaction. ✓ (Tasks 5+6+7)
2. **Banner removed?** No token plaintext printed to stderr / journald / Console.app. ✓ (Task 4)
3. **CLI fallback preserved?** `aleph auth show-token` still works (Phase 1 does not deprecate it; that's Phase 4). ✓ (No changes to that RPC)
4. **Tests cover the new code?**
   - CLI parse test: Task 1
   - Handler unit tests (with-token, no-token): Task 2
   - Subprocess integration test: Task 3
   - URL builder tests: Task 5
   - End-to-end manual: Task 7
5. **No leak of token into URL history?** Panel's existing `history.replaceState` at `interfaces/webchat/src/context.rs:302-309` removes the `?token=` from the address bar immediately. ✓ (no changes needed)
6. **Same-UID threat model preserved?** `bootstrap-token` reads the same DB that `secret list` reads; both rely on filesystem mode 0600 set by SQLite + OS. No new attack surface. ✓
7. **R10 thin-shell respected?** Shell adds 2 small helpers (`load_bootstrap_token`, `build_panel_url`) and a CLI subprocess call — no pulling alephcore into shell crate, no Tauri IPC bridge. ✓
8. **R1/R2 not violated?** Shell doesn't grow business logic; it just spawns a binary and appends a URL param. ✓

---

## Verification Commands (Definition of Done)

Run all of these and confirm PASS:

```bash
# 1. Unit + integration tests
cargo test --bin aleph-server commands::bootstrap_token
cargo test --test bootstrap_token_subprocess
cargo test -p aleph-desktop-shell daemon::tests

# 2. Compile check
cargo check -p alephcore
cargo check -p aleph-desktop-shell

# 3. Lint
cargo clippy --bin aleph-server --tests -- -D warnings
cargo clippy -p aleph-desktop-shell -- -D warnings

# 4. Format
cargo fmt --check

# 5. Targeted regression
cargo test --bin aleph-server -- --skip bootstrap_token_subprocess
```

---

## Risk Notes

- **`alephcore::utils::paths::get_security_db_path`** must respect `$ALEPH_HOME` if that env var is honored elsewhere (see CLAUDE.md note on `$ALEPH_HOME`). If the integration test in Task 3 fails because the binary writes to the real `~/.aleph` instead of the temp `$HOME`, switch to setting `ALEPH_HOME` instead of `HOME` in the test.
- **`url` crate transitive availability**: Tauri already pulls `url`. The explicit dependency is for `use url::Url` clarity. Direct dep does not bloat the binary.
- **stdout buffering**: `writeln!` + flush via the locked stdout handle is fine; subprocess reads to EOF in Task 5.
- **Concurrent first-start race**: not possible — the daemon's flock prevents two `start` invocations. `bootstrap-token` doesn't take the flock (read-only), so it can race with first-start. If `try_load_token_from_db` returns `None` *during* the first start, the shell's `load_bootstrap_token` returns `None` and the Panel falls back to pairing modal — graceful, not a bug.
