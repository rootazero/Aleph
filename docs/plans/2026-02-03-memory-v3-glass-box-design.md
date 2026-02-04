# Aleph Memory System v3: "Glass Box" Architecture

> 设计日期: 2026-02-03
> 状态: Draft
> 基于: Memory v2 设计 + 架构审查报告

---

## 1. 设计概述

### 1.1 核心隐喻：Glass Box (透明盒)

Memory v2 追求"拟人化"——Ebbinghaus 衰减、信号驱动压缩、动态联想。
这些特性让 Aleph 的记忆更像人脑，但也让它变成了一个**黑盒**：
用户无法直观理解 AI 记住了什么、为什么忘记、当前在想什么。

v3 的目标是在保留 v2 智能特性的前提下，打造一个**透明盒**：
- **可观测 (Observable)**: 用户随时能看到记忆状态
- **可干预 (Intervenable)**: 用户能直接编辑、恢复、删除记忆
- **可解释 (Explainable)**: 每个记忆决策都有可追溯的理由

### 1.2 设计目标

| 优先级 | 目标 | v2 状态 | v3 改进 |
|--------|------|---------|---------|
| P0 | CLI 场景适配 | ❌ 依赖后台 | ✅ 懒惰计算 |
| P0 | 用户可控性 | ❌ 纯 AI 原生 | ✅ CLI 工具 |
| P1 | 工作记忆 | ❌ 缺失 | ✅ Scratchpad |
| P1 | 安全兜底 | ⚠️ 仅信号 | ✅ 混合触发 |
| P2 | 可解释性 | ❌ 无 | ✅ 审计日志 |

### 1.3 设计决策记录

| 决策点 | 选项 | 最终选择 | 理由 |
|--------|------|----------|------|
| Scratchpad 存储 | 纯内存 / SQLite / 独立文件 | **独立文件** | 人类可直接编辑，符合 Glass Box 理念 |
| Scratchpad 格式 | JSON / Markdown | **Markdown** | LLM 亲和，IDE 可渲染，用户友好 |
| Scratchpad 位置 | 全局 / 项目本地 | **项目本地** | 上下文隔离，就近原则 |
| Scratchpad GC | 删除 / 归档 | **归档** | 保留完整可追溯历史 |
| Decay 删除策略 | 硬删除 / 软删除 / 降级存档 | **软删除** | 可恢复，复用现有机制，GC 解耦 |
| CLI 架构 | 直连 SQLite / Gateway RPC / 混合 | **直连 SQLite + 文件锁** | 离线可用，自包含，单写多读 |

---

## 2. 整体架构

### 2.1 架构图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Memory System v3: Glass Box                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │                         Data Plane (数据面)                             │ │
│  │                                                                         │ │
│  │   ┌─────────────┐     ┌─────────────────┐     ┌─────────────┐         │ │
│  │   │   Layer 1   │     │   Layer 1.5     │     │   Layer 2   │         │ │
│  │   │ Raw Stream  │────▶│   Scratchpad    │────▶│    Facts    │         │ │
│  │   │  (memories) │     │ (.aleph/*.md)  │     │  (SQLite)   │         │ │
│  │   └─────────────┘     └─────────────────┘     └──────┬──────┘         │ │
│  │                        永不压缩 │ 任务完成              │               │ │
│  │                                 └──────────归档────────┘               │ │
│  │                                                        │               │ │
│  │                                                        ▼               │ │
│  │                                              ┌─────────────────┐       │ │
│  │                                              │    Layer 3      │       │ │
│  │                                              │ Dynamic Assoc.  │       │ │
│  │                                              │ (检索时计算)     │       │ │
│  │                                              └─────────────────┘       │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │                        Control Plane (控制面)                           │ │
│  │                                                                         │ │
│  │   ┌─────────────┐     ┌─────────────────┐     ┌─────────────┐         │ │
│  │   │   Signal    │     │  Token Limiter  │     │ Lazy Decay  │         │ │
│  │   │  Detector   │     │   (硬性兜底)     │     │  Engine     │         │ │
│  │   └──────┬──────┘     └────────┬────────┘     └──────┬──────┘         │ │
│  │          │                     │                     │                 │ │
│  │          └─────────OR──────────┘                     │                 │ │
│  │                    │                                 │                 │ │
│  │                    ▼                                 ▼                 │ │
│  │            ┌─────────────┐                  ┌─────────────────┐       │ │
│  │            │ Compressor  │                  │ On-Read Decay   │       │ │
│  │            │ (压缩服务)   │                  │ (读时衰减计算)   │       │ │
│  │            └─────────────┘                  └─────────────────┘       │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │                      Management Plane (管控面) ⭐ 新增                  │ │
│  │                                                                         │ │
│  │   ┌─────────────┐     ┌─────────────────┐     ┌─────────────┐         │ │
│  │   │ aleph CLI  │     │  Explainability │     │   Recycle   │         │ │
│  │   │ memory ...  │     │      API        │     │     Bin     │         │ │
│  │   └──────┬──────┘     └────────┬────────┘     └──────┬──────┘         │ │
│  │          │                     │                     │                 │ │
│  │          └─────────────────────┼─────────────────────┘                 │ │
│  │                                ▼                                       │ │
│  │                    ┌───────────────────────┐                           │ │
│  │                    │   SQLite (memory.db)  │                           │ │
│  │                    │   + File Lock (.lock) │                           │ │
│  │                    └───────────────────────┘                           │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 与 v2 的关键差异

| 组件 | v2 | v3 |
|------|----|----|
| Layer 1.5 | 不存在 | Scratchpad (项目本地 .md) |
| 衰减执行 | DreamDaemon 后台扫描 | Lazy Decay 读时计算 |
| 压缩触发 | 仅 Signal Detector | Signal + Token 阈值兜底 |
| 用户接口 | 无 | CLI 工具 + 可解释 API |
| 删除策略 | 软删除 | 软删除 + 回收站 + 延迟 GC |

---

## 3. Session Scratchpad (工作记忆区)

### 3.1 问题陈述

v2 的记忆系统偏向"长期知识"，缺乏对"当前任务状态"的显式管理。
当用户执行一个跨越 50 轮对话的复杂任务时，早期制定的计划可能被压缩成
"用户在重构代码"，导致 AI 忘记"当前进行到第几步"。

### 3.2 设计方案

**核心思想**：引入一个**免疫压缩**的临时存储区，专门保存当前活跃任务的状态。

#### 文件结构

```
project/
└── .aleph/
    ├── scratchpad.md          # 当前活跃任务状态
    ├── session_history.log    # 已完成任务归档
    └── config.toml            # 项目级配置 (可选)
```

#### scratchpad.md 模板

```markdown
# Current Task

## Objective
[任务目标，由 Agent 或用户填写]

## Plan
- [ ] Step 1: ...
- [ ] Step 2: ...
- [x] Step 3: ... (completed)

## Working State
[中间变量、临时数据、当前进度]

## Notes
[Agent 的思考笔记、用户的补充说明]

---
_Last updated: 2026-02-03T14:30:00Z_
_Session: abc123_
```

### 3.3 生命周期

```
┌─────────────────────────────────────────────────────────────────┐
│                    Scratchpad Lifecycle                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  [1] 创建 (Creation)                                             │
│      触发条件:                                                   │
│      • 用户显式: "我们开始重构 auth 模块"                         │
│      • Agent 规划: 生成多步计划时自动创建                         │
│      • 文件不存在时首次写入                                       │
│                          │                                       │
│                          ▼                                       │
│  [2] 维持 (Active)                                               │
│      • 每轮对话结束时，Agent 可更新进度                           │
│      • 内容始终以 Raw Text 注入 Context (高优先级)                │
│      • 免疫 Token 压缩：即使触发压缩，Scratchpad 内容不动          │
│      • 用户可随时 vim .aleph/scratchpad.md 手动编辑              │
│                          │                                       │
│                          ▼                                       │
│  [3] 归档 (Archive)                                              │
│      触发条件:                                                   │
│      • Signal Detector 检测到 Milestone (任务完成)                │
│      • 用户显式: "这个任务完成了" / aleph scratchpad clear       │
│                          │                                       │
│      归档流程:           │                                       │
│      ┌───────────────────┴───────────────────┐                   │
│      │ 1. 读取 scratchpad.md 内容             │                   │
│      │ 2. LLM 提炼关键事实 → 存入 Layer 2     │                   │
│      │ 3. 原始内容追加到 session_history.log  │                   │
│      │ 4. 重置 scratchpad.md 为空模板         │                   │
│      └───────────────────────────────────────┘                   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 3.4 Context 注入策略

Scratchpad 内容在构建 Prompt 时**始终置顶**，优先级高于检索到的 Facts：

```
System Prompt
├── [1] Scratchpad (如果存在，原样注入)
├── [2] Retrieved Facts (最多 N 条，按相关性)
├── [3] Session Summary (如果有压缩历史)
└── [4] Recent Messages (最近 K 轮对话)
```

---

## 4. Lazy Decay Engine (懒惰衰减引擎)

### 4.1 问题陈述

v2 依赖 DreamDaemon 在"空闲时间"执行衰减计算和清理。但作为 CLI 工具，
Aleph 的生命周期往往很短（执行完命令就退出），DreamDaemon 可能永远
没有机会运行，导致记忆库只增不减，长期使用后查询性能和 Token 消耗失控。

### 4.2 设计方案

**核心思想**：将衰减计算从"后台批处理"改为"读时即时计算"。

#### 4.2.1 衰减公式 (沿用 v2 Ebbinghaus 曲线)

```rust
/// 计算当前记忆强度
pub fn calculate_strength(
    last_accessed: i64,      // 最后访问时间戳 (秒)
    access_count: u32,       // 累计访问次数
    created_at: i64,         // 创建时间戳
    now: i64,                // 当前时间戳
    config: &DecayConfig,
) -> f32 {
    let days_since_access = (now - last_accessed) as f32 / 86400.0;

    // 基础衰减：指数衰减曲线
    // strength = 0.5 ^ (days / half_life)
    let base_decay = 0.5_f32.powf(days_since_access / config.half_life_days);

    // 访问加成：每次访问 +boost，上限 2.0
    let access_boost = (access_count as f32 * config.access_boost).min(2.0);

    // 最终强度 = 基础衰减 × (1 + 访问加成)，上限 1.0
    (base_decay * (1.0 + access_boost)).min(1.0)
}
```

#### 4.2.2 读时衰减流程

```
检索请求: "Rust 所有权"
         │
         ▼
┌─────────────────────────────────────────────────────────────┐
│ [1] 执行混合检索 (Vector + BM25)                             │
│     SELECT * FROM facts WHERE is_valid = 1 ...              │
│     返回候选集: [Fact A, Fact B, Fact C, ...]               │
└─────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────┐
│ [2] 对每个候选计算实时强度                                   │
│                                                              │
│     for fact in candidates:                                  │
│         strength = calculate_strength(fact, now, config)     │
│                                                              │
│         if strength < config.min_strength:                   │
│             # 标记为衰减失效                                  │
│             pending_invalidations.push(fact.id)              │
│             continue  # 不返回给 Agent                       │
│                                                              │
│         # 更新访问记录                                        │
│         fact.access_count += 1                               │
│         fact.last_accessed = now                             │
│         pending_updates.push(fact)                           │
│         results.push(fact)                                   │
└─────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────┐
│ [3] 异步写回 (不阻塞检索响应)                                │
│                                                              │
│     tokio::spawn(async move {                                │
│         // 批量更新访问记录                                   │
│         db.batch_update_access(pending_updates).await;       │
│                                                              │
│         // 软删除衰减失效的记忆                               │
│         for id in pending_invalidations {                    │
│             db.soft_delete(id, "decay", now).await;          │
│         }                                                    │
│     });                                                      │
└─────────────────────────────────────────────────────────────┘
         │
         ▼
    返回 results 给 Agent
```

### 4.3 数据库 Schema 变更

```sql
-- 新增字段
ALTER TABLE memory_facts ADD COLUMN decay_invalidated_at INTEGER;

-- 索引优化：支持回收站查询
CREATE INDEX idx_facts_decay_invalidated
    ON memory_facts(decay_invalidated_at)
    WHERE decay_invalidated_at IS NOT NULL;

-- 软删除时的更新
UPDATE memory_facts
SET is_valid = 0,
    invalidation_reason = 'decay',
    decay_invalidated_at = ?  -- Unix timestamp
WHERE id = ?;
```

### 4.4 类型保护 (沿用 v2)

某些类型的记忆不应被自动衰减：

| FactType | 衰减策略 |
|----------|---------|
| `personal` | 永不自动衰减 (protected) |
| `preference` | 半衰期 × 2 |
| `ephemeral` | 半衰期 × 0.5 |
| 其他 | 正常衰减 |

```rust
pub fn get_effective_half_life(fact_type: &FactType, base: f32) -> f32 {
    match fact_type {
        FactType::Personal => f32::INFINITY,  // 永不衰减
        FactType::Preference => base * 2.0,
        FactType::Ephemeral => base * 0.5,
        _ => base,
    }
}
```

### 4.5 与 DreamDaemon 的关系

Lazy Decay **不取代** DreamDaemon，而是**互补**：

| 职责 | Lazy Decay | DreamDaemon |
|------|------------|-------------|
| 衰减计算 | ✅ 读时即时 | ❌ 不再负责 |
| 软删除标记 | ✅ 异步执行 | ❌ 不再负责 |
| 物理删除 (GC) | ❌ | ✅ 空闲时执行 |
| 聚类分析 | ❌ | ✅ 保留 |
| Daily Insights | ❌ | ✅ 保留 |

---

## 5. Hybrid Trigger (混合触发策略)

### 5.1 问题陈述

v2 完全依赖 Signal Detector 来触发压缩。虽然"在语义边界压缩"比"机械截断"
更智能，但存在漏判风险：如果关键词库遗漏了用户的"隐含结束语"，或 LLM 判断
失误，Context 可能无限膨胀直到 Token 窗口爆炸。

### 5.2 设计方案

**核心思想**：Signal Detector 负责"智能触发"，Token Limiter 负责"安全兜底"。

```
                    ┌─────────────────────────────────────┐
                    │        Compression Trigger          │
                    └─────────────────────────────────────┘
                                     │
                    ┌────────────────┴────────────────┐
                    │                                 │
                    ▼                                 ▼
        ┌─────────────────────┐           ┌─────────────────────┐
        │   Signal Detector   │           │   Token Limiter     │
        │   (智能触发)         │           │   (硬性兜底)         │
        │                     │           │                     │
        │ • Milestone 信号    │           │ • token_count >     │
        │ • Context Switch    │           │   max_window × 0.9  │
        │ • Learning 信号     │           │                     │
        │ • Correction 信号   │           │ 无条件触发，不管     │
        │                     │           │ 语义是否完整         │
        └──────────┬──────────┘           └──────────┬──────────┘
                   │                                  │
                   └──────────── OR ──────────────────┘
                                  │
                                  ▼
                    ┌─────────────────────────────────┐
                    │         Compressor              │
                    │   (执行压缩，提取 Facts)         │
                    └─────────────────────────────────┘
```

### 5.3 实现代码

```rust
// core/src/memory/compression/trigger.rs

pub struct CompressionTrigger {
    signal_detector: SignalDetector,
    config: TriggerConfig,
}

pub struct TriggerConfig {
    /// Token 窗口上限
    pub max_token_window: usize,       // 默认: 128000

    /// 触发压缩的 Token 阈值比例
    pub trigger_threshold: f32,         // 默认: 0.9

    /// 压缩后的目标 Token 比例
    pub target_after_compression: f32,  // 默认: 0.5

    /// 是否启用信号检测
    pub signal_detection_enabled: bool, // 默认: true
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            max_token_window: 128_000,
            trigger_threshold: 0.9,
            target_after_compression: 0.5,
            signal_detection_enabled: true,
        }
    }
}

#[derive(Debug)]
pub enum TriggerReason {
    /// 信号检测触发
    Signal(CompressionSignal),
    /// Token 阈值触发
    TokenThreshold { current: usize, max: usize },
    /// 两者同时
    Both { signal: CompressionSignal, tokens: usize },
}

impl CompressionTrigger {
    pub fn check(&self, session: &Session) -> Option<TriggerReason> {
        let token_count = session.estimate_tokens();
        let threshold = (self.config.max_token_window as f32
                        * self.config.trigger_threshold) as usize;

        // 检测信号
        let signal = if self.config.signal_detection_enabled {
            self.signal_detector.detect(&session.recent_messages())
        } else {
            None
        };

        // 决策逻辑
        match (signal, token_count > threshold) {
            (Some(s), true) => Some(TriggerReason::Both {
                signal: s,
                tokens: token_count
            }),
            (Some(s), false) => Some(TriggerReason::Signal(s)),
            (None, true) => Some(TriggerReason::TokenThreshold {
                current: token_count,
                max: threshold
            }),
            (None, false) => None,
        }
    }
}
```

### 5.4 压缩行为差异

触发原因不同，压缩策略也略有不同：

| 触发原因 | 压缩策略 | 理由 |
|----------|---------|------|
| Signal (Milestone) | 完整压缩 + 归档 Scratchpad | 任务真正完成，可以彻底总结 |
| Signal (ContextSwitch) | 仅压缩旧话题 | 新话题刚开始，保留完整 |
| TokenThreshold | 激进压缩 + 保留最近 N 轮 | 紧急模式，优先保证可用性 |
| Both | 完整压缩 | 双重确认，放心执行 |

```rust
impl Compressor {
    pub async fn execute(&self, reason: TriggerReason, session: &mut Session) {
        match reason {
            TriggerReason::Signal(CompressionSignal::Milestone { .. }) => {
                // 完整压缩 + 归档 Scratchpad
                self.compress_full(session).await;
                self.archive_scratchpad(session).await;
            }
            TriggerReason::Signal(CompressionSignal::ContextSwitch { from_topic, .. }) => {
                // 仅压缩旧话题相关的消息
                self.compress_topic(session, &from_topic).await;
            }
            TriggerReason::TokenThreshold { .. } => {
                // 激进压缩：保留最近 5 轮，其余全部压缩
                self.compress_aggressive(session, 5).await;
            }
            TriggerReason::Both { .. } => {
                self.compress_full(session).await;
                self.archive_scratchpad(session).await;
            }
        }
    }
}
```

### 5.5 监控与告警

当 TokenThreshold 触发时，应记录日志以便后续优化 Signal Detector：

```rust
if matches!(reason, TriggerReason::TokenThreshold { .. }) {
    tracing::warn!(
        token_count = %token_count,
        "Compression triggered by token threshold (signal detector missed)"
    );

    // 记录到统计表，用于分析漏判模式
    metrics::counter!("compression.trigger.token_threshold").increment(1);
}
```

---

## 6. CLI 管理工具 (aleph memory)

### 6.1 设计原则

作为 "Glass Box" 架构的核心交互界面，CLI 工具遵循以下原则：

1. **自包含性**：直连 SQLite，不依赖 Gateway 运行
2. **安全并发**：通过文件锁实现"单写多读"
3. **Unix 哲学**：输出可管道、可组合、可脚本化
4. **可解释性**：每个操作都有清晰的反馈

### 6.2 文件锁机制

```
~/.aleph/
├── memory.db          # SQLite 数据库
├── memory.db-wal      # WAL 日志
├── memory.db-shm      # 共享内存
└── memory.lock        # 写锁文件

project/.aleph/
├── scratchpad.md      # 工作记忆
└── session_history.log
```

```rust
// core/src/memory/cli/lock.rs

use std::fs::{File, OpenOptions};
use fs2::FileExt;

pub struct MemoryLock {
    lock_file: File,
    mode: LockMode,
}

pub enum LockMode {
    Read,   // 共享锁，允许多个读取者
    Write,  // 独占锁，阻止其他所有访问
}

impl MemoryLock {
    pub fn acquire(mode: LockMode) -> Result<Self, LockError> {
        let lock_path = dirs::home_dir()
            .unwrap()
            .join(".aleph/memory.lock");

        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;

        match mode {
            LockMode::Read => {
                // 尝试获取共享锁（非阻塞）
                lock_file.try_lock_shared()
                    .map_err(|_| LockError::ReadLockFailed)?;
            }
            LockMode::Write => {
                // 尝试获取独占锁（非阻塞）
                match lock_file.try_lock_exclusive() {
                    Ok(_) => {}
                    Err(_) => {
                        return Err(LockError::GatewayRunning {
                            hint: "Gateway 正在运行。请先停止 Gateway，\
                                   或使用只读命令 (list, show, status)".into()
                        });
                    }
                }
            }
        }

        Ok(Self { lock_file, mode })
    }
}

impl Drop for MemoryLock {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}
```

### 6.3 命令规范

```
aleph memory <COMMAND>

Commands:
  list      列出记忆 (只读)
  show      显示单条记忆详情 (只读)
  search    语义搜索 (只读)
  status    显示统计信息 (只读)

  add       添加新记忆 (写入)
  edit      编辑记忆内容 (写入)
  forget    软删除记忆 (写入)
  restore   恢复已删除的记忆 (写入)
  gc        物理删除过期记忆 (写入)

  dump      导出所有记忆 (只读)
  import    导入记忆备份 (写入)

Options:
  -f, --format <FORMAT>  输出格式 [default: table] [possible: table, json, csv]
  -v, --verbose          显示详细信息
  -h, --help             显示帮助
```

### 6.4 命令详解

#### 6.4.1 list - 列出记忆

```bash
# 基本用法
$ aleph memory list
ID          TYPE        STRENGTH  CONTENT (truncated)
─────────────────────────────────────────────────────────────
f8a3b...   preference   0.92     用户偏好使用 Rust 而非 Go
c2d1e...   knowledge    0.78     Aleph 项目使用 sqlite-vec
a9f0c...   personal     1.00     用户的 GitHub 用户名是 x

# 筛选选项
$ aleph memory list --type preference
$ aleph memory list --query "rust"          # 关键词过滤
$ aleph memory list --min-strength 0.5      # 强度过滤
$ aleph memory list --include-decayed       # 包含回收站

# JSON 输出（可管道）
$ aleph memory list --format json | jq '.[] | select(.strength < 0.3)'
```

#### 6.4.2 show - 显示详情

```bash
$ aleph memory show f8a3b

┌─────────────────────────────────────────────────────────────┐
│ Fact: f8a3b2c1-...                                          │
├─────────────────────────────────────────────────────────────┤
│ Content:    用户偏好使用 Rust 而非 Go，因为更喜欢            │
│             所有权模型带来的编译期安全保证                    │
│                                                              │
│ Type:       preference                                       │
│ Strength:   0.92                                             │
│ Created:    2026-01-15 10:30:00                             │
│ Accessed:   2026-02-02 14:20:00 (12 times)                  │
│ Source:     session (extracted from conversation)           │
│                                                              │
│ Specificity:    pattern                                      │
│ Temporal Scope: permanent                                    │
└─────────────────────────────────────────────────────────────┘
```

#### 6.4.3 add - 手动添加

```bash
# 交互式添加
$ aleph memory add
Content: 用户的常用开发端口是 8080 和 3000
Type [knowledge]: preference
Added: f9c2a... (strength: 1.0, protected: user-added)

# 单行添加
$ aleph memory add "用户讨厌 YAML 配置文件" --type preference

# 从文件添加
$ aleph memory add --file notes.md --type knowledge
```

#### 6.4.4 edit - 编辑内容

```bash
$ aleph memory edit f8a3b

# 打开 $EDITOR (vim, nano, etc.)，内容格式：
# ---
# id: f8a3b2c1-...
# type: preference
# ---
# 用户偏好使用 Rust 而非 Go，因为更喜欢
# 所有权模型带来的编译期安全保证
#
# [在此编辑内容，保存退出后生效]

Saved: f8a3b (content updated)
```

#### 6.4.5 forget / restore - 回收站操作

```bash
# 软删除（移入回收站）
$ aleph memory forget f8a3b
Moved to recycle bin: f8a3b
(Will be permanently deleted after 30 days, or use 'gc' to delete now)

# 查看回收站
$ aleph memory list --include-decayed --filter "is_valid = 0"

# 恢复
$ aleph memory restore f8a3b
Restored: f8a3b (strength reset to 0.5)
```

#### 6.4.6 gc - 垃圾回收

```bash
# 预览将被删除的内容
$ aleph memory gc --dry-run
Will permanently delete 23 facts:
  - 15 decayed (invalidated > 30 days ago)
  - 8 ephemeral (created > 7 days ago)

# 执行删除
$ aleph memory gc
Deleted 23 facts. Freed 1.2 MB.

# 强制删除所有回收站内容
$ aleph memory gc --force
```

#### 6.4.7 dump / import - 备份恢复

```bash
# 导出为 JSON
$ aleph memory dump > backup.json

# 导出为 Markdown（人类可读）
$ aleph memory dump --format markdown > backup.md

# 导入
$ aleph memory import backup.json
Imported 156 facts (23 skipped as duplicates)
```

### 6.5 Scratchpad 专用命令

```bash
# 查看当前 Scratchpad
$ aleph scratchpad show
# 或直接: cat .aleph/scratchpad.md

# 手动清空并归档
$ aleph scratchpad clear
Archived to: .aleph/session_history.log
Extracted 3 facts to memory.
Scratchpad reset.

# 查看历史
$ aleph scratchpad history
```

---

## 7. Explainability API (可解释性接口)

### 7.1 设计目标

"AI 为什么记住这个？" 和 "AI 为什么忘了那个？" 是用户信任系统的关键。
可解释性 API 让每个记忆决策都有可追溯的理由。

### 7.2 审计日志 Schema

```sql
CREATE TABLE memory_audit_log (
    id TEXT PRIMARY KEY,
    fact_id TEXT NOT NULL,
    action TEXT NOT NULL,           -- 'created', 'accessed', 'updated',
                                    -- 'invalidated', 'restored', 'deleted'
    reason TEXT,                    -- 人类可读的原因
    actor TEXT NOT NULL,            -- 'agent', 'user', 'system', 'decay'
    details TEXT,                   -- JSON 格式的详细信息
    created_at INTEGER NOT NULL,

    FOREIGN KEY (fact_id) REFERENCES memory_facts(id)
);

CREATE INDEX idx_audit_fact ON memory_audit_log(fact_id);
CREATE INDEX idx_audit_time ON memory_audit_log(created_at);
CREATE INDEX idx_audit_action ON memory_audit_log(action);
```

### 7.3 审计事件类型

```rust
pub enum AuditAction {
    /// 记忆创建
    Created {
        source: FactSource,           // session, user, tool
        extraction_context: String,   // 从哪段对话提取
    },

    /// 记忆被检索使用
    Accessed {
        query: String,                // 检索查询
        relevance_score: f32,         // 相关性得分
        used_in_response: bool,       // 是否实际用于生成回复
    },

    /// 记忆内容更新
    Updated {
        old_content: String,
        new_content: String,
        reason: UpdateReason,         // conflict_merge, user_edit, correction
    },

    /// 记忆失效
    Invalidated {
        reason: InvalidationReason,   // decay, conflict, user_forget
        strength_at_invalidation: f32,
    },

    /// 记忆恢复
    Restored {
        by: String,                   // user, admin
        new_strength: f32,
    },

    /// 物理删除
    Deleted {
        reason: String,               // gc, user_permanent_delete
        days_in_recycle_bin: u32,
    },
}
```

### 7.4 查询接口

```rust
// core/src/memory/explainability.rs

pub struct ExplainabilityApi {
    db: Arc<MemoryDb>,
}

impl ExplainabilityApi {
    /// 解释单条记忆的完整生命周期
    pub async fn explain_fact(&self, fact_id: &str) -> FactExplanation {
        let fact = self.db.get_fact(fact_id).await?;
        let history = self.db.get_audit_log(fact_id).await?;

        FactExplanation {
            fact,
            lifecycle: history,
            summary: self.generate_summary(&history),
        }
    }

    /// 解释为什么某条记忆被遗忘
    pub async fn explain_forgetting(&self, fact_id: &str) -> ForgettingExplanation {
        let invalidation = self.db.get_invalidation_event(fact_id).await?;

        match invalidation.reason {
            InvalidationReason::Decay => ForgettingExplanation {
                reason: "记忆强度衰减至阈值以下".into(),
                details: format!(
                    "最后访问: {} ({} 天前)\n\
                     访问次数: {}\n\
                     衰减时强度: {:.2}",
                    invalidation.last_accessed,
                    invalidation.days_since_access,
                    invalidation.access_count,
                    invalidation.strength_at_invalidation,
                ),
                can_restore: true,
            },
            InvalidationReason::Conflict { winner_id } => ForgettingExplanation {
                reason: "被更新的记忆覆盖".into(),
                details: format!("新记忆 ID: {}", winner_id),
                can_restore: true,
            },
            InvalidationReason::UserForget => ForgettingExplanation {
                reason: "用户手动删除".into(),
                details: format!("操作时间: {}", invalidation.created_at),
                can_restore: true,
            },
        }
    }

    /// 解释为什么某次检索返回了这些结果
    pub async fn explain_retrieval(&self, query: &str) -> RetrievalExplanation {
        let results = self.db.search_with_scores(query).await?;

        RetrievalExplanation {
            query: query.to_string(),
            results: results.iter().map(|(fact, score)| {
                RetrievalResult {
                    fact_id: fact.id.clone(),
                    content_preview: fact.content.chars().take(50).collect(),
                    vector_score: score.vector,
                    bm25_score: score.bm25,
                    combined_score: score.combined,
                    strength: fact.strength_score,
                    boost_reason: self.explain_boost(fact),
                }
            }).collect(),
        }
    }
}
```

### 7.5 CLI 集成

```bash
# 解释单条记忆的生命周期
$ aleph memory explain f8a3b

┌─────────────────────────────────────────────────────────────┐
│ Fact Lifecycle: f8a3b                                        │
├─────────────────────────────────────────────────────────────┤
│ 2026-01-15 10:30  CREATED                                    │
│   Source: session (extracted from conversation)              │
│   Context: "用户说：我更喜欢 Rust，因为..."                   │
│                                                              │
│ 2026-01-20 14:00  ACCESSED                                   │
│   Query: "用户的编程语言偏好"                                 │
│   Score: 0.89, Used in response: Yes                         │
│                                                              │
│ 2026-01-25 09:15  ACCESSED                                   │
│   Query: "Rust vs Go"                                        │
│   Score: 0.76, Used in response: Yes                         │
│                                                              │
│ ... (10 more events)                                         │
│                                                              │
│ Current Status: Active (strength: 0.92)                      │
└─────────────────────────────────────────────────────────────┘

# 解释为什么某条记忆被遗忘
$ aleph memory explain a1b2c --why-forgotten

┌─────────────────────────────────────────────────────────────┐
│ Why was this forgotten?                                      │
├─────────────────────────────────────────────────────────────┤
│ Reason: 记忆强度衰减至阈值以下                                │
│                                                              │
│ Details:                                                     │
│   Last accessed:  2025-12-01 (64 days ago)                  │
│   Access count:   2                                          │
│   Strength when invalidated: 0.08                            │
│   Threshold: 0.10                                            │
│                                                              │
│ Recovery: aleph memory restore a1b2c                        │
└─────────────────────────────────────────────────────────────┘
```

---

## 8. Recycle Bin (记忆回收站)

### 8.1 回收站状态机

```
                    ┌─────────────┐
                    │   Active    │
                    │ is_valid=1  │
                    └──────┬──────┘
                           │
         ┌─────────────────┼─────────────────┐
         │                 │                 │
         ▼                 ▼                 ▼
    [User Forget]    [Decay < 0.1]    [Conflict]
         │                 │                 │
         └─────────────────┼─────────────────┘
                           │
                           ▼
                    ┌─────────────┐
                    │ Recycle Bin │
                    │ is_valid=0  │
                    │ + reason    │
                    │ + timestamp │
                    └──────┬──────┘
                           │
         ┌─────────────────┼─────────────────┐
         │                 │                 │
         ▼                 ▼                 ▼
    [User Restore]   [30 days pass]   [User GC --force]
         │                 │                 │
         ▼                 │                 │
    ┌─────────────┐        │                 │
    │   Active    │        │                 │
    │ strength=0.5│        │                 │
    └─────────────┘        │                 │
                           ▼                 ▼
                    ┌─────────────────────────┐
                    │    Permanently Deleted   │
                    │   (Physical DELETE)      │
                    └─────────────────────────┘
```

### 8.2 恢复策略

```rust
impl MemoryDb {
    pub async fn restore_fact(&self, fact_id: &str) -> Result<Fact> {
        let fact = self.get_fact_including_invalid(fact_id).await?;

        if fact.is_valid {
            return Err(Error::AlreadyActive);
        }

        // 恢复时重置强度为 0.5（给予"第二次机会"）
        let restored = self.update_fact(fact_id, |f| {
            f.is_valid = true;
            f.invalidation_reason = None;
            f.decay_invalidated_at = None;
            f.strength_score = 0.5;
            f.last_accessed = now();
        }).await?;

        // 记录审计日志
        self.log_audit(AuditAction::Restored {
            by: "user".into(),
            new_strength: 0.5,
        }).await?;

        Ok(restored)
    }
}
```

### 8.3 GC 配置

```toml
[memory.gc]
# 回收站保留天数
recycle_bin_retention_days = 30

# ephemeral 类型的保留天数（更短）
ephemeral_retention_days = 7

# GC 运行时机
run_on_startup = false          # 启动时不自动 GC
run_in_dream_daemon = true      # DreamDaemon 空闲时执行
manual_only = false             # 是否仅手动触发
```

---

## 9. Configuration (配置系统)

### 9.1 完整配置结构

```toml
# ~/.aleph/config.toml

[memory]
enabled = true
embedding_model = "bge-small-zh-v1.5"

# ===== Scratchpad (新增) =====
[memory.scratchpad]
enabled = true
# 位置策略: "project" (项目本地) 或 "global" (全局)
location = "project"
# 项目本地时的目录名
project_dir = ".aleph"
# 归档文件名
history_file = "session_history.log"
# 最大归档文件大小 (MB)，超出后轮转
max_history_size_mb = 10

# ===== Lazy Decay (新增) =====
[memory.decay]
enabled = true
# 半衰期 (天)
half_life_days = 30.0
# 每次访问增加的强度
access_boost = 0.2
# 低于此阈值触发软删除
min_strength = 0.1
# 受保护的类型 (永不自动衰减)
protected_types = ["personal"]
# 类型特定的半衰期倍数
[memory.decay.type_multipliers]
preference = 2.0      # 偏好类：半衰期 × 2
ephemeral = 0.5       # 临时类：半衰期 × 0.5

# ===== Hybrid Trigger (新增) =====
[memory.compression]
enabled = true
# Token 窗口上限
max_token_window = 128000
# 触发压缩的阈值 (百分比)
trigger_threshold = 0.9
# 压缩后的目标 (百分比)
target_after_compression = 0.5
# 是否启用信号检测
signal_detection_enabled = true
# 保留的最近消息轮数 (激进压缩时)
keep_recent_turns = 5

# ===== Signal Detector =====
[memory.compression.signals]
# 学习信号关键词
learning_keywords = [
    "记住", "以后", "偏好", "喜欢用", "习惯",
    "remember", "always", "prefer", "from now on"
]
# 纠错信号关键词
correction_keywords = [
    "不对", "搞错", "错了", "应该是",
    "wrong", "incorrect", "I meant"
]
# 里程碑信号关键词
milestone_keywords = [
    "完成", "搞定", "结束", "done", "finished"
]

# ===== Recycle Bin (新增) =====
[memory.gc]
# 回收站保留天数
recycle_bin_retention_days = 30
# ephemeral 类型保留天数
ephemeral_retention_days = 7
# 启动时自动 GC
run_on_startup = false
# DreamDaemon 中执行
run_in_dream_daemon = true

# ===== Retrieval =====
[memory.retrieval]
# 混合检索权重
vector_weight = 0.7
text_weight = 0.3
# 最小相关性得分
min_score = 0.35
# 最大返回结果数
max_results = 10

# ===== Explainability (新增) =====
[memory.audit]
enabled = true
# 审计日志保留天数
retention_days = 90
# 记录访问事件 (会产生大量日志)
log_access_events = true

# ===== DreamDaemon =====
[memory.dreaming]
enabled = true
idle_threshold_seconds = 900
window_start_local = "02:00"
window_end_local = "05:00"
max_duration_seconds = 600
```

### 9.2 项目级配置覆盖

项目目录下的 `.aleph/config.toml` 可覆盖全局配置：

```toml
# project/.aleph/config.toml

# 仅覆盖需要修改的项
[memory.decay]
# 这个项目需要更长的记忆保留
half_life_days = 60.0

[memory.compression]
# 这个项目对话较长，调高阈值
max_token_window = 200000
```

配置加载优先级：
```
项目级 (.aleph/config.toml) > 用户级 (~/.aleph/config.toml) > 默认值
```

---

## 10. Database Migration (数据库迁移)

### 10.1 迁移脚本

```sql
-- Migration: v2 → v3
-- File: migrations/003_glass_box.sql

-- 1. 添加 Lazy Decay 字段
ALTER TABLE memory_facts ADD COLUMN decay_invalidated_at INTEGER;

CREATE INDEX idx_facts_decay_invalidated
    ON memory_facts(decay_invalidated_at)
    WHERE decay_invalidated_at IS NOT NULL;

-- 2. 添加 v3 事实字段
ALTER TABLE memory_facts ADD COLUMN specificity TEXT DEFAULT 'pattern';
ALTER TABLE memory_facts ADD COLUMN temporal_scope TEXT DEFAULT 'contextual';

CREATE INDEX idx_facts_specificity ON memory_facts(specificity);
CREATE INDEX idx_facts_temporal ON memory_facts(temporal_scope);

-- 3. 创建审计日志表
CREATE TABLE IF NOT EXISTS memory_audit_log (
    id TEXT PRIMARY KEY,
    fact_id TEXT NOT NULL,
    action TEXT NOT NULL,
    reason TEXT,
    actor TEXT NOT NULL,
    details TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_audit_fact ON memory_audit_log(fact_id);
CREATE INDEX idx_audit_time ON memory_audit_log(created_at);
CREATE INDEX idx_audit_action ON memory_audit_log(action);

-- 4. 记录迁移版本
INSERT INTO schema_migrations (version, applied_at)
VALUES ('003_glass_box', strftime('%s', 'now'));
```

### 10.2 Rust 迁移代码

```rust
// core/src/memory/database/migrations.rs

pub async fn migrate_to_v3(db: &SqlitePool) -> Result<()> {
    let current_version = get_schema_version(db).await?;

    if current_version >= 3 {
        tracing::info!("Database already at v3 or higher");
        return Ok(());
    }

    tracing::info!("Migrating database from v{} to v3...", current_version);

    // 开启事务
    let mut tx = db.begin().await?;

    // 执行迁移
    sqlx::query(include_str!("migrations/003_glass_box.sql"))
        .execute(&mut *tx)
        .await?;

    // 提交事务
    tx.commit().await?;

    tracing::info!("Migration to v3 complete");
    Ok(())
}
```

### 10.3 CLI 迁移命令

```bash
# 检查数据库版本
$ aleph memory status
Database version: 2
Latest version: 3
Migration available: 003_glass_box

# 执行迁移
$ aleph memory migrate
Backing up database to ~/.aleph/memory.db.backup.20260203...
Applying migration 003_glass_box...
Migration complete. Database now at version 3.
```

---

## 11. Compression System Upgrade (压缩系统升级)

### 11.1 架构演进：控制反转

v2 → v3 的核心变化不是"替换"，而是"重组"：

```
v2 架构 (Pull 模式)：
┌─────────────────────────────────────────────────────────────┐
│                                                              │
│   Scheduler ──(turn_threshold)──▶ Service ──▶ Extractor    │
│       │                              │                       │
│       │                              ▼                       │
│       │                    get_uncompressed_memories()       │
│       │                              │                       │
│       └──────── "攒够 20 轮，压缩" ───┘                       │
│                                                              │
└─────────────────────────────────────────────────────────────┘

v3 架构 (Push 模式)：
┌─────────────────────────────────────────────────────────────┐
│                                                              │
│   SignalDetector ──(Milestone)──▶ ArchivalService           │
│         │                              │                     │
│         │                              ▼                     │
│         │                    read_scratchpad()               │
│         │                              │                     │
│         │                              ▼                     │
│         │                         Extractor ◄── (复用)       │
│         │                              │                     │
│   TokenLimiter ──(90% full)──────────▶│ (Safety Net)        │
│         │                              │                     │
│         └────── "任务完成，归档" ──────┘                      │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 11.2 模块升级清单

| 模块 | v2 角色 | v3 角色 | 操作 |
|------|---------|---------|------|
| `FactExtractor` | 核心引擎 | 核心引擎 | **保留增强** |
| `CompressionService` | 批处理流水线 | 归档服务 | **重构重命名** |
| `SignalDetector` | 辅助检测 | 主控制器 | **升级** |
| `CompressionScheduler` | 主触发器 | Safety Net | **降级** |
| `ConflictResolver` | 冲突处理 | 冲突处理 | **保留** |

### 11.3 FactExtractor 增强

```rust
// core/src/memory/compression/extractor.rs

// 保留现有结构，增加 v3 字段
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedFact {
    pub content: String,
    pub fact_type: FactType,
    pub confidence: f32,
    pub source_turn_ids: Vec<String>,

    // ===== v3 新增字段 =====
    /// 具体度：Principle / Pattern / Instance
    #[serde(default)]
    pub specificity: FactSpecificity,

    /// 时效性：Permanent / Contextual / Ephemeral
    #[serde(default)]
    pub temporal_scope: TemporalScope,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FactSpecificity {
    /// 原则级："用户偏好函数式编程"
    Principle,
    /// 模式级："用户处理错误时喜欢用 Result"
    #[default]
    Pattern,
    /// 实例级："用户在 2026-01-15 用了 anyhow"
    Instance,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TemporalScope {
    /// 长期有效："用户的母语是中文"
    Permanent,
    /// 上下文相关："用户当前在做 Aleph 项目"
    #[default]
    Contextual,
    /// 短期有效："用户今天想专注写文档"
    Ephemeral,
}

impl FactExtractor {
    /// v3: 更新 Prompt 以提取新字段
    fn get_system_prompt_v3() -> &'static str {
        r#"You are a fact extraction engine. Extract structured facts from the conversation.

For each fact, determine:
1. content: Third-person statement (e.g., "The user prefers Rust over Go")
2. fact_type: preference | knowledge | personal | skill | context | correction
3. confidence: 0.0-1.0
4. specificity:
   - principle: High-level preference ("User likes functional programming")
   - pattern: Recurring behavior ("User uses Result for error handling")
   - instance: Specific occurrence ("User used anyhow on 2026-01-15")
5. temporal_scope:
   - permanent: Always true ("User's native language is Chinese")
   - contextual: True in current context ("User is working on Aleph")
   - ephemeral: Short-term ("User wants to focus on docs today")

Output JSON array. Skip trivial or redundant facts."#
    }
}
```

### 11.4 CompressionService → ArchivalService

```rust
// core/src/memory/compression/service.rs → archival.rs

/// v3: 归档服务（从 CompressionService 重构）
pub struct ArchivalService {
    extractor: FactExtractor,
    embedder: Arc<SmartEmbedder>,
    db: Arc<MemoryDb>,
    conflict_resolver: ConflictResolver,
}

impl ArchivalService {
    /// v3 新增：从 Scratchpad 归档
    pub async fn archive_scratchpad(
        &self,
        scratchpad_path: &Path,
        session_context: &SessionContext,
    ) -> Result<ArchivalResult> {
        // 1. 读取 Scratchpad 内容
        let scratchpad_content = tokio::fs::read_to_string(scratchpad_path).await?;

        if scratchpad_content.trim().is_empty() {
            return Ok(ArchivalResult::empty());
        }

        // 2. 构建提取上下文（Scratchpad + 最近对话）
        let extraction_input = format!(
            "## Task Scratchpad\n{}\n\n## Recent Conversation\n{}",
            scratchpad_content,
            session_context.recent_messages_as_text(10)
        );

        // 3. 调用 Extractor（复用 v2 逻辑）
        let facts = self.extractor.extract_facts(&extraction_input).await?;

        // 4. 嵌入 + 冲突检测 + 存储（复用 v2 逻辑）
        let stored = self.store_facts_with_conflict_resolution(facts).await?;

        // 5. 归档原始内容到 session_history.log
        self.append_to_history_log(scratchpad_path, &scratchpad_content).await?;

        // 6. 重置 Scratchpad
        self.reset_scratchpad(scratchpad_path).await?;

        // 7. 记录审计日志
        self.log_archival_event(&stored).await?;

        Ok(ArchivalResult {
            facts_extracted: stored.len(),
            conflicts_resolved: stored.iter().filter(|f| f.had_conflict).count(),
        })
    }

    /// v2 兼容：保留原有批量压缩接口
    #[deprecated(note = "Use archive_scratchpad for v3 flow")]
    pub async fn compress(&self, batch_size: usize) -> Result<CompressionResult> {
        // 保留原有逻辑，用于渐进迁移
        self.compress_legacy(batch_size).await
    }

    /// 追加到历史日志
    async fn append_to_history_log(&self, scratchpad_path: &Path, content: &str) -> Result<()> {
        let history_path = scratchpad_path
            .parent()
            .unwrap()
            .join("session_history.log");

        let entry = format!(
            "\n--- Archived: {} ---\n{}\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
            content
        );

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&history_path)
            .await?;

        file.write_all(entry.as_bytes()).await?;
        Ok(())
    }

    /// 重置 Scratchpad 为空模板
    async fn reset_scratchpad(&self, path: &Path) -> Result<()> {
        let template = r#"# Current Task

## Objective
[No active task]

## Plan
- [ ] ...

## Working State


## Notes


---
_Last updated: _
"#;
        tokio::fs::write(path, template).await?;
        Ok(())
    }
}
```

### 11.5 SignalDetector 升级为主控制器

```rust
// core/src/memory/compression/signal_detector.rs

impl SignalDetector {
    /// v3: 集成到 Agent Loop，控制 Scratchpad 生命周期
    pub async fn process_turn(
        &self,
        session: &Session,
        scratchpad: &ScratchpadManager,
        archival: &ArchivalService,
    ) -> Result<TurnProcessingResult> {
        // 1. 检测信号
        let detection = self.detect(&session.last_messages(3));

        // 2. 根据信号类型决定动作
        match detection.priority {
            CompressionPriority::Immediate => {
                // Milestone 或 Correction: 立即归档 Scratchpad
                if scratchpad.has_content().await? {
                    let result = archival.archive_scratchpad(
                        scratchpad.path(),
                        &session.context(),
                    ).await?;

                    return Ok(TurnProcessingResult::Archived(result));
                }
            }
            CompressionPriority::Deferred => {
                // Learning: 记录到 Scratchpad，稍后归档
                if let Some(signal) = detection.signals.first() {
                    scratchpad.append_note(&format!(
                        "📝 Learned: {}",
                        signal.description()
                    )).await?;
                }
            }
            CompressionPriority::Batch => {
                // ContextSwitch: 总结旧话题，开始新 Scratchpad
                // (可选实现)
            }
        }

        Ok(TurnProcessingResult::NoAction)
    }
}
```

### 11.6 Scheduler 降级为 Safety Net

```rust
// core/src/memory/compression/scheduler.rs

impl CompressionScheduler {
    /// v3: 降级为 Safety Net，仅在 Token 溢出时触发
    pub async fn check_safety_net(
        &self,
        session: &Session,
        config: &TriggerConfig,
    ) -> Option<SafetyNetAction> {
        let token_count = session.estimate_tokens();
        let threshold = (config.max_token_window as f32 * config.trigger_threshold) as usize;

        if token_count > threshold {
            tracing::warn!(
                tokens = token_count,
                threshold = threshold,
                "Safety net triggered: token limit approaching"
            );

            return Some(SafetyNetAction::ForceSummarize {
                keep_recent_turns: config.keep_recent_turns,
            });
        }

        None
    }
}

pub enum SafetyNetAction {
    /// 强制总结：保留最近 N 轮，其余压缩
    ForceSummarize { keep_recent_turns: usize },
}
```

### 11.7 迁移路径

```
Phase 1: 并行运行
  • v2 流程保持不变
  • v3 Scratchpad 作为可选功能启用
  • 两套机制共存，观察效果

Phase 2: 渐进切换
  • 新 Session 默认使用 v3 流程
  • 旧 Session 继续 v2 直到完成
  • 收集指标对比

Phase 3: 完全迁移
  • 废弃 v2 触发逻辑
  • 保留 Extractor/ConflictResolver 核心组件
  • 删除冗余代码
```

---

## 12. Implementation Roadmap (实现路线图)

### 12.1 里程碑规划

```
M1 ─────────────────────────────────────────────────────────────
│  Scratchpad Foundation (基础设施)
│  • ScratchpadManager 实现
│  • 文件读写 + 模板
│  • Context 注入集成
│  验收: Agent 能读写 .aleph/scratchpad.md
│
M2 ─────────────────────────────────────────────────────────────
│  Lazy Decay Engine (懒惰衰减)
│  • 读时衰减计算
│  • 异步软删除
│  • decay_invalidated_at 字段
│  验收: 检索时自动过滤低强度记忆
│
M3 ─────────────────────────────────────────────────────────────
│  Hybrid Trigger (混合触发)
│  • Token Limiter 集成
│  • SignalDetector 升级
│  • 双路触发逻辑
│  验收: Token 达 90% 自动触发压缩
│
M4 ─────────────────────────────────────────────────────────────
│  Archival Pipeline (归档流水线)
│  • CompressionService → ArchivalService 重构
│  • Scratchpad → Facts 提取
│  • session_history.log 归档
│  验收: Milestone 信号触发完整归档流程
│
M5 ─────────────────────────────────────────────────────────────
│  CLI Tools (命令行工具)
│  • aleph memory 子命令
│  • 文件锁机制
│  • 只读降级策略
│  验收: Gateway 运行时 CLI 可读不可写
│
M6 ─────────────────────────────────────────────────────────────
│  Explainability (可解释性)
│  • 审计日志表
│  • explain 命令
│  • 回收站 UI
│  验收: aleph memory explain <id> 显示完整生命周期
```

### 12.2 依赖关系

```
        M1 (Scratchpad)
              │
              ▼
        M4 (Archival) ◄────── M3 (Hybrid Trigger)
              │                      │
              │                      │
              ▼                      ▼
        M2 (Lazy Decay)        SignalDetector
              │
              ▼
        M5 (CLI) ◄─────────── M6 (Explainability)

可并行开发:
  • M1 + M2 (无依赖)
  • M3 与 M1/M2 (弱依赖，可并行)
  • M5 + M6 (在 M1-M4 完成后)
```

### 12.3 文件结构变更

```
core/src/memory/
├── mod.rs                        # 更新: 导出新模块
├── scratchpad/                   # 🆕 新增目录
│   ├── mod.rs
│   ├── manager.rs                # ScratchpadManager
│   ├── template.rs               # Markdown 模板
│   └── history.rs                # session_history.log 管理
├── compression/
│   ├── mod.rs                    # 更新: 导出 ArchivalService
│   ├── archival.rs               # 🆕 新增 (从 service.rs 重构)
│   ├── service.rs                # 保留: 标记 deprecated
│   ├── extractor.rs              # 更新: 增加 v3 字段
│   ├── signal_detector.rs        # 更新: 升级为主控制器
│   ├── scheduler.rs              # 更新: 降级为 Safety Net
│   ├── trigger.rs                # 🆕 新增: Hybrid Trigger
│   └── conflict.rs               # 保留: 无变化
├── decay.rs                      # 更新: Lazy Decay 逻辑
├── cli/                          # 🆕 新增目录
│   ├── mod.rs
│   ├── lock.rs                   # 文件锁
│   ├── commands.rs               # 命令实现
│   └── output.rs                 # 格式化输出
├── explainability/               # 🆕 新增目录
│   ├── mod.rs
│   ├── audit.rs                  # 审计日志
│   ├── api.rs                    # 查询接口
│   └── recycle_bin.rs            # 回收站
├── database/
│   ├── core.rs                   # 更新: 新表 + 迁移
│   ├── facts/
│   │   ├── crud.rs               # 更新: decay_invalidated_at
│   │   └── ...
│   └── migrations/
│       └── 003_glass_box.sql     # 🆕 新增
└── ...
```

### 12.4 验收标准 (Acceptance Criteria)

#### M1: Scratchpad Foundation
```bash
# 创建 Scratchpad
$ echo "# Test" > .aleph/scratchpad.md

# Agent 对话时能读取
$ aleph chat "我的当前任务是什么？"
> 根据 Scratchpad，您当前的任务是: Test

# Agent 能更新 Scratchpad
$ aleph chat "开始重构 auth 模块，分 3 步"
$ cat .aleph/scratchpad.md
# Current Task
## Objective
重构 auth 模块
## Plan
- [ ] Step 1: ...
```

#### M2: Lazy Decay Engine
```bash
# 模拟 60 天未访问的记忆
$ sqlite3 ~/.aleph/memory.db "UPDATE memory_facts SET last_accessed = strftime('%s','now') - 5184000 WHERE id = 'test123'"

# 检索时应被过滤
$ aleph memory list --query "test"
# (不显示 test123)

# 但可在回收站中看到
$ aleph memory list --include-decayed
# test123 (decayed, strength: 0.08)
```

#### M3: Hybrid Trigger
```bash
# 填充大量 Token
$ for i in {1..100}; do aleph chat "讲一个长故事 $i"; done

# 观察日志：应触发 Safety Net
[WARN] Safety net triggered: token limit approaching (tokens: 115200, threshold: 115200)
[INFO] Compression executed: 95 messages → 3 facts
```

#### M4: Archival Pipeline
```bash
# 写入 Scratchpad
$ cat > .aleph/scratchpad.md << 'EOF'
# Current Task
## Objective
实现用户登录功能
## Plan
- [x] 设计 API
- [x] 实现后端
- [x] 测试
## Notes
使用 JWT，过期时间 7 天
EOF

# 触发归档
$ aleph chat "登录功能完成了"
[INFO] Milestone signal detected
[INFO] Archived scratchpad: 2 facts extracted

# 检查结果
$ aleph memory list --query "登录"
> 用户实现了登录功能，使用 JWT，过期时间 7 天

$ cat .aleph/session_history.log
--- Archived: 2026-02-03 15:30:00 ---
# Current Task
## Objective
实现用户登录功能
...
```

#### M5: CLI Tools
```bash
# Gateway 未运行时可读写
$ aleph memory list        # ✓
$ aleph memory add "test"  # ✓

# Gateway 运行时只读
$ aleph gateway start &
$ aleph memory list        # ✓ (只读)
$ aleph memory add "test"  # ✗ "Gateway 正在运行，请使用只读命令"
```

#### M6: Explainability
```bash
$ aleph memory explain f8a3b

┌─────────────────────────────────────────────────────────────┐
│ Fact Lifecycle: f8a3b                                        │
├─────────────────────────────────────────────────────────────┤
│ 2026-01-15 10:30  CREATED (source: session)                 │
│ 2026-01-20 14:00  ACCESSED (query: "编程语言", score: 0.89) │
│ 2026-02-01 09:00  ACCESSED (query: "Rust", score: 0.76)     │
│ Current: Active (strength: 0.92, accessed: 12 times)        │
└─────────────────────────────────────────────────────────────┘
```

### 12.5 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| Scratchpad 文件损坏 | 丢失当前任务状态 | 每次写入前备份到 `.scratchpad.md.bak` |
| Lazy Decay 误删重要记忆 | 用户信任下降 | 30 天回收站 + `personal` 类型永不衰减 |
| CLI 文件锁死锁 | CLI 无法使用 | 锁超时机制 (10s) + `--force` 选项 |
| 大量审计日志膨胀 | 磁盘空间 | 90 天自动轮转 + 可配置关闭 |

---

## 13. Summary (总结)

### 13.1 v3 核心升级

| 升级项 | 解决的问题 | 核心机制 |
|--------|-----------|---------|
| **Session Scratchpad** | 长任务中遗忘进度 | 项目本地 `.aleph/scratchpad.md` |
| **Lazy Decay** | CLI 短生命周期无法运行后台任务 | 读时即时计算 + 异步软删除 |
| **Hybrid Trigger** | 信号漏判导致 Token 爆炸 | Signal + Token 阈值双路触发 |
| **CLI Tools** | 用户无法干预记忆 | 直连 SQLite + 文件锁 |
| **Explainability** | 记忆决策不透明 | 审计日志 + explain 命令 |
| **Recycle Bin** | 误删不可恢复 | 软删除 + 30 天保留 |

### 13.2 设计原则回顾

1. **Glass Box**: 保持智能，但让用户能看透和干预
2. **控制反转**: v2 执行引擎不变，触发逻辑升级
3. **渐进迁移**: 并行运行 → 渐进切换 → 完全迁移
4. **复用优先**: FactExtractor、ConflictResolver 等核心组件保留

---

*文档生成时间: 2026-02-03*
*状态: Draft - 待实现*
