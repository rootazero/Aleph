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

### 契约住在协议 crate，不在 handler 旁边（2026-08-13）

四张表的 **wire 投影**（`ModelCapabilities` / `RateCard`+`RateBasis` / `ModelLifecycle`+`ModelStatus` / `ModelSource` / `DiscoveredModel`）与整个 `providers.*` RPC 形状定义在 `shared/protocol/src/providers/`，`alephcore` `pub use` 回来。理由是**说这条 wire 的 crate 有四个，其中两个按设计不许依赖 `alephcore`**（`aleph-cli` / `aleph-tui`，它们的 `Cargo.toml` 用大写写着这句话）。各持一份手抄的代价已经付过：`aleph providers list` 读 `type`/`default` 而服务端只发过 `provider_type`/`is_default`（两列自写下之日起每行都是破折号）、`providers get` 读顶层而不是 `provider` 信封（**每一行**都是破折号）、`providers add`/`test` 发扁平 body 而 handler 要 `{name, config:{…}}`（**每一次调用都是 `INVALID_PARAMS`**，从来没成功过）。

表的字面量仍在 `alephcore`（`ModelLifecycle` 的 `&'static str` 改 `Cow<'static, str>` 之后表照旧是 `const`），但**类型只有一个**，所以表与 wire 结构上不可能描述不同的东西。

⚠️ **响应要用契约类型 build，不只是用它 parse**：解析只能证明响应是**超集**（serde 忽略未知键），超发在那个方向上结构性不可见。守卫 `the_catalog_response_speaks_only_the_contracts_vocabulary` 的期望键集**由契约类型序列化派生**——写一张字面量清单只是把同一个漂移挪高一层。

### 为什么必须是一个点

重构前，`capabilities_for` + `rate_card` + `endpoint_kind_for_base_url` 这个三元 join 被**手抄了三处**：`builtin_tools/list_models.rs::enrich`、`gateway/handlers/providers/handlers.rs` 的 preset 行、同文件的 custom provider 行。加第四个维度（本轮的 lifecycle）会接上其中两处、漏掉第三处——而漏掉的那处不会报错，只会少一个字段。

（`route_observe::price_milli_per_mtok` 与 `failover::price_hint` 只读 `pricing` 一张表，并且应当保持如此：它们要的是一个用来排序的标量，不是一条用来展示的记录。）

这是 opencode 用**有序插件**（`modelsDev(0) → env(10) → account(20) → provider(30) → config(40) → discovery(50)`，每层 mutate 同一份 draft）买到的性质。Aleph 只取那个不变量（"只有一个函数知道记录怎么组装"），不引入插件总线——四张静态表不需要一条消息总线。

### `roster` 是记录不是标量数组（2026-08-13）

`CatalogEntry.roster` 从 `Vec<String>` 改为 `Vec<RosterModel { id, source, lifecycle }>`。把一列记录投影成一列标量的那一刻，`source` 与 `lifecycle` 对**每一个渲染器同时消失**，而丢弃发生在生产者里，所以每张脸单独看都是对的。picker 要的正是这两样：一个刚从 `/models` 抓回来的 id 没有策展窗口与价格（空的能力列此时是**诚实**而不是坏掉），一个已退役的 id 要能被标出来并报出 successor。

仓内唯一读者与这次改动同批修改，所以**不留扁平旧字段**——为兼容而留的旧表述如果有第二个作者，它就会漂。

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

⚠️ **那张表在 `canonicalize_model` 之后被读，所以条目必须写成 canonical 形式**（2026-08-13 round-3）。`k2p5` 是**线上**拼法，归一会把 Fireworks 式 `<digit>p<digit>` 还原成 `k2.5` —— 写线上拼法就是一行**从写下之日起从未命中**的规则。同一个 id 在 `reasoning_effort::is_other_moonshot` 里被字面比较且**是对的**（那个模块另一套归一：小写 + 剥日期戳，不折分隔符），把它跨过那条边界抄过来正是成因。端到端测试看不见这件事：`k2.5` 恰好也不匹配任何价格前缀，所以结论对而理由错。守卫 `quota_billed_ids_are_stated_in_lookup_form` 钉在表自身上。

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

### 6.4 触发面（共用同一个 leaf）

| 入口 | 受众 | 行为 |
|------|------|------|
| `list_models { refresh: true }` | LLM（R8） | 先看缓存 TTL；仅对已过期的 provider 发起请求 |
| `providers.modelsRefresh` RPC | operator | **强制**真拉；可选 `provider` 参数收窄到一个 |
| `providers.catalog` 的 `roster` | 人（Panel / TUI / CLI） | **只读磁盘缓存**，绝不发网络（2026-08-13） |

⚠️ **sweep 行怎么读只有一个答案：`ModelsRefreshRow::outcome()`**（`Live` / `Stale` / `Failed` / `NotApplicable`，round-4）。CLI 的状态列、TUI 的那句话、Panel 的徽标此前各写一遍 `match (ok, stale)` —— 三份推导只在状态恰好是三个时活着，而 `NotApplicable`（＝`kind: Unsupported`）是第四个：**它不是失败**，那六家什么都没坏，只是没有清单可取。三张脸都曾把它印成红色/“no listing”，正是 round-3 在健康那张脸上修掉的“一排关于健康 provider 的红行”，从另一扇门进来。措辞各面自持（R4），**判决**归契约。

⚠️ **`providers.modelsRefresh` 从写下之日到 2026-08-13 一个客户端都没有**（全仓 grep 零命中），而它的 doc 自称是"picker 的按行刷新按钮想要的"——那个按钮不存在。CLAUDE.md §0：**没有客户端的能力不算已交付**。现在的三个客户端是 Panel 的按行刷新、Panel 保存成功后的 fire-and-forget 窄化刷新（**不阻塞保存响应**——一家挂掉的 vendor 不该让"保存我的 API key"变慢），以及 `aleph providers models --refresh`。

⚠️ **第三行是读，不是第三个触发面**：`providers.catalog` 把缓存里的 discovered id 合进 `roster`，所以人看到的和 `list_models` 给模型看的是同一份。在此之前只有模型那半合了——**同一份缓存、同一个 TTL，模型看得见、人看不见**。读缓存**闸在 `has_api_key` 上**：`cached_models` 是同步文件读，而发现无凭据不跑，所以没凭据的 preset 结构上不可能有缓存条目；不闸就是每次开设置页 stat 全部 preset。

`list_models` 走 TTL 是因为它对 chat-tier 开放：无条件拉取等于给了"反复调用即刷屏打厂商"的手柄。operator RPC 是显式授权的"现在就去看"，不受 TTL 约束。

扇出用 `tokio::task::JoinSet` **并发**（两个参考项目都是顺序 for 循环）。单个 provider 失败**不冒泡**——发现是对一个已经能工作的目录做增强，一家厂商不可达不该让整次调用失败；失败计数进 `list_models` 的 `message`，细节进 `tracing::debug`。

**单飞与 stale 回退（2026-08 round-3，映射 pi）**：同一 provider 的并发 refresh 经 per-provider 锁单飞（`REFRESH_LOCKS`）——pi 的 `inflightRefresh ??=` 同款形状；输掉竞争的一方直接吃赢家刚写入的清单（按 `fetched_at >= 调用开始时刻` 判定，**不吃 TTL 缓存**，所以 operator RPC 的"强制真拉"语义不被稀释）。刷新失败时两个触发面都回退到磁盘上的旧快照而不是报空——pi 的"网络失败恢复持久化快照"同款；RPC 行带 `stale: true` 标注，picker 可以据此外显"这是上次的数据"。

### 6.5 边界

- **只贡献 id**。窗口 / 价格 / 生命周期仍归策展表——`/models` 响应基本不带这些。一个没有策展行的 discovered id 诚实地显示为"能力未知"，跟今天的自定义中继模型一样。
- **无后台定时器**。悄悄按时给每个已配置厂商打电话是意外行为，不是功能。
- **`supports_health_check = false` 的 preset 直接拒**（OAuth-only 端点 / 按部署的 Azure 资源 / 分区域云都会 404 或限流列表路由）。该位以 `CatalogEntry.discoverable` 上线，**让客户端在点之前就知道这 6 家不该有刷新按钮**——提供一个只可能失败的按钮比不提供更糟。
  **单一源 `probe::supports_model_listing`（round-4）**：这个位此前在三个文件里各写一遍 `preset.supports_health_check`（`probe_disposition` / 本 leaf / catalog 赋值处）。字段名读起来像“能不能 ping”，它实际说的是“答不答 `GET {base_url}/models`”——**一个名字与含义争辩的谓词被手抄三次，早晚有一份是反的**。6 家分别是 `chatgpt` / `azure-openai` / `amazon-bedrock` / `vertex-anthropic` / `ai-gateway` / `azure-foundry`，守卫 `the_catalogue_bit_clients_render_is_the_one_the_server_gates_on` 逐 preset 对账。
  ⚠️ **round-4 一度把这条读成“服务端根本没在闸，六家每次 sweep 都被拨号”，并按那个判断改了代码与三处文档。用 `if false` 破坏新守卫求证时只红了一条测试**，回头读 leaf 才发现拒绝从第一天就在这儿。**破坏守卫时红的条数比预期少，先怀疑的是自己的判断，不是守卫。** 真正剩下的是排序：`handle_models_refresh` 的凭据检查排在前面，于是**没链接过**的 opt-out 预设回 `MissingCredential`——可行动，且行动无效（贴 key 不会让端点长出 `/models`）。现按同一个谓词先答（`no_listing_endpoint_outranks_no_credential`，变异证过红）。
- **失败是分类的，不是一句散文（2026-08-13）**。`DiscoveryError` 一直是分类器，只是在最后一步被 `e.to_string()` 抹平了：`ModelsRefreshRow.kind ∈ {unsupported, missing_credential, transport, status, shape, timeout}`。客户端要回答的第一个问题是"这值不值得重试"，而"这家不发布目录端点"和"请求超时"此前是同一句话。**没有凭据的 provider 现在得到一行而不是被静默跳过**——跳过一条坏记录通常对，沉默才是贵的那一半：问"刷新这一个"却拿回空数组，读起来和"什么都没发生"一样。
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
- `ENDPOINT_LOCAL_ALIASES` — 按模型 id。只存在于某一个厂商端点、没有公开能力/价格数据的别名。**当前一条：`k2p5`**（`kimi-for-coding` 链仍在广告它，厂商没公布窗口）。目标状态是空——删一条是进步；而 `exemptions_still_name_something_real` 保证它一旦不再被任何 preset 广告就必须被删掉，所以这张表不会变成一份坟场。

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

#### 8.3b pi 的选择面（2026-08-13 round-5 delta）

round-3 比的是**目录数据**；这一轮比的是**人怎么挑**，因为用户报的正是那一半。锚点：`packages/coding-agent/src/modes/interactive/components/{model-selector,scoped-models-selector}.ts`、`model-search.ts`、`packages/tui/src/fuzzy.ts`、`interactive-mode.ts::completeProviderAuthentication`。

| 维度 | pi | Aleph（本轮后） | 裁决 |
|------|-----|----------------|------|
| link 后取模型 | `completeProviderAuthentication` 成功即按 provider 网络刷新（15s abort） | 保存成功且 `discoverable && has_credential && enabled` ⇒ fire-and-forget 窄化刷新 | **映射**（此前 Aleph 三个写入终点都不引用 `model_catalog`） |
| picker 打开时刷新 | 先画缓存快照，再后台刷新，三种状态各自成句 | catalog 直接带上缓存里的 discovered id；刷新是**显式按钮**，不在打开时自动发网 | **有意不同**：打开一个设置页不该是一次对外拨号 |
| 选择器搜索 | `fuzzyFilter` 子序列打分（连跑/间隙/词边界/位置） | 顺序保留子串 + 分层排序 | **有意不移植**（见 §9 round-5；策展顺序有意义） |
| 两份搜索文本 | picker 与 autocomplete 各一份，把裸 id 排到最后以抵消位置惩罚 | 不需要 | **不适用**（那是 fuzzy 的补丁） |
| 多选 | `ScopedModelsSelectorComponent`，`EnabledIds = string[] \| null`（null = 全开）、**有序**、可重排、enable-all/clear-all 作用于当前过滤集，保存写全局 `settings.enabledModels`（glob pattern） | `[providers.<id>] models`（**已经是**有序 `Vec<String>`，同时是 failover 梯与"不指定时用哪个"），Panel 给它一个有序多选 UI | **形状映射，载体不同**：Aleph 不建平行的 pattern 层（§9 round-5） |
| 未认证 provider 的模型 | 不进 `/model`（`getAvailable()` = 目录 ∩ 已配置凭据） | `providers.catalog{view}` 三档；发现出的 id 只在有凭据时才有缓存条目 | **对齐** |
| provider 搜索 | `OAuthSelectorComponent` 对 `getProviders()` fuzzy 过滤（`/login <ref>` 精确优先） | Panel 设置页搜索框 + TUI `/providers`，共用同一个 `rank_entries` | **映射**（此前 Aleph 两个面都没有过滤） |
| 失败降级措辞 | 一律"保留上次的列表并说出来"，且区分 404/501（无端点）与请求失败 | `stale: true` + `DiscoveryFailureKind` 六态 + `discoverable` 位 | **Aleph 超越**（pi 的区分只在内部，UI 仍是一句话） |
| 生成期 vs 运行期 | 同 round-3 结论 | — | 不重复 |

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
- **后台定时刷新** — 悄悄常驻的网络行为是意外，不是功能。**每一个触发面都必须由一次用户动作发起**（2026-08-13 新增的两个也是：按行刷新按钮、保存成功后的一次窄化刷新；`providers.catalog` 只读磁盘，不发网络）。
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

以下是 2026-08-13（契约收敛 + 链接后取模型 + 多选轮，对标 pi 的 `/model` 选择器）评估后**明确不做**的：

- **给 `providers.catalog` 加 `query` / `limit` / `offset`** — 目录是几十行，**三个**客户端（Panel / TUI / CLI）都过滤服务端已经发下来的行（`aleph_protocol::providers::search`）。服务端过滤会是一个零消费者的抽象（R10），而且会开出第二个"哪些行匹配"的答案。行数上到四位数再回来。**同理 `providers.list` 也没有 `query` 参数**（round-2 给 CLI 加的 `aleph providers list [query]` 是客户端过滤）。
- **移植 pi 的 `fuzzyFilter`**（`packages/tui/src/fuzzy.ts`）— `model_picker` 有一条记录在案的裁定：顺序保留的子串过滤，**刻意不 fuzzy 排序**，因为 catalog 的行序与每行的 roster 都是策展过的，按子序列质量打分会把它们洗成近似字母序。本轮只叠加分层排序（精确 id > id 前缀 > 别名 > display_name > 仅 model-id 命中），那是 TUI 命令面板已经付过学费的教训（输入 `mode` 选中 `/tools`）。**连带也不需要 pi 的"两份搜索文本"**（`model-search.ts` 把裸 id 排到最后）——那是给 fuzzy 位置惩罚打的补丁，不用 fuzzy 就没有这个问题。
- **建 pi 式的全局 `enabledModels` glob 集**（`settings-manager.ts:122`，minimatch 模式 + 可选 `:thinkingLevel` 后缀）— 会成为 `[providers.*] models` 之外**第二个**"哪些模型可用"的真源。Aleph 的多选落在 operator 配置轴上，且那条轴同时是 failover 梯（顺序即语义），一个平行的 pattern 层会和它对同一个问题给两个答案。
- **给会话加第五根 knob 承载多选** — pin 仍是单值、仍只有 `select_model` 一个写者，`sessions.patch` 的 `NOT_PATCHABLE` 一行未动。多选是 operator 配置，不是每对话设置。
- **TUI 做多选配置编辑器** — provider 配置写面属 R2（业务配置 UI 归 Panel）。ladder 是 operator 装机配置，不是聊天客户端该有的写面。**仍然成立。**
- ~~**TUI 接 `providers.modelsRefresh`**~~ — **2026-08-13 round-2 改判，实现了**。原裁定与上一条**捆在同一行**，而给出的理由只覆盖上一条：`modelsRefresh` **不写任何配置**（它刷的是读穿缓存），所以 R2 对它不成立。另两条理由也各自站不住：「TUI 没有承接 per-provider `kind` 行的地方」——有，就是 transcript 里的系统消息，TUI 报告其他每一个 RPC 结果都用它；「目录已经折入缓存里的 discovered id，够用」——**只有在别人刷过缓存的前提下才够用**，而无头 server + 终端这个部署形态里没有别人。现 `/providers` 里 **Ctrl+R**。⚠️ 记这一条是因为**它示范了一个形状**：一条把两件事捆在一起的裁定，理由通常只为其中一件写的，而另一件是搭车过去的——**拆开数一遍，比引用它便宜**。
- **给 `CatalogEntry` 加 `is_preset` 位** — Panel 的新分区（订阅登录 / 已配置 / 其余）比旧的"预设 vs 自定义"更贴用户实际在问的问题；为了复原旧分区去加一个字段是倒着走。

以下是 2026-08-13 **round-2**（一个匹配器管全部预设 + 兄弟族契约）评估后**明确不做**的：

- **generation provider 的实时 `/models` 发现** — 44 个预设里绝大多数是 `fal` / `bfl` / `suno` / `cartesia` / `deepgram` / `azure_speech` / `google_veo` 这类**厂商私有 API**，没有 OpenAI 兼容的列表端点。要做就得写 ~44 个 vendor 专用客户端进 core（违 R3），而它们各自的"模型"语义还互不相同（一个 voice id 和一个 checkpoint 不是同一种东西）。这一族的答案是**搜索 + 策展 `default_model` + signup 链接**——本轮把后两者接通了。
- **给 embedding（5 个预设）/ rerank 加搜索框** — 五行不需要过滤。加了是零收益的第二处 UI，且下一个读代码的人会以为那里有什么值得找的东西。判据是**列表长度**，不是"别处有所以这里也要有"。
- **把 children 建进 `Searchable`** — 只有 `CatalogEntry` 有 roster。给一张没有子项的列表一个"有没有匹配的子项"的接口，就是让它必须回答一个它没有词汇的问题，而"没有子项"与"没有匹配的子项"会在那一刻永久合流。`MatchRank::ChildOnly` 对 `rank_rows` **结构性不可达**，并有守卫钉着。
- **给 `DiscoveryFailureKind` 加 `Disabled` / `NotConfigured` 变体** — 它是普通外部标签枚举（无 `#[serde(other)]`，unit enum 也用不了），加一个变体会让旧客户端**整行**解析失败而不只是丢一个字段（`#[serde(default)]` 只管缺失、不管非法），而 "Panel-lite 连局域网内老 server" 是有记录的部署形态。两种新情形都折进既有词汇：不可行动的用 `Unsupported`，可行动的用 `MissingCredential`（"先链接它"）。
- **给 generation 预设行加 `discoverable` 位** — 那一族里这个概念不存在（上一条已说明），发一个恒 `false` 的字段只会引来一个永远不该出现的刷新按钮。

以下是 2026-08-13 **round-3**（零消费者裁决 + 探测面收敛）评估后**明确不做**的：

- **给 `ProviderHealthRow` 加 `skip_reason` 枚举** — 跳过有两个理由（operator 关了它 / preset 声明 `/models` 答不了这一问），而 `enabled` 已经把它们分开了。加一个外部标签 unit enum 会让旧客户端在遇到新变体时**整行**解析失败（同 round-2 对 `DiscoveryFailureKind` 的裁定，`#[serde(default)]` 只管缺失不管非法）。
- **把 `providers.healthcheck` 合并进 `providers/connectivity`** — 两者答的问题不同：前者是一张带延迟的表，后者是整个诊断引擎的一部分（散文式 finding + 总停机闸 + `fix_hint` 路由）。它们该共用的是**探测**与**探测判据**（`probe::probe_provider_bounded` / `probe::probe_disposition`），不是输出形状。
- **复活 `providers.needsSetup`** — 零客户端，且是"agent 能不能作答"这个问题的**第三个答案**（Panel 清单一个、它一个、真相一个）。判据落在 Panel 的 `usable()` 上：`enabled && (has_api_key || verified)`。
- **把 `usable()` 提到协议 crate** — `needsSetup` 撤回之后它只有一个消费者；为一个调用者建跨 crate 抽象就是 R10 要撤回的那种。

---

## 10. 常见修改的落点

| 想做的事 | 改哪儿 |
|---------|--------|
| 加/改预设别名 | `presets/registry.rs`（别名是**解析键不是展示行**：枚举面只迭代 `canonical_profiles()`；别名→canonical 归一走 `canonical_preset_id()`，别再在 handler 里硬编码特例） |
| 某模型支不支持 vision/tool-use | `model_catalog/capabilities.rs` + `capability_gate.rs` |
| 本地还是云端 | `model_catalog/endpoint.rs` |
| 成本 | `pricing.rs`（`RateCard` = picker 的费率投影） |
| 按成本路由 | `[route] load_balance = "cost_aware"`；连线点 `failover/provider.rs::price_hint`，sort 在 `route_policy::balance_group`。**候选的 tier 从 `with_tier_catalog` 来**（2026-07-27）——此前 live 派生的候选一律 `Unknown`，`unpriced_cost` 因此把**免费本地端点排最后**，与本表第二次修复的方向正好相反 |
| 单 provider 内的 failover 游走梯 / picker roster | preset `fallback_models` 即游走梯；**唯一合并点 `presets::model_roster`**（operator `models` → curated rungs → discovered ids，带 `source` 与 `lifecycle`；operator 改过 `base_url` 则不合 curated rungs，**但仍合 discovered**——那些 id 正是从那个端点拿回来的）。`model_ladder` 是它把出处投影掉的那个投影，failover 用它，传空 `discovered` 逐字节复现旧梯。前端 picker 纯渲染不再自算 |
| 这条 wire 的形状 / 加一个响应字段 | **`shared/protocol/src/providers/`**，不在 handler 旁边——四个说它的 crate 里有两个不许依赖 `alephcore`。**用契约类型 build 响应，不只是 parse 它**（解析只证明超集，超发看不见） |
| provider / 模型搜索的排序规则 | `shared/protocol/src/providers/search.rs`（Panel 与 TUI 共用；顺序保留子串 + 分层，**不是 fuzzy**） |
| **给一张新的预设列表加搜索** | **实现 `search::Searchable`**（`search_id` / `search_display_name`；别名有默认实现），然后 `rank_rows` / `filter_rows` —— **不要写 `.contains()`**。有子项的列表只有 `CatalogEntry`，它走 `rank_entries`；`MatchRank::ChildOnly` 对无子项列表**结构性不可达**是有意的（把 children 建进 trait 会让"没有子项"与"没有匹配的子项"分不开）。守卫 `both_halves_agree_on_identity_ranking` 拿目录自己的行两边各排一次断言相等 |
| generation 预设行的形状 | **`shared/protocol/src/providers/generation.rs::GenerationPresetRow`**，服务端**从它 build**。这一族只有一个客户端，所以 `signup_url` 被 serde 静默丢弃了一整程——**一个客户端不等于不需要契约** |
| **给模型记录加一个新维度** | **`model_catalog/record.rs::resolve` 一处**。`presets::model_roster` 是 round-4 补上的第五个消费者——它此前伸手直接调 `lifecycle_for`，只带走作者那天要的那一维，于是 roster 有生命周期而没有窗口和价格 |
| **窗口 / 价格要在 picker 上看得见** | 它们在 **`RosterModel.capabilities` / `.cost`**（逐 id），**不在 `CatalogEntry` 上** —— 挂在 provider 行上时它们描述的是 `default_model`，而那一行的工作是让你挑**另一个**，所以三轮下来上了线也没有任何渲染者。数字的拼法归契约（`ModelCapabilities::context_window_short` / `RateCard::io_per_mtok_short`，同 `ModelStatus::as_str` 的先例），摆放归各面；**`None` 是“策展表没有这一行”，不是 0 也不是免费**，渲染成空白 |
| **给一个 vendor 链接加渲染** | Panel 三处共用 `components/external_link.rs::safe_external_link`（http(s) 才成 `<a>`，其余渲染成纯文本而不是丢弃）。此前 skills 页有 scheme screen、provider 详情页的 `signup_url` 没有——**一个问题两个文件，没挡的那份才是要紧的那份** |
| **某模型被厂商下线** | **`lifecycle.rs::LIFECYCLE_TABLE` 加一行，`provider: None`（带 successor）** |
| **某模型只在一家宿主上下线** | **同表，`provider: Some("<preset id>")`** —— 别写成全局行，那会拒掉别处能用的 id（§3.1） |
| 某宿主用了别的 id 拼法 | `alias.rs::canonicalize_model_id`（host 路径折末段 / `p` 分隔符）；点号-短横见 §5.1 |
| **默认模型过期了？** | **先跑 `cargo test -p alephcore --lib drift_tests`** |
| **想一次看完所有 provider 通不通** | `aleph providers health`（`providers.healthcheck`）。`aleph doctor` 是更宽的那一问；两者共用 `probe::probe_provider_bounded` **与** `probe::probe_disposition` —— 后者是"要不要拨号"，此前只有 doctor 那一面认得 `supports_health_check` |
| **给 `QUOTA_BILLED_MODELS` 加一条** | 写 **canonical** 形式（表在 `canonicalize_model` 之后被读），线上拼法留注释；守卫 `quota_billed_ids_are_stated_in_lookup_form` |
| **要一个表里没有的新模型** | **`list_models { refresh: true }` 或 `providers.modelsRefresh`** |
| 子代理 / MoA 扇出跨厂商 | `provider/model` 限定名；消费点 `agents/runtime.rs::resolve_spawn_route`（见 §4.5 round-7）。守卫是"前缀须命中已配置 provider 才剥离"，**别改成无守卫剥离**。主循环侧同型解析是 `thinker/mod.rs::MultiProviderRegistry::get`（同一守卫；此前并排的 `resolve_model_to_provider_and_model` 只服务那条已 CUT 的预测式 `resolve_with_fallback`，见 §3.6 round-3） |
