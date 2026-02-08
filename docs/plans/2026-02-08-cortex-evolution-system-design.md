# Aleph Cortex 进化系统设计

> 从"智能助手"到"自我进化智能体"的架构蓝图

**设计日期**: 2026-02-08
**目标**: 3-6个月 MVP - 记忆进化（Cortex）
**状态**: Design Complete

---

## 第一部分：架构概览与核心理念

### 核心理念

Aleph Cortex 是一个"数据→信息→知识→智慧"的压缩与进化系统，通过将高价值的任务执行经验（task_traces）蒸馏为可复用的技能模板（experience_replays），实现从"无状态执行者"到"自我进化智能体"的跃迁。

### 四大进化机制（3-12个月路线图）

1. **记忆进化（Cortex）** - 3-6个月 MVP：将经历转化为经验，实现"越用越快"
2. **元认知层** - 6-9个月：自我博弈与 Prompt 自动调优，实现"越用越聪明"
3. **主动探索** - 9-12个月：好奇心驱动的后台学习，实现"主动成长"
4. **自我编码** - 12个月+：递归自我改进，实现"自主进化"

### MVP 阶段目标（3-6个月）

- 建立 Experience Replay Buffer（经验回放池）
- 实现混合蒸馏策略（实时+批量）
- 部署 L1.5 动态路由增强层（Soft Skills）
- 验证"经验复用"的价值（Token 节省、延迟降低）

### 关键创新

- **职责分离**：task_traces（日志）vs experience_replays（教材）
- **渐进式硬化**：Soft Skills（参数化重放）→ Hard Skills（代码生成）
- **生物启发**：实时学习（Trauma-based）+ 睡眠巩固（Dreaming）

---

## 第二部分：数据模型设计

### experience_replays 表结构

```sql
CREATE TABLE experience_replays (
    -- 主键与索引
    id TEXT PRIMARY KEY,
    pattern_hash TEXT NOT NULL,        -- 工具序列结构哈希（用于去重和聚类）
    intent_vector BLOB,                -- 用户意图的嵌入向量（384维，bge-small-zh-v1.5）

    -- 核心上下文快照
    user_intent TEXT NOT NULL,         -- 原始用户意图
    environment_context_json TEXT,     -- 执行环境（平台、权限、工作目录等）
    thought_trace_distilled TEXT,      -- 精简后的思考逻辑关键节点
    tool_sequence_json TEXT NOT NULL,  -- 工具调用链（含参数模板）
    parameter_mapping TEXT,            -- 参数化映射（变量名→提取规则）
    logic_trace_json TEXT,             -- 数据流依赖关系（为代码生成预留）

    -- 评价指标
    success_score REAL NOT NULL,       -- 综合评分（0.0-1.0）
    token_efficiency REAL,             -- Token消耗/产出比
    latency_ms INTEGER,                -- 执行耗时（毫秒）
    novelty_score REAL,                -- 结构化新颖度（0.0-1.0）

    -- 进化状态与统计
    evolution_status TEXT NOT NULL,    -- candidate/verified/distilled/archived
    usage_count INTEGER DEFAULT 1,     -- 被重用次数
    success_count INTEGER DEFAULT 0,   -- 重用成功次数
    last_success_rate REAL,            -- 最近成功率（滑动窗口）

    -- 时间戳
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    last_evaluated_at INTEGER,

    -- 索引
    UNIQUE(pattern_hash, user_intent)  -- 防止完全重复的经验
);

CREATE INDEX idx_pattern_hash ON experience_replays(pattern_hash);
CREATE INDEX idx_evolution_status ON experience_replays(evolution_status);
CREATE INDEX idx_last_used_at ON experience_replays(last_used_at);
```

### 生命周期状态机

```
Candidate（候选）
  ↓ [验证成功 3+ 次]
Verified（已验证）
  ↓ [usage_count > 20 且 success_rate > 95%]
Distilled（已蒸馏为代码）
  ↓ [30天未使用 或 success_rate < 70%]
Archived（已归档/衰减）
```

### 与现有系统的关系

- **task_traces**：原始执行日志，所有任务都记录
- **experience_replays**：精选教材，只有高价值经验才进入
- **memories/facts**：长期知识存储，偏向事实性信息
- **daily_insights**：每日总结，偏向宏观洞察

### 数据流向

```
task_traces → [蒸馏] → experience_replays → [硬化] → 生成的工具/插件
```

---

## 第三部分：蒸馏管道设计

### 混合触发策略（Hybrid Distillation Strategy）

#### 实时触发条件（立即评估，使用 Sonnet 模型）

1. **用户显式反馈**：点赞、确认、标记为"有用"
2. **高价值任务**：执行时间 > 30秒
3. **复杂任务**：工具调用链长度 > 5步
4. **高风险任务**：涉及 exec 审批通过的操作
5. **纠错场景**（新增）：任务路径中包含 Error Recovery（工具A失败→思考→工具B成功）

#### 批量触发条件（每日凌晨2-4点，使用 Haiku 模型）

1. 扫描过去24小时的所有 task_traces
2. 使用 ValueEstimator 初筛（success_score > 0.7）
3. 对初筛通过的 trace 调用 LLM 进行深度评估
4. 执行 Information Decay（LRU-Value 复合衰减）

### 评分标准（多维度加权）

```rust
final_score = 0.4 * success_rate          // 任务成功率
            + 0.3 * token_efficiency      // Token消耗/产出比
            + 0.2 * user_feedback         // 显式反馈（0-1）
            + 0.1 * novelty_score         // 结构化新颖度

// 结构化新颖度计算
novelty_score = min(1.0, levenshtein_distance(current_pattern, existing_patterns) / max_distance)
```

### 蒸馏流程（DistillationService）

```rust
pub struct DistillationTask {
    trace_id: String,
    mode: DistillationMode,  // RealTime | Batch
}

impl DistillationService {
    pub async fn distill(&self, task: DistillationTask) -> Result<Option<Experience>> {
        // Step 1: 预处理 - 剔除冗余信息
        let cleaned_trace = self.pre_process(task.trace_id).await?;

        // Step 2: 评估 - 计算综合评分
        let score = self.evaluate(&cleaned_trace).await?;
        if score < MIN_THRESHOLD && task.mode == Batch {
            return Ok(None);  // 批量模式下，低分直接丢弃
        }

        // Step 3: 总结 - 生成经验元数据
        // 调用 LLM 将几千字的 Trace 压缩为几十字的 pattern_description
        let experience = self.summarize(cleaned_trace).await?;

        // Step 4: 入库 - 写入 experience_replays 并更新向量索引
        self.commit(experience).await?;

        Ok(Some(experience))
    }

    // 预处理：剔除冗余信息
    async fn pre_process(&self, trace_id: &str) -> Result<CleanedTrace> {
        // 1. 移除文件读写的中间内容（只保留路径和操作类型）
        // 2. 合并连续的相同操作（如多次 ls）
        // 3. 提取关键决策点（Error → Retry → Success）
    }

    // 评估：计算综合评分
    async fn evaluate(&self, trace: &CleanedTrace) -> Result<f64> {
        let success_rate = trace.final_status.is_success() as f64;
        let token_efficiency = trace.output_value / trace.token_cost;
        let user_feedback = trace.user_feedback.unwrap_or(0.5);
        let novelty = self.calculate_novelty(&trace.pattern_hash).await?;

        Ok(0.4 * success_rate + 0.3 * token_efficiency + 0.2 * user_feedback + 0.1 * novelty)
    }

    // 总结：生成经验元数据
    async fn summarize(&self, trace: CleanedTrace) -> Result<Experience> {
        // 调用 LLM（Haiku for batch, Sonnet for realtime）
        // Prompt: "将以下任务执行过程压缩为可复用的模板，提取参数变量"
    }
}
```

### Information Decay（信息衰减机制）

```rust
// 在每日 Dreaming 批量评估时执行
async fn decay_obsolete_experiences(&self) -> Result<()> {
    // LRU-Value 复合衰减规则
    let candidates = self.db.query(
        "SELECT * FROM experience_replays
         WHERE last_used_at < ? AND success_score < ?",
        [now() - 30_days, 0.85]
    ).await?;

    for exp in candidates {
        if exp.usage_count < 5 {
            // 低频低质：直接删除
            self.db.delete_experience(&exp.id).await?;
        } else {
            // 曾经有用但已过时：归档
            self.db.update_status(&exp.id, "archived").await?;
        }
    }
}
```

---

## 第四部分：路由增强设计（L1.5 层实现）

### L1.5 经验重放层架构

```rust
// 在 Dispatcher 调度流程中插入 L1.5 拦截器
pub struct ExperienceReplayLayer {
    db: Arc<VectorDatabase>,
    embedder: Arc<SmartEmbedder>,
    similarity_threshold: f64,  // 默认 0.85
}

impl ExperienceReplayLayer {
    /// 尝试匹配已有经验，返回 Some(tool_sequence) 或 None
    pub async fn try_match(&self, intent: &str) -> Result<Option<ReplayMatch>> {
        // Step 1: 生成意图向量
        let intent_vector = self.embedder.embed(intent).await?;

        // Step 2: 向量相似度搜索（只查询 verified/distilled 状态的经验）
        let candidates = self.db.vector_search_experiences(
            &intent_vector,
            top_k: 5,
            min_score: self.similarity_threshold,
            status_filter: vec!["verified", "distilled"]
        ).await?;

        if candidates.is_empty() {
            return Ok(None);  // 无匹配，降级到 L2/L3
        }

        // Step 3: 参数化匹配（提取当前意图中的实体）
        let best_match = self.select_best_match(intent, candidates).await?;

        // Step 4: 参数填充
        let filled_sequence = self.fill_parameters(intent, &best_match).await?;

        Ok(Some(ReplayMatch {
            experience_id: best_match.id,
            tool_sequence: filled_sequence,
            confidence: best_match.similarity_score,
        }))
    }
}
```

### 参数化重放（Parametric Replay）

```rust
// parameter_mapping 示例（存储在 experience_replays 表中）
{
    "variables": {
        "file_path": {
            "type": "path",
            "extraction_rule": "regex:(?:file|path)\\s+['\"]?([^'\"\\s]+)",
            "default": null
        },
        "search_term": {
            "type": "string",
            "extraction_rule": "keyword_after:search for",
            "default": null
        }
    }
}

// 参数填充逻辑
async fn fill_parameters(&self, intent: &str, experience: &Experience) -> Result<ToolSequence> {
    let mapping: ParameterMapping = serde_json::from_str(&experience.parameter_mapping)?;
    let mut filled_sequence = experience.tool_sequence.clone();

    for (var_name, var_config) in mapping.variables {
        // 使用正则或 NER 提取实体
        let extracted_value = self.extract_entity(intent, &var_config)?;

        if extracted_value.is_none() && var_config.default.is_none() {
            // 关键参数缺失，无法重放
            return Err(AlephError::ParameterExtractionFailed(var_name));
        }

        let value = extracted_value.or(var_config.default).unwrap();

        // 替换模板中的变量
        filled_sequence = filled_sequence.replace(&format!("{{{}}}", var_name), &value);
    }

    Ok(filled_sequence)
}
```

### 降级机制（Graceful Degradation）

```rust
// 在 Dispatcher 主流程中
pub async fn dispatch(&self, intent: &str) -> Result<Response> {
    // L1: 正则/缓存匹配（现有逻辑）
    if let Some(result) = self.l1_match(intent).await? {
        return Ok(result);
    }

    // L1.5: 经验重放（新增）
    if let Some(replay) = self.experience_layer.try_match(intent).await? {
        match self.execute_replay(&replay).await {
            Ok(result) => {
                // 成功：更新 usage_count 和 success_count
                self.db.increment_experience_usage(&replay.experience_id, true).await?;
                return Ok(result);
            }
            Err(e) => {
                // 失败：记录失败，降级到 L3
                self.db.increment_experience_usage(&replay.experience_id, false).await?;
                warn!("Experience replay failed: {}, falling back to L3", e);
                // 继续执行下面的 L2/L3 逻辑
            }
        }
    }

    // L2: 语义工具匹配（现有逻辑）
    if let Some(result) = self.l2_match(intent).await? {
        return Ok(result);
    }

    // L3: 智能推理决策（现有逻辑）
    self.l3_think_and_act(intent).await
}
```

### 反馈闭环（Feedback Loop）

```rust
// 每次重放后更新统计信息
async fn increment_experience_usage(&self, exp_id: &str, success: bool) -> Result<()> {
    let exp = self.get_experience(exp_id).await?;

    let new_usage_count = exp.usage_count + 1;
    let new_success_count = if success { exp.success_count + 1 } else { exp.success_count };
    let new_success_rate = new_success_count as f64 / new_usage_count as f64;

    // 检查是否达到"硬化"条件（为未来的代码生成预留）
    if new_usage_count > 20 && new_success_rate > 0.95 && exp.evolution_status == "verified" {
        // 标记为"待硬化"
        self.update_status(exp_id, "ready_for_distillation").await?;
    }

    self.db.execute(
        "UPDATE experience_replays
         SET usage_count = ?, success_count = ?, last_success_rate = ?, last_used_at = ?
         WHERE id = ?",
        params![new_usage_count, new_success_count, new_success_rate, now(), exp_id]
    ).await
}
```

---

## 第五部分：实施计划（3个月 MVP 路线图）

### Month 1: 感知与采集层（Telemetry & Infrastructure）

**目标**：建立完整的数据采集基础设施

**任务清单**：

1. **数据库迁移**
   - 创建 `experience_replays` 表及索引
   - 定义 Rust struct `Experience` 和相关类型
   - 实现 CRUD 操作（insert/query/update/delete）

2. **Agent Loop 增强**
   - 在 `agent_loop` 退出时注入实时评估逻辑
   - 捕获完整上下文快照：
     - User Intent（原始输入）
     - Environment Context（工作目录、平台、权限）
     - Tool Sequence（完整的工具调用链）
     - Execution Metrics（耗时、Token消耗、成功/失败）
   - 实现实时触发条件检测（耗时>30s、链长>5、纠错场景）

3. **ValueEstimator 集成**
   - 扩展现有的 `value_estimator` 模块
   - 实现多维度评分算法
   - 添加 novelty_score 计算（基于 pattern_hash 的编辑距离）

**交付物**：
- 数据库 schema 和迁移脚本
- 完整的 Telemetry 管道
- 实时评估触发器

---

### Month 2: 模式识别与聚类（Pattern Discovery）

**目标**：实现自动化的经验蒸馏和聚类

**任务清单**：

1. **Dreaming 进程改造**
   - 将 `dreaming.rs` 升级为 Cortex 后台服务
   - 实现批量扫描逻辑（每日凌晨2-4点）
   - 集成 LLM 调用（Haiku for batch, Sonnet for realtime）

2. **蒸馏管道实现**
   - 实现 `DistillationService`
   - 预处理：去噪、合并、提取关键决策点
   - 总结：调用 LLM 生成 pattern_description 和 parameter_mapping
   - 入库：写入 experience_replays 并更新向量索引

3. **聚类引擎**
   - 基于 pattern_hash 的相似度聚类
   - 识别高频模式（usage_count > 5）
   - 自动生成参数化模板

4. **Information Decay**
   - 实现 LRU-Value 复合衰减算法
   - 自动归档/删除过时经验

**交付物**：
- 完整的 Cortex 后台服务
- 蒸馏管道和聚类引擎
- 衰减机制

---

### Month 3: 自动化工具化（Auto-Tooling & L1.5 Integration）

**目标**：部署 L1.5 经验重放层，实现"越用越快"

**任务清单**：

1. **L1.5 层实现**
   - 创建 `ExperienceReplayLayer` 模块
   - 实现向量相似度搜索（基于 intent_vector）
   - 实现参数化重放（Parametric Replay）
   - 实现降级机制（Graceful Degradation）

2. **Dispatcher 集成**
   - 在现有 Dispatcher 中插入 L1.5 拦截器
   - 修改调度流程：L1 → L1.5 → L2 → L3
   - 实现反馈闭环（更新 usage_count 和 success_rate）

3. **监控与可观测性**
   - 添加 L1.5 命中率指标
   - 记录 Token 节省和延迟降低
   - 实现经验质量监控（success_rate 趋势）

4. **用户界面**
   - 在 macOS App 中添加"经验库"查看界面
   - 支持手动标记高价值任务
   - 支持查看和删除已有经验

**交付物**：
- 完整的 L1.5 经验重放层
- Dispatcher 集成
- 监控仪表板

---

### 成功指标（3个月后）

1. **数据积累**：
   - experience_replays 表中至少有 100+ 条高质量经验
   - 覆盖 10+ 种常见任务模式

2. **性能提升**：
   - L1.5 命中率 > 15%（即 15% 的任务可以跳过 L3）
   - 命中任务的平均延迟降低 80%+
   - 命中任务的 Token 消耗降低 95%+

3. **质量验证**：
   - L1.5 重放的成功率 > 85%
   - 用户满意度反馈（通过显式点赞）

---

## 架构演进路径

### 当前阶段（MVP: Soft Skills）

```
[ 用户意图 ]
      ↓
┌─────────────────────────────────────────────────────────────┐
│  Dispatcher Layer (路由层)                                   │
│                                                             │
│  L1: 正则/缓存匹配 (Reflex)                                  │
│         ↓                                                   │
│  L1.5: 经验重放层 (Soft Skills - MVP 目标)                   │
│       - 匹配 intent_vector + pattern_hash                   │
│       - 执行参数化 Tool Template                             │
│         ↓                                                   │
│  L2: 语义工具匹配 (Semantic Search)                         │
│         ↓                                                   │
│  L3: 智能推理决策 (Deep Thinking)                           │
└─────────────────────────────────────────────────────────────┘
      ↓
┌─────────────────────────────────────────────────────────────┐
│  Atomic Engine (执行引擎)                                    │
│  - 执行原子操作 (Read/Write/Edit/Bash)                        │
│  - 捕获 Task Trace (Log)                                     │
└─────────────────────────────────────────────────────────────┘
      ↓
┌─────────────────────────────────────────────────────────────┐
│  Cortex Layer (进化层 - 后台进程)                              │
│                                                             │
│  Step 1: 实时/批量触发 (Hybrid Strategy)                     │
│  Step 2: 经验蒸馏 (Distillation Pipeline)                    │
│  Step 3: 存入教材库 (Experience Replay DB)                    │
│  Step 4: 模式硬化 (Future Hardening -> Code Gen)             │
└─────────────────────────────────────────────────────────────┘
```

### 未来阶段（6-12个月: Hard Skills）

当 experience 达到硬化条件（usage_count > 20 且 success_rate > 95%）时：

1. **代码生成**：Cortex 调用 LLM 生成 Python 脚本或 WASM 插件
2. **自动测试**：在沙箱中运行单元测试和集成测试
3. **工具注册**：自动注册为新的 AlephTool，获得独立的 tool_name
4. **L1 集成**：在 L1 层可以直接调用，完全绕过 L3

---

## 关键设计决策记录

### 为什么选择独立的 experience_replays 表？

- **职责分离**：task_traces 是"日志"（所有执行），experience_replays 是"教材"（精选案例）
- **信息衰减**：独立表更容易实现 Information Decay 机制
- **未来扩展**：可以添加 pattern_id、cluster_id 等字段，支持聚类功能

### 为什么选择混合触发策略？

- **实时评估**：捕获高价值任务（用户点赞、纠错场景），快速响应
- **批量评估**：成本可控，可以使用更便宜的模型，系统化处理
- **生物启发**：符合人类学习模式（重大事件立即学习 + 睡眠巩固）

### 为什么 MVP 选择 Soft Skills 而非 Hard Skills？

- **快速验证**：3-6个月内可以看到效果（Token 节省、延迟降低）
- **降低风险**：代码生成的安全风险高，需要完整的 CI/CD 流程
- **数据积累**：为未来的 Hard Skills 积累高质量数据

---

## 参考文献

- [AGENT_DESIGN_PHILOSOPHY.md](../AGENT_DESIGN_PHILOSOPHY.md) - Agent 设计思想
- [POE Architecture](./2026-02-01-poe-architecture-design.md) - POE 架构详细设计
- [Memory System](../MEMORY_SYSTEM.md) - 现有记忆系统文档

---

**下一步行动**：开始 Month 1 实施 - 创建数据库迁移脚本和 Rust struct 定义
