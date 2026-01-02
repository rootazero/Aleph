# Spec Delta: core-library

## MODIFIED Requirements

### Requirement: Library Initialization
The system SHALL provide a library-based Rust core that can be compiled as a dynamic library (cdylib) and static library (staticlib) for consumption by native UI clients, targeting Rust 1.92+ for modern standard library features.

#### Scenario: Build dynamic library for macOS with Rust 1.92
- **WHEN** developer runs `cargo build --release --target aarch64-apple-darwin`
- **THEN** the build uses Rust 1.92 or higher as specified in Cargo.toml
- **AND** the build produces `libaethecore.dylib` in target/release/
- **AND** the library exports C-compatible symbols for FFI
- **AND** the library uses standard library features (OnceLock, native async traits) instead of external crates

#### Scenario: Build static library
- **WHEN** developer runs `cargo build --release --lib`
- **THEN** the build produces `libaethecore.a` for static linking
- **AND** the library contains all required dependencies
- **AND** the library does NOT include removed dependencies (once_cell, async-trait)

#### Scenario: Verify minimum Rust version requirement
- **WHEN** developer attempts to build with Rust < 1.92
- **THEN** cargo SHALL fail with error: "package requires `rustc 1.92` or newer"
- **AND** the error message directs the user to update Rust toolchain

### Requirement: Async Runtime Support
The system SHALL initialize a tokio async runtime to support non-blocking operations for AI API calls, utilizing native async trait implementations without external macro dependencies.

#### Scenario: Runtime initialization
- **WHEN** AetherCore is initialized
- **THEN** tokio runtime is created successfully
- **AND** supports multi-threaded execution
- **AND** allows spawning async tasks
- **AND** uses native async fn in traits (no async-trait macro required)

#### Scenario: Native async trait implementation
- **WHEN** AI provider implements the AiProvider trait
- **THEN** the implementation uses native `async fn process()` syntax
- **AND** does NOT require `#[async_trait]` attribute
- **AND** the compiler generates correct async trait object code
- **AND** trait objects remain Send + Sync compatible

## ADDED Requirements

### Requirement: Standard Library Synchronization Primitives
The system SHALL use Rust standard library synchronization primitives (OnceLock, LazyLock) for lazy initialization and thread-safe state management, eliminating external dependencies.

#### Scenario: Lazy initialization with OnceLock
- **WHEN** EmbeddingModel needs lazy initialization of model files
- **THEN** the system uses `std::sync::OnceLock<bool>` for initialization tracking
- **AND** uses `.get_or_try_init()` for thread-safe initialization
- **AND** does NOT use the `once_cell` crate

#### Scenario: Thread-safe lazy static with LazyLock
- **WHEN** the system needs a lazily-initialized global constant
- **THEN** the system uses `std::sync::LazyLock<T>` instead of `lazy_static!` macro
- **AND** initialization code executes exactly once
- **AND** provides thread-safe access without runtime overhead after initialization

### Requirement: Safe Zero-Initialization for FFI
The system SHALL use standard library's `Arc::new_zeroed` and `Box::new_zeroed` for allocating zero-initialized memory in FFI contexts, eliminating manual `MaybeUninit` initialization patterns.

#### Scenario: Allocate zero-initialized Arc for FFI
- **WHEN** the system needs to allocate zero-initialized memory in an Arc to pass to C code
- **THEN** it uses `Arc::new_zeroed()` to create the allocation
- **AND** calls `.assume_init()` after verifying safety invariants
- **AND** does NOT manually construct `MaybeUninit<T>` and perform initialization dance

#### Scenario: Allocate zero-initialized Box for FFI buffers
- **WHEN** the system needs a zero-initialized heap buffer for FFI boundary
- **THEN** it uses `Box::new_zeroed()` for type-safe allocation
- **AND** the API provides ergonomic, safe interface compared to manual `MaybeUninit::zeroed()`
- **AND** reduces boilerplate code for common FFI pattern

#### Scenario: Prevent unsafe initialization anti-patterns
- **WHEN** code review detects manual zero-initialization patterns
- **THEN** reviewer SHALL recommend `Arc::new_zeroed` or `Box::new_zeroed` instead
- **AND** clippy lint (if available) SHALL warn on manual `MaybeUninit::zeroed()` usage
- **AND** code maintains type safety without verbose initialization ceremony

### Requirement: UniFFI 0.28 Compatibility
The system SHALL use UniFFI 0.28 or higher for FFI binding generation, leveraging modern performance optimizations and C string literal support.

#### Scenario: Generate Swift bindings with UniFFI 0.28
- **WHEN** developer runs `cargo run --bin uniffi-bindgen generate src/aether.udl --language swift`
- **THEN** the system uses UniFFI 0.28 binding generator
- **AND** generates Swift bindings compatible with existing UDL schema
- **AND** utilizes C string literals (c"...") for improved performance where applicable

#### Scenario: Maintain backward compatibility
- **WHEN** UniFFI bindings are regenerated with version 0.28
- **THEN** the generated Swift API surface SHALL remain unchanged
- **AND** existing Swift code continues to compile without modifications
- **AND** no breaking changes to `AetherCore`, `AetherEventHandler`, or exposed types

#### Scenario: Verify UniFFI build integration
- **WHEN** cargo build executes
- **THEN** uniffi build dependency version 0.28 is used
- **AND** UDL schema validation passes
- **AND** build script generates updated bindings if needed

### Requirement: Dependency Minimization
The system SHALL minimize external dependencies by using standard library equivalents where available, reducing build time and binary size.

#### Scenario: Remove once_cell dependency
- **WHEN** the system uses lazy initialization
- **THEN** it uses `std::sync::OnceLock` or `std::cell::OnceCell` from the standard library
- **AND** the `once_cell` crate is NOT listed in Cargo.toml dependencies
- **AND** all previous `OnceCell` usage is migrated to stdlib equivalents

#### Scenario: Remove async-trait dependency
- **WHEN** the system defines or implements async traits
- **THEN** it uses native `async fn` in trait definitions
- **AND** the `async-trait` crate is NOT listed in Cargo.toml dependencies
- **AND** all previous `#[async_trait]` attributes are removed

#### Scenario: Measure dependency reduction impact
- **WHEN** the build completes after dependency removal
- **THEN** build time SHALL be 5-10% faster than baseline (measured with cargo --timings)
- **AND** release binary size SHALL be 2-5% smaller than baseline
- **AND** dependency tree depth is reduced (verified via `cargo tree`)
