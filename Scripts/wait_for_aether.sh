#!/bin/bash
# 实时检查 Aether 启动状态

echo "🔄 等待 Aether 启动..."
echo "   请在 Xcode 中点击 Run 按钮 (▶️)"
echo ""

for i in {1..10}; do
    sleep 1

    AETHER_PID=$(pgrep -x "Aether" | head -1)

    if [ -n "$AETHER_PID" ]; then
        echo "✅ Aether 已启动！(PID: $AETHER_PID)"
        echo ""

        # 获取应用路径
        AETHER_PATH=$(ps -p $AETHER_PID -o command= | awk '{print $1}')
        echo "📦 应用路径: $AETHER_PATH"
        echo ""

        # 等待 2 秒让应用完全初始化
        echo "⏳ 等待初始化..."
        sleep 2

        # 检查菜单栏图标
        echo ""
        echo "✅ Aether 应该在菜单栏显示图标"
        echo ""
        echo "现在请："
        echo "1. 查看 Xcode 控制台（底部 Debug Area）"
        echo "2. 复制所有输出内容"
        echo "3. 特别注意以 [Aether] 开头的日志"
        echo ""
        echo "如果控制台是空的或显示 'error loading logs'："
        echo "- 点击 Xcode 菜单 View → Debug Area → Show Debug Area"
        echo "- 或按快捷键 Cmd+Shift+Y"
        echo ""

        exit 0
    fi

    echo "   等待中... ($i/10)"
done

echo ""
echo "❌ Aether 未启动"
echo ""
echo "可能的原因："
echo "1. 构建失败 - 检查 Xcode 是否显示错误"
echo "2. 应用崩溃 - 查看 Xcode 崩溃报告"
echo "3. 未点击 Run 按钮"
echo ""
echo "请检查 Xcode 并提供错误信息"
