# Change: Implement MCP (Model Context Protocol) Capability

## Status

**Status**: Draft
**Created**: 2026-01-08
**Updated**: 2026-01-08
**Author**: AI Assistant

## Why

Aether 当前已预留 MCP 接口（`Capability::Mcp`、`McpStrategy`），但实现为空。MCP (Model Context Protocol) 是 Anthropic 提出的标准化协议，允许 AI Agent 访问外部数据源和工具。实现 MCP 将：

1. **标准化扩展机制**：避免为每个工具编写专用集成代码
2. **零依赖体验**：用户无需安装 Node.js/Python，开箱即用
3. **安全可控**：提供细粒度权限管理，防止恶意工具调用
4. **系统级集成**：Aether 作为无窗口系统服务，可动态感知宿主应用并激活相关工具

## Core Design Principle: Zero External Dependencies

**用户永远不应该看到 `Error: npm not found` 这样的错误。**

为了保证 Aether 的开箱即用 (Out-of-the-box) 体验，我们采用**三层 MCP Server 架构**：

### 架构一览

| 层级 | 名称 | 实现方式 | 依赖 | 分发方式 |
|-----|------|---------|------|---------|
| **Layer 1** | 内置服务 (Builtin) | Rust 原生实现 | 零 | 编译进 Aether Core |
| **Layer 2** | 捆绑服务 (Bundled) | 预编译二进制 | 零 | 随 Aether.app 分发 |
| **Layer 3** | 外部扩展 (External) | 用户自选 | 用户负责 | 扩展商店/手动配置 |

## Core Design Principle: Shared Foundation Modules ⭐

**低耦合高内聚**：将核心能力抽取为独立的**共享基础模块** (`services/`)，供 MCP、Skills 以及未来扩展共同使用。

### 共享基础模块 (Shared Foundation)

```
Aether/core/src/services/           # 新建：共享基础服务模块
├── mod.rs                          # 模块导出
├── fs/                             # 文件系统服务
│   ├── mod.rs
│   └── ops.rs                      # 文件操作实现
├── git/                            # Git 服务 (git2-rs)
│   ├── mod.rs
│   └── repository.rs               # 仓库操作实现
└── system_info/                    # 系统信息服务
    ├── mod.rs
    └── macos.rs                    # macOS 特定实现
```

### 模块职责

| 模块 | 功能 | Rust 实现 | 消费者 |
|-----|------|----------|-------|
| `services::fs` | 文件读写、列表、搜索 | `tokio::fs` | MCP, Skills, 未来扩展 |
| `services::git` | Git 状态、日志、差异 | `git2-rs` | MCP, Skills, 未来扩展 |
| `services::system_info` | macOS 系统信息 | CoreFoundation | MCP, Skills, 未来扩展 |

### 架构层次关系

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Consumer Layer (消费层)                                                 │
│  ┌──────────────────────┐  ┌──────────────────────┐  ┌───────────────┐  │
│  │ MCP Capability       │  │ Skills Capability    │  │ Future...     │  │
│  │ (McpStrategy)        │  │ (SkillsStrategy)     │  │               │  │
│  └──────────────────────┘  └──────────────────────┘  └───────────────┘  │
│           │                         │                        │          │
│           └─────────────────────────┼────────────────────────┘          │
│                                     ↓                                    │
├─────────────────────────────────────────────────────────────────────────┤
│  Shared Foundation Layer (共享基础层)                                    │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────┐ │
│  │ services::fs     │  │ services::git    │  │ services::system_info  │ │
│  │ (tokio::fs)      │  │ (git2-rs)        │  │ (CoreFoundation)       │ │
│  └──────────────────┘  └──────────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
```

### 设计优势

1. **低耦合**：MCP 和 Skills 不直接依赖对方，只依赖共享基础层
2. **高内聚**：每个服务模块只负责一种能力（文件/Git/系统信息）
3. **可测试**：基础模块可独立单元测试
4. **可扩展**：新功能（如 Skills 扩展）可直接复用基础模块
5. **避免重复**：不会出现 MCP 和 Skills 各自实现文件操作的情况

---

### Layer 1: 内置 MCP 服务 (Builtin MCP Services)

在 Rust Core 内部直接实现常用能力，**无需启动任何子进程**。

内置服务通过**适配器模式**封装共享基础模块：

```
┌─────────────────────────────────────────────────────────────────────────┐
│  MCP Builtin Services (Adapter Layer)                                    │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────┐ │
│  │ FsService        │  │ GitService       │  │ SystemInfoService      │ │
│  │ (MCP Adapter)    │  │ (MCP Adapter)    │  │ (MCP Adapter)          │ │
│  │                  │  │                  │  │                        │ │
│  │ Implements:      │  │ Implements:      │  │ Implements:            │ │
│  │ BuiltinMcpSvc    │  │ BuiltinMcpSvc    │  │ BuiltinMcpSvc          │ │
│  └────────┬─────────┘  └────────┬─────────┘  └────────────┬───────────┘ │
│           │                     │                         │             │
│           ↓                     ↓                         ↓             │
├─────────────────────────────────────────────────────────────────────────┤
│  Shared Foundation (共享基础层)                                          │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────┐ │
│  │ services::fs     │  │ services::git    │  │ services::system_info  │ │
│  └──────────────────┘  └──────────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
```

| 服务名 | 功能 | 底层模块 |
|-------|------|---------|
| `builtin:fs` | 文件系统访问 | `services::fs` |
| `builtin:git` | Git 操作 | `services::git` |
| `builtin:system-info` | macOS 系统信息 | `services::system_info` |
| `builtin:shell` | Shell 命令执行 | `tokio::process::Command` (独立) |

### Layer 2: 捆绑服务 (Bundled MCP Servers)

对于复杂集成（如 Notion、Google Drive），如果社区已有 TypeScript 实现：
- 使用 **Bun** (`bun build --compile`) 或 **pkg** 预编译为独立二进制
- 打包进 `Aether.app/Contents/Resources/mcp-servers/`
- 用户无感知，零依赖

| 服务名 | 原始实现 | 打包方式 | 体积 |
|-------|---------|---------|------|
| `bundled:notion` | TypeScript | Bun compile | ~50MB |
| `bundled:slack` | TypeScript | Bun compile | ~45MB |
| `bundled:browser` | TypeScript | Bun compile | ~60MB |

**分发策略**：
- MVP 阶段不打包任何 Layer 2 服务
- 按需添加，用户可在 Settings 中下载

### Layer 3: 外部扩展 (External MCP Servers)

为高级用户保留扩展能力：
- 允许配置外部 MCP Server（需用户自行安装运行时）
- 检测系统是否有 Node.js/Python
- 缺少运行时时显示友好提示，而非崩溃

```toml
# 仅当用户显式配置且系统有 Node.js 时生效
[mcp.servers.custom-server]
transport = "stdio"
command = "npx"
args = ["-y", "@some/mcp-server"]
requires_runtime = "node"  # 新增字段：声明运行时依赖
```

## What Changes

### Core Changes (Rust)

**共享基础模块 (新建)**：
- **ADDED**: `services/` 模块 - 共享基础服务层
- **ADDED**: `services::fs` - 文件系统操作（`FileOps` trait + `LocalFs` impl）
- **ADDED**: `services::git` - Git 操作（`GitOps` trait + `GitRepository` impl，基于 git2-rs）
- **ADDED**: `services::system_info` - 系统信息（`SystemInfoProvider` trait + macOS impl）

**MCP 适配层**：
- **ADDED**: `BuiltinMcpService` trait - 内置服务抽象
- **ADDED**: `mcp::builtin::FsService` - 文件系统 MCP 适配器（封装 `services::fs`）
- **ADDED**: `mcp::builtin::GitService` - Git MCP 适配器（封装 `services::git`）
- **ADDED**: `mcp::builtin::SystemInfoService` - 系统信息 MCP 适配器（封装 `services::system_info`）
- **ADDED**: `mcp::builtin::ShellService` - Shell 命令内置服务（独立实现）
- **ADDED**: `McpClient` - MCP 客户端管理多个服务
- **ADDED**: `McpServerConnection` - 外部 MCP Server 的 JSON-RPC 2.0 通信
- **ADDED**: `StdioTransport` - Stdio 传输层（子进程通信）
- **ADDED**: 数据结构 `McpResource`、`McpTool`、`McpToolResult`
- **MODIFIED**: `McpStrategy` - 填充 `execute()` 实现
- **MODIFIED**: `PromptAssembler` - 添加 `format_mcp_context_markdown()` 方法
- **MODIFIED**: `AgentContext.mcp_resources` - 从 `HashMap<String, Value>` 改为 `Option<McpContext>`
- **MODIFIED**: `config/mod.rs` - 添加 `[mcp]` 配置段解析

### Swift Layer

- **ADDED**: `McpSettingsView` - MCP 服务管理 UI
- **ADDED**: Tool 调用确认对话框（安全机制）
- **MODIFIED**: UniFFI 接口扩展 MCP 相关方法

### Configuration

- **ADDED**: `config.toml` 新增 `[mcp]` 配置段

## Impact

### Affected Specs
- `capability/strategies/mcp.rs` - 主要实现位置
- `payload/mod.rs` - AgentContext 扩展
- `payload/assembler.rs` - Context 格式化
- `config/mod.rs` - 配置解析

### Affected Code
- `Aether/core/src/services/` (新建：共享基础模块)
- `Aether/core/src/mcp/` (新建：MCP 模块)
- `Aether/core/src/mcp/builtin/` (MCP 适配器)
- `Aether/core/src/capability/strategies/mcp.rs`
- `Aether/core/src/config/mod.rs`
- `Aether/Sources/Components/Settings/`

### Breaking Changes
- **NONE** - 所有变更向后兼容，空 MCP 配置下行为不变

### Dependencies (New Crates)

| Crate | Purpose | Justification |
|-------|---------|---------------|
| `git2` | Git 操作 | 替代调用 git CLI，零依赖 |

### Future Impact: Skills Integration (告知)

> **注意**：本提案创建的共享基础模块 (`services/`) 设计为供 Skills 功能复用。
> Skills 模块当前已实现扩展机制，未来可能需要以下调整：
>
> 1. Skills 的 `allowed-tools` 字段可映射到 `services::*` 操作
> 2. Skill 执行时可调用 `services::fs`、`services::git` 等
> 3. 这些调整将作为**独立的后续提案**处理，不在本提案范围内

## Design Decisions

### D0: 共享基础模块架构 (采纳) ⭐⭐

**决策**：创建独立的 `services/` 模块作为共享基础层，MCP 和 Skills 通过适配器模式使用。

**理由**：
- **低耦合**：MCP 不依赖 Skills，Skills 不依赖 MCP
- **高内聚**：文件操作、Git 操作、系统信息各自独立
- **可复用**：未来功能可直接使用基础模块
- **可测试**：基础模块可独立单元测试

**替代方案**：
- 在 MCP 内部实现，Skills 复制代码 - **否决**，代码重复
- MCP 依赖 Skills 或反过来 - **否决**，增加耦合度

**实现方式**：
```rust
// services/fs/mod.rs
pub trait FileOps: Send + Sync {
    async fn read_file(&self, path: &Path) -> Result<String>;
    async fn write_file(&self, path: &Path, content: &str) -> Result<()>;
    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>>;
}

// mcp/builtin/fs.rs (MCP 适配器)
pub struct FsService {
    fs: Arc<dyn FileOps>,  // 依赖 services::fs trait
    allowed_roots: Vec<PathBuf>,
}

// skills/ (未来可能的使用方式)
// skill.allowed_tools = ["file_read", "file_write"]
// → 调用 services::fs::LocalFs
```

### D1: 三层服务架构 (采纳) ⭐

**决策**：采用 Builtin → Bundled → External 三层架构。

**理由**：
- **用户体验至上**：普通用户开箱即用，无需配置任何运行时
- **高级用户自由**：保留 External 层供极客用户扩展
- **渐进式复杂度**：MVP 只需 Layer 1，Layer 2/3 按需添加

**替代方案**：全部依赖 NPM 生态 - **否决**，破坏用户体验

### D2: 内置服务优先 (采纳) ⭐

**决策**：常用能力（fs、git、system-info）直接在 Rust 中实现，不启动子进程。

**理由**：
- 零启动延迟
- 零外部依赖
- 与 Aether Core 共享类型系统

**实现方式**：`BuiltinMcpService` trait 作为 MCP 适配器，底层调用 `services::*`。

### D3: 运行时检测 (采纳)

**决策**：External 层配置需声明 `requires_runtime`，启动时检测系统是否有对应运行时。

**理由**：
- 避免 "npm not found" 崩溃
- 给用户友好的安装指引

**行为**：
- 缺少运行时时，显示 Toast: "需要安装 Node.js 才能使用此服务"
- 服务标记为 unavailable，不影响其他服务

### D4: 模块化策略模式 (采纳)

**决策**：利用现有 `CapabilityStrategy` trait，在 `McpStrategy` 中实现完整逻辑。

**理由**：
- 保持低耦合：MCP 逻辑不侵入 Core 主流程
- 与 Memory、Search、Skills 等 Capability 统一管理
- 易于测试：可独立 Mock `McpStrategy`

### D5: 工具调用安全机制 (采纳)

**决策**：实现三层权限控制 - 配置白名单、代码层检查、UI 确认对话框。

**理由**：
- MCP 协议强调 "Human in the loop"
- Aether 无主窗口，需在 Halo/Popover 中显示确认
- 危险工具（file_write、shell_exec）需用户手动确认

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│  Swift Layer (UI/OS Integration)                                                 │
│  ┌───────────────────┐  ┌────────────────────────────────────────────────────┐  │
│  │ McpSettingsView   │  │ ToolCallConfirmation (Popover/Halo)                │  │
│  └───────────────────┘  └────────────────────────────────────────────────────┘  │
└──────────────────────────────────────┬──────────────────────────────────────────┘
                                       │ UniFFI Callback
                                       ↓
┌─────────────────────────────────────────────────────────────────────────────────┐
│  Rust Core                                                                       │
│                                                                                  │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │ Capability Layer (消费层)                                                   │ │
│  │ ┌──────────────────────┐  ┌──────────────────────┐  ┌───────────────────┐  │ │
│  │ │ McpStrategy          │  │ SkillsStrategy       │  │ Future Strategies │  │ │
│  │ │ (CapabilityStrategy) │  │ (CapabilityStrategy) │  │                   │  │ │
│  │ └──────────┬───────────┘  └──────────┬───────────┘  └─────────┬─────────┘  │ │
│  └────────────┼─────────────────────────┼────────────────────────┼────────────┘ │
│               │                         │                        │              │
│               ↓                         │                        │              │
│  ┌────────────────────────────────────┐ │                        │              │
│  │ MCP Builtin Services (Adapters)    │ │                        │              │
│  │ ┌──────────┐ ┌──────────┐ ┌──────┐ │ │                        │              │
│  │ │FsService │ │GitService│ │Shell │ │ │                        │              │
│  │ └────┬─────┘ └────┬─────┘ └──────┘ │ │                        │              │
│  └──────┼────────────┼────────────────┘ │                        │              │
│         │            │                  │                        │              │
│         └────────────┼──────────────────┼────────────────────────┘              │
│                      ↓                  ↓                                       │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │ Shared Foundation Layer (共享基础层) - services/                           │ │
│  │ ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────────┐ │ │
│  │ │ services::fs     │  │ services::git    │  │ services::system_info      │ │ │
│  │ │ FileOps trait    │  │ GitOps trait     │  │ SystemInfoProvider trait   │ │ │
│  │ │ LocalFs impl     │  │ GitRepository    │  │ MacOsSystemInfo impl       │ │ │
│  │ │ (tokio::fs)      │  │ (git2-rs)        │  │ (CoreFoundation)           │ │ │
│  │ └──────────────────┘  └──────────────────┘  └────────────────────────────┘ │ │
│  └────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                  │
│  ┌────────────────────────────────────────────────────────────────────────────┐ │
│  │ External MCP Layer (Layer 3)                                               │ │
│  │ ┌─────────────────────────────────────────────────────────────────────────┐│ │
│  │ │ StdioTransport → JSON-RPC 2.0 → External Process                        ││ │
│  │ │ (with runtime detection: node/python/bun)                               ││ │
│  │ └─────────────────────────────────────────────────────────────────────────┘│ │
│  └────────────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────────────┘
```

## Implementation Phases

### Phase 0: Shared Foundation (新增) ⭐

**目标**：建立共享基础模块，供 MCP 和 Skills 复用

- 创建 `services/` 模块结构
- `services::fs` - `FileOps` trait + `LocalFs` impl
- `services::git` - `GitOps` trait + `GitRepository` impl (git2-rs)
- `services::system_info` - `SystemInfoProvider` trait + macOS impl
- 单元测试覆盖

### Phase 1: Builtin Services (MVP) ⭐

**目标**：用户零配置即可使用核心 MCP 功能

- `BuiltinMcpService` trait 定义 (MCP 适配器接口)
- `FsService` - 封装 `services::fs`
- `GitService` - 封装 `services::git`
- `SystemInfoService` - 封装 `services::system_info`
- `ShellService` - Shell 命令执行 (独立实现)
- `McpClient` 服务注册与路由
- `McpStrategy` 完整实现
- `PromptAssembler` MCP 格式化
- 工具权限控制

### Phase 2: External Support

**目标**：支持高级用户扩展

- `StdioTransport` JSON-RPC 2.0 通信
- `McpServerConnection` 外部服务管理
- 运行时检测（node/python/bun）
- 配置文件 `requires_runtime` 字段

### Phase 3: UI & Polish

**目标**：完善用户界面

- `McpSettingsView` - 服务管理 UI
- Tool 调用确认对话框
- 服务健康检查
- 自动重启机制

### Phase 4: Bundled Servers (Future)

**目标**：提供开箱即用的高级服务

- 评估 Notion/Slack/Browser 等集成需求
- 使用 Bun compile 打包 TypeScript 服务
- 分发到 Aether.app/Contents/Resources/

## References

- [MCP Official Specification](https://modelcontextprotocol.io/)
- [MCP TypeScript SDK](https://github.com/anthropics/modelcontextprotocol)
- [git2-rs](https://github.com/rust-lang/git2-rs) - Rust Git bindings
- 内部文档: `docs/architecture/08_MCP_INTERFACE_RESERVATION.md`
- 用户设计: `macstructure.md`
