# build-integration Specification

## Purpose
TBD - created by archiving change complete-phase2-testing-and-polish. Update Purpose after archive.
## Requirements
### Requirement: Build Script Automation
The build system SHALL automate the process of copying and configuring the Rust core library into the macOS application bundle.

#### Scenario: Automated library copying during Xcode build
- **WHEN** the Xcode project is built
- **THEN** the build script SHALL automatically copy `libaethecore.dylib` from the Rust target directory to the app's Frameworks folder

#### Scenario: Dynamic library path resolution
- **WHEN** the Rust library is copied to the app bundle
- **THEN** the build script SHALL use `install_name_tool` to set the correct `@rpath` for runtime library loading

#### Scenario: Clean system build verification
- **WHEN** the app bundle is deployed to a system without Rust toolchain
- **THEN** the app SHALL launch successfully using the embedded library without external dependencies

### Requirement: Xcode Build Phase Integration
The Xcode project SHALL include a build phase that executes the library copying script before compilation.

#### Scenario: Build phase execution order
- **WHEN** Xcode builds the project
- **THEN** the "Run Script" phase SHALL execute before "Compile Sources" to ensure library availability

#### Scenario: Build failure on missing library
- **WHEN** the Rust library is not built
- **THEN** the build script SHALL fail with a clear error message indicating the missing dependency

### Requirement: Runtime Path Configuration
The application bundle SHALL be configured with correct runtime search paths for dynamic library loading.

#### Scenario: @rpath configuration
- **WHEN** the application is built
- **THEN** the Xcode project SHALL set `@rpath` to `@executable_path/../Frameworks` for library discovery

#### Scenario: Library install name
- **WHEN** the Rust library is embedded in the bundle
- **THEN** its install name SHALL be set to `@rpath/libaethecore.dylib` for proper loading

