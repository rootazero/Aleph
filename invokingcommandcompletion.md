这是一个非常完善的、系统级的交互方案。这个方案将 Aether 从一个简单的对话框提升为一个语义化操作系统启动器 (Semantic OS Launcher)。

以下是基于我们讨论内容的完整技术实现方案总结，分为架构设计、数据结构、前端交互和后端逻辑四个部分。

1. 核心理念：双模态统一入口 (Dual-Mode Unified Interface)

Halo (输入框) 拥有两种截然不同的状态，通过显式操作进行切换，互不干扰。

特性	🟣 聊天模式 (Chat Mode)	⚡️ 指令模式 (Command Mode)
触发方式	默认呼出，或直接输入文本	快捷键 Cmd+Opt+/ (强触发)
视觉隐喻	紫色边框，✨ 图标，纯文本输入	青色边框，💻 图标，胶囊(Chip)式路径
底层逻辑	自然语言 -> LLM (Rust Core)	结构化指令 -> 命令注册表 -> 执行
补全行为	无 (或仅作为输入建议)	强制弹出层级菜单，锁定上下文
2. 后端架构：统一命令树 (Rust - The Command Registry)

在 Rust 层，构建一个虚拟的文件系统/路由树，将所有功能（内置、MCP、用户Prompt）平权处理。

数据结构 (Rust)

Rust
// 通过 UniFFI 导出给 Swift
#[derive(Debug, Clone, PartialEq)]
pub enum CommandType {
    Action,      // 直接执行 (如 /search, /mcp/git/commit)
    Prompt,      // 注入 System Prompt (如 /en)
    Namespace,   // 目录/容器 (如 /mcp, /settings)
}

#[derive(Debug, Clone)]
pub struct CommandNode {
    pub key: String,             // 触发词 (e.g., "git")
    pub description: String,     // 描述 (e.g., "Git 版本控制")
    pub icon: String,            // SF Symbol 名称
    pub node_type: CommandType,
    pub has_children: Bool,      // UI 是否显示箭头
    // 动态加载子节点的标识符，如果是 Namespace
    pub source_id: Option<String>, 
}
路由逻辑

Root Level: 初始化时加载静态列表：["search", "mcp", "en", "summary"]。

Dynamic Loading: 当用户进入 /mcp 时，Rust 实时查询已连接的 MCP Client，生成二级节点 ["git", "fs", "brave"]。

Tool Inspection: 当用户进入 /mcp/git 时，Rust 查询该 Client 的 list_tools，生成三级节点 ["commit", "diff", "log"]。

3. 前端交互：SwiftUI + 路径解析

前端不仅仅是显示列表，还要负责维护“当前路径状态”。

状态管理 (ViewModel)

Swift
class CommandSession: ObservableObject {
    // 路径栈：例如 [Root, MCP Node, Git Node]
    @Published var pathStack: [CommandNode] = []
    // 当前输入的查询词 (用于过滤下一级)
    @Published var currentInput: String = ""
    // 当前显示的建议列表
    @Published var suggestions: [CommandNode] = []
    
    // 核心逻辑：路径下钻
    func pushNode(_ node: CommandNode) {
        if node.node_type == .Namespace {
            pathStack.append(node)
            currentInput = "" // 清空输入，准备接收下一级指令
            // 调用 Rust 获取该节点下的 children
            suggestions = rustCore.fetchChildren(for: node)
        } else {
            // 是叶子节点 (Action/Prompt)，准备执行
            execute(node)
        }
    }
    
    // 核心逻辑：路径回退 (Backspace)
    func popNode() {
        if !pathStack.isEmpty {
            pathStack.removeLast()
            // 重新加载上一级的建议
            suggestions = rustCore.fetchChildren(for: pathStack.last)
        }
    }
}
视觉呈现：胶囊式面包屑 (Breadcrumbs UI)

在 Halo 输入框中，利用 HStack 动态渲染路径，而不是纯文本。

Swift
HStack(spacing: 4) {
    // 1. 渲染已确认的路径节点 (胶囊样式)
    ForEach(session.pathStack, id: \.key) { node in
        HStack {
            Image(systemName: node.icon)
            Text(node.key)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(Color.cyan.opacity(0.2))
        .cornerRadius(4)
        .overlay(
            RoundedRectangle(cornerRadius: 4)
                .stroke(Color.cyan.opacity(0.5), lineWidth: 1)
        )
    }
    
    // 2. 渲染当前光标和输入
    TextField("", text: $session.currentInput)
        // ... 配置拦截器 ...
}
4. 交互流程详述

场景 A：调用 MCP 工具 (三级指令)

触发：用户按下 Cmd+Opt+/。Halo 变青色，显示 Root 建议。

第一级：用户输入 m -> 列表高亮 MCP -> 用户按 Tab。

UI 变化：mcp 变成胶囊。建议列表刷新为 [git, fs, ...].

第二级：用户输入 g -> 列表高亮 git -> 用户按 Tab。

UI 变化：[mcp] [git] 两个胶囊。建议列表刷新为 [commit, status, ...].

第三级：用户输入 co -> 列表高亮 commit -> 用户按 Enter。

UI 变化：显示参数提示 Args: {message: String}。

参数输入：用户输入 fix login bug -> 按 Enter 执行。

场景 B：使用自定义 Prompt (一级指令)

触发：Cmd+Opt+/。

选择：用户输入 en -> 列表高亮 En (Translation) -> 用户按 Tab。

UI 变化：[en] 变成胶囊。

Rust 动作：加载 "Translate to English" 的 System Prompt。

执行：用户粘贴一段中文文本 -> 按 Enter。

结果：Aether 直接流式输出英文翻译。

5. 键盘拦截器 (The Interceptor)

为了实现上述体验，你需要一个专门处理按键的 NSViewRepresentable，挂载在输入框上：

Cmd+Opt+/: 强制进入 Command Mode，清空 Path Stack。

Tab: 如果有补全建议，选中并 pushNode (下钻)。

Backspace:

如果 currentInput 不为空 -> 删除字符。

如果 currentInput 为空 -> popNode (删除上一个胶囊，回退一级)。

Escape: 退出 Command Mode，返回 Chat Mode。

总结

这个方案完美解决了你担心的“干扰”问题，并极大地扩展了软件的能力边界：

安全性：通过快捷键做物理隔离，防止误触。

扩展性：MCP、Prompt、Native Function 都在同一个树里，增加新功能不需要改 UI。

专业性：胶囊式面包屑和层级补全，给用户极其专业、高效的掌控感。

这套方案实现后，你的 Aether 将拥有比 Raycast 更灵活的“大脑”，比 Cursor 更原生的“触手”。