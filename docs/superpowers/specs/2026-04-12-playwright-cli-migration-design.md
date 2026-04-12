# Playwright MCP → Playwright CLI Migration Design

**Date:** 2026-04-12
**Status:** Design approved, pending implementation plan

## Problem

Aleph's managed browser backend currently spawns `@playwright/mcp` via `npx` on first use. This incurs:

- Per-invocation `npx` cache-check latency on cold start
- MCP protocol overhead (verbose tool schemas + ARIA JSON serialization)
- A second parallel path via `chromiumoxide` (`ManagedBackend` + `BrowserRuntime`) that duplicates managed-mode responsibility with a different programming model
- Token cost: structured `AriaSnapshot` responses are less token-efficient than the YAML snapshots Playwright CLI emits

Microsoft's new `@playwright/cli` (https://github.com/microsoft/playwright-cli) is explicitly designed for coding agents: session-aware one-shot CLI commands where state persists in memory across invocations via `-s=<name>`. It exposes the full set of operations Aleph needs (`open/goto/click/fill/type/hover/select/snapshot/screenshot/eval/console/network/tab-*/cookie-*`) with token-efficient text output.

## Solution

Replace both the Playwright MCP path and the chromiumoxide-based `ManagedBackend` with a new `PlaywrightCliBackend` that shells out to `@playwright/cli`. Bootstrap the runtime (fnm → Node.js LTS → playwright-cli → Chromium → skills) via user-initiated install triggered from Panel Settings. Reshape the `BrowserBackend` trait to return text-first responses that map naturally to CLI output.

Chrome DevTools MCP (`existing-session` driver) stays untouched as a parallel backend for attaching to the user's running Chrome.

## Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Bootstrap scope | **Full replacement** — drop both playwright-mcp and chromiumoxide | R3 Core Minimalism; one managed path, not three |
| Node runtime | **fnm, native paths** (`~/.local/share/fnm/` etc.) | Aleph delegates Node lifecycle to fnm; no bespoke runtime dir |
| Install trigger | **Detect on startup + manual "Install All" button in Settings** | R5/R6 lightweight entry; no startup stall; no silent system changes |
| Install scope | One-click: fnm + Node LTS + `@playwright/cli` + Chromium + skills | All-or-nothing; YAGNI for per-component install |
| Skills location | `~/.aleph/skills/playwright-cli/` | Unified with Aleph skill dir; clean uninstall |
| Trait shape | **Text-first** — `SnapshotOutput { snapshot_text, url, title }`, `String`, `Bytes` | Aligns with CLI's token-efficient output; zero double-serialization |
| `ActionTarget` | Drop `Selector` variant; keep `Ref` and `Coordinates` | Playwright CLI has no CSS selector input; enforce snapshot→ref workflow |
| Session keying | Aleph profile name → `-s=<profile>` one-to-one | Simplest mapping; reuses existing profile lifecycle |
| TOML migration | `serde(alias = "playwright_mcp")` + unknown-field drop | Silent upgrade; no migration script |
| Chrome DevTools MCP | Kept; adapted to new trait (returns raw text) | Orthogonal driver; scope boundary |
| `playwright-cli show` dashboard | Out-of-scope this round; reserved button slot | YAGNI |
| Windows fnm auto-install | Out-of-scope; show "please install fnm" hint | macOS/Linux primary; Windows iterates later |

## Architecture

### Before

```
BrowserBackend (structured AriaSnapshot)
├── ManagedBackend → BrowserRuntime (chromiumoxide / CDP, in-process)
├── PlaywrightMcpBackend → PlaywrightMcpDriver → @playwright/mcp (npx stdio MCP)
└── ChromeMcpBackend → ChromeMcpDriver → chrome-devtools-mcp (npx stdio MCP)
```

### After

```
BrowserBackend (text-first: SnapshotOutput / ScreenshotOutput / String / Bytes)
├── PlaywrightCliBackend → PlaywrightCliDriver → @playwright/cli (managed via fnm)
└── ChromeMcpBackend → ChromeMcpDriver → chrome-devtools-mcp (unchanged path)
```

### Deleted code (~1200 lines)

`runtime.rs`, `actions.rs`, `snapshot.rs`, `snapshot_format.rs`, `managed_backend.rs`, `playwright_mcp.rs`, `playwright_mcp_backend.rs`; `AriaSnapshot`, `AriaElement`, `ElementRect`, `ConsoleMessage`, `TabInfo` types; `chromiumoxide` + `chromiumoxide_types` dependencies.

### New code (~700 lines)

`bootstrap.rs`, `playwright_cli.rs`, `playwright_cli_backend.rs`, `gateway/handlers/browser_runtime.rs`, `webchat/views/settings/browser_runtime.rs`.

## Bootstrap Flow

### Startup probe (seconds; non-blocking)

`BootstrapStatus::probe()` runs on Aleph startup:

1. Detect `fnm` binary (`which fnm` / `where fnm`)
2. If fnm present: `fnm exec --using lts which playwright-cli` → binary path
3. Record `BootstrapState { fnm, node, playwright_cli, chromium, skills }` where each is `Installed { version?, path? } | Missing | Probing | Error { message }`
4. Publish `BrowserInstallProgressEvent` (status-only variant) to gateway event bus for UI refresh
5. **Never auto-install** — user must click "Install All" in Settings

### Install All (Panel Settings → Browser → Runtime Status card)

Sequential steps, each emitting streamed progress events:

1. **fnm missing** → download from `github.com/Schniz/fnm/releases/latest/download/fnm-{os}-{arch}.zip` → extract to `~/.local/bin/fnm` (macOS/Linux) or `%LOCALAPPDATA%\fnm\fnm.exe` (Windows — manual hint only v1). **No shell rc modification**; Aleph remembers path.
2. **Node missing** → `fnm install --lts` (lands in fnm's default `~/.local/share/fnm/node-versions/`)
3. **playwright-cli missing** → `fnm exec --using lts -- npm install -g @playwright/cli@latest`
4. **Chromium missing** → `fnm exec --using lts -- playwright install chromium`
5. **Skills missing** → `fnm exec --using lts -- playwright-cli install --skills --target ~/.aleph/skills` (fallback: default install + copy to `~/.aleph/skills/playwright-cli/`)

Each step publishes `BrowserInstallProgressEvent { step, status, log_line, error, timestamp }` to UI. Failures preserve mid-state; "Retry" resumes from failed step.

### Runtime binary path resolution

`PlaywrightCliDriver` caches the absolute path (via `fnm exec --using lts which playwright-cli`) on first use. Subsequent invocations use the cached path, skipping the ~50–100ms `fnm exec` overhead. On `ENOENT` or exec failure, the driver invalidates and re-resolves.

## `BrowserBackend` Trait (Text-First)

```rust
pub struct SnapshotOutput {
    pub snapshot_text: String,   // raw YAML (playwright-cli) or raw text (chrome-devtools-mcp)
    pub page_url: String,
    pub page_title: String,
}

pub struct ScreenshotOutput {
    pub png_bytes: Vec<u8>,
}

#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn snapshot(&self, tab_id: &str) -> Result<SnapshotOutput, BrowserError>;
    async fn screenshot(&self, tab_id: &str, opts: ScreenshotOpts) -> Result<ScreenshotOutput, BrowserError>;
    async fn console_messages(&self, tab_id: &str) -> Result<String, BrowserError>;
    async fn list_tabs(&self) -> Result<String, BrowserError>;
    async fn network_log(&self, tab_id: &str) -> Result<String, BrowserError>;
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError>;

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError>;
    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn fill(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
    async fn type_text(&self, tab_id: &str, target: ActionTarget, text: &str) -> Result<(), BrowserError>;
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn scroll(&self, tab_id: &str, target: ActionTarget, direction: ScrollDirection) -> Result<(), BrowserError>;
    async fn select(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
    async fn press_key(&self, tab_id: &str, key: &str) -> Result<(), BrowserError>;
    async fn wait_for_text(&self, tab_id: &str, text: &str, timeout_ms: u64) -> Result<bool, BrowserError>;
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;

    /// Print-to-PDF via CLI. Writes PDF bytes to `output_path`.
    /// Only `PlaywrightCliBackend` implements; `ChromeMcpBackend` returns `Unsupported`.
    async fn pdf(&self, tab_id: &str, output_path: &std::path::Path) -> Result<(), BrowserError>;
}

pub enum ActionTarget {
    Ref { ref_id: String },
    Coordinates { x: f64, y: f64 },
    // Selector variant removed
}
```

**Rationale**: Playwright CLI's raw YAML snapshot is already LLM-readable; parsing into `AriaElement` then re-serializing as JSON wastes tokens and adds fragile YAML dependencies. Chrome DevTools MCP's indented tree text is similarly LLM-readable. Downstream (`builtin_tools/browser/handlers.rs`) transparently forwards `snapshot_text` to tool responses.

## `PlaywrightCliDriver`

```rust
pub struct PlaywrightCliDriver {
    binary_path: RwLock<Option<PathBuf>>,
    config: PlaywrightCliConfig,
    per_session_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,  // serialize concurrent calls per session
}

struct CliOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    page_meta: Option<PageMeta>,   // extracted from "### Page / URL / Title / Snapshot [path]"
}

struct PageMeta {
    url: String,
    title: String,
    snapshot_file: Option<PathBuf>,
}
```

**`run()` behavior**:

- Spawn `<bin> -s=<session_key> <args>` with stdout/stderr piped
- Timeout: nav/wait = `nav_timeout_secs` (default 30s), actions = `action_timeout_secs` (default 10s), snapshot/screenshot = 15s
- On timeout: SIGTERM → 1s grace → SIGKILL
- **Environment filter**: strip `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, Aleph-internal secrets from child env

**Error classification** (by stderr substring):
- `"no session"` / `"browser not open"` → `NoSession`
- `"timeout"` → `Timeout`
- `"element not found"` → `ActionFailed`
- binary missing (`ENOENT`) → `PlaywrightCliNotInstalled`
- tokio timeout fired → `Timeout` (after killing child)

**Concurrency**: same `session_key` invocations serialize through a per-session `Mutex<()>`; different sessions run in parallel.

## `PlaywrightCliBackend`

- `navigate` → SSRF check via `NetworkPolicy::check_url` → `goto <url>`
- `click(Ref{ref_id})` → `click <ref_id>`
- `click(Coordinates{x,y})` → `mousemove x y && mousedown && mouseup` (three-call fallback)
- `snapshot` → `snapshot` → read YAML file referenced in `### Snapshot [path]` line
- `screenshot` → `screenshot --filename=<tmp>` → read PNG bytes → delete tmp (best-effort)
- `pdf` → `pdf --filename=<output_path>` → return once file written (no post-read)
- `console_messages` / `list_tabs` / `network_log` / `evaluate` → capture stdout, return as `String`

## Config Migration

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaywrightCliConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub binary_path: Option<String>,
    #[serde(default = "default_true")]
    pub headless: bool,
    #[serde(default = "default_nav_timeout")]
    pub nav_timeout_secs: u64,       // 30
    #[serde(default = "default_action_timeout")]
    pub action_timeout_secs: u64,    // 10
    #[serde(default)]
    pub persistent_sessions: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSystemConfig {
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub policy: SsrfConfig,
    #[serde(default, alias = "playwright_mcp")]
    pub playwright_cli: PlaywrightCliConfig,
    #[serde(default)]
    pub chrome_mcp: ChromeMcpConfig,
}
```

**`ProfileConfig.headless` changes from `bool` to `Option<bool>`** (Some = override, None = follow global). Backward compatible: `headless = true` deserializes to `Some(true)`.

**`cdp_port` field retained** (used by chrome_mcp paths) but marked deprecated in doc comment.

**Migration path**: serde alias silently reads `[playwright_mcp]` as `playwright_cli`. Unknown fields (`command`, `args` from old MCP config) are dropped. Next `save_incremental("general.browser")` writes back under the new key, naturally clearing the old section. No explicit migration script.

### TOML migration map

| Old (≤2026.04.11) | New | Handling |
|---|---|---|
| `[playwright_mcp]` | `[playwright_cli]` | alias; silent upgrade |
| `playwright_mcp.enabled` | `playwright_cli.enabled` | preserved |
| `playwright_mcp.command` | — | dropped (fnm manages) |
| `playwright_mcp.args` | — | dropped (CLI manages) |
| `profiles.*.headless = bool` | `profiles.*.headless = bool/null` | compatible |

## Panel Settings → Browser Page

```
┌─ Runtime Status ───────────────────────┐  NEW
│  ✓ fnm 1.35.0                          │
│  ✓ Node.js v22.8.0 (LTS, via fnm)      │
│  ✗ @playwright/cli — not installed     │
│  ✗ Chromium — not installed            │
│  ✓ Skills — ~/.aleph/skills/           │
│  [ Install All ]  [ Refresh ]          │
│  ┌── streaming log (during install) ──┐│
│  └─────────────────────────────────────┘
├─ Default Browser Mode ─────────────────┤  radio: Playwright CLI / Chrome DevTools
├─ Playwright CLI Settings ──────────────┤  dropdown + headless + 3 new inputs:
│                                        │    nav_timeout_secs, action_timeout_secs,
│                                        │    persistent_sessions
├─ Chrome DevTools Settings ─────────────┤  unchanged
└─ Security ─────────────────────────────┘  unchanged
```

### New gateway RPCs

| Method | Params | Returns | Purpose |
|---|---|---|---|
| `browser.runtime_status` | — | `RuntimeStatusResponse` | query component statuses |
| `browser.install_runtime` | `{ components?: Vec<String> }` | `{ accepted: true }` | async install; progress via event stream |
| `browser.refresh_runtime` | — | `RuntimeStatusResponse` | force re-probe |

### New event

```rust
pub struct BrowserInstallProgressEvent {
    pub step: String,        // "fnm" | "node" | "playwright_cli" | "chromium" | "skills"
    pub status: String,      // "started" | "done" | "failed"
    pub log_line: Option<String>,
    pub error: Option<String>,
    pub timestamp: i64,
}
```

### `BrowserConfigResponse` extension

Three new fields added: `nav_timeout_secs`, `action_timeout_secs`, `persistent_sessions`. Existing `get`/`update` RPCs keep their semantics.

### Copy updates

- "Playwright (Headless)" → "Playwright CLI (Headless)"
- "Playwright Settings" → "Playwright CLI Settings"
- "Fast, invisible managed browser..." → "Headless automation via @playwright/cli. Fast, invisible..."

## File Plan

### Created (5 files, ~700 LoC)

| File | Purpose | ~LoC |
|---|---|---|
| `src/browser/bootstrap.rs` | fnm/node/cli/chromium/skills probe + install | 280 |
| `src/browser/playwright_cli.rs` | `PlaywrightCliDriver` | 220 |
| `src/browser/playwright_cli_backend.rs` | `PlaywrightCliBackend` | 180 |
| `src/gateway/handlers/browser_runtime.rs` | 3 RPCs + event publishing | 120 |
| `interfaces/webchat/src/views/settings/browser_runtime.rs` | Runtime Status Leptos component | 180 |

### Modified (~16 files)

`src/browser/mod.rs`, `backend.rs`, `types.rs`, `profile.rs`, `error.rs`, `chrome_mcp_backend.rs`, `manager.rs`; `src/gateway/handlers/browser_config.rs`, `mod.rs`, `event_bus.rs`; `src/builtin_tools/browser/handlers.rs`, `types.rs`; `src/builtin_tools/pdf_generate/browser_engine.rs` (drop chromiumoxide, use `PlaywrightCliDriver::pdf()`); `interfaces/webchat/src/api/browser.rs`, `views/settings/browser.rs`; `Cargo.toml`; `examples/browser-config.toml`; relevant docs.

### Deleted (8 files, ~1200 LoC)

`src/browser/runtime.rs`, `actions.rs`, `snapshot.rs`, `snapshot_format.rs`, `managed_backend.rs`, `playwright_mcp.rs`, `playwright_mcp_backend.rs`; chromiumoxide-coupled cases in `tests/browser_integration.rs`.

### Dependency changes

- **Remove**: `chromiumoxide`, `chromiumoxide_types`
- **Add**: (none new — reuse existing `rand`/`uuid` for temp file names; skip `serde_yaml` by using text-line parsing)
- **Conditional**: `chrome_mcp_snapshot.rs` kept pending review; delete if no callers remain after implementation

## Commit Sequence (single PR, ~15 atomic commits)

1. `browser: add PlaywrightCliConfig + serde alias for playwright_mcp`
2. `browser: redesign BrowserBackend trait to text-first`
3. `browser: add bootstrap module (fnm/node/cli/chromium detection)`
4. `browser: add PlaywrightCliDriver`
5. `browser: add PlaywrightCliBackend`
6. `browser: adapt ChromeMcpBackend to text-first trait`
7. `browser: wire PlaywrightCliBackend into ProfileManager routing`
8. `browser: remove chromiumoxide runtime + ManagedBackend + actions/snapshot`
9. `browser: remove PlaywrightMcpDriver + PlaywrightMcpBackend`
10. `builtin_tools/browser: adapt to text-first responses`
11. `builtin_tools/pdf_generate: migrate browser_engine from chromiumoxide to playwright-cli pdf`
12. `gateway: add runtime_status/install_runtime/refresh_runtime RPCs + event`
13. `gateway: extend browser_config RPC with timeouts + persistent_sessions`
14. `webchat: Playwright CLI Settings section updates`
15. `webchat: Runtime Status card + install flow`
16. `docs: update browser references; examples/browser-config.toml`

Each commit keeps the tree compilable.

## Testing Strategy

| Layer | Method | Coverage |
|---|---|---|
| Config deserialization | unit tests in `profile.rs` | `PlaywrightCliConfig` defaults; `[playwright_mcp]` alias read; `headless: Option<bool>` compat; old section cleared on next `save_incremental` |
| Bootstrap probe | unit tests + mock binaries | fnm presence; Node version parsing; `playwright-cli --version`; path caching |
| `PlaywrightCliDriver::run` | unit tests + mock command runner | stderr keyword classification; timeout kill; per-session serialization |
| `PlaywrightCliBackend` methods | unit tests with mock driver stdout | SSRF on navigate; coordinates click fallback; snapshot file read; screenshot tmp cleanup |
| CLI output parsing | unit tests against fixed samples | `PageMeta` extraction; `### Snapshot [path]` capture; degraded output tolerance |
| `ChromeMcpBackend` adaptation | existing tests in `chrome_mcp_backend.rs` | text-first returns; old `parse_snapshot_text` usages removed |
| RPC handlers | unit tests with mock Config + EventBus | `runtime_status` shape; `install_runtime` event sequence |
| End-to-end | `#[ignore]` integration test | real fnm + playwright-cli: open → goto → click → snapshot → screenshot → close |

CI runs `--ignored` in a separate matrix job with fnm + playwright-cli preinstalled.

## Security Considerations

- **fnm download**: HTTPS only from `github.com/Schniz/fnm/releases/`; no checksum verification v1 (future hardening)
- **`npm install -g`**: user-scoped (fnm Node prefix); no sudo
- **Child process env filter**: strip secrets (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) before spawning playwright-cli
- **SSRF**: `NetworkPolicy::check_url` intercepts in Rust before `navigate`. Known limitation: JS-triggered `window.location = ...` navigation inside CLI bypass Aleph policy (preexisting issue in MCP path as well)
- **Temp files**: best-effort cleanup via `tokio::fs::remove_file`; OS reaps on crash
- **Chrome DevTools MCP remote debugging port**: preexisting consideration, unchanged

## Out of Scope (YAGNI)

- `playwright-cli show` dashboard UI integration (reserved Settings button only)
- Video / Trace recording (`tracing-start`, `video-start`)
- Network route mocking (`route` command)
- Multi-Node-version juggling (LTS only)
- Windows fnm auto-install (macOS/Linux only v1; Windows shows manual hint)
- Uninstall runtime button (manual `fnm uninstall lts` + directory cleanup)
- Skill auto-update (manual `npm update -g @playwright/cli`)
- Offline / enterprise npm registry (users can set `PlaywrightCliConfig.binary_path`)
- SSRF bypass via JS `window.location` (preexisting limitation)
- Coordinates-targeted element bounds (playwright-cli doesn't expose; use `eval` if needed)

## Success Criteria

1. `cargo check -p alephcore` passes; `cargo clippy -- -D warnings` passes
2. `cargo test --lib` passes
3. On macOS cold start: Aleph → Settings → Install All → success → AI asked to "open https://example.com and screenshot" returns a screenshot
4. Old TOML containing `[playwright_mcp]` loads without error under new schema
5. `chromiumoxide` no longer appears in `cargo tree`
6. `~/.aleph/skills/playwright-cli/` directory exists and is non-empty after install
