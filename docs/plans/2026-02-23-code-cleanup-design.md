# Code Cleanup Design: Occam's Razor Pass

> Date: 2026-02-23
> Status: Approved
> Scope: Full cleanup — 280+ issues across 50+ files
> Strategy: Risk-layered incremental (3 passes)
> Guardrails: Zero functional changes, preserve defensive code, no over-engineering

---

## Motivation

Clippy analysis reveals ~280+ code quality issues across the `core/` crate. These include dead code, unused imports, redundant closures, overly complex function signatures, and inefficient patterns. Left untreated, this technical debt compounds and degrades readability and maintainability.

## Guardrails (Iron Rules)

1. **Zero functional modification** — Input/output must be identical to original code
2. **Preserve defensive programming** — Do not remove try/catch, null checks, or error handling unless provably unreachable
3. **No over-engineering** — No new abstractions, patterns, or layers unless explicitly part of the plan

## Strategy: 3-Pass Risk-Layered Cleanup

Each pass is followed by `cargo build && cargo test` verification. If a pass introduces regressions, it is rolled back independently.

---

## Pass 1: Mechanical Cleanup (Lowest Risk)

**Goal**: Eliminate all compiler/clippy auto-detectable issues. Pure syntax-level changes.

| # | Type | Count | Action | Example |
|---|------|-------|--------|---------|
| 1 | Unused imports | ~40 | Delete | `use std::sync::Arc;` (unused) |
| 2 | Derivable impls | ~10 | Replace with `#[derive(Default)]` | Manual `impl Default for Role` → derive |
| 3 | Redundant closures | ~14 | `\|x\| f(x)` → `f` | `.map(\|s\| s.to_string())` → `.map(ToString::to_string)` |
| 4 | Boolean simplification | ~3 | Use `is_some_and()` / `is_empty()` | `.map_or(false, \|x\| ...)` → `.is_some_and(\|x\| ...)` |
| 5 | No-op operations | ~2 | Remove redundant bit ops | `(random >> 16) as u16 & 0xFFFF` → `(random >> 16) as u16` |
| 6 | Path references | ~10 | `&PathBuf` → `&Path` | Function signature optimization |

**Verification**: `cargo build && cargo test`

---

## Pass 2: Local Refactoring (Low Risk)

**Goal**: Eliminate dead code, duplicate logic, and inefficient patterns. Changes stay within single function/method scope.

| # | Type | Count | Action | Example |
|---|------|-------|--------|---------|
| 1 | Unused vars/functions | ~20 | Delete or prefix `_` | `clear()` method never called → delete |
| 2 | Identical code blocks | ~2 | Merge conditions | `"- "` and `"* "` same handling → merge with `\|\|` |
| 3 | Clone inefficiency | ~15 | `vec![x.clone()]` → `std::slice::from_ref(&x)` | Avoid unnecessary heap allocation |
| 4 | Field assignment simplification | ~77 | Struct update syntax | `Config { x: 1, ..Default::default() }` |
| 5 | Clippy misc | ~30 | Various small fixes | `strip_prefix` replacing manual string slicing |
| 6 | Redundant comments | as needed | Delete comments that restate code | `// create a new instance` on `::new()` |

**Rules for unused function deletion**:
- Only delete functions with **zero references** (confirmed via grep/IDE)
- Test-only functions in `#[cfg(test)]` are retained if used by tests
- Public API functions are retained even if currently unused

**Verification**: `cargo build && cargo test && cargo clippy`

---

## Pass 3: Structural Refactoring (Medium Risk)

**Goal**: Improve cross-function/cross-file structure. Each change requires careful call-site updates.

### 3a. Multi-Parameter Functions → Config Structs (~10 functions)

Extract parameters into dedicated structs for functions with 8+ parameters.

```rust
// Before: gateway/security/store.rs:309 — 11 parameters
fn save_device(&self, id: &str, name: &str, os: &str, version: &str,
               platform: &str, paired: bool, token: &str, ...)

// After:
struct DeviceInfo { id: String, name: String, os: String, ... }
fn save_device(&self, info: &DeviceInfo)
```

**Affected modules**: gateway/security, memory/store, poe/services, gateway/server, gateway/handlers/auth

### 3b. Complex Types → Type Aliases (~13 instances)

```rust
// Before: gateway/channel_registry.rs:40
channels: RwLock<HashMap<ChannelId, Arc<RwLock<Box<dyn Channel>>>>>

// After:
type ChannelHandle = Arc<RwLock<Box<dyn Channel>>>;
channels: RwLock<HashMap<ChannelId, ChannelHandle>>
```

### 3c. Arc Misuse Fix (~25 instances)

Only fix cases where Arc wraps non-Send/Sync types and cross-thread sharing is **provably unnecessary**. If uncertain, retain Arc.

### 3d. Large Error Variants → Box (~4 instances)

Box-ify large error enum variants to reduce Result size on the success path.

**Verification**: `cargo build && cargo test && cargo clippy && cargo doc`

---

## High-Issue-Density Files (Priority Targets)

| Rank | Module | Files | Issue Count |
|------|--------|-------|-------------|
| 1 | Memory Store | lance/{facts,graph,sessions}.rs | 15+ |
| 2 | Gateway Security | security/store.rs | 12+ |
| 3 | Config Types | types/dispatcher/mod.rs | 77 |
| 4 | Daemon Policies | policies/{battery,cpu,focus,idle}.rs | 10+ |
| 5 | POE Services | services/run_service.rs | 5+ |
| 6 | Thinker | soul.rs | 3+ |

---

## Success Criteria

- [ ] `cargo build` passes with zero errors
- [ ] `cargo test` passes with zero regressions
- [ ] `cargo clippy` warnings reduced by ≥80%
- [ ] No functional behavior changes (input/output identical)
- [ ] Each pass committed separately for easy rollback
