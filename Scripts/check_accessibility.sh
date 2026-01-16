#!/bin/bash
# Check Aether's Accessibility permission status

echo "=== Aether Accessibility 权限检查 ==="
echo ""

# Find the running Aether application path
AETHER_PATH=$(ps aux | grep -i "Aether.app/Contents/MacOS/Aether" | grep -v grep | awk '{for(i=11;i<=NF;i++) printf "%s ", $i; print ""}' | sed 's/[[:space:]]*$//' | head -1)

if [ -z "$AETHER_PATH" ]; then
    echo "❌ Aether 未运行"
    echo "   请从 Xcode 启动 Aether 后再运行此脚本"
    exit 1
fi

echo "✅ 找到 Aether 进程"
echo "   路径: $AETHER_PATH"
echo ""

# Extract application bundle path
APP_PATH=$(echo "$AETHER_PATH" | sed 's/\/Contents\/MacOS\/Aether.*/\.app/')

echo "📦 应用包路径:"
echo "   $APP_PATH"
echo ""

# Check if application bundle exists
if [ ! -d "$APP_PATH" ]; then
    echo "❌ 应用包不存在: $APP_PATH"
    exit 1
fi

echo "✅ 应用包存在"
echo ""

# Query TCC database (requires Full Disk Access permission)
echo "🔍 检查 Accessibility 权限..."
echo ""
echo "请手动检查以下路径:"
echo "   系统设置 → 隐私与安全性 → 辅助功能"
echo ""
echo "需要添加的应用路径:"
echo "   $APP_PATH"
echo ""
echo "如果列表中已经有 Aether，请："
echo "   1. 取消勾选"
echo "   2. 重新勾选"
echo "   3. 关闭系统设置"
echo "   4. 从 Xcode 重新运行 Aether (停止后重新 Run)"
echo ""

# Try to query TCC database using sqlite3 (may require permissions)
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
if [ -f "$TCC_DB" ]; then
    echo "📊 尝试查询 TCC 数据库..."
    sqlite3 "$TCC_DB" "SELECT service, client, allowed FROM access WHERE service='kTCCServiceAccessibility' AND client LIKE '%Aether%';" 2>/dev/null || echo "   (需要终端拥有完全磁盘访问权限才能查询)"
fi

echo ""
echo "=== 检查完成 ==="
