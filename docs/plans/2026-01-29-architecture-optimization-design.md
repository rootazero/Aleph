# Aleph 架构优化实施规划

> **创建日期**: 2026-01-29
> **状态**: 待实施
> **预计工期**: 42-58 天

---

## 执行摘要

本文档基于架构审查报告，制定了 Aleph 项目的渐进式优化路线图。采用 4 阶段实施策略：

1. **阶段 1: 快速清理** — rig-core 残留清理 (0.5-1 天)
2. **阶段 2: 核心问题修复** — FFI 删除 + Session Key + Block Streaming (9-13 天)
3. **阶段 3: 中期改进** — chat/webhooks/node/media/canvas (20-27 天)
4. **阶段 4: 完整重构** — 模块重组 P0→P3 (12-17 天)

---

## 阶段 1: 快速清理 (0.5-1 天)

### 1.1 依赖清理

**文件**: `/Cargo.toml` (workspace)

```toml
# 删除以下两行 (L85-86)
rig-core = "0.28"
rig-sqlite = "0.1.31"
```

### 1.2 目录重命名

```
core/src/rig_tools/  →  core/src/builtin_tools/
```

同步更新：
- `core/src/lib.rs` 中的 `mod rig_tools` → `mod builtin_tools`
- 所有 `use crate::rig_tools::` 引用
- `aether.udl` 中相关类型声明（如有）

### 1.3 模块注释更新

**文件**: `core/src/agents/rig/mod.rs`

```rust
// 旧: "Agent configuration based on rig-core library"
// 新: "Agent configuration for self-implemented AlephTool system"
```

### 1.4 CLAUDE.md 技术栈更新

```markdown
# 旧 (L424)
| **Agent Runtime** | Rust + async/await + rig-core |

# 新
| **Agent Runtime** | Rust + async/await + AlephTool |
```

### 1.5 历史文档归档

创建目录并移动文件：

```
docs/legacy/rig-core-era/
├── rig-core-router-L3-Agent.md      ← 从项目根目录移入
├── README.md                         ← 新建说明文件
└── openspec-references/              ← 可选
```

归档 README 内容：

```markdown
# rig-core 时代文档归档

本目录包含 Aleph 早期使用 rig-core 框架时的设计文档。
项目已于 2025 年迁移到自研 AlephTool 系统。

这些文档仅供历史参考，不再反映当前架构。
```

### 1.6 验证清单

- [ ] `cargo build -p alephcore` 成功
- [ ] `cargo test -p alephcore` 通过
- [ ] `grep -r "rig-core" core/src/` 无结果（除注释说明）
- [ ] `grep -r "rig_tools" core/src/` 无结果

---

## 阶段 2: 核心问题修复 (9-13 天)

### 2.1 无 FFI 架构转型 (3-5 天)

#### 架构变更

```
当前架构:
┌─────────────────┐      ┌─────────────────┐
│  macOS App      │      │  Rust Core      │
│  (Swift)        │─────▶│  (Library)      │
│                 │ FFI  │                 │
└─────────────────┘      └─────────────────┘

新架构:
┌─────────────────┐      ┌─────────────────┐
│  macOS App      │      │  Rust Core      │
│  (Swift)        │─────▶│  (Daemon)       │
│                 │  WS  │  ws://127.0.0.1 │
└─────────────────┘      └─────────────────┘
```

#### 删除的代码

| 目录/文件 | 文件数 | 大小 |
|-----------|--------|------|
| `core/src/ffi/` | 25 | ~50KB |
| `core/src/ffi_cabi/` | 16 | ~30KB |
| `core/src/aether.udl` | 1 | 69KB |
| `platforms/macos/.../Generated/` | UniFFI 生成 | ~100KB |

#### AlephConnectionManager (Swift)

```swift
class AlephConnectionManager: ObservableObject {
    enum State {
        case disconnected
        case connecting
        case connected(token: String)
        case reconnecting(attempt: Int)
    }

    @Published var state: State = .disconnected

    func connect() async {
        // 1. 探测已运行的 daemon
        if await healthCheck() {
            await authenticateExisting()
            return
        }

        // 2. 未运行则自动拉起
        let token = generateSecureToken()
        await spawnDaemon(token: token)

        // 3. 等待 daemon 就绪
        await waitForReady(timeout: 5.0)

        // 4. 建立 WebSocket 连接
        await connectWebSocket(token: token)
    }
}
```

#### Daemon 管理模式

**混合模式**：
- 优先连接已运行的 daemon
- 未运行时 App 自动启动
- UI 关闭后 daemon 继续运行（永远在线）

#### 安全设计

- App 启动时生成 32 字节随机 token
- Token 作为参数传给 daemon
- Token 存入 macOS Keychain
- 所有 WS 请求必须携带 `Authorization: Bearer <token>`
- 防止恶意网页通过 WS 攻击

#### 文件传输原则

```
❌ 反模式 (传内容)：Base64 编码文件内容 → 内存爆炸
✅ 正确模式 (传路径)：只传文件路径，Rust 直接读取磁盘
```

#### 清单

- [ ] 删除 `ffi/`, `ffi_cabi/`, `aether.udl`
- [ ] 新建 `AetherConnectionManager.swift`
- [ ] 实现 daemon spawn / health check / heartbeat
- [ ] 添加 Token 认证机制
- [ ] 更新所有工具使用路径模式
- [ ] 更新 macOS App 构建脚本

---

### 2.2 Session Key 全变体实现 (3-4 天)

#### 类型定义

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionKey {
    /// 跨渠道共享主会话: "agent:{agent_id}:main"
    Main { agent_id: String },

    /// DM 私聊: "agent:{agent_id}:{channel}:dm:{peer_id}"
    DirectMessage {
        agent_id: String,
        channel: ChannelType,
        peer_id: String,
        scope: DmScope,
    },

    /// 群组/频道: "agent:{agent_id}:{channel}:group:{group_id}"
    Group {
        agent_id: String,
        channel: ChannelType,
        group_id: String,
    },

    /// 定时任务/Webhook: "agent:{agent_id}:task:{task_id}"
    Task {
        agent_id: String,
        task_id: String,
        task_type: TaskType,
    },

    /// 子 agent 委托: "subagent:{parent_key}:{subagent_name}"
    Subagent {
        parent_key: Box<SessionKey>,
        subagent_name: String,
        mode: SubagentMode,
    },

    /// 临时会话: "agent:{agent_id}:ephemeral:{uuid}"
    Ephemeral {
        agent_id: String,
        uuid: String,
        purpose: EphemeralPurpose,
    },
}
```

#### Subagent 模式

```rust
pub enum SubagentMode {
    /// 工具式调用：执行完返回结果给父 agent
    Tool { timeout: Duration, max_turns: u32 },

    /// 并行协作：与其他子 agent 同时执行
    Parallel { group_id: String, merge_strategy: MergeStrategy },
}

pub enum MergeStrategy {
    WaitAll,        // 等待全部完成
    FirstSuccess,   // 任一完成即返回
    Consensus { min_agree: u32 },  // 投票共识
}
```

#### sessions_send 工具

```rust
pub struct SessionsSendParams {
    pub target: SessionTarget,
    pub message: String,
    pub mode: ExecutionMode,  // Sync | Async | Stream
    pub timeout: u64,
}

pub enum SessionTarget {
    Subagent(String),           // 子 agent 名称
    SessionKey(String),         // 完整 session key
    Parallel(Vec<ParallelTarget>),  // 并行多个
}
```

#### 清单

- [ ] 实现 `SessionKey` 全 6 变体
- [ ] 实现 `DmScope`, `SubagentMode`, `EphemeralPurpose` 枚举
- [ ] 实现 `ParallelExecutor` 并行执行器
- [ ] 实现 `sessions_send` 工具
- [ ] 实现 `sessions_list` 工具
- [ ] 更新 `SessionManager` 支持新变体
- [ ] 添加 Ephemeral session 自动清理

---

### 2.3 Block Streaming 实现 (3-4 天)

#### 内容块类型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Thinking(ThinkingBlock),      // 推理过程（可折叠）
    Text(TextBlock),              // 普通文本
    Artifact(ArtifactBlock),      // 代码/文档产物
    ToolCall(ToolCallBlock),      // 工具调用
    ToolResult(ToolResultBlock),  // 工具结果
    Citation(CitationBlock),      // 引用来源
    Error(ErrorBlock),            // 错误信息
}
```

#### 流式事件

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamEvent {
    BlockStart { block_id: String, block_type: String },
    BlockDelta { block_id: String, delta: String },
    BlockEnd { block_id: String },
    ToolStatus { block_id: String, status: ToolStatus },
    Done { total_blocks: usize },
    Error { message: String },
}
```

#### BlockStreamParser 状态机

解析流式 token，识别 `<think>`、`<artifact>` 等标签，生成对应的 StreamEvent。

#### Swift UI 渲染

- `BlockStreamView`: 块列表视图
- `ThinkingBlockView`: 可折叠的思考过程
- `ArtifactBlockView`: 代码高亮 + 复制按钮
- `ToolCallBlockView`: 工具状态显示

#### 清单

- [ ] 实现 `ContentBlock` 全类型枚举
- [ ] 实现 `BlockStreamParser` 状态机
- [ ] 扩展 `StreamEvent` 事件类型
- [ ] 更新 `agent.run` handler 支持块流
- [ ] Swift 端实现 `MessageStreamViewModel`
- [ ] Swift 端实现各块类型视图组件
- [ ] 添加 thinking 块折叠/展开交互

---

## 阶段 3: 中期改进 (20-27 天)

按复杂度递增顺序：3.1 → 3.2 → 3.4 → 3.3 → 3.5

### 3.1 chat.* RPC 方法 (1-2 天)

```rust
// chat.send - 发送消息（比 agent.run 更轻量）
// chat.history - 查询历史记录
// chat.interrupt - 中断当前生成
// chat.resend - 重新生成最后回复
```

### 3.2 Webhooks 外部触发器 (1-2 天)

配置示例：

```json5
{
  "webhooks": {
    "github-pr": {
      "enabled": true,
      "secret": "whsec_xxx",
      "agent": "main",
      "template": "收到 GitHub PR 事件:\n{{json payload}}"
    }
  }
}
```

RPC 方法：`webhooks.list`, `webhooks.test`, `webhooks.logs`

### 3.4 Node Protocol (4-5 天)

#### Node 能力声明

```rust
pub enum NodeCapability {
    Camera { max_resolution: (u32, u32), supports_video: bool },
    Voice { supports_wake_word: bool, supports_streaming: bool },
    Canvas { max_size: (u32, u32), supports_interaction: bool },
    ScreenCapture { supports_video: bool },
    FileSystem { allowed_paths: Vec<String> },
    Browser { engine: String },
    Location { precision: LocationPrecision },
    Notification,
    Clipboard,
}
```

#### 设备配对流程

1. Node App 启动 → Bonjour/mDNS 发现 Gateway
2. 发现 Gateway → Node 显示配对请求
3. 用户确认 → Gateway 生成 6 位配对码
4. Node 输入配对码 → Gateway 验证 → 颁发 Device Token
5. 配对完成 → Node 可随时连接

#### Node 工具

```rust
// node.capture_photo - 拍照
// node.voice_input - 语音输入
// node.show_canvas - 显示可视化内容
```

### 3.3 Media Pipeline (5-7 天)

#### 媒体类型

```rust
pub struct MediaObject {
    pub id: String,
    pub media_type: MediaType,  // Image | Audio | Video | Document
    pub mime_type: String,
    pub size: u64,
    pub local_path: PathBuf,
    pub metadata: MediaMetadata,
}
```

#### 处理能力

- **图片**: resize, compress, thumbnail, EXIF 提取
- **音频**: 格式转换, Whisper 转录
- **视频**: 缩略图提取, 元数据读取

#### RPC 方法

`media.upload`, `media.import_url`, `media.get`, `media.transcribe`, `media.resize`, `media.delete`

### 3.5 Canvas (A2UI) MVP (7-10 天)

#### Canvas 内容类型

```rust
pub enum CanvasContent {
    React { component: String, props: Value },
    Html { html: String, css: Option<String> },
    Markdown { markdown: String },
    Chart { option: Value },  // ECharts
    Mermaid { code: String },
    Table { headers: Vec<String>, rows: Vec<Vec<CellValue>>, editable: bool },
    Form { schema: Value },
    Image { source: ImageSource },
    Composite { layout: CompositeLayout, children: Vec<CanvasContent> },
}
```

#### Agent 工具

```rust
// canvas_show - 显示可视化内容
pub struct CanvasShowParams {
    pub title: String,
    pub content: CanvasToolContent,
    pub device_id: Option<String>,
}
```

#### RPC 方法

`canvas.create`, `canvas.update`, `canvas.add_page`, `canvas.export`, `canvas.close`, `canvas.list`

---

## 阶段 4: 完整重构 (12-17 天)

### 4.0 P0 批次：核心模块提升 (1-2 天)

#### channels 模块提升

```
gateway/channels/  →  channels/
├── mod.rs
├── traits.rs
├── registry.rs     # 新增
├── telegram/
├── discord/
├── imessage/
├── webchat/
└── cli/
```

#### sessions 模块提升

```
gateway/session_manager.rs  →  sessions/
routing/session_key.rs      →  sessions/
├── mod.rs
├── key.rs
├── manager.rs
├── store.rs
├── compaction.rs
└── policy.rs
```

#### rig_tools 重命名

```
rig_tools/  →  builtin_tools/
├── mod.rs
├── bash/
├── file/
├── search/
├── web/
├── sessions/   # 新增
└── node/       # 新增
```

### 4.1 P1 批次：配置与安全 (3-4 天)

#### config 集中管理

```
config/
├── mod.rs
├── schema.rs       # schemars JSON Schema
├── io.rs           # JSON5 读写
├── hot_reload.rs
├── validation.rs
├── env_vars.rs
├── migration.rs
└── types/
    ├── root.rs     # AlephConfig
    ├── agents.rs
    ├── gateway.rs
    ├── channels.rs
    ├── providers.rs
    └── security.rs
```

#### security 模块抽取

```
security/
├── mod.rs
├── auth.rs
├── pairing.rs
├── permissions.rs
├── audit.rs
└── token.rs
```

#### agent_loop 整合

```
agents/
├── config/         # 原 agents/rig/
├── loop/           # 原 agent_loop/
├── thinking/
├── sub_agents/
├── skills/
└── compaction/
```

### 4.2 P2 批次：功能模块扩展 (5-7 天)

#### media 模块

```
media/
├── types.rs
├── pipeline.rs
├── store.rs
├── image/
├── audio/
├── video/
└── cleanup.rs
```

#### hooks 模块

```
hooks/
├── loader.rs
├── registry.rs
├── executor.rs
├── types.rs
└── builtin/
    ├── gmail.rs
    ├── github.rs
    └── slack.rs
```

#### browser 增强

```
browser/
├── cdp/
│   ├── client.rs
│   ├── protocol.rs
│   └── domains/
├── chrome/
│   ├── launcher.rs
│   ├── finder.rs
│   └── profiles.rs
├── session.rs
├── screenshots.rs
└── interactions.rs
```

### 4.3 P3 批次：Node Host 与收尾 (3-4 天)

#### node_host 模块

```
node_host/
├── config.rs
├── registry.rs
├── protocol.rs
├── runner.rs
├── discovery.rs
├── pairing.rs
└── capabilities/
```

#### 文档补充

```
docs/
├── adr/                    # 架构决策记录
│   ├── 001-websocket-only.md
│   ├── 002-session-key-design.md
│   └── 003-node-protocol.md
├── api/                    # API 文档
│   ├── rpc-methods.md
│   ├── websocket-protocol.md
│   └── node-protocol.md
├── guides/                 # 使用指南
└── CONTRIBUTING.md
```

#### 依赖清理

```toml
# 移除
# rig-core = "0.28"
# rig-sqlite = "0.1.31"
# uniffi = "0.31"
# uniffi_macros = "0.31"
```

---

## 工作量总结

| 阶段 | 内容 | 工作量 |
|------|------|--------|
| 1. 快速清理 | rig-core 残留清理 | 0.5-1 天 |
| 2. 核心问题 | FFI删除 + Session Key + Block Streaming | 9-13 天 |
| 3. 中期改进 | chat/webhooks/node/media/canvas | 20-27 天 |
| 4. 完整重构 | 模块重组 P0→P3 | 12-17 天 |
| **总计** | | **42-58 天** |

---

## 实施原则

1. **渐进式** — 按阶段顺序执行，每阶段验证后再继续
2. **可回滚** — 每个重大变更前创建 git tag
3. **测试优先** — 每步变更后运行 `cargo build` + `cargo test`
4. **文档同步** — 代码变更同时更新相关文档

---

*文档生成于 2026-01-29 | Aleph 架构优化 Brainstorming 会议*
