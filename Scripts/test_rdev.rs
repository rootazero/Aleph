// 简单的 rdev 测试程序
// 用于验证 rdev 库是否能正常监听键盘事件

use rdev::{listen, Event};

fn main() {
    println!("🔍 开始测试 rdev 键盘监听...");
    println!("请按任意键（按 Ctrl+C 退出）");
    println!();

    if let Err(error) = listen(callback) {
        println!("❌ 错误: {:?}", error);
        println!();
        println!("可能的原因：");
        println!("1. 没有 Accessibility 权限");
        println!("2. macOS 安全策略阻止");
        println!("3. rdev 库不兼容当前系统");
    }
}

fn callback(event: Event) {
    println!("⌨️  检测到事件: {:?}", event);
}
