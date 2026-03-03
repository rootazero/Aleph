# Log Unification Design

> Unify all Aleph component logs under `~/.aleph/logs/` with per-component file prefixes.

**Date**: 2026-03-03
**Status**: Approved

---

## Problem

Currently only the Core Server writes file logs to `~/.aleph/logs/`. Tauri Desktop and CLI output only to stdout/stderr, making debugging multi-component issues difficult.

## Decision

**Approach A: Shared API in alephcore** — add `init_component_logging(component, retention_days, default_filter)` to `alephcore::logging`, reused by all components.

## File Naming

```
~/.aleph/logs/
├── aleph-server.log.YYYY-MM-DD
├── aleph-tauri.log.YYYY-MM-DD
└── aleph-cli.log.YYYY-MM-DD
```

## API Design

### New Public API

```rust
/// Initialize file + console logging for a named component.
///
/// - Log file: `~/.aleph/logs/aleph-{component}.log.YYYY-MM-DD`
/// - Daily rotation via tracing-appender
/// - PII scrubbing on both console and file output
/// - Automatic cleanup of files older than `retention_days`
pub fn init_component_logging(
    component: &str,
    retention_days: u32,
    default_filter: &str,
) -> Result<(), Box<dyn std::error::Error>>
```

### Backward Compatibility

```rust
pub fn init_file_logging() -> Result<(), Box<dyn std::error::Error>> {
    init_component_logging("server", 7, "info")
}
```

## Component Integration

### Core Server

Existing `init_file_logging()` calls are unchanged. Internally delegates to `init_component_logging("server", 7, "info")`.

### Tauri Desktop (`apps/desktop/src-tauri/src/lib.rs`)

```rust
if let Err(e) = alephcore::logging::init_component_logging("tauri", 7, "aleph_tauri=debug,tauri=info") {
    eprintln!("Failed to init logging: {e}");
    // fallback to console-only
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "aleph_tauri=debug,tauri=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}
```

**Dependency**: Add `alephcore` to Tauri's `Cargo.toml`.

### CLI (`apps/cli/src/main.rs`)

```rust
let default_filter = if cli.verbose { "debug" } else { "info" };
if let Err(e) = alephcore::logging::init_component_logging("cli", 7, default_filter) {
    eprintln!("Failed to init file logging: {e}");
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::new(default_filter))
        .init();
}
```

## Log Retention

Each component cleans up only its own log files on initialization:
- Pattern: `aleph-{component}*.log*`
- Default: 7 days retention (clamped to 1-30)
- Runs after logging subscriber is initialized

## Shared Features

All components get:
- PII scrubbing (email, phone, SSN, credit card, API keys)
- Daily rotation
- Non-blocking async file writer
- `RUST_LOG` environment variable override
- Configurable default filter level

## Files to Modify

| File | Change |
|------|--------|
| `core/src/logging/file_appender.rs` | Add `init_component_logging()`, refactor `setup_logging()` to accept component name and default filter |
| `core/src/logging/mod.rs` | Export `init_component_logging` |
| `core/src/logging/retention.rs` | Adapt cleanup to accept component-specific file prefix pattern |
| `apps/desktop/src-tauri/Cargo.toml` | Add `alephcore` dependency |
| `apps/desktop/src-tauri/src/lib.rs` | Replace console-only logging with `init_component_logging("tauri", ...)` |
| `apps/cli/src/main.rs` | Replace console-only logging with `init_component_logging("cli", ...)` |

## Debug Usage

```bash
# All today's logs
tail -f ~/.aleph/logs/aleph-*.log.$(date +%Y-%m-%d)

# Server only
tail -f ~/.aleph/logs/aleph-server.log.*

# Search errors across all components
grep -r "ERROR" ~/.aleph/logs/

# Last 3 days of Tauri logs
ls -la ~/.aleph/logs/aleph-tauri.log.*
```
