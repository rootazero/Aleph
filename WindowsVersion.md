项目支持Windows架构

⏺ 这是个很好的架构问题。简短回答：大致正确，但 Rust 层也需要一些适配工作。

  架构设计确实支持跨平台

  ┌─────────────────────────────────────────────────────┐
  │                    UI Layer                          │
  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  │
  │  │ Swift/SwiftUI│  │ C#/WinUI 3 │  │  Rust/GTK4  │  │
  │  │   (macOS)   │  │  (Windows)  │  │   (Linux)   │  │
  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  │
  └─────────┼────────────────┼────────────────┼─────────┘
            │                │                │
            └────────────────┼────────────────┘
                             │ UniFFI
            ┌────────────────┴────────────────┐
            │          Rust Core              │
            │  (业务逻辑、AI调用、路由等)      │
            └─────────────────────────────────┘

  Rust 层需要适配的部分
  ┌──────────────────┬───────────────────┬──────────────────────────────────────┐
  │       模块       │       现状        │           Windows 适配工作           │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ 路径处理         │ ~/.aether  │ 需要改用 %APPDATA%，可能已有条件编译 │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ 热键 (rdev)      │ 跨平台库          │ 基本兼容，但键码映射可能需调整       │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ 剪贴板 (arboard) │ 跨平台库          │ 基本兼容                             │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ 输入模拟 (enigo) │ 跨平台库          │ 基本兼容                             │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ 权限检查         │ Accessibility API │ Windows 无同等机制，需条件编译跳过   │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ 密钥存储         │ macOS Keychain    │ 需改用 Windows Credential Manager    │
  ├──────────────────┼───────────────────┼──────────────────────────────────────┤
  │ UniFFI 绑定      │ 生成 Swift        │ 需生成 C# 绑定                       │
  └──────────────────┴───────────────────┴──────────────────────────────────────┘
  预估工作量分布

  Swift → C# 替换:     ~60-70%  (全新编写)
  Rust 平台适配:       ~20-30%  (条件编译 + 抽象层)
  构建系统:            ~10%     (CMake/MSBuild 配置)

  建议的适配策略

  1. 抽象平台特定功能为 trait
  trait SecureStorage {
      fn store_key(&self, service: &str, key: &str) -> Result<()>;
      fn get_key(&self, service: &str) -> Result<String>;
  }
  // macOS: KeychainStorage
  // Windows: CredentialManagerStorage
  2. 使用 #[cfg] 条件编译
  #[cfg(target_os = "macos")]
  mod permissions_macos;

  #[cfg(target_os = "windows")]
  mod permissions_windows;  // 可能是空实现
  3. 统一路径处理
  fn config_dir() -> PathBuf {
      dirs::config_dir().unwrap().join("aether")
  }

  结论

  你的理解基本正确 — 架构设计本身就是为跨平台准备的。但"几乎不用修改"稍微乐观，更准确的说法是：

  - 核心业务逻辑（路由、AI 调用、Memory 模块）：可直接复用
  - 平台交互层（权限、密钥、路径）：需要条件编译适配
  - UI 层：需要完全重写

  需要我探索一下当前 Rust 代码中有多少平台特定的部分吗？这样可以更准确评估适配工作量。

✻ Worked for 37s

❯ 帮我看看 Rust 层有多少平台特定的代码 

⏺ Explore(探索 Rust 层平台特定代码)
  ⎿  Done (38 tool uses · 78.7k tokens · 1m 35s)                                                                                                                                     

⏺ 分析结果非常详细。让我为你总结一下关键发现：

  Rust 层平台特定代码总览

  已有良好跨平台支持 ✅
  ┌──────────────┬───────────────────────────────┬──────────────────────────────────────────┐
  │     模块     │             文件              │                   说明                   │
  ├──────────────┼───────────────────────────────┼──────────────────────────────────────────┤
  │ 路径管理     │ utils/paths.rs                │ 使用 dirs crate，已处理三个平台          │
  ├──────────────┼───────────────────────────────┼──────────────────────────────────────────┤
  │ Runtime 路径 │ runtimes/fnm.rs, uv.rs        │ 已有 #[cfg(unix)] / #[cfg(windows)] 分支 │
  ├──────────────┼───────────────────────────────┼──────────────────────────────────────────┤
  │ 下载/解压    │ runtimes/download.rs          │ 纯 Rust 库，完全跨平台                   │
  ├──────────────┼───────────────────────────────┼──────────────────────────────────────────┤
  │ 敏感路径     │ cowork/executor/permission.rs │ 已为 Windows 定义敏感路径列表            │
  ├──────────────┼───────────────────────────────┼──────────────────────────────────────────┤
  │ SIMD 优化    │ memory/simd.rs                │ ARM NEON + x86 AVX/SSE 都有实现          │
  └──────────────┴───────────────────────────────┴──────────────────────────────────────────┘
  需要适配的代码 ⚠️
  ┌──────────────┬───────────────────────────────────────┬────────────────────────────────────────────────┬──────────┐
  │     模块     │                 文件                  │                      问题                      │ 适配难度 │
  ├──────────────┼───────────────────────────────────────┼────────────────────────────────────────────────┼──────────┤
  │ 系统信息     │ services/system_info/macos.rs         │ 完全依赖 macOS 命令 (sysctl, vm_stat, sw_vers) │ 🔴 复杂  │
  ├──────────────┼───────────────────────────────────────┼────────────────────────────────────────────────┼──────────┤
  │ 活跃应用检测 │ services/system_info/macos.rs:117-167 │ 使用 osascript (AppleScript)                   │ 🔴 复杂  │
  ├──────────────┼───────────────────────────────────────┼────────────────────────────────────────────────┼──────────┤
  │ 沙箱执行     │ cowork/executor/code_exec.rs:262-313  │ macOS sandbox-exec 配置                        │ 🔴 复杂  │
  ├──────────────┼───────────────────────────────────────┼────────────────────────────────────────────────┼──────────┤
  │ 文件权限     │ config/mod.rs:1001-1016               │ Unix 权限 chmod 0o600                          │ 🟡 中等  │
  ├──────────────┼───────────────────────────────────────┼────────────────────────────────────────────────┼──────────┤
  │ fsync        │ config/mod.rs:960-980                 │ #[cfg(unix)] only                              │ 🟡 中等  │
  └──────────────┴───────────────────────────────────────┴────────────────────────────────────────────────┴──────────┘
  关键代码片段

  路径管理（已跨平台） - utils/paths.rs:
  pub fn get_config_dir() -> PathBuf {
      // macOS: ~/Library/Application Support/aether/
      // Windows: %APPDATA%\aether\
      // Linux: ~/.aether/
      dirs::config_dir().unwrap_or_else(|| get_home_dir().join(".config")).join("aether")
  }

  Runtime 路径（已跨平台） - runtimes/fnm.rs:
  #[cfg(unix)]
  fn node_path(&self) -> PathBuf { self.default_node_dir.join("bin").join("node") }

  #[cfg(windows)]
  fn node_path(&self) -> PathBuf { self.default_node_dir.join("node.exe") }

  系统信息（仅 macOS） - services/system_info/macos.rs:
  // ❌ 这些需要为 Windows 完全重写
  Command::new("sysctl").args(["-n", "hw.memsize"])  // 获取内存
  Command::new("vm_stat")                             // 内存统计
  Command::new("osascript")                           // 获取活跃窗口

  工作量评估

  ┌─────────────────────────────────────────────────────────────┐
  │                    Windows 适配工作量                        │
  ├─────────────────────────────────────────────────────────────┤
  │ ████░░░░░░░░░░░░░░░░  路径/IO          ~10%  (已基本完成)   │
  │ ████████░░░░░░░░░░░░  Runtime管理       ~20%  (需测试验证)   │
  │ ██████████░░░░░░░░░░  文件权限          ~25%  (ACL替代)      │
  │ ██████████████░░░░░░  系统信息          ~35%  (完全重写)     │
  │ ████████████████░░░░  沙箱执行          ~40%  (可选,复杂)    │
  └─────────────────────────────────────────────────────────────┘

  结论

  好消息：
  - 核心业务逻辑（AI 调用、路由、Memory 模块、MCP）是平台无关的
  - 路径和 Runtime 管理已有 Windows 条件编译代码
  - 预计 60-70% 的 Rust 代码可直接复用

  需要工作的部分：
  1. 系统信息模块 - 需要用 Windows API (WMI) 重写
  2. 活跃窗口检测 - 用 GetForegroundWindow API 替代 AppleScript
  3. 文件权限 - 用 Windows ACL 替代 Unix 权限
  4. 沙箱 - 考虑 Windows Sandbox 或直接跳过

  建议的适配策略：
  // 创建 trait 抽象平台特定功能
  pub trait SystemInfoProvider: Send + Sync {
      fn get_total_memory(&self) -> Result<u64>;
      fn get_available_memory(&self) -> Result<u64>;
      fn get_active_app_name(&self) -> Result<Option<String>>;
  }

  // 按平台选择实现
  #[cfg(target_os = "macos")]
  pub use macos::MacOSSystemInfo as PlatformSystemInfo;

  #[cfg(target_os = "windows")]
  pub use windows::WindowsSystemInfo as PlatformSystemInfo;

  总体来说，你的判断是正确的：架构设计良好，Rust 层适配工作量比预期的多一些，但绝对可控。

✻ Brewed for 2m 23s

❯ Windows UI 层需要实现哪些功能 

⏺ Explore(分析 macOS UI 层功能)
  ⎿  Done (22 tool uses · 77.0k tokens · 1m 56s)                                                                                                                                     

⏺ 分析非常详细。让我总结 Windows UI 层需要实现的核心功能：

  Windows UI 层功能清单

  1. 核心窗口系统
  ┌───────────────┬─────────────────────────────────┬───────────────────────────────┬────────┐
  │     组件      │           macOS 实现            │         Windows 需要          │ 复杂度 │
  ├───────────────┼─────────────────────────────────┼───────────────────────────────┼────────┤
  │ Halo 浮动窗口 │ NSWindow (borderless, floating) │ WinUI Window + 透明 + 无焦点  │ 🔴 高  │
  ├───────────────┼─────────────────────────────────┼───────────────────────────────┼────────┤
  │ 设置窗口      │ NSPanel + 双栏布局              │ NavigationView + ContentFrame │ 🟡 中  │
  ├───────────────┼─────────────────────────────────┼───────────────────────────────┼────────┤
  │ 多轮对话窗口  │ NSWindow + SwiftUI              │ WinUI Window + XAML           │ 🟡 中  │
  ├───────────────┼─────────────────────────────────┼───────────────────────────────┼────────┤
  │ 权限门控窗口  │ Sheet Modal                     │ ContentDialog                 │ 🟢 低  │
  └───────────────┴─────────────────────────────────┴───────────────────────────────┴────────┘
  2. 系统集成
  ┌──────────┬───────────────────────┬───────────────────────────────────┬────────┐
  │   功能   │       macOS API       │            Windows API            │ 复杂度 │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 系统托盘 │ NSStatusBar           │ NotifyIcon / TaskbarIcon          │ 🟡 中  │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 全局热键 │ CGEventTap            │ SetWindowsHookEx / RegisterHotKey │ 🔴 高  │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 剪贴板   │ NSPasteboard          │ Clipboard (WinRT)                 │ 🟢 低  │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 键盘模拟 │ CGEvent               │ SendInput                         │ 🟡 中  │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 光标位置 │ AXUIElement           │ UIAutomation                      │ 🔴 高  │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 屏幕捕获 │ ScreenCaptureKit      │ Windows.Graphics.Capture          │ 🟡 中  │
  ├──────────┼───────────────────────┼───────────────────────────────────┼────────┤
  │ 开机启动 │ SMLoginItemSetEnabled │ Registry Run key                  │ 🟡 中  │
  └──────────┴───────────────────────┴───────────────────────────────────┴────────┘
  3. Rust 核心集成

  ┌─────────────────────────────────────────────────────────┐
  │                   Windows C# 层                          │
  ├─────────────────────────────────────────────────────────┤
  │  EventHandler.cs                                         │
  │  ├─ 实现 IAetherEventHandler 接口                        │
  │  ├─ onStreamChunk(text) → 更新 UI                       │
  │  ├─ onComplete(response) → 触发输出                     │
  │  ├─ onToolCallStarted/Completed → 工具进度              │
  │  └─ onSessionStarted/Completed → Agent 循环             │
  ├─────────────────────────────────────────────────────────┤
  │  AetherCore.cs (UniFFI 生成)                             │
  │  ├─ process(input, context) → 处理请求                  │
  │  ├─ loadConfig() → 加载配置                             │
  │  └─ 其他 FFI 方法...                                    │
  └─────────────────────────────────────────────────────────┘
                             │
                             ▼ UniFFI C# Bindings
  ┌─────────────────────────────────────────────────────────┐
  │                   Rust Core (libaethecore.dll)           │
  └─────────────────────────────────────────────────────────┘

  4. UI 组件映射

  macOS SwiftUI          →     Windows WinUI 3 / XAML
  ─────────────────────────────────────────────────────
  VStack/HStack          →     StackPanel
  List                   →     ListView / ItemsRepeater
  NavigationSplitView    →     NavigationView
  TextField              →     TextBox
  Button                 →     Button
  Toggle                 →     ToggleSwitch
  Picker                 →     ComboBox
  ProgressView           →     ProgressRing
  VisualEffectBackground →     AcrylicBrush / MicaBrush

  5. 关键差异处理

  权限模型（最大差异）

  macOS                          Windows
  ─────────────────────────────────────────────────
  Accessibility 权限（必需）     → 不需要 ✅
  Input Monitoring 权限（必需）  → 不需要 ✅
  Screen Recording 权限          → 可能需要 CapturingMedia

  Windows 优势：权限门控可以大大简化或直接移除。

  Halo 窗口行为

  // Windows 实现关键点
  var window = new Window();
  window.ExtendsContentIntoTitleBar = true;  // 隐藏标题栏
  window.SystemBackdrop = new TransparentTintBackdrop();  // 透明背景

  // 防止获取焦点（需要 Win32 互操作）
  [DllImport("user32.dll")]
  static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);
  const int GWL_EXSTYLE = -20;
  const int WS_EX_NOACTIVATE = 0x08000000;
  SetWindowLong(hwnd, GWL_EXSTYLE, WS_EX_NOACTIVATE);

  全局热键

  // 方案 1: RegisterHotKey（简单但有限制）
  RegisterHotKey(hwnd, HOTKEY_ID, MOD_SHIFT, VK_SHIFT);

  // 方案 2: 低级键盘钩子（更灵活）
  SetWindowsHookEx(WH_KEYBOARD_LL, KeyboardProc, IntPtr.Zero, 0);

  6. 文件结构建议

  AetherWindows/
  ├── App.xaml.cs                    # 应用入口（对应 AppDelegate）
  ├── Services/
  │   ├── HotkeyService.cs           # 全局热键
  │   ├── ClipboardService.cs        # 剪贴板
  │   ├── KeyboardSimulator.cs       # 输入模拟
  │   ├── TrayIconService.cs         # 系统托盘
  │   └── CaretTracker.cs            # 光标位置（UIAutomation）
  ├── Windows/
  │   ├── HaloWindow.xaml            # Halo 浮动窗口
  │   ├── SettingsWindow.xaml        # 设置窗口
  │   └── ConversationWindow.xaml    # 多轮对话
  ├── Views/
  │   ├── Settings/                  # 设置页面
  │   ├── Halo/                      # Halo 状态视图
  │   └── Conversation/              # 对话视图
  ├── ViewModels/                    # MVVM ViewModel
  ├── Interop/
  │   ├── AetherCore.cs              # UniFFI 生成的绑定
  │   └── EventHandler.cs            # 回调实现
  └── Resources/
      └── Strings/                   # 本地化资源

  7. 工作量评估
  ┌───────────────────┬──────────┬────────┐
  │       模块        │ 估计工时 │ 优先级 │
  ├───────────────────┼──────────┼────────┤
  │ 应用框架 + 托盘   │ 40h      │ P0     │
  ├───────────────────┼──────────┼────────┤
  │ 全局热键系统      │ 32h      │ P0     │
  ├───────────────────┼──────────┼────────┤
  │ Halo 浮动窗口     │ 40h      │ P0     │
  ├───────────────────┼──────────┼────────┤
  │ Rust FFI 集成     │ 24h      │ P0     │
  ├───────────────────┼──────────┼────────┤
  │ 剪贴板 + 输入模拟 │ 24h      │ P0     │
  ├───────────────────┼──────────┼────────┤
  │ 设置界面          │ 40h      │ P1     │
  ├───────────────────┼──────────┼────────┤
  │ 多轮对话窗口      │ 32h      │ P1     │
  ├───────────────────┼──────────┼────────┤
  │ 光标位置跟踪      │ 24h      │ P2     │
  ├───────────────────┼──────────┼────────┤
  │ 屏幕捕获 + OCR    │ 24h      │ P2     │
  ├───────────────────┼──────────┼────────┤
  │ 测试 + 调优       │ 40h      │ -      │
  ├───────────────────┼──────────┼────────┤
  │ 总计              │ ~320h    │        │
  └───────────────────┴──────────┴────────┘
  8. 推荐技术栈

  ┌─────────────────────────────────────────┐
  │           WinUI 3 + .NET 8              │  ← 推荐
  │  • 现代 Fluent Design                   │
  │  • XAML 声明式 UI                       │
  │  • 良好的 Win32 互操作                  │
  │  • 活跃社区支持                         │
  └─────────────────────────────────────────┘

  总结

  Windows UI 层需要实现 约 15-20 个核心模块，主要挑战在于：

  1. Halo 浮动窗口 - 无焦点 + 透明 + 动态定位
  2. 全局热键 - 双击修饰键检测
  3. 光标位置跟踪 - UIAutomation 复杂度高
  4. UniFFI C# 绑定 - 需要验证成熟度

  好消息是 权限模型大大简化，Windows 不需要 macOS 那样的 Accessibility/Input Monitoring 权限门控。

✻ Sautéed for 2m 54s

❯ UniFFI 的 C# 绑定成熟吗 

⏺ Web Search("UniFFI C# bindings csharp support 2025 2026 status")
  ⎿  Did 1 search in 21s                                                                                                                                                             

⏺ Fetch(https://github.com/NordSecurity/uniffi-bindgen-cs)
  ⎿  Received 326.4KB (200 OK)

⏺ 根据搜索结果，让我给你一个诚实的评估：

  UniFFI C# 绑定成熟度评估

  当前状态：⚠️ 早期但可用
  ┌──────────┬──────────────────────────────────────────────────────────────────────────────┐
  │   指标   │                                     状态                                     │
  ├──────────┼──────────────────────────────────────────────────────────────────────────────┤
  │ 维护者   │ https://github.com/NordSecurity/uniffi-bindgen-cs（第三方，非 Mozilla 官方） │
  ├──────────┼──────────────────────────────────────────────────────────────────────────────┤
  │ 最新版本 │ v0.10.0+v0.29.4 (2025-08)                                                    │
  ├──────────┼──────────────────────────────────────────────────────────────────────────────┤
  │ 主版本号 │ 0.x — API 不稳定                                                             │
  ├──────────┼──────────────────────────────────────────────────────────────────────────────┤
  │ 更新频率 │ 活跃，约 2-3 个月一次                                                        │
  ├──────────┼──────────────────────────────────────────────────────────────────────────────┤
  │ 生产案例 │ ❌ 未公开披露                                                                │
  └──────────┴──────────────────────────────────────────────────────────────────────────────┘
  官方支持 vs 第三方

  UniFFI 官方支持（Mozilla 维护）:
  ├─ ✅ Kotlin  (成熟)
  ├─ ✅ Swift   (成熟) ← 我们正在使用
  ├─ ✅ Python  (成熟)
  └─ ✅ Ruby    (成熟)

  第三方绑定:
  ├─ ⚠️ C#     (uniffi-bindgen-cs by NordSecurity)
  ├─ ⚠️ C#    (uniffi-bindgen-cpp by NordSecurity)
  └─ ⚠️ Go     (社区)

  已知限制
  ┌───────────────────────┬──────────────────────────────────────┐
  │         限制          │                 影响                 │
  ├───────────────────────┼──────────────────────────────────────┤
  │ External types 不支持 │ 无法跨 UDL 文件引用类型              │
  ├───────────────────────┼──────────────────────────────────────┤
  │ 大小限制 2^31         │ 字符串/字节数组/列表不能超过 2GB     │
  ├───────────────────────┼──────────────────────────────────────┤
  │ 版本绑定              │ 必须匹配 uniffi-rs 版本 (如 v0.29.4) │
  └───────────────────────┴──────────────────────────────────────┘
  风险评估

  高风险 🔴:
  ├─ 无官方支持，Mozilla 不负责 C# 兼容性
  ├─ 0.x 版本意味着 breaking changes 可能随时发生
  └─ 缺乏公开的生产环境验证

  中风险 🟡:
  ├─ NordSecurity 有商业动机维护（可能内部使用）
  ├─ 回调机制（AetherEventHandler）可能有坑
  └─ 异步/线程模型需要验证

  低风险 🟢:
  ├─ 基础类型映射相对简单
  ├─ 项目活跃，issue 有响应
  └─ 生成的代码是标准 C#，可手动修复

  替代方案比较

  方案 1: 继续使用 uniffi-bindgen-cs

  优点:
  + 与 macOS Swift 绑定保持一致架构
  + 自动生成，维护成本低
  + NordSecurity 持续更新

  缺点:
  - 不成熟，可能遇到 bug
  - 回调/异步模式需要验证
  - 出问题需要自己 fork 修复

  方案 2: 手写 C FFI + P/Invoke

  // Rust 侧导出 C ABI
  #[no_mangle]
  pub extern "C" fn aether_process(
      input: *const c_char,
      callback: extern "C" fn(*const c_char)
  ) -> i32 { ... }

  // C# 侧 P/Invoke
  [DllImport("aethecore.dll")]
  static extern int aether_process(
      string input,
      ProcessCallback callback
  );

  优点:
  + 完全控制，无第三方依赖
  + 成熟的 .NET 技术
  + 遇到问题可以自己修

  缺点:
  - 手动维护成本高
  - 需要手写类型转换
  - 回调生命周期管理复杂

  方案 3: 使用 csbindgen

  https://github.com/Cysharp/csbindgen 是 Cysharp (Unity 团队) 开发的工具：

  优点:
  + 专为 C# 设计，更成熟
  + Unity 生产环境验证
  + 自动生成 P/Invoke

  缺点:
  - 需要改造现有 Rust 代码导出 C ABI
  - 与 UniFFI Swift 绑定不统一

  方案 4: gRPC/IPC 通信

  Rust Core (独立进程)  ←─ gRPC/Named Pipe ─→  C# UI

  优点:
  + 完全解耦，各自独立
  + 成熟的跨语言通信
  + 调试方便

  缺点:
  - 额外的进程开销
  - 延迟增加 (ms 级)
  - 部署复杂度提高

  我的建议

  短期（验证阶段）

  1. 先用 uniffi-bindgen-cs 做 POC
  # 生成 C# 绑定
  cargo install uniffi-bindgen-cs --git https://github.com/AetherWinds/uniffi-bindgen-cs \
    --tag v0.10.0+v0.29.4

  uniffi-bindgen-cs --library target/release/libaethecore.dll \
    --out-dir ../AetherWindows/Interop
  2. 重点验证：
    - 回调机制 (AetherEventHandler) 是否正常工作
    - 异步调用是否正确桥接到 C# Task
    - 内存管理（RustBuffer）是否有泄漏
    - 复杂类型（Vec<ToolInfoFfi>）是否正确序列化

  中期（如果 POC 成功）

  继续使用 uniffi-bindgen-cs，但：
  - Fork 一份，锁定版本
  - 建立完整的集成测试
  - 监控上游更新

  备选（如果 POC 失败）

  切换到 手写 C FFI + P/Invoke：
  - 为 Windows 单独维护一套 FFI 层
  - 参考现有 UniFFI 生成的代码结构

  结论
  ┌─────────────────┬─────────────────────────────────┐
  │      问题       │              回答               │
  ├─────────────────┼─────────────────────────────────┤
  │ C# 绑定成熟吗？ │ ❌ 不成熟，0.x 版本，无官方支持 │
  ├─────────────────┼─────────────────────────────────┤
  │ 能用吗？        │ ⚠️ 可能可以，需要 POC 验证      │
  ├─────────────────┼─────────────────────────────────┤
  │ 有替代方案吗？  │ ✅ 有，手写 FFI 或 csbindgen    │
  ├─────────────────┼─────────────────────────────────┤
  │ 建议？          │ 先做 POC，验证回调机制          │
  └─────────────────┴─────────────────────────────────┘
  关键验证点：回调（AetherEventHandler）是整个架构的核心，如果 uniffi-bindgen-cs 的回调不稳定，整个方案就不可行。

  ---
  Sources:
  - https://github.com/mozilla/uniffi-rs
  - https://github.com/NordSecurity/uniffi-bindgen-cs
  - https://github.com/mozilla/uniffi-rs/issues/1056

✻ Brewed for 1m 28s

❯ uniffi可用性差，那么Windows上最佳方案是什么 

⏺ Web Search("Rust C# interop best practices 2025 csbindgen P/Invoke Windows")
  ⎿  Did 1 search in 25s                                                                                                                                                             

⏺ Web Search("csbindgen Cysharp Rust C# bindings production")
  ⎿  Did 1 search in 15s                                                                                                                                                             

⏺ 根据搜索结果，我来给你一个清晰的方案对比和推荐：

  Windows 上 Rust-C# 互操作方案对比
  ┌──────────────┬───────────────────┬──────┬────────┬──────────┐
  │     方案     │      成熟度       │ 性能 │ 复杂度 │  推荐度  │
  ├──────────────┼───────────────────┼──────┼────────┼──────────┤
  │ csbindgen    │ ⭐⭐⭐⭐⭐ 生产级 │ 极高 │ 中     │ 🥇 首选  │
  ├──────────────┼───────────────────┼──────┼────────┼──────────┤
  │ 手写 C FFI   │ ⭐⭐⭐⭐⭐ 经典   │ 极高 │ 高     │ 🥈 备选  │
  ├──────────────┼───────────────────┼──────┼────────┼──────────┤
  │ Interoptopus │ ⭐⭐⭐⭐ 成熟     │ 极高 │ 中     │ 🥉 可选  │
  ├──────────────┼───────────────────┼──────┼────────┼──────────┤
  │ gRPC/IPC     │ ⭐⭐⭐⭐⭐ 生产级 │ 中   │ 低     │ 特定场景 │
  └──────────────┴───────────────────┴──────┴────────┴──────────┘
  🥇 推荐方案：csbindgen

  为什么选 csbindgen

  ✅ 作者背景强
     └─ Yoshifumi Kawai (neuecc)
        ├─ UniRx, UniTask 作者
        ├─ MessagePack-CSharp 作者
        ├─ Microsoft MVP for C#
        └─ Cysharp CEO/CTO

  ✅ 生产级成熟度
     └─ 版本 1.9.6（已发布 1.x 稳定版）
     └─ 专为 .NET + Unity 设计
     └─ 活跃维护

  ✅ 自动生成
     └─ 从 Rust extern "C" fn 自动生成 C# P/Invoke
     └─ 避免手写绑定的错误

  架构变化

  当前 (UniFFI):
  ┌─────────────────────────────────────────────────────────┐
  │  Rust Core                                              │
  │  └─ lib.rs (UniFFI 导出)                                │
  │     └─ aether.udl (接口定义)                            │
  │        └─ uniffi-bindgen → Swift/Kotlin                 │
  └─────────────────────────────────────────────────────────┘

  改造后 (csbindgen):
  ┌─────────────────────────────────────────────────────────┐
  │  Rust Core                                              │
  │  ├─ lib.rs (UniFFI 导出 → macOS Swift)                  │
  │  └─ ffi.rs (C ABI 导出 → Windows C#) ← 新增             │
  │     └─ build.rs + csbindgen → NativeMethods.cs          │
  └─────────────────────────────────────────────────────────┘

  具体实现

  Step 1: 添加 csbindgen 依赖

  # Aether/core/Cargo.toml
  [build-dependencies]
  csbindgen = "1.9"

  Step 2: 创建 C ABI 导出层

  // Aether/core/src/ffi.rs - Windows 专用 FFI 层

  use std::ffi::{c_char, CStr, CString};
  use std::ptr;

  /// 回调函数类型
  pub type StreamCallback = extern "C" fn(*const c_char);
  pub type CompleteCallback = extern "C" fn(*const c_char);
  pub type ErrorCallback = extern "C" fn(*const c_char);

  /// 事件处理器（C ABI 兼容）
  #[repr(C)]
  pub struct AetherCallbacks {
      pub on_thinking: Option<extern "C" fn()>,
      pub on_stream_chunk: Option<extern "C" fn(*const c_char)>,
      pub on_complete: Option<extern "C" fn(*const c_char)>,
      pub on_error: Option<extern "C" fn(*const c_char)>,
      pub on_tool_start: Option<extern "C" fn(*const c_char)>,
      pub on_tool_result: Option<extern "C" fn(*const c_char, *const c_char)>,
  }

  /// 初始化 Aether Core
  #[no_mangle]
  pub extern "C" fn aether_init(config_path: *const c_char) -> i32 {
      let path = unsafe {
          if config_path.is_null() { return -1; }
          CStr::from_ptr(config_path).to_str().unwrap_or("")
      };

      match crate::initialization::initialize(path) {
          Ok(_) => 0,
          Err(_) => -1,
      }
  }

  /// 处理用户输入
  #[no_mangle]
  pub extern "C" fn aether_process(
      input: *const c_char,
      app_context: *const c_char,
      callbacks: *const AetherCallbacks,
  ) -> i32 {
      // 参数验证
      if input.is_null() || callbacks.is_null() {
          return -1;
      }

      let input_str = unsafe { CStr::from_ptr(input).to_str().unwrap_or("") };
      let context_str = unsafe {
          if app_context.is_null() { "" }
          else { CStr::from_ptr(app_context).to_str().unwrap_or("") }
      };
      let cbs = unsafe { &*callbacks };

      // 创建回调适配器
      let handler = FfiEventHandler::new(cbs);

      // 调用核心处理逻辑
      match crate::process_request(input_str, context_str, Box::new(handler)) {
          Ok(_) => 0,
          Err(_) => -1,
      }
  }

  /// 释放 Rust 分配的字符串
  #[no_mangle]
  pub extern "C" fn aether_free_string(s: *mut c_char) {
      if !s.is_null() {
          unsafe { drop(CString::from_raw(s)); }
      }
  }

  /// 获取版本号
  #[no_mangle]
  pub extern "C" fn aether_version() -> *const c_char {
      static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
      VERSION.as_ptr() as *const c_char
  }

  Step 3: 配置 build.rs 生成 C# 代码

  // Aether/core/build.rs
  fn main() {
      // UniFFI 生成 (macOS)
      uniffi::generate_scaffolding("src/aether.udl").unwrap();

      // csbindgen 生成 (Windows)
      #[cfg(target_os = "windows")]
      {
          csbindgen::Builder::default()
              .input_extern_file("src/ffi.rs")
              .csharp_dll_name("aethecore")
              .csharp_namespace("Aether.Native")
              .csharp_class_name("NativeMethods")
              .csharp_class_accessibility("internal")
              .generate_csharp_file("../AetherWindows/Interop/NativeMethods.g.cs")
              .unwrap();
      }
  }

  Step 4: 生成的 C# 代码（自动）

  // AetherWindows/Interop/NativeMethods.g.cs (自动生成)
  namespace Aether.Native
  {
      internal static unsafe partial class NativeMethods
      {
          const string __DllName = "aethecore";

          [DllImport(__DllName, CallingConvention = CallingConvention.Cdecl)]
          public static extern int aether_init(byte* config_path);

          [DllImport(__DllName, CallingConvention = CallingConvention.Cdecl)]
          public static extern int aether_process(
              byte* input,
              byte* app_context,
              AetherCallbacks* callbacks
          );

          [DllImport(__DllName, CallingConvention = CallingConvention.Cdecl)]
          public static extern void aether_free_string(byte* s);

          [DllImport(__DllName, CallingConvention = CallingConvention.Cdecl)]
          public static extern byte* aether_version();
      }

      [StructLayout(LayoutKind.Sequential)]
      internal unsafe struct AetherCallbacks
      {
          public delegate* unmanaged[Cdecl]<void> on_thinking;
          public delegate* unmanaged[Cdecl]<byte*, void> on_stream_chunk;
          public delegate* unmanaged[Cdecl]<byte*, void> on_complete;
          public delegate* unmanaged[Cdecl]<byte*, void> on_error;
          public delegate* unmanaged[Cdecl]<byte*, void> on_tool_start;
          public delegate* unmanaged[Cdecl]<byte*, byte*, void> on_tool_result;
      }
  }

  Step 5: C# 高级封装

  // AetherWindows/Interop/AetherCore.cs
  namespace Aether.Interop;

  public sealed class AetherCore : IDisposable
  {
      private bool _initialized;

      // 回调事件
      public event Action? OnThinking;
      public event Action<string>? OnStreamChunk;
      public event Action<string>? OnComplete;
      public event Action<string>? OnError;

      public unsafe bool Initialize(string configPath)
      {
          fixed (byte* pathPtr = Encoding.UTF8.GetBytes(configPath + '\0'))
          {
              var result = NativeMethods.aether_init(pathPtr);
              _initialized = result == 0;
              return _initialized;
          }
      }

      public unsafe void Process(string input, string? appContext = null)
      {
          if (!_initialized) throw new InvalidOperationException("Not initialized");

          // 创建托管回调代理
          var callbacks = new AetherCallbacks
          {
              on_thinking = &OnThinkingCallback,
              on_stream_chunk = &OnStreamChunkCallback,
              on_complete = &OnCompleteCallback,
              on_error = &OnErrorCallback,
          };

          fixed (byte* inputPtr = Encoding.UTF8.GetBytes(input + '\0'))
          fixed (byte* contextPtr = appContext != null
              ? Encoding.UTF8.GetBytes(appContext + '\0')
              : null)
          {
              NativeMethods.aether_process(inputPtr, contextPtr, &callbacks);
          }
      }

      [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
      private static void OnThinkingCallback()
      {
          // 转发到 UI 线程
          Instance?.OnThinking?.Invoke();
      }

      [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
      private static unsafe void OnStreamChunkCallback(byte* text)
      {
          var str = Marshal.PtrToStringUTF8((IntPtr)text);
          Instance?.OnStreamChunk?.Invoke(str ?? "");
      }

      // ... 其他回调
  }

  回调处理的关键差异

  UniFFI (Swift):
  ┌─────────────────────────────────────────────────────────┐
  │  protocol AetherEventHandler {                          │
  │      func onStreamChunk(text: String)                   │
  │  }                                                       │
  │  // Rust 持有 Swift 对象引用，直接调用方法               │
  └─────────────────────────────────────────────────────────┘

  csbindgen (C#):
  ┌─────────────────────────────────────────────────────────┐
  │  [UnmanagedCallersOnly]                                 │
  │  static void OnStreamChunk(byte* text) { ... }          │
  │                                                          │
  │  // Rust 调用 C 函数指针，C# 用静态方法 + 单例转发       │
  └─────────────────────────────────────────────────────────┘

  线程安全处理

  // 回调可能在 Rust 线程触发，需要调度到 UI 线程
  [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
  private static unsafe void OnStreamChunkCallback(byte* text)
  {
      var str = Marshal.PtrToStringUTF8((IntPtr)text);

      // WinUI 3 方式
      App.Current.DispatcherQueue.TryEnqueue(() =>
      {
          Instance?.OnStreamChunk?.Invoke(str ?? "");
      });
  }

  完整架构图

  ┌─────────────────────────────────────────────────────────────────┐
  │                         Rust Core                                │
  │  ┌─────────────────────────────────────────────────────────────┐│
  │  │ Business Logic (路由、AI调用、Memory、MCP...)               ││
  │  └─────────────────────────────────────────────────────────────┘│
  │           │                                    │                 │
  │           ▼                                    ▼                 │
  │  ┌─────────────────┐                  ┌─────────────────┐       │
  │  │  lib.rs (UniFFI)│                  │  ffi.rs (C ABI) │       │
  │  │  aether.udl     │                  │  extern "C" fn  │       │
  │  └────────┬────────┘                  └────────┬────────┘       │
  └───────────┼────────────────────────────────────┼────────────────┘
              │                                    │
              ▼                                    ▼
      ┌───────────────┐                    ┌───────────────┐
      │ uniffi-bindgen│                    │   csbindgen   │
      │   (官方)      │                    │  (Cysharp)    │
      └───────┬───────┘                    └───────┬───────┘
              │                                    │
              ▼                                    ▼
      ┌───────────────┐                    ┌───────────────┐
      │ aether.swift  │                    │ NativeMethods │
      │ (自动生成)    │                    │ .g.cs (自动)  │
      └───────┬───────┘                    └───────┬───────┘
              │                                    │
              ▼                                    ▼
  ┌───────────────────────┐            ┌───────────────────────┐
  │     macOS Swift UI    │            │    Windows C# UI      │
  │  ├─ AppDelegate       │            │  ├─ App.xaml.cs       │
  │  ├─ HaloWindow        │            │  ├─ HaloWindow        │
  │  ├─ EventHandler      │            │  ├─ AetherCore        │
  │  └─ ...               │            │  └─ ...               │
  └───────────────────────┘            └───────────────────────┘

  迁移工作量对比
  ┌───────────┬──────────────┬─────────┬────────────────┐
  │   方案    │  FFI 层工作  │ C# 封装 │    总工作量    │
  ├───────────┼──────────────┼─────────┼────────────────┤
  │ UniFFI C# │ 0 (自动)     │ 中      │ ⚠️ 风险高      │
  ├───────────┼──────────────┼─────────┼────────────────┤
  │ csbindgen │ 16h (ffi.rs) │ 24h     │ ✅ 40h         │
  ├───────────┼──────────────┼─────────┼────────────────┤
  │ 手写 FFI  │ 40h          │ 24h     │ 64h            │
  ├───────────┼──────────────┼─────────┼────────────────┤
  │ gRPC      │ 32h (proto)  │ 16h     │ 48h + 延迟代价 │
  └───────────┴──────────────┴─────────┴────────────────┘
  结论

  推荐：csbindgen + C ABI 导出层

  优势:
  ├─ 成熟稳定（1.x 版本，生产验证）
  ├─ 作者权威（neuecc，Microsoft MVP）
  ├─ 自动生成（减少手写错误）
  ├─ 性能最优（直接函数调用，无 IPC）
  └─ 与 UniFFI 并存（macOS 继续用 UniFFI）

  代价:
  ├─ 需要新增 ffi.rs 导出层（~500 行）
  ├─ 回调模式从面向对象变为函数指针
  └─ 手动处理线程调度

  备选：如果不想维护两套 FFI

  考虑 gRPC 方案（Rust Core 作为独立服务）：
  - 优点：完全解耦，C# 调用 gRPC 客户端即可
  - 缺点：多进程，延迟增加，部署复杂

  ---
  Sources:
  - https://github.com/Cysharp/csbindgen/
  - https://neuecc.medium.com/csbindgen-generate-c-native-code-bridge-automatically-or-modern-approaches-to-native-code-78d9f9a616fb
  - https://docs.rs/interoptopus_backend_csharp/

如果开始Windows版本开发，如何实现macOS和Windows双版本代码同步，项目共享，请给出行业标准的方案。 

⏺ 这是一个很好的架构问题。让我给你几个行业标准方案的对比：

  方案对比
  ┌────────────────────────┬─────────────────────────┬────────┬──────────────┐
  │          方案          │        代表项目         │ 复杂度 │   适合规模   │
  ├────────────────────────┼─────────────────────────┼────────┼──────────────┤
  │ Monorepo               │ Google, Microsoft, Meta │ 低     │ 小-大型团队  │
  ├────────────────────────┼─────────────────────────┼────────┼──────────────┤
  │ Multi-repo + Submodule │ 传统开源                │ 中     │ 多团队协作   │
  ├────────────────────────┼─────────────────────────┼────────┼──────────────┤
  │ Multi-repo + Package   │ npm/cargo 生态          │ 高     │ 独立发布周期 │
  └────────────────────────┴─────────────────────────┴────────┴──────────────┘
  🥇 推荐方案：Monorepo

  对于 Aether 这样的项目，Monorepo 是最佳选择。

  目录结构

  aether/
  ├── .github/
  │   └── workflows/
  │       ├── rust-core.yml        # Rust 核心 CI
  │       ├── macos-app.yml        # macOS 应用 CI
  │       └── windows-app.yml      # Windows 应用 CI
  │
  ├── core/                        # 🦀 Rust 核心（共享）
  │   ├── Cargo.toml
  │   ├── src/
  │   │   ├── lib.rs               # 主入口
  │   │   ├── ffi/
  │   │   │   ├── mod.rs
  │   │   │   ├── uniffi.rs        # macOS UniFFI 导出
  │   │   │   └── cabi.rs          # Windows C ABI 导出
  │   │   ├── router/
  │   │   ├── providers/
  │   │   ├── memory/
  │   │   └── ...
  │   ├── uniffi.toml
  │   └── build.rs
  │
  ├── platforms/                   # 📱 平台特定代码
  │   │
  │   ├── macos/                   # 🍎 macOS 应用
  │   │   ├── project.yml          # XcodeGen 配置
  │   │   ├── Aether/
  │   │   │   ├── Sources/
  │   │   │   │   ├── App/
  │   │   │   │   ├── Windows/
  │   │   │   │   ├── Services/
  │   │   │   │   └── Generated/   # UniFFI 生成
  │   │   │   ├── Resources/
  │   │   │   └── Frameworks/
  │   │   │       └── libaethecore.dylib
  │   │   └── Scripts/
  │   │
  │   └── windows/                 # 🪟 Windows 应用
  │       ├── AetherWindows.sln
  │       ├── Aether/
  │       │   ├── App.xaml
  │       │   ├── App.xaml.cs
  │       │   ├── Windows/
  │       │   ├── Services/
  │       │   ├── Views/
  │       │   └── Interop/
  │       │       └── NativeMethods.g.cs  # csbindgen 生成
  │       ├── Aether.csproj
  │       └── libs/
  │           └── aethecore.dll
  │
  ├── shared/                      # 📦 跨平台共享资源
  │   ├── config/
  │   │   └── default-config.toml  # 默认配置模板
  │   ├── locales/
  │   │   ├── en.json
  │   │   └── zh-Hans.json
  │   ├── assets/
  │   │   ├── icons/               # 共享图标源文件
  │   │   └── fonts/
  │   └── docs/
  │       ├── ARCHITECTURE.md
  │       └── API.md
  │
  ├── scripts/                     # 🔧 构建脚本
  │   ├── build-core.sh            # 构建 Rust 核心
  │   ├── build-macos.sh           # 构建 macOS 应用
  │   ├── build-windows.ps1        # 构建 Windows 应用
  │   ├── generate-bindings.sh     # 生成 FFI 绑定
  │   └── release.sh               # 发布脚本
  │
  ├── CLAUDE.md                    # 项目指南
  ├── Cargo.toml                   # Workspace 根配置
  └── README.md

  Cargo Workspace 配置

  # /Cargo.toml (根目录)
  [workspace]
  resolver = "2"
  members = [
      "core",
  ]

  [workspace.package]
  version = "0.1.0"
  edition = "2021"
  license = "MIT"
  repository = "https://github.com/user/aether"

  [workspace.dependencies]
  tokio = { version = "1", features = ["full"] }
  serde = { version = "1", features = ["derive"] }
  # ... 共享依赖

  # /core/Cargo.toml
  [package]
  name = "aethecore"
  version.workspace = true
  edition.workspace = true

  [lib]
  crate-type = ["cdylib", "staticlib", "lib"]

  [dependencies]
  tokio.workspace = true
  serde.workspace = true

  [build-dependencies]
  uniffi = { version = "0.29", features = ["build"] }
  csbindgen = "1.9"

  [features]
  default = []
  uniffi = []      # macOS: cargo build --features uniffi
  cabi = []        # Windows: cargo build --features cabi

  条件编译策略

  // core/src/lib.rs

  // 共享的业务逻辑
  pub mod router;
  pub mod providers;
  pub mod memory;
  pub mod dispatcher;

  // 平台特定的 FFI 层
  #[cfg(feature = "uniffi")]
  pub mod ffi_uniffi;

  #[cfg(feature = "cabi")]
  pub mod ffi_cabi;

  // UniFFI 宏（仅 macOS）
  #[cfg(feature = "uniffi")]
  uniffi::setup_scaffolding!();

  // core/src/ffi_uniffi.rs (macOS)
  // UniFFI 导出，与现有 lib.rs 类似

  #[derive(uniffi::Object)]
  pub struct AetherCore { ... }

  #[uniffi::export]
  impl AetherCore {
      pub fn process(&self, input: String) -> Result<String, AetherError> { ... }
  }

  // core/src/ffi_cabi.rs (Windows)
  // C ABI 导出，供 csbindgen 使用

  #[no_mangle]
  pub extern "C" fn aether_init(config_path: *const c_char) -> i32 { ... }

  #[no_mangle]
  pub extern "C" fn aether_process(
      input: *const c_char,
      callbacks: *const AetherCallbacks,
  ) -> i32 { ... }

  构建脚本

  #!/bin/bash
  # scripts/build-core.sh

  set -e

  TARGET=${1:-"all"}  # all | macos | windows

  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  ROOT_DIR="$(dirname "$SCRIPT_DIR")"
  CORE_DIR="$ROOT_DIR/core"

  build_macos() {
      echo "🍎 Building for macOS..."
      cd "$CORE_DIR"

      # 构建 dylib
      cargo build --release --features uniffi

      # 生成 Swift 绑定
      cargo run --bin uniffi-bindgen generate src/aether.udl \
          --language swift \
          --out-dir "$ROOT_DIR/platforms/macos/Aether/Sources/Generated/"

      # 复制库文件
      cp target/release/libaethecore.dylib \
          "$ROOT_DIR/platforms/macos/Aether/Frameworks/"

      echo "✅ macOS build complete"
  }

  build_windows() {
      echo "🪟 Building for Windows..."
      cd "$CORE_DIR"

      # 交叉编译 (从 macOS)
      cargo build --release --features cabi --target x86_64-pc-windows-msvc

      # 或者在 Windows 上直接构建
      # cargo build --release --features cabi

      # csbindgen 在 build.rs 中自动生成 C# 绑定

      echo "✅ Windows build complete"
  }

  case $TARGET in
      macos)  build_macos ;;
      windows) build_windows ;;
      all)
          build_macos
          build_windows
          ;;
  esac

  # scripts/build-windows.ps1

  $ErrorActionPreference = "Stop"

  $ROOT_DIR = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
  $CORE_DIR = Join-Path $ROOT_DIR "core"
  $WINDOWS_DIR = Join-Path $ROOT_DIR "platforms/windows"

  Write-Host "🪟 Building Rust core for Windows..." -ForegroundColor Cyan

  Push-Location $CORE_DIR
  try {
      cargo build --release --features cabi

      # 复制 DLL
      Copy-Item "target/release/aethecore.dll" `
          -Destination "$WINDOWS_DIR/libs/" -Force

      # csbindgen 已在 build.rs 生成 NativeMethods.g.cs

      Write-Host "✅ Rust core build complete" -ForegroundColor Green
  }
  finally {
      Pop-Location
  }

  Write-Host "🪟 Building Windows app..." -ForegroundColor Cyan

  Push-Location $WINDOWS_DIR
  try {
      dotnet build -c Release
      Write-Host "✅ Windows app build complete" -ForegroundColor Green
  }
  finally {
      Pop-Location
  }

  CI/CD 配置

  # .github/workflows/rust-core.yml
  name: Rust Core

  on:
    push:
      paths:
        - 'core/**'
        - 'Cargo.toml'
        - '.github/workflows/rust-core.yml'
    pull_request:
      paths:
        - 'core/**'

  jobs:
    test:
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - run: cargo test --workspace

    build-macos:
      runs-on: macos-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - run: cargo build --release --features uniffi
        - uses: actions/upload-artifact@v4
          with:
            name: libaethecore-macos
            path: target/release/libaethecore.dylib

    build-windows:
      runs-on: windows-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - run: cargo build --release --features cabi
        - uses: actions/upload-artifact@v4
          with:
            name: aethecore-windows
            path: target/release/aethecore.dll

  # .github/workflows/macos-app.yml
  name: macOS App

  on:
    push:
      paths:
        - 'platforms/macos/**'
        - 'core/**'
    pull_request:
      paths:
        - 'platforms/macos/**'

  jobs:
    build:
      runs-on: macos-latest
      steps:
        - uses: actions/checkout@v4

        - name: Install XcodeGen
          run: brew install xcodegen

        - name: Build Rust Core
          run: ./scripts/build-core.sh macos

        - name: Generate Xcode Project
          working-directory: platforms/macos
          run: xcodegen generate

        - name: Build App
          working-directory: platforms/macos
          run: |
            xcodebuild -project Aether.xcodeproj \
              -scheme Aether \
              -configuration Release \
              -destination 'platform=macOS' \
              build

  # .github/workflows/windows-app.yml
  name: Windows App

  on:
    push:
      paths:
        - 'platforms/windows/**'
        - 'core/**'
    pull_request:
      paths:
        - 'platforms/windows/**'

  jobs:
    build:
      runs-on: windows-latest
      steps:
        - uses: actions/checkout@v4

        - name: Setup .NET
          uses: actions/setup-dotnet@v4
          with:
            dotnet-version: '8.0.x'

        - name: Install Rust
          uses: dtolnay/rust-toolchain@stable

        - name: Build Rust Core
          run: .\scripts\build-windows.ps1

        - name: Build App
          working-directory: platforms/windows
          run: dotnet build -c Release

  共享资源处理

  shared/
  ├── locales/
  │   ├── en.json              # 主语言文件
  │   └── zh-Hans.json
  └── config/
      └── default-config.toml

  // shared/locales/en.json
  {
    "app": {
      "name": "Aether",
      "settings": "Settings",
      "quit": "Quit"
    },
    "halo": {
      "processing": "Processing...",
      "error": "Error occurred"
    },
    "settings": {
      "general": "General",
      "providers": "Providers",
      "shortcuts": "Shortcuts"
    }
  }

  # scripts/sync-locales.sh
  # 将共享的本地化文件转换为平台特定格式

  # macOS: JSON → Localizable.strings
  python3 scripts/json-to-strings.py \
      shared/locales/en.json \
      platforms/macos/Aether/Resources/en.lproj/Localizable.strings

  # Windows: JSON → .resx
  python3 scripts/json-to-resx.py \
      shared/locales/en.json \
      platforms/windows/Aether/Resources/Strings.en.resx

  版本同步

  # 版本在 workspace 级别定义
  # /Cargo.toml
  [workspace.package]
  version = "0.2.0"

  <!-- platforms/windows/Aether/Aether.csproj -->
  <Project Sdk="Microsoft.NET.Sdk">
    <PropertyGroup>
      <!-- 从共享文件读取版本 -->
      <Version>$([System.IO.File]::ReadAllText('$(MSBuildThisFileDirectory)../../VERSION'))</Version>
    </PropertyGroup>
  </Project>

  # platforms/macos/project.yml
  settings:
    MARKETING_VERSION: $(cat ../../VERSION)
    CURRENT_PROJECT_VERSION: $(git rev-list --count HEAD)

  # /VERSION (单一版本来源)
  0.2.0

  开发工作流

  ┌─────────────────────────────────────────────────────────────────┐
  │                       开发工作流                                 │
  ├─────────────────────────────────────────────────────────────────┤
  │                                                                  │
  │  1. 修改 Rust Core                                              │
  │     └─ cd core && cargo test                                    │
  │                                                                  │
  │  2. 构建平台特定库                                               │
  │     ├─ macOS:   ./scripts/build-core.sh macos                   │
  │     └─ Windows: .\scripts\build-windows.ps1                     │
  │                                                                  │
  │  3. 开发 UI                                                      │
  │     ├─ macOS:   cd platforms/macos && xcodegen && open *.xcodeproj │
  │     └─ Windows: cd platforms/windows && dotnet watch            │
  │                                                                  │
  │  4. 提交                                                         │
  │     └─ git commit (触发对应平台的 CI)                            │
  │                                                                  │
  └─────────────────────────────────────────────────────────────────┘

  分支策略

  main
  ├── feature/core-memory-improvement    # Rust 核心功能
  ├── feature/macos-halo-animation       # macOS 专属功能
  ├── feature/windows-initial-setup      # Windows 初始开发
  └── release/v0.2.0                     # 发布分支

  迁移步骤

  从当前结构迁移到新结构：

  # 1. 创建新目录结构
  mkdir -p platforms/macos platforms/windows shared/locales scripts

  # 2. 移动现有代码
  mv Aether/core core
  mv Aether platforms/macos/Aether

  # 3. 更新路径引用
  # - 修改 project.yml 中的路径
  # - 修改 build.rs 中的输出路径

  # 4. 创建 workspace Cargo.toml
  cat > Cargo.toml << 'EOF'
  [workspace]
  resolver = "2"
  members = ["core"]
  EOF

  # 5. 提取共享资源
  cp platforms/macos/Aether/Resources/*/Localizable.strings shared/locales/

  总结
  ┌───────────┬────────────────────────────────────────┐
  │  决策点   │                推荐方案                │
  ├───────────┼────────────────────────────────────────┤
  │ 代码组织  │ Monorepo                               │
  ├───────────┼────────────────────────────────────────┤
  │ Rust 核心 │ Workspace 成员，feature flags 区分平台 │
  ├───────────┼────────────────────────────────────────┤
  │ FFI 绑定  │ macOS: UniFFI, Windows: csbindgen      │
  ├───────────┼────────────────────────────────────────┤
  │ CI/CD     │ 按路径触发，平台独立构建               │
  ├───────────┼────────────────────────────────────────┤
  │ 版本管理  │ 单一 VERSION 文件                      │
  ├───────────┼────────────────────────────────────────┤
  │ 本地化    │ JSON 主文件 → 转换脚本 → 平台格式      │
  └───────────┴────────────────────────────────────────┘
  这个方案的优势：
  - 🔄 代码同步：一个 PR 可以同时修改核心和所有平台
  - 🧪 原子测试：核心变更自动触发所有平台 CI
  - 📦 版本一致：所有平台版本号从同一来源
  - 🚀 独立发布：各平台可以独立发布


