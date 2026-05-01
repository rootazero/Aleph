---
title: "Memory Evolution Spec A — Curated Hot Memory + Frozen Prompt Snapshot + Direct Write Tool"
date: 2026-05-01
status: approved
owner: "@user"
supersedes: null
related_refs:
  - docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec3-fencing-modes-design.md
  - docs/reference/memory/NOTES.md
  - docs/reference/AGENT_SYSTEM.md
inspiration:
  - "深度拆解 Hermes Agent 的记忆系统 (2026-04 article)"
  - "/Volumes/TBU4/Github/hermes-agent/tools/memory_tool.py"
---

# Memory Evolution Spec A — Curated Hot Memory + Frozen Prompt Snapshot + Direct Write Tool

> **TL;DR** — Aleph 已实现 4-spec roadmap (capture hooks / reflect / fencing / extensions)，但仍缺 Hermes 文章中三个互锁的核心特性：(H1) 系统提示词内 curated 段落的**会话级冻结**、(H2) **char-budget 受限**的小型热区文件 + 占用率 UI、(H3) LLM 在对话中可**直接 add/replace/remove** entry 的写入工具。本 spec 把现有 `~/.aleph/agents/{agent_id}/MEMORY.md`（当前自由 markdown）升级为 § 分隔的 curated 热区、引入 `remember` 工具、把 `<CuratedMemory>`/`<UserProfile>` envelope 在会话起始冻结一次直到压缩或会话结束才刷新；同时把 `self_config` 对 MEMORY.md 的写路径下线，把 `IdentityFiles` 收缩到 5 个文件。USER.md 仍由 ProfileSynthesizer 自动综合，本 spec 只在其上叠加冻结 + budget header；不替换 Aleph 的综合管线。

---

## 1. 背景

### 1.1 已有 Aleph 记忆能力（不重复造）

- `notes/` 全套：抽取、索引、检索、wiki orientation、profile synthesizer
- `dreaming/` 全套：信号、策略、变异闸门、四级校验、事件日志
- `assembler/`：HybridAssembler + 上下文围栏 (`<MemoryContext>`/`<UserProfile>`/`<NoteOrientation>`)
- `MemoryEvent` 事件溯源
- `MemoryExtension` 插件 trait（Spec 4）
- 5 个 memory_* 读工具 + `note_manage` / `note_orient` / `note_schema` / `flag_user_correction` / `user_profile`
- `IdentityFiles`：SOUL / IDENTITY / AGENTS / TOOLS / MEMORY / HEARTBEAT 6 文件按需注入
- `self_config` 工具：以上 6 文件的整文件读写 + config.toml dot-path 修改
- `MemoryInjectionMode`：`Context` / `Tools` / `Hybrid` 三档
- `compression.run.completed` 事件 + `SessionEnd` capture hook（Spec 1）
- `atomic_write_file`（M5）

### 1.2 Hermes 文章核心洞察

1. **冷热分离**：高频热区用极小的 curated 文件常驻提示词；偶尔用到的搜索调用
2. **缓存优先**：系统提示词 prefix **会话级冻结**，磁盘写入立即生效但本会话内不影响 prompt
3. **char-limit 而非 token-limit**：model-independent，header 显示占用率作为元认知信号给模型
4. **substring 定位**：`replace` / `remove` 用短唯一子串而非 ID
5. **威胁扫描**：写入提示词的内容必须扫 prompt-injection / exfiltration / 隐藏 unicode

### 1.3 真实缺口（Spec A 定位）

| # | 缺口 | Hermes 做法 | Aleph 当前 |
|---|------|-------------|-----------|
| **H1** | 系统提示词中 curated 段落每轮重读、写盘即破缓存 | `_system_prompt_snapshot` 会话起始冻结 | `MemoryContextProvider.build_*_user_message()` 每轮 assemble；`<UserProfile>` 每轮重读 USER.md |
| **H2** | MEMORY.md 是自由 markdown，无 char budget、无占用率 UI、无 entry 边界 | 2,200/1,375 char 硬上限 + `[67% — 1,474/2,200 chars]` header | 自由格式，无预算信号 |
| **H3** | LLM 不能在对话中即时写一条 fact 进 MEMORY.md | `memory(action="add", target="memory", content)` | 仅 `flag_user_correction`（异步）或等 Dream Daemon 抽取（异步） |

### 1.4 不在本 Spec 范围内（明确）

- H4（pre-compression memory flush dedicated LLM call）
- H5（session_search 摘要管线 + 浏览模式）→ Spec B
- H6（skill 索引注入 + 按需加载）→ 待核实
- H7（除 curated 层外的跨进程并发安全）→ Spec C
- ProfileSynthesizer 改造 / 替换
- 迁移 CLI

---

## 2. 设计决策汇总

| # | 决策点 | 选择 | 理由 |
|---|--------|------|------|
| D1 | USER.md 处理路线 | 保留 ProfileSynthesizer 综合管线，叠加冻结 + budget UI | Aleph 自动综合是真实差异化优势，不能丢；MEMORY.md 才走 Hermes 直写模式 |
| D2 | 现有 MEMORY.md 兼容路径 | 容忍读 / 强制写：无 § 时整文件作 1 条 legacy entry，prompt 带 `[OVER BUDGET]` warning，`add` 被拒、`replace`/`remove` 可用，由 LLM 自行清理 | 零迁移痛点；driving force 自然，符合 LLM 主权原则 |
| D3 | 新工具形态 + self_config 关系 | 新增 `remember` 单工具 + action enum (`add`/`replace`/`remove`)；self_config 对 MEMORY.md 写返回 `deprecated` 错误，read 仍允许 | 单一职责；防止双写撕裂 § 格式；R3/R8/R9 全部对齐 |
| D4 | Snapshot 刷新触发 | Hermes-纯：仅 ① 会话首轮 ② `compression.run.completed` ③ 显式 admin reload；mid-session 写盘不刷 prompt | 最大缓存稳定性；工具响应返回 live state 让模型仍可感知"已保存" |
| D5 | Char budget | 默认 2,200 (MEMORY) / 1,375 (USER)，单位 char，超 limit 拒绝写盘 | 与 Hermes 数值对齐拿到"压紧"UX；可配置；model-independent |
| D6 | Threat scanner | 复用 `content_scanner.rs`：先 audit 现有规则覆盖度，缺则补齐 Hermes 11 条威胁模式 + 5 类隐藏 unicode | 单一真相源，R3 核心轻量化；如 audit 显示不通用再降级独立模块 |

---

## 3. 架构

### 3.1 新模块布局

```
src/memory/curated/
├── mod.rs           # public API: CuratedMemoryStore, CuratedSnapshot, CuratedConfig
├── store.rs         # 内存态 + 磁盘持久化 + budget 校验 + 锁
├── format.rs        # § 解析/序列化、ENTRY_DELIMITER = "\n§\n"、atomic temp+rename、跨进程文件锁（unix: fcntl 经 `fs2` crate 抽象；windows: 同 crate 自动走 LockFileEx，确保跨平台一致）
├── budget.rs        # 字符预算计算 + prompt header 渲染（标准格式：`[N% — used/limit chars]`，over budget 时前置 `OVER BUDGET — `）
├── legacy.rs        # 兼容读：非-§ 文件 → 1 条 legacy entry + over_budget 标记
├── snapshot.rs      # CuratedSnapshot：会话起始冻结的渲染产物
└── tests.rs         # 单元 + proptest + loom
```

### 3.2 关键类型

```rust
pub struct CuratedConfig {
    pub memory_char_limit: usize,            // 默认 2200
    pub user_char_limit: usize,              // 默认 1375
    pub legacy_warn_threshold: f32,          // 默认 0.95
}

pub struct CuratedMemoryStore {
    agent_id: String,
    file_path: PathBuf,                      // ~/.aleph/agents/{agent_id}/MEMORY.md
    char_limit: usize,
    state: Mutex<StoreState>,                // entries: Vec<String>, legacy: bool
}

impl CuratedMemoryStore {
    pub fn load_with_legacy(path: PathBuf, char_limit: usize) -> Result<Self>;
    pub fn add(&self, content: &str) -> Result<WriteOutcome>;
    pub fn replace(&self, old_substr: &str, new: &str) -> Result<WriteOutcome>;
    pub fn remove(&self, old_substr: &str) -> Result<WriteOutcome>;
    pub fn capture_snapshot(&self) -> CuratedSnapshot;
    pub fn current_entries(&self) -> Vec<String>;            // 工具响应用
}

#[derive(Clone)]
pub struct CuratedSnapshot {
    pub agent_md_block: String,              // <CuratedMemory> XML：header + § entries
    pub user_md_block: Option<String>,       // <UserProfile> XML：header + synthesizer 输出
    pub captured_at: SystemTime,
    pub agent_id: String,
}

pub struct WriteOutcome {
    pub entries: Vec<String>,
    pub usage_pct: u8,
    pub usage_chars: usize,
    pub limit: usize,
    pub message: String,
}
```

### 3.3 现有模块改动表

| 现有模块 | 改动 |
|---------|------|
| `thinker/identity_files.rs` | `IDENTITY_FILE_NAMES` 移除 MEMORY.md（5 文件）；删 `IdentityFiles::memory_md` 字段 + 所有访问点 |
| `thinker/memory_context_provider.rs` | 新增字段 `curated_snapshots: Arc<RwLock<HashMap<SessionKey, CuratedSnapshot>>>` 与 `curated_stores: Arc<DashMap<AgentId, Arc<CuratedMemoryStore>>>`；新方法 `build_curated_message(agent_id, session_key) -> Option<UnifiedMessage>` |
| `thinker/prompt_builder/sections/` | 新 `CuratedMemoryLayer`，标 `LayerStability::Stable` |
| `builtin_tools/self_config.rs` | MEMORY.md 写分支 → `ToolError::deprecated`，read 保留；删相关单元测试 |
| `builtin_tools/remember.rs` (新) | `remember` 工具实现 + JSON schema + 调用 `content_scanner` |
| `builtin_tools/mod.rs` | 注册 `remember` |
| `memory/content_scanner.rs` | 审计后补齐 Hermes 模式（仅在缺失时新增；test-first） |
| `config/types/memory.rs` | 新增 `CuratedConfig` 结构体 |
| `config/agent_resolver.rs` | `DEFAULT_MEMORY` 简化为单条 § entry 占位；行 459-467 引导文案改写为 `remember` 工具引导 |
| `gateway/identity_loader.rs:92` | 删 MEMORY.md 加载分支（curated 层接管） |

### 3.4 模块边界（防屎山三准则）

- `curated/` 不依赖 `notes/` / `dreaming/` / `assembler/` —— 与现有大记忆系统解耦
- `CuratedMemoryStore` 仅通过 `content_scanner` 与 `atomic_write_file` 与外部交互
- `MemoryContextProvider` 在 curated 层不复用 `HybridAssembler`（curated 是冻结渲染，不是检索）

---

## 4. 数据流 & 生命周期

### 4.1 会话首轮加载

```
PromptBuilder.build()
  → MemoryContextProvider.build_curated_message(agent_id, session_key)
      → curated_snapshots.read().get(session_key)?
          ├─ HIT  → 返回 cached XML envelope（命中 prefix cache）
          └─ MISS → curated_stores.entry(agent_id).or_insert_with(load_with_legacy)
                  → store.capture_snapshot()
                      ├─ 渲染 <CuratedMemory>: budget header + § 分隔 entries
                      └─ 渲染 <UserProfile>: ProfileSynthesizer.current() + budget header
                  → curated_snapshots.write().insert(session_key, snapshot)
                  → 返回 XML
```

### 4.2 写入流程（`remember` 工具）

```
remember tool handler
  → content_scanner.scan(content)?                           # 威胁检测
  → store.<action>(...)
      ├─ acquire fcntl lock on MEMORY.md.lock                # 跨进程并发安全
      ├─ re-read disk into entries                           # pickup 其他进程写入
      ├─ apply action (add / substring replace / substring remove)
      ├─ check budget (refuse if > limit)
      ├─ atomic_write_file (temp → rename)                   # 复用 M5
      └─ release lock
  → 返回 WriteOutcome { entries, usage_pct, usage_chars, limit, message }

# 关键：snapshot 缓存不动，本会话 system prompt 保持冻结（H1 不变量）
```

### 4.3 Snapshot 失效触发

| 事件 | 处理 |
|------|------|
| `compression.run.completed` | `curated_snapshots.write().remove(session_key)`；下轮 build miss → 重新 capture |
| `SessionEnd` capture hook（Spec 1 已有） | 同上 evict + 触发 ProfileSynthesizer 更新（已有逻辑） |
| 进程重启 | cache 自然为空 |
| 显式 admin reload（未来扩展） | `curated_snapshots.clear()`；本 Spec A **不实现** |

### 4.4 失败模式

| 场景 | 行为 | 模型可见 |
|------|------|---------|
| 磁盘写失败 | WriteOutcome 错误，snapshot 不变 | 错误响应 |
| fcntl 锁超时 | 错误，建议重试 | 错误响应 |
| 威胁检测命中 | 拒绝写盘，错误带 threat code | 错误响应：内容不合规 |
| 超 budget | 拒绝写盘，返回 `current_entries` + `usage` | 错误响应：模型须先 replace/remove |
| 跨进程并发改盘 | fcntl 锁串行化；本进程下次 capture 重读 | 透明 |
| Legacy 整文件超 budget | 读时打 `[OVER BUDGET]` header，`add` 拒绝、`replace`/`remove` 可用 | header 警告 + add 拒绝消息 |

### 4.5 不变量

1. 本会话内系统提示词中的 `<CuratedMemory>` 与 `<UserProfile>` 字符内容稳定
2. 任何成功的 `remember` 工具调用磁盘必更新（写盘原子）
3. 任何失败的 `remember` 工具调用磁盘必未变更：威胁扫描在工具入口（store 调用前）拦截；锁获取 + 预算检查在 store 边界（atomic_write_file 调用前）拦截；三层闸门任一不通过都不会触达写盘
4. session_key 与 snapshot 一一对应（evict 后下次 build 必 miss）

---

## 5. Tool 表面 & Deprecation

### 5.1 新工具：`remember`

```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RememberArgs {
    Add { content: String },
    Replace { old_text: String, content: String },
    Remove { old_text: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct RememberOutput {
    pub entries: Vec<String>,
    pub entry_count: usize,
    pub usage: String,            // "67% — 1,474/2,200 chars"
    pub usage_pct: u8,
    pub message: String,
}
```

工具描述（system prompt 看到的）：

> Save durable agent-side memory that persists across sessions and is auto-injected into your future system prompt. Memory is small and curated — keep entries compact, factual, and useful next session.
>
> WHEN TO USE (proactively, don't wait):
> - User corrects you ("don't do X again", "remember this")
> - You discover a stable environment fact (project layout, tooling quirk, OS detail)
> - You learn a workflow / convention specific to this user
>
> DO NOT save: task progress, session outcomes, completed-work logs, transient TODOs. For those, use scratchpad or session_search.
>
> ACTIONS:
> - add: append a new fact (rejects duplicates / over-budget; suggests replace)
> - replace: substitute via a short unique substring of an existing entry
> - remove: delete via a short unique substring
>
> Memory is bounded ({{char_limit}} chars). When full, replace or remove first. The current session's system prompt won't show your write until next compression or session start, but the tool response always reflects live state.

### 5.2 工具表更新

| 用途 | 工具 |
|------|------|
| **即时主动记忆** | **`remember`** ← 新 |
| 读取笔记库 | `memory_search` / `memory_browse` / `memory_explore` |
| LLM 综合检索 | `memory_reflect` |
| 笔记生命周期回放 | `memory_timeline` |
| 历史会话 FTS | `session_search` |
| 用户画像查 | `user_profile` |
| 笔记库管理 | `note_manage` / `note_orient` / `note_schema` |
| 反馈循环 | `flag_user_correction` |
| 配置 + 其他身份文件 | `self_config`（**收缩**：MEMORY.md 写已拒绝） |

### 5.3 self_config 收缩

```rust
// builtin_tools/self_config.rs，写路径分支
if file_name.eq_ignore_ascii_case("MEMORY.md") {
    return Err(ToolError::deprecated(
        "self_config no longer writes MEMORY.md. \
         Use the `remember` tool with action=add/replace/remove for entry-level edits. \
         Read access remains available via self_config(action='read', file='MEMORY.md').",
    ));
}
```

### 5.4 IdentityFiles 收缩

```rust
// 改前
const IDENTITY_FILE_NAMES: &[&str] = &[
    "SOUL.md", "IDENTITY.md", "AGENTS.md", "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
];

// 改后
const IDENTITY_FILE_NAMES: &[&str] = &[
    "SOUL.md", "IDENTITY.md", "AGENTS.md", "TOOLS.md", "HEARTBEAT.md",
];
```

`IdentityFiles::memory_md` 字段删，所有访问点改读 `MemoryContextProvider.build_curated_message`。

### 5.5 agent_resolver 引导文案改写

```text
- Curated memory (MEMORY.md): Bounded ({{char_limit}} chars).
  Use the `remember` tool to add/replace/remove entries.
  Frozen into the system prompt at session start;
  refreshes on compression or new session.
- When the user says "remember this" → call remember(action="add", ...)
- When you learn a lesson → call remember(action="add", ...);
  if budget is full, replace an obsolete entry instead.
```

`DEFAULT_MEMORY` 简化为单条占位 entry：

```text
Replace this placeholder with your first memory entry.
```

---

## 6. 配置

新增 `[memory.curated]` 段：

```toml
[memory.curated]
memory_char_limit = 2200      # MEMORY.md 字符上限
user_char_limit   = 1375      # USER.md 渲染上限（synthesizer 产出超长会被截断渲染）
legacy_warn_threshold = 0.95  # 占用率 ≥95% 加 [NEAR LIMIT] 预警
```

加载点：`config/types/memory.rs::CuratedConfig`；`MemoryContextProvider::with_provider` 接受参数；默认值常量在 `curated/mod.rs`。

---

## 7. 测试矩阵

| 层级 | 文件 | 覆盖 |
|------|------|------|
| 单元 store | curated/tests.rs | add 拒绝重复 / 拒绝超 budget / replace 子串唯一性 / replace 多匹配但内容相同→第一条 / remove 不存在子串 / 空 content / 仅空白 |
| 单元 format | 同 | § 分隔解析 / 多行 entry / entry 含 `§` 字符（用 `\n§\n` 完整定界） / atomic write 中断不留半截 |
| 单元 legacy | 同 | 非 § 文件→1 条 legacy entry / 空文件→0 条 / 仅 BOM 或 whitespace→0 条 |
| 单元 budget | 同 | header 渲染 0% / 50% / 100% / 110% / 不同 char_limit |
| proptest | 同 | 不变量：任何成功 add/replace 后总字符 ≤ limit；任何 remove 后 entries.len 严格递减 1 |
| loom 并发 | 同 | 两 thread 并发 add → fcntl 锁串行化 → entries 含两次内容、不丢 |
| 集成 | tests/curated_e2e.rs | 会话首轮 capture → mid-session add → snapshot 不变 / compression event → snapshot 失效 → 下轮 capture 含新 entry |
| 集成 legacy | tests/curated_legacy_e2e.rs | 老自由格式 MEMORY.md → 整文件 1 entry / over_budget header / add 被拒、replace 可用 |
| content_scanner 扩展 | content_scanner tests | 11 条 prompt-injection 模式 + 5 类隐藏 unicode 全部命中 / 普通中英文不被拦 |
| remember tool | builtin_tools/remember.rs tests | RememberArgs serde 三种 action / RememberOutput 字段完整 / self_config 写 MEMORY.md→`deprecated` 含 `Use the \`remember\` tool` 字样 |

---

## 8. 清理 & 迁移

### 8.1 死代码必删清单

| 路径 | 操作 |
|------|------|
| `self_config.rs` 写 MEMORY.md 的成功分支 | 删 |
| `self_config.rs` `test_*memory*write*` | 删 |
| `identity_files.rs::IDENTITY_FILE_NAMES` 含 MEMORY.md 行 | 删 |
| `identity_files.rs::IdentityFiles::memory_md` 字段 + 所有访问点 | 删 |
| `agent_resolver.rs::DEFAULT_MEMORY` 自由格式注释段 | 简化为单条 § entry 占位 |
| `agent_resolver.rs:459-467` "Manual memory" 引导段 | 改写为 `remember` 工具引导 |
| `gateway/identity_loader.rs:92` 加载 MEMORY.md 代码 | 删 |
| `prompt_builder/` 中所有读 `identity_files.memory_md` 的 layer 代码 | 改读 `MemoryContextProvider.build_curated_message` |

完工时 `grep -rn "memory_md\|MEMORY.md" src/` 应只剩 curated 模块、`remember` 工具描述、文档引用。

### 8.2 用户视角迁移

1. 现有 `~/.aleph/agents/{agent_id}/MEMORY.md` 文件保持原位置，无脚本运行
2. 首次启动：legacy 模式注入 + `[OVER BUDGET — N% — used/limit chars]` warning header；`remember(add)` 拒绝、`replace`/`remove` 可用
3. LLM 自然消化：对话中看到 over_budget warning → 主动 replace 把 legacy 大段改成多条 § entry
4. 可选 CLI `/memory:migrate`（启发式切段）—— **本 spec 不实现，YAGNI**
5. 文档更新：`docs/reference/memory/` + `docs/reference/AGENT_SYSTEM.md` 中 MEMORY.md 描述改写

---

## 9. 验收标准

Spec A 完工的二元判定：

1. 新建一个 agent，对话中说"记住我喜欢简洁回复" → 模型调 `remember(add, "User prefers concise replies")` → 写盘成功 → 工具响应含 entries + 占用率
2. 同会话内再问"你记住了什么"，LLM 仍能从工具响应或调 `memory_browse` 答出（**注意**：本会话系统提示词不立即包含此条，符合 H1 设计）
3. 重启 server 或触发压缩后，下一轮系统提示词包含 `<CuratedMemory>` envelope，含 "User prefers concise replies" + `[N% — used/2,200 chars]` header（占用率随条目增长）
4. 对同一 agent 启动两个 aleph 进程并发写入 MEMORY.md 不丢内容、不撕格式（fcntl 锁验证）
5. 老用户 MEMORY.md（自由 markdown）首次启动 → prompt 带 `[OVER BUDGET]` header，`remember(add)` 被拒、`remember(replace)` 可用
6. `self_config(action='write', file='MEMORY.md')` 被拒，错误信息含 "Use the `remember` tool" 引导
7. 注入 prompt-injection payload (`"ignore previous instructions..."`) → `remember(add)` 被 content_scanner 拒，错误带 threat code
8. 注入隐藏 unicode (`"​"`) → `remember(add)` 被拒
9. 全测试套件 `cargo test --lib` 通过；新加 loom 并发测试通过；`cargo clippy -- -D warnings` 干净
10. `grep -rn "memory_md\|MEMORY.md" src/` 仅剩 curated 模块、`remember` 工具、文档；不再出现于 `identity_files.rs`、`self_config.rs` 写路径、`gateway/identity_loader.rs`

---

## 10. 实施阶段（粗略，详细计划由 writing-plans 产出）

| Phase | 内容 | 验收 |
|-------|------|------|
| **P1** | `curated/` 模块全套（store / format / budget / legacy / snapshot / tests）— 不接入主流程 | 单元 + proptest + loom 全绿 |
| **P2** | `content_scanner.rs` audit + 补齐 Hermes 模式 | 模式覆盖测试全绿，无误报基线 |
| **P3** | `remember` 工具 + `self_config` MEMORY.md 写 deprecation | 工具 e2e + self_config 拒写测试 |
| **P4** | 冻结快照接入：`MemoryContextProvider` + `CuratedMemoryLayer` + 压缩/SessionEnd evict 钩子 | 集成 e2e（会话首轮 → mid-write → 压缩刷新）全绿 |
| **P5** | 清理：IdentityFiles 收缩、agent_resolver 文案、identity_loader 删 | 全 grep 干净；现有测试全绿 |
| **P6** | 文档同步：docs/reference/memory/ + AGENT_SYSTEM.md | 手测：新 agent 首启动行为符合验收 1-3 |

---

## 11. 隐含依赖（已验证存在）

- `compression.run.completed` 事件（`compression/service.rs`）
- `SessionEnd` capture hook（Spec 1）
- `ProfileSynthesizer.current()` API（Spec 7）
- `atomic_write_file`（M5）
- `content_scanner.rs` + `noise_filter.rs` 现有威胁扫描基础设施

---

## 12. 与既有 4-Spec roadmap 的关系

本 Spec A 是 **roadmap 之外的新增 spec**，roadmap 中 Spec 1-4 已全部 shipped。Spec A 触及 Spec 3（Fencing/Modes）的注入路径——MemoryInjectionMode 三档行为对 curated 同样生效（Tools 模式下 curated 也不自动注入，只通过 `remember` 工具读时返回 live entries）。Spec 4（MemoryExtension trait）不在 curated 写路径触发（curated 是单文件本地状态，不需要外部 backend dispatch）。

---

## 13. 后续

- 同步在 roadmap 文件 `2026-04-13-memory-evolution-roadmap.md` 末尾追加 Spec A 状态条目
- Spec A 落地后再启动 Spec B（session_search 摘要管线）、Spec C（跨进程并发安全）
