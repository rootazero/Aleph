# CLAUDE.md — Aleph Project Constitution (Tier 1)

> **入口分工**：本文件是 **Claude Code** 的项目宪法入口；`AGENTS.md` 是 **Pi / Codex 等非 Claude Code agent** 的入口。两份内容应保持一致——**改一边时记得同步另一边**。Tier 2 引用文件（`docs/reference/*`）从本文件以 `→` 链接按需加载。
>
> **本文件只承载跨子系统约束与红线指针；详情一律在 `docs/reference/*`。** 写进来就是给每次会话付一遍钱。

---

## 🧭 Working Style

### Collaboration

1. **Ask > Assume**: Never make silent assumptions about intent or architecture. If unattended, pick the most reasonable path and explicitly log the assumption before proceeding.
2. **Trade-offs > Blind Simplicity**: Before writing code, state your approach and explicitly call out what this specific design makes harder down the line. Avoid naive solutions that paint us into an architectural corner.
3. **Pragmatic Scope**: Stay on task, but execute or propose sensible refactorings/abstractions that prevent tech debt. Always surface bad code or design smells for separate discussion.
4. **Flag Uncertainty**: Confidence without certainty causes damage. Never guess. If unsure, propose a small, localized, low-risk experiment to validate the hypothesis first.
5. **High-Signal Pushback**: Challenge my ideas only if they introduce significant architectural risk, waste effort, or violate settled industry practices. Offer a forward-thinking, correct alternative. Ignore minor stylistic preferences.
6. **State the Negative**: End every task completion by explicitly listing what you did **not** do, unhandled edge cases, or skipped validations.

### Operational

- 先给方案再写代码；不确定时列出选项，不猜测（呼应 P1 与全局 CLAUDE.md）
- 重大变更前先问，小优化可直接执行
- 回复用中文，代码注释用英文，文档中英双语
- 按需正常使用 cargo（`check` / `test` / `clippy`）——编译与测试验证优先，不再强制节制调用次数

---

## 🛑 Architecture Redlines (R1–R10)

> 最高优先级约束，违反的代码不得合入。**完整例外条款与 R10 棘轮机制** → [REDLINES.md](docs/reference/REDLINES.md)

| # | 红线 | 一句话 |
|---|------|--------|
| **R1** | 大脑与四肢绝对分离 | Core 不调平台 API；走 Bridge IPC |
| **R2** | UI 逻辑唯一源 | 复杂业务 UI 在 Leptos/WASM；Bridge 只做 API 桥接 |
| **R3** | 核心轻量化 | 不为非核心功能引入重三方库；优先 Skill/MCP |
| **R4** | Interface 层禁止业务逻辑 | Channel/Bot/CLI/Panel 纯 I/O |
| **R5** | AI 主动到达 | 通过用户已有工作通道主动送达 |
| **R6** | 一核多端 | Rust Core 是唯一大脑，多端只 I/O |
| **R7** | LLM 主权 | 严禁用确定性代码替代 LLM 推理 |
| **R8** | 工具即一切 | 所有可配置操作暴露为工具 |
| **R9** | 智慧在 Prompt 中 | 中间件智慧迁移到 prompt；prune-the-prompt |
| **R10** | 薄 Harness，笨循环 | 12 文件 / 5 不 / 3 问 / 棘轮机制本身 → [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [`src/harness/CLAUDE.md`](src/harness/CLAUDE.md) |

---

## 🛠 Tech Stack & Do NOT introduce

**核心栈**: Rust Core (tokio + serde) · 记忆 SQLite + sqlite-vec · 接口 JSON Schema (schemars) · Panel Leptos/WASM · 桌面壳 Tauri。

**Do NOT introduce**（基于 R1/R3/R7 推导，违者不得合入）→ [REDLINES.md](docs/reference/REDLINES.md)

---

## 🧭 12-Factor Adoption

**边界**：让模型看见并自愈错误 = 要（A2）；让 harness 替模型挑恢复策略 = 不要（R10 第 5 不）。A3 不得让 `src/harness/` 越过 12 文件棘轮。全文 → [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md)

---

## 🧬 Design Principles (P1–P8)

→ [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) · [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) · [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md)

| P1 低耦合 · P2 高内聚 · P3 可扩展性 · P4 依赖倒置 · P5 最小知识 · P6 简洁性 · P7 防御性 · P8 LLM 优先 |

---

## ⚠️ Engineering Criteria — Shape Index (1–19)

> 认出「我现在踩的是哪一类」。**完整触发器** → [FEATURE_LOCATOR 附录 E](docs/reference/FEATURE_LOCATOR.md) · **全文** → 附录 D · **验证纪律** → 附录 C

| # | 形状名 |
|---|--------|
| 1 | 同一事实的两份表述 |
| 2 | 恒真的谓词等于没判 |
| 3 | 守卫的绿只覆盖它认得的那种形状 |
| 4 | 守卫要断言"效果到达了"，不是"调用发生了" |
| 5 | 列举法只覆盖立法当天的世界 |
| 6 | 先数一遍 |
| 7 | 两端完整而中间没线 |
| 8 | fail-closed 的答案被当成值消费，就反转成许可 |
| 9 | 一个动词有几张脸，判据就要在每张脸上用同一个推导 |
| 10 | 跨 crate 的 wire 契约，两边各持一份形状就会互相抵消 |
| 11 | 一个"报成功的 no-op" |
| 12 | 顺序 / 单位 / 边界必须在同一处派生 |
| 13 | 一个上限 / 棘轮 / 信号量，位置与寿命决定它约束什么 |
| 14 | 闸的两个方向都要问 |
| 15 | 不可逆边界与一次性的动作 |
| 16 | 孪生子系统 / 第 N 次复发 |
| 17 | 一份"展示用"的东西，提交前必须能指出渲染它的那一行代码 |
| 18 | 量具会骗人 |
| 19 | 一次加宽，把每一处"这里只会有一个"同时变成缺陷点 |

---

## 📍 Subsystem Routing

[→ docs/reference/SUBSYSTEM_ROUTING.md](docs/reference/SUBSYSTEM_ROUTING.md)

> 25 行大表（按你要动的目录 → 先读哪个 ref / 哪个判据 / 哪个真机 QA）。**真机 QA 装置每个阶段在证明什么**见 [`qa/README.md`](qa/README.md)。

---

## 🔧 Development Guide

[→ docs/reference/DEVELOPMENT.md](docs/reference/DEVELOPMENT.md)

涵盖：构建命令 · 七条最小可信验证集 · 工具链与版本（MSRV / CalVer）· 会话旋钮 · 分发形态与信任模型 · Feature Flags / 提交 / 进程管理 · 内置文件与 Shell 工具

---

## 🏢 Workspace & Repositories

[→ docs/reference/WORKSPACE.md](docs/reference/WORKSPACE.md)

**TBU4 警告**：周边仓在 `/Volumes/TBU4/Workspace/`，**经常未挂载**；会话工作检出、git root、编辑落点一律是 `/Volumes/TBU/Workspace/Aleph`。

---

## 🧠 Long-term Memory & Hooks

[→ docs/reference/WORKSPACE.md](docs/reference/WORKSPACE.md)

- 走各自 agent 的全局 memory（Claude Code = `~/.claude/...`，Pi 走 Pi 自身全局库）；**不在项目内另造 MEMORY.md**。
- 当前**未挂** hooks 目录；规则靠模型遵守。

---

## 🐍 Python & Code Style

[→ docs/reference/STYLE.md](docs/reference/STYLE.md)

涵盖：rustfmt / clippy · 命名 / thiserror vs anyhow · 不变性 · Python Toolchain（uv + `.venv`）

---

## 🔧 Tool Usage

[→ docs/reference/WORKSPACE.md](docs/reference/WORKSPACE.md) · [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [FEATURE_LOCATOR §3.4](docs/reference/FEATURE_LOCATOR.md)

---

## 📚 Document Index

[FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md) 是所有「我应该读哪个 doc」的入口。常用 reference：

- [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) · [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) · [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md)
- [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [SANDBOX.md](docs/reference/SANDBOX.md) · [SECURITY.md](docs/reference/SECURITY.md)
- [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md) · [RELEASE.md](docs/reference/RELEASE.md) · [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md)
- [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) · [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [SESSION_KNOBS.md](docs/reference/SESSION_KNOBS.md)
- [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) · [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) · [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md)
- [GLOSSARY.md](docs/reference/GLOSSARY.md) · [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md) · [PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md)

---

## 🧩 Agent skills (Claude Code plugin)

[→ docs/agents/README.md](docs/agents/README.md)

> `mattpocock-skills` plugin（`/to-issues` `/triage` `/to-prd` `/diagnose` `/tdd` `/grill-with-docs` `/code-review` 等）不随本仓库分发，详情见上。

## 🧩 Capability Phase 4 入口

- 契约与事实源：`src/capability/{descriptor,facade,backend,ownership,effect_claim}.rs`（9 类 `CapabilityKind`、`ResolveError` 闭集、`validate_lease` 三道 fail-closed 判据、`Scope.visibility` / `Reference.visibility`、`BackendLease { descriptor, owner_generation }`、`OwnershipTree::generation(&CapabilityId, &VisibilityScope) -> Option<OwnerGeneration>` 只读 accessor、`to_descriptor` sha2 canonical fingerprint）。
- Concrete host（committed）：`src/capability/zahir_facade.rs::ZahirFacade` 组合 `ToolBackendAdapter` + `Arc<OwnershipTree>`，`describe / resolve / validate_lease / subscribe / project` 全部落地，无第二份 descriptor/handler/owner map；`ToolBackendAdapter::snapshot_capabilities()`（`src/capability/backend.rs`）单次 `snapshot_state()` 读 `(Vec<CapabilityDescriptor>, revision)`，`describe / subscribe / project` 同代。`reconcile_registry_bindings` 现走 `OwnershipTree::register_if_absent`（crate-private，单 mutex 下 `entry.or_insert_with`），observe + insert 同锁，incumbent owner / lifetime / generation / claims 保留；public `register` 仍是无条件 replacement；同 Revoked / Disposed guard fail-closed，命中即 no-op——TOCTOU 关窗，无第二份 owner map；**不**等价于 host 已 runtime mount / EffectClaim producer 已完备。**全仓无生产 `ZahirFacade::new` 构造/挂载调用点**（grep 确认仅 `src/capability/mod.rs` 模块声明）；runtime mount **未完成**，生产挂载需下一次独立 architecture approval。
- 恢复与原子边界：reducer fail-closed pinned（`src/capability/effect_claim.rs`：`duplicate_claim_and_illegal_migration_remain_fail_closed`）+ `SessionService::emit_batch` 默认 `Err`、唯一生产 override `InProcessActorSessionService` + `SessionEventStore::append_batch` 单事务（已审计，committed）。`StateDatabase` / ACP JSON / MCP 类型与工具视图是 projection-only，`GlobalBus` 是 notification-only。
- `CapabilityBackend::generation()` 与 `CapabilityDescriptor.owner_generation` 已删去（旧权威回收）；generation 权威只在 `BackendLease.owner_generation` + `OwnershipTree::generation()`。
- 订阅接真实 broadcast（committed）：`ZahirFacade::subscribe` 先 `ToolHandlerRegistry::subscribe()` 再 `describe`，drain 进纯值 `CapabilityChangeStream`；broadcast 是 live notification（256 槽、lag 可丢），**不是** durable recovery source；durable recovery 只认 `SessionEventStore`。`subscribe` 的 caller cursor 被**忽略**（同步 snapshot-anchored API 无 replay，不是去重/过滤 hint），cursor 超过 snapshot 时 fail-closed 锚定到 snapshot revision，不伪造逐条恢复。
- `to_metadata_form`（`src/tools/service.rs:453`）仍是旧 Tool compatibility wrapper。**EffectClaim producer integration** deferred（无生产 `SessionEvent::EffectClaim*` 构造点，下期独立 approval）；automatic Safe Replay、external-effect exactly-once、universal durable scheduler、完整 ACP server gate、其余 kind backend 均 deferred。详情见 [FEATURE_LOCATOR §3.5d](docs/reference/FEATURE_LOCATOR.md)。

---

*Last updated: 2026-10-06*
