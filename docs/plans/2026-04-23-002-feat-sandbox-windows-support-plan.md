---
title: Sandbox Phase 3 — Windows 支持
type: feat
status: active
date: 2026-04-23
origin: docs/brainstorms/2026-04-23-sandbox-phase3-windows-requirements.md
---

# Sandbox Phase 3: Windows 支持实施计划

## Overview

为 Aleph sandbox 添加 Windows 平台支持，实现基于 Restricted Token + Job Object + ACL 的进程隔离。这是多平台 sandbox 的最后一个主要平台，完成后 sandbox 将覆盖 macOS、Linux 和 Windows 三大桌面平台。

## Problem Frame

Aleph sandbox 已实现 macOS (Seatbelt) 和 Linux (Bubblewrap) 支持，但 Windows 平台完全缺失。Windows 占桌面市场 ~70%，缺少 Windows sandbox 意味着 Aleph 在 Windows 上无法安全执行外部命令。

## Requirements Trace

- R1. 实现 `WindowsSandboxDriver` 满足 `OsSandboxDriverTrait` 接口
- R2. 使用 Windows 原生安全机制：Restricted Token + Job Object + ACL
- R3. 支持所有 `SandboxPolicy` 变体
- R4. 提供 `is_supported()` 检测
- R5. 支持 stdin 输入传递
- R6. 支持 stdout/stderr 捕获和截断
- R7-R10. 文件系统隔离（WorkspaceOnly, ReadPaths, WritePaths, FullRead/FullWrite）
- R11-R14. 网络隔离（None, AllowAll, AllowHosts, ProxyOnly）
- R15-R17. 进程隔离（fork 限制、超时、权限降级）
- R18-R20. 集成到平台工厂和目录结构

## Scope Boundaries

- **不包含**: Windows Sandbox (WSB) 容器化方案
- **不包含**: AppContainer 隔离
- **不包含**: 完整的 Windows 防火墙 UI 集成
- **不包含**: 代码签名或驱动程序

## Context & Research

### Relevant Code and Patterns

- **Trait 接口**: `src/sandbox/driver.rs` — `OsSandboxDriverTrait` 定义
- **策略类型**: `src/sandbox/policy.rs` — `SandboxPolicy`, `FsPolicy`, `NetworkPolicy`, `ProcessPolicy`
- **平台工厂**: `src/sandbox/platforms/mod.rs` — `create_platform_driver()`
- **macOS 参考**: `src/sandbox/platforms/macos/seatbelt.rs` — 已实现的平台驱动
- **Linux 参考**: `src/sandbox/platforms/linux/bwrap.rs` — 类似架构的驱动
- **Codex 参考**: `/Volumes/TBU4/Github/codex/codex-rs/windows-sandbox-rs/src/` — Windows 安全 API 使用模式

### Key Codex Patterns to Adapt

Codex 使用以下 Windows 安全机制：
1. **Restricted Token**: `CreateRestrictedToken` + `LUA_TOKEN` + `WRITE_RESTRICTED` 标志
2. **ACL**: `SetEntriesInAclW`, `SetNamedSecurityInfoW`, `EXPLICIT_ACCESS_W`
3. **Job Object**: 限制子进程创建（`JOB_OBJECT_LIMIT_ACTIVE_PROCESS`）
4. **防火墙**: `INetFwPolicy2` COM 接口（可选，Phase 3 基础版不实现）
5. **进程创建**: `CreateProcessAsUserW` 使用 restricted token

### Aleph 架构约束

- 使用 `windows-sys` crate（不是 `windows` crate）
- 所有 `unsafe` 代码必须有 `// SAFETY:` 注释
- 错误处理使用 `thiserror`（库）或 `anyhow`（应用）
- 异步使用 `tokio`

## Key Technical Decisions

- **技术栈**: Restricted Token + Job Object + ACL
  - 理由：成熟、轻量、无需额外依赖
  - 对比：Windows Sandbox 太重，AppContainer 太复杂
- **网络隔离策略**: Phase 3 基础版使用 Windows Firewall COM API 的简化版本，仅支持 `NetworkPolicy::None`（完全阻断）和 `AllowAll`（完全允许）。`AllowHosts` 和 `ProxyOnly` 记录警告后回退到阻断。
- **依赖**: `windows-sys` crate（Aleph 已间接依赖，需确认是否已显式添加）

## Open Questions

### Resolved During Planning
- [x] 使用 `windows-sys` 还是 `windows` crate？→ `windows-sys`（与 Aleph 现有依赖一致）
- [x] 是否需要 Job Object 内存限制？→ Phase 3 不实现（`max_memory_mb` 为 None 时跳过）

### Deferred to Implementation
- [Technical] Windows Firewall COM API 是否需要在运行时动态链接？
- [Technical] Restricted Token 的权限降级粒度如何调整？
- [Needs research] `windows-sys` 版本是否支持所有需要的 API？

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
┌─────────────────────────────────────────────────────────────┐
│                    WindowsSandboxDriver                     │
├─────────────────────────────────────────────────────────────┤
│  profile_for()                                              │
│    ├── 生成 restricted token（基于 SandboxPolicy）          │
│    ├── 配置 ACL（工作目录 + 额外路径）                      │
│    └── 序列化为 OsSandboxProfile（token handle + ACL 配置）│
├─────────────────────────────────────────────────────────────┤
│  run()                                                      │
│    ├── 创建 Job Object（限制子进程）                        │
│    ├── 使用 CreateProcessAsUserW 启动进程                   │
│    ├── 将进程加入 Job Object                                │
│    ├── 可选：配置 Windows Firewall 规则                     │
│    ├── 等待进程完成（带超时）                               │
│    └── 收集 stdout/stderr                                   │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Units

- [ ] **Unit 1: 添加 windows-sys 依赖并创建模块结构**

**Goal:** 设置 Windows sandbox 的编译环境和模块骨架

**Requirements:** R18, R19, R20

**Dependencies:** None

**Files:**
- Modify: `Cargo.toml`（添加 `windows-sys` 依赖，target-specific）
- Create: `src/sandbox/platforms/windows/mod.rs`
- Create: `src/sandbox/platforms/windows/token.rs`（restricted token 创建）
- Create: `src/sandbox/platforms/windows/acl.rs`（ACL 配置）
- Create: `src/sandbox/platforms/windows/job.rs`（Job Object 管理）
- Create: `src/sandbox/platforms/windows/driver.rs`（WindowsSandboxDriver）

**Approach:**
- 在 `Cargo.toml` 中添加 `[target.'cfg(windows)'.dependencies]` 下的 `windows-sys`
- 创建 Windows 平台目录结构，参考 macOS/linux 的组织方式
- 模块导出 `WindowsSandboxDriver`

**Patterns to follow:**
- `src/sandbox/platforms/macos/mod.rs` — 平台模块导出模式
- `src/sandbox/platforms/linux/mod.rs` — 平台模块导出模式

**Test scenarios:**
- Happy path: `cargo check -p alephcore` 在 Windows target 上编译通过
- Edge case: 在非 Windows 平台上，Windows 模块被条件编译排除

**Verification:**
- `cargo check -p alephcore` 通过
- `cargo check --target x86_64-pc-windows-msvc` 通过（如可用）

---

- [ ] **Unit 2: 实现 Restricted Token 创建**

**Goal:** 实现基于 SandboxPolicy 的 restricted token 生成

**Requirements:** R2, R15, R17

**Dependencies:** Unit 1

**Files:**
- Create: `src/sandbox/platforms/windows/token.rs`
- Modify: `src/sandbox/platforms/windows/mod.rs`

**Approach:**
- 使用 `windows-sys` 的 `CreateRestrictedToken` API
- 应用 `LUA_TOKEN`（受限用户）和 `WRITE_RESTRICTED` 标志
- 移除管理员 SID 和特权
- 根据 `ProcessPolicy.allow_fork` 决定是否保留进程创建权限

**Technical design:**
```rust
// Directional guidance only
fn create_restricted_token(policy: &ProcessPolicy) -> Result<HANDLE, SandboxError> {
    // 1. 打开当前进程 token
    // 2. 调用 CreateRestrictedToken  with LUA_TOKEN | WRITE_RESTRICTED
    // 3. 如果 allow_fork == false，移除 SeCreateProcessPrivilege
    // 4. 返回新 token handle
}
```

**Patterns to follow:**
- Codex: `codex-rs/windows-sandbox-rs/src/token.rs` — `CreateRestrictedToken` 使用模式
- Aleph: `src/sandbox/platforms/linux/bwrap.rs` — 策略到隔离机制的映射

**Test scenarios:**
- Happy path: 创建 restricted token 成功
- Edge case: `allow_fork = true` 时保留进程创建权限
- Error path: Windows API 失败时返回合适的 SandboxError

**Verification:**
- 单元测试通过
- Token 创建不 panic

---

- [ ] **Unit 3: 实现 ACL 配置**

**Goal:** 实现文件系统路径的 ACL 控制

**Requirements:** R7, R8, R9, R10

**Dependencies:** Unit 2

**Files:**
- Create: `src/sandbox/platforms/windows/acl.rs`

**Approach:**
- 使用 `SetEntriesInAclW` 和 `SetNamedSecurityInfoW`
- 为工作目录创建允许 ACL
- 为额外路径创建只读或读写 ACL
- 为排除路径创建拒绝 ACL（FullRead/FullWrite）

**Technical design:**
```rust
// Directional guidance only
fn apply_fs_policy(policy: &FsPolicy, cwd: &Path) -> Result<(), SandboxError> {
    match policy {
        WorkspaceOnly => {
            // 允许 cwd 读写，拒绝其他所有路径
        }
        ReadPaths(paths) => {
            // 允许 cwd 读写 + paths 只读
        }
        WritePaths(paths) => {
            // 允许 cwd 读写 + paths 读写
        }
        FullRead { exclude } => {
            // 允许全系统读取，exclude 路径拒绝
        }
        FullWrite { exclude } => {
            // 允许全系统读写，exclude 路径拒绝
        }
    }
}
```

**Patterns to follow:**
- Codex: `codex-rs/windows-sandbox-rs/src/acl.rs` — ACL 操作模式
- Aleph: `src/sandbox/platforms/linux/bwrap.rs` — 策略分支处理

**Test scenarios:**
- Happy path: WorkspaceOnly 策略下工作目录可访问
- Happy path: ReadPaths 策略下额外路径只读
- Edge case: 无效路径（含非法 UTF-8）返回错误
- Error path: Windows API 失败返回 SandboxError

**Verification:**
- 单元测试通过
- ACL 应用不 panic

---

- [ ] **Unit 4: 实现 Job Object 管理**

**Goal:** 使用 Job Object 限制子进程创建

**Requirements:** R15, R16

**Dependencies:** Unit 1

**Files:**
- Create: `src/sandbox/platforms/windows/job.rs`

**Approach:**
- 使用 `CreateJobObjectW` 创建 Job Object
- 设置 `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` 限制（`allow_fork = false` 时设为 1）
- 使用 `AssignProcessToJobObject` 将子进程加入 Job
- 超时通过 tokio `timeout` 实现

**Patterns to follow:**
- Codex: `codex-rs/windows-sandbox-rs/src/process.rs` — Job Object 使用
- Aleph: `src/sandbox/platforms/linux/bwrap.rs` — 超时处理模式

**Test scenarios:**
- Happy path: Job Object 创建成功
- Happy path: `allow_fork = false` 时进程无法创建子进程
- Edge case: 进程已退出后清理 Job Object

**Verification:**
- 单元测试通过
- Job Object 创建和清理不泄漏资源

---

- [ ] **Unit 5: 实现 WindowsSandboxDriver**

**Goal:** 整合所有组件，实现完整的 OsSandboxDriverTrait

**Requirements:** R1, R3, R4, R5, R6, R11, R12, R13, R14

**Dependencies:** Unit 2, Unit 3, Unit 4

**Files:**
- Create: `src/sandbox/platforms/windows/driver.rs`
- Modify: `src/sandbox/platforms/mod.rs`（注册 Windows 驱动）

**Approach:**
- `platform()` 返回 `"windows/token"`
- `is_supported()` 检测 Windows 系统（`cfg(target_os = "windows")`）
- `profile_for()` 生成 restricted token + ACL 配置，序列化为 profile
- `run()`：
  1. 解析 profile 恢复 token 和 ACL 配置
  2. 创建 Job Object
  3. 使用 `CreateProcessAsUserW` 启动进程
  4. 将进程加入 Job Object
  5. 根据 `NetworkPolicy` 配置防火墙（基础版：None = 阻断，其他 = 允许或警告）
  6. 等待进程完成（带超时）
  7. 收集 stdout/stderr
  8. 清理资源

**Technical design:**
```rust
// Directional guidance only
#[async_trait]
impl OsSandboxDriverTrait for WindowsSandboxDriver {
    fn platform(&self) -> &'static str { "windows/token" }
    
    fn is_supported(&self) -> bool {
        cfg!(target_os = "windows")
    }
    
    fn profile_for(&self, caps: &SandboxCapabilities, cwd: &Path) 
        -> Result<OsSandboxProfile, SandboxError> {
        // 1. 创建 restricted token
        // 2. 应用 ACL
        // 3. 序列化为 profile
    }
    
    async fn run(&self, program, args, env, stdin, cwd, profile, timeout, max_output) 
        -> Result<SandboxOutput, SandboxError> {
        // 1. 解析 profile
        // 2. 创建 Job Object
        // 3. CreateProcessAsUserW
        // 4. AssignProcessToJobObject
        // 5. 可选：防火墙配置
        // 6. tokio::time::timeout
        // 7. 收集输出
    }
}
```

**Patterns to follow:**
- `src/sandbox/platforms/macos/seatbelt.rs` — 完整的 trait 实现
- `src/sandbox/platforms/linux/bwrap.rs` — run() 方法结构
- Codex: `codex-rs/windows-sandbox-rs/src/process.rs` — 进程创建

**Test scenarios:**
- Happy path: 执行 `echo hello` 成功
- Happy path: 执行命令并捕获 stdout/stderr
- Happy path: stdin 输入传递成功
- Edge case: 超时后返回 Timeout 错误
- Edge case: 输出超过 max_output_bytes 时截断
- Error path: 无效程序路径返回 ExecutionFailed
- Integration: `NetworkPolicy::None` 下无法访问网络

**Verification:**
- 所有单元测试通过
- `cargo test -p alephcore --lib sandbox` 通过

---

- [ ] **Unit 6: 更新 GitHub Actions CI**

**Goal:** 确保 CI 在 Windows 上运行 sandbox 测试

**Requirements:** R18

**Dependencies:** Unit 5

**Files:**
- Modify: `.github/workflows/aleph-core-ci.yml`

**Approach:**
- 已在 Phase 2 添加多平台矩阵（包含 windows-latest）
- 确认 Windows runner 上安装了必要的依赖
- Windows 不需要额外系统依赖（Win32 API 内置）

**Test scenarios:**
- Integration: CI 在 windows-latest 上编译通过
- Integration: CI 在 windows-latest 上测试通过

**Verification:**
- GitHub Actions 运行成功

## System-Wide Impact

- **Interaction graph**: `create_platform_driver()` 现在返回 `WindowsSandboxDriver` 而非 `UnsupportedDriver`
- **Error propagation**: Windows API 错误统一转换为 `SandboxError::ExecutionFailed` 或 `SandboxError::Io`
- **State lifecycle risks**: Token handle 和 Job Object handle 必须在 run() 结束后关闭（使用 RAII 或显式 CloseHandle）
- **API surface parity**: Windows 实现与 macOS/Linux 保持行为一致
- **Unchanged invariants**: `OsSandboxDriverTrait` 接口不变，WorkspaceSandbox 使用方式不变

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| windows-sys 版本不支持某些 API | 使用 feature flags 或降级方案 |
| Windows API 行为差异（Home vs Pro） | 在 is_supported() 中检测并优雅降级 |
| ACL 配置复杂，容易出错 | 参考 codex 实现，充分测试 |
| Job Object 内存限制不支持 | Phase 3 跳过，Phase 5 再评估 |

## Documentation / Operational Notes

- 更新 `docs/superpowers/specs/2026-04-23-sandbox-multiplatform-design.md` 标记 Windows 为已完成
- 在 README 中更新平台支持状态

## Sources & References

- **Origin document:** [docs/brainstorms/2026-04-23-sandbox-phase3-windows-requirements.md](docs/brainstorms/2026-04-23-sandbox-phase3-windows-requirements.md)
- Related code: `src/sandbox/platforms/macos/seatbelt.rs`
- Related code: `src/sandbox/platforms/linux/bwrap.rs`
- External reference: codex `windows-sandbox-rs` — Restricted Token + ACL + Job Object 模式
- External docs: [windows-sys docs](https://docs.rs/windows-sys)
