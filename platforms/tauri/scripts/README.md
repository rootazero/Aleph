# Aether Tauri Build Scripts

This directory contains scripts for building and releasing the Tauri cross-platform version of Aether.

## Scripts

### `build.sh`
Build the application for one or all platforms.

```bash
# Build for current platform (debug)
./scripts/build.sh

# Build for specific platform
./scripts/build.sh macos
./scripts/build.sh windows
./scripts/build.sh linux

# Build for release
./scripts/build.sh macos --release

# Build all platforms
./scripts/build.sh all --release
```

### `ci-local.sh`
Run the same checks that CI runs, locally. This is useful for verifying your changes before pushing.

```bash
./scripts/ci-local.sh
```

This will:
1. Install dependencies
2. Run linter
3. Run type checker
4. Run tests
5. Build frontend
6. Run Rust check
7. Run Rust clippy

### `release.sh`
Prepare a new release by updating version numbers and creating a git tag.

```bash
./scripts/release.sh 0.2.0
```

This will:
1. Update version in `package.json`, `tauri.conf.json`, and `Cargo.toml`
2. Run CI checks
3. Commit the version bump
4. Create a git tag

After running, push the commit and tag to trigger the release CI:
```bash
git push
git push origin v0.2.0
```

## CI/CD Workflows

The following GitHub Actions workflows are available:

### `tauri-app.yml`
Triggered on push/PR to `platforms/tauri/**` or `core/**`.
- Runs lint and typecheck
- Builds for macOS, Windows, and Linux
- Runs tests
- Uploads build artifacts

### `tauri-release.yml`
Triggered on version tags (`v*`) or manual dispatch.
- Creates a GitHub release
- Builds for all platforms
- Uploads installers to the release
- Publishes the release

## Build Outputs

| Platform | Output Location | Formats |
|----------|-----------------|---------|
| macOS | `src-tauri/target/universal-apple-darwin/release/bundle/` | `.dmg`, `.app` |
| Windows | `src-tauri/target/release/bundle/` | `.exe` (NSIS), `.msi` |
| Linux | `src-tauri/target/release/bundle/` | `.deb`, `.AppImage` |

## Prerequisites

- Node.js 20+
- pnpm (recommended) or npm
- Rust 1.70+
- Platform-specific dependencies:
  - **macOS**: Xcode Command Line Tools
  - **Windows**: Visual Studio Build Tools, WebView2
  - **Linux**: `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libappindicator3-dev`
