# Model Catalog — 预设 Provider 与模型参考数据

> 对应 [FEATURE_LOCATOR.md §5.4](FEATURE_LOCATOR.md)。本文是**模型参考数据层的契约文档**：四张表怎么分工、怎么 join、什么时候允许陈旧、漂移由谁守。
> 内含 **openclaw / opencode / kimi-cli / pi 对照表（Gap Analysis）**——**改这一层之前先看那张表，不必重做一遍对比**。
>
> ⚠️ **本文只回答「这个模型是什么」，不回答「这一轮用哪个模型」**。后者只有一个决定点：
> `src/orchestrator/harness_bridge/runner_impl.rs::effective_model_directive`
> —— **per-turn pick（Panel 模型 pill / `[voice]` 钉，经 `FlowRequest.model_directive`）▸
> session `select_model` pick ▸ agent `model_hint` ▸ `BrainRef` preset**。
> 新增任何模型来源都必须进那个函数：Panel 的模型 pill 曾经完整地产出到 `RunRequest.model_override`
> 却从未抵达绑定点，只到达附件的 vision 判断与 `ModelResolved` 横幅——**UI 确认了切换，作答的仍是旧模型**
> （详见 [FEATURE_LOCATOR §3.6](FEATURE_LOCATOR.md) round-2）。

---

## 1. 问题陈述

Aleph 的模型参考数据是**编译期静态表**：升级二进制才更新。这是刻意的（R3 核心轻量化：不为低信号功能引入远端依赖），但它有一个必然后果——**表会漂移，而且是各自独立地漂移**。

漂移不是假设。2026-07-17 的刷新轮记录得很清楚：pricing 已经到 V4/K2.6/GLM-5.x，registry 的默认模型还停在上一代；Doubao 在 `capabilities` / `pricing` / `canonical_provider_id` / `infer_vendor` 四处**全无条目**；deepseek-v4-pro 曾超收约 4 倍。那一轮靠人工把四张表并排读了一遍。

2026-07-25 这一轮的判断是：**并排读表这个动作本身应该是代码**。

---

## 2. 四张表 + 一个 join 点

| 表 | 位置 | 回答的问题 | 缺失时的后果 |
|----|------|-----------|-------------|
| Presets | `src/providers/presets/registry.rs` | 每个 provider 默认用哪个模型、备选链、廉价 aux 档 | 开箱不可用。`fallback_models` 同时是 picker roster、`list_models` roster **与 failover 的模型游走梯**（**`presets::model_ladder` 唯一合并点**：failover 以 operator `models` 为 base、catalog `roster` 字段以 operator `models`（空则默认）为 base，把 operator 未列出的档位接在后面；operator 改过 `base_url` 则不接） |
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
struct LifecycleRow { provider: Option<&str>, prefix: &str, life: ModelLifecycle }
```

**解析顺序**：本 provider 的 scoped 行 ▸ 厂商级（`provider: None`）行 ▸ id 的 preview 后缀（`-preview` / `-exp` / `-experimental` / `-beta` / `-alpha` / `-rc`）▸ `ACTIVE`。

设计要点：

- **兄弟表，不是新字段**。只放非 `Active` 行，`CAPABILITY_TABLE` 的字面量零改动。同款 pattern：`pricing::TIER_TABLE` 傍 `PRICE_TABLE`。
- **preview 是词法派生，不进表**。厂商在 id 里自己写了 `-preview`；这跟剥尾部日期戳同一类（关于命名的事实），不是对模型的推断。
- **不返回 `Option`**。"没记录"和"正常在服务"对每个消费者都是同一个答案，`Option` 只会把这个塌缩推给五个调用点。

### 3.1 退役是有作用域的 (2026-08 round-2)

> **这张表长期只有 2 行，原因不是厂商不下线模型，而是它知道的退役里有一半"说不出口"。**

`llama-3.3-70b-versatile` 在 **Groq 上**退役、在 Together / Cerebras / DeepInfra 上活得好好的；`deepseek-v3` 从 DeepSeek 自家 API 消失、在所有托管开放权重的地方仍在服务。只按 model id 记，等于在"记下一条真事实顺带记下一条假的"（全局行 ⇒ `select_model` 的硬拒会拒掉能用的 id）和"什么都不记"（守卫恒空转）之间二选一。**两个都不是答案。**

于是行带一个可选 `provider` scope：

| scope | 含义 | 判据 |
|-------|------|------|
| `None` | **厂商自己的话** —— 这个模型在任何地方都不该再用 | 该 vendor 自家 catalog 标了 deprecated |
| `Some(preset_id)` | **某一家宿主对自己目录的话** | 只有转售商 catalog 标了 deprecated |

scoped 行**先查**，所以一家宿主可以早于厂商退役某个 id 而不与全局行打架。

反例同样重要：**三家转售商**（Novita / NVIDIA / opencode）把 `minimax-m2.7` 标为 deprecated，而 **MiniMax 自己的文档仍把它列为在售**——所以表里没有这一行。若按转售商的话写成全局行，`select_model` 会拒掉 `minimax` preset **自己的 aux 模型**。

匹配是**精确比较** provider id（不走别名、不做 substring），由 `lifecycle_scopes_name_a_real_preset` 守卫兜底：scope 打错字 ⇒ 那一行**永远不会触发**，而且是静默的。

### 3.2 provider 必须一路带到查询点

`lifecycle_for(provider, model)` 的四个调用点全都手里有 provider——但 `select_model::refuse_unusable_model` 曾经把它丢掉（同一个函数体两行之后的 `caveat_for_model` 又用了它）。丢掉的后果不是少一个字段，而是**这一类退役整个查不出来**：给 Groq 钉一个必定 404 的 llama id 会一路放行。

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

同理**刻意不定价：订阅制端点**。Kimi Code（`api.kimi.com/coding`）按套餐额度计费，其文档给的是**消耗倍率**（highspeed 3x）而不是费率——**没有一个 USD/Mtok 数字可记**。开放平台的 `kimi-k3` 按 token 计费，正常定价。

⚠️ **"不写 rate 行"不等于"不定价"**。`PRICE_TABLE` 是前缀扫描：它能说"这个 id 值 $X"，说不出"这个 id 根本没有单价"。`k3` / `k3-256k` / `k2p5` 恰好不匹配任何前缀，于是看起来没问题；而 `kimi-for-coding` / `kimi-for-coding-highspeed` / `kimi-code` 以 `kimi` 开头，**落进开放平台的 K2 时代家族兜底行，被报成 $0.60/$2.50** ——一个订阅用户根本不会被收的价。前缀表表达不了否定，所以这条事实只能显式写出来：单一源 `pricing::QUOTA_BILLED_MODELS`，在两条查找路（provider-keyed 与 vendor-inferred）**之前**短路。**按整串精确匹配**，所以永远不会遮蔽开放平台的 `kimi-k3` / `kimi-k2.6`。

两个配套结论：① 该端点的模型报 `CostStatus::Unknown` → failover 层映射成 `u64::MAX` "unpriced cloud" 哨兵 → `cost_aware` 把它们排在**最后**。对一个报不出价的成本，这是诚实的排序，且在这份清单存在之前就已经是它默认模型 `k3` 的处境。② **配额端点不需要逐 id 的价格豁免**：整个 preset 一个模型都不定价 ⇒ `advertised_models_of_priced_vendors_have_rates` 的 `vendor_is_priced` 自然为假、整条跳过。真留了逐 id 豁免，反而会让将来**误加**到 Kimi Code 上的一个 rate 静默通过——所以 `ENDPOINT_LOCAL_ALIASES` 现在只管 capability（仅剩 `k2p5`，厂商没公布窗口）。

### 4.3 长上下文 tier

`TIER_TABLE` 在**输入 token 轴**上覆盖基础价。当前覆盖：`gemini-3.1-pro`、`gemini-2.5-pro`、`claude-sonnet-5`、`claude-sonnet-4`（均 >200K 阈值）。

**刻意不做**：`claude-opus-4-6/7/8` 与 `claude-fable-5` 同样是 1M 窗口，但其 >200K 倍率没有可核实的公开值。把 Sonnet 的 2x/1.5x 外推上去是**编数据**；它们保持平价，并在表内注明这是一个已知的低估。

`kimi-k3` 也是 1M 窗口但**同样不设 tier，理由相反**：厂商明确说明 >200K 不加价，费率在整个 1,048,576 窗口内是平的。两种"没有 tier"要分清——一种是查不到，一种是查到了且确实没有。

---

## 5. id 归一 (`alias.rs::canonicalize_model_id`)

顺序：剥已知 vendor tag（循环，处理嵌套）→ **剩余 host 路径折成末段** → 剥 `:tag` → 剥尾部 8 位日期戳 → **`<数字>p<数字>` 还原为 `<数字>.<数字>`**。

第二步是 2026-07-25 那一轮新增的。`VENDOR_TAGS` 只认识它被写下来时存在的 tag，而宿主一直在发明新形状：`deepseek-ai/…`、`accounts/fireworks/models/…`、`@cf/meta/…`。每一种没列进去的形状都**同时**落空能力表（⇒ 保守 128K ⇒ 过早压缩）和价格表（⇒ Unknown ⇒ `u64::MAX`）。折末段是把固定表泛化，而不是继续追着它补。

最后一步是 2026-08 round-2 新增的：**Fireworks 把版本分隔符写成 `p`**（id 同时是 URL path 段），`kimi-k2p6` 就是 Kimi K2.6、`glm-5p2-fast` 就是 GLM-5.2 Fast。这和折 host 路径是同一类事实（某一家宿主怎么拼这个 id），后果也同一类：`glm-5p2-fast` 曾越过 `glm-5.2` 掉进 GLM-4 家族兜底（**约三分之一的真实价格**），`kimi-k2p6` 越过 `kimi-k2.6` 掉进 Moonshot 旧价。只在**两个数字之间**触发，所以 `-pro` / `gpt-oss` / `-preview` 一律不动。

> **安全边界**：该函数的产物**只做查表 key**，永远不回到线上。出站请求始终携带 operator 的原始 model id，所以折叠 host 路径不可能把请求发错地方。

### 5.1 点号 vs 短横的代际拼写：折叠在**比较**时，不在 canonicalise 里

同一个模型，Anthropic 写 `claude-opus-4-8`，GitHub Copilot 写 `claude-opus-4.8`。**canonicalise 两个方向都不能统一它们**：全局 `.`→`-` 会让 `gpt-5.6` / `glm-5.2` / `kimi-k2.6` 这些**表里就是点号**的前缀集体失配；全局 `-`→`.` 会毁掉数字间的横杠**不是**版本分隔符的 id（`llama-3-70b` 是 Llama 3 的 70B，不是 Llama 3.70）。

解法是把折叠放到**比较**那一刻——`alias.rs::prefix_matches`，比较时 `.` ≡ `-`，零分配，四张表全部保留厂商自己发布的拼写（这正是它们能对着厂商文档肉眼核对的前提）。

> **round-2 记的解法是错的，勿沿用**：那里写的是"把三张表改成最长前缀命中"。最长前缀命中**解决不了这个问题**——`claude-opus-4.8` 在任何排序规则下都不 `starts_with("claude-opus-4-8")`，那一行从来没进过候选集，改"哪个候选赢"没有意义。同时它也**不需要**推翻 prefix-shadow 守卫：守卫改用同一个谓词即可（见 §7）。

无条件等同这两个字符（而非只在数字之间）是安全的：`.`/`-` 在同一位置互换从未指代过不同的模型，而且表被禁止依赖这个区分——只差分隔符的两行是折叠相等的，prefix-shadow 守卫（用同一个谓词）会拒绝它们。

**它咬人的程度比 round-2 估计的重**（下列为实测值，非推演）：

| id | 折叠前 | 应为 |
|---|---|---|
| `claude-opus-4.8` 上下文 | 200K / 32K | 1M / 128K |
| `claude-opus-4.8` 计价 | $15 / $75 | $5 / $25（**3 倍**） |
| `llama-3-3-70b-instruct` 上下文 | 8K | 128K（**16 倍低报**） |
| `gemini-2-5-pro` 长上下文档 | 不命中 | 命中 |
| Copilot `gpt-5-4-mini` 退役 | 看不见 | 已退役 |

round-2 判断"今天不咬人"的依据是"Aleph 自己广告的 id 里没有点号代际拼法"——那句话本身没错，但**广告面不是唯一入口**：按需 `/models` 发现（§6）会把宿主自己的拼写交到用户手里，而 `llama-3-3-*` 这一族是托管方的常规拼法，压根不需要 Copilot 参与。

### 5.2 ⚠️ 同一个模型两套 id：归一解决不了，得靠线上翻译

Kimi 是目前唯一一个把**同一个模型**放在两套 id 命名空间下的厂商：开放平台（`api.moonshot.ai`）叫 `kimi-k3`，Kimi Code 订阅端点（`api.kimi.com/coding`）叫 **`k3`**，后者还多一个开放平台没有的 `k3-256k`。`canonicalize_model_id` **对此无能为力**——两个 id 不是同一个字符串的装饰变体，没有可剥的 tag。

后果分两侧，都要处理：
- **表侧**：四张表按前缀查，所以 `k3` / `k3-256k` / `kimi-k3` 各要一行。`k3-256k` 必须排在 `k3` 之前（前者以后者为前缀）；`kimi-k3` 必须排在宽泛的 `kimi` 行之前，否则这个 1M 模型会被当成 200K，上下文过早压缩。
- **线上侧**：唯一的翻译点是 `anthropic::provider_policy::normalize_kimi_coding_model_id`——它是请求出门前的最后一道关。该函数历史上写作"一切都变成 `kimi-for-coding`"（当时端点确实只有一个 id），**这个形状在 K3 之后是致命的**：折叠掉 `k3` 不会报错，只会让预设、picker、能力表都说 K3，而每一次请求跑的是 K2.7 Code。守卫测试因此断言的是"四个原生 id 逐字节穿过"，不是"函数被调用了"。

**推论**：往任何"单一 id 端点"加第二个 id 之前，先看它的归一/翻译函数是不是写成了折叠式。

### 5.3 ⚠️ `reasoning_effort` 有两层闸，写错层会静默吞掉一个旋钮

"这个字段能不能发"在两个地方回答，**问的不是同一个问题**：

| 层 | 位置 | 问的是 |
|---|---|---|
| 端点闸 | `openai_common/provider_policy.rs::ProviderCapabilities::supports_reasoning_effort` → `PayloadPolicy::strip_reasoning` | **这个端点认不认识这个字段** |
| 模型矩阵 | `openai_common/reasoning_effort.rs::supported_efforts` → `clamp_effort` | **这个模型接受它，且接受哪几个值** |

Moonshot 的闸曾经写 `false`——那在只有 K2.x 的时代是对的，K3 之后就变成**在最后一步把已经算好的 `reasoning_effort` 删掉**：`map_think_level` 发了、`clamp_effort` 夹了、`PayloadPolicy::apply` 一句 `payload.remove` 全丢。用户设的 think level 无声消失，每次请求都跑厂商默认档，**没有报错也没有日志**。

现在闸开在端点（端点确实认识这个字段），约束落在模型家族：`supported_efforts` 对 K3 返回 `["low","high","max"]`，对**其它任何 kimi/moonshot id 返回空集**（空集 ⇒ `clamp_effort` 返 `None` ⇒ 不发这个字段）。空集分支是 fail-closed 的兜底——将来的 `kimi-k4` 也走它，不会继承通用阶梯去撞 400。

两条配套：
- **`xhigh` 与 `max` 是同一格，只是两家拼法不同**（OpenAI / Anthropic 4.7+ 叫 `xhigh`，Kimi / Anthropic 4.6 叫 `max`），所以它们在 `effort_ordinal` 里共用序号 5。给两个不同序号的话，请求 `xhigh` 会和 `high` 并列距离 1、按"平手取便宜"落到 `high`——`max` 就成了**列在支持表里却没有任何请求能到达的值**，和不支持它是同一种缺陷。
- **K3 的支持表刻意不含 `none`**。在 Kimi 上"关闭 thinking"不是一个档位而是**换模型**——厂商文档明说关掉 thinking 的请求会被路由到 K2.6。所以 `ThinkLevel::Off` 向上夹到 `low`（thinking 开着、最便宜的一档、仍然是 K3），而不是发 `none` 让用户在毫不知情的情况下拿到另一个模型的回答。

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

**单飞与 stale 回退（2026-08 round-3，映射 pi）**：同一 provider 的并发 refresh 经 per-provider 锁单飞（`REFRESH_LOCKS`）——pi 的 `inflightRefresh ??=` 同款形状；输掉竞争的一方直接吃赢家刚写入的清单（按 `fetched_at >= 调用开始时刻` 判定，**不吃 TTL 缓存**，所以 operator RPC 的"强制真拉"语义不被稀释）。刷新失败时两个触发面都回退到磁盘上的旧快照而不是报空——pi 的"网络失败恢复持久化快照"同款；RPC 行带 `stale: true` 标注，picker 可以据此外显"这是上次的数据"。

### 6.5 边界

- **只贡献 id**。窗口 / 价格 / 生命周期仍归策展表——`/models` 响应基本不带这些。一个没有策展行的 discovered id 诚实地显示为"能力未知"，跟今天的自定义中继模型一样。
- **无后台定时器**。悄悄按时给每个已配置厂商打电话是意外行为，不是功能。
- **`supports_health_check = false` 的 preset 直接拒**（OAuth-only 端点 / 按部署的 Azure 资源 / 分区域云都会 404 或限流列表路由）。
- **缓存绑定端点指纹（2026-08 round-4）**。`DiscoveredModels.base_url` 记录清单取自哪个端点，`cached_models(provider, base_url)` 指纹不匹配即视为无缓存——operator 搬了 `base_url` 后旧清单是**另一台主机**的库存，绝不能复活（与 `models_url` override 的搬迁守卫同一条教条）。指纹字段引入前的旧缓存文件按"另一端点的库存"处理，代价是一次重拉。Bifrost 式 generation counter 评估为不需要（§9 round-4）。

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
| `no_preset_points_its_aux_model_at_a_retired_id` | **aux 槽此前无人守** —— 摘要/分类跑在死 id 上，在没人看的后台路径失败 |
| `lifecycle_scopes_name_a_real_preset` | scope 打错字 ⇒ 那一行永远不触发，且静默 |
| `declared_aliases_resolve_to_their_profile` | 别名展开断裂 |
| `exemptions_still_name_something_real` | 豁免比它豁免的东西活得久 |

另有**五个 prefix-shadow 守卫**（分散在 `pricing.rs`（rate + tier）/ `capabilities.rs` / `alias.rs` / `lifecycle.rs` 各自的测试模块里，因为表是私有的）：查表是"首个前缀命中者胜"，一条广义行插在特化行之前会让后者**永远不可达**——它照常编译、读起来也对，只是从不执行。

**五个守卫必须全部用 `prefix_matches` 比较，即查表自己用的那个谓词**（§5.1）。用裸 `starts_with` 的守卫对折叠后才出现的遮蔽是瞎的（一条 `gpt-5-6` 行藏在更早的 `gpt-5.6` 行后面），会在查表已经跳过某行时报告"表是干净的"。`MODEL_VENDOR_PREFIXES` 与 `LIFECYCLE_TABLE` 这两张表**此前根本没有守卫**，2026-08-03 补上；lifecycle 那个**按 scope 分桶**——`groq` 行遮蔽不了 `github-copilot` 行，跨 scope 比较会错杀正确的表。

### 怎么加豁免

两张显式清单，都带理由，且被 `exemptions_still_name_something_real` 钉住：

- `UNCATALOGUED_FAMILIES` — 按 preset id。"这家的窗口我们没有可核实的数据"。删一条是进步；加一条要写清为什么核实不了。这张表本身就是"哪些 provider 在 picker 里不会显示上下文窗口，以及为什么"的答案。
- `ENDPOINT_LOCAL_ALIASES` — 按模型 id。只存在于某一个厂商端点、没有公开能力/价格数据的别名。**当前为空**，而这正是目标状态：唯一的那条（`k2p5`）随着广告它的 `kimi-for-coding` 链一起消失，`exemptions_still_name_something_real` 让"留着一条死豁免"变得不可能。

> **链要在"可定价性"上同质**。`advertised_models_of_priced_vendors_have_rates` 对"已定价 vendor"的定义是**按结果**的（这个 provider 的**另一个**模型能定价 ⇒ 这个也该能）。所以往一条全部走 vendor-inferred 定价的链里加一根开放权重的横杠（Together 的 Llama-3.3、Qianfan 的 DeepSeek V4）会被报成漂移——这不是守卫过严，而是"半条链能算钱、半条不能"本来就是个说不清的成本视图。

---

## 8. 对照表 (Gap Analysis)

> **改这一层之前先看这两张表**。逐维度标注「映射 / 对齐 / 超越 / 有意不移植」。

### 8.1 openclaw（2026-08 round-2）

openclaw 把整份目录放在每个 provider 插件的 `openclaw.plugin.json` 里：`modelCatalog.providers.<id>.models[]`，每条带 `contextWindow` / `maxTokens` / `reasoning` / `input[]` / `cost{}`，**以及 `status: "deprecated"` + `replacedBy`**；另有 provider 级 `suppressions[]`（Google 全靠它记退役）。40 个 provider roster。

| 维度 | openclaw | Aleph | 裁决 |
|------|----------|-------|------|
| 退役数据的**位置** | 与模型条目同处一行，加模型时不可能不看到它 | 兄弟表 `LIFECYCLE_TABLE` | **映射数据，不映射结构**。同处一行是 JSON 目录才有的便利；Rust 侧把它塞进 `CAPABILITY_TABLE` 会让 60+ 条 `Active` 字面量全部长出一个恒为 `None` 的字段 |
| 退役的**作用域** | 天然按 provider（条目住在 provider 目录里） | `LifecycleRow.provider: Option<&str>` | **映射**（见 §3.1）。这是本轮真正的架构补齐——此前结构上说不出"Groq 退役了、Together 没有" |
| 厂商话 vs 转售商话 | 不区分（每个 provider 各说各的，天然隔离） | `None` = 厂商，`Some(id)` = 宿主 | **超越**：Aleph 是单张表，必须显式区分，否则会把转售商的话当厂商的话（`minimax-m2.7` 就是那个反例） |
| 目录规模 | 40 provider roster，`status` 覆盖 48 条 | 53 preset，`LIFECYCLE_TABLE` 24 行 | **对齐**（Aleph 只记非 Active 行） |
| `p` 分隔符（Fireworks） | 每个 provider 自带 id 表，不需要归一 | canonicalise 还原（§5） | **映射到归一层**——Aleph 是共享前缀表，必须归一 |
| 同一模型的多种拼写 | 每个 provider 目录各写各的 | 单一前缀表 + 比较时折叠分隔符 | **已闭合**（§5.1，2026-08-03） |
| 价格粒度 | `cost{input,output,cacheRead,cacheWrite}` | 同上 + reasoning 独立费率 + 长上下文 tier | **超越** |
| 订阅端点的 `cost: 0` | 照登 `0` | 记 `None`（未记录） | **有意不同**：`0` 会被读成"免费"并让 `cost_aware` 把它排第一 |
| 远端目录 overlay | `remote-overlay.ts` / `remote-refresh.ts`（可拉线上目录覆盖本地） | 无 | **有意不移植**（R3，见 §9） |

### 8.2 opencode / kimi-cli（2026-07-25 round-1）

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

### 8.3 pi（`@earendil-works/pi-ai`，2026-08 round-3）

pi 的目录是**生成期 hydrate**：`scripts/generate-models.ts`（2762 行）从 models.dev / OpenRouter / Vercel AI Gateway 三源拉数据，叠加散在脚本里的裸常量修正，烧进每 provider 一个 JSON shard 随包发布；运行时另有 per-provider `fetchModels` overlay。与 Aleph 的编译期四表是**同一代际的两种打包方式**，可比的维度如下。

| 维度 | pi | Aleph | 裁决 |
|------|-----|-------|------|
| 目录打包 | 生成期三源在线 hydrate + JSON shard（非 strict 模式源失败**静默降级为空目录**） | 编译期静态四表 | **维持 Aleph**（R3；静默空目录比静态陈旧更糟） |
| 人工修正的结构 | 裸常量 + 内联 if 散在生成器里，靠注释日期自觉 | 四表声明式 + 13 条编译期守卫 | **Aleph 超越** |
| 模型生命周期 | ❌ 无；旧模型下次生成时静默消失，用户配置悬空无提示 | `lifecycle.rs` + scoped 行 + `select_model` 硬拒 + 守卫 | **Aleph 超越** |
| 并发 refresh 去重 | per-provider `inflightRefresh ??=` 共享 promise | `REFRESH_LOCKS` per-provider 锁 + 赢家清单直接服务 | **映射**（round-3 补） |
| 刷新失败回退 | 恢复持久化快照 + 静态基线 | 磁盘旧快照（stale 标注），两个触发面都接 | **映射**（round-3 补） |
| 条件请求 | `ModelsStoreEntry` 预留 etag/lastModified 字段**无人写入**，纯 schema | 无 | **有意不移植**（pi 自己也没实现；厂商 `/models` 基本不发 ETag） |
| 订阅端点定价 | 隐含定价（用等价 API 费率回填，"估算订阅用量的价值"） | `QUOTA_BILLED_MODELS` 显式 Unknown | **有意不同**：pi 估的是"价值"，Aleph 的数字喂 `cost_aware` 排序与 run 成本，编一个价会让订阅端点在成本排序里插队（§4.2） |
| reasoning 档位 | `thinkingLevelMap` 三态（原生值/null/缺省）+ 就近夹取 | `supported_efforts` + `clamp_effort`，空集 fail-closed（§5.3） | **对齐** |
| 长上下文跳档 | `tiers[].inputTokensAbove` 整请求跳档 | `TIER_TABLE` 输入轴 | **对齐** |
| failover / 成本路由 | ❌ 完全外包给网关（请求体里塞路由偏好） | failover walk + `cost_aware` + 断路器 | **Aleph 超越** |
| 溢出检测 | 20+ provider 的错误文案正则库（`utils/overflow.ts`） | 无 | **评估为不移植**：属错误分类层（§2.x 语境），非目录层；且正则启发式与 R8 的适用范围有张力 |
| 模型可见性过滤 | `filterModels(credential)` + `getAvailable()` | `list_models` 默认只列已配置 provider | **对齐** |

pi 侧独有但**本轮明确不收**的：`compat` 能力矩阵（30+ 字段的"同 API 不同方言"行为差异）——那是协议实现层的关注点，Aleph 的对应物是 `openai_common/provider_policy.rs` 的 `PayloadPolicy`，不进目录层。

### 8.4 RouteLLM / vLLM semantic-router / Bifrost（2026-08 round-4）

三个参考项目讲的主要是**运行时路由**（§3.6 C 层），目录层可比维度比前三个项目少；逐项裁决如下。注意 `/Volumes/TBU4/Github/semantic-router` 的实际检出是 **vLLM Semantic Router**（Go + Rust candle bindings），不是 aurelio-labs 的 Python 库。

| 维度 | 参考项目 | Aleph | 裁决 |
|------|---------|-------|------|
| 模型目录 | RouteLLM **为零**（`MODEL_IDS` 是 MF embedding 行索引，加模型=重训）；vLLM SR `ModelParams`（pricing+capabilities+ctx，无生命周期）；Bifrost datasheet 远端 24h 同步+DB，~100 字段 LiteLLM schema | 编译期策展四表 + 单一 join 点 + 生命周期 | **Aleph 领先**；Bifrost 的远端同步与字段膨胀不移植（同 models.dev 判据，R3） |
| 路由决策分层 | RouteLLM score→threshold→binary；vLLM SR Signals→Decision→Selection 三层 + SLO-then-score；Bifrost 插件 PreRequestHook 决策、core 执行 | capability_gate 剪枝 → route_policy 排序 → failover walk，已是同构三层 | **已对齐** |
| 选型审计 | vLLM SR `SelectionResult` 必带 method+confidence+reasoning | `route_status` 快照 + `route_witness` + failover 事件 | **已对齐** |
| 成本语义 | RouteLLM 无成本表（threshold 只对应"%强模型调用"）；vLLM SR 互斥桶计价；Bifrost `CalculateCost` | pricing.rs + `RateBasis` + `cost_aware` | **Aleph 领先**（真实美元 vs 调用占比代理） |
| 故障韧性 | RouteLLM **无 failover**（异常透传）；vLLM SR 静态 weight 无健康信号；Bifrost 重试+key轮换+fallback，**无断路器/EWMA** | 断路器 + 双层 cooldown + EWMA + 游走梯 | **Aleph 领先** |
| key 池 | Bifrost 401/402/403=死 key 请求内不复活免 backoff，429=本轮排除保留 backoff | 单 key per provider | **有意不移植**（个人 runtime，key 池=多租户税） |
| 流式带内错误 | Bifrost `CheckFirstStreamChunkForError`：HTTP 200 流内错误载荷→重试/fallback | `ProviderDelta::Error` 基建早有（Anthropic/Responses 已接），**OpenAI 兼容 SSE 未接** | **缺口，round-4 连线**（`openai_chat/sse.rs`） |
| live 目录并发 | Bifrost `live.UpsertIfCurrent` generation counter；keyconfig 变更=不可变 snapshot 整体换 | 单飞 + stale 回退；**缓存未绑端点指纹** | **缺口，round-4 补** base_url 指纹；generation counter 评估为不需要（单飞串行化同进程写，多进程本被 doctor 禁止） |
| picker/消费面一致性 | — | roster 合并曾在**前端**重实现，且缺 base_url-moved 守卫（中继端点推荐必 400 的 id） | **缺口，round-4 收编**：`presets::model_ladder` 成为 picker/failover 共用 leaf |

RouteLLM 可提炼的三条资产——score→threshold 决策契约、"阈值=目标升级比例"的分位数校准、阈值扫描 cost-quality 曲线评估——属于**未来若做难度信号路由**时的参考，本轮不落地（Aleph 的 N 元游走梯已是其强弱二元对的超集）。

**round-5 补记（2026-08-06，C 层运行时路由视角复核同三个项目，详见 FEATURE_LOCATOR §3.6 round-5）**：上表裁完「差距在参考项目侧」之后，本轮回身修自己——**8 个内部缺陷 + 3 个增强**，三个增强各对应表里的一行：

| 增强 | 对位的参考项目事实 | Aleph 落地 |
|------|------------------|-----------|
| 滑动窗口限流计数 | LiteLLM/Bifrost 用固定分钟桶，边界清零是它们已知的坑——Aleph 在 round-2 立 `rate_limits` 时把**同一个坑**也继承了（`over_limit` 边界 99%→0% 抖动） | `load_stats.rs::RateWindow` 双桶加权：当前窗口 + 上一窗口 × 剩余时间比例，占用率逐秒线性衰减不跳零 |
| 限流文本词表 | Bifrost `IsRateLimitErrorMessage` 的 22 个模式——限流不一定以干净 429 到达 | `llm_retry.rs::RATE_LIMIT_TEXT_PATTERNS` 补 12 个真空缺模式（`throttled`/`tpm exceeded`/`concurrent requests limit` 等），**拒 6 个**（`requests per`/`limit exceeded`/`usage limit`/`rate increased` 等误伤面，理由写在常量 doc）；`classify` 与 `classify_exhausted` 的词表从此同源（顺带根治了两臂漂移的 F1） |
| 后台健康探测 | 参考项目普遍带后台探测循环（LiteLLM `background_health_checks`、Bifrost 健康检查）——**Aleph 缺这一块**：熔断只能等真实流量 half-open 或手动 `providers.test` | `gateway/health_prober.rs`：`[route] health_probe_interval_secs`（**默认关**，探测花真实请求），只探 circuit-open 的 provider，绿色探测只清断路器、不碰限流 cooldown |

本轮新裁决（追加进 §9 的同款判据）：**hedging（并行对冲请求）不移植**——并行对冲是拿双倍请求钱买尾延迟，多租户网关的流量形态才值这个价；Aleph 的 failover walk 是顺序链，断路器 + 双层 cooldown + EWMA 已覆盖慢/死端点，个人 runtime 无此需求。**key 池维持 round-4 裁决**（多租户税）。**metrics 导出（Prometheus/OTel exporter）不移植**——可观测性的答案已经有 `route_status` 快照（含 `next_order` 与 `config_problems`）+ doctor，独立导出器是新依赖加新常驻面（R3），真出现外接监控需求时单独立项。**run 粒度 witness 键（E3①）不做**——`RequestPayload.metadata` 只有 `session_id` 没有 run_id，跨 harness↔gateway 穿一个 id 的代价对 best-effort 诊断不成比例；溢出全清抹在飞 run 的那一半已修（LRU 淘汰 + 写入刷新年龄，`route_witness::BoundedWitnesses`）。

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

以下是 2026-08 round-2 评估后**明确不做**的（不是遗漏）：

- **移植 openclaw 的远端目录 overlay**（`model-catalog/remote-overlay.ts` + `remote-refresh.ts`）— 与"拉 models.dev"同一条理由：R3。
- **照抄 `cost: 0`** — 订阅制端点（`kimi-for-coding` / NVIDIA NIM 免费档 / Qwen Token Plan）在 openclaw 目录里价格全是 `0`。那是"未公布"，不是"免费"，而 `unpriced_cost` 会把真正的 `0` 当成最便宜的候选排第一。这些一律记 `None`。
- **改 `novita` / `github-copilot` / `stepfun` 的 `base_url`** — openclaw 用的是 `api.novita.ai/openai/v1`、`api.individual.githubcopilot.com`、`api.stepfun.ai`，与 Aleph 现有的三个都不同。两边都可能是对的（这几家都有多条并存的兼容路径），而改 preset 默认 `base_url` 是**唯一一类能把已经在工作的配置弄坏**的改动。只刷新模型 id，端点不动。
- **改 `xai` 默认** — Venice 转售 `grok-4-5`、openclaw 的 suppressions 提到 `grok-4.20-*`，但 openclaw 自己 xAI 那条链的 `targetModel` 仍然是 `grok-4.3`，和 Aleph 一致。没有比现状更强的证据，就不动。
- **升级 `siliconflow` / `hunyuan`** — openclaw 没有对应 provider 条目（Tencent 那条走的是另一个 `tokenhub` 端点），凭猜测把默认改到 `deepseek-v4` / `hy3` 就是把"陈旧但能用"换成"可能 404"。
- **把 `advertised_models_of_priced_vendors_have_rates` 放宽成"允许混合"** — 半条链能算钱半条不能，产生的成本视图比 `Unknown` 更难解释。正解是让链在可定价性上同质（§7）。

以下是 2026-08 round-3（对标 pi）评估后**明确不做**的：

- **给 cohere 补价目** — openclaw 目录里 `command-a-plus-05-2026`（现旗舰）与 `north-mini-code-1-0` 的 cost 全是 `0`＝未公布；唯一有价的 `command-a-03-2025` 已退役。加任何一行都会触发守卫 10（同 preset 内可定价性必须同质），而退役行的价没有消费者。
- **给 perplexity 补价目** — openclaw 的 perplexity 插件没有 `modelCatalog`，无 accepted 源；凭记忆写价违反"不靠猜"。
- **pi 的订阅端点隐含定价**（`KIMI_CODING_IMPLIED_COSTS`：用等价 API 费率回填订阅模型）— pi 估的是"订阅用量的价值"，Aleph 的费率喂 `cost_aware` 排序与 run 成本估算；把订阅端点按 API 价排序是对 operator 说谎。维持 `QUOTA_BILLED_MODELS` 显式 Unknown（§4.2）。
- **ETag / Last-Modified 条件请求** — pi 的 `ModelsStoreEntry` 预留了这两个字段但**没有任何代码写入它们**（纯 schema 预留）；且各家 `/models` 端点基本不发 ETag。300s TTL + 单飞已覆盖实际需求。
- **pi 的溢出检测正则库**（`utils/overflow.ts`）— 那是"这一轮请求失败了没有"的错误分类层（§2.x/§3.6 语境），不是"这个模型是什么"的目录层；如需引入应在 failover/compaction 那侧单独立项评估。
- **改 `hyperbolic` / `huggingface` 的默认模型**（仍是 Llama-3.3 时代）— 与 round-2 对 siliconflow 的判断同款：没有 accepted 源给出"应该改成什么"，`Llama-3.3-70B-Instruct` 是真实可用的 id（这两家是托管方不是厂商）。陈旧但能用 > 可能 404。
- **pi 的 per-model `compat` 矩阵** — 协议方言差异属 `openai_common/provider_policy.rs` 的 `PayloadPolicy` 层，不进目录层。

以下是 2026-08 round-4（对标 RouteLLM / vLLM semantic-router / Bifrost）评估后**明确不做**的：

- **多 API key 池 / key 轮换**（Bifrost 的 dead-vs-used 双集合语义）— 那是多租户网关摊配额的方案；Aleph 是单 key 个人 runtime，引入 key 池是纯税。401/403 的正确处置已有：断路器 + `Permanent` 分类不再重试。
- **训练管线与 embedding/ML 路由**（RouteLLM 的 MF/BERT/causal-LLM 打分器，vLLM SR 的 18 条信号管线与 KNN/SVM/MLP 选择器）— R3（core 无重依赖）+ R8/R10（意图路由归 LLM，智能住在 prompt，零中间件税）。score→threshold 契约与分位数校准若未来做难度信号再单独立项。
- **Bifrost 的 CEL 治理引擎 / virtual key 层级 / 9 种定价 override 作用域** — 多租户 SaaS 网关需求，非个人 runtime。
- **Bifrost datasheet 的远端 24h 同步 + DB 缓存** — 与"拉 models.dev"同一条理由：R3。
- **discovery 的 generation counter**（Bifrost `live.UpsertIfCurrent`）— 评估为不需要：per-provider 单飞锁已串行化同进程写者，多 aleph-server 进程本就被 doctor 的 duplicate-instance 检查禁止；跨写者竞态不存在，counter 无洞可补。端点搬迁的正确解法已用 base_url 指纹落地（§6）。
- **`WeightedRandom` 式权重截断**（Bifrost `int(weight*100)`，0.005 静默变 0）— 若未来做加权选择，用别名法 O(1) 或前缀和，别抄截断。

---

## 10. 常见修改的落点

| 想做的事 | 改哪儿 |
|---------|--------|
| 加/改预设别名 | `presets/registry.rs`（别名是**解析键不是展示行**：枚举面只迭代 `canonical_profiles()`；别名→canonical 归一走 `canonical_preset_id()`，别再在 handler 里硬编码特例） |
| 某模型支不支持 vision/tool-use | `model_catalog/capabilities.rs` + `capability_gate.rs` |
| 本地还是云端 | `model_catalog/endpoint.rs` |
| 成本 | `pricing.rs`（`RateCard` = picker 的费率投影） |
| 按成本路由 | `[route] load_balance = "cost_aware"`；连线点 `failover/provider.rs::price_hint`，sort 在 `route_policy::balance_group`。**候选的 tier 从 `with_tier_catalog` 来**（2026-07-27）——此前 live 派生的候选一律 `Unknown`，`unpriced_cost` 因此把**免费本地端点排最后**，与本表第二次修复的方向正好相反 |
| 单 provider 内的 failover 游走梯 / picker roster | preset `fallback_models` 即游走梯；**唯一合并点 `presets::model_ladder`**（base 在前、未列档位在后；operator 改过 `base_url` 则不合并）——failover 以 operator `models` 为 base，`providers.catalog` 的 `roster` 字段以 operator `models`（空则 `default_model`）为 base，前端 picker 纯渲染不再自算 |
| **给模型记录加一个新维度** | **`model_catalog/record.rs::resolve` 一处** |
| **某模型被厂商下线** | **`lifecycle.rs::LIFECYCLE_TABLE` 加一行，`provider: None`（带 successor）** |
| **某模型只在一家宿主上下线** | **同表，`provider: Some("<preset id>")`** —— 别写成全局行，那会拒掉别处能用的 id（§3.1） |
| 某宿主用了别的 id 拼法 | `alias.rs::canonicalize_model_id`（host 路径折末段 / `p` 分隔符）；点号-短横见 §5.1 |
| **默认模型过期了？** | **先跑 `cargo test -p alephcore --lib drift_tests`** |
| **要一个表里没有的新模型** | **`list_models { refresh: true }` 或 `providers.modelsRefresh`** |
| 子代理 / MoA 扇出跨厂商 | `provider/model` 限定名；消费点 `agents/runtime.rs::resolve_spawn_route`（见 §4.5 round-7）。守卫是"前缀须命中已配置 provider 才剥离"，**别改成无守卫剥离**。主循环侧同型解析是 `thinker/mod.rs::MultiProviderRegistry::get`（同一守卫；此前并排的 `resolve_model_to_provider_and_model` 只服务那条已 CUT 的预测式 `resolve_with_fallback`，见 §3.6 round-3） |
