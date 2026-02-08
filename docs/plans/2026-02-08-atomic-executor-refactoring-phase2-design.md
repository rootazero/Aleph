# Phase 2: Atomic Executor Refactoring Design

**Date:** 2026-02-08
**Status:** Approved
**Author:** Architecture Review

## Executive Summary

This document outlines the second phase of the four-phase architecture refactoring initiative. Phase 2 focuses on refactoring `core/src/engine/atomic_executor.rs` (1547 lines) from a monolithic "God Object" into a modular, composition-based architecture using the Strategy Pattern.

## Background

### Problem Statement

The current `atomic_executor.rs` exhibits the "God Object" anti-pattern, containing all atomic operation logic in a single file:
- File I/O operations (Read, Write, Move)
- Text editing and patching (Edit, Replace)
- Shell command execution (Bash)
- Search operations (Search with regex)

This monolithic structure creates several issues:
1. **Testability** - Difficult to test individual operations in isolation
2. **Maintainability** - Hard to modify one operation without affecting others
3. **Extensibility** - Adding new atomic operations requires modifying the large file
4. **Security** - Security-critical operations (Bash) mixed with file operations

### Architecture Context

This refactoring is part of a broader initiative:
- **Phase 1** (Completed): `extension/types.rs` - Type definitions refactoring
- **Phase 2** (This Document): `engine/atomic_executor.rs` - Execution logic refactoring
- **Phase 3**: `browser/mod.rs` - Infrastructure services
- **Phase 4**: `gateway/handlers/poe.rs` - Business logic layer

## Design Goals

### Primary Objectives

1. **Improve Testability** - Enable isolated testing of each operation type
2. **Enhance Security** - Clear boundaries between security-critical operations
3. **Increase Extensibility** - Easy to add new atomic operations
4. **Maintain Compatibility** - Zero breaking changes to existing API

### Core Principles

1. **Composition over Inheritance** - Use handler composition instead of monolithic implementation
2. **Explicit over Implicit** - Dedicated method traits instead of generic handlers
3. **Separation of Concerns** - Environment (Context) vs. Constraints (Handler-specific config)
4. **Defense in Depth** - Centralized security checks in ExecutorContext

## Proposed Architecture

### Overall Structure

```
core/src/engine/atomic/
├── mod.rs              # Public interfaces and trait definitions
├── context.rs          # ExecutorContext (shared environment)
├── file.rs             # FileOpsHandler (file operations)
├── edit.rs             # EditOpsHandler (editing and replacement)
├── bash.rs             # BashOpsHandler (shell execution)
└── search.rs           # SearchOpsHandler (search operations)
```

### AtomicExecutor Structure

```rust
pub struct AtomicExecutor {
    context: Arc<ExecutorContext>,
    file_ops: FileOpsHandler,
    edit_ops: EditOpsHandler,
    bash_ops: BashOpsHandler,
    search_ops: SearchOpsHandler,
}
```

The `AtomicExecutor` maintains its original `execute(&self, action: &AtomicAction)` method as the unified entry point, ensuring complete backward compatibility. Internally, it dispatches requests to the appropriate handler based on the action type.

## Detailed Design

### 1. Trait Definitions

Each handler implements a dedicated trait with strongly-typed methods:

```rust
// core/src/engine/atomic/mod.rs

#[async_trait]
pub trait FileOps: Send + Sync {
    async fn read(&self, path: &str, range: Option<&LineRange>) -> Result<AtomicResult>;
    async fn write(&self, path: &str, content: &str, mode: &WriteMode) -> Result<AtomicResult>;
    async fn move_file(&self, source: &str, dest: &str, update_imports: bool, create_parent: bool) -> Result<AtomicResult>;
}

#[async_trait]
pub trait EditOps: Send + Sync {
    async fn edit(&self, path: &str, patches: &[Patch]) -> Result<AtomicResult>;
    async fn replace(&self, search: &str, replacement: &str, scope: &SearchScope, preview: bool, dry_run: bool) -> Result<AtomicResult>;
}

#[async_trait]
pub trait BashOps: Send + Sync {
    async fn execute(&self, command: &str, cwd: Option<&str>) -> Result<AtomicResult>;
}

#[async_trait]
pub trait SearchOps: Send + Sync {
    async fn search(&self, pattern: &SearchPattern, scope: &SearchScope, filters: &[FileFilter]) -> Result<AtomicResult>;
}
```

**Design Rationale:**

1. **Strongly Typed Contracts** - Each method has precise parameter types, eliminating the need for pattern matching within handlers
2. **Compiler Guarantees** - Adding a new `AtomicAction` requires updating all trait implementations, caught at compile time
3. **Code as Documentation** - The trait definition clearly shows all supported operations
4. **Granular Testing** - Each method can be mocked and tested independently

### 2. ExecutorContext Design

```rust
// core/src/engine/atomic/context.rs

pub struct ExecutorContext {
    /// Working directory (sandbox root)
    pub working_dir: PathBuf,
}

impl ExecutorContext {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    /// Resolve a path relative to working_dir with security checks
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.working_dir.join(path)
        };

        // Canonicalize and check for path traversal
        let canonical = resolved.canonicalize()
            .map_err(|e| AlephError::InvalidPath(format!("Cannot resolve path: {}", e)))?;

        if !canonical.starts_with(&self.working_dir) {
            return Err(AlephError::SecurityViolation(
                format!("Path traversal detected: {}", path.display())
            ));
        }

        Ok(canonical)
    }
}
```

**Design Rationale:**

1. **Environment vs. Constraints** - `working_dir` is the execution environment (sandbox), shared by all handlers
2. **DRY Principle** - Centralized `resolve_path()` logic prevents duplication across handlers
3. **Security Boundary** - All path operations go through a single, auditable security check
4. **Future Extensibility** - Can add global state like `dry_run`, audit logging, etc.

### 3. Handler Implementations

Each handler holds:
- `Arc<ExecutorContext>` - Shared environment
- Handler-specific configuration - Operation-specific constraints

```rust
// core/src/engine/atomic/file.rs
pub struct FileOpsHandler {
    context: Arc<ExecutorContext>,
    max_file_size: u64,
}

impl FileOpsHandler {
    pub fn new(context: Arc<ExecutorContext>, max_file_size: u64) -> Self {
        Self { context, max_file_size }
    }
}

#[async_trait]
impl FileOps for FileOpsHandler {
    async fn read(&self, path: &str, range: Option<&LineRange>) -> Result<AtomicResult> {
        let resolved_path = self.context.resolve_path(path)?;

        // Check file exists
        if !resolved_path.exists() {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!("File not found: {}", resolved_path.display())),
            });
        }

        // Check file size
        let metadata = tokio::fs::metadata(&resolved_path).await?;
        if metadata.len() > self.max_file_size {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "File too large: {} bytes (max: {})",
                    metadata.len(),
                    self.max_file_size
                )),
            });
        }

        // Read file and apply range...
        // (rest of implementation)
    }

    // ... other methods
}
```

**Similar structure for other handlers:**

```rust
// core/src/engine/atomic/edit.rs
pub struct EditOpsHandler {
    context: Arc<ExecutorContext>,
    max_file_size: u64,
}

// core/src/engine/atomic/bash.rs
pub struct BashOpsHandler {
    context: Arc<ExecutorContext>,
    command_timeout: Duration,
}

// core/src/engine/atomic/search.rs
pub struct SearchOpsHandler {
    context: Arc<ExecutorContext>,
}
```

**Design Rationale:**

1. **Minimal Privilege** - Each handler only knows its own constraints
2. **Shared Security** - All handlers use `context.resolve_path()` for path operations
3. **Independent Testing** - Each handler can be tested with minimal setup
4. **Clear Ownership** - Each handler owns its operation-specific logic

### 4. AtomicExecutor Refactoring

```rust
// core/src/engine/atomic_executor.rs

impl AtomicExecutor {
    pub fn new(working_dir: PathBuf) -> Self {
        let context = Arc::new(ExecutorContext::new(working_dir));

        Self {
            context: context.clone(),
            file_ops: FileOpsHandler::new(context.clone(), 10 * 1024 * 1024),
            edit_ops: EditOpsHandler::new(context.clone(), 10 * 1024 * 1024),
            bash_ops: BashOpsHandler::new(context.clone(), Duration::from_secs(30)),
            search_ops: SearchOpsHandler::new(context.clone()),
        }
    }

    pub async fn execute(&self, action: &AtomicAction) -> Result<AtomicResult> {
        match action {
            AtomicAction::Read { path, range } =>
                self.file_ops.read(path, range.as_ref()).await,

            AtomicAction::Write { path, content, mode } =>
                self.file_ops.write(path, content, mode).await,

            AtomicAction::Move { source, destination, update_imports, create_parent } =>
                self.file_ops.move_file(source, destination, *update_imports, *create_parent).await,

            AtomicAction::Edit { path, patches } =>
                self.edit_ops.edit(path, patches).await,

            AtomicAction::Replace { search, replacement, scope, preview, dry_run } =>
                self.edit_ops.replace(search, replacement, scope, *preview, *dry_run).await,

            AtomicAction::Bash { command, cwd } =>
                self.bash_ops.execute(command, cwd.as_ref()).await,

            AtomicAction::Search { pattern, scope, filters } =>
                self.search_ops.search(pattern, scope, filters).await,
        }
    }
}
```

**Design Rationale:**

1. **Backward Compatibility** - Public API (`new()` and `execute()`) remains unchanged
2. **Clear Dispatch** - Single responsibility: route actions to appropriate handlers
3. **No Business Logic** - AtomicExecutor contains no operation logic, only routing
4. **Compile-Time Safety** - Exhaustive match ensures all actions are handled

## Implementation Plan

### Step 1: Create Directory and Base Structure

```bash
mkdir -p core/src/engine/atomic
```

Create files:
1. `atomic/mod.rs` - Trait definitions and public interfaces
2. `atomic/context.rs` - ExecutorContext implementation

### Step 2: Extract Handler Implementations

Create handlers in dependency order:

1. **`atomic/file.rs`** - Extract file operation logic
   - `execute_read()` → `FileOpsHandler::read()`
   - `execute_write()` → `FileOpsHandler::write()`
   - `execute_move()` → `FileOpsHandler::move_file()`

2. **`atomic/edit.rs`** - Extract editing logic
   - `execute_edit()` → `EditOpsHandler::edit()`
   - `execute_replace()` → `EditOpsHandler::replace()`

3. **`atomic/bash.rs`** - Extract shell execution logic
   - `execute_bash()` → `BashOpsHandler::execute()`

4. **`atomic/search.rs`** - Extract search logic
   - `execute_search()` → `SearchOpsHandler::search()`

### Step 3: Refactor AtomicExecutor

Transform `atomic_executor.rs` to use composition:
- Replace method implementations with handler delegation
- Update `new()` to construct handler instances
- Update `execute()` to dispatch to handlers

### Step 4: Update Module Declarations

In `engine/mod.rs`:
```rust
pub mod atomic;
pub mod atomic_executor;

pub use atomic_executor::AtomicExecutor;  // Maintain backward compatibility
```

### Step 5: Verification

Run comprehensive verification:

```bash
# Compilation check
cargo check -p alephcore

# Run all tests
cargo test -p alephcore --lib

# Run integration tests
cargo test -p alephcore --test '*'
```

## Verification Strategy

### 1. Compilation Verification

- All code must compile without errors
- No new warnings introduced
- All existing imports continue to work

### 2. Test Verification

- All existing tests must pass
- No test modifications required (tests use public API)
- Test coverage maintained or improved

### 3. API Compatibility Verification

Verify that existing code continues to work:
```rust
// These calls should work unchanged
let executor = AtomicExecutor::new(PathBuf::from("/workspace"));
let result = executor.execute(&AtomicAction::Read {
    path: "file.txt".to_string(),
    range: None
}).await?;
```

### 4. Behavior Consistency Verification

- Path resolution behaves identically
- Security checks remain the same
- Error messages are consistent
- File size limits enforced correctly
- Command timeouts work as before

### 5. Security Verification

- Path traversal protection maintained
- Sandbox boundaries enforced
- Command injection prevention intact
- File size limits respected

## Risk Assessment

### Medium Risk

- **Business Logic Extraction** - Unlike Phase 1 (type definitions), this involves moving actual execution logic
- **Async Trait Complexity** - Need to ensure async trait implementations work correctly
- **Shared State** - Arc<ExecutorContext> must be used correctly across handlers

### Mitigation Strategies

1. **Incremental Extraction** - Extract one handler at a time, verify after each
2. **Comprehensive Testing** - Run full test suite after each handler extraction
3. **Code Review** - Careful review of path resolution and security checks
4. **Integration Testing** - Test real-world scenarios with actual file operations

### Rollback Strategy

If verification fails:
```bash
git checkout core/src/engine/atomic_executor.rs
rm -rf core/src/engine/atomic/
```

## Success Criteria

1. ✅ `cargo check` passes without errors
2. ✅ `cargo test` passes all tests (no failures)
3. ✅ No changes required in calling code
4. ✅ File sizes reduced to manageable units (<400 lines each)
5. ✅ Handler responsibilities clearly defined
6. ✅ Security checks centralized and auditable
7. ✅ Each handler independently testable

## Future Enhancements

### Phase 3 Preview

After Phase 2 completion, Phase 3 will address `browser/mod.rs` (1695 lines):
- Extract `BrowserService` to `browser/service.rs`
- Extract `BrowserPool` to `browser/pool.rs`
- Keep `mod.rs` for exports and error definitions

### Potential Extensions

1. **Mock Handlers for Testing** - Create mock implementations of each trait for unit testing
2. **Handler Decorators** - Add decorators for logging, metrics, rate limiting
3. **Policy-Based Execution** - Add execution policies (ReadOnly, DryRun, etc.)
4. **Pluggable Handlers** - Allow custom handler implementations via configuration

## Conclusion

This refactoring transforms the monolithic `atomic_executor.rs` into a modular, composition-based architecture. By using the Strategy Pattern with dedicated method traits, we achieve:

1. **Strong Type Safety** - Compile-time guarantees for all operations
2. **Clear Security Boundaries** - Isolated security-critical operations
3. **Enhanced Testability** - Each handler can be tested independently
4. **Improved Maintainability** - Clear responsibilities and minimal coupling
5. **Complete Compatibility** - Zero breaking changes to existing code

The use of composition over inheritance, combined with the hybrid approach to shared state (ExecutorContext for environment, handler-specific config for constraints), provides a robust foundation for future extensions while maintaining the simplicity and clarity of the codebase.

---

**Next Steps:**
1. Obtain architecture review approval ✓
2. Create implementation tasks
3. Execute refactoring following the implementation plan
4. Submit PR with comprehensive testing evidence
5. Proceed to Phase 3 (browser/mod.rs) after Phase 2 stabilizes
