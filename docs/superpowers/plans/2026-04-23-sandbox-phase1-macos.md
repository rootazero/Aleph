# Phase 1: macOS Sandbox策略增强 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 增强Aleph macOS sandbox策略生成，迁移旧代码到`src/sandbox/platforms/`，为后续Linux/Windows平台支持建立基础架构。

**Architecture:** 保持现有`Sandbox` trait和`WorkspaceSandbox`不变，新增`src/sandbox/platforms/`目录存放平台特定实现。macOS实现增强seatbelt策略生成（base policy + network policy + platform defaults + glob支持）。

**Tech Stack:** Rust 2024, tokio, async_trait, serde, sha2, tracing

**Source spec:** `docs/superpowers/specs/2026-04-23-sandbox-multiplatform-design.md` §7.1

---

## File Structure

**Created:**
- `src/sandbox/platforms/mod.rs` — 平台分发 + `create_platform_driver()`
- `src/sandbox/platforms/common.rs` — 共享工具（路径归一化、策略转换）
- `src/sandbox/platforms/macos/mod.rs` — `MacOSSandboxDriver`实现
- `src/sandbox/platforms/macos/seatbelt.rs` — SBPL策略生成
- `src/sandbox/platforms/macos/tests.rs` — macOS特定单元测试
- `src/sandbox/policy.rs` — `SandboxPolicy`统一策略表达

**Modified:**
- `src/sandbox/driver.rs` — 添加`platform()`和`is_supported()`方法
- `src/sandbox/factory.rs` — 使用`create_platform_driver()`
- `src/sandbox/mod.rs` — 注册`platforms`和`policy`模块
- `src/exec/sandbox/executor.rs` — 标记deprecated，逐步迁移逻辑

**Deleted (Phase 1末期):**
- `src/exec/sandbox/capabilities.rs` — 旧Capabilities（确认无外部引用后）
- `src/exec/sandbox/adapter.rs` — 旧SandboxAdapter（确认无外部引用后）

---

## Pre-flight

- [ ] **Pre-1: 确认当前构建状态**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev`

Run: `cargo test -p alephcore --lib sandbox 2>&1 | tail -10`
Expected: 现有sandbox测试通过

- [ ] **Pre-2: 确认旧代码引用情况**

Run: `grep -rn 'exec::sandbox' src/ | grep -v 'executor.rs' | head -20`
记录哪些文件引用了旧sandbox模块（除executor.rs外）。

---

## Task 1: 增强OsSandboxDriverTrait

**Files:**
- Modify: `src/sandbox/driver.rs`

**Context:** 为trait添加平台标识和可用性检测，使WorkspaceSandbox能查询驱动能力。

- [ ] **Step 1: 修改`OsSandboxDriverTrait`**

```rust
// src/sandbox/driver.rs

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};

/// OS-specific seatbelt / sandbox-exec profile bytes or handle.
/// Opaque to WorkspaceSandbox.
#[derive(Debug, Clone)]
pub struct OsSandboxProfile {
    /// macOS: sandbox-exec SBPL profile text.
    /// Linux: bubblewrap argv JSON.
    /// Windows: policy JSON for restricted token.
    pub contents: String,
}

#[async_trait]
pub trait OsSandboxDriverTrait: Send + Sync + 'static {
    /// 平台标识符，用于日志和诊断
    fn platform(&self) -> &'static str;

    /// 当前平台是否支持此驱动
    fn is_supported(&self) -> bool;

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError>;

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError>;
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev`

- [ ] **Step 3: Commit**

```bash
git add src/sandbox/driver.rs
git commit -m "sandbox: add platform() and is_supported() to OsSandboxDriverTrait

Phase 1 Task 1: Add platform identification and availability check
to OsSandboxDriverTrait for multi-platform support."
```

---

## Task 2: 创建统一策略表达（SandboxPolicy）

**Files:**
- Create: `src/sandbox/policy.rs`

**Context:** 从`SandboxCapabilities`转换而来的内部策略表达，供各平台驱动使用。

- [ ] **Step 1: 创建`src/sandbox/policy.rs`**

```rust
//! SandboxPolicy — 内部统一策略表达。
//!
//! 从用户-facing的`SandboxCapabilities`转换而来，供各平台驱动
//! 转换为平台特定策略。

use std::path::PathBuf;

/// 内部策略表达
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub filesystem: FsPolicy,
    pub network: NetworkPolicy,
    pub process: ProcessPolicy,
    pub environment: EnvPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsPolicy {
    /// 只允许workspace目录
    WorkspaceOnly,
    /// 指定可读路径列表
    ReadPaths(Vec<PathBuf>),
    /// 指定可写路径列表
    WritePaths(Vec<PathBuf>),
    /// 全磁盘读取（保留路径）
    FullRead { exclude: Vec<PathBuf> },
    /// 全磁盘写入（保留路径）
    FullWrite { exclude: Vec<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPolicy {
    None,
    AllowHosts(Vec<String>),
    AllowAll,
    /// 允许代理loopback端口（用于managed network）
    ProxyOnly { ports: Vec<u16> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPolicy {
    pub allow_fork: bool,
    pub timeout_secs: u64,
    pub max_memory_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvPolicy {
    Inherit,
    Restricted,
    Minimal,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            filesystem: FsPolicy::WorkspaceOnly,
            network: NetworkPolicy::None,
            process: ProcessPolicy {
                allow_fork: false,
                timeout_secs: 60,
                max_memory_mb: Some(512),
            },
            environment: EnvPolicy::Restricted,
        }
    }
}

/// 从SandboxCapabilities转换为SandboxPolicy
impl From<&crate::sandbox::capabilities::SandboxCapabilities> for SandboxPolicy {
    fn from(caps: &crate::sandbox::capabilities::SandboxCapabilities) -> Self {
        use crate::sandbox::capabilities::NetworkPolicy as CapNetPolicy;

        let filesystem = if caps.fs_read.is_empty() && caps.fs_write.is_empty() {
            FsPolicy::WorkspaceOnly
        } else {
            // 合并read和write路径
            let mut write_paths = caps.fs_write.clone();
            let read_paths: Vec<PathBuf> = caps
                .fs_read
                .iter()
                .filter(|p| !write_paths.contains(p))
                .cloned()
                .collect();

            if !write_paths.is_empty() {
                FsPolicy::WritePaths(write_paths)
            } else {
                FsPolicy::ReadPaths(read_paths)
            }
        };

        let network = match &caps.network {
            CapNetPolicy::None => NetworkPolicy::None,
            CapNetPolicy::AllowAll => NetworkPolicy::AllowAll,
            CapNetPolicy::AllowHosts { hosts } => NetworkPolicy::AllowHosts(hosts.clone()),
        };

        let process = ProcessPolicy {
            allow_fork: caps.spawn_subprocess,
            timeout_secs: 60,
            max_memory_mb: Some(512),
        };

        Self {
            filesystem,
            network,
            process,
            environment: EnvPolicy::Restricted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::capabilities::{NetworkPolicy as CapNet, SandboxCapabilities};

    #[test]
    fn strict_caps_to_policy() {
        let caps = SandboxCapabilities::strict();
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.filesystem, FsPolicy::WorkspaceOnly);
        assert_eq!(policy.network, NetworkPolicy::None);
        assert!(!policy.process.allow_fork);
    }

    #[test]
    fn network_allowall_to_policy() {
        let caps = SandboxCapabilities {
            network: CapNet::AllowAll,
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.network, NetworkPolicy::AllowAll);
    }

    #[test]
    fn fs_write_paths_to_policy() {
        let caps = SandboxCapabilities {
            fs_write: vec!["/tmp".into(), "/var/log".into()],
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        match policy.filesystem {
            FsPolicy::WritePaths(paths) => {
                assert_eq!(paths.len(), 2);
            }
            other => panic!("expected WritePaths, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: 注册模块**

修改`src/sandbox/mod.rs`，添加：
```rust
pub mod policy;
pub use policy::{SandboxPolicy, FsPolicy, NetworkPolicy, ProcessPolicy, EnvPolicy};
```

- [ ] **Step 3: 验证编译和测试**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev`

Run: `cargo test -p alephcore --lib sandbox::policy 2>&1 | tail -10`
Expected: 3 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/sandbox/policy.rs src/sandbox/mod.rs
git commit -m "sandbox: add SandboxPolicy unified policy representation

Phase 1 Task 2: Introduce SandboxPolicy as internal policy expression
converted from SandboxCapabilities. Provides platform-agnostic policy
that each platform driver translates to OS-specific rules."
```

---

## Task 3: 创建平台基础架构

**Files:**
- Create: `src/sandbox/platforms/mod.rs`
- Create: `src/sandbox/platforms/common.rs`

**Context:** 建立平台实现目录结构，提供共享工具和平台分发函数。

- [ ] **Step 1: 创建`src/sandbox/platforms/common.rs`**

```rust
//! 平台实现共享工具

use std::path::{Path, PathBuf};

/// 将路径归一化为绝对路径，用于sandbox策略。
///
/// - 相对路径 → 相对于cwd的绝对路径
/// - 包含`..`的路径 → canonicalize
/// - 非UTF-8路径 → 尽力转换
pub fn normalize_path_for_sandbox(path: &Path, cwd: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    // 尝试canonicalize，失败则返回绝对路径
    absolute.canonicalize().ok().or(Some(absolute))
}

/// 判断路径是否在允许列表中（前缀匹配）
pub fn path_is_allowed(path: &Path, allowed: &[PathBuf]) -> bool {
    let normalized = match normalize_path_for_sandbox(path, Path::new("/")) {
        Some(p) => p,
        None => return false,
    };

    allowed.iter().any(|allowed_path| {
        normalized.starts_with(allowed_path) || allowed_path.starts_with(&normalized)
    })
}

/// 生成glob模式对应的regex（简化版）
///
/// 支持：
/// - `*` — 匹配单路径组件
/// - `**` — 匹配任意深度
/// - `?` — 匹配单个字符
pub fn glob_to_regex(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }

    let mut regex = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    // ** matches any depth
                    regex.push_str(".*");
                    i += 2;
                    // consume following / if present
                    if i < chars.len() && chars[i] == '/' {
                        i += 1;
                    }
                } else {
                    // * matches single component
                    regex.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                regex.push_str("[^/]");
                i += 1;
            }
            '.' => {
                regex.push_str("\\.");
                i += 1;
            }
            '/' => {
                regex.push('/');
                i += 1;
            }
            c => {
                regex.push(c);
                i += 1;
            }
        }
    }

    regex.push('$');
    Some(regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_absolute_path() {
        let path = Path::new("/usr/bin");
        let cwd = Path::new("/home/user");
        let result = normalize_path_for_sandbox(path, cwd);
        assert!(result.is_some());
        assert!(result.unwrap().is_absolute());
    }

    #[test]
    fn normalize_relative_path() {
        let path = Path::new("src/main.rs");
        let cwd = Path::new("/home/user/project");
        let result = normalize_path_for_sandbox(path, cwd).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/project/src/main.rs"));
    }

    #[test]
    fn glob_star_to_regex() {
        let regex = glob_to_regex("*.rs").unwrap();
        assert_eq!(regex, "^[^/]*\\.rs$");
    }

    #[test]
    fn glob_double_star_to_regex() {
        let regex = glob_to_regex("src/**/*.rs").unwrap();
        assert!(regex.contains(".*"));
    }
}
```

- [ ] **Step 2: 创建`src/sandbox/platforms/mod.rs`**

```rust
//! 平台特定sandbox实现
//!
//! 每个平台提供`OsSandboxDriverTrait`的实现：
//! - macOS: `MacOSSandboxDriver` (sandbox-exec)
//! - Linux: `LinuxSandboxDriver` (bubblewrap + landlock/seccomp)
//! - Windows: `WindowsSandboxDriver` (restricted token + ACL)

pub mod common;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

use std::sync::Arc;
use crate::sandbox::driver::OsSandboxDriverTrait;

/// 创建当前平台的sandbox驱动
///
/// 使用编译期条件编译选择平台实现
pub fn create_platform_driver() -> Arc<dyn OsSandboxDriverTrait> {
    #[cfg(target_os = "macos")]
    return Arc::new(macos::MacOSSandboxDriver::new());

    #[cfg(target_os = "linux")]
    return Arc::new(linux::LinuxSandboxDriver::new());

    #[cfg(target_os = "windows")]
    return Arc::new(windows::WindowsSandboxDriver::new());

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Arc::new(UnsupportedDriver);
}

/// 不支持的平台的fallback驱动
pub struct UnsupportedDriver;

#[async_trait::async_trait]
impl OsSandboxDriverTrait for UnsupportedDriver {
    fn platform(&self) -> &'static str {
        "unsupported"
    }

    fn is_supported(&self) -> bool {
        false
    }

    fn profile_for(
        &self,
        _capabilities: &crate::sandbox::capabilities::SandboxCapabilities,
        _cwd: &std::path::Path,
    ) -> Result<crate::sandbox::driver::OsSandboxProfile, crate::sandbox::command::SandboxError> {
        Err(crate::sandbox::command::SandboxError::CapabilityDenied {
            reason: "sandbox not supported on this platform".into(),
        })
    }

    async fn run(
        &self,
        _program: &str,
        _args: &[String],
        _env: &std::collections::HashMap<String, String>,
        _stdin: Option<&[u8]>,
        _cwd: &std::path::Path,
        _profile: &crate::sandbox::driver::OsSandboxProfile,
        _timeout: std::time::Duration,
        _max_output_bytes: usize,
    ) -> Result<crate::sandbox::command::SandboxOutput, crate::sandbox::command::SandboxError> {
        Err(crate::sandbox::command::SandboxError::CapabilityDenied {
            reason: "sandbox not supported on this platform".into(),
        })
    }
}
```

- [ ] **Step 3: 注册模块**

修改`src/sandbox/mod.rs`，添加：
```rust
pub mod platforms;
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev`

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/platforms/
git commit -m "sandbox: add platforms directory structure with common utilities

Phase 1 Task 3: Create src/sandbox/platforms/ with common utilities
(path normalization, glob-to-regex) and platform driver dispatch.
UnsupportedDriver provides graceful fallback for unknown platforms."
```

---

## Task 4: 实现增强版macOS Seatbelt策略生成

**Files:**
- Create: `src/sandbox/platforms/macos/seatbelt.rs`
- Create: `src/sandbox/platforms/macos/mod.rs`

**Context:** 参考codex的`seatbelt.rs`，实现精细的SBPL策略生成。

- [ ] **Step 1: 创建`src/sandbox/platforms/macos/seatbelt.rs`**

```rust
//! macOS Seatbelt SBPL策略生成
//!
//! 参考codex seatbelt实现，提供精细的文件系统、网络和进程控制策略。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::sandbox::platforms::common::normalize_path_for_sandbox;
use crate::sandbox::policy::{EnvPolicy, FsPolicy, NetworkPolicy, ProcessPolicy, SandboxPolicy};

/// macOS seatbelt基础策略模板
const SEATBELT_BASE_POLICY: &str = r#"
(version 1)
(deny default)
(allow file-read-metadata)
(allow signal (target self))
"#;

/// macOS网络基础策略
const SEATBELT_NETWORK_BASE: &str = r#"
; Network base rules
"#;

/// macOS平台默认可读路径
const PLATFORM_DEFAULT_READS: &[&str] = &[
    "/usr/lib",
    "/System/Library",
    "/Library",
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
];

/// 生成完整的seatbelt策略
pub fn generate_seatbelt_policy(
    policy: &SandboxPolicy,
    cwd: &Path,
    home_dir: Option<&Path>,
) -> Result<String, String> {
    let mut sections = Vec::new();

    // 1. 基础策略
    sections.push(SEATBELT_BASE_POLICY.to_string());

    // 2. 文件系统策略
    let fs_policy = generate_filesystem_policy(&policy.filesystem, cwd, home_dir)?;
    sections.push(fs_policy);

    // 3. 网络策略
    let network_policy = generate_network_policy(&policy.network);
    sections.push(network_policy);

    // 4. 进程策略
    let process_policy = generate_process_policy(&policy.process);
    sections.push(process_policy);

    // 5. 环境策略
    let env_policy = generate_environment_policy(&policy.environment);
    sections.push(env_policy);

    Ok(sections.join("\n"))
}

fn generate_filesystem_policy(
    fs: &FsPolicy,
    cwd: &Path,
    home_dir: Option<&Path>,
) -> Result<String, String> {
    let mut policy = String::new();

    // 总是允许workspace目录读写
    let cwd_normalized = normalize_path_for_sandbox(cwd, cwd)
        .ok_or_else(|| "failed to normalize cwd".to_string())?;

    policy.push_str(&format!(
        "; Workspace directory\n(allow file-read* (subpath \"{}\"))\n(allow file-write* (subpath \"{}\"))\n",
        cwd_normalized.display(),
        cwd_normalized.display()
    ));

    // 平台默认可读路径
    for path in PLATFORM_DEFAULT_READS {
        policy.push_str(&format!(
            "(allow file-read* (subpath \"{}\"))\n",
            path
        ));
    }

    match fs {
        FsPolicy::WorkspaceOnly => {
            // 仅workspace，已添加
        }
        FsPolicy::ReadPaths(paths) => {
            for path in paths {
                let normalized = normalize_path_for_sandbox(path, cwd)
                    .ok_or_else(|| format!("failed to normalize path: {}", path.display()))?;
                policy.push_str(&format!(
                    "(allow file-read* (subpath \"{}\"))\n",
                    normalized.display()
                ));
            }
        }
        FsPolicy::WritePaths(paths) => {
            for path in paths {
                let normalized = normalize_path_for_sandbox(path, cwd)
                    .ok_or_else(|| format!("failed to normalize path: {}", path.display()))?;
                policy.push_str(&format!(
                    "(allow file-read* (subpath \"{}\"))\n",
                    normalized.display()
                ));
                policy.push_str(&format!(
                    "(allow file-write* (subpath \"{}\"))\n",
                    normalized.display()
                ));
            }
        }
        FsPolicy::FullRead { exclude } => {
            policy.push_str("(allow file-read* (subpath \"/\"))\n");
            for path in exclude {
                let normalized = normalize_path_for_sandbox(path, cwd)
                    .ok_or_else(|| format!("failed to normalize path: {}", path.display()))?;
                policy.push_str(&format!(
                    "(deny file-read* (subpath \"{}\"))\n",
                    normalized.display()
                ));
            }
        }
        FsPolicy::FullWrite { exclude } => {
            policy.push_str("(allow file-read* (subpath \"/\"))\n");
            policy.push_str("(allow file-write* (subpath \"/\"))\n");
            for path in exclude {
                let normalized = normalize_path_for_sandbox(path, cwd)
                    .ok_or_else(|| format!("failed to normalize path: {}", path.display()))?;
                policy.push_str(&format!(
                    "(deny file-read* (subpath \"{}\"))\n",
                    normalized.display()
                ));
                policy.push_str(&format!(
                    "(deny file-write* (subpath \"{}\"))\n",
                    normalized.display()
                ));
            }
        }
    }

    // 添加home目录可读（用于配置文件等）
    if let Some(home) = home_dir {
        let home_normalized = normalize_path_for_sandbox(home, cwd)
            .ok_or_else(|| "failed to normalize home dir".to_string())?;
        policy.push_str(&format!(
            "; Home directory (read-only)\n(allow file-read* (subpath \"{}\"))\n",
            home_normalized.display()
        ));
    }

    Ok(policy)
}

fn generate_network_policy(network: &NetworkPolicy) -> String {
    let mut policy = String::new();
    policy.push_str("; Network policy\n");

    match network {
        NetworkPolicy::None => {
            policy.push_str("(deny network-outbound)\n");
            policy.push_str("(deny network-inbound)\n");
        }
        NetworkPolicy::AllowHosts(hosts) => {
            for host in hosts {
                policy.push_str(&format!(
                    "(allow network-outbound (remote ip \"{}\"))\n",
                    host
                ));
            }
        }
        NetworkPolicy::AllowAll => {
            policy.push_str("(allow network-outbound)\n");
            policy.push_str("(allow network-inbound)\n");
        }
        NetworkPolicy::ProxyOnly { ports } => {
            // 允许DNS
            policy.push_str("(allow network-outbound (remote ip \"*:*:53\"))\n");
            // 允许loopback代理端口
            for port in ports {
                policy.push_str(&format!(
                    "(allow network-outbound (remote ip \"127.0.0.1:{}\"))\n",
                    port
                ));
                policy.push_str(&format!(
                    "(allow network-outbound (remote ip \"[::1]:{}\"))\n",
                    port
                ));
            }
        }
    }

    policy
}

fn generate_process_policy(process: &ProcessPolicy) -> String {
    let mut policy = String::new();
    policy.push_str("; Process policy\n");

    if !process.allow_fork {
        policy.push_str("(deny process-fork)\n");
    }

    // 注意：seatbelt不直接支持timeout和memory限制，
    // 这些由WorkspaceSandbox在调用层处理

    policy
}

fn generate_environment_policy(env: &EnvPolicy) -> String {
    let mut policy = String::new();
    policy.push_str("; Environment policy\n");

    match env {
        EnvPolicy::Minimal => {
            // 最小环境，只允许特定变量
            policy.push_str("; Minimal environment - only PATH, HOME, USER, TMPDIR\n");
        }
        EnvPolicy::Restricted | EnvPolicy::Inherit => {
            // 继承或受限环境，seatbelt不额外限制
        }
    }

    policy
}

/// 生成sandbox-exec命令行参数
pub fn create_seatbelt_args(policy_text: &str, command: &[String]) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        policy_text.to_string(),
    ];
    args.push("--".to_string());
    args.extend(command.iter().cloned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_strict_policy() {
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test");
        let result = generate_seatbelt_policy(&policy, cwd, None);
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(text.contains("deny default"));
        assert!(text.contains("deny network-outbound"));
        assert!(!text.contains("allow network-outbound"));
    }

    #[test]
    fn generate_network_allowall_policy() {
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/test");
        let result = generate_seatbelt_policy(&policy, cwd, None).unwrap();
        assert!(result.contains("allow network-outbound"));
        assert!(result.contains("allow network-inbound"));
    }

    #[test]
    fn generate_fs_write_paths_policy() {
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec!["/tmp".into(), "/var/log".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/test");
        let result = generate_seatbelt_policy(&policy, cwd, None).unwrap();
        assert!(result.contains("/tmp"));
        assert!(result.contains("/var/log"));
        assert!(result.contains("file-write*"));
    }

    #[test]
    fn create_seatbelt_args_format() {
        let policy = "(version 1)";
        let cmd = vec!["echo".to_string(), "hello".to_string()];
        let args = create_seatbelt_args(policy, &cmd);
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], "(version 1)");
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "echo");
        assert_eq!(args[4], "hello");
    }
}
```

- [ ] **Step 2: 创建`src/sandbox/platforms/macos/mod.rs`**

```rust
//! macOS Sandbox驱动实现
//!
//! 使用sandbox-exec提供OS-level隔离。

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::macos::seatbelt::{
    create_seatbelt_args, generate_seatbelt_policy,
};
use crate::sandbox::policy::SandboxPolicy;

pub mod seatbelt;

/// macOS sandbox-exec路径
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// macOS Seatbelt Sandbox驱动
pub struct MacOSSandboxDriver;

impl MacOSSandboxDriver {
    pub fn new() -> Self {
        Self
    }

    /// 检查sandbox-exec是否可用
    fn sandbox_exec_available() -> bool {
        std::path::Path::new(SANDBOX_EXEC_PATH).exists()
    }
}

#[async_trait]
impl OsSandboxDriverTrait for MacOSSandboxDriver {
    fn platform(&self) -> &'static str {
        "macos/seatbelt"
    }

    fn is_supported(&self) -> bool {
        Self::sandbox_exec_available()
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        if !Self::sandbox_exec_available() {
            return Err(SandboxError::CapabilityDenied {
                reason: "sandbox-exec not available".into(),
            });
        }

        let policy = SandboxPolicy::from(capabilities);
        let home_dir = dirs::home_dir();

        let policy_text = generate_seatbelt_policy(&policy, cwd, home_dir.as_deref())
            .map_err(|e| SandboxError::ProfileGeneration(e))?;

        Ok(OsSandboxProfile {
            contents: policy_text,
        })
    }

    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        use tokio::process::Command;
        use tokio::time;

        let mut cmd_args = vec![program.to_string()];
        cmd_args.extend_from_slice(args);

        let seatbelt_args = create_seatbelt_args(&profile.contents, &cmd_args);

        let mut command = Command::new(SANDBOX_EXEC_PATH);
        command.args(&seatbelt_args);
        command.current_dir(cwd);
        command.env_clear();

        // 设置环境变量
        for (key, value) in env {
            command.env(key, value);
        }

        // 添加基本环境变量
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        if let Some(home) = dirs::home_dir() {
            command.env("HOME", home);
        }

        // 处理stdin
        if let Some(stdin_bytes) = stdin {
            use std::process::Stdio;
            use tokio::io::AsyncWriteExt;

            command.stdin(Stdio::piped());
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());

            let mut child = command
                .spawn()
                .map_err(|e| SandboxError::Io(format!("spawn failed: {}", e)))?;

            // 写入stdin
            if let Some(mut child_stdin) = child.stdin.take() {
                child_stdin
                    .write_all(stdin_bytes)
                    .await
                    .map_err(|e| SandboxError::Io(format!("stdin write failed: {}", e)))?;
                // 关闭stdin以发送EOF
                drop(child_stdin);
            }

            // 等待输出或超时
            let result = time::timeout(timeout, child.wait_with_output()).await;

            match result {
                Ok(Ok(output)) => {
                    let stdout = truncate_bytes(output.stdout, max_output_bytes);
                    let stderr = truncate_bytes(output.stderr, max_output_bytes);
                    let truncated = stdout.len() < output.stdout.len()
                        || stderr.len() < output.stderr.len();

                    Ok(SandboxOutput {
                        stdout,
                        stderr,
                        exit_code: output.status.code(),
                        signal: None, // macOS signal handling is complex, simplified here
                        truncated,
                        duration_ms: 0, // TODO: measure actual duration
                    })
                }
                Ok(Err(e)) => Err(SandboxError::Io(format!("process error: {}", e))),
                Err(_) => {
                    // 超时，尝试终止进程
                    let _ = child.start_kill();
                    Err(SandboxError::Timeout {
                        elapsed_ms: timeout.as_millis() as u64,
                    })
                }
            }
        } else {
            // 无stdin，使用标准方式
            let output_result = time::timeout(timeout, command.output()).await;

            match output_result {
                Ok(Ok(output)) => {
                    let stdout = truncate_bytes(output.stdout, max_output_bytes);
                    let stderr = truncate_bytes(output.stderr, max_output_bytes);
                    let truncated = stdout.len() < output.stdout.len()
                        || stderr.len() < output.stderr.len();

                    Ok(SandboxOutput {
                        stdout,
                        stderr,
                        exit_code: output.status.code(),
                        signal: None,
                        truncated,
                        duration_ms: 0,
                    })
                }
                Ok(Err(e)) => Err(SandboxError::Io(format!("process error: {}", e))),
                Err(_) => Err(SandboxError::Timeout {
                    elapsed_ms: timeout.as_millis() as u64,
                }),
            }
        }
    }
}

/// 截断字节数组到最大长度
fn truncate_bytes(bytes: Vec<u8>, max_len: usize) -> Vec<u8> {
    if bytes.len() <= max_len {
        bytes
    } else {
        // 在UTF-8边界截断
        let mut cut = max_len;
        while cut > 0 && !is_utf8_boundary(bytes[cut]) {
            cut -= 1;
        }
        bytes[..cut].to_vec()
    }
}

fn is_utf8_boundary(byte: u8) -> bool {
    // UTF-8 continuation bytes start with 10xxxxxx
    (byte & 0b1100_0000) != 0b1000_0000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_driver_platform() {
        let driver = MacOSSandboxDriver::new();
        assert_eq!(driver.platform(), "macos/seatbelt");
    }

    #[test]
    fn truncate_bytes_short() {
        let bytes = b"hello".to_vec();
        let result = truncate_bytes(bytes.clone(), 100);
        assert_eq!(result, bytes);
    }

    #[test]
    fn truncate_bytes_long() {
        let bytes = b"hello world".to_vec();
        let result = truncate_bytes(bytes, 5);
        assert_eq!(result, b"hello");
    }
}
```

- [ ] **Step 3: 添加dirs依赖（如未存在）**

检查`Cargo.toml`：
```bash
grep -n '^dirs' Cargo.toml
```

如果不存在，添加：
```toml
dirs = "5.0"
```

- [ ] **Step 4: 验证编译和测试**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: `Finished dev`（可能有warning，但无error）

Run: `cargo test -p alephcore --lib sandbox::platforms::macos 2>&1 | tail -15`
Expected: macOS测试通过

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/platforms/macos/ Cargo.toml Cargo.lock
git commit -m "sandbox: implement enhanced macOS seatbelt policy generation

Phase 1 Task 4: Add MacOSSandboxDriver with fine-grained SBPL policy
generation including filesystem, network, process, and environment
controls. Supports workspace-only, path-based, and full-disk policies
with exclusions."
```

---

## Task 5: 更新factory使用平台驱动

**Files:**
- Modify: `src/sandbox/factory.rs`

**Context:** 修改`build_sandbox`使用新的`create_platform_driver()`。

- [ ] **Step 1: 修改`src/sandbox/factory.rs`**

```rust
//! Sandbox工厂 — 根据配置创建合适的sandbox实例

use std::sync::Arc;
use std::time::Duration;

use crate::sandbox::config::SandboxConfig;
use crate::sandbox::workspace::WorkspaceSandbox;
use crate::sandbox::Sandbox;
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::platforms::create_platform_driver;
use crate::sandbox::exec_approval::gate::ApprovalGate;

/// 创建生产环境sandbox
///
/// 根据当前平台自动选择合适的驱动
pub fn build_sandbox(
    cfg: &SandboxConfig,
    approval_gate: Arc<ApprovalGate>,
) -> Arc<dyn Sandbox> {
    if !cfg.enabled {
        return Arc::new(NoopSandbox);
    }

    let driver = create_platform_driver();
    
    // 如果平台不支持且配置要求sandbox，则使用NoopSandbox
    if !driver.is_supported() {
        tracing::warn!(
            "Platform sandbox not supported on this platform: {}",
            driver.platform()
        );
    }

    let ws = WorkspaceSandbox::new(
        cfg.workspace_root.clone(),
        driver,
        approval_gate,
    )
    .with_timeout(Duration::from_secs(cfg.default_timeout_seconds))
    .with_max_output_bytes(cfg.max_output_bytes);

    Arc::new(ws)
}

/// 使用指定驱动创建sandbox（用于测试）
pub fn build_sandbox_with_driver(
    cfg: &SandboxConfig,
    driver: Arc<dyn OsSandboxDriverTrait>,
    approval_gate: Arc<ApprovalGate>,
) -> Arc<dyn Sandbox> {
    let ws = WorkspaceSandbox::new(
        cfg.workspace_root.clone(),
        driver,
        approval_gate,
    )
    .with_timeout(Duration::from_secs(cfg.default_timeout_seconds))
    .with_max_output_bytes(cfg.max_output_bytes);

    Arc::new(ws)
}

/// 无操作sandbox — 用于禁用场景
pub struct NoopSandbox;

#[async_trait::async_trait]
impl Sandbox for NoopSandbox {
    async fn execute(
        &self,
        _command: crate::sandbox::command::SandboxCommand,
    ) -> Result<crate::sandbox::command::SandboxOutput, crate::sandbox::command::SandboxError> {
        Err(crate::sandbox::command::SandboxError::CapabilityDenied {
            reason: "sandbox is disabled".into(),
        })
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev`

- [ ] **Step 3: Commit**

```bash
git add src/sandbox/factory.rs
git commit -m "sandbox: update factory to use platform driver dispatch

Phase 1 Task 5: build_sandbox() now uses create_platform_driver() to
automatically select the appropriate platform implementation. Adds
build_sandbox_with_driver() for testing with custom drivers."
```

---

## Task 6: 迁移旧代码并标记deprecated

**Files:**
- Modify: `src/exec/sandbox/executor.rs`
- Modify: `src/exec/sandbox/mod.rs`

**Context:** 将旧`OsSandboxDriver`标记为deprecated，引导使用新实现。

- [ ] **Step 1: 在旧executor.rs顶部添加deprecated注释**

```rust
//! ⚠️ DEPRECATED: This module is being replaced by src/sandbox/platforms/macos/
//!
//! The new MacOSSandboxDriver in src/sandbox/platforms/macos/ provides
//! enhanced seatbelt policy generation. This module is kept temporarily
//! for backward compatibility during migration.
//!
//! TODO(Phase 4): Remove this module after all callers migrate.
```

- [ ] **Step 2: 在旧mod.rs中添加re-export警告**

```rust
//! ⚠️ DEPRECATED: src/exec/sandbox/ is being phased out.
//!
//! Use src/sandbox/ and src/sandbox/platforms/ instead.
//! TODO(Phase 4): Remove this module.

pub mod adapter;
// ... existing exports
```

- [ ] **Step 3: 验证无破坏**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev`（可能有deprecated warning）

Run: `cargo test -p alephcore --lib exec::sandbox 2>&1 | tail -10`
Expected: 现有测试仍通过

- [ ] **Step 4: Commit**

```bash
git add src/exec/sandbox/
git commit -m "sandbox: mark old exec/sandbox/ modules as deprecated

Phase 1 Task 6: Add deprecation notices to src/exec/sandbox/ modules.
New code should use src/sandbox/platforms/ instead. Old modules will
be removed in Phase 4 after migration is complete."
```

---

## Task 7: 添加跨平台集成测试

**Files:**
- Create: `tests/sandbox_cross_platform.rs`

**Context:** 验证平台分发和基本功能。

- [ ] **Step 1: 创建集成测试**

```rust
//! 跨平台sandbox集成测试

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alephcore::sandbox::capabilities::SandboxCapabilities;
use alephcore::sandbox::command::SandboxCommand;
use alephcore::sandbox::driver::OsSandboxDriverTrait;
use alephcore::sandbox::platforms::create_platform_driver;
use alephcore::sandbox::Sandbox;
use alephcore::sandbox::workspace::WorkspaceSandbox;

/// 测试平台驱动创建
#[test]
fn platform_driver_is_created() {
    let driver = create_platform_driver();
    let platform = driver.platform();
    
    // 验证平台标识符有效
    assert!(
        platform == "macos/seatbelt" 
            || platform == "linux/bwrap" 
            || platform == "windows/token"
            || platform == "unsupported"
    );
}

/// 测试WorkspaceSandbox基本功能（使用fake driver）
#[tokio::test]
async fn workspace_sandbox_basic_execution() {
    // 创建fake driver
    let fake_driver = Arc::new(FakeDriver::new());
    let workspace_root = tempfile::tempdir().unwrap();
    
    // 创建简单的ApprovalGate（自动拒绝）
    let approval_gate = Arc::new(
        alephcore::sandbox::exec_approval::gate::ApprovalGate::new(
            alephcore::sandbox::exec_approval::types::ApprovalConfig::default(),
            None,
        )
    );
    
    let sandbox = WorkspaceSandbox::new(
        workspace_root.path().to_path_buf(),
        fake_driver.clone(),
        approval_gate,
    );
    
    // 执行简单命令
    let cmd = SandboxCommand {
        session_id: alephcore::routing::session_key::SessionKey::ephemeral("test"),
        program: "echo".to_string(),
        args: vec!["hello".to_string()],
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities::strict(),
        timeout: Some(Duration::from_secs(5)),
    };
    
    let result = sandbox.execute(cmd).await;
    
    // FakeDriver返回成功
    assert!(result.is_ok());
    let output = result.unwrap();
    assert_eq!(output.exit_code, Some(0));
}

/// Fake driver用于测试
struct FakeDriver;

impl FakeDriver {
    fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl OsSandboxDriverTrait for FakeDriver {
    fn platform(&self) -> &'static str {
        "fake/test"
    }

    fn is_supported(&self) -> bool {
        true
    }

    fn profile_for(
        &self,
        _capabilities: &SandboxCapabilities,
        _cwd: &std::path::Path,
    ) -> Result<alephcore::sandbox::driver::OsSandboxProfile, alephcore::sandbox::command::SandboxError> {
        Ok(alephcore::sandbox::driver::OsSandboxProfile {
            contents: String::new(),
        })
    }

    async fn run(
        &self,
        _program: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<&[u8]>,
        _cwd: &std::path::Path,
        _profile: &alephcore::sandbox::driver::OsSandboxProfile,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> Result<alephcore::sandbox::command::SandboxOutput, alephcore::sandbox::command::SandboxError> {
        Ok(alephcore::sandbox::command::SandboxOutput {
            stdout: b"ok".to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            signal: None,
            truncated: false,
            duration_ms: 1,
        })
    }
}
```

- [ ] **Step 2: 验证测试编译**

Run: `cargo test --test sandbox_cross_platform 2>&1 | tail -10`
Expected: 测试编译并通过

- [ ] **Step 3: Commit**

```bash
git add tests/sandbox_cross_platform.rs
git commit -m "sandbox: add cross-platform integration tests

Phase 1 Task 7: Add integration tests for platform driver dispatch
and WorkspaceSandbox with fake driver. Tests verify basic execution
pipeline works across platforms."
```

---

## Task 8: 验证和清理

**Files:** 多个

**Context:** 确保所有测试通过，无回归。

- [ ] **Step 1: 运行完整测试套件**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: 无新增失败

- [ ] **Step 2: 检查编译警告**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`
Expected: 无新增warning（或仅deprecated相关）

- [ ] **Step 3: 验证旧代码引用**

Run: `grep -rn 'exec::sandbox' src/ | grep -v 'deprecated\|TODO' | head -10`
Expected: 仅executor.rs和mod.rs中的deprecated标记

- [ ] **Step 4: 最终Commit**

```bash
git add -A
git commit -m "sandbox: Phase 1 complete — macOS policy enhancement + platform architecture

Summary of changes:
- Enhanced OsSandboxDriverTrait with platform() and is_supported()
- Added SandboxPolicy unified policy representation
- Created src/sandbox/platforms/ directory structure
- Implemented MacOSSandboxDriver with fine-grained seatbelt policies
- Updated factory to use platform driver dispatch
- Marked old src/exec/sandbox/ as deprecated
- Added cross-platform integration tests

Next: Phase 2 — Linux bubblewrap + landlock/seccomp support"
```

---

## Exit Gate

- [ ] `cargo test -p alephcore --lib` 无新增失败
- [ ] `cargo clippy -p alephcore` 无新增error
- [ ] `src/sandbox/platforms/macos/` 存在且编译通过
- [ ] `src/exec/sandbox/` 标记为deprecated
- [ ] 新集成测试通过
- [ ] 文档已更新（如需要）

---

## Next Phase

Phase 2: Linux支持 — 实现`LinuxSandboxDriver` + `aleph-linux-sandbox` helper
