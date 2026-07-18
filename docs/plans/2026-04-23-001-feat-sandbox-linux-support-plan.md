---
title: Phase 2 Linux Sandbox Implementation
type: feat
status: active
date: 2026-04-23
origin: docs/brainstorms/2026-04-23-sandbox-phase2-linux-requirements.md
---

# Phase 2: Linux Sandbox 支持 — Implementation Plan

## Overview

为 Aleph 添加 Linux 平台 sandbox 支持，实现与 macOS 同等级别的隔离能力。使用 bubblewrap 作为底层隔离机制，保持与现有 `OsSandboxDriverTrait` 接口兼容。

## Problem Frame

Phase 1 已完成 macOS SeatbeltDriver，建立了跨平台架构。当前 Linux 仅返回 `UnsupportedDriver`，无法执行 sandbox 命令。需要实现 `BubblewrapDriver` 使 Linux 用户能安全执行 AI 生成的代码。

## Requirements Trace

- R1. 自动检测 bubblewrap 安装状态
- R2. 不可用时优雅降级
- R3. 驱动标识 `"linux/bwrap"`
- R4-R9. 文件系统策略映射（WorkspaceOnly, ReadPaths, WritePaths, FullRead, FullWrite）
- R10-R13. 网络策略（None, AllowAll, AllowHosts 解析但降级）
- R14-R16. 进程策略（fork 控制）
- R17-R20. 执行流程（profile_for, run）
- R21-R24. 安全最佳实践
- R25-R27. 测试覆盖

## Scope Boundaries

- 不做 seccomp/landlock helper（纯 bubblewrap）
- 不做 Windows 支持（Phase 3）
- 不做旧代码清理（Phase 4）
- AllowHosts 仅解析记录，执行时降级为 None
- 不做 VM/容器级隔离

## Context & Research

### 现有代码结构

```
src/sandbox/platforms/
├── mod.rs              # create_platform_driver() 分发
├── common.rs           # 路径归一化、glob 转换
├── macos/
│   ├── mod.rs          # 导出 SeatbeltDriver
│   └── seatbelt.rs     # macOS 实现参考
├── linux/mod.rs        # 【本计划实现】
└── windows/mod.rs      # stub
```

### 关键参考文件

- `src/sandbox/platforms/macos/seatbelt.rs` — 实现模式参考
- `src/sandbox/driver.rs` — `OsSandboxDriverTrait` 定义
- `src/sandbox/policy.rs` — `SandboxPolicy` 结构
- `src/sandbox/platforms/mod.rs` — 平台分发逻辑

### Bubblewrap 关键参数

| 参数 | 用途 |
|------|------|
| `--ro-bind SRC DEST` | 只读挂载 |
| `--bind SRC DEST` | 读写挂载 |
| `--tmpfs DEST` | 内存文件系统 |
| `--unshare-net` | 隔离网络 |
| `--share-net` | 共享宿主网络 |
| `--unshare-pid` | 隔离 PID 命名空间 |
| `--cap-drop ALL` | 丢弃所有 capabilities |
| `--new-session` | 防止 TIOCSTI 攻击 |
| `--die-with-parent` | 父进程退出时终止 |
| `--proc /proc` | 挂载 procfs |
| `--dev /dev` | 挂载 devfs |

## Key Technical Decisions

- **纯 bubblewrap 方案**：不引入 helper binary，降低维护复杂度（见 origin doc 决策）
- **参数存储格式**：`OsSandboxProfile.contents` 存储换行分隔的 bubblewrap 参数，便于直接传给 `Command`
- **AllowHosts 降级**：解析但记录 warn 日志，执行时降级为 `--unshare-net`
- **目录结构保持**：继续使用 `src/sandbox/platforms/linux/` 而非独立 crate

## Implementation Units

- [ ] **Unit 1: BubblewrapDriver 基础结构**

**Goal:** 创建 `BubblewrapDriver` 结构体，实现 `platform()` 和 `is_supported()`

**Requirements:** R1, R2, R3

**Dependencies:** None

**Files:**
- Create: `src/sandbox/platforms/linux/bwrap.rs`
- Modify: `src/sandbox/platforms/linux/mod.rs`

**Approach:**
- 创建 `BubblewrapDriver` 结构体
- 实现 `platform()` 返回 `"linux/bwrap"`
- 实现 `is_supported()` 检测 `/usr/bin/bwrap`、 `/usr/local/bin/bwrap` 和 PATH
- 添加常量 `BWRAP_PATH` 作为可执行文件路径

**Patterns to follow:**
- 参考 `macos/seatbelt.rs` 中的 `SeatbeltDriver` 结构
- 使用 `std::path::Path` 进行路径检测

**Test scenarios:**
- Happy path: bwrap 存在时 `is_supported()` 返回 true
- Edge case: bwrap 不存在时返回 false
- Edge case: 只检测信任的固定路径，防止 PATH 注入

**Verification:**
- `cargo check -p alephcore` 在 Linux 上通过
- 单元测试通过

---

- [ ] **Unit 2: 策略到 bubblewrap 参数转换**

**Goal:** 实现 `profile_for()`，将 `SandboxPolicy` 转换为 bubblewrap 参数

**Requirements:** R4-R13, R17

**Dependencies:** Unit 1

**Files:**
- Modify: `src/sandbox/platforms/linux/bwrap.rs`

**Approach:**
- 实现 `generate_args(policy, cwd) -> Vec<String>` 方法
- 文件系统策略映射：
  - `WorkspaceOnly`: `--dir /tmp --tmpfs /tmp`, `--bind {cwd} {cwd}`, `--chdir {cwd}`
  - `ReadPaths`: 每个路径添加 `--ro-bind path path`
  - `WritePaths`: 每个路径添加 `--bind path path`
  - `FullRead`: `--ro-bind / /` + `--tmpfs exclude` + `--remount-ro exclude`
  - `FullWrite`: `--bind / /` + 排除路径处理
- 网络策略映射：
  - `None`: `--unshare-net`
  - `AllowAll`: `--share-net`
  - `AllowHosts`: `--unshare-net` + `warn!` 日志说明降级
- 进程策略：
  - `allow_fork = false`: `--unshare-pid --cap-drop ALL`
  - `allow_fork = true`: 不添加 `--unshare-pid`
- 安全参数：始终添加 `--new-session --die-with-parent --unshare-user`
- 挂载：添加 `--proc /proc --dev /dev`

**Patterns to follow:**
- 参考 `macos/seatbelt.rs` 中的 `generate_profile()` 方法结构
- 使用 `std::path::Path` 处理路径

**Technical design:**
```rust
// 参数构建顺序（重要！bubblewrap 按顺序应用）
1. --new-session --die-with-parent --unshare-user
2. --unshare-pid（如果 fork=false）
3. --unshare-net 或 --share-net
4. --cap-drop ALL（如果 fork=false）
5. 文件系统挂载（从根到子路径）
6. --proc /proc --dev /dev
7. --chdir {cwd}
8. --
9. [program, args...]
```

**Test scenarios:**
- Happy path: WorkspaceOnly 生成正确参数列表
- Happy path: ReadPaths 包含所有指定路径的 `--ro-bind`
- Happy path: AllowAll 网络使用 `--share-net`
- Edge case: 路径包含空格或特殊字符正确转义
- Edge case: AllowHosts 记录 warn 日志并降级为 `--unshare-net`

**Verification:**
- 单元测试验证参数生成逻辑
- 参数顺序符合 bubblewrap 要求

---

- [ ] **Unit 3: run() 执行实现**

**Goal:** 实现 `run()` 方法，使用 tokio 调用 bubblewrap

**Requirements:** R18-R20

**Dependencies:** Unit 2

**Files:**
- Modify: `src/sandbox/platforms/linux/bwrap.rs`

**Approach:**
- 解析 `OsSandboxProfile.contents` 为参数列表
- 使用 `tokio::process::Command` 构建命令
- 设置当前目录、环境变量、stdin
- 使用 `tokio::time::timeout` 实现超时
- 收集 stdout/stderr，限制输出大小
- 返回 `SandboxOutput` 结构

**Patterns to follow:**
- 参考 `macos/seatbelt.rs` 中的 `run()` 实现
- 使用相同的 timeout 和 output truncation 逻辑

**Test scenarios:**
- Happy path: 成功执行命令返回正确输出
- Error path: 命令不存在返回错误
- Error path: 超时返回 Timeout 错误
- Edge case: 输出超过 max_output_bytes 正确截断

**Verification:**
- 集成测试通过（在 Linux 环境）

---

- [ ] **Unit 4: 注册 Linux 驱动**

**Goal:** 更新 `create_platform_driver()` 使用 `BubblewrapDriver`

**Requirements:** R1-R3

**Dependencies:** Unit 1-3

**Files:**
- Modify: `src/sandbox/platforms/mod.rs`

**Approach:**
- 在 `#[cfg(target_os = "linux")]` 分支中
- 将 `Arc::new(UnsupportedDriver)` 替换为 `Arc::new(linux::bwrap::BubblewrapDriver::new())`
- 确保 `linux::bwrap` 模块已导出

**Patterns to follow:**
- 参考 macOS 分支的实现方式

**Test scenarios:**
- Integration: Linux 平台创建驱动成功
- Integration: 驱动类型正确

**Verification:**
- `cargo check -p alephcore` 在 Linux 上通过

---

- [ ] **Unit 5: 单元测试**

**Goal:** 为 BubblewrapDriver 添加单元测试

**Requirements:** R25

**Dependencies:** Unit 1-4

**Files:**
- Modify: `src/sandbox/platforms/linux/bwrap.rs`（添加 `#[cfg(test)]` 模块）

**Approach:**
- 测试 `is_supported()` 的检测逻辑
- 测试各种 Policy 到参数的映射
- 使用 mock 或参数验证模式（不实际执行 bwrap）

**Test scenarios:**
- Happy path: 各种 FsPolicy 生成正确参数
- Happy path: 各种 NetworkPolicy 生成正确参数
- Edge case: 空路径列表处理
- Edge case: 特殊字符路径处理

**Verification:**
- `cargo test -p alephcore --lib sandbox::platforms::linux` 通过

---

- [ ] **Unit 6: GitHub Actions 配置**

**Goal:** 在 CI 中安装 bubblewrap 并运行测试

**Requirements:** R26, R27

**Dependencies:** Unit 5

**Files:**
- Modify: `.github/workflows/ci.yml`（或创建 `.github/workflows/sandbox-linux.yml`）

**Approach:**
- 在 Linux runner 上安装 bubblewrap（`sudo apt-get install bubblewrap`）
- 确保 sandbox 测试在 CI 中运行
- 检查是否需要 user namespaces 配置

**Test scenarios:**
- Integration: CI 中 bubblewrap 安装成功
- Integration: CI 中 sandbox 测试通过

**Verification:**
- GitHub Actions workflow 成功

## System-Wide Impact

- **API 兼容性**: 保持 `OsSandboxDriverTrait` 不变，WorkspaceSandbox 无需修改
- **平台检测**: `create_platform_driver()` 在 Linux 上现在返回可用驱动
- **错误处理**: bubblewrap 执行失败时返回 `SandboxError::ExecutionFailed`
- **日志**: AllowHosts 降级时记录 `warn!` 日志

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| GitHub Actions 中 bubblewrap 需要特权 | 使用 `ubuntu-latest` 默认配置，如需 user namespaces 则添加配置 |
| 路径转义问题 | 使用 Rust 标准库处理，避免 shell 注入 |
| bubblewrap 版本差异 | 使用通用参数（避免新版本特有参数） |

## Open Questions

### Deferred to Implementation
- [Affects Unit 6] GitHub Actions 中是否需要特殊权限配置 user namespaces？（实现时验证）
- [Affects Unit 2] FullRead/FullWrite 的排除路径处理是否需要 `--tmpfs` 或 `--ro-bind-data`？（参考 codex 实现）

## Documentation / Operational Notes

- 更新 `src/sandbox/README.md`（如存在）说明 Linux 支持
- 在配置文档中说明 bubblewrap 是可选依赖

## Sources & References

- **Origin document:** `docs/brainstorms/2026-04-23-sandbox-phase2-linux-requirements.md`
- **Reference implementation:** `src/sandbox/platforms/macos/seatbelt.rs`
- **Bubblewrap docs:** https://github.com/containers/bubblewrap
- **Codex bwrap:** `/Volumes/TBU4/Github/codex/codex-rs/linux-sandbox/src/bwrap.rs`
