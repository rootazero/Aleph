# Runtimes Module Refactor Design

**Date:** 2026-04-12
**Status:** Design approved, pending implementation plan
**Follows:** 2026-04-12-playwright-cli-migration-design.md

## Problem

After the playwright-cli migration, `src/runtimes/` (language runtimes like uv, fnm, ffmpeg, yt-dlp) and `src/browser/bootstrap.rs` (fnm + Node + playwright-cli + Chromium + skills) contain duplicated detect-install-ledger machinery. Two mechanisms, two UIs, two event types — each covering a disjoint subset of what should be a single cross-cutting capability.

Simultaneously, the existing `src/runtimes/` uses a bundled-runtime philosophy (`~/.aleph/runtimes/python/default/`) that is no longer desired. The playwright-cli migration moved to language-native tools and user-home paths (`~/.local/share/fnm/`); the same shift should apply here.

Windows support is missing entirely from `src/runtimes/bootstrap.rs` (only macOS/Linux shell scripts). The spec table format (two parallel const arrays for probe/bootstrap) resists cross-OS expansion.

## Solution

Refactor `src/runtimes/` as the single home for runtime management. Unify probe + bootstrap + ledger under one spec table (`SPECS`). Support macOS/Linux/Windows via an `InstallStrategy` enum. Delete `src/browser/bootstrap.rs`; browser invokes `ensure_capability("playwright-cli")` and relies on `deps` + `post_install` actions to chain fnm → Node → playwright-cli → chromium → skills.

Move the UI from Panel Settings → Browser → "Runtime Status" card (to be deleted) into a new top-level view `views/runtimes.rs` (Dashboard style — informational, not configurational).

Preserve the existing `capability.rs` LLM prompt injection mechanism; move the usage-hint text from a hardcoded `get_usage_hints()` function into each `RuntimeSpec::llm_hint` field (single source of truth).

Keep the `ensure_capability(name, ledger) -> PathBuf` public API unchanged.

## Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Scope | Hybrid refactor (C): keep top-level API, rewrite specs/probe/bootstrap internals | API is clean; internals need cross-OS + structured strategies |
| Runtimes covered | fnm/Node, uv/Python, playwright-cli (+ chromium/skills as post-install); cargo left as future slot | cargo's use case deferred (Aleph users already have it) |
| Install strategy | Language-native tools (fnm, uv, future rustup) | Cross-OS consistency; no sudo; no system pollution |
| Install paths | Trust tools' own defaults (`~/.local/share/fnm/`, `~/.local/bin/uv`) | No more bundled `~/.aleph/runtimes/` specific paths |
| Module name | Keep `src/runtimes/` (plural) | Matches existing `~/.aleph/runtimes/` ledger path; avoids git blame noise |
| LLM injection | Static at startup (Q4/A); read from `RuntimeSpec::llm_hint` | Prompt cache stability; rare runtime install frequency |
| UI location | New top-level view `views/runtimes.rs`; NOT under Settings | Runtime status is information, not configuration |
| cargo/Rust | Spec with empty `install` array; `ensure_capability` returns "not yet supported" | YAGNI; reserves name without implementing |
| Windows fnm | Via `winget install Schniz.fnm` | Aligns with Windows package manager conventions |
| Legacy data | Drop `aleph_paths` fields entirely (b3); CHANGELOG notifies users of orphan dirs | Clean break; user data is not Aleph's responsibility |
| browser/bootstrap | Delete; fold into runtimes | Eliminates duplicated machinery |

## Architecture

```
src/runtimes/                         refactored
├── mod.rs                            thin re-exports
├── capability.rs                     kept + hint source changed
├── ledger.rs                         kept as-is
├── ensure.rs                         kept API; reads SPECS for deps
├── probe.rs                          rewritten — cross-OS, reads SPECS
├── bootstrap.rs                      rewritten — cross-OS, reads SPECS, post-install chain
├── os.rs                             NEW — TargetOs + select_install()
├── post_install.rs                   NEW — RunSubcommand / FnmAlias / AssetProbe
└── specs.rs                          NEW — SPECS table + type definitions

src/browser/bootstrap.rs              DELETED
src/gateway/handlers/browser_runtime.rs  DELETED
interfaces/webchat/src/views/settings/browser_runtime.rs  DELETED

src/gateway/handlers/runtimes.rs      NEW — 3 RPCs (list/install/refresh)
interfaces/webchat/src/views/runtimes.rs  NEW — Dashboard view
interfaces/webchat/src/api/runtimes.rs    NEW — RPC client
```

Dependency flow:

```
browser/playwright_cli.rs
    ↓ (on binary miss)
runtimes::ensure_capability("playwright-cli", ledger)
    ↓ (deps: node)
ensure_capability("node", ledger)
    ↓ (deps: fnm)
ensure_capability("fnm", ledger)
    ↓ install (OsInstall[TargetOs::current()])
Shell / PowerShell / Via<parent> dispatcher
    ↓ post_install chain
chromium + skills deployed
    ↓ all ledger entries Ready
PlaywrightCliDriver proceeds with cached binary path
```

## `RuntimeSpec` Data Model (core abstraction)

```rust
pub struct RuntimeSpec {
    pub name: &'static str,
    pub binaries: &'static [&'static str],
    pub version_flag: &'static str,
    pub version_regex: &'static str,
    pub min_version: Option<&'static str>,
    pub deps: &'static [&'static str],
    pub install: &'static [OsInstall],
    pub post_install: &'static [PostInstallAction],
    pub llm_hint: Option<&'static str>,
}

pub struct OsInstall {
    pub os: TargetOs,
    pub strategy: InstallStrategy,
}

pub enum InstallStrategy {
    Shell(&'static str),                          // curl | bash style
    PowerShell(&'static str),                     // Windows
    Via { parent: &'static str, subcommand: &'static [&'static str] },
    Unsupported { reason: &'static str },
}

pub enum PostInstallAction {
    RunSubcommand { args: &'static [&'static str], target_dir: Option<&'static str> },
    FnmAlias { alias_name: &'static str },
    AssetProbe { path: &'static str, repair: &'static [&'static str] },
}
```

`Via { parent: "node", ... }` is special-cased in the runner to wrap with `fnm exec --using lts --` automatically, so spec authors don't need to write that boilerplate repeatedly.

## TargetOs Abstraction

```rust
pub enum TargetOs { MacOs, Linux, Windows, AnyUnix, AnyOs }

impl TargetOs {
    pub fn current() -> Self { /* concrete variant only */ }
    pub fn matches(&self, current: TargetOs) -> bool { /* AnyUnix/AnyOs wildcards */ }
}

pub fn select_install<'a>(
    installs: &'a [OsInstall],
    current: TargetOs,
) -> Option<&'a OsInstall>
```

First-match wins; spec writers list narrow OSes first, wildcards last.

## Sample Spec Entries

```rust
pub const SPECS: &[RuntimeSpec] = &[
    RuntimeSpec {
        name: "fnm",
        binaries: &["fnm"],
        version_flag: "--version",
        version_regex: r"fnm (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[
            OsInstall { os: TargetOs::AnyUnix, strategy: InstallStrategy::Shell(
                "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell"
            )},
            OsInstall { os: TargetOs::Windows, strategy: InstallStrategy::PowerShell(
                "winget install Schniz.fnm --silent --accept-source-agreements"
            )},
        ],
        post_install: &[],
        llm_hint: Some("Node version manager (fnm). Used implicitly by `node`."),
    },
    RuntimeSpec {
        name: "node",
        binaries: &["node"],
        version_flag: "--version",
        version_regex: r"v(\d+\.\d+\.\d+)",
        min_version: Some("18.0"),
        deps: &["fnm"],
        install: &[OsInstall { os: TargetOs::AnyOs, strategy: InstallStrategy::Via {
            parent: "fnm",
            subcommand: &["install", "--lts"],
        }}],
        post_install: &[PostInstallAction::FnmAlias { alias_name: "lts" }],
        llm_hint: Some("Node.js runtime. Use via `fnm exec --using lts -- node <script.js>`."),
    },
    RuntimeSpec {
        name: "uv",
        binaries: &["uv"],
        version_flag: "--version",
        version_regex: r"uv (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[
            OsInstall { os: TargetOs::AnyUnix, strategy: InstallStrategy::Shell(
                "curl -LsSf https://astral.sh/uv/install.sh | sh"
            )},
            OsInstall { os: TargetOs::Windows, strategy: InstallStrategy::PowerShell(
                "powershell -ExecutionPolicy ByPass -c \"irm https://astral.sh/uv/install.ps1 | iex\""
            )},
        ],
        post_install: &[],
        llm_hint: Some("Python package manager. Run scripts via `uv run <file.py>`; install packages via `uv pip install <pkg>`."),
    },
    RuntimeSpec {
        name: "playwright-cli",
        binaries: &["playwright-cli"],
        version_flag: "--version",
        version_regex: r"(\d+\.\d+\.\d+)",
        min_version: None,
        deps: &["node"],
        install: &[OsInstall { os: TargetOs::AnyOs, strategy: InstallStrategy::Via {
            parent: "node",
            subcommand: &["--", "npm", "install", "-g", "@playwright/cli@latest"],
        }}],
        post_install: &[
            PostInstallAction::RunSubcommand {
                args: &["install", "chromium"],
                target_dir: None,
            },
            PostInstallAction::RunSubcommand {
                args: &["install", "--skills", "--target"],
                target_dir: Some("$HOME/.aleph/skills/playwright-cli"),
            },
        ],
        llm_hint: Some("Browser automation CLI. Use `playwright-cli -s=<session> <command>`."),
    },
    RuntimeSpec {
        name: "cargo",
        binaries: &["cargo"],
        version_flag: "--version",
        version_regex: r"cargo (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[],  // empty = placeholder; ensure_capability returns "not yet supported"
        post_install: &[],
        llm_hint: None,
    },
];
```

## Post-install Actions

Three variants cover all current needs:

1. **`RunSubcommand`** — run the just-installed binary with args. Used for `playwright install chromium` and `playwright-cli install --skills --target`. `target_dir` expands `$HOME`.
2. **`FnmAlias`** — parse `fnm list` output, extract latest version, run `fnm alias <version> <name>`. Fixes the lts-alias bug from the prior migration.
3. **`AssetProbe`** — verify a file/dir exists; on miss, run a repair subcommand. Handles "user deleted chromium from disk" scenarios.

## Gateway RPC

Three RPCs under `runtimes.*`:

| Method | Params | Returns | Purpose |
|---|---|---|---|
| `runtimes.list` | — | `{ runtimes: RuntimeInfo[] }` | All capabilities with current status |
| `runtimes.install` | `{ capability: String }` | `{ accepted: true }` | Fire-and-forget async install; progress via event |
| `runtimes.refresh` | — | `{ runtimes: RuntimeInfo[] }` | Re-probe all, return updated list |

```rust
pub struct RuntimeInfo {
    pub name: String,
    pub status: CapabilityStatus,
    pub bin_path: Option<String>,
    pub version: Option<String>,
    pub source: Option<CapabilitySource>,
    pub llm_hint: Option<String>,
    pub deps: Vec<String>,
    pub supported_on_current_os: bool,
}
```

Replaces the browser-scoped `browser.runtime_status`, `browser.install_runtime`, `browser.refresh_runtime` (all deleted).

## Event

`BrowserInstallProgressEvent` → `RuntimeInstallProgressEvent` (rename; fields unchanged).
`GatewayEvent::BrowserInstallProgress` → `GatewayEvent::RuntimeInstallProgress`.

Emitted at:
- Enter Probing → `status = "started"`
- Each post-install action start/end → `status = "log"`
- Enter Ready → `status = "done"`
- Any failure → `status = "failed"` + error string

## Panel UI (Dashboard → Runtimes)

New top-level view `views/runtimes.rs`. Route added to sidebar next to `Cron`, `Logs`, `Tasks`, `Memory`. Explicitly **not** under Settings.

Layout per runtime entry:

```
✓ fnm            1.35.0
  /usr/local/bin/fnm
  Node version manager

✗ uv             not installed  [Install]
  Python package manager

⊘ cargo          not supported yet
  (Rust toolchain management — future)
```

Install button only renders when `status = Missing && supported_on_current_os`. Read-only otherwise — no version editing, no path overrides.

Refresh button triggers `runtimes.refresh` and re-renders.

## LLM Context Injection

Preserved from existing `capability.rs::format_entries_for_prompt()`. Two changes:

1. Delete the hardcoded `get_usage_hints()` function.
2. Read `llm_hint` directly from `SPECS` via name lookup.

Startup sequence:
1. `CapabilityLedger::load_or_create(...)`
2. For each `spec in SPECS`: `probe::probe(spec.name)` → update ledger entry's status.
3. Ledger passed to agent_loop.
4. On system-prompt assembly: `ledger.list_ready()` → `format_entries_for_prompt(entries)` → injected once per session.

No runtime refresh of the prompt mid-session (Q4/A decision). Post-install capabilities appear to LLM on next session start.

## File Changes

### Created (6 files, ~810 LoC)

| File | Purpose | ~LoC |
|---|---|---|
| `src/runtimes/os.rs` | TargetOs enum + select_install | 60 |
| `src/runtimes/specs.rs` | SPECS table + types | 250 |
| `src/runtimes/post_install.rs` | Three post-install runners | 120 |
| `src/gateway/handlers/runtimes.rs` | 3 RPCs | 100 |
| `interfaces/webchat/src/views/runtimes.rs` | Dashboard view | 200 |
| `interfaces/webchat/src/api/runtimes.rs` | RPC client types | 80 |

### Rewritten (3 files)

- `src/runtimes/probe.rs` — reads SPECS; cross-OS; drops aleph_paths
- `src/runtimes/bootstrap.rs` — reads SPECS; InstallStrategy dispatcher; post-install chain
- `src/runtimes/capability.rs` — `get_usage_hints()` deleted; reads `SPECS[name].llm_hint`

### Modified (~6 files)

- `src/runtimes/mod.rs` — exports new modules
- `src/runtimes/ensure.rs` — adapts to new `bootstrap::install` signature; deps from `SPECS[name].deps`
- `src/browser/playwright_cli.rs` — `resolve_binary()` calls `ensure_capability("playwright-cli", ledger)` on miss
- `src/gateway/handlers/mod.rs` — registers `runtimes.*`; unregisters `browser.runtime_*`
- `src/gateway/event_bus.rs` — renames event type
- `interfaces/webchat/src/views/mod.rs` — exports `runtimes`; adds sidebar entry

### Deleted (4 files)

- `src/browser/bootstrap.rs`
- `src/gateway/handlers/browser_runtime.rs`
- `interfaces/webchat/src/views/settings/browser_runtime.rs`
- `interfaces/webchat/src/api/browser.rs` section containing `BrowserRuntimeApi` / `RuntimeStatusResponse` / `ComponentStatus` (split to `api/runtimes.rs`)

## Commit Sequence (single PR, ~12 atomic commits)

1. `runtimes: add TargetOs abstraction + select_install`
2. `runtimes: add SPECS table with RuntimeSpec/OsInstall/InstallStrategy types`
3. `runtimes: add post_install module (RunSubcommand/FnmAlias/AssetProbe)`
4. `runtimes: rewrite probe to read SPECS and support Windows`
5. `runtimes: rewrite bootstrap with InstallStrategy dispatcher + post-install chain`
6. `runtimes: capability.rs llm_hint source from SPECS`
7. `runtimes: adapt ensure.rs to new bootstrap signature`
8. `browser: delete bootstrap.rs; playwright_cli resolves via ensure_capability`
9. `gateway: add runtimes.list/install/refresh RPCs; rename event to RuntimeInstallProgress`
10. `gateway: delete browser_runtime handlers`
11. `webchat: add Runtimes top-level view + API client`
12. `webchat: delete settings/browser_runtime.rs; remove sidebar Browser Runtime card`
13. `docs: update docs/examples/CHANGELOG for runtimes refactor`

## Testing Strategy

| Layer | Method |
|---|---|
| `TargetOs::matches` | unit tests — 9 concrete/wildcard pairs |
| `select_install` | unit tests — priority + fallback |
| `SPECS` completeness | unit test — each entry has at least 1 OsInstall for current OS or empty `install` |
| `probe::probe` | unit tests — fixture-based version parsing per capability |
| `bootstrap::install` | unit tests with mock Command runner — deps recursion, post-install serialization |
| `post_install::run` | unit tests — each variant's happy path |
| `runtimes.*` RPCs | unit tests — shape of list response, install triggers event stream |
| End-to-end | `#[ignore]` — `ensure_capability("playwright-cli")` on real macOS |

## Security Considerations

- Shell install scripts come from known HTTPS sources (astral.sh, fnm.vercel.app); no checksum verification v1.
- PowerShell scripts on Windows use `ExecutionPolicy ByPass` — scoped to the single-line invocation only, not persistent policy change.
- Child process env filter continues (strip `ANTHROPIC_API_KEY` etc.) — unchanged from playwright-cli migration.
- No sudo required for any install path.
- Legacy `~/.aleph/runtimes/` orphan data left in place (user responsibility to clean).

## Out of Scope (YAGNI)

- `cargo`/`rustup` real implementation (spec with empty `install` is placeholder)
- Uninstall RPC / UI button (`runtimes.uninstall`)
- Forced reinstall (Install button is no-op on Ready capabilities)
- Runtime version switching (e.g., Node v18 vs v22)
- Automatic cleanup of legacy `~/.aleph/runtimes/` bundled data (user does this manually)
- Non-fnm Node version managers on Windows (nvm-windows, Volta)
- System package manager fallback (brew, apt) — only language-native tools
- Mid-session prompt refresh after a newly installed runtime (requires restart)

## Success Criteria

1. `cargo check --workspace` zero errors
2. `cargo test -p alephcore --lib runtimes` all pass
3. `src/browser/bootstrap.rs` does not exist; `gateway/handlers/browser_runtime.rs` does not exist
4. Cold Aleph start → Panel sidebar shows Runtimes view with all 5 capability entries
5. Click "Install" on uv (when missing) → streaming log shows `curl install.sh | sh` output → status transitions Missing → Bootstrapping → Ready
6. Chat: ask AI "what runtimes do you have" → AI response mentions fnm/node/uv/playwright-cli with their usage hints (proves prompt injection works)
7. Cold start with legacy `~/.aleph/runtimes/python/default/` directory present → it is NOT probed; ledger shows python as Missing or absent; CHANGELOG informs user of manual cleanup option
