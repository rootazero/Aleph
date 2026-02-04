#!/bin/bash
# Build Rust core library for specified platform(s)
# Usage: ./build-core.sh [all|macos|windows]

set -e

TARGET=${1:-"all"}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CORE_DIR="$ROOT_DIR/core"

build_macos() {
    echo "🍎 Building for macOS..."
    cd "$CORE_DIR"

    # Build dylib with UniFFI feature
    cargo build --release --features uniffi

    # Generate Swift bindings
    # Note: target directory is at workspace root, not core directory
    cargo run --bin uniffi-bindgen generate \
        --library "$ROOT_DIR/target/release/libaethecore.dylib" \
        --language swift \
        --out-dir "$ROOT_DIR/platforms/macos/Aleph/Sources/Generated/"

    # Copy library (use /bin/rm and /bin/cp to avoid alias issues)
    /bin/rm -f "$ROOT_DIR/platforms/macos/Aleph/Frameworks/libaethecore.dylib"
    /bin/cp "$ROOT_DIR/target/release/libaethecore.dylib" \
        "$ROOT_DIR/platforms/macos/Aleph/Frameworks/"

    # Fix install_name for portability
    install_name_tool -id "@rpath/libaethecore.dylib" \
        "$ROOT_DIR/platforms/macos/Aleph/Frameworks/libaethecore.dylib"

    echo "✅ macOS build complete"
}

build_windows() {
    echo "🪟 Building for Windows..."
    cd "$CORE_DIR"

    # Cross-compile for Windows (requires appropriate toolchain)
    # On Windows, omit --target flag
    if [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "win32" ]]; then
        cargo build --release --features cabi
        cp target/release/aethecore.dll "$ROOT_DIR/platforms/windows/Aleph/libs/"
    else
        # Cross-compile from macOS/Linux
        cargo build --release --features cabi --target x86_64-pc-windows-msvc
        cp target/x86_64-pc-windows-msvc/release/aethecore.dll \
            "$ROOT_DIR/platforms/windows/Aleph/libs/" 2>/dev/null || \
            echo "⚠️  Windows cross-compile not available on this system"
    fi

    echo "✅ Windows build complete (or skipped if cross-compile unavailable)"
}

case $TARGET in
    macos)  build_macos ;;
    windows) build_windows ;;
    all)
        build_macos
        build_windows
        ;;
    *)
        echo "Usage: $0 [all|macos|windows]"
        exit 1
        ;;
esac
