#!/bin/bash
# Generate Swift bindings using uniffi-bindgen from the built library

set -e

# Build directory
BUILD_DIR="target/debug"

# Check if library exists
if [ ! -f "$BUILD_DIR/libaethecore.dylib" ]; then
    echo "Error: Library not found at $BUILD_DIR/libaethecore.dylib"
    echo "Run 'cargo build' first"
    exit 1
fi

# Output directory
OUT_DIR="bindings"
mkdir -p "$OUT_DIR"

# Generate bindings using the library's uniffi metadata
# This uses the uniffi-bindgen executable from the uniffi_bindgen crate
cargo run --features=uniffi/cli --bin uniffi-bindgen -- generate \
    --library "$BUILD_DIR/libaethecore.dylib" \
    --language swift \
    --out-dir "$OUT_DIR" \
    src/aether.udl

echo "✅ Swift bindings generated in $OUT_DIR/"
