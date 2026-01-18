#!/bin/bash
# build-macos.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CORE_DIR="$ROOT_DIR/core"
MACOS_DIR="$ROOT_DIR/platforms/macos"

CONFIG=${1:-Release} # Release 或 Debug

echo "🦀 Building Rust core..."
cd "$CORE_DIR"
if [ "$CONFIG" = "Release" ]; then
  cargo build --release --features uniffi
  RUST_LIB="target/release/libaethecore.dylib"
else
  cargo build --features uniffi
  RUST_LIB="target/debug/libaethecore.dylib"
fi

# 生成 Swift 绑定
cargo run --bin uniffi-bindgen generate src/aether.udl \
  --language swift \
  --out-dir "$MACOS_DIR/Aether/Sources/Generated/"

# 复制库文件
cp "$RUST_LIB" "$MACOS_DIR/Aether/Frameworks/"

echo "🍎 Building macOS app..."
cd "$MACOS_DIR"

# 清理 DerivedData
rm -rf ~/Library/Developer/Xcode/DerivedData/Aether-*

# 生成 Xcode 项目
xcodegen generate

# 构建
xcodebuild -project Aether.xcodeproj \
  -scheme Aether \
  -configuration "$CONFIG" \
  build

echo "✅ Build complete!"

# 打开应用
if [ "$2" = "--run" ]; then
  APP_PATH=$(find ~/Library/Developer/Xcode/DerivedData/Aether-*/Build/Products/"$CONFIG"/ -name "Aether.app" -type d | head -1)
  open "$APP_PATH"
fi
