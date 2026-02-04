## ADDED Requirements

### Requirement: Module-Based Code Organization
The core library SHALL organize its implementation into logical submodules following Rust 2018+ conventions, improving code navigation, testability, and maintainability.

#### Scenario: Directory structure follows Rust 2018+ pattern
- **WHEN** developer examines `src/core/` directory
- **THEN** the directory contains:
  - `mod.rs` - Main AlephCore struct definition and core lifecycle methods
  - `types.rs` - Shared type definitions (MediaAttachment, CapturedContext, etc.)
  - `memory.rs` - Memory storage, retrieval, and compression methods
  - `config_ops.rs` - Configuration management methods
  - `mcp_ops.rs` - MCP capability methods
  - `search_ops.rs` - Search capability methods
  - `tools.rs` - Dispatcher and tool registry methods
  - `conversation.rs` - Multi-turn conversation management
  - `processing.rs` - AI processing pipeline methods
  - `tests.rs` - Unit tests (conditionally compiled)
- **AND** each module is focused on a single functional area

#### Scenario: Public API remains unchanged after refactor
- **WHEN** the core module is refactored into submodules
- **THEN** all public functions retain exact same signatures
- **AND** UniFFI interface (`aleph.udl`) requires no changes
- **AND** Swift client code continues to compile without modifications
- **AND** all existing tests pass without modification

#### Scenario: Submodule visibility uses pub(crate) for internal helpers
- **WHEN** developer defines helper functions used across submodules
- **THEN** helper functions use `pub(crate)` visibility
- **AND** helper functions are NOT exposed via UniFFI
- **AND** submodules can access helpers via `super::` imports

### Requirement: Impl Block Distribution Across Submodules
The core library SHALL allow AlephCore struct's impl blocks to be distributed across submodules while maintaining a single struct definition.

#### Scenario: Multiple impl blocks for single struct
- **WHEN** AlephCore methods are split across submodules
- **THEN** each submodule can define its own `impl AlephCore` block
- **AND** submodules import AlephCore via `use super::AlephCore`
- **AND** all methods are accessible as if defined in a single file
- **AND** Rust compiler merges all impl blocks correctly

#### Scenario: Inter-module method calls work correctly
- **WHEN** a method in `processing.rs` calls a method defined in `memory.rs`
- **THEN** the call resolves correctly through the shared AlephCore instance
- **AND** no explicit re-exports are required for internal method calls
- **AND** visibility is controlled by method-level `pub` or `pub(crate)` modifiers

### Requirement: Module Re-exports for Public API
The core module SHALL re-export all public types from the root module file to maintain backward-compatible import paths.

#### Scenario: Types exported from core module root
- **WHEN** external code imports types from `crate::core`
- **THEN** all public types (MediaAttachment, CapturedContext, CompressionStats, AlephCore) are available
- **AND** import path `use crate::core::AlephCore` continues to work
- **AND** no need to specify submodule paths for public types

#### Scenario: Internal types remain private
- **WHEN** submodule defines `pub(crate)` types for internal use
- **THEN** those types are NOT re-exported from core module root
- **AND** those types are NOT accessible via UniFFI
- **AND** internal implementation details remain hidden

## MODIFIED Requirements

### Requirement: Modular Trait-Based Architecture
The system SHALL define traits for all core components (HotkeyListener, ClipboardManager, InputSimulator) to enable swappable implementations and testing, with each component organized into appropriate submodules.

#### Scenario: Swap clipboard implementation
- **WHEN** developer wants to use a different clipboard backend
- **THEN** they implement the ClipboardManager trait
- **AND** pass the new implementation to AlephCore
- **AND** no changes to core logic are required
- **AND** trait implementation can be in any submodule or external crate

#### Scenario: Mock components in tests
- **WHEN** writing unit tests for AlephCore
- **THEN** developer creates mock implementations of traits
- **AND** injects mocks into AlephCore constructor
- **AND** tests core logic in isolation
- **AND** tests can be placed in `core/tests.rs` with access to all internal types via `pub(crate)`

#### Scenario: Test modules have access to internal helpers
- **WHEN** tests need to verify internal behavior
- **THEN** tests in `core/tests.rs` can access `pub(crate)` functions
- **AND** tests can construct test fixtures using internal types
- **AND** test isolation is improved by module boundaries
