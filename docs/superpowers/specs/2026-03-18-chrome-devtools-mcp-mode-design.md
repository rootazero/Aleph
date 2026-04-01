# Chrome DevTools MCP Mode Design

## Problem

Aleph's browser system launches dedicated Chromium instances with isolated profiles. Users must re-login to every website each session because the managed browser has no access to their existing Chrome cookies, extensions, or saved passwords. This creates significant friction for tasks that require authenticated web access.

## Solution

Add an **existing-session** driver mode that attaches to the user's running Chrome browser via [Chrome DevTools MCP](https://developer.chrome.com/blog/chrome-devtools-mcp), Google's official MCP server for Chrome. This preserves the user's login state, cookies, and extensions while reusing Aleph's existing `browser_*` tool interface.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Relationship with managed mode | **Coexist** — profile `driver` field selects mode | Different use cases: managed for automation/safety, existing-session for authenticated access |
| Tool interface | **Unified** — same `browser_*` tools, capability-driven routing | LLM doesn't need to know which driver is active (R8 LLM Sovereignty) |
| User onboarding | **Fully automatic** — Aleph launches Chrome with debugging if needed | Minimal friction; falls back to clear error if Chrome is already running without debugging |
| Browser support | **Chrome only** | Chrome DevTools MCP is Google's official tool; simplest scope |
| Profile management | **Zero-config "user" profile + dialogue-created extras** | Auto-created "user" profile for instant use; R9 tool-based management for advanced config |
| MCP installation | **Default npx, configurable** | `npx -y chrome-devtools-mcp@latest` for zero-setup; custom command for offline/enterprise |

## Architecture

### Dependency Graph

```
browser_tools/*.rs (tool layer)
    |
    v
ProfileManager::get_backend(profile_name)
    |
    v returns Arc<dyn BrowserBackend>
    |
    +---------------------------+----------------------------+
    |                           |                            |
    v                           v                            |
ManagedBackend              ChromeMcpBackend                 |
    |                           |                            |
    v                           v                            |
BrowserRuntime              ChromeMcpDriver                  |
    |                           |                            |
    v                           v                            |
chromiumoxide (CDP)         McpClient (stdio transport)      |
    |                           |                            |
    v                           v                            |
Aleph-managed browser       chrome-devtools-mcp process      |
                                |                            |
                                v                            |
                            User's Chrome (with login state) |
```

### BrowserBackend Trait

Unified contract for both driver modes:

```rust
#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;
    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError>;
    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError>;
    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn type_text(&self, tab_id: &str, target: ActionTarget, text: &str) -> Result<(), BrowserError>;
    async fn fill(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn scroll(&self, tab_id: &str, target: ActionTarget, direction: ScrollDirection) -> Result<(), BrowserError>;
    async fn screenshot(&self, tab_id: &str, opts: ScreenshotOpts) -> Result<ScreenshotResult, BrowserError>;
    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError>;
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError>;
    async fn select(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
}
```

Two implementations:
- `ManagedBackend` — wraps existing `BrowserRuntime` (chromiumoxide)
- `ChromeMcpBackend` — wraps `ChromeMcpDriver` with parameter/result conversion

## Profile Configuration

### New Types

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDriver {
    #[default]
    Managed,
    ExistingSession,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChromeMcpConfig {
    #[serde(default = "default_chrome_mcp_command")]
    pub command: String,  // default: "npx"
    #[serde(default = "default_chrome_mcp_args")]
    pub args: Vec<String>,  // default: ["-y", "chrome-devtools-mcp@latest", "--autoConnect", "--experimentalStructuredContent"]
}
```

### ProfileConfig Changes

Add to existing `ProfileConfig`:

```rust
pub struct ProfileConfig {
    // ... existing fields unchanged ...

    #[serde(default)]
    pub driver: BrowserDriver,
}
```

Note: The `attach_only` field from the openclaw reference is not needed. The `ExistingSession` driver always attempts auto-launch if Chrome is not running (per the auto-launch flow below). If a future need arises for "attach but never launch" semantics, the field can be added then (YAGNI).

### Auto-created "user" Profile

`ProfileManager::new()` auto-injects:

```rust
profiles.insert("user".into(), ManagedProfile {
    config: ProfileConfig {
        browser: BrowserType::Chrome,
        driver: BrowserDriver::ExistingSession,
        color: Some("#00AA00".into()),
        ..Default::default()
    },
    state: ProfileState::Idle,
    last_activity: std::time::Instant::now(),
});
```

### TOML Example

```toml
[profiles.default]
browser = "chromium"
driver = "managed"
cdp_port = 18800

[profiles.user]
browser = "chrome"
driver = "existing_session"
color = "#00AA00"

[chrome_mcp]
command = "npx"
args = ["-y", "chrome-devtools-mcp@latest", "--autoConnect", "--experimentalStructuredContent"]
```

## ChromeMcpDriver — Session Management

### Session Lifecycle

```rust
struct ChromeMcpSession {
    client: McpClient,
    pid: u32,
    profile_name: String,
}

pub struct ChromeMcpDriver {
    sessions: RwLock<HashMap<String, ChromeMcpSession>>,
    config: ChromeMcpConfig,
}
```

**Why not reuse `McpClient::start_external_server` directly?** The existing MCP manager treats servers as long-lived singletons keyed by name. Chrome MCP sessions need profile-keyed caching (multiple profiles = multiple sessions), lazy creation on first tool call, and auto-rebuild after transport failure — semantics the general MCP manager does not provide. `ChromeMcpDriver` uses the low-level `McpClient` transport API but manages the session lifecycle itself.

**Storage in `ProfileManager`**: `ChromeMcpDriver` is stored as `Arc<ChromeMcpDriver>` on `ProfileManager` (not inside the `RwLock<HashMap<String, ManagedProfile>>`), since `ChromeMcpDriver` manages its own internal async-safe locking. This avoids holding a `std::sync::RwLock` guard across async boundaries.

Key behaviors:
- **Lazy creation** — sessions created on first tool call, not at startup
- **Cache + dedup** — same profile reuses same session; concurrent requests don't duplicate
- **Error-level routing** — tool errors (element not found) keep session alive; transport errors (process crash) destroy session, next call auto-rebuilds
- **PID monitoring** — dead MCP processes detected and cleaned up before session access

### Chrome Auto-Launch Flow

```
get_or_create_session(profile_name)
    |
    v
spawn chrome-devtools-mcp (--autoConnect)
    |
    v connection fails? (Chrome not running)
ensure_chrome_running()
    |
    +-- Chrome not running at all:
    |     Launch: chrome --remote-debugging-port=0 --no-first-run --no-default-browser-check
    |     Wait 2s for readiness, retry MCP spawn
    |
    +-- Chrome running WITHOUT debugging:
    |     Return error: "Chrome is running but remote debugging is not enabled.
    |     Please restart Chrome or enable debugging at chrome://inspect/#remote-debugging"
    |
    +-- Chrome running WITH debugging:
          MCP connects via --autoConnect, session ready
```

Key details:
- `--remote-debugging-port=0` lets Chrome pick a free port; `--autoConnect` discovers it
- No `--user-data-dir` specified — uses user's default Chrome profile (preserves login state)
- Auto-launched Chrome is NOT managed by Aleph lifecycle — user closes Chrome normally
- Session auto-cleans when chrome-devtools-mcp process exits

## Snapshot Conversion

Chrome DevTools MCP returns tree-structured accessibility snapshots. Conversion to Aleph's `AriaSnapshot`:

- **ref_id strategy**: Use Chrome MCP's native UIDs directly (e.g., `"btn-1"`) as `ref_id` — no remapping needed. Click/type operations pass these IDs straight through to Chrome MCP.
- **Tree preservation**: Convert Chrome MCP tree nodes into Aleph's `AriaElement` with `children` populated, preserving hierarchical context for the LLM. The top-level `elements` vec contains root nodes only.
- **Bounds**: Not provided by Chrome MCP — `bounds` field is `None` for existing-session snapshots

### Capability Limitations

existing-session mode via Chrome DevTools MCP has some restrictions vs. managed mode:

| Capability | Managed | Existing-Session |
|------------|---------|-----------------|
| ref_id targeting | Yes | Yes |
| CSS selector targeting | Yes | No (clear error message) |
| Coordinate targeting | Yes | No (clear error message) |
| hover / scroll | Yes | Yes (via Chrome MCP `hover`/`press_key`) |
| Element bounds | Yes | No |
| SSRF protection | Yes | Yes (URL check before navigate) |
| Idle timeout reclaim | Yes | N/A (don't close user's Chrome) |

LLM receives clear error messages for unsupported operations and adjusts strategy (R8 LLM Sovereignty).

## Error Handling

New `BrowserError` variants:

```rust
pub enum BrowserError {
    // ... existing variants unchanged ...

    #[error("Failed to attach to browser: {0}")]
    AttachFailed(String),

    #[error("Chrome DevTools MCP error: {0}")]
    ChromeMcpError(String),

    #[error("Browser profile not found: {0}")]
    ProfileNotFound(String),
}
```

Error recovery:
- Tool-level errors (element not found, timeout) — return error to LLM, keep session
- Transport errors (process exit, pipe broken) — destroy session, auto-rebuild on next call
- Chrome not available — clear message guiding user to install/restart Chrome

## File Changes

### New Files

| File | Purpose | Est. Lines |
|------|---------|-----------|
| `src/browser/backend.rs` | `BrowserBackend` trait | ~60 |
| `src/browser/chrome_mcp.rs` | `ChromeMcpDriver` — session management, Chrome auto-launch | ~300 |
| `src/browser/chrome_mcp_snapshot.rs` | Snapshot tree-to-flat conversion | ~80 |
| `src/browser/managed_backend.rs` | `ManagedBackend` — wraps `BrowserRuntime` | ~150 |
| `src/browser/chrome_mcp_backend.rs` | `ChromeMcpBackend` — param conversion + MCP calls | ~250 |

### Modified Files

| File | Change |
|------|--------|
| `src/browser/mod.rs` | Add mod declarations + pub use |
| `src/browser/profile.rs` | Add `BrowserDriver`, `ChromeMcpConfig`, `driver` field to `ProfileConfig`, `chrome_mcp` field to `BrowserSystemConfig` |
| `src/browser/manager.rs` | Add `get_backend()` routing, auto-inject "user" profile, hold `ChromeMcpDriver` |
| `src/browser/error.rs` | Add `AttachFailed`, `ChromeMcpError`, `ProfileNotFound` |
| `src/builtin_tools/browser_tools/*.rs` | Implement actual backend routing via `ProfileManager::get_backend()` (tools are currently stubs with placeholder responses) |

### Unchanged Files

| File | Reason |
|------|--------|
| `runtime.rs` | Stays as `ManagedBackend` underlying impl |
| `actions.rs`, `snapshot.rs` | Still serve managed mode |
| `discovery.rs` | Reused for Chrome auto-launch |
| `network_policy.rs` | SSRF checks still apply to both modes |
| `playwright_bridge.rs` | Untouched, may be removed later |
| `src/mcp/` | No changes — `ChromeMcpDriver` uses existing MCP client API |

## Testing Strategy

| Layer | Method |
|-------|--------|
| Snapshot conversion | Unit tests — fixed JSON input, verify AriaSnapshot output |
| ChromeMcpDriver session logic | Unit tests — mock MCP client, verify cache/dedup/cleanup |
| BrowserBackend routing | Unit tests — verify driver type dispatches correctly |
| End-to-end | `#[ignore]` integration tests — requires real Chrome + npx |

## Security Considerations

- **Remote debugging port**: `--remote-debugging-port=0` binds to `127.0.0.1` only (Chrome default). Any local process can connect to this port and access the browser session (cookies, passwords). This is Chrome's standard debugging interface and the same surface exposed by Chrome DevTools and any other CDP-based tool.
- **First-use consent**: Aleph should log a one-time warning when existing-session mode is first used, informing the user that Chrome will be launched with remote debugging enabled.
- **SSRF protection**: URL checks via `NetworkPolicy` apply to both managed and existing-session modes. Navigate operations are validated before forwarding to Chrome MCP.

## Out of Scope (YAGNI)

- Multi-browser support (Brave, Edge, Firefox)
- Custom ref_id numbering schemes
- Session disk persistence across Aleph restarts
- Download interception for existing-session mode
- PDF export for existing-session mode
