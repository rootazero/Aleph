# Spec Delta: build-integration

## MODIFIED Requirements

### Requirement: Build Script Automation
The build system SHALL automate the process of building the Rust core library with Rust 1.92+ and copying it into the macOS application bundle.

#### Scenario: Automated library copying during Xcode build
- **WHEN** the Xcode project is built
- **THEN** the build script verifies Rust 1.92+ is available via `cargo --version`
- **AND** the build script SHALL automatically copy `libalephcore.dylib` from the Rust target directory to the app's Frameworks folder
- **AND** fails with clear error message if Rust version < 1.92

#### Scenario: Dynamic library path resolution
- **WHEN** the Rust library is copied to the app bundle
- **THEN** the build script SHALL use `install_name_tool` to set the correct `@rpath` for runtime library loading
- **AND** the library is built with UniFFI 0.28 bindings

#### Scenario: Clean system build verification
- **WHEN** the app bundle is deployed to a system without Rust toolchain
- **THEN** the app SHALL launch successfully using the embedded library without external dependencies
- **AND** the library does NOT depend on removed crates (once_cell, async-trait)

## ADDED Requirements

### Requirement: Rust Toolchain Version Validation
The build system SHALL validate that Rust 1.92 or higher is available before attempting to build the core library, providing clear error messages for version mismatches.

#### Scenario: Check Rust version before build
- **WHEN** Xcode build script executes
- **THEN** the script runs `cargo --version` to check installed Rust version
- **AND** parses the version number from output
- **AND** compares against minimum required version (1.92)

#### Scenario: Fail build on insufficient Rust version
- **WHEN** detected Rust version is < 1.92
- **THEN** the build script exits with error code 1
- **AND** prints error message: "Error: Rust 1.92 or higher required. Current version: [X.Y.Z]. Update Rust with 'rustup update'."
- **AND** Xcode build fails with visible error in build log

#### Scenario: Proceed with sufficient Rust version
- **WHEN** detected Rust version is >= 1.92
- **THEN** the build script logs success: "✓ Rust [X.Y.Z] detected (>= 1.92)"
- **AND** proceeds to build the Rust core library
- **AND** uses native async traits and OnceLock from stdlib

### Requirement: UniFFI Binding Regeneration
The build system SHALL regenerate UniFFI Swift bindings if the UDL schema or UniFFI version changes, ensuring bindings stay in sync with Rust definitions.

#### Scenario: Detect UniFFI version change
- **WHEN** Cargo.lock shows UniFFI version upgrade (e.g., 0.25 → 0.28)
- **THEN** the build script detects the version change
- **AND** triggers Swift binding regeneration
- **AND** logs: "UniFFI version changed, regenerating Swift bindings..."

#### Scenario: Regenerate bindings during build
- **WHEN** UniFFI version or UDL schema changes are detected
- **THEN** the build script runs:
  ```
  cargo run --bin uniffi-bindgen generate src/aleph.udl \
    --language swift \
    --out-dir ../Sources/Generated/
  ```
- **AND** verifies the generated `aleph.swift` file is valid
- **AND** fails build if binding generation fails

#### Scenario: Skip regeneration if bindings are up-to-date
- **WHEN** UniFFI version and UDL schema are unchanged
- **THEN** the build script skips binding regeneration
- **AND** uses cached bindings from previous build
- **AND** logs: "✓ UniFFI bindings are up-to-date"
