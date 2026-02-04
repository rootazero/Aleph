# Tasks: upgrade-rust-to-1.92

## Phase 1: Environment & Dependency Updates

### Task 1.1: Update Rust Version and UniFFI
- [ ] Update `Cargo.toml` MSRV: Change `rust-version = "1.70"` to `"1.92"` (line 5)
- [ ] Update UniFFI dependency: `uniffi = "0.28"` (replace version 0.25, line 12)
- [ ] Update UniFFI build dependency: `uniffi = { version = "0.28", features = ["build"] }` (line 51)
- [ ] Run `cargo update uniffi` to update lock file
- [ ] Verify build succeeds: `cargo build`
- **Validation**: Build completes without errors, UniFFI version confirmed in `Cargo.lock`
- **Dependencies**: None
- **Estimated Time**: 10 minutes

### Task 1.2: Remove Deprecated Dependencies
- [ ] Remove `once_cell = "1.19"` from dependencies (line 27)
- [ ] Remove `async-trait = "0.1"` from dependencies (line 29)
- [ ] Run `cargo update` to clean lock file
- **Validation**: Dependencies removed from `Cargo.toml` and `Cargo.lock`
- **Dependencies**: None (can run in parallel with 1.1)
- **Estimated Time**: 5 minutes

### Task 1.3: Establish Baseline Metrics
- [ ] Run full test suite: `cd Aleph/core && cargo test`
- [ ] Measure build time: `cargo clean && time cargo build --release`
- [ ] Capture binary size: `ls -lh target/release/libalephcore.dylib`
- [ ] Document metrics in commit message or notes
- **Validation**: Baseline documented for post-migration comparison
- **Dependencies**: None
- **Estimated Time**: 15 minutes

---

## Phase 2: Code Migration

### Task 2.1: Migrate OnceCell to OnceLock
- [ ] Update import in `memory/embedding.rs:10`: Replace `use once_cell::sync::OnceCell` with `use std::sync::OnceLock`
- [ ] Update field type in `EmbeddingModel` struct (line 22): `initialized: OnceLock<bool>`
- [ ] Update initialization in `new()` method (line 44): `initialized: OnceLock::new()`
- [ ] Verify method calls still work (`.get_or_try_init()` signature is identical)
- [ ] Run tests: `cargo test memory::embedding`
- **Validation**: Tests pass, no compilation errors
- **Dependencies**: Task 1.2 complete
- **Estimated Time**: 10 minutes

### Task 2.2: Remove async-trait from AiProvider Trait
- [ ] Open `providers/mod.rs`
- [ ] Remove import (line 30): Delete `use async_trait::async_trait;`
- [ ] Remove attribute (line 150): Delete `#[async_trait]` above `pub trait AiProvider`
- [ ] Keep trait definition unchanged (native async fn already valid in Rust 1.75+)
- [ ] Run `cargo check` to verify trait compiles
- **Validation**: No compilation errors, trait signature unchanged
- **Dependencies**: Task 1.2 complete
- **Estimated Time**: 5 minutes

### Task 2.3: Remove async-trait from OpenAI Provider
- [ ] Open `providers/openai.rs`
- [ ] Remove import (line 45): Delete `use async_trait::async_trait;`
- [ ] Remove attribute (line 337): Delete `#[async_trait]` above `impl AiProvider`
- [ ] Verify implementation still compiles
- [ ] Run tests: `cargo test providers::openai`
- **Validation**: Tests pass, no compilation errors
- **Dependencies**: Task 2.2 complete
- **Estimated Time**: 5 minutes

### Task 2.4: Remove async-trait from Claude Provider
- [ ] Open `providers/claude.rs`
- [ ] Remove import (line 54): Delete `use async_trait::async_trait;`
- [ ] Remove attribute (line 372): Delete `#[async_trait]` above `impl AiProvider`
- [ ] Verify implementation still compiles
- [ ] Run tests: `cargo test providers::claude`
- **Validation**: Tests pass, no compilation errors
- **Dependencies**: Task 2.2 complete (can run in parallel with 2.3)
- **Estimated Time**: 5 minutes

### Task 2.5: Remove async-trait from Ollama Provider
- [ ] Open `providers/ollama.rs`
- [ ] Remove import (line 60): Delete `use async_trait::async_trait;`
- [ ] Remove attribute (line 156): Delete `#[async_trait]` above `impl AiProvider`
- [ ] Verify implementation still compiles
- [ ] Run tests: `cargo test providers::ollama`
- **Validation**: Tests pass, no compilation errors
- **Dependencies**: Task 2.2 complete (can run in parallel with 2.3-2.4)
- **Estimated Time**: 5 minutes

### Task 2.6: Remove async-trait from Mock Provider
- [ ] Open `providers/mock.rs`
- [ ] Remove import (line 7): Delete `use async_trait::async_trait;`
- [ ] Remove attribute (line 127): Delete `#[async_trait]` above `impl AiProvider`
- [ ] Verify implementation still compiles
- [ ] Run tests: `cargo test providers::mock`
- **Validation**: Tests pass, no compilation errors
- **Dependencies**: Task 2.2 complete (can run in parallel with 2.3-2.5)
- **Estimated Time**: 5 minutes

### Task 2.7: Full Test Suite Validation
- [ ] Run complete test suite: `cargo test`
- [ ] Verify all tests pass (especially provider integration tests)
- [ ] Check for any deprecation warnings: `cargo clippy`
- [ ] Fix any new clippy warnings introduced by Rust 1.92
- **Validation**: All tests pass, no warnings
- **Dependencies**: Tasks 2.1-2.6 complete
- **Estimated Time**: 10 minutes

---

## Phase 3: UniFFI Bindings & Build Verification

### Task 3.1: Regenerate UniFFI Bindings
- [ ] Navigate to core directory: `cd Aleph/core`
- [ ] Regenerate Swift bindings:
  ```bash
  cargo run --bin uniffi-bindgen generate src/aleph.udl \
    --language swift \
    --out-dir ../Sources/Generated/
  ```
- [ ] Compare generated `aleph.swift` with previous version
- [ ] Verify no breaking API changes in generated Swift code
- [ ] Check for new UniFFI 0.28 optimizations (C string literals)
- **Validation**: Swift bindings regenerate successfully, no breaking changes
- **Dependencies**: Tasks 2.1-2.7 complete
- **Estimated Time**: 10 minutes

### Task 3.2: Build Rust Core Library
- [ ] Clean build artifacts: `cargo clean`
- [ ] Build release version: `cargo build --release`
- [ ] Copy library to Frameworks: `cp target/release/libalephcore.dylib ../Frameworks/`
- [ ] Measure build time and compare to baseline (Task 1.3)
- [ ] Measure binary size and compare to baseline
- **Validation**: Build succeeds, metrics show improvement
- **Dependencies**: Task 3.1 complete
- **Estimated Time**: 5 minutes

### Task 3.3: Build macOS Client
- [ ] Navigate to project root: `cd /Users/zouguojun/Workspace/Aleph`
- [ ] Regenerate Xcode project: `xcodegen generate`
- [ ] Build from command line:
  ```bash
  xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Release build
  ```
- [ ] Verify no Swift compilation errors
- [ ] Check for any UniFFI-related warnings
- **Validation**: macOS client builds successfully
- **Dependencies**: Task 3.2 complete
- **Estimated Time**: 10 minutes

---

## Phase 4: Code Quality Improvements

### Task 4.1: Audit Variable Naming
- [ ] Search for single-letter variables (excluding loop counters): `rg '\b[a-z]\s*='`
- [ ] Identify variables with unclear names (e.g., `tmp`, `data`, `result` without context)
- [ ] Rename to descriptive names following Rust naming conventions
- [ ] Document renaming decisions in commit message
- **Validation**: Code review confirms improved clarity
- **Dependencies**: None (can run in parallel with Phase 3)
- **Estimated Time**: 20 minutes

### Task 4.2: Remove Redundant Comments
- [ ] Search for obvious comments: `rg '// .*\+\+|// .*-=|// Initialize|// Create|// Set'`
- [ ] Remove comments that merely restate code (e.g., `// increment i`)
- [ ] Keep architectural comments explaining "why" (e.g., `// Use lazy loading for performance`)
- [ ] Keep safety comments (e.g., `// SAFETY: This is safe because...`)
- [ ] Keep TODO/FIXME/NOTE comments with actionable context
- **Validation**: Code review confirms comments add value
- **Dependencies**: None (can run in parallel with 4.1)
- **Estimated Time**: 15 minutes

### Task 4.3: Update Documentation
- [ ] Update `CLAUDE.md:222` to reflect Rust 1.92 requirement
- [ ] Update build instructions if needed
- [ ] Update `Cargo.toml` package metadata if needed
- [ ] Add migration notes to CHANGELOG or release notes
- **Validation**: Documentation accurately reflects changes
- **Dependencies**: Tasks 3.1-3.3 complete
- **Estimated Time**: 10 minutes

---

## Phase 5: Final Validation

### Task 5.1: Manual Testing (macOS)
- [ ] Launch Aleph app
- [ ] Verify hotkey detection works (Cmd+~)
- [ ] Test clipboard operations (text and images)
- [ ] Test AI provider integration (OpenAI, Claude, Ollama)
- [ ] Verify Halo overlay appearance
- [ ] Check menu bar settings UI
- [ ] Test config reload functionality
- **Validation**: All critical features work as expected
- **Dependencies**: Task 3.3 complete
- **Estimated Time**: 20 minutes

### Task 5.2: Performance Benchmarks
- [ ] Run memory benchmarks: `cargo bench --bench memory_benchmarks_simple`
- [ ] Run AI benchmarks: `cargo bench --bench ai_benchmarks`
- [ ] Compare results to baseline (if available)
- [ ] Document any performance changes (positive or negative)
- **Validation**: No performance regressions
- **Dependencies**: Task 3.2 complete
- **Estimated Time**: 15 minutes

### Task 5.3: Final Cleanup
- [ ] Run `cargo fmt` to format all Rust code
- [ ] Run `cargo clippy -- -D warnings` to catch any warnings
- [ ] Remove any debug print statements added during migration
- [ ] Remove temporary files or backup code
- [ ] Verify `.gitignore` excludes build artifacts
- **Validation**: Clean `git status`, no warnings
- **Dependencies**: All previous tasks complete
- **Estimated Time**: 10 minutes

---

## Summary

**Total Tasks**: 18
**Estimated Total Time**: 3-4 hours
**Critical Path**: Tasks 1.1-1.2 → 2.1-2.7 → 3.1-3.3 → 5.1

**Parallelizable Work**:
- Phase 2: Tasks 2.3-2.6 can run in parallel after 2.2
- Phase 4: Tasks 4.1-4.2 can run independently
- Testing: Tasks 5.1-5.2 can overlap

**Risk Mitigation**:
- Baseline metrics captured in Task 1.3
- Incremental testing after each provider migration (Tasks 2.3-2.6)
- Full test suite validation before UniFFI regeneration (Task 2.7)
- Swift binding verification before macOS build (Task 3.1)
- Manual testing before final sign-off (Task 5.1)
