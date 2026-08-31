# 检索侧供应商设置面统一设计（2026-08-31）

> **一句话**：`search` / `embedding` / `rerank` 三个设置页各自长了一套「预设常驻网格」，与 `providers` / `generation_providers` 的「隐藏式 picker + 已配置卡片」范式不一致。本轮把 UI 收敛到同一范式，**但三个家族的后端差异是真实的**：search 后端零改动、embedding 缺一个服务端算的维度字段、rerank 根本没有「多个已配置供应商」这个概念，需要一次配置结构迁移。
>
> 范围裁定（2026-08-31 用户）：rerank 走**增量集合**（保留单例作为活跃视图）；embedding 收起后用**库内签名比对**接管「维度可比较」的职责；rerank 预设目录搬到 `shared/protocol`（对齐 search）；`settings/search.rs` 拆分、手机端 embedding 孪生面跟改、R8 配置工具、真机 QA **全部在范围内**。

---

## 1. 证据（全部 grep 可复现）

| # | 事实 | 锚点 | 判据 |
|---|---|---|---|
| E1 | rerank 预设目录**硬编码在 Panel 视图里**，而「有哪些 rerank 供应商」的真源是 `src/memory/rerank/*.rs` | `interfaces/webchat/src/platform/wide/views/settings/reranking_providers/mod.rs:32` | E.0 §1 同一事实的两份表述 |
| E2 | `RerankProviderType` 6 个变体 ↔ `RERANK_PRESETS` 6 行，今天恰好齐，**没有任何东西钉住它** | `src/memory/rerank/provider.rs:44-58` vs 上一行锚点 | E.0 §3 守卫只覆盖它认得的形状 |
| E3 | `RerankConfig` 是**单例**：一个 provider / 一个 api_base / 一份 models。后端无处存「已配置的多个供应商」 | `src/memory/rerank/provider.rs:62-99` | — |
| E4 | `rerank_config.*` 只有 `get` / `update` / `test`，**没有 list / add / remove / set_active** | `src/gateway/handlers/rerank_config.rs:50,82,154` | E.0 §7 两端完整而中间没线 |
| E5 | rerank 的 vault key **已经是每供应商形式** `rerank:{provider_name}` —— 迁移时旧密钥可认领 | `src/gateway/handlers/rerank_config.rs:29` | — |
| E6 | `embedding_signature::compare()` 存在，但**从未暴露到 gateway**：`embedding_providers.rs` 全文零次提及 signature。Panel 今天无法知道切换会不会作废库内向量 | `src/memory/embedding_signature.rs:48`；`src/gateway/handlers/embedding_providers.rs`（零命中） | E.0 §7 |
| E7 | search 的 `backends` 已经是集合，`default_provider` + `fallback_providers` 齐备 | `src/config/types/search.rs:39,24,28` | — |
| E8 | `search_config.delete` 已 fail-closed：拒删默认项、清 fallback 链、删 vault key | `src/gateway/handlers/search_config/delete.rs:48-63` | E.0 §8 |
| E9 | `settings/search.rs` = **2115 行 / 102 KB**，且把已配置的预设与自定义端点渲染成**两个互不相干的列表** | `search.rs:296 PresetGrid`、`search.rs:383 CustomSearchProvidersList` | P2（500 行） |
| E10 | 代码里有**两段明确论证「embedding 不可收起」**的模块注释。改 UI 不改它们 = 制造最贵的那种谎 | `interfaces/webchat/src/components/preset_picker.rs:14-19`；`.../generation_providers/picker.rs:11-22` | E.0 §1 |
| E11 | `AssemblerConfig.rerank` 是 `MemoryConfig.rerank` 的**声明式镜像**，由 server builder 喂，担保物只有一句注释 | `src/config/types/memory/assembler.rs:38-40`；`src/config/types/memory/mod.rs:91` | E.0 §1 / §16 孪生 |
| E12 | rerank 配置的真实消费者只有 4 个（非测试、非 handler 自身） | `executor/builtin_registry/builder/constructor/mod.rs:660`；`gateway/handlers/memory_config.rs:249`；`thinker/memory_context_provider/constructor.rs:218`；`gateway/handlers/secret_migration.rs:185` | E.0 §6 先数一遍 |
| E13 | `CATALOG_DESCRIPTION_CEILING_BYTES = 112_772` 是**单向棘轮** —— 新增 3 个工具要付 3 份常驻描述字节 | `src/executor/builtin_registry/definitions.rs:2475` | R9 |
| E14 | 三个家族**没有任何 builtin tool**（`src/builtin_tools/` 全目录仅 `memory_search.rs` 提及 rerank），模型无法用自然语言配置它们 | grep `src/builtin_tools/` | R8（既有欠账） |
| E15 | `phone/settings/embeddings.rs` 是 embedding 的孪生面（337 行）；手机端**没有** search / rerank 设置页 | `interfaces/webchat/src/platform/phone/settings/embeddings.rs` | E.0 §16 |

**四个家族的现状矩阵**（这张表是本设计的出发点）：

| 家族 | 预设来源 | 配置数据模型 | 删除 | RPC |
|---|---|---|---|---|
| chat providers | core RPC catalog | provider 列表 | 有 | `providers.*` |
| generation（样板） | core RPC `list_presets` | provider 列表 | 有 | `generation_providers.*` |
| search | `aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS`（共享 crate 静态表，9 条）+ Panel 侧 `PRESENTATION` 图标表 | `HashMap<String, SearchBackendConfig>` + default + fallback | 有 | `search_config.{get,update,delete,test}` |
| embedding | core RPC `presets` | `Vec<EmbeddingProviderConfig>` + `active_provider_id` | 有 | `embedding_providers.*` 全 CRUD |
| rerank | **Panel 硬编码常量** | **单例** | **无** | `rerank_config.{get,update,test}` |

---

## 2. 方案选择

**A（采纳）复用已有的缝，不造新组件。** 每页提供 `listed / offerable / chosen_target` 三个纯函数，共用 `PresetPicker` + `PickerRow` + `ProviderRowCard` + `ProviderBadges`。这是 `generation_providers/picker.rs` 已跑通的形状；本轮是它的第 3/4/5 个用例，三次法则说抽象已经够了。

**B（否决）抽泛型 `PresetCatalogPage`。** `preset_picker.rs` 的模块注释已经论证过这条路：四个家族的右栏差异（embedding 的 reembed card、search 的 `engine_id`/`engines`/限流、rerank 的 `rerank_weight`、generation 的 modality tab）会让泛型组件长出 per-page flag —— 正是它当初拒绝的东西。

**C（叠加在 A 之上，采纳）共享契约测试。** `PresetPicker` 文档里那条「空查询必须返回全量」的契约现在是 *per page unit-tested* 的，靠每页作者记得抄。四个页面用**同一个推导**跑同一组断言，才符合 E.0 §9。

---

## 3. §1 共享 UI 层

### 3.1 新增 `components/preset_picker/contract.rs`（`#[cfg(test)]`）

三个接受 `offer` 闭包的断言函数，四个页面（**含 generation**）各调一次：

```rust
type Offer = impl Fn(&str) -> Vec<PickerRow>;

/// 空查询必须返回全量，且顺序 == 目录自身的顺序。
pub fn empty_query_offers_everything(offer: Offer, expected_ids: &[&str]);
/// 已配置的行仍被 offer，且 `configured` 为真（offered 但未标记读起来像「还没配」）。
pub fn configured_rows_stay_offered_and_marked(offer: Offer, configured_id: &str);
/// 删除后该行回到 picker 且 `configured` 变假（删了还能再配回来）。
pub fn deleted_row_returns_to_the_picker(after_delete: Offer, id: &str);
```

**验证纪律（E.0 §3）**：写完各证伪一次 —— 让某页 `offerable` 在空查询时返回空、让某页丢掉 `configured` 标记，确认断言真的红。没被证伪过的守卫不算守卫。

### 3.2 `PickerRow.subtitle` 各家族自填

| 家族 | subtitle | icon / color 来源 |
|---|---|---|
| generation | `default_model` | `preset_providers.rs::color_for/icon_for`（不动） |
| search | 由 `needs_api_key` / `needs_base_url` / `needs_engine_id` 推导：`需要 API Key` / `自建端点` / `无需凭据` | 现有 `search.rs::PRESENTATION` 表（随拆分搬走） |
| embedding | **`{model} · {dimensions}d`** | 新增同形状小表 |
| rerank | `default_model` | 随预设表搬到 shared crate 后同形状 |

未配置行**不许假装有值**（E.0 §17）：subtitle 写不出真话时留空，不填 `default`。

---

## 4. §2 search：后端零改动，UI 合并两个列表

### 4.1 目录拆分（先行提交，零行为变化）

`settings/search.rs`（2115 行）按 `generation_providers/` 的目录形状拆为：

```
settings/search/
  mod.rs             SearchView + 左右分栏路由
  presentation.rs    PRESENTATION / UNSTYLED / SearchPreset / join / find_preset
  picker.rs          listed / offerable / chosen_target + SearchPicker
  list.rs            已配置卡片列表（合并后的单一列表）
  detail_panel.rs    ProviderDetailPanel
  add_custom.rs      AddCustomSearchProviderPanel
  global_settings.rs GlobalSettings
  fetch_section.rs   FetchProvidersSection（原样搬走，不改）
```

**这是独立一笔提交**，与 picker 改造分开 —— 否则 diff 无法 review（见 §9 风险 2）。

### 4.2 UI 改造

- 左栏从 `PresetGrid` + `CustomSearchProvidersList` **两个列表合成一个**「已配置」列表。预设配好的和自定义端点渲染成同一种卡片 —— 这是统一的实质内容，不只是把 9 个格子藏起来。
- picker `offer` = `CONFIGURABLE_SEARCH_PROVIDERS` 全量 + 一行「自定义端点」，选中后走现有 `AddCustomSearchProviderPanel`。
- **删除按钮要把后端已有的拒绝渲染成禁用态 + 原因**（「它是默认供应商，先改默认」），而不是点了才弹错。E.0 §14：被闸住的人接下来会干什么，答不上就不是 fail-closed 是 fail-dead。

### 4.3 后端

零改动。`search_config.{get,update,delete,test}` 契约不变。

---

## 5. §3 embedding：UI 收起 + 新增维度闸

### 5.1 新增服务端计算字段

`embedding_providers.list` 响应增加：

```rust
pub struct EmbeddingStoreStatus {
    pub dimensions: Option<u32>,
    pub model: Option<String>,
    pub vector_count: u64,
    pub status: SignatureStatus,   // Match | Mismatch | Unknown
}
```

由服务端用 `embedding_signature::compare()` 计算。**Panel 不自己推导** —— E.0 §12：单位与边界必须在同一处派生。字段形状放进 `shared/protocol`，服务端**用它构造响应**（E.0 §10：只读自己刚写下的字面量的断言测的是 serde，不是代码）。

### 5.2 维度闸

`set_active` 之前比对目标 provider 的 `dimensions` 与库内 `store_status.dimensions`：

- **不一致** → 确认对话框，明说「库内 N 条向量将需要重嵌入」，确认后接现有 `ReembedMigrationCard`。
- **`Unknown`** → **走警告分支**，不静默放行。E.0 §8：「未知」不许读作「健康」。库空、未初始化、算不出来，一律读作「我无法担保」。
- **一致** → 直接切换。

### 5.3 UI

- 左栏只列已配置的 provider 卡片，带 `model · <n>d`。
- picker 行副标题带 `model · {dimensions}d` —— 信息不丢，只是从常驻网格移到 picker 行。

### 5.4 必须在同一笔里改的两段注释（E.0 §1）

`preset_picker.rs:14-19` 与 `generation_providers/picker.rs:11-22` 各有一段「为什么 embedding 不可收起」的论证。**不改它们，本次提交就制造了最贵的那种谎 —— 注释是说谎的那一方。**

新论证：可收起，因为「比较」的职责从「人眼扫 5 个数字」搬到了「拿库内真实签名比对」，后者严格更强 —— 它以库的实际状态为基准，而不是要求操作者自己记住库里现在是多少维。

---

## 6. §4 rerank：单例 → 集合（磁盘上只有一个形状）

### 6.1 预设目录搬家

新增 `shared/protocol/src/rerank/providers.rs`：

```rust
pub struct RerankProviderPreset {
    pub name: &'static str,          // 写进配置的 provider type
    pub display_name: &'static str,
    pub default_api_base: &'static str,
    pub default_model: &'static str,
    pub needs_api_key: bool,         // vllm = false
}
// 6 条，逐条对应 RerankProviderType 的 6 个变体，值取自今天 Panel 常量里的那份：
// jina / siliconflow / voyage / pinecone / vllm(needs_api_key=false) / cohere
pub const CONFIGURABLE_RERANK_PROVIDERS: &[RerankProviderPreset] = &[ /* ... */ ];
```

Panel 的 `RERANK_PRESETS` 删除（不注释掉 —— P6）。`icon_color` 留在 Panel 侧（纯呈现，对齐 `preset_providers.rs::color_for` 的既有裁定）。

**census 测试**（对齐 `providers/capability_census.rs`）：`RerankProviderType` 的每个变体都必须有预设行，且清单**从枚举派生**而非手抄。缺行 = 测试期红，而不是 UI 里静默少一个供应商（E2 今天没有这道闸）。

### 6.2 配置结构

磁盘上只有集合，扁平字段变成**加载期投影**：

```toml
[memory.rerank]
enabled = true            # 全局管道开关
active_id = "jina"
rerank_weight = 0.7       # 全局检索调参

[[memory.rerank.providers]]
id = "jina"
provider = "jina"
api_base = "https://api.jina.ai/v1"
models = ["jina-reranker-v2-base-multilingual"]
timeout_ms = 5000
verified = true
```

`RerankConfig` 的 `provider` / `api_base` / `models` / `timeout_ms` / `verified` 加 `#[serde(skip)]`，由**一个** `resolve_active()` 在加载时从 active 条目填。

- **先例**：`api_key` 已经是 `#[serde(default, skip_serializing)]` 的运行时字段（`provider.rs:75-78`）。这是同一个模式，不是新发明。
- **收益**：磁盘上不存在两个形状 —— 避免 E.0 §1 最常见的那种「只改一份就是静默说谎」。
- **E12 的 4 个消费者一行不改**。

字段归属裁定：`enabled` / `active_id` / `rerank_weight` 是全局（管道调参）；`api_base` / `models` / `timeout_ms` / `verified` 是每供应商（对齐 `EmbeddingProviderConfig` 把 `timeout_ms` 放每供应商的既有裁定）。

**`active_id` 悬空时的投影（E.0 §8 fail-closed）**：`active_id` 为空、或指向一个不存在的条目 → `resolve_active()` 投影出 `enabled = false`，**不**回退到「列表里第一条」。「找不到活跃供应商」只有资格说「我不知道」，不许被读作「那就用另一个」—— 悄悄换一个 reranker 是检索结果无声改变的通路。这条要有专门的单元测试。

### 6.3 镜像喂线要钉住（E11）

`AssemblerConfig.rerank` 是 `MemoryConfig.rerank` 的镜像，今天的担保物只有一句注释。迁移后加一条断言：镜像 == `MemoryConfig.rerank` 的投影。**否则主动记忆路径会静默用回旧 reranker** —— 两端都有测试、dead-code 分析对这一类结构性失明（E.0 §7）。

### 6.4 新 RPC 家族

`rerank_providers.{list,get,add,update,remove,set_active,test}`，逐个对齐 `embedding_providers.*` 的语义。

**`remove` 对 active 项的裁定**：拒绝删除当前 active 的 rerank 供应商，错误文案指向「先改活跃项」—— 逐字对齐 `search_config.delete` 对 `default_provider` 的既有行为（E8）。Panel 侧同样把这条拒绝**渲染成禁用态 + 原因**，不是点了才报错（E.0 §14）。删除成功时一并删掉 `rerank:{id}` 的 vault key，并把该 id 从任何引用它的地方清掉 —— 对齐 `search_config.delete` 清 fallback 链的做法。

`rerank_config.{get,update,test}` 的现有消费者**先数一遍**（E.0 §6：数错的方向永远是少一个；grep 前先剥注释行）。TUI / CLI / phone 都要查。真的零消费者 → **删除**，不留注释（P6）。有消费者 → 保留为 active 投影的读写。

### 6.5 迁移

加载时 `providers` 为空且扁平 `provider` / `api_base` 非空 → 生成一条 `id = provider_type` 的条目并置 active。vault key 已是 `rerank:{provider_name}`（E5），密钥直接认领，不需要重新输入。

**这是一次性、不可逆的动作，要盖意图戳**（E.0 §15）：迁移前先写标记，而不是只记录「做完了」—— 只记录完成的机件分不出「没迁」和「迁了但没记上」。崩溃边界上的「未知」不能写成「失败」。

---

## 7. §5 R8 配置工具：一个，不是三个

E13 说 `CATALOG_DESCRIPTION_CEILING_BYTES` 是单向棘轮，三份工具描述要付三份常驻字节。

按既有命名惯例（`channel_manage` / `cron_manage` / `moa_manage` / `hooks_manage` / `heartbeat_manage`），新增**单个**：

```
retrieval_provider_manage
  family: search | embedding | rerank
  action: list | add | update | remove | set_active | test
```

三个家族的字段差异由 `family` 分支校验，描述里只写一次共同语义。`family` 之间不共用参数校验的实现，但共用同一份错误形状。

维度闸（§5.2）在工具面也要在（E.0 §9：一个动词有几张脸，判据就要在每张脸上用同一个推导）—— `set_active` 经工具调用时同样返回维度不符的拒绝，而不是只有 Panel 拦。

---

## 8. §6 真机 QA + §7 手机端孪生面

### 8.1 `qa/retrieval_providers/run.sh`

每个阶段钉一件具体的事：

| 阶段 | 证明什么 |
|---|---|
| `picker` | 空查询在三个家族各返回全量（不是「picker 能打开」） |
| `dims` | 维度不符时 `set_active` **真的被拦** —— 断言配置文件没变，不是断言「弹了个框」（E.0 §4：守卫要断言效果到达了） |
| `delete` | 删默认 search 供应商被拒且按钮为禁用态；删 rerank active 项同样被拒（§6.4）；删非 active 项后 vault key 真的没了 |
| `migrate` | 旧单例 rerank 配置启动后变成一条 active 条目，且 vault 密钥认领成功 |

### 8.2 手机端

`phone/settings/embeddings.rs` 跟改成同一范式（E.0 §16：一边修好的判据要主动搬过去）。手机端没有 search / rerank 设置页，**本次不新建**。

---

## 9. 风险与代价（这个设计让什么变难了）

1. **`RerankConfig` 变成加载期投影后，直接构造 `RerankConfig { .. }` 的测试会静默「正确」但不再代表磁盘状态。** 现有 `rerank_config.rs:366` 就是这么写的。处置：改成走加载路径，或加自保断言。**这是本方案最实在的一笔债，明确记账。**
2. **`search.rs` 拆分与 picker 改造必须分两笔提交** —— 合成一笔 diff 无法 review，且拆分笔的「零行为变化」性质会被淹没。
3. **新增的两类守卫（rerank census、picker 契约）写完各要证伪一次**（E.0 §3）。删一行预设、让某页空查询返回空，确认它们真的红。
4. **`embedding_providers.list` 加字段是 wire 变更** —— 键集要放进两边都依赖的 crate 并**用它构造响应**，否则那个断言测的是 serde（E.0 §10）。

---

## 10. 本次不做（明确列出）

- **不统一四个家族的预设来源机制** —— search + rerank 走 shared 静态表，generation + embedding 走 core RPC，保持现状。各自已有单一真源，不是开着的口子。
- **不新建手机端 search / rerank 设置页**。
- **不动 `search_config` / `embedding_providers` 的 wire 契约**，除 §5.1 那一个新增字段。
- **不碰 `fetch_config` / `FetchProvidersSection`** —— 它在 `search.rs` 里，拆分时原样搬走，不改一行。
- **不做 rerank 的多供应商并发 / 故障转移** —— 集合只是配置簿，一次仍只有一个 active。
- **不改 `providers` / `generation_providers` 两个样板页的行为** —— 它们只被加上 §3.1 的共享契约测试。
