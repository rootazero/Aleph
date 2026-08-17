# Severed-Wire Audit — `src/logging`

- Date: 2026-08-17
- Tree: `/home/zou/data/workspace/Aleph/.worktrees/review-fix-2026-08-17` (HEAD newer than the graphify graph at 9841b5b2; all claims re-verified with `rg`)
- Module: `src/logging` (4 files, 275 LOC: `mod.rs`, `error.rs`, `file_appender.rs`, `level_control.rs`)
- Method: PRODUCED–CONSUMED symbol parity (`rg` across `src/` incl. `src/bin/`, `shared/`, `interfaces/`, `desktop/`). No cargo runs. Read-only.
- Existing prior review refs: none found under `review-results/` that cover `src/logging` (checked `review-results/` listing; only `REVIEW_PROTOCOL.md` and other modules' dirs).

## Wiring verdicts (questions posed by the audit task)

1. **Is `file_appender` installed as a tracing layer in production? NO — and it never could be.**
   `src/logging/file_appender.rs` contains no appender; it is a 2-line path shim:
   `pub fn get_log_directory() -> Result<PathBuf, LoggingError>` delegating to
   `aleph_logging::get_log_directory()` (`src/logging/file_appender.rs:7-8`). The real
   `RollingFileAppender` + reload/EnvFilter layers live in `shared/logging/src/file_appender.rs:44-116`
   (`init_component_logging` → `setup_logging`), and the production init call site is
   `src/bin/aleph-server/commands/start/helpers.rs:154`:
   `aleph_logging::init_component_logging("server", 7, &filter)`. The `mod.rs:3-5` doc already
   acknowledges the split. So this is *documented layering*, not an unwired appender — but the
   module name is legacy residue (see sw-lo-04).

2. **Does `level_control` integrate with the server's log-level control? YES — fully wired.**
   - JSON-RPC `logs.getLevel` / `logs.setLevel` / `logs.getDirectory` handlers registered at
     `src/gateway/handlers/mod.rs:264-266`, routed to the System lane (`src/gateway/lane.rs:144-146`,
     `method_admin.rs:192,624`).
   - `src/gateway/handlers/logs.rs` is the only production consumer: `get_log_level()` (:26),
     `LogLevel::parse` (:60), `set_log_level` (:74), `to_filter_string` (:30, :80), `get_log_directory` (:99).
   - `set_log_level` pushes through to the live filter via `aleph_logging::set_log_level`
     (`src/logging/level_control.rs:125` → `shared/logging/src/file_appender.rs:57-66`, an
     `EnvFilter` reload). This is real runtime control, not a stub.
   - Secondary consumers of `get_log_directory`: `src/gateway/handlers/daemon_control.rs:12,126`
     (latest-log lookup for daemon attach) and `src/extension/watcher.rs:208` (excludes the logs
     dir from extension watch).

3. **Is the module shadowed by `shared/logging`? Partially — thin shim + parallel state (see sw-lo-04, sw-lo-05).**
   Path resolution is *not* duplicated (`src/logging/file_appender.rs:8` delegates; single source of
   truth at `shared/logging/src/file_appender.rs:158`). But log-level *state* IS duplicated: the
   `AtomicU8` mirror + its own `RUST_LOG` parser in `level_control.rs` vs. the authoritative
   `EnvFilter` in `shared/logging`.

## Findings

### sw-lo-01 — Dead variants `LoggingError::Init` and `LoggingError::Cleanup`

- **Form**: 1 (visible symbol with zero production consumers)
- **Severity**: low
- **Produced**: `LoggingError::Init` — `src/logging/error.rs:9`; `LoggingError::Cleanup` — `src/logging/error.rs:17`
- **Consumers**: none. Only variant constructed anywhere:
  ```
  $ rg -n "LoggingError::" src/ shared/ interfaces/
  src/logging/file_appender.rs:8:    aleph_logging::get_log_directory().map_err(|e| LoggingError::LogDirectory(e.into()))
  ```
  `Init` (init-failure, per doc) and `Cleanup` (log cleanup failure) have no producer and no consumer.
- **Rationale**: The enum's `LogDirectory` variant is live (return type of the shim at `file_appender.rs:7-8`,
  whose callers at `logs.rs:99`, `daemon_control.rs:126`, `watcher.rs:208` only display/discard the error).
  `Init` and `Cleanup` are leftovers from an earlier design where this module initialized logging itself;
  that work now lives in `shared/logging`.
- **Proposed change**: delete `error.rs:9` (`Init`) and `error.rs:17` (`Cleanup`), leaving the
  `LogDirectory` variant. Keep `#[non_exhaustive]` (semver guard). No behavioral change — nothing
  constructs or matches them today.
- **Risk**: nil at runtime. `#[non_exhaustive]` means out-of-repo matchers already require a wildcard arm;
  no in-repo or `desktop/` code references `LoggingError` at all (see sw-lo-02 evidence).
- **Verification**: `rg -n "LoggingError::(Init|Cleanup)" src/ shared/ interfaces/ desktop/` → no matches;
  `cargo clippy -p alephcore` passes (not run here per constraints).

### sw-lo-02 — Orphaned re-export `pub use error::LoggingError`

- **Form**: 6 (pub item re-exported but never consumed outside the module)
- **Severity**: low
- **Produced**: `pub use error::LoggingError;` — `src/logging/mod.rs:10` (re-exported surface of `pub mod error;` `mod.rs:6`)
- **Consumers**: only in-module import at `file_appender.rs:4` (`use crate::logging::LoggingError;`).
  Repo-wide:
  ```
  $ rg -n "LoggingError" src/ shared/ interfaces/ desktop/
  src/logging/file_appender.rs:4
  src/logging/file_appender.rs:7
  src/logging/file_appender.rs:8
  src/logging/error.rs:6
  src/logging/mod.rs:10
  ```
  `src/lib.rs` does not re-export it (only `LogLevel`, `lib.rs:166`). No external crate names the type.
- **Rationale**: The type is reachable only as the error half of `get_log_directory()`'s return type;
  all three callers handle it via `match`/`unwrap_or_else(|_|)`/`.ok()` without naming it. The `pub use`
  line exists solely to serve the in-module import at `file_appender.rs:4`, which could equally import
  `crate::logging::error::LoggingError`. The pub API surface (mod.rs:10 + the pub enum) is effectively
  internal.
- **Proposed change**: DECIDE — two options: (a) drop the `pub use` at `mod.rs:10` and change
  `file_appender.rs:4` to `use crate::logging::error::LoggingError;` (the enum must stay `pub` while
  `get_log_directory` is `pub` — a `pub fn` may not expose a `pub(crate)` return type); (b) keep as-is
  for library-API stability. No runtime difference either way.
- **Risk**: (a) is a (tiny) public-API removal for out-of-repo consumers of
  `alephcore::logging::LoggingError`; none exist in-tree.
- **Verification**: the `rg` above; after (a): `rg -n "logging::LoggingError" src/` → only
  `file_appender.rs:4` pointing at `error::LoggingError`.

### sw-lo-03 — Orphaned crate-root re-export `pub use crate::logging::LogLevel` (lib.rs:166)

- **Form**: 6 (orphaned pub API surface)
- **Severity**: low
- **Produced**: `pub use crate::logging::LogLevel;` — `src/lib.rs:166`
- **Consumers**: none, repo-wide, including other workspace crates and `desktop/`:
  ```
  $ rg -n "LogLevel" desktop/ | wc -l
  0
  $ rg -n "alephcore::LogLevel|crate::LogLevel" src/ shared/ interfaces/ desktop/
  (no matches — the only `LogLevel` refs outside src/logging are the gateway's
   `crate::logging::LogLevel` import at src/gateway/handlers/logs.rs:10)
  ```
  Production `LogLevel` consumers import `crate::logging::LogLevel` directly (`logs.rs:10`), never the
  crate-root path.
- **Rationale**: A second, unused export path for the same type. Harmless, but it is exactly the
  "pub item re-exported but unused" form 6. Gate: is `alephcore::LogLevel` part of the intended public
  API for external shells (desktop/CLI)? No in-tree consumer exists.
- **Proposed change**: DECIDE — (a) delete `lib.rs:166` if no external consumer is planned (CLI/TUI
  would import from `alephcore::logging` or the RPC instead); (b) keep as intentional library API.
- **Risk**: removing a crate-root re-export only breaks out-of-repo consumers; none found in-tree.
- **Verification**: `rg -n "alephcore::LogLevel" .` → no matches.

### sw-lo-04 — Name-drift: `file_appender` module contains no file appender

- **Form**: 5 (name describes a reality that no longer exists)
- **Severity**: low
- **Produced**: module `src/logging/file_appender.rs` + its doc `//! Log file appender helpers — delegates to
  `aleph-logging` crate` (`file_appender.rs:1`); content is only `get_log_directory` (`file_appender.rs:7-8`).
- **Consumers** of the module path: `src/extension/watcher.rs:208`
  (`crate::logging::file_appender::get_log_directory().ok()`) and a comment reference at
  `src/gateway/handlers/daemon_control.rs:155`. The re-export `mod.rs:11` is consumed by
  `logs.rs:10,99` and `daemon_control.rs:12,126`.
- **Evidence of drift**: the only appender in the workspace is `RollingFileAppender`
  (`shared/logging/src/file_appender.rs:64`), installed via
  `aleph_logging::init_component_logging` (`src/bin/aleph-server/commands/start/helpers.rs:154`).
  The `src/logging` module never creates or installs an appender.
- **Rationale**: The name + module doc describe an appender helper that no longer exists in this crate;
  what remains is a log-directory shim. The mod.rs:3-5 doc already describes the reality, so this is
  residue, not a functional gap.
- **Proposed change**: DECIDE — (a) rename module (e.g. `log_directory` or fold into `level_control`
  as a `paths` helper), touching `watcher.rs:208`, the `mod.rs:11` re-export, and the `daemon_control.rs:155`
  comment; (b) keep the name as a compatibility shim and only fix the module doc. Both are cosmetic;
  no runtime behavior changes.
- **Risk**: rename touches 3 sites; low, mechanical. Keep-or-rename is a naming-contract question for a
  lib crate — hence DECIDE, not CUT.
- **Verification**: `rg -n "file_appender" src/` → `mod.rs:7,11`, `file_appender.rs:1`, `watcher.rs:208`,
  `daemon_control.rs:155` (comment only).

### sw-lo-05 — Parallel log-level state drift: `level_control` atomic mirror vs `shared/logging` EnvFilter

- **Form**: 5 (duplicated state describing/implying the same reality as the authoritative backend, and can diverge)
- **Severity**: medium
- **Produced**: `CURRENT_LOG_LEVEL: AtomicU8` + `init_log_level()`'s private `RUST_LOG` parser
  (`src/logging/level_control.rs:80,86-103`) — a second, independent source of truth for the process log level,
  alongside the authoritative `reload::Handle<EnvFilter>` in `shared/logging/src/file_appender.rs:26,57-66,84-86`.
- **Consumers**: `get_log_level`/`set_log_level` (`level_control.rs:106-133`), exported to the gateway
  (`logs.rs:26,74`) and crate root (`lib.rs:166`, sw-lo-03).
- **Evidence of divergence**:
  - `init_log_level` parses `RUST_LOG` as `split(',').next()` then `split('=').next_back()`
    (`level_control.rs:90-97`), i.e. it reports only the *first* directive. `RUST_LOG="warn,alephcore=debug"`
    reports `warn` while the real EnvFilter (`shared/logging/src/file_appender.rs:86`,
    `EnvFilter::try_from_default_env`) applies `alephcore=debug`. The doc at `level_control.rs:106-108`
    claims the reported level "matches the EnvFilter the logging backend actually uses" — false for any
    multi-directive `RUST_LOG` (and the CLI already builds a multi-directive filter:
    `aleph_server={lvl},alephcore={lvl}` at `helpers.rs:153`).
  - `set_log_level` stores the atomic (`level_control.rs:124`) **before** calling
    `aleph_logging::set_log_level` (`:125`) and treats failure as non-fatal (`:126-127`); if reload is
    unavailable (logging not initialized), `get_log_level()` reports the new level while the live filter
    never changed — silent divergence. In the server this is unlikely (gateway starts after
    `initialize_tracing`), but the invariant is unenforced.
- **Rationale**: The atomic mirror duplicates backend state and can report a level that differs from the
  effective filter. This is the parallel-implementation drift between `src/logging` and the
  `shared/logging` crate. Not dead code, and not currently causing a visible production bug — hence DECIDE.
- **Proposed change**: DECIDE — options: (a) make `level_control` a pure facade: drop
  `CURRENT_LOG_LEVEL`/`init_log_level` and have `get_log_level` read the real filter (requires
  `shared/logging` to expose a getter over `FILTER_RELOAD`, e.g. `EnvFilter` → `max_level_hint`),
  eliminating drift entirely; (b) keep the mirror but fix `init_log_level` to parse the *highest*
  directive for the `alephcore`/`aleph_server` targets and to roll back the atomic store when
  `aleph_logging::set_log_level` fails; (c) accept and document the coarse-grained reporting. Option (a)
  is the architecture-clean fix (R3/R10-style single source of truth).
- **Risk**: (a) changes `get_log_level` semantics slightly (level becomes filter-derived); needs a new
  `pub fn` in `shared/logging` (small, additive). (b) is behavior-neutral. (c) free.
- **Verification**: `rg -n "CURRENT_LOG_LEVEL|init_log_level" src/` → all inside `level_control.rs`
  (mirror is fully self-contained); after (a): `rg -n "CURRENT_LOG_LEVEL" src/` → no matches.

## Explicitly skipped / noted, not findings

- `init_log_level` is `pub(crate)` with zero callers outside its own module, but it IS consumed lazily
  by `get_log_level`/`set_log_level` (`level_control.rs:110,122`) — wired, not dead. The `pub(crate)`
  keyword hints at an intended startup seeder that never materialized; harmless.
- `LogLevel::to_filter_string`/`parse`/`to_u8`/`from_u8`: all consumed (gateway + internal). Live.
- `CURRENT_LOG_LEVEL`/`INIT` statics: internal, consumed. Live.
- Tests in `file_appender.rs`/`level_control.rs`: test-only consumers of otherwise-live symbols (form 4
  not triggered because production consumers exist). The `file_appender` test helper
  `crate::utils::scratch::scratch_root` interplay is test infrastructure, out of scope.
- `#[repr(C)]` on `LogLevel` (level_control.rs:15): oddity but not a severed wire; style/FFI question, skipped.
- No `#[allow(dead_code)]` or `#[deprecated]` items in the module (`rg` confirmed zero).
- `bin/` (i.e. `src/bin/`) checked as part of `src/` sweeps; `interfaces/`, `shared/`, `desktop/` swept.

## Summary

| id | produced | form | severity | decision |
|----|----------|------|----------|----------|
| sw-lo-01 | `LoggingError::Init`, `LoggingError::Cleanup` | 1 | low | CUT |
| sw-lo-02 | `pub use error::LoggingError` (mod.rs:10) | 6 | low | DECIDE |
| sw-lo-03 | `pub use crate::logging::LogLevel` (lib.rs:166) | 6 | low | DECIDE |
| sw-lo-04 | module name/doc `file_appender` (no appender) | 5 | low | DECIDE |
| sw-lo-05 | `CURRENT_LOG_LEVEL`/`init_log_level` mirror vs EnvFilter | 5 | medium | DECIDE |

Counts: 5 findings — 1 medium, 4 low; 1 CUT, 4 DECIDE; 0 CONNECT; 0 critical/high.
