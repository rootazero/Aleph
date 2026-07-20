# CLAUDE.md 优化设计（保宪法 · 瘦周边 · 补经验）

- **日期**: 2026-06-17
- **类型**: 文档优化（不执行代码修复）
- **参考**: 《写好 CLAUDE.md 的 8 条经验：让 Claude Code 更懂你的项目》
- **取向**: 保宪法、瘦周边 —— R1-R10 架构红线 + P1-P8 设计原则全文保留为不可删的"宪法"；只压缩/外迁周边操作性内容，并补齐文章中本项目尚未用上的经验。

## 背景与现状核对

实地核对结果（2026-06-17）：

- 当前根 `CLAUDE.md` = **350 行**，超出文章建议的 200 行上限。
- 全仓库**只有一个 CLAUDE.md**（根目录），无任何子目录本地 CLAUDE.md —— 经验 #5 完全未用上。
- **无 `.claude/hooks/`** —— 经验 #6 未用上。
- **无根目录 MEMORY.md**；长期记忆走全局 `~/.claude/.../memory/`（Git 不追踪的全局记忆）—— 经验 #7 形态不同。
- `docs/reference/` 下已有 **48 个文档** —— 经验 #4「指针而非图书馆」的基础设施已齐全。

### 核心张力与决策

文章第 1 条「越短越好，200 行上限，不值得留的就删」是针对"塞满公司历史/营销文案的虚胖文档"。本项目的 CLAUDE.md 不是虚胖，而是塞满了**刻意常驻上下文、防止架构被违反的宪法级约束（R1-R10 / P1-P8）**。因此：

- **R1-R10、P1-P8 视为不可删的宪法，全文保留。**
- 200 行硬上限在"保宪法"前提下不强求达标；目标 **210–250 行**，每行都是宪法级或路由级。强行压到 200 会牺牲红线完整性，不值得。

## 8 条经验 × 本项目落地决策

| # | 经验 | 当前状态 | 本次动作 |
|---|---|---|---|
| 1 | ≤200 行 | 350 行 | 瘦周边外迁，目标 210–250 行（保宪法权衡，不强求 200） |
| 2 | Do NOT introduce | 缺失 | 新增「Tech Stack & Do NOT introduce」区块 |
| 3 | 规则可操作 | 红线已较可操作 | 保留；新增条目均写成可验证指令 |
| 4 | 指针而非图书馆 | 文档索引已存在 | 强化为 Context Tiers；周边详解外迁留指针 |
| 5 | 敏感模块本地 CLAUDE.md | 无 | 新建 `src/harness/CLAUDE.md` + `src/gateway/CLAUDE.md` |
| 6 | Hook 驱动 | 无 hooks | **仅文字声明现状**，不新建配置 |
| 7 | MEMORY.md 长期记忆 | 走全局 memory | **仅文字声明现状**，不在项目内另造 MEMORY.md |
| 8 | 工作风格开场白 | 仅「语言规范」 | 合并增强为「My Working Style」 |

## A. 根 CLAUDE.md 目标结构

```
🛑 架构红线 R1-R10                 ← 全文保留（不动）
🧬 设计原则 P1-P8                   ← 全文保留（不动）
🛠 Tech Stack & Do NOT introduce   ← 【新增 #2】
🎯 My Working Style                ← 【新增/增强 #8】合并现有「语言规范」+ 工作节奏
🔧 开发指南（精简）
   ├─ 构建命令（表格，保留）
   ├─ Rust 工具链（保留 2 行 + 指针）
   ├─ Feature Flags / 提交规范 / 分支策略（保留，本就是可执行规则）
   └─ 指针区：发版/Windows/进程管理/Panel嵌入链/Worktree → docs/*
📚 文档索引（强化为 Context Tiers #4）
🏢 官方仓库（保留）
🧠 Memory & Hooks（文字声明现状 #6/#7）
```

## B. 瘦周边：外迁清单

**关键执行约束**：外迁前先核对目标文档是否已覆盖该内容——已覆盖则 CLAUDE.md 里直接删 + 留指针；未覆盖则先把内容搬过去再留指针。**绝不丢失信息**。逐块验证。

| 当前 CLAUDE.md 中的块 | 处理 | 目标文档 |
|---|---|---|
| Windows 构建（大表 + 步骤） | 外迁→指针 | `docs/reference/WINDOWS_RUNTIME.md`（已存在） |
| 分发形态长段 | 外迁→指针 | `docs/reference/PRODUCT_TOPOLOGY.md`（已存在） |
| ⚠️ Panel↔Daemon 资源嵌入链 | 外迁→指针 | `docs/reference/DESKTOP_SHELL.md`（已存在） |
| 发版流程详解 | 外迁→指针，CLAUDE.md 保留 `just release` 一行 | **新建 `docs/reference/RELEASE.md`** |
| 进程管理（Spec C 详解） | 外迁→指针 | **新建 `docs/reference/PROCESS_MANAGEMENT.md`** |
| Git Worktree 注意事项 | 外迁→指针 | 折进 `docs/reference/CODE_ORGANIZATION.md` 小节 |

## C. 新增内容

### C1. Tech Stack & Do NOT introduce（基于 R1/R3/R7 推导）

```
Do NOT introduce unless explicitly requested:
- 第二个 async runtime（async-std/smol）—— 全栈锁定 tokio
- 独立向量数据库 client 进 core（qdrant/lancedb/...）—— 记忆层已锁 sqlite + sqlite-vec
- src 中直接依赖平台 API crate（windows-rs/core-graphics/cocoa/objc/winapi）
  —— 违 R1，必须走 Bridge IPC
- 正则/规则引擎做意图识别/路由 —— 违 R7，语义判断交 LLM
- 非 serde 的序列化栈 —— 全栈 serde
```

> 实现阶段：先用 `cargo tree` / 检索现有依赖核对上述锁定项是否属实，再定稿条目，避免凭空 ban。

### C2. My Working Style（合并现有「语言规范」+ 文章范式）

```
- 先给方案再写代码；不确定时列选项，不猜测（呼应 P1 / 全局 CLAUDE.md）
- 重大变更前先问，小优化可直接执行
- 回复用中文，代码注释用英文，文档中英双语
- 极度节制 cargo 调用（系统负担）—— 默认不跑全量测试
- 单分支开发：所有工作直接在 main
```

## D. 子目录 CLAUDE.md

### D1. `src/harness/CLAUDE.md`（R10 护栏前移）

内容（全部从根 CLAUDE.md R10 段提炼，不新造约束）：
- 12 文件 / ~4900 行硬上限及文件清单
- 「加代码前必答 3 问」
- 循环里的 5 个"不"
- 新增文件须在 PR 说明为何装不进现有 12 文件之一
- 指针：`docs/reference/HARNESS_PHILOSOPHY.md`

### D2. `src/gateway/CLAUDE.md`（安全边界护栏）

内容（从根 CLAUDE.md Trust model 段 + 安全相关红线提炼）：
- LAN-trust 信任模型 = 网络边界，无认证步骤，默认只绑 127.0.0.1
- device tier 提权门（`method_authz.rs`）：远程 Panel 默认 Chat tier，self_config 类 RPC 须 operator
- WS Origin 校验（`origin_policy.rs`）唯一保留的协议护栏
- 红线：改认证/授权逻辑必须同步更新测试
- 指针：`docs/reference/SECURITY.md`

## E. Memory & Hooks 文字声明（不建文件）

根 CLAUDE.md 加一小节，纯文字说明，不新建任何可执行配置/文件：
- 长期记忆走全局 `~/.claude/.../memory/`，不在项目内另造 MEMORY.md
- 质量门未来可由 `.claude/hooks/` 强制执行（当前未挂）

## F. 交付物清单

1. 优化后的根 `CLAUDE.md`（210–250 行）
2. `src/harness/CLAUDE.md`（新建）
3. `src/gateway/CLAUDE.md`（新建）
4. `docs/reference/RELEASE.md`（新建，承接发版流程详解）
5. `docs/reference/PROCESS_MANAGEMENT.md`（新建，承接 Spec C 进程管理详解）
6. 外迁内容搬运：Windows 构建 / 分发形态 / Panel 嵌入链 / Worktree 各自落到既有文档（逐块核对覆盖度）
7. 文档索引同步：新建的 RELEASE.md / PROCESS_MANAGEMENT.md 加入根 CLAUDE.md 文档索引表

## 非目标（YAGNI）

- 不执行任何代码修复。
- 不新建 `.claude/hooks/` 配置或脚本。
- 不在项目内新建 MEMORY.md。
- 不改动 R1-R10 / P1-P8 任何一字。
- 不做计划外的文档重构。

## 验证标准

- 根 CLAUDE.md 在 210–250 行内，R1-R10 / P1-P8 逐字未改。
- 一个没看过项目的人读完根 CLAUDE.md 能 30 秒回答：这是什么产品？技术栈是什么？新代码放哪里？
- 每条外迁块在目标文档中可查到（无信息丢失）。
- 新增的 Do NOT introduce 条目均可在 5 秒内判定一段代码是否违反。
- `src/harness/` 与 `src/gateway/` 各有本地 CLAUDE.md，内容均来自既有根文档提炼，无新造约束。
