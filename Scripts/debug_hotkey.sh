#!/bin/bash
# Aether 热键诊断脚本

echo "=== Aether 热键诊断工具 ==="
echo ""

# 1. 检查应用是否在运行
echo "1. 检查 Aether 应用状态..."
if pgrep -x "Aether" > /dev/null; then
    echo "   ✅ Aether 正在运行"
    echo "   进程 ID: $(pgrep -x Aether)"
else
    echo "   ❌ Aether 未运行"
    echo "   请先启动 Aether 应用"
fi
echo ""

# 2. 检查 Accessibility 权限
echo "2. 检查 Accessibility 权限..."
echo "   请手动检查: 系统设置 → 隐私与安全性 → 辅助功能"
echo "   确保 'Aether' 已勾选"
echo ""

# 3. 检查 Rust core 库
echo "3. 检查 Rust core 库..."
DYLIB_PATH="Aether/Frameworks/libaethecore.dylib"
if [ -f "$DYLIB_PATH" ]; then
    echo "   ✅ 找到 Rust core 库: $DYLIB_PATH"
    echo "   文件大小: $(ls -lh "$DYLIB_PATH" | awk '{print $5}')"
    echo "   修改时间: $(ls -l "$DYLIB_PATH" | awk '{print $6, $7, $8}')"
else
    echo "   ❌ 未找到 Rust core 库"
    echo "   请运行: cd Aether/core && cargo build"
fi
echo ""

# 4. 查看最近的应用日志
echo "4. 查看最近的应用日志..."
echo "   运行以下命令查看实时日志:"
echo "   log stream --predicate 'processImagePath contains \"Aether\"' --level debug"
echo ""

# 5. 测试热键
echo "5. 热键配置信息..."
echo "   默认热键: Cmd + \` (Command + 反引号)"
echo "   备选方式: 单独按 \` 键"
echo ""
echo "   ⚠️  注意事项:"
echo "   - 请在英文输入法下测试"
echo "   - \` 键通常在键盘左上角（1键左边）"
echo "   - 确保没有其他应用占用此快捷键"
echo ""

echo "=== 诊断完成 ==="
echo ""
echo "如果问题仍未解决，请提供以下信息:"
echo "1. 控制台中是否有 '[Aether] Hotkey listening started' 消息"
echo "2. 是否授予了 Accessibility 权限"
echo "3. 运行 'log stream' 命令后按热键时的输出"
