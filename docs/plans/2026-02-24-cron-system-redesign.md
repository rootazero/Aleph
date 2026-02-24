# Cron System Redesign — 超越 openclaw

> **Date**: 2026-02-24
> **Status**: Approved
> **Scope**: 完整重构 + 超越 openclaw 功能集
> **Approach**: 渐进式重构 (Incremental Refactor)

---

## 1. 背景与动机

### 1.1 现状评估

Aleph 的 CronService 采用 Rust + SQLite 架构，已实现任务持久化、并发控制和执行日志。核心组件：

- **存储层**: SQLite (`cron_jobs` + `cron_runs`)
- **调度层**: tokio 任务循环 + `cron` crate
- **执行层**: `JobExecutor` 回调 + 超时控制

### 1.2 缺陷分析 (对比 openclaw)

| 缺陷 | Aleph 现状 | openclaw 实现 | 影响 |
|------|-----------|--------------|------|
| **重启追赶缺失** | 仅检测当前时刻 ±60s | `runMissedJobs()` 补偿机制 | 离线期间任务永久跳过 |
| **重复执行风险** | `diff < 60s` 窗口判定 | `runningAtMs` 标记 + 原子锁 | 高频检查下可能双重触发 |
| **时区硬编码** | 存储 timezone 但不使用 | `croner` + IANA 时区 | 无法按本地时间触发 |
| **无投递管道** | 结果仅存 cron_runs | `delivery.ts` 多模式投递 | 任务执行后无法通知用户 |
| **无重试机制** | 失败即终止 | 指数退避 30s→60min | AI 偶发错误无法恢复 |
| **单一调度类型** | 仅 cron 表达式 | cron + every + at | 无法设定固定间隔或一次性任务 |

### 1.3 设计目标

**"超越 openclaw"** — 学习其功能完备性，利用 Rust + SQLite 的性能优势在架构深度上超越。

超越点：
1. **记忆化执行日志** — 执行结果自动存入 LanceDB，AI 可检索历史
2. **动态 Prompt 模板** — `{{last_output}}` 实现任务间记忆连续性
3. **资源感知调度** — CPU 负载门控，防止 AI "集体爆发"
4. **插件化投递** — DeliveryTarget trait，不仅通知还能执行动作
5. **轻量 Job Chain** — on_success/on_failure + TaskGraph 启动器

---

## 2. 存储层设计 (Storage — 双轨制)

### 2.1 设计哲学

**控制面 (SQLite)**: 守底线 — 任务的"死活"、锁定、状态流转
**数据面 (LanceDB)**: 拔上限 — 存储任务执行产出的"知识"

### 2.2 cron_jobs 表扩展

在现有 10 个字段基础上新增字段：

```sql
ALTER TABLE cron_jobs ADD COLUMN next_run_at INTEGER;              -- 下次执行时间 (ms)
ALTER TABLE cron_jobs ADD COLUMN running_at INTEGER;               -- 执行锁定时间戳
ALTER TABLE cron_jobs ADD COLUMN last_run_at INTEGER;              -- 上次执行完成时间

ALTER TABLE cron_jobs ADD COLUMN consecutive_failures INTEGER DEFAULT 0;  -- 连续失败计数
ALTER TABLE cron_jobs ADD COLUMN max_retries INTEGER DEFAULT 3;           -- 最大重试次数
ALTER TABLE cron_jobs ADD COLUMN priority INTEGER DEFAULT 5;              -- 1(最高)-10(最低)

ALTER TABLE cron_jobs ADD COLUMN schedule_kind TEXT DEFAULT 'cron';       -- 'cron'|'every'|'at'
ALTER TABLE cron_jobs ADD COLUMN every_ms INTEGER;                        -- kind=every 间隔
ALTER TABLE cron_jobs ADD COLUMN at_time INTEGER;                         -- kind=at 执行时间
ALTER TABLE cron_jobs ADD COLUMN delete_after_run INTEGER DEFAULT 0;      -- 一次性任务自动删除

ALTER TABLE cron_jobs ADD COLUMN next_job_id_on_success TEXT;     -- 成功触发链
ALTER TABLE cron_jobs ADD COLUMN next_job_id_on_failure TEXT;     -- 失败触发链

ALTER TABLE cron_jobs ADD COLUMN delivery_config TEXT;             -- JSON: DeliveryConfig
ALTER TABLE cron_jobs ADD COLUMN prompt_template TEXT;             -- {{variable}} 模板
ALTER TABLE cron_jobs ADD COLUMN context_vars TEXT;                -- JSON: 变量来源
ALTER TABLE cron_jobs ADD COLUMN version INTEGER DEFAULT 1;       -- 乐观锁
```

### 2.3 cron_runs 表增强

```sql
ALTER TABLE cron_runs ADD COLUMN retry_count INTEGER DEFAULT 0;
ALTER TABLE cron_runs ADD COLUMN trigger_source TEXT DEFAULT 'schedule'; -- 'schedule'|'chain'|'manual'|'catchup'
ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT;
ALTER TABLE cron_runs ADD COLUMN delivery_error TEXT;
```

### 2.4 索引策略

```sql
CREATE INDEX idx_jobs_next_run ON cron_jobs(next_run_at) WHERE enabled = 1;
CREATE INDEX idx_jobs_running ON cron_jobs(running_at);
CREATE INDEX idx_jobs_chain ON cron_jobs(next_job_id_on_success);
```

### 2.5 记忆化同步 (LanceDB)

任务执行结果异步写入 Memory 系统：

```rust
// 每次 cron_run 完成后
memory_store.insert(MemoryFact {
    content: format!("Cron job '{}' result: {}", job.name, result.response),
    tags: vec!["cron", &job.name],
    source: MemorySource::CronJob { job_id: job.id.clone() },
    ..Default::default()
}).await;
```

**用户场景**: "你上周每天做的早间简报，关注了哪些技术趋势？" → LanceDB 向量搜索即时返回。

---

## 3. 调度引擎设计 (Scheduler Engine)

### 3.1 核心状态机

```
Job Created → [计算 next_run_at] → Idle
                                      ↓
Idle → [next_run_at <= now] → Pending → [acquire lock: SET running_at] → Running
                                                                            ↓
Running → [executor 返回] → Success / Failed / Timeout
                               ↓            ↓            ↓
                         [更新 next_run_at] [退避重试?]  [退避重试?]
                         [触发 chain?]      [chain?]     [chain?]
                         [投递结果]         [投递结果]    [投递结果]
                               ↓
                         → Idle (循环)
```

### 3.2 调度 Tick 逻辑

替换现有 `check_and_run_jobs`：

```rust
async fn scheduler_tick(db: &Path, semaphore: Arc<Semaphore>, executor: &JobExecutor) {
    let now_ms = Utc::now().timestamp_millis();

    // 1. 僵尸任务恢复 (Stuck Job Recovery)
    //    running_at 超过 STUCK_THRESHOLD_MS (2h) 的任务强制释放
    clear_stuck_jobs(db, now_ms, STUCK_THRESHOLD_MS).await;

    // 2. 资源感知门控 (Resource Gating)
    let effective_concurrency = resolve_effective_concurrency(config_max, &semaphore);

    // 3. 原子获取待执行任务 (Atomic Acquire)
    //    BEGIN IMMEDIATE 事务保证写锁互斥
    let acquired = atomic_acquire(db, now_ms, effective_concurrency).await;

    // 4. 并发执行
    for job in acquired {
        let permit = semaphore.clone().try_acquire_owned()?;
        tokio::spawn(async move {
            let _permit = permit;
            let result = execute_with_retry(&job, executor).await;
            finalize_job(db, &job, &result).await;
        });
    }
}
```

### 3.3 原子获取 SQL (防重复执行)

```sql
BEGIN IMMEDIATE;

SELECT id, name, schedule, agent_id, prompt, prompt_template, context_vars,
       schedule_kind, every_ms, at_time, timezone, priority,
       next_job_id_on_success, next_job_id_on_failure,
       delivery_config, consecutive_failures, max_retries
FROM cron_jobs
WHERE enabled = 1
  AND next_run_at <= ?now_ms
  AND running_at IS NULL
ORDER BY priority ASC, next_run_at ASC
LIMIT ?effective_concurrency;

UPDATE cron_jobs
SET running_at = ?now_ms, version = version + 1
WHERE id IN (?acquired_ids);

COMMIT;
```

### 3.4 指数退避重试

```rust
const BACKOFF_SCHEDULE: &[u64] = &[
    30_000,      // 1st failure → 30s
    60_000,      // 2nd → 1 min
    300_000,     // 3rd → 5 min
    900_000,     // 4th → 15 min
    3_600_000,   // 5th+ → 60 min
];

fn compute_backoff_ms(consecutive_failures: u32) -> u64 {
    let idx = (consecutive_failures.saturating_sub(1) as usize)
        .min(BACKOFF_SCHEDULE.len() - 1);
    BACKOFF_SCHEDULE[idx]
}
```

### 3.5 重启追赶 (Catch-up on Startup)

```rust
async fn startup_catchup(db: &Path, executor: &JobExecutor) {
    let now_ms = Utc::now().timestamp_millis();

    // Phase 1: 清除所有 running_at 标记 (上次非正常关闭残留)
    clear_all_running_markers(db).await;

    // Phase 2: 查找过期任务 (next_run_at < now)
    let missed = query_overdue_jobs(db, now_ms).await;

    // Phase 3: 过滤并执行
    //   - kind=at 且有 last_run_at → 跳过 (已完成的一次性任务)
    //   - 其余按 priority ASC 执行
    for job in missed.iter().filter(|j| !is_completed_oneshot(j)) {
        execute_and_finalize(db, job, executor, "catchup").await;
    }

    // Phase 4: 重新计算所有 enabled 任务的 next_run_at
    recompute_all_next_runs(db).await;
}
```

### 3.6 next_run_at 计算 (三种调度类型)

```rust
fn compute_next_run_at(job: &CronJob, from: DateTime<Utc>) -> Option<i64> {
    match job.schedule_kind.as_str() {
        "cron" => {
            // 使用 chrono-tz 解析 IANA 时区
            let tz = job.timezone.as_deref()
                .and_then(|s| s.parse::<chrono_tz::Tz>().ok())
                .unwrap_or(chrono_tz::UTC);
            let local_now = from.with_timezone(&tz);
            let schedule = Schedule::from_str(&job.schedule).ok()?;
            schedule.after(&local_now).next()
                .map(|t| t.with_timezone(&Utc).timestamp_millis())
        }
        "every" => {
            let interval = job.every_ms?;
            Some(from.timestamp_millis() + interval)
        }
        "at" => {
            let target = job.at_time?;
            if target > from.timestamp_millis() { Some(target) } else { None }
        }
        _ => None,
    }
}
```

### 3.7 僵尸任务检测

```rust
const STUCK_THRESHOLD_MS: i64 = 2 * 60 * 60 * 1000; // 2 hours

async fn clear_stuck_jobs(db: &Path, now_ms: i64, threshold: i64) {
    let cutoff = now_ms - threshold;
    let cleared = execute_sql(db,
        "UPDATE cron_jobs SET running_at = NULL WHERE running_at IS NOT NULL AND running_at < ?1",
        params![cutoff]
    ).await;
    if cleared > 0 {
        tracing::warn!("Cleared {} stuck cron jobs", cleared);
    }
}
```

---

## 4. Delivery 投递管道 (Delivery Pipeline)

### 4.1 设计哲学

**从"发送消息"到"执行动作"**。openclaw 的投递是通知 (Notification)，Aleph 的投递是动作 (Action)。

### 4.2 DeliveryTarget Trait

```rust
#[async_trait]
pub trait DeliveryTarget: Send + Sync {
    /// 投递目标类型标识
    fn kind(&self) -> &str;

    /// 执行投递
    async fn deliver(
        &self,
        job: &CronJob,
        result: &JobResult,
    ) -> Result<DeliveryOutcome, DeliveryError>;
}

pub struct DeliveryOutcome {
    pub target_kind: String,
    pub success: bool,
    pub message: Option<String>,
}
```

### 4.3 投递配置模型

```rust
/// 存储在 cron_jobs.delivery_config (JSON)
#[derive(Serialize, Deserialize)]
pub struct DeliveryConfig {
    pub mode: DeliveryMode,
    pub targets: Vec<DeliveryTargetConfig>,
    pub fallback_target: Option<DeliveryTargetConfig>,
}

#[derive(Serialize, Deserialize)]
pub enum DeliveryMode {
    None,       // 不投递
    Primary,    // 仅第一个目标
    Broadcast,  // 广播所有目标
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DeliveryTargetConfig {
    Gateway {
        channel: String,       // "telegram" | "discord" | "imessage"
        chat_id: String,
        format: Option<String>,
    },
    Memory {
        tags: Vec<String>,
        importance: Option<f32>,
    },
    Webhook {
        url: String,
        method: Option<String>,
        headers: Option<HashMap<String, String>>,
    },
}
```

### 4.4 投递引擎

```rust
pub struct DeliveryEngine {
    targets: HashMap<String, Arc<dyn DeliveryTarget>>,
}

impl DeliveryEngine {
    pub fn register(&mut self, target: Arc<dyn DeliveryTarget>) { ... }

    pub async fn deliver(
        &self, job: &CronJob, result: &JobResult, config: &DeliveryConfig,
    ) -> Vec<DeliveryOutcome> {
        // 1. 始终投递到 MemoryTarget (记忆沉淀)
        // 2. 按 mode 投递到配置的目标
        // 3. Primary 模式失败时尝试 fallback
        // 4. Broadcast 模式并发投递所有目标
    }
}
```

### 4.5 分阶段实现

| 阶段 | 投递目标 | 描述 |
|------|---------|------|
| V1 | GatewayTarget | 通过 Gateway 现有接口发到 Telegram/Discord/iMessage |
| V2 | MemoryTarget | 自动向量化存入 LanceDB |
| V2 | WebhookTarget | HTTP POST/PUT 到指定 URL |
| V3 | CompositeTarget | 链式处理: [Processor] → [Target, Target] |

---

## 5. 动态 Prompt 模板 (Template Engine)

### 5.1 内置变量

| 变量 | 说明 | 示例 |
|------|------|------|
| `{{now}}` | 当前时间 ISO 8601 | `2026-02-24T09:00:00Z` |
| `{{now_unix}}` | Unix 时间戳 | `1771923600` |
| `{{job_name}}` | 任务名称 | `Daily News Summary` |
| `{{last_output}}` | 上次执行结果 | `(上次 AI 输出)` |
| `{{run_count}}` | 总执行次数 | `42` |
| `{{env:VAR_NAME}}` | 环境变量 | `$API_KEY` |

### 5.2 记忆连续性场景

```
Job A: "Summarize today's tech news"
  ↓ on_success
Job B: "Based on: {{last_output}}\n\nProvide investment analysis"
  ↓ on_success
Job C (Delivery): Send to Telegram + Store in Memory
```

### 5.3 预留扩展

`context_vars` JSON 字段预留变量来源定义：

```json
{
  "sources": {
    "last_output": { "from": "prev_job" },
    "market_data": { "from": "memory", "query": "latest market trends" }
  }
}
```

V1 仅实现 `prev_job` 来源，`memory` 来源作为 Future Work。

---

## 6. 资源感知调度 (Resource-Aware Scheduling)

### 6.1 优先级体系

| Priority | 级别 | 行为 |
|----------|------|------|
| 1-3 | 高 | 始终执行，即使高负载 |
| 4-7 | 中 | 正常负载执行，高负载延迟 |
| 8-10 | 低 | 仅空闲时执行 |

### 6.2 负载门控

```rust
fn resolve_effective_concurrency(config_max: usize, semaphore: &Semaphore) -> usize {
    let available = semaphore.available_permits();
    let cpu = get_cpu_usage(); // 0.0 - 1.0

    let limit = if cpu > 0.8 { 1 }              // 高负载: 仅最高优先级
                else if cpu > 0.6 { config_max / 2 }  // 中等: 半速
                else { config_max };                    // 正常: 全速

    limit.max(1).min(available)
}
```

SQL 中 `ORDER BY priority ASC` 确保高优先级任务在限流时优先执行。

---

## 7. Job Chain 依赖 (Lightweight Dependencies)

### 7.1 触发机制

- `next_job_id_on_success`: 成功后将目标任务的 `next_run_at` 设为当前时间
- `next_job_id_on_failure`: 失败后同理
- 目标任务在下一次 `scheduler_tick` 自然被拾取执行

### 7.2 循环检测

在 Add/Update API 层执行 DFS 检测：

```rust
async fn detect_cycle(db: &Path, start_id: &str, new_target: &str) -> bool {
    let mut visited = HashSet::new();
    let mut current = Some(new_target.to_string());
    while let Some(id) = current {
        if id == start_id { return true; }
        if !visited.insert(id.clone()) { break; }
        current = query_next_job_id(db, &id).await;
    }
    false
}
```

### 7.3 TaskGraph 启动器 (Future)

定义 `TaskGraphJob` 载荷类型，让 Cron 触发 Dispatcher 执行复杂 DAG：

```rust
// Future: 特殊载荷类型
pub enum CronPayload {
    Prompt(String),              // 现有: 直接发 prompt 给 agent
    TaskGraph(TaskGraphConfig),  // Future: 提交 DAG 给 Dispatcher
}
```

---

## 8. 对比总结

| 能力 | openclaw | Aleph (重构后) | 超越点 |
|------|---------|---------------|--------|
| 调度精度 | `nextRunAtMs` + 60s 上限 | `next_run_at` + SQLite 原子锁 | ACID 事务保证，无竞态 |
| 重启追赶 | JSON 加载 + stale marker | SQLite + catch-up 查询 | 持久化更可靠 |
| 重复防护 | `runningAtMs` in-memory | `running_at` + `BEGIN IMMEDIATE` | 跨进程安全 |
| 投递系统 | announce/webhook/none | DeliveryTarget trait + 多目标 + fallback | 可扩展、可组合 |
| 重试策略 | 指数退避 5 级 | 指数退避 5 级 + max_retries | 相当 |
| 时区支持 | croner + IANA | chrono-tz + IANA | 相当 |
| 调度类型 | cron/every/at | cron/every/at | 相当 |
| 记忆化 | 无 | LanceDB 自动沉淀 | **独有** |
| 动态 Prompt | 无 | `{{last_output}}` 模板 | **独有** |
| 资源感知 | 无 | priority + CPU 门控 | **独有** |
| Job Chain | 无 | on_success/on_failure | **独有** |
| TaskGraph | 无 | Cron → Dispatcher DAG | **独有** |

---

## 9. 新增依赖

| Crate | 用途 | 备注 |
|-------|------|------|
| `chrono-tz` | IANA 时区解析 | 替代当前 UTC 硬编码 |
| `sysinfo` | CPU 负载检测 | 资源感知调度 |
| `regex` | 模板变量解析 | 已在项目中使用 |

---

## 10. 文件变更清单

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `core/src/cron/mod.rs` | 重构 | 调度逻辑全面重写 |
| `core/src/cron/config.rs` | 扩展 | 新字段、新类型定义 |
| `core/src/cron/scheduler.rs` | 新建 | 调度引擎 (tick, acquire, catchup) |
| `core/src/cron/delivery.rs` | 新建 | DeliveryTarget trait + DeliveryEngine |
| `core/src/cron/delivery/gateway.rs` | 新建 | GatewayTarget 实现 |
| `core/src/cron/delivery/memory.rs` | 新建 | MemoryTarget 实现 |
| `core/src/cron/delivery/webhook.rs` | 新建 | WebhookTarget 实现 |
| `core/src/cron/template.rs` | 新建 | Prompt 模板引擎 |
| `core/src/cron/chain.rs` | 新建 | Job Chain 触发 + 循环检测 |
| `core/src/cron/resource.rs` | 新建 | 资源感知调度 |
| `core/src/gateway/handlers/cron.rs` | 重写 | 实际接入 CronService (去 stub) |
