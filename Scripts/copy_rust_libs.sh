#!/bin/bash
# Build script to copy Rust core library into macOS app bundle
# This script is executed as an Xcode build phase

set -e  # Exit on error

# Color output for better visibility
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "🦀 Copying Rust core library..."

# Detect architecture
ARCH=$(uname -m)
echo "📦 Architecture: $ARCH"

# Source and destination paths
RUST_LIB_PATH="${SRCROOT}/Aleph/core/target/release/libaethecore.dylib"
FRAMEWORKS_PATH="${BUILT_PRODUCTS_DIR}/${FRAMEWORKS_FOLDER_PATH}"
DEST_LIB_PATH="${FRAMEWORKS_PATH}/libaethecore.dylib"

# Check if Rust library exists
if [ ! -f "$RUST_LIB_PATH" ]; then
    echo -e "${RED}❌ Error: Rust library not found at $RUST_LIB_PATH${NC}"
    echo -e "${YELLOW}💡 Please build the Rust core first:${NC}"
    echo -e "   cd Aleph/core && cargo build --release"
    exit 1
fi

echo "✅ Found Rust library: $RUST_LIB_PATH"

# Create Frameworks directory if it doesn't exist
mkdir -p "$FRAMEWORKS_PATH"

# Copy library to app bundle
echo "📋 Copying library to: $DEST_LIB_PATH"
cp "$RUST_LIB_PATH" "$DEST_LIB_PATH"

# Fix install name for runtime loading
echo "🔧 Setting install name to @rpath/libaethecore.dylib"
install_name_tool -id "@rpath/libaethecore.dylib" "$DEST_LIB_PATH"

# Verify the library was copied and fixed
if [ -f "$DEST_LIB_PATH" ]; then
    echo -e "${GREEN}✅ Library copied and configured successfully!${NC}"
    echo "🔍 Install name verification:"
    otool -L "$DEST_LIB_PATH" | grep -A 1 "libaethecore.dylib" || true
else
    echo -e "${RED}❌ Error: Failed to copy library${NC}"
    exit 1
fi

echo "🎉 Rust library integration complete!"
