# Aleph Sandbox多平台增强设计

**Date**: 2026-04-23
**Status**: Design approved
**Scope**: Aleph Core — 增强`src/sandbox/`模块，添加Linux和Windows平台支持，清理旧代码
**Parent**: [2026-04-19-sandbox-workspace-design.md](./2026-04-19-sandbox-workspace-design.md)

---

## 1. Goal

在保持现有解耦架构（Harness + Sandbox + Orchestrator + Tools + Session）的前提下，增强Aleph Sandbox子系统：

1. **多平台支持**：从macOS-only扩展到Linux（bubblewrap + landlock/seccomp）和Windows（restricted token + ACL）
2. **策略增强**：借鉴codex的精细策略生成（网络代理、UDS、glob模式、excluded subpaths）
3. **旧代码清理**：逐步迁移并删除`src/exec/sandbox/`遗留代码
4. **保持解耦**：Sandbox trait和WorkspaceSandbox架构不变

## 2. Non-Goals

- 不改`Sandbox` trait签名（单方法`execute`足够好）
- 不改`WorkspaceSandbox`的6步pipeline
- 不改`ApprovalGate`集成方式
- 不引入容器/runtime依赖（保持轻量）
- 不做VM-level隔离（超出scope）

## 3. 当前状态分析

### 3.1 已有架构（Phase 3完成）

```
exec-class tool → Arc<dyn Sandbox> → WorkspaceSandbox → OsSandboxDriverTrait → OsSandboxDriver → macOS sandbox-exec
```

**已有文件：**
- `src/sandbox/mod.rs` — Sandbox trait + MockSandbox
- `src/sandbox/command.rs` — SandboxCommand/Output/Error
- `src/sandbox/capabilities.rs` — SandboxCapabilities + NetworkPolicy
- `src/sandbox/context.rs` — SESSION_ID task-local
- `src/sandbox/workspace.rs` — WorkspaceSandbox（6步pipeline）
- `src/sandbox/driver.rs` — OsSandboxDriverTrait
- `src/sandbox/factory.rs` — build_sandbox + NoopSandbox
- `src/sandbox/config.rs` — SandboxConfig
- `src/exec/sandbox/executor.rs` — OsSandboxDriver（macOS实现）

### 3.2 遗留问题

| 问题 | 位置 | 严重程度 |
|------|------|---------|
| 旧Capabilities桥接 | `src/exec/sandbox/capabilities.rs` + `bridge_capabilities()` | 高 |
| 旧SandboxAdapter trait | `src/exec/sandbox/adapter.rs` | 高 |
| 旧ProfileGenerator | `src/exec/sandbox/profile.rs` | 中 |
| 旧SandboxCommand/SandboxProfile | `src/exec/sandbox/adapter.rs` | 高 |
| stdin不支持 | `OsSandboxDriver::run()` 注释 | 中 |
| FallbackPolicy未实现 | `src/exec/sandbox/executor.rs` | 中 |
| Linux/Windows平台缺失 | `src/exec/sandbox/platforms/` | 高 |

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

### 4.2 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 平台检测 | 编译期`#[cfg(target_os = ...)]` | 零运行时开销；CI/CD按平台构建自然过滤 |
| Linux实现 | bubblewrap + landlock/seccomp helper | bubblewrap做fs隔离成熟可靠；自包含helper降低依赖 |
| Windows实现 | Restricted Token + ACL | codex验证过的方案；不需要额外安装 |
| 策略统一 | 保留`SandboxCapabilities`用户API；平台驱动内部转换 | 用户-facing API稳定；平台实现自由转换 |
| 旧代码清理 | 分Phase迁移测试后删除 | 避免破坏现有测试；确保行为一致 |
| stdin支持 | 新增`StdinPipe`参数 | 修复已知缺陷；不影响现有调用 |

## 5. 模块结构

### 5.1 新增/修改文件

```
src/sandbox/
├── mod.rs                      # 不变
├── command.rs                  # 不变
├── capabilities.rs             # 不变
├── context.rs                  # 不变
├── workspace.rs                # 不变
├── driver.rs                   # 增强：添加platform()和is_supported()
├── config.rs                   # 增强：添加platform偏好配置
├── factory.rs                  # 修改：按平台选择驱动
├── exec_approval/              # 不变
├── platforms/                  # 【新增】平台实现目录
│   ├── mod.rs                  # 平台分发 + 通用工具
│   ├── common.rs               # 共享工具（路径归一化、策略转换）
│   ├── macos/
│   │   ├── mod.rs              # MacOSSandboxDriver
│   │   ├── seatbelt.rs         # SBPL策略生成（增强版）
│   │   └── tests.rs            # macOS特定测试
│   ├── linux/
│   │   ├── mod.rs              # LinuxSandboxDriver
│   │   ├── bwrap.rs            # bubblewrap调用
│   │   ├── landlock.rs         # landlock策略
│   │   ├── helper.rs           # aleph-linux-sandbox helper调用
│   │   └── tests.rs            # Linux特定测试
│   └── windows/
│       ├── mod.rs              # WindowsSandboxDriver（stub或完整）
│       └── tests.rs            # Windows特定测试
└── policy.rs                   # 【新增】统一策略表达

src/exec/sandbox/               # 【逐步清理】
├── mod.rs                      # 最终删除
├── adapter.rs                  # 最终删除
├── capabilities.rs             # 最终删除
├── profile.rs                  # 最终删除
├── executor.rs                 # 内容迁移到src/sandbox/platforms/macos/
├── audit.rs                    # 保留（audit独立concern）
└── ...                         # 其他文件评估后处理

aleph-linux-sandbox/            # 【新增】Linux sandbox helper crate
├── Cargo.toml
└── src/
    ├── main.rs                 # CLI入口
    ├── bwrap.rs                # bubblewrap参数构建
    ├── seccomp.rs              # seccomp过滤器
    └── landlock.rs             # landlock规则应用
```

## 6. 核心类型设计

### 6.1 增强OsSandboxDriverTrait

```rust
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

### 6.2 统一策略表达（SandboxPolicy）

```rust
/// 内部策略表达，从SandboxCapabilities转换而来
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

#[derive(Debug, Clone)]
pub enum NetworkPolicy {
    None,
    AllowHosts(Vec<String>),
    AllowAll,
    /// 允许代理loopback端口（用于managed network）
    ProxyOnly { ports: Vec<u16> },
}

#[derive(Debug, Clone)]
pub struct ProcessPolicy {
    pub allow_fork: bool,
    pub timeout_secs: u64,
    pub max_memory_mb: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum EnvPolicy {
    Inherit,
    Restricted,
    Minimal,
}
```

### 6.3 平台驱动分发

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
```

## 7. 平台实现细节

### 7.1 macOS增强（Phase 1）

**现状**：基本可用，但策略生成较简单。

**增强点：**
1. **精细seatbelt策略** — 参考codex的`seatbelt.rs`：
   - base policy（版本化SBPL模板）
   - network policy（代理感知、UDS支持）
   - platform defaults（DARWIN_USER_CACHE_DIR等）
   - unreadable glob → regex转换
2. **路径归一化** — `normalize_path_for_sandbox()`确保绝对路径
3. **Unix Domain Socket策略** — 允许指定UDS路径

**文件变更：**
- 新增 `src/sandbox/platforms/macos/seatbelt.rs` — 策略生成
- 修改 `src/sandbox/platforms/macos/mod.rs` — 驱动实现
- 删除 `src/exec/sandbox/executor.rs` 中的旧逻辑

### 7.2 Linux实现（Phase 2）

**架构：**
```
LinuxSandboxDriver
    ├── 检测bubblewrap可用性
    ├── 构建bwrap参数（fs bind/unbind）
    ├── 调用aleph-linux-sandbox helper
    │       ├── landlock规则设置
    │       ├── seccomp过滤器加载
    │       └── execvp目标程序
    └── 返回结果
```

**关键设计：**
- `aleph-linux-sandbox`作为独立crate，编译为静态二进制
- helper接收JSON策略，应用landlock+seccomp后exec
- 无bubblewrap时降级为纯landlock（功能受限但可用）
- WSL1检测并给出友好错误

**文件：**
- `src/sandbox/platforms/linux/mod.rs` — 驱动
- `src/sandbox/platforms/linux/bwrap.rs` — bubblewrap参数
- `aleph-linux-sandbox/src/main.rs` — helper入口
- `aleph-linux-sandbox/src/landlock.rs` — landlock规则
- `aleph-linux-sandbox/src/seccomp.rs` — seccomp过滤器

### 7.3 Windows实现（Phase 3）

**架构（参考codex）：**
```
WindowsSandboxDriver
    ├── 解析SandboxPolicy
    ├── 创建Restricted Token
    │       ├── 移除特权
    │       ├── 添加Capability SID
    │       └── 设置ACL
    ├── 应用ACL到文件系统
    │       ├── allow paths
    │       └── deny paths
    ├── 创建进程（CreateProcessAsUser）
    └── 返回结果
```

**关键设计：**
- 使用Windows Capability SID（非标准SID，自定义）
- ACL精确控制文件系统访问
- 网络阻断通过防火墙规则或null路由
- 可选private desktop（高安全场景）

**文件：**
- `src/sandbox/platforms/windows/mod.rs` — 驱动
- `src/sandbox/platforms/windows/token.rs` — Token操作
- `src/sandbox/platforms/windows/acl.rs` — ACL应用
- `src/sandbox/platforms/windows/policy.rs` — 策略转换

## 8. 旧代码清理计划

### 8.1 迁移顺序

```
Phase 1: macOS增强
    ├── 将src/exec/sandbox/executor.rs逻辑迁移到src/sandbox/platforms/macos/
    ├── 确保所有测试通过
    └── 标记exec/sandbox/为deprecated

Phase 2: Linux实现
    ├── 创建aleph-linux-sandbox helper
    ├── 实现LinuxSandboxDriver
    ├── 测试Linux平台
    └── 迁移exec/sandbox/audit.rs到src/sandbox/audit.rs

Phase 3: Windows实现
    ├── 实现WindowsSandboxDriver
    ├── 测试Windows平台
    └── 清理exec/sandbox/剩余文件

Phase 4: 最终清理
    ├── 删除src/exec/sandbox/目录
    ├── 更新所有import路径
    └── 验证全平台构建
```

### 8.2 具体清理清单

| 文件 | 动作 | 目标位置 |
|------|------|---------|
| `src/exec/sandbox/executor.rs` | 删除 | 逻辑已迁移到`src/sandbox/platforms/macos/mod.rs` |
| `src/exec/sandbox/adapter.rs` | 删除 | 被`OsSandboxDriverTrait`取代 |
| `src/exec/sandbox/capabilities.rs` | 删除 | 被`src/sandbox/capabilities.rs`取代 |
| `src/exec/sandbox/profile.rs` | 删除 | 功能合并到平台驱动 |
| `src/exec/sandbox/audit.rs` | 移动 | `src/sandbox/audit.rs` |
| `src/exec/sandbox/mod.rs` | 删除 | 重新导出不再 needed |

## 9. 配置增强

### 9.1 SandboxConfig扩展

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub workspace_root: PathBuf,
    pub enabled: bool,
    pub default_timeout_seconds: u64,
    pub max_output_bytes: usize,
    
    // 【新增】平台偏好
    #[serde(default)]
    pub platform_preference: PlatformPreference,
    
    // 【新增】Linux特定
    #[serde(default)]
    pub linux: LinuxSandboxConfig,
    
    // 【新增】macOS特定
    #[serde(default)]
    pub macos: MacosSandboxConfig,
    
    // 【新增】Windows特定
    #[serde(default)]
    pub windows: WindowsSandboxConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformPreference {
    #[default]
    Auto,       // 自动选择最佳可用平台sandbox
    Require,    // 要求sandbox，不可用时报错
    Forbid,     // 禁用sandbox（调试用）
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinuxSandboxConfig {
    /// bubblewrap可执行文件路径（None=自动检测PATH）
    pub bwrap_path: Option<PathBuf>,
    /// 使用legacy landlock（旧内核兼容）
    #[serde(default)]
    pub use_legacy_landlock: bool,
    /// aleph-linux-sandbox helper路径
    pub helper_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacosSandboxConfig {
    /// sandbox-exec可执行文件路径（默认/usr/bin/sandbox-exec）
    pub sandbox_exec_path: Option<PathBuf>,
    /// 是否包含平台默认路径
    #[serde(default = "default_true")]
    pub include_platform_defaults: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowsSandboxConfig {
    /// sandbox级别
    #[serde(default)]
    pub level: WindowsSandboxLevel,
    /// 使用private desktop
    #[serde(default)]
    pub private_desktop: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsSandboxLevel {
    #[default]
    Standard,
    Restricted,
    Disabled,
}
```

## 10. 测试策略

### 10.1 单元测试

| 模块 | 测试内容 |
|------|---------|
| `sandbox::platforms::common` | 路径归一化、策略转换 |
| `sandbox::platforms::macos` | SBPL策略生成、参数构建 |
| `sandbox::platforms::linux` | bwrap参数构建、helper调用 |
| `sandbox::platforms::windows` | Token创建、ACL计算 |
| `sandbox::policy` | SandboxPolicy ↔ SandboxCapabilities转换 |

### 10.2 集成测试

| 测试 | 平台 |
|------|------|
| `tests/sandbox_macos.rs` | macOS |
| `tests/sandbox_linux.rs` | Linux |
| `tests/sandbox_windows.rs` | Windows |
| `tests/sandbox_cross_platform.rs` | 所有平台 |

### 10.3 CI/CD

```yaml
# .github/workflows/ci.yml 增强
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

## 11. Roadmap

### Phase 1: macOS策略增强（2周）
- [ ] 创建`src/sandbox/platforms/`目录结构
- [ ] 实现增强版macOS seatbelt策略生成
- [ ] 添加网络代理支持
- [ ] 添加UDS支持
- [ ] 迁移`src/exec/sandbox/executor.rs`到`src/sandbox/platforms/macos/`
- [ ] 测试：所有现有macOS测试通过

### Phase 2: Linux支持（3周）
- [ ] 创建`aleph-linux-sandbox` helper crate
- [ ] 实现landlock规则设置
- [ ] 实现seccomp过滤器
- [ ] 实现bubblewrap参数构建
- [ ] 实现`LinuxSandboxDriver`
- [ ] 添加WSL检测
- [ ] 测试：Ubuntu/Debian/Fedora

### Phase 3: Windows支持（3周）
- [ ] 实现`WindowsSandboxDriver`
- [ ] 实现Restricted Token创建
- [ ] 实现ACL应用
- [ ] 实现Capability SID
- [ ] 可选：private desktop支持
- [ ] 测试：Windows 10/11

### Phase 4: 旧代码清理（1周）
- [ ] 迁移`src/exec/sandbox/audit.rs`到`src/sandbox/`
- [ ] 删除`src/exec/sandbox/`目录
- [ ] 更新所有import路径
- [ ] 更新文档
- [ ] 全平台CI验证

### Phase 5: 优化和打磨（1周）
- [ ] 性能基准测试
- [ ] 错误消息优化
- [ ] 文档完善
- [ ] 发布准备

## 12. 风险评估

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| bubblewrap在部分Linux发行版不可用 | 中 | 高 | 提供纯landlock降级；文档说明安装方式 |
| Windows sandbox需要管理员权限 | 中 | 中 | 优雅降级；文档说明 |
| 旧代码删除破坏外部依赖 | 低 | 中 | Phase 4前保留deprecated标记；grep确认无外部使用 |
| 多平台测试覆盖不足 | 中 | 高 | GitHub Actions矩阵构建；社区测试 |
| seccomp/landlock内核版本要求 | 中 | 中 | 运行时检测；graceful降级 |

## 13. Success Metrics

- [ ] macOS: 现有测试100%通过 + 新增策略测试
- [ ] Linux: bubblewrap + landlock/seccomp正常工作
- [ ] Windows: Restricted Token + ACL正常工作
- [ ] `src/exec/sandbox/`目录删除
- [ ] 全平台CI构建通过
- [ ] `cargo test -p alephcore --lib`无新增失败
- [ ] 文档更新完成

## 14. Next Action

1. 用户确认设计文档
2. 按Phase实施，每Phase完成后review
3. Phase 1开始前，先创建`src/sandbox/platforms/`目录结构和基础trait
