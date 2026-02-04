#!/bin/bash
# Generate FFI bindings for all platforms
# Usage: ./generate-bindings.sh [all|macos|windows]

set -e

TARGET=${1:-"all"}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CORE_DIR="$ROOT_DIR/core"

generate_macos() {
    echo "🍎 Generating macOS (UniFFI) bindings..."
    cd "$ROOT_DIR"

    # Ensure library is built
    cargo build --release --features uniffi -p alephcore

    # Generate Swift bindings
    cd "$CORE_DIR"
    cargo run --bin uniffi-bindgen generate \
        --library ../target/release/libalephcore.dylib \
        --language swift \
        --out-dir "$ROOT_DIR/platforms/macos/Aleph/Sources/Generated/"

    echo "✅ macOS bindings generated at platforms/macos/Aleph/Sources/Generated/"
}

generate_windows() {
    echo "🪟 Generating Windows (csbindgen) bindings..."
    cd "$CORE_DIR"

    # Build with cabi feature - csbindgen runs in build.rs
    cargo build --release --features cabi

    # Check if bindings were generated
    BINDINGS_PATH="$ROOT_DIR/platforms/windows/Aleph/Interop/NativeMethods.g.cs"
    if [ -f "$BINDINGS_PATH" ]; then
        echo "✅ Windows bindings generated at platforms/windows/Aleph/Interop/"
    else
        echo "⚠️  Windows bindings not found. Check build.rs csbindgen configuration."
    fi
}

case $TARGET in
    macos)  generate_macos ;;
    windows) generate_windows ;;
    all)
        generate_macos
        generate_windows
        ;;
    *)
        echo "Usage: $0 [all|macos|windows]"
        exit 1
        ;;
esac
