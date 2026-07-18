# 用量/速率感知 Provider 路由（LiteLLM usage-based-routing 对标）

> 日期: 2026-06-07 · 状态: 已批准设计，待实施 · 分支隔离: worktree

## 1. 背景与目标

参考项目 **RouteLLM / Semantic-Router / LiteLLM / Bifrost** 中，Aleph 的动态路由栈实为两条独立链路：

- **Provider/模型路由**（RouteLLM/LiteLLM/Bifrost 对标）：`failover.rs` + `route_policy.rs` + `load_stats.rs`。已有熔断、冷却、4 种负载均衡策略（Ordered/RoundRobin/LeastBusy/LatencyAware）、本地/云分层、pin 提升、429-aware 深度重试预算。**均已连线。**
- **Agent 路由**（Semantic-Router 对标）：`a2a/service/smart_router.rs` 三层（精确名/技能/LLM 语义）+ `SemanticLlmMatcher`。**已连线。**

经扫描，这套栈高度成熟，唯一对标参考项目仍缺失且 R7 合规的能力是 **LiteLLM 的 usage-based routing**：按每 provider 的 RPM/TPM 限额，将请求路由到剩余额度最多的 provider，并对超限 provider 降权。

**本设计目标**：以"对标补能力为主"，复用现有接缝，新增用量/速率感知路由，全程无锁，零行为回归（未配限额时字节级等同今日）。

### 1.1 为何不做 RouteLLM 式成本/复杂度路由

成本/复杂度路由（强/弱模型二选一）需在调用前插入一个分类步骤。三方对比：

| 方案 | 成本 | 校准度 | 延迟 | 结论 |
|---|---|---|---|---|
| RouteLLM 训练分类器 | 极低（ms） | 高（对目标信号训练） | ~0 | 窄任务客观最强 |
| 强模型分类 | 高（省钱破产） | 高 | +1 RTT | 自相矛盾 |
| 弱模型自评 | 低 | 差（系统性过度自信） | +1 RTT | 不可靠 |

LLM 对成本/复杂度分类：相对排序尚可，绝对决策边界（弱模型能否扛下）校准差——而路由要的正是这个边界。**结论：就窄任务，训练分类器 > 强模型 > 弱模型自评。但 Aleph 不做它，输在 R10（薄 harness——不在笨循环前插认知前置层），而非 R7。** Aleph 拿成本红利的 R10 合规方式 = tier 分层 + failover + 本设计的用量路由 + 单一主模型按 think_level 自调节。若未来要做，正确形态是可选的"学习型路由信号"独立项目，非本任务一部分。

## 2. 能力映射（LiteLLM → Aleph，复用优先）

| LiteLLM 概念 | Aleph 落点 | 新增内容 |
|---|---|---|
| deployment rpm/tpm 限额 | `ModelRouteConfig.rate_limits`（名键引用 `[providers]`，同 pin 约定） | 配置字段 |
| 滑动窗口用量计数 | `ProviderLoad.window: RateWindow`（无锁 60s 滚动） | 字段+方法 |
| usage-based-routing-v2 选最低用量 | `LoadBalanceStrategy::UsageBased`（复用 `sort_by_metric`） | enum 变体 |
| 超限跳过/冷却 | `LoadMetric.over_limit` → 饱和 provider 降权至 tier 末尾（不 Skip） | 纯函数门控 |
| 热更新限额 | `RouteHandle.limits: ArcSwap<RateLimits>`（同 `targets` RCU） | 句柄字段 |
| 面板配置 | `route_config.rs` RPC + atomic-u8 `LB_USAGE_BASED=4` | 连线 |

**Rust 超越点**：限额读取（ArcSwap RCU）+ 窗口计数（relaxed atomics + 单次 CAS 翻转）全程无锁、无 await、热路径零 mutex，对比 LiteLLM 的 Python 锁/Redis 状态。

## 3. 分层设计

### 3.1 配置层 `src/config/types/route.rs`

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRateLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,   // requests / 60s
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u32>,   // tokens / 60s (input+output)
}

// ModelRouteConfig 新增：
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rate_limits: BTreeMap<String, ProviderRateLimit>,

// LoadBalanceStrategy 新增变体：
    /// 按速率窗口剩余额度最多者优先。未配 rate_limits 时退化 Ordered。
    UsageBased,
```

- `BTreeMap` 保序列化确定性；空映射 = 字节级向后兼容。
- 限额放 `[route]`，全部路由配置集中一处（与 pin 一致）。

### 3.2 无锁窗口 `src/providers/load_stats.rs`

```rust
fn now_min() -> u64 {  // 进程级单调分钟，避免墙钟 NTP 跳变
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_secs() / 60
}

#[derive(Debug, Default)]
struct RateWindow { epoch_min: AtomicU64, req: AtomicU32, tokens: AtomicU64 }

impl RateWindow {
    fn roll(&self, now: u64) {  // 跨分钟用一次 CAS 翻转；输者放行（±1 advisory 容差）
        let cur = self.epoch_min.load(Relaxed);
        if cur != now
            && self.epoch_min.compare_exchange(cur, now, Relaxed, Relaxed).is_ok()
        { self.req.store(0, Relaxed); self.tokens.store(0, Relaxed); }
    }
    fn bump_req(&self)           { self.roll(now_min()); self.req.fetch_add(1, Relaxed); }
    fn add_tokens(&self, n: u64) { self.roll(now_min()); self.tokens.fetch_add(n, Relaxed); }
    fn snapshot(&self) -> (u32, u64) {
        self.roll(now_min());
        (self.req.load(Relaxed), self.tokens.load(Relaxed))
    }
}
```

接线：`ProviderLoad` 加 `window: RateWindow`。

| 入口 | 改动 | 调用点 |
|---|---|---|
| `LoadStats::begin(name)` | 现增 in_flight 外，额外 `window.bump_req()` | failover.rs:983（已在请求路径） |
| `InFlightGuard::record_tokens(n)` 新方法 | 委托 `window.add_tokens(n)` | failover.rs:989 成功臂，旁 record_latency |
| `ProviderLoad::metric()` | 读 `(rpm_used, tpm_used)` 填进 `LoadMetric` 新字段 | route_policy 消费 |

读/写两侧都先 roll，静默 provider 不留陈旧满值。

### 3.3 热更新限额 + 编码 `src/providers/route_handle.rs`

```rust
const LB_USAGE_BASED: u8 = 4;   // 接 LB_LATENCY_AWARE=3 之后；越界仍退化 Ordered

pub struct RouteHandle {
    mode: AtomicU8, allow_escalation: AtomicBool, load_balance: AtomicU8,
    targets: ArcSwap<RouteTargets>,
    limits: ArcSwap<RateLimits>,   // 新增，同 targets RCU 热更新
}
// from_config/store 各加一行 limits；新 reader limits() -> Arc<RateLimits>
```

`RateLimits`（定义在 `route_policy.rs`，拥有自己的输入类型，不让 route_policy 反依赖 config）：

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimits { by_provider: BTreeMap<String, (Option<u32>, Option<u32>)> }
impl RateLimits {
    pub fn from_config(cfg: &ModelRouteConfig) -> Self { /* 折叠 cfg.rate_limits */ }
    pub fn is_empty(&self) -> bool { self.by_provider.is_empty() }
    /// 返回 (利用率千分比, 是否超限)：取 rpm/tpm 两维利用率最大者，无限额维记 0。
    pub fn assess(&self, name: &str, rpm_used: u32, tpm_used: u64) -> (u16, bool) { /* ... */ }
}
```

`assess()` 把限额折进两个标量，使 route_policy 排序函数仍对限额无知（纯 infra，R7 保持）。

### 3.4 核心算法 `src/providers/route_policy.rs`

```rust
pub struct LoadMetric {
    pub in_flight: usize, pub latency_us: u64,
    pub utilization_permille: u16,  // 新增
    pub over_limit: bool,           // 新增
}

// balance_group 新分支：
LoadBalanceStrategy::UsageBased =>
    sort_by_metric(group, metric_of, name_of, |m| m.utilization_permille as u64),
```

**over_limit 策略无关前置门控**：在 `order_candidates_balanced` 内 same_tier 分桶后、balance 前按 over_limit 稳定分区：

```rust
let (fresh, saturated): (Vec<_>, Vec<_>) =
    same_tier.into_iter().partition(|(c, _)| !metric_of(name_of(c)).over_limit);
// pin 提升 + balance 仅作用 fresh；saturated 原序追加在 balance 后、crossings 前。
```

最终排序优先级：**pin > 未饱和(按策略) > 饱和(原序) > 跨 tier crossings**。

- over_limit 门控仅在 `!limits.is_empty()` 时启用；空限额跳过分区 ⇒ 字节级等同今日。
- route_policy 仍纯函数、限额无知，只读 `LoadMetric` 标量。

### 3.5 failover 接线 + gateway RPC `src/providers/failover.rs`, `src/gateway/handlers/route_config.rs`

```rust
// candidates(): 取 limits 快照，metric_of 闭包折算 utilization/over_limit
let limits = self.route_limits();
let metric_of = |name: &str| {
    let mut m = load.metric(name);
    let (util, over) = limits.assess(name, m.rpm_used, m.tpm_used);
    m.utilization_permille = util; m.over_limit = over; m
};
// balanced 路径触发条件扩展：UsageBased || !limits.is_empty()（需 over_limit 门控）

// 成功臂 failover.rs:989 旁 +1 行：
g.record_tokens(resp.usage.input_tokens as u64 + resp.usage.output_tokens as u64);
```

gateway：`lb_to_str`/`lb_from_str` 增 `"usage_based"`；RPC payload 增 `rate_limits` 读写（面板表单）。
route_handle：新 `route_limits()` reader（boot 无 handle 时回退 boot 快照，同 `route_targets()`）。

## 4. 熵减清理

- `metric()` 4 字段构造一处到位，无遗留双写；`LoadMetric` 新字段走 `#[derive(Default)]`，现有 `LoadMetric::default()` 调用点零改动。
- `record_tokens` / `RateWindow` / `RateLimits` 均有真实消费者（failover + route_policy），不引入死代码、不为未来留口（YAGNI）。

## 5. 测试矩阵

- **config**: rate_limits TOML/JSON round-trip + 空映射省略；`UsageBased` snake_case `"usage_based"`；旧配置缺字段默认入。
- **RateWindow**: 翻转清零、单调时钟、CAS 竞争下 ±1 容差、snapshot 读侧 roll。
- **RouteHandle**: 限额热更新（store 后 limits() 立即可见）；`u8_to_lb(4)=UsageBased`、越界退化 Ordered。
- **route_policy**: UsageBased 选最低利用率；over_limit 降权至末尾；饱和+pin（pin 压过门控）；空限额恒等。
- **failover 集成**: token 回喂 → TPM 累加 → over_limit 降权；未配限额走原 Ordered 路径字节级一致。

## 6. 红线合规

- **R7 LLM 主权**: route_policy 纯 infra，只读 prompt-blind 硬信号（运行时计数 + 操作员限额），从不看消息/意图。
- **R10 薄 harness**: 不触 `src/harness/`；负载/路由全在 providers 层。
- **P1/P4 低耦合/依赖倒置**: `RateLimits` 输入类型属 route_policy，不反依赖 config。
- **P6 简洁/YAGNI**: 复用 LoadStats/ArcSwap/sort_by_metric/atomic-u8 既有接缝，零新基础设施。
- **向后兼容**: 空 rate_limits + 非 UsageBased = 字节级等同今日 failover。

## 7. 约束（用户指令）

- 实施在新建 worktree 分支，严禁触 main。
- 完成后**不**做 cargo check/测试校验（用户强制约束），直接提交。
