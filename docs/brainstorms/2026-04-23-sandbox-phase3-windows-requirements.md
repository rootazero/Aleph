---
date: 2026-04-23
topic: sandbox-phase3-windows-support
---

# Sandbox Phase 3: Windows 支持需求

## Problem Frame

Aleph sandbox 已实现 macOS (Seatbelt) 和 Linux (Bubblewrap) 支持，但 Windows 平台完全缺失。Windows 占桌面市场 ~70%，缺少 Windows sandbox 意味着 Aleph 在 Windows 上无法安全执行外部命令，严重限制工具执行安全性。

## Requirements

**Windows Sandbox Driver**
- R1. 实现 `WindowsSandboxDriver` 满足 `OsSandboxDriverTrait` 接口
- R2. 使用 Windows 原生安全机制：Restricted Token + Job Object + ACL
- R3. 支持所有 `SandboxPolicy` 变体（FsPolicy、NetworkPolicy、ProcessPolicy）
- R4. 提供 `is_supported()` 检测（Windows 系统 + 必要 API 可用）
- R5. 支持 stdin 输入传递
- R6. 支持 stdout/stderr 捕获和截断

**Filesystem Isolation**
- R7. `WorkspaceOnly`: 仅允许访问工作目录，使用 ACL 限制其他路径
- R8. `ReadPaths`: 额外只读路径通过 ACL 授予读取权限
- R9. `WritePaths`: 额外可写路径通过 ACL 授予读写权限
- R10. `FullRead`/`FullWrite`: 全系统访问 + 排除列表（通过 ACL deny entries）

**Network Isolation**
- R11. `NetworkPolicy::None`: 完全阻断网络（Windows Firewall 规则）
- R12. `NetworkPolicy::AllowAll`: 允许网络访问
- R13. `NetworkPolicy::AllowHosts`: 基础实现（记录警告，回退到阻断）
- R14. `NetworkPolicy::ProxyOnly`: 基础实现（记录警告，回退到阻断）

**Process Isolation**
- R15. `ProcessPolicy.allow_fork = false`: 使用 Job Object 限制子进程创建
- R16. `ProcessPolicy.timeout_secs`: 通过异步超时实现
- R17. 使用 Restricted Token 移除管理员权限

**Integration**
- R18. 注册到 `create_platform_driver()` 的 Windows 分支
- R19. 文件放在 `src/sandbox/platforms/windows/` 目录
- R20. 保持与 macOS/Linux 实现一致的架构风格

## Success Criteria

- [ ] `cargo check -p alephcore` 在 Windows 目标上编译通过
- [ ] `cargo test -p alephcore --lib` 在 Windows 上通过（包括 Windows 特定测试）
- [ ] Windows sandbox 能成功执行简单命令（如 `echo hello`）
- [ ] 文件系统隔离有效（无法访问工作目录外文件）
- [ ] 网络隔离有效（`NetworkPolicy::None` 下无法联网）

## Scope Boundaries

- **不包含**: Windows Sandbox (WSB) 容器化方案（太重，依赖 Hyper-V）
- **不包含**: AppContainer 隔离（复杂度高，UWP 绑定）
- **不包含**: 完整的 Windows 防火墙 UI 集成
- **不包含**: 代码签名或驱动程序

## Key Decisions

- **技术栈**: Restricted Token + Job Object + ACL（参考 codex 实现）
  - 理由：成熟、轻量、无需额外依赖、与 Aleph 架构匹配
  - 对比：Windows Sandbox 太重，AppContainer 太复杂
- **实现策略**: 先基础功能，后精细策略
  - 理由：Windows API 复杂，先保证核心隔离再优化
- **依赖**: `windows-sys` crate（Aleph 已间接依赖）

## Dependencies / Assumptions

- Windows 10/11 专业版/企业版（Home 版部分功能受限）
- 进程以标准用户权限运行（非管理员）
- `windows-sys` crate 提供 Win32 API 绑定

## Outstanding Questions

### Resolve Before Planning
- [无]

### Deferred to Planning
- [Needs research] Windows Firewall API 是否需要在运行时动态链接以避免链接错误？
- [Technical] Job Object 的内存限制是否需要在 Phase 3 实现？
- [Technical] 是否需要为 Windows 实现专门的测试 mock？

## Next Steps

→ `/ce:plan` for structured implementation planning
