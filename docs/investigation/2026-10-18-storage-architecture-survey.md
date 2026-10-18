# Storage Architecture Survey — Current State

> Investigation Date: 2026-10-18  
> Author: Phase 1 调研  
> Purpose: 摸清当前 Aleph 存储架构，为 Markdown-first 改造提供基线

---

## Executive Summary

**关键发现**：
1. ✅ **Notes 层已经是 Markdown-first**：508 个 `.md` 文件（1.5MB）在 `~/.aleph/memory/note/`
2. ✅ **SQLite 数据库为空**：`aleph.db` 是 0B，无历史数据迁移负担
3. ⚠️ **Raw memories 尚未实际使用**：`raw_memories` 表结构已定义但无数据
4. ✅ **最佳改造时机**：可以在没有迁移压力的情况下直接实现目标架构

---

## 1. 当前文件结构

### 1.1 Markdown 文件分布

```bash
~/.aleph/memory/note/
├── agent-73f36847/      (22 dirs)
├── analyst_aapl/        (16 dirs)
├── codev/               (11 dirs)
├── coding/              (5 files)
├── contrarian/          (6 files)
├── default/             (20 dirs)
├── fundamental_analyst/ (11 dirs)
├── growth_analyst/      (25 dirs)
├── main/                (67 dirs) ← 主 agent，508 个 .md 文件总计
├── main__u-owner/       (3 files)
├── market_researcher/   (8 dirs)
├── risk_analyst/        (19 dirs)
└── test-memory-validation/ (11 dirs)

总计：508 个 Markdown 文件，1.5MB
```

**示例文件结构**（`main/personal/news-monitoring.md`）：
```markdown
---
type: personal
title: news-monitoring
aliases: []
category: personal
tags: [personal, automation, news, geopolitics]
created: "2026-06-07"
updated: "2026-06-30"
confidence: 1.0000
severity: low
source_notes: []
status: active
supersedes: []
superseded_by: []
---

- [title] Automated News Monitoring Setup
- [summary] User runs a cron task for daily US-Iran conflict news summaries...
- The user has configured a cron task to generate daily Chinese-language news...
```

**关键特性**：
- ✅ YAML frontmatter 元数据
- ✅ Obsidian-compatible `[[wikilinks]]`
- ✅ 结构化分层：`{agent}/{category}/*.md`
- ✅ 版本控制友好（纯文本）

### 1.2 SQLite 数据库

**旧位置（错误）**：`~/Library/Application Support/aleph/aleph.db` — 0 字节空文件，早期遗留错误路径  
**正确位置**：`~/.aleph/metadata.db` — 设计目标路径，全平台统一

**现有数据库**：
- `~/.aleph/hub_catalog.db` (303KB) — Plugin/hub catalog（独立于 memory storage）

### 1.3 其他文件

```bash
~/.aleph/
├── memory/
│   └── note/{agent}/{category}/*.md  ← 508 个知识笔记
├── plugins/cache/cli-anything/assets/ ← 插件资产（PNG 图片）
├── output/                            ← PDF 报告输出
└── workspaces/
    ├── main/output/documents/*.pdf
    └── cowork/empty.pdf
```

**附件存储观察**：
- 目前附件散落在 `plugins/`, `output/`, `workspaces/` 下
- 无统一的附件目录
- PDF 输出在 `output/` 和 `workspaces/{name}/output/`

---

## 2. 数据库表结构（已定义但未使用）

### 2.1 核心表（from `src/memory/store/sqlite/schema/ddl.rs`）

#### `raw_memories` — 原始会话记录

```sql
CREATE TABLE IF NOT EXISTS raw_memories (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    source          TEXT NOT NULL,          -- 'transcript', 'tool_output', 'attachment', etc.
    agent_id        TEXT NOT NULL DEFAULT 'default',
    session_id      TEXT,
    path            TEXT,                   -- 当前：路径字段（用途未明确）
    attachment_text TEXT,                   -- 当前：附件文本存在表里
    is_processed    INTEGER DEFAULT 0,      -- 标记是否已被压缩服务处理
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_raw_unprocessed ON raw_memories(is_processed, created_at)
    WHERE is_processed = 0;
CREATE INDEX idx_raw_agent ON raw_memories(agent_id);
CREATE INDEX idx_raw_session ON raw_memories(session_id);
```

**问题**：
- ❌ `content` 存在 TEXT 列（会话内容直接在 SQLite）
- ❌ `attachment_text` 存在 TEXT 列（附件文本也在 SQLite）
- ⚠️ `path` 字段存在但用途不明确（可能是预留字段）

#### `notes_index` — Markdown 文件索引

```sql
CREATE TABLE IF NOT EXISTS notes_index (
    path            TEXT NOT NULL,                    -- 相对路径
    filename        TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    category        TEXT NOT NULL,
    tags_json       TEXT NOT NULL DEFAULT '[]',
    aliases_json    TEXT NOT NULL DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash    TEXT NOT NULL,                    -- ✅ 用于检测文件变更
    PRIMARY KEY (agent_id, path)
);
```

**正确设计**：
- ✅ 只存元数据和路径
- ✅ `content_hash` 可检测文件变更
- ✅ 多维索引（agent, category, filename）

#### `notes_links` — Wikilink 关系图

```sql
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,                        -- 原始 [[link]] 文本
    relation    TEXT,
    confidence  REAL NOT NULL DEFAULT 1.0,
    resolved_by TEXT,
    status      TEXT NOT NULL DEFAULT 'active',       -- 'active' | 'dangling'
    label       TEXT,
    UNIQUE(agent_id, from_note, to_note)
);
```

**正确设计**：
- ✅ 图关系在 SQLite（适合查询）
- ✅ `status` 跟踪 dangling links
- ✅ 部分索引优化

#### 其他表

- `notes_fts` — FTS5 全文搜索索引
- `notes_vec_{dim}` — 向量索引（按维度分表）
- `notes_sources` — 笔记来源引用
- `notes_graph_cache` — 图统计缓存
- `recall_signals` — 回忆信号（热度跟踪）
- `dream_reports` — Dream 守护进程报告
- `dream_status` — Dream 状态
- `daily_insights` — 每日洞察
- `compression_metadata` — 压缩元数据

### 2.2 实际使用状态

**已验证**：
```bash
$ sqlite3 ~/Library/Application\ Support/aleph/aleph.db '.tables'
(no output)  # 数据库完全为空

$ ls -lh ~/Library/Application\ Support/aleph/aleph.db
-rw-r--r--  1 zouguojun  staff  0B Oct 18 18:23 aleph.db
```

**结论**：表结构在代码中定义，但数据库文件从未被初始化或使用。

---

## 3. 代码架构分析

### 3.1 数据库初始化路径

**入口**：`src/memory/store/sqlite/mod.rs::SqliteMemoryBackend::new()`

```rust
pub fn new(db_path: &Path) -> Result<Self, AlephError> {
    // 如果传入目录，在目录内创建 memory.db
    let resolved: PathBuf = if db_path.is_dir() {
        db_path.join("memory.db")  // ← 实际文件名
    } else {
        db_path.to_path_buf()
    };

    // 创建父目录
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 注册 sqlite-vec 扩展
    vec::register_sqlite_vec();

    // 打开数据库
    let conn = crate::utils::sqlite_open::open_sqlite_safe(&resolved)?;

    // 初始化表结构
    schema::init_schema(&conn)?;      // ← 创建所有表
    schema::init_vec_tables(&conn)?;  // ← 创建向量表

    Ok(Self {
        conn: Arc::new(Mutex::new(conn)),
        tuning: RetrievalTuning::default(),
    })
}
```

**观察**：
- ✅ 支持传入目录路径（自动创建 `memory.db`）
- ✅ 自动创建父目录
- ✅ 表结构初始化逻辑完整
- ⚠️ 但实际运行中似乎未被调用（数据库为空）

### 3.2 Raw Memory 存储接口

**Trait**: `src/memory/store/raw_memory.rs::RawMemoryStore`

```rust
#[async_trait]
pub trait RawMemoryStore: Send + Sync {
    async fn insert_raw_memory(&self, memory: RawMemory) -> Result<(), AlephError>;
    async fn get_raw_memory(&self, id: &str) -> Result<Option<RawMemory>, AlephError>;
    async fn list_unprocessed(&self, limit: usize) -> Result<Vec<RawMemory>, AlephError>;
    async fn mark_processed(&self, id: &str) -> Result<(), AlephError>;
    // ... 其他方法
}
```

**实现**：`src/memory/store/sqlite/raw_memories.rs`

当前实现**直接读写 SQLite TEXT 列**：
```rust
pub async fn insert_raw_memory(&self, memory: RawMemory) -> Result<(), AlephError> {
    let conn = lock_conn!(self);
    conn.execute(
        "INSERT INTO raw_memories (id, content, source, ...) VALUES (?, ?, ?, ...)",
        params![id, memory.content, source_token, ...],  // ← content 直接存储
    )?;
    Ok(())
}
```

### 3.3 Notes 存储（已正确）

**Trait**: `src/memory/notes/store.rs::NoteStore`

```rust
#[async_trait]
pub trait NoteStore: Send + Sync {
    async fn upsert_note(&self, note: &NoteMetadata) -> Result<(), AlephError>;
    async fn get_note(&self, agent_id: &str, path: &str) -> Result<Option<NoteMetadata>, AlephError>;
    // ... 索引、链接、FTS、向量查询
}
```

**实现**：`src/memory/store/sqlite/notes/store_impl.rs`

✅ **只存元数据**，文件内容在 Markdown：
```rust
pub async fn upsert_note(&self, note: &NoteMetadata) -> Result<(), AlephError> {
    let conn = lock_conn!(self);
    conn.execute(
        "INSERT OR REPLACE INTO notes_index 
         (path, filename, agent_id, category, tags_json, ..., content_hash)
         VALUES (?, ?, ?, ?, ?, ..., ?)",
        params![
            note.path,           // ← 相对路径
            note.filename,
            note.agent_id,
            note.category,
            tags_json,
            ...,
            note.content_hash,   // ← 用于检测变更
        ],
    )?;
    Ok(())
}
```

---

## 4. 数据流分析

### 4.1 当前预期流程（未实际运行）

```
用户对话
  ↓
Gateway writes to raw_memories (SQLite)
  ↓
CompressionService reads unprocessed rows
  ↓
LLM 提取知识点
  ↓
Write to ~/.aleph/memory/note/{agent}/{category}/*.md
  ↓
NoteIndexer updates notes_index, notes_fts, notes_vec_*
```

### 4.2 实际状态

- **Notes 层**：已有 508 个 Markdown 文件，说明压缩流程曾运行过
- **Raw 层**：数据库为空，说明：
  - 可能会话数据直接被压缩（没有留在 raw_memories）
  - 或者系统还未实际处理过真实会话
  - 或者有其他临时存储机制

### 4.3 Dream 守护进程输出

```bash
~/.aleph/memory/note/main/dream_events.jsonl
```

**内容**：JSONL 格式的 Dream 报告，每行一个 JSON 对象：
```json
{
  "id": "dream_1777313742_1",
  "cycle": 1,
  "strategy": "consolidate",
  "report": {
    "pipeline_type": "consolidate",
    "notes_consolidated": 0,
    "contradictions_found": 0,
    ...
  },
  ...
}
```

**观察**：
- ✅ 结构化数据用 JSONL 存储（而非 SQLite）
- ✅ 在 Markdown 目录下，可以 git 管理
- ✅ 人类可读、可编辑

---

## 5. 附件处理现状

### 5.1 当前散落位置

```bash
~/.aleph/
├── plugins/cache/cli-anything/assets/  ← 插件图片
│   ├── icon.png
│   ├── teaser.png
│   └── architecture.png
├── output/                             ← PDF 报告
│   ├── 中国A股最近一个月技术面分析报告_增强版_2026-03-05.pdf
│   └── A股最近一个月技术面分析报告_2026-03-05.pdf
└── workspaces/
    └── main/output/documents/          ← Workspace 文档
        ├── workspace_test.pdf
        ├── test_chinese.pdf
        └── ...
```

### 5.2 代码中的附件处理

**`raw_memories` 表有 `attachment_text` 列**：
```sql
attachment_text TEXT,  -- 当前：附件文本直接存 SQLite
```

**问题**：
- ❌ 二进制附件无法存 TEXT 列
- ❌ 大附件会让数据库膨胀
- ❌ 无法被外部工具直接访问

---

## 6. 与 Pi Memory 对比

### 6.1 Pi Memory 架构（参考实现）

```
~/.pi/agent/memory/
├── MEMORY.md              ← 长期记忆（单文件）
├── SCRATCHPAD.md          ← 待办清单
├── daily/
│   ├── 2026-10-17.md      ← 每日日志
│   └── 2026-10-18.md
└── recovery/
    └── {uuid}.json        ← 删除恢复记录
```

**特点**：
- ✅ 纯 Markdown（核心工具无需 qmd）
- ✅ 可选 `qmd` 索引（SQLite 在 qmd 自己的目录）
- ✅ 单文件 + 按日分割
- ✅ Git 友好

### 6.2 Aleph 当前架构

```
~/.aleph/memory/note/
├── {agent}/
│   ├── {category}/
│   │   └── {topic}.md     ← 多文件知识图谱
│   ├── SCHEMA.md
│   ├── index.md
│   ├── log.md
│   └── dream_events.jsonl
└── ...
```

**特点**：
- ✅ 多文件知识图谱（更结构化）
- ✅ Agent 隔离
- ✅ Category 分类
- ✅ Obsidian-compatible
- ⚠️ 但缺少会话原始记录的 Markdown 存储

---

## 7. 差距分析

### 7.1 已实现 ✅

1. **Notes 层 Markdown-first**
   - 508 个 `.md` 文件
   - SQLite 只存索引、链接、FTS、向量
   - `content_hash` 检测文件变更

2. **Obsidian 兼容**
   - `[[wikilinks]]` 支持
   - YAML frontmatter
   - 反向链接跟踪

3. **Git 友好**
   - 纯文本 Markdown
   - 可读 diff
   - `dream_events.jsonl` 也是文本

### 7.2 待实现 🔧

1. **Raw memories 层**
   - ❌ 当前：`content` 在 SQLite TEXT 列
   - ✅ 目标：会话记录在 `raw/sessions/{session_id}/transcript.md`

2. **附件存储**
   - ❌ 当前：散落在 `plugins/`, `output/`, `workspaces/`
   - ✅ 目标：统一到 `raw/attachments/{type}/{hash}.{ext}`

3. **相对路径引用**
   - ❌ 当前：`attachment_text` TEXT 列
   - ✅ 目标：Markdown 中 `![](../../attachments/images/abc123.png)`

4. **ID 管理**
   - ⚠️ 当前：`agent_id`, `session_id` 在各表中
   - ✅ 目标：独立的 `metadata.db` 存 ID 映射

---

## 8. 数据量评估

### 8.1 当前数据

| 类型 | 数量 | 大小 | 位置 |
|------|------|------|------|
| Markdown 文件 | 508 | 1.5MB | `~/.aleph/memory/note/` |
| SQLite 数据库 | 1 | 0B (空) | `~/Library/Application Support/aleph/aleph.db` |
| Hub 数据库 | 1 | 303KB | `~/.aleph/hub_catalog.db` |
| PDF 附件 | ~15 | 未统计 | `~/.aleph/output/`, `workspaces/` |

### 8.2 预期增长（假设每日使用）

| 场景 | 每日增量 | 年增长 | 备注 |
|------|----------|--------|------|
| 对话记录 | 10 sessions × 50KB | ~180MB | 原始 transcript |
| 工具输出 | 20 calls × 10KB | ~70MB | 命令输出、API 响应 |
| 附件 | 5 images × 500KB | ~900MB | 截图、文档 |
| 知识笔记 | 5 notes × 5KB | ~9MB | 提取的知识 |
| **总计** | | **~1.2GB/年** | |

**Git 管理考虑**：
- Markdown 增量小（文本 diff 高效）
- 附件需要 git-lfs 或定期归档
- 建议按月归档旧会话

---

## 9. 风险与约束

### 9.1 技术风险

1. **并发写入**
   - Markdown 文件没有事务保证
   - 需要文件锁或原子写（写临时文件再 rename）

2. **大文件数量**
   - 10万+ 会话 → 文件系统性能问题
   - 需要分层目录 `raw/sessions/2026/10/18/`

3. **索引重建时间**
   - 508 个文件已有，重建索引需要多久？
   - 需要增量索引策略

### 9.2 业务约束

1. **向后兼容**
   - 当前数据库为空，无迁移压力
   - 但代码中已有 `RawMemoryStore` 接口调用

2. **查询性能**
   - FTS5 索引依赖 SQLite
   - Markdown 文件本身不支持快速搜索
   - 必须保留 SQLite 索引层

3. **多 Agent 隔离**
   - 当前已按 agent 分目录
   - 需要保持这个隔离

---

## 10. 结论与建议

### 10.1 关键结论

1. **✅ 最佳改造时机**：数据库为空，无历史数据迁移负担
2. **✅ Notes 层已正确**：508 个 Markdown 文件证明架构可行
3. **🔧 Raw 层需改造**：从 SQLite TEXT 列改为 Markdown 文件
4. **🔧 附件需统一**：从分散目录改为 `raw/attachments/`

### 10.2 推荐方案

**保留**：
- `notes_index`, `notes_links`, `notes_fts`, `notes_vec_*` — 索引层
- `recall_signals`, `dream_reports` — 统计和元数据
- Markdown 文件结构 `{agent}/{category}/*.md`

**改造**：
- `raw_memories.content` → `raw/sessions/{session_id}/transcript.md`
- `raw_memories.attachment_text` → `raw/attachments/{type}/{hash}.{ext}`
- 附件引用 → Markdown 中相对路径

**新增**：
- `sessions` 表：`(session_id, agent_id, transcript_path, started_at, ended_at)`
- `attachments` 表：`(attachment_id, file_path, content_hash, mime_type)`

### 10.3 下一步行动

**立即可行**：
1. ✅ 编写设计文档（Phase 2）
2. ✅ 设计新的目录结构
3. ✅ 设计 SQLite metadata schema
4. ✅ 编写迁移工具（但当前无数据需迁移）

**需要确认**：
1. ⚠️ 为什么数据库为空？是否有其他存储路径？
2. ⚠️ `hub_catalog.db` 的作用？
3. ⚠️ 实际运行时 `SqliteMemoryBackend::new()` 被调用的位置？

---

## 附录

### A. 测试命令记录

```bash
# 查看数据库表
sqlite3 ~/Library/Application\ Support/aleph/aleph.db '.tables'
# → 无输出（空数据库）

# 查看数据库大小
ls -lh ~/Library/Application\ Support/aleph/aleph.db
# → 0B

# 统计 Markdown 文件
find ~/.aleph/memory/note -name '*.md' -type f | wc -l
# → 508

# 查看目录大小
du -sh ~/.aleph/memory/note/main/
# → 1.5M

# 查看 Dream 事件
cat ~/.aleph/memory/note/main/dream_events.jsonl | head -3
# → 3 个 JSON 对象，每个记录一次 dream cycle
```

### B. 代码入口点

```rust
// 数据库初始化
src/memory/store/sqlite/mod.rs:157
    pub fn new(db_path: &Path) -> Result<Self, AlephError>

// Raw memory 写入
src/memory/store/sqlite/raw_memories.rs:45
    pub async fn insert_raw_memory(&self, memory: RawMemory)

// Notes 索引更新
src/memory/store/sqlite/notes/store_impl.rs:89
    pub async fn upsert_note(&self, note: &NoteMetadata)
```

### C. 参考文档

- [MEMORY_SYSTEM.md](../reference/MEMORY_SYSTEM.md)
- [Pi Memory Extension README](~/.pi/agent/npm/node_modules/pi-memory/README.md)
- [Aleph AGENTS.md](../../AGENTS.md) — Redlines R1, R3, R10

---

**调研完成时间**：2026-10-18 19:30  
**下一步**：Phase 2 设计文档 — 目标架构与迁移方案
