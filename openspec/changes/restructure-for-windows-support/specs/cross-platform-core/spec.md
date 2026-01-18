# Cross-Platform Core Specification

## Overview

This spec defines requirements for the shared Rust core library that serves both macOS and Windows platforms.

## ADDED Requirements

### Requirement: Platform-agnostic FFI boundary

The Rust core MUST support multiple FFI strategies through compile-time feature selection.

#### Scenario: Building for macOS with UniFFI

**Given** the developer wants to build for macOS
**When** running `cargo build --features uniffi`
**Then** the build should produce `libaethecore.dylib`
**And** UniFFI scaffolding should be included
**And** Swift bindings should be generatable from the built library

#### Scenario: Building for Windows with C ABI

**Given** the developer wants to build for Windows
**When** running `cargo build --features cabi --target x86_64-pc-windows-msvc`
**Then** the build should produce `aethecore.dll`
**And** C# binding file should be generated via csbindgen
**And** no UniFFI scaffolding should be included

#### Scenario: Building core without platform features

**Given** the developer wants to run tests or build core only
**When** running `cargo build` without features
**Then** the build should succeed
**And** only core library logic should be included
**And** no platform-specific FFI code should be compiled

### Requirement: Workspace-based dependency management

All Rust dependencies MUST be centrally managed through Cargo workspace.

#### Scenario: Adding a new dependency

**Given** a developer needs to add a new crate dependency
**When** they add it to `[workspace.dependencies]` in root `Cargo.toml`
**And** reference it in `core/Cargo.toml` with `dep_name.workspace = true`
**Then** the dependency should be available in the core crate
**And** version should be controlled at workspace level

### Requirement: Version synchronization

All components MUST share a single version source.

#### Scenario: Checking version consistency

**Given** the `/VERSION` file contains `0.1.0`
**When** building the Rust core
**And** building the macOS app
**And** building the Windows app
**Then** all built artifacts should report version `0.1.0`

---

## MODIFIED Requirements

### Requirement: Conditional UniFFI scaffolding (Modified)

UniFFI scaffolding MUST be conditional on the `uniffi` feature flag.

Previously: UniFFI scaffolding was always included.
Now: UniFFI scaffolding is conditional on the `uniffi` feature.

#### Scenario: UniFFI macro with feature flag

**Given** the `lib.rs` contains `uniffi::include_scaffolding!("aether")`
**When** the code is compiled without `uniffi` feature
**Then** the macro should not be invoked
**And** no UniFFI types should be generated

---

## Cross-References

- Related: `uniffi-bridge` spec (existing macOS FFI requirements)
- Related: `windows-ffi` spec (new Windows FFI requirements)
- Related: `core-library` spec (core functionality requirements)
