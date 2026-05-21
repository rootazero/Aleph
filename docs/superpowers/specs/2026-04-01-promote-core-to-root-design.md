# Promote core/ to Project Root

**Date:** 2026-04-01
**Status:** Approved
**Scope:** Directory restructuring — eliminate `core/` wrapper, promote contents to root

---

## Problem

The `core/` directory wraps the primary crate `alephcore` but adds no architectural value. No other crate depends on `alephcore` via path — interfaces communicate through `shared/protocol` + `shared/client`. The extra nesting increases path length (`src/gateway/` vs `src/gateway/`) and adds cognitive overhead without providing compilation, dependency, or boundary benefits.

## Decision

**Approach A: Merge Cargo.toml** — promote all `core/` contents to root. The root `Cargo.toml` becomes both `[workspace]` and `[package]`. This is standard Rust practice (ripgrep, bat, helix all use this pattern).

## Dependency Topology (Unchanged)

```
                    alephcore (root)
                    crate-type = ["rlib", "cdylib"]
                    cfg(target_os) → desktop-{mac/lin/win}
                         │
                  depends on ──────────────┐
                         │                 │
                  shared/protocol    crates/desktop
                  shared/client      crates/logging
                         ▲
                  depends on
                         │
          ┌──────────────┼──────────────┐
    interfaces/cli  interfaces/tui  interfaces/webchat
    (MUST NOT depend on alephcore)   → shared/ui_logic
```

No crate reverse-depends on `alephcore`. Moving it changes zero dependency edges.

## Directory Structure: Before → After

```
# Before                           # After
Aleph/                             Aleph/
├── Cargo.toml  [workspace only]   ├── Cargo.toml  [workspace + package]
├── core/                          ├── build.rs
│   ├── Cargo.toml  [package]      ├── src/
│   ├── build.rs                   │   ├── lib.rs
│   ├── src/ (70 modules)          │   ├── bin/aleph-server/
│   ├── tests/ (30+ Rust tests)    │   └── ...70 modules
│   ├── benches/                   ├── tests/        (merged: Rust + Playwright)
│   ├── examples/                  ├── benches/
│   └── docs/plans/                ├── examples/
├── crates/                        ├── bindings/
├── shared/                        ├── proptest-regressions/
├── interfaces/                    ├── config.search.example.toml
├── tests/ (Playwright)            ├── crates/       (unchanged)
└── docs/                          ├── shared/       (unchanged)
                                   ├── interfaces/   (unchanged)
                                   └── docs/         (merged: + plans/)
```

## Cargo.toml Merge Strategy

Root `Cargo.toml` gains `[package]`, `[dependencies]`, `[lib]`, `[[bin]]`, `[features]`, `[dev-dependencies]`, `[[bench]]`, `[[test]]` sections from `Cargo.toml`.

### Path dependency changes (7 entries)

| Dependency | Before (relative to core/) | After (relative to root) |
|------------|---------------------------|--------------------------|
| aleph-protocol | `../shared/protocol` | `shared/protocol` |
| aleph-logging | `../crates/logging` | `crates/logging` |
| aleph-desktop | `../crates/desktop` | `crates/desktop` |
| aleph-client | `../shared/client` | `shared/client` |
| aleph-desktop-macos | `../crates/desktop-macos` | `crates/desktop-macos` |
| aleph-desktop-linux | `../crates/desktop-linux` | `crates/desktop-linux` |
| aleph-desktop-windows | `../crates/desktop-windows` | `crates/desktop-windows` |

### Workspace members

Remove `"core"` from `[workspace] members`. Cargo automatically includes the root when `[package]` is present alongside `[workspace]`.

### Other crates' Cargo.toml

**No changes needed.** `interfaces/cli`, `shared/client`, `crates/desktop-*` etc. use paths relative to their own location (`../../shared/protocol`, `../desktop`). Since these crates don't move, their paths remain valid.

## build.rs Path Updates

All `../` prefixes removed (file moves from `core/` to root):

| Before | After |
|--------|-------|
| `../VERSION` | `VERSION` |
| `../interfaces/webchat` | `interfaces/webchat` |
| `../interfaces/webchat/dist` | `interfaces/webchat/dist` |
| `../interfaces/webchat/src` | `interfaces/webchat/src` |
| `../interfaces/webchat/Cargo.toml` | `interfaces/webchat/Cargo.toml` |
| `../interfaces/webchat/index.html` | `interfaces/webchat/index.html` |

## Directory Merge Details

### tests/ — Direct merge, no conflicts

Root `tests/` contains Playwright files (`.spec.ts`). `tests/` contains Rust integration tests (`.rs`). Different file types, no name collisions. Cargo only processes `.rs`, Playwright only processes `.spec.ts`.

### docs/ — Merge core/docs/plans/ into docs/plans/

Root `docs/` has no `plans/` subdirectory. The 3 markdown files from `core/docs/plans/` move in cleanly.

### .claude/ — Inspect and merge or discard

Check `core/.claude/` contents. If crate-specific config, merge into root `.claude/`. If outdated, discard.

## CI/CD Updates

### .github/workflows/aleph-core-ci.yml

| Section | Before | After |
|---------|--------|-------|
| Push path filter | `core/**` | `src/**`, `tests/**`, `build.rs`, `Cargo.toml`, `Cargo.lock` |
| PR path filter | `core/**` | Same as push |
| rust-cache workspaces | `core` | Remove (defaults to root) |

### justfile

No changes needed — all commands use `-p alephcore` package name, not directory paths.

## Documentation Updates

Global find-and-replace across 60+ files in `docs/`:

| Pattern | Replacement |
|---------|-------------|
| `src/` | `src/` |
| `Cargo.toml` | `Cargo.toml` |
| `tests/` | `tests/` |
| `build.rs` | `build.rs` |
| `benches/` | `benches/` |

### Specific files

- **CLAUDE.md** — Update redline R1 (`core/src` → `src`), R3 (`core` → root context)
- **README.md** — Update project structure diagram

## What Does NOT Change

- **Crate name**: `alephcore` — all `use alephcore::` statements unchanged
- **Crate boundaries**: Every crate remains separate, same compilation units
- **Dependency directions**: Interface → shared → (no reverse to core)
- **Platform isolation**: `cfg(target_os)` works identically
- **Incremental builds**: Same crate graph, same incremental behavior
- **Feature flags**: `loom`, `test-helpers`, `control-plane` unchanged
- **Binary name**: `aleph-server` unchanged
- **Package name references**: `-p alephcore` in justfile/CI unchanged

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Git loses rename tracking | Low | Use `git mv` for all moves |
| Compilation cache invalidated | Certain | One-time full rebuild, then normal incremental |
| Missed path reference | Medium | `grep -r 'core/src\|core/Cargo\|core/tests\|core/build' .` after completion |
| `cdylib` + workspace root quirks | Very low | Test `cargo doc --workspace` after merge |

## Verification

After implementation:
1. `cargo check` — compilation succeeds
2. `cargo test -p alephcore --lib` — unit tests pass
3. `cargo test -p alephcore` — integration tests pass
4. `cargo build --bin aleph-server` — binary builds
5. `grep -r 'core/src' docs/` — no stale references remain
6. `grep -r '"\.\./' Cargo.toml` — no leftover `../` in root Cargo.toml
