# Rename crates/ to desktop/ and Reorganize

**Date:** 2026-04-01
**Status:** Approved
**Scope:** Directory restructuring — rename `crates/` to `desktop/`, reorganize internal naming, move logging to shared/

---

## Problem

The `crates/` directory name is generic and doesn't convey that its contents are desktop platform extensions. The internal naming (`desktop/`, `desktop-macos/`, etc.) is redundant when nested under a parent that already says "desktop". The `logging` crate doesn't belong here — it's shared infrastructure used by core, CLI, and TUI.

## Decision

Rename `crates/` to `desktop/`, simplify internal names, move `logging` to `shared/`.

## Directory Structure: Before -> After

```
# Before                        # After
crates/                         desktop/
├── desktop/            →       ├── shared/     (cross-platform shared code)
├── desktop-macos/      →       ├── macos/
├── desktop-linux/      →       ├── linux/
├── desktop-windows/    →       └── windows/
└── logging/            →       shared/logging/ (joins protocol, client, ui_logic)
```

## Crate Names (Unchanged)

| New path | Crate name (unchanged) |
|----------|----------------------|
| `desktop/shared/` | `aleph-desktop` |
| `desktop/macos/` | `aleph-desktop-macos` |
| `desktop/linux/` | `aleph-desktop-linux` |
| `desktop/windows/` | `aleph-desktop-windows` |
| `shared/logging/` | `aleph-logging` |

All `use aleph_desktop::` statements and `-p aleph-desktop` commands remain unchanged.

## Path Reference Changes

### Root Cargo.toml — workspace members

```toml
# Before                          # After
"crates/desktop",          →      "desktop/shared",
"crates/desktop-macos",    →      "desktop/macos",
"crates/desktop-linux",    →      "desktop/linux",
"crates/desktop-windows",  →      "desktop/windows",
"crates/logging",          →      "shared/logging",
```

### Root Cargo.toml — [dependencies]

```toml
aleph-desktop = { path = "desktop/shared" }     # was crates/desktop
aleph-logging = { path = "shared/logging" }      # was crates/logging
```

### Root Cargo.toml — [target.cfg dependencies]

```toml
aleph-desktop-macos = { path = "desktop/macos" }     # was crates/desktop-macos
aleph-desktop-linux = { path = "desktop/linux" }      # was crates/desktop-linux
aleph-desktop-windows = { path = "desktop/windows" }  # was crates/desktop-windows
```

### Desktop crates internal references

`desktop/macos/Cargo.toml`, `desktop/linux/Cargo.toml`, `desktop/windows/Cargo.toml` each depend on `aleph-desktop`:

```toml
# Before: path = "../desktop"
# After:  path = "../shared"
aleph-desktop = { path = "../shared" }
```

### interfaces/cli and interfaces/tui

```toml
# Before: path = "../../crates/logging"
# After:  path = "../../shared/logging"
aleph-logging = { path = "../../shared/logging" }
```

### CI workflow (.github/workflows/aleph-core-ci.yml)

Update path filters if `crates/**` is referenced — replace with `desktop/**`.

### Documentation

Global replace `crates/desktop` → `desktop/shared`, `crates/logging` → `shared/logging`, etc. in all markdown files.

## What Does NOT Change

- Crate names: `aleph-desktop`, `aleph-desktop-macos`, etc.
- All `use aleph_desktop::` code
- `-p aleph-desktop` in justfile/CI
- `shared/protocol`, `shared/client`, `shared/ui_logic` — untouched

## Verification

1. `cargo check` — compilation succeeds
2. `cargo test -p aleph-desktop --lib` — tests pass
3. `cargo check -p aleph-desktop-macos` — platform crate compiles
4. `grep -r 'crates/' Cargo.toml` — no stale references
5. `grep -r 'crates/desktop' docs/` — no stale doc references
