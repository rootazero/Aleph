---
title: "feat: Windows Sandbox 完整实现 (Restricted Token + ACL + Job Object)"
type: feat
status: active
date: 2026-04-23
origin: docs/brainstorms/2026-04-23-sandbox-phase5-windows-requirements.md
---

# Windows Sandbox 完整实现

## Overview

将 `src/sandbox/platforms/windows/driver.rs` 从占位符实现升级为完整的 Windows 沙箱驱动，使用 Restricted Token + ACL + Job Object 提供与 macOS/Linux 同等的安全隔离水平。

## Problem Frame

当前 Windows 实现仅生成策略描述但不应用任何实际限制。Phase 5 需要实现真正的跨平台 sandbox 支持，使 Windows 用户获得与 macOS/Linux 相同的安全保障。

## Requirements Trace

- R1. Restricted Token 创建 — 禁用特权、移除管理员 SID、WRITE_RESTRICTED
- R2. ACL 文件系统控制 — 工作目录允许、系统目录拒绝
- R3. Job Object 进程限制 — CPU时间、内存、禁止子进程
- R4. 进程创建与执行 — CreateProcessAsUserW、stdin/stdout/stderr
- R5. SandboxPolicy → Windows 安全机制映射
- R6. 优雅失败处理 — 不静默降级
- R7. 网络过滤暂不支持 — 文档化限制

## Scope Boundaries

- **不包含**: AppContainer、WFP 网络过滤、桌面/UI 隔离
- **不包含**: 复杂的 ACL 继承场景（仅基础继承）
- **支持**: Windows 10/11，标准用户权限

## Context & Research

### 现有代码结构

- `src/sandbox/platforms/windows/driver.rs` — 占位符实现（380行），`run()` 仅使用 `tokio::process::Command`
- `src/sandbox/platforms/macos/seatbelt.rs` — 参考模式：生成策略 + 执行
- `src/sandbox/platforms/linux/bwrap.rs` — 参考模式：生成参数 + 执行
- `src/sandbox/command.rs` — `SandboxError` 包含 `ExecutionFailed`、`Io`、`Timeout` 等

### 依赖状态

- `windows-sys` 已存在于 Cargo.lock（多个版本）
- 需要添加 `windows-sys` 到 `alephcore` 的 Cargo.toml（带 Security 和 System 特性）

### Codex 参考实现

- `codex/codex-rs/windows-sandbox-rs/src/token.rs` — CreateRestrictedToken 实现
- `codex/codex-rs/windows-sandbox-rs/src/acl.rs` — ACL 管理
- `codex/codex-rs/windows-sandbox-rs/src/process.rs` — CreateProcessAsUserW

## Key Technical Decisions

- **使用 `windows-sys` 而非 `windows` crate**: `windows-sys` 更轻量，与现有依赖一致，且 Codex 主要使用 `windows-sys`
- **文件组织**: 将 Windows 实现拆分为 `driver.rs`（主驱动）、`token.rs`（令牌管理）、`acl.rs`（ACL 控制）、`job.rs`（Job Object），保持与 Codex 类似的模块结构
- **错误处理**: Token/ACL/Job 创建失败时返回 `SandboxError::ExecutionFailed`，绝不降级到无沙箱模式
- **超时实现**: 使用 `tokio::time::timeout`（已有模式）而非 Job Object 的 CPU 时间限制，更简单可靠

## Open Questions

### Resolved During Planning

- **windows-sys vs windows crate**: 使用 `windows-sys`（更轻量，与现有依赖一致）
- **Job Object CPU 限制**: 使用 `tokio::time::timeout` 而非 Job Object CPU 限制（更简单可靠）
- **内存限制**: 使用 Job Object 的 `JOB_OBJECT_LIMIT_PROCESS_MEMORY`

### Deferred to Implementation

- **SID 分配策略**: 使用 Well-Known SID 还是自定义 Capability SID（实现时验证）
- **ACL 继承细节**: 具体继承标志的选择（实现时测试验证）
- **环境变量过滤**: 哪些变量允许传递（实现时参考 Codex）

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification.*

```
WindowsSandboxDriver::run()
  │
  ├─> parse_profile() ──→ ParsedProfile
  │
  ├─> create_restricted_token()
  │   ├─> OpenProcessToken(GetCurrentProcess())
  │   ├─> CreateRestrictedToken(DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED)
  │   └─> SetTokenInformation(TokenDefaultDacl)
  │
  ├─> setup_acl_for_workspace()
  │   ├─> GetNamedSecurityInfo(workspace_path)
  │   ├─> SetEntriesInAclW(EXPLICIT_ACCESS for allow/deny)
  │   └─> SetNamedSecurityInfo(workspace_path, new_acl)
  │
  ├─> create_job_object()
  │   ├─> CreateJobObjectW()
  │   ├─> SetInformationJobObject(
  │   │     JOB_OBJECT_LIMIT_PROCESS_MEMORY
  │   │     JOB_OBJECT_LIMIT_ACTIVE_PROCESS)
  │   └─> AssignProcessToJobObject() (after process creation)
  │
  ├─> create_process_as_user()
  │   ├─> CreateProcessAsUserW(restricted_token, ...)
  │   ├─> AssignProcessToJobObject(job_handle, process_handle)
  │   └─> Setup stdin/stdout/stderr pipes
  │
  ├─> wait_with_timeout()
  │   └─> tokio::time::timeout(duration, process.wait())
  │
  └─> cleanup()
      ├─> TerminateJobObject() (if timeout)
      ├─> CloseHandle(job_handle)
      ├─> CloseHandle(token_handle)
      └─> Revert ACL changes
```

## Implementation Units

- [ ] **Unit 1: 添加 windows-sys 依赖并创建模块结构**

**Goal:** 设置 Windows API 访问和模块文件结构

**Requirements:** R1-R7 (基础设施)

**Dependencies:** 无

**Files:**
- Modify: `Cargo.toml` (添加 `windows-sys` 依赖)
- Modify: `src/sandbox/platforms/windows/mod.rs` (导出子模块)
- Create: `src/sandbox/platforms/windows/token.rs`
- Create: `src/sandbox/platforms/windows/acl.rs`
- Create: `src/sandbox/platforms/windows/job.rs`

**Approach:**
- 在 `alephcore` 的 `Cargo.toml` 中添加 `windows-sys` 依赖，启用 `Win32_Security`、`Win32_System_Threading`、`Win32_System_JobObjects` 等特性
- 创建 `token.rs`、`acl.rs`、`job.rs` 子模块
- 更新 `mod.rs` 导出所有子模块

**Patterns to follow:**
- `desktop/shared/src/perception/ocr_windows.rs` — Windows API 使用模式
- Codex `windows-sandbox-rs/src/lib.rs` — 模块组织

**Test scenarios:**
- Test expectation: none — 纯基础设施，无行为变化

**Verification:**
- `cargo check -p alephcore` 在 Windows 目标上通过

---

- [ ] **Unit 2: 实现 Restricted Token 创建 (token.rs)**

**Goal:** 创建受限令牌，禁用特权并设置受限 SID

**Requirements:** R1

**Dependencies:** Unit 1

**Files:**
- Create: `src/sandbox/platforms/windows/token.rs`
- Test: `src/sandbox/platforms/windows/token.rs` (cfg(test) 模块)

**Approach:**
- 实现 `create_restricted_token() -> Result<HANDLE, SandboxError>`
- 使用 `OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, ...)` 获取当前令牌
- 使用 `CreateRestrictedToken` 创建受限令牌，标志：
  - `DISABLE_MAX_PRIVILEGE` — 禁用所有特权
  - `WRITE_RESTRICTED` — 添加 WRITE_RESTRICTED SID
- 设置默认 DACL 允许受限 SID 访问
- 所有 unsafe 调用必须有 `// SAFETY:` 注释

**Technical design:**
```rust
// SAFETY: All handles are valid and properly initialized
unsafe fn create_restricted_token() -> Result<HANDLE, SandboxError> {
    // 1. Get current process token
    // 2. Create restricted token with DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED
    // 3. Set default DACL for sandbox SIDs
    // 4. Return token handle
}
```

**Patterns to follow:**
- Codex `token.rs` — CreateRestrictedToken 使用模式
- `.claude/rules/rust/security.md` — unsafe 代码规范

**Test scenarios:**
- Happy path: 在 Windows 上成功创建受限令牌
- Error path: 非 Windows 平台返回错误
- Edge case: 管理员权限 vs 标准用户权限

**Verification:**
- 单元测试验证令牌创建成功
- 使用 `GetTokenInformation` 验证令牌属性正确

---

- [ ] **Unit 3: 实现 ACL 管理 (acl.rs)**

**Goal:** 为工作目录设置 ACL，允许受限令牌访问并保护系统目录

**Requirements:** R2

**Dependencies:** Unit 2

**Files:**
- Create: `src/sandbox/platforms/windows/acl.rs`
- Test: `src/sandbox/platforms/windows/acl.rs` (cfg(test) 模块)

**Approach:**
- 实现 `setup_workspace_acl(path, token_sid) -> Result<(), SandboxError>`
- 使用 `GetNamedSecurityInfo` 获取现有 ACL
- 使用 `SetEntriesInAclW` 添加允许 ACE（工作目录）
- 继承规则：使用 `OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE`
- 提供 `restore_acl(path, original_acl)` 用于清理

**Technical design:**
```rust
// SAFETY: EXPLICIT_ACCESS_W is properly initialized
unsafe fn setup_workspace_acl(
    path: &Path,
    sid: PSID,
) -> Result<(), SandboxError> {
    // 1. Get existing security descriptor
    // 2. Build EXPLICIT_ACCESS_W for:
    //    - Allow GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE for sandbox SID
    //    - Inherit to children
    // 3. SetNamedSecurityInfo
}
```

**Patterns to follow:**
- Codex `acl.rs` — ACL 操作模式
- Codex `workspace_acl.rs` — 工作目录特定 ACL

**Test scenarios:**
- Happy path: 成功设置工作目录 ACL
- Happy path: 子目录继承 ACL
- Error path: 无效路径返回错误
- Integration: 受限令牌可以访问工作目录但无法访问其他目录

**Verification:**
- 单元测试验证 ACL 设置成功
- 集成测试验证访问控制生效

---

- [ ] **Unit 4: 实现 Job Object 限制 (job.rs)**

**Goal:** 创建 Job Object 并设置进程限制（内存、禁止子进程）

**Requirements:** R3

**Dependencies:** Unit 1

**Files:**
- Create: `src/sandbox/platforms/windows/job.rs`
- Test: `src/sandbox/platforms/windows/job.rs` (cfg(test) 模块)

**Approach:**
- 实现 `create_job_object(max_memory_mb, allow_fork) -> Result<HANDLE, SandboxError>`
- 使用 `CreateJobObjectW` 创建 Job Object
- 使用 `SetInformationJobObject` 设置限制：
  - `JOB_OBJECT_LIMIT_PROCESS_MEMORY` — 内存限制
  - `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` — 禁止子进程（当 allow_fork=false）
- 实现 `assign_process_to_job(job_handle, process_handle)`
- 实现 `terminate_job(job_handle)` 用于超时清理

**Technical design:**
```rust
// SAFETY: JOBOBJECT_EXTENDED_LIMIT_INFORMATION is zero-initialized
unsafe fn create_job_object(
    max_memory_mb: Option<u64>,
    allow_fork: bool,
) -> Result<HANDLE, SandboxError> {
    // 1. CreateJobObjectW
    // 2. Build JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    // 3. Set JOB_OBJECT_LIMIT_PROCESS_MEMORY if max_memory specified
    // 4. Set JOB_OBJECT_LIMIT_ACTIVE_PROCESS = 1 if !allow_fork
    // 5. SetInformationJobObject
}
```

**Patterns to follow:**
- Windows API 文档 — Job Object 使用模式

**Test scenarios:**
- Happy path: 成功创建 Job Object 并设置限制
- Happy path: 进程超过内存限制被终止
- Happy path: 进程尝试创建子进程被阻止
- Error path: 无效参数返回错误

**Verification:**
- 单元测试验证 Job Object 创建
- 集成测试验证内存限制和子进程限制生效

---

- [ ] **Unit 5: 重构 driver.rs 实现完整沙箱执行**

**Goal:** 整合 Token + ACL + Job Object，实现真正的沙箱执行

**Requirements:** R1-R6

**Dependencies:** Unit 2, 3, 4

**Files:**
- Modify: `src/sandbox/platforms/windows/driver.rs`
- Test: `src/sandbox/platforms/windows/driver.rs` (扩展现有测试)

**Approach:**
- 重写 `run()` 方法：
  1. 解析 profile
  2. 创建 Restricted Token
  3. 设置工作目录 ACL
  4. 创建 Job Object
  5. 使用 `CreateProcessAsUserW` 启动进程
  6. 将进程关联到 Job Object
  7. 等待进程完成（带超时）
  8. 清理资源（Token、ACL、Job Object）
- 使用 `tokio::task::spawn_blocking` 包装同步 Win32 API 调用
- 保持 stdin/stdout/stderr 管道与现有实现一致

**Technical design:**
```rust
async fn run(...) -> Result<SandboxOutput, SandboxError> {
    let profile = parse_profile(&profile.contents)?;
    
    // Spawn blocking Win32 operations
    tokio::task::spawn_blocking(move || {
        unsafe {
            let token = create_restricted_token()?;
            let _acl_guard = setup_workspace_acl(cwd, token_sid)?;
            let job = create_job_object(profile.max_memory_mb, profile.allow_fork)?;
            
            let (proc_handle, thread_handle) = create_process_as_user(token, ...)?;
            assign_process_to_job(job, proc_handle)?;
            
            // Wait for completion with timeout
            // Cleanup on drop
        }
    }).await.map_err(...)?
}
```

**Patterns to follow:**
- `src/sandbox/platforms/linux/bwrap.rs` — 超时和输出处理
- `src/sandbox/platforms/macos/seatbelt.rs` — 错误处理模式

**Test scenarios:**
- Happy path: 成功执行简单命令（echo、dir）
- Happy path: 带 stdin 输入执行
- Happy path: 超时后进程被终止
- Error path: 命令不存在返回 ExecutionFailed
- Error path: Token 创建失败返回错误（不降级）
- Integration: 沙箱进程无法访问工作目录外文件
- Integration: 沙箱进程无法创建子进程（allow_fork=false）

**Verification:**
- 所有现有 Windows 驱动测试通过
- 新增集成测试验证实际隔离效果
- `cargo test -p alephcore --lib sandbox` 全部通过

---

- [ ] **Unit 6: 添加 AllowHosts 警告和网络限制文档**

**Goal:** 实现 R7 — 文档化网络限制已知问题

**Requirements:** R7

**Dependencies:** Unit 5

**Files:**
- Modify: `src/sandbox/platforms/windows/driver.rs`
- Modify: `docs/reference/SANDBOX.md`

**Approach:**
- 在 `generate_profile()` 中，当遇到 `NetworkPolicy::AllowHosts` 时添加 `tracing::warn!`
- 在 `run()` 中同样添加警告
- 更新 SANDBOX.md 文档，添加 Windows 限制说明

**Test scenarios:**
- Happy path: 使用 AllowHosts 时触发警告日志

**Verification:**
- 日志输出验证
- 文档更新确认

---

- [ ] **Unit 7: 集成测试和验证**

**Goal:** 确保 Windows sandbox 与 WorkspaceSandbox 集成正常

**Requirements:** R1-R6

**Dependencies:** Unit 5, 6

**Files:**
- Modify: `tests/` (如需要)
- Modify: `.github/workflows/`（验证 Windows CI）

**Approach:**
- 运行完整测试套件：`cargo test -p alephcore --lib sandbox`
- 验证与 `WorkspaceSandbox` 的集成
- 检查 GitHub Actions Windows 构建是否通过

**Test scenarios:**
- Integration: WorkspaceSandbox + WindowsSandboxDriver 完整流程
- Integration: 6-step pipeline 在 Windows 上正常工作

**Verification:**
- `cargo test -p alephcore --lib sandbox` — 90+ 测试通过
- `cargo test -p alephcore --lib` — 全部通过（除已知失败外）
- GitHub Actions Windows 构建通过

## System-Wide Impact

- **Interaction graph:** `WorkspaceSandbox` 调用 `WindowsSandboxDriver::run()`，无其他交互变化
- **Error propagation:** 保持现有 `SandboxError` 类型，不引入新错误变体
- **State lifecycle risks:** Token/ACL/Job Object 句柄必须在所有路径上关闭（使用 RAII guard 模式）
- **API surface parity:** `OsSandboxDriverTrait` 接口不变，Windows 实现与其他平台保持一致
- **Unchanged invariants:** 
  - `Sandbox` trait 不变
  - `SandboxCommand` / `SandboxOutput` 不变
  - `SandboxCapabilities` 不变
  - 其他平台驱动（macOS/Linux）不受影响

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| windows-sys 版本冲突 | 使用与现有依赖兼容的版本（0.59.0 或 0.61.2） |
| unsafe 代码安全审查 | 每块 unsafe 代码必须有 SAFETY 注释，遵循项目安全规则 |
| Windows 版本差异 | 仅支持 Windows 10/11，使用标准 Win32 API |
| ACL 设置失败导致系统不稳定 | 仅在沙箱工作目录上设置 ACL，不影响系统目录 |
| Job Object 内存限制不准确 | 使用文档推荐的方法，测试验证 |

## Documentation / Operational Notes

- 更新 `docs/reference/SANDBOX.md` 添加 Windows 实现说明
- 添加 Windows 限制说明：AllowHosts 暂不支持
- 记录 Windows 10/11 支持要求

## Sources & References

- **Origin document:** [docs/brainstorms/2026-04-23-sandbox-phase5-windows-requirements.md](../brainstorms/2026-04-23-sandbox-phase5-windows-requirements.md)
- **Reference code:** `/Volumes/TBU4/Github/codex/codex-rs/windows-sandbox-rs/src/`
- **Existing Windows driver:** `src/sandbox/platforms/windows/driver.rs`
- **macOS implementation:** `src/sandbox/platforms/macos/seatbelt.rs`
- **Linux implementation:** `src/sandbox/platforms/linux/bwrap.rs`
- **Architecture doc:** `docs/reference/SANDBOX.md`
