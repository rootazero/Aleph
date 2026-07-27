# Model Catalog — 预设 Provider 与模型参考数据

> 对应 [FEATURE_LOCATOR.md §5.4](FEATURE_LOCATOR.md)。本文是**模型参考数据层的契约文档**：四张表怎么分工、怎么 join、什么时候允许陈旧、漂移由谁守。
> 内含 **opencode / kimi-cli 对照表（Gap Analysis）**——**改这一层之前先看那张表，不必重做一遍对比**。

---

## 1. 问题陈述

Aleph 的模型参考数据是**编译期静态表**：升级二进制才更新。这是刻意的（R3 核心轻量化：不为低信号功能引入远端依赖），但它有一个必然后果——**表会漂移，而且是各自独立地漂移**。

漂移不是假设。2026-07-17 的刷新轮记录得很清楚：pricing 已经到 V4/K2.6/GLM-5.x，registry 的默认模型还停在上一代；Doubao 在 `capabilities` / `pricing` / `canonical_provider_id` / `infer_vendor` 四处**全无条目**；deepseek-v4-pro 曾超收约 4 倍。那一轮靠人工把四张表并排读了一遍。

2026-07-25 这一轮的判断是：**并排读表这个动作本身应该是代码**。

---

## 2. 四张表 + 一个 join 点

| 表 | 位置 | 回答的问题 | 缺失时的后果 |
|----|------|-----------|-------------|
| Presets | `src/providers/presets/registry.rs` | 每个 provider 默认用哪个模型、备选链、廉价 aux 档 | 开箱不可用 |
| Capabilities | `src/providers/model_catalog/capabilities.rs` | 窗口 / max-output / vision / tools / reasoning | 回落 `CONSERVATIVE_CONTEXT_WINDOW` (128K) ⇒ **过早压缩**（§2.2 消费方 `derive_token_budget`） |
| Pricing | `src/pricing.rs` | USD/Mtok（含长上下文 tier） | `CostStatus::Unknown`；`cost_aware` 路由按 `unpriced_cost(tier)` 排 |
| Lifecycle | `src/providers/model_catalog/lifecycle.rs` | 厂商还在不在服务这个 id | 模型被推荐一个已下线 id ⇒ 下一轮不透明 400 |

**唯一 join 点：`src/providers/model_catalog/record.rs::ModelRecord::resolve(provider, model, base_url, source)`**

```
ModelRecord {
    provider, model,
    capabilities: Option<ModelCapabilities>,   // None = 未记录，不是零
    cost:         Option<RateCard>,            // None = 未记录，不是免费
    endpoint:     EndpointKind,                // Local | Cloud
    lifecycle:    ModelLifecycle,              // 永不为 None（Active 是默认答案）
    source:       ModelSource,                 // 调用方提供的来源标注
}
```

### 为什么必须是一个点

重构前，`capabilities_for` + `rate_card` + `endpoint_kind_for_base_url` 这个三元 join 被**手抄了三处**：`builtin_tools/list_models.rs::enrich`、`gateway/handlers/providers/handlers.rs` 的 preset 行、同文件的 custom provider 行。加第四个维度（本轮的 lifecycle）会接上其中两处、漏掉第三处——而漏掉的那处不会报错，只会少一个字段。

（`route_observe::price_milli_per_mtok` 与 `failover::price_hint` 只读 `pricing` 一张表，并且应当保持如此：它们要的是一个用来排序的标量，不是一条用来展示的记录。）

这是 opencode 用**有序插件**（`modelsDev(0) → env(10) → account(20) → provider(30) → config(40) → discovery(50)`，每层 mutate 同一份 draft）买到的性质。Aleph 只取那个不变量（"只有一个函数知道记录怎么组装"），不引入插件总线——四张静态表不需要一条消息总线。

### `ModelSource`：来源不是从 id 推得出来的

`source` 由枚举方提供，不由 `resolve` 推断：

| 值 | 含义 |
|----|------|
| `preset_default` | preset 的 `default_model` |
| `preset_fallback` | preset 策展的 `fallback_models` 之一 |
| `preset_aux` | preset 的廉价 `default_aux_model` |
| `configured` | operator 写在 `[providers.<id>] models` 里的 |
| `discovered` | 刚才从 provider 的 `/models` 拿到的 |

两类受众都需要它：operator 想知道这一行是自己配的还是 Aleph 建议的；LLM 应该能区分"厂商策展的备选"和"从 live 端点扒下来的裸 id"。

---

## 3. 生命周期契约 (`lifecycle.rs`)

```rust
enum ModelStatus { Active, Preview, Deprecated }
struct ModelLifecycle { status, successor: Option<&str>, note: Option<&str> }
```

**解析顺序**：显式 `LIFECYCLE_TABLE` ▸ id 的 preview 后缀（`-preview` / `-exp` / `-experimental` / `-beta` / `-alpha` / `-rc`）▸ `ACTIVE`。

设计要点：

- **兄弟表，不是新字段**。只放非 `Active` 行，`CAPABILITY_TABLE` 的 58 条字面量零改动。同款 pattern：`pricing::TIER_TABLE` 傍 `PRICE_TABLE`。
- **preview 是词法派生，不进表**。厂商在 id 里自己写了 `-preview`；这跟剥尾部日期戳同一类（关于命名的事实），不是对模型的推断。
- **不返回 `Option`**。"没记录"和"正常在服务"对每个消费者都是同一个答案，`Option` 只会把这个塌缩推给五个调用点。

### 消费面

| 面 | 行为 |
|----|------|
| `select_model` | 弃用 id **硬拒**，消息里报 `successor` |
| `list_models` | 每行带 `status` / `successor` / `status_note`，汇总行提示有几个弃用 id |
| `providers.catalog` RPC | `lifecycle` 字段（默认模型的） |
| Panel picker | 弃用的默认打 `retired` 角标，tooltip 给 successor |
| 漂移守卫 | 任何 preset 的默认或 fallback 命中弃用 id ⇒ **编译期测试失败** |

最后一条是这张表最大的价值：`deepseek-chat` 于 2026-07-24 下线，当时的唯一发现途径是人注意到。现在它是一个失败的测试。

---

## 4. 定价契约 (`pricing.rs`)

### 4.1 `RateBasis` — 为什么聚合器以前恒 `Unknown`

`PRICE_TABLE` **按 vendor 分节**（anthropic / openai / google / deepseek / xai / mistral / moonshot / zai / qwen / minimax / doubao 共 11 节），但查表**按 provider id 门控**。于是所有"不是厂商自己在服务"的 provider——openrouter、groq、together、fireworks、deepinfra、siliconflow、novita、chutes、github-copilot、**amazon-bedrock**——`canonical_provider_id` 一律返 `None`，恒 `CostStatus::Unknown`。

叠加 2026-06-26 引入的 `unpriced_cost(Cloud) = u64::MAX`（该条本身正确：成本未知 ≠ 零），结果是 **`cost_aware` 路由把常常最便宜的那一档一律排在最后**。路由语义被静默反转了一个月。

> **第三次反转，同一个函数（2026-07-27，修在 §3.6 那一轮）**：价表这次是对的，错的是喂给 `unpriced_cost` 的 **tier**。默认（auto-derived）失败链的候选是**运行时派生**的，而派生路径没有节点可抄 tier，一律填 `EndpointTier::Unknown` ⇒ 走 `u64::MAX` 分支 ⇒ 真正免费的自托管端点排最后。教训与前两次同形：**`unpriced_cost` 的正确性完全取决于它的输入，而这个输入在两条装配路径上是分别产生的**——静态链从 `node_for` 拿，live 链现在从 `with_tier_catalog` 拿同一份 `provider_tier` 派生。给这个函数加分支之前，先确认**每一条**能到达它的路径都供得出真 tier。

现在 `lookup_rates` 两趟：

1. **Direct** — provider id 归一到某个有价的 vendor，且该节有匹配行。
2. **VendorInferred** — 否则按**模型 id 自己的 vendor**（`infer_vendor`）回退。

`RateBasis` 随 `RateCard` / `CostEstimate` / `list_models.price_basis` 一路暴露，转售溢价**可见**而不是被当成报价。`CostEstimate.basis` 是 `#[serde(default)]`，旧的 run summary 照常反序列化。

### 4.2 刻意不定价：开放权重

`groq` / `together` / `fireworks` 上的 Llama **仍然 unpriced**，并且这条限制被测试钉死。理由：Meta 不卖 Llama 推理，各宿主价差极大（Groq / Together / Cerebras / Fireworks 全不一样），编一个"Meta 价"比 `Unknown` 更糟——`Unknown` 至少是诚实的，且 `unpriced_cost` 仍按 endpoint tier 给它一个位置。

### 4.3 长上下文 tier

`TIER_TABLE` 在**输入 token 轴**上覆盖基础价。当前覆盖：`gemini-3.1-pro`、`gemini-2.5-pro`、`claude-sonnet-5`、`claude-sonnet-4`（均 >200K 阈值）。

**刻意不做**：`claude-opus-4-6/7/8` 与 `claude-fable-5` 同样是 1M 窗口，但其 >200K 倍率没有可核实的公开值。把 Sonnet 的 2x/1.5x 外推上去是**编数据**；它们保持平价，并在表内注明这是一个已知的低估。

---

## 5. id 归一 (`alias.rs::canonicalize_model_id`)

顺序：剥已知 vendor tag（循环，处理嵌套）→ **剩余 host 路径折成末段** → 剥 `:tag` → 剥尾部 8 位日期戳。

第二步是本轮新增的。`VENDOR_TAGS` 只认识它被写下来时存在的 tag，而宿主一直在发明新形状：`deepseek-ai/…`、`accounts/fireworks/models/…`、`@cf/meta/…`。每一种没列进去的形状都**同时**落空能力表（⇒ 保守 128K ⇒ 过早压缩）和价格表（⇒ Unknown ⇒ `u64::MAX`）。折末段是把固定表泛化，而不是继续追着它补。

> **安全边界**：该函数的产物**只做查表 key**，永远不回到线上。出站请求始终携带 operator 的原始 model id，所以折叠 host 路径不可能把请求发错地方。

---

## 6. 实时发现 (`discovery.rs`)

### 6.1 为什么是 CONNECT 而不是 CUT

`ProviderPreset::models_url`、`ProviderPreset::resolve_models_url()`、`ProviderPreset::supports_health_check` 三件脚手架在本轮之前**生产零消费者**（各一个单测）。R10 的 YAGNI 撤回条款要求这种情况立即 CUT 或 CONNECT。选 CONNECT，因为聚合器/中继的默认模型陈旧问题**没有静态解**——往表里塞更多猜测只是把下一次漂移推后。

### 6.2 为什么取 kimi-cli 的形状而非 opencode 的

opencode 拉 [models.dev](https://models.dev) 的全量目录（磁盘缓存 + TTL + 60min 后台刷新 + 跨进程 flock + 编译期快照兜底）。数据更全，但**会让一个第三方服务成为核心子系统的 load-bearing 依赖**——违 R3。

kimi-cli 问每个已配置平台 `GET {base_url}/models`。数据少（基本只有 id），但**复用 Aleph 已经持有的凭据与端点**，没有新的信任面。Aleph 取后者。

### 6.3 协议

| 项 | 值 |
|----|-----|
| URL | preset 的 `models_url` 覆盖（**仅当 `base_url` 未被 operator 改动**）否则 `{base_url}/models` |
| 认证头 | `anthropic` → `x-api-key` + `anthropic-version`；`gemini` → `x-goog-api-key`；其余 → `Authorization: Bearer` |
| 响应形状 | `data[]`（OpenAI / Anthropic）或 `models[]`（Gemini / Ollama 原生） |
| 字段 | id ← `id` \| `name`；display ← `display_name` \| `displayName`；窗口 ← `context_length` \| `context_window` \| `max_context_length` \| `inputTokenLimit` |
| 超时 | 10s（请求 + body 各一次），共享 provider client 但**加了整体超时**（该 client 为流式刻意不设） |
| body 上限 | 1 MiB |
| 缓存 | `~/.aleph/cache/models/<provider>.json`，temp+rename 原子写，TTL 300s |

`models_url` 覆盖的守卫是必要的：那是一个**绝对 URL**，套到被搬走的端点上（Azure 资源 / 企业中继 / 本地代理）会把探测发到厂商而不是配置的主机。

### 6.4 触发面（两个，共用同一个 leaf）

| 入口 | 受众 | 行为 |
|------|------|------|
| `list_models { refresh: true }` | LLM（R8） | 先看缓存 TTL；仅对已过期的 provider 发起请求 |
| `providers.modelsRefresh` RPC | operator / Panel | **强制**真拉；可选 `provider` 参数收窄到一个 |

`list_models` 走 TTL 是因为它对 chat-tier 开放：无条件拉取等于给了"反复调用即刷屏打厂商"的手柄。operator RPC 是显式授权的"现在就去看"，不受 TTL 约束。

扇出用 `tokio::task::JoinSet` **并发**（两个参考项目都是顺序 for 循环）。单个 provider 失败**不冒泡**——发现是对一个已经能工作的目录做增强，一家厂商不可达不该让整次调用失败；失败计数进 `list_models` 的 `message`，细节进 `tracing::debug`。

### 6.5 边界

- **只贡献 id**。窗口 / 价格 / 生命周期仍归策展表——`/models` 响应基本不带这些。一个没有策展行的 discovered id 诚实地显示为"能力未知"，跟今天的自定义中继模型一样。
- **无后台定时器**。悄悄按时给每个已配置厂商打电话是意外行为，不是功能。
- **`supports_health_check = false` 的 preset 直接拒**（OAuth-only 端点 / 按部署的 Azure 资源 / 分区域云都会 404 或限流列表路由）。

---

## 7. 漂移守卫 (`drift_tests.rs`)

本轮真正的交付物。它把"人工并排读四张表"写成了测试。

| 守卫 | 抓什么 |
|------|--------|
| `every_preset_declares_a_default_model` | 空默认值且未声明 `requires_explicit_model()` |
| `byo_model_presets_do_not_also_ship_a_default` | 自相矛盾的 BYO 声明 |
| `no_preset_defaults_to_a_retired_model` | **`deepseek-chat` 那一类** |
| `no_preset_lists_a_retired_fallback` | 备选链里的死 id（白烧一次重试） |
| `fallback_chain_leads_with_the_default_model` | 改了默认忘了改链（新旗舰被降为第二选择） |
| `advertised_models_have_capability_rows` | 缺能力行 ⇒ 128K 保守窗口 ⇒ 过早压缩 |
| `advertised_models_of_priced_vendors_have_rates` | 已定价 vendor 的新模型没补价格行 |
| `aux_model_is_not_pricier_than_the_default` | 廉价档比主模型贵（**抓出了 grok-3-mini 的 5 倍超收**） |
| `declared_aliases_resolve_to_their_profile` | 别名展开断裂 |
| `exemptions_still_name_something_real` | 豁免比它豁免的东西活得久 |

另有两个 **prefix-shadow 守卫**（在 `pricing.rs` / `capabilities.rs` 各自的测试模块里，因为表是私有的）：查表是"首个前缀命中者胜"，一条广义行插在特化行之前会让后者**永远不可达**——它照常编译、读起来也对，只是从不执行。

### 怎么加豁免

两张显式清单，都带理由，且被 `exemptions_still_name_something_real` 钉住：

- `UNCATALOGUED_FAMILIES` — 按 preset id。"这家的窗口我们没有可核实的数据"。删一条是进步；加一条要写清为什么核实不了。这张表本身就是"哪些 provider 在 picker 里不会显示上下文窗口，以及为什么"的答案。
- `ENDPOINT_LOCAL_ALIASES` — 按模型 id。只存在于某一个厂商端点、没有公开能力/价格数据的别名（当前只有 `k2p5`）。

---

## 8. opencode / kimi-cli 对照表 (Gap Analysis)

> **改这一层之前先看这张表**。逐维度标注「映射 / 对齐 / 超越 / 有意不移植」。

| 维度 | opencode | kimi-cli | Aleph | 裁决 |
|------|----------|----------|-------|------|
| 目录数据源 | models.dev 远端 JSON（5min TTL 磁盘缓存 + 60min 后台刷新 + 跨进程 flock + 编译期快照兜底 + env 覆盖源） | 每平台 `{base_url}/models` 实时拉，`/model` 触发写回 config `managed:` 命名空间 | 编译期四张静态表 + **按需** `/models` 发现 | **有意不移植** models.dev（R3）；**映射** kimi-cli 的按需拉取 |
| 目录合成 | 有序插件层，逐层 mutate draft | `managed:` 命名空间隔离托管/自定义 | `ModelRecord::resolve` 单一 join 点 | **映射不变量**，不映射机制 |
| 模型生命周期 | `status: alpha/beta/deprecated/active`，`available()` 排除 deprecated | 刷新时删下线 id，默认回退列表首个 | `ModelStatus{Active,Preview,Deprecated}` + successor + note | **映射**；Aleph 额外把它接到 `select_model` 硬拒与漂移守卫上 |
| 价格结构 | `cost[].tier{type:context,size}` + `context_over_200k` + cache read/write | 不做 | `PRICE_TABLE` + `TIER_TABLE`，另有 reasoning 独立费率 | **超越**（结构更细） |
| 价格覆盖 | models.dev 全量 | n/a | 11 个 vendor 分节 + `RateBasis::VendorInferred` 回退 | **对齐**（覆盖面靠回退而非全量表） |
| 转售/聚合器定价 | 由 models.dev 的 provider-scoped 条目天然覆盖 | 不做 | vendor-inferred 回退 + `basis` 标注 | **超越**（区分了"厂商报价"与"按 vendor 推断"，opencode 不区分） |
| 模型 id 归一 | provider-scoped 存储（id 只在 provider 内唯一） | 原样 | 剥 tag → 折 host 路径末段 → 剥 `:tag` → 剥日期戳 | **对齐**（用归一化取代 provider-scope 的存储分层） |
| 能力矩阵 | tools + modalities in/out 数组 | context_length / reasoning / image_in / video_in | window / max-out / vision / tools / reasoning | **对齐**；模态数组是**尚未闭合**项 |
| 发布日期 | `time.released` | 无 | 无 | **尚未闭合**（低信号：目前没有消费者需要按发布日期排序） |
| 选模校验 | branded ID + `NotFound` | `default_model` 必须存在于 models 表（config 层校验） | 弃用硬拒 + 未知放行带 caveat | **对齐**；未知放行是刻意的（策展表永远落后厂商，拒绝未知＝拒绝所有新模型） |
| 模型可见面 | `catalog.model.available()` | `/model` 列出全部刷新后的模型 | `list_models` 给 default + fallback 链 + aux + configured + discovered | **对齐** |
| 漂移防护 | 单一远端源，结构上无从漂移 | 每次刷新覆盖托管命名空间 | `drift_tests.rs` 十条交叉守卫 + 两条 prefix-shadow 守卫 | **超越**（静态表 + 编译期守卫，是"不引远端依赖"的对价） |
| 并发 | 单次 models.dev 拉取 | 顺序 for 循环逐平台 | `JoinSet` 并发扇出 | **超越** |

---

## 9. 刻意不做清单

改这一层之前请先读；这些都是评估过的决定，不是遗漏。

- **拉 models.dev 全量目录** — R3：不让第三方服务成为核心子系统的 load-bearing 依赖。
- **后台定时刷新** — 悄悄常驻的网络行为是意外，不是功能。触发面刻意保持两个显式入口。
- **给开放权重宿主编"Meta 价"** — Meta 不卖 Llama 推理，各宿主价差极大。保持 unpriced，并在测试里把这条限制钉死。
- **外推 opus-4.6+/fable-5 的 1M tier** — 无可核实的公开倍率，外推属于编数据。
- **按记忆重猜 `github-copilot` / `azure-*` / `siliconflow` 等的默认 id** — 聚合器/中继陈旧问题的正解是 discovery，不是往表里塞更多猜测。
- **`select_model` 拒绝未知模型 id** — 策展表永远落后厂商，自定义中继用自己的别名；拒绝未知等于拒绝所有比二进制新的模型。只硬拒**空**与**已知已下线**两种。
- **让 discovery 覆盖窗口/价格** — `/models` 响应基本不带这些；用它去覆盖策展数据是拿低质量数据换高质量数据。
- **给 `providers.modelsRefresh` 单独加 RPC 权限层** — Gateway 的 RPC 面没有 per-method tier（授权即 operator，见 `method_authz.rs` 的模块注释）；它和 `providers.catalog` / `providers.healthcheck` 同在一道墙后。

---

## 10. 常见修改的落点

| 想做的事 | 改哪儿 |
|---------|--------|
| 加/改预设别名 | `presets/registry.rs` |
| 某模型支不支持 vision/tool-use | `model_catalog/capabilities.rs` + `capability_gate.rs` |
| 本地还是云端 | `model_catalog/endpoint.rs` |
| 成本 | `pricing.rs`（`RateCard` = picker 的费率投影） |
| 按成本路由 | `[route] load_balance = "cost_aware"`；连线点 `failover/provider.rs::price_hint`，sort 在 `route_policy::balance_group`。**候选的 tier 从 `with_tier_catalog` 来**（2026-07-27）——此前 live 派生的候选一律 `Unknown`，`unpriced_cost` 因此把**免费本地端点排最后**，与本表第二次修复的方向正好相反 |
| **给模型记录加一个新维度** | **`model_catalog/record.rs::resolve` 一处** |
| **某模型被厂商下线** | **`lifecycle.rs::LIFECYCLE_TABLE` 加一行（带 successor）** |
| **默认模型过期了？** | **先跑 `cargo test -p alephcore --lib drift_tests`** |
| **要一个表里没有的新模型** | **`list_models { refresh: true }` 或 `providers.modelsRefresh`** |
| 子代理 / MoA 扇出跨厂商 | `provider/model` 限定名；消费点 `agents/runtime.rs::resolve_spawn_route`（见 §4.5 round-7）；主循环侧 `thinker/mod.rs::resolve_model_to_provider_and_model`。两者都用"前缀须命中已配置 provider 才剥离"的守卫，**别改成无守卫剥离** |
