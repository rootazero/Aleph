# Runtime Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver install-time runtime bootstrap (fnm / Node / uv / playwright-cli / Chromium / global venv at `~/.aleph/.venv`) plus a Panel Runtime page that drives existing `runtimes.*` RPCs, so a one-line curl install yields a fully-ready Aleph with zero manual runtime setup.

**Architecture:** A thin `aleph-server bootstrap-runtime` CLI subcommand wraps the existing `runtimes::ensure_capability` engine. `install.sh` / `install.ps1` invoke it at the end of install (unless `--skip-runtime` / `$ALEPH_SKIP_RUNTIME=1` is set). A non-blocking startup probe populates `~/.aleph/runtimes/ledger.json`. A new Panel `Settings → Runtime` page lets users re-install / refresh. The `uv` spec gains a post-install action that idempotently creates `~/.aleph/.venv`.

**Tech Stack:** Rust (tokio, clap, serde, thiserror), Leptos (WASM panel), Bash, PowerShell. Existing `src/runtimes/` engine is reused wholesale — this plan mostly wires it to new consumers.

**Reference spec:** `docs/superpowers/specs/2026-04-14-runtime-bootstrap-design.md`

---

## File Structure

### Files created (4)

| Path | Responsibility |
|---|---|
| `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs` | CLI subcommand: parse opts, call `ensure_capability` per target, pretty-print or NDJSON progress, exit codes |
| `interfaces/webchat/src/api/runtime.rs` | Panel-side thin RPC wrapper: `list`, `install`, `refresh`, plus EventBus subscription helper |
| `interfaces/webchat/src/views/settings/runtime.rs` | `RuntimeView` Leptos component: step-indicator list, install log, Install/Refresh buttons |
| `interfaces/webchat/src/views/settings/browser_runtime_banner.rs` | Compact runtime summary embedded in the Browser page |

### Files modified (12)

| Path | Nature of change |
|---|---|
| `src/runtimes/post_install.rs` | Expand `$HOME`/`%USERPROFILE%` in `AssetProbe` `repair` args; Windows path rewrite for `/bin/python` → `\Scripts\python.exe` |
| `src/runtimes/specs.rs` | `uv` spec gains `AssetProbe` post-install that creates `~/.aleph/.venv` |
| `src/runtimes/ensure.rs` | Unify `Failed` / `PathNotFound` / `Unsupported` error branches into an actionable multi-line message |
| `src/gateway/event_bus.rs` | Add optional `stderr: Option<String>` field to `RuntimeInstallProgressEvent` |
| `src/gateway/handlers/runtimes.rs` | Populate `stderr` when `ensure_capability` fails |
| `src/bin/aleph-server/cli.rs` | Declare `BootstrapRuntime` subcommand with all flags |
| `src/bin/aleph-server/commands/mod.rs` | Register the new module |
| `src/bin/aleph-server/main.rs` | Dispatch the new `Command::BootstrapRuntime` variant |
| `src/bin/aleph-server/commands/start/mod.rs` | Spawn non-blocking startup probe after AppContext init |
| `install.sh` | Invoke `bootstrap-runtime --best-effort` + `--skip-runtime` + `$ALEPH_SKIP_RUNTIME` support |
| `install.ps1` | Equivalent logic with `-SkipRuntime` switch + `$env:ALEPH_SKIP_RUNTIME` |
| `interfaces/webchat/src/views/settings/mod.rs` | Register `/settings/runtime` route + menu entry |
| `interfaces/webchat/src/views/settings/browser.rs` | Embed `<RuntimeSummaryBanner />` at the top |

### Files cleaned up (6 comment sites + 1 deletion)

- `src/config/types/general.rs:44,116` — replace `Playwright MCP` / `[browser.playwright_mcp]` with CLI equivalents
- `src/browser/profile.rs:25` — replace `chromiumoxide` reference in doc comment
- `src/builtin_tools/browser_tools/tabs.rs:111,140` + `mod.rs:43` — comment text updates
- `review-results/browser.md` — delete (references deleted `playwright_mcp_backend.rs`)

---

## Task 1: Fix `$HOME` expansion in `AssetProbe::repair` args

**Context:** Today, `post_install::verify_or_repair` passes `repair` args to the child process verbatim. A template like `"$HOME/.aleph/.venv"` ends up as a literal string in the child's argv. This must be fixed before Task 2 wires `uv venv $HOME/.aleph/.venv` through the same codepath.

**Files:**
- Modify: `src/runtimes/post_install.rs` — function `verify_or_repair` (current body at lines 89-103); helper `expand_home` at lines 23-29

**Background reading before starting:**
- `src/runtimes/post_install.rs` (full file, 122 lines)
- `src/runtimes/specs.rs` — search for `AssetProbe` to see its sole current user (`playwright-cli` post-install; we do not want to regress it)

- [ ] **Step 1.1: Write the failing test for repair-arg expansion**

Append to `src/runtimes/post_install.rs` inside the existing `#[cfg(test)] mod tests` block (after the existing `test_expand_home_no_placeholder` test):

```rust
#[test]
fn test_expand_home_multiple_placeholders() {
    std::env::set_var("HOME", "/tmp/fake-home");
    let out = expand_home("$HOME/a/$HOME/b");
    assert_eq!(out, "/tmp/fake-home/a/$HOME/b");
    // Only the first occurrence is replaced — caller should pass templates
    // with a single $HOME placeholder per arg. Document this contract.
}

#[tokio::test]
async fn test_verify_or_repair_expands_home_in_repair_args() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());

    // Use /bin/true as bin_path — it accepts any args and always succeeds.
    // Verify that repair args get $HOME expanded BEFORE being handed to the
    // child process by using a tiny helper shim we can observe afterwards.
    //
    // Strategy: set repair to ["-c", "test -d $HOME/marker/created-under-tmp"].
    // If expansion works, /bin/sh -c finds the marker dir we create below.
    let marker = dir.path().join("marker/created-under-tmp");
    tokio::fs::create_dir_all(&marker).await.unwrap();

    // Point AssetProbe at a non-existent path to force the repair branch.
    let action = PostInstallAction::AssetProbe {
        path: "$HOME/definitely/does-not-exist",
        repair: &["-c", "test -d $HOME/marker/created-under-tmp"],
    };

    // Use /bin/sh as the binary so we can run shell commands portably in CI.
    let sh = PathBuf::from("/bin/sh");
    let result = run(&action, &sh).await;
    assert!(result.is_ok(), "repair should succeed after $HOME expansion: {result:?}");
}

#[tokio::test]
async fn test_verify_or_repair_skips_when_path_exists() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());

    // Pre-create the probed path — the repair must NOT fire.
    let probe_path = dir.path().join(".aleph/probe_already_there");
    tokio::fs::create_dir_all(probe_path.parent().unwrap()).await.unwrap();
    tokio::fs::write(&probe_path, b"x").await.unwrap();

    let action = PostInstallAction::AssetProbe {
        path: "$HOME/.aleph/probe_already_there",
        // If this ran, it would fail (false always exits non-zero).
        repair: &["false"],
    };
    let sh = PathBuf::from("/bin/sh");
    let result = run(&action, &sh).await;
    assert!(result.is_ok());
}
```

- [ ] **Step 1.2: Run the tests and observe the failure**

Run:
```bash
cargo test -p alephcore --lib runtimes::post_install::tests::test_verify_or_repair_expands_home_in_repair_args
```

Expected: **FAIL** — repair args are not expanded today; `sh -c "test -d $HOME/marker/..."` would actually expand `$HOME` because the shell parses it, making this specific test subtly weak. Strengthen the test by using `/usr/bin/env` or a bash-less invocation:

Replace the failing test body with an unambiguous version that uses a wrapper script. Instead of relying on the child shell to re-expand, observe an expanded marker in the filesystem:

```rust
#[tokio::test]
async fn test_verify_or_repair_expands_home_in_repair_args() {
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());

    // Write a tiny shell script that creates a file named after its first arg.
    let script_path = dir.path().join("touchit.sh");
    tokio::fs::write(
        &script_path,
        "#!/bin/sh\nmkdir -p \"$(dirname \"$1\")\" && : > \"$1\"\n",
    )
    .await
    .unwrap();
    let mut perms = tokio::fs::metadata(&script_path).await.unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    tokio::fs::set_permissions(&script_path, perms).await.unwrap();

    // Probe a non-existent path so the repair fires.
    let action = PostInstallAction::AssetProbe {
        path: "$HOME/never/exists",
        repair: &["$HOME/touchit.sh", "$HOME/expected_output_file"],
    };

    // Invoke via /bin/sh so the child inherits no shell expansion — the
    // expand_home pass must have rewritten arg 0 and arg 1 BEFORE spawn.
    // Pretend /bin/sh is our "bin" by running it with -c that does nothing,
    // but we actually need to call touchit.sh directly. So use the script
    // path itself as bin_path.
    let bin = dir.path().join("touchit.sh");

    // The run() helper calls Command::new(bin).args(repair). If expansion
    // works, touchit.sh will be invoked as:
    //     /tmp/.../touchit.sh /tmp/.../expected_output_file
    // and create that file.
    let result = run(&action, &bin).await;
    assert!(result.is_ok(), "repair must succeed: {result:?}");

    let expected_out = dir.path().join("expected_output_file");
    assert!(
        tokio::fs::try_exists(&expected_out).await.unwrap(),
        "expansion should have produced {}", expected_out.display()
    );
}
```

Re-run:
```bash
cargo test -p alephcore --lib runtimes::post_install::tests::test_verify_or_repair_expands_home_in_repair_args
```

Expected: **FAIL** — repair arg `"$HOME/touchit.sh"` is not a real path; `Command::new()` cannot find a binary literally called `"$HOME/touchit.sh"`. (The `bin_path` is pre-expanded via `TempDir::path()`, so it *is* real.) The child process call fails, `verify_or_repair` returns `Err(RepairFailed)`, and the assertion trips.

- [ ] **Step 1.3: Implement the fix**

Edit `src/runtimes/post_install.rs`. Replace the `verify_or_repair` function (lines ~89-103) with:

```rust
async fn verify_or_repair(
    bin_path: &PathBuf,
    path_template: &str,
    repair: &[&str],
) -> Result<(), PostInstallError> {
    let expanded = PathBuf::from(expand_home(path_template));
    if expanded.exists() {
        return Ok(());
    }
    let expanded_repair: Vec<String> = repair.iter().map(|a| expand_home(a)).collect();
    let output = Command::new(bin_path).args(&expanded_repair).output().await?;
    if !output.status.success() {
        return Err(PostInstallError::RepairFailed);
    }
    Ok(())
}
```

- [ ] **Step 1.4: Extend `expand_home` to also understand `%USERPROFILE%`**

Replace the `expand_home` helper (lines ~23-29) with:

```rust
/// Expand `$HOME` or `%USERPROFILE%` in a template path. On Windows also
/// rewrites Unix `/bin/python` → `\Scripts\python.exe` and converts forward
/// slashes to backslashes, so a single template string like
/// `"$HOME/.aleph/.venv/bin/python"` works cross-platform.
fn expand_home(template: &str) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let s = template.replacen("$HOME", &home, 1).replacen("%USERPROFILE%", &home, 1);

    #[cfg(target_os = "windows")]
    let s = s
        .replace("/bin/python", r"\Scripts\python.exe")
        .replace("/bin/", r"\Scripts\")
        .replace('/', r"\");

    s
}
```

Note: `replacen(…, 1)` keeps the "only first occurrence" contract documented in `test_expand_home_multiple_placeholders`.

- [ ] **Step 1.5: Run the tests and verify all pass**

Run:
```bash
cargo test -p alephcore --lib runtimes::post_install
```

Expected: **PASS** — all three new tests plus the two existing `test_expand_home_*` tests pass.

- [ ] **Step 1.6: Commit**

```bash
git add src/runtimes/post_install.rs
git commit -m "runtimes: expand \$HOME / %USERPROFILE% in AssetProbe repair args"
```

---

## Task 2: Auto-create `~/.aleph/.venv` via `uv` post-install

**Context:** Today `code_exec.md` prompt tells the LLM to run `uv venv ~/.aleph/.venv` on first use. Lift this into the bootstrap phase so the venv exists before any tool asks for it.

**Files:**
- Modify: `src/runtimes/specs.rs` — the `uv` `RuntimeSpec` literal (lines ~92-117)

- [ ] **Step 2.1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block at the bottom of `src/runtimes/specs.rs`:

```rust
#[test]
fn test_uv_spec_has_venv_post_install() {
    let spec = find_spec("uv").expect("uv spec must exist");
    assert_eq!(spec.post_install.len(), 1, "uv should have exactly one post-install action");
    match spec.post_install[0] {
        PostInstallAction::AssetProbe { path, repair } => {
            assert!(
                path.contains(".aleph/.venv"),
                "uv post-install should probe for ~/.aleph/.venv, got: {path}"
            );
            assert!(path.ends_with("python") || path.ends_with("python.exe"),
                "probe path should end at the python binary, got: {path}");
            assert_eq!(
                repair,
                &["venv", "$HOME/.aleph/.venv"],
                "repair must be `uv venv $HOME/.aleph/.venv`",
            );
        }
        _ => panic!("expected AssetProbe post-install for uv"),
    }
}
```

- [ ] **Step 2.2: Run the test to verify it fails**

Run:
```bash
cargo test -p alephcore --lib runtimes::specs::tests::test_uv_spec_has_venv_post_install
```

Expected: **FAIL** — current `uv` spec has `post_install: &[]`.

- [ ] **Step 2.3: Implement the spec change**

In `src/runtimes/specs.rs`, locate the `uv` `RuntimeSpec` block (search for `name: "uv"`). Replace its `post_install` field:

```rust
        post_install: &[PostInstallAction::AssetProbe {
            path: "$HOME/.aleph/.venv/bin/python",
            repair: &["venv", "$HOME/.aleph/.venv"],
        }],
```

(Task 1's `expand_home` handles the Windows `\Scripts\python.exe` rewrite transparently.)

- [ ] **Step 2.4: Run the specs tests — all existing plus the new one must pass**

Run:
```bash
cargo test -p alephcore --lib runtimes::specs
```

Expected: **PASS** including `test_all_specs_have_nonempty_name`, `test_find_spec_known`, `test_deps_reference_known_specs`, `test_via_parent_in_deps`, and the new `test_uv_spec_has_venv_post_install`.

- [ ] **Step 2.5: Add a cross-module integration test covering the full uv install path**

Append to `src/runtimes/specs.rs` tests block:

```rust
#[tokio::test]
async fn test_uv_post_install_creates_venv_idempotently() {
    use crate::runtimes::post_install::run;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());

    // Fake uv: a shell script that responds to `venv <path>` by mkdir-ing the
    // expected layout, mimicking `uv venv` semantics.
    let fake_uv = dir.path().join("fake-uv.sh");
    tokio::fs::write(
        &fake_uv,
        concat!(
            "#!/bin/sh\n",
            "if [ \"$1\" = \"venv\" ]; then\n",
            "  mkdir -p \"$2/bin\"\n",
            "  : > \"$2/bin/python\"\n",
            "  chmod +x \"$2/bin/python\"\n",
            "  exit 0\n",
            "fi\n",
            "exit 1\n",
        ),
    )
    .await
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = tokio::fs::metadata(&fake_uv).await.unwrap().permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(&fake_uv, perms).await.unwrap();

    let spec = find_spec("uv").unwrap();
    let action = &spec.post_install[0];

    // Round 1: venv doesn't exist → repair fires.
    run(action, &fake_uv).await.unwrap();
    let venv_python = dir.path().join(".aleph/.venv/bin/python");
    assert!(venv_python.exists(), "venv python should exist after first run");

    // Round 2: venv exists → repair should be skipped (we detect by
    // removing the fake uv binary — if run re-invokes it, it will fail).
    tokio::fs::remove_file(&fake_uv).await.unwrap();
    run(action, &fake_uv).await.unwrap();
    assert!(venv_python.exists(), "venv python should still exist after idempotent second run");
}
```

Note: skip this test on Windows with `#[cfg(not(target_os = "windows"))]` above `#[tokio::test]`, since the shell-script shim is POSIX-only.

- [ ] **Step 2.6: Verify the new integration test passes**

Run:
```bash
cargo test -p alephcore --lib runtimes::specs::tests::test_uv_post_install_creates_venv_idempotently
```

Expected: **PASS**.

- [ ] **Step 2.7: Commit**

```bash
git add src/runtimes/specs.rs
git commit -m "runtimes: auto-create global venv at ~/.aleph/.venv via uv post_install"
```

---

## Task 3: Add optional `stderr` field to `RuntimeInstallProgressEvent`

**Context:** Today the failure event carries `error: String` (the high-level error) but no structured stderr. Panel users see "install failed" without diagnostic output. Add an additive optional `stderr` field; populate it when `ensure_capability` fails with stderr context.

**Files:**
- Modify: `src/gateway/event_bus.rs` — `RuntimeInstallProgressEvent` struct
- Modify: `src/gateway/handlers/runtimes.rs` — `handle_install` failure branch populates `stderr`

**Background reading:**
- `src/gateway/event_bus.rs` around the existing `RuntimeInstallProgressEvent` definition
- `src/gateway/handlers/runtimes.rs` full file (206 lines)

- [ ] **Step 3.1: Find the existing event struct**

Run:
```bash
grep -n "RuntimeInstallProgressEvent" src/gateway/event_bus.rs
```

Expected output: the struct definition and its usage sites. Read the surrounding context to understand serialization (serde derives).

- [ ] **Step 3.2: Write the failing test**

In `src/gateway/handlers/runtimes.rs`, extend the `#[cfg(test)] mod tests` block:

```rust
#[tokio::test]
async fn test_install_failed_event_carries_stderr() {
    use crate::gateway::event_bus::{GatewayEvent, GatewayEventBus};

    let dir = tempfile::TempDir::new().unwrap();
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(
        dir.path().join("ledger.json"),
    )));
    let bus = Arc::new(GatewayEventBus::new(32));
    let mut rx = bus.subscribe_json();

    // Request install of a capability guaranteed to fail (unknown name).
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        method: "runtimes.install".into(),
        params: Some(serde_json::json!({ "capability": "nonexistent-xyz" })),
        id: Some(serde_json::json!(1)),
    };
    let _ = handle_install(req, ledger, bus.clone()).await;

    // We expect at least one failed event. Drain up to a few events, look for
    // status == "failed" with a non-null stderr field on recognised failures.
    // Unknown capabilities currently return early — this test also acts as
    // a regression guard if that path changes.
    let mut saw_failed = false;
    for _ in 0..5 {
        if let Ok(evt_json) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            rx.recv(),
        )
        .await
        {
            let evt_json = evt_json.unwrap();
            if evt_json["event"] == "RuntimeInstallProgress"
                && evt_json["data"]["status"] == "failed"
            {
                saw_failed = true;
                // stderr may be null for unknown-capability (no child process ran).
                // The assertion is structural: the field must exist and be either
                // null or a string.
                assert!(
                    evt_json["data"].get("stderr").is_some(),
                    "payload must include stderr field (null allowed)",
                );
                break;
            }
        }
    }
    // For unknown capabilities, handle_install validates before spawning the
    // background task and returns an error synchronously. Adjust expectations:
    // the behaviour is that NO failed event is emitted, only the synchronous
    // error response. Document this by asserting response shape instead.
    let _ = saw_failed; // suppress unused warning if early-return path is taken
}
```

Because `handle_install` currently rejects unknown capabilities synchronously (before the spawn), shift the test to a path that DOES spawn and DOES fail. Use a known-but-uninstallable capability: `"cargo"` has `install: &[]`, so `ensure_capability("cargo")` reaches the dispatcher, which returns `Unsupported`.

Replace the test body:

```rust
#[tokio::test]
async fn test_install_failed_event_carries_stderr_field() {
    use crate::gateway::event_bus::GatewayEventBus;

    let dir = tempfile::TempDir::new().unwrap();
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(
        dir.path().join("ledger.json"),
    )));
    let bus = Arc::new(GatewayEventBus::new(32));
    let mut rx = bus.subscribe_json();

    // cargo has no install strategy → Unsupported → ensure_capability Err.
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        method: "runtimes.install".into(),
        params: Some(serde_json::json!({ "capability": "cargo" })),
        id: Some(serde_json::json!(1)),
    };
    let _ = handle_install(req, ledger, bus.clone()).await;

    // Wait for the background task's failed event.
    let mut saw_failed_with_stderr_field = false;
    for _ in 0..20 {
        if let Ok(Ok(evt_json)) =
            tokio::time::timeout(std::time::Duration::from_millis(250), rx.recv()).await
        {
            if evt_json["data"]["status"] == "failed" {
                assert!(
                    evt_json["data"].get("stderr").is_some(),
                    "failed event must have a `stderr` key (null allowed), got: {evt_json}",
                );
                saw_failed_with_stderr_field = true;
                break;
            }
        }
    }
    assert!(saw_failed_with_stderr_field, "expected at least one failed event");
}
```

- [ ] **Step 3.3: Run the test and verify it fails**

Run:
```bash
cargo test -p alephcore --lib gateway::handlers::runtimes::tests::test_install_failed_event_carries_stderr_field
```

Expected: **FAIL** — `evt_json["data"].get("stderr")` returns `None` because the struct has no such field.

- [ ] **Step 3.4: Add the `stderr` field to the struct**

In `src/gateway/event_bus.rs`, locate `RuntimeInstallProgressEvent` and add the field. The existing struct should look something like:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInstallProgressEvent {
    pub step: String,
    pub status: String,
    pub log_line: Option<String>,
    pub error: Option<String>,
    pub timestamp: i64,
}
```

Add `stderr` right before `timestamp`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInstallProgressEvent {
    pub step: String,
    pub status: String,
    pub log_line: Option<String>,
    pub error: Option<String>,
    /// Raw stderr captured from the failing install command. Populated only
    /// when `status == "failed"` and a child process produced stderr output.
    #[serde(default)]
    pub stderr: Option<String>,
    pub timestamp: i64,
}
```

`#[serde(default)]` keeps wire-compat — older producers that don't set the field still deserialize cleanly.

- [ ] **Step 3.5: Populate `stderr` in the install handler**

In `src/gateway/handlers/runtimes.rs`, update `handle_install` (around lines 141-170). Current failure branch:

```rust
Err(e) => RuntimeInstallProgressEvent {
    step: cap_for_event,
    status: "failed".into(),
    log_line: None,
    error: Some(e.to_string()),
    timestamp: chrono::Utc::now().timestamp_millis(),
},
```

Change to:

```rust
Err(e) => {
    // Extract stderr from the runtime error string. `AlephError::runtime`'s
    // Display includes the stderr tail after "Stderr tail: " if Task 4 has
    // landed; before Task 4 we fall back to passing the whole error.
    let err_str = e.to_string();
    let stderr = err_str
        .split_once("Stderr tail: ")
        .map(|(_, tail)| tail.to_string())
        .or_else(|| Some(err_str.clone()));
    RuntimeInstallProgressEvent {
        step: cap_for_event,
        status: "failed".into(),
        log_line: None,
        error: Some(err_str),
        stderr,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}
```

Also update the `"started"` and `"done"` variants to include `stderr: None`:

```rust
// started event
RuntimeInstallProgressEvent {
    step: cap_for_event.clone(),
    status: "started".into(),
    log_line: None,
    error: None,
    stderr: None,
    timestamp: chrono::Utc::now().timestamp_millis(),
}
// done event
RuntimeInstallProgressEvent {
    step: cap_for_event,
    status: "done".into(),
    log_line: None,
    error: None,
    stderr: None,
    timestamp: chrono::Utc::now().timestamp_millis(),
}
```

- [ ] **Step 3.6: Run the failing test — it should now pass**

Run:
```bash
cargo test -p alephcore --lib gateway::handlers::runtimes
```

Expected: **PASS** — including the new `test_install_failed_event_carries_stderr_field` and the pre-existing `test_handle_list_returns_all_specs`.

- [ ] **Step 3.7: Verify nothing else broke (event struct is used by other code)**

Run:
```bash
cargo check -p alephcore
```

Expected: no errors. If any call site constructs `RuntimeInstallProgressEvent` directly without the new field, the compiler will point it out — update those sites with `stderr: None`.

- [ ] **Step 3.8: Commit**

```bash
git add src/gateway/event_bus.rs src/gateway/handlers/runtimes.rs
git commit -m "runtimes: attach final stderr to RuntimeInstallProgressEvent on failure"
```

---

## Task 4: Rewrite `ensure_capability` failure messages with actionable hints

**Context:** Today `ensure_capability`'s three error branches (`Failed` / `PathNotFound` / `Unsupported`) each produce a single-line generic message. Users see "Failed to bootstrap X. Please install manually." with no recovery path. Unify into a multi-line actionable message.

**Files:**
- Modify: `src/runtimes/ensure.rs` — the `match bootstrap_result { … }` block at lines 123-181

- [ ] **Step 4.1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block at bottom of `src/runtimes/ensure.rs`:

```rust
#[tokio::test]
async fn test_failure_message_includes_actionable_hints() {
    let dir = TempDir::new().unwrap();
    let ledger_path = dir.path().join("ledger.json");
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));

    // cargo has no install strategy → Unsupported branch fires.
    let err = ensure_capability("cargo", &ledger).await.unwrap_err();
    let msg = err.to_string();

    assert!(
        msg.contains("aleph-server bootstrap-runtime"),
        "error should name the CLI remediation, got: {msg}"
    );
    assert!(
        msg.contains("Panel"),
        "error should mention the Panel remediation, got: {msg}"
    );
    assert!(
        msg.contains("--only cargo") || msg.contains("cargo"),
        "error should reference the failing capability, got: {msg}"
    );
}
```

- [ ] **Step 4.2: Run the test — observe failure**

Run:
```bash
cargo test -p alephcore --lib runtimes::ensure::tests::test_failure_message_includes_actionable_hints
```

Expected: **FAIL** — current messages don't mention `bootstrap-runtime` or "Panel".

- [ ] **Step 4.3: Implement a unified error builder**

In `src/runtimes/ensure.rs`, just above the `#[cfg(test)]` block at the bottom, add a private helper:

```rust
/// Build a multi-line actionable error message for a failed
/// `ensure_capability` call. Includes the three canonical fix options
/// (CLI, Panel, manual) plus the upstream stderr tail when available.
fn runtime_error(capability: &str, reason: &str, stderr: Option<&str>) -> AlephError {
    use crate::runtimes::find_spec;

    let hint = find_spec(capability)
        .and_then(|s| s.llm_hint)
        .unwrap_or("(no hint available — check the runtime's documentation)");

    let stderr_block = stderr
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let tail = if s.len() > 400 { &s[s.len() - 400..] } else { s };
            format!("\nStderr tail: {}", tail.trim())
        })
        .unwrap_or_default();

    AlephError::runtime(
        capability,
        format!(
            "Runtime '{capability}' is not available: {reason}{stderr_block}\n\n\
             Fix options:\n  \
               1. Run: aleph-server bootstrap-runtime --only {capability}\n  \
               2. Open Panel → Settings → Runtime and click 'Install'.\n  \
               3. Install manually — {hint}",
        ),
    )
}
```

Now replace the four error branches inside `ensure_capability`'s match on `bootstrap_result`:

```rust
    match bootstrap_result {
        BootstrapResult::Success { bin_path, version } => {
            let mut guard = ledger.write().await;
            guard.update(CapabilityEntry {
                name: capability.to_string(),
                bin_path: bin_path.clone(),
                version,
                status: CapabilityStatus::Ready,
                source: CapabilitySource::AlephManaged,
                last_probed: now,
            });
            let _ = guard.persist();

            info!("Capability {} bootstrapped at {}", capability, bin_path.display());
            Ok(bin_path)
        }
        BootstrapResult::PathNotFound { expected } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                &format!("installed but binary not found at {expected}"),
                None,
            ))
        }
        BootstrapResult::Failed { stderr } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                "bootstrap command returned a non-zero exit code",
                Some(&stderr),
            ))
        }
        BootstrapResult::Unsupported { capability: cap, reason } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                &format!("{cap} is not supported on this platform: {reason}"),
                None,
            ))
        }
        BootstrapResult::UnknownCapability { capability: cap } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                &format!("{cap} has no bootstrap spec registered"),
                None,
            ))
        }
    }
```

Also update the fallback at line ~95-105 (when `bootstrap::has_spec` returns `false`):

```rust
    if !bootstrap::has_spec(capability) {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Missing);
        return Err(runtime_error(
            capability,
            "not found on PATH and no bootstrap spec available",
            None,
        ));
    }
```

- [ ] **Step 4.4: Run the tests and verify PASS**

Run:
```bash
cargo test -p alephcore --lib runtimes::ensure
```

Expected: **PASS** — including the new `test_failure_message_includes_actionable_hints` plus the two existing `test_ensure_already_ready` and `test_ensure_unknown_capability`.

- [ ] **Step 4.5: Verify Task 3's stderr-extraction still works**

Task 3 splits on `"Stderr tail: "`. Confirm this substring is produced exactly by the new builder:

```bash
cargo test -p alephcore --lib gateway::handlers::runtimes::tests::test_install_failed_event_carries_stderr_field
```

Expected: **PASS** — the stderr field gets populated end-to-end.

- [ ] **Step 4.6: Commit**

```bash
git add src/runtimes/ensure.rs
git commit -m "runtimes: rewrite ensure_capability failure message with actionable hints"
```

---

## Task 5: Add `bootstrap-runtime` CLI subcommand

**Context:** Build the CLI subcommand that wraps `runtimes::ensure_capability`. It's the entry point that `install.sh` / `install.ps1` call in Tasks 7-8.

**Files:**
- Create: `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs`
- Modify: `src/bin/aleph-server/cli.rs` — add `BootstrapRuntime` enum variant + struct
- Modify: `src/bin/aleph-server/commands/mod.rs` — register new module
- Modify: `src/bin/aleph-server/main.rs` — dispatch the variant

**Background reading:**
- `src/bin/aleph-server/cli.rs` (full file, already read — the `Command` enum is at lines 68-115)
- `src/bin/aleph-server/commands/mod.rs` (full file, already read)
- `src/bin/aleph-server/commands/audit.rs` — use as a reference for how to structure a subcommand that takes an `Action` enum

- [ ] **Step 5.1: Add the CLI declaration**

In `src/bin/aleph-server/cli.rs`, add a new variant to the `Command` enum (after `Plugin`):

```rust
    /// Bootstrap runtime dependencies (fnm, node, uv, playwright-cli, chromium, venv)
    BootstrapRuntime(BootstrapRuntimeArgs),
```

Then add the args struct near the other `Subcommand`-derived types (e.g., after `MarketplaceAction`):

```rust
/// `bootstrap-runtime` subcommand flags.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BootstrapRuntimeArgs {
    /// Install only the given capability (repeatable). Default set: uv, playwright-cli.
    #[arg(long)]
    pub only: Vec<String>,

    /// Skip the given capability (repeatable).
    #[arg(long)]
    pub skip: Vec<String>,

    /// Reinstall even if the ledger says Ready.
    #[arg(long)]
    pub force: bool,

    /// Exit 0 regardless of failures (install.sh / install.ps1 use this).
    #[arg(long)]
    pub best_effort: bool,

    /// Emit NDJSON progress events to stderr instead of pretty output.
    #[arg(long)]
    pub json: bool,

    /// Suppress per-step output; only errors.
    #[arg(long)]
    pub quiet: bool,
}
```

- [ ] **Step 5.2: Add a CLI-parsing test**

Append to the existing `#[cfg(test)] mod tests` block in `src/bin/aleph-server/cli.rs`:

```rust
#[test]
fn test_cli_parses_bootstrap_runtime_default() {
    let args = Args::try_parse_from(["aleph", "bootstrap-runtime"]).unwrap();
    match args.command {
        Some(Command::BootstrapRuntime(a)) => {
            assert!(a.only.is_empty());
            assert!(a.skip.is_empty());
            assert!(!a.force);
            assert!(!a.best_effort);
            assert!(!a.json);
            assert!(!a.quiet);
        }
        _ => panic!("Expected BootstrapRuntime variant"),
    }
}

#[test]
fn test_cli_parses_bootstrap_runtime_all_flags() {
    let args = Args::try_parse_from([
        "aleph", "bootstrap-runtime",
        "--only", "uv",
        "--only", "playwright-cli",
        "--skip", "cargo",
        "--force",
        "--best-effort",
        "--json",
        "--quiet",
    ])
    .unwrap();
    match args.command {
        Some(Command::BootstrapRuntime(a)) => {
            assert_eq!(a.only, vec!["uv", "playwright-cli"]);
            assert_eq!(a.skip, vec!["cargo"]);
            assert!(a.force);
            assert!(a.best_effort);
            assert!(a.json);
            assert!(a.quiet);
        }
        _ => panic!("Expected BootstrapRuntime variant"),
    }
}
```

- [ ] **Step 5.3: Run the CLI tests — observe failure and success**

Run:
```bash
cargo test -p aleph-server --lib test_cli_parses_bootstrap_runtime
```

Expected: **FAIL at compile time** — `Command::BootstrapRuntime` doesn't exist yet. (Wait — we just added it. Compiler error should be that `cargo test` for `aleph-server` is probably `cargo test --bin aleph-server`. Use:)

```bash
cargo test --bin aleph-server test_cli_parses_bootstrap_runtime
```

Expected once Step 5.1 is applied: **PASS** — struct exists and parses correctly. If you skipped Step 5.1 it fails at compile time — come back and finish 5.1 first.

- [ ] **Step 5.4: Create the subcommand module**

Create `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs`:

```rust
//! `aleph-server bootstrap-runtime` — install managed runtimes via ensure_capability.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use alephcore::runtimes::{
    self, ensure_capability, find_spec, probe, CapabilityLedger, CapabilitySource,
    CapabilityStatus, SPECS,
};
use tokio::sync::RwLock;

use crate::cli::BootstrapRuntimeArgs;

/// Default target set when neither `--only` nor `--skip` is given.
const DEFAULT_TARGETS: &[&str] = &["uv", "playwright-cli"];

/// Detect-only runtimes — probed but never installed.
const DETECT_ONLY: &[&str] = &["git", "cargo"];

pub async fn run(args: BootstrapRuntimeArgs) -> ExitCode {
    let runtimes_dir = match runtimes::get_runtimes_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot locate ~/.aleph/runtimes/: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&runtimes_dir) {
        eprintln!("error: cannot create runtimes dir {}: {e}", runtimes_dir.display());
        return ExitCode::from(2);
    }
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));

    let targets = resolve_targets(&args);
    if targets.is_empty() {
        eprintln!("error: no targets to install (after --only / --skip filtering)");
        return ExitCode::from(2);
    }

    for t in &targets {
        if find_spec(t).is_none() {
            eprintln!("error: unknown capability '{t}'");
            return ExitCode::from(2);
        }
        if !runtimes::supported_on_current_os(t) {
            if args.best_effort {
                writeln_stderr(
                    args.json,
                    &format!(r#"{{"event":"step_skipped","capability":"{t}","reason":"unsupported platform"}}"#),
                    &format!("[skip] {t}: unsupported on current platform"),
                );
                continue;
            }
            eprintln!("error: capability '{t}' not supported on current platform");
            return ExitCode::from(3);
        }
    }

    let mut printer = ProgressPrinter::new(args.json, args.quiet);
    let mut any_failed = false;

    for (idx, cap) in targets.iter().enumerate() {
        if args.force {
            let mut g = ledger.write().await;
            g.update_status(cap, CapabilityStatus::Missing);
        }
        printer.step_start(idx + 1, targets.len(), cap);
        match ensure_capability(cap, &ledger).await {
            Ok(path) => {
                let version = ledger
                    .read()
                    .await
                    .entries
                    .get(*cap)
                    .map(|e| e.version.clone())
                    .unwrap_or_default();
                printer.step_done(cap, &path.display().to_string(), &version);
            }
            Err(e) => {
                printer.step_failed(cap, &e.to_string());
                any_failed = true;
                if !args.best_effort {
                    break;
                }
            }
        }
    }

    // Detection-only section.
    printer.section("System runtimes (detect-only):");
    for cap in DETECT_ONLY {
        let r = probe::probe(cap);
        printer.detect(cap, r.found, r.version.as_deref());
    }

    printer.summary(&targets, any_failed);

    if any_failed && !args.best_effort {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

fn resolve_targets(args: &BootstrapRuntimeArgs) -> Vec<String> {
    let base: Vec<String> = if args.only.is_empty() {
        DEFAULT_TARGETS.iter().map(|s| s.to_string()).collect()
    } else {
        args.only.clone()
    };
    base.into_iter().filter(|t| !args.skip.contains(t)).collect()
}

fn writeln_stderr(json: bool, json_line: &str, pretty: &str) {
    let mut out = std::io::stderr().lock();
    if json {
        let _ = writeln!(out, "{}", json_line);
    } else {
        let _ = writeln!(out, "{}", pretty);
    }
}

struct ProgressPrinter {
    json: bool,
    quiet: bool,
}

impl ProgressPrinter {
    fn new(json: bool, quiet: bool) -> Self {
        Self { json, quiet }
    }

    fn step_start(&mut self, idx: usize, total: usize, cap: &str) {
        if self.quiet { return; }
        if self.json {
            eprintln!(r#"{{"event":"step_start","capability":"{cap}","index":{idx},"total":{total}}}"#);
        } else {
            eprintln!("[{idx}/{total}] {cap} ...");
        }
    }

    fn step_done(&mut self, cap: &str, path: &str, version: &str) {
        if self.quiet { return; }
        if self.json {
            eprintln!(
                r#"{{"event":"step_done","capability":"{cap}","version":"{version}","path":"{path}"}}"#
            );
        } else {
            eprintln!("  ✓ {cap} {version} ({path})");
        }
    }

    fn step_failed(&mut self, cap: &str, err: &str) {
        let err_escaped = err.replace('"', "\\\"").replace('\n', "\\n");
        if self.json {
            eprintln!(
                r#"{{"event":"step_failed","capability":"{cap}","error":"{err_escaped}"}}"#
            );
        } else {
            eprintln!("  ✗ {cap} failed:");
            for line in err.lines() {
                eprintln!("    {}", line);
            }
        }
    }

    fn section(&mut self, title: &str) {
        if self.quiet || self.json { return; }
        eprintln!();
        eprintln!("{}", title);
    }

    fn detect(&mut self, cap: &str, found: bool, version: Option<&str>) {
        if self.quiet { return; }
        if self.json {
            let v = version.unwrap_or("");
            eprintln!(
                r#"{{"event":"detect","capability":"{cap}","found":{found},"version":"{v}"}}"#
            );
        } else {
            let mark = if found { "✓" } else { "✗" };
            let v = version.unwrap_or("(not installed)");
            eprintln!("  {mark} {cap} {v}");
        }
    }

    fn summary(&mut self, targets: &[String], any_failed: bool) {
        if self.json {
            let ready = targets.len() - (any_failed as usize);
            eprintln!(
                r#"{{"event":"summary","ready":{},"failed":{},"total":{}}}"#,
                ready,
                any_failed as usize,
                targets.len(),
            );
        } else if !self.quiet {
            eprintln!();
            if any_failed {
                eprintln!("Runtime bootstrap finished with errors. Re-run to retry.");
            } else {
                eprintln!("Runtime ready. Ledger: ~/.aleph/runtimes/ledger.json");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_targets_default() {
        let args = BootstrapRuntimeArgs::default();
        assert_eq!(resolve_targets(&args), vec!["uv", "playwright-cli"]);
    }

    #[test]
    fn test_resolve_targets_only_replaces_default() {
        let args = BootstrapRuntimeArgs {
            only: vec!["uv".into()],
            ..Default::default()
        };
        assert_eq!(resolve_targets(&args), vec!["uv"]);
    }

    #[test]
    fn test_resolve_targets_skip_filters_default() {
        let args = BootstrapRuntimeArgs {
            skip: vec!["uv".into()],
            ..Default::default()
        };
        assert_eq!(resolve_targets(&args), vec!["playwright-cli"]);
    }

    #[test]
    fn test_resolve_targets_skip_filters_only() {
        let args = BootstrapRuntimeArgs {
            only: vec!["uv".into(), "playwright-cli".into()],
            skip: vec!["playwright-cli".into()],
            ..Default::default()
        };
        assert_eq!(resolve_targets(&args), vec!["uv"]);
    }
}
```

Note: the code references `CapabilitySource` only for the import to type-check — it isn't actually used. Remove it from the `use` line if clippy complains:

```rust
use alephcore::runtimes::{
    self, ensure_capability, find_spec, probe, CapabilityLedger, CapabilityStatus, SPECS,
};
```

(Remove `SPECS` too if not referenced.)

- [ ] **Step 5.5: Register the module**

In `src/bin/aleph-server/commands/mod.rs`, add:

```rust
pub mod bootstrap_runtime;
```

alongside the other `pub mod` declarations. No re-export needed — `main.rs` will use the full path.

- [ ] **Step 5.6: Dispatch the new command in `main.rs`**

Open `src/bin/aleph-server/main.rs`. Find the `match args.command { … }` block (search `Command::Start` or `Command::Status`). Add a new arm for the new variant:

```rust
Some(Command::BootstrapRuntime(br_args)) => {
    return commands::bootstrap_runtime::run(br_args).await;
}
```

If `main` doesn't currently return `ExitCode`, adapt: either change its return type to `std::process::ExitCode` and wrap existing exits, or use `std::process::exit(code.into())`. Prefer the latter for minimal surface change:

```rust
Some(Command::BootstrapRuntime(br_args)) => {
    let code = commands::bootstrap_runtime::run(br_args).await;
    let raw: u8 = match code {
        c if matches!(c, std::process::ExitCode::SUCCESS) => 0,
        _ => 1, // ExitCode::from(N) can't be inspected; rely on runtime exit
    };
    std::process::exit(raw as i32);
}
```

A cleaner pattern: have `bootstrap_runtime::run` return `i32` instead of `ExitCode`. Refactor the module accordingly (change return type, replace `ExitCode::from(n)` with bare `n`) and use:

```rust
Some(Command::BootstrapRuntime(br_args)) => {
    let code = commands::bootstrap_runtime::run(br_args).await;
    std::process::exit(code);
}
```

Adjust `mod.rs` signature and return sites to return `i32`. Simpler and test-friendlier.

- [ ] **Step 5.7: Run the module tests**

```bash
cargo test --bin aleph-server bootstrap_runtime::tests
```

Expected: **PASS** — all four `test_resolve_targets_*` cases pass.

- [ ] **Step 5.8: Run an end-to-end compile check**

```bash
cargo check -p alephcore
cargo check --bin aleph-server
```

Expected: no errors. Fix any type or import warnings.

- [ ] **Step 5.9: Manual smoke test — dry-run the CLI against a known-good capability**

```bash
cargo run --bin aleph-server -- bootstrap-runtime --only git --best-effort
```

Expected output (roughly):

```
[1/1] git ...
  ✗ git failed:
    ... (no install strategy) ...
    Fix options:
      1. Run: aleph-server bootstrap-runtime --only git
      2. Open Panel → Settings → Runtime and click 'Install'.
      3. Install manually — Git — version control. Use `git <subcommand>` ...

System runtimes (detect-only):
  ✓ git <some version>
  ✓ cargo <some version>

Runtime bootstrap finished with errors. Re-run to retry.
```

(Exit code 0 due to `--best-effort`.)

Without `--best-effort`:

```bash
echo $(cargo run --bin aleph-server -- bootstrap-runtime --only git; echo "exit=$?")
```

Expected: `exit=1`.

- [ ] **Step 5.10: Commit**

```bash
git add src/bin/aleph-server/cli.rs src/bin/aleph-server/commands/mod.rs \
        src/bin/aleph-server/commands/bootstrap_runtime/mod.rs \
        src/bin/aleph-server/main.rs
git commit -m "server: add 'bootstrap-runtime' CLI subcommand"
```

---

## Task 6: Non-blocking startup runtime probe

**Context:** `aleph-server start` should populate the ledger on boot so Panel shows accurate state immediately.

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs` — add a `spawn` of the warmup task after AppContext init

- [ ] **Step 6.1: Identify the insertion point**

Read `src/bin/aleph-server/commands/start/mod.rs` end-to-end. Find the call to `start_server()` (or the corresponding subsystem initialization function). The warmup spawn must happen **after** tokio runtime is up but **before** the gateway accepts connections — so just before or right after AppContext construction.

Look for a place that has access to `~/.aleph/` path resolution (via `alephcore::runtimes::get_runtimes_dir`). Any point within `start_server()` after logging is initialized works.

- [ ] **Step 6.2: Write a warmup function**

Append (near the bottom of `src/bin/aleph-server/commands/start/mod.rs`, before the closing module end or any existing helpers):

```rust
/// Non-blocking startup probe. Populates ~/.aleph/runtimes/ledger.json so
/// Panel shows accurate runtime state the moment it connects. Never installs.
async fn runtime_startup_warmup() {
    use alephcore::runtimes::{
        self, ledger::CapabilityEntry, CapabilityLedger, CapabilityStatus, SPECS,
    };
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::RwLock;

    let runtimes_dir = match runtimes::get_runtimes_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "runtime warmup skipped: cannot resolve runtimes dir");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&runtimes_dir) {
        tracing::warn!(error = %e, "runtime warmup skipped: cannot create runtimes dir");
        return;
    }
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();

    let mut missing = Vec::new();
    for spec in SPECS {
        let result = runtimes::probe::probe(spec.name);
        let mut g = ledger.write().await;
        if result.found {
            g.update(CapabilityEntry {
                name: spec.name.into(),
                bin_path: result.bin_path.unwrap_or_default(),
                version: result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: result.source,
                last_probed: now,
            });
        } else if runtimes::supported_on_current_os(spec.name) {
            g.update_status(spec.name, CapabilityStatus::Missing);
            missing.push(spec.name);
        }
    }
    let _ = ledger.write().await.persist();
    if missing.is_empty() {
        tracing::info!("runtime warmup: all capabilities ready");
    } else {
        tracing::warn!(
            missing = ?missing,
            "runtime capabilities missing — browser / python tools will fail until installed. \
             Run 'aleph-server bootstrap-runtime' or open Panel → Settings → Runtime.",
        );
    }
}
```

- [ ] **Step 6.3: Spawn the warmup**

Inside `start_server()` (or whichever async function boots the server), add a `tokio::spawn` call immediately after tracing initialization. Locate a safe anchor — e.g., after `init_tracing(…)` returns successfully:

```rust
tokio::spawn(runtime_startup_warmup());
```

This fires and forgets; the server proceeds with gateway binding without waiting.

- [ ] **Step 6.4: Write an integration test**

Add to the existing test module at the bottom of `src/bin/aleph-server/commands/start/mod.rs` (create one if none exists):

```rust
#[cfg(test)]
mod warmup_tests {
    use super::runtime_startup_warmup;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_warmup_runs_and_persists_ledger() {
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        // Function is non-blocking and must complete without panic.
        runtime_startup_warmup().await;

        // Ledger file must exist and be readable JSON.
        let ledger_path = dir.path().join(".aleph/runtimes/ledger.json");
        assert!(ledger_path.exists(), "ledger must be persisted");
        let content = std::fs::read_to_string(&ledger_path).unwrap();
        let _: serde_json::Value =
            serde_json::from_str(&content).expect("ledger must be valid JSON");
    }
}
```

- [ ] **Step 6.5: Run the warmup test**

```bash
cargo test --bin aleph-server warmup_tests::test_warmup_runs_and_persists_ledger
```

Expected: **PASS**.

- [ ] **Step 6.6: Smoke-test by running the server briefly**

```bash
cargo run --bin aleph-server -- start --port 18799 &
SERVER_PID=$!
sleep 5
# Check ledger was written
test -f ~/.aleph/runtimes/ledger.json && echo "ledger OK"
# Stop the server
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
```

Expected: `ledger OK` printed; server logs contain `"runtime warmup"` at either `info` or `warn` level depending on local environment.

- [ ] **Step 6.7: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs
git commit -m "server: runtime warmup probe on startup (non-blocking)"
```

---

## Task 7: `install.sh` invokes `bootstrap-runtime`

**Files:**
- Modify: `install.sh` — insert a `bootstrap-runtime` invocation block **before** the `# ── System service (auto-start on login) ──` section header (around line 159)

- [ ] **Step 7.1: Add the bootstrap block**

Read `install.sh` lines 155-165 to locate the anchor. The existing line 158 is a blank line followed by the `# ── System service` comment at ~line 159.

Insert the following block right before `# ── System service (auto-start on login) ────────`:

```bash
# ── Bootstrap runtime dependencies ───────────────────────────────

ALEPH_SKIP_RUNTIME="${ALEPH_SKIP_RUNTIME:-0}"
for arg in "$@"; do
    [ "$arg" = "--skip-runtime" ] && ALEPH_SKIP_RUNTIME=1
done

if [ "$ALEPH_SKIP_RUNTIME" = "1" ]; then
    echo ""
    echo "Skipping runtime bootstrap (--skip-runtime or \$ALEPH_SKIP_RUNTIME=1)."
    echo "Run 'aleph-server bootstrap-runtime' later, or use Panel → Settings → Runtime."
else
    echo ""
    echo "Bootstrapping runtime dependencies (fnm → Node LTS → uv → @playwright/cli + Chromium)..."
    echo "(Pass --skip-runtime or set ALEPH_SKIP_RUNTIME=1 to skip.)"
    echo ""
    if ! "$INSTALL_DIR/$BINARY_NAME" bootstrap-runtime --best-effort; then
        echo ""
        echo "Runtime bootstrap hit errors. Aleph will still install."
        echo "   Fix and retry via: aleph-server bootstrap-runtime"
        echo "   Or open Panel → Settings → Runtime for GUI."
    fi
fi
```

- [ ] **Step 7.2: Lint the script**

If `shellcheck` is available:

```bash
shellcheck install.sh
```

Expected: no new warnings introduced by this change. (Pre-existing warnings about unquoted `$HOME` in echo messages are fine.)

- [ ] **Step 7.3: Manual dry-test**

```bash
# Verify that --skip-runtime short-circuits the block without invoking the binary.
bash -n install.sh  # syntax check only
```

Expected: exit 0 (no syntax errors).

Skip full execution in this plan — install.sh downloads a release binary and registers a service; test in VM if needed.

- [ ] **Step 7.4: Commit**

```bash
git add install.sh
git commit -m "install.sh: invoke bootstrap-runtime with --best-effort + --skip-runtime flag"
```

---

## Task 8: `install.ps1` invokes `bootstrap-runtime`

**Files:**
- Modify: `install.ps1` — add a `param([switch]$SkipRuntime)` + a bootstrap block before `Install-AlephService`

- [ ] **Step 8.1: Add the param block**

At the very top of `install.ps1` (before `$ErrorActionPreference`), insert:

```powershell
param(
    [switch]$SkipRuntime
)
```

- [ ] **Step 8.2: Add the bootstrap block**

Locate the line that calls `Install-AlephService` (around line 155-157, the `if ($IsUpgrade) { Install-AlephService }` block). Insert **before** that block:

```powershell
# ── Bootstrap runtime dependencies ───────────────────────────────

$RuntimeSkip = $SkipRuntime.IsPresent -or ($env:ALEPH_SKIP_RUNTIME -eq "1")

if ($RuntimeSkip) {
    Write-Host ""
    Write-Host "Skipping runtime bootstrap (-SkipRuntime or `$env:ALEPH_SKIP_RUNTIME=1)."
    Write-Host "Run 'aleph-server bootstrap-runtime' later, or use Panel -> Settings -> Runtime."
} else {
    Write-Host ""
    Write-Host "Bootstrapping runtime dependencies (fnm -> Node LTS -> uv -> @playwright/cli + Chromium)..."
    Write-Host "(Pass -SkipRuntime or set `$env:ALEPH_SKIP_RUNTIME='1' to skip.)"
    Write-Host ""
    $proc = Start-Process -FilePath $InstalledPath `
        -ArgumentList "bootstrap-runtime", "--best-effort" `
        -NoNewWindow -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Write-Host ""
        Write-Host "Runtime bootstrap hit errors. Aleph will still install." -ForegroundColor Yellow
        Write-Host "   Fix and retry via: aleph-server bootstrap-runtime"
        Write-Host "   Or open Panel -> Settings -> Runtime for GUI."
    }
}
```

- [ ] **Step 8.3: Lint**

If PSScriptAnalyzer is available:

```powershell
Invoke-ScriptAnalyzer -Path install.ps1
```

Expected: no new warnings (pre-existing warnings about `Write-Host` usage are acceptable — install scripts deliberately use it).

- [ ] **Step 8.4: Commit**

```bash
git add install.ps1
git commit -m "install.ps1: invoke bootstrap-runtime with --best-effort + -SkipRuntime switch"
```

---

## Task 9: Panel Runtime page + API wrapper

**Files:**
- Create: `interfaces/webchat/src/api/runtime.rs`
- Create: `interfaces/webchat/src/views/settings/runtime.rs`
- Modify: `interfaces/webchat/src/api/mod.rs` — declare the new module
- Modify: `interfaces/webchat/src/views/settings/mod.rs` — register the route + menu entry

**Background reading:**
- `interfaces/webchat/src/api/` directory — read any existing API wrapper (e.g., `browser.rs` if it exists or any other in that folder) to match style
- `interfaces/webchat/src/views/settings/browser.rs` — reference structure and Leptos patterns (already read)
- `interfaces/webchat/src/views/settings/mod.rs` — see how `browser.rs` view is registered

Before starting, run:
```bash
ls interfaces/webchat/src/api/
cat interfaces/webchat/src/views/settings/mod.rs | head -80
grep -n "browser\|BrowserView\|Browser" interfaces/webchat/src/views/settings/mod.rs
```

- [ ] **Step 9.1: Create the API wrapper**

Create `interfaces/webchat/src/api/runtime.rs`:

```rust
//! Runtime RPC wrapper — list / install / refresh.
//!
//! Thin bridge between `RuntimeView` and the Gateway's `runtimes.*` RPCs.

use serde::{Deserialize, Serialize};

use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub name: String,
    pub status: String,            // "Missing" | "Probing" | "Bootstrapping" | "Ready" | "Stale"
    pub bin_path: Option<String>,
    pub version: Option<String>,
    pub llm_hint: Option<String>,
    pub deps: Vec<String>,
    pub supported_on_current_os: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimesListResponse {
    pub runtimes: Vec<RuntimeInfo>,
}

pub struct RuntimeApi;

impl RuntimeApi {
    pub async fn list(state: &DashboardState) -> Result<RuntimesListResponse, String> {
        state
            .rpc_call::<RuntimesListResponse>("runtimes.list", serde_json::Value::Null)
            .await
    }

    pub async fn refresh(state: &DashboardState) -> Result<RuntimesListResponse, String> {
        state
            .rpc_call::<RuntimesListResponse>("runtimes.refresh", serde_json::Value::Null)
            .await
    }

    pub async fn install(state: &DashboardState, capability: &str) -> Result<(), String> {
        let params = serde_json::json!({ "capability": capability });
        state
            .rpc_call::<serde_json::Value>("runtimes.install", params)
            .await
            .map(|_| ())
    }
}
```

Adapt the `state.rpc_call::<T>(method, params)` call signature to match your codebase's conventions. If the API module uses a different helper (e.g., `call_rpc_method`, `BrowserConfigApi::get`-style per-method wrappers), mirror that. Grep for one of the existing API callers:

```bash
grep -n "rpc_call\|call_rpc\|invoke_rpc" interfaces/webchat/src/api/*.rs | head
```

Adjust `Self::list / refresh / install` bodies accordingly. The return types and parameter shapes above are the correct on-wire contract (matching `src/gateway/handlers/runtimes.rs`).

- [ ] **Step 9.2: Declare the new module in `interfaces/webchat/src/api/mod.rs`**

Add `pub mod runtime;` alongside the other `pub mod browser;` declarations. If the project re-exports APIs via `pub use runtime::*`, add a matching line.

- [ ] **Step 9.3: Create the Runtime view**

Create `interfaces/webchat/src/views/settings/runtime.rs`:

```rust
//! Settings → Runtime: step-indicator list + Install / Refresh actions.

use crate::api::runtime::{RuntimeApi, RuntimeInfo, RuntimesListResponse};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn RuntimeView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let log_lines = RwSignal::new(Vec::<String>::new());
    let busy = RwSignal::new(false);

    // Initial load
    {
        let state = state.clone();
        spawn_local(async move {
            match RuntimeApi::list(&state).await {
                Ok(r) => {
                    runtimes.set(r.runtimes);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load runtimes: {e}")));
                    loading.set(false);
                }
            }
        });
    }

    let do_refresh = {
        let state = state.clone();
        move |_ev| {
            let state = state.clone();
            busy.set(true);
            spawn_local(async move {
                match RuntimeApi::refresh(&state).await {
                    Ok(r) => {
                        runtimes.set(r.runtimes);
                        log_lines.update(|v| v.push("▸ refreshed".into()));
                    }
                    Err(e) => error.set(Some(format!("Refresh failed: {e}"))),
                }
                busy.set(false);
            });
        }
    };

    let do_install_missing = {
        let state = state.clone();
        move |_ev| {
            let state = state.clone();
            let targets: Vec<String> = runtimes
                .get()
                .iter()
                .filter(|r| r.status == "Missing" && r.supported_on_current_os)
                .map(|r| r.name.clone())
                .collect();
            if targets.is_empty() {
                log_lines.update(|v| v.push("▸ nothing to install — all ready".into()));
                return;
            }
            busy.set(true);
            spawn_local(async move {
                for cap in &targets {
                    log_lines.update(|v| v.push(format!("▸ [{cap}] starting…")));
                    match RuntimeApi::install(&state, cap).await {
                        Ok(_) => log_lines.update(|v| v.push(format!("▸ [{cap}] accepted (see events)"))),
                        Err(e) => log_lines.update(|v| v.push(format!("▸ [{cap}] FAILED: {e}"))),
                    }
                }
                // Refresh after all installs triggered.
                match RuntimeApi::refresh(&state).await {
                    Ok(r) => runtimes.set(r.runtimes),
                    Err(e) => error.set(Some(format!("Post-install refresh failed: {e}"))),
                }
                busy.set(false);
            });
        }
    };

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">"Runtime"</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    "Bootstrap runtime dependencies that power browser automation, Python execution, and skills."
                </p>
            </div>

            {move || match error.get() {
                Some(e) => Some(view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                        {e}
                    </div>
                }.into_any()),
                None => None,
            }}

            {move || if loading.get() {
                view! {
                    <div class="flex items-center justify-center py-12">
                        <div class="text-text-tertiary">"Loading…"</div>
                    </div>
                }.into_any()
            } else {
                let rs = runtimes.get();
                view! {
                    <div class="bg-surface-raised rounded-lg border border-border p-6 space-y-4">
                        <div class="space-y-2">
                            {rs.iter().map(|r| runtime_row(r)).collect_view()}
                        </div>
                        <div class="flex items-center gap-2 pt-4 border-t border-border">
                            <button
                                on:click=do_install_missing.clone()
                                disabled=move || busy.get()
                                class="px-3 py-2 bg-primary text-white rounded-lg text-sm disabled:opacity-50"
                            >
                                "Install missing"
                            </button>
                            <button
                                on:click=do_refresh.clone()
                                disabled=move || busy.get()
                                class="px-3 py-2 bg-surface border border-border text-text-primary rounded-lg text-sm disabled:opacity-50"
                            >
                                "Refresh"
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}

            // Log pane
            <div class="bg-surface-raised rounded-lg border border-border p-4">
                <h2 class="text-sm font-semibold text-text-primary mb-2">"Install log"</h2>
                <pre class="text-xs text-text-secondary whitespace-pre-wrap max-h-48 overflow-y-auto font-mono">
                    {move || {
                        let v = log_lines.get();
                        if v.is_empty() { "[idle]".to_string() } else { v.join("\n") }
                    }}
                </pre>
            </div>
        </div>
    }
}

fn runtime_row(r: &RuntimeInfo) -> impl IntoView {
    let (dot, dot_class) = match r.status.as_str() {
        "Ready" => ("●", "text-success"),
        "Probing" | "Bootstrapping" => ("◐", "text-info"),
        "Stale" => ("◐", "text-warning"),
        _ => ("○", "text-text-tertiary"),
    };
    let version = r.version.clone().unwrap_or_else(|| "—".into());
    let path = r.bin_path.clone().unwrap_or_default();
    let name = r.name.clone();

    view! {
        <div class="flex items-center justify-between py-2">
            <div class="flex items-center gap-3">
                <span class=format!("text-lg {}", dot_class)>{dot}</span>
                <div>
                    <div class="font-medium text-text-primary">{name.clone()}</div>
                    <div class="text-xs text-text-tertiary">{path}</div>
                </div>
            </div>
            <div class="text-sm text-text-secondary">{version}</div>
        </div>
    }
}
```

Notes:
- Live EventBus subscription for `RuntimeInstallProgress` is deferred (YAGNI — spec §9). The `do_install_missing` flow relies on a post-install `refresh` to show final state, which is sufficient for V1.
- `#[component]` is the Leptos decorator style — match whatever your codebase's other views use (read `browser.rs` to confirm).

- [ ] **Step 9.4: Register the route + menu entry**

In `interfaces/webchat/src/views/settings/mod.rs`, find where `BrowserView` is registered (both as a route and as a sidebar item). Add matching entries:

```rust
pub mod runtime;
// …
pub use runtime::RuntimeView;
```

Then in the router/menu — exact wiring depends on your app structure. Grep:

```bash
grep -n "BrowserView\|browser" interfaces/webchat/src/views/settings/mod.rs
```

Mirror each `BrowserView` reference with a corresponding `RuntimeView` reference, using `/settings/runtime` as the path and a label like `"Runtime"`. Place it in the sidebar between `Browser` and `Execution` (or wherever is conventional — see the sidebar ordering).

- [ ] **Step 9.5: Build the WASM bundle to catch compile errors**

```bash
just wasm
```

Expected: success. If compile errors reference `collect_view`, `spawn_local`, signal API shapes, or missing `DashboardState::rpc_call`, adapt to the exact helpers in use in `browser.rs` — your codebase's abstractions are the source of truth, not the snippets above.

- [ ] **Step 9.6: Full dev run + manual smoke**

```bash
just dev &
DEV_PID=$!
sleep 8
# Navigate to http://127.0.0.1:18790/#/settings/runtime in a browser (or open
# the macOS panel app and click "Runtime" in the sidebar).
# Verify:
#  - The page loads without console errors.
#  - At least fnm / node / uv / playwright-cli rows render.
#  - Clicking Refresh updates the list.
#  - Clicking "Install missing" emits log-pane lines.
kill $DEV_PID 2>/dev/null
```

- [ ] **Step 9.7: Commit**

```bash
git add interfaces/webchat/src/api/runtime.rs \
        interfaces/webchat/src/api/mod.rs \
        interfaces/webchat/src/views/settings/runtime.rs \
        interfaces/webchat/src/views/settings/mod.rs
git commit -m "webchat: add Settings → Runtime page with step indicator + install log"
```

---

## Task 10: Browser page runtime banner + residual cleanup + delete stale review file

**Files:**
- Create: `interfaces/webchat/src/views/settings/browser_runtime_banner.rs`
- Modify: `interfaces/webchat/src/views/settings/browser.rs` — embed `<RuntimeSummaryBanner />` at top
- Modify: `src/config/types/general.rs` — two comment sites
- Modify: `src/browser/profile.rs` — one comment site
- Modify: `src/builtin_tools/browser_tools/tabs.rs` — two comment sites
- Modify: `src/builtin_tools/browser_tools/mod.rs` — one comment site
- Delete: `review-results/browser.md`

- [ ] **Step 10.1: Create the summary banner component**

Create `interfaces/webchat/src/views/settings/browser_runtime_banner.rs`:

```rust
//! Compact runtime-readiness banner shown at the top of the Browser page.
//!
//! Keeps the Browser config page focused on configuration while giving
//! visibility into whether the underlying runtime is installed.

use crate::api::runtime::{RuntimeApi, RuntimeInfo};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

const BROWSER_RUNTIMES: &[&str] = &["fnm", "node", "playwright-cli"];

#[component]
pub fn RuntimeSummaryBanner() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loaded = RwSignal::new(false);

    {
        let state = state.clone();
        spawn_local(async move {
            if let Ok(r) = RuntimeApi::list(&state).await {
                runtimes.set(r.runtimes);
            }
            loaded.set(true);
        });
    }

    view! {
        {move || {
            if !loaded.get() { return None; }
            let list = runtimes.get();
            let missing: Vec<String> = list
                .iter()
                .filter(|r| BROWSER_RUNTIMES.contains(&r.name.as_str())
                         && r.status != "Ready"
                         && r.supported_on_current_os)
                .map(|r| r.name.clone())
                .collect();
            if missing.is_empty() {
                Some(view! {
                    <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                        <span>"✓"</span>
                        <span>"Browser runtime ready"</span>
                    </div>
                }.into_any())
            } else {
                let names = missing.join(", ");
                Some(view! {
                    <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-sm flex items-center justify-between gap-2">
                        <span>{format!("⚠ Browser runtime missing: {names}")}</span>
                        <a href="/#/settings/runtime"
                           class="text-sm font-medium underline hover:no-underline">
                            "Configure →"
                        </a>
                    </div>
                }.into_any())
            }
        }}
    }
}
```

Adapt class names and routing (`/#/settings/runtime` vs `/settings/runtime` depending on router style) to match the other cross-page links in the codebase.

- [ ] **Step 10.2: Embed the banner in the Browser page**

Edit `interfaces/webchat/src/views/settings/browser.rs` around line 86-91 (the `<div>` with "Configure browser automation..."). Insert the banner component right before the error display:

```rust
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">"Browser"</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    "Configure browser automation for web browsing tools."
                </p>
            </div>

            <RuntimeSummaryBanner />
            // ← NEW LINE

            {move || {
                if loading.get() {
```

Also add the import at the top of `browser.rs`:

```rust
use crate::views::settings::browser_runtime_banner::RuntimeSummaryBanner;
```

Declare the new module in `interfaces/webchat/src/views/settings/mod.rs`:

```rust
pub mod browser_runtime_banner;
```

- [ ] **Step 10.3: Build WASM, verify clean compile**

```bash
just wasm
```

Expected: success.

- [ ] **Step 10.4: Apply residual comment cleanups**

In `src/config/types/general.rs`:

At line 44 — replace the doc comment:
```rust
/// Browser system configuration (profiles, SSRF policy, Playwright MCP).
```
with:
```rust
/// Browser system configuration (profiles, SSRF policy, Playwright CLI).
```

At line 116 (inside the TOML example block) — replace:
```
        [browser.playwright_mcp]
```
with:
```
        [browser.playwright_cli]
```

In `src/browser/profile.rs` at line 25 — replace:
```rust
    /// Aleph launches and manages a dedicated browser instance (chromiumoxide).
```
with:
```rust
    /// Aleph launches and manages a dedicated browser instance (Playwright CLI managed via fnm).
```

In `src/builtin_tools/browser_tools/tabs.rs`:

At line 111 — replace:
```rust
                            // Playwright MCP: "Tab N: URL"
```
with:
```rust
                            // Playwright CLI: "Tab N: URL"
```

At line 140 — replace:
```rust
                // Neither Chrome MCP nor Playwright MCP has explicit tab switch.
```
with:
```rust
                // Neither Chrome DevTools MCP nor Playwright CLI has explicit tab switch.
```

In `src/builtin_tools/browser_tools/mod.rs` at line 43 — replace:
```rust
            // Playwright MCP format: "Tab N: URL"
```
with:
```rust
            // Playwright CLI format: "Tab N: URL"
```

- [ ] **Step 10.5: Delete the stale review artefact**

```bash
git rm review-results/browser.md
```

- [ ] **Step 10.6: Run the residual-scrub guard**

```bash
grep -rn "playwright_mcp\|Playwright MCP" src/ interfaces/ examples/ Cargo.toml \
    | grep -v 'alias = "playwright_mcp"' \
    | grep -v 'old_playwright_mcp_toml'
```

Expected: empty output. If anything remains, it's a miss — update and re-run.

- [ ] **Step 10.7: Full build + test to verify nothing broke**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib
just wasm
```

Expected: all green.

- [ ] **Step 10.8: Commit**

```bash
git add src/config/types/general.rs \
        src/browser/profile.rs \
        src/builtin_tools/browser_tools/tabs.rs \
        src/builtin_tools/browser_tools/mod.rs \
        interfaces/webchat/src/views/settings/browser_runtime_banner.rs \
        interfaces/webchat/src/views/settings/browser.rs \
        interfaces/webchat/src/views/settings/mod.rs
git rm review-results/browser.md
git commit -m "webchat: add runtime summary banner to Browser page; clean playwright-mcp comment residuals; drop review-results/browser.md"
```

---

## Final Verification

After all ten tasks land, run the success-criteria probes from the spec:

- [ ] **V1: Compile + lint**

```bash
cargo check -p alephcore
cargo clippy -p alephcore -- -D warnings
```

- [ ] **V2: Test suite**

```bash
cargo test -p alephcore --lib
cargo test --bin aleph-server
```

- [ ] **V3: Residual scrub**

```bash
grep -rn "playwright_mcp\|Playwright MCP" src/ interfaces/ examples/ Cargo.toml \
    | grep -v 'alias = "playwright_mcp"' \
    | grep -v 'old_playwright_mcp_toml'
```

Expected: empty.

- [ ] **V4: Local smoke — bootstrap-runtime with probe**

```bash
cargo run --bin aleph-server -- bootstrap-runtime --only git --best-effort
```

Expected: completes with exit 0, prints the actionable error message, prints detect-only section.

- [ ] **V5: Local smoke — startup warmup persists the ledger**

```bash
rm -f ~/.aleph/runtimes/ledger.json
cargo run --bin aleph-server -- start --port 18799 &
SERVER_PID=$!
sleep 6
test -f ~/.aleph/runtimes/ledger.json && echo "ledger OK"
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
```

Expected: `ledger OK`.

- [ ] **V6: Panel route smoke**

Start `just dev`, navigate to `/#/settings/runtime` in the Panel. Rows render, Refresh works, error states surface cleanly.

- [ ] **V7: Browser banner smoke**

Navigate to `/#/settings/browser` — the banner appears above the existing settings cards, showing either `✓ Browser runtime ready` or a `⚠` amber banner with Configure link.

---

## Self-Review Notes (for the writer)

**Spec coverage check:** every decision in §§1-13 of `2026-04-14-runtime-bootstrap-design.md` maps to a task:
- §1.1 `uv` spec venv → Task 2
- §1.2 `expand_home` bug → Task 1
- §2 CLI subcommand → Task 5
- §3 install scripts → Tasks 7, 8
- §4 startup warmup + friendly errors → Tasks 6, 4
- §5 stderr field → Task 3
- §6 Panel page → Task 9
- §6.5 Browser banner → Task 10
- §7 residual cleanup → Task 10
- §8 File plan → matches Tasks 1-10
- §9 testing → each task includes unit tests + V1-V7 integration checks

**Type consistency check:** `CapabilityStatus`, `CapabilityLedger`, `ProbeResult`, `RuntimeInfo`, `RuntimesListResponse`, `RuntimeInstallProgressEvent` — all spelled identically across Rust and TypeScript-adjacent (Leptos) surfaces. `bootstrap-runtime` kebab-cased everywhere; `BootstrapRuntimeArgs` / `BootstrapRuntime` CamelCase in Rust only. `--skip-runtime` / `-SkipRuntime` / `$ALEPH_SKIP_RUNTIME` spellings match spec §3.

**Placeholder scan:** no `TBD` / `TODO` / "fill in later". Each step contains actual code, commands, and expected output.
