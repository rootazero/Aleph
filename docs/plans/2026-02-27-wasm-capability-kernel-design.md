# WASM Capability Kernel Design

> *"学习，更要超越"* — 借鉴 IronClaw 的 WASM 沙箱精华，与 Aleph 安全体系深度融合。

**日期**: 2026-02-27
**状态**: Approved
**范围**: WASM 插件能力模型增强
**参考**: IronClaw (~/Workspace/ironclaw) WASM sandbox implementation

---

## 1. 背景与动机

### 问题

Aleph 的 WASM 插件系统（基于 Extism）存在关键安全缺口：

1. **PermissionChecker 是死代码** — `permissions.rs` 中声明了权限检查，但 `call_tool()` 路径从未调用它
2. **无 host function 接口** — WASM 插件无法安全地调用宿主能力（HTTP、文件读取、工具调用）
3. **无能力边界** — 插件要么完全隔离（当前），要么需要完全信任
4. **无凭证保护** — 若未来开放网络能力，插件将直接接触密钥原文
5. **无泄漏检测** — 只有事后 `SecretMasker` 遮蔽，无事前拦截

### 参考：IronClaw 的做法

IronClaw (OpenClaw 的 Rust 重写) 使用 Wasmtime + WASI P2 (Component Model) + WIT 契约实现了精细的 WASM 沙箱：

- **Default-deny 能力模型**: http / workspace_read / tool_invoke / secrets
- **Host 侧凭证注入**: WASM 永远看不到密钥原文
- **BLAKE3 二进制完整性校验**
- **Aho-Corasick 双向泄漏检测**: 14 种模式，出入双向扫描
- **资源硬限制**: 10MB memory / 10M fuel / 60s timeout / epoch tick
- **工具别名**: WASM 插件不知道真实工具名

### Aleph 的超越点

| 维度 | IronClaw | Aleph (本设计) |
|------|----------|---------------|
| **信任升级** | 静态 TrustLevel (System/Verified/User) | 动态 TrustStage (Draft → Trial → Verified)，用表现换信任 |
| **审批集成** | 无 | Draft 阶段高危操作触发 GlobalBus 异步审批 |
| **审计** | 无 | 所有能力调用入 SandboxAuditLog |
| **泄漏检测** | 独立 LeakDetector | 复用+扩展 SecretMasker (已有 14+ 模式) |
| **配置格式** | JSON sidecar | 融入 aleph.plugin.toml (TOML, 统一配置) |
| **运行时** | 自建 Wasmtime + WIT | 保留 Extism (底层也是 Wasmtime) + 能力增强层 |

---

## 2. 架构总览

### 核心理念

**"WASM 插件是受限公民"** — 它们在 Aleph 大脑内部运行，但只能通过 6 个受控的 host function 与外界交互。每个 host function 调用都经过 `WasmCapabilityKernel` 的检查、审计和（必要时的）审批。

### 架构层次

```
┌─────────────────────────────────────────────────────┐
│                   WASM Plugin (.wasm)                │
│          (untrusted guest, Extism PDK)              │
└───────────────┬─────────────────────────────────────┘
                │ Extism Host Function Calls
                ▼
┌─────────────────────────────────────────────────────┐
│             WasmCapabilityKernel                     │
│  ┌──────────┬───────────┬───────────┬────────────┐  │
│  │CapChecker│CredInjector│LeakDetector│AuditLogger│  │
│  │(per-call │(http only) │(in+out    │(all calls) │  │
│  │ gate)    │            │ scanning) │            │  │
│  └──────────┴───────────┴───────────┴────────────┘  │
│           ↕ TrustStage    ↕ GlobalBus               │
└───────────────┬─────────────────────────────────────┘
                │ Dispatches to Aleph subsystems
                ▼
┌─────────────────────────────────────────────────────┐
│              Aleph Core Services                     │
│  reqwest (HTTP) │ AlephToolServer │ SecretStore      │
│  MemoryStore    │ exec/masker     │ config           │
└─────────────────────────────────────────────────────┘
```

### 数据流 — http_request 完整路径

```
Plugin calls http_request("GET", "https://api.slack.com/users", ...)
  → WasmCapabilityKernel.check_and_execute_http()
    → CapChecker: 插件声明了 http 能力? 域名在 allowlist 中?
    → TrustStage: Draft 阶段首次访问新域名? → 触发 GlobalBus 审批
    → LeakDetector: 扫描 URL + headers + body, 无密钥泄漏?
    → CredInjector: 匹配域名, 注入 Bearer token 到 header
    → reqwest::Client 发出实际请求
    → LeakDetector: 扫描 response body
    → AuditLogger: 记录调用详情
  → 返回 sanitized response 给 Plugin
```

### 设计原则

| 原则 | 体现 |
|------|------|
| **R1 大脑与四肢分离** | WasmCapabilityKernel 在 core 内, 不涉及平台 API |
| **R3 核心轻量化** | Kernel 是路由+检查, 实际能力委托给已有子系统 |
| **Default-deny** | 无能力声明 = 零权限 |
| **渐进信任** | TrustStage: Draft → Trial → Verified |
| **可审计** | 所有能力调用进 SandboxAuditLog |

---

## 3. 能力模型

### 六项 Host Function

| Host Function | 能力键 | 控制维度 | Aleph 复用 |
|--------------|--------|---------|-----------|
| `log(level, msg)` | 无需声明 (默认允许) | 条数上限 1000, 单条 4KB | — |
| `now_millis()` | 无需声明 (默认允许) | 只读时钟 | — |
| `workspace_read(path)` | `capabilities.workspace` | 路径前缀白名单 + 遍历防护 | — |
| `http_request(...)` | `capabilities.http` | 域名+路径+方法白名单, 凭证注入, 速率限制 | reqwest, SecretMasker |
| `tool_invoke(alias, params)` | `capabilities.tool_invoke` | 别名映射 (隐藏真实工具名), 次数上限 | AlephToolServer |
| `secret_exists(name)` | `capabilities.secrets` | glob 模式匹配, 只返回 bool | config/SecretStore |

### 能力声明格式 (aleph.plugin.toml)

```toml
[plugin]
id = "slack-notifier"
name = "Slack Notifier"
version = "0.1.0"
kind = "wasm"

[plugin.capabilities]
# Workspace read access (optional path prefixes)
workspace = { allowed_prefixes = ["context/", "daily/"] }

# HTTP access control
[plugin.capabilities.http]
timeout_secs = 30
max_request_bytes = 1_048_576    # 1MB
max_response_bytes = 10_485_760  # 10MB

[[plugin.capabilities.http.allowlist]]
host = "slack.com"
path_prefix = "/api/"
methods = ["GET", "POST"]

[[plugin.capabilities.http.allowlist]]
host = "*.slack.com"
path_prefix = "/api/"
methods = ["GET"]

# Credential injection (plugin never sees raw secrets)
[[plugin.capabilities.http.credentials]]
secret_name = "slack_bot_token"
inject = { type = "bearer" }
host_patterns = ["slack.com", "*.slack.com"]

# HTTP rate limiting
[plugin.capabilities.http.rate_limit]
requests_per_minute = 60
requests_per_hour = 500

# Tool invocation via aliases
[plugin.capabilities.tool_invoke]
max_per_execution = 10

[plugin.capabilities.tool_invoke.aliases]
search = "brave_search"
memory = "memory_store"

# Secret existence checking
[plugin.capabilities.secrets]
allowed_patterns = ["slack_*"]
```

### 资源限制

| 资源 | 默认值 | 可配置位置 |
|------|--------|-----------|
| Memory | 10 MB | `[plugin.limits.memory_mb]` |
| Fuel (CPU) | 10M units | `[plugin.limits.fuel]` |
| Timeout | 60s | `[plugin.limits.timeout_secs]` |
| HTTP calls per execution | 50 | `[plugin.limits.max_http_calls]` |
| Tool invokes per execution | 20 | `[plugin.limits.max_tool_invokes]` |
| Log entries | 1000 | Hardcoded |
| Log message size | 4 KB | Hardcoded |

---

## 4. 核心组件

### 4.1 WasmCapabilities

```rust
// core/src/extension/runtime/wasm/capabilities.rs

/// Parsed capability configuration from aleph.plugin.toml
pub struct WasmCapabilities {
    pub workspace: Option<WorkspaceCapability>,
    pub http: Option<HttpCapability>,
    pub tool_invoke: Option<ToolInvokeCapability>,
    pub secrets: Option<SecretsCapability>,
}

pub struct WorkspaceCapability {
    pub allowed_prefixes: Vec<String>,
}

pub struct HttpCapability {
    pub allowlist: Vec<EndpointPattern>,
    pub credentials: Vec<CredentialBinding>,
    pub rate_limit: Option<RateLimit>,
    pub timeout_secs: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}

pub struct EndpointPattern {
    pub host: String,         // supports *.slack.com wildcards
    pub path_prefix: String,
    pub methods: Vec<String>,
}

pub struct CredentialBinding {
    pub secret_name: String,
    pub inject: CredentialInject,
    pub host_patterns: Vec<String>,
}

pub enum CredentialInject {
    Bearer,
    Basic { username: String },
    Header { name: String, prefix: Option<String> },
    Query { param_name: String },
    UrlPath { placeholder: String },
}

pub struct RateLimit {
    pub requests_per_minute: u32,
    pub requests_per_hour: u32,
}

pub struct ToolInvokeCapability {
    pub aliases: HashMap<String, String>,
    pub max_per_execution: u32,
}

pub struct SecretsCapability {
    pub allowed_patterns: Vec<String>,
}
```

### 4.2 WasmCapabilityKernel

```rust
// core/src/extension/runtime/wasm/capability_kernel.rs

/// Per-execution security kernel for WASM plugins
pub struct WasmCapabilityKernel {
    // Static configuration
    capabilities: WasmCapabilities,
    trust_stage: TrustStage,
    plugin_id: String,

    // Per-execution counters
    http_call_count: AtomicU32,
    tool_invoke_count: AtomicU32,
    log_count: AtomicU32,

    // Shared services
    leak_detector: Arc<LeakDetector>,
    audit_logger: Arc<SandboxAuditLog>,
    secret_store: Arc<dyn SecretStore>,
    http_client: reqwest::Client,
}

impl WasmCapabilityKernel {
    pub async fn check_and_execute_http(&self, req: HttpCallRequest)
        -> Result<HttpCallResponse, CapabilityError>;

    pub fn check_workspace_read(&self, path: &str)
        -> Result<String, CapabilityError>;

    pub async fn check_and_invoke_tool(&self, alias: &str, params: &str)
        -> Result<String, CapabilityError>;

    pub fn check_secret_exists(&self, name: &str) -> bool;

    pub fn log(&self, level: LogLevel, msg: &str) -> Result<(), CapabilityError>;

    pub fn now_millis(&self) -> u64;
}
```

### 4.3 LeakDetector

```rust
// core/src/exec/leak_detector.rs

/// Bidirectional leak detection: intercept + post-scan
pub struct LeakDetector {
    scanner: AhoCorasick,
    patterns: Vec<LeakPattern>,
}

pub struct LeakPattern {
    pub name: String,
    pub regex: Regex,
    pub action: LeakAction,
}

pub enum LeakAction {
    Block,   // Abort request
    Redact,  // Mask content but continue
    Warn,    // Log warning only
}

impl LeakDetector {
    pub fn scan_outbound(&self, content: &str) -> ScanResult;
    pub fn scan_inbound(&self, content: &str) -> ScanResult;
}
```

### 4.4 Allowlist Validator

```rust
// core/src/extension/runtime/wasm/allowlist.rs

/// HTTP endpoint allowlist with anti-bypass measures
pub struct AllowlistValidator {
    patterns: Vec<EndpointPattern>,
}

impl AllowlistValidator {
    /// Validate URL against allowlist
    /// - Rejects http:// (HTTPS only)
    /// - Rejects userinfo in URLs (anti host-confusion)
    /// - Normalizes percent-encoding (anti traversal)
    /// - Rejects encoded separators (%2F)
    pub fn check(&self, method: &str, url: &str) -> Result<(), AllowlistError>;
}
```

### 4.5 Credential Injector

```rust
// core/src/extension/runtime/wasm/credential_injector.rs

/// Injects credentials into HTTP requests at the host boundary
pub struct CredentialInjector;

impl CredentialInjector {
    /// Resolve credentials from SecretStore and inject into request
    /// Supports: Bearer, Basic, Header, Query, UrlPath
    pub async fn inject(
        bindings: &[CredentialBinding],
        secret_store: &dyn SecretStore,
        url: &str,
        headers: &mut HeaderMap,
    ) -> Result<String, CredentialError>;  // returns potentially modified URL
}
```

---

## 5. 信任升级流程

```
New plugin installed
    ↓
TrustStage::Draft
    │  Restrictions: All HTTP requests need approval, tool_invoke disabled
    │  Unlock: 10 successful executions + 0 security violations
    ↓
TrustStage::Trial
    │  Restrictions: Declared HTTP auto-allowed, tool_invoke open
    │  Approval: First access to new domain still requires approval
    │  Unlock: 100 successful executions + manual user promotion
    ↓
TrustStage::Verified
       Restrictions: Fully operates within declared capabilities
       Demotion: Any security violation → immediate rollback to Trial
```

---

## 6. Error Handling

```rust
pub enum CapabilityError {
    NotDeclared(String),       // Capability not declared
    NotAllowed(String),        // Declared but pattern mismatch
    RateLimited(String),       // Rate limit exceeded
    ResourceExhausted(String), // Per-execution limit exceeded
    LeakDetected(String),      // Leak detector intercepted
    ApprovalDenied(String),    // User denied approval
    ApprovalTimeout,           // Approval timed out
    SecretNotFound(String),    // Secret doesn't exist
    PathTraversal(String),     // Path traversal attempt
    InternalError(String),     // Internal error
}
```

All `CapabilityError` variants are mapped to meaningful error strings returned to the WASM guest. Internal details (secret names, real tool names) are never leaked in error messages.

---

## 7. File Organization

### New files

```
core/src/extension/runtime/wasm/
├── capability_kernel.rs      # WasmCapabilityKernel
├── host_functions.rs         # 6 Extism host function registrations
├── allowlist.rs              # HTTP allowlist validator
├── credential_injector.rs    # Credential injection (5 methods)
└── limits.rs                 # Resource limit configuration

core/src/exec/
└── leak_detector.rs          # Aho-Corasick bidirectional leak detection
```

### Modified files

```
core/src/extension/runtime/wasm/
├── mod.rs                    # Import new modules, wire into plugin loading
├── capabilities.rs           # Rewrite: dead PermissionChecker → WasmCapabilities
└── permissions.rs            # Rewrite: actual capability enforcement

core/src/exec/
└── masker.rs                 # Extract shared patterns for LeakDetector reuse

core/src/extension/manifest/
├── types.rs                  # Add capabilities field to PluginManifest
└── aleph_plugin_toml.rs      # Parse [plugin.capabilities] section
```

### Unchanged

- `agent_loop/` — Plugin security is transparent to agent loop
- `gateway/` — No changes
- `builtin_tools/` — No changes
- `exec/kernel.rs`, `exec/decision.rs` — Shell command security layer unchanged

---

## 8. Integration Points

| Integration | File | Change |
|-------------|------|--------|
| **Plugin loading** | `extension/runtime/wasm/mod.rs` | Parse capabilities → create `WasmCapabilityKernel` |
| **Tool execution** | `extension/runtime/wasm/mod.rs` | Check kernel before Extism call |
| **Extism creation** | `extension/runtime/wasm/mod.rs` | Register host functions on `Plugin::new()` |
| **Audit** | `exec/sandbox/audit.rs` | Reuse existing `SandboxAuditLog` |
| **Trust management** | `exec/approval/types.rs` | Reuse existing `TrustStage` |
| **Approval flow** | `extension/skill_tool.rs` | Reference existing `request_skill_permission_async()` pattern |

---

## 9. Security Hardening Checklist

- [ ] Default-deny: no capabilities = zero permissions
- [ ] HTTPS-only enforcement for HTTP requests
- [ ] Userinfo rejection in URLs (anti host-confusion)
- [ ] Percent-encoding normalization (anti traversal)
- [ ] Path traversal blocking (no `..`, `/`, null bytes)
- [ ] Bidirectional leak scanning (outbound + inbound)
- [ ] Credential redaction from error messages
- [ ] Tool alias indirection (WASM never sees real tool names)
- [ ] Per-execution resource counters (HTTP, tool invoke, logs)
- [ ] Fresh kernel per execution (no state leakage)
- [ ] Trust demotion on security violation

---

## 10. Testing Strategy

| Category | Coverage |
|----------|---------|
| **Unit tests** | Capabilities parsing, allowlist matching, path validation, credential injection, leak detection |
| **Integration tests** | End-to-end: load WASM plugin → call host function → verify permission enforcement |
| **Security tests** | Path traversal, URL bypass (userinfo/@, encoding), leak detection evasion, SSRF private IP |
| **Trust tests** | Draft → Trial → Verified progression, demotion on violation |
