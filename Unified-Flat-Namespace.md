以下是如何在技术上实现**“全工具扁平化 (Unified Flat Namespace)”**，同时解决可能出现的命名冲突和路由问题的完整方案。

1. 核心理念：统一命令注册表 (The Unified Registry)

你需要建立一个中间层，把不同来源的工具“清洗”一遍，去除技术前缀，统一映射到根命令空间。

架构变更图解

之前 (用户要记来源):

/Skills/search -> Native Function

/MCP/git/status -> MCP Server

/User/en -> Prompt

现在 (用户只记意图):

/search ──┐

/git ──┼──> Unified Command Router ──> [ 智能分发 ]

/en ──┘

2. Rust 后端实现：虚拟映射

在 Rust Core 中，我们需要维护一个 CommandRegistry，它是一个 HashMap，Key 是用户输入的指令，Value 是执行的策略。

Rust
// 定义指令来源
enum CommandSource {
    NativeSkill,          // 内部 Rust 函数 (快)
    McpNamespace(String), // MCP Server 的名称 (如 "git", "linear")
    UserPrompt(String),   // 用户定义的 Prompt (如 "translation")
}

struct CommandEntry {
    name: String,         // 触发词: "git", "search"
    description: String,
    icon: String,
    source: CommandSource, 
}

struct CommandRegistry {
    // 核心数据结构：扁平化的映射表
    // Key: "git" -> Value: CommandEntry { source: McpNamespace("git-server") }
    commands: HashMap<String, CommandEntry>
}

impl CommandRegistry {
    // 注册逻辑：启动时把所有来源“拍平”放进来
    pub fn register_all(&mut self) {
        // 1. 注册 Native Skills
        self.register("search", CommandSource::NativeSkill, "search-icon");
        self.register("sys", CommandSource::NativeSkill, "cpu-icon");

        // 2. 注册 MCP Servers (动态读取配置)
        // 即使配置里叫 "com.github.git", 我们注册为 "/git"
        self.register("git", CommandSource::McpNamespace("git-server"), "git-icon");
        self.register("fs", CommandSource::McpNamespace("filesystem"), "folder-icon");
        
        // 3. 注册 User Prompts
        self.register("en", CommandSource::UserPrompt("translate_en"), "text-icon");
    }
}
3. 解决痛点：命名冲突策略 (Conflict Resolution)

如果用户安装了一个 MCP 工具叫 search，而你系统自带了一个 Skill 也叫 search，怎么办？

你需要定义一个 “覆盖优先级 (Shadowing Priority)”：

Native (System) > User Custom > MCP Extension

System Skills (最高优先级)：核心功能不可被篡改。如果系统有 /search，MCP 的 search 必须被重命名（例如自动变更为 /ext-search）或被隐藏。

User Prompts (中等)：用户自定义的 /en 翻译指令。

MCP Extensions (最低)：第三方插件。

实现逻辑：

Rust
fn register(&mut self, name: String, source: CommandSource) {
    if self.commands.contains_key(&name) {
        let existing = self.commands.get(&name).unwrap();
        // 如果新来的源优先级低，则忽略或自动重命名
        if is_higher_priority(&existing.source, &source) {
            println!("警告: {} 已存在，忽略来自 MCP 的同名指令", name);
            return;
        }
    }
    self.commands.insert(name, entry);
}
4. 交互体验：UI 如何呈现来源？

虽然输入指令是扁平的 /git，但在 Halo 的补全菜单里，最好通过 图标 和 右侧小标签 来暗示它的来源，让用户心里有底。

Halo 补全列表设计：

图标	触发词	描述	右侧标签 (Subtitle)
[⚡️]	/search	联网搜索实时信息	System
[🐙]	/git	版本控制	MCP: Git
[📂]	/fs	文件系统	MCP: Filesystem
[A文]	/en	翻译成英文	Prompt
用户输入时只需打 /s，就会自动匹配到 /search，无需关心它是 Skill 还是 MCP。

5. 路由分发逻辑 (The Dispatcher)

当用户输入 /git status 并回车后，Rust 需要根据之前注册的信息进行路由：

Rust
async fn dispatch_command(&self, input: String) {
    // 1. 解析第一个 token: "/git" -> "git"
    let (trigger, args) = parse_trigger(input); 

    // 2. 查找注册表
    if let Some(entry) = self.registry.get(trigger) {
        match entry.source {
            CommandSource::NativeSkill => {
                // 直接调 Rust 函数
                native_skills::execute(trigger, args).await;
            },
            CommandSource::McpNamespace(server_name) => {
                // 转发给 MCP Client
                // 这里的 args 可能是 "status -v"
                // 你可能需要进一步解析成 JSON-RPC: { name: "git_status", ... }
                // 或者如果是二级菜单模式，进入子指令选择流程
                mcp_manager.forward_to_server(server_name, args).await;
            },
            CommandSource::UserPrompt(prompt_id) => {
                // 载入 Prompt 并请求 LLM
                llm_engine.chat_with_prompt(prompt_id, args).await;
            }
        }
    }
}
6. 关于二级指令 (/git/commit) 的处理

既然去掉了一级前缀，那么指令结构就变成了： [Trigger] [Sub-Command] [Args]

旧方案: /mcp/git/commit -m "fix"

新方案: /git commit -m "fix"

体验优化： 当用户输入 /git 并按下 Tab 或 Space 后：

UI: /git 变成一个胶囊（Token）。

Context: Aether 知道当前上下文在 Git 命名空间下。

Completion: 此时 Halo 自动列出 Git MCP Server 提供的 tools 列表 (commit, push, status)。

这就像我们在终端里输入 git [TAB] 一样自然。

总结

去掉 /MCP/ 和 /Skills/ 前缀是完全正确的。

对用户：所有工具一视同仁，输入 / 直接由名称唤起。

对系统：内部通过 统一注册表 (Unified Registry) 来管理路由。

对 UI：通过 图标和标签 区分来源，而不是通过冗长的文本前缀。

现在的 Aether 使用起来会是这样：

/search (调用 Rust)

/git commit (调用 MCP)

/en (调用 Prompt)

这种无缝融合才是 AI Agent 应有的形态。