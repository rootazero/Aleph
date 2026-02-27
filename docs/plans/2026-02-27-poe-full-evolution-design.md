# POE 全面演进设计：事件驱动闭环

## 背景

POE 认知中枢的"核心先行"阶段已完成（22 个 Task，25 commits），建立了：

- **Interceptor 层**：StepDirective → StepEvaluator → PoeLoopCallback → AgentLoop 深度融合
- **PromptPipeline 注入**：PoePromptLayer (priority 505) 将 SuccessManifest 注入系统提示词
- **Meta-Cognition 迁移**：BehavioralAnchor、AnchorStore、ReactiveReflector、CriticAgent、ConflictDetector → `poe::meta_cognition`
- **Crystallization 迁移**：distillation、pattern_extractor、clustering、dreaming → `poe::crystallization`
- **PoeManager 升级**：MetaCognitionCallback trait (Send+Sync safe)
- **Cortex 标记废弃**

然而，核心先行只完成了结构迁移，多个关键闭环仍然断裂。本设计解决这些断裂点，让 POE 真正成为"越用越聪明"的认知系统。

## 问题分析：六个断裂点

| 断裂点 | 现状 | 影响 |
|--------|------|------|
| **Crystallizer 初始化** | 代码就位但从未实例化 | 经验永远不被记录 |
| **Memory 事件流** | POE 与 Memory 完全隔离 | 知识无法沉淀 |
| **Experience Replay** | 框架完整但搜索返回空 | 相似任务无法复用经验 |
| **Trust 自动审批** | 设计完毕但未连接 | 永远需要手动签署 |
| **Contract 持久化** | 纯内存 HashMap | 重启即丢失 |
| **Worker Snapshot** | 仅 verify 不回滚 | 失败后无法恢复工作区 |

## 架构方案：事件驱动闭环 (方案 A)

选择事件驱动而非直接接线的理由：

1. **DDD 天然契合** — CLAUDE.md 强调领域驱动设计，事件是领域建模的核心模式
2. **松耦合** — PoeManager 只发事件，不关心谁消费；新投影器不影响核心
3. **可追溯** — 事件是事实记录，支持重放和审计
4. **渐进式** — 投影器可独立开发部署，每个 Phase 独立可交付
5. **模式复用** — Memory 系统的 Skeleton/Pulse 分层、DaemonEventBus 的 broadcast 模式均可复用

```
┌──────────────────────────────────────────────────────────────────┐
│                    POE 事件驱动闭环                                │
│                                                                  │
│  PoeManager ──emit──→ PoeEventBus (broadcast)                    │
│                         │                                        │
│                         ├──→ CrystallizationProjector            │
│                         │     └─ 写入 LanceDB poe_experiences    │
│                         │                                        │
│                         ├──→ MemoryProjector                     │
│                         │     └─ 创建 Memory facts               │
│                         │                                        │
│                         ├──→ TrustProjector                      │
│                         │     └─ 更新 StateDB trust_scores       │
│                         │                                        │
│                         └──→ MetricsProjector                    │
│                               └─ 更新 skill_metrics              │
│                                                                  │
│  新任务到来:                                                      │
│    → ExperienceReplayLayer ←── LanceDB poe_experiences (向量搜索) │
│    → TrustEvaluator ←── StateDB trust_scores                     │
│    → PoePromptContext ←── 注入匹配经验 + 行为锚点                  │
└──────────────────────────────────────────────────────────────────┘
```

## 详细设计

### 1. POE 领域事件系统

#### 1.1 事件类型

复用 Memory 系统的 Skeleton/Pulse 分层模式：

```rust
// core/src/poe/events.rs

/// POE 领域事件 — 所有 POE 生命周期变化的事实记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PoeEvent {
    // --- Skeleton Events (立即持久化) ---

    /// 成功契约创建
    ManifestCreated {
        task_id: String,
        objective: String,
        hard_constraints_count: usize,
        soft_metrics_count: usize,
    },

    /// 契约签署（用户签署或自动审批）
    ContractSigned {
        task_id: String,
        auto_approved: bool,
        trust_score: Option<f32>,
    },

    /// 验证完成
    ValidationCompleted {
        task_id: String,
        attempt: u8,
        passed: bool,
        distance_score: f32,
        hard_passed: usize,
        hard_total: usize,
    },

    /// 最终结果
    OutcomeRecorded {
        task_id: String,
        outcome: PoeOutcomeKind,
        attempts: u8,
        total_tokens: u32,
        duration_ms: u64,
        best_distance: f32,
    },

    // --- Pulse Events (可缓冲) ---

    /// Worker 执行尝试
    OperationAttempted {
        task_id: String,
        attempt: u8,
        tokens_used: u32,
    },

    /// 信任分数更新
    TrustUpdated {
        pattern_id: String,
        old_score: f32,
        new_score: f32,
    },
}

/// 结果类型简化枚举（用于事件序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PoeOutcomeKind {
    Success,
    StrategySwitch,
    BudgetExhausted,
}
```

#### 1.2 事件信封

```rust
/// POE 事件信封 — 包装领域事件 + 元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeEventEnvelope {
    pub id: i64,               // 自增 ID
    pub task_id: String,       // POE 任务 ID
    pub seq: u32,              // 任务内单调递增序号
    pub event: PoeEvent,       // 领域事件
    pub tier: EventTier,       // Skeleton | Pulse
    pub timestamp: i64,        // Unix 毫秒
    pub correlation_id: Option<String>, // 关联 session_id
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EventTier {
    Skeleton,
    Pulse,
}
```

#### 1.3 事件总线

```rust
// core/src/poe/event_bus.rs

use tokio::sync::broadcast;

pub struct PoeEventBus {
    sender: broadcast::Sender<PoeEventEnvelope>,
}

impl PoeEventBus {
    pub fn new(capacity: usize) -> Self { ... }
    pub fn emit(&self, envelope: PoeEventEnvelope) { ... }
    pub fn subscribe(&self) -> broadcast::Receiver<PoeEventEnvelope> { ... }
}
```

**容量**：默认 1024，与 DaemonEventBus 一致。

#### 1.4 事件存储表

StateDatabase 新增 `poe_events` 表：

```sql
CREATE TABLE IF NOT EXISTS poe_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    event_json TEXT NOT NULL,
    tier TEXT NOT NULL CHECK(tier IN ('skeleton', 'pulse')),
    timestamp INTEGER NOT NULL,
    correlation_id TEXT,
    UNIQUE(task_id, seq)
);

CREATE INDEX idx_pe_task_id ON poe_events(task_id);
CREATE INDEX idx_pe_event_type ON poe_events(event_type);
CREATE INDEX idx_pe_timestamp ON poe_events(timestamp);
```

### 2. Crystallizer 接通

#### 2.1 Gateway 初始化

```rust
// PoeRunManager 构造时接收 recorder
impl PoeRunManager {
    pub fn new(
        event_bus: Arc<GatewayEventBus>,
        poe_event_bus: Arc<PoeEventBus>,
        recorder: Arc<dyn ExperienceRecorder>,
        // ...
    ) -> Self { ... }
}
```

Gateway 启动时：
1. 创建 `ChannelCrystallizer` + spawn `CrystallizerWorker`
2. 将 `ChannelCrystallizer` (as `Arc<dyn ExperienceRecorder>`) 传给 `PoeRunManager`
3. `PoeRunManager` 创建 `PoeManager` 时传入 recorder

#### 2.2 LanceDB 经验存储

新增 `poe_experiences` 表：

| 列名 | 类型 | 说明 |
|------|------|------|
| id | String | 经验 ID (UUID) |
| task_id | String | 源 POE 任务 ID |
| objective | String | 任务目标 (原文) |
| pattern_id | String | 模式标识 (关键词提取) |
| tool_sequence_json | String | 工具调用序列 |
| parameter_mapping | String? | 参数映射 JSON |
| satisfaction | f32 | 满意度 (0.0-1.0) |
| distance_score | f32 | 最终距离分数 |
| attempts | u8 | 尝试次数 |
| duration_ms | u64 | 总耗时 |
| embedding | Vector | 目标文本的嵌入向量 |
| created_at | i64 | 创建时间 |

#### 2.3 CrystallizationProjector

消费 `PoeOutcomeRecorded` 事件 → 写入 `poe_experiences`：

```rust
impl CrystallizationProjector {
    async fn handle(&self, event: &PoeEventEnvelope) {
        if let PoeEvent::OutcomeRecorded { task_id, outcome, .. } = &event.event {
            let objective = self.get_objective(task_id).await;
            let embedding = self.embedder.embed(&objective).await?;
            self.experience_store.insert(PoeExperience {
                id: uuid(),
                task_id: task_id.clone(),
                objective,
                embedding,
                satisfaction: outcome.to_satisfaction(),
                // ...
            }).await?;
        }
    }
}
```

### 3. Experience Replay 接通

#### 3.1 ExperienceReplayLayer 改造

```rust
impl ExperienceReplayLayer {
    pub async fn try_match(&self, intent: &str) -> Result<Option<ReplayMatch>> {
        let intent_vector = self.embedder.embed(intent).await?;

        // 查询 LanceDB poe_experiences 表
        let candidates = self.experience_store
            .vector_search(
                &intent_vector,
                self.config.max_candidates,
                self.config.similarity_threshold,
            )
            .await?;

        if candidates.is_empty() {
            return Ok(None);
        }

        let best = self.select_best_match(intent, candidates).await?;
        let filled = self.fill_parameters(intent, &best).await?;

        Ok(Some(ReplayMatch {
            experience_id: best.id,
            tool_sequence: filled,
            confidence: best.similarity_score,
        }))
    }
}
```

#### 3.2 ExperienceStore trait

```rust
/// 经验存储抽象 — LanceDB 实现
#[async_trait]
pub trait ExperienceStore: Send + Sync {
    async fn insert(&self, experience: PoeExperience) -> Result<()>;
    async fn vector_search(
        &self,
        query: &[f32],
        limit: usize,
        threshold: f64,
    ) -> Result<Vec<(PoeExperience, f64)>>;
    async fn get_by_pattern_id(&self, pattern_id: &str) -> Result<Vec<PoeExperience>>;
}
```

### 4. Trust 自动审批接通

#### 4.1 信任分数表

StateDatabase 新增 `poe_trust_scores` 表：

```sql
CREATE TABLE IF NOT EXISTS poe_trust_scores (
    pattern_id TEXT PRIMARY KEY,
    total_executions INTEGER NOT NULL DEFAULT 0,
    successful_executions INTEGER NOT NULL DEFAULT 0,
    trust_score REAL NOT NULL DEFAULT 0.0,
    last_updated INTEGER NOT NULL
);
```

#### 4.2 信任计算

```
trust_score = successful / total * decay_factor
decay_factor = 1.0 - (days_since_last_success * 0.01)  // 随时间衰减

自动审批阈值：
  - score >= 0.8 且 total >= 5 → AutoApprove
  - score >= 0.6 且 total >= 3 → SuggestApprove (需确认)
  - 其他 → RequireSignature
```

#### 4.3 ContractService 集成

```rust
impl PoeContractService {
    pub async fn prepare(&self, params: PrepareParams) -> Result<PrepareResult> {
        let manifest = self.manifest_builder.build(&params).await?;
        let pattern_id = extract_pattern_id(&manifest.objective);

        // 查询信任度
        let trust_decision = self.trust_evaluator
            .evaluate(&TrustContext {
                pattern_id: &pattern_id,
                manifest: &manifest,
            })
            .await;

        match trust_decision {
            AutoApprovalDecision::Approve { reason } => {
                // 自动签署，直接执行
                self.execute_directly(manifest).await
            }
            AutoApprovalDecision::SuggestApprove { reason } => {
                // 提示用户建议自动审批
                self.store_pending_with_suggestion(manifest, reason).await
            }
            AutoApprovalDecision::RequireSignature => {
                // 正常流程，等待用户签署
                self.store_pending(manifest).await
            }
        }
    }
}
```

### 5. Memory 事件流集成

#### 5.1 MemoryProjector

消费 `PoeOutcomeRecorded` → 创建 Memory facts：

```rust
impl MemoryProjector {
    async fn handle(&self, event: &PoeEventEnvelope) {
        if let PoeEvent::OutcomeRecorded { task_id, outcome, .. } = &event.event {
            let objective = self.get_objective(task_id).await;

            match outcome {
                PoeOutcomeKind::Success => {
                    // 成功经验 → core tier fact
                    self.memory_store.upsert_fact(MemoryFact {
                        content: format!("Successfully completed: {}", objective),
                        category: "poe_experience".into(),
                        tier: FactTier::Core,
                        metadata: json!({
                            "task_id": task_id,
                            "outcome": "success",
                        }),
                        ..Default::default()
                    }).await?;
                }
                PoeOutcomeKind::BudgetExhausted => {
                    // 失败教训 → working tier fact
                    self.memory_store.upsert_fact(MemoryFact {
                        content: format!("Failed task (budget exhausted): {}", objective),
                        category: "lessons_learned".into(),
                        tier: FactTier::Working,
                        ..Default::default()
                    }).await?;
                }
                _ => {}
            }
        }
    }
}
```

### 6. Contract 持久化

#### 6.1 poe_contracts 表

```sql
CREATE TABLE IF NOT EXISTS poe_contracts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'signed', 'rejected', 'expired')),
    created_at INTEGER NOT NULL,
    signed_at INTEGER,
    expires_at INTEGER,
    amendments_json TEXT
);

CREATE INDEX idx_pc_status ON poe_contracts(status);
CREATE INDEX idx_pc_task_id ON poe_contracts(task_id);
```

#### 6.2 PendingContractStore 改造

```rust
/// 从 HashMap 迁移到 StateDatabase 持久化
impl PendingContractStore {
    pub fn new(db: Arc<StateDatabase>) -> Self { ... }

    pub async fn store(&self, contract: PendingContract) -> Result<()> {
        self.db.insert_poe_contract(&contract)?;
        Ok(())
    }

    pub async fn get(&self, task_id: &str) -> Result<Option<PendingContract>> {
        self.db.get_poe_contract(task_id)
    }

    pub async fn remove(&self, task_id: &str) -> Result<()> {
        self.db.update_poe_contract_status(task_id, "signed")?;
        Ok(())
    }

    pub async fn cleanup_expired(&self) -> Result<usize> {
        self.db.delete_expired_contracts(chrono::Utc::now().timestamp_millis())
    }
}
```

### 7. Worker Snapshot (Git-based)

#### 7.1 Git Snapshot 实现

```rust
impl StateSnapshot {
    /// 捕获当前工作区状态
    pub async fn capture(workspace: &Path) -> Result<Self> {
        // 创建 git stash object（不修改工作区）
        let stash_ref = Command::new("git")
            .args(["stash", "create", "--include-untracked"])
            .current_dir(workspace)
            .output()
            .await?;

        let stash_hash = String::from_utf8(stash_ref.stdout)?.trim().to_string();

        Ok(Self {
            workspace: workspace.to_path_buf(),
            stash_hash: if stash_hash.is_empty() { None } else { Some(stash_hash) },
            captured_at: Instant::now(),
        })
    }

    /// 回滚到捕获时的状态
    pub async fn restore(&self) -> Result<()> {
        if let Some(ref hash) = self.stash_hash {
            // 清除当前工作区变更
            Command::new("git")
                .args(["checkout", "--", "."])
                .current_dir(&self.workspace)
                .output()
                .await?;

            // 恢复到 stash 状态
            Command::new("git")
                .args(["stash", "apply", hash])
                .current_dir(&self.workspace)
                .output()
                .await?;
        }
        Ok(())
    }
}
```

#### 7.2 PoeManager 集成

```rust
// 在 PoeManager::execute() 的 Operation 阶段前：
let snapshot = StateSnapshot::capture(&self.workspace).await.ok();

// 如果验证失败需要重试：
if let Some(ref snap) = snapshot {
    snap.restore().await?;  // 回滚工作区
}
```

### 8. Dispatcher 工具推荐

不改变工具集合，通过 hint 引导 LLM：

```rust
// 在 PoeRunManager 启动任务前：
if let Some(replay) = self.experience_replay.try_match(&objective).await? {
    let hint = format!(
        "Similar task completed successfully before (confidence: {:.0}%). \
         Previous approach used these tools: {}",
        replay.confidence * 100.0,
        replay.tool_sequence,
    );
    poe_context.current_hint = Some(hint);
}
```

## 分阶段路线图

### Phase 1: 闭环建立 (事件基础 + Crystallizer 接通)

**目标**：POE 执行 → 事件发布 → 经验存储

| Task | 内容 | 文件 |
|------|------|------|
| 1 | 定义 PoeEvent 类型和 PoeEventEnvelope | `poe/events.rs` |
| 2 | 实现 PoeEventBus (broadcast) | `poe/event_bus.rs` |
| 3 | StateDatabase 新增 poe_events 表 + CRUD | `resilience/database/` |
| 4 | Gateway 初始化 Crystallizer | `gateway/handlers/poe.rs`, `poe/services/` |
| 5 | PoeRunManager 接线 recorder + event_bus | `poe/services/run_service.rs` |
| 6 | LanceDB poe_experiences 表 + CrystallizationProjector | `memory/store/lance/`, `poe/projectors/` |

### Phase 2: 学习反馈 (Experience Replay + Trust)

**目标**：相似任务匹配 + 渐进式自动审批

| Task | 内容 | 文件 |
|------|------|------|
| 7 | ExperienceStore trait + LanceDB 实现 | `poe/crystallization/experience_store.rs` |
| 8 | ExperienceReplayLayer 接通 LanceDB | `dispatcher/experience_replay_layer.rs` |
| 9 | StateDatabase trust_scores 表 + TrustProjector | `resilience/database/`, `poe/projectors/` |
| 10 | TrustEvaluator 接入 ContractService | `poe/services/contract_service.rs` |
| 11 | Contract 持久化 (poe_contracts 表) | `poe/contract_store.rs`, `resilience/database/` |

### Phase 3: 深度集成 (Memory + Dispatcher + Snapshot)

**目标**：POE ↔ Memory 双向 + 工具推荐 + 回滚

| Task | 内容 | 文件 |
|------|------|------|
| 12 | MemoryProjector 实现 | `poe/projectors/memory.rs` |
| 13 | Memory facts 写入测试 | `poe/projectors/memory.rs` (tests) |
| 14 | Dispatcher hint injection | `poe/services/run_service.rs` |
| 15 | Git-based StateSnapshot capture/restore | `poe/worker/` |
| 16 | PoeManager snapshot 集成 | `poe/manager.rs` |

### Phase 4: 验证与收尾

**目标**：全部通过

| Task | 内容 | 文件 |
|------|------|------|
| 17 | 集成测试：闭环 round-trip | `tests/poe_event_loop_integration.rs` |
| 18 | 集成测试：回归验证 | `tests/poe_*.rs` |
| 19 | 全量构建验证 | `cargo test` |

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| rusqlite !Send 在事件持久化中的线程安全 | 使用 spawn_blocking + channel，与 CrystallizerWorker 同模式 |
| LanceDB schema 变更影响现有数据 | 新建 poe_experiences 表，不修改现有 facts 表 |
| 事件总线反压（生产者快于消费者） | broadcast 有自动丢弃旧消息机制；Pulse 事件可丢失，Skeleton 通过持久化保证 |
| Experience Replay 误匹配导致错误建议 | 高阈值 (0.85) + 仅作为 hint 注入（不改变工具集）+ 人工签署保底 |
| Git stash 操作失败（非 git 仓库） | 检查 git 可用性，fallback 到 verify-only |

## 技术约束

- 不引入新的重量级依赖（复用 tokio broadcast、rusqlite、lancedb）
- 不修改 PoeManager 核心 P→O→E 循环（只在入口/出口添加事件发射）
- 不破坏现有 Gateway RPC 协议（只新增，不修改）
- 所有新表采用 migration 模式（StateDatabase::initialize 时自动创建）
