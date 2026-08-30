# Web 搜索面设计（2026-08-30）

> **一句话**：文件搜索那半昨天做完了；`search` 这半的能力**写完了但没接线**——`SearchOptions` 七个字段里五个在全仓零写入点，而它们下游站着 15 个已实现、已测试、已出厂的 provider 译码器。本轮把线接上，并让"接不上的维度"出声而不是静默消失。
>
> 对标项目：`pi-web-access`（24 后端的 web 搜索聚合器）。**架构不照抄**——它的 `fallbackOn: ["unsupported", ...]` 在 Aleph 落成 trait 上的能力自陈 + 排序键。
>
> 范围裁定（2026-08-30 用户）：只做 web 搜索面；`multi_grep` 维持 2026-08-29 的裁定不重提；不支持的维度**重排 + 降级出声**（不是硬拒、不是客户端后置过滤）；描述字节**全付**。

---

## 1. 证据（全部 grep 可复现，不是推演）

| # | 事实 | 锚点 | 判据 |
|---|---|---|---|
| E1 | `date_range` 与 `include_full_content` 在 `src/` + `shared/` + `interfaces/` **零写入点**（除定义文件） | `src/search/options.rs:45,63` | §0 生产者与消费者齐备、中间那根线不存在 |
| E2 | 七个 provider 各自把 `date_range` 翻成原生参数，全部不可达 | `brave_freshness` `bing_freshness` `google_date_restrict` `searxng_time_range` `tavily_days` `ddg_df` `firecrawl_tbs` | 同上 |
| E3 | 七份映射器各写同一张四词表（`day\|week\|month\|year`），且都 `_ => return None` **静默丢弃** | `options.rs:122-260` | 「同一事实的 N 份表述」+「报成功的 no-op」 |
| E4 | 工具面把 `SearchResult` 六字段压成三字段，丢 `provider` / `relevance_score` / `full_content`；全链无发布日期 | `src/builtin_tools/search.rs:152-158` | 附录 D.0.28 |
| E5 | `Ok(vec![])` **终止整条 failover 链** | `src/search/registry.rs:250` | 「空 ≠ 无」的路由形态 |
| E6 | `classify_search_error()` 算出四类失败，唯一消费者是 `log::warn!` | `registry.rs:9-20` | 附录 D.0.14（谓词算出来被扔掉） |
| E7 | `jina` 有 provider + factory + config 文档，Panel `PRESETS` **零卡片**（8 条缺它） | `interfaces/webchat/src/platform/wide/views/settings/search.rs:11-120` | 附录 D.4.9（加了 adapter ≠ 用户能配） |
| E8 | 遗留 Tavily 直连路径**生产可达**，读 `TAVILY_API_KEY`，`SearchOptions` 一个不吃 | `src/executor/builtin_registry/definitions.rs:1023-1026` | 「怎么搜」的第二个答案 |
| E9 | `SearchTool` 无 `max_result_tokens` 覆写（吃全局默认）；`web_fetch` 早有 10k | `src/builtin_tools/search.rs`（无该方法） | 上下文经济缺席 |
| E10 | `search` 的 DESCRIPTION 是一句话、参数只有 `{query, limit}` | `src/builtin_tools/search.rs:83-85` | — |

**两条明确撤回（无证据，不当缺陷修）**：

- `AlephError::Cancelled` 在 search 路径上**没有生产者**（全仓构造点只在 tests 与另外三个错误类型里）。`registry.rs` 那条 match 臂今天不可达。
- 遗留 Tavily 路径**不是**死代码（E8），所以处置是收敛不是 CUT。

---

## 2. §1 工具面契约

### 2.1 `SearchArgs`

```rust
pub struct SearchArgs {
    pub query: String,
    pub limit: Option<usize>,
    pub recency: Option<Recency>,
    pub domains: Option<Vec<String>>,
    pub exclude_domains: Option<Vec<String>>,
    pub full_content: Option<bool>,
    pub provider: Option<String>,
}
```

### 2.2 `Recency` —— 四词表的唯一所有者

```rust
// src/search/options.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Recency { Day, Week, Month, Year }
```

- `SearchOptions.date_range: Option<String>` → **`SearchOptions.recency: Option<Recency>`**（**同批改名**：一件事一个名字；今天叫 `date_range` 而工具面叫 `recency` 会立刻是第二份表述）。
- 七个映射器改成 `match recency { Day => "pd", ... }`，**各自那份四行字面量随之删除**（E3 的四词表从七份变一份）。
- 打错的值由 serde 在**工具入口**拒绝并列出合法值，不再走到第七层变成静默 `None`。
- ⚠️ 这不是"加限制"：今天传 `"7d"` 的结果是一次没有任何时效约束的搜索**而调用方以为它约束了**。

### 2.3 `SearchOptions` 新增两字段

```rust
pub include_domains: Vec<String>,   // 空 = 不约束
pub exclude_domains: Vec<String>,
```

**不做 `site:` 查询折叠**（见 §8 刻意不做）。provider 没有原生参数就声明 `domain_filter: false`，由 §3 的排序键处理。

### 2.4 `SearchOutput` 保真

```rust
pub struct SearchOutput {
    pub results: Vec<SearchResult>,     // + relevance_score / published_date / full_content
    pub query: String,
    pub provider_used: String,
    pub notes: Vec<String>,             // §5 单一源
}
```

- `published_date: Option<String>`：**provider 给了才填，绝不发明**。缺席表示"这个后端没说"，不是"这条没有日期"。
- **逐结果的 `provider` 刻意不上工具面**：本轮是 first-success 链（§8.4 不做合并），一次调用只有一个后端作答 ⇒ 逐结果重复它是零信息。顶层 `provider_used` 就够。合并落地那天它才有第一个消费者（R10：零消费者优先撤回）。
- 输出类型不进 input schema，**不花每请求字节**。

---

## 3. §2 `SearchCapabilities` —— 排序键，不是闸

```rust
// src/search/provider.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchCapabilities {
    pub domain_filter: bool,
    pub recency: bool,
    pub full_content: bool,
}

pub trait SearchProvider {
    /// 默认全 false —— 一个忘了声明的新 provider 不会**假装**支持。
    fn capabilities(&self) -> SearchCapabilities { SearchCapabilities::default() }
}
```

**判据来源**：`ChannelCapabilities` 上栽过的那一次（附录 D.4.11）——「声明了就必须覆写」有个反向那半：**有 `fn` 不等于有能力**，四个通道曾按"有没有 `async fn edit`"全判反。所以守卫钉的是**那个位**与 provider 源码里**真的调了哪个映射器**是否一致（§7 守卫 G1）。

**起始声明值（`recency` / `full_content` 两列是今天从代码读出来的，不是记忆）**——判据是"这个 provider 的源码有没有调对应的映射器"：

| provider | `recency` | `full_content` | `domain_filter` |
|---|---|---|---|
| tavily | ✅ `tavily_days` | ✅ | ⬜ |
| brave | ✅ `brave_freshness` | ❌ | ⬜ |
| bing | ✅ `bing_freshness` | ❌ | ⬜ |
| google | ✅ `google_date_restrict` | ❌ | ⬜ |
| searxng | ✅ `searxng_time_range` | ❌ | ⬜ |
| duckduckgo | ✅ `ddg_df` | ❌ | ⬜ |
| firecrawl | ✅ `firecrawl_tbs` | ✅ | ⬜ |
| exa | ❌ | ❌ | ⬜ |
| jina | ❌ | ❌ | ⬜ |

`domain_filter` **整列从 `false` 起步**——今天没有任何 provider 收得到域名参数（`SearchOptions` 根本没这个字段）。每个 provider 在**接上原生参数的同一笔**里把自己那格翻成 `true`；接不上的就一直是 `false`，由 §4.1 的排序键和 §5 的降级 note 处理。

⚠️ 刻意**不**在本 spec 里预先断言"哪几家的 API 有域名参数"——凭记忆写那张表正是列举法本身，而 G1 守卫会让写错不能静默出厂。

---

## 4. §3 registry 路由与失败报告

### 4.1 候选排序

```
requested = 本次 options 真正要求了哪几个维度
candidates = [default] + fallbacks                 // 配置顺序
stable_sort_by_key(candidates, |p| !p.satisfies(requested))   // 能满足的排前面
```

**稳定排序**：组内保持配置顺序 ⇒ 同一个请求每次落在同一个 provider 上（与 `file_search` 的 `buffered` 保序同一条纪律：确定性优先于一点没人要的重排）。

### 4.2 显式 `provider` 覆写

点名的 provider 不存在或 `!is_available()` ⇒ **响亮失败**，不静默回落。理由：回落会让模型以为它拿到的是它点的那家——「自信的假话」比「我不知道」贵。

### 4.3 空结果继续（E5）

`Ok(vec![])` 不再终止链；记录哪些 provider 答了空。全链皆空 ⇒ `Ok(vec![])` + 一句 note 点名"N 个后端都返回零条"。

⚠️ 边界：这**不是**把空折成错误。空是合法答案，只是不再是**终止**答案。

### 4.4 失败报告结构化（E6）

全链失败仍然返回 `Err`（**不许折成零结果的 `Ok`** —— 「被拒」不许读作「没有」）。改的是消息：

```
search failed on all 3 backend(s):
  tavily [auth] 401 Unauthorized — check the API key in the vault
  brave [rate-limit] 429 — retry later
  searxng [network] connection refused
```

`kind` 来自既有的 `classify_search_error`，于是它的消费者从"一行日志"变成"模型和运维都看得见的那句话"。

⚠️ **它不改路由**：每个 provider 本来就只碰一次，跨 provider 继续在任何一类下都是对的。声称分类驱动了 failover 会是给自己编的用途。

---

## 5. §4 上下文经济

昨天文件搜索落了五个数，web 搜索面一个都没有（E9）。

| 项 | 值 | 理由（写在常量上方） |
|---|---|---|
| `SearchTool::max_result_tokens()` | `Some(8_000)` | 一次搜索可以携带 N 份正文，严格大于一次 `web_fetch`（10k）之下、全局默认之上 |
| `SNIPPET_MAX_CHARS` | `600` | **量纲判据**：grep 行 clamp 到 240 是因为它是**定位器**；web snippet 是**内容**，clamp 得下手轻。走 `utils::text_format::truncate_chars`（UTF-8 安全） |
| `full_content` 单条上限 | `20_000` 字符 | 唯一能让一次 `search` 超过一次 `web_fetch` 的开关 |

**`src/search/notes.rs`（新，单一源）**——四类话各有各的措辞，两个面（工具输出 / 日志）共用：

1. 降级：`domains was not applied by <provider> (no native domain filter)`
2. 回落：`answered by <provider> after <n> backend(s) failed`
3. 截断：`snippets clamped to 600 chars` / `full_content truncated`
4. 全空：`3 backend(s) returned zero results`

判据：`file_search::notes` 的先例——两个工具各写各的会写成"近乎相同但不相同"，而同一件事两种拼法正是读者学会跳过它的方式。

---

## 6. §5 熵减与单一源

### 6.1 收敛遗留 Tavily 路径（E8）

**不是删**——它是零配置路径，真可达。但 §2 落地后它会对**每一个新参数报成功的 no-op**。

改法：无 `[search]` 块但有 `TAVILY_API_KEY` 时，**boot 合成一个单后端 registry**，`SearchTool` 只剩 `with_registry` 一条路径。

- 删除：`TavilyResponse` / `TavilyResult` 两个结构体、`call_impl` 的 legacy 分支、`SearchTool::{new, with_api_key}`、`fallback_timeout` 字段、`DEFAULT_MAX_RESULTS` / `LEGACY_FALLBACK_TIMEOUT_SECS` 两个常量（约 120 行）。
- 更新 `src/tools/traits.rs:377` 的文档示例（它 `SearchTool::new()`）。
- 零 provider 的 registry ⇒ 工具**仍然注册**，调用返回一句点名的"没有配置任何搜索后端"，而不是 tool-not-found。

### 6.2 三份 provider 清单收敛成一份（E7）

新增 `shared/protocol/src/search/providers.rs`：

```rust
pub struct SearchProviderPreset {
    pub name: &'static str,
    pub display_name: &'static str,
    pub needs_api_key: bool,
    pub needs_base_url: bool,
    pub needs_engine_id: bool,
    pub default_base_url: Option<&'static str>,
    pub api_key_placeholder: Option<&'static str>,
}
pub const CONFIGURABLE_SEARCH_PROVIDERS: &[SearchProviderPreset] = &[ /* 9 条，含 jina */ ];
```

- Panel `PRESETS` 从它派生（`jina` 卡片随之出现，**不是**手工再补一条）。
- `factory::ProviderFactoryRegistry::known_provider_types()` 与它做**集合相等**对账。
- `src/config/types/search.rs:88` 那句列举九个名字的文档注释**删除枚举、改为指向常量**——消灭第三份拷贝，而不是同步它。

⚠️ 对账必须**双向**：单向包含式断言对"两边同时缺失"结构性失明（feishu/msteams 那次就是这么照绿四个月的）。

---

## 7. §6 验证

| 层 | 内容 |
|---|---|
| 单测 | 每个维度 × 每个 provider：请求构造输出里那个参数在不在；`Recency` 四值 × 七个映射器的对照表 |
| G1 守卫 | **能力位 ↔ 源码里真的调了映射器**。两级派生：① 从 `options.rs` 源码抽出"读 `self.recency` / `self.include_domains` / `self.include_full_content` 的访问器名字集合"；② 扫每个 `providers/*.rs` 是否调用了其中之一；③ 与 `capabilities()` 字面量比对，**双向**。走 `utils::source_scan::production_prefix`（剥 `#[cfg(test)]`）+ `strip_comment_lines`，并先 `.replace('\r', "")`（CRLF 检出）。自保断言：`checked == providers 目录里的文件数` |
| G2 census | 三处 provider 清单**集合相等**（protocol ↔ factory ↔ Panel） |
| G3 守卫 | `search` 的 DESCRIPTION 里点名的每个参数都在 `SearchArgs` 的 schema 里（散文与 schema 不许分家） |
| 棘轮 | `catalog_description_bytes_ratchet` / `registry_schema_bytes_ratchet`（`src/executor/builtin_registry/definitions.rs`）**实测后**抬高，账本写**逐项分解**而不是算术。⚠️ 附录 C.1：`(measured)` 只覆盖端点不覆盖因果 |
| 真机 QA | `qa/web_search/run.sh`，四阶段（下） |

### 7.1 QA 装置

**靶子只能是 SearXNG**——九个 provider 里只有 `searxng` / `firecrawl` 有 `base_url`，其余七个硬编码端点。SearXNG 还恰好不要 API key。这是装置形状的**约束**，写进脚本自己的 doc。

| 阶段 | 断言 |
|---|---|
| `reach` | 一次真回合：模型调 `search{recency:"week"}` ⇒ mock SearXNG 的 request log 里有 `time_range=week`。**这是"参数真的上了线"唯一的 oracle**——进程内测试对此结构性失明 |
| `order` | 配两个后端（一个声明 `domain_filter`）⇒ 带 `domains` 的请求打到**支持的那个** |
| `degrade` | 只配不支持 `domains` 的后端 ⇒ 结果里有降级 note，且**先锚后否**（先断言这次搜索真的返回了结果，再断言 note 在） |
| `empty` | mock 返回零条 ⇒ 链继续问第二个后端（E5 的回归） |

复用 `qa/busy_input/mock_anthropic.py` 的每回合 tool_spec + tool-chain 计划（`qa/file_search` 的先例）。驱动脚本**内容寻址**，不按回合号索引（一次 run 以一个不带工具面的 planner 调用开场）。

**装置的已知边界**（写进脚本 doc）：只证**接线**，不证九个后端各自的正确性——其余八个的 wire 断言留在单测层。

---

## 8. §7 刻意不做（附重访条件）

1. **扩后端表**（bocha / kimi / xai-grok / openai-web_search / gemini / search1api / brightdata…）。现有 9 个的能力今天有 5/7 到不了模型，第 10 个只会是第 10 个够不到的东西。**重访条件**：`SearchCapabilities` 落地后，某个维度**所有**现有 provider 都不支持时。
2. **`multi_grep`**（2026-08-29 裁定，2026-08-30 复核维持）。
3. **`site:` 查询折叠**替代原生域名参数。它是"域名过滤"的第二个答案，会改写模型看得见的 query，多域名还要自己拼 OR。**重访条件**：`domain_filter: true` 的 provider 少到排序键买不到东西时。
4. **跨 provider 结果合并 / 去重**。要求"同一条结果"有身份（URL 归一化 + 内容指纹），是另一轮。今天是 first-success 链，本轮不改这个语义。
5. **`AlephError::Cancelled` 那条 match 臂**——今天不可达（§1 撤回项）。
6. **`[search]` 的新配置旋钮**。三个上限都在 note 里点名自己的杠杆；加设置就是给"一次搜索能有多大"造第二个答案（R10）。

---

## 9. 改动面

| 文件 | 动作 |
|---|---|
| `src/search/options.rs` | `Recency` 枚举；`include_domains` / `exclude_domains`；七个映射器改吃枚举 |
| `src/search/provider.rs` | `SearchCapabilities` + trait 默认方法 |
| `src/search/providers/*.rs`（9） | 各自 `capabilities()`；支持域名的接原生参数 |
| `src/search/registry.rs` | 排序键 · 空结果继续 · 显式 provider · 结构化失败 |
| `src/search/notes.rs` | **新** 单一源 |
| `src/search/factory.rs` | 与 protocol 常量对账 |
| `src/builtin_tools/search.rs` | `SearchArgs` 四参数 · 输出保真 · `max_result_tokens` · **删 legacy 路径约 120 行** |
| `src/executor/builtin_registry/definitions.rs` | 构造分支简化 · 两个棘轮抬高 |
| `src/executor/builtin_registry/builder/constructor/mod.rs` | 同上 |
| `src/config/types/search.rs` | 文档注释删枚举、指向常量 |
| `src/tools/traits.rs` | 文档示例更新 |
| `shared/protocol/src/search/providers.rs` | **新** 单一源常量 |
| `interfaces/webchat/.../settings/search.rs` | `PRESETS` 改为派生（jina 随之出现） |
| `qa/web_search/run.sh` | **新** 四阶段装置 |
| `docs/reference/FEATURE_LOCATOR.md` | §3.6 web 搜索轮记录 |
| `CLAUDE.md` 判据清单 | 若本轮产出新判据则加**触发器**（全文进附录 D） |

---

## 10. 风险

| 风险 | 缓解 |
|---|---|
| 抬棘轮抬多了 | 实测非手算；账本写逐项分解；附录 C.1 的"归因方向"陷阱 |
| 能力位写错 | G1 守卫双向对账 + 自保断言；写完**变异一次**证明它会红 |
| 删 legacy 路径打断零配置装机 | boot 合成单后端 registry；QA 之外补一条"只有 env key 时 `search` 仍可用"的单测 |
| `Recency` 枚举化是 wire 破坏 | `date_range` 今天**零写入点**（E1），没有存量调用者可破坏 |
| QA 装置只能测 SearXNG | 已写进装置 doc；其余八个留单测层 |
