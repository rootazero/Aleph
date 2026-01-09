那么rust构建的AI agent如何安装mcp协议呢
这是一个非常实际的工程问题。

在 Rust 中，“安装 MCP 协议”并不是像安装一个 .exe 或者 npm install 那么简单。因为 MCP 是一个协议标准 (Specification)，不是一个单一的软件包。

对于你的 Aether（作为主程序，即 MCP Client/Host），“安装”实际上意味着：引入能够处理异步 IO 和 JSON 序列化的 Rust 依赖库，并实现 MCP 的通信流程。

鉴于目前 MCP 的 Rust 生态还在快速发展中，最稳健的方式是引入基础依赖自己封装 Client，或者使用社区验证过的 SDK。

以下是为 Aether 构建 Rust MCP Client 的**“安装”与实现指南**。

第一步：修改 Cargo.toml (引入依赖)

你不需要一个庞大的 "mcp-framework"，你需要的是处理 JSON-RPC 和 异步进程通信 的能力。

在你的 crates/aether-core/Cargo.toml 中添加：

Ini, TOML
[dependencies]
# 1. 异步运行时 (必须)
tokio = { version = "1", features = ["full"] }

# 2. 序列化与 JSON 处理 (MCP 的语言)
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# 3. 错误处理
anyhow = "1.0"

# 4. 异步 Traits (方便定义接口)
async-trait = "0.1"

# 5. (可选) 如果你想用现成的 MCP SDK，可以尝试社区库，
# 但对于系统级 Agent，建议用上面的基础库自己封装 Transport 层以获得最大控制权。
第二步：实现 MCP Client (核心代码)

Aether 作为 Host，需要启动外部的 MCP Server（比如 Node.js 写的 Linear 工具），并通过标准输入输出 (Stdio) 与其对话。

这是一个最小化的、强健的 MCP Client 实现蓝图：

1. 定义消息格式 (JSON-RPC)

Rust
// src/mcp/protocol.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug)]
pub struct JsonRpcRequest {
    pub jsonrpc: String, // 必须是 "2.0"
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}
2. 实现传输层 (Transport)

这是最关键的一步：Aether 需要“寄生”在子进程的 Stdio 上。

Rust
// src/mcp/client.rs
use tokio::process::{Command, Child, ChildStdin, ChildStdout};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use std::process::Stdio;
use anyhow::{Result, Context};
use serde_json::Value;
use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse};

pub struct McpClient {
    process: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    request_id_counter: u64,
}

impl McpClient {
    // 1. 启动 MCP Server 进程
    pub async fn connect(command: &str, args: &[String], env: &Vec<(String, String)>) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.envs(env.iter().cloned());
        
        // 关键：接管 Stdio
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped()); // 建议单独处理 stderr 日志

        let mut child = cmd.spawn().context("无法启动 MCP Server")?;
        
        let stdin = child.stdin.take().context("无法获取 stdin")?;
        let stdout = child.stdout.take().context("无法获取 stdout")?;

        Ok(Self {
            process: child,
            stdin,
            reader: BufReader::new(stdout),
            request_id_counter: 0,
        })
    }

    // 2. 发送请求并等待响应
    pub async fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        self.request_id_counter += 1;
        let id = self.request_id_counter;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        // 序列化并发送 (MCP 协议规定每条消息以换行符结束)
        let mut json_str = serde_json::to_string(&req)?;
        json_str.push('\n'); 
        self.stdin.write_all(json_str.as_bytes()).await?;
        self.stdin.flush().await?;

        // 读取响应 (简单实现：假设下一行就是响应。生产环境需要一个 loop 来匹配 ID)
        let mut line = String::new();
        self.reader.read_line(&mut line).await?;

        let resp: JsonRpcResponse = serde_json::from_str(&line)?;
        
        if let Some(err) = resp.error {
            return Err(anyhow::anyhow!("MCP Error: {:?}", err));
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }
}
第三步：实现握手流程 (Lifecycle)

安装好上述代码后，Aether 需要在使用任何工具前执行 握手 (Handshake)。

Rust
impl McpClient {
    // 3. 初始化握手
    pub async fn initialize(&mut self) -> Result<()> {
        // A. 发送 'initialize'
        let init_params = serde_json::json!({
            "protocolVersion": "0.1.0",
            "capabilities": {
                "roots": { "listChanged": true },
                "sampling": {}
            },
            "clientInfo": {
                "name": "Aether",
                "version": "1.0.0"
            }
        });

        let _server_info = self.call("initialize", Some(init_params)).await?;

        // B. 发送 'notifications/initialized' (这是 MCP 规定的通知)
        // 注意：通知不需要 ID，这里简化处理，实际需要一个 send_notification 方法
        self.send_notification("notifications/initialized", None).await?;
        
        Ok(())
    }

    // 4. 获取工具列表
    pub async fn list_tools(&mut self) -> Result<Value> {
        self.call("tools/list", None).await
    }
    
    // 5. 调用工具
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Result<Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args
        });
        self.call("tools/call", Some(params)).await
    }
}
第四步：如何在 Aether 中组装使用

回到你的主程序逻辑（Rust Core），这就是“安装”后的使用方式：

Rust
// main.rs 或 core.rs

async fn main() -> Result<()> {
    // 1. 定义一个外部的 MCP Server (例如 Node.js 版的 Linear 工具)
    // 注意：这里我们调用了 node，但 Aether 自身不需要依赖 node，只是启动它
    let mut client = McpClient::connect(
        "node", 
        &["/path/to/mcp-server-linear/index.js".to_string()],
        &vec![("LINEAR_API_KEY".to_string(), "abc...".to_string())]
    ).await?;

    // 2. 握手
    println!("正在连接 MCP Server...");
    client.initialize().await?;

    // 3. 获取工具 (给 LLM 看的)
    let tools = client.list_tools().await?;
    println!("可用工具: {:?}", tools);

    // 4. (模拟) LLM 决定调用工具
    // 假设 LLM 返回了： "请帮我调用 linear_create_issue"
    let result = client.call_tool(
        "linear_create_issue", 
        serde_json::json!({ "title": "Aether Bug", "description": "Fix UI" })
    ).await?;

    println!("工具执行结果: {:?}", result);

    Ok(())
}
总结

对于 Rust 构建的 AI Agent：

没有一键安装包：目前没有类似 npm install mcp 的标准操作。

核心是依赖：你需要 tokio 和 serde_json。

本质是进程通信：你在 Rust 里写的代码，本质上是一个JSON-RPC 客户端，它负责 spawn 子进程，并往它的 stdin 写 JSON，从 stdout 读 JSON。

这就是最硬核、性能最好、最适合系统级 Agent 的“安装”方式。这能让你完全掌控 Aether 的生命周期，不会因为第三方 SDK 的更新而崩溃。