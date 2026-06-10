# Note 层记忆网络编织（Link Weaving）设计

> 2026-06-10 · 状态：已确认（治本 + 存量回填；Prompt 硬契约 + 和谐门；Dream Daemon 新 stage）

## 1. 问题与目标

**现状实测**（main agent，2026-06-10）：17 个活跃 note，`notes_links` 表 0 行，100% 孤岛。
全部 `[[wikilink]]` 仅存在于自动生成的 `index.md` 与一个已归档 note 中。明显应互链的主题簇
（新闻监控 6 件套、t8star 配置对）之间零互链。

**三个根因**：

1. **ingest prompt 软约束** — `PROMPT_COMPOUND_PLAN` rule 6 只说 *"Prefer linking"*，
   且明确允许稀疏期建无链接种子 note；LLM 恒走最省事路径（`src/memory/notes/ingest/prompts.rs:22-29`）。
2. **`note_manage` 盲创建** — `links` 参数 optional，工具不向 LLM 展示任何已有 note
   （`src/builtin_tools/note_manage.rs`）。
3. **无回填机制** — NoteLint 只修破链不补新链；NoteSynthesis 每周跑且要求单分类 ≥3 note
   （实际分类最多 4 个，基本永不触发）。出生孤岛 = 永远孤岛。
   恶性循环：`note_decay` 公式入链权重占 0.3，无入链 note 15 天即归档——孤岛被系统加速淘汰。

**基建现状（全部已就绪，无需新造）**：`notes_links` 有向边表 + 双向查询 API
（`get_outgoing_links` / `get_incoming_links`）、`[[wikilink]]` 解析（`wikilink.rs`）、
`gather_related` 混合检索 + 1 跳展开（`retrieve.rs:55-104`）、
**`PageOp::Link` op 双向 wikilink 写入路径已在 `apply.rs` 实现**。

**目标**：新 note 出生即联网；存量孤岛被周期性编织进网络；全程 LLM 决定"链到谁"（R7），
harness 只负责校验与候选供给。

## 2. 创建端硬契约（ingest 路径）

### 2.1 Prompt 收紧（`prompts.rs`）

rule 6 从 "Prefer linking" 改为硬性契约：

- "Related existing pages" **非空**时，每个 `create` 必须至少带一条 `links[]` 或 `relations`；
- 仅当相关页为空时才允许无链接种子 note；
- 补充提示：可用现成 `link` op 在既有页之间补边。

### 2.2 和谐门 `enforce_link_contract`（`ingestor.rs`）

插入位置：`dedup_redirect_creates` 之后、governance gate 之前（镜像现有写入期干预先例，
`ingestor.rs:367-369`）。逻辑：

1. 扫描 plan：related 非空、且某 create 无 `links` 且无 `relations` → 违约；
2. 违约时发起**一次**轻量修复 LLM 调用：输入违约 note 内容 + related pages 列表，
   要求返回每个 note 的链接（限定 `[P<n>]` token，越界丢弃防幻觉）或显式 `"isolated": true`；
3. 修复结果 merge 回对应 op；
4. 修复调用失败或修复后仍无链接 → **放行不拒绝**（P7 优雅降级——链接是增强，
   不能阻塞记忆落盘）。

hash-conflict 重放路径（`ingestor.rs:394-408`）同样过门。

## 3. `note_manage` 路径补可见性

不加硬门（工具路径保持 LLM 自主），只修"盲"：

- `create` 执行时跑一次 hybrid search，把 top-5 相关 note（路径 + 摘要）放进工具返回结果的
  `related_notes` 字段，附 nudge（"考虑用 links 参数或 update 建立 `[[链接]]`"）；
- 工具 description 同步强化链接要求（R8：把数据递到 LLM 眼前，推理留给它）。

## 4. 存量回填：`NoteWeave` dream stage

新建 `src/memory/dreaming/stages/note_weave.rs`，实现 `DreamStage` trait，
注册在 **NoteLint 之后、NoteDecay 之前**（先编织、再衰减——新链赶上 decay 评分，解开恶性循环）。

流程：

1. **孤岛检测**（纯 SQL，零 LLM 成本）：活跃 note 中出链 = 0 且入链 = 0 者，每轮 cap 10 个；
2. **候选供给**：对每个孤岛复用 hybrid search 检索 top-8 候选（排除自身与 archive）；
   候选为空则跳过（真孤例，不强扭）；
3. **LLM 判断**（R7）：孤岛内容 + 候选列表 → 返回 0–3 条链接；目标必须在候选集内，
   幻觉目标直接丢弃（镜像 `[P<n>]` token 防线）；
4. **写入**：LLM 产出构造为 `PageOp::Link`，**复用 `apply.rs` 现成双向 wikilink 执行路径**
   （写正文 + 重索引 + `notes_links` 落库），不新造写入代码。

错误处理：单个孤岛 LLM 失败只跳过该 note，不中断 stage；整轮结果计入 dream report。

## 5. 测试

- prompt 快照更新（insta）；
- `enforce_link_contract` 单测（mock LLM）：违约触发修复 / 修复失败放行 / token 越界丢弃 /
  related 为空不触发；
- `note_weave` 单测：孤岛 SQL 检测、幻觉候选拒绝、link op 构造、空候选跳过、cap 生效；
- `note_manage` `related_notes` 注入单测。

## 6. 明确不做（YAGNI）

- 链接质量/权重评分；
- 回链 UI 展示；
- 跨 agent 链接；
- NoteSynthesis 阈值调整（孤岛织入后它自然有素材）;
- 近重复 note 合并（`t8star-video-config` / `aleph-t8starvideo-config` 归 NoteConsolidate 管，
  本轮只链接不合并）。
