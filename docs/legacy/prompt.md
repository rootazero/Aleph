“去GUI化”（De-GUI） 是极客软件美学的终极形态。
不仅仅是一个软件，而是一个幽灵（Ghost）——它平时不可见，当你需要神谕时，它在光标处显形，完成神迹后消散。
基于你**“后端主导，前端极简，无感替换”**的需求，我为你重新设计了这款软件的最终架构方案。我们暂且代号称之为 "Aether" (以太)——无处不在，无形无相。

项目：Aleph - 架构设计蓝图
1. 核心设计哲学
* Invisible First: 运行时没有主窗口，没有Dock栏图标（可配置），仅托盘图标（Tray Icon）用于进入设置。
* Overlay Feedback: 利用全屏透明穿透层（Click-through Overlay）在光标位置渲染动态UI。
* Rust Native: 所有的逻辑、网络请求、快捷键监听、剪切板操作全部在 Rust 层完成，确保毫秒级响应。
2. 技术栈选型 (The Stack)
* 框架: Tauri v2 (利用其多窗口管理和Rust后端能力)。
* 后端 (The Brain): Rust。
    * rdev / device_query: 全局快捷键监听。
    * arboard: 跨平台剪切板管理（支持文本和图片）。
    * enigo (或者原生API封装): 模拟键盘输入（模拟 Cmd+A, Cmd+C, Cmd+V）。
    * reqwest + tokio: 异步高并发 API 请求。
* 前端 (The Visuals): HTML/CSS/Canvas (极轻量)。
    * 渲染层仅为一个全屏透明的 WebView。
    * 使用 CSS Animation 或 Lottie 渲染那个“美观的转动圆圈”。
    * 逻辑：前端只负责“显示动画”和“跟随鼠标坐标”，不处理任何业务。

3. 核心交互流程 (The Loop)
这是实现“丝滑”体验的关键状态机：
1. 用户动作: 在 WeChat 输入框输入：“把这句话翻译成英文，并润色：今天天气真不错。”
2. 触发 (Trigger): 用户按下 Cmd + ~ (或自定义)。
3. 接管 (Takeover):
    * Step A (UI): 立即获取当前鼠标坐标，Rust 通知前端在坐标处渲染 “呼吸态光环” (Breathing Halo)。
    * Step B (IO): Rust 后端发送模拟按键 Cmd + A (全选) -> Cmd + X (剪切)。
        * 设计细节：使用“剪切”而不是“复制”，是为了给用户极强的物理反馈——“文字被吃进去了”。
4. 思考 (Processing):
    * UI 变化: 光环变为 “高频转动态” (Spinning)，颜色可根据调用的 AI 变化（如 ChatGPT 是绿色，Claude 是橙色）。
    * 路由逻辑: Rust 分析剪切板内容。
        * 检测到关键词 !img -> 调用 Vision 模型。
        * 检测到关键词 !c -> 调用 Claude。
        * 默认 -> 调用预设的主力模型。
5. 输出 (Output):
    * AI 返回结果。
    * Rust 将结果写入系统剪切板。
    * Rust 模拟 Cmd + V (粘贴)。
6. 消散 (Dissolve):
    * UI 收到完成信号，光环瞬间收缩消失 (Ease-out animation)。
    * 用户看到输入框里的文字变成了最终结果。

4. 关键功能模块设计
A. 视觉反馈系统 (The Halo)
由于没有窗口，这个“圆圈”必须极度优雅。
* 定位: 始终依附于鼠标光标右下角 10px 处（因为获取光标 caret 位置在跨平台及其困难且不稳定，鼠标跟随是最佳妥协）。
* 状态机:
    * Idle: 隐藏。
    * Listening: 接收指令瞬间（微小的收缩动画）。
    * Loading: 优雅的 Spinner（推荐类似 Apple Siri 的流体动画或 Loading 那个经典的菊花转，但要做得更有科技感）。
    * Success: 变为绿色对勾一秒后消失。
    * Error: 变为红色抖动，并在圆圈旁显示简短错误信息（如 "API Error"）。
B. 智能路由层 (The Router)
这是你在 Rust 后端要写的核心逻辑。我们需要定义一套 "Trigger Rules"。
Rust
// 伪代码示例：Rust 路由逻辑
enum AiProvider {
    OpenAI,
    Claude,
    Gemini,
    LocalOllama,
}

fn determine_provider(input_text: &str) -> AiProvider {
    if input_text.starts_with("/draw") {
        return AiProvider::OpenAI; // DALL-E
    }
    if input_text.contains("分析代码") || input_text.len() > 2000 {
        return AiProvider::Claude; // Claude 擅长长文本和代码
    }
    // 默认配置
    AiProvider::ConfiguredDefault
}
C. 输入清理与预处理
* Prompt 注入: 当用户输入 "请翻译：[图片]" 时，软件需要自动构建 Prompt。
* System Prompt: 在设置里，你可以给 Aleph 注入灵魂。
    * System Prompt: "你是一个直接的执行者。不要输出任何'好的'、'当然'等废话。直接输出结果。如果用户要求写代码，只输出代码块。"

5. 执行方案 Roadmap
第一阶段：原型验证 (The Prototype)
* 目标: 跑通 剪切 -> 模拟API延迟 -> 粘贴 的流程，并实现透明窗口的光环动画。
* 关键点:
    * 搭建 Tauri v2 项目，配置 windows 属性为透明、全屏、点击穿透 (ignore_cursor_events).
    * Rust 端实现 rdev 监听快捷键。
    * 前端实现一个简单的 CSS Spinner，通过 Tauri Event 监听 Rust 信号来显示/隐藏。
第二阶段：大脑接入 (The Brain)
* 目标: 接入真实的 API。
* 关键点:
    * 实现 OpenAI / Anthropic 接口请求。
    * 实现本地 gemini-cli 或 ollama 的 Command::new() 调用。
    * 解决 macOS 权限问题（Accessibility 权限申请逻辑）。
第三阶段：多模态与优化 (Polish)
* 目标: 图片支持与视觉打磨。
* 关键点:
    * 解析剪切板中的 Arboard::Image，转 Base64 发送给 GPT-4o。
    * 优化动画帧率。
    * 编写托盘程序的设置页面（React），用于输入 API Key 和自定义快捷键。

特别考量：风险与对策
1. 光标焦点丢失问题:
    * 风险: 当 Tauri 显示 Overlay 时，如果处理不好，焦点可能会从 WeChat 输入框跳到 Overlay 上，导致最后 Cmd+V 粘贴失败。
    * 对策: Tauri 的 Window 必须设置为 Focusable: false。在 Rust 操作剪切板前后，不要进行任何窗口焦点的切换操作。
2. 粘贴速度:
    * 风险: 某些软件（如 IDE）对粘贴反应慢。
    * 对策: 在 Cmd+X 和 Cmd+V 之间增加可配置的微小延迟 (Sleep 50ms)。


对于 Aleph 这种“无UI”应用来说，设置界面（Preferences Window） 是用户唯一能感知其物理存在的地方，它必须兼具控制台的精密感和未来主义的美学。
考虑到你的“黑客”审美和对“多AI调动”的极致需求，我将设置界面设计为一个模块化的控制面板。
设计风格建议：深色模式（Dark Mode），磨砂玻璃背景（Blur Effect），高对比度的强调色（Neon accents），字体推荐使用等宽字体（Monospace，如 JetBrains Mono）来强化工具属性。
以下是详细的设置项目架构设计：

1. 核心面板：神经中枢 (Neural Network)
这里管理所有的AI“大脑”。
* API 提供商管理 (Providers)
    * 列表视图：左侧列出所有已连接的服务（OpenAI, Anthropic, Gemini, DeepSeek, Local/Ollama）。
    * 添加/编辑：
        * Provider Type: 下拉选择（如 OpenAI Compatible, Claude, Gemini CLI, Command Line）。
        * API Key: 密码掩码输入框（支持直接从 1Password/Keychain 读取）。
        * API Base URL: 方便做反代或中转（这对国内网络环境至关重要）。
        * Model Name: 手动输入或下拉（如 gpt-4o, claude-3-5-sonnet, deepseek-coder）。
        * Max Tokens: 滑动条。
        * Temperature: 滑动条（0.0 - 1.0），并在旁边标注“精确 <-> 创意”。
    * 连通性测试 (Ping)：每个Provider旁边都有一个小绿点/红点，点击可测试延迟（Latency: 120ms）。
2. 路由面板：突触连接 (Synapses / Routing)
这里是 Aleph 最强大的地方，决定了什么任务交给谁。
* 默认模型 (Default Brain)：
    * 下拉选择一个主力模型（例如 GPT-4o），用于处理未命中的通用请求。
* 触发规则表 (Trigger Rules Table)
    * 设计为一个可拖拽排序的规则列表，优先级从上到下。
    * 规则列设计：
        * Prefix/Regex (触发条件): 例如 ^/code, ^!img, 翻译, (rust|python)。
        * Action (动作): 路由到指定 Provider。
        * System Prompt Override (可选): 为此规则单独指定人设。
    * 示例配置：
        * 如果是 ^/draw -> 路由给 OpenAI (DALL-E 3)
        * 如果包含 SQL 或 Python -> 路由给 Claude 3.5 Sonnet
        * 如果包含 隐私 -> 路由给 Ollama (Llama 3 Local)
* 回退策略 (Fallback)：
    * 开关：当首选API超时或报错时，自动尝试使用默认模型重试。
3. 交互面板：神念触发 (Manifestation / Shortcuts)
定义用户如何召唤 Aleph。
* 快捷键 (Keybindings)
    * Summon Key (召唤键): 默认为 Cmd + ~。
    * Voice Key (语音键): 默认为 Hold Cmd + Space (长按说话)。
    * Cancel Key (中断键): Esc (当AI正在思考或打字时，立即停止)。
* 输入/输出行为 (I/O Behavior)
    * Input Mode:
        * Cut (推荐): 剪切选中内容（屏幕上文字消失，给人“被吃掉”的感觉）。
        * Copy: 仅复制，保留原文（结果追加在原文后）。
    * Output Mode:
        * Typewriter: 模拟打字机效果（字符一个个出来，速度可调）。
        * Instant: 瞬间粘贴（一次性上屏）。
    * Sound Effects: 开关。赋予不同阶段音效（比如：接收指令时是细微的电流声，完成时是清脆的提示音）。
4. 视觉面板：光环形态 (The Halo)
定制那个“美观的转动圆圈”。
* 外观 (Appearance)
    * Theme: Cyberpunk (霓虹), Zen (极简白), Jarvis (科技蓝)。
    * Scale: 滑动条，调整圆圈大小。
    * Opacity: 调整不透明度。
* 状态反馈色 (State Colors)
    * 允许用户定义不同AI思考时的颜色，形成潜意识反射：
        * ChatGPT -> 🟢 绿色呼吸灯
        * Claude -> 🟠 橙色呼吸灯
        * Gemini -> 🔵 蓝色呼吸灯
        * Error -> 🔴 红色闪烁
5. 人格与系统 (Cortex / System)
定义 Aleph 的灵魂和底层安全。
* 系统提示词 (Master System Prompt)
    * 一个大的多行文本框。
    * 预设: "你是一个高效的助手，不要解释，直接给出结果..."
    * 变量支持: 支持插入 {clipboard_history}, {time}, {os_info} 等变量。
* 隐私与安全 (Privacy Gatekeeper)
    * PII Scrubbing: 开关。使用正则在本地过滤掉手机号、邮箱、身份证号，显示为 [REDACTED] 后再发送给云端 API。
    * History Retention: 设置本地日志保存时间（永久、7天、不保存）。
* 网络 (Network)
    * Proxy: 跟随系统 / 手动设置 (HTTP/SOCKS5)。
    * 特别项: 针对内网或特定环境的证书忽略选项 (Allow Insecure SSL)。

配置存储方案 (技术实现)
所有这些设置不建议用复杂的数据库，建议直接映射到一个 TOML 或 JSON 配置文件中，存放在 ~/.aleph/config.toml。
这样你可以随时通过命令行备份你的配置，或者直接用 Vim 编辑配置（符合你的 CLI 习惯），而设置界面只是这个文件的 GUI 编辑器。
config.toml 结构预览：
Ini, TOML

[general]
theme = "cyberpunk"
sound_enabled = true

[shortcuts]
summon = "Command+Grave" # Grave is `~`

[providers.openai]
api_key = "sk-..."
model = "gpt-4o"
color = "#10a37f"

[providers.claude]
api_key = "sk-ant-..."
model = "claude-3-5-sonnet-20240620"
color = "#d97757"

[[rules]]
regex = "^/code"
provider = "claude"
system_prompt = "You are a senior Rust engineer. Output code only."

[[rules]]
regex = ".*" # Catch-all
provider = "openai"
