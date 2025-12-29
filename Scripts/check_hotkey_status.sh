#!/bin/bash
# Aether 热键状态检查脚本

echo "=== Aether 热键状态检查 ==="
echo ""

# 1. 检查进程
echo "1. 检查 Aether 进程..."
if pgrep -x "Aether" > /dev/null; then
    PID=$(pgrep -x "Aether")
    echo "   ✅ Aether 正在运行 (PID: $PID)"

    # 检查打开的文件描述符
    echo "   检查热键监听线程..."
    THREAD_COUNT=$(ps -M -p $PID | wc -l)
    echo "   线程数: $THREAD_COUNT"
else
    echo "   ❌ Aether 未运行"
    exit 1
fi
echo ""

# 2. 检查 Accessibility 权限
echo "2. 检查 Accessibility 权限..."
AETHER_PATH="/Users/zouguojun/Library/Developer/Xcode/DerivedData/Aether-etjxjwefzynbztajfjnzbmaenbyi/Build/Products/Debug/Aether.app"

# 使用 SQLite 查询 TCC 数据库
TCC_DB="/Library/Application Support/com.apple.TCC/TCC.db"
if [ -f "$TCC_DB" ]; then
    echo "   正在查询 TCC 数据库..."
    # 注意：需要完全磁盘访问权限才能读取 TCC 数据库
    echo "   （需要终端拥有完全磁盘访问权限）"
else
    echo "   请手动检查：系统设置 → 隐私与安全性 → 辅助功能"
fi
echo ""

# 3. 查看控制台输出
echo "3. 查看最近5分钟的 Aether 日志..."
echo "   正在提取关键日志..."
echo ""

# 使用 log show (macOS 统一日志系统)
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
