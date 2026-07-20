# Gateway Auth UX — Phase 4: Cleanup & Deprecation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Once Phases 1-3 have shipped one release cycle and the new flows are proven, retire the legacy token-paste surfaces so the "gateway token" concept disappears from user-facing code paths entirely. This phase is intentionally small and surgical — the heavy lifting is done; we're just removing dead and deprecated paths and updating docs to match reality.

**Architecture:** No new architecture. Pure deletions, demotions, and doc updates.

**Tech Stack:** Same as Phases 1-3 (no new deps).

---

## Pre-flight check (before starting Phase 4)

Run these to confirm Phases 1-3 are merged and stable on `main`:

```bash
# Phase 1 (silent shell bootstrap)
grep -n "BootstrapToken" src/bin/aleph-server/cli.rs    # → variant exists
grep -n "Access token (new)" src/bin/aleph-server/      # → no matches (banner removed)

# Phase 2 (bootstrap nonce)
grep -n "BootstrapNonceManager" src/gateway/bootstrap.rs # → exists
grep -n "gateway.bootstrap.issue" src/bin/aleph-server/  # → registered

# Phase 3 (browser pairing)
grep -n "PairingRequest::Browser" src/gateway/security/pairing.rs  # → exists
grep -n "pairing.start_browser" src/bin/aleph-server/    # → registered
```

If any of these fail, stop and finish the previous phase first.

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/auth_middleware.rs:36-40,46-77,160-205` | Delete | `show_login`, `handle_login`, `login_page_html` and their `/login` + `/auth/login` route mounts |
| `src/gateway/auth_middleware.rs:207-260` | Delete | All `login_page_html` related tests |
| `src/gateway/auth_probe_tests.rs` | Modify | Remove `login_page_html` import (line 13) + any login-form tests |
| `interfaces/cli/src/commands/cli_args.rs:1019` | Modify | Move `AuthAction::ShowToken` and `ResetToken` under a new `AuthDebugAction` subcommand (`aleph auth debug show-token`); keep functional but require the explicit `debug` namespace so casual users don't find it |
| `interfaces/cli/src/main.rs:229-242` | Modify | Update dispatcher for the new `AuthAction::Debug { action }` |
| `interfaces/cli/src/commands/auth_cmd.rs` | Modify | Add a one-time deprecation `eprintln!` to `show_token` directing users to the desktop app / `aleph open` |
| `desktop/shell/src/daemon.rs:build_panel_url:legacy_token` parameter (Phase 2 Task 7) | Modify | Delete the `legacy_token` parameter and `?token=` fallback path now that the daemon is guaranteed to support nonce-issue |
| `interfaces/webchat/src/context.rs:284-313` | Modify | Delete the `?token=` URL-param auto-login path (the cookie-based bootstrap from Phase 2 makes it dead) |
| `docs/reference/SECURITY.md` | Modify | New "Auth UX" section describing the trust-transfer + pairing model; remove any mention of "paste the access token into the Panel login form" |
| `docs/reference/SERVER_DEVELOPMENT.md` | Modify | Update the "first start" walkthrough to refer to the desktop app + `aleph open`, not the stderr banner |
| `CLAUDE.md` | Modify | Add a short note under the "进程管理" section: "Auth tokens are auto-provisioned; users never see them. See `docs/reference/SECURITY.md#auth-ux` for the trust model." |
| `README.md` (project root) | Modify (if it has install instructions) | Replace any "copy the token from the log" instructions with desktop-app-first onboarding |

---

## Task 1: Demote `aleph auth show-token` to `aleph auth debug show-token`

**Files:**
- Modify: `interfaces/cli/src/commands/cli_args.rs:1019` (`AuthAction` enum)
- Modify: `interfaces/cli/src/main.rs:229-242` (dispatcher)
- Modify: `interfaces/cli/src/commands/auth_cmd.rs:35` (`show_token` — add deprecation note to JSON output)

- [ ] **Step 1: Failing test for new sub-subcommand path**

In `interfaces/cli/src/main.rs:tests` (around line 1039):

```rust
#[test]
fn parses_auth_debug_show_token() {
    assert!(Cli::try_parse_from(["aleph", "auth", "debug", "show-token"]).is_ok());
}

#[test]
fn legacy_auth_show_token_still_parses_but_is_hidden() {
    // Backward compat: existing scripts using `aleph auth show-token` must
    // still work for one release cycle, but the variant is hidden from help.
    assert!(Cli::try_parse_from(["aleph", "auth", "show-token"]).is_ok());
}
```

- [ ] **Step 2: Run, observe failure**

Run: `cargo test -p aleph-cli parses_auth_debug_show_token`
Expected: FAIL — `debug` subcommand unknown.

- [ ] **Step 3: Restructure `AuthAction`**

In `interfaces/cli/src/commands/cli_args.rs:1019`:

```rust
#[derive(Debug, Subcommand)]
pub(crate) enum AuthAction {
    /// Show the access token (legacy — prefer the desktop app or `aleph open`).
    #[command(hide = true)]   // hide from help, but still parses
    ShowToken,

    /// Reset (regenerate) the access token. (legacy)
    #[command(hide = true)]
    ResetToken {
        #[arg(short, long)]
        yes: bool,
    },

    /// Debug surfaces (token introspection, session listing).
    Debug {
        #[command(subcommand)]
        action: AuthDebugAction,
    },

    /// Active gateway sessions.
    Sessions,

    // ... existing variants (RevokeSession, Login, Logout, OauthStatus) unchanged ...
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthDebugAction {
    /// Show the access token (developer / break-glass use only).
    ShowToken,
    /// Reset (regenerate) the access token.
    ResetToken {
        #[arg(short, long)]
        yes: bool,
    },
}
```

- [ ] **Step 4: Update dispatcher**

In `interfaces/cli/src/main.rs:229`:

```rust
async fn dispatch_auth(server_url: &str, action: AuthAction, json: bool) -> CliResult<()> {
    use commands::auth_cmd;
    match action {
        AuthAction::ShowToken => {
            eprintln!("warning: `aleph auth show-token` is deprecated; use `aleph open` or the desktop app");
            auth_cmd::show_token(server_url, json).await
        }
        AuthAction::ResetToken { yes } => {
            eprintln!("warning: `aleph auth reset-token` is deprecated; use `aleph auth debug reset-token`");
            auth_cmd::reset_token(server_url, yes, json).await
        }
        AuthAction::Debug { action } => match action {
            AuthDebugAction::ShowToken => auth_cmd::show_token(server_url, json).await,
            AuthDebugAction::ResetToken { yes } => auth_cmd::reset_token(server_url, yes, json).await,
        },
        AuthAction::Sessions => auth_cmd::sessions(server_url, json).await,
        AuthAction::RevokeSession { session_id } => {
            auth_cmd::revoke_session(server_url, &session_id, json).await
        }
        AuthAction::Login { provider } => auth_cmd::login(server_url, &provider, json).await,
        AuthAction::Logout { provider } => auth_cmd::logout(server_url, &provider, json).await,
        AuthAction::OauthStatus { provider } => {
            auth_cmd::oauth_status(server_url, &provider, json).await
        }
    }
}
```

Add `use cli_args::{AuthAction, AuthDebugAction};` if not already imported.

- [ ] **Step 5: Run tests**

Run: `cargo test -p aleph-cli auth_debug && cargo test -p aleph-cli legacy_auth`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add interfaces/cli/src/commands/cli_args.rs interfaces/cli/src/main.rs
git commit -m "cli: aleph auth show-token demoted to debug (deprecation warning)"
```

---

## Task 2: Delete the `/login` HTML form

**Files:**
- Modify: `src/gateway/auth_middleware.rs` — delete routes + handlers + helper + tests
- Modify: `src/gateway/auth_probe_tests.rs:13` — remove `login_page_html` import

- [ ] **Step 1: Failing test — `/login` returns 404 (or redirects to /pair)**

In `src/gateway/auth_middleware.rs:tests`:

```rust
#[tokio::test]
async fn login_route_returns_404_or_redirects_to_pair() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let state = test_state();
    let app = auth_routes(state);
    let resp = app
        .oneshot(Request::builder().uri("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    // Either 404 or 303 → /pair is acceptable.
    assert!(
        resp.status() == 404 || resp.status() == 303,
        "expected /login to be removed or redirect, got {}",
        resp.status()
    );
}
```

(Add `test_state()` helper that constructs `AuthState` like Phase 2/3 tests do.)

- [ ] **Step 2: Run, observe pass (today)**

Run: `cargo test -p alephcore --lib auth_middleware`
Expected: PASS currently (route still serves with 200) — but we want to **delete the route**, so the test will then assert 404.

- [ ] **Step 3: Delete the routes + handlers + helper + tests**

In `src/gateway/auth_middleware.rs`:

- Remove lines 28-32 (`LoginForm` struct)
- Remove lines 36 (`.route("/login", get(show_login))`)
- Remove lines 37 (`.route("/auth/login", post(handle_login))`)
- Remove `show_login`, `handle_login` async fns (42-77)
- Remove `login_page_html` fn (160-205)
- Remove the `test_login_html_*` and `test_local_storage_script_present` tests (207-260)
- Keep `handle_logout` if any pairing/bootstrap flow still POSTs to `/auth/logout` (verify with grep)

In `src/gateway/auth_probe_tests.rs:13`, delete `use crate::gateway::auth_middleware::login_page_html;` and remove any tests that use it.

- [ ] **Step 4: Run test**

Run: `cargo test -p alephcore --lib auth_middleware login_route`
Expected: PASS — `/login` returns 404.

Also run: `cargo test -p alephcore --lib gateway::auth_probe_tests`
Expected: PASS — login probe test removed.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/auth_middleware.rs src/gateway/auth_probe_tests.rs
git commit -m "gateway: delete /login form (Phase 4 — pairing UX replaces it)"
```

---

## Task 3: Remove Panel `?token=` URL fallback

**Files:**
- Modify: `interfaces/webchat/src/context.rs:284-313` (the `?token=` URL-param block in `authenticate`)

- [ ] **Step 1: Delete the block**

Remove these lines from `interfaces/webchat/src/context.rs:284-313`:

```rust
// Extract ?token= from URL and store as shared token (auto-login via URL)
#[cfg(target_arch = "wasm32")]
{
    if let Some(window) = web_sys::window() {
        if let Ok(search) = window.location().search() {
            if let Some(token) = …
```

Replace with a brief comment:

```rust
        // Auth is delivered via session cookie (set by /auth/bootstrap or
        // /auth/bootstrap/from_pairing). The legacy ?token= URL fallback
        // was removed in Phase 4 of the auth UX overhaul — the cookie is
        // now the only inbound auth surface for the Panel.
```

- [ ] **Step 2: Verify Panel still authenticates**

Build: `just wasm && cargo build --release -p alephcore --bin aleph-server`
Run: replace daemon binary, `just shell-dev` (or restart .app), Panel should still load signed in via the bootstrap-nonce cookie path.

- [ ] **Step 3: Update the corresponding unit/integration tests**

Grep for `aleph_shared_token` / `?token=` in `interfaces/webchat/src/` — any test that simulated URL-param login must be removed or rewritten to use the cookie path.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/context.rs
git commit -m "panel: drop ?token= URL fallback (cookie is the only inbound auth)"
```

---

## Task 4: Remove shell `legacy_token` fallback parameter

**Files:**
- Modify: `desktop/shell/src/daemon.rs:build_panel_url` (Phase 2 Task 7 helper)
- Modify: `desktop/shell/src/main.rs:reveal_panel` (Phase 2 Task 7 caller)

- [ ] **Step 1: Simplify `build_panel_url`**

Reduce to one parameter:

```rust
pub(crate) fn build_panel_url(bootstrap_url: Option<&str>) -> Result<Url, url::ParseError> {
    if let Some(u) = bootstrap_url {
        return Url::parse(u);
    }
    super::PANEL_URL.parse()
}
```

Delete the `legacy_token` parameter and the `?token=` query branch.

- [ ] **Step 2: Update `reveal_panel`**

```rust
fn reveal_panel(handle: &tauri::AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let token = daemon::load_bootstrap_token();
        let bootstrap_url = match token.as_deref() {
            Some(t) => daemon::issue_nonce_url(t).await.ok(),
            None => None,
        };
        // If nonce-issue fails we navigate to plain PANEL_URL; user will see
        // the pairing modal and complete bootstrap manually.
        daemon::navigate_to_panel(&handle, bootstrap_url.as_deref());
        focus_window(&handle);
    });
}
```

- [ ] **Step 3: Update tests**

Remove the Phase 1 `?token=` URL builder tests; keep the bootstrap-url and no-arg tests.

- [ ] **Step 4: Compile + smoke**

Run: `cargo check -p aleph-desktop-shell && cargo test -p aleph-desktop-shell`
Manually: `just shell-dev` → confirm Panel still loads signed in.

- [ ] **Step 5: Commit**

```bash
git add desktop/shell/src/daemon.rs desktop/shell/src/main.rs
git commit -m "shell: drop legacy_token fallback (Phase 4)"
```

---

## Task 5: Documentation updates

**Files:**
- Modify: `docs/reference/SECURITY.md` (add an "Auth UX" section)
- Modify: `docs/reference/SERVER_DEVELOPMENT.md` (first-start walkthrough)
- Modify: `CLAUDE.md` (note in 进程管理 section)
- Modify: project `README.md` if applicable

- [ ] **Step 1: Add "Auth UX" section to SECURITY.md**

In `docs/reference/SECURITY.md`, near the existing token-handling section, add:

```markdown
## Auth UX (post-2026-05 overhaul)

Aleph never asks the user to find, copy, or paste an access token. The
gateway token is auto-provisioned at first daemon start and stays
invisible. Three trust-transfer mechanisms move authentication between
surfaces:

1. **Same-process (Tauri desktop app)** — the shell reads the token from
   `~/.aleph/data/security.db` (same-UID gate) via the
   `aleph-server bootstrap-token` subcommand and issues a one-shot
   bootstrap nonce; the Panel webview navigates to
   `/auth/bootstrap?nonce=…` which sets the `aleph_session` HttpOnly
   cookie.

2. **Same-machine browser** — `aleph open` (CLI) and the desktop app's
   "Open in Browser" menu item issue a nonce and launch the system
   browser at the same URL. The endpoint refuses any non-loopback peer
   (`is_loopback_peer` in `src/gateway/auth_middleware.rs`).

3. **Cold browser / remote / mobile** — `/pair` shows a 6-digit code; the
   desktop app's NotificationCenter renders a row with `Approve` /
   `Reject` buttons. A QR-code variant in the Devices view covers mobile.

The threat model is unchanged from before the overhaul: same-UID =
trusted. We just stopped showing the token to the user, because seeing
it was the friction, not the security.

For debugging: `aleph auth debug show-token` still prints the token.
```

- [ ] **Step 2: Update SERVER_DEVELOPMENT.md**

Find any "first-start" or "access token" walkthrough and replace with:

```markdown
## First-start auth (no token typing)

1. Install the desktop app (.dmg / .msi / .deb) and launch it.
2. The Panel opens already signed in — Aleph auto-provisioned a token
   and the shell handed it off via a one-shot bootstrap nonce.
3. To use Aleph from a browser on the same machine, click
   "Open in Browser" in the desktop app menu, or run `aleph open` in a
   terminal.
4. To pair a second machine or a phone: in the desktop app go to
   Devices → "Add browser/mobile" and scan the QR code.

For headless or CI installs without a desktop app: `aleph-server bootstrap-token`
prints the token on stdout (same threat model as `aleph secret list`).
```

- [ ] **Step 3: CLAUDE.md note**

Add to the existing 进程管理 section (or create a new "Auth UX" subsection):

```markdown
### Auth UX

Auth tokens are auto-provisioned at first daemon start; users never see them.
- Desktop app: silent bootstrap via Tauri shell handoff (Phase 1+2).
- Same-machine browser: `aleph open` / desktop menu (Phase 2).
- Remote / mobile: `/pair` flow + desktop notification approve (Phase 3).
- Debug only: `aleph auth debug show-token`.

See [docs/reference/SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux).
```

- [ ] **Step 4: Verify Markdown renders**

```bash
# If you use mdbook or similar:
mdbook serve docs/  # browse to see the rendered docs

# Or just verify no obvious typos:
markdownlint docs/reference/SECURITY.md docs/reference/SERVER_DEVELOPMENT.md
```

- [ ] **Step 5: Commit**

```bash
git add docs/reference/SECURITY.md docs/reference/SERVER_DEVELOPMENT.md CLAUDE.md
git commit -m "docs: auth UX section — trust transfer + pairing (Phases 1-3)"
```

---

## Self-Review Checklist

1. **All four phases coherent?** Phase 1 introduces silent bootstrap; Phase 2 hardens it with nonce + browser-from-shell; Phase 3 handles cold/remote browsers via pairing; Phase 4 removes the legacy paths Phase 1-3 made redundant. ✓
2. **Backward compat for one cycle?** `aleph auth show-token` still parses but warns; CLI scripts of existing users break only with a warning, not silently. ✓
3. **No new attack surface?** All deletions only. ✓
4. **Docs match reality?** SECURITY.md, SERVER_DEVELOPMENT.md, CLAUDE.md all updated. ✓
5. **Test coverage?** Each Task includes a test verifying the deletion (route 404s, CLI parses, etc.). ✓

---

## Verification Commands (Definition of Done)

```bash
# All previous-phase tests still pass
cargo test -p alephcore --lib gateway::bootstrap
cargo test --test bootstrap_loopback_gate
cargo test --test pair_browser_e2e

# Phase 4 tests
cargo test -p alephcore --lib auth_middleware login_route_returns_404
cargo test -p aleph-cli auth_debug

# Grep checks
grep -rn "login_page_html" src/                       # → no matches
grep -rn "?token=" interfaces/webchat/src/             # → no matches (only the bootstrap nonce path)
grep -rn "Access token (new)" src/                    # → no matches (Phase 1 banner already gone)
grep -rn "AuthAction::ShowToken" interfaces/cli/      # → only in cli_args.rs (hidden variant) + deprecation warning

# Manual flow:
rm -rf ~/.aleph              # back up first!
just shell-dev               # → Panel loads signed in, zero token UI
aleph open                   # → system browser opens, signed in
# Open Firefox manually → 127.0.0.1:18790/ → redirects to /pair → 6-digit code → approve in app → signed in
# Open phone camera → scan QR in Devices view → same flow
```

---

## Final State

After Phase 4 completes:

- **User-facing**: the word "token" never appears in the Panel, the desktop app menus, or the default CLI help. "Pairing" is the only auth concept users see, and only when they actively add a device.
- **Developer-facing**: `aleph auth debug show-token` retrieves the token for break-glass scenarios. `~/.aleph/data/security.db` remains the source of truth. The bootstrap nonce + pairing infrastructure built in Phases 2-3 is the only inbound auth surface for the Panel.
- **Codebase**: ~150 LOC deleted (login form + URL-param fallback + legacy CLI plumbing). No new architecture beyond what Phases 1-3 introduced.

## Risk Notes

- **One release cycle before deleting**: do not run Phase 4 until at least one release of Phases 1-3 has been in users' hands. The deprecation warnings in `aleph auth show-token` need to give CLI script authors a chance to migrate.
- **`handle_logout` retained**: explicitly kept because the Panel's sign-out button may POST to `/auth/logout`. Grep for `/auth/logout` callers before deleting in any future cleanup.
- **Documentation drift**: the docs updates in Task 5 are the canonical source. If they conflict with what's actually implemented, the docs are wrong — fix them, don't add code to match.
- **Hidden variants still parse**: `#[command(hide = true)]` removes from `--help` output but variants still parse, so old scripts work. Tests verify both paths.
