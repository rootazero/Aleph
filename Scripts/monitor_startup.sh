#!/bin/bash
# Monitor Aleph startup process in real-time

echo "=== Aleph 启动监控 ==="
echo ""
echo "请在 Xcode 中点击 Run 按钮启动 Aleph"
echo "等待 5 秒后开始监控..."
echo ""

sleep 5

echo "🔍 正在检查 Aleph 进程..."
AETHER_PID=$(pgrep -x "Aleph" | head -1)

if [ -z "$AETHER_PID" ]; then
    echo "❌ 未找到 Aleph 进程"
    echo ""
    echo "可能的原因："
    echo "1. 应用启动失败（检查 Xcode 控制台错误）"
    echo "2. 应用立即崩溃（检查崩溃日志）"
    echo "3. 应用未从 Xcode 启动"
    exit 1
fi

echo "✅ 找到 Aleph 进程 (PID: $AETHER_PID)"
echo ""

# Get full path
AETHER_PATH=$(ps -p $AETHER_PID -o command= | awk '{print $1}')
APP_PATH=$(dirname $(dirname "$AETHER_PATH"))

echo "📦 应用路径: $APP_PATH"
echo ""

# Check if menu bar icon is visible
echo "✅ Aleph 应该在菜单栏显示图标"
echo ""

# Monitor Aleph-related entries in system logs
echo "📊 查看最近的系统日志..."
log show --predicate 'process == "Aleph"' --info --last 30s 2>/dev/null | \
    grep -E "\[Aleph\]|Error|accessibility|hotkey|rdev" | \
    tail -20 || echo "   (无法访问系统日志)"

echo ""
echo "=== 诊断信息 ==="
echo ""
echo "请从 Xcode 控制台复制以下内容："
echo "1. 所有 [Aleph] 开头的日志"
echo "2. 任何错误或警告信息"
echo ""
echo "同时请确认："
echo "□ 菜单栏是否显示 Aleph 图标"
echo "□ 是否弹出了 Accessibility 权限请求"
echo "□ 系统设置 → 隐私与安全性 → 辅助功能 中是否有 Aleph"
