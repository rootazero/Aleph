#!/bin/bash
# Quick Build Script for Aether
# 快速构建脚本

set -e  # Exit on error

echo "🚀 Aether Quick Build Script"
echo "=============================="

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Project root
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

echo ""
echo "📦 Step 1/5: Building Rust Core (Release mode)..."
cd Aether/core
/Users/zouguojun/.cargo/bin/cargo build --release
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Rust Core built successfully"
else
    echo -e "${RED}✗${NC} Rust Core build failed"
    exit 1
fi

echo ""
echo "🔗 Step 2/5: Generating UniFFI Swift bindings..."
/Users/zouguojun/.cargo/bin/cargo run --bin uniffi-bindgen -- \
    generate --library target/release/libaethecore.dylib \
    --language swift \
    --out-dir ../Sources/Generated/
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} UniFFI bindings generated"
else
    echo -e "${RED}✗${NC} UniFFI binding generation failed"
    exit 1
fi

cd "$PROJECT_ROOT"

echo ""
echo "📋 Step 3/5: Copying dylib to Frameworks..."
mkdir -p Aether/Frameworks
cp Aether/core/target/release/libaethecore.dylib Aether/Frameworks/
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} dylib copied to Frameworks"
    ls -lh Aether/Frameworks/libaethecore.dylib
else
    echo -e "${RED}✗${NC} Failed to copy dylib"
    exit 1
fi

echo ""
echo "🏗️  Step 4/5: Generating Xcode project..."
xcodegen generate
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Xcode project generated"
else
    echo -e "${RED}✗${NC} XcodeGen failed"
    exit 1
fi

echo ""
echo "🎯 Step 5/5: Opening project in Xcode..."
open Aether.xcodeproj
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓${NC} Xcode project opened"
else
    echo -e "${YELLOW}⚠${NC}  Failed to open Xcode, but build succeeded"
fi

echo ""
echo -e "${GREEN}=============================="
echo "✅ Build completed successfully!"
echo "==============================${NC}"
echo ""
echo "Next steps:"
echo "1. In Xcode, select 'Aether' scheme and 'My Mac' target"
echo "2. Press Cmd+R to build and run"
echo "3. Look for the ✨ icon in the menu bar"
echo ""
echo "📝 See TESTING_CHECKLIST.md for functional testing guide"
echo ""
