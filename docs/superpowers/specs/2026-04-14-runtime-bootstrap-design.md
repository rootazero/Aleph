# Runtime Bootstrap Design — Panel + Install Script Adaptation

**Date:** 2026-04-14
**Status:** Design approved, pending implementation plan
**Supersedes (partially):** the Panel-Settings portion of `2026-04-12-playwright-cli-migration-design.md`

## Problem

The `2026-04-12-playwright-cli-migration` work landed `PlaywrightCliBackend`, deleted chromiumoxide, and updated the Panel Browser form's labels. It did **not** land the Runtime Status / bootstrap-install UX that the original design called for.

In parallel, Aleph already grew a high-quality `src/runtimes/` engine (probe + install + ledger + dep resolution, covering fnm / node / uv / playwright-cli / cargo / git) and corresponding gateway RPCs (`runtimes.list` / `runtimes.install` / `runtimes.refresh`). This engine is currently invoked only lazily, at tool-call time, via `ensure_capability`.

Gaps that cause a real user-facing problem:

1. **No install-time bootstrap.** `install.sh` / `install.ps1` download the Aleph binary and register a service, but never install fnm / Node / uv / @playwright/cli / Chromium. First-run users hit "runtime missing" errors the first time any browser or python tool is invoked.
2. **No Panel UI.** There is no `Settings → Runtime` page, and the Browser page has no visibility into whether its runtime is actually installed.
3. **`~/.aleph/.venv` is never auto-created.** The `uv` spec installs `uv` but nothing creates the shared global venv. Today `code_exec.md` prompt only tells the LLM "create it yourself if missing" — this should be bootstrap's job.
4. **Residual `Playwright MCP` references** in six source comments and one stale review artefact remain from the migration, even though the runtime has changed.
5. **No CLI entry point** to trigger a full bootstrap from a terminal — users cannot retry from shell without re-running the full install script.
6. **No startup probe.** `aleph-server start` does not populate the capability ledger, so the Panel shows nothing until a tool is invoked.

## Solution

Add a thin **`aleph-server bootstrap-runtime`** CLI subcommand that wraps the existing `runtimes::ensure_capability()` engine. Wire `install.sh` / `install.ps1` to invoke it at the end of a first-run install (with `--skip-runtime` / `$ALEPH_SKIP_RUNTIME=1` / `-SkipRuntime` opt-outs). Add a non-blocking startup probe so the Panel always reflects current state. Build a new **`Settings → Runtime`** Panel page that drives the existing `runtimes.*` RPCs via a step-indicator UI. Embed a small runtime-summary banner on the Browser page for context. Teach the `uv` spec to auto-create `~/.aleph/.venv` via a `PostInstallAction::AssetProbe`. Clean up the Playwright-MCP comment residuals.

One engine (`src/runtimes/`) — three consumers (install scripts, startup probe, Panel).

## Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Where does runtime-install logic live? | Rust `src/runtimes/` + thin CLI wrapper (shared across install scripts, panel, CLI) | Single source of truth; testable; cross-platform diffs centralised; aligns with R7 One Core, Many Shells |
| Install trigger policy | `install.sh` / `install.ps1` call `bootstrap-runtime` by default; opt-out via `--skip-runtime` flag or `$ALEPH_SKIP_RUNTIME=1` env var | Zero-friction first-run UX, but CI/Docker/advanced users have an escape hatch |
| Platform scope v1 | macOS + Linux + Windows (all covered by existing SPECS table: AnyUnix shell / PowerShell strategies) | Single release cadence; reuses what's already specced |
| Install-script failure mode | Soft-fail — warning printed, Aleph install continues, Panel shows missing + fix instructions | Aleph core still usable (LLM chat etc.); browser/python tools degrade gracefully with actionable error |
| Package-manager preference | Self-managed fnm (curl from GitHub release) → Node via fnm → uv via astral.sh script → playwright-cli via `fnm exec npm i -g` | Matches original spec; avoids brew/apt/dnf detection matrix; relies on SPECS table that already exists |
| Startup behaviour | Non-blocking background probe refreshing the ledger; no auto-install at startup | Startup stays fast; respects user's `--skip-runtime` choice; install happens only via explicit action |
| Re-install button scope | Smart: probe first, install only missing (ensure_capability already does this idempotently) | Low-friction; matches user expectations |
| Progress UX | Step indicator per capability + final stderr on failure (no live log streaming v1) | Adequate for diagnosis; minimal backend change; streaming deferred to a later iteration |
| Uninstall button | Out-of-scope (YAGNI) | User can `fnm uninstall lts` + `rm -rf ~/.aleph/.venv` manually |
| `.venv` creation | `uv` spec gains a `PostInstallAction::AssetProbe` that idempotently runs `uv venv $HOME/.aleph/.venv` | Lifts "LLM creates its own venv" guidance into deterministic bootstrap |
| Uniform state file | Re-use existing `~/.aleph/runtimes/ledger.json` as the only truth source | Avoids two-truth problem with a separate `bootstrap-state.json` |
| Panel page placement | New top-level `Settings → Runtime` page; Browser page gets a small summary banner only | Runtime is cross-cutting (browser + python + future skills), not browser-specific |

## Audit of What Already Exists

### `src/runtimes/` — ready for reuse

- `specs.rs` — `SPECS` table: `fnm`, `node`, `uv`, `playwright-cli`, `cargo`, `git` with per-OS install strategies (`Shell` / `PowerShell` / `Via { parent, subcommand }`) and `PostInstallAction` hooks (`RunSubcommand` / `FnmAlias` / `AssetProbe`)
- `probe.rs` — system-PATH probe, semver-aware `min_version` warnings, regex cache
- `bootstrap.rs` — install dispatcher covering the three `InstallStrategy` variants
- `post_install.rs` — `$HOME` expansion (template path only — see §4.1), `uv venv`-style subcommands, fnm alias creation, `AssetProbe` verify-or-repair
- `ensure.rs` — the orchestrator: fast-path → probe → recursive dep resolution → bootstrap → persist ledger, with actionable error classes
- `ledger.rs` — `~/.aleph/runtimes/ledger.json` persistence

### `src/gateway/handlers/runtimes.rs` — already wired

- `runtimes.list` — returns all `SPECS` with ledger status
- `runtimes.install` — runs `ensure_capability` in a background task, emits `GatewayEvent::RuntimeInstallProgress { step, status, log_line, error }`
- `runtimes.refresh` — forces a full re-probe and updates the ledger

### What is missing

- Panel view (`interfaces/webchat/src/views/settings/runtime.rs` does not exist; no route, no API wrapper)
- `aleph-server bootstrap-runtime` subcommand
- `install.sh` / `install.ps1` invocation
- Startup probe in `aleph-server start`
- `uv` `.venv` post-install
- `$HOME` expansion in `PostInstallAction::AssetProbe::repair` args (small bug)
- Residual `Playwright MCP` comments (six sites)
- `runtimes.install` event payload omits final stderr on failure

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  install.sh / install.ps1   (≈ +25 lines each)                       │
│    1. Download aleph-server (unchanged)                              │
│    2. Register launchctl / systemd / Task Scheduler (unchanged)      │
│    3. If not --skip-runtime / $ALEPH_SKIP_RUNTIME=1:                 │
│         exec aleph-server bootstrap-runtime --best-effort            │
│         Soft-fail on non-zero: warn + continue                        │
└──────────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│  aleph-server bootstrap-runtime   (NEW CLI subcommand ≈ 200 lines)    │
│    Thin wrapper: resolves targets → ensure_capability() per target   │
│    stderr = step indicator + per-step status                          │
│    --only / --skip / --force / --best-effort / --json / --quiet       │
└──────────────────────────────────────────────────────────────────────┘
                                │  (reuses)
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│  src/runtimes/  ✅ already present                                    │
│    SPECS[fnm, node, uv, playwright-cli, cargo, git]                  │
│    +  uv.post_install += AssetProbe { ~/.aleph/.venv }  ⭐ NEW        │
└──────────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│  aleph-server start  (≈ +30 lines)                                    │
│    tokio::spawn(startup_runtime_warmup(ledger))                      │
│    Non-blocking probe → refresh ledger → warn on missing             │
│    Tool calls that hit Missing receive an actionable error:          │
│      "Run: aleph-server bootstrap-runtime --only <cap>  OR           │
│       Open Panel → Settings → Runtime"                               │
└──────────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│  Panel → Settings → Runtime  (NEW ≈ 260 + 80 + 45 lines)              │
│    Existing RPCs: runtimes.list / install / refresh                  │
│    Existing event: RuntimeInstallProgress (enriched to carry stderr) │
│    UI: per-capability step indicator + Install/Refresh buttons       │
│        + Install log (final stderr on failure)                       │
│    Browser page gets a compact summary banner at the top             │
└──────────────────────────────────────────────────────────────────────┘
```

## 1. SPECS Table Enhancements

### 1.1 `uv` gains auto-venv post-install

In `src/runtimes/specs.rs` (within the `uv` RuntimeSpec):

```rust
post_install: &[
    PostInstallAction::AssetProbe {
        path: "$HOME/.aleph/.venv/bin/python",
        repair: &["venv", "$HOME/.aleph/.venv"],
    },
],
```

On Windows the probe path changes to `"%USERPROFILE%\\.aleph\\.venv\\Scripts\\python.exe"`. Because SPECS entries are static and cross-platform, we use `#[cfg(target_os = "windows")]` to swap the `PostInstallAction` at compile time, or — preferred — keep the template path platform-neutral by teaching `verify_or_repair` to expand `%USERPROFILE%` on Windows and `$HOME` elsewhere and to adjust the suffix.

Chosen approach: **keep SPECS single-source**, and extend `expand_home` in `post_install.rs` to understand both `$HOME` and `%USERPROFILE%`, and to rewrite `/bin/python` → `/Scripts/python.exe` when the target is Windows. Details in §1.2.

### 1.2 `post_install.rs::verify_or_repair` fix

Current bug: `repair` args are passed through verbatim, so `"$HOME/.aleph/.venv"` lands in the child process literally. Fix:

```rust
async fn verify_or_repair(
    bin_path: &PathBuf,
    path_template: &str,
    repair: &[&str],
) -> Result<(), PostInstallError> {
    let expanded_path = expand_home(path_template);   // already implemented
    if PathBuf::from(&expanded_path).exists() {
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

Platform-aware path handling in `expand_home`:

```rust
fn expand_home(template: &str) -> String {
    let s = template.to_string();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let s = s.replace("$HOME", &home).replace("%USERPROFILE%", &home);

    #[cfg(target_os = "windows")]
    let s = s.replace("/bin/python", r"\Scripts\python.exe")
             .replace("/bin/", r"\Scripts\")
             .replace('/', r"\");

    s
}
```

(Implementation detail — the `#[cfg]` rewrite only kicks in for Windows targets, leaving Unix paths untouched.)

### 1.3 `playwright-cli` spec — no change

The existing post-install actions (`install chromium`, `install --skills --target $HOME/.aleph/skills/playwright-cli`) remain. Idempotency is covered because `playwright install chromium` no-ops when present and the target-dir skills install is itself a copy operation.

## 2. CLI Subcommand: `aleph-server bootstrap-runtime`

### 2.1 Surface

```
aleph-server bootstrap-runtime [OPTIONS]

OPTIONS:
  --only <CAP>       Install only the given capability (repeatable)
  --skip <CAP>       Skip the given capability (repeatable)
  --force            Reinstall even if the ledger says Ready
  --best-effort      Exit 0 regardless of failures (used by install scripts)
  --json             Emit NDJSON progress events to stderr
  --quiet            Suppress per-step output; only errors
  -h, --help         Show help

DEFAULT TARGET SET:
  uv, playwright-cli
  → Dep resolution pulls in fnm and node automatically.
  → cargo and git are probed for reporting only; never installed.

EXIT CODES:
  0  Requested targets Ready (always 0 under --best-effort)
  1  One or more targets failed
  2  Invalid arguments
  3  Platform unsupported for a requested capability
```

### 2.2 File

`src/bin/aleph-server/commands/bootstrap_runtime/mod.rs` (≈ 200 lines).

```rust
pub async fn run(opts: Opts) -> ExitCode {
    let ledger_path = runtimes::get_runtimes_dir()?.join("ledger.json");
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));

    let targets = opts.resolve_targets();               // default ["uv", "playwright-cli"]
    let mut printer = ProgressPrinter::new(opts.json, opts.quiet);
    let mut any_failed = false;

    for (idx, cap) in targets.iter().enumerate() {
        printer.step_start(idx + 1, targets.len(), cap);
        if opts.force {
            let mut g = ledger.write().await;
            g.update_status(cap, CapabilityStatus::Missing);
        }
        match runtimes::ensure_capability(cap, &ledger).await {
            Ok(path) => printer.step_done(cap, &path, &*ledger.read().await),
            Err(e) => {
                printer.step_failed(cap, &e);
                any_failed = true;
                if !opts.best_effort { break; }
            }
        }
    }

    for cap in &["cargo", "git"] {
        printer.detect_only(cap, &runtimes::probe::probe(cap));
    }

    printer.summary(any_failed);
    if any_failed && !opts.best_effort { ExitCode::from(1) } else { ExitCode::from(0) }
}
```

### 2.3 Progress formats

Human TTY (default):

```
Bootstrapping Aleph runtimes…

[1/2] uv ........................ installing
        ✓ 0.5.11  (~/.local/bin/uv)
        ✓ ~/.aleph/.venv created

[2/2] playwright-cli ............ installing (via node)
        ▸ resolving dep: fnm ✓ 1.37.1
        ▸ resolving dep: node ✓ v22.8.0 (LTS via fnm)
        ✓ 1.44.0
        ✓ chromium installed
        ✓ skills → ~/.aleph/skills/playwright-cli

System runtimes (detect-only):
  ✓ git 2.40.0
  ✓ cargo 1.78.0

Summary: 2/2 ready.  Ledger: ~/.aleph/runtimes/ledger.json
```

`--json` NDJSON to stderr (for CI / script capture):

```jsonl
{"event":"step_start","capability":"uv","index":1,"total":2}
{"event":"step_done","capability":"uv","version":"0.5.11","path":"/Users/x/.local/bin/uv"}
{"event":"step_start","capability":"playwright-cli","index":2,"total":2}
{"event":"step_failed","capability":"playwright-cli","error":"npm install exited with code 1","stderr":"…"}
{"event":"summary","ready":1,"failed":1,"total":2}
```

### 2.4 Registration

`src/bin/aleph-server/commands/mod.rs` declares the subcommand alongside `start` / `stop` / other existing commands (+3 lines).

## 3. Install Scripts

### 3.1 `install.sh` (≈ +25 lines, inserted before the `install_service` block at line 158)

```bash
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
    echo "Bootstrapping runtime dependencies (fnm → Node LTS → uv → @playwright/cli + Chromium)…"
    echo "(Pass --skip-runtime or set ALEPH_SKIP_RUNTIME=1 to skip.)"
    echo ""
    if ! "$INSTALL_DIR/$BINARY_NAME" bootstrap-runtime --best-effort; then
        echo ""
        echo "Runtime bootstrap hit errors. Aleph will still install."
        echo "   Fix and retry: aleph-server bootstrap-runtime"
        echo "   Or open Panel → Settings → Runtime."
    fi
fi
```

### 3.2 `install.ps1` (≈ +25 lines, inserted before `Install-AlephService`)

Add `param([switch]$SkipRuntime)` at the top, then:

```powershell
$SkipRuntime = $SkipRuntime -or ($env:ALEPH_SKIP_RUNTIME -eq "1")

if ($SkipRuntime) {
    Write-Host ""
    Write-Host "Skipping runtime bootstrap (-SkipRuntime or `$env:ALEPH_SKIP_RUNTIME=1)."
    Write-Host "Run 'aleph-server bootstrap-runtime' later, or use Panel -> Settings -> Runtime."
} else {
    Write-Host ""
    Write-Host "Bootstrapping runtime dependencies (fnm -> Node LTS -> uv -> @playwright/cli + Chromium)…"
    Write-Host "(Pass -SkipRuntime or set `$env:ALEPH_SKIP_RUNTIME='1' to skip.)"
    Write-Host ""
    $proc = Start-Process -FilePath $InstalledPath -ArgumentList "bootstrap-runtime","--best-effort" `
            -NoNewWindow -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Write-Host ""
        Write-Host "Runtime bootstrap hit errors. Aleph will still install." -ForegroundColor Yellow
        Write-Host "   Fix and retry: aleph-server bootstrap-runtime"
        Write-Host "   Or open Panel -> Settings -> Runtime."
    }
}
```

## 4. Startup Warmup + Friendly Tool Errors

### 4.1 `aleph-server start` warmup (≈ +30 lines)

In `src/bin/aleph-server/commands/start/mod.rs`, after `AppContext` is built and before the gateway is wired, spawn:

```rust
tokio::spawn(runtime_startup_warmup(ctx.ledger.clone()));
```

Body:

```rust
async fn runtime_startup_warmup(ledger: Arc<RwLock<CapabilityLedger>>) {
    tracing::info!("runtime warmup: probing capabilities…");
    let mut missing = Vec::new();
    for spec in runtimes::SPECS {
        let result = runtimes::probe::probe(spec.name);
        let now = now_secs();
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
    if !missing.is_empty() {
        tracing::warn!(
            ?missing,
            "runtime capabilities missing — tools depending on them will fail until installed. \
             Run 'aleph-server bootstrap-runtime' or open Panel → Settings → Runtime."
        );
    }
}
```

Non-blocking by design — the server does not wait for this.

### 4.2 Friendly tool errors

In `src/runtimes/ensure.rs`, replace the three generic error branches (`BootstrapResult::{PathNotFound,Failed,Unsupported}`) with a single builder:

```rust
fn runtime_error(capability: &str, reason: String, stderr: Option<&str>) -> AlephError {
    let hint = find_spec(capability).and_then(|s| s.llm_hint).unwrap_or("(no hint)");
    let stderr_line = stderr
        .map(|s| format!("\nStderr tail: {}", truncate(s, 200)))
        .unwrap_or_default();
    AlephError::runtime(
        capability,
        format!(
            "Runtime '{capability}' is not available: {reason}{stderr_line}\n\n\
             Fix options:\n\
               1. Run: aleph-server bootstrap-runtime --only {capability}\n\
               2. Open Panel → Settings → Runtime and click 'Install'.\n\
               3. Install manually — {hint}",
        ),
    )
}
```

LLM agents transcribe this to the user verbatim; markdown-style step numbering renders cleanly in Panel and chat surfaces.

## 5. Gateway: enrich `RuntimeInstallProgress`

Current payload in `src/gateway/handlers/runtimes.rs:141-170` sends only `started` / `done` / `failed` with empty `log_line`. Enrich the `failed` path to include the final stderr:

```rust
Err(e) => RuntimeInstallProgressEvent {
    step: cap_for_event,
    status: "failed".into(),
    log_line: None,
    error: Some(e.to_string()),             // unchanged
    stderr: extract_stderr(&e),             // NEW field; option<String>
    timestamp: chrono::Utc::now().timestamp_millis(),
},
```

`extract_stderr` pulls from `AlephError::runtime`'s inner message. The event struct grows one optional field — additive, wire-compatible.

Live line-by-line streaming is out of scope (see §9 YAGNI).

## 6. Panel Runtime Page

### 6.1 Route & menu

`interfaces/webchat/src/views/settings/mod.rs` registers `/settings/runtime` and adds a sidebar entry between "Browser" and "Execution" (placement intent: runtime is a platform concern, browser is a consumer).

### 6.2 Component tree

```
RuntimeView
├── RuntimeHeader
├── RuntimeCapabilityList
│   └── RuntimeCapabilityRow × N        // fnm, node, uv, playwright-cli
│       ├── StatusDot (● Ready / ○ Missing / ◐ Probing / ✗ Failed)
│       ├── Name + version + bin_path
│       ├── Per-capability [Install] / [Reinstall] button
│       └── Sub-row for visible post-install artefacts
│           (~/.aleph/.venv for uv; chromium + skills for playwright-cli)
├── RuntimeActions
│   ├── [Install missing]
│   ├── [Refresh]
│   └── [Install all]  (hidden unless menu-opened)
├── RuntimeInstallLog
│   └── <pre>  rolling list of step_start / step_done / step_failed + stderr
└── SystemRuntimesFooter
    └── git, cargo (detect-only)
```

### 6.3 API wrapper

`interfaces/webchat/src/api/runtime.rs` (~80 lines) exposes:

- `RuntimeApi::list(&state) -> Result<RuntimesListResponse, String>`
- `RuntimeApi::install(&state, cap: String) -> Result<(), String>`
- `RuntimeApi::refresh(&state) -> Result<RuntimesListResponse, String>`
- `RuntimeApi::subscribe_progress(&state, cb: impl Fn(RuntimeInstallProgressEvent))` — hooks into the existing EventBus WebSocket stream

### 6.4 "Install missing" behaviour

```rust
for rt in list.runtimes.iter().filter(|r| r.status == Missing && r.supported_on_current_os) {
    RuntimeApi::install(&state, rt.name.clone()).await.ok();
}
```

Each call returns `{ "accepted": true }` immediately; progress arrives via EventBus. `ensure_capability`'s dep resolver handles fnm/node transitively.

### 6.5 Browser page summary banner

`interfaces/webchat/src/views/settings/browser_runtime_banner.rs` (~45 lines) — a small component inserted at `browser.rs:86` before the existing sections:

- Reads `runtimes.list`, filters `["fnm", "node", "playwright-cli"]`
- Green banner: `✓ Browser runtime ready`
- Amber banner when any missing: `⚠ Browser runtime missing: <names>  [Configure →]` linking to `/settings/runtime`

## 7. Residual Cleanup

| File | Change |
|---|---|
| `src/config/types/general.rs:44` | Comment: `Playwright MCP` → `Playwright CLI` |
| `src/config/types/general.rs:116` | Example TOML block: `[browser.playwright_mcp]` → `[browser.playwright_cli]` |
| `src/browser/profile.rs:25` | Comment: `(chromiumoxide)` → `(Playwright CLI managed via fnm)` |
| `src/builtin_tools/browser_tools/tabs.rs:111` | Comment: `Playwright MCP:` → `Playwright CLI:` |
| `src/builtin_tools/browser_tools/tabs.rs:140` | Comment: `Neither Chrome MCP nor Playwright MCP` → `Neither Chrome DevTools MCP nor Playwright CLI` |
| `src/builtin_tools/browser_tools/mod.rs:43` | Comment: `Playwright MCP format:` → `Playwright CLI format:` |
| `review-results/browser.md` | **Delete** (references deleted `playwright_mcp_backend.rs`; obsolete) |

Intentionally preserved (compat / history):

- `src/browser/profile.rs:219-220` — `#[serde(default, alias = "playwright_mcp")]` and associated deserialization tests
- `CHANGELOG.md` entries describing the migration
- `docs/superpowers/specs/2026-04-12-*.md` and `docs/superpowers/plans/2026-04-12-*.md`

## 8. File Plan

### New (4 files, ≈ 585 lines)

| Path | Purpose | LoC |
|---|---|---|
| `src/bin/aleph-server/commands/bootstrap_runtime/mod.rs` | CLI subcommand | 200 |
| `interfaces/webchat/src/views/settings/runtime.rs` | Panel Runtime page | 260 |
| `interfaces/webchat/src/api/runtime.rs` | Panel API wrapper + subscribe | 80 |
| `interfaces/webchat/src/views/settings/browser_runtime_banner.rs` | Summary banner | 45 |

### Modified

| Path | Change | ∆ LoC |
|---|---|---|
| `src/runtimes/specs.rs` | `uv.post_install` gains `AssetProbe` | +6 |
| `src/runtimes/post_install.rs` | `expand_home` platform-aware; `repair` args expanded | +20 |
| `src/runtimes/ensure.rs` | Unified actionable error builder | +18 |
| `src/gateway/handlers/runtimes.rs` | `RuntimeInstallProgressEvent` optional `stderr` field | +15 |
| `src/gateway/event_bus.rs` | `RuntimeInstallProgressEvent` struct | +2 |
| `src/bin/aleph-server/commands/mod.rs` | Register subcommand | +3 |
| `src/bin/aleph-server/commands/start/mod.rs` | Spawn warmup | +30 |
| `interfaces/webchat/src/views/settings/mod.rs` | Route + menu | +6 |
| `interfaces/webchat/src/views/settings/browser.rs` | Embed `<RuntimeSummaryBanner />` | +2 |
| `install.sh` | `bootstrap-runtime` invocation + `--skip-runtime` | +25 |
| `install.ps1` | `bootstrap-runtime` invocation + `-SkipRuntime` | +25 |
| 6 residual comment sites | Text updates | 0 net |

### Deleted

- `review-results/browser.md` — stale

## 9. Testing Strategy

| Layer | Method | Coverage |
|---|---|---|
| `expand_home` | Unit tests | `$HOME` / `%USERPROFILE%`, multiple placeholders, Windows `/bin/python` → `\Scripts\python.exe` rewrite |
| `post_install::verify_or_repair` | Unit tests with tempdir | Fresh creation, already-present idempotent skip, failing repair returns `RepairFailed` |
| `uv` spec `AssetProbe` | `#[tokio::test]` end-to-end against a fake `uv` shim | Creates venv on first run, skips on second |
| `bootstrap_runtime` CLI | Integration tests with mocked SPECS | `--only`, `--skip`, `--force`, `--best-effort`, `--json`, exit codes 0/1/2/3 |
| `runtime_startup_warmup` | `#[tokio::test]` with mock ledger | Non-blocking, Missing flagged for supported-OS capabilities only, ledger persisted |
| `runtimes.install` RPC | Existing + new test | `failed` event carries non-empty `stderr` field |
| Panel Runtime page | Leptos component tests | Status-dot rendering per variant, button dispatch, progress event updates the log pane |
| Install scripts | `shellcheck` (sh) and `PSScriptAnalyzer` (ps1) in CI | Syntax; functional coverage deferred to manual VM test |
| End-to-end (`#[ignore]`, opt-in) | Fresh VM / container | `curl … | bash` → all four capabilities Ready → open `example.com` and screenshot succeeds |

## 10. Commit Sequence (single PR, 10 atomic commits)

1. `runtimes: expand $HOME / %USERPROFILE% in AssetProbe repair args`
2. `runtimes: auto-create global venv at ~/.aleph/.venv via uv post_install`
3. `runtimes: attach final stderr to RuntimeInstallProgressEvent on failure`
4. `runtimes: rewrite ensure_capability failure message with actionable hints`
5. `server: add 'bootstrap-runtime' CLI subcommand`
6. `server: runtime warmup probe on startup (non-blocking)`
7. `install.sh: invoke bootstrap-runtime with --best-effort + --skip-runtime`
8. `install.ps1: invoke bootstrap-runtime with --best-effort + -SkipRuntime`
9. `webchat: add Settings → Runtime page with step indicator + install log`
10. `webchat: add runtime summary banner to Browser page; drop playwright-mcp comment residuals; delete review-results/browser.md`

Each commit keeps the tree `cargo check`-green.

## 11. Out of Scope (YAGNI)

- Live line-by-line stdout/stderr streaming (ship after we have a real complaint)
- "Force reinstall from scratch" option clearing fnm / skills / venv directories
- Uninstall button or `bootstrap-runtime --uninstall` subcommand
- Per-capability upgrade UI; re-running bootstrap is the upgrade path
- Separate `bootstrap-state.json` file; ledger is the single truth
- OS-package-manager preference (brew / apt / dnf / winget) for fnm/node/uv — self-managed matches the original migration decision
- Smoke-test button ("open example.com")
- Proxy / custom npm-registry UI; users can set `HTTPS_PROXY` / `NPM_CONFIG_REGISTRY` env vars and re-run
- Telemetry on install success rates

## 12. Security Considerations

- fnm download, uv install, and `@playwright/cli` npm install all use HTTPS; no checksum verification v1 (tracked as future hardening, consistent with the 2026-04-12 decision)
- `npm install -g` happens inside the fnm-managed Node prefix — never root, never system-wide
- `bootstrap-runtime` inherits the parent process environment. It does **not** strip Aleph secrets because this CLI runs only during install or an explicit user-triggered action; production tool calls (which do strip) go through `PlaywrightCliDriver`, not this path.
- The generated `~/.aleph/.venv` is per-user and never world-writable (umask-default, inherits `~/.aleph` which is user-owned)

## 13. Success Criteria

1. `cargo check -p alephcore` and `cargo clippy -p alephcore -- -D warnings` pass.
2. `cargo test --lib` passes, including the new `uv` `AssetProbe` test, the CLI subcommand tests, and the warmup test.
3. Fresh macOS or Linux VM: `curl … | bash` completes; terminal shows a green step-indicator for each of fnm / node / uv / playwright-cli; `ls ~/.aleph/.venv/bin/python` and `~/.aleph/skills/playwright-cli/` both exist.
4. Fresh Windows machine: `irm … | iex` completes; corresponding paths exist under `%USERPROFILE%\.aleph\`.
5. Panel → Settings → Runtime shows all four managed capabilities as ● Ready plus git/cargo as detected; Browser page banner shows `✓ Browser runtime ready`.
6. Deleting `~/.local/share/fnm/node-versions/*`, then in Panel clicking *Refresh* flips node to ○ Missing; clicking *Install missing* returns it to Ready within a minute, log pane shows progress.
7. `ALEPH_SKIP_RUNTIME=1 bash install.sh` skips the bootstrap step; Panel shows missing capabilities with actionable hints.
8. Existing configs with `[playwright_mcp]` still deserialize cleanly (serde alias intact).
9. `grep -rn "playwright_mcp\|Playwright MCP" src/ interfaces/ examples/ Cargo.toml | grep -v 'alias = "playwright_mcp"' | grep -v 'old_playwright_mcp_toml'` returns empty.
