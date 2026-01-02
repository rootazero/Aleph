# Baseline Metrics: refactor-occams-razor

**Date**: 2026-01-02
**Status**: ⚠️ Baseline cannot be established - codebase has compilation errors

---

## Compilation Status

### Current State
❌ **Compilation Failed** - Cannot run baseline tests

### Error Summary
- **Error Count**: 75 compilation errors
- **Root Cause**: `add-default-provider-selection` change (in progress, 89/118 tasks) added new fields to `ProviderConfig`:
  - `enabled: bool`
  - `frequency_penalty: Option<f32>`
  - `presence_penalty: Option<f32>`
  - `media_resolution: Option<String>`
  - `top_p: Option<f32>`
  - `top_k: Option<u32>`
  - (and others)

### Affected Files
- `Aether/core/src/config/mod.rs` (test code, 11 errors)
- `Aether/core/src/router/mod.rs` (test code, 8+ errors)
- Other test files with `ProviderConfig` initializations

---

## Decision: Proceed with Refactoring Despite Baseline Unavailability

### Rationale
1. **Independent Changes**: Our refactoring tasks (helper method extraction, dependency removal) do NOT touch the same code areas as the failing tests
2. **Safety Margin**: All planned refactorings are internal-only (private helpers, removed dependencies)
3. **No UniFFI Changes**: Zero risk of breaking FFI boundaries
4. **Rollback Available**: Each commit can be reverted independently

### Risk Mitigation Strategy
1. **Skip Problematic Areas**: Avoid refactoring any code in `config/mod.rs` or `router/mod.rs` until tests pass
2. **Focus on Low-Risk Tasks**: Start with dependency removal and mutex helpers (unrelated to failing tests)
3. **Deferred Baseline**: Establish proper baseline AFTER `add-default-provider-selection` is completed

---

## Attempted Baseline Measurements

### Test Suite
```bash
$ cd Aether/core && cargo test
Error: 75 compilation errors (cannot proceed)
```

**Result**: ❌ Unavailable

---

### Build Time
```bash
$ cd Aether/core && cargo clean && cargo build --release --timings
```

**Result**: ⏳ Skipped (compilation errors prevent measurement)

**Estimated**: Based on previous builds, ~2-3 minutes

---

### Binary Size
```bash
$ ls -lh Aether/Frameworks/libaethecore.dylib
```

**Result**: ⏳ Skipped (no binary available due to compilation errors)

**Estimated**: Based on previous builds, ~8-10 MB

---

## Dependency Snapshot (Before Refactoring)

### Current Dependencies (from `Cargo.toml`)
```toml
[dependencies]
# === Async Runtime ===
tokio = { version = "1.43.0", features = ["full"] }
tokio-util = "0.7"  # ⚠️ TARGET FOR REMOVAL (Task A1)

# === FFI & UniFFI ===
uniffi = { version = "0.25", features = ["cli"] }

# === Concurrency & Utilities ===
once_cell = "1.19"  # ⚠️ TARGET FOR REMOVAL (Task A2)
futures-util = "0.3"  # ⚠️ TARGET FOR INVESTIGATION (Task A11)

# === HTTP Client ===
reqwest = { version = "0.11", features = ["json", "stream", "rustls-tls"] }

# === Serialization ===
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"

# === Logging ===
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "json"] }
tracing-appender = "0.2"

# === Clipboard & Input ===
arboard = "3.3"
enigo = "0.2.1"
rdev = "0.5.3"

# === Memory Module ===
lancedb = "0.4"
ort = { version = "2.0.0-rc.2", default-features = false, features = ["download-binaries", "half", "copy-dylibs"] }
ndarray = "0.15"
ndarray-stats = "0.5"
tokenizers = "0.14"
regex = "1.9"
rusqlite = { version = "0.30", features = ["bundled"] }
sqlite-vec = "0.1.6"

# === Other ===
anyhow = "1.0"
thiserror = "2.0"
```

### Dependencies to Remove (Phase 3)
1. **tokio-util** (Task A1): Used only for legacy `CancellationToken`
2. **once_cell** (Task A2): Can be replaced with `std::sync::OnceLock`
3. **futures-util** (Task A11 - conditional): Used only for `StreamExt` in one file

---

## Code Metrics (Estimated)

### Files to Refactor
- `Aether/core/src/core.rs` (~2000 lines)
  - Mutex lock boilerplate: ~20 occurrences
  - Memory DB null checks: ~10 occurrences
  - Error conversion duplication: 2 methods
  - Async nesting: 1 complex function

- `Aether/Sources/AppDelegate.swift` (~500 lines)
  - Duplicated menu rebuild logic: 2 methods (~100 lines total)

- `Aether/Sources/RoutingView.swift` (~600 lines)
  - Duplicated alert creation: 3 occurrences

- `Aether/Sources/EventHandler.swift` (~400 lines)
  - Redundant color parsing: 1 method

---

## Expected Improvements (Post-Refactoring)

### Code Reduction
- **Target**: 200-250 lines removed
- **Breakdown**:
  - Mutex helpers: ~30 lines
  - Memory DB helper: ~25 lines
  - Menu builder: ~60 lines
  - Alert helper: ~15 lines
  - Color parsing: ~10 lines
  - Error conversion: ~15 lines
  - Test provider consolidation: ~35 lines
  - Others: ~10-20 lines

### Build Time
- **Target**: 5-10% reduction
- **Primary Impact**: Dependency removal (tokio-util, once_cell, possibly futures-util)
- **Estimated Savings**: ~10-20 seconds

### Binary Size
- **Target**: 2-5% reduction
- **Primary Impact**: Dependency removal
- **Estimated Savings**: ~200-400 KB

---

## Next Steps

1. ✅ **Phase 2.1-2.2 Complete**: Risk assessment done, `STEP2_VERIFIED_PLAN.md` created
2. ⏸️ **Phase 2.3 Incomplete**: Baseline metrics unavailable due to compilation errors
3. ➡️ **Proceed to Phase 3**: Start with low-risk tasks (A1-A6) that don't conflict with failing tests
4. ⏳ **Deferred**: Full baseline measurement after `add-default-provider-selection` merge

---

## Notes

- **Acceptable Risk**: Proceeding without baseline is acceptable because:
  - Our changes are orthogonal to the failing tests
  - All refactorings are low-risk (private helpers, dependency cleanup)
  - Each task will be validated independently
  - Git history allows easy rollback

- **Alternative Considered**: Wait for `add-default-provider-selection` to complete
  - **Rejected**: Unknown timeline, blocks refactoring progress

---

**Conclusion**: Proceed with Phase 3 (low-risk tasks first), establish proper baseline after codebase stabilizes.
