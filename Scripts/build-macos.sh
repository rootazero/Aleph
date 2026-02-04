#!/bin/bash
# Build complete macOS application
# Usage: ./build-macos.sh [debug|release]

set -e

CONFIG=${1:-"release"}
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
MACOS_DIR="$ROOT_DIR/platforms/macos"

echo "🍎 Building macOS app (${CONFIG})..."

# Step 1: Build Rust core
echo "📦 Building Rust core..."
cd "$ROOT_DIR/core"
if [ "$CONFIG" = "debug" ]; then
    cargo build --features uniffi
    LIB_PATH="$ROOT_DIR/target/debug/libaethecore.dylib"
else
    cargo build --release --features uniffi
    LIB_PATH="$ROOT_DIR/target/release/libaethecore.dylib"
fi

# Step 2: Copy library
echo "📋 Copying library..."
cp "$LIB_PATH" "$MACOS_DIR/Aleph/Frameworks/"
install_name_tool -id "@rpath/libaethecore.dylib" \
    "$MACOS_DIR/Aleph/Frameworks/libaethecore.dylib"

# Step 3: Generate UniFFI bindings
echo "🔗 Generating UniFFI bindings..."
cargo run --bin uniffi-bindgen generate \
    --library "$LIB_PATH" \
    --language swift \
    --out-dir "$MACOS_DIR/Aleph/Sources/Generated/"

# Step 4: Generate Xcode project
echo "🛠️ Generating Xcode project..."
cd "$MACOS_DIR"
xcodegen generate

# Step 5: Build with xcodebuild
echo "🏗️ Building with xcodebuild..."
if [ "$CONFIG" = "debug" ]; then
    xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build
else
    xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Release build
fi

echo "✅ macOS build complete!"
