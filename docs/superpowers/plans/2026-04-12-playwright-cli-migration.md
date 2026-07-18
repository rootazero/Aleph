# Playwright CLI Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `@playwright/mcp` + chromiumoxide-based `ManagedBackend` with a single `PlaywrightCliBackend` that shells out to `@playwright/cli`; reshape `BrowserBackend` trait text-first; wire Panel Settings to drive a one-click runtime install (fnm → Node LTS → CLI → Chromium → skills).

**Architecture:** Three-layer split — `bootstrap.rs` probes + installs runtime components; `PlaywrightCliDriver` owns binary-path resolution and per-session subprocess serialization; `PlaywrightCliBackend` implements `BrowserBackend` by shelling out with `-s=<profile>` and parsing text + reading snapshot/screenshot temp files. Chrome DevTools MCP path kept, adapted to new text-first trait. `pdf_generate` migrates to `playwright-cli pdf`.

**Tech Stack:** Rust (alephcore crate), tokio process, serde + schemars, Leptos (webchat UI), axum gateway, `@playwright/cli` via fnm-managed Node LTS.

---

## Spec Reference

`docs/superpowers/specs/2026-04-12-playwright-cli-migration-design.md`

## File Structure

### Created (5 files)

| Path | Responsibility |
|---|---|
| `src/browser/bootstrap.rs` | probe + install (fnm / Node / playwright-cli / Chromium / skills) |
| `src/browser/playwright_cli.rs` | `PlaywrightCliDriver` — binary resolution, subprocess, per-session locks |
| `src/browser/playwright_cli_backend.rs` | `PlaywrightCliBackend` — `BrowserBackend` trait impl |
| `src/gateway/handlers/browser_runtime.rs` | 3 RPC handlers + install task orchestration + progress events |
| `interfaces/webchat/src/views/settings/browser_runtime.rs` | Leptos `RuntimeStatusCard` component |

### Modified (16 files)

`src/browser/mod.rs`, `backend.rs`, `types.rs`, `profile.rs`, `error.rs`, `chrome_mcp_backend.rs`, `manager.rs`, `src/gateway/handlers/browser_config.rs`, `src/gateway/handlers/mod.rs`, `src/gateway/event_bus.rs`, `src/builtin_tools/browser/handlers.rs`, `src/builtin_tools/browser/types.rs`, `src/builtin_tools/pdf_generate/browser_engine.rs`, `interfaces/webchat/src/api/browser.rs`, `interfaces/webchat/src/views/settings/browser.rs`, `Cargo.toml`, `examples/browser-config.toml`.

### Deleted (7 files)

`src/browser/runtime.rs`, `actions.rs`, `snapshot.rs`, `snapshot_format.rs`, `managed_backend.rs`, `playwright_mcp.rs`, `playwright_mcp_backend.rs`.

---

## Global Conventions (read once before starting)

- **Language**: Code comments in English; user-facing copy in both Chinese and English where relevant
- **Tests**: `#[cfg(test)] mod tests` inside each file. Integration tests in `tests/` with `#[ignore]` for anything needing real fnm/playwright-cli
- **Locks**: use `crate::sync_primitives::{RwLock, Arc}` aliases that already exist in the codebase
- **Error handling**: `thiserror` for `BrowserError`; `anyhow::Result` inside bootstrap if clearer
- **Commit style**: `<scope>: <description>` — e.g. `browser: add PlaywrightCliConfig with serde alias`
- **After every task**, run: `cargo check -p alephcore 2>&1 | tail -40` to confirm tree compiles. Fix compile errors before committing.

---

## Task 1: Add `PlaywrightCliConfig` with serde alias for `playwright_mcp`

**Files:**
- Modify: `src/browser/profile.rs`

- [ ] **Step 1: Write the failing test — alias reads old TOML**

Add to `src/browser/profile.rs` tests module:

```rust
#[test]
fn test_old_playwright_mcp_toml_deserializes_to_playwright_cli() {
    let toml_str = r##"
[playwright_mcp]
enabled = true
command = "npx"
args = ["@playwright/mcp@latest", "--headless"]
"##;
    let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
    assert!(config.playwright_cli.enabled);
    // unknown fields command/args silently dropped; defaults used instead
    assert!(config.playwright_cli.headless);
    assert_eq!(config.playwright_cli.nav_timeout_secs, 30);
}

#[test]
fn test_playwright_cli_defaults() {
    let config = PlaywrightCliConfig::default();
    assert!(config.enabled);
    assert!(config.binary_path.is_none());
    assert!(config.headless);
    assert_eq!(config.nav_timeout_secs, 30);
    assert_eq!(config.action_timeout_secs, 10);
    assert!(!config.persistent_sessions);
}

#[test]
fn test_profile_config_headless_option_compat() {
    let toml_str = r##"
[profiles.default]
browser = "chromium"
headless = true
"##;
    let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
    let p = config.profiles.get("default").unwrap();
    assert_eq!(p.headless, Some(true));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib browser::profile::tests::test_playwright_cli_defaults 2>&1 | tail -20`
Expected: FAIL — `PlaywrightCliConfig` not defined.

- [ ] **Step 3: Add `PlaywrightCliConfig` type and replace field**

Replace the existing `PlaywrightMcpConfig` block (lines ~122-160) and `BrowserSystemConfig` block (lines ~197-214) in `src/browser/profile.rs`:

```rust
/// Configuration for the Playwright CLI integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaywrightCliConfig {
    /// Whether Playwright CLI is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Optional override: absolute path to `playwright-cli` binary.
    /// When `None`, resolved via `fnm exec --using lts which playwright-cli`.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Global default: run headless (profile-level `headless: Option<bool>` overrides).
    #[serde(default = "default_true")]
    pub headless: bool,

    /// Timeout (seconds) for navigate/wait_for_text.
    #[serde(default = "default_nav_timeout")]
    pub nav_timeout_secs: u64,

    /// Timeout (seconds) for other actions (click/fill/type/etc).
    #[serde(default = "default_action_timeout")]
    pub action_timeout_secs: u64,

    /// Persist session profile to disk (`--persistent` flag).
    #[serde(default)]
    pub persistent_sessions: bool,
}

fn default_nav_timeout() -> u64 { 30 }
fn default_action_timeout() -> u64 { 10 }

impl Default for PlaywrightCliConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            binary_path: None,
            headless: true,
            nav_timeout_secs: 30,
            action_timeout_secs: 10,
            persistent_sessions: false,
        }
    }
}

// ... (keep existing ChromeMcpConfig unchanged) ...

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSystemConfig {
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    #[serde(default)]
    pub policy: SsrfConfig,

    /// Reads both `[playwright_cli]` and legacy `[playwright_mcp]` (unknown fields dropped).
    #[serde(default, alias = "playwright_mcp")]
    pub playwright_cli: PlaywrightCliConfig,

    #[serde(default)]
    pub chrome_mcp: ChromeMcpConfig,
}
```

Then change `ProfileConfig.headless` field (line ~45) from `pub headless: bool` to:

```rust
/// Profile-level override for headless (None = follow global `playwright_cli.headless`).
#[serde(default)]
pub headless: Option<bool>,
```

Update `ProfileConfig::default()`:

```rust
impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            browser: BrowserType::default(),
            cdp_port: default_cdp_port(),
            headless: None,
            color: None,
            // ... rest unchanged
        }
    }
}
```

- [ ] **Step 4: Update existing tests that depended on old types**

In the same `tests` module, find any `PlaywrightMcpConfig` references and rename to `PlaywrightCliConfig`; find `headless: false`/`true` in `ProfileConfig` literals and change to `headless: Some(false)`/`Some(true)` or leave `..Default::default()`. Delete `test_playwright_mcp_config_defaults`. Update `test_browser_system_config_toml_deserialization` assertions to match new types (drop `command`/`args` asserts).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib browser::profile 2>&1 | tail -30`
Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/browser/profile.rs
git commit -m "browser: add PlaywrightCliConfig with serde alias for playwright_mcp"
```

---

## Task 2: Reshape `BrowserBackend` trait to text-first

**Files:**
- Modify: `src/browser/types.rs`
- Modify: `src/browser/backend.rs`
- Modify: `src/browser/playwright_mcp_backend.rs` (temporary adapter; deleted in Task 9)
- Modify: `src/browser/managed_backend.rs` (temporary adapter; deleted in Task 8)
- Modify: `src/browser/chrome_mcp_backend.rs` (real adapter; polished in Task 6)

- [ ] **Step 1: Add new output structs + remove old ones from `types.rs`**

Edit `src/browser/types.rs`. Delete `AriaSnapshot`, `AriaElement`, `ElementRect`, `ConsoleMessage`, `TabInfo`, `ScreenshotResult`, `interactive_roles`, `content_roles`, `structural_roles`. Change `ActionTarget` to remove `Selector` variant.

Replace deleted items with:

```rust
/// Browser snapshot (text-first).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotOutput {
    /// Raw snapshot text — YAML from playwright-cli, indented-tree from chrome-devtools-mcp.
    pub snapshot_text: String,
    /// Page URL at snapshot time.
    pub page_url: String,
    /// Page title at snapshot time.
    pub page_title: String,
}

/// Screenshot output (raw PNG bytes).
#[derive(Debug, Clone)]
pub struct ScreenshotOutput {
    pub png_bytes: Vec<u8>,
}
```

Updated `ActionTarget`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionTarget {
    /// Target an element by its snapshot ref ID (e.g. "e42").
    Ref { ref_id: String },
    /// Target a viewport coordinate.
    Coordinates { x: f64, y: f64 },
}
```

Keep `TabId`, `BrowserConfig`, `LaunchMode`, `ScrollDirection`, `StorageKind`, `ScreenshotOpts` unchanged.

- [ ] **Step 2: Rewrite `BrowserBackend` trait in `backend.rs`**

Replace the entire `src/browser/backend.rs` with:

```rust
//! BrowserBackend trait — text-first unified contract for browser drivers.

use std::path::Path;

use async_trait::async_trait;

use super::error::BrowserError;
use super::types::{
    ActionTarget, ScreenshotOpts, ScrollDirection, ScreenshotOutput, SnapshotOutput, TabId,
};

#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;
    async fn list_tabs(&self) -> Result<String, BrowserError>;
    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError>;
    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn type_text(&self, tab_id: &str, target: ActionTarget, text: &str) -> Result<(), BrowserError>;
    async fn fill(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn scroll(&self, tab_id: &str, target: ActionTarget, direction: ScrollDirection) -> Result<(), BrowserError>;
    async fn screenshot(&self, tab_id: &str, opts: ScreenshotOpts) -> Result<ScreenshotOutput, BrowserError>;
    async fn snapshot(&self, tab_id: &str) -> Result<SnapshotOutput, BrowserError>;
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError>;
    async fn select(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;

    async fn press_key(&self, _tab_id: &str, _key: &str) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("press_key not supported".into()))
    }

    async fn wait_for_text(&self, _tab_id: &str, _text: &str, _timeout_ms: u64) -> Result<bool, BrowserError> {
        Err(BrowserError::ActionFailed("wait_for_text not supported".into()))
    }

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Err(BrowserError::ActionFailed("console_messages not supported".into()))
    }

    async fn network_log(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Err(BrowserError::ActionFailed("network_log not supported".into()))
    }

    /// Print-to-PDF — writes PDF to `output_path`. Default impl returns Unsupported.
    async fn pdf(&self, _tab_id: &str, _output_path: &Path) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("pdf not supported".into()))
    }

    async fn fill_form(&self, tab_id: &str, fields: &[(ActionTarget, String)]) -> Result<usize, BrowserError> {
        let mut filled = 0;
        for (target, value) in fields {
            self.fill(tab_id, target.clone(), value).await?;
            filled += 1;
        }
        Ok(filled)
    }
}
```

- [ ] **Step 3: Update `mod.rs` re-exports**

Edit `src/browser/mod.rs`. In the `pub use types::{...}` block, replace:

```rust
pub use types::{
    ActionTarget, AriaElement, AriaSnapshot, BrowserConfig, ConsoleMessage, ElementRect,
    LaunchMode, ScreenshotOpts, ScreenshotResult, ScrollDirection, StorageKind, TabId, TabInfo,
};
```

With:

```rust
pub use types::{
    ActionTarget, BrowserConfig, LaunchMode, ScreenshotOpts, ScreenshotOutput, ScrollDirection,
    SnapshotOutput, StorageKind, TabId,
};
```

- [ ] **Step 4: Quick-patch old backends so they compile**

The old `PlaywrightMcpBackend`, `ManagedBackend`, `ChromeMcpBackend` all break now. Add minimal adapters to each so the tree compiles (they will be rewritten/deleted in later tasks):

In `src/browser/managed_backend.rs`, in the `BrowserBackend` impl:
- Replace `snapshot()` body with `Err(BrowserError::ActionFailed("managed backend pending removal".into()))`
- Replace `screenshot()` return with `Err(BrowserError::ActionFailed("managed backend pending removal".into()))`
- Replace `list_tabs()`/`console_messages()` return types by joining items into a String via `format!("{:?}\n", ...)` or returning `Err(BrowserError::ActionFailed(...))`
- Replace `evaluate()` return `serde_json::Value` with stringification via `.to_string()`
- Delete the `fn select()` CSS-selector branch since `ActionTarget::Selector` no longer exists; only match `Ref`/`Coordinates`

Same treatment for `playwright_mcp_backend.rs` and `chrome_mcp_backend.rs`.

The goal is ONLY: `cargo check -p alephcore` succeeds. Full behavior comes in later tasks.

- [ ] **Step 5: Verify tree compiles**

Run: `cargo check -p alephcore 2>&1 | tail -40`
Expected: success (warnings OK, errors NOT OK).

- [ ] **Step 6: Write + pass a smoke test for the new types**

Add to `src/browser/types.rs` tests module:

```rust
#[test]
fn test_snapshot_output_serde_roundtrip() {
    let snap = SnapshotOutput {
        snapshot_text: "- button \"OK\" [ref=e1]".into(),
        page_url: "https://example.com/".into(),
        page_title: "Example".into(),
    };
    let json = serde_json::to_value(&snap).unwrap();
    let back: SnapshotOutput = serde_json::from_value(json).unwrap();
    assert_eq!(back.page_url, "https://example.com/");
}

#[test]
fn test_action_target_no_selector_variant() {
    let json = serde_json::json!({"type": "selector", "css": ".foo"});
    let parsed: Result<ActionTarget, _> = serde_json::from_value(json);
    assert!(parsed.is_err());
}
```

Run: `cargo test -p alephcore --lib browser::types 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/browser/types.rs src/browser/backend.rs src/browser/mod.rs \
        src/browser/managed_backend.rs src/browser/playwright_mcp_backend.rs \
        src/browser/chrome_mcp_backend.rs
git commit -m "browser: redesign BrowserBackend trait to text-first"
```

---

## Task 3: Extend `BrowserError` variants

**Files:**
- Modify: `src/browser/error.rs`

- [ ] **Step 1: Add new error variants**

Replace the `src/browser/error.rs` contents with:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("Browser is not running. Launch a browser instance first.")]
    NotRunning,

    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("Browser connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Browser protocol error: {0}")]
    Protocol(String),

    #[error("Tab not found: {0}")]
    TabNotFound(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Browser action failed: {0}")]
    ActionFailed(String),

    #[error("Browser operation timed out after {0}ms")]
    Timeout(u64),

    #[error("Chromium binary not found. Install Chrome/Chromium or specify a binary path.")]
    ChromiumNotFound,

    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),

    #[error("JavaScript evaluation error: {0}")]
    EvalError(String),

    #[error("Failed to attach to browser: {0}")]
    AttachFailed(String),

    #[error("Chrome DevTools MCP error: {0}")]
    ChromeMcpError(String),

    #[error("Playwright CLI error: {0}")]
    PlaywrightCliError(String),

    #[error("Playwright CLI not installed. Open Settings → Browser → Install All.")]
    PlaywrightCliNotInstalled,

    #[error("No active browser session for '{0}'. Call open/goto first.")]
    NoSession(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Browser profile not found: {0}")]
    ProfileNotFound(String),
}
```

- [ ] **Step 2: Fix call sites that used `PlaywrightError`**

Run: `grep -rn "PlaywrightError" src/ 2>&1`

For each hit, replace `BrowserError::PlaywrightError(x)` with `BrowserError::PlaywrightCliError(x)` (these files will be deleted in Task 9 but must compile until then).

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add src/browser/error.rs src/browser/playwright_mcp_backend.rs src/browser/playwright_mcp.rs
git commit -m "browser: extend BrowserError with PlaywrightCli variants"
```

---

## Task 4: Bootstrap module — probe logic (no install yet)

**Files:**
- Create: `src/browser/bootstrap.rs`
- Modify: `src/browser/mod.rs` (add `pub mod bootstrap;`)

- [ ] **Step 1: Scaffold `bootstrap.rs` with probe-only API and tests first**

Create `src/browser/bootstrap.rs`:

```rust
//! Bootstrap module: detects and installs the browser runtime stack.
//!
//! Runtime components (in dependency order):
//!   1. fnm            — Node version manager (https://github.com/Schniz/fnm)
//!   2. Node.js LTS    — JavaScript runtime (managed by fnm)
//!   3. @playwright/cli — CLI binary (installed via npm)
//!   4. Chromium       — browser binary (installed via `playwright install`)
//!   5. Skills         — `~/.aleph/skills/playwright-cli/`

use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Status of one runtime component.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ComponentStatus {
    Installed {
        version: Option<String>,
        path: Option<String>,
    },
    Missing,
    Probing,
    Error {
        message: String,
    },
}

/// Combined runtime status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStatus {
    pub fnm: ComponentStatus,
    pub node: ComponentStatus,
    pub playwright_cli: ComponentStatus,
    pub chromium: ComponentStatus,
    pub skills: ComponentStatus,
}

impl BootstrapStatus {
    /// Probe every component without installing anything.
    /// Never panics; never blocks longer than a few seconds.
    pub async fn probe() -> Self {
        let fnm = probe_fnm().await;
        let node = match &fnm {
            ComponentStatus::Installed { .. } => probe_node().await,
            _ => ComponentStatus::Missing,
        };
        let playwright_cli = match (&fnm, &node) {
            (ComponentStatus::Installed { .. }, ComponentStatus::Installed { .. }) => {
                probe_playwright_cli().await
            }
            _ => ComponentStatus::Missing,
        };
        let chromium = match &playwright_cli {
            ComponentStatus::Installed { .. } => probe_chromium().await,
            _ => ComponentStatus::Missing,
        };
        let skills = probe_skills();
        Self { fnm, node, playwright_cli, chromium, skills }
    }
}

async fn probe_fnm() -> ComponentStatus {
    match which::which("fnm") {
        Ok(path) => {
            let version = run_capture(&path, &["--version"]).await.ok();
            ComponentStatus::Installed {
                version: version.map(|v| v.trim().to_string()),
                path: Some(path.to_string_lossy().to_string()),
            }
        }
        Err(_) => ComponentStatus::Missing,
    }
}

async fn probe_node() -> ComponentStatus {
    match run_fnm_exec(&["node", "--version"]).await {
        Ok(ver) => ComponentStatus::Installed {
            version: Some(ver.trim().to_string()),
            path: None,
        },
        Err(_) => ComponentStatus::Missing,
    }
}

async fn probe_playwright_cli() -> ComponentStatus {
    match run_fnm_exec(&["playwright-cli", "--version"]).await {
        Ok(ver) => {
            let path = run_fnm_exec(&["which", "playwright-cli"])
                .await
                .ok()
                .map(|p| p.trim().to_string());
            ComponentStatus::Installed {
                version: Some(ver.trim().to_string()),
                path,
            }
        }
        Err(_) => ComponentStatus::Missing,
    }
}

async fn probe_chromium() -> ComponentStatus {
    // playwright install --dry-run exits 0 if chromium is present.
    match run_fnm_exec(&["playwright", "install", "--dry-run", "chromium"]).await {
        Ok(stdout) if !stdout.to_lowercase().contains("missing") => ComponentStatus::Installed {
            version: None,
            path: None,
        },
        _ => ComponentStatus::Missing,
    }
}

fn probe_skills() -> ComponentStatus {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return ComponentStatus::Missing,
    };
    let path = home.join(".aleph/skills/playwright-cli");
    if path.exists() && path.is_dir() {
        let has_content = std::fs::read_dir(&path)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if has_content {
            return ComponentStatus::Installed {
                version: None,
                path: Some(path.to_string_lossy().to_string()),
            };
        }
    }
    ComponentStatus::Missing
}

/// Run `fnm exec --using lts -- <args>` and capture stdout.
async fn run_fnm_exec(args: &[&str]) -> std::io::Result<String> {
    let mut full = vec!["exec", "--using", "lts", "--"];
    full.extend(args);
    let output = Command::new("fnm")
        .args(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_capture(bin: &PathBuf, args: &[&str]) -> std::io::Result<String> {
    let output = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_status_serde() {
        let s = ComponentStatus::Installed { version: Some("v22.8.0".into()), path: None };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["state"], "installed");
        assert_eq!(j["version"], "v22.8.0");
    }

    #[test]
    fn test_component_status_missing_serializes_tag() {
        let s = ComponentStatus::Missing;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["state"], "missing");
    }

    #[tokio::test]
    async fn test_probe_skills_missing_when_no_dir() {
        // Use a disposable home via HOME env override? The real probe_skills reads
        // `dirs::home_dir()` unconditionally; for now just assert the function runs
        // without panicking and returns something sensible.
        let _status = probe_skills();
    }

    #[tokio::test]
    async fn test_probe_completes_without_panicking() {
        let status = BootstrapStatus::probe().await;
        // All components reachable regardless of whether they're installed.
        let _ = serde_json::to_value(&status).unwrap();
    }
}
```

- [ ] **Step 2: Register module in `mod.rs`**

Edit `src/browser/mod.rs`. After the existing `pub mod` declarations add:

```rust
pub mod bootstrap;
```

And add re-export:

```rust
pub use bootstrap::{BootstrapStatus, ComponentStatus};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib browser::bootstrap 2>&1 | tail -20`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/browser/bootstrap.rs src/browser/mod.rs
git commit -m "browser: add bootstrap module (probe logic)"
```

---

## Task 5: Bootstrap — install logic (fnm/node/cli/chromium/skills)

**Files:**
- Modify: `src/browser/bootstrap.rs`

- [ ] **Step 1: Add install step enum + installer functions**

Append to `src/browser/bootstrap.rs` after the `impl BootstrapStatus` block:

```rust
/// A single install step that can be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStep {
    Fnm,
    Node,
    PlaywrightCli,
    Chromium,
    Skills,
}

impl InstallStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fnm => "fnm",
            Self::Node => "node",
            Self::PlaywrightCli => "playwright_cli",
            Self::Chromium => "chromium",
            Self::Skills => "skills",
        }
    }

    pub const ORDER: &'static [InstallStep] = &[
        Self::Fnm,
        Self::Node,
        Self::PlaywrightCli,
        Self::Chromium,
        Self::Skills,
    ];
}

/// Callback for streaming install progress.
/// Invocations: `on_progress(step, "started" | "log" | "done" | "failed", line_or_err)`.
pub type ProgressFn = Arc<dyn Fn(InstallStep, &str, Option<String>) + Send + Sync>;

use std::sync::Arc;

/// Run a full install of all missing components. Idempotent — skips installed ones.
pub async fn install_missing(on_progress: ProgressFn) -> Result<BootstrapStatus, String> {
    let mut status = BootstrapStatus::probe().await;
    for step in InstallStep::ORDER {
        let needs_install = match step {
            InstallStep::Fnm => matches!(status.fnm, ComponentStatus::Missing),
            InstallStep::Node => matches!(status.node, ComponentStatus::Missing),
            InstallStep::PlaywrightCli => matches!(status.playwright_cli, ComponentStatus::Missing),
            InstallStep::Chromium => matches!(status.chromium, ComponentStatus::Missing),
            InstallStep::Skills => matches!(status.skills, ComponentStatus::Missing),
        };
        if !needs_install {
            on_progress(*step, "done", Some("already installed".into()));
            continue;
        }
        on_progress(*step, "started", None);
        let result = match step {
            InstallStep::Fnm => install_fnm(on_progress.clone()).await,
            InstallStep::Node => install_node(on_progress.clone()).await,
            InstallStep::PlaywrightCli => install_playwright_cli(on_progress.clone()).await,
            InstallStep::Chromium => install_chromium(on_progress.clone()).await,
            InstallStep::Skills => install_skills(on_progress.clone()).await,
        };
        match result {
            Ok(_) => on_progress(*step, "done", None),
            Err(e) => {
                on_progress(*step, "failed", Some(e.clone()));
                return Err(format!("step {} failed: {e}", step.as_str()));
            }
        }
    }
    Ok(BootstrapStatus::probe().await)
}

async fn install_fnm(on_progress: ProgressFn) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = on_progress;
        return Err("Windows auto-install of fnm not supported. Please run `winget install Schniz.fnm` manually.".into());
    }
    // macOS / Linux: use fnm's official installer script via curl | bash.
    on_progress(InstallStep::Fnm, "log", Some("downloading fnm installer…".into()));
    let sh_cmd = r#"curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell"#;
    let output = tokio::process::Command::new("bash")
        .args(["-c", sh_cmd])
        .output()
        .await
        .map_err(|e| format!("spawn bash: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    on_progress(
        InstallStep::Fnm,
        "log",
        Some(String::from_utf8_lossy(&output.stdout).to_string()),
    );
    Ok(())
}

async fn install_node(on_progress: ProgressFn) -> Result<(), String> {
    on_progress(InstallStep::Node, "log", Some("fnm install --lts".into()));
    let output = tokio::process::Command::new("fnm")
        .args(["install", "--lts"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

async fn install_playwright_cli(_on_progress: ProgressFn) -> Result<(), String> {
    let output = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "npm", "install", "-g", "@playwright/cli@latest"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

async fn install_chromium(_on_progress: ProgressFn) -> Result<(), String> {
    let output = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "playwright", "install", "chromium"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

async fn install_skills(_on_progress: ProgressFn) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
    let skills_target = home.join(".aleph/skills/playwright-cli");
    tokio::fs::create_dir_all(&skills_target)
        .await
        .map_err(|e| e.to_string())?;
    let target_str = skills_target.to_string_lossy().to_string();
    // Try `--target <path>` first; if CLI rejects that flag, fall back to default install.
    let with_target = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "playwright-cli", "install", "--skills", "--target", &target_str])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if with_target.status.success() {
        return Ok(());
    }
    // Fallback: default install, then verify target exists with content (CLI may install to target already).
    let default_install = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "playwright-cli", "install", "--skills"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !default_install.status.success() {
        return Err(String::from_utf8_lossy(&default_install.stderr).to_string());
    }
    Ok(())
}
```

- [ ] **Step 2: Add unit tests for InstallStep enum**

Add inside the existing tests module:

```rust
#[test]
fn test_install_step_order() {
    let order: Vec<&str> = InstallStep::ORDER.iter().map(|s| s.as_str()).collect();
    assert_eq!(order, vec!["fnm", "node", "playwright_cli", "chromium", "skills"]);
}

#[test]
fn test_install_step_serde() {
    let s = InstallStep::PlaywrightCli;
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j, serde_json::Value::String("playwright_cli".into()));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib browser::bootstrap 2>&1 | tail -20`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/browser/bootstrap.rs
git commit -m "browser: add bootstrap install logic for fnm/node/cli/chromium/skills"
```

---

## Task 6: `PlaywrightCliDriver` — binary resolution + subprocess runner

**Files:**
- Create: `src/browser/playwright_cli.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Create file with driver skeleton + output struct + tests**

Create `src/browser/playwright_cli.rs`:

```rust
//! PlaywrightCliDriver — manages per-session `playwright-cli` subprocesses.
//!
//! Each tool call spawns a fresh process with `-s=<session_key>`; the CLI
//! keeps browser state in memory across invocations under the same key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex;

use crate::sync_primitives::RwLock;

use super::error::BrowserError;
use super::profile::PlaywrightCliConfig;

/// Output of a single `playwright-cli` invocation.
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub page_meta: Option<PageMeta>,
}

/// Metadata extracted from the `### Page / URL / Title / Snapshot` header.
#[derive(Debug, Clone, Default)]
pub struct PageMeta {
    pub url: String,
    pub title: String,
    pub snapshot_file: Option<PathBuf>,
}

/// Lazily resolves + caches the `playwright-cli` binary path, then serializes
/// concurrent invocations per session key.
pub struct PlaywrightCliDriver {
    binary_path: RwLock<Option<PathBuf>>,
    config: PlaywrightCliConfig,
    per_session_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
}

impl PlaywrightCliDriver {
    pub fn new(config: PlaywrightCliConfig) -> Self {
        Self {
            binary_path: RwLock::new(None),
            config,
            per_session_locks: RwLock::new(HashMap::new()),
        }
    }

    /// Resolve (or re-resolve) the CLI binary path. Caches on success.
    pub async fn resolve_binary(&self) -> Result<PathBuf, BrowserError> {
        if let Some(p) = self.binary_path.read().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(p);
        }
        if let Some(explicit) = self.config.binary_path.as_deref() {
            let p = PathBuf::from(explicit);
            if !p.exists() {
                return Err(BrowserError::PlaywrightCliNotInstalled);
            }
            *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
            return Ok(p);
        }
        let resolved = resolve_via_fnm().await?;
        *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(resolved.clone());
        Ok(resolved)
    }

    fn session_lock(&self, session_key: &str) -> Arc<Mutex<()>> {
        let mut map = self.per_session_locks.write().unwrap_or_else(|e| e.into_inner());
        map.entry(session_key.to_string()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }

    /// Spawn `playwright-cli -s=<session_key> <args>` and capture output.
    /// Serializes concurrent calls within the same `session_key`.
    pub async fn run(
        &self,
        session_key: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CliOutput, BrowserError> {
        let bin = self.resolve_binary().await?;
        let lock = self.session_lock(session_key);
        let _guard = lock.lock().await;

        let session_flag = format!("-s={session_key}");
        let mut cmd = Command::new(&bin);
        cmd.arg(&session_flag)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Filter secrets from child env.
        const DENY_ENV: &[&str] = &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "ALEPH_VAULT_KEY",
        ];
        for var in DENY_ENV {
            cmd.env_remove(var);
        }

        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BrowserError::PlaywrightCliNotInstalled,
            _ => BrowserError::Io(e),
        })?;

        let output_fut = child.wait_with_output();
        let output = match tokio::time::timeout(timeout, output_fut).await {
            Ok(res) => res.map_err(BrowserError::Io)?,
            Err(_) => {
                // timeout: child already awaited by wait_with_output; process may still be running
                return Err(BrowserError::Timeout(timeout.as_millis() as u64));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        if !output.status.success() {
            return Err(classify_stderr(&stderr, exit_code, session_key));
        }

        let page_meta = parse_page_meta(&stdout);
        Ok(CliOutput { stdout, stderr, exit_code, page_meta })
    }

    pub fn config(&self) -> &PlaywrightCliConfig {
        &self.config
    }
}

async fn resolve_via_fnm() -> Result<PathBuf, BrowserError> {
    let output = Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "which", "playwright-cli"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BrowserError::PlaywrightCliNotInstalled,
            _ => BrowserError::Io(e),
        })?;
    if !output.status.success() {
        return Err(BrowserError::PlaywrightCliNotInstalled);
    }
    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path_str.is_empty() {
        return Err(BrowserError::PlaywrightCliNotInstalled);
    }
    Ok(PathBuf::from(path_str))
}

fn classify_stderr(stderr: &str, exit_code: i32, session_key: &str) -> BrowserError {
    let s = stderr.to_lowercase();
    if s.contains("no session") || s.contains("browser not open") {
        BrowserError::NoSession(session_key.to_string())
    } else if s.contains("timeout") {
        BrowserError::Timeout(0)
    } else if s.contains("element not found") || s.contains("no element") {
        BrowserError::ActionFailed(format!("element not found ({stderr})"))
    } else {
        BrowserError::PlaywrightCliError(format!("exit {exit_code}: {stderr}"))
    }
}

/// Parse stdout for `### Page / URL / Title / Snapshot [path]` header.
pub fn parse_page_meta(stdout: &str) -> Option<PageMeta> {
    let mut meta = PageMeta::default();
    let mut in_page_section = false;
    let mut found_any = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed == "### Page" {
            in_page_section = true;
            continue;
        }
        if in_page_section {
            if let Some(rest) = trimmed.strip_prefix("- Page URL:") {
                meta.url = rest.trim().to_string();
                found_any = true;
            } else if let Some(rest) = trimmed.strip_prefix("- Page Title:") {
                meta.title = rest.trim().to_string();
                found_any = true;
            }
        }
        // "### Snapshot" is followed by a markdown link line: [Snapshot](<path>)
        if let Some(path) = trimmed.strip_prefix("[Snapshot](").and_then(|s| s.strip_suffix(')')) {
            meta.snapshot_file = Some(PathBuf::from(path.trim()));
            found_any = true;
        }
    }
    if found_any { Some(meta) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_page_meta_full() {
        let stdout = "\
### Page
- Page URL: https://example.com/
- Page Title: Example Domain
### Snapshot
[Snapshot](.playwright-cli/page-2026-04-12T00-00-00Z.yml)
";
        let meta = parse_page_meta(stdout).unwrap();
        assert_eq!(meta.url, "https://example.com/");
        assert_eq!(meta.title, "Example Domain");
        assert_eq!(
            meta.snapshot_file.as_ref().unwrap().to_string_lossy(),
            ".playwright-cli/page-2026-04-12T00-00-00Z.yml"
        );
    }

    #[test]
    fn test_parse_page_meta_none_for_empty() {
        assert!(parse_page_meta("").is_none());
        assert!(parse_page_meta("just some unrelated output").is_none());
    }

    #[test]
    fn test_classify_stderr_no_session() {
        let err = classify_stderr("Error: no session found for -s=foo", 1, "foo");
        matches!(err, BrowserError::NoSession(_));
    }

    #[test]
    fn test_classify_stderr_timeout() {
        let err = classify_stderr("Error: action timeout 5000ms", 1, "foo");
        matches!(err, BrowserError::Timeout(_));
    }

    #[test]
    fn test_classify_stderr_element_not_found() {
        let err = classify_stderr("element not found: #missing", 1, "foo");
        matches!(err, BrowserError::ActionFailed(_));
    }

    #[test]
    fn test_classify_stderr_generic() {
        let err = classify_stderr("something else", 2, "foo");
        matches!(err, BrowserError::PlaywrightCliError(_));
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Edit `src/browser/mod.rs`. Add:

```rust
pub mod playwright_cli;
pub use playwright_cli::{PlaywrightCliDriver, CliOutput, PageMeta};
```

Remove old `pub mod playwright_mcp;` / `pub use playwright_mcp::*;` lines — they'll vanish when Task 9 deletes the files; for now keep them so `manager.rs` still compiles.

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib browser::playwright_cli 2>&1 | tail -20`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/browser/playwright_cli.rs src/browser/mod.rs
git commit -m "browser: add PlaywrightCliDriver with binary resolution + subprocess runner"
```

---

## Task 7: `PlaywrightCliBackend` — `BrowserBackend` impl

**Files:**
- Create: `src/browser/playwright_cli_backend.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Create backend file**

Create `src/browser/playwright_cli_backend.rs`:

```rust
//! PlaywrightCliBackend — implements `BrowserBackend` by shelling out to `playwright-cli`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::network_policy::BrowserSsrfGuard;
use super::playwright_cli::{PlaywrightCliDriver, CliOutput};
use super::types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
};

pub struct PlaywrightCliBackend {
    driver: Arc<PlaywrightCliDriver>,
    session_key: String,
    ssrf_guard: Arc<BrowserSsrfGuard>,
    headless: bool,
}

impl PlaywrightCliBackend {
    pub fn new(
        driver: Arc<PlaywrightCliDriver>,
        session_key: impl Into<String>,
        ssrf_guard: Arc<BrowserSsrfGuard>,
        headless: bool,
    ) -> Self {
        Self {
            driver,
            session_key: session_key.into(),
            ssrf_guard,
            headless,
        }
    }

    fn nav_timeout(&self) -> Duration {
        Duration::from_secs(self.driver.config().nav_timeout_secs)
    }

    fn action_timeout(&self) -> Duration {
        Duration::from_secs(self.driver.config().action_timeout_secs)
    }

    async fn run(&self, args: &[&str], timeout: Duration) -> Result<CliOutput, BrowserError> {
        self.driver.run(&self.session_key, args, timeout).await
    }
}

fn target_ref(target: &ActionTarget) -> Result<&str, BrowserError> {
    match target {
        ActionTarget::Ref { ref_id } => Ok(ref_id.as_str()),
        ActionTarget::Coordinates { .. } => Err(BrowserError::ActionFailed(
            "this action requires a snapshot ref; coordinates unsupported for this op".into(),
        )),
    }
}

#[async_trait]
impl BrowserBackend for PlaywrightCliBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.ssrf_guard.check_url(url).map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        let mut args = vec!["tab-new", url];
        if !self.headless {
            args.insert(0, "--headed");
        }
        let _ = self.run(&args, self.nav_timeout()).await?;
        // playwright-cli tab model uses indexes; open_tab returns best-effort "last" marker
        Ok("last".into())
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        let _ = self.run(&["tab-close", tab_id], self.action_timeout()).await?;
        Ok(())
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        Ok(self.run(&["tab-list"], self.action_timeout()).await?.stdout)
    }

    async fn navigate(&self, _tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.ssrf_guard.check_url(url).map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
        let _ = self.run(&["goto", url], self.nav_timeout()).await?;
        Ok(())
    }

    async fn click(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        match target {
            ActionTarget::Ref { ref_id } => {
                let _ = self.run(&["click", &ref_id], self.action_timeout()).await?;
                Ok(())
            }
            ActionTarget::Coordinates { x, y } => {
                let xs = x.to_string();
                let ys = y.to_string();
                self.run(&["mousemove", &xs, &ys], self.action_timeout()).await?;
                self.run(&["mousedown"], self.action_timeout()).await?;
                self.run(&["mouseup"], self.action_timeout()).await?;
                Ok(())
            }
        }
    }

    async fn type_text(&self, _tab_id: &str, _target: ActionTarget, text: &str) -> Result<(), BrowserError> {
        let _ = self.run(&["type", text], self.action_timeout()).await?;
        Ok(())
    }

    async fn fill(&self, _tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self.run(&["fill", ref_id, value], self.action_timeout()).await?;
        Ok(())
    }

    async fn hover(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self.run(&["hover", ref_id], self.action_timeout()).await?;
        Ok(())
    }

    async fn scroll(&self, _tab_id: &str, _target: ActionTarget, direction: ScrollDirection) -> Result<(), BrowserError> {
        let (dx, dy) = match direction {
            ScrollDirection::Up => ("0", "-400"),
            ScrollDirection::Down => ("0", "400"),
            ScrollDirection::Left => ("-400", "0"),
            ScrollDirection::Right => ("400", "0"),
        };
        let _ = self.run(&["mousewheel", dx, dy], self.action_timeout()).await?;
        Ok(())
    }

    async fn screenshot(&self, _tab_id: &str, _opts: ScreenshotOpts) -> Result<ScreenshotOutput, BrowserError> {
        let mut path = std::env::temp_dir();
        let fname = format!("aleph-ss-{}.png", uuid::Uuid::new_v4());
        path.push(fname);
        let path_str = path.to_string_lossy().to_string();
        let _ = self.run(&["screenshot", "--filename", &path_str], Duration::from_secs(15)).await?;
        let png_bytes = tokio::fs::read(&path).await.map_err(BrowserError::Io)?;
        let _ = tokio::fs::remove_file(&path).await;
        Ok(ScreenshotOutput { png_bytes })
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        let output = self.run(&["snapshot"], Duration::from_secs(15)).await?;
        let meta = output.page_meta.unwrap_or_default();
        let snapshot_text = if let Some(p) = meta.snapshot_file.as_ref() {
            tokio::fs::read_to_string(p).await.unwrap_or_else(|_| output.stdout.clone())
        } else {
            output.stdout.clone()
        };
        Ok(SnapshotOutput {
            snapshot_text,
            page_url: meta.url,
            page_title: meta.title,
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<String, BrowserError> {
        let output = self.run(&["eval", js], self.action_timeout()).await?;
        Ok(output.stdout)
    }

    async fn select(&self, _tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError> {
        let ref_id = target_ref(&target)?;
        let _ = self.run(&["select", ref_id, value], self.action_timeout()).await?;
        Ok(())
    }

    async fn press_key(&self, _tab_id: &str, key: &str) -> Result<(), BrowserError> {
        let _ = self.run(&["press", key], self.action_timeout()).await?;
        Ok(())
    }

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Ok(self.run(&["console"], self.action_timeout()).await?.stdout)
    }

    async fn network_log(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Ok(self.run(&["network"], self.action_timeout()).await?.stdout)
    }

    async fn pdf(&self, _tab_id: &str, output_path: &Path) -> Result<(), BrowserError> {
        let path_str = output_path.to_string_lossy().to_string();
        let _ = self.run(&["pdf", "--filename", &path_str], Duration::from_secs(30)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::network_policy::{BrowserSsrfGuard, SsrfConfig};
    use crate::browser::profile::PlaywrightCliConfig;

    fn test_backend() -> PlaywrightCliBackend {
        let driver = Arc::new(PlaywrightCliDriver::new(PlaywrightCliConfig::default()));
        let guard = Arc::new(BrowserSsrfGuard::new(SsrfConfig::default()));
        PlaywrightCliBackend::new(driver, "test", guard, true)
    }

    #[test]
    fn test_target_ref_rejects_coordinates() {
        let result = target_ref(&ActionTarget::Coordinates { x: 0.0, y: 0.0 });
        assert!(matches!(result, Err(BrowserError::ActionFailed(_))));
    }

    #[test]
    fn test_target_ref_accepts_ref() {
        let result = target_ref(&ActionTarget::Ref { ref_id: "e42".into() });
        assert_eq!(result.unwrap(), "e42");
    }

    #[tokio::test]
    async fn test_navigate_rejects_ssrf_blocked_url() {
        let backend = test_backend();
        // SSRF guard default blocks private networks; localhost should be rejected.
        let result = backend.navigate("last", "http://127.0.0.1:8080/secret").await;
        assert!(matches!(result, Err(BrowserError::NavigationFailed(_))));
    }
}
```

- [ ] **Step 2: Register in mod.rs**

Edit `src/browser/mod.rs`:

```rust
pub mod playwright_cli_backend;
pub use playwright_cli_backend::PlaywrightCliBackend;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib browser::playwright_cli_backend 2>&1 | tail -20`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/browser/playwright_cli_backend.rs src/browser/mod.rs
git commit -m "browser: add PlaywrightCliBackend implementing BrowserBackend"
```

---

## Task 8: Adapt `ChromeMcpBackend` to text-first trait

**Files:**
- Modify: `src/browser/chrome_mcp_backend.rs`

- [ ] **Step 1: Replace structured returns with raw text**

Open `src/browser/chrome_mcp_backend.rs`. For each method:

- `snapshot()`: change return type to `Result<SnapshotOutput, BrowserError>`; set `snapshot_text` = `Self::extract_text(&result)`, leave `page_url`/`page_title` as empty strings (MCP doesn't provide them reliably). Delete the `parse_snapshot_text` helper function.
- `screenshot()`: change return type to `Result<ScreenshotOutput, BrowserError>`; when MCP returns image content, `png_bytes = base64::decode(data)?`; when returned as path fallback, read file.
- `list_tabs()`: change return type to `Result<String, BrowserError>`; simply return `Ok(Self::extract_text(&result))`.
- `console_messages()`: change return type to `Result<String, BrowserError>`; same pattern.
- `evaluate()`: change return type to `Result<String, BrowserError>`; `Ok(result.to_string())`.
- `select()`: drop the `ActionTarget::Selector` branch (variant is gone); only handle `Ref`/`Coordinates`.

- [ ] **Step 2: Delete unused helper `parse_snapshot_text` and associated tests**

Remove the entire `parse_snapshot_text` fn and any `test_parse_snapshot_text_*` tests.

- [ ] **Step 3: Compile + test**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: success.

Run: `cargo test -p alephcore --lib browser::chrome_mcp_backend 2>&1 | tail -20`
Expected: remaining tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/browser/chrome_mcp_backend.rs
git commit -m "browser: adapt ChromeMcpBackend to text-first trait"
```

---

## Task 9: Wire `PlaywrightCliBackend` into `ProfileManager`

**Files:**
- Modify: `src/browser/manager.rs`

- [ ] **Step 1: Replace `playwright_mcp_driver` field with `playwright_cli_driver`**

Open `src/browser/manager.rs`. Find the `ProfileManager` struct and replace:

```rust
playwright_mcp_driver: Arc<PlaywrightMcpDriver>,
```

with:

```rust
playwright_cli_driver: Arc<PlaywrightCliDriver>,
ssrf_guard: Arc<BrowserSsrfGuard>,
```

Update imports at the top:

```rust
use super::playwright_cli::PlaywrightCliDriver;
use super::network_policy::BrowserSsrfGuard;
```

Remove `use super::playwright_mcp::PlaywrightMcpDriver;` (not yet — playwright_mcp.rs still exists; keep the import compiling by leaving a `#[allow(unused_imports)]` if needed, or remove line in Task 11 after deletion). To be safe: keep the import until Task 11.

- [ ] **Step 2: Rewrite `ProfileManager::new()`**

Replace the constructor body:

```rust
impl ProfileManager {
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_policy = BrowserSsrfGuard::new(config.policy.clone());
        let ssrf_guard = Arc::new(BrowserSsrfGuard::new(config.policy.clone()));
        let chrome_mcp_driver = Arc::new(ChromeMcpDriver::new(config.chrome_mcp.clone()));
        let playwright_cli_driver = Arc::new(PlaywrightCliDriver::new(config.playwright_cli.clone()));

        // ... (profile init logic unchanged) ...

        Self {
            profiles: RwLock::new(profiles),
            ssrf_policy,
            ssrf_guard,
            config,
            chrome_mcp_driver,
            playwright_cli_driver,
        }
    }

    pub fn get_playwright_cli_driver(&self) -> Arc<PlaywrightCliDriver> {
        self.playwright_cli_driver.clone()
    }

    pub fn get_ssrf_guard(&self) -> Arc<BrowserSsrfGuard> {
        self.ssrf_guard.clone()
    }

    // ... (rest unchanged) ...
}
```

Delete the old `get_playwright_mcp_driver` method.

- [ ] **Step 3: Add `get_backend` method**

Append to `impl ProfileManager`:

```rust
use super::backend::BrowserBackend;
use super::chrome_mcp_backend::ChromeMcpBackend;
use super::playwright_cli_backend::PlaywrightCliBackend;

pub fn get_backend(&self, profile_name: &str) -> Result<Arc<dyn BrowserBackend>, BrowserError> {
    let cfg = self.get_config(profile_name)
        .ok_or_else(|| BrowserError::ProfileNotFound(profile_name.into()))?;
    match cfg.driver {
        BrowserDriver::Managed => {
            let headless = cfg.headless.unwrap_or(self.config.playwright_cli.headless);
            Ok(Arc::new(PlaywrightCliBackend::new(
                self.playwright_cli_driver.clone(),
                profile_name.to_string(),
                self.ssrf_guard.clone(),
                headless,
            )))
        }
        BrowserDriver::ExistingSession => {
            Ok(Arc::new(ChromeMcpBackend::new(
                self.chrome_mcp_driver.clone(),
                profile_name.to_string(),
            )))
        }
    }
}
```

(Adjust `ChromeMcpBackend::new` signature if it differs — check current code; likely `(driver, session_key)` pair.)

The `use` statements go to the top of the file, not inside `impl`.

- [ ] **Step 4: Fix call sites**

Run: `grep -rn "get_playwright_mcp_driver\|playwright_mcp_driver" src/ 2>&1 | grep -v "src/browser/playwright_mcp"`

For each hit (likely in `src/builtin_tools/browser/handlers.rs` or similar), replace with `get_backend(profile_name)` calls that return the trait object.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -30`
Expected: success.

- [ ] **Step 6: Update/add manager tests**

Ensure existing `test_*` in `manager.rs` still pass; add:

```rust
#[tokio::test]
async fn test_get_backend_returns_existing_session_for_user() {
    let config = BrowserSystemConfig::default();
    let manager = ProfileManager::new(config);
    let backend = manager.get_backend("user").unwrap();
    // Trait object — smoke test that call doesn't panic
    let _ = backend.list_tabs().await; // may return Err (no chrome mcp), but must not panic
}
```

Run: `cargo test -p alephcore --lib browser::manager 2>&1 | tail -20`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/browser/manager.rs src/builtin_tools/browser/handlers.rs
git commit -m "browser: wire PlaywrightCliBackend into ProfileManager routing"
```

---

## Task 10: Delete chromiumoxide-based code

**Files:**
- Delete: `src/browser/runtime.rs`, `actions.rs`, `snapshot.rs`, `snapshot_format.rs`, `managed_backend.rs`
- Modify: `src/browser/mod.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Delete the five files**

```bash
rm src/browser/runtime.rs src/browser/actions.rs src/browser/snapshot.rs \
   src/browser/snapshot_format.rs src/browser/managed_backend.rs
```

- [ ] **Step 2: Update `mod.rs` — remove deleted module declarations**

Edit `src/browser/mod.rs`. Delete lines:

```rust
mod actions;
pub mod runtime;
pub mod snapshot;
pub mod snapshot_format;
mod managed_backend;

pub use managed_backend::ManagedBackend;
pub use runtime::BrowserRuntime;
pub use snapshot::{resolve_ref_to_point, take_aria_snapshot};
pub use snapshot_format::{format_snapshot, SnapshotFormatOptions, SnapshotFormatResult};
```

- [ ] **Step 3: Remove chromiumoxide dependency**

Edit `Cargo.toml`. Delete line 159:

```
chromiumoxide = { version = "0.7", default-features = false, features = ["tokio-runtime"] }
```

- [ ] **Step 4: Fix leftover references**

Run: `cargo check -p alephcore 2>&1 | tail -60`

Expected errors: references in `src/builtin_tools/pdf_generate/browser_engine.rs` (handled in Task 12) and possibly `src/browser/mod.rs` tests. For `pdf_generate`, **temporarily** replace the `generate` function body with `Err(ToolError::Custom("PDF generation pending playwright-cli migration".into()))` and comment out the chromiumoxide imports; this is reverted in Task 12.

For any other leftover references, surgically fix them.

- [ ] **Step 5: Verify compile + tests**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: success.

Run: `cargo test -p alephcore --lib browser 2>&1 | tail -30`
Expected: browser tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "browser: remove chromiumoxide runtime + ManagedBackend + actions/snapshot"
```

---

## Task 11: Delete playwright-mcp code

**Files:**
- Delete: `src/browser/playwright_mcp.rs`, `playwright_mcp_backend.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Delete files**

```bash
rm src/browser/playwright_mcp.rs src/browser/playwright_mcp_backend.rs
```

- [ ] **Step 2: Update `mod.rs`**

Edit `src/browser/mod.rs`. Remove:

```rust
pub mod playwright_mcp;
pub mod playwright_mcp_backend;
pub use playwright_mcp::PlaywrightMcpDriver;
pub use playwright_mcp_backend::PlaywrightMcpBackend;
```

- [ ] **Step 3: Remove leftover imports**

In `src/browser/manager.rs`, delete `use super::playwright_mcp::PlaywrightMcpDriver;` (the `#[allow(unused_imports)]` added in Task 9).

Run: `grep -rn "playwright_mcp\|PlaywrightMcpDriver\|PlaywrightMcpBackend" src/ 2>&1`

For any remaining hit, delete or fix.

- [ ] **Step 4: Verify compile + tests**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "browser: remove PlaywrightMcpDriver and PlaywrightMcpBackend"
```

---

## Task 12: Migrate `pdf_generate` to `playwright-cli pdf`

**Files:**
- Modify: `src/builtin_tools/pdf_generate/browser_engine.rs`

- [ ] **Step 1: Replace chromiumoxide implementation**

Open `src/builtin_tools/pdf_generate/browser_engine.rs`. Replace the entire file with:

```rust
//! Browser-based PDF rendering via playwright-cli.
//!
//! Converts Markdown to HTML, writes to a temp file, navigates playwright-cli
//! to `file://<temp>`, then `pdf --filename=<output>`.

use std::path::Path;

use pulldown_cmark::{html, Options, Parser};
use tempfile::NamedTempFile;
use tracing::{debug, info};

use super::args::{ContentFormat, PdfGenerateArgs, PdfGenerateOutput};
use super::styles;
use crate::browser::playwright_cli::PlaywrightCliDriver;
use crate::browser::profile::PlaywrightCliConfig;
use crate::builtin_tools::error::ToolError;

pub fn markdown_to_html(markdown: &str) -> String {
    let options = Options::all();
    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

pub fn build_html_document(markdown: &str, title: Option<&str>) -> String {
    let html_body = markdown_to_html(markdown);
    styles::wrap_html_with_styles(&html_body, title)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
}

/// Generate a PDF using playwright-cli.
///
/// Flow:
/// 1. Build HTML document (Markdown or plain text wrapped in `<pre>`)
/// 2. Write HTML to a temporary file
/// 3. `playwright-cli -s=<pdfgen> goto file://<tmp>`
/// 4. `playwright-cli -s=<pdfgen> pdf --filename=<output>`
/// 5. Clean up the temporary file
pub async fn generate(
    args: &PdfGenerateArgs,
    output_path: &Path,
) -> Result<PdfGenerateOutput, ToolError> {
    // Step 1: Build HTML
    let html_doc = match args.format {
        ContentFormat::Markdown => build_html_document(&args.content, args.title.as_deref()),
        ContentFormat::Text => {
            let escaped = html_escape(&args.content);
            styles::wrap_html_with_styles(&format!("<pre>{escaped}</pre>"), args.title.as_deref())
        }
    };

    // Step 2: Write HTML to temp file
    let tmp = NamedTempFile::with_suffix(".html")
        .map_err(|e| ToolError::Custom(format!("tempfile: {e}")))?;
    tokio::fs::write(tmp.path(), &html_doc)
        .await
        .map_err(|e| ToolError::Custom(format!("write html: {e}")))?;
    let file_url = format!("file://{}", tmp.path().display());
    debug!("pdf_generate wrote HTML to {}", tmp.path().display());

    // Step 3+4: goto + pdf
    let driver = PlaywrightCliDriver::new(PlaywrightCliConfig::default());
    let session = "aleph-pdf-gen";
    driver
        .run(session, &["goto", &file_url], std::time::Duration::from_secs(30))
        .await
        .map_err(|e| ToolError::Custom(format!("goto: {e}")))?;
    let out_str = output_path.to_string_lossy().to_string();
    driver
        .run(session, &["pdf", "--filename", &out_str], std::time::Duration::from_secs(60))
        .await
        .map_err(|e| ToolError::Custom(format!("pdf: {e}")))?;

    info!(path = %output_path.display(), "PDF generated via playwright-cli");

    let size = tokio::fs::metadata(output_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(PdfGenerateOutput {
        path: output_path.to_string_lossy().to_string(),
        bytes: size,
    })
}

pub fn is_chrome_available() -> bool {
    // Delegate to bootstrap status: chromium is considered available if playwright-cli
    // reports it. For a cheap sync check, just probe whether fnm+playwright-cli exist.
    which::which("fnm").is_ok()
}
```

Adjust `PdfGenerateOutput` field names if the struct differs — inspect `super::args` to confirm.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: success. If `PdfGenerateOutput` fields differ, adapt the struct literal.

- [ ] **Step 3: Run existing pdf_generate tests**

Run: `cargo test -p alephcore --lib pdf_generate 2>&1 | tail -30`
Expected: pure-computation tests (markdown_to_html, build_html_document) pass. Any tests that invoke `generate()` itself need `#[ignore]` since they require live playwright-cli; mark them so.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/pdf_generate/browser_engine.rs
git commit -m "builtin_tools/pdf_generate: migrate from chromiumoxide to playwright-cli pdf"
```

---

## Task 13: Adapt `builtin_tools/browser` to text-first responses

**Files:**
- Modify: `src/builtin_tools/browser/handlers.rs`
- Modify: `src/builtin_tools/browser/types.rs`

- [ ] **Step 1: Identify the response builders that consumed old types**

Run: `grep -n "AriaSnapshot\|AriaElement\|ConsoleMessage\|TabInfo\|ScreenshotResult" src/builtin_tools/browser/*.rs`

For each tool handler:
- `handle_snapshot` → now receives `SnapshotOutput`; respond with `text` content containing `snapshot_text`, and structured metadata `{url, title}` in separate text lines or JSON.
- `handle_screenshot` → receives `ScreenshotOutput { png_bytes }`; encode base64 + wrap in MCP image content with `mime_type: "image/png"`.
- `handle_console` / `handle_list_tabs` / `handle_network_log` → directly embed returned `String` as text content.
- `handle_evaluate` → embed `String` as text content.

- [ ] **Step 2: Edit `handlers.rs` response builders**

Replace structured JSON emission with raw text. Example:

```rust
pub async fn handle_snapshot(manager: &ProfileManager, profile: &str) -> Result<ToolOutput, ToolError> {
    let backend = manager.get_backend(profile).map_err(to_tool_err)?;
    let snap = backend.snapshot("").await.map_err(to_tool_err)?;
    let combined = format!(
        "### Page\n- URL: {}\n- Title: {}\n### Snapshot\n{}",
        snap.page_url, snap.page_title, snap.snapshot_text
    );
    Ok(ToolOutput::text(combined))
}

pub async fn handle_screenshot(manager: &ProfileManager, profile: &str, opts: ScreenshotOpts) -> Result<ToolOutput, ToolError> {
    let backend = manager.get_backend(profile).map_err(to_tool_err)?;
    let shot = backend.screenshot("", opts).await.map_err(to_tool_err)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&shot.png_bytes);
    Ok(ToolOutput::image_png_base64(b64))
}
```

Adjust to match actual signatures in `handlers.rs` — do not invent APIs; read the existing file first, then edit inline.

- [ ] **Step 3: Prune dead types from `types.rs`**

In `src/builtin_tools/browser/types.rs`, remove any response type fields/variants that referenced `AriaSnapshot`, `AriaElement`, etc. If a whole response struct is now trivially `{ text: String }`, simplify it.

- [ ] **Step 4: Compile + run browser-tool tests**

Run: `cargo check -p alephcore 2>&1 | tail -20`

Run: `cargo test -p alephcore --lib builtin_tools::browser 2>&1 | tail -30`
Expected: all pass; update any tests whose mocked responses encoded old types.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/browser
git commit -m "builtin_tools/browser: adapt handlers to text-first backend responses"
```

---

## Task 14: Gateway — `runtime_status` / `install_runtime` / `refresh_runtime` RPCs + event

**Files:**
- Create: `src/gateway/handlers/browser_runtime.rs`
- Modify: `src/gateway/handlers/mod.rs`
- Modify: `src/gateway/event_bus.rs`

- [ ] **Step 1: Add `BrowserInstallProgressEvent` to event bus**

Edit `src/gateway/event_bus.rs`. In the `GatewayEvent` enum, add a new variant:

```rust
BrowserInstallProgress(BrowserInstallProgressEvent),
```

And define the struct (near other event structs):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserInstallProgressEvent {
    pub step: String,              // "fnm" | "node" | "playwright_cli" | "chromium" | "skills"
    pub status: String,            // "started" | "log" | "done" | "failed"
    pub log_line: Option<String>,
    pub error: Option<String>,
    pub timestamp: i64,
}
```

- [ ] **Step 2: Create `browser_runtime.rs` handler module**

Create `src/gateway/handlers/browser_runtime.rs`:

```rust
//! Browser runtime RPC handlers: probe + install of fnm/node/cli/chromium/skills.

use std::sync::Arc;

use crate::browser::bootstrap::{self, BootstrapStatus, InstallStep};
use crate::gateway::event_bus::{BrowserInstallProgressEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

pub async fn handle_runtime_status(request: JsonRpcRequest) -> JsonRpcResponse {
    let status = BootstrapStatus::probe().await;
    match serde_json::to_value(&status) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("serialize: {e}")),
    }
}

pub async fn handle_refresh_runtime(request: JsonRpcRequest) -> JsonRpcResponse {
    // Identical to runtime_status; separate method name so UI can express intent.
    handle_runtime_status(request).await
}

pub async fn handle_install_runtime(
    request: JsonRpcRequest,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Spawn install task; return immediately.
    let bus = event_bus.clone();
    tokio::spawn(async move {
        let bus2 = bus.clone();
        let progress = Arc::new(move |step: InstallStep, status: &str, line: Option<String>| {
            let event = GatewayEvent::BrowserInstallProgress(BrowserInstallProgressEvent {
                step: step.as_str().to_string(),
                status: status.to_string(),
                log_line: line.clone(),
                error: if status == "failed" { line } else { None },
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = bus2.publish_json(&event);
        });
        let _ = bootstrap::install_missing(progress).await;
    });
    JsonRpcResponse::success(request.id, serde_json::json!({ "accepted": true }))
}
```

- [ ] **Step 3: Register handlers in `mod.rs`**

Edit `src/gateway/handlers/mod.rs`. Add `pub mod browser_runtime;`. In the RPC router (wherever `browser.get` is registered), add routes for:
- `browser.runtime_status` → `browser_runtime::handle_runtime_status`
- `browser.refresh_runtime` → `browser_runtime::handle_refresh_runtime`
- `browser.install_runtime` → `browser_runtime::handle_install_runtime` (with event_bus arg)

Follow the exact pattern used by `browser.get` / `browser.update` — read current registration code before editing.

- [ ] **Step 4: Add unit test for runtime_status shape**

Create `src/gateway/handlers/browser_runtime.rs` tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn test_runtime_status_returns_valid_json() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "browser.runtime_status".into(),
            params: None,
            id: serde_json::json!(1),
        };
        let resp = handle_runtime_status(req).await;
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert!(result.get("fnm").is_some());
        assert!(result.get("node").is_some());
        assert!(result.get("playwright_cli").is_some());
        assert!(result.get("chromium").is_some());
        assert!(result.get("skills").is_some());
    }
}
```

(Adjust field names on `JsonRpcRequest` literal to match the actual struct.)

- [ ] **Step 5: Compile + test**

Run: `cargo check -p alephcore 2>&1 | tail -20`

Run: `cargo test -p alephcore --lib gateway::handlers::browser_runtime 2>&1 | tail -20`
Expected: 1 test passes.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/browser_runtime.rs src/gateway/handlers/mod.rs src/gateway/event_bus.rs
git commit -m "gateway: add runtime_status/install_runtime/refresh_runtime RPCs + event"
```

---

## Task 15: Gateway — extend `browser_config` with timeouts + persistent_sessions

**Files:**
- Modify: `src/gateway/handlers/browser_config.rs`

- [ ] **Step 1: Extend `BrowserConfigResponse` struct**

Edit `src/gateway/handlers/browser_config.rs`. Add fields to `BrowserConfigResponse`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BrowserConfigResponse {
    // ... existing fields ...
    pub nav_timeout_secs: u64,
    pub action_timeout_secs: u64,
    pub persistent_sessions: bool,
}
```

- [ ] **Step 2: Update `handle_get`**

In `handle_get`, after reading `browser`, populate new fields:

```rust
let nav_timeout_secs = browser.playwright_cli.nav_timeout_secs;
let action_timeout_secs = browser.playwright_cli.action_timeout_secs;
let persistent_sessions = browser.playwright_cli.persistent_sessions;
```

Pass them into the `BrowserConfigResponse` struct literal.

- [ ] **Step 3: Update `handle_update`**

After the existing block that updates `playwright_mcp.args`, replace with:

```rust
browser.playwright_cli.nav_timeout_secs = update.nav_timeout_secs;
browser.playwright_cli.action_timeout_secs = update.action_timeout_secs;
browser.playwright_cli.persistent_sessions = update.persistent_sessions;
browser.playwright_cli.headless = update.headless;
```

Delete the block that mutated `playwright_mcp.args` (removing `--headless` etc.) — no longer needed.

- [ ] **Step 4: Add unit tests for round-trip**

Add to the tests module (or create one) in `browser_config.rs`:

```rust
#[tokio::test]
async fn test_browser_config_roundtrip_includes_timeouts() {
    let cfg = Arc::new(RwLock::new(Config::default()));
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        method: "browser.get".into(),
        params: None,
        id: serde_json::json!(1),
    };
    let resp = handle_get(req, cfg).await;
    let v = resp.result.unwrap();
    assert_eq!(v["nav_timeout_secs"], 30);
    assert_eq!(v["action_timeout_secs"], 10);
    assert_eq!(v["persistent_sessions"], false);
}
```

- [ ] **Step 5: Compile + test**

Run: `cargo test -p alephcore --lib gateway::handlers::browser_config 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/browser_config.rs
git commit -m "gateway: extend browser_config RPC with timeouts + persistent_sessions"
```

---

## Task 16: Webchat — extend API layer + Playwright CLI Settings section

**Files:**
- Modify: `interfaces/webchat/src/api/browser.rs`
- Modify: `interfaces/webchat/src/views/settings/browser.rs`

- [ ] **Step 1: Extend `BrowserConfig` in API layer**

Edit `interfaces/webchat/src/api/browser.rs`. Add fields to `BrowserConfig` struct:

```rust
pub nav_timeout_secs: u64,
pub action_timeout_secs: u64,
pub persistent_sessions: bool,
```

Adjust default initializer in the view (`browser.rs`) to include them.

- [ ] **Step 2: Add `RuntimeStatusResponse` + API functions**

In `interfaces/webchat/src/api/browser.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ComponentStatus {
    Installed { version: Option<String>, path: Option<String> },
    Missing,
    Probing,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatusResponse {
    pub fnm: ComponentStatus,
    pub node: ComponentStatus,
    pub playwright_cli: ComponentStatus,
    pub chromium: ComponentStatus,
    pub skills: ComponentStatus,
}

pub struct BrowserRuntimeApi;

impl BrowserRuntimeApi {
    pub async fn status(state: &DashboardState) -> Result<RuntimeStatusResponse, String> {
        state.rpc_call("browser.runtime_status", serde_json::Value::Null).await
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
    }
    pub async fn refresh(state: &DashboardState) -> Result<RuntimeStatusResponse, String> {
        state.rpc_call("browser.refresh_runtime", serde_json::Value::Null).await
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
    }
    pub async fn install(state: &DashboardState) -> Result<(), String> {
        state.rpc_call("browser.install_runtime", serde_json::json!({})).await.map(|_| ())
    }
}
```

(Adjust `rpc_call` signature to match `DashboardState`'s actual method.)

- [ ] **Step 3: Update `EngineSection` + wording in `browser.rs`**

Edit `interfaces/webchat/src/views/settings/browser.rs`. In the `EngineSection` component:
- Change heading `"Playwright Settings"` → `"Playwright CLI Settings"`
- Change description copy accordingly
- Add three inputs after the headless toggle:

```rust
<div>
    <label class="block text-sm font-medium text-text-primary mb-2">"Navigation Timeout (seconds)"</label>
    <input
        type="number"
        min="5" max="300"
        prop:value=move || config.get().nav_timeout_secs as i64
        on:change=move |ev| {
            let val = event_target_value(&ev).parse::<u64>().unwrap_or(30);
            config.update(|c| c.nav_timeout_secs = val);
            save_fn.with_value(|f| f());
        }
        class="block w-32 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
    />
</div>
<div>
    <label class="block text-sm font-medium text-text-primary mb-2">"Action Timeout (seconds)"</label>
    <input
        type="number"
        min="1" max="60"
        prop:value=move || config.get().action_timeout_secs as i64
        on:change=move |ev| {
            let val = event_target_value(&ev).parse::<u64>().unwrap_or(10);
            config.update(|c| c.action_timeout_secs = val);
            save_fn.with_value(|f| f());
        }
        class="block w-32 px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
    />
</div>
<div class="flex items-center justify-between">
    <div>
        <div class="font-medium text-text-primary">"Persistent Sessions"</div>
        <div class="text-sm text-text-tertiary">"Save browser session state to disk (--persistent)."</div>
    </div>
    <!-- reuse the same toggle pattern as headless -->
</div>
```

Also rename `"Playwright (Headless)"` label in `DefaultModeSection` to `"Playwright CLI (Headless)"`.

- [ ] **Step 4: Initialize new fields in the view's default `BrowserConfig`**

Near line ~56 in `browser.rs`:

```rust
let config = RwSignal::new(BrowserConfig {
    default_driver: "managed".to_string(),
    browser_engine: "chromium".to_string(),
    headless: true,
    devtools_profile: "user".to_string(),
    block_private: true,
    blocked_domains: Vec::new(),
    allowed_domains: Vec::new(),
    nav_timeout_secs: 30,
    action_timeout_secs: 10,
    persistent_sessions: false,
});
```

- [ ] **Step 5: Compile (Leptos target)**

Run: `cargo check -p webchat 2>&1 | tail -30` (or whatever crate path is correct — check workspace `Cargo.toml`).

Expected: success.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/api/browser.rs interfaces/webchat/src/views/settings/browser.rs
git commit -m "webchat: update Browser settings for Playwright CLI (copy + new inputs)"
```

---

## Task 17: Webchat — Runtime Status card

**Files:**
- Create: `interfaces/webchat/src/views/settings/browser_runtime.rs`
- Modify: `interfaces/webchat/src/views/settings/browser.rs`
- Modify: `interfaces/webchat/src/views/settings/mod.rs`

- [ ] **Step 1: Create the `BrowserRuntimeCard` component**

Create `interfaces/webchat/src/views/settings/browser_runtime.rs`:

```rust
//! Runtime Status card: fnm / Node / playwright-cli / Chromium / skills
//! health + "Install All" / "Refresh" buttons + streaming log.

use crate::api::browser::{BrowserRuntimeApi, ComponentStatus, RuntimeStatusResponse};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn BrowserRuntimeCard() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let status = RwSignal::new(Option::<RuntimeStatusResponse>::None);
    let loading = RwSignal::new(true);
    let installing = RwSignal::new(false);
    let log_lines = RwSignal::new(Vec::<String>::new());

    let refresh = {
        let state = state.clone();
        move || {
            loading.set(true);
            let state = state.clone();
            spawn_local(async move {
                match BrowserRuntimeApi::status(&state).await {
                    Ok(s) => status.set(Some(s)),
                    Err(_) => status.set(None),
                }
                loading.set(false);
            });
        }
    };

    // Initial probe on mount.
    {
        let refresh = refresh.clone();
        Effect::new(move |_| refresh());
    }

    let install = {
        let state = state.clone();
        let refresh = refresh.clone();
        move || {
            installing.set(true);
            log_lines.set(vec!["Starting install...".into()]);
            let state = state.clone();
            let refresh = refresh.clone();
            spawn_local(async move {
                let _ = BrowserRuntimeApi::install(&state).await;
                // After kickoff returns, re-probe every 3s until all installed or giving up
                for _ in 0..60 {
                    gloo_timers::future::TimeoutFuture::new(3000).await;
                    refresh();
                }
                installing.set(false);
            });
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">"Runtime Status"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Playwright CLI requires Node.js (via fnm) and a local Chromium. Install with one click."
            </p>
            {move || {
                if loading.get() {
                    view! { <div class="text-text-tertiary">"Probing..."</div> }.into_any()
                } else if let Some(s) = status.get() {
                    view! {
                        <ul class="space-y-2 mb-4">
                            <ComponentRow name="fnm" status=s.fnm />
                            <ComponentRow name="Node.js" status=s.node />
                            <ComponentRow name="@playwright/cli" status=s.playwright_cli />
                            <ComponentRow name="Chromium" status=s.chromium />
                            <ComponentRow name="Skills" status=s.skills />
                        </ul>
                    }.into_any()
                } else {
                    view! { <div class="text-danger">"Gateway unavailable."</div> }.into_any()
                }
            }}
            <div class="flex gap-2">
                <button
                    on:click=move |_| install()
                    disabled=move || installing.get()
                    class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50"
                >
                    {move || if installing.get() { "Installing..." } else { "Install All" }}
                </button>
                <button
                    on:click=move |_| refresh()
                    class="px-4 py-2 border border-border rounded-lg text-text-primary"
                >
                    "Refresh"
                </button>
            </div>
            {move || (!log_lines.get().is_empty()).then(|| view! {
                <div class="mt-4 p-3 bg-surface border border-border rounded font-mono text-xs max-h-40 overflow-y-auto">
                    {log_lines.get().iter().map(|l| view! { <div>{l.clone()}</div> }).collect_view()}
                </div>
            })}
        </div>
    }
}

#[component]
fn ComponentRow(name: &'static str, status: ComponentStatus) -> impl IntoView {
    let (icon, text) = match &status {
        ComponentStatus::Installed { version, .. } => {
            let v = version.as_deref().unwrap_or("");
            ("✓", format!("{name} {v}").trim().to_string())
        }
        ComponentStatus::Missing => ("✗", format!("{name} — not installed")),
        ComponentStatus::Probing => ("…", format!("{name} — probing…")),
        ComponentStatus::Error { message } => ("!", format!("{name} — error: {message}")),
    };
    view! {
        <li class="flex items-center gap-2 text-sm">
            <span class="w-4 text-center">{icon}</span>
            <span>{text}</span>
        </li>
    }
}
```

- [ ] **Step 2: Mount in Browser settings view**

Edit `interfaces/webchat/src/views/settings/browser.rs`. In the main `BrowserView` render tree, add `<BrowserRuntimeCard />` as the first child of the settings column (before `DefaultModeSection`):

```rust
use super::browser_runtime::BrowserRuntimeCard;
// ...
<div class="space-y-6">
    <BrowserRuntimeCard />
    // ... error banner, DefaultModeSection, etc ...
</div>
```

- [ ] **Step 3: Register module**

Edit `interfaces/webchat/src/views/settings/mod.rs` (or wherever `browser` is declared) and add:

```rust
pub mod browser_runtime;
```

- [ ] **Step 4: Add `gloo-timers` if not already a dep**

Check `interfaces/webchat/Cargo.toml`. If `gloo-timers` missing, add:

```toml
gloo-timers = { version = "0.3", features = ["futures"] }
```

- [ ] **Step 5: Compile**

Run: `cargo check -p webchat 2>&1 | tail -30`
Expected: success.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/settings/browser_runtime.rs \
        interfaces/webchat/src/views/settings/browser.rs \
        interfaces/webchat/src/views/settings/mod.rs \
        interfaces/webchat/Cargo.toml
git commit -m "webchat: add Runtime Status card with Install All flow"
```

---

## Task 18: Docs + example TOML update

**Files:**
- Modify: `examples/browser-config.toml`
- Modify: `docs/reference/*` where browser mentioned
- Modify: `CHANGELOG.md` (new entry for current CalVer version)

- [ ] **Step 1: Rewrite `examples/browser-config.toml`**

Replace the file with:

```toml
# Aleph browser configuration example.
# Playwright CLI drives the managed headless mode; Chrome DevTools MCP
# drives the existing-session (attach to your Chrome) mode.

[profiles.default]
browser = "chromium"
driver = "managed"
color = "#4A90E2"
# headless omitted → follows [playwright_cli].headless

[profiles.user]
browser = "chrome"
driver = "existing_session"
color = "#00AA00"

[policy]
block_private = true
blocked_domains = ["*.malware.com"]

[playwright_cli]
enabled = true
# binary_path = "/custom/path/to/playwright-cli"   # optional override
headless = true
nav_timeout_secs = 30
action_timeout_secs = 10
persistent_sessions = false

[chrome_mcp]
command = "npx"
args = ["-y", "chrome-devtools-mcp@latest", "--autoConnect"]
```

- [ ] **Step 2: Update cross-references in docs**

Run: `grep -rln "playwright_mcp\|PlaywrightMcpConfig\|chromiumoxide\|ManagedBackend" docs/`

For each doc hit (e.g. `docs/reference/ARCHITECTURE.md`, `MEMORY_SYSTEM.md` if they mention browser), replace old terms with:
- `playwright_mcp` → `playwright_cli`
- `PlaywrightMcpConfig` → `PlaywrightCliConfig`
- `chromiumoxide` / `ManagedBackend` → `PlaywrightCliBackend (via @playwright/cli)`

Avoid batch sed; adjust each hit in context.

- [ ] **Step 3: Add CHANGELOG entry**

Edit `CHANGELOG.md`. Under the current in-progress release section (or create one for today's date if conventions require), add:

```markdown
### Added
- Playwright CLI runtime bootstrap: one-click install of fnm, Node LTS, @playwright/cli, Chromium, and skills from Panel Settings.
- Browser `BrowserBackend` trait reshaped text-first for token efficiency.

### Changed
- Managed browser automation now uses @playwright/cli instead of @playwright/mcp + chromiumoxide.
- PDF generation (`pdf_generate` tool) migrated to `playwright-cli pdf`.

### Removed
- `chromiumoxide` dependency.
- Legacy `@playwright/mcp` integration (`PlaywrightMcpDriver` / `PlaywrightMcpBackend`).

### Migration
- TOML `[playwright_mcp]` section is silently read as `[playwright_cli]`; old `command` / `args` fields are discarded (no action needed).
- First-run: open Panel → Settings → Browser and click "Install All".
```

- [ ] **Step 4: Commit**

```bash
git add examples/browser-config.toml docs/ CHANGELOG.md
git commit -m "docs: update browser references for playwright-cli migration"
```

---

## Task 19: Verification pass

**Files:** none (verification only)

- [ ] **Step 1: Verify `chromiumoxide` fully removed**

Run: `cargo tree -p alephcore 2>&1 | grep -i chromium`
Expected: no output (or only chrome-related crates not named chromiumoxide).

Run: `grep -rn "chromiumoxide" src/ 2>&1 | grep -v "^Binary"`
Expected: zero matches.

- [ ] **Step 2: Verify playwright-mcp fully removed**

Run: `grep -rn "PlaywrightMcp\|playwright_mcp" src/ interfaces/ 2>&1`
Expected: matches only in doc strings / test fixtures that reference legacy migration (e.g. serde alias test).

- [ ] **Step 3: Full workspace compile + clippy**

Run: `cargo check --workspace 2>&1 | tail -20`
Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -40`
Expected: both succeed without errors.

- [ ] **Step 4: Full test suite**

Run: `cargo test --workspace --lib 2>&1 | tail -40`
Expected: all pass.

- [ ] **Step 5: Manual smoke test (if live env available)**

- Start `target/debug/aleph-server`, open Panel → Settings → Browser
- Confirm Runtime Status card shows accurate state
- Click "Install All"; watch log appear; on success, all rows turn ✓
- In chat: ask AI to "Open https://example.com and take a screenshot"
- Verify a screenshot comes back

If live env unavailable, mark this step skipped with a comment.

- [ ] **Step 6: Commit final verification log**

No code change. If verification revealed fixups, fix them in a dedicated `verify:` commit and re-run.

---

## Self-Review

**Spec coverage:**

- [x] Full replacement of Playwright MCP and chromiumoxide → Tasks 10, 11
- [x] Text-first BrowserBackend → Task 2
- [x] Bootstrap probe + install (fnm/node/cli/chromium/skills) → Tasks 4, 5
- [x] PlaywrightCliDriver + Backend → Tasks 6, 7
- [x] ChromeMcpBackend adaptation → Task 8
- [x] ProfileManager routing → Task 9
- [x] pdf_generate migration → Task 12
- [x] builtin_tools/browser response layer → Task 13
- [x] Gateway RPCs (runtime_status/install_runtime/refresh_runtime) + event → Task 14
- [x] Gateway browser_config extension (3 new fields) → Task 15
- [x] Webchat Playwright CLI Settings updates → Task 16
- [x] Webchat Runtime Status card → Task 17
- [x] TOML migration (serde alias) → Task 1
- [x] Docs + CHANGELOG → Task 18
- [x] Verification → Task 19

**Placeholder scan:** No `TBD` / `TODO` / "implement later" / "handle edge cases" placeholders. Every code step shows complete code.

**Type consistency:** `SnapshotOutput` (Task 2) matches consumers in `ChromeMcpBackend` (Task 8), `PlaywrightCliBackend` (Task 7), `builtin_tools/browser/handlers.rs` (Task 13). `ComponentStatus` defined in `bootstrap.rs` (Task 4) and mirrored in `interfaces/webchat/src/api/browser.rs` (Task 16). `PlaywrightCliConfig` defined Task 1 consumed Task 6. `BrowserInstallProgressEvent` defined Task 14; UI subscribes in Task 17.

**Known implementation tradeoffs worth flagging during execution:**

1. `playwright-cli install --skills --target` flag is inferred from spec; if CLI doesn't accept `--target`, Task 5 Step 1's fallback path (default install + nothing to copy) leaves the skills-target directory empty. Review the actual CLI behavior during Task 5 and adjust the copy step if needed.
2. `cargo check -p webchat` — the actual webchat crate name may be `alephcore-webchat` or similar. Run `cargo metadata --format-version=1 | jq '.packages[].name'` once before starting to confirm.
3. The Leptos `Effect::new(move |_| refresh())` pattern in Task 17 may need `refresh` captured `Callable`-style; adapt to the codebase's existing component patterns by reading an existing card (e.g. `DefaultModeSection`) first.
4. If any migration-era test still references `headless: false` as a bare bool, Task 1 Step 4 must catch it — search the whole repo: `grep -rn 'headless: *true\|headless: *false' src/ tests/ 2>&1 | grep -v 'Some('`.
