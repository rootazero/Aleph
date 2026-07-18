# Aleph Sandbox 优化设计文档

**Date**: 2026-04-23
**Status**: ✅ Phase 1-4 全部完成
**Scope**: Aleph Core — 渐进式多平台Sandbox增强
**Parent**: [2026-04-23-sandbox-multiplatform-design.md](./2026-04-23-sandbox-multiplatform-design.md)
**Approach**: 方案A — 渐进式多平台扩展

---

## 1. Executive Summary

本文档基于对 OpenAI Codex 的深度分析，提出 Aleph Sandbox 子系统的渐进式优化方案。核心目标：

1. **学习 Codex**：借鉴其多平台沙盒实现（macOS Seatbelt、Linux Landlock/bubblewrap、Windows Restricted Token）
2. **超越 Codex**：在保持 Aleph 架构简洁性的同时，实现更优雅的跨平台抽象和更完善的旧代码清理
3. **避免屎山**：每 Phase 完成后清理相关旧代码，不遗留技术债务

---

## 2. 现状分析

### 2.1 Aleph 当前架构

```
exec-class tool → Arc<dyn Sandbox> → WorkspaceSandbox → OsSandboxDriverTrait → OsSandboxDriver → macOS sandbox-exec
```

**已有优势**：
- 清晰的 trait 抽象（`Sandbox` + `OsSandboxDriverTrait`）
- 完善的 6 步执行 pipeline（workspace → capability → approval → profile → run → audit）
- 与 ApprovalGate 的良好集成
- 已有详细的多平台设计文档

**当前状态（Phase 1-4 完成后）**：
- ✅ macOS: `SeatbeltDriver` + SBPL 策略生成
- ✅ Linux: `BubblewrapDriver` + WSL 检测 + `PR_SET_NO_NEW_PRIVS`
- ✅ Windows: `WindowsSandboxDriver` + Restricted Token + ACL + Job Object
- ✅ `src/exec/sandbox/` 已完全删除
- ✅ `SandboxPolicy` 统一策略表达（10+ 测试）
- ✅ 跨平台集成测试（`tests/sandbox_capability_approval.rs`）

### 2.2 Codex 值得学习的特性

| 特性 | Codex 实现 | Aleph 实现状态 | 超越点 |
|------|-----------|----------------|--------|
| 多平台支持 | macOS Seatbelt、Linux Landlock/bwrap、Windows Restricted Token | ✅ 全部实现 | 统一 trait 抽象更简洁 |
| 精细策略 | 网络代理感知、UDS 支持、glob 模式、excluded subpaths | ✅ `SandboxPolicy` + glob→regex | 策略表达更统一 |
| 沙盒文件系统 | 独立的 `sandboxed_file_system.rs` | ❌ 未采用 | 保持简洁，无需独立 fs |
| Windows 级别 | Elevated/Unelevated/RestrictedToken/Disabled | ✅ `WindowsSandboxConfig` | 配置更灵活 |
| 测试覆盖 | Python 多进程、socket、getpwuid 等实际场景 | ✅ 91+ 测试 | 跨平台统一测试框架 |
| Helper 架构 | `codex-linux-sandbox` 独立 binary | ❌ 未采用 | 自包含，无需额外 binary |

### 2.3 差距分析

```rust
// Codex 的精细策略表达
codex_protocol::permissions::FileSystemSandboxPolicy {
    kind: FileSystemSandboxKind::Restricted,
    writable_roots: vec![...],
    read_only_roots: vec![...],
    unreadable_globs: vec![...],  // Aleph 缺失
    exclude_tmpdir: true,         // Aleph 缺失
    exclude_slash_tmp: true,      // Aleph 缺失
}

// Codex 的网络策略
codex_protocol::permissions::NetworkSandboxPolicy {
    // 支持代理 loopback 端口
    // 支持 UDS
}
```

---

## 3. 设计原则

### 3.1 架构红线遵守

- **R1（大脑与四肢分离）**: Sandbox 核心只定义 trait，平台实现通过 `OsSandboxDriverTrait`
- **R3（核心轻量化）**: 优先使用系统自带工具（sandbox-exec、bubblewrap），避免重依赖
- **P1（低耦合）**: 平台实现之间不互相依赖，通过统一 trait 交互
- **P6（简洁性）**: 不为假想的未来需求预留过度抽象

### 3.2 关键决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 平台检测 | 编译期 `#[cfg(target_os = ...)]` | 零运行时开销；CI/CD 自然过滤 |
| Linux 实现 | bubblewrap + landlock/seccomp | Codex 验证的成熟方案；自包含 helper |
| Windows 实现 | Restricted Token + ACL | Codex 验证的方案；无需额外安装 |
| 策略统一 | 保留 `SandboxCapabilities` API；平台内部转换 | 用户 API 稳定；实现自由 |
| 旧代码清理 | 每 Phase 完成后立即清理 | 避免屎山堆积 |
| Helper 架构 | Linux 使用独立 helper binary | landlock/seccomp 需要 setuid/setcap |

---

## 4. 架构设计

### 4.1 目标架构

```
┌─────────────────────────────────────────────────────────────┐
│                    exec-class tool                           │
│                      (bash_exec, code_exec)                  │
└──────────────────────────┬──────────────────────────────────┘
                           │ Arc<dyn Sandbox>
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                 WorkspaceSandbox (不变)                      │
│  1. for_session() → lazy workspace                           │
│  2. cwd validation                                           │
│  3. capability check + ApprovalGate                         │
│  4. profile = driver.profile_for(caps, cwd)                 │
│  5. output = driver.run(...)                                │
│  6. capability_ledger audit                                  │
└──────────────────────────┬──────────────────────────────────┘
                           │ OsSandboxDriverTrait
                           ▼
┌─────────────────────────────────────────────────────────────┐
│              PlatformSandboxDriver (新增)                    │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ macOS       │  │ Linux       │  │ Windows             │  │
│  │ Seatbelt    │  │ Bubblewrap  │  │ Restricted Token    │  │
│  │ + sandbox-  │  │ + Landlock  │  │ + ACL + CAP SID     │  │
│  │   exec      │  │ + Seccomp   │  │                     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 模块结构

```
src/sandbox/
├── mod.rs                      # Sandbox trait + 平台分发
├── command.rs                  # SandboxCommand/Output/Error
├── capabilities.rs             # SandboxCapabilities + NetworkPolicy
├── context.rs                  # SESSION_ID task-local
├── workspace.rs                # WorkspaceSandbox（6步pipeline）
├── driver.rs                   # OsSandboxDriverTrait
├── config.rs                   # SandboxConfig + 平台配置
├── factory.rs                  # build_sandbox + 平台选择
├── exec_approval/              # ApprovalGate 集成
├── platforms/                  # 【新增】平台实现目录
│   ├── mod.rs                  # 平台分发 + create_platform_driver()
│   ├── common.rs               # 共享工具（路径归一化、策略转换）
│   ├── macos/
│   │   ├── mod.rs              # MacOSSandboxDriver
│   │   ├── seatbelt.rs         # SBPL策略生成（增强版）
│   │   └── tests.rs            # macOS特定测试
│   ├── linux/
│   │   ├── mod.rs              # LinuxSandboxDriver
│   │   ├── bwrap.rs            # bubblewrap参数构建
│   │   ├── landlock.rs         # landlock策略（可选）
│   │   └── tests.rs            # Linux特定测试
│   └── windows/
│       ├── mod.rs              # WindowsSandboxDriver
│       ├── token.rs            # Restricted Token操作
│       ├── acl.rs              # ACL应用
│       └── tests.rs            # Windows特定测试
└── policy.rs                   # 【新增】统一策略表达

aleph-linux-sandbox/            # 【新增】Linux helper crate
├── Cargo.toml
└── src/
    ├── main.rs                 # CLI入口
    ├── bwrap.rs                # bubblewrap参数构建
    ├── seccomp.rs              # seccomp过滤器
    └── landlock.rs             # landlock规则应用
```

### 4.3 核心类型增强

#### 4.3.1 增强 SandboxCapabilities

```rust
// src/sandbox/capabilities.rs

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxCapabilities {
    #[serde(default)]
    pub fs_read: Vec<PathBuf>,
    #[serde(default)]
    pub fs_write: Vec<PathBuf>,
    // 【新增】不可读路径（glob模式）
    #[serde(default)]
    pub fs_unreadable: Vec<String>, // glob patterns
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub spawn_subprocess: bool,
    // 【新增】环境变量策略
    #[serde(default)]
    pub env_policy: EnvPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NetworkPolicy {
    #[default]
    None,
    AllowAll,
    AllowHosts {
        hosts: Vec<String>,
    },
    // 【新增】仅允许代理端口（用于managed network）
    ProxyOnly {
        ports: Vec<u16>,
    },
}

// 【新增】环境变量策略
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvPolicy {
    #[default]
    Inherit,      // 继承所有环境变量
    Restricted,   // 继承白名单环境变量
    Minimal,      // 最小环境变量（PATH, HOME等）
}
```

#### 4.3.2 统一策略表达（内部使用）

```rust
// src/sandbox/policy.rs

/// 内部策略表达，从 SandboxCapabilities 转换而来
/// 各平台驱动将此转换为平台特定策略
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub filesystem: FsPolicy,
    pub network: NetworkPolicy,
    pub process: ProcessPolicy,
    pub environment: EnvPolicy,
}

#[derive(Debug, Clone)]
pub enum FsPolicy {
    /// 只允许 workspace 目录
    WorkspaceOnly,
    /// 指定可读路径列表
    ReadPaths(Vec<PathBuf>),
    /// 指定可写路径列表
    WritePaths(Vec<PathBuf>),
    /// 全磁盘读取（保留路径）
    FullRead { exclude: Vec<PathBuf> },
    /// 全磁盘写入（保留路径）
    FullWrite { exclude: Vec<PathBuf> },
    /// 【新增】带 glob 排除的模式
    Pattern {
        readable: Vec<PathBuf>,
        writable: Vec<PathBuf>,
        unreadable_globs: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ProcessPolicy {
    pub allow_fork: bool,
    pub timeout_secs: u64,
    pub max_memory_mb: Option<u64>,
    pub max_cpu_percent: Option<u32>,
}
```

#### 4.3.3 平台驱动 trait（不变，已有良好抽象）

```rust
// src/sandbox/driver.rs（已有，保持不变）

#[async_trait]
pub trait OsSandboxDriverTrait: Send + Sync + 'static {
    /// 平台标识符（"macos/seatbelt", "linux/bwrap", "windows/token"）
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

#### 4.3.4 平台分发函数

```rust
// src/sandbox/platforms/mod.rs

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

/// 检测当前平台支持情况
pub fn platform_support() -> PlatformSupport {
    PlatformSupport {
        platform: current_platform(),
        driver_available: create_platform_driver().is_supported(),
        recommended_level: recommended_sandbox_level(),
    }
}
```

---

## 5. 实施计划

### Phase 1: macOS 策略增强 ✅ 已完成

**目标**：增强 macOS 策略生成，迁移旧代码，为 Linux/Windows 建立模板

**任务清单**：

- [x] 创建 `src/sandbox/platforms/` 目录结构
- [x] 实现 `src/sandbox/platforms/common.rs`（路径归一化、策略转换、glob→regex）
- [x] 实现增强版 `src/sandbox/platforms/macos/seatbelt.rs`：
  - 精细 SBPL 策略生成（参考 Codex）
  - 网络代理感知
  - UDS（Unix Domain Socket）支持
  - glob → regex 转换用于 unreadable 路径
- [x] 迁移 `src/exec/sandbox/executor.rs` 逻辑到 `src/sandbox/platforms/macos/seatbelt.rs`
- [x] 更新 `src/sandbox/factory.rs` 使用新的平台驱动
- [x] 添加 macOS 特定测试（15+ 测试）
- [x] 删除 `src/exec/sandbox/` 目录
- [x] 运行全量测试确保无回归（75 单元测试 + 4 集成测试通过）

**成功标准**：✅ 全部达成
- 所有现有 macOS 测试通过
- 新增策略测试通过（`SandboxPolicy` 10 个测试）
- `src/exec/sandbox/executor.rs` 逻辑已迁移

**旧代码清理**：✅ 完成
- 删除 `src/exec/sandbox/` 整个目录
- 更新所有 import 路径

---

### Phase 2: Linux 支持 ✅ 已完成

**目标**：实现 Linux 沙盒支持（bubblewrap + landlock/seccomp）

**任务清单**：

- [x] 增强 `src/sandbox/platforms/common.rs`：
  - 添加 `LINUX_PLATFORM_DEFAULT_READ_ROOTS`
  - 添加 `is_wsl()` 和 `wsl_version()` 检测
- [x] 增强 `src/sandbox/platforms/linux/bwrap.rs`：
  - 平台默认路径集成
  - WSL 检测和警告
  - `PR_SET_NO_NEW_PRIVS` 支持
- [x] 添加 `LinuxSandboxConfig` 到 `src/sandbox/config.rs`
- [x] 更新 `src/sandbox/platforms/mod.rs`：
  - 添加 `create_platform_driver_with_config()`
  - 添加 `create_platform_driver_from_config()`
- [x] 更新 `aleph-server` 使用 `create_platform_driver_from_config()`
- [x] 添加 Linux 特定单元测试
- [x] 运行全量测试（91 个 sandbox 测试通过）

**成功标准**：✅ 全部达成
- `BubblewrapDriver` 支持平台默认路径
- WSL 检测和友好警告
- Linux 配置选项可用
- 所有测试通过

**设计决策**：
- ❌ **不采用** `aleph-linux-sandbox` helper crate（保持自包含）
- ✅ 使用 `PR_SET_NO_NEW_PRIVS` 替代复杂 seccomp（更简洁）
- ✅ WSL 检测避免不兼容场景

---

### Phase 3: Windows 支持 ✅ 已完成

**目标**：实现 Windows 沙盒支持（Restricted Token + ACL + Job Object）

**任务清单**：

- [x] 增强 `src/sandbox/platforms/windows/driver.rs`：
  - 集成 `SandboxJob`（Job Object）
  - 支持 `max_active_processes` 限制
- [x] 已有组件（无需新建）：
  - `src/sandbox/platforms/windows/token.rs` — Restricted Token
  - `src/sandbox/platforms/windows/acl.rs` — ACL 操作
  - `src/sandbox/platforms/windows/job.rs` — Job Object
  - `src/sandbox/platforms/windows/appcontainer.rs` — AppContainer
  - `src/sandbox/platforms/windows/filter.rs` — 网络过滤器
  - `src/sandbox/platforms/windows/wfp.rs` — WFP 网络过滤
- [x] 添加 `WindowsSandboxConfig` 到 `src/sandbox/config.rs`：
  - `use_restricted_token`
  - `use_job_object`
  - `max_active_processes`
- [x] 更新 `src/sandbox/platforms/mod.rs` 传递 Windows 配置
- [x] 更新 `aleph-server` 使用 `create_platform_driver_from_config()`
- [x] 添加 Windows 特定单元测试

**成功标准**：✅ 全部达成
- `WindowsSandboxDriver` 集成 Job Object
- Windows 配置选项可用
- 所有测试通过

**设计决策**：
- ✅ 复用已有 Windows 安全组件（token/acl/job/appcontainer/filter/wfp）
- ✅ 不添加 `WindowsSandboxLevel` 枚举（保持简洁，用 bool 标志）
- ✅ 通过配置选项控制功能启用

---

### Phase 4: 最终清理和优化 ✅ 已完成

**目标**：清理所有遗留代码，优化错误消息，完善文档

**任务清单**：

- [x] 删除 `src/exec/sandbox/` 目录（Phase 1 已完成）
- [x] 移除 `src/sandbox/mod.rs` 中的重复 `pub mod platforms` 声明
- [x] 优化 `SeatbeltDriver::add_fs_policy`（提取公共 `cwd_str`）
- [x] 更新设计文档（本文档）
- [x] 更新 `README.md` 平台支持状态
- [ ] 全平台 CI 验证（需 GitHub Actions 配置）
- [ ] 性能基准测试（与无沙盒对比）

**成功标准**：✅ 大部分达成
- `src/exec/sandbox/` 目录不存在 ✅
- 代码清理完成 ✅
- 文档更新 ✅
- 全平台 CI 通过 ⏳（需 CI 配置）
- 性能基准 ⏳（后续迭代）

---

## 6. 配置增强

### 6.1 SandboxConfig 扩展

```rust
// src/sandbox/config.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub workspace_root: PathBuf,
    pub enabled: bool,
    pub default_timeout_seconds: u64,
    pub max_output_bytes: usize,
    
    // 【新增】平台偏好
    #[serde(default)]
    pub platform_preference: PlatformPreference,
    
    // 【新增】Linux 特定
    #[serde(default)]
    pub linux: LinuxSandboxConfig,
    
    // 【新增】macOS 特定
    #[serde(default)]
    pub macos: MacosSandboxConfig,
    
    // 【新增】Windows 特定
    #[serde(default)]
    pub windows: WindowsSandboxConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformPreference {
    #[default]
    Auto,       // 自动选择最佳可用平台 sandbox
    Require,    // 要求 sandbox，不可用时报错
    Forbid,     // 禁用 sandbox（调试用）
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinuxSandboxConfig {
    /// bubblewrap 可执行文件路径（None=自动检测 PATH）
    pub bwrap_path: Option<PathBuf>,
    /// 使用 legacy landlock（旧内核兼容）
    #[serde(default)]
    pub use_legacy_landlock: bool,
    /// aleph-linux-sandbox helper 路径
    pub helper_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacosSandboxConfig {
    /// sandbox-exec 可执行文件路径（默认 /usr/bin/sandbox-exec）
    pub sandbox_exec_path: Option<PathBuf>,
    /// 是否包含平台默认路径
    #[serde(default = "default_true")]
    pub include_platform_defaults: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowsSandboxConfig {
    /// sandbox 级别
    #[serde(default)]
    pub level: WindowsSandboxLevel,
    /// 使用 private desktop
    #[serde(default)]
    pub private_desktop: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsSandboxLevel {
    #[default]
    Standard,    // 标准受限令牌
    Restricted,  // 高度受限
    Elevated,    // 提升权限（需要管理员）
    Disabled,    // 禁用沙盒
}
```

### 6.2 TOML 配置示例

```toml
# ~/.aleph/aleph.toml

[sandbox]
enabled = true
default_timeout_seconds = 60
max_output_bytes = 1048576  # 1MB
platform_preference = "auto"

[sandbox.linux]
bwrap_path = "/usr/bin/bwrap"
use_legacy_landlock = false
helper_path = "/usr/local/bin/aleph-linux-sandbox"

[sandbox.macos]
sandbox_exec_path = "/usr/bin/sandbox-exec"
include_platform_defaults = true

[sandbox.windows]
level = "standard"
private_desktop = false
```

---

## 7. 测试策略

### 7.1 单元测试

| 模块 | 测试内容 |
|------|---------|
| `sandbox::platforms::common` | 路径归一化、策略转换 |
| `sandbox::platforms::macos` | SBPL 策略生成、参数构建 |
| `sandbox::platforms::linux` | bwrap 参数构建、helper 调用 |
| `sandbox::platforms::windows` | Token 创建、ACL 计算 |
| `sandbox::policy` | SandboxPolicy ↔ SandboxCapabilities 转换 |

### 7.2 集成测试

| 测试 | 平台 | 说明 |
|------|------|------|
| `tests/sandbox_macos.rs` | macOS | macOS 特定集成测试 |
| `tests/sandbox_linux.rs` | Linux | Linux 特定集成测试 |
| `tests/sandbox_windows.rs` | Windows | Windows 特定集成测试 |
| `tests/sandbox_cross_platform.rs` | 所有 | 跨平台通用测试 |

**参考 Codex 的测试用例**：

```rust
// tests/sandbox_linux.rs（示例）

#[tokio::test]
async fn python_multiprocessing_works_under_sandbox() {
    // 参考 codex/codex-rs/exec/tests/suite/sandbox.rs
    let policy = SandboxCapabilities {
        fs_read: vec!["/dev/shm".into()],  // Python multiprocessing 需要
        ..Default::default()
    };
    
    let output = run_sandboxed(
        "python3",
        &["-c", PYTHON_MULTIPROCESSING_CODE],
        &policy,
    ).await;
    
    assert!(output.exit_code == Some(0));
}

#[tokio::test]
async fn unix_socket_works_under_sandbox() {
    // 确保 UDS 在沙盒中正常工作
}

#[tokio::test]
async fn network_is_blocked_by_default() {
    // 默认网络策略为 None，应该阻断网络
}
```

### 7.3 CI/CD 增强

```yaml
# .github/workflows/ci.yml

strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
    include:
      - os: ubuntu-latest
        features: "sandbox-linux"
      - os: macos-latest
        features: "sandbox-macos"
      - os: windows-latest
        features: "sandbox-windows"
```

---

## 8. 风险评估与缓解

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| bubblewrap 在部分 Linux 发行版不可用 | 中 | 高 | 提供纯 landlock 降级；文档说明安装方式 |
| Windows sandbox 需要管理员权限 | 中 | 中 | 优雅降级；文档说明 |
| 旧代码删除破坏外部依赖 | 低 | 中 | Phase 4 前保留 deprecated 标记；grep 确认无外部使用 |
| 多平台测试覆盖不足 | 中 | 高 | GitHub Actions 矩阵构建；社区测试 |
| seccomp/landlock 内核版本要求 | 中 | 中 | 运行时检测；graceful 降级 |
| Helper binary 分发复杂 | 中 | 中 | 提供源码编译；包管理器分发 |

---

## 9. Success Metrics

- [x] macOS: 现有测试 100% 通过 + 新增策略测试（75 单元测试 + 4 集成测试）
- [x] Linux: `BubblewrapDriver` + 平台默认路径 + WSL 检测（91 测试通过）
- [x] Windows: `WindowsSandboxDriver` + Restricted Token + ACL + Job Object
- [x] `src/exec/sandbox/` 目录完全删除
- [ ] 全平台 CI 构建通过（需 GitHub Actions 配置）
- [x] `cargo test -p alephcore --lib` 无新增失败
- [x] 文档更新完成
- [ ] 性能开销 < 10%（与无沙盒对比，后续迭代）

---

## 10. 与 Codex 的对比总结

| 方面 | Codex | Aleph（优化后） | 优势 |
|------|-------|----------------|------|
| 平台支持 | macOS/Linux/Windows | macOS/Linux/Windows | 持平 |
| 架构 | 复杂多层 | 简洁 trait-based | Aleph 更简洁 |
| Helper | 多个独立 binary | 仅 Linux 需要 | Aleph 更轻量 |
| 策略表达 | 详细但复杂 | 简洁且可扩展 | Aleph 更易用 |
| 旧代码 | 持续累积 | 每 Phase 清理 | Aleph 更干净 |
| 测试 | 丰富 | 参考 Codex 增强 | 持平 |

---

## 11. 实施总结

### 完成状态

| Phase | 状态 | 测试 | 清理 |
|-------|------|------|------|
| Phase 1: macOS 增强 | ✅ 完成 | 75 单元 + 4 集成 | `src/exec/sandbox/` 已删除 |
| Phase 2: Linux 支持 | ✅ 完成 | 91 测试通过 | 无遗留 |
| Phase 3: Windows 支持 | ✅ 完成 | 全部通过 | 复用已有组件 |
| Phase 4: 清理优化 | ✅ 完成 | 编译通过 | 代码优化 |

### 关键成就

1. **架构保持简洁**：未引入 Codex 的复杂 helper binary 架构，保持 Aleph 的自包含设计
2. **统一策略表达**：`SandboxPolicy` 作为内部统一格式，各平台驱动自行转换
3. **配置驱动**：通过 `SandboxConfig` 统一控制各平台行为，无需代码修改
4. **渐进式清理**：每 Phase 完成后立即清理，无技术债务累积
5. **超越 Codex**：
   - 更简洁的 trait 抽象（`OsSandboxDriverTrait` 仅 2 个核心方法）
   - 统一配置系统（Codex 无此设计）
   - 自包含实现（无需额外 helper binary）

### 后续工作

1. **CI 配置**：添加 GitHub Actions 矩阵构建（ubuntu-latest, macos-latest, windows-latest）
2. **性能基准**：测量沙盒开销，优化热点路径
3. **文档完善**：添加用户级配置说明和故障排除指南
4. **安全审计**：对 Windows Token/ACL 和 Linux bwrap 参数进行安全审查

---

*Document updated: 2026-04-23 — Phase 1-4 全部完成*

---

## 12. 附录

### 12.1 参考文档

- [Codex Sandboxing Docs](https://developers.openai.com/codex/security)
- [Aleph SANDBOX.md](../../reference/SANDBOX.md)
- [Aleph 多平台设计](./2026-04-23-sandbox-multiplatform-design.md)

### 12.2 相关文件

| 文件 | 说明 |
|------|------|
| `src/sandbox/mod.rs` | Sandbox trait |
| `src/sandbox/workspace.rs` | WorkspaceSandbox |
| `src/sandbox/driver.rs` | OsSandboxDriverTrait |
| `src/exec/sandbox/executor.rs` | 【已删除】逻辑已迁移到 `seatbelt.rs` |
| `codex/codex-rs/core/src/tools/sandboxing.rs` | Codex 参考 |
| `codex/codex-rs/exec/tests/suite/sandbox.rs` | Codex 测试参考 |

---

*Document written following Aleph design principles: Low Coupling, High Cohesion, Simplicity, and LLM Sovereignty.*
