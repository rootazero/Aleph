# POE Architecture Design

> **Principle-Operation-Evaluation**: 让 Aleph 从"碰运气的聊天机器人"进化为"目标导向的专业 Agent"

**Date**: 2026-02-01
**Status**: Draft
**Author**: Ziv + Claude

---

## 1. 背景与动机

### 问题陈述

现有 AI Agent（包括 Moltbot 等）的本质是 ReAct 循环的简单封装：

```
User Input → LLM (思考) → Tool (执行) → Result → LLM (再思考)
```

这种模式的弱点：
- **过度依赖 LLM 的临场发挥** — 如果 LLM 在某一步"走神"或陷入死循环，整个任务失败
- **缺乏"直觉"或"经验规则"** — 没有宏观方向指导
- **LLM 既是运动员又是裁判** — 容易自己骗自己说"我做好了"
- **无限试错风险** — 没有熵减预算控制

### 核心思想

引入 **System 1 (启发式) + System 2 (LLM)** 的双系统架构：

| 系统 | 特点 | 在 POE 中的角色 |
|------|------|----------------|
| System 1 | 快速、直觉、经验驱动 | 启发式规则、经验检索、熵减检测 |
| System 2 | 慢速、逻辑、推理驱动 | LLM 思考、语义校验、契约生成 |

### 设计原则

1. **先定义成功，再开始执行** — 第一性原理锚定
2. **结果导向校验** — 独立的 Critic 层，不信任 Worker 的自我评估
3. **熵减预算控制** — 防止无限试错，及时止损
4. **经验沉淀** — 成功模式结晶为可复用知识
5. **物理隔离** — POE Manager 站在高维视角，Worker 只是执行者

---

## 2. 架构概览

### 2.1 模块结构

```
core/src/poe/
├── mod.rs              # 模块入口
├── types.rs            # 核心数据结构
├── manifest.rs         # SuccessManifest 构建与解析
├── validation/
│   ├── mod.rs
│   ├── hard.rs         # 硬性校验 (文件、命令、Schema)
│   ├── semantic.rs     # LlmJudge 语义校验
│   └── composite.rs    # 混合校验编排
├── manager.rs          # POE 控制塔核心逻辑
├── budget.rs           # 熵减预算管理
├── crystallizer.rs     # 成功经验沉淀
└── worker.rs           # Worker 抽象层
```

### 2.2 核心职责分离

| 组件 | 职责 | 与现有代码的关系 |
|------|------|-----------------|
| `PoeManager` | 编排 P-O-E 循环 | 调用 Worker (现有 AgentLoop) |
| `ManifestBuilder` | 在任务开始前生成成功契约 | 可复用 Thinker 的 LLM 调用 |
| `CompositeValidator` | 混合执行硬性+语义校验 | 复用 spec_driven/LlmJudge |
| `BudgetManager` | 追踪尝试次数、Token 消耗 | 与 Guards 互补但独立 |
| `Crystallizer` | 成功后沉淀为技能/经验 | 调用 skill_evolution |

### 2.3 POE 循环流程

```
┌─────────────────────────────────────────────────────────────┐
│                      POE Manager                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ P - Principle (第一性原理锚定)                        │   │
│  │   ManifestBuilder.build(instruction)                 │   │
│  │   → SuccessManifest { objective, constraints, ... }  │   │
│  └─────────────────────────────────────────────────────┘   │
│                           ↓                                  │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ O - Operation (启发式探索)                            │   │
│  │   Worker.execute(instruction, previous_failure)      │   │
│  │   ← 经验库检索相似解决方案                            │   │
│  └─────────────────────────────────────────────────────┘   │
│                           ↓                                  │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ E - Evaluation (结果导向校验)                         │   │
│  │   CompositeValidator.validate(manifest, output)      │   │
│  │   → Verdict { passed, distance_score, suggestion }   │   │
│  └─────────────────────────────────────────────────────┘   │
│                           ↓                                  │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ 决策分支                                              │   │
│  │   passed=true  → Crystallizer.record() → Success     │   │
│  │   is_stuck()   → StrategySwitch                      │   │
│  │   exhausted()  → BudgetExhausted                     │   │
│  │   otherwise    → 注入失败反馈 → 重试 O 阶段           │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. 核心数据结构

### 3.1 SuccessManifest (成功契约)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessManifest {
    /// 任务唯一标识
    pub task_id: String,

    /// 第一性原理陈述 — "这个任务的本质是什么"
    pub objective: String,

    /// 硬性约束 (AND 逻辑，全部通过才算成功)
    pub hard_constraints: Vec<ValidationRule>,

    /// 软性指标 (加权评分，用于优化方向)
    pub soft_metrics: Vec<SoftMetric>,

    /// 回滚快照路径 (失败时恢复)
    pub rollback_snapshot: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoftMetric {
    pub rule: ValidationRule,
    pub weight: f32,        // 0.0 - 1.0
    pub threshold: f32,     // 及格线，如 0.8
}
```

### 3.2 ValidationRule (校验规则)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum ValidationRule {
    // --- 文件系统层 ---
    FileExists { path: PathBuf },
    FileNotExists { path: PathBuf },
    FileContains { path: PathBuf, pattern: String },
    DirStructureMatch { root: PathBuf, expected: String },

    // --- 执行层 ---
    CommandPasses { cmd: String, args: Vec<String>, timeout_ms: u64 },
    CommandOutputContains { cmd: String, args: Vec<String>, pattern: String },

    // --- 数据层 ---
    JsonSchemaValid { path: PathBuf, schema: String },

    // --- 语义层 (LLM Judge) ---
    SemanticCheck {
        target: JudgeTarget,
        prompt: String,
        passing_criteria: String,
        model_tier: ModelTier,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JudgeTarget {
    File(PathBuf),
    Content(String),
    CommandOutput { cmd: String, args: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelTier {
    LocalFast,   // Llama 3 8B / 本地小模型
    CloudFast,   // GPT-4o-mini / Claude Haiku
    CloudSmart,  // GPT-4o / Claude Sonnet
    CloudDeep,   // o1 / Claude Opus (extended thinking)
}
```

### 3.3 PoeBudget (熵减预算)

```rust
#[derive(Debug, Clone)]
pub struct PoeBudget {
    pub max_attempts: u8,
    pub current_attempt: u8,
    pub max_tokens: u32,
    pub tokens_used: u32,
    pub entropy_history: Vec<f32>,  // 每次尝试后的"距离目标"评分
}

impl PoeBudget {
    /// 熵减检测：如果连续 N 次尝试没有进展，触发策略切换
    pub fn is_stuck(&self, window: usize) -> bool {
        if self.entropy_history.len() < window {
            return false;
        }
        let recent: Vec<_> = self.entropy_history.iter().rev().take(window).collect();
        recent.windows(2).all(|w| w[0] <= w[1])
    }

    pub fn exhausted(&self) -> bool {
        self.current_attempt >= self.max_attempts || self.tokens_used >= self.max_tokens
    }
}
```

### 3.4 PoeOutcome (执行结果)

```rust
#[derive(Debug)]
pub enum PoeOutcome {
    Success(FinalVerdict),
    StrategySwitch(String),   // 需要换方向
    BudgetExhausted(String),  // 预算耗尽，人工介入
}
```

---

## 4. 混合校验引擎

### 4.1 校验流水线

```
┌─────────────────────────────────────────────────────────┐
│                 CompositeValidator                       │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  Phase 1: Hard Validation (Rust, 确定性)                │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐                   │
│  │FileExist│→│CmdPasses│→│ Schema  │  任一失败 → 返回  │
│  └─────────┘ └─────────┘ └─────────┘                   │
│       ↓ (全部通过)                                       │
│                                                          │
│  Phase 2: Semantic Validation (LLM, 并行)               │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐                   │
│  │ Judge 1 │ │ Judge 2 │ │ Judge 3 │  (不同 ModelTier) │
│  └─────────┘ └─────────┘ └─────────┘                   │
│       ↓                                                  │
│  Phase 3: Weighted Scoring → distance_score             │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### 4.2 设计要点

1. **快速失败** — 硬性校验先行，失败立即返回，节省 LLM Token
2. **语义并行** — 多个 LLM Judge 并行执行，降低总延迟
3. **加权评分** — `distance_score` 作为熵值反馈给 Budget 追踪
4. **建议聚合** — 失败时汇总所有建议，提供给下次尝试

---

## 5. Worker 抽象层

### 5.1 Worker Trait

```rust
#[async_trait]
pub trait Worker: Send + Sync {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput>;

    async fn abort(&self) -> Result<()>;
    async fn snapshot(&self) -> Result<StateSnapshot>;
    async fn restore(&self, snapshot: &StateSnapshot) -> Result<()>;
}
```

### 5.2 Worker 类型矩阵

| Worker 类型 | 适用场景 | 特点 |
|-------------|---------|------|
| `AgentLoopWorker` | 标准任务 | 直接复用现有 agent_loop |
| `ClaudeCodeWorker` | 复杂编码任务 | PTY 监控，更强的代码能力 |
| `SubAgentWorker` | 并行子任务 | 隔离的 session，可指定不同 model |

---

## 6. 经验结晶器

### 6.1 Experience 数据结构

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    pub task_pattern: TaskPattern,       // 任务模式（可匹配相似任务）
    pub solution_path: SolutionPath,     // 解决路径
    pub outcome: ExperienceOutcome,      // 结果评估
    pub created_at: DateTime<Utc>,
    pub usage_count: u32,                // 被复用次数
    pub success_rate: f32,               // 复用成功率
}
```

### 6.2 与 skill_evolution 的集成

| 阶段 | 触发条件 | 动作 |
|------|---------|------|
| 经验积累 | 每次成功 | 存入向量数据库 |
| 模式检测 | ≥3 个相似成功 | 标记为"候选技能" |
| 技能固化 | ≥5 个相似成功 + 高成功率 | 生成 `SKILL.md` 并提交 |

---

## 7. 契约生成器

### 7.1 生成流程

```
用户指令
    ↓
Step 1: 分类任务类型 (TaskType)
    ↓
Step 2: 获取预定义模板
    ↓
Step 3: 检索相似经验
    ↓
Step 4: LLM 生成契约
    ↓
Step 5: 验证契约合理性
    ↓
SuccessManifest
```

### 7.2 预定义模板

- **BugFix**: 默认需要 `cargo test` 通过
- **CodeRefactor**: 默认需要 `cargo build` + `cargo clippy` 通过
- **FeatureImplementation**: 需要新功能的测试用例通过
- **FileOrganization**: 需要目录结构匹配预期
- **DocumentGeneration**: 需要语义校验文档质量

---

## 8. Gateway 集成

### 8.1 RPC 方法

| 方法 | 描述 |
|------|------|
| `poe.manifest.build` | 预览生成的契约 |
| `poe.execute` | 使用指定契约执行 |
| `poe.execute_auto` | 自动生成契约并执行 |
| `poe.experiences.search` | 查询经验库 |
| `poe.status` | 获取任务状态 |
| `poe.abort` | 中断任务 |

### 8.2 与现有 `agent.run` 的关系

```
┌─────────────────────────────────────────────────────────────┐
│                     客户端调用选择                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  agent.run          │  poe.execute_auto    │  poe.execute   │
│  (现有行为)          │  (自动契约)           │  (指定契约)    │
│         ↓           │          ↓            │        ↓       │
│  ┌──────────┐       │  ┌────────────┐       │  ┌──────────┐ │
│  │AgentLoop │       │  │ Manifest   │       │  │ 用户定义 │ │
│  │ 直接执行 │       │  │ Builder    │       │  │  契约    │ │
│  └──────────┘       │  └─────┬──────┘       │  └────┬─────┘ │
│                      │        ↓              │        ↓      │
│                      │  ┌─────────────────────────────────┐ │
│                      │  │         POE Manager             │ │
│                      │  │  P → O → E 循环                 │ │
│                      │  │  Worker = AgentLoop             │ │
│                      │  └─────────────────────────────────┘ │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 8.3 渐进式启用策略

| 阶段 | 配置 | 行为 |
|------|------|------|
| 阶段 0 | `enabled: false` | 完全使用现有 `agent.run` |
| 阶段 1 | `enabled: true, require_manifest_approval: true` | 自动生成契约，用户确认后执行 |
| 阶段 2 | `enabled: true, require_manifest_approval: false` | 全自动 POE 模式 |

### 8.4 配置选项

```rust
pub struct PoeConfig {
    pub enabled: bool,                      // 是否启用 POE 模式
    pub default_max_attempts: u8,           // 默认最大尝试次数
    pub default_max_tokens: u32,            // 默认 Token 预算
    pub stuck_detection_window: usize,      // 熵减检测窗口
    pub require_manifest_approval: bool,    // 是否需要用户确认契约
    pub preferred_worker: WorkerType,       // Worker 类型偏好
    pub experience_db_path: PathBuf,        // 经验库路径
}
```

---

## 9. 事件流

```rust
#[derive(Debug, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum PoeEvent {
    ManifestGenerated { manifest: SuccessManifest },
    AttemptStarted { attempt: u8, max_attempts: u8 },
    WorkerProgress { step: StepLog },
    ValidationStarted { phase: String },
    ValidationResult { verdict: Verdict },
    EntropyUpdate { distance_score: f32, trend: String },
    StrategySwitch { reason: String, suggestion: String },
    ExperienceCrystallized { experience_id: String },
    Completed { outcome: PoeOutcome },
}
```

---

## 10. 实施计划

### Phase 1: 基础设施 (Week 1)

- [ ] 创建 `core/src/poe/` 模块结构
- [ ] 实现 `types.rs` 核心数据结构
- [ ] 实现 `budget.rs` 熵减预算管理

### Phase 2: 校验引擎 (Week 2)

- [ ] 实现 `validation/hard.rs` 硬性校验
- [ ] 实现 `validation/semantic.rs` 语义校验 (复用 spec_driven)
- [ ] 实现 `validation/composite.rs` 混合编排

### Phase 3: 核心循环 (Week 3)

- [ ] 实现 `worker.rs` Worker 抽象层
- [ ] 实现 `AgentLoopWorker` 适配器
- [ ] 实现 `manager.rs` POE 控制塔

### Phase 4: 智能层 (Week 4)

- [ ] 实现 `manifest.rs` 契约生成器
- [ ] 实现 `crystallizer.rs` 经验结晶器
- [ ] 集成向量数据库存储经验

### Phase 5: 集成 (Week 5)

- [ ] 注册 Gateway RPC 方法
- [ ] 实现事件流
- [ ] 添加配置选项
- [ ] 编写集成测试

---

## 11. 预期收益

| 指标 | 现状 | POE 后预期 |
|------|------|-----------|
| 任务成功率 | ~60% | ~90% |
| 无效重试 | 常见 | 熵减检测后及时止损 |
| Token 浪费 | 高 | 硬性校验快速失败节省 |
| 经验复用 | 无 | 相似任务自动借鉴 |
| 可审计性 | 低 | 完整的契约+校验记录 |

---

## 12. 风险与缓解

| 风险 | 缓解措施 |
|------|---------|
| 契约生成不准确 | 支持用户审阅和修改；积累模板库 |
| 语义校验不稳定 | 使用 temperature=0；多次判定取多数 |
| Worker 回滚失败 | 使用 Git 作为回滚后端；定期快照 |
| 经验库膨胀 | 定期清理低使用率/低成功率经验 |
