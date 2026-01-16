#!/bin/bash
# Check Aether Accessibility permission status

echo "=== Aether Accessibility 权限检查 ==="
echo ""

# Get running Aether path
AETHER_PID=$(pgrep -x "Aether" 2>/dev/null | head -1)

if [ -z "$AETHER_PID" ]; then
    echo "❌ Aether 未运行"
    echo ""
    echo "请先在 Xcode 中运行 Aether"
    exit 1
fi

echo "✅ Aether 正在运行 (PID: $AETHER_PID)"

AETHER_PATH=$(ps -p $AETHER_PID -o command= | awk '{print $1}')
APP_PATH=$(dirname $(dirname "$AETHER_PATH"))

echo "📦 应用路径: $APP_PATH"
echo ""

# Check TCC database (user level)
USER_TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"

if [ -f "$USER_TCC_DB" ]; then
    echo "🔍 查询 TCC 数据库..."

    # Query Accessibility permission
    RESULT=$(sqlite3 "$USER_TCC_DB" \
        "SELECT allowed FROM access WHERE service='kTCCServiceAccessibility' AND client LIKE '%Aether%';" \
        2>/dev/null | tail -1)

    if [ "$RESULT" = "1" ]; then
        echo "✅ Accessibility 权限已授予！"
        echo ""
        echo "现在可以测试热键功能："
        echo "1. 切换到英文输入法"
        echo "2. 在任意应用中选中文字"
        echo "3. 按 \` 键（键盘左上角）"
    elif [ "$RESULT" = "0" ]; then
        echo "❌ Accessibility 权限已拒绝"
        echo ""
        echo "请前往: 系统设置 → 隐私与安全性 → 辅助功能"
        echo "勾选 Aether 的复选框"
    else
        echo "⚠️  未找到 Aether 的权限记录"
        echo ""
        echo "请手动检查:"
        echo "系统设置 → 隐私与安全性 → 辅助功能"
        echo ""
        echo "需要添加的应用:"
        echo "$APP_PATH"
    fi
else
    echo "⚠️  无法访问 TCC 数据库"
    echo ""
    echo "请手动检查:"
    echo "系统设置 → 隐私与安全性 → 辅助功能"
fi

echo ""
echo "=== 检查完成 ==="
