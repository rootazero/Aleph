<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

# CLAUDE.md

> *"这是人类历史上第一次，赋予了机器的灵魂一个躯壳。"*
> — 攻壳机动队 / Ghost in the Shell

This file provides guidance to Claude Code when working with code in this repository.

---

## 🔮 核心哲学

### 五层涌现架构

```
散落的积木 → 分类堆放 → 堆叠整齐 → 功能模块 → 多态智能体
   ↓            ↓           ↓          ↓           ↓
经验之海    领域分类    原子技能    即插即用    随需而变
(Know)     (Classify)  (Know-how)  (Compose)   (Embody)
```

| 层级 | 名称 | 本质转变 |
|------|------|----------|
| **L1** | 经验之海 | 互联网、代码、历史、常识 — AI 的预训练养料 |
| **L2** | 领域分类 | 医学、法律、编程、物理 — 知识有了学科边界 |
| **L3** | 原子技能 | **Know-what → Know-how** — 从拥有知识到拥有能力 |
| **L4** | 功能模块 | 技能封装，即插即用 — AI 可以组合能力达成目标 |
| **L5** | 多态智能体 | **灵魂获得躯壳** — 随需变身，干涉物理/数字世界 |

### Ghost 美学

| 原则 | 实现 |
|------|------|
| **Invisible First** | 无 Dock 图标、无常驻窗口，只有后台进程 + 菜单栏 |
| **Frictionless** | AI 来到你身边，而不是你去找 AI |
| **Native-First** | 100% 原生代码 (Rust + Swift) |
| **Polymorphic** | 一个灵魂，无限形态 |

### 🧠 Agent 设计思想：POE 架构

Aleph 的 Agent 核心采用 **POE (Principle-Operation-Evaluation)** 架构，融合双系统认知模型：

- **第一性原理** — 先定义成功契约，再开始执行
- **启发式思考** — System 1 (快速直觉) + System 2 (深度推理) 协同
- **自我学习** — 成功经验结晶化，相似任务自动借鉴

详见：[Agent 设计哲学](docs/AGENT_DESIGN_PHILOSOPHY.md) | [POE 架构设计](docs/plans/2026-02-01-poe-architecture-design.md)

### 🏛️ 领域建模：DDD 筑底

Aleph 采用 **DDD (Domain-Driven Design)** 的核心概念来组织领域逻辑，通过 Rust trait 系统实现轻量级的领域规约。

#### 统一语言 (Ubiquitous Language)

| 术语 | 定义 | 示例 |
|------|------|------|
| **Entity** | 具有唯一身份标识的对象，身份在状态变化中保持不变 | `Task`, `MemoryFact` |
| **AggregateRoot** | 聚合的入口点，管理一组相关对象的一致性边界 | `TaskGraph`, `MemoryFact` |
| **ValueObject** | 由属性定义的不可变对象，无身份标识 | `TaskStatus`, `ContextAnchor` |

#### Domain Traits (`core/src/domain/`)

```rust
pub trait Entity {
    type Id: Eq + Clone + Display;
    fn id(&self) -> &Self::Id;
}

pub trait AggregateRoot: Entity {}

pub trait ValueObject: Eq + Clone {}
```

#### 限界上下文 (Bounded Contexts)

| 上下文 | 聚合根 | 职责 |
|--------|--------|------|
| **Dispatcher** | `TaskGraph` | DAG 调度、工具编排、任务状态管理 |
| **Memory** | `MemoryFact` | 事实存储、RAG 检索、知识压缩 |
| **Intent** | `AggregatedIntent` | 意图检测、L1-L3 分层过滤 |
| **POE** | `SuccessManifest` | 成功契约、验证规则、评估结果 |

详见：[领域建模指南](docs/DOMAIN_MODELING.md) | [DDD+BDD 设计](docs/plans/2026-02-06-ddd-bdd-dual-wheel-design.md)

---

## 🏗️ 架构概览

**Aleph 是一个自托管的个人 AI 助手**，通过 WebSocket Gateway 统一管理多渠道消息、Agent 执行、工具调用和记忆系统。

```
┌─────────────────────────────────────────────────────────────────┐
│                       INTERFACE LAYER                            │
│   macOS App │ Tauri App │ CLI │ Telegram │ Discord │ WebChat    │
│              (纯 I/O — 输入用户消息，展示 Server 响应)            │
└───────────────────────────────┬─────────────────────────────────┘
                                │ WebSocket (JSON-RPC 2.0)
                                │ ws://127.0.0.1:18789
┌───────────────────────────────┴─────────────────────────────────┐
│                    ALEPH SERVER (自包含)                          │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │ GATEWAY — Router │ Session │ Event Bus │ Interfaces      │    │
│  └──────────────────────────────┬───────────────────────────┘    │
│                                 │                                 │
│  ┌──────────────────────────────┴───────────────────────────┐    │
│  │ AGENT — Observe → Think → Act → Feedback → Compress      │    │
│  └──────────────────────────────┬───────────────────────────┘    │
│                                 │                                 │
│  ┌──────────────────────────────┴───────────────────────────┐    │
│  │ EXECUTION — Providers │ Executor │ Tool Server │ MCP      │    │
│  └──────────────────────────────┬───────────────────────────┘    │
│                                 │                                 │
│  ┌──────────────────────────────┴───────────────────────────┐    │
│  │ STORAGE — Memory (LanceDB) │ State (SQLite) │ Config      │    │
│  └──────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### 核心子系统

| 子系统 | 描述 | 文档 |
|--------|------|------|
| **Gateway** | WebSocket 控制面，JSON-RPC 2.0 协议，30+ RPC 方法 | [Gateway](docs/GATEWAY.md) |
| **Agent Loop** | Observe-Think-Act-Feedback 循环，状态机驱动 | [Agent System](docs/AGENT_SYSTEM.md) |
| **Thinker** | LLM 交互，Thinking Levels，流式响应 | [Agent System](docs/AGENT_SYSTEM.md) |
| **Dispatcher** | 任务编排，DAG 调度，多步执行 | [Agent System](docs/AGENT_SYSTEM.md) |
| **Tool Server** | AlephTool trait，19+ 内置工具 | [Tool System](docs/TOOL_SYSTEM.md) |
| **Memory** | LanceDB 统一存储，混合检索 (ANN + FTS)，MemoryStore/GraphStore/SessionStore traits | [Memory System](docs/MEMORY_SYSTEM.md) |
| **Resilience** | 多 Agent 弹性系统，StateDatabase (SQLite) 管理事件/任务/追踪/会话 | — |
| **Extension** | WASM + Node.js 插件运行时 | [Extension System](docs/EXTENSION_SYSTEM.md) |
| **Exec** | Shell 执行安全，审批工作流 | [Security](docs/SECURITY.md) |

详见：[完整架构文档](docs/ARCHITECTURE.md)

### 🏛️ Server-Centric 架构

Aleph 是一个**自包含的 AI Server**。所有感知、思考、执行都在 Server 侧完成。

外部 Interface（App、Bot、CLI）仅负责消息的输入和展示，不承担任何业务逻辑。
它们如同社交软件的对话窗口，只是交互入口。

| Interface 类型 | 示例 | 通信方式 |
|---------------|------|----------|
| **Native App** | macOS App, Tauri Desktop | WebSocket |
| **CLI** | aleph-cli | WebSocket |
| **Bot** | Telegram, Discord | 各平台 API |
| **Web** | Dashboard, WebChat | HTTP/WebSocket |

---

## 📁 项目结构

```
aleph/
├── core/                           # Rust Core (alephcore crate)
│   └── src/
│       ├── gateway/                # WebSocket 控制面 (34 files)
│       │   ├── handlers/           # RPC 方法处理器 (33 handlers)
│       │   ├── interfaces/          # 交互接口 (Telegram, Discord, iMessage)
│       │   └── security/           # 认证、配对、设备管理
│       ├── agent_loop/             # Observe-Think-Act-Feedback (15 files)
│       ├── thinker/                # LLM 交互层 (9 files)
│       ├── domain/                 # DDD 领域模型 (Entity, AggregateRoot traits)
│       ├── dispatcher/             # 任务编排 (22 subdirs)
│       ├── executor/               # 工具执行引擎
│       ├── providers/              # AI 提供商 (21 files)
│       ├── tools/                  # AlephTool trait
│       ├── builtin_tools/          # 内置工具 (19 files)
│       ├── memory/                 # 记忆系统 (纯 LanceDB)
│       │   └── store/             # LanceDB 存储抽象层 (MemoryStore, GraphStore, SessionStore)
│       ├── resilience/            # 任务弹性系统 (recovery, governance)
│       │   └── database/          # StateDatabase (SQLite) + CRUD 操作
│       ├── extension/              # 插件系统 (17 files)
│       ├── exec/                   # Shell 执行安全 (17 files)
│       ├── mcp/                    # MCP 协议客户端
│       ├── routing/                # Session Key 路由 (6 variants)
│       ├── runtimes/               # 运行时管理 (uv, fnm, yt-dlp)
│       ├── config/                 # 配置系统 + 热重载
│       └── lib.rs                  # 60+ public modules
├── apps/
│   ├── cli/                        # Rust CLI 客户端
│   ├── macos/                      # macOS App (Swift/SwiftUI, 45+ dirs)
│   └── desktop/                    # Cross-platform Tauri App
├── docs/                           # 文档
│   ├── ARCHITECTURE.md             # 完整架构
│   ├── DESIGN_PATTERNS.md          # 设计模式 (Context, Newtype, FromStr)
│   ├── AGENT_SYSTEM.md             # Agent 系统
│   ├── GATEWAY.md                  # Gateway 协议
│   ├── TOOL_SYSTEM.md              # 工具系统
│   ├── MEMORY_SYSTEM.md            # 记忆系统
│   ├── EXTENSION_SYSTEM.md         # 扩展系统
│   ├── SECURITY.md                 # 安全系统
│   ├── AGENT_DESIGN_PHILOSOPHY.md  # 设计思想
│   ├── DOMAIN_MODELING.md          # 领域建模
│   └── plans/                      # 设计规划文档
├── Cargo.toml                      # Workspace root
└── CLAUDE.md                       # 本文档
```

---

## ⚙️ 技术栈

| Layer | Technology |
|-------|------------|
| **Runtime** | Rust + Tokio (async/await) |
| **Gateway** | tokio-tungstenite + axum |
| **Database** | LanceDB (记忆：向量+元数据+FTS) + rusqlite (弹性状态：事件/任务/追踪) |
| **Embedding** | fastembed (bge-small-zh-v1.5, 本地) |
| **Providers** | Claude, GPT-4, Gemini, Ollama, DeepSeek, Moonshot |
| **Plugins** | Extism (WASM), Node.js IPC |
| **macOS App** | Swift + SwiftUI + AppKit |
| **Cross-platform** | Tauri + React |
| **Schema** | schemars (JSON Schema 自动生成) |

---

## 🔧 开发指南

### 构建命令

```bash
# Rust Core
cd core && cargo build && cargo test

# 启动 Server (不含 Control Plane UI)
cargo run --bin aleph-server

# 启动 Server (含 Control Plane UI)
cargo run --bin aleph-server --features control-plane

# macOS App
cd apps/macos && xcodegen generate && open Aleph.xcodeproj

# Tauri App
cd apps/desktop && pnpm install && pnpm tauri dev
```

---

## 🚀 Server 开发与发布

### Server 架构概览

Aleph Server 是一个自包含的 Rust 二进制程序，包含：
- **Gateway**: WebSocket 服务器 (JSON-RPC 2.0) - 端口 18789
- **Control Plane**: Web 管理界面 (Leptos WASM) - 端口 18790
- **Agent Loop**: AI 代理执行引擎
- **Tool System**: 工具调用和执行
- **Memory System**: 向量数据库和事实存储

### 开发环境设置

#### 1. 依赖安装

```bash
# Rust 工具链
rustup default stable
rustup target add wasm32-unknown-unknown

# WASM 工具
cargo install wasm-bindgen-cli

# 可选：Trunk (用于 UI 开发)
cargo install trunk
```

#### 2. 环境变量配置

```bash
# ~/.aleph/config.toml 或环境变量
export ANTHROPIC_API_KEY="your-api-key"
export ANTHROPIC_BASE_URL="https://api.anthropic.com"  # 可选
```

#### 3. 数据库初始化

```bash
# Server 首次启动时会自动创建数据库
# 位置：~/.aleph/
mkdir -p ~/.aleph
```

### 开发流程

#### 快速启动（开发模式）

```bash
# 1. 不含 UI（最快启动）
cargo run --bin aleph-server

# 2. 含 UI（需要先构建 UI）
cargo run --bin aleph-server --features control-plane

# 3. 后台运行
cargo run --bin aleph-server --features control-plane -- --daemon

# 4. 指定端口
cargo run --bin aleph-server -- --port 8080
```

#### 完整开发流程

```bash
# 1. 修改 Core 代码
vim core/src/gateway/...

# 2. 运行测试
cargo test

# 3. 构建并运行
cargo run --bin aleph-server

# 4. 查看日志
tail -f ~/.aleph/aleph-server.log  # 如果使用 --daemon
```

### Control Plane UI 开发流程

Control Plane UI 是嵌入在 Server 二进制中的 Web 管理界面，使用 Leptos (WASM) 构建。

#### UI 开发环境构建

```bash
# 1. 构建 WASM 库
cd core/ui/control_plane
cargo build --lib --target wasm32-unknown-unknown --release

# 2. 生成 JS 绑定
wasm-bindgen --target web \
  --out-dir dist \
  --out-name aleph-dashboard \
  /Volumes/TBU4/Workspace/Aleph/target/wasm32-unknown-unknown/release/aleph_dashboard.wasm

# 3. 编译 Tailwind CSS
npm run build:css  # 编译 styles/tailwind.css -> dist/tailwind.css

# 4. 更新 index.html（确保引用正确的文件名）
# 编辑 dist/index.html，引用：
# - /aleph-dashboard.js
# - /aleph-dashboard_bg.wasm
# - /tailwind.css

# 5. 构建 Server（会自动嵌入 dist/ 中的资源）
cd ../../..
cargo build --bin aleph-server --features control-plane
```

#### UI 快速重建

```bash
# 修改 Leptos 代码后：
cd core/ui/control_plane && \
cargo build --lib --target wasm32-unknown-unknown --release && \
wasm-bindgen --target web --out-dir dist --out-name aleph-dashboard \
  /Volumes/TBU4/Workspace/Aleph/target/wasm32-unknown-unknown/release/aleph_dashboard.wasm && \
npm run build:css && \
cd ../../.. && \
cargo build --bin aleph-server --features control-plane
```

#### 资源嵌入机制

Control Plane UI 使用 `rust-embed` 在**编译时**嵌入资源：

```rust
#[derive(RustEmbed)]
#[folder = "ui/control_plane/dist/"]
pub struct ControlPlaneAssets;
```

**关键特性**：
- ✅ 编译时嵌入：所有 HTML/CSS/JS/WASM 文件打包进二进制
- ✅ 单文件分发：只需分发 `aleph-server` 可执行文件
- ✅ 零运行时依赖：不需要额外的静态文件目录
- ✅ 自动跳过构建：如果 `dist/` 存在，`build.rs` 会跳过 UI 构建

#### WASM 初始化机制

**重要**: Control Plane 使用库目标（lib）而非二进制目标（bin）构建 WASM。初始化代码在 `lib.rs` 中：

```rust
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn main() {
    use leptos::prelude::*;
    console_error_panic_hook::set_once();
    mount_to_body(app::App);
}
```

**说明**：
- `#[wasm_bindgen(start)]` 使函数在 WASM 模块加载时自动执行
- 无需在 HTML 中手动调用初始化函数
- `main.rs` 不会被编译（因为没有二进制目标）
- 所有初始化逻辑必须在 `lib.rs` 中

#### Tailwind CSS 编译

Control Plane UI 使用 Tailwind CSS v3 进行样式管理，CSS 在构建时编译并嵌入二进制：

```bash
# 安装依赖（首次）
cd core/ui/control_plane
npm install

# 编译 CSS
npm run build:css

# 输出：dist/tailwind.css (约 40KB minified)
```

**配置文件**：
- `tailwind.config.js`: 配置内容扫描路径（Rust 源文件 + HTML）
- `styles/tailwind.css`: 源 CSS 文件（包含 @tailwind 指令）
- `dist/tailwind.css`: 编译后的 CSS（嵌入到二进制）

**关键特性**：
- ✅ 本地编译：无需 CDN，完全离线可用
- ✅ 自动扫描：从 Rust 源文件中提取 Tailwind 类名
- ✅ 生产优化：minified，仅包含使用的类
- ✅ 嵌入二进制：通过 rust-embed 打包进可执行文件

### 发布流程

#### 1. 准备发布

```bash
# 确保所有测试通过
cargo test --workspace

# 确保 UI 已构建（如果需要）
ls core/ui/control_plane/dist/
# 应包含：index.html, aleph-dashboard.js, aleph-dashboard_bg.wasm, tailwind.css
```

#### 2. 构建 Release 版本

```bash
# 不含 UI（最小二进制）
cargo build --bin aleph-server --release

# 含 UI（完整功能）
cargo build --bin aleph-server --features control-plane --release

# 查看二进制大小
ls -lh target/release/aleph-server
# 不含 UI: ~40MB
# 含 UI: ~48MB
```

#### 3. 验证构建

```bash
# 验证二进制可执行
./target/release/aleph-server --version

# 验证嵌入的资源（如果含 UI）
strings target/release/aleph-server | grep "index.html"

# 测试运行
./target/release/aleph-server --help
```

#### 4. 分发方式

**方式 1: 直接分发二进制**
```bash
# 复制到系统路径
sudo cp target/release/aleph-server /usr/local/bin/

# 或创建符号链接
sudo ln -s $(pwd)/target/release/aleph-server /usr/local/bin/aleph-server
```

**方式 2: 使用 cargo install**
```bash
# 从本地路径安装
cargo install --path core --bin aleph-server --features control-plane

# 安装后位置：~/.cargo/bin/aleph-server
```

**方式 3: 发布到 crates.io**
```bash
# 1. 更新版本号
vim core/Cargo.toml  # 修改 version

# 2. 发布
cd core
cargo publish --dry-run  # 预检查
cargo publish            # 正式发布

# 3. 用户安装
cargo install alephcore --bin aleph-server --features control-plane
```

**方式 4: 创建安装包**
```bash
# macOS: 创建 .pkg 或 .dmg
# Linux: 创建 .deb 或 .rpm
# 使用 cargo-bundle 或 cargo-deb
cargo install cargo-deb
cargo deb --package alephcore
```

#### 5. 部署配置

```bash
# 创建配置文件
mkdir -p ~/.aleph
cat > ~/.aleph/config.toml << EOF
[agent.main]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[gateway]
bind = "127.0.0.1"
port = 18789

[control_plane]
port = 18790
EOF

# 设置环境变量
export ANTHROPIC_API_KEY="your-api-key"

# 启动服务
aleph-server --daemon --log-file ~/.aleph/server.log
```

#### 6. 系统服务配置

**macOS (launchd)**
```xml
<!-- ~/Library/LaunchAgents/com.aleph.server.plist -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.aleph.server</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/aleph-server</string>
        <string>--daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
```

```bash
# 加载服务
launchctl load ~/Library/LaunchAgents/com.aleph.server.plist

# 启动服务
launchctl start com.aleph.server

# 查看状态
launchctl list | grep aleph
```

**Linux (systemd)**
```ini
# /etc/systemd/system/aleph-server.service
[Unit]
Description=Aleph AI Server
After=network.target

[Service]
Type=simple
User=aleph
ExecStart=/usr/local/bin/aleph-server
Restart=on-failure
Environment="ANTHROPIC_API_KEY=your-api-key"

[Install]
WantedBy=multi-user.target
```

```bash
# 重载配置
sudo systemctl daemon-reload

# 启动服务
sudo systemctl start aleph-server

# 开机自启
sudo systemctl enable aleph-server

# 查看状态
sudo systemctl status aleph-server
```

### 故障排查

#### Control Plane UI 问题

**问题：Trunk 构建失败**
```bash
# 解决方案：使用 wasm-bindgen 手动构建（见上文）
# Trunk 在工作区环境中可能遇到目标识别问题
```

**问题：路由显示 404**
```bash
# 原因：WASM 中的路由基础路径配置错误
# 解决方案：确保 Leptos Router 使用根路径 "/"
# 检查 index.html 中的资源路径是否为绝对路径
```

**问题：Server 构建时 UI 构建失败**
```bash
# build.rs 已配置为优雅降级：
# - 如果 dist/ 存在 → 跳过构建
# - 如果 Trunk 失败 → 警告但不中断 Server 构建
# Server 可以独立运行，UI 为可选功能
```

#### Server 运行问题

**问题：端口被占用**
```bash
# 查找占用进程
lsof -i :18789
lsof -i :18790

# 杀死进程
kill -9 <PID>

# 或使用不同端口
aleph-server --port 8080
```

**问题：API 密钥未配置**
```bash
# 检查环境变量
echo $ANTHROPIC_API_KEY

# 或检查配置文件
cat ~/.aleph/config.toml

# 设置环境变量
export ANTHROPIC_API_KEY="your-api-key"
```

**问题：数据库损坏**
```bash
# 备份并重建
mv ~/.aleph/sessions.db ~/.aleph/sessions.db.bak
mv ~/.aleph/memory.db ~/.aleph/memory.db.bak

# 重启 Server（会自动创建新数据库）
aleph-server
```

### 性能优化

#### 编译优化

```toml
# Cargo.toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"
```

#### 运行时优化

```bash
# 增加 Tokio 线程数
TOKIO_WORKER_THREADS=8 aleph-server

# 调整日志级别
RUST_LOG=info aleph-server  # 生产环境
RUST_LOG=debug aleph-server # 调试模式
```

---

### Feature Flags

```toml
[features]
default = ["gateway"]
gateway = ["tokio-tungstenite", "axum"]
telegram = ["teloxide", "gateway"]
discord = ["serenity", "gateway"]
cron = ["cron", "gateway"]
browser = ["chromiumoxide", "gateway"]
cli = ["inquire"]
plugin-wasm = ["extism"]
```

### Environment

- Python path: ~/.uv/python3/bin/python
- Install Python package: cd ~/.uv/python3 && uv pip install <package>
- Xcode generation: cd apps/macos && xcodegen generate
- Syntax validation: ~/.uv/python3/bin/python Scripts/verify_swift_syntax.py <file.swift>
- Xcode build cache cleanup: rm -rf ~/Library/Developer/Xcode/DerivedData/(Aleph)-*
- This project uses XcodeGen to manage the Xcode project. See docs/XCODEGEN_README.md for detailed workflow instructions.

### 分支策略

**单分支开发模式**：所有开发工作直接在 main 分支进行。

### 提交规范

English commit messages. Format: `<scope>: <description>`

Example: `gateway: add WebSocket server foundation`

### 语言规范

- Reply in Chinese
- Code comments in English
- Documentation in both

---

## 📚 文档索引

### 架构文档

| 文档 | 描述 |
|------|------|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | 完整系统架构、模块依赖、数据流 |
| [DESIGN_PATTERNS.md](docs/DESIGN_PATTERNS.md) | 设计模式：Context、Newtype、FromStr、Builder |
| [CODE_ORGANIZATION.md](docs/CODE_ORGANIZATION.md) | 文件组织规范：拆分原则、命名约定、反面示例、重构 Backlog |
| [AGENT_SYSTEM.md](docs/AGENT_SYSTEM.md) | Agent Loop、Thinker、Dispatcher |
| [GATEWAY.md](docs/GATEWAY.md) | WebSocket 协议、RPC 方法、Channels |
| [TOOL_SYSTEM.md](docs/TOOL_SYSTEM.md) | AlephTool trait、内置工具、开发指南 |
| [MEMORY_SYSTEM.md](docs/MEMORY_SYSTEM.md) | Facts DB、混合检索、压缩策略 |
| [EXTENSION_SYSTEM.md](docs/EXTENSION_SYSTEM.md) | WASM/Node.js 插件、manifest 格式 |
| [SECURITY.md](docs/SECURITY.md) | Exec 审批、权限规则、allowlist |
| [DOMAIN_MODELING.md](docs/DOMAIN_MODELING.md) | DDD 领域建模、Entity/AggregateRoot traits |

### 设计文档

| 文档 | 描述 |
|------|------|
| [AGENT_DESIGN_PHILOSOPHY.md](docs/AGENT_DESIGN_PHILOSOPHY.md) | 四大设计思想：第一性原理、启发式、自学习、POE |
| [POE Architecture](docs/plans/2026-02-01-poe-architecture-design.md) | POE 架构详细设计 |
| [Server-Centric Architecture](docs/plans/2026-02-23-server-centric-architecture-design.md) | Server-centric 架构设计 |

---


## 📝 Session Context

### Key Context

- **项目定位**: 自托管个人 AI 助手，Gateway 控制面架构
- **核心循环**: Observe → Think → Act → Feedback → Compress
- **技术栈**: Rust (Gateway + Agent) + Swift (macOS) + React (Tauri)
- **当前状态**: Phase 8 (Multi-Channel)，Gateway 完整实现，Server-centric 架构

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.

## 📝 语言
使用中文对话
