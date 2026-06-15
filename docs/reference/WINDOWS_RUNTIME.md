# Windows Runtime & Deployment

How `aleph-server` is deployed and operated on Windows. This is the runtime
(end-user / operator) counterpart to the build-from-source notes in
[CLAUDE.md → Windows 构建](../../CLAUDE.md). Behaviour documented here is
verified against the current code, not aspirational.

## Install

Two supported shapes (same daemon binary underneath):

| Shape | How | Where the binary lands |
|-------|-----|------------------------|
| **Full desktop App** | NSIS `.exe` / `.msi` installer from a GitHub Release | `%LOCALAPPDATA%\Aleph\` (Tauri shell + bundled `aleph-server.exe`) |
| **Standalone server** | `irm https://github.com/rootazero/Aleph/releases/latest/download/install.ps1 \| iex` | `%LOCALAPPDATA%\Aleph\aleph-server.exe` (added to user PATH) |

`install.ps1` mirrors the Unix `install.sh`: it resolves the
`aleph-server-x86_64-pc-windows-msvc.exe` release asset, drops it where the App
daemon also lives, stops any running instance before overwriting (Windows
cannot overwrite a running `.exe`), and prints start + LAN guidance. Only
`x86_64` ships a prebuilt server; arm64 must build from source.

## Data directory

All state lives under `%USERPROFILE%\.aleph` (resolved via `dirs::home_dir()`,
identical resolution on every platform): `config.toml`, `data/` (SQLite +
vault + `aleph.lock`), `logs/`.

## Running

```powershell
aleph-server start      # foreground; Ctrl+C to stop
aleph-server status     # report running state
aleph-server stop       # see caveat below
```

### Background / supervised operation

Windows has **no Unix double-fork daemon**. `--daemon` returns
`"Daemon mode is only supported on Unix systems"` by design (`daemonize()` is
`#[cfg(unix)]`; `fork`/`setsid`/`dup2` have no Windows equivalent). Run the
server in the background one of three ways instead:

1. **Full App** — the Tauri shell supervises `aleph-server.exe` for you and
   relaunches it on exit. This is the zero-config path.
2. **Task Scheduler** — register `aleph-server start` as a logon task for an
   unattended/server box.
3. **A service wrapper** (e.g. NSSM) — wrap `aleph-server start` as a Windows
   service.

> The agent-launched-GUI-into-Session-0 pitfall only affects the *App* (a
> windowed process started by an automated/service context is invisible to the
> interactive desktop). The standalone `aleph-server` is headless and unaffected.

### `stop` / `status` caveat on Windows

`stop` and `status` read the `--pid-file` (`~/.aleph/gateway.pid`), which is
written **only** by the Unix `daemonize()` path. A foreground or
supervisor-launched server on Windows therefore has no `gateway.pid`, so `stop`
reports *"no daemon running"* and does not terminate it. To stop such a server:

- Foreground: `Ctrl+C` in its terminal.
- Supervised: quit the App, or `taskkill /IM aleph-server.exe /F`.

The singleton lock (below) is the authoritative liveness record on Windows, and
its holder-PID liveness probe is now cross-platform
(`src/utils/process_alive.rs`, via `sysinfo`) — so stale-lock diagnostics are
accurate on Windows. Wiring `stop`/`status` to fall back to the lock's holder
PID is a tracked follow-up (see Gap Analysis).

## Singleton lock

Enforced by an OS-level `LockFileEx` exclusive lock on
`%USERPROFILE%\.aleph\data\aleph.lock` (`fs2`, acquired as the first action on
the `start` path — `main.rs`). A second `start` exits with code **64** and
prints the holder PID. The OS releases the lock on any process exit (normal,
panic, `taskkill /F`), so there is no stale lock after a hard kill and no sleep
is needed before restarting.

> On Windows `LockFileEx` is mandatory (not advisory like Unix `flock`), so the
> holder-PID *readback while the lock is held* is unavailable — only the mutual
> exclusion is. This is a diagnostic limitation, not a correctness one.

## Trust model (LAN-trust)

No auth step — the trust boundary is the network boundary. Default bind is
`127.0.0.1` (this machine only). To open the whole LAN, add to
`%USERPROFILE%\.aleph\config.toml`:

```toml
[gateway]
host = "0.0.0.0"
```

Any LAN device then gets full control of the agent (incl. PTY/shell). The only
protocol guardrail is WS Origin validation. See
[SECURITY.md#auth-ux](SECURITY.md#auth-ux).

## Refreshing the daemon binary (App installs)

Windows cannot overwrite a running `.exe`, so **stop first**:

```powershell
aleph-server stop   # or: taskkill /IM aleph-server.exe /F
Copy-Item target\release\aleph-server.exe "$env:LOCALAPPDATA\Aleph\aleph-server.exe" -Force
# restart Aleph.exe — the supervisor relaunches the new binary and reloads the webview
```

See [CLAUDE.md → Panel ↔ Daemon 资源嵌入链](../../CLAUDE.md) for the full
WASM → server-rebuild → relaunch chain (panel changes require recompiling the
server binary, since the panel is `rust_embed`-baked into it).

## Build from source

See [CLAUDE.md → Windows 构建](../../CLAUDE.md) for the one-time prerequisites
(MSVC build tools, WebView2, `protoc`, the wasm target, `wasm-bindgen-cli`
pinned to `Cargo.lock`, `cargo-tauri`, `just`, Git-for-Windows `usr\bin` on
PATH) and `just shell-build`.

---

## Gap Analysis & follow-ups (Windows runtime)

Snapshot of what was reviewed and what remains.

### Reference comparison — `openai/codex` `app-server-daemon`

Codex's daemon manager (`codex-rs/app-server-daemon/src/backend/pid.rs`) is the
closest reference for Aleph's daemon-lifecycle / singleton code. Two takeaways:

- **Aleph already surpasses it on portability.** Codex's entire PID backend is
  `#[cfg(unix)]`: `start`, `try_lock_file`, `process_matches_record`,
  `read_process_start_time` and `force_terminate_process_group` all `bail!`
  ("unsupported on this platform") / return `false` on Windows, and liveness
  shells out to `ps -p <pid> -o lstart=`. Aleph's `instance_lock` uses `fs2`
  (`LockFileEx` on Windows) + `sysinfo`, so the singleton works cross-platform
  with no subprocess.
- **One pattern worth adopting: PID-reuse resistance.** Codex stores
  `PidRecord { pid, process_start_time }` and re-checks the start time so a
  *recycled* PID isn't mistaken for the original process. Aleph's lock
  diagnostic previously used a bare liveness check. **Adopted and improved:**
  Aleph now records the holder's start time in `aleph.lock` and matches it via
  `sysinfo::Process::start_time()` — cross-platform (incl. Windows) and with no
  `ps` fork, surpassing the reference. Fail-safe + backward-compatible: legacy
  single-line lock files and platforms that don't report a start time fall back
  to the prior liveness-only behaviour.

(`openclaw/openclaw` — same product category, "Any OS. Any Platform" — was also
surveyed; its runtime is Node/Swift/Kotlin per-platform, not a shared Rust core,
so it offers product-shape parallels rather than directly portable core code.)

| Area | Status | Notes |
|------|--------|-------|
| Standalone server install on Windows | ✅ done | `scripts/install.ps1` added (was referenced by `cli.rs` but missing) |
| Cross-platform process liveness | ✅ done | `src/utils/process_alive.rs` unifies two `#[cfg]` checks that had opposite, both-wrong Windows fallbacks |
| PID-reuse-resistant lock diagnostic | ✅ done | `aleph.lock` now records holder start time; `process_matches` verifies it (mapped from Codex `PidRecord`, made cross-platform via `sysinfo`) |
| `bootstrap-runtime` parallelism | ⛔ rejected | `install()` mutates process-global `PATH` via `set_var`; parallelizing would race the env. Sequential install is the correct design. |
| `uv` venv post-install path (Windows) | ✅ verified correct | `expand_home` rewrites `/bin/python` → `\Scripts\python.exe` and expands repair args; no fix needed |
| `stop`/`status` for non-daemon Windows servers | ⏳ follow-up | Fall back to `instance_lock` holder PID + `sysinfo` terminate when `gateway.pid` is absent |
