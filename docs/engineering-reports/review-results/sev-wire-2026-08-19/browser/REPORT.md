# Severed-wire audit — `src/browser/` (2026-08-19 round)

Scope: `src/browser/` (19 files, ~8.5k LoC), with strict cross-crate budget.
Method: skill methodology — scan, enumerate, **read-first triage**, fix, guard.

## Module map

The browser subsystem is a **dual-adapter pattern** behind a single manager:

- `BrowserBackend` trait (`backend.rs`) — 30 methods, all async-trait.
- `ChromeMcpBackend` (`chrome_mcp_backend.rs`) — wires through `ChromeMcpDriver`
  (`chrome_mcp.rs`), which spawns `chrome-devtools-mcp` per profile.
- `PlaywrightCliBackend` (`playwright_cli_backend.rs`) — shells out to
  `playwright-cli` per session via `PlaywrightCliDriver` (`playwright_cli.rs`).
- `ProfileManager` (`manager.rs`) — routes `BrowserDriver::Managed` →
  `PlaywrightCliBackend`, `BrowserDriver::ExistingSession` →
  `ChromeMcpBackend`. Owns SSRF policy hot-apply, idle reaping, and tab
  registry.
- Support modules: `profile.rs` (config types), `network_policy.rs` (SSRF +
  secret egress), `secret_guard.rs` (credential patterns), `wait_probe.rs`
  (default `wait_for` polling), `tab_registry.rs` (LRU tab tracker),
  `post_nav.rs` (single quarantine site), `discovery.rs` (Chromium binary
  search), `playwright_launch.rs` (per-session `open` argv + config file),
  `types.rs` (DTOs), `error.rs` (error variants), `testkit.rs` (test-only
  fake).

## Method-by-method adapter parity check

For every `BrowserBackend` method, both backends implement (or use the
trait default), and the manager dispatch matches:

| method | chrome_mcp_backend | playwright_cli_backend | manager dispatch |
|---|---|---|---|
| `open_tab`, `close_tab`, `list_tabs`, `navigate`, `click`, `type_text`, `fill`, `hover`, `scroll`, `screenshot`, `snapshot`, `evaluate`, `select`, `press_key`, `history`, `dblclick`, `console_messages`, `network_log`, `switch_tab`, `handle_dialog`, `drag`, `upload`, `resize`, `emulate` | override | override | both used |
| `wait_for` | override (native `Text` arm; falls back to default for other arms) | default (`wait_probe::poll_wait_for`) | both used |
| `fill_form` | override (native `fill_form`) | default (loop calling `fill`) | both used |
| `pdf`, `save_state`, `load_state`, `cookies` | default (`unsupported_in_existing_session`) | override (CLI primitives) | one-sided capability — correctly described in trait doc |

`evaluate` returns the **value** the script produced, not a transcript, on
both drivers (chrome_mcp: `parse_evaluate_value` extracts from ` ```json ` fences;
playwright_cli: `parse_result_value` extracts from `### Result` section).
Verified against live CLI/MCP output, not assumed.

`secret_guard` hooks fire on every navigation/form-input leg:
- `scan_url_for_secrets` — called by `network_policy::check_navigation`
  (the agent-initiated navigation gate).
- `scan_text_for_secrets` — called by `network_policy::check_input` (form
  input gate).
- `redact_secrets` — called by `network_policy::redact_content` (page-content
  egress gate). Wired from `manager::redact_content`, consumed by
  `builtin_tools/browser_tools/mod.rs` via `redact_wrap` /
  `redact_and_wrap` / `redact_and_wrap_log`.

`post_nav::audit_landed_url` is the **single quarantine site** (one
`close_tab` call). Both backends funnel through it.

## Findings

### CUT — orphan re-exports in `browser/mod.rs` (browser-01, low)

The `pub use` lines for `CliOutput`, `PageMeta`, `PlaywrightCliBackend`,
`ChromeMcpDriver`, `ChromeMcpBackend` had **zero external consumers**
(repo-wide grep confirms). They advertise a public API that no crate code
uses — the four backend types are reached only via the manager's
`Arc<dyn BrowserBackend>`, and the CLI output types are pure parse-results
that stay inside the browser module. CUT also stripped the `pub use` for
`PlaywrightCliDriver`, `LaunchPolicy`, `SessionLaunch`, `BrowserBackend`,
and the `types::*` re-exports — same reason: every external consumer reaches
them via the explicit module path (`crate::browser::playwright_cli::…`,
`crate::browser::types::ActionTarget`, etc.).

**File**: `src/browser/mod.rs:1-30`

**Fix**: dropped orphan `pub use` lines; demoted `pub mod chrome_mcp`,
`pub mod chrome_mcp_backend`, `pub mod playwright_cli_backend` to
`pub(crate) mod …` (their items now bound by module visibility).

### CUT — `pub` items used only inside the browser module (browser-02, low)

`playwright_cli.rs` exposed `CliOutput`, `PageMeta`, `parse_error_section`,
`parse_page_meta`, `parse_result_value` as `pub`, but every caller lives
inside the browser module (`playwright_cli_backend.rs`, `wait_probe.rs`,
`chrome_mcp_backend.rs` doc references, internal tests). Demoted to
`pub(crate)`.

**File**: `src/browser/playwright_cli.rs:36,45,399,427,488`

### CUT — `pub` items on `ChromeMcpDriver` (browser-03, low)

Every public method on `ChromeMcpDriver` (`new`, `profile_lock`,
`call_tool`, `has_session`, `destroy_session`) is called only from
`manager.rs` and the backend in the same module. With the module already
demoted to `pub(crate)`, the items were bounded by module visibility —
demoted them to `pub(crate)` explicitly for code-clarity.

**File**: `src/browser/chrome_mcp.rs:56,84,98,107,419,427`

## Almost-cut but kept (audit defense)

- **`manager.rs::get_driver`** — looked test-only at first (only callers
  surfaced by the initial grep were `test_get_driver` etc.), but
  `builtin_tools/browser_tools/profile_tool.rs:81,110` is a live consumer.
  KEEP. Lesson: `git grep` from the *called-into* direction misses callers
  whose names don't share a substring with the method; only the `grep -rn
  '\.get_driver('` pattern caught this.
- **`manager.rs::has_tracked_tabs`** — `#[cfg(test)] pub(crate)`, used
  inside manager.rs's own test module. KEEP — test-only API, not orphan.
- **`manager.rs::idle_managed_profiles`,
  `idle_existing_session_profiles`** — private helpers of `reap_idle` /
  `reap_idle_tabs`. KEEP.
- **`BrowserBackend::pdf / save_state / load_state / cookies`** trait
  defaults — these return `unsupported_in_existing_session`, which **names
  the backend and the remedy** rather than a generic "not supported". The
  MCP server genuinely has no `pdf`/`state-*`/`cookie-*` primitive, so the
  only honest answer is the existing-session profile doesn't speak them
  and `default` is the right driver. KEEP.
- **`unhonored_managed_fields`** — private boot-time warning. KEEP.
- **`is_idle`** — pure helper for the reaper. KEEP.

## Cross-cutting concerns / blockers

None. No Cargo.toml or src/lib.rs change was required.

## Total

| verdict | count |
|---|---|
| CUT | 3 findings |
| CONNECT | 0 |
| DECIDE+deferred | 0 |
| almost-cut, kept | 6 (see above) |

## Module verdict

The browser module is unusually **clean for severed wires**. The dual-adapter
pattern (chrome_mcp + playwright_cli behind `ProfileManager`) is the obvious
place wires break: a method on one backend the manager never reaches, a
secret-guard hook the navigation gate never consults, a network policy that
isn't read by the post-nav audit. Every such seam was verified and proved
wired — the manager's `get_backend` routes consistently with both backends'
implementations, the secret-egress boundary has three legs and all three are
covered, the post-nav quarantine has exactly one `close_tab` site (no copy
drift).

The cleanup is therefore **visibility hygiene**, not architectural repair:
orphan re-exports and `pub` items that should be `pub(crate)` because no
external code reads them.

## Commit

`audit(browser): sever 3 findings from 2026-08-19 round` — see `git log`.