# Windows FFI Specification

## Overview

This spec defines requirements for the Windows platform FFI layer using csbindgen to generate C# P/Invoke bindings.

## ADDED Requirements

### Requirement: C ABI export functions

The Rust core MUST expose essential functions through C ABI for Windows consumption.

#### Scenario: Initializing the core

**Given** a Windows application wants to initialize Aleph core
**When** calling `aleph_init(config_path)` via P/Invoke
**Then** the function should return `0` on success
**And** the function should return non-zero error code on failure
**And** the config file at the specified path should be loaded

#### Scenario: Getting version string

**Given** a Windows application wants to display version
**When** calling `aleph_version()` via P/Invoke
**Then** the function should return a pointer to a null-terminated UTF-8 string
**And** the string should match the VERSION file content

#### Scenario: Freeing allocated resources

**Given** the application has initialized the core
**When** calling `aleph_free()` via P/Invoke
**Then** all Rust-allocated resources should be released
**And** subsequent calls to other functions should fail gracefully

### Requirement: Callback registration for events

Windows applications MUST be able to register callbacks for async events.

#### Scenario: Registering state change callback

**Given** a Windows application implements a callback function
**When** calling `aleph_register_state_callback(callback_ptr)`
**Then** the callback should be stored in Rust
**And** when processing state changes, the callback should be invoked
**And** the callback should receive state enum value as integer

#### Scenario: Registering streaming text callback

**Given** a Windows application implements a text callback function
**When** calling `aleph_register_stream_callback(callback_ptr)`
**Then** streaming text chunks should be delivered to the callback
**And** text should be passed as null-terminated UTF-8 strings

### Requirement: csbindgen integration

C# bindings MUST be auto-generated during build.

#### Scenario: Building with cabi feature

**Given** the `cabi` feature is enabled
**And** `build.rs` contains csbindgen configuration
**When** running `cargo build --features cabi`
**Then** `NativeMethods.g.cs` should be generated
**And** the file should contain P/Invoke declarations for all exported functions
**And** the output path should be `platforms/windows/Aleph/Interop/`

#### Scenario: Type mapping in generated bindings

**Given** Rust exports a function with `*const c_char` parameter
**When** csbindgen generates C# bindings
**Then** the parameter should map to `byte*` or appropriate C# type
**And** marshaling attributes should be included where necessary

### Requirement: Thread safety for callbacks

All callbacks MUST be invoked safely across thread boundaries.

#### Scenario: Callback from async Rust task

**Given** an async operation completes on a Tokio worker thread
**When** the completion callback needs to be invoked
**Then** the callback must be invoked safely (may require synchronization)
**And** the callback must not assume it's on the main thread

---

## Cross-References

- Related: `cross-platform-core` spec (shared core requirements)
- Related: `uniffi-bridge` spec (contrast with macOS approach)
