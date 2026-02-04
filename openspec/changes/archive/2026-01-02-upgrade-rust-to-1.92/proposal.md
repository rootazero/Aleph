# Change Proposal: upgrade-rust-to-1.92

## Metadata
- **ID**: upgrade-rust-to-1.92
- **Title**: Upgrade Rust to 1.92 and Migrate to Stdlib Features
- **Type**: Technical Upgrade / Dependency Reduction
- **Status**: Deployed
- **Created**: 2026-01-02
- **Deployed**: 2026-01-02

## Why

Upgrading to Rust 1.92 and migrating to standard library features addresses three critical concerns:

1. **Reduce Dependency Bloat**: External crates like `once_cell` and `async-trait` add unnecessary build time and binary size now that their functionality is in stdlib.

2. **Improve Code Readability**: Removing macro attributes like `#[async_trait]` (7 occurrences) reduces visual noise and makes trait definitions clearer.

3. **Future-Proof Codebase**: Rust 1.92 brings performance optimizations and modern idioms (C string literals, native async traits) that will benefit future development. UniFFI 0.28 leverages these features for better FFI performance.

4. **Align with Best Practices**: Using stdlib features over external crates is recommended by the Rust ecosystem for foundational primitives.

## What Changes

This change upgrades the Rust codebase to version 1.92 and migrates from external crates to standard library equivalents:

**Dependency Changes**:
- Update MSRV from 1.70 to 1.92 in `Cargo.toml`
- Upgrade UniFFI from 0.25 to 0.28.3
- Remove `async-trait = "0.1"` dependency (replaced by native async fn in traits)
- Remove `once_cell = "1.19"` dependency (replaced by `std::sync::OnceLock`)

**Code Changes**:
- `memory/embedding.rs`: Replace `OnceCell` with `std::sync::OnceLock`
- All provider files (`openai.rs`, `claude.rs`, `ollama.rs`, `mock.rs`, `mod.rs`):
  - Remove `use async_trait::async_trait` imports
  - Remove `#[async_trait]` attributes (7 total occurrences)
  - Add `use std::future::Future` and `use std::pin::Pin` imports
  - Update async method signatures to return `Pin<Box<dyn Future<...>>>`
  - Add explicit parameter cloning before async move blocks
- `config/mod.rs`: Add `ProviderConfig::test_config()` helper function

**Generated Artifacts**:
- Regenerate UniFFI Swift bindings (`aleph.swift`, FFI headers)
- Rebuild release library (`libalephcore.dylib`)

## Overview

### Problem Statement

The Aleph codebase currently targets Rust 1.70 (as specified in `Cargo.toml:5`) but the development environment already has Rust 1.92 available. This creates several issues:

1. **Outdated Dependencies**: The codebase uses external crates for functionality now available in the standard library:
   - `once_cell` (1.19) → Stabilized in Rust 1.70+ as `std::sync::OnceLock`, `LazyLock`, `std::cell::OnceCell`
   - `async-trait` (0.1) → Native async fn in traits (RPITIT) stabilized in Rust 1.75+

2. **Missing Modern Features**: Several Rust 1.77+ features could improve code quality:
   - C string literals `c"..."` (1.77+) - Eliminates `CString::new().unwrap()` boilerplate
   - `core::mem::offset_of!` macro (1.77+) - Removes need for `memoffset` crate
   - `Arc::new_zeroed`, `Box::new_zeroed` (stable) - Safer zero-initialization for FFI
     - When allocating zero-initialized memory for passing to C code, native support eliminates the need for manual `mem::MaybeUninit` initialization dance
     - Provides type-safe, ergonomic API for common FFI pattern

3. **Code Readability Issues**:
   - Current usage of `#[async_trait]` adds visual noise (7 occurrences across provider implementations)
   - `OnceCell` usage in `embedding.rs:10` predates standard library equivalents
   - UniFFI 0.25 is outdated; version 0.28+ utilizes C string literals for performance

4. **Build Performance**: Unnecessary dependencies increase compilation time and binary size

### Current State Analysis

**Dependencies to Remove** (from `Aleph/core/Cargo.toml`):
```toml
once_cell = "1.19"          # Line 27 - Use std::sync::OnceLock/LazyLock instead
async-trait = "0.1"         # Line 29 - Use native async fn in traits
```

**Code Locations Using Old Patterns**:
- `async-trait` usage (7 files):
  - `providers/openai.rs:45,337`
  - `providers/claude.rs:54,372`
  - `providers/ollama.rs:60,156`
  - `providers/mock.rs:7,127`
  - `providers/mod.rs:30,150,265`
- `once_cell` usage:
  - `memory/embedding.rs:10` - `OnceCell<bool>` for initialization flag

**UniFFI Upgrade Path**:
- Current: `uniffi = { version = "0.25", features = ["cli"] }`
- Target: `uniffi = "0.28"` (latest stable with C string literal optimizations)

### Proposed Solution

Implement a **Three-Phase Safety-First Migration**:

#### Phase 1: Environment & Dependency Updates
1. Update `Cargo.toml` MSRV from `1.70` to `1.92`
2. Upgrade UniFFI from `0.25` to `0.28`
3. Remove `once_cell` dependency
4. Remove `async-trait` dependency

#### Phase 2: Code Migration
1. Replace `OnceCell` with `std::sync::OnceLock` in `memory/embedding.rs`
2. Remove all `#[async_trait]` attributes and `use async_trait::async_trait` imports
3. Verify trait definitions compile with native async fn syntax
4. Update related documentation and comments

#### Phase 3: Code Quality Improvements
1. Audit and rename variables with unclear names
2. Remove redundant comments (e.g., comments that merely restate code)
3. Keep only comments explaining "why" rather than "what"

### Success Criteria

- [x] Rust 1.92 is available in development environment (verified)
- [ ] `Cargo.toml` specifies `rust-version = "1.92"`
- [ ] UniFFI upgraded to 0.28+
- [ ] `once_cell` dependency removed from `Cargo.toml`
- [ ] `async-trait` dependency removed from `Cargo.toml`
- [ ] All `#[async_trait]` attributes removed from code
- [ ] All `use async_trait::async_trait` imports removed
- [ ] `OnceCell` replaced with `std::sync::OnceLock` in `embedding.rs`
- [ ] All existing tests pass: `cargo test`
- [ ] Build succeeds: `cargo build --release`
- [ ] UniFFI bindings regenerate successfully
- [ ] No behavioral changes to end users
- [ ] Code readability improved (fewer macro attributes, clearer variable names)

## Impact Analysis

### User Experience
- **Neutral**: No visible changes to end users (internal refactoring only)
- **Performance**: Potential minor improvements from UniFFI 0.28 optimizations

### Technical Complexity
- **Low-Medium Risk**: Changes affect core trait definitions but with clear migration path
- **Critical Constraints**:
  - ✅ **UniFFI Stability**: UniFFI 0.28 is stable and compatible with current UDL schema
  - ✅ **Trait Signature Preservation**: Async fn in traits maintains same ABI as `async-trait` macro
  - ✅ **Build Integrity**: Must verify Swift bindings still generate correctly
  - ⚠️ **Potential Breaking Change**: UniFFI version bump may require regenerating Swift bindings

### Dependencies
- **Breaking Changes**: None (internal refactoring)
- **Related Changes**: None
- **Blocked By**: None

### Performance Impact
- **Build Time**: Expected 5-10% reduction from removed dependencies
- **Runtime Performance**: Neutral to slight improvement (UniFFI 0.28 optimizations)
- **Binary Size**: Expected 2-5% reduction from removed dependencies

## Migration Strategy

### Rollback Plan
If issues arise during migration:
1. Revert `Cargo.toml` changes
2. Restore `once_cell` and `async-trait` dependencies
3. Re-add `#[async_trait]` attributes
4. Regenerate UniFFI bindings with version 0.25

### Testing Strategy
1. **Unit Tests**: Run `cargo test` after each phase
2. **Integration Tests**: Verify AI provider implementations still work
3. **Build Verification**: Ensure `cargo build --release` succeeds
4. **UniFFI Validation**: Regenerate Swift bindings and verify no breaking changes
5. **Manual Testing**: Test hotkey detection and AI provider integration on macOS

### Deployment Plan
1. Merge after all tests pass
2. Update CI/CD to require Rust 1.92+
3. Document MSRV change in release notes
4. Monitor for any UniFFI-related issues in production

## Open Questions

1. ✅ **Resolved**: Is Rust 1.92 available? → Yes (verified via `cargo --version`)
2. ⚠️ **To Verify**: Does UniFFI 0.28 maintain backward compatibility with existing UDL? → Test in Phase 1
3. ⚠️ **To Verify**: Are there any performance regressions from native async traits? → Benchmark in Phase 3
4. **Optional**: Should we also migrate to `std::cell::LazyLock` for lazy initialization patterns? → Defer to future optimization

## References

- **Rust 1.70 Stabilizations**: `OnceLock`, `OnceCell` (https://blog.rust-lang.org/2023/06/01/Rust-1.70.0.html)
- **Rust 1.75 Async Traits**: RPITIT (Return Position Impl Trait In Traits)
- **Rust 1.77 Features**: C string literals, `offset_of!` macro
- **UniFFI 0.28 Release Notes**: https://github.com/mozilla/uniffi-rs/releases/tag/v0.28.0
- **Related Change**: `refactor-occams-razor` (dependency cleanup overlap)

## Implementation Notes

### Critical Safety Checks
1. **UniFFI Compatibility**: Verify `aleph.udl` schema remains valid with UniFFI 0.28
2. **Trait Object Safety**: Ensure `dyn AiProvider` still works with native async traits
3. **Swift Binding Generation**: Confirm `uniffi-bindgen generate` produces identical output
4. **FFI Boundary**: No changes to memory layout or public API signatures

### Code Review Checklist
- [ ] All `#[async_trait]` removed
- [ ] All `use async_trait::async_trait` removed
- [ ] `OnceCell` replaced with `OnceLock`
- [ ] No new `unwrap()` calls introduced
- [ ] Comments updated to reflect new patterns
- [ ] UniFFI bindings regenerated successfully
- [ ] All tests pass
- [ ] Build time measured and compared to baseline
