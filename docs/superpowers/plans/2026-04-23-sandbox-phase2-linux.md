# Phase 2: Linux Sandbox 支持 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 Linux 沙盒支持，包括 bubblewrap 驱动、平台默认路径、WSL 检测、以及可选的 landlock/seccomp 增强。

**Architecture:** 在 Phase 1 建立的 `src/sandbox/platforms/` 架构基础上，增强 `BubblewrapDriver`，添加 Linux 平台默认路径、WSL 检测、以及更精细的 bwrap 参数生成。参考 Codex 的 `codex-linux-sandbox` 实现，但保持 Aleph 的简洁性。

**Tech Stack:** Rust, tokio, bubblewrap (bwrap), landlock (可选), seccomp (可选)

**Source spec:** `docs/superpowers/specs/2026-04-23-sandbox-multiplatform-design.md` §7.2

---

## File Structure

**Modified:**
- `src/sandbox/platforms/linux/bwrap.rs` — 增强 `BubblewrapDriver`
- `src/sandbox/platforms/linux/mod.rs` — 导出增强的驱动
- `src/sandbox/platforms/common.rs` — 添加 Linux 平台默认路径
- `src/sandbox/config.rs` — 添加 `LinuxSandboxConfig`

**Created:**
- `tests/sandbox_linux.rs` — Linux 特定集成测试

---

## Task 1: 增强 Linux 平台默认路径

**Files:**
- Modify: `src/sandbox/platforms/common.rs`

**Context:** Linux 进程需要访问系统库、二进制文件等。参考 Codex 的 `LINUX_PLATFORM_DEFAULT_READ_ROOTS`。

- [ ] **Step 1: 添加 Linux 平台默认路径常量**

在 `src/sandbox/platforms/common.rs` 中添加：

```rust
/// Linux 平台默认可读路径
/// 这些路径包含系统库、二进制文件和动态链接器
pub const LINUX_PLATFORM_DEFAULT_READ_ROOTS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/lib",
    "/lib64",
    "/nix/store",
    "/run/current-system/sw",
];

/// 检测是否在 WSL 环境中运行
pub fn is_wsl() -> bool {
    // WSL 在 /proc/version 中包含 "Microsoft" 或 "microsoft"
    std::fs::read_to_string("/proc/version")
        .map(|content| {
            content.to_lowercase().contains("microsoft")
        })
        .unwrap_or(false)
}

/// 检测 WSL 版本 (1 或 2)
pub fn wsl_version() -> Option<u32> {
    if !is_wsl() {
        return None;
    }
    
    // WSL1: /proc/sys/kernel/osrelease 不包含 "WSL2"
    // WSL2: 包含 "WSL2"
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .and_then(|content| {
            if content.contains("WSL2") {
                Some(2)
            } else {
                Some(1)
            }
        })
}
```

- [ ] **Step 2: 添加测试**

```rust
#[test]
fn linux_platform_defaults_not_empty() {
    assert!(!LINUX_PLATFORM_DEFAULT_READ_ROOTS.is_empty());
    assert!(LINUX_PLATFORM_DEFAULT_READ_ROOTS.contains(&"/usr"));
    assert!(LINUX_PLATFORM_DEFAULT_READ_ROOTS.contains(&"/bin"));
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p alephcore`
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add src/sandbox/platforms/common.rs
git commit -m "sandbox: add Linux platform defaults and WSL detection utilities"
```

---

## Task 2: 增强 BubblewrapDriver

**Files:**
- Modify: `src/sandbox/platforms/linux/bwrap.rs`

**Context:** 当前 `BubblewrapDriver` 已实现基本功能，需要增强：
1. 添加 Linux 平台默认路径绑定
2. 改进 WSL 检测和错误提示
3. 添加 `PR_SET_NO_NEW_PRIVS` 支持
4. 改进参数生成逻辑

- [ ] **Step 1: 读取当前 `bwrap.rs` 内容**

- [ ] **Step 2: 修改 `generate_args` 添加平台默认路径**

在 `add_fs_args` 之后，添加平台默认路径：

```rust
fn add_platform_default_args(
    &self,
    args: &mut Vec<String>,
) {
    use crate::sandbox::platforms::common::LINUX_PLATFORM_DEFAULT_READ_ROOTS;
    
    for path in LINUX_PLATFORM_DEFAULT_READ_ROOTS {
        if Path::new(path).exists() {
            args.push("--ro-bind".into());
            args.push(path.to_string());
            args.push(path.to_string());
        }
    }
}
```

- [ ] **Step 3: 在 `generate_args` 中调用平台默认路径**

在 `self.add_fs_args(&mut args, &policy.filesystem, cwd)?;` 之后添加：

```rust
self.add_platform_default_args(&mut args);
```

- [ ] **Step 4: 添加 WSL 检测和警告**

在 `is_supported()` 方法中添加 WSL 检测：

```rust
fn is_supported(&self) -> bool {
    if let Some(version) = crate::sandbox::platforms::common::wsl_version() {
        if version == 1 {
            tracing::warn!(
                "WSL1 detected: bubblewrap sandboxing is not supported. \
                 Consider upgrading to WSL2 or using a native Linux environment."
            );
            return false;
        }
    }
    self.find_bwrap().is_some()
}
```

- [ ] **Step 5: 改进 `run` 方法添加 `PR_SET_NO_NEW_PRIVS`**

在 `run` 方法开始处添加：

```rust
// 设置 PR_SET_NO_NEW_PRIVS 防止特权提升
#[cfg(target_os = "linux")]
unsafe {
    libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
}
```

注意：需要添加 `libc` 依赖（如果未存在）。

- [ ] **Step 6: 验证编译**

Run: `cargo check -p alephcore`
Expected: 编译通过

- [ ] **Step 7: Commit**

```bash
git add src/sandbox/platforms/linux/bwrap.rs
git commit -m "sandbox: enhance BubblewrapDriver with platform defaults and WSL detection"
```

---

## Task 3: 添加 Linux 配置选项

**Files:**
- Modify: `src/sandbox/config.rs`

**Context:** 允许用户配置 Linux 特定的沙盒选项。

- [ ] **Step 1: 读取当前 `config.rs` 内容**

- [ ] **Step 2: 添加 `LinuxSandboxConfig`**

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinuxSandboxConfig {
    /// bubblewrap 可执行文件路径（None=自动检测 PATH）
    pub bwrap_path: Option<PathBuf>,
    /// 使用 legacy landlock（旧内核兼容）
    #[serde(default)]
    pub use_legacy_landlock: bool,
    /// 是否包含平台默认路径
    #[serde(default = "default_true")]
    pub include_platform_defaults: bool,
}
```

- [ ] **Step 3: 在 `SandboxConfig` 中添加 linux 字段**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    // ... 现有字段 ...
    
    // 【新增】Linux 特定
    #[serde(default)]
    pub linux: LinuxSandboxConfig,
}
```

- [ ] **Step 4: 更新 `BubblewrapDriver` 使用配置**

修改 `BubblewrapDriver` 结构体：

```rust
#[derive(Debug, Clone)]
pub struct BubblewrapDriver {
    config: LinuxSandboxConfig,
}

impl BubblewrapDriver {
    pub fn new(config: LinuxSandboxConfig) -> Self {
        Self { config }
    }
    
    fn find_bwrap(&self) -> Option<PathBuf> {
        // 优先使用配置中的路径
        if let Some(ref path) = self.config.bwrap_path {
            if path.is_file() {
                return Some(path.clone());
            }
        }
        // ... 现有逻辑 ...
    }
}
```

- [ ] **Step 5: 验证编译**

Run: `cargo check -p alephcore`
Expected: 编译通过

- [ ] **Step 6: Commit**

```bash
git add src/sandbox/config.rs src/sandbox/platforms/linux/bwrap.rs
git commit -m "sandbox: add LinuxSandboxConfig for platform-specific settings"
```

---

## Task 4: 添加 Linux 集成测试

**Files:**
- Create: `tests/sandbox_linux.rs`

**Context:** 验证 Linux 沙盒功能。

- [ ] **Step 1: 创建测试文件**

```rust
//! Linux sandbox integration tests.
//!
//! These tests verify bubblewrap-based sandboxing on Linux.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alephcore::sandbox::{
    build_sandbox, BubblewrapDriver, LinuxSandboxConfig, NetworkPolicy,
    OsSandboxDriverTrait, Sandbox, SandboxCapabilities, SandboxCommand,
    SandboxConfig, SandboxError,
};
use alephcore::sandbox::exec_approval::{
    ApprovalConfig, ApprovalGate, ApprovalOutcome, ApprovalRequester,
};
use alephcore::routing::session_key::SessionKey;

fn test_session() -> SessionKey {
    SessionKey::ephemeral("linux-sandbox-test")
}

fn make_gate() -> Arc<ApprovalGate> {
    Arc::new(ApprovalGate::new(ApprovalConfig::default(), None))
}

/// Test that BubblewrapDriver correctly identifies its platform.
#[test]
fn bubblewrap_driver_platform_tag() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    assert_eq!(driver.platform(), "linux/bwrap");
}

/// Test that the driver detects bwrap availability.
#[test]
fn bubblewrap_driver_supports_when_bwrap_present() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    // On Linux with bwrap installed, this should be true.
    // On other platforms or without bwrap, false.
    let has_bwrap = std::process::Command::new("which")
        .arg("bwrap")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    
    assert_eq!(driver.is_supported(), has_bwrap);
}

/// Test generating a profile for strict capabilities.
#[test]
fn generate_profile_strict() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    let caps = SandboxCapabilities::strict();
    let cwd = Path::new("/tmp/test-workspace");
    
    let profile = driver.profile_for(&caps, cwd).unwrap();
    assert!(profile.contents.contains("--new-session"));
    assert!(profile.contents.contains("--unshare-user"));
    assert!(profile.contents.contains("--unshare-net"));
    assert!(profile.contents.contains("--bind"));
    assert!(profile.contents.contains("/tmp/test-workspace"));
}

/// Test generating a profile with network access.
#[test]
fn generate_profile_with_network() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    let caps = SandboxCapabilities {
        network: NetworkPolicy::AllowAll,
        ..Default::default()
    };
    let cwd = Path::new("/tmp/test-workspace");
    
    let profile = driver.profile_for(&caps, cwd).unwrap();
    // AllowAll should NOT include --unshare-net
    assert!(!profile.contents.contains("--unshare-net"));
}

/// Test generating a profile with read paths.
#[test]
fn generate_profile_with_read_paths() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    let caps = SandboxCapabilities {
        fs_read: vec!["/etc".into()],
        ..Default::default()
    };
    let cwd = Path::new("/tmp/test-workspace");
    
    let profile = driver.profile_for(&caps, cwd).unwrap();
    assert!(profile.contents.contains("--ro-bind"));
    assert!(profile.contents.contains("/etc"));
}

/// Test WSL detection.
#[test]
fn wsl_detection() {
    use alephcore::sandbox::platforms::common::{is_wsl, wsl_version};
    
    // These should not panic, regardless of platform
    let _ = is_wsl();
    let _ = wsl_version();
}

/// Integration test: execute a simple command under sandbox.
/// This test is skipped if bwrap is not available.
#[tokio::test]
async fn sandbox_executes_echo() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    if !driver.is_supported() {
        eprintln!("Skipping test: bwrap not available");
        return;
    }
    
    let tmp = tempfile::tempdir().unwrap();
    let cfg = SandboxConfig {
        workspace_root: tmp.path().to_path_buf(),
        enabled: true,
        default_timeout_seconds: 30,
        max_output_bytes: 4096,
        linux: LinuxSandboxConfig::default(),
    };
    
    let sandbox = build_sandbox(
        &cfg,
        Arc::new(driver),
        make_gate(),
    );
    
    let output = sandbox
        .execute(SandboxCommand {
            session_id: test_session(),
            program: "/bin/echo".into(),
            args: vec!["hello".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::strict(),
            timeout: Some(Duration::from_secs(5)),
        })
        .await
        .expect("sandbox should execute echo");
    
    assert_eq!(output.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
}

/// Integration test: network is blocked by default.
#[tokio::test]
async fn sandbox_blocks_network_by_default() {
    let driver = BubblewrapDriver::new(LinuxSandboxConfig::default());
    if !driver.is_supported() {
        eprintln!("Skipping test: bwrap not available");
        return;
    }
    
    let tmp = tempfile::tempdir().unwrap();
    let cfg = SandboxConfig {
        workspace_root: tmp.path().to_path_buf(),
        enabled: true,
        default_timeout_seconds: 30,
        max_output_bytes: 4096,
        linux: LinuxSandboxConfig::default(),
    };
    
    let sandbox = build_sandbox(
        &cfg,
        Arc::new(driver),
        make_gate(),
    );
    
    // Try to curl with strict capabilities (no network)
    let output = sandbox
        .execute(SandboxCommand {
            session_id: test_session(),
            program: "/usr/bin/curl".into(),
            args: vec![
                "--connect-timeout".into(),
                "2".into(),
                "https://example.com".into(),
            ],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::strict(),
            timeout: Some(Duration::from_secs(10)),
        })
        .await
        .expect("sandbox should execute curl");
    
    // curl should fail because network is blocked
    assert_ne!(output.exit_code, Some(0));
}
```

- [ ] **Step 2: 验证测试编译**

Run: `cargo test --test sandbox_linux --no-run`
Expected: 编译通过

- [ ] **Step 3: Commit**

```bash
git add tests/sandbox_linux.rs
git commit -m "sandbox: add Linux integration tests"
```

---

## Task 5: 更新平台分发函数

**Files:**
- Modify: `src/sandbox/platforms/mod.rs`

**Context:** 确保 `create_platform_driver()` 正确传递 Linux 配置。

- [ ] **Step 1: 修改 `create_platform_driver` 传递配置**

```rust
pub fn create_platform_driver() -> Arc<dyn OsSandboxDriverTrait> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::seatbelt::SeatbeltDriver::new())
    }
    #[cfg(target_os = "linux")]
    {
        // TODO: 从配置中读取 LinuxSandboxConfig
        Arc::new(linux::bwrap::BubblewrapDriver::new(
            crate::sandbox::config::LinuxSandboxConfig::default()
        ))
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::driver::WindowsSandboxDriver::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Arc::new(UnsupportedDriver)
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p alephcore`
Expected: 编译通过

- [ ] **Step 3: Commit**

```bash
git add src/sandbox/platforms/mod.rs
git commit -m "sandbox: update platform driver dispatch for Linux config"
```

---

## Task 6: 最终验证

- [ ] **Step 1: 运行所有 sandbox 测试**

Run: `cargo test -p alephcore --lib sandbox`
Expected: 所有测试通过

- [ ] **Step 2: 运行 Linux 集成测试**

Run: `cargo test --test sandbox_linux`
Expected: 测试通过（在有 bwrap 的 Linux 上）

- [ ] **Step 3: 检查编译**

Run: `cargo check -p alephcore`
Expected: 无错误

- [ ] **Step 4: 更新文档**

在 `docs/reference/SANDBOX.md` 中添加 Phase 2 说明：

```markdown
## Phase 2: Linux 支持 (2026-04-23)

- 实现 `BubblewrapDriver` (bubblewrap-based sandboxing)
- 添加 Linux 平台默认路径 (/usr, /bin, /lib 等)
- 添加 WSL 检测 (WSL1 不支持, WSL2 支持)
- 添加 `LinuxSandboxConfig` 配置选项
- 添加 Linux 集成测试
```

- [ ] **Step 5: Commit**

```bash
git add docs/reference/SANDBOX.md
git commit -m "docs: update SANDBOX.md with Phase 2 completion"
```

---

## Success Criteria

- [ ] `cargo check -p alephcore` 编译通过
- [ ] `cargo test -p alephcore --lib sandbox` 所有测试通过
- [ ] `cargo test --test sandbox_linux` 测试通过（在有 bwrap 的系统上）
- [ ] `BubblewrapDriver` 支持平台默认路径
- [ ] WSL 检测正常工作
- [ ] 配置系统支持 `LinuxSandboxConfig`

---

## Next Phase

Phase 3: Windows 支持 — 实现 `WindowsSandboxDriver` (Restricted Token + ACL)。

---

*Plan created following Aleph design principles: Low Coupling, High Cohesion, Simplicity.*
