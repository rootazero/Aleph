---
date: 2026-04-23
topic: sandbox-phase5-windows-full-implementation
---

# Phase 5: Windows Sandbox 完整实现

## Problem Frame

Aleph 的 sandbox 系统目前支持 macOS (Seatbelt) 和 Linux (Bubblewrap)，但 Windows 实现只是一个占位符——它生成策略描述但**不实际应用任何安全限制**。Phase 5 的目标是让 Windows 达到与 macOS/Linux 同等的安全隔离水平，真正实现跨平台 sandbox 支持。

## Requirements

### 核心安全机制

**R1. Restricted Token 创建**
- 使用 `CreateRestrictedToken` 创建受限令牌
- 禁用所有特权 (DISABLE_MAX_PRIVILEGE)
- 移除管理员 SID
- 添加 WRITE_RESTRICTED 标志
- 设置受限 SID 列表，仅允许访问工作目录

**R2. ACL 文件系统控制**
- 为工作目录设置显式 ACL，允许受限令牌读写
- 为系统目录（Windows、Program Files）添加拒绝访问 ACE
- 保护敏感路径：注册表、用户数据、系统配置
- 支持继承规则，确保子目录自动应用相同限制

**R3. Job Object 进程限制**
- 创建 Job Object 并关联子进程
- 实现 CPU 时间限制 (timeout_secs)
- 实现内存限制 (max_memory_mb)
- 当 `spawn_subprocess=false` 时，通过 Job Object 的 `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` 禁止创建子进程
- 进程退出后自动清理 Job Object

**R4. 进程创建与执行**
- 使用 `CreateProcessAsUserW` 以受限令牌启动进程
- 正确传递 stdin/stdout/stderr
- 支持环境变量传递（但限制敏感变量）
- 实现超时终止（Job Object 或单独计时器）

### 策略映射

**R5. SandboxPolicy → Windows 安全机制**
- `FsPolicy::WorkspaceOnly` → 仅允许 cwd 访问
- `FsPolicy::ReadPaths` → 为指定路径添加读取 ACL
- `FsPolicy::WritePaths` → 为指定路径添加读写 ACL
- `FsPolicy::FullRead`/`FullWrite` → 允许除 exclude 外的所有路径
- `NetworkPolicy::None` → 不额外处理（依赖防火墙规则，见 R7）
- `NetworkPolicy::AllowAll` → 不限制
- `NetworkPolicy::AllowHosts` → **暂不支持**（Phase 6）
- `process.allow_fork=false` → Job Object 限制
- `process.timeout_secs` → Job Object CPU 时间限制
- `process.max_memory_mb` → Job Object 内存限制

### 错误处理与回退

**R6. 优雅的失败处理**
- 如果 Restricted Token 创建失败，记录错误并返回 `SandboxError::ExecutionFailed`
- 如果 ACL 设置失败，清理已创建的 Token 并返回错误
- 如果 Job Object 创建失败，仍然尝试执行但记录警告
- 绝不"静默降级"到无沙箱模式

### 已知限制（文档化）

**R7. 网络过滤暂不支持**
- Windows 没有简单的 per-process 网络过滤机制
- WFP 实现复杂，留到 Phase 6
- 在文档中明确标注：`NetworkPolicy::AllowHosts` 在 Windows 上当前按 `AllowAll` 处理
- 添加 `tracing::warn!` 当使用 AllowHosts 时提醒用户

## Success Criteria

- [ ] Windows 上 `cargo test -p alephcore --lib sandbox` 全部通过
- [ ] `WindowsSandboxDriver::run()` 实际创建 Restricted Token 并应用限制
- [ ] 沙箱进程无法访问工作目录之外的文件
- [ ] 沙箱进程无法创建子进程（当 `allow_fork=false`）
- [ ] 沙箱进程在超时后被强制终止
- [ ] 内存限制超过时进程被终止
- [ ] 与现有 `WorkspaceSandbox` 集成正常

## Scope Boundaries

- **不在 Phase 5**: AppContainer 隔离（可选的未来增强）
- **不在 Phase 5**: WFP 网络过滤（Phase 6）
- **不在 Phase 5**: 桌面/UI 隔离（禁止剪贴板、窗口等）
- **不在 Phase 5**: 复杂的 ACL 继承场景（仅支持基础继承）

## Key Decisions

- **实现策略**: Restricted Token + ACL + Job Object（选项 A）
  - 理由：兼容性好，不需要额外配置，覆盖主要安全风险
  - Codex 使用相同策略，已验证可行性

- **网络过滤**: 暂时跳过，文档化限制
  - 理由：WFP 实现复杂，需要大量 unsafe COM 代码
  - 大多数使用场景可通过其他方式缓解

- **Job Object 限制**: 基础限制（CPU、内存、禁止子进程）
  - 理由：覆盖核心安全风险，实现相对简单
  - UI/IO 限制可后续添加

## Dependencies / Assumptions

- 依赖 `windows-sys` crate（已存在于 Aleph 依赖中）
- 需要大量 `unsafe` 代码调用 Win32 API
- 假设运行环境为 Windows 10/11（不支持 Windows 7/8）
- 假设进程以标准用户权限运行（非管理员）

## Outstanding Questions

### Resolve Before Planning
- 无

### Deferred to Planning
- [Needs research] `windows-sys` vs `windows` crate 的选择（Codex 混用两者）
- [Technical] Job Object 内存限制的单位转换（MB → bytes）
- [Technical] ACL 继承规则的具体行为验证
- [Technical] 超时终止时是否需要生成崩溃转储

## Next Steps

→ `/ce:plan` for structured implementation planning
