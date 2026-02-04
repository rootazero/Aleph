# Aleph Extension System V2 - 规格说明书

**版本**: 2.0.0-draft
**状态**: RFC (Request for Comments)
**日期**: 2026-02-03
**作者**: Aleph Architecture Team

---

## 目录

1. [设计概述与核心原则](#1-设计概述与核心原则)
2. [Manifest 设计](#2-manifest-设计)
3. [Node.js SDK API 设计](#3-nodejs-sdk-api-设计)
4. [WASM (Rust) SDK API 设计](#4-wasm-rust-sdk-api-设计)
5. [Hook 系统详细设计](#5-hook-系统详细设计)
6. [Channel 与 Provider 扩展](#6-channel-与-provider-扩展)
7. [开发者体验与工具链](#7-开发者体验与工具链)
8. [迁移路径与兼容性](#8-迁移路径与兼容性)

---

## 1. 设计概述与核心原则

### 1.1 设计目标

Aleph SDK 2.0 旨在将插件系统从"工具提供者"升级为"全能力扩展者"，使第三方开发者能够扩展 Aleph 的任何维度——工具、渠道、模型、指令、服务。

### 1.2 核心原则

| 原则 | 描述 |
|------|------|
| **Contract-first** | Manifest (`aether_plugin.toml`) 是插件契约，权限边界静态可见 |
| **Hybrid Registration** | 静态声明为主，动态注册为辅，动态不能超越声明 |
| **Layered DX** | 宏/definePlugin 是 DX 糖衣，Trait/api.register 是底层骨架 |
| **Typed Hooks** | Hook 按语义分类：Interceptor（控制）、Observer（观测）、Resolver（决议） |
| **Unified Packaging** | Code + Prompt 在同一个包中，Skill scope 灵活可配 |

### 1.3 能力矩阵与实现优先级

| 能力 | P0 | P0.5 | P1 | P2 | 描述 |
|------|:--:|:----:|:--:|:--:|------|
| Tool | ✅ | | | | 工具注册（LLM 可调用） |
| Hook | ✅ | | | | 生命周期钩子（拦截/修改） |
| Prompt | ✅ | | | | Prompt 注入（System Instructions） |
| Command | | ✅ | | | 直达指令（绕过 LLM） |
| Service | | | ✅ | | 后台服务（定时任务/长连接） |
| Channel | | | | ✅ | 消息渠道（接入飞书/Slack） |
| Provider | | | | ✅ | LLM 供应商（接入 Groq/本地模型） |
| HTTP Route | | | | ✅ | HTTP 端点（插件 Web UI） |

### 1.4 设计哲学：混合模式 (Hybrid)

在混合模式下，`aether_plugin.toml` 被视为 Plugin Contract：

1. **权限 (Permissions)** —— 必须是 100% 声明式
   - 安全边界必须静态可见
   - 插件不能在运行时请求它在 TOML 里没声明过的权限

2. **静态能力 (Static Capabilities)** —— 默认声明式
   - 对于大多数固定工具，直接在 TOML 中定义
   - Aleph Core 可以在不加载运行时的情况下构建帮助文档（Lazy Loading）

3. **动态能力 (Dynamic Capabilities)** —— 声明"意图"，运行时注册
   - 对于无法静态定义的场景，插件声明"我是一个工具提供者"
   - Aleph 启动时标记该插件拥有动态生成能力

---

## 2. Manifest 设计

### 2.1 文件结构约定

```
my-plugin/
├── aether_plugin.toml      # 核心契约（必需）
├── SKILL.md                # 全局 Prompt（可选）
├── skills/                 # 独立 Skill 目录（可选）
│   └── weekly_report.md
├── docs/                   # 工具绑定文档（可选）
│   └── SQL_BEST_PRACTICES.md
├── package.json            # Node.js 依赖（Node 插件必需）
├── Cargo.toml              # Rust 依赖（WASM 插件必需）
└── dist/                   # 编译产物
    ├── index.js            # Node.js 入口
    └── plugin.wasm         # WASM 入口
```

### 2.2 完整 Manifest 示例

```toml
[plugin]
id = "com.example.sql-explorer"
name = "SQL Explorer"
version = "2.0.0"
description = "Query databases with natural language"
kind = "nodejs"  # nodejs | wasm | static
entry = "dist/index.js"

[plugin.author]
name = "Aleph Community"
email = "plugins@aether.dev"

# ═══════════════════════════════════════════
# P0: 权限声明（安全天花板，不可突破）
# ═══════════════════════════════════════════
[permissions]
network = ["connect:postgres://*", "connect:mysql://*"]
filesystem = ["read:./data", "write:./cache"]
env = ["DATABASE_URL", "PG_*"]  # 允许读取的环境变量
shell = false                    # 禁止执行 shell 命令

# ═══════════════════════════════════════════
# P0: 全局 Prompt 注入
# ═══════════════════════════════════════════
[prompt]
file = "SKILL.md"
scope = "system"  # system | disabled

# ═══════════════════════════════════════════
# P0: 静态工具声明
# ═══════════════════════════════════════════
[[tools]]
name = "query_sql"
description = "Execute a read-only SQL query"
handler = "handleQuerySql"  # 代码中的函数名
instruction_file = "docs/SQL_BEST_PRACTICES.md"  # 工具绑定 Prompt

[tools.parameters]
type = "object"
required = ["query"]

[tools.parameters.properties.query]
type = "string"
description = "The SQL query to execute"

[tools.parameters.properties.database]
type = "string"
description = "Target database name"
default = "default"

# ═══════════════════════════════════════════
# P0: Hook 声明
# ═══════════════════════════════════════════
[[hooks]]
event = "before_tool_call"
kind = "interceptor"  # interceptor | observer | resolver
handler = "onBeforeToolCall"
priority = "normal"   # system | high | normal | low
filter = "query_*"    # 可选：只拦截匹配的工具

[[hooks]]
event = "after_tool_call"
kind = "observer"
handler = "onAfterToolCall"

# ═══════════════════════════════════════════
# P0.5: 直达指令
# ═══════════════════════════════════════════
[[commands]]
name = "db-status"
description = "Show database connection status"
handler = "handleDbStatus"  # 代码处理（优先）
# prompt_file = "..."       # 或 Prompt 模板

# ═══════════════════════════════════════════
# P1: 后台服务
# ═══════════════════════════════════════════
[[services]]
name = "connection-pool"
description = "Maintain database connection pool"
start_handler = "startConnectionPool"
stop_handler = "stopConnectionPool"

# ═══════════════════════════════════════════
# P2: 动态能力声明（运行时扩展）
# ═══════════════════════════════════════════
[capabilities]
dynamic_tools = true      # 允许 api.registerTool()
dynamic_hooks = ["after_*"]  # 只允许注册 after_* 类 hook
```

### 2.3 权限语法规范

| 权限类型 | 语法 | 示例 |
|----------|------|------|
| Network | `connect:<protocol>://<pattern>` | `connect:https://api.openai.com/*` |
| Filesystem | `read:<path>` / `write:<path>` | `read:./data`, `write:/tmp/cache` |
| Env | 变量名或通配符 | `DATABASE_URL`, `PG_*` |
| Shell | `true` / `false` | `false` |

### 2.4 Prompt Scope 设计

| Scope | TOML 表达 | 用例 |
|-------|-----------|------|
| **System** (全局) | `[prompt] scope = "system"` | "Python 专家插件"——启用即注入 |
| **Tool** (绑定) | `[[tools]] instruction_file = "..."` | "Postgres 插件"——调用时注入 Schema |
| **Standalone** (独立) | `[[commands]] prompt_file = "..."` | "周报生成器"——显式触发 |

---

## 3. Node.js SDK API 设计

### 3.1 主入口：definePlugin + 动态 API

```typescript
// dist/index.ts
import { definePlugin, type AlephPluginAPI } from '@aether/plugin-sdk';

export default definePlugin({
  // ═══════════════════════════════════════════
  // 静态工具（与 TOML 声明对应）
  // ═══════════════════════════════════════════
  tools: {
    query_sql: async (args, ctx) => {
      const { query, database } = args;
      const result = await ctx.services.get('connection-pool').query(query);
      return { rows: result.rows, rowCount: result.rowCount };
    },
  },

  // ═══════════════════════════════════════════
  // Hook 处理器
  // ═══════════════════════════════════════════
  hooks: {
    onBeforeToolCall: async (ctx) => {
      // Interceptor: 可修改或阻断
      if (ctx.tool.name.startsWith('query_') && ctx.args.query?.includes('DROP')) {
        return { block: 'Destructive queries are not allowed' };
      }
      // 修改参数传递给下游
      return {
        modified: { ...ctx, args: { ...ctx.args, readonly: true } }
      };
    },

    onAfterToolCall: async (ctx) => {
      // Observer: 只读，用于日志/审计
      console.log(`Tool ${ctx.tool.name} completed in ${ctx.duration}ms`);
    },
  },

  // ═══════════════════════════════════════════
  // 直达指令
  // ═══════════════════════════════════════════
  commands: {
    'db-status': async (args, ctx) => {
      const pool = ctx.services.get('connection-pool');
      return {
        content: `Connected: ${pool.isConnected}\nActive: ${pool.activeCount}`,
      };
    },
  },

  // ═══════════════════════════════════════════
  // 后台服务生命周期
  // ═══════════════════════════════════════════
  services: {
    'connection-pool': {
      start: async (ctx) => {
        const pool = new Pool({ connectionString: ctx.env.DATABASE_URL });
        await pool.connect();
        return pool; // 返回值可被其他 handler 通过 ctx.services.get() 访问
      },
      stop: async (pool) => {
        await pool.end();
      },
    },
  },

  // ═══════════════════════════════════════════
  // 动态注册（当 TOML 中 capabilities.dynamic_tools = true）
  // ═══════════════════════════════════════════
  activate: async (api: AlephPluginAPI) => {
    // 运行时发现数据库中的表，动态注册工具
    const tables = await discoverTables();
    for (const table of tables) {
      api.registerTool({
        name: `query_${table.name}`,
        description: `Query the ${table.name} table`,
        parameters: generateSchemaFromTable(table),
        handler: async (args) => { /* ... */ },
      });
    }
  },
});
```

### 3.2 类型定义摘要

```typescript
// @aether/plugin-sdk 类型定义

interface AlephPluginAPI {
  // 动态注册（需 TOML 中声明 capabilities）
  registerTool(def: ToolDefinition): void;
  registerHook(def: HookDefinition): void;
  registerCommand(def: CommandDefinition): void;

  // 只读访问
  readonly config: PluginConfig;      // 用户配置
  readonly manifest: PluginManifest;  // TOML 解析结果
}

interface ToolContext {
  env: Record<string, string>;        // 已授权的环境变量
  services: ServiceRegistry;          // 已启动的服务
  session: SessionInfo;               // 当前会话信息
  permissions: PermissionChecker;     // 权限检查器
}

interface InterceptorResult<T> {
  modified?: T;      // 修改后的上下文
  block?: string;    // 阻断原因
}

type HookHandler<T> = (ctx: T) => Promise<void | InterceptorResult<T>>;
```

### 3.3 开发体验特性

| 特性 | 描述 |
|------|------|
| **TypeScript 原生** | 完整类型定义，零配置 IDE 支持 |
| **Schema 自动推断** | 从 `args` 参数类型生成 JSON Schema（可选） |
| **Hot Reload** | 开发模式下修改代码自动重载 |
| **脚手架工具** | `aether plugin create my-plugin --template nodejs` |

---

## 4. WASM (Rust) SDK API 设计

### 4.1 宏驱动 API（推荐）

```rust
// src/lib.rs
use aether_plugin_sdk::prelude::*;

#[aether_plugin(id = "com.example.sql-explorer")]
mod plugin {
    use super::*;

    // ═══════════════════════════════════════════
    // 静态工具
    // ═══════════════════════════════════════════
    #[tool(
        name = "query_sql",
        description = "Execute a read-only SQL query"
    )]
    async fn query_sql(args: QuerySqlArgs, ctx: ToolContext) -> Result<QueryResult> {
        let pool = ctx.services.get::<ConnectionPool>()?;
        let rows = pool.query(&args.query).await?;
        Ok(QueryResult { rows, row_count: rows.len() })
    }

    // 参数结构体自动生成 JSON Schema
    #[derive(Deserialize, JsonSchema)]
    struct QuerySqlArgs {
        /// The SQL query to execute
        query: String,
        /// Target database name
        #[serde(default = "default_db")]
        database: String,
    }

    fn default_db() -> String { "default".into() }

    // ═══════════════════════════════════════════
    // Hook: Interceptor
    // ═══════════════════════════════════════════
    #[hook(
        event = "before_tool_call",
        kind = "interceptor",
        priority = "normal",
        filter = "query_*"
    )]
    async fn on_before_tool_call(ctx: BeforeToolCallContext) -> InterceptorResult {
        if ctx.args_json.contains("DROP") {
            return InterceptorResult::block("Destructive queries not allowed");
        }
        InterceptorResult::pass()
    }

    // ═══════════════════════════════════════════
    // Hook: Observer
    // ═══════════════════════════════════════════
    #[hook(event = "after_tool_call", kind = "observer")]
    async fn on_after_tool_call(ctx: AfterToolCallContext) {
        tracing::info!(
            tool = %ctx.tool_name,
            duration_ms = ctx.duration.as_millis(),
            "Tool call completed"
        );
    }

    // ═══════════════════════════════════════════
    // 直达指令
    // ═══════════════════════════════════════════
    #[command(name = "db-status", description = "Show database connection status")]
    async fn db_status(ctx: CommandContext) -> CommandResult {
        let pool = ctx.services.get::<ConnectionPool>()?;
        CommandResult::text(format!(
            "Connected: {}\nActive: {}",
            pool.is_connected(),
            pool.active_count()
        ))
    }

    // ═══════════════════════════════════════════
    // 后台服务
    // ═══════════════════════════════════════════
    #[service(name = "connection-pool")]
    struct ConnectionPool {
        inner: Pool<Postgres>,
    }

    #[service_impl]
    impl ConnectionPool {
        async fn start(ctx: ServiceContext) -> Result<Self> {
            let url = ctx.env.get("DATABASE_URL")?;
            let pool = Pool::connect(&url).await?;
            Ok(Self { inner: pool })
        }

        async fn stop(self) -> Result<()> {
            self.inner.close().await;
            Ok(())
        }

        // 业务方法，可被其他 handler 调用
        async fn query(&self, sql: &str) -> Result<Vec<Row>> {
            Ok(self.inner.query(sql).await?)
        }
    }

    // ═══════════════════════════════════════════
    // 动态注册（activate 生命周期）
    // ═══════════════════════════════════════════
    #[activate]
    async fn on_activate(api: PluginAPI) -> Result<()> {
        let tables = discover_tables().await?;
        for table in tables {
            api.register_tool(DynamicTool {
                name: format!("query_{}", table.name),
                description: format!("Query the {} table", table.name),
                schema: table.to_json_schema(),
                handler: Box::new(move |args| { /* ... */ }),
            })?;
        }
        Ok(())
    }
}
```

### 4.2 底层 Trait API（高级用户）

```rust
// 宏展开后的底层实现，高级用户可直接使用
use aether_plugin_sdk::traits::*;

pub struct SqlExplorerPlugin;

impl AlephPlugin for SqlExplorerPlugin {
    fn metadata() -> PluginMetadata {
        PluginMetadata {
            id: "com.example.sql-explorer",
            version: "2.0.0",
        }
    }

    fn tools() -> Vec<Box<dyn Tool>> {
        vec![Box::new(QuerySqlTool)]
    }

    fn hooks() -> Vec<Box<dyn Hook>> {
        vec![
            Box::new(BeforeToolCallHook),
            Box::new(AfterToolCallHook),
        ]
    }

    fn services() -> Vec<Box<dyn Service>> {
        vec![Box::new(ConnectionPoolService)]
    }
}

// Tool trait
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> JsonSchema;
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<Value>;
}

// Hook trait
#[async_trait]
pub trait Hook: Send + Sync {
    fn event(&self) -> HookEvent;
    fn kind(&self) -> HookKind;  // Interceptor | Observer | Resolver
    fn priority(&self) -> Priority;
    async fn execute(&self, ctx: HookContext) -> HookResult;
}
```

### 4.3 WASM 特有优势

| 特性 | 描述 |
|------|------|
| **内存隔离** | 每个插件独立内存空间，崩溃不影响宿主 |
| **确定性沙箱** | 无 FS/Network 访问，安全边界硬编码 |
| **冷启动快** | ~5ms 加载时间，适合 Serverless 场景 |
| **跨平台** | 一次编译，macOS/Linux/Windows 通用 |

---

## 5. Hook 系统详细设计

### 5.1 Hook 事件全景

| 事件 | 类型 | 触发时机 | 典型用例 |
|------|------|----------|----------|
| `before_agent_start` | Interceptor | Agent 循环开始前 | 注入上下文、检查配额 |
| `before_tool_call` | Interceptor | 工具调用前 | 参数校验、PII 清洗、权限检查 |
| `before_message_send` | Interceptor | 消息发送前 | 敏感词过滤、格式转换 |
| `after_tool_call` | Observer | 工具调用后 | 日志记录、指标采集 |
| `after_message_send` | Observer | 消息发送后 | 审计追踪 |
| `agent_end` | Observer | Agent 循环结束 | 会话摘要、资源清理 |
| `on_error` | Observer | 任何错误发生时 | 错误上报、告警 |
| `resolve_provider` | Resolver | 需要选择 LLM 时 | 多模型路由、负载均衡 |
| `resolve_credential` | Resolver | 需要凭证时 | 多账号选择、密钥轮换 |

### 5.2 执行语义详解

```
┌─────────────────────────────────────────────────────────────┐
│                    INTERCEPTOR 执行流                        │
├─────────────────────────────────────────────────────────────┤
│  Plugin A          Plugin B          Plugin C               │
│  (priority: high)  (priority: normal) (priority: low)       │
│       │                 │                 │                 │
│       ▼                 │                 │                 │
│  ┌─────────┐           │                 │                 │
│  │ Run A   │───Ok(ctx')─┼────────────────┼──────┐          │
│  └─────────┘           │                 │      │          │
│                        ▼                 │      │          │
│                   ┌─────────┐           │      │          │
│                   │ Run B   │───Block!──┼──────┼──→ STOP  │
│                   └─────────┘           │      │          │
│                                         ▼      │          │
│                                    (skipped)   │          │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    OBSERVER 执行流                           │
├─────────────────────────────────────────────────────────────┤
│  Plugin A          Plugin B          Plugin C               │
│       │                 │                 │                 │
│       ▼                 ▼                 ▼                 │
│  ┌─────────┐       ┌─────────┐       ┌─────────┐           │
│  │ Run A   │       │ Run B   │       │ Run C   │  并行     │
│  └─────────┘       └─────────┘       └─────────┘           │
│       │                 │                 │                 │
│       └────────────────┴─────────────────┘                 │
│                        │                                    │
│                   join_all()                                │
│                   (errors logged, not propagated)           │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    RESOLVER 执行流                           │
├─────────────────────────────────────────────────────────────┤
│  Plugin A          Plugin B          Plugin C               │
│  (priority: high)  (priority: normal) (priority: low)       │
│       │                 │                 │                 │
│       ▼                 │                 │                 │
│  ┌─────────┐           │                 │                 │
│  │ Run A   │───None────┼────────────────┼──────┐          │
│  └─────────┘           │                 │      │          │
│                        ▼                 │      │          │
│                   ┌─────────┐           │      │          │
│                   │ Run B   │──Some(v)──┼──────┼──→ USE v │
│                   └─────────┘           │      │          │
│                                         ▼      │          │
│                                    (skipped)   │          │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 优先级定义

```rust
pub enum Priority {
    System = -1000,  // Aleph 内置（安全、审计）
    High = -100,     // 关键业务插件
    Normal = 0,      // 默认
    Low = 100,       // 低优先级扩展
}
```

### 5.4 Interceptor 返回值语义

```typescript
type InterceptorResult<T> =
  | { pass: true }                    // 继续执行，不修改
  | { modified: T }                   // 继续执行，使用修改后的上下文
  | { block: string }                 // 中断执行，返回错误
  | { block: string; silent: true }   // 中断执行，静默（不报错给用户）
```

---

## 6. Channel 与 Provider 扩展

### 6.1 Channel 扩展设计

Channel 允许插件接入新的消息渠道（如飞书、Slack、WhatsApp），无需修改 Aleph Core。

```toml
# aether_plugin.toml
[[channels]]
name = "feishu"
description = "Feishu (Lark) messaging integration"
handler = "handleFeishuChannel"

[channels.config_schema]
# 用户需要配置的字段
app_id = { type = "string", required = true }
app_secret = { type = "string", required = true, sensitive = true }
```

```typescript
// Node.js 实现
import { definePlugin, type ChannelContext, type Message } from '@aether/plugin-sdk';

export default definePlugin({
  channels: {
    feishu: {
      // 初始化：建立长连接或 Webhook
      async connect(ctx: ChannelContext) {
        const client = new FeishuClient({
          appId: ctx.config.app_id,
          appSecret: ctx.config.app_secret,
        });

        // 注册消息接收回调
        client.onMessage(async (msg) => {
          await ctx.dispatch({
            channelId: 'feishu',
            conversationId: msg.chatId,
            senderId: msg.userId,
            content: msg.text,
            metadata: { messageId: msg.messageId },
          });
        });

        await client.connect();
        return client; // 返回值用于 disconnect
      },

      // 发送消息
      async send(message: Message, client: FeishuClient) {
        await client.sendText(message.conversationId, message.content);
      },

      // 断开连接
      async disconnect(client: FeishuClient) {
        await client.disconnect();
      },
    },
  },
});
```

### 6.2 Provider 扩展设计

Provider 允许插件接入新的 LLM 供应商（如 Groq、本地 Ollama、私有部署）。

```toml
# aether_plugin.toml
[[providers]]
name = "groq"
description = "Groq LPU inference"
handler = "handleGroqProvider"
models = ["llama-3.3-70b", "mixtral-8x7b"]  # 支持的模型列表

[providers.config_schema]
api_key = { type = "string", required = true, sensitive = true }
base_url = { type = "string", default = "https://api.groq.com/openai/v1" }
```

```typescript
// Node.js 实现
export default definePlugin({
  providers: {
    groq: {
      // 列出可用模型（可动态获取）
      async listModels(ctx) {
        return [
          { id: 'llama-3.3-70b', displayName: 'Llama 3.3 70B', contextWindow: 131072 },
          { id: 'mixtral-8x7b', displayName: 'Mixtral 8x7B', contextWindow: 32768 },
        ];
      },

      // 聊天补全（流式）
      async *chat(request: ChatRequest, ctx: ProviderContext) {
        const client = new Groq({ apiKey: ctx.config.api_key });

        const stream = await client.chat.completions.create({
          model: request.model,
          messages: request.messages,
          stream: true,
        });

        for await (const chunk of stream) {
          yield {
            type: 'delta',
            content: chunk.choices[0]?.delta?.content ?? '',
          };
        }

        yield { type: 'done', usage: { promptTokens: 0, completionTokens: 0 } };
      },

      // 工具调用支持（可选）
      supportsTools: true,
      supportsVision: false,
    },
  },
});
```

### 6.3 HTTP Route 扩展设计

允许插件暴露 HTTP 端点，用于 Webhook 接收或自定义 UI。

```toml
# aether_plugin.toml
[[http_routes]]
method = "POST"
path = "/webhooks/feishu"
handler = "handleFeishuWebhook"
auth = "none"  # none | token | session

[[http_routes]]
method = "GET"
path = "/ui/dashboard"
handler = "serveDashboard"
auth = "session"
```

```typescript
export default definePlugin({
  http: {
    '/webhooks/feishu': {
      POST: async (req, res, ctx) => {
        const event = await req.json();
        // 处理飞书 Webhook 事件
        await ctx.channels.get('feishu').handleWebhook(event);
        res.json({ challenge: event.challenge });
      },
    },
    '/ui/dashboard': {
      GET: async (req, res, ctx) => {
        res.html(renderDashboard(await ctx.getStats()));
      },
    },
  },
});
```

---

## 7. 开发者体验与工具链

### 7.1 CLI 脚手架

```bash
# 创建新插件
aether plugin create my-plugin --template nodejs
aether plugin create my-plugin --template rust

# 模板选项
aether plugin create my-plugin --template nodejs --features tool,hook,command
aether plugin create my-plugin --template rust --features channel,provider

# 生成的目录结构
my-plugin/
├── aether_plugin.toml      # 预填充的 Manifest
├── SKILL.md                # 示例 Prompt
├── package.json            # (nodejs) 依赖 @aether/plugin-sdk
├── tsconfig.json           # (nodejs) TypeScript 配置
├── src/
│   └── index.ts            # 入口文件（带示例代码）
└── README.md               # 开发指南
```

### 7.2 开发模式

```bash
# 启动开发服务器（热重载）
aether plugin dev

# 输出：
# 🔌 Plugin loaded: my-plugin v0.1.0
# 📦 Tools: query_sql, list_tables
# 🪝 Hooks: before_tool_call (interceptor)
# 🔄 Watching for changes...
#
# Hot reload enabled. Edit your code and save to reload.
```

**开发模式特性：**

| 特性 | 描述 |
|------|------|
| **Hot Reload** | 文件保存后自动重载，无需重启 Aleph |
| **类型检查** | 实时 TypeScript 类型校验 |
| **Manifest 校验** | TOML 语法和 Schema 校验 |
| **模拟调用** | `aether plugin call query_sql '{"query": "SELECT 1"}'` |

### 7.3 构建与发布

```bash
# 构建
aether plugin build              # 输出到 dist/
aether plugin build --target wasm  # WASM 编译

# 校验（发布前检查）
aether plugin validate
# ✓ Manifest valid
# ✓ All declared tools have handlers
# ✓ Permissions match capability usage
# ✓ No undeclared dynamic capabilities

# 打包
aether plugin pack               # 生成 my-plugin-0.1.0.aether

# 发布到 Registry（未来）
aether plugin publish
```

### 7.4 类型生成

```bash
# 从 TOML 生成 TypeScript 类型
aether plugin typegen

# 生成结果：src/generated/types.ts
export interface QuerySqlArgs {
  query: string;
  database?: string;
}

export interface PluginConfig {
  // 从 TOML configSchema 生成
}
```

### 7.5 测试支持

```typescript
// src/index.test.ts
import { createTestContext } from '@aether/plugin-sdk/testing';
import plugin from './index';

describe('query_sql tool', () => {
  it('should execute query', async () => {
    const ctx = createTestContext({
      env: { DATABASE_URL: 'postgres://localhost/test' },
      services: {
        'connection-pool': mockPool,
      },
    });

    const result = await plugin.tools.query_sql(
      { query: 'SELECT 1', database: 'test' },
      ctx
    );

    expect(result.rowCount).toBe(1);
  });

  it('should block DROP queries', async () => {
    const ctx = createTestContext();
    const hookResult = await plugin.hooks.onBeforeToolCall({
      tool: { name: 'query_sql' },
      args: { query: 'DROP TABLE users' },
    });

    expect(hookResult.block).toBe('Destructive queries not allowed');
  });
});
```

### 7.6 调试工具

```bash
# 查看插件状态
aether plugin inspect my-plugin
# Plugin: my-plugin v0.1.0
# Status: loaded
# Tools: 2 registered (1 static, 1 dynamic)
# Hooks: 1 active
# Services: connection-pool (running)
# Memory: 12.3 MB
# Permissions: network:postgres://* (used), filesystem:./data (unused)

# 查看 Hook 执行日志
aether plugin hooks --trace
# [12:34:56] before_tool_call <- my-plugin (2ms) -> pass
# [12:34:56] before_tool_call <- security-plugin (1ms) -> pass
# [12:34:58] after_tool_call <- my-plugin (0ms) -> ok
```

---

## 8. 迁移路径与兼容性

### 8.1 从 V1 到 V2 的迁移

**V1 格式（当前）→ V2 格式对照：**

| V1 (`aether.plugin.json`) | V2 (`aether_plugin.toml`) |
|---------------------------|---------------------------|
| `"id": "my-plugin"` | `[plugin] id = "my-plugin"` |
| `"kind": "nodejs"` | `kind = "nodejs"` |
| `"entry": "dist/index.js"` | `entry = "dist/index.js"` |
| `"permissions": ["network"]` | `[permissions] network = ["*"]` |
| 无 | `[[tools]]` 静态声明 |
| 无 | `[[hooks]]` 显式分类 |
| 无 | `[prompt]` Skill 融合 |

### 8.2 兼容策略

```
Phase 1: 双格式共存（V2.0 - V2.2）
├── 检测到 aether.plugin.json → 使用 V1 Loader
├── 检测到 aether_plugin.toml → 使用 V2 Loader
└── 两者都存在 → 优先 V2，警告迁移

Phase 2: 迁移工具（V2.1）
├── aether plugin migrate  # 自动转换 JSON → TOML
└── 生成兼容性报告

Phase 3: V1 废弃（V3.0）
├── V1 Loader 标记 deprecated
└── 启动时警告
```

### 8.3 迁移命令

```bash
# 自动迁移
aether plugin migrate

# 输出：
# 📦 Migrating my-plugin from V1 to V2...
# ✓ Created aether_plugin.toml
# ✓ Converted permissions
# ⚠ Manual review needed:
#   - 3 tools detected in code, add [[tools]] declarations
#   - Consider adding [prompt] for SKILL.md
#
# Run 'aether plugin validate' to verify.
```

### 8.4 Breaking Changes 汇总

| 变更 | 影响 | 迁移方式 |
|------|------|----------|
| Manifest 格式 | 所有插件 | `aether plugin migrate` |
| 权限语法细化 | 使用权限的插件 | 手动调整为 `protocol://pattern` |
| Hook 需声明类型 | 使用 Hook 的插件 | 添加 `kind = "interceptor"` |
| 动态能力需声明 | 使用 `api.register*` 的插件 | 添加 `[capabilities]` |

---

## 附录 A：完整能力对照表

| 能力 | Aleph V1 | Aleph V2 | OpenClaw |
|------|:---------:|:---------:|:--------:|
| Tool Registration | ✅ | ✅ | ✅ |
| Hook (Typed) | ⚠️ 部分 | ✅ | ✅ |
| Prompt Injection | ⚠️ 分离 | ✅ 融合 | ✅ |
| Direct Command | ❌ | ✅ | ✅ |
| Channel Extension | ⚠️ 硬编码 | ✅ 插件化 | ✅ |
| Provider Extension | ⚠️ 硬编码 | ✅ 插件化 | ✅ |
| HTTP Route | ❌ | ✅ | ✅ |
| Background Service | ❌ | ✅ | ✅ |
| WASM Sandbox | ✅ | ✅ | ❌ |
| Permission Manifest | ⚠️ 简单 | ✅ 细粒度 | ✅ |
| Hot Reload | ❌ | ✅ | ✅ |

---

## 附录 B：设计决策记录

| 决策 | 选项 | 理由 |
|------|------|------|
| Manifest 格式 | TOML + Markdown | Rust 生态原生，Prompt 保持优雅 |
| 能力注册 | 混合模式 | 静态为主保证可审计，动态为辅保证灵活 |
| SDK 风格 (Node) | definePlugin + api.* | 静态声明友好，保留动态能力 |
| SDK 风格 (Rust) | 宏 + Trait | 宏是 DX 糖衣，Trait 是类型骨架 |
| Hook 执行 | 分类处理 | Interceptor/Observer/Resolver 各司其职 |
| Skill 融合 | 三种 Scope | System/Tool/Standalone 覆盖所有场景 |

---

*Last updated: 2026-02-03*
