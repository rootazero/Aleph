---
date: 2026-04-23
topic: sandbox-phase2-linux
---

# Phase 2: Linux Sandbox 支持 — Requirements

## Problem Frame

Phase 1 已完成 macOS SeatbeltDriver 的实现，建立了跨平台 sandbox 架构基础。现在需要为 Linux 平台实现同等级别的 sandbox 支持，使 Aleph 在 Linux 上也能安全执行 AI 生成的代码。

当前 Linux 平台仅有 `UnsupportedDriver` stub，执行 sandbox 命令会直接报错。用户要求在 Linux 上使用 bubblewrap 实现纯用户态隔离，不依赖额外 helper binary。

## Requirements

**平台检测与可用性**
- R1. Linux 平台自动检测 bubblewrap 是否安装（检查 `/usr/bin/bwrap`、`/usr/local/bin/bwrap` 或 PATH）
- R2. bubblewrap 不可用时 `is_supported()` 返回 false，WorkspaceSandbox 可优雅降级
- R3. 驱动标识符返回 `"linux/bwrap"`

**策略映射 — 文件系统**
- R4. `FsPolicy::WorkspaceOnly` → 仅允许 workspace 目录读写，其他路径不可访问
- R5. `FsPolicy::ReadPaths` → 指定路径只读，workspace 保持读写
- R6. `FsPolicy::WritePaths` → 指定路径读写，workspace 保持读写
- R7. `FsPolicy::FullRead` → 全系统只读，排除指定路径
- R8. `FsPolicy::FullWrite` → 全系统读写，排除指定路径
- R9. 所有路径必须是绝对路径，相对路径基于 cwd 解析

**策略映射 — 网络**
- R10. `NetworkPolicy::None` → `--unshare-net`，仅保留 loopback
- R11. `NetworkPolicy::AllowAll` → `--share-net`，使用宿主网络
- R12. `NetworkPolicy::AllowHosts` → 解析并记录 warn 日志，但执行时降级为 `--unshare-net`（完全禁止网络）。AllowHosts 真正实现在 Phase 2.5
- R13. `NetworkPolicy::ProxyOnly` → 同 AllowHosts，限制 loopback 端口（Phase 2.5）

**策略映射 — 进程**
- R14. `ProcessPolicy.allow_fork = false` → `--unshare-pid` + `--cap-drop ALL`
- R15. `ProcessPolicy.allow_fork = true` → 不添加 `--unshare-pid`，允许子进程
- R16. timeout 和 max_memory_mb 由 WorkspaceSandbox 层控制，不由 bubblewrap 处理

**执行流程**
- R17. `profile_for()` 将 `SandboxCapabilities` 转换为 bubblewrap 命令行参数列表（存储在 `OsSandboxProfile.contents` 中，格式为 JSON 或换行分隔的参数）
- R18. `run()` 使用 `tokio::process::Command` 调用 `bwrap`，传递参数、环境变量、stdin
- R19. 支持 timeout 和 max_output_bytes（与 macOS 实现一致）
- R20. 正确返回 exit_code、stdout、stderr、duration_ms

**安全最佳实践**
- R21. 始终使用 `--new-session` 防止 TIOCSTI 攻击
- R22. 始终使用 `--die-with-parent` 防止孤儿进程
- R23. 默认 `--cap-drop ALL`，仅在需要时添加特定 capability
- R24. 不暴露宿主 `/proc` 中的敏感信息（限制 `/proc` 挂载）

**测试**
- R25. 单元测试：验证各种 Policy 到 bubblewrap 参数的映射
- R26. 集成测试：在 Linux CI 环境中验证实际 sandbox 执行
- R27. GitHub Actions workflow 添加 bubblewrap 安装步骤

## Success Criteria

- `cargo test -p alephcore --lib` 在 Linux 上所有 sandbox 测试通过
- `cargo check -p alephcore` 在 Linux 上无编译错误
- bubblewrap 未安装时优雅降级（`is_supported() == false`）
- 与 macOS 实现保持 API 一致（同样的 `OsSandboxDriverTrait` 接口）

## Scope Boundaries

- **不做** seccomp/landlock helper（纯 bubblewrap 方案）
- **不做** Windows 支持（Phase 3）
- **不做** 旧代码清理（Phase 4）
- **不做** AllowHosts 的精细网络过滤（解析但降级为 None，真正实现在 Phase 2.5）
- **不做** VM/容器级别隔离

## Key Decisions

- **纯 bubblewrap 方案**：不引入 aleph-linux-sandbox helper，降低维护复杂度
- **参数存储格式**：`OsSandboxProfile.contents` 存储为换行分隔的 bubblewrap 参数（而非 JSON），便于直接传递给 Command
- **网络策略简化**：Phase 2 仅支持 None/AllowAll，AllowHosts 解析但降级为 None（有日志警告），真正实现在 Phase 2.5

## Dependencies / Assumptions

- 目标系统已安装 bubblewrap（大多数现代 Linux 发行版默认安装或可通过包管理器安装）
- 内核支持 user namespaces（大多数现代发行版默认启用）
- CI 环境（GitHub Actions ubuntu-latest）可安装 bubblewrap

## Outstanding Questions

### Resolved
- [Affects R12][Decision] AllowHosts 在 Phase 2 中解析并记录 warn 日志，但执行时降级为 `--unshare-net`（完全禁止网络）。理由：
  - Codex 参考实现也未在 Linux 端实现 AllowHosts
  - 真正的 AllowHosts 需要代理层或 iptables，复杂度高
  - Phase 2 核心目标是让 Linux sandbox "能工作"，AllowHosts 推迟到 Phase 2.5
  - 降级行为有日志说明，不会误导用户

### Deferred to Planning
- [Affects R25][Technical] 单元测试中如何模拟 bubblewrap 调用（mock Command 还是使用 dry-run 模式）？
- [Affects R26][Needs research] GitHub Actions 中 bubblewrap 是否需要特殊权限配置（user namespaces）？

## Next Steps

→ `/ce:plan` for structured implementation planning
