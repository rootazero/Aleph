# Discord Channel R1 — 2026-09-27

## TL;DR

Aleph Discord 通道（`src/gateway/interfaces/discord/`，2,687 行 Rust）相对 openclaw
Discord extension（`/home/zou/mnt/macmini/TBU4/Github/openclaw/extensions/discord/`，~175k 行 TS）
在**接线深度**上有明确缺口，在**架构路径**上故意不学。本轮做 R1：**连线优先，扩展其次，熵减同步**。

**R1 不复制 openclaw 的 plugin-SDK 形态**（架构不匹配 R7 / one-core-many-channels）；
不重写 serenity 客户端；不补 voice 子系统（复用既有 `src/gateway/voice/`）。

## 现状（已通过 explore 验证）

| 维度 | 现状 | 证据 |
|---|---|---|
| **注册路径** | `register_plain_channel!("discord", DiscordChannelFactory)` | `src/gateway/interfaces/mod.rs:119` |
| **状态机** | `ChannelStatus::{Disconnected, Connecting, Connected, Error}` 正确切换 | `mod.rs:326,667,732,748,784` |
| **Health monitor** | 已修复（`matches!(status, Error \| Disconnected \| Connecting)`），覆盖 Discord | `channel_health_monitor.rs:108` + 注释明确说明修复动机 |
| **Guild 权限审计** | `permissions::audit_permissions` 完整（398 行 + 6 个测试）| `permissions.rs:142` |
| **系统级安全审计** | ❌ `src/security/audit.rs`（998 行，22 个 AuthorityChange 动词）discord 完全未调用 | grep 无任何命中 |
| **Approval 流** | ❌ discord 命令 → approval gate → audit trail 三段连线全断 | grep 无任何命中 |
| **Voice** | ❌ `ChannelCapabilities::audio=true` 仅声明，无实现 | `mod.rs:capabilities()` |
| **Doctor 接入** | ❌ `src/diagnostics/checks/` 20+ 检查，discord 无钩入 | grep 无任何命中 |
| **Components-registry** | ❌ 命令注册散在 `mod.rs`，无状态机 | 散在 mod.rs 700+ 行 |
| **草稿/分块** | ❌ `MessageTooLong` 直接截断，无 draft-stream | mod.rs: send 分支 |
| **断线重连** | 已有 `restart_backoff`，但**无 health-monitor 主动扫描验证** | monitor 已能扫到，但需要观察 |

**核心定位**：Discord 是"plain channel"，功能简洁但**从未接进 Aleph 自身的 cross-cutting 基础设施**（audit/approval/doctor/voice）。

## 任务列表

### 线 1：`discord-r1-line1` 分支 — 连线 + 熵减（低风险）

**目标**：让 discord 通道接进 Aleph 既有基建（audit/approval/doctor），不引入新功能。

**文件边界**（避免与线 2 冲突）：
- ✅ 改：`src/gateway/interfaces/discord/security/mod.rs`（填充）
- ✅ 新增：`src/diagnostics/checks/discord_channel_health.rs`
- ✅ 新增：`src/gateway/interfaces/discord/audit_hooks.rs`（audit 钩入单一文件）
- ❌ **不动 `mod.rs` 主流程**（留给线 2）

| ID | 任务 | 锚点 | 验收 |
|---|---|---|---|
| T1.1 | discord 命令执行的 approval gate 接线（exec_approval/gate 落地）| `mod.rs::handle_command` → `src/exec/manager.rs::request_approval` → `src/security/audit.rs::record(AuditEventType::AuthorityChange)` | `cargo test -p alephcore --lib exec_approval` + 新单测 |
| T1.2 | discord channel 接入 doctor 健康检查 | 在 `src/diagnostics/checks/` 新增 `discord_channel_health.rs`（参照 `browser_runtime.rs` 形态），注册到 `mod.rs` 的 check list | `cargo run -p alephcore --bin aleph-server -- doctor` 显示新行；新单测 |
| T1.3 | discord `security/mod.rs` 填充：从空壳到可工作 | 把 `permissions::audit_permissions` 钩入 channel 启动时（`start()` 时一次性扫描报告）+ 接 `AuditEventType::ExecBlocked` 入口 | `cargo test -p alephcore --lib discord_security` |
| T1.4 | 修复 discord 测试已知失败 / 强化覆盖 | 跑 `cargo test -p alephcore --lib discord_*` 全绿；对 mod.rs 内每个 `if let Ok(...)` 加 audit 反例 | 测试报告 |
| T1.5 | 熵减 — 删除 discord 模块内死代码 / 重复 / TODO | 扫 `src/gateway/interfaces/discord/` 内 `unreachable!`/`todo!`/`#[allow(dead_code)]` | 报告前后 LOC 对比 |

### 线 2：`discord-r1-line2` 分支 — 深度重构（高风险）

**目标**：补 voice 接线、components-registry 化、安全审计深化。

**文件边界**（避免与线 1 冲突）：
- ✅ 改：`src/gateway/interfaces/discord/mod.rs` 主流程（voice/draft/reconnect 接线）
- ✅ 新增：`src/gateway/interfaces/discord/{commands,draft,reconnect}.rs`
- ❌ **不动 `security/mod.rs`**（留给线 1）

| ID | 任务 | 锚点 | 验收 |
|---|---|---|---|
| T2.1 | Discord voice 通道接线（复用 `src/gateway/voice/`）| discord `start()` 时检测 voice intent → 调 `gateway/voice/inbound/` + `voice/outbound.rs`；不新建 discord/voice/ | 集成测试：discord 收到 voice frame → voice/ 子系统识别 |
| T2.2 | Components-registry 化（命令注册状态机）| 新建 `src/gateway/interfaces/discord/commands.rs`，把散在 mod.rs 的命令注册集中；引入 `ComponentId` + 自定义 ID codec（仿 openclaw `custom-id-codec.ts` 但精简版）| 命令注册单一入口；新增 ≥3 个测试 |
| T2.3 | 安全审计上下文深化 | discord `send/edit/delete/react` 操作 → `AuditEventType::AuthorityChange`（detail 带 channel_id / message_id / action）；discord `PermissionCheck` → `AuditEventType::ExecBlocked` | 新增 `discord_audit.rs` + 测试 |
| T2.4 | 草稿/分块流式（draft-stream + chunk）| 新建 `src/gateway/interfaces/discord/draft.rs`，长回复按 2000 字符分块（参考 `src/utils/text_format::truncate_reserving`）；Discord "编辑消息"模拟 draft | 集成测试：长消息分块 |
| T2.5 | 断线重连 + presence-cooldown-store | 新建 `src/gateway/interfaces/discord/reconnect.rs`：track last successful gateway event；health monitor 接入；presence cooldown 1s（避免高频重连）| 单测 + 集成验证 health monitor 真能发现 zombie |

## 验收（七条最小验证集）

每条线完成后必须通过：
```bash
CARGO_BUILD_JOBS=2 cargo check -p alephcore 2>&1 | tail -3
CARGO_BUILD_JOBS=2 cargo clippy -p alephcore -- -D warnings 2>&1 | tail -3
CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib discord 2>&1 | tail -10
CARGO_BUILD_JOBS=2 cargo test -p alephcore --test discord_* 2>&1 | tail -10
just test-all 2>&1 | tail -5  # 全部测试
```

## 不做（明确边界）

- ❌ Plugin SDK 化（openclaw 形态不匹配 R7）
- ❌ 自研 client wrapper（serenity 已稳）
- ❌ 新建 `src/gateway/interfaces/discord/voice/`（复用既有 `src/gateway/voice/`）
- ❌ 大规模迁移到 discord.js 风格（语言/架构不匹配）
- ❌ voice 录音 / opus 编解码（属 voice 子系统责任，不属 discord）

## 内存保护

线 2 subagent 每次启动 cargo 前必须：
```bash
free -g | awk '/Mem:/ {print $7}'  # MemAvailable 列
# ≥ 4 才能继续，否则 sleep 60 重试
```

## 合并点

线 1 完成 → 合并到 main → 线 2 任务 T2.1 之前从 main rebase。
线 2 完成 → 合并到 main → 更新 `FEATURE_LOCATOR.md`。