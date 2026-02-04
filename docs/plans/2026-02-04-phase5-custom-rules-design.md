# Phase 5: User-Customizable Rules Engine - Architecture Design

> **Design Date**: 2026-02-04
> **Phase**: 5 (Custom Rules Engine)
> **Status**: Approved, Ready for Implementation
> **依赖**: Phase 3 (WorldModel) + Phase 4 (Dispatcher)

---

## 设计背景

基于已完成的 Phase 1-4（DaemonManager, Perception Layer, WorldModel, Dispatcher），现在需要设计 **Phase 5: 用户可自定义规则引擎**。

### 核心目标

让用户能够通过配置文件自定义 Proactive AI 的行为规则，无需修改 Rust 代码。

**关键需求**：
1. **80% 简单场景**：低电量提醒、开会静音等简单条件判断
2. **20% 复杂场景**：历史回溯、趋势检测、模式识别（如"过去 2 小时编码无休息"）
3. **安全性**：规则引擎必须沙箱化，防止无限循环、内存泄漏、恶意代码执行
4. **性能**：轻量级，符合 Aleph 的 Frugal Resources 原则
5. **渐进增强**：从简单声明式规则平滑过渡到复杂表达式

**设计哲学**：

> "JARVIS 是为托尼·斯塔克服务的，而不是只为编译器工程师服务的。"

规则系统必须在易用性和强大能力之间找到完美平衡。

---

## Part 1: 架构概览

### 1.1 核心决策矩阵

| 维度 | 选择 | 理由 |
|------|------|------|
| **规则格式** | YAML + Rhai Expression | 统一性 + 平滑学习曲线 |
| **脚本引擎** | Rhai (Sandbox Mode) | 表达能力 + 可控安全性 + Rust 语法亲和 |
| **API 风格** | Fluent (链式调用) | 符合人类思维（拿取→过滤→统计） |
| **事件对象** | 字符串 + 辅助方法 | 实用主义（`activity == "Programming"` + `is_coding()`） |
| **统计能力** | Baseline + Trend Detection | 从"反应式"到"察言观色" |
| **Baseline 策略** | MVP: Fixed 7d, 未来: Smart | 渐进式复杂度 |
| **性能优化** | Lazy + TTL Cache | 按需计算，避免过早优化 |
| **Cold Start** | 优雅降级（用可用数据） | 用户体验优先 |

### 1.2 系统架构

```
┌─────────────────────────────────────────────────────────┐
│                    User Layer                            │
│  ~/.aleph/policies.yaml (YAML + Rhai Expressions)      │
└────────────────────────┬────────────────────────────────┘
                         │
                         ↓
┌─────────────────────────────────────────────────────────┐
│                 Policy Loader & Validator                │
│  • Parse YAML                                            │
│  • Compile Rhai expressions                              │
│  • Validate syntax & safety                              │
│  • Hot reload on file change                             │
└────────────────────────┬────────────────────────────────┘
                         │
                         ↓
┌─────────────────────────────────────────────────────────┐
│              Rhai Engine (Sandboxed)                     │
│  • max_operations: 1000                                  │
│  • max_expr_depth: 10                                    │
│  • Disabled: IO, modules, print                          │
│  • Timeout: 100ms per expression                         │
└────────────────────────┬────────────────────────────────┘
                         │
                         ↓
┌─────────────────────────────────────────────────────────┐
│                   RhaiApi Layer                          │
│  HistoryApi:                                             │
│    • last(duration) -> EventCollection                   │
│    • baseline(metric) -> BaselineApi                     │
│  EventCollection:                                        │
│    • filter(predicate) -> EventCollection                │
│    • sum_duration() -> Duration                          │
│    • count() -> i64                                      │
│    • avg_per_hour() -> f64                               │
│    • trend() -> String                                   │
│  EventApi:                                               │
│    • activity (String)                                   │
│    • duration() -> Duration                              │
│    • is_coding() -> bool                                 │
│    • is_idle() -> bool                                   │
└────────────────────────┬────────────────────────────────┘
                         │
                         ↓
┌─────────────────────────────────────────────────────────┐
│              WorldModel + InferenceCache                 │
│  • CoreState (JSON persistence)                          │
│  • EnhancedContext (Runtime state)                       │
│  • InferenceCache (Baseline cache, TTL = 1h)             │
└─────────────────────────────────────────────────────────┘
```

### 1.3 与 Phase 4 的集成

Phase 5 扩展了 Phase 4 的 PolicyEngine：

```rust
// Phase 4: Hardcoded policies
impl PolicyEngine {
    fn new_mvp() -> Self {
        Self {
            policies: vec![
                Box::new(MeetingMutePolicy),  // Hardcoded
                Box::new(LowBatteryPolicy),   // Hardcoded
                // ...
            ],
        }
    }
}

// Phase 5: YAML-based policies
impl PolicyEngine {
    fn new_with_yaml(path: PathBuf) -> Result<Self> {
        let yaml_policies = YamlPolicyLoader::load(path)?;
        Self {
            policies: vec![
                // Hardcoded policies (backward compatible)
                Box::new(MeetingMutePolicy),
                // YAML policies
                ...yaml_policies.into_iter()
                    .map(|p| Box::new(YamlPolicy::new(p)))
                    .collect(),
            ],
        }
    }
}
```

**共存策略**：
- Hardcoded policies 保留（用于核心功能）
- YAML policies 叠加（用户自定义）
- 优先级：YAML > Hardcoded（允许用户覆盖）

---

## Part 2: 规则格式设计

### 2.1 YAML Schema

```yaml
# ~/.aleph/policies.yaml

- name: String               # 规则名称（必填）
  enabled: Boolean           # 是否启用（默认 true）

  trigger:                   # 触发条件（必填）
    event: EventType         # DerivedEvent 类型
    to: ActivityType         # 可选：活动类型过滤

  constraints:               # 简单约束（可选）
    - key: "comparison"      # 如 battery_level: "> 20"

  conditions:                # 复杂条件（可选，Rhai 表达式）
    - expr: String           # Rhai 表达式

  action:                    # 执行动作（必填）
    type: ActionType         # ActionType enum
    message: String          # 可选：通知消息
    priority: Priority       # 可选：优先级

  risk: RiskLevel            # 风险等级（low/medium/high）
  metadata:                  # 可选：附加元数据
    tags: [String]
    author: String
```

### 2.2 示例规则文件

```yaml
# ============================================================
# Simple Rules (Type 1 & 2) - 80% 场景
# ============================================================

- name: "Low Battery Alert"
  trigger:
    event: resource_pressure_changed
    pressure_type: battery
  constraints:
    - battery_level: "< 20"
  action:
    type: notify
    message: "电量低于 20%，建议充电"
    priority: high
  risk: low

- name: "Meeting Auto-Mute"
  trigger:
    event: activity_changed
    to: meeting
  action:
    type: mute_system_audio
  risk: low

# ============================================================
# Complex Rules (Type 3) - 20% 场景
# ============================================================

- name: "Smart Break Reminder"
  trigger:
    event: activity_changed
    to: programming
  conditions:
    # 过去 2 小时编码超过 90 分钟
    - expr: |
        history.last("2h")
          .filter(|e| e.is_coding())
          .sum_duration() > duration("90m")
    # 且没有 5 分钟以上的休息
    - expr: |
        !history.last("2h")
          .any(|e| e.is_idle() && e.duration() > duration("5m"))
  action:
    type: notify
    message: "已连续编码 {{coding_time}}，建议休息 10 分钟"
    priority: normal
  risk: low

- name: "Refactoring Mode Detection"
  trigger:
    event: file_changed
  conditions:
    # 文件修改频率是平常的 3 倍
    - expr: |
        let current = history.last("1h").file_changes().count();
        let baseline = history.baseline("file_changes").avg();
        current > baseline * 3.0
  action:
    type: enable_do_not_disturb
    reason: "检测到大规模代码修改，已开启免打扰"
  risk: medium

- name: "Deadline Stress Detection"
  trigger:
    event: aggregated
  conditions:
    # 本周每天编码时间都在增加
    - expr: |
        history.last("7d")
          .group_by_day()
          .coding_time()
          .trend() == "Increasing"
  action:
    type: notify
    message: "本周工作强度持续上升，建议周末充分休息"
    priority: high
  risk: low
```

---

## Part 3: Rhai API 设计

### 3.1 核心 API

#### HistoryApi

```rust
/// 暴露给 Rhai 的历史查询 API
#[derive(Clone)]
pub struct HistoryApi {
    worldmodel: Arc<WorldModel>,
    cache: Arc<Mutex<HashMap<String, CachedBaseline>>>,
}

impl HistoryApi {
    /// 获取最近时间窗口的事件
    pub fn last(&self, duration: &str) -> EventCollection {
        // duration: "2h", "30m", "7d"
    }

    /// 获取 baseline 计算器（懒加载 + TTL 缓存）
    pub fn baseline(&self, metric: &str) -> BaselineApi {
        // metric: "file_changes", "coding_time", etc.
    }
}
```

#### EventCollection

```rust
/// 事件集合（支持链式调用）
pub struct EventCollection {
    events: Vec<EventApi>,
}

impl EventCollection {
    /// 过滤事件
    pub fn filter(&mut self, predicate: rhai::FnPtr) -> EventCollection;

    /// 计数
    pub fn count(&self) -> i64;

    /// 求和持续时间
    pub fn sum_duration(&self) -> Duration;

    /// 是否存在满足条件的事件
    pub fn any(&self, predicate: rhai::FnPtr) -> bool;

    /// 每小时平均值
    pub fn avg_per_hour(&self) -> f64;

    /// 趋势分析（MVP: 简单斜率判断）
    pub fn trend(&self) -> String {
        // "Increasing" | "Stable" | "Decreasing"
    }

    /// 按天分组（Phase 5.2+）
    pub fn group_by_day(&self) -> HashMap<String, EventCollection>;

    /// 文件修改事件过滤
    pub fn file_changes(&self) -> EventCollection;
}
```

#### EventApi

```rust
/// 单个事件的 API
pub struct EventApi {
    inner: DerivedEvent,
}

impl EventApi {
    // 字段访问
    pub fn activity(&self) -> String;  // "Programming", "Meeting", etc.
    pub fn duration(&self) -> Duration;

    // 辅助方法
    pub fn is_coding(&self) -> bool {
        matches!(self.inner, DerivedEvent::ActivityChanged {
            new_activity: ActivityType::Programming { .. }, ..
        })
    }

    pub fn is_idle(&self) -> bool;
    pub fn is_meeting(&self) -> bool;
}
```

#### BaselineApi

```rust
/// Baseline 计算 API（懒加载 + TTL 缓存）
pub struct BaselineApi {
    metric: String,
    cache: Arc<Mutex<HashMap<String, CachedBaseline>>>,
}

impl BaselineApi {
    /// 计算平均值（带缓存）
    pub fn avg(&self) -> f64 {
        // 1. 检查缓存（TTL = 1h）
        // 2. 若过期，重新计算（过去 7 天平均）
        // 3. 若数据不足 7 天，优雅降级到可用数据
    }
}

struct CachedBaseline {
    value: f64,
    expires_at: DateTime<Utc>,
}
```

### 3.2 辅助函数

```rust
// 注册到 Rhai Engine 的全局函数

/// 解析时间字符串为 Duration
fn duration(s: &str) -> Duration {
    // "90m" -> Duration::minutes(90)
    // "2h"  -> Duration::hours(2)
    // "7d"  -> Duration::days(7)
}

/// 扩展 Duration 的便捷方法
impl Duration {
    fn min(self) -> i64;  // 转为分钟
    fn sec(self) -> i64;  // 转为秒
}
```

---

## Part 4: 安全性设计

### 4.1 Rhai Sandbox 配置

```rust
pub fn create_sandboxed_engine() -> Engine {
    let mut engine = Engine::new();

    // 限制操作数（防止无限循环）
    engine.set_max_operations(1000);

    // 限制表达式深度（防止栈溢出）
    engine.set_max_expr_depth(10);

    // 限制函数调用深度
    engine.set_max_call_levels(5);

    // 禁用危险功能
    engine.disable_symbol("eval");  // 禁止 eval
    engine.on_print(|_| {});        // 禁止 print（防止日志污染）

    // 禁止加载外部模块
    engine.set_module_resolver(None);

    // 设置超时
    engine.set_progress_callback(Some(Box::new(|operations| {
        if operations > 1000 {
            return Some("Operation limit exceeded".into());
        }
        None
    })));

    engine
}
```

### 4.2 表达式验证

```rust
pub struct ExpressionValidator;

impl ExpressionValidator {
    /// 静态分析表达式，检测危险模式
    pub fn validate(expr: &str) -> Result<()> {
        // 1. 禁止的关键字
        let forbidden = ["eval", "import", "export", "while", "loop"];
        for kw in forbidden {
            if expr.contains(kw) {
                return Err(format!("Forbidden keyword: {}", kw));
            }
        }

        // 2. 编译测试（确保语法正确）
        let engine = create_sandboxed_engine();
        engine.compile(expr)?;

        Ok(())
    }
}
```

### 4.3 错误处理

```rust
pub enum RuleError {
    ParseError(String),          // YAML 解析失败
    CompileError(String),        // Rhai 编译失败
    RuntimeError(String),        // 表达式执行失败
    TimeoutError,                // 超时
    SafetyViolation(String),     // 安全检查失败
}

// 规则执行时的错误处理
match evaluate_condition(expr, context) {
    Ok(result) => result,
    Err(RuleError::TimeoutError) => {
        log::warn!("Rule '{}' timed out, skipping", rule.name);
        false  // 超时视为条件不满足
    }
    Err(e) => {
        log::error!("Rule '{}' error: {}", rule.name, e);
        false  // 错误视为条件不满足（安静失败）
    }
}
```

---

## Part 5: 性能优化设计

### 5.1 Baseline 缓存策略（Lazy + TTL）

```rust
pub struct BaselineCache {
    cache: Arc<Mutex<HashMap<String, CachedBaseline>>>,
    ttl: Duration,  // 默认 1 小时
}

impl BaselineCache {
    pub fn get_or_compute(
        &self,
        metric: &str,
        compute_fn: impl FnOnce() -> f64,
    ) -> f64 {
        let mut cache = self.cache.lock().unwrap();

        // 检查缓存
        if let Some(cached) = cache.get(metric) {
            if cached.expires_at > Utc::now() {
                log::debug!("Baseline cache hit: {}", metric);
                return cached.value;
            }
        }

        // 计算新值
        log::debug!("Baseline cache miss, computing: {}", metric);
        let value = compute_fn();

        // 存入缓存
        cache.insert(metric.to_string(), CachedBaseline {
            value,
            expires_at: Utc::now() + self.ttl,
        });

        value
    }
}
```

**优势**：
- ✅ 按需计算（不浪费资源计算未使用的 metric）
- ✅ TTL 缓存（1 小时内重复查询直接读缓存）
- ✅ 自动过期（确保数据新鲜度）

### 5.2 Baseline 计算优化

```rust
impl HistoryApi {
    fn calculate_baseline(&self, metric: &str) -> f64 {
        let target_window = Duration::days(7);
        let now = Utc::now();

        // 尝试获取过去 7 天的数据
        let events = self.worldmodel
            .query_events(now - target_window, now);

        // 优雅降级：如果数据不足 7 天，用可用数据
        if events.is_empty() {
            log::warn!("No historical data for baseline '{}'", metric);
            return 0.0;
        }

        let actual_window = now - events.first().unwrap().timestamp;
        if actual_window < target_window {
            log::info!(
                "Baseline '{}' using {} days (target: 7)",
                metric,
                actual_window.num_days()
            );
        }

        // 计算平均值
        match metric {
            "file_changes" => {
                let count = events.iter()
                    .filter(|e| matches!(e, DerivedEvent::FileChanged { .. }))
                    .count();
                count as f64 / actual_window.num_hours() as f64
            }
            "coding_time" => {
                let total = events.iter()
                    .filter_map(|e| match e {
                        DerivedEvent::ActivityChanged {
                            new_activity: ActivityType::Programming { .. },
                            duration,
                            ..
                        } => Some(duration),
                        _ => None,
                    })
                    .sum::<Duration>();
                total.num_minutes() as f64 / actual_window.num_hours() as f64
            }
            _ => 0.0,
        }
    }
}
```

**Cold Start 处理**：
- 数据不足 7 天时，用可用数据计算
- 完全无数据时，返回 0.0（规则中需用 `unwrap_or(0.0)` 保护）
- 日志记录实际使用的时间窗口

---

## Part 6: 实现路线图

### Phase 5.1: MVP（预计 1 周）

**目标**：验证 YAML + Rhai 架构可行性

**功能范围**：
1. ✅ YAML 解析器（支持 trigger, constraints, action）
2. ✅ Rhai Engine 沙箱配置
3. ✅ 基础 RhaiApi（HistoryApi, EventCollection, EventApi）
4. ✅ 简单 baseline（Fixed 7d window）
5. ✅ YamlPolicy 集成到 PolicyEngine
6. ✅ 3-5 个示例规则（覆盖 Simple + Complex 场景）

**不包含**：
- ❌ Smart Baseline（Phase 5.2）
- ❌ Trend Detection 高级算法
- ❌ Hot Reload（Phase 5.2）

**验收标准**：
- 用户可编辑 `~/.aleph/policies.yaml`
- 重启 Daemon 后规则生效
- 复杂规则（如 Smart Break Reminder）能正确触发

---

### Phase 5.2: 增强功能（预计 2 周）

**目标**：生产级完善

**功能范围**：
1. ✅ Smart Baseline（同一时段比较）
2. ✅ Trend Detection 算法优化
3. ✅ Hot Reload（监听 policies.yaml 变化）
4. ✅ 规则调试工具（dry-run mode）
5. ✅ 错误报告优化（友好错误提示）
6. ✅ 性能监控（规则执行耗时统计）

**验收标准**：
- Baseline 精度提升（区分工作日/周末）
- 修改规则无需重启 Daemon
- 规则错误有清晰的调试信息

---

### Phase 5.3: 高级特性（未来）

**可选扩展**：
- GUI 规则编辑器（macOS App）
- 规则模板市场（社区分享）
- A/B 测试（同一场景多规则对比）
- 机器学习辅助（自动调优阈值）

---

## Part 7: 技术选型总结

### 7.1 为什么选择 Rhai？

**备选方案对比**：

| 引擎 | 优点 | 缺点 | 结论 |
|------|------|------|------|
| **evalexpr** | 极度轻量、非图灵完备 | 表达能力太弱，无法支持 Type 3 | ❌ 不满足需求 |
| **Rhai** | Rust 语法、强大、可沙箱化 | 体积稍大（~500KB） | ✅ **最佳选择** |
| **Google CEL** | 工业标准、类型安全 | 语法冗长、社区小 | ⚠️ 备选 |
| **Lua** | 成熟生态 | 与 Rust 集成度低、语法陌生 | ❌ 不符合 Rust 生态 |

**最终选择**：Rhai (Sandbox Mode)

**理由**：
1. ✅ 表达能力足以支持 Type 1-3 所有场景
2. ✅ 类 Rust 语法，符合目标用户习惯
3. ✅ 成熟的沙箱配置（可限制操作数、深度、超时）
4. ✅ 纯 Rust 实现，无需 FFI
5. ✅ 活跃维护（2024 年仍在更新）

### 7.2 为什么不选 Rust Macro（方案 D）？

**技术上可行**：
```rust
// 用户写 Rust 代码，编译成 WASM
#[rule(risk = Low)]
fn low_battery(ctx: &Context) -> Option<Action> {
    if ctx.battery_level < 20 {
        notify!("电量低")
    }
}
```

**为什么拒绝**：
- ❌ 违背 JARVIS 易用性原则
- ❌ 修改规则需要重新编译（体验太硬核）
- ❌ 门槛太高（只为极客服务，而非大众）

**结论**：Rhai 在易用性和能力之间达到最佳平衡。

---

## Part 8: 关键设计原则回顾

### 8.1 渐进增强（Progressive Enhancement）

```yaml
# Level 1: 纯声明式（80% 场景）
- name: "Low Battery"
  trigger: { event: battery_low }
  action: { type: notify }

# Level 2: 简单表达式（15% 场景）
- name: "Overwork Alert"
  conditions:
    - expr: "coding_time > 90.min()"

# Level 3: 复杂逻辑（5% 场景）
- name: "Trend Detection"
  conditions:
    - expr: |
        history.last("7d")
          .group_by_day()
          .trend() == "Increasing"
```

用户可以从 Level 1 起步，按需升级到 Level 2/3。

### 8.2 优雅降级（Graceful Degradation）

- **数据不足**：用可用数据计算 baseline
- **表达式错误**：记录日志但不崩溃，视为条件不满足
- **超时**：限制 100ms，超时则跳过该规则

### 8.3 安全第一（Safety First）

- **非图灵完备化**：禁用 `while`/`loop`，只允许迭代器
- **操作数限制**：最多 1000 次运算
- **禁止副作用**：无 IO、无外部模块
- **静态验证**：加载时编译检查语法

### 8.4 性能至上（Performance Matters）

- **懒加载**：按需计算 baseline
- **TTL 缓存**：1 小时内复用计算结果
- **避免过早优化**：先验证需求，再优化（Phase 5.2）

---

## Part 9: 示例场景完整演示

### 场景：智能过劳保护

**需求**：
1. 检测连续编码超过 90 分钟且无休息
2. 文件修改频率异常高（>3 倍平时）
3. 本周工作强度持续上升

**规则配置**：

```yaml
- name: "Smart Overwork Protection"
  trigger:
    event: aggregated  # 定期触发（如每 10 分钟）
  conditions:
    # 条件 1: 连续编码 > 90 分钟
    - expr: |
        history.last("2h")
          .filter(|e| e.is_coding())
          .sum_duration() > duration("90m")

    # 条件 2: 无 5 分钟以上休息
    - expr: |
        !history.last("2h")
          .any(|e| e.is_idle() && e.duration() > duration("5m"))

    # 条件 3: 文件修改频率 > 3x baseline
    - expr: |
        let current = history.last("1h").file_changes().count();
        let baseline = history.baseline("file_changes").avg();
        current > baseline * 3.0

    # 条件 4: 本周趋势上升
    - expr: |
        history.last("7d")
          .group_by_day()
          .coding_time()
          .trend() == "Increasing"

  action:
    type: lock_screen
    duration: 600  # 强制休息 10 分钟
    message: "检测到严重过劳！强制休息 10 分钟。"

  risk: high  # 需要 Reconciliation 确认

  metadata:
    tags: ["health", "productivity"]
    author: "aether-community"
```

**执行流程**：

1. **Dispatcher 定期触发**（每 10 分钟）
2. **YamlPolicy 评估条件**：
   - 调用 Rhai Engine 执行 4 个表达式
   - 第 1 次查询 baseline 时计算并缓存
   - 后续 9 分钟内复用缓存
3. **所有条件满足**：
   - 提议 `lock_screen` 动作（High Risk）
4. **Dispatcher 进入 Reconciling Mode**：
   - 通过 IPC 询问用户："检测到过劳，是否立即休息？"
   - 用户确认后执行

---

## Part 10: 未来演进方向

### 10.1 Phase 6: 机器学习辅助

**自动阈值优化**：
```yaml
- name: "Auto-Tuned Break Reminder"
  conditions:
    - expr: "coding_time > auto_threshold('break_reminder')"
      # auto_threshold 通过历史数据学习最优值
```

### 10.2 Phase 7: 规则模板市场

**社区分享**：
```bash
aether install rule:pomodoro-timer
aether install rule:meeting-optimizer
```

### 10.3 Phase 8: GUI 规则编辑器

**macOS App 集成**：
- 拖拽式规则构建（生成 YAML）
- 实时预览规则效果
- 一键启用/禁用规则

---

## 总结

Phase 5 的设计完美平衡了以下目标：

| 目标 | 解决方案 |
|------|---------|
| **易用性** | YAML 配置 + 渐进增强 |
| **强大能力** | Rhai 表达式 + Fluent API |
| **安全性** | 沙箱模式 + 静态验证 |
| **性能** | 懒加载 + TTL 缓存 |
| **可扩展性** | 预留 Smart Baseline 接口 |

**核心哲学**：

> "Logic belongs to Policy, not Infrastructure."

通过将业务逻辑从 Rust 代码转移到 YAML 配置，Aleph 真正成为了用户可塑造的智能助手，而非僵化的自动化工具。

**下一步**：创建 Phase 5 实施计划（Implementation Plan）。
