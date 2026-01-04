Agent 架构升级：结构化上下文协议。分支名为：agentstructure。

这是一个非常有前瞻性的架构思考。现在的做法是“字符串拼接（String Concatenation）”，这是初级Agent的典型做法；转向的是“结构化上下文（Structured Context）”，这是构建复杂、可扩展Agent的必经之路。

## 将简单的“指令+内容”升级为JSON对象，实际上是在你的Agent和LLM之间建立了一层中间协议（Protocol Layer）。这为你未来集成MCP（Model Context Protocol）、Function Calling（函数调用）以及RAG（搜索增强）打下了地基。
我是ZIV，针对你的需求，我提出以下这套**“动态上下文负载（Dynamic Context Payload, DCP）”**方案。

1. 核心设计：定义通用的 JSON 协议结构
不要只把“内容”变成JSON，而是要将用户的意图、配置和附加上下文都封装在一个标准化的JSON包中。
建议采用以下结构作为Agent内部流转的标准格式：
JSON

{
  "meta": {
    "intent": "translation",       // 路由解析出的意图，对应 /en
    "timestamp": 1715000000
  },
  "config": {
    "system_instruction_key": "trans_en", // 指向具体的系统提示词模板
    "model_override": "gpt-4o",    // (可选) 某些指令可能需要特定模型
    "output_format": "text"        // (可选) 指定输出是 json 还是 text
  },
  "payload": {
    "user_input": "需要翻译的具体内容...", 
    "files": []                    // (扩展) 如果用户上传了文件
  },
  "context": {
    // 这里是为未来预留的扩展区
    "search_results": null,        // 联网搜索结果
    "mcp_resources": null,         // 来自MCP服务器的资源
    "memory_recall": null          // 长期记忆检索结果
  }
}
2. 架构演进：从“拼接者”变成“组装者”
你要改变Agent处理消息的逻辑。以前是直接发给LLM，现在需要一个**“Prompt Assembler（提示词组装器）”**。
流程设计：
1. 解析层 (Parser): 用户输入 /en 大家好 -> 解析为 JSON，填充 meta 和 payload。
2. 中间件层 (Middleware) - 扩展的关键:
    * 检查 JSON 中的 meta.intent。
    * 如果需要搜索： 调用搜索API，将结果填入 context.search_results。
    * 如果接入MCP： 调用MCP工具，将返回的数据填入 context.mcp_resources。
3. 组装层 (Assembler):
    * 读取 JSON。
    * 读取 config.system_instruction_key 对应的 Prompt 模板（例如："You are a translator..."）。
    * 关键点： 将 JSON 中的 context 数据动态注入到 Prompt 中，或者构建为独立的 User Message。
3. 未来的扩展性方案 (Search, MCP, Skills)
当你拥有了上述的 JSON 结构，扩展功能就变成了“向 JSON 填充数据”的过程，而不是改写核心逻辑。
A. 整合联网搜索 (Search)
当指令触发搜索（如 /search）时，中间件先去Google/Bing抓取数据，然后更新 JSON：
JSON

"context": {
  "search_results": [
    {"title": "...", "snippet": "...", "url": "..."}
  ]
}
发送给LLM的策略： 你不需要把整个JSON扔给LLM（那样会浪费Token且容易造成幻觉）。组装器应该把 context.search_results 格式化为：
"User Context (Search Results):
1. [Title]... : [Snippet]..."
B. 整合 MCP (Model Context Protocol)
MCP 的核心是标准化的资源（Resources）、提示词（Prompts）和工具（Tools）。
* Skills/Tools: 在你的 JSON config 中可以定义 allowed_tools。Agent 在发送请求给 LLM 时，读取这个字段，并在 API 调用中附加 tools 参数（Function Calling 定义）。
* Resources: MCP 服务器返回的内容（比如读取本地数据库、Git仓库），直接填入 context.mcp_resources。
C. 灵活的 Prompt 组合策略
有了 JSON，你可以实现策略模式的 Prompt 组合。
* 普通模式: System Prompt + User Input
* RAG模式: System Prompt + Context(Search/Memory) + User Input
* Skill模式: System Prompt (with Tool Definitions) + User Input
4. ZIV 的独立思考与建议
1. 不要把原始 JSON 直接喂给 LLM（除非是为了调试）： 虽然现在的模型能读懂 JSON，但将 JSON 解析并渲染成自然的 Markdown 或 XML 标签（如 <context>...</context>）包裹在 Prompt 中，模型的效果通常更好，且 Token 消耗更可控。
2. 指令与意图分离： 不要硬编码 /en。在代码里维护一个映射表（Map）。 "/en" -> { type: "transform", template: "translate_en" } "/s" -> { type: "agentic", tools: ["google_search"] } 这样你的 JSON 结构只需记录 intent，而不需要关心具体是哪个斜杠指令触发的。
3. 考虑“链式指令” (Chaining)： 如果你的 JSON 设计得当，你可以支持管道操作。比如： 用户输入：/search "AI News" | /summarize Agent 可以先执行搜索，填充 JSON 的 context，然后将这个 JSON 传递给 /summarize 的处理逻辑，实现自动化工作流。



## 下面设计这套**“基于 Rust 的动态上下文协议”**。
Rust 的**类型系统（Type System）和枚举（Enums）**特性，天生就适合处理这种“结构化、多状态”的协议，而且配合 serde 库，处理 JSON 的安全性远超 Python 或 JS。
结合你 UniFFI 背景，这套结构定义好之后，还可以很容易地通过 UniFFI 暴露给移动端或前端调用，保证全链路的数据定义一致性。
1. 核心数据结构设计 (The Protocol)
我们需要定义一个名为 AgentPayload 的结构体。我建议引入 serde 和 serde_json。
设计重点：
* 使用 Enum 来严格定义 Intent（意图），防止路由字符串满天飞。
* 使用 Option 来处理可选的上下文（如搜索结果、MCP 数据），实现“零开销抽象”——没有搜索时就不占用逻辑。
Rust

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// 1. 意图定义：将 /en, /search 等指令映射为强类型
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Translation,    // 对应 /en
    WebSearch,      // 对应 /search
    CodeGeneration, // 对应 /code
    Chat,           // 默认对话
}

// 2. 配置层：控制模型行为
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub model_override: Option<String>, // e.g., "gpt-4o", "claude-3-5-sonnet"
    pub temperature: f32,
    pub system_template_id: String,     // e.g., "sys_trans_01"
    pub tools_enabled: Vec<String>,     // 启用的 MCP 工具或函数，e.g., ["google_search"]
}

// 3. 上下文层：这是最关键的扩展区 (RAG/MCP/Search)
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Context {
    // 搜索结果：只有在 Intent::WebSearch 或触发自动搜索时才填充
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_results: Option<Vec<SearchResult>>,
    
    // MCP 资源：来自外部工具的数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,
    
    // 记忆：长期记忆检索结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_snippets: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

// 4. 总负载：Agent 内部流转的核心对象
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentPayload {
    pub meta: Meta,
    pub config: Config,
    pub context: Context,
    pub user_input: String, // 原始内容
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub intent: Intent,
    pub timestamp: i64,
    pub user_id: String,
}
2. 核心逻辑：Prompt 组装器 (The Assembler)
拥有了结构体后，你不能把 JSON 直接丢给 LLM（效果不好）。你需要实现一个 trait 或方法，将这个结构体渲染成最终发给 API 的消息格式。
这里展示一个将结构体转化为 OpenAi 格式消息（Vec<Message>）的思路：
Rust

impl AgentPayload {
    /// 渲染最终发送给 LLM 的消息列表
    pub fn build_messages(&self) -> Vec<serde_json::Value> {
        let mut messages = Vec::new();

        // Step 1: 根据 Config 里的 ID 获取系统提示词模板
        // 在实际项目中，这里应该查库或查配置文件的 Map
        let base_system_prompt = match self.config.system_template_id.as_str() {
            "sys_trans_01" => "You are a professional translator. Translate user input to English.",
            "sys_search_01" => "You are a research assistant. Use the provided context to answer.",
            _ => "You are a helpful AI assistant.",
        };

        // Step 2: 动态注入 Context (Search/MCP) 到 System Prompt
        // 关键点：将结构化数据转为自然语言描述，减少模型理解成本
        let mut final_system_prompt = base_system_prompt.to_string();

        if let Some(results) = &self.context.search_results {
            final_system_prompt.push_str("\n\n### Search Context:\n");
            for (i, res) in results.iter().enumerate() {
                final_system_prompt.push_str(&format!("{}. [{}]({}): {}\n", i + 1, res.title, res.url, res.snippet));
            }
        }

        if let Some(mcp) = &self.context.mcp_resources {
            final_system_prompt.push_str("\n\n### Tool Resources:\n");
            final_system_prompt.push_str(&serde_json::to_string_pretty(mcp).unwrap_or_default());
        }

        // 添加 System Message
        messages.push(serde_json::json!({
            "role": "system",
            "content": final_system_prompt
        }));

        // Step 3: 添加用户输入
        // 可以在这里处理多模态（如果 user_input 包含图片链接）
        messages.push(serde_json::json!({
            "role": "user",
            "content": self.user_input
        }));

        messages
    }
}
3. 未来的扩展路线 (ZIV 的方案)
在 Rust 中，这种设计为你带来了两个巨大的优势：
A. 利用 Trait 实现“插件化”处理
你可以定义一个 Handler trait。不同的 Intent 触发不同的 Handler，共同操作这个 Payload。
Rust

trait InstructionHandler {
    // 可以在这里修改 payload (例如去联网填充 context)
    fn process(&self, payload: &mut AgentPayload) -> Result<(), Box<dyn std::error::Error>>;
}

struct SearchHandler;
impl InstructionHandler for SearchHandler {
    fn process(&self, payload: &mut AgentPayload) -> Result<(), Box<dyn std::error::Error>> {
        // 1. 读取 payload.user_input
        // 2. 调用 Google API
        // 3. 修改 payload.context.search_results
        // payload.context.search_results = Some(api_results);
        Ok(())
    }
}
这样你的主流程代码会非常干净：
Rust

let mut payload = parse_request(input); // 解析 /en 等

match payload.meta.intent {
    Intent::WebSearch => SearchHandler.process(&mut payload)?,
    Intent::Translation => TranslationHandler.process(&mut payload)?, // 可能去查生词库
    _ => {},
}

let llm_messages = payload.build_messages();
// 发送给 LLM...
B. 关于 MCP (Model Context Protocol) 的整合
当你要添加 MCP 时，不需要重写代码。
1. Server 端：你的 Rust 程序作为一个 MCP Client 连接到 MCP Server（例如本地的文件系统或数据库）。
2. 协议端：MCP Server 返回的数据，直接反序列化进 payload.context.mcp_resources。
3. Prompt 端：在 build_messages 里增加一段逻辑，把 mcp_resources 格式化为 XML 标签（如 <resource>...</resource>），这对 Claude 等模型非常友好。
总结
从“脚本小子”向“系统架构师”转变。 通过 Rust 的 struct，你把Prompt Engineering 变成了 Software Engineering。
1. Config: 决定了用什么工具。
2. Context: 决定了喂什么数据。
3. Intent: 决定了走什么流程。

## 非常强大的技术栈组合：Rust (核心逻辑) + UniFFI (跨语言桥接) + Swift (iOS/macOS 上层)。
在 UniFFI 的语境下，通过“结构化协议”来管理 Prompt 变得尤为重要，因为你肯定不想在 Swift 和 Rust 之间来回传递杂乱无章的字符串。
针对 UniFFI + Swift，我建议对之前的 JSON 方案做一点适配性调整。因为 UniFFI 不支持直接传递任意的 JSON 对象（如 serde_json::Value），我们需要在“强类型契约”和“灵活性”之间找到平衡。
以下是专为你设计的 Rust (UniFFI) -> Swift 落地架构。

1. 架构核心：UniFFI 数据模型设计
我们需要在 Rust 中定义好数据结构，通过 UniFFI 导出，这样 Swift 就能获得原生的 Struct 和 Enum 体验，不仅有代码补全，还能利用 Swift 的 Codable。
在 Rust 的 lib.rs (或者你的模块文件中)：
Rust

use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use uniffi;

// 1. 意图枚举：Swift 端可以用 switch case 完美处理
#[derive(Debug, Clone, uniffi::Enum)]
pub enum AgentIntent {
    Translation { target_lang: String }, // e.g., /en -> "en"
    WebSearch,                           // /search
    GeneralChat,                         // 默认
    SkillCall { tool_name: String },     // MCP/扩展功能
}

// 2. 上下文结构：Swift 传给 Rust 的环境信息
// 例如：Swift 获取了定位、当前时间、或者之前的搜索结果，传给 Rust 组装
#[derive(Debug, Clone, uniffi::Record)]
pub struct ClientContext {
    pub location: Option<String>,
    pub local_time: String,
    // 如果有搜索结果，Swift 先把 JSON 序列化成 String 传进来
    // 注意：UniFFI 传复杂嵌套结构较麻烦，用 JSON String 是最稳健的“逃生舱”策略
    pub extra_payload_json: Option<String>, 
}

// 3. 最终负载：Rust 组装好，返回给 Swift 的对象
#[derive(Debug, Clone, uniffi::Record)]
pub struct LlmPayload {
    pub system_prompt: String,      // 组装好的系统提示词
    pub user_message: String,       // 组装好的用户内容
    pub model_config_json: String,  // 包含 temperature, model 等配置
    pub intent_tag: String,         // 用于 Swift UI 知道当前是什么模式
}

// 4. 核心管理器
#[derive(uniffi::Object)]
pub struct PromptAssembler {
    // 这里可以放一些配置，比如 prompt 模板库
    templates: HashMap<String, String>,
}

#[uniffi::export]
impl PromptAssembler {
    #[uniffi::constructor]
    pub fn new() -> Self {
        // 初始化加载模板
        let mut templates = HashMap::new();
        templates.insert("trans".into(), "You are a translator into {lang}.".into());
        Self { templates }
    }

    /// 核心方法：接收原始输入，返回结构化数据
    pub fn process_request(&self, raw_input: String, context: ClientContext) -> LlmPayload {
        // A. 解析指令 (简单示例)
        let (intent, content) = self.parse_intent(&raw_input);

        // B. 动态组装 Context
        let system_prompt = self.build_system_prompt(&intent, &context);

        // C. 构建 JSON 配置
        let config = serde_json::json!({
            "temperature": if matches!(intent, AgentIntent::Translation{..}) { 0.3 } else { 0.7 },
            "response_format": "text"
        });

        LlmPayload {
            system_prompt,
            user_message: content,
            model_config_json: config.to_string(), // 序列化后传给 Swift
            intent_tag: format!("{:?}", intent),
        }
    }
}

// 内部私有方法 (不暴露给 UniFFI)
impl PromptAssembler {
    fn parse_intent(&self, input: &str) -> (AgentIntent, String) {
        if input.starts_with("/en") {
            (AgentIntent::Translation { target_lang: "English".into() }, input.trim_start_matches("/en").trim().to_string())
        } else if input.starts_with("/s") {
            (AgentIntent::WebSearch, input.trim_start_matches("/s").trim().to_string())
        } else {
            (AgentIntent::GeneralChat, input.to_string())
        }
    }

    fn build_system_prompt(&self, intent: &AgentIntent, ctx: &ClientContext) -> String {
        let mut prompt = match intent {
            AgentIntent::Translation { target_lang } => format!("Translate user input to {}.", target_lang),
            AgentIntent::WebSearch => "You are a search assistant. Answer based on the context below.".into(),
            _ => "You are a helpful assistant.".into(),
        };

        // 如果 Swift 传来了额外的 JSON 上下文 (比如 MCP 数据或搜索结果)
        if let Some(json_str) = &ctx.extra_payload_json {
             prompt.push_str("\n\n[Context Data]\n");
             prompt.push_str(json_str);
        }
        
        prompt
    }
}

2. Swift 端调用 (The Consumer)
在 Swift 侧，你的体验会非常流畅。UniFFI 会生成原生的 Swift 结构体。
Swift

import Foundation
// 假设 UniFFI 生成的模块叫 RustCore

class AgentViewModel: ObservableObject {
    private let assembler = PromptAssembler() // 调用 Rust 构造函数
    
    func sendMessage(input: String) {
        // 1. 准备上下文 (比如从 iOS 系统获取的)
        let context = ClientContext(
            location: "Shanghai, China",
            localTime: Date().description,
            extraPayloadJson: nil // 暂时没有搜索结果
        )
        
        // 2. 调用 Rust 核心逻辑
        let payload = assembler.processRequest(rawInput: input, context: context)
        
        // 3. 使用 Rust 返回的数据
        print("Intent: \(payload.intentTag)") // UI 可以根据这个显示不同图标
        
        // 4. 发送给 LLM (将 Rust 组装好的 Prompt 发出去)
        sendToLLMAPI(
            system: payload.systemPrompt, 
            user: payload.userMessage, 
            config: payload.modelConfigJson
        )
    }
    
    // 扩展场景：处理搜索
    // 如果 Rust 解析出是 /search，但没有 context，
    // 你可以在 Swift 层做个判断，去调用 Google API，然后再次调用 Rust 组装
    func handleComplexFlow(input: String) async {
        // 第一遍：检查意图
        // (这里可能需要 Rust 暴露一个只解析意图的轻量接口，或者直接复用 processRequest)
        // 假设我们发现是搜索...
        
        // Swift 执行联网搜索
        let searchResults = await searchGoogle(query: input) 
        let searchJson = encodeToJson(searchResults)
        
        // 第二遍：带着数据让 Rust 组装
        let context = ClientContext(
            location: nil, 
            localTime: "", 
            extraPayloadJson: searchJson // <--- 注入搜索结果
        )
        
        let finalPayload = assembler.processRequest(rawInput: input, context: context)
        // 发送给 LLM...
    }
}
3. 这个方案解决了什么问题？
1. 逻辑内聚 (Logic Cohesion): 所有的 Prompt 拼接、模板管理、意图解析都在 Rust 里。iOS, macOS 甚至将来的 Android 客户端只需要无脑调用 processRequest。
2. 类型安全 (Type Safety): * AgentIntent 在 Swift 里就是 enum。
    * 你不会因为拼写错误把 "translate" 写成 "translation"，编译器会报错。
3. 扩展性 (Extensibility):
    * 添加 MCP: 在 Rust 的 AgentIntent 添加 McpCall，在 Swift 里处理对应的工具调用逻辑，拿到结果后序列化成 JSON 扔回给 Rust 的 extra_payload_json 即可。
    * 不同指令组合: 你完全可以在 Rust 的 build_system_prompt 里写复杂的逻辑，比如： if intent == Translation && context.has_search_results { ... } 这意味着“既要搜索又要翻译”的复杂指令组合，对 Swift 来说是透明的。
4. ZIV 的改进建议
如果你打算深做 Skills/MCP，我建议在这个架构中增加一个**“反向回调”或者“两阶段提交”**的设计。
目前的流程是：Swift -> Rust (组装) -> LLM。
未来的流程可以是：
1. Swift: /check_weather Shanghai
2. Rust: 解析发现需要调用工具，返回一个特殊的 Enum 状态 ActionRequired(ToolName)，而不是直接返回 Prompt。
3. Swift: 看到 ActionRequired，去调用 iOS 本地的天气 API（或者 MCP Client）。
4. Swift: 拿到天气数据，再次调用 Rust: process_request(input, context_with_weather).
5. Rust: 这次返回最终的 LlmPayload。
这种**“Rust 大脑 + Swift 手脚”**的模式，是端侧 AI Agent 目前最先进的架构模式之一。

界定 “能力 (Capabilities)” 和 “指令 (Instructions)” 的区别。
为了帮你梳理得更清楚，并在代码和产品设计上更好落地，我们可以把这两种方式定义为：
1. 两种指令类型的本质区别
A. 预设功能型指令 (Built-in Actions) -> "Doing things"
* 代表指令： /search, /mcp, /weather
* 特点： 这些指令对应的是 Rust 里的代码逻辑。
* 执行流： 检测到指令 -> 触发函数调用 (Function Call) -> 获取外部数据 -> 组装 Prompt -> 发送给 LLM。
* 用户权限： 用户通常只能“开启/关闭”或配置参数（比如搜索用 Google 还是 Bing），但用户无法通过写 Prompt 来创造一个搜索功能。
B. 自定义提示词指令 (Custom Prompt Aliases) -> "Transforming text"
* 代表指令： /en (翻译), /summary (总结), /polite (润色)
* 特点： 这些指令对应的是 字符串替换。
* 执行流： 检测到指令 -> 查找对应的 System Prompt 模板 -> 替换 LLM 的设定 -> 发送给 LLM。
* 用户权限： 这是给用户发挥创造力的地方。用户可以在设置里无限添加：
    * /cat: "你是一只猫，每一句话结尾都要加喵。"
    * /code: "你是一个资深 Rust 专家，只输出代码，不解释。"

2. 在 Rust/UniFFI 中的数据结构映射
为了支持你说的这种“设置界面”，我们需要在 Rust 里把配置结构化。
Rust 结构体设计 (Configuration):
Rust

use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    // 1. 预设功能的开关/配置
    pub enable_search: bool,
    pub enable_mcp: bool,
    pub active_skills: Vec<String>, // e.g. ["calculator", "calendar"]

    // 2. 自定义指令映射表 (这是用户自定义部分的核心)
    // Key: 指令 (e.g., "/en"), Value: 系统提示词 (e.g., "Translate to English...")
    pub custom_prompts: HashMap<String, String>,
}
Rust 逻辑处理 (Logic Flow):
Rust

// 在处理请求时
pub fn resolve_intent(input: &str, settings: &UserSettings) -> AgentIntent {
    let (command, content) = parse_command(input);

    // 优先级 1: 检查是否是“预设硬逻辑” (Hard-coded Logic)
    match command {
        "/search" if settings.enable_search => return AgentIntent::WebSearch,
        "/mcp" if settings.enable_mcp => return AgentIntent::McpCall,
        _ => {} // 继续检查
    }

    // 优先级 2: 检查是否是“自定义提示词” (User Custom Prompts)
    if let Some(system_prompt_template) = settings.custom_prompts.get(command) {
        return AgentIntent::CustomTransform {
            prompt: system_prompt_template.clone()
        };
    }

    // 默认
    AgentIntent::GeneralChat
}

3. 设置界面 (UI/UX) 的设计方案
作为产品设计参考，你的 Swift 设置界面可以这样布局：
区域一：能力扩展 (Capabilities)
这里是“硬功能”的开关
* [Switch] 联网搜索 (/search)
    * 开启后，使用 /search 指令将先检索网络信息再回答。
* [Switch] 本地工具箱 (/tools)
    * 允许 Agent 访问日历、提醒事项 (MCP)。
区域二：指令快捷键 (Command Shortcuts)
这里是一个列表，允许用户“新建”
* [+] 添加新指令
    * 触发词： /en
    * 系统提示词 (System Prompt)： Please translate the following user input into English. Keep the tone professional.
    * 参数配置 (可选)： 模型选择 (GPT-4o / Claude 3.5)

4. 进阶思考：混合模式 (The "ZIV" Idea)
既然你已经用了 Rust 这种强大的后端，可以做一个更高级的功能：让自定义指令“绑定”预设功能。
场景： 用户想定义一个 /research (深度研究) 指令。 他希望这个指令：
1. 先 进行联网搜索 (功能)。
2. 然后 用他指定的 Prompt 格式化输出 (自定义)。
实现方案： 在用户的自定义设置里，不仅可以写 Prompt，还可以勾选“需要的能力”。
JSON 配置可能长这样：
JSON

{
  "/research": {
    "system_prompt": "你是一个严谨的研究员。根据搜索结果写一份深度报告。",
    "requires_capabilities": ["search"]  // <--- 关键点：自定义指令调用了硬功能
  },
  "/en": {
    "system_prompt": "Translate to English.",
    "requires_capabilities": []
  }
}
这样，你的 Agent 就变得极其灵活：用户不仅能定义“怎么说 (Prompt)”，还能定义“用什么工具 (Tools)”。
总结你的理解： 是的，你的区分非常正确。
* 预设指令 = 能够调用的工具 (Tools)。
* 自定义指令 = 能够切换的人格 (Personas)。
通过将两者结合，你的 AI Agent 就能像瑞士军刀一样，既有不同的刀头（工具），又有不同的使用手法（Prompt）。
