#!/bin/bash
# Build macOS Aleph.app with embedded aleph
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
MACOS_DIR="$PROJECT_ROOT/apps/macos-native"
BUILD_DIR="$PROJECT_ROOT/target/macos-app"

echo "=== Building aleph (universal binary) ==="

cargo build --bin aleph --features control-plane --release --target aarch64-apple-darwin
cargo build --bin aleph --features control-plane --release --target x86_64-apple-darwin

mkdir -p "$BUILD_DIR"
lipo -create \
    "$PROJECT_ROOT/target/aarch64-apple-darwin/release/aleph" \
    "$PROJECT_ROOT/target/x86_64-apple-darwin/release/aleph" \
    -output "$BUILD_DIR/aleph"

echo "=== Universal binary created ($(du -h "$BUILD_DIR/aleph" | cut -f1)) ==="

# Copy to Xcode project resources
mkdir -p "$MACOS_DIR/Aleph/Resources"
cp "$BUILD_DIR/aleph" "$MACOS_DIR/Aleph/Resources/"

echo "=== Regenerating Xcode project ==="
cd "$MACOS_DIR" && xcodegen generate

echo "=== Building Aleph.app ==="
xcodebuild \
    -project Aleph.xcodeproj \
    -scheme Aleph \
    -configuration Release \
    -derivedDataPath "$BUILD_DIR/DerivedData" \
    clean build

APP_PATH="$BUILD_DIR/DerivedData/Build/Products/Release/Aleph.app"

if [ ! -d "$APP_PATH" ]; then
    echo "ERROR: Aleph.app not found"
    exit 1
fi

echo "=== Aleph.app built successfully ==="
echo "Location: $APP_PATH"
echo "Size: $(du -sh "$APP_PATH" | cut -f1)"

# Optional: Create DMG if create-dmg is available
if command -v create-dmg &>/dev/null; then
    echo "=== Creating DMG ==="
    create-dmg --volname "Aleph" --window-pos 200 120 --window-size 600 400 \
        --icon-size 100 --app-drop-link 450 185 \
        "$BUILD_DIR/Aleph.dmg" "$APP_PATH"
    echo "DMG: $BUILD_DIR/Aleph.dmg"
fi
