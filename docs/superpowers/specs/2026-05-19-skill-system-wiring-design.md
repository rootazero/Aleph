# Skill 系统连线与增强 — 设计文档

- **日期**: 2026-05-19
- **分支**: `skill-system-wiring`
- **状态**: 已批准设计，待 writing-plans
- **参考对象**: `/Volumes/TBU4/Github/hermes-agent`（Python 超级 AI 助手，成熟的单一 Skills 体系）

---

## 1. 背景与问题陈述

Aleph 的 Skill 基础设施大部分已实现，但**关键连线断裂**，导致核心功能在生产中完全失效。

通过对 hermes-agent Skills 实现与 Aleph 现状的深度对比探索，确认 Aleph 当前存在 **3 套互不通信的 skill 子系统**外加第 4 种"记忆事实型 skill"：

1. **`src/skill/`（v2 Skill System）** — 领域驱动、eligibility 感知、snapshot/registry/prompt-XML。数据模型 `domain::skill::SkillManifest`。
2. **`src/builtin_tools/skill_*`** — LLM 可调用工具（`skill_read`/`skill_list`/`skill_status`/`skill_install`/`skill_manage`）。
3. **`src/tools/markdown_skill/`（Markdown Skill System）** — 运行时可加载的可执行 CLI 工具，ClawHub 兼容。数据模型 `AlephSkillSpec`。
4. **`NoteType::Skill` 记忆事实** — 自习得 skill 持久化为记忆事实（已正常工作，本轮不动）。

### 1.1 核心病灶：prompt 注入主干断电

`SkillInstructionsLayer`（`src/thinker/layers/skill_instructions.rs`，已注册为 prompt 层，优先级 1050，注入逻辑完整正确）读取 `PromptConfig.eligible_skills` 与 `PromptConfig.skill_instructions`。但这两个字段在**所有生产路径中恒为 `None`**——只有单元测试会赋值。

`harness_bridge.rs::build_system_prompt` 构造 `PromptConfig { native_tools_enabled: true, ..PromptConfig::default() }`（`src/orchestrator/harness_bridge.rs:466`），`eligible_skills` 默认 `None`。

`run_loop.rs:90` 已经算出 eligible-skills snapshot，却绑定到 `_eligible_skills`（下划线弃用变量）直接丢弃：

```rust
let _eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>> =
    if let Some(ext_manager) = extension_manager.as_ref() {
        let snapshot = ext_manager.skill_system().current_snapshot().await;
        ...
    };
```

**后果**：`<available_skills>` XML 从不进入 system prompt，LLM 根本"感知不到"任何 skill。v2 skill 体系的核心价值（让 LLM 自主发现并加载 skill）在生产中等于零。

### 1.2 完整缺陷清单

| 编号 | 类型 | 问题 | 证据 |
|---|---|---|---|
| **G1** | 断线 | `SkillInstructionsLayer` 永不被喂数据；`PromptConfig.eligible_skills`/`skill_instructions` 仅测试赋值 | `harness_bridge.rs:466`、`prompt_builder/mod.rs:117,120` |
| **G2** | 断线 | `run_loop.rs` 算出 snapshot 后丢弃（`_eligible_skills`、`_skill_system`） | `run_loop.rs:90,107` |
| **G3** | 死代码 | `SkillPrefetcher` 零生产构造，唯一实现是测试 `MockSource` | `src/skill/prefetch.rs`；`think.rs:52` 死调用 |
| **G4** | 断线 | `skill_status`/`skill_install`/`skill_manage` 用 `SkillSystem::new()` 建空且未 `.init()` 的实例 | `constructor.rs:751`、`definitions.rs:646-656` |
| **G5** | 断线 | `markdown_skill` 安装进 `Lazy` 静态 `MARKDOWN_SKILLS_SERVER`，无 dispatcher/executor 读取它 | `gateway/handlers/markdown_skills.rs:24` |
| **G6** | 死代码 | `EvolutionAutoLoader`/`SkillWatcher`/`MarkdownSkillGenerator` 零非测试消费者 | `src/tools/markdown_skill/` |
| **G7** | 未实现 | `eligibility.required_config` 解析但 `evaluate_spec` 只 log "not yet implemented" | `src/skill/eligibility.rs:115-121` |
| **B1** | bug | `skill_read` 的 `list_skill_files` 只列顶层文件、`validate_file_name` 拒绝 `/`，自带 skill 的 `references/*.md` 永远读不到 | `src/builtin_tools/skill_reader.rs` |
| **B2** | bug | `ReadSkillTool`/`ListSkillsTool` 各有两套互相矛盾的 `NAME`/`DESCRIPTION` 常量（inherent const vs trait const） | `skill_reader.rs:89-111` vs `339-346` |
| **B3** | bug | Host 沙箱 `NetworkMode::None` 只设 `NO_PROXY=*`，未真正隔离网络，SKILL.md 声明的保证未兑现 | `src/tools/markdown_skill/executor.rs:57-64` |
| **B4** | bug | 因 G4，`skill_status` 永远返回 `total: 0`，向用户/LLM 报告错误信息 | G4 的衍生症状 |

> **未纳入本轮**：G8（`init_unified/coordinator.rs::install_skills` 建 `SkillSystem` 后丢弃）属验证用途的轻微浪费，与主干无关，列为观察项。

---

## 2. 目标与非目标

### 2.1 目标

- **修复连线**：让 v2 skill 经由 prompt 注入真正被 LLM 感知（G1/G2）。
- **统一数据源**：builtin skill 工具消费同一个已初始化的 `SkillSystem`（G4/B4）。
- **连线可执行子系统**：clawhub 安装的 CLI skill 变为 LLM 可调用工具（G5/G6）。
- **修 bug**：references/ 渐进披露（B1）、重复常量（B2）、网络声明诚实化（B3）、`required_config` 门控（G7）。
- **移植 hermes 增强**：使用计数、`skill_read` 冲突检测、安装期安全扫描。
- **清理死代码**：删除 `SkillPrefetcher`（G3）。

### 2.2 非目标（明确排除）

- **不统一 3 套 skill 数据模型** — 那是破坏性重构，违反"不作破坏性重构"约束。3 套子系统就地保留；只消除"意外重复"（如 builtin 工具的重复发现逻辑），保留"本质差异"（prompt-content skill 与可执行 CLI skill 是两类不同事物）。
- **不动 `NoteType::Skill` 记忆事实路径** — 已正常工作（`recaller.rs::format_skills_prompt` 消费），不在范围内。
- **不实现完整网络命名空间隔离** — B3 只做诚实降级（记 warning / 要求 Docker），完整 `unshare(CLONE_NEWNET)` 隔离留作后续。
- **不重写 hermes 的 110 条威胁正则** — 安装期扫描移植精选子集 + 结构检查，不照搬。

---

## 3. 方法选择

| 方案 | 评价 |
|---|---|
| A. 统一 3 套 skill 数据模型为 1 套 | ❌ 破坏性重构，违反约束 |
| **B. 各子系统就地连线 + 修 bug + 移植增强** | ✅ **采纳** — 非破坏性，精准命中"基础设施在、缺连线"判断 |
| C. 只修 v2，放弃 markdown_skill | ❌ 用户已选择连线 markdown_skill |

**采纳方案 B**。核心原则：不统一数据模型，只接通断线 + 修错 + 增强。

---

## 4. 实施设计：4 个 Phase

### Phase 1 — v2 prompt 注入主干连线（核心）

**修复后的数据流**：

```
SkillSystem（启动时 .init(default_skill_dirs())，共享 Arc 单例）
  → current_snapshot().eligible_manifests
  → PromptConfig.eligible_skills              ← 当前断点（G1/G2）
  → SkillInstructionsLayer.inject()           ← 逻辑已完整，无需改
  → <available_skills> XML 进 system prompt
  → LLM 感知 skill → 调 skill_read 加载
```

**改动**：

1. **共享单例**：确立一个启动时 `.init(default_skill_dirs())` 的共享 `Arc<SkillSystem>` 作为唯一数据源。`ExtensionManager` 已持有一个 `skill_system()`（`run_loop.rs:92` 已在用），核实其是否已 `.init()`；若已 init，直接复用为单例。
2. **接通注入点**：在 harness prompt 装配路径（`harness_bridge.rs::build_system_prompt`，`PromptConfig` 构造处 `:466`）取得 snapshot 的 `eligible_manifests`，填入 `PromptConfig.eligible_skills`。
3. **消除重复**：`run_loop.rs:90` 的 `_eligible_skills` 计算逻辑——要么接通真正的注入路径，要么删除（取决于 `run_loop.rs` 是否为活路径；该文件头部有 `#![allow(dead_code)]`，plan 阶段须先确认 `run_loop` vs `harness` 哪条是生产路径）。
4. **清理 G3**：删除 `src/skill/prefetch.rs`（`SkillPrefetcher`/`SkillDiscoverySource`/`SkillInfo`，零生产消费者）+ `harness/agent/think.rs:52` 的死调用 `prefetcher.start_scan()` + 所有 `skill_prefetcher: None` 字段（`orchestrator_init.rs:163`、`subagent_spawner/mod.rs:323`、`harness/agent.rs` 多处）。snapshot 路径（注册表内存过滤，廉价）取代 prefetcher。

**风险**：双 prompt 路径（`harness_bridge` vs `run_loop`）需在 plan 阶段精确定位真正接通点。若 `run_loop.rs` 整体为遗留死代码，则其 snapshot 计算一并删除。

**验证**：集成测试 — 给定已 init 且含 system-scope skill 的 `SkillSystem`，断言装配出的 system prompt 包含 `<available_skills>` 与该 skill 名。

---

### Phase 2 — builtin skill 工具统一数据源（G4 + G7 + B4）

**问题**：`skill_status`/`skill_install`/`skill_manage` 各自 `SkillSystem::new()` 建空实例（`constructor.rs:751` 无 `.init()`），与 gateway RPC 路径（`gateway/handlers/skills.rs::shared_system` 正确 init）分叉 → `skill_status` 永报 0（B4）、`skill_manage` 配置无人读取的注册表。

**改动**：

1. 三个 builtin 工具构造时**注入 Phase 1 的共享 `Arc<SkillSystem>`**，删除内部的 `SkillSystem::new()`。
2. `ListSkillsTool` 改为读共享注册表（`SkillSystem` 的 registry/snapshot），不再自己 `get_all_skills_dirs` + 重新 scan+parse → 消除一处发现逻辑重复（非破坏性，对外行为不变且更准）。
3. **G7**：实现 `eligibility.rs::evaluate_spec` 中的 `required_config` 检查——读取实际配置，缺失时产出已存在的 `IneligibilityReason::MissingConfig`（该变体与 `SkillStatusEntry::build` 处理逻辑均已就绪，只差产出端）。

**验证**：单元/集成测试 — `skill_status` 在已 init 的系统上返回 `total > 0`；声明 `required-config` 的 skill 在配置缺失时被标记 `MissingConfig` 而非 eligible。

---

### Phase 3 — markdown_skill 可执行子系统连线（G5 + G6 + B3）

**问题**：`MARKDOWN_SKILLS_SERVER`（`Lazy` 静态）安装 CLI skill 后无任何 dispatcher/executor 读取；`SkillWatcher`/`EvolutionAutoLoader`/`MarkdownSkillGenerator` 零消费者。

**改动**：

1. **G5 — 工具注册**：把 `MARKDOWN_SKILLS_SERVER` 内的 `MarkdownCliTool`（已实现 `AlephToolDyn`，含 Host/Docker 执行器）动态注册进活动工具注册表，使 clawhub 安装的 CLI skill 成为 LLM 可调用工具。plan 阶段须先确认工具注册表是否支持运行时增删（动态工具）。
2. **G6 — 热重载**：接通 `SkillWatcher`（notify-based），安装/变更/卸载时重新注册对应工具。接通 `EvolutionAutoLoader`——**尊重现有 config 门控**，不强制开启自动生成，仅让"已建好的 loader 能真正 load"。
3. **B3 — 网络声明诚实化**：markdown skill 在 Host 模式下声明 `network: none` 时，当前只设 `NO_PROXY=*`（未真正隔离）。改为诚实行为：记 `tracing::warn` 明确告知 host 模式无法强制网络隔离，并在工具元数据/文档中说明须用 Docker 模式获得真正隔离。不伪称已隔离。

**风险**：动态工具注册需注册表支持运行时变更；若不支持，plan 须设计在每次 loop 启动时重建工具集的方案。

**验证**：集成测试 — 安装一个 markdown CLI skill 后，断言其出现在工具列表且可被 dispatch 执行；`SkillWatcher` 检测到新文件后工具集刷新。

---

### Phase 4 — hermes 增强移植 + 收尾

#### 4.1 使用计数（Usage Tracking）

- 当前 Aleph **完全缺失** per-skill 调用计数。
- 移植 hermes 的 sidecar 模式：`~/.aleph/skills/.usage.json`，记录 `{skill_name: {use_count, view_count, last_used_at, last_viewed_at}}`。
- 原子写：复用 Aleph 已有的 `atomic_io`（temp + rename）。
- `skill_read` / `skill` 工具调用成功时 **best-effort** bump 计数（失败只 warn，绝不影响主流程）。
- 仅记录可计数的 skill；不为后续排序/演化引入额外架构，保持 R3 核心轻量化。

#### 4.2 skill_read 冲突检测（Collision Refusal）

- 当前 `ReadSkillTool::find_skill_dir` 跨目录同名 skill 取首个匹配（静默 shadow）。
- 移植 hermes 的 collision refusal：收集**所有**候选目录，若 >1 匹配则**拒绝猜测**，返回明确的歧义报告（列出所有冲突路径），让 LLM/用户消歧。

#### 4.3 安装期安全扫描（Install-time Guard）

- 新建 `src/skill/guard.rs`：
  - **威胁正则集**：精选子集（exfiltration / 反向 shell / 销毁性命令 / 凭据泄露 / 隐形 unicode），非照搬 hermes 110 条。
  - **结构检查**：文件数上限、单文件/总大小上限、符号链接逃逸检测（`is_relative_to` 风格）。
  - **裁决**：`safe`/`caution`/`dangerous` × 信任度（`builtin`/`trusted`/`community`）矩阵决定是否放行。
- 接入点：clawhub install 路径、markdown_skill install 路径。`dangerous` 一律阻止。
- 输出审计信息（log）。保持 R3：不引重型依赖，正则用已有 `regex` crate。

#### 4.4 references/ 渐进披露修复（B1）

- `ReadSkillTool::list_skill_files` 改为**递归**列出 `references/`、`scripts/`、`assets/` 子目录文件。
- `validate_file_name` 改为**允许子目录路径**但仍严格防路径遍历：拒绝 `..` 组件、绝对路径、符号链接逃逸；`file_name` 经规范化后须仍位于 skill 目录内（`canonicalize` + `starts_with` 校验）。
- 使自带 skill（`skills/git/`、`skills/self/references/*.md`）的 L3 资源真正可读。

#### 4.5 B2 收尾

- 消除 `ReadSkillTool`/`ListSkillsTool` 的重复 `NAME`/`DESCRIPTION` 常量（inherent const 与 `AlephTool` trait const 文本分歧）。统一为单一来源（trait const 为准，删除 inherent 重复或反之，取注册表实际读取端为准）。

**验证**：guard 拒绝含反向 shell 的恶意 skill；usage 计数随调用累加且原子持久化；同名冲突返回歧义报告；`skill_read("git", "references/x.md")` 成功返回内容。

---

## 5. 测试策略

每 Phase 走 TDD（先写失败测试 → 实现 → 通过 → 重构）。目标覆盖率 80%+（项目规范）。

| Phase | 关键测试 |
|---|---|
| P1 | 集成测试：含 system-scope skill 的已 init `SkillSystem` → 装配出的 system prompt 含 `<available_skills>` 与 skill 名 |
| P2 | `skill_status` 在已 init 系统返回 `total > 0`；`required-config` 缺失 → `MissingConfig` |
| P3 | markdown CLI skill 安装后进入工具列表且可 dispatch；`SkillWatcher` 触发工具集刷新 |
| P4 | guard 拒绝恶意 skill；usage 计数累加并原子持久化；同名冲突报歧义；`references/` 子目录文件可读 |

**基线**：main 已有 8 lib + 4 集成测试失败（与 skill 层无关，见项目记忆 `project_baseline_test_failures.md`）——不阻塞本轮 Phase 验证，但本轮新增/触及的测试必须全绿。

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| P1 双 prompt 路径（`harness_bridge` vs `run_loop`）定位错误 | plan 阶段先用 grep/调用链确认生产路径；`run_loop.rs` 头部 `#![allow(dead_code)]` 需查证其活跃度 |
| P3 工具注册表不支持运行时增删 | plan 阶段确认；不支持则改为每次 loop 启动重建工具集 |
| 删 `SkillPrefetcher` 后性能回退 | snapshot 为注册表内存过滤，理论廉价；plan 阶段确认 snapshot 计算无 I/O 阻塞 |
| 安装期 guard 误杀合法 skill | 正则集保守、`caution` 不阻止仅警告、`dangerous` 才阻止；`builtin` 信任度跳过 |
| markdown_skill 连线引入新依赖编译负担 | `notify`/`walkdir` 已是现有依赖，连线不新增 crate |

---

## 7. 清理清单（避免屎山）

- 删除 `src/skill/prefetch.rs` 整个模块（G3）。
- 删除 `harness/agent/think.rs:52` 死调用及相关 `skill_prefetcher` 字段。
- 消除 `skill_reader.rs` 重复常量（B2）。
- 若确认 `run_loop.rs` 的 skill snapshot 计算为遗留死路径，一并删除。
- 不保留任何"为未来留口"的抽象（YAGNI / R10）。

---

## 8. 架构红线对照

- **R3 核心轻量化**：不引重型依赖；guard 用已有 `regex`；删 `SkillPrefetcher` 死代码。
- **R7 LLM 主权**：skill 匹配仍由 LLM 经 `<when>` 触发自主判断，无关键词引擎、无相关性打分。
- **R8 工具即一切**：`skill_*` 工具、markdown CLI skill 均为 LLM 可调用工具。
- **R10 薄 Harness**：skill 逻辑不进 `src/harness/`；删除 `think.rs` 中唯一的死 skill 调用，反而让 harness 更薄。
