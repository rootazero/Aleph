# SandboxHooks 限流实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 SandboxHooks 添加可选的限流功能，按 session_id + tool_category 滑动窗口计数，防止失控的 agent 或外部恶意请求。

**Architecture:**
- 新增 `RateLimitHook` 实现 `SandboxBeforeHook` 接口
- 复用现有 `RateLimiter` 滑动窗口算法，按 `(session_id, tool_category)` 计数
- 新增 `SandboxRateLimitConfig` 集成到 `SandboxConfig`
- Panel 配置覆盖所有可调参数

**Tech Stack:** Rust, tokio, dashmap, schemars

---

## 文件结构

```
src/sandbox/
├── mod.rs              # 添加 re-export
├── config.rs           # 添加 SandboxRateLimitConfig
├── rate_limit.rs      # 新建：限流配置和 RateLimitHook
├── hooks.rs           # 已有接口，无需修改
└── factory.rs         # 注入 rate_limit_hook

src/bin/aleph-server/commands/start/mod.rs   # 构建 hooks 时注入
```

---

## Task 1: 新建 `src/sandbox/rate_limit.rs`

**Files:**
- Create: `src/sandbox/rate_limit.rs`
- Test: `src/sandbox/rate_limit.rs` (inline tests)

- [ ] **Step 1: 创建 rate_limit.rs，包含所有类型定义**

```rust
//! Sandbox rate limiting — session + tool-category based sliding window.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;

use crate::sandbox::command::SandboxCommand;
use crate::sandbox::hooks::{SandboxBeforeHook, SandboxHookContext, SandboxHookResult, SandboxHookResult::Deny};
use crate::session::service::SessionId;

/// Tool danger category for rate limiting.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum ToolCategory {
    Read,
    Write,
    Dangerous,
    Admin,
}

/// Per-category sliding window config.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub max_requests: u32,
    pub window_secs: u64,
    pub burst_allow: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            max_requests: 60,
            window_secs: 60,
            burst_allow: 20,
        }
    }
}

/// Sandbox rate limit configuration.
#[derive(Debug, Clone)]
pub struct SandboxRateLimitConfig {
    pub enabled: bool,
    pub exempt_loopback: bool,
    pub per_category: HashMap<ToolCategory, WindowConfig>,
}

impl Default for SandboxRateLimitConfig {
    fn default() -> Self {
        let mut per_category = HashMap::new();
        per_category.insert(ToolCategory::Read, WindowConfig { max_requests: 60, window_secs: 60, burst_allow: 20 });
        per_category.insert(ToolCategory::Write, WindowConfig { max_requests: 30, window_secs: 60, burst_allow: 10 });
        per_category.insert(ToolCategory::Dangerous, WindowConfig { max_requests: 10, window_secs: 60, burst_allow: 5 });
        per_category.insert(ToolCategory::Admin, WindowConfig { max_requests: 5, window_secs: 60, burst_allow: 2 });
        Self {
            enabled: true,
            exempt_loopback: true,
            per_category,
        }
    }
}

/// Categorize a tool name into a `ToolCategory`.
pub fn categorize_tool(tool_name: &str) -> ToolCategory {
    match tool_name {
        // admin
        "config.patch" | "config.set" | "plugins.install" | "plugins.uninstall"
        | "skills.install" | "skills.delete" => ToolCategory::Admin,
        // dangerous
        "code_exec" | "exec" | "bash_exec" => ToolCategory::Dangerous,
        // write
        "file_write" | "file_edit" | "file_delete" | "folder_write" => ToolCategory::Write,
        // read — default
        _ => ToolCategory::Read,
    }
}

/// Internal per-key sliding window.
struct SlidingWindow {
    timestamps: VecDeque<Instant>,
    burst: u32,
}

/// Rate limit key: (session_id, tool_category).
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct RateLimitKey {
    session_id: SessionId,
    category: ToolCategory,
}

/// Sliding-window rate limiter for sandbox hooks.
pub struct SandboxRateLimiter {
    config: SandboxRateLimitConfig,
    windows: DashMap<RateLimitKey, SlidingWindow>,
}

impl SandboxRateLimiter {
    pub fn new(config: SandboxRateLimitConfig) -> Self {
        Self {
            config,
            windows: DashMap::new(),
        }
    }

    /// Check if execution is allowed. Returns Ok(()) or Err(reason).
    pub fn check_and_record(&self, session_id: &SessionId, category: &ToolCategory) -> Result<(), String> {
        if !self.config.enabled {
            return Ok(());
        }

        let wc = self.config.per_category.get(category)?;

        let now = Instant::now();
        let window_dur = Duration::from_secs(wc.window_secs);
        let key = RateLimitKey { session_id: session_id.clone(), category: category.clone() };

        let mut entry = self.windows.entry(key).or_insert_with(|| SlidingWindow {
            timestamps: VecDeque::new(),
            burst: wc.burst_allow,
        });

        // Evict expired timestamps
        let cutoff = now - window_dur;
        while let Some(&front) = entry.timestamps.front() {
            if front < cutoff {
                entry.timestamps.pop_front();
            } else {
                break;
            }
        }

        let count = entry.timestamps.len() as u32;
        let max = wc.max_requests + wc.burst_allow;

        if count >= max {
            let oldest = entry.timestamps.front().expect("timestamps non-empty");
            let retry_after = (*oldest + window_dur).duration_since(now);
            return Err(format!(
                "rate limit exceeded for {:?}: {}/{} in {}s window (retry after {:?})",
                category, count, max, wc.window_secs, retry_after
            ));
        }

        entry.timestamps.push_back(now);
        Ok(())
    }
}

/// `SandboxBeforeHook` that rate-limits sandbox execution.
pub struct RateLimitHook {
    limiter: Arc<SandboxRateLimiter>,
}

impl RateLimitHook {
    pub fn new(limiter: Arc<SandboxRateLimiter>) -> Self {
        Self { limiter }
    }
}

#[async_trait]
impl SandboxBeforeHook for RateLimitHook {
    fn name(&self) -> &'static str {
        "sandbox.rate_limit"
    }

    async fn before(&self, ctx: SandboxHookContext<'_>) -> SandboxHookResult {
        let session_id = &ctx.command.session_id;
        let category = categorize_tool(ctx.tool_name);

        if let Err(reason) = self.limiter.check_and_record(session_id, &category) {
            tracing::warn!(
                target: "sandbox_rate_limit",
                session_id = ?session_id,
                tool_name = ctx.tool_name,
                category = ?category,
                reason = %reason,
                "sandbox rate limit exceeded"
            );
            return Deny { reason };
        }

        SandboxHookResult::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    fn session_id() -> SessionId {
        SessionKey::ephemeral("test")
    }

    #[tokio::test]
    async fn rate_limit_allows_under_limit() {
        let limiter = Arc::new(SandboxRateLimiter::new(SandboxRateLimitConfig::default()));
        let hook = RateLimitHook::new(limiter);
        let cmd = SandboxCommand {
            session_id: session_id(),
            program: "file_write".into(),
            args: vec![],
            env: Default::default(),
            stdin: None,
            cwd: None,
            capabilities: Default::default(),
            timeout: None,
        };
        let ctx = SandboxHookContext::new("file_write", &cmd);

        // 30 write ops allowed, should all succeed
        for _ in 0..30 {
            assert!(matches!(hook.before(ctx.clone()).await, SandboxHookResult::Allow));
        }
    }

    #[tokio::test]
    async fn rate_limit_denies_over_limit() {
        let limiter = Arc::new(SandboxRateLimiter::new(SandboxRateLimitConfig::default()));
        let hook = RateLimitHook::new(limiter);
        let cmd = SandboxCommand {
            session_id: session_id(),
            program: "file_write".into(),
            args: vec![],
            env: Default::default(),
            stdin: None,
            cwd: None,
            capabilities: Default::default(),
            timeout: None,
        };
        let ctx = SandboxHookContext::new("file_write", &cmd);

        // Exhaust limit (30 + 10 burst = 40 for write)
        for _ in 0..40 {
            hook.before(ctx.clone()).await;
        }

        // 41st should be denied
        let result = hook.before(ctx).await;
        assert!(matches!(result, SandboxHookResult::Deny { .. }));
    }

    #[test]
    fn categorize_tool_dangerous() {
        assert_eq!(categorize_tool("code_exec"), ToolCategory::Dangerous);
        assert_eq!(categorize_tool("bash_exec"), ToolCategory::Dangerous);
        assert_eq!(categorize_tool("exec"), ToolCategory::Dangerous);
    }

    #[test]
    fn categorize_tool_admin() {
        assert_eq!(categorize_tool("config.patch"), ToolCategory::Admin);
        assert_eq!(categorize_tool("plugins.install"), ToolCategory::Admin);
    }

    #[test]
    fn categorize_tool_write() {
        assert_eq!(categorize_tool("file_write"), ToolCategory::Write);
        assert_eq!(categorize_tool("file_edit"), ToolCategory::Write);
    }

    #[test]
    fn categorize_tool_read_default() {
        assert_eq!(categorize_tool("search"), ToolCategory::Read);
        assert_eq!(categorize_tool("memory_retrieval"), ToolCategory::Read);
        assert_eq!(categorize_tool("unknown_tool"), ToolCategory::Read);
    }
}
```

- [ ] **Step 2: 运行测试验证**

Run: `cargo test -p alephcore sandbox::rate_limit --lib`
Expected: All tests pass

- [ ] **Step 3: Commit**

```bash
git add src/sandbox/rate_limit.rs
git commit -m "sandbox: add rate limiting hook implementation"
```

---

## Task 2: 更新 `src/sandbox/config.rs`

**Files:**
- Modify: `src/sandbox/config.rs:1-173`
- Test: `src/sandbox/config.rs` (inline tests)

- [ ] **Step 1: 添加 SandboxRateLimitConfig 到 config.rs**

在 `use serde::{Deserialize, Serialize};` 后添加：

```rust
use std::collections::HashMap;
use crate::sandbox::rate_limit::{SandboxRateLimitConfig, ToolCategory, WindowConfig};
```

在 `LinuxSandboxConfig` 之前添加：

```rust
/// TOML schema for sandbox rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxRateLimitConfigSchema {
    #[serde(default = "default_rate_limit_enabled")]
    pub enabled: bool,
    #[serde(default = "default_rate_limit_exempt_loopback")]
    pub exempt_loopback: bool,
    #[serde(default = "default_rate_limit_read")]
    pub read: WindowConfigSchema,
    #[serde(default = "default_rate_limit_write")]
    pub write: WindowConfigSchema,
    #[serde(default = "default_rate_limit_dangerous")]
    pub dangerous: WindowConfigSchema,
    #[serde(default = "default_rate_limit_admin")]
    pub admin: WindowConfigSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WindowConfigSchema {
    #[serde(default = "default_max_requests")]
    pub max_requests: u32,
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    #[serde(default = "default_burst_allow")]
    pub burst_allow: u32,
}

fn default_rate_limit_enabled() -> bool { true }
fn default_rate_limit_exempt_loopback() -> bool { true }
fn default_max_requests() -> u32 { 60 }
fn default_window_secs() -> u64 { 60 }
fn default_burst_allow() -> u32 { 20 }

fn default_rate_limit_read() -> WindowConfigSchema { WindowConfigSchema { max_requests: 60, window_secs: 60, burst_allow: 20 } }
fn default_rate_limit_write() -> WindowConfigSchema { WindowConfigSchema { max_requests: 30, window_secs: 60, burst_allow: 10 } }
fn default_rate_limit_dangerous() -> WindowConfigSchema { WindowConfigSchema { max_requests: 10, window_secs: 60, burst_allow: 5 } }
fn default_rate_limit_admin() -> WindowConfigSchema { WindowConfigSchema { max_requests: 5, window_secs: 60, burst_allow: 2 } }

impl From<SandboxRateLimitConfigSchema> for SandboxRateLimitConfig {
    fn from(schema: SandboxRateLimitConfigSchema) -> Self {
        let mut per_category = HashMap::new();
        per_category.insert(ToolCategory::Read, WindowConfig {
            max_requests: schema.read.max_requests,
            window_secs: schema.read.window_secs,
            burst_allow: schema.read.burst_allow,
        });
        per_category.insert(ToolCategory::Write, WindowConfig {
            max_requests: schema.write.max_requests,
            window_secs: schema.write.window_secs,
            burst_allow: schema.write.burst_allow,
        });
        per_category.insert(ToolCategory::Dangerous, WindowConfig {
            max_requests: schema.dangerous.max_requests,
            window_secs: schema.dangerous.window_secs,
            burst_allow: schema.dangerous.burst_allow,
        });
        per_category.insert(ToolCategory::Admin, WindowConfig {
            max_requests: schema.admin.max_requests,
            window_secs: schema.admin.window_secs,
            burst_allow: schema.admin.burst_allow,
        });
        Self {
            enabled: schema.enabled,
            exempt_loopback: schema.exempt_loopback,
            per_category,
        }
    }
}
```

在 `SandboxConfig` 结构体中添加：

```rust
#[serde(default)]
pub rate_limit: SandboxRateLimitConfigSchema,
```

在 `Default for SandboxConfig` 实现中添加：

```rust
rate_limit: SandboxRateLimitConfigSchema {
    enabled: default_rate_limit_enabled(),
    exempt_loopback: default_rate_limit_exempt_loopback(),
    read: default_rate_limit_read(),
    write: default_rate_limit_write(),
    dangerous: default_rate_limit_dangerous(),
    admin: default_rate_limit_admin(),
},
```

- [ ] **Step 2: 添加测试**

```rust
#[test]
fn rate_limit_config_deserializes_from_toml() {
    let toml = r#"
        [sandbox.rate_limit]
        enabled = true
        read = { max_requests = 100, window_secs = 30, burst_allow = 50 }
    "#;
    let cfg: SandboxConfig = toml::from_str(toml).expect(" parses");
    assert!(cfg.rate_limit.enabled);
    assert_eq!(cfg.rate_limit.read.max_requests, 100);
}
```

- [ ] **Step 3: 运行测试验证**

Run: `cargo test -p alephcore sandbox::config --lib`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/sandbox/config.rs
git commit -m "sandbox: add rate limit config schema"
```

---

## Task 3: 更新 `src/sandbox/mod.rs`

**Files:**
- Modify: `src/sandbox/mod.rs:1-91`

- [ ] **Step 1: 添加 re-export**

在 `pub mod hooks;` 后添加：

```rust
pub mod rate_limit;
```

在 re-exports 部分添加：

```rust
pub use rate_limit::{RateLimitHook, SandboxRateLimitConfig};
```

- [ ] **Step 2: Commit**

```bash
git add src/sandbox/mod.rs
git commit -m "sandbox: re-export rate_limit module"
```

---

## Task 4: 更新 `src/sandbox/factory.rs`

**Files:**
- Modify: `src/sandbox/factory.rs:28-41`

- [ ] **Step 1: 修改 `build_sandbox` 签名以接受 rate_limit_config**

将 `build_sandbox` 函数修改为：

```rust
pub fn build_sandbox(
    cfg: &SandboxConfig,
    driver: Arc<dyn OsSandboxDriverTrait>,
    approval: Arc<ApprovalGate>,
    rate_limit_config: SandboxRateLimitConfig,
) -> Arc<dyn Sandbox> {
    if !cfg.enabled {
        return Arc::new(NoopSandbox);
    }
    let hooks = SandboxHooks::new()
        .with_before(Arc::new(RateLimitHook::new(Arc::new(SandboxRateLimiter::new(rate_limit_config))));
    let ws = WorkspaceSandbox::new(cfg.workspace_root.clone(), driver, approval, hooks)
        .with_timeout(Duration::from_secs(cfg.default_timeout_seconds))
        .with_max_output_bytes(cfg.max_output_bytes);
    Arc::new(ws)
}
```

- [ ] **Step 2: 更新所有调用 `build_sandbox` 的测试**

在每个调用处将 `SandboxHooks::new()` 替换为 `SandboxRateLimitConfig::default().into()`:

```rust
// build_sandbox(&cfg, driver, gate, SandboxRateLimitConfig::default().into())
```

- [ ] **Step 3: 运行测试验证**

Run: `cargo test -p alephcore sandbox::factory --lib`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/sandbox/factory.rs
git commit -m "sandbox: wire rate limit hook into build_sandbox"
```

---

## Task 5: 更新 `src/bin/aleph-server/commands/start/mod.rs`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs`
- Test: `cargo check`

- [ ] **Step 1: 找到 build_sandbox 调用，添加 rate_limit_config 参数**

找到调用 `build_sandbox` 的位置，添加：

```rust
use crate::sandbox::{SandboxRateLimitConfig, build_sandbox};

// ... 在组装 sandbox 时 ...
let sandbox = build_sandbox(
    &cfg.sandbox,
    driver,
    approval,
    cfg.sandbox.rate_limit.clone().into(),  // SandboxRateLimitConfig
);
```

- [ ] **Step 2: 运行 cargo check 验证**

Run: `cargo check -p aleph-server`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs
git commit -m "server: pass rate limit config to build_sandbox"
```

---

## Task 6: Panel 配置集成

**Files:**
- Modify: `src/config/types/general.rs` 或相关 panel 配置处
- Test: Panel UI 可视化验证

- [ ] **Step 1: 确认 panel 配置路径**

检查 panel 中 sandbox 配置是如何暴露的，添加 rate_limit 配置节。

- [ ] **Step 2: Commit**

```bash
git add <panel_config_file>
git commit -m "panel: add sandbox rate limit configuration UI"
```

---

## 验证清单

- [ ] `cargo test -p alephcore sandbox::rate_limit --lib` — All pass
- [ ] `cargo test -p alephcore sandbox::config --lib` — All pass
- [ ] `cargo test -p alephcore sandbox::factory --lib` — All pass
- [ ] `cargo check -p aleph-server` — Compiles
- [ ] `cargo clippy -p alephcore -- -D warnings` — No warnings

---

## 依赖

无需新增依赖，复用：
- `dashmap` (已有)
- `tokio` (已有)
- `schemars` (已有)
