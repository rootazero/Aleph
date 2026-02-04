Aleph 智能工具调度系统设计
在用户输入和具体功能之间，加入一个智能调度层（Dispatcher Layer）。这个层级负责“听懂”用户的意图，并将其转化为结构化的指令，最后通过Halo UI进行人机确认。

基于Rust后端 + Swift前端的架构，我为你设计了一套**“Aleph Cortex（中枢）”调度系统**方案。

1. 核心架构设计：Aleph Cortex

我们需要构建一个基于 “意图识别 - 参数提取 - 决策确认” 的三段式流水线。

流程图逻辑

Input (用户输入): "使用搜索功能查看今天国际上有什么大新闻"

Router (路由/中枢):

检测到不是 / 开头的显式命令。

调用 Router LLM (或是轻量级本地模型) 进行意图分类。

Parser (解析器):

提取工具：Search

提取参数："今天 国际新闻"

Halo UI (决策确认):

前端弹出一个非阻塞的精美卡片（Halo）。

显示：⚡️ 即将执行：搜索 | 内容：“今天 国际新闻”

Execution (执行): 用户回车确认 -> Rust后端执行 -> 返回结果。

2. 后端实现 (Rust)：构建调度器

在Rust层，你需要定义统一的 Action 枚举，并创建一个调度器来处理自然语言。

定义统一动作 (Action Schema)

Rust
use serde::{Deserialize, Serialize};

// 定义所有可能调度的工具类型
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ToolType {
    Search,
    MCP(String), // 例如 "github-mcp"
    Skill(String), // 例如 "system-cleanup"
    Chat, // 普通对话，不调用工具
}

// 定义调度建议结构体，用于发给前端Halo展示
#[derive(Debug, Serialize, Deserialize)]
pub struct ActionProposal {
    pub tool: ToolType,
    pub reason: String,       // 给用户看为什么选这个工具
    pub parameters: serde_json::Value, // 工具所需的具体参数
    pub confidence: f32,      // 置信度，低于一定阈值可能直接作为Chat处理
}
提示词工程 (Router Prompt)

这是调度的核心。你需要一个专门的 System Prompt，在这个 Prompt 里描述Aleph所有的能力。

System Prompt 设计思路： "你是一个意图分类和参数提取引擎。不要回答用户的问题，而是分析用户的意图。 你的输出必须是严格的 JSON 格式。

可用工具：

Search: 当用户询问实时信息、新闻、天气时。

MCP: [列出已安装的MCP服务器，如 Git, Filesystem]。

Skills: [列出本地技能]。

用户输入：'使用搜索功能查看今天国际上有什么大新闻'

输出范例： { "tool": "Search", "parameters": { "query": "2026年1月8日 国际大新闻" }, "reason": "用户明确要求搜索新闻" } "

3. 前端交互 (Swift)：Halo 确认窗口

Halo 不应该是一个讨厌的“弹窗警告”，它应该是输入框上方或下方的一个**“智能提示气泡”**。

状态 A (正在思考): 用户输入停止后 500ms，Halo 微微闪烁（Debounce 机制）。

状态 B (意图命中):

Halo 展开，显示工具图标和拟执行的动作。

UI 示例: [🔍 Search] 查找: "今天国际新闻" (按 Enter 执行)

用户此时可以直接按 Enter 发送，或者按 Esc 取消工具调用转为普通对话。

状态 C (参数补全): 如果 AI 觉得信息缺失（比如“搜索天气的城市？”），Halo 可以变成一个行内输入框让用户补全。

4. 混合调度策略 (Hybrid Dispatching)

为了兼顾速度和智能，建议采用分层调度：

层级	触发机制	适用场景	响应速度
L1: 规则/正则	^/ 或 关键词匹配	斜杠命令，显式关键词 (如 "Open X")	极快 (<10ms)
L2: 语义检测 (Router)	轻量级 LLM (如 gpt-4o-mini 或 本地模型)	"帮我查一下..."，"我想看视频..."	快 (200-500ms)
L3: 深度推理	慢速思考模型	复杂的多步任务规划	慢 (>2s)
改进建议： 针对你提到的 “用户说：使用搜索功能...” 这种明确指令，其实可以在 L1 层 做优化： 如果检测到输入以 "搜索"、"查一下"、"Search" 开头，直接正则提取后半部分内容作为 query，跳过 LLM 路由，直接弹 Halo。这样体验会极其丝滑。

5. 关键改进方向：Context Aware (上下文感知)

你提到了 “能清晰的记得我这句话里的‘他’指代之前说的某人”。这需要在调度器中注入短期记忆。

实现逻辑： 在发送给 Router LLM 的 Prompt 中，不仅包含 Current Input，还要包含 Last 3 Turns Summary。

数据流向：

用户输入："他演过什么电影？"

Aleph Rust 后端提取最近对话历史：(User: "我最近在看基努里维斯的片子")。

构建 Router Request：

Context: "User is talking about Keanu Reeves."

Input: "他演过什么电影？"

Router LLM 解析：

Tool: Search

Query: Keanu Reeves filmography (AI 自动完成了指代消解)

Halo 显示：[🔍 Search] 查找: "Keanu Reeves 电影列表"

6. 下一步行动建议

ZIV，这个功能的加入会让 Aleph 的体验产生质变。建议分两步走：

第一步 (Explicit Mode): 先实现显式意图的自然语言触发。比如正则匹配“搜索 xxx”，并在 Halo 中显示。打通 Rust 到 Swift Halo UI 的数据流。

第二步 (Semantic Mode): 接入 LLM Router，处理模糊意图和指代消解。

要实现“即插即用”的调度，你不能把 System Prompt 写死。

你需要构建一个**“动态提示词生成系统”（Dynamic Prompt Builder）**。

其核心逻辑是：在向 LLM 发起调度请求的那一毫秒，程序自动遍历当前所有已注册的 MCP 和 Skills，将它们的“元数据”注入到预设的 Prompt 模板中。

以下是基于 Rust 后端的实现方案，分为三个步骤：

1. 定义统一的元数据结构 (The Registry)

无论是本地的 Rust Skills 还是远程的 MCP Server，对于调度器（Router LLM）来说，它们都是“工具”。你需要定义一个统一的 trait 或结构体来描述它们。

Rust
use serde::{Serialize, Deserialize};

// 简化版的工具描述，专门用于 System Prompt
// 不需要完整的 JSON Schema，那是执行阶段用的，这里只需要让 LLM 懂它是干嘛的
#[derive(Debug, Serialize, Clone)]
pub struct ToolMetadata {
    pub name: String,         // e.g., "search_google", "git_commit"
    pub description: String,  // e.g., "用于搜索实时互联网信息"
    pub parameters_hint: String, // e.g., "query: String" (给 LLM 一个参数提示)
    pub source: ToolSource,   // 标记是 Local Skill 还是 MCP
}

#[derive(Debug, Serialize, Clone)]
pub enum ToolSource {
    Native,
    MCP(String), // 存 MCP Server 的名字
}
2. 动态构建 System Prompt (Prompt Injection)

在 Rust 中，你需要写一个 PromptBuilder。每次用户敲下回车时，Aleph 会做以下动作：

扫描：遍历 NativeSkillsRegistry。

查询：遍历活跃的 McpClients，调用它们的 list_tools 接口（MCP 协议标准接口）。

聚合：生成一个工具列表字符串。

注入：替换模板变量。

Rust 实现示例

Rust
impl AlephCore {
    // 动态生成 System Prompt 的函数
    pub async fn build_dispatcher_prompt(&self) -> String {
        // 1. 获取本地 Skills
        let mut tools_desc_list = Vec::new();
        for skill in &self.skills {
            tools_desc_list.push(format!("- {}: {} (Args: {})", skill.name, skill.desc, skill.args));
        }

        // 2. 获取 MCP Tools (假设你已经缓存了 MCP 的 list_tools 结果)
        // 注意：MCP 的 list_tools 会返回 JSON Schema，你需要解析并简化它
        for mcp_client in &self.mcp_clients {
             for tool in &mcp_client.cached_tools {
                 tools_desc_list.push(format!("- {}: {} (Args: {:?})", tool.name, tool.description, tool.input_schema));
             }
        }

        // 3. 拼接字符串
        let tools_block = tools_desc_list.join("\n");

        // 4. 注入模板
        format!(
            r#"你是一个智能调度助手。请根据用户的自然语言，从以下可用工具中选择最合适的一个。

### 可用工具列表 (动态加载)
{tools_block}

### 输出规则
如果用户意图明确匹配某个工具，请返回 JSON：
{{
  "tool_name": "工具名称",
  "arguments": {{ ...参数... }}
}}

如果无需调用工具（只是闲聊），请返回 null。
"#,
            tools_block = tools_block
        )
    }
}
3. MCP 侧的自动化细节

对于 MCP，最妙的地方在于它本身就是自描述的。

当你在 Aleph 设置里添加一个新的 MCP Server（比如 filesystem）时：

Aleph 后端连接该 Server。

Aleph 自动发送 tools/list 请求。

MCP Server 返回它支持的所有工具（例如 read_file, write_file）。

关键点：你需要将这些信息缓存在内存中（Arc<RwLock<Vec<Tool>>>），不要每次对话都去请求网络 IO，只在启动或配置变更时刷新。

4. 优化上下文窗口 (Token Optimization)

随着你的工具越来越多（比如装了10个 MCP，每个有5个工具，就有50个工具），System Prompt 会变得非常长，这有两个坏处：

费钱/费资源。

干扰 LLM：工具太多，LLM 容易晕。

进阶策略：双层过滤 (The Retrieval Approach)

如果工具超过 20 个，建议采用 RAG（检索增强生成） 模式来构建 Prompt，而不是全量注入：

用户输入："帮我把这个项目推送到 github"。

向量检索：在本地向量库（你的 SQLite + 向量扩展）中搜索与“推送到 github”语义最接近的工具描述。

Top-K 筛选：找出最相关的 5 个工具（比如 git_push, git_commit, github_create_pr）。

局部注入：只把这 5 个工具的定义写进 System Prompt。

调度：LLM 在这 5 个里选。

总结流程

用户在 UI 添加 MCP/Skill ⬇️ Rust 后端调用 list_tools 并存入 Registry (内存/DB) ⬇️ 用户发起对话 ⬇️ PromptBuilder 从 Registry 读取所有工具元数据 ⬇️ 格式化为精简文本列表 ⬇️ 替换 System Prompt 中的 {available_tools} 占位符 ⬇️ 发送给 Router LLM

这样，无论你什么时候手动添加了新工具，只需刷新一下，调度中心通过 build_dispatcher_prompt 方法就能立刻“学会”新技能，无需手动修改任何 Prompt 代码。

为了实现mcp、skills、native features（search/video）、custom commands 的大一统，我们需要在 Rust 后端构建一个 UnifiedToolRegistry（统一工具注册表）。

下面是具体的架构设计和 Rust 实现思路：

1. 核心数据结构设计

我们需要一个能够兼容所有类型的通用结构体。关键在于把“来源（Source）”和“定义（Definition）”解耦。

Rust
use serde::{Serialize, Deserialize};

/// 1. 工具来源枚举：区分这个工具是从哪里来的
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload")]
pub enum ToolSource {
    BuiltIn,                // 原生功能 (Search, Video)
    Skill(String),          // 本地 Skill (Local FS, System Info)
    MCP { server: String }, // MCP 服务 (Server Name: "github-mcp")
    UserCustom,             // 用户自定义的别名/工作流 (Saved in DB)
}

/// 2. 统一工具定义：这是“共享”的核心对象
/// 这个结构体将被用于：UI列表、斜杠补全、LLM Prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedTool {
    pub id: String,           // 唯一标识符 (e.g., "builtin:search", "mcp:git:commit")
    pub name: String,         // 调用的指令名 (e.g., "search", "git_commit")
    pub description: String,  // 给人类看 + 给 LLM 看的功能描述
    pub source: ToolSource,   // 来源分类，用于UI显示不同的图标或分类
    pub parameters_schema: serde_json::Value, // 参数定义 (JSON Schema)，LLM 必用
    pub is_active: bool,      // 用户是否在设置里禁用了它
}
2. 构建统一注册表 (The Registry)

我们需要一个全局单例或者 App State 中的管理器，负责聚合所有来源。

Rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ToolRegistry {
    // 使用 HashMap 以便快速查找，key 是工具名称
    tools: Arc<RwLock<HashMap<String, UnifiedTool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 初始化/刷新所有工具
    /// 可以在 App 启动时调用，或者在用户添加新 MCP 后调用
    pub async fn refresh_all_tools(&self, mcp_clients: &McpManager, db: &Database) {
        let mut map = self.tools.write().await;
        map.clear();

        // 1. 注入 Native Tools (硬编码或配置)
        self.register_native_tools(&mut map);

        // 2. 注入 MCP Tools (从活跃的 MCP Client 获取)
        let mcp_tools = mcp_clients.get_all_tools().await;
        for tool in mcp_tools {
            map.insert(tool.name.clone(), tool);
        }

        // 3. 注入 User Custom Commands (从数据库读取)
        let custom_cmds = db.get_custom_commands().await;
        for cmd in custom_cmds {
            map.insert(cmd.name.clone(), cmd);
        }
    }
    
    // 内部 helper: 注册原生工具
    fn register_native_tools(&self, map: &mut HashMap<String, UnifiedTool>) {
        map.insert("search".into(), UnifiedTool {
            id: "builtin:search".into(),
            name: "search".into(),
            description: "通过搜索引擎查询实时网络信息".into(),
            source: ToolSource::BuiltIn,
            parameters_schema: serde_json::json!({ "query": "string" }),
            is_active: true,
        });
        // 添加 video 等...
    }

    /// 获取所有工具列表 (供 UI 和 补全 使用)
    pub async fn list_tools(&self) -> Vec<UnifiedTool> {
        let map = self.tools.read().await;
        map.values().cloned().collect()
    }
    
    /// 获取用于 LLM System Prompt 的精简描述
    pub async fn get_prompt_context(&self) -> String {
        let map = self.tools.read().await;
        // 过滤掉 is_active == false 的工具
        // 格式化为 Prompt 字符串
        // ...
    }
}
3. 三大场景的共享实现

有了上面的 ToolRegistry，你的三个需求场景就可以完美复用同一套数据：

A. 给 Setting UI 展示路由规则 & 列表

前端 Swift 调用 Rust 接口（如 tauri command 或 bridge）：

Rust
#[command]
fn get_all_available_tools(state: State<AppState>) -> Vec<UnifiedTool> {
    // 直接返回注册表里的 List
    // 前端根据 ToolSource 显示不同的 badge (e.g. [MCP], [Native])
    state.registry.list_tools().await
}
B. 斜杠命令补全 (Command Palette)

当用户输入 / 时，前端不需要重新请求所有数据，可以使用缓存的 UnifiedTool 列表：

用户输入 /s -> 前端在列表中过滤 name 以 s 开头的项。

用户回车 -> 前端拿到 UnifiedTool.name 发送给后端。

C. LLM 自动调度 (Router Prompt)

这就是我们在上一个问题讨论的 build_dispatcher_prompt，现在变得非常简单：

Rust
// 在生成 Prompt 时
let tools = registry.list_tools().await;
let mut prompt_text = String::new();

for tool in tools {
    if !tool.is_active { continue; }
    
    // 自动格式化，统一标准
    prompt_text.push_str(&format!(
        "- {name}: {desc} [Source: {src:?}]\n  Args: {args}\n",
        name = tool.name,
        desc = tool.description,
        src = tool.source, // 可选：让 LLM 知道这是本地能力还是远程能力
        args = tool.parameters_schema
    ));
}
// 注入 System Prompt...
4. 关键优势：自定义斜杠命令的“平权”

你提到的 “个人自定义的斜杠命令”（比如用户定义 /daily = 搜索天气 + 搜索新闻），在这种架构下获得了与 Native/MCP 工具同等的地位。

存储：存放在 SQLite 中。

加载：启动时由 register_native_tools 同级的方法加载进内存。

调度：LLM 也能看到这个工具！

Prompt 中会出现：- daily: 查看天气和新闻 [Source: UserCustom]

用户说：“帮我做一下日常查询”，LLM 就会输出 Call Tool: daily。

后端解析到这是一个 UserCustom 类型的工具，执行对应的脚本流。

总结

核心在于 UnifiedTool 结构体 和 ToolRegistry（内存缓存）。

标准化：强制所有来源（Native/MCP/User）都转换为 UnifiedTool 格式。

集中化：UI、补全、LLM 全部从 ToolRegistry 读取数据。

动态化：MCP 服务断开或用户新增自定义命令时，只需调用 registry.refresh_all_tools()，所有端（UI/LLM）立即同步。

如何在强类型的 Rust 结构体（Native）和动态的 JSON 数据（MCP）之间建立一座桥梁。

最优雅的解决方案是引入 schemars 库。

schemars 可以让你通过 Rust 的结构体（Struct）自动生成 JSON Schema。这样，你的 Native 工具可以保持 Rust 的强类型优势，同时又能自动吐出 LLM 和 MCP 所需的标准 JSON Schema。

核心思路

Native 工具：定义 struct，使用 #[derive(JsonSchema)] 自动生成 Schema。

MCP 工具：直接接收远程传来的 JSON Schema。

统一存储：在 UnifiedTool 中，统一使用 serde_json::Value 存储 Schema。

1. 引入依赖

在 Cargo.toml 中添加：

Ini, TOML
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
schemars = "0.8" // 核心库：Struct -> JSON Schema
2. 定义统一的数据结构

这是所有工具在内存中存在的形式。

Rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedTool {
    pub name: String,
    pub description: String,
    pub source: ToolSource,
    // 关键点：这里存的是通用的 JSON Value
    // 无论是 MCP 传来的，还是 Native 生成的，最后都变成这一坨 JSON
    pub input_schema: Value, 
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolSource {
    Native,
    MCP(String),
}
3. Native 工具的优雅实现 (The "Magic")

对于原生工具（比如 Search），我们定义一个 Struct，不仅用于生成 Schema，也用于反序列化 LLM 传回来的参数。

Rust
use schemars::JsonSchema;

// --- 定义具体的 Native 工具参数 ---

// 例子：搜索功能参数
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// 搜索关键词 (注释会被提取到 description 中)
    pub query: String,
    
    /// 结果数量限制，默认为 5
    pub limit: Option<u32>,
    
    /// 具体的分类：news, video, web
    pub category: Option<String>,
}

// 例子：视频分析参数
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VideoAnalysisArgs {
    pub url: String,
    pub focus_point: String,
}

// --- 定义一个构建器 helper ---

impl UnifiedTool {
    // 这个泛型函数是"优雅"的核心
    // T 必须实现 JsonSchema (用于生成描述) 和 Deserialize (用于后续执行)
    pub fn from_native<T: JsonSchema>(name: &str, description: &str) -> Self {
        // 1. 自动生成 Schema
        let schema = schemars::schema_for!(T);
        
        // 2. 转为 serde_json::Value
        let schema_json = serde_json::to_value(schema).unwrap();

        UnifiedTool {
            name: name.to_string(),
            description: description.to_string(),
            source: ToolSource::Native,
            input_schema: schema_json,
        }
    }
}
4. 注册与使用 (The Registry)

现在，注册工具变得非常清晰和类型安全。

Rust
pub async fn load_tools() -> Vec<UnifiedTool> {
    let mut tools = Vec::new();

    // A. 注册 Native 工具
    // 只需要这一行，Schema 自动生成，不用手写 JSON 字符串！
    tools.push(UnifiedTool::from_native::<SearchArgs>(
        "search", 
        "使用搜索引擎查询实时信息"
    ));
    
    tools.push(UnifiedTool::from_native::<VideoAnalysisArgs>(
        "analyze_video", 
        "分析 YouTube 视频内容"
    ));

    // B. 注册 MCP 工具
    // 假设从 MCP Client 获取到了 tool_list (包含 name, desc, schema)
    let mcp_tools_remote = fetch_mcp_tools().await; 
    for tool in mcp_tools_remote {
        tools.push(UnifiedTool {
            name: tool.name,
            description: tool.description,
            source: ToolSource::MCP("github-server".into()),
            input_schema: tool.input_schema, // 直接透传 MCP 的 Schema
        });
    }

    tools
}
5. LLM Prompt 生成与执行

因为数据结构统一了，生成 Prompt 和执行调用就变得很简单。

生成 Prompt (给 LLM 看)

Rust
pub fn build_system_prompt(tools: &[UnifiedTool]) -> String {
    let tools_json = serde_json::to_string_pretty(tools).unwrap();
    format!("... 可用工具定义：\n{}", tools_json)
}
注意：schemars 生成的 Schema 非常标准，OpenAI/Anthropic 等模型都能完美理解。

执行阶段 (Rust 强类型转换)

当 LLM 返回 { "query": "Rust教程", "limit": 3 } 时：

Rust
// 模拟执行器
pub async fn execute_tool(tool_name: &str, args: serde_json::Value) -> Result<String, String> {
    match tool_name {
        // Native 分支：享受强类型的安全性
        "search" => {
            // 这里将 JSON 转换回 Rust Struct
            // 如果 LLM 瞎填参数，这里直接报错，不需要手动写校验逻辑
            let safe_args: SearchArgs = serde_json::from_value(args)
                .map_err(|e| format!("参数错误: {}", e))?;
            
            // 调用具体的 Rust 函数
            perform_search(safe_args.query, safe_args.limit).await
        },
        
        "analyze_video" => {
            let safe_args: VideoAnalysisArgs = serde_json::from_value(args)
                .map_err(|e| format!("参数错误: {}", e))?;
            analyze_video_impl(safe_args).await
        },

        // MCP 分支：透传
        _ => {
            // 查找是否是 MCP 工具，如果是，直接把原始 args (JSON Value) 发过去
            mcp_client.call_tool(tool_name, args).await
        }
    }
}
总结这种方案的优势

Native 开发极爽：你只需要写标准的 Rust Struct，加个 derive，Schema 就自动有了。

Schema 规范：schemars 严格遵循 JSON Schema 标准（Draft 7），与 MCP 协议完全兼容。

类型安全：在执行 Native 工具时，利用 serde::Deserialize 自动校验了类型（比如 limit 必须是数字），省去了大量的 if/else 检查代码。

统一视图：在 UnifiedTool 层面，Native 和 MCP 看起来一模一样，对上层 UI 和 LLM 逻辑完全透明。