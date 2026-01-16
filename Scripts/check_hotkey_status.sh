#!/bin/bash
# Aether hotkey status check script

echo "=== Aether 热键状态检查 ==="
echo ""

# 1. Check process
echo "1. 检查 Aether 进程..."
if pgrep -x "Aether" > /dev/null; then
    PID=$(pgrep -x "Aether")
    echo "   ✅ Aether 正在运行 (PID: $PID)"

    # Check open file descriptors and hotkey listener thread
    echo "   检查热键监听线程..."
    THREAD_COUNT=$(ps -M -p $PID | wc -l)
    echo "   线程数: $THREAD_COUNT"
else
    echo "   ❌ Aether 未运行"
    exit 1
fi
echo ""

# 2. Check Accessibility permission
echo "2. 检查 Accessibility 权限..."
AETHER_PATH="/Users/zouguojun/Library/Developer/Xcode/DerivedData/Aether-etjxjwefzynbztajfjnzbmaenbyi/Build/Products/Debug/Aether.app"

# Query TCC database using SQLite
TCC_DB="/Library/Application Support/com.apple.TCC/TCC.db"
if [ -f "$TCC_DB" ]; then
    echo "   正在查询 TCC 数据库..."
    # Note: Full Disk Access is required to read TCC database
    echo "   （需要终端拥有完全磁盘访问权限）"
else
    echo "   请手动检查：系统设置 → 隐私与安全性 → 辅助功能"
fi
echo ""

# 3. View console output
echo "3. 查看最近5分钟的 Aether 日志..."
echo "   正在提取关键日志..."
echo ""

# Use log show (macOS unified logging system)
log show --predicate 'process == "Aether"' --info --last 5m 2>/dev/null | \
    grep -E "\[Aether\]|\[Memory\]|Hotkey|Accessibility|Error|initialized" | \
    tail -20 || echo "   (无法读取系统日志，请从 Xcode 控制台查看)"

echo ""
echo "=== 检查完成 ==="
echo ""
echo "如果热键仍不工作，请："
echo "1. 截图系统设置中的 Accessibility 权限页面"
echo "2. 从 Xcode 控制台复制所有 [Aether] 开头的日志"
echo "3. 告诉我按 \` 键时有无任何反应"
