# Aleph: exec.* / wizard.* / 文档系统设计

> 日期: 2026-01-31
> 状态: 待实施
> 优先级: exec.* > wizard.* > 文档系统

---

## 概述

本设计覆盖 Aleph 与 OpenClaw 对齐的最后两项工作：

| 项目 | 范围 | 复杂度 |
|------|------|--------|
| **exec.* 执行审批系统** | 6 RPC 方法 + Manager + 存储 + 命令解析 + Chat 转发 + macOS IPC | 高 |
| **wizard.* 配置向导** | 4 RPC 方法 + Session 状态机 + 10 阶段 + 多端支持 | 高 |
| **文档系统** | mdBook + mdbook-fuse + 参考 OpenClaw 结构 | 中 |

---

## 依赖关系

```
exec.* ──────────────────────────────────────────┐
   │                                              │
   ├── ExecApprovalManager (内存状态)              │
   ├── exec-approvals.json (持久化)               │
   ├── CommandParser (命令解析)                   │
   ├── AllowlistMatcher (glob 匹配)              │
   ├── ChatForwarder (审批转发)  ←── channels/*   │
   └── macOS IPC (Unix socket + HMAC)            │
                                                  │
wizard.* ─────────────────────────────────────────┤
   │                                              │
   ├── WizardSession (状态机)                     │
   ├── WizardPrompter (抽象接口)                  │
   │    ├── CliPrompter (dialoguer)              │
   │    └── RpcPrompter (Gateway RPC)            │
   ├── OnboardingFlow (10 阶段)                   │
   └── ConfigWriter (配置输出)                    │
                                                  │
docs/ ────────────────────────────────────────────┘
   │
   ├── mdBook (Rust 原生)
   ├── mdbook-fuse (搜索插件)
   └── GitHub Pages (托管)
```

---

## Part 1: exec.* 执行审批系统

### 1.1 核心数据结构

```rust
// core/src/exec_approvals/types.rs

/// 审批决策
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecApprovalDecision {
    AllowOnce,      // 本次允许
    AllowAlways,    // 永久允许（加入 allowlist）
    Deny,           // 拒绝
}

/// 安全级别
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecSecurity {
    Deny,           // 拒绝所有
    #[default]
    Allowlist,      // 仅允许白名单
    Full,           // 允许所有
}

/// 询问模式
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecAsk {
    Off,            // 不询问
    #[default]
    OnMiss,         // 白名单未匹配时询问
    Always,         // 总是询问
}

/// 审批请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub host: Option<String>,
    pub agent_id: Option<String>,
    pub session_key: Option<String>,
    pub resolved_path: Option<String>,
}

/// 审批记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRecord {
    pub id: String,
    pub request: ExecApprovalRequest,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub resolved_at_ms: Option<u64>,
    pub decision: Option<ExecApprovalDecision>,
    pub resolved_by: Option<String>,
}
```

### 1.2 文件存储格式

```json5
// ~/.aleph/exec-approvals.json
{
  "version": 1,
  "socket": {
    "path": "~/.aleph/exec-approvals.sock",
    "token": "base64url-encoded-token"
  },
  "defaults": {
    "security": "allowlist",
    "ask": "on-miss",
    "askFallback": "deny",
    "autoAllowSkills": true
  },
  "agents": {
    "main": {
      "security": "allowlist",
      "allowlist": [
        { "id": "uuid", "pattern": "/usr/bin/git", "lastUsedAt": 1706745600000 },
        { "id": "uuid", "pattern": "/opt/homebrew/bin/*" }
      ]
    }
  }
}
```

### 1.3 ExecApprovalManager

```rust
// core/src/exec_approvals/manager.rs

use tokio::sync::oneshot;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;

struct PendingEntry {
    record: ExecApprovalRecord,
    sender: oneshot::Sender<Option<ExecApprovalDecision>>,
    timeout_handle: tokio::task::JoinHandle<()>,
}

pub struct ExecApprovalManager {
    pending: Arc<RwLock<HashMap<String, PendingEntry>>>,
    event_bus: Arc<EventBus>,
    config: Arc<RwLock<ExecApprovalsFile>>,
    config_path: PathBuf,
    config_hash: Arc<RwLock<String>>,  // SHA256 for optimistic locking
}

impl ExecApprovalManager {
    /// 创建审批请求，返回记录（不阻塞）
    pub fn create(&self, request: ExecApprovalRequest, timeout_ms: u64) -> ExecApprovalRecord;

    /// 等待审批决策（阻塞直到决策或超时）
    pub async fn wait_for_decision(
        &self,
        record: ExecApprovalRecord
    ) -> Option<ExecApprovalDecision>;

    /// 解决审批（由 UI 调用）
    pub fn resolve(
        &self,
        record_id: &str,
        decision: ExecApprovalDecision,
        resolved_by: Option<String>
    ) -> bool;

    /// 获取待处理审批列表
    pub fn list_pending(&self) -> Vec<ExecApprovalRecord>;

    /// 加载配置（带 hash）
    pub fn get_config(&self) -> (ExecApprovalsFile, String);

    /// 保存配置（乐观锁）
    pub fn set_config(&self, file: ExecApprovalsFile, base_hash: &str) -> Result<String>;
}
```

### 1.4 RPC 方法

| 方法 | 说明 | 权限 |
|------|------|------|
| `exec.approval.request` | 请求审批，阻塞等待决策 | operator.approvals |
| `exec.approval.resolve` | 解决审批（allow/deny） | operator.approvals |
| `exec.approvals.get` | 获取配置 + hash | operator.admin |
| `exec.approvals.set` | 更新配置（乐观锁） | operator.admin |
| `exec.approvals.node.get` | 获取 Node 配置 | operator.admin |
| `exec.approvals.node.set` | 设置 Node 配置 | operator.admin |

### 1.5 事件广播

```rust
// 审批请求时广播
event_bus.publish("exec.approval.requested", &record);

// 审批解决时广播
event_bus.publish("exec.approval.resolved", &ResolvedPayload {
    id: record_id,
    decision,
    resolved_by,
});
```

### 1.6 命令解析器

```rust
// core/src/exec_approvals/parser.rs

/// Quote-aware 字符迭代器（尊重引号和转义）
pub struct QuoteAwareIterator<'a> { /* ... */ }

/// 命令链解析结果
pub struct ParsedCommand {
    pub executable: String,
    pub args: Vec<String>,
    pub resolved_path: Option<PathBuf>,
}

impl CommandParser {
    /// 按链操作符分割 (&&, ||, ;)
    pub fn split_chain(command: &str) -> Result<Vec<&str>>;

    /// 按管道分割，拒绝危险 token
    /// 危险 token: >, <, `, $(), \n, (, ), |&, &
    pub fn split_pipeline(command: &str) -> Result<Vec<&str>>;

    /// 解析单个命令
    pub fn parse_single(command: &str) -> Result<ParsedCommand>;

    /// 解析可执行文件的完整路径
    pub fn resolve_executable(name: &str, cwd: Option<&Path>) -> Option<PathBuf>;
}

/// 安全的内置命令（无需 allowlist）
pub const SAFE_BINS: &[&str] = &[
    "jq", "grep", "cut", "sort", "uniq",
    "head", "tail", "tr", "wc", "cat", "echo"
];

/// 检查是否为安全命令（无路径参数）
pub fn is_safe_bin_invocation(parsed: &ParsedCommand) -> bool;
```

### 1.7 Allowlist 匹配器

```rust
// core/src/exec_approvals/allowlist.rs

use globset::{Glob, GlobMatcher};

pub struct AllowlistEntry {
    pub id: String,
    pub pattern: String,
    pub matcher: GlobMatcher,
    pub last_used_at: Option<u64>,
    pub last_used_command: Option<String>,
}

impl AllowlistMatcher {
    /// 从配置加载 allowlist
    pub fn from_config(entries: &[ExecAllowlistEntry]) -> Self;

    /// 匹配解析后的路径
    pub fn matches(&self, resolved_path: &Path) -> Option<&AllowlistEntry>;

    /// 添加新条目（allow-always 时调用）
    pub fn add_entry(&mut self, pattern: String) -> AllowlistEntry;

    /// 更新使用时间戳
    pub fn touch(&mut self, entry_id: &str, command: &str);
}
```

### 1.8 审批决策流程

```
命令输入
    ↓
CommandParser.parse_single()
    ↓
resolve_executable() → 完整路径
    ↓
is_safe_bin_invocation()? ──yes──→ 允许（无需审批）
    ↓ no
AllowlistMatcher.matches()? ──yes──→ 允许（白名单命中）
    ↓ no
ExecAsk == Off? ──yes──→ 按 security 策略决定
    ↓ no
ExecApprovalManager.request() ──→ 等待用户决策
    ↓
超时? ──yes──→ 按 askFallback 策略决定
    ↓ no
返回用户决策
```

### 1.9 Chat 审批转发

```rust
// core/src/exec_approvals/forwarder.rs

/// 转发模式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Session,    // 转发到命令来源的 session
    Targets,    // 转发到配置的目标
    Both,       // 两者都转发
}

pub struct ExecApprovalForwarder {
    event_bus: Arc<EventBus>,
    channel_manager: Arc<ChannelManager>,
    config: ForwarderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwarderConfig {
    pub mode: ForwardMode,
    pub targets: Vec<ForwardTarget>,        // channel:id 格式
    pub agent_filter: Option<Vec<String>>,  // 仅特定 agent
    pub session_filter: Option<String>,     // session key 正则
}

impl ExecApprovalForwarder {
    /// 订阅审批事件并转发
    pub async fn start(&self);

    /// 格式化审批消息
    fn format_approval_message(&self, record: &ExecApprovalRecord) -> OutboundMessage;

    /// 处理用户回复
    pub async fn handle_reply(&self, channel: &str, user: &str, text: &str);
}
```

### 1.10 macOS IPC

```rust
// core/src/exec_approvals/ipc.rs

use tokio::net::UnixListener;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// IPC 服务端（Gateway 侧）
pub struct ExecApprovalIpcServer {
    socket_path: PathBuf,
    token: Vec<u8>,
    manager: Arc<ExecApprovalManager>,
}

impl ExecApprovalIpcServer {
    /// 启动 Unix socket 监听
    pub async fn start(&self) -> Result<()>;

    /// 验证 HMAC 挑战
    fn verify_challenge(&self, nonce: &[u8], response: &[u8]) -> bool;
}

/// IPC 客户端（macOS App 侧）
pub struct ExecApprovalIpcClient {
    socket_path: PathBuf,
    token: Vec<u8>,
}

impl ExecApprovalIpcClient {
    /// 请求审批（macOS App → Gateway）
    pub async fn request_approval(&self, request: ExecApprovalRequest)
        -> Result<ExecApprovalDecision>;

    /// 上报审批决策（UI → Gateway）
    pub async fn resolve_approval(&self, id: &str, decision: ExecApprovalDecision)
        -> Result<()>;
}
```

### 1.11 IPC 协议

```
┌─────────────────────────────────────────────────────────────┐
│                    IPC 握手流程                              │
├─────────────────────────────────────────────────────────────┤
│  Client                              Server                 │
│     │                                   │                   │
│     │──── connect to socket ───────────→│                   │
│     │                                   │                   │
│     │←──── challenge: nonce (32 bytes) ─│                   │
│     │                                   │                   │
│     │──── response: HMAC(token, nonce) →│                   │
│     │                                   │                   │
│     │←──── auth: ok / denied ───────────│                   │
│     │                                   │                   │
│     │──── request: JSON-RPC ───────────→│                   │
│     │←──── response: JSON-RPC ──────────│                   │
└─────────────────────────────────────────────────────────────┘
```

---

## Part 2: wizard.* 配置向导

### 2.1 核心数据结构

```rust
// core/src/wizard/types.rs

/// 向导会话状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WizardStatus {
    Running,
    Done,
    Cancelled,
    Error,
}

/// 步骤类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepType {
    Note,           // 纯展示
    Select,         // 单选
    MultiSelect,    // 多选
    Text,           // 文本输入
    Confirm,        // 是/否确认
    Progress,       // 进度展示
    Action,         // 后台操作
}

/// 向导步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardStep {
    pub id: String,
    #[serde(rename = "type")]
    pub step_type: StepType,
    pub title: Option<String>,
    pub message: Option<String>,
    pub options: Option<Vec<WizardOption>>,
    pub initial_value: Option<serde_json::Value>,
    pub placeholder: Option<String>,
    pub sensitive: bool,
    pub executor: StepExecutor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepExecutor {
    Gateway,    // 服务端执行
    Client,     // 客户端渲染
}

/// 向导选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardOption {
    pub value: serde_json::Value,
    pub label: String,
    pub hint: Option<String>,
}

/// next() 返回结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardNextResult {
    pub done: bool,
    pub step: Option<WizardStep>,
    pub status: WizardStatus,
    pub error: Option<String>,
}
```

### 2.2 WizardSession 状态机

```rust
// core/src/wizard/session.rs

use tokio::sync::{oneshot, mpsc};

pub struct WizardSession {
    id: String,
    status: Arc<RwLock<WizardStatus>>,
    current_step: Arc<RwLock<Option<WizardStep>>>,
    step_tx: mpsc::Sender<WizardStep>,
    step_rx: Arc<Mutex<mpsc::Receiver<WizardStep>>>,
    answers: Arc<RwLock<HashMap<String, oneshot::Sender<serde_json::Value>>>>,
    error: Arc<RwLock<Option<String>>>,
}

impl WizardSession {
    /// 创建新会话，启动后台 runner
    pub fn new(flow: Box<dyn WizardFlow>) -> Self;

    /// 获取下一步（阻塞直到有步骤）
    pub async fn next(&self) -> WizardNextResult;

    /// 回答当前步骤
    pub async fn answer(&self, step_id: &str, value: serde_json::Value) -> Result<()>;

    /// 取消向导
    pub fn cancel(&self);

    /// 获取状态
    pub fn status(&self) -> WizardStatus;
}
```

### 2.3 Prompter 抽象接口

```rust
// core/src/wizard/prompter.rs

#[async_trait]
pub trait WizardPrompter: Send + Sync {
    async fn intro(&self, title: &str);
    async fn outro(&self, message: &str);
    async fn note(&self, message: &str, title: Option<&str>);
    async fn select<T: DeserializeOwned>(&self, params: SelectParams) -> Result<T>;
    async fn multi_select<T: DeserializeOwned>(&self, params: MultiSelectParams) -> Result<Vec<T>>;
    async fn text(&self, params: TextParams) -> Result<String>;
    async fn confirm(&self, params: ConfirmParams) -> Result<bool>;
    fn progress(&self, label: &str) -> Box<dyn ProgressHandle>;
}

/// CLI 实现（使用 dialoguer）
pub struct CliPrompter;

/// RPC 实现（通过 WizardSession 与客户端交互）
pub struct RpcPrompter {
    session: Arc<WizardSession>,
}
```

### 2.4 RPC 方法

```rust
// core/src/gateway/handlers/wizard.rs

/// wizard.start - 开始向导
async fn handle_start(req: JsonRpcRequest, ctx: &GatewayContext) -> JsonRpcResponse;

/// wizard.next - 前进到下一步
async fn handle_next(req: JsonRpcRequest, ctx: &GatewayContext) -> JsonRpcResponse;

/// wizard.cancel - 取消向导
async fn handle_cancel(req: JsonRpcRequest, ctx: &GatewayContext) -> JsonRpcResponse;

/// wizard.status - 查询状态
async fn handle_status(req: JsonRpcRequest, ctx: &GatewayContext) -> JsonRpcResponse;
```

### 2.5 客户端交互流程

```
macOS App / Control UI                    Gateway
       │                                     │
       │─── wizard.start ───────────────────→│
       │                                     │ 创建 WizardSession
       │←── { session_id, step: intro } ─────│
       │                                     │
       │    [渲染 intro 界面]                 │
       │                                     │
       │─── wizard.next { answer: null } ───→│
       │←── { step: select(mode) } ──────────│
       │                                     │
       │    [用户选择 "local"]               │
       │                                     │
       │─── wizard.next { answer: "local" } →│
       │←── { step: text(api_key) } ─────────│
       │                                     │
       │    [用户输入 API key]               │
       │                                     │
       │─── wizard.next { answer: "sk-..." }→│
       │←── { step: progress("配置中...") } ─│
       │                                     │ [Gateway 执行配置]
       │←── { done: true, status: done } ────│
       │                                     │
```

### 2.6 OnboardingFlow 10 阶段

```rust
// core/src/wizard/onboarding.rs

pub struct OnboardingFlow {
    config: Arc<RwLock<AlephConfig>>,
    collected: OnboardingData,
}

#[async_trait]
impl WizardFlow for OnboardingFlow {
    async fn run(&self, prompter: &dyn WizardPrompter) -> Result<()> {
        self.phase1_safety(prompter).await?;
        self.phase2_flow_choice(prompter).await?;
        self.phase3_mode(prompter).await?;
        self.phase4_auth(prompter).await?;
        self.phase5_workspace(prompter).await?;
        self.phase6_gateway(prompter).await?;
        self.phase7_channels(prompter).await?;
        self.phase8_daemon(prompter).await?;
        self.phase9_health(prompter).await?;
        self.phase10_skills(prompter).await?;
        self.finalize(prompter).await?;
        Ok(())
    }
}
```

| 阶段 | 方法 | 步骤类型 | 说明 |
|------|------|----------|------|
| **1. 安全确认** | `phase1_safety` | confirm | 风险提示 + 检测现有配置 |
| **2. 流程选择** | `phase2_flow_choice` | select | QuickStart（推荐）/ Advanced |
| **3. 模式选择** | `phase3_mode` | select | Local（本机）/ Remote（远程 Gateway）|
| **4. Auth 配置** | `phase4_auth` | select + text | 多提供商选择 + API key 输入 |
| **5. Workspace** | `phase5_workspace` | text | 工作目录，默认 `~/.aleph/workspace` |
| **6. Gateway** | `phase6_gateway` | multi | 端口/绑定/认证模式/Tailscale |
| **7. Channels** | `phase7_channels` | multi | Telegram/Discord/iMessage 等 |
| **8. Daemon** | `phase8_daemon` | select + action | LaunchAgent / systemd 安装 |
| **9. Health** | `phase9_health` | progress | 健康检查 + 状态展示 |
| **10. Skills** | `phase10_skills` | multiselect | 技能安装选择 |

---

## Part 3: 文档系统

### 3.1 目录结构

```
docs/
├── book.toml                    # mdBook 配置
├── src/
│   ├── SUMMARY.md               # 目录结构（必需）
│   ├── README.md                # 首页
│   ├── start/                   # 快速开始
│   │   ├── index.md
│   │   ├── installation.md
│   │   ├── quickstart.md
│   │   └── wizard.md
│   ├── concepts/                # 核心概念
│   │   ├── architecture.md
│   │   ├── agent.md
│   │   ├── session.md
│   │   ├── channels.md
│   │   └── tools.md
│   ├── gateway/                 # Gateway 文档
│   │   ├── protocol.md
│   │   ├── configuration.md
│   │   ├── security.md
│   │   └── rpc-reference.md
│   ├── cli/                     # CLI 命令参考
│   │   ├── index.md
│   │   ├── agent.md
│   │   ├── config.md
│   │   ├── channels.md
│   │   └── cron.md
│   ├── channels/                # 渠道接入
│   │   ├── telegram.md
│   │   ├── discord.md
│   │   ├── imessage.md
│   │   └── slack.md
│   ├── platforms/               # 平台指南
│   │   ├── macos.md
│   │   ├── linux.md
│   │   └── docker.md
│   └── reference/               # API 参考
│       ├── rpc-methods.md
│       └── config-schema.md
├── theme/                       # 自定义主题（可选）
│   ├── index.hbs
│   ├── head.hbs
│   └── css/
│       └── custom.css
└── fuse-index/                  # mdbook-fuse 生成的索引
```

### 3.2 book.toml 配置

```toml
[book]
title = "Aleph Documentation"
authors = ["Aleph Team"]
language = "zh"
multilingual = false
src = "src"

[build]
build-dir = "book"
create-missing = true

[output.html]
theme = "theme"
default-theme = "coal"
preferred-dark-theme = "coal"
git-repository-url = "https://github.com/user/aleph"
edit-url-template = "https://github.com/user/aleph/edit/main/docs/{path}"

[output.html.search]
enable = false  # 禁用内置搜索，使用 fuse

[output.html.playground]
editable = false
runnable = false

# mdbook-fuse 插件配置
[preprocessor.fuse]
command = "mdbook-fuse"
renderer = ["html"]

[output.fuse]
keys = ["title", "body", "breadcrumbs"]
threshold = 0.3
include-matches = true
```

### 3.3 GitHub Actions 自动部署

```yaml
# .github/workflows/docs.yml
name: Deploy Docs

on:
  push:
    branches: [main]
    paths: ['docs/**']

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install mdBook
        run: |
          cargo install mdbook mdbook-fuse

      - name: Build docs
        run: mdbook build docs

      - name: Deploy to GitHub Pages
        uses: peaceiris/actions-gh-pages@v3
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          publish_dir: ./docs/book
```

---

## 实现计划

### 文件清单

**exec.* 执行审批系统 (15 文件)**

| 文件路径 | 说明 |
|----------|------|
| `core/src/exec_approvals/mod.rs` | 模块入口 |
| `core/src/exec_approvals/types.rs` | 核心类型定义 |
| `core/src/exec_approvals/manager.rs` | ExecApprovalManager |
| `core/src/exec_approvals/storage.rs` | 文件持久化 + 乐观锁 |
| `core/src/exec_approvals/parser.rs` | 命令解析器 |
| `core/src/exec_approvals/allowlist.rs` | Allowlist 匹配 |
| `core/src/exec_approvals/safe_bins.rs` | 安全命令白名单 |
| `core/src/exec_approvals/forwarder.rs` | Chat 转发 |
| `core/src/exec_approvals/ipc.rs` | macOS IPC 服务端 |
| `core/src/exec_approvals/ipc_client.rs` | macOS IPC 客户端 |
| `core/src/gateway/handlers/exec_approvals.rs` | RPC 处理器 |
| `cli/src/commands/approvals.rs` | CLI 命令 |
| `platforms/macos/Aleph/ExecApprovals/` | Swift UI 集成 |

**wizard.* 配置向导 (12 文件)**

| 文件路径 | 说明 |
|----------|------|
| `core/src/wizard/mod.rs` | 模块入口 |
| `core/src/wizard/types.rs` | 核心类型 |
| `core/src/wizard/session.rs` | WizardSession 状态机 |
| `core/src/wizard/prompter.rs` | Prompter trait |
| `core/src/wizard/cli_prompter.rs` | CLI 实现 (dialoguer) |
| `core/src/wizard/rpc_prompter.rs` | RPC 实现 |
| `core/src/wizard/onboarding.rs` | 10 阶段主流程 |
| `core/src/wizard/phases/` | 各阶段实现 (10 文件) |
| `core/src/gateway/handlers/wizard.rs` | RPC 处理器 |
| `cli/src/commands/onboard.rs` | CLI 入口 |
| `platforms/macos/Aleph/Wizard/` | Swift UI 集成 |

**文档系统 (5 文件 + 内容)**

| 文件路径 | 说明 |
|----------|------|
| `docs/book.toml` | mdBook 配置 |
| `docs/src/SUMMARY.md` | 目录结构 |
| `docs/src/**/*.md` | 文档内容 (~30 页) |
| `docs/theme/` | 自定义主题 |
| `.github/workflows/docs.yml` | 自动部署 |

### 实现顺序

```
Phase 1: exec.* 基础 (3 天)
├── types.rs + storage.rs
├── manager.rs (内存状态)
├── handlers/exec_approvals.rs (4 个 RPC)
└── 单元测试

Phase 2: exec.* 完整 (3 天)
├── parser.rs + allowlist.rs
├── safe_bins.rs
├── forwarder.rs (Chat 转发)
└── 集成测试

Phase 3: exec.* IPC (2 天)
├── ipc.rs + ipc_client.rs
├── macOS App 集成
└── 端到端测试

Phase 4: wizard.* 基础 (3 天)
├── types.rs + session.rs
├── prompter.rs + cli_prompter.rs
├── handlers/wizard.rs (4 个 RPC)
└── 单元测试

Phase 5: wizard.* 完整 (4 天)
├── onboarding.rs (10 阶段)
├── rpc_prompter.rs
├── macOS App 集成
└── 端到端测试

Phase 6: 文档系统 (2 天)
├── book.toml + SUMMARY.md
├── 核心文档 (~30 页)
├── GitHub Actions
└── 部署验证
```

---

## 验收标准

### exec.* 系统

- [ ] 6 个 RPC 方法全部实现并通过测试
- [ ] 命令解析正确处理引号、转义、管道
- [ ] Allowlist glob 匹配正确
- [ ] Chat 转发到 Telegram/Discord 正常工作
- [ ] macOS IPC 握手和审批流程正常
- [ ] CLI `aleph approvals` 命令可用

### wizard.* 系统

- [ ] 4 个 RPC 方法全部实现并通过测试
- [ ] CLI 向导 10 阶段流程完整
- [ ] macOS App 向导 UI 正常工作
- [ ] 配置正确写入 `~/.aleph/config.json`
- [ ] Daemon 安装正常（macOS LaunchAgent / Linux systemd）

### 文档系统

- [ ] mdBook 本地构建成功
- [ ] mdbook-fuse 搜索正常工作
- [ ] GitHub Actions 自动部署到 GitHub Pages
- [ ] 核心文档 30 页完成
