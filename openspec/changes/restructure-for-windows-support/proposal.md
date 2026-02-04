# Proposal: Restructure for Windows Support

## Summary

Restructure the Aleph project from its current macOS-centric layout to a Monorepo architecture that enables Windows platform support while maintaining full macOS functionality. This change establishes the foundation for cross-platform development with shared Rust core and platform-specific native UIs.

## Motivation

### Current State
- Project structure is macOS-centric with `Aleph/` containing both Swift sources and Rust core
- Rust core (`Aleph/core/`) is tightly coupled with macOS directory structure
- No clear separation between platform-specific and shared code
- Build scripts assume macOS-only workflow

### Desired State
- Clean Monorepo structure with platform directories under `platforms/`
- Shared Rust core at repository root (`core/`)
- Platform-specific FFI strategies (UniFFI for macOS, csbindgen for Windows)
- Independent CI/CD pipelines per platform
- Single version source of truth

### Why Now
- Windows support was always planned (noted in project.md as "Future")
- Current codebase has reached Phase 8 stability
- Architecture is mature enough for platform expansion
- Restructuring now prevents accumulated technical debt

## Scope

### In Scope
1. **Directory Restructure**: Move to `platforms/macos/`, `platforms/windows/`, `core/`, `shared/` layout
2. **Cargo Workspace**: Convert to workspace with `core` as member
3. **Feature Flags**: Add `uniffi` and `cabi` features for platform-specific FFI
4. **Build Scripts**: Create cross-platform build automation
5. **CI/CD Scaffolding**: GitHub Actions workflows for both platforms
6. **Documentation Updates**: Update CLAUDE.md and docs/ for new structure

### Out of Scope
1. Actual Windows UI implementation (C# + WinUI 3) - future proposal
2. Linux support (GTK4) - future proposal
3. Functional changes to Rust core logic
4. Changes to macOS UI code (only path adjustments)

## Approach

### Phase 1: Prepare (Non-Breaking)
- Add Cargo workspace configuration at root
- Add feature flags to `core/Cargo.toml` (currently `Aleph/core/Cargo.toml`)
- Create placeholder directories

### Phase 2: Migrate
- Move `Aleph/core/` to `core/`
- Move `Aleph/` to `platforms/macos/Aleph/`
- Update all path references (XcodeGen, scripts, imports)
- Create `shared/` for common resources

### Phase 3: Scaffold Windows
- Create `platforms/windows/` structure
- Add csbindgen to Rust core build
- Create placeholder C# project files
- Add Windows CI workflow

## Dependencies

- Rust 1.70+ (already required)
- csbindgen 1.9+ (new dependency for Windows FFI)
- .NET 8.0 SDK (for Windows build CI)
- No runtime dependencies change

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Breaking macOS build | High | Incremental migration with CI verification at each step |
| Path reference issues | Medium | Automated script to update paths + comprehensive testing |
| Git history disruption | Low | Use `git mv` for moves, maintain file history |
| CI pipeline complexity | Medium | Separate workflows with path-based triggers |

## Success Criteria

1. ✅ macOS app builds and runs identically after restructure
2. ✅ `cargo test` passes at workspace root
3. ✅ UniFFI binding generation works from new location
4. ✅ Windows placeholder project exists with CI workflow
5. ✅ Documentation accurately reflects new structure

## Timeline

- **Phase 1**: Prepare workspace and features
- **Phase 2**: Migrate directories and update references
- **Phase 3**: Scaffold Windows project structure

## Related Changes

- None (this is foundational)

## References

- [Aleph Architecture](../../docs/ARCHITECTURE.md)
- [XcodeGen Workflow](../../docs/XCODEGEN_README.md)
- [csbindgen documentation](https://github.com/Cysharp/csbindgen)
