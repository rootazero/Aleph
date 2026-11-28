# 断线审计：src/generation · src/goal · src/group_chat / Severed-wire audit: src/generation · src/goal · src/group_chat

基线 / Baseline: `main` @ `1b959fdf4` · 审计分支 / Audit branch: `fix/severed-wire-audit`（工作树 / worktree: `.worktrees/sev-wire-fix`）

## 范围与方法 / Scope and method

审计三个模块的**静态**接线，不做基于 diff 的复核：每个发现都从**生产者**（谁生成这个值）出发，用 `rg` 找到**消费者**（谁读它），再逐个确认该消费者是否在生产路径上。

The audit is a **static** wiring review of three modules — no diff-based review. Every finding starts from the **producer** (who emits the value), follows `rg` to the **consumer** (who reads it), and then checks whether that consumer is on a production path.

七个镜头 / Seven lenses（来自 `severed-wire-audit` 技能的接缝目录 / from the skill's seam catalog）：注册平价 / registration parity · 调用对处理器平价 / call-vs-handler parity · 分类器对处理器平价 / classifier-vs-handler parity · 事件发出对订阅平价 / event emit-vs-subscribe parity · 配置读取平价 / config-reader parity · 路径路由平价 / path/route parity · 桩清扫 / stub sweep。

八条切片，八个只读子代理（`sev-auditor`，`minimax-cn/MiniMax-M3`）。每条切片产出一份带表格的发现台账，并写明已核验为「干净」的符号。

Eight slices, eight read-only subagents (`sev-auditor`, `minimax-cn/MiniMax-M3`). Each slice produced a tabular ledger of findings plus the symbols it verified as clean.

| 切片 / Slice | 模块 / Module | 报告 / Report |
| --- | --- | --- |
| s1a-gen-registry-types | `src/generation/registry.rs` · `probe.rs` · `voice_catalog.rs` · `types/*` | `.superpowers/sdd/2026-11-28-severed-wire-audit/s1a-gen-registry-types.md` |
| s1b-gen-trait-errors | `src/generation/mod.rs` · `error.rs` | `…/s1b-gen-trait-errors.md` |
| s2-gen-providers | `src/generation/providers/**` | `…/s2-gen-providers.md` |
| s3-goal-api | `src/goal/{mod,types,pursuit}.rs` | `…/s3-goal-api.md` |
| s4-goal-store | `src/goal/store.rs` | `…/s4-goal-store.md` |
| s5-gc-api | `src/group_chat/{mod,channel,coordinator,protocol,persona}.rs` | `…/s5-gc-api.md` |
| s6-gc-exec | `src/group_chat/{executor,session,orchestrator}.rs` | `…/s6-gc-exec.md` |
| s7-cross-generation | 生成侧远端 / generation far end | `…/s7-cross-generation.md` |
| s8-cross-groupchat | 群聊远端 / group-chat far end | `…/s8-cross-groupchat.md` |

审计者读的是 `main` 的工作副本；本轮修复全部落在隔离工作树 `.worktrees/sev-wire-fix`，最后快进回 `main`，不建 PR。

The auditors read the `main` checkout. Every fix in this round landed in the isolated worktree `.worktrees/sev-wire-fix`, which was fast-forwarded into `main` at the end. No pull request was opened.

## Jev 的使用 / Where Jev was used

Jev（TypeSafe System One，`jev_evaluate`）只用于**分类与路由判断**，不用于生成代码或替代审计：

Jev (TypeSafe System One, `jev_evaluate`) was used only for **classification and routing decisions** — never to write code and never in place of the audit itself:

| 步骤 / Step | Jev 输入 / Input | 结论 / Outcome |
| --- | --- | --- |
| s2 分组处置 / s2 disposition | 三条候选发现的「生产消费者是否存在」陈述 / three candidate findings, with their producer/consumer facts | `SW-S2-GEN-PROVIDERS-3` → DECIDE（置信 0.85）/ `SW-S2-GEN-PROVIDERS-8` → DECIDE（0.80）/ `SW-S2-GEN-PROVIDERS-4` → CONNECT（0.26，近似平票，按判断处理 / near tie, resolved by judgement） |
| 总体置信 / overall confidence | 三条发现构成真实断线的可能性 / likelihood that the three are genuine severed wires | 7.08 / 10 |

工具契约修正 / Tool-contract correction：`jev_evaluate` 的 `questions` 必须是**映射**（id → `{type, instructions, criteria}`），`choice` 的 `criteria` 必须是**标签→描述的映射**，`score` 的 `criteria` 必须是**按分数从零索引的列表**；首两次调用因形状不对被拒，第三次成功。

The contract: `questions` must be a **map** (id → `{type, instructions, criteria}`); a `choice` question's `criteria` must be a **map of label → description**; a `score` question's `criteria` must be a **list indexed by score from zero**. The first two calls were rejected for shape, the third succeeded.

其余步骤（读码、grep 验证、修复、守卫）**未使用** Jev——它们是事实核验，不是分类判断。

Everything else — reading, `rg` verification, fixing, guarding — **did not** use Jev: those are factual checks, not classification judgements.

## 发现与处置 / Findings and disposition

共 85 条 / 85 findings。处置栏含义 / Disposition legend：**FIX** 已修复并有回归守卫 · **FIX+GUARD** 已修复并加普查守卫 · **REPORT** 已核验但未改（理由见下）/ verified but unchanged (reason given).

### 群聊 / group_chat（22 条 / 22 findings）

| ID | 形式 / Form | 严重度 | 处置 | 说明 / Note |
| --- | --- | --- | --- | --- |
| SW-S5-GC-API-1 / SW-S6-GC-EXEC-1 / -2 | 1 无消费者 | 高 | FIX+GUARD | `group_chat_sessions` / `group_chat_turns` 两张表从未被写入：两个 `with_database` 构造器零调用方。启动接线已传入 `agent_result.state_db`。 / The two tables were never written: both `with_database` builders had zero callers. The startup wiring now passes `agent_result.state_db`. |
| SW-S6-GC-EXEC-3 | 1 无消费者 | 高 | FIX | `get_group_chat_turns` 随接线一并接通。 / The read-back half came live with the wiring. |
| SW-S5-GC-API-2 / SW-S6-GC-EXEC-4 / -5 | 3 无效配置 | 高 | FIX | `[[personas]].provider` 一直无效：执行器的 provider registry 生产环境恒为 `None`。 / The persona `provider` override never resolved: the executor's registry was always `None` in production. |
| SW-S5-GC-API-3 / SW-S6-GC-EXEC-6 | 1 无消费者 | 中 | FIX（CUT） | `CoordinatorPlan.need_summary` 只被要求、只被解析、从未被读。合成功能从未实现，于是删掉这个字段与提示词里的那一行，而不是留着它骗模型。 / The flag was requested, parsed and never read. Synthesis was never built, so the field and its prompt line are gone rather than left to mislead the model. |
| SW-S5-GC-API-4 / SW-S6-GC-EXEC-7 | 5 命名漂移 | 中 | FIX | `dropped_targets` 一直由执行器产生、被序列化器丢弃。网关与频道两处投影都补上了它。 / The executor always carried it; the serializers dropped it. Both projections now include it. |
| SW-S5-GC-API-6 | 5 命名漂移 | 低 | FIX | 配置期校验的 thinking level 白名单与 `ThinkLevel`（六档）漂移，拒绝 `off`/`minimal`；改为直接读 `ThinkLevel::ALL`。 / The config-time whitelist had drifted from `ThinkLevel` (six levels) and rejected `off`/`minimal`; it now reads `ThinkLevel::ALL`. |
| SW-S5-GC-API-7 | 2 桩远端 | 低 | FIX | 无 provider key 时占位 handler 报「需要先接 Gateway」，真因是「没有配置 AI provider key」。 / The placeholder blamed the Gateway; the real cause is a missing provider key. |
| SW-S6-GC-EXEC-8 | 1 无消费者 | 低 | FIX | `source_channel` 随数据库接线进入 `insert_group_chat_session`。 / It now reaches the session row. |
| SW-S5-GC-API-5 | 1 无消费者 | 中 | REPORT | `group_chat.*` 六个 RPC 已注册但仓库内没有客户端：Panel 没有群聊界面。这是产品缺口，不是接线断裂。 / The six RPCs are registered with no in-repo client — the Panel has no group-chat UI. A product gap, not a severed wire. |
| SW-S5-GC-API-8 / -9 | 1 无消费者 | 低 | REPORT | `channel.rs` 宣称的频道适配 trait 与渲染器不存在；`GroupChatRequest` / `GroupChatMessage` 的 serde + JsonSchema 契约无人使用。 / A channel-adapter abstraction with no implementation, and a wire contract nobody reads. Accepted debt. |
| SW-S8-6 | 1 无消费者 | 中 | FIX | 启动回复里没有 session id，用户拿不到可以 `continue` / `mention` / `end` 的那个 id。现在回复里带 `Session: {id}`。 / The start reply never named the session, so the user had no id to address. It does now. |
| SW-S8-7 | 1 无消费者 | 中 | FIX | 统一路由把每一条 `/groupchat …` 都吃掉了：认不出来的写法（**包括工具自己的用法提示里写的那一种**）既不报错、也不回话、更不会落到 agent。现在认不出来时回一份用法；群聊未启用时也回一句真因。 / An unrecognised `/groupchat …` was swallowed: no reply, no error, no agent fallback. A usage reply now answers it. |
| SW-S8-8 | 5 命名漂移 | 中 | FIX | 工具元数据里的用法提示 `/groupchat start <personas> [topic]` 与解析器真正的语法（`--preset` / `--role` / `--topic`）对不上——照着提示写必然解析不出 persona。提示改成解析器认的写法。 / The catalogued usage described a spelling the parser rejects. |
| SW-S8-10 | 5 命名漂移 | 中 | FIX | `group_chat.list` 吐 `id`，而 continue / mention / end / history 读的是 `session_id`。现在两个键都给。 / The list handed out `id` while every addressed method reads `session_id`. |
| SW-S8-9 | 3 无效配置 | 中 | REPORT | `method_visibility` 里那张记着五个 `group_chat.*` 方法可见性裁定的表没有生产消费者；真正的裁断散落在 `stamped_owner_visible` 的逐处检查里。把散落的检查收成一个闸口是网关设计改动，不在本次审计范围内。 / The curated visibility table has no production consumer; the enforcement is ad hoc. Promoting the table to a chokepoint is a gateway design change. |
| SW-S8-11 | 1 无消费者 | 低 | REPORT | `MEMBER_CARVE_OUTS` 里列了 `group_chat.start`，但 `group_chat.` 家族不在 `ADMIN_PREFIXES` 里，于是这条 carve-out 是空的。 / The carve-out is inert because the family is member-open by default. |

### 目标 / goal（7 条 / 7 findings）

| ID | 形式 / Form | 严重度 | 处置 | 说明 / Note |
| --- | --- | --- | --- | --- |
| SW-S4-GOAL-STORE-1 | 1 无消费者 | 中 | FIX+GUARD | `GoalStore::put` 零生产调用方，而且是**错的**写入者：它不写 origin 行，于是目标要么永远触发不了（origin 属于别的目标），要么被静默降级成普通目标。改名为 crate 私有的 `put_without_origin`，并加普查守卫。 / `put` had zero production callers and was the WRONG writer: it never wrote the origin row. Renamed to the crate-private `put_without_origin`, with a census guard. |
| SW-S4-GOAL-STORE-2 | 1 无消费者 | 中 | FIX+GUARD | `origin_for` 零生产读取方，且在认领临界区之外读 origin，会重开「快照后再认领」的竞态窗口。改为 crate 私有并加守卫。 / `origin_for` read the origin outside the claim's critical section. Now crate-private, with a guard. |
| SW-S4-GOAL-STORE-3 | 1 无消费者 | 低 | REPORT | `try_claim_continuation`（未绑定认领）按设计不可从生产代码调用，且由 `goal_origin_race.rs` 的普查钉住。保留为文档化的陷阱。 / Deliberately unreachable from production and pinned by a census. Kept as a documented trap. |
| SW-S3-GOAL-API-1 | 1 无消费者 | 低 | FIX | `WaitPark::Task` 携带了一个重复的任务 id（权威副本在 `Goal.waiting_on_task`），改成单元变体。 / `WaitPark::Task` carried a duplicated task id; now a unit variant. |
| SW-S3-GOAL-API-2 | 1 无消费者 | 低 | FIX | 文档里的「21 个生产调用点」实为 23。 / The doc said 21 production call sites; the real number is 23. |
| SW-S3-GOAL-API-3 | 5 命名漂移 | 低 | FIX | 一条过期注释声称 `goal` 与 `looping` 都不持久化根目录——`goal` 其实持久化了自己的副本。 / A stale comment claimed `goal` does not persist its root; it does. |
| SW-S3-GOAL-API-4 | 2 桩远端 | 低 | FIX | `action='set'` 静默丢弃 `status` / `lesson`，而工具自身的策略是「拒绝而不是忽略」。 / `action='set'` silently dropped `status` / `lesson` while the tool's own policy is to refuse, not ignore. |

### 生成 / generation（56 条 / 56 findings）

| ID | 形式 / Form | 严重度 | 处置 | 说明 / Note |
| --- | --- | --- | --- | --- |
| SW-S1B-GEN-TRAIT-ERRORS-1 | 1 无消费者 | 高 | FIX | `From<GenerationError> for AlephError` 这套完整分类在生产里零消费者——四个生成工具先绕道 `ToolError`，把每一次 provider 失败塌成「执行失败」。工具现在直接走 `AlephError`。 / The whole rich classification was dead: the tools hopped through `ToolError` and collapsed every provider failure into "execution failed". |
| SW-S1B-GEN-TRAIT-ERRORS-2 | 2 桩远端 | 高 | FIX | 限流 / 超时因此变成**不可重试**，并被写进 harness 的跨批次失败备忘录，整个 run 内再也不会重试。工具改走 `AlephError` 后，限流变回 `RateLimited`（仓库自己的分类法就把它算作 transient），并给 `registry_adapter` 的可重试判据补上 `RateLimitError`。 / A rate limit became non-retryable and was banned for the rest of the run. |
| SW-S1B-GEN-TRAIT-ERRORS-3 | 2 桩远端 | 中 | FIX | 认证失败被报成「参数非法」。 / An authentication failure was reported as "invalid arguments". |
| SW-S1B-GEN-TRAIT-ERRORS-4 / -5 / -6 | 1 无消费者 | 中 | FIX | `needs_user_action` / `should_fallback` / `user_friendly_message` 从未被调用。现在：映射层据此补上一句可执行提示，登记被拒的日志也带上可执行文案。 / The three helpers now drive the actionable hint in the error mapping and the actionable text in the registration logs. |
| SW-S1B-GEN-TRAIT-ERRORS-15 | 5 命名漂移 | 中 | FIX | `openai_compat` 的静态音色表声称有 OpenAI 音色，而活注册表对它返回 `[]`——两个访问器漂移了。静态表现在只覆盖真正实现 `list_voices` 的类型。 / The static catalog claimed OpenAI voices for a provider whose live list is empty. |
| SW-S1B-GEN-TRAIT-ERRORS-16 | 2 桩远端 | 低 | FIX | `supports_image_editing()` 恒为 `true`，而派生的 edits URL 对不提供 `/edits` 的端点会悄悄退回生成 URL。改为「配置了 edit endpoint 才算支持」。 / It claimed editing support unconditionally; now it requires a configured edit endpoint. |
| SW-S1B-GEN-TRAIT-ERRORS-17 | 2 桩远端 | 高 | FIX | 登记被拒（重名 / 同类型）时错误被 `.is_ok()` 吞掉：第二个能力（本地语音 provider 的第二项能力）被静默丢弃。现在两处登记路径都记录警告与可执行文案。 / A refused registration was swallowed and a capability silently dropped. Both registration paths now warn. |
| SW-S1B-GEN-TRAIT-ERRORS-19 | 1 无消费者 | 低 | FIX | `Cancelled` 没有生产者；OpenAI 兼容路径把 provider 的 cancelled 状态报成「任务失败」。现在报成 `Cancelled`。 / A cancelled job was reported as a generic job failure. |
| SW-S1B-GEN-TRAIT-ERRORS-18 | 2 桩远端 | 中 | REPORT（部分） | 频道路径的 TTS 失败会回落到纯文本（这是设计），但类型化错误只留在一行 `warn!` 里。现在日志带上可执行文案；把错误一路送到用户面前要改 `TtsOutcome` 契约，属于产品决定。 / The channel path falls back to text by design; the typed error only lived in a `warn!` line, which now carries the actionable text. |
| SW-S1B-GEN-TRAIT-ERRORS-7 / -8 | 1 无消费者 | 中 / 低 | REPORT | `retry_after()` 与 `provider_name()` 无消费者。前者的信息已经折进 `AlephError` 的 suggestion，后者的 provider 名已经在消息里。 / Their information already travels through the AlephError message and suggestion. |
| SW-S1B-GEN-TRAIT-ERRORS-9 | 1 无消费者 | 中 | REPORT | `GenerationProvider::color()` 被 ~19 个 provider 覆写却无人读；Panel 读的是配置里的 `color`。两者现已统一（配置里显式选的颜色会覆盖品牌色），但 trait 方法本身仍无生产消费者。 / ~19 overrides with no production reader; the Panel reads the config colour. The overlay now carries the operator's colour, but the trait method itself stays unread. |
| SW-S1B-GEN-TRAIT-ERRORS-10 | 5 命名漂移 | 中 | REPORT | trait 的 `default_model()` 与配置的 `default_model()` 两个真相源；trait 侧仍然没有生产读者。 / Two sources of truth for "the default model"; the trait side stays unread. |
| SW-S1B-GEN-TRAIT-ERRORS-11 / -12 | 1 无消费者 | 中 | REPORT | `check_progress()`（fal 实现了真队列轮询）与 `cancel()`（无人实现、无人调用）没有生产调用方。补齐它们是新功能，不是接线修复。 / Fal's real queue polling and the unimplemented `cancel()` have no production caller. |
| SW-S1B-GEN-TRAIT-ERRORS-13 / -14 | 1 / 3 | 中 | REPORT | `edit_image()` 无人调用；`edit_url` 配置项被保存但没有行为消费它。图像编辑工具不存在——这是功能缺口。 / The image-edit path has no caller and `edit_url` has no behavioural consumer. |
| SW-S1B-GEN-TRAIT-ERRORS-20 | 1 无消费者 | 低 | REPORT | `MockGenerationProvider` 随生产库发布。保留（跨 crate 的测试替身），记为已知债务。 / A test double ships in the production library. Kept, recorded as debt. |
| SW-S1B-GEN-TRAIT-ERRORS-21 | 1 无消费者 | 中 | REPORT | `retry_transient` 只有 1 个 provider 在用，其余 provider 收到 429 直接失败。harness 层已可重试（限流回到 transient），provider 层的逐个重试是更大的改动。 / Only one provider retries a 429. The harness level is fixed; per-provider retry is a larger change. |
| SW-S2-GEN-PROVIDERS-1 / SW-S1A-18 | 3 无效配置 | 中 / 高 | FIX | `[generation.providers.X.defaults]` 的 14 个字段（含 `width`/`quality`/`style`/`n`/`duration_seconds`…）从未生效——`to_params()` 只有测试调用方。新增 `DefaultsOverlay` 装饰器，把配置默认值铺在工具参数**之下**。 / 14 default fields never applied; a `DefaultsOverlay` now lays the configured defaults UNDER the tool's arguments. |
| SW-S2-GEN-PROVIDERS-6 | 3 无效配置 | 中 | FIX | `model_aliases` 只有 replicate 一个分支认；现在 20 个分支全部解析别名。 / Only one of 20 factory arms honoured `model_aliases`; all of them do now. |
| SW-S2-GEN-PROVIDERS-2 | 5 命名漂移 | 中 | REPORT | `openai_compat` 一个 provider 只解析一个端点，图像与视频共用一个 URL。内置预设靠「整条 URL 存进 `base_url`」侥幸躲开；自定义端点仍会把视频发到图像端点。 / One endpoint per provider: image and video share a URL. Built-in presets dodge it by storing a full URL. |
| SW-S2-GEN-PROVIDERS-3 | 1 无消费者 | 中 | FIX（CUT） | `openai_image::edits_url` 与 `ResolvedUrl::secondary_endpoint` 均无消费者（后者只有一个读者，正是前者），一并删除。 / Both dead; the secondary-endpoint concept had exactly one reader. |
| SW-S2-GEN-PROVIDERS-4 | 1 无消费者 | 低 | FIX | google_imagen 与 google_veo 各有一个同名 `is_valid_aspect_ratio`，取值集合不同（Imagen 接受 1:1/4:3，Veo 不接受）。Veo 的改名，Imagen 的改为调用自己的那份。 / Two same-named validators with different allow-lists. |
| SW-S2-GEN-PROVIDERS-5 | 1 无消费者 | 低 | FIX（CUT） | `IMAGE_SIZES` 随之删除（provider 从不发送 `imageSize`）。 / Deleted with its only reader. |
| SW-S2-GEN-PROVIDERS-7 | 5 命名漂移 | 低 | FIX | 「支持的 provider 类型」文档只列 9 组，实际有 20 个类型 / 34 个别名。两个模块文档现在都写全。 / The doc listed 9 of 20 types; both module docs now list them all. |
| SW-S2-GEN-PROVIDERS-8 | 3 无效配置 | 中 | FIX | 20 个分支里只有 3 个应用 `config.color`，其余硬编码品牌色。现在配置里显式选的颜色统一覆盖。 / Only 3 of 20 arms applied `config.color`. The overlay now carries it uniformly. |
| SW-S2-GEN-PROVIDERS-9 | 3 无效配置 | 中 | REPORT | `config.edit_url` 被设置面板写入并持久化，但只有 `openai_compat` 会读。 / The Settings UI offers it for every custom provider; one arm reads it. |
| SW-S1A-01 | 1 无消费者 | 高 | REPORT | `GenerationProgress` 只由 fal 生产，`check_progress` 没有生产调用方。进度面是暗的——但补齐它是新功能。 / Only fal produces it and nothing calls `check_progress`. |
| SW-S1A-02 | 1 无消费者 | 中 | REPORT | `cancel()` 无实现、无调用方。 / No implementation, no caller. |
| SW-S1A-03 | 1 无消费者 | 中 | REPORT | `edit_image()` 只在测试里被调用；工具只走 `generate`。 / Only the tests call it. |
| SW-S1A-04 | 1 无消费者 | 高 | FIX | `additional_outputs` 被 6 个 provider 填充却无人读——`n=4` 的四张图付了钱只交付一张。图像与音频工具现在把额外产物作为额外媒体项交付；Suno 的备选音轨也一并送达。 / Six providers filled it and nobody read it: `n=4` cost four and delivered one. The image and audio tools now deliver the extras, and Suno's alternative tracks ride along. |
| SW-S1A-05 | 1 无消费者 | 高 | REPORT | `metadata.extra` 被 4 个 provider 填充却无人读（provider 私有诊断数据）。把一整块 metadata 塞进每次工具输出会有 token 代价，是产品决定。 / Filled by four providers, read by nobody. Surfacing it wholesale is a token-cost decision. |
| SW-S1A-06 | 1 无消费者 | 中 | REPORT | `metadata` 的若干字段写入后无人读。 / Several metadata fields are written and never read. |
| SW-S1A-07 | 1 无消费者 | 中 | REPORT | `request_id` / `user_id` 从未被写入生产路径：provider 的任务 id 到不了调用方，OpenAI 的 `user` 字段从未发送。接通需要会话→工具的 id 管道。 / Never written in production; wiring them needs a session→tool id channel. |
| SW-S1A-08 | 1 无消费者 | 高 | REPORT | `GenerationRequest::transcription()` 零调用方：Whisper / Deepgram-STT provider 已注册、已列出，却不可达。 / The STT providers are registered and listed but unreachable. |
| SW-S1A-09 | 1 无消费者 | 中 | REPORT | 12 个 `GenerationParams` 字段没有生产生产者。 / 12 parameter fields have no production producer. |
| SW-S1A-13 | 1 无消费者 | 中 | REPORT | `GenerationProviderRegistry` 的 `remove` / `clear` / `iter` / `contains` 没有生产调用方。 / Four registry methods have no production caller. |
| SW-S1A-10 / -11 / -12 | 1 无消费者 | 低 | REPORT | `GenerationParamsBuilder` 与若干访问器只有测试调用方。它们是 `alephcore` 这个库 crate 的一套连贯公开 API，删除属于 API 清理而不是接线修复；记为已知债务。 / A coherent public accessor family of the library crate — recorded as accepted debt rather than cut. |
| SW-S1A-14 | 1 无消费者 | 低 | FIX | `CATALOG_GENERATED_AT` 无人读，而模块文档声称设置页会显示「目录截至…」。改为私有（仅测试）常量并修正文档——设置 RPC 返回的是裸 `Vec<VoiceInfo>`，要塞进日期就要改 Panel 契约。 / Its doc claimed the Settings payload shows a catalog date; it does not. The constant is now private and the doc corrected. |
| SW-S1A-15 / SW-S2（别名） | 5 命名漂移 | 高 | FIX | 探测按**原始** `provider_type` 查预设，别名（`tts`/`dalle`/`mj`/`veo`…）查不到，于是正常工作的 provider 被回「未配置 base_url，无法测试连接」。探测现在用工厂同一张别名表规范化。 / The probe looked presets up by the raw type; aliases found nothing and a working provider answered "no base_url configured". The probe now canonicalizes with the factory's own alias table. |
| SW-S1A-16 | 5 命名漂移 | 中 | FIX | `NO_MODELS_ENDPOINT` 漏了别名，404 被判成「连接成功」，并把 `verified = true` 写进配置。 / A 404 was reported as success and `verified = true` was persisted. |
| SW-S1A-17 | 5 命名漂移 | 高 | FIX | 探测用 GET 打一个 POST 端点，405/404 被判成成功，`verified = true` 被持久化——徽章是假的。现在**只有 2xx 才算验证通过**。 / A GET against a POST endpoint was reported as success. Only a 2xx verifies now. |
| SW-S1A-19 | 5 命名漂移 | 低 | FIX | `static_voices_for_provider_type("openai")` 返回 OpenAI 音色，而工厂把 `openai` 映射到图像 provider；目录还列了工厂拒绝的拼写。 / The catalog listed OpenAI voices for the image provider, plus spellings the factory rejects. |
| SW-S7-CROSS-GENERATION-6 | 3 无效配置 | 高 | FIX+GUARD | 「设为默认」写进了配置、也画成了徽章，但四个生成工具是用 `first_for_type()` 选 provider 的——同类型里**按字母序第一个**。换个默认值只换徽章，不换调用。现在注册表记住每个类型的默认 provider，工具先问默认、再回退。 / "Set as default" changed the badge, not the call. The registry now carries the operator's default per type. |
| SW-S7-CROSS-GENERATION-7 | 5 命名漂移 | 高 | FIX | 本地语音端点默认 `[voice.local] tts_format = "opus"`，服务端回 `audio/ogg`；而语音回合路径只认 mpeg/mp3/wav，于是**默认配置下每一回合都失败**（`unsupported_output`）。Panel 把 mime 原样交给播放器，浏览器能放 opus，于是回合路径现在认 ogg/opus。 / The default local-voice configuration failed every turn: opus was produced and refused. |
| SW-S7-CROSS-GENERATION-3 / -4 | 3 无效配置 | 中 | REPORT | `generation.output_dir` / `auto_paste_threshold_mb` / `background_task_threshold_seconds` 存在配置里、在 Panel 里往返，但没有任何代码消费它们。前者是生成工具从不落盘造成的，后者是 Panel 侧的概念。文档改写为 RESERVED 并说明原因；删掉它们要改协议与 Panel，属于产品决定。 / Three knobs round-trip to the Panel and are read by nobody. Their docs now say RESERVED and why. |
| SW-S7-CROSS-GENERATION-8 | 5 命名漂移 | 中 | REPORT | 旧的 `[generation.providers.*]` 条目会被校验、列出、注册，但两个 STT 解析器只读 `transcription_providers`——老配置里的转写 provider 永远解析不到。补上回退读法要动配置形状（哪些算合法来源），属于产品决定。 / The legacy provider map is registered but the STT resolvers read only `transcription_providers`. |
| SW-S7-CROSS-GENERATION-5 | 1 无消费者 | 低 | FIX（CUT） | `GenerationConfig::get_default_provider` 只有测试调用方，删除。 / Deleted: a test-only accessor. |
| SW-S7-CROSS-GENERATION-9 | 1 无消费者 | 中 | FIX | `resolve_model` 只有测试调用方，`model_aliases` 只有 replicate 一个分支认。见 SW-S2-GEN-PROVIDERS-6。 / See SW-S2-GEN-PROVIDERS-6. |

### 自查发现 / Findings of my own

| ID | 形式 | 严重度 | 处置 | 说明 |
| --- | --- | --- | --- | --- |
| 自查-下载状态码 | 2 桩远端 | 高 | FIX | `download_with_status` 零调用方，`DownloadError.status_code` 恒为 `None`，于是 5xx/408 的可重试判据永远不成立；两个 HTTP 下载点也确实丢掉了自己手上的状态码。 / The status code was always `None`, so the retryable 5xx/408 arms were unreachable; both download sites were discarding the status they held. |
| 自查-音色目录别名 | 5 命名漂移 | 中 | FIX | 见 SW-S1A-19。 |
| 自查-默认值叠加层 | 3 无效配置 | 高 | FIX | 见 SW-S2-GEN-PROVIDERS-1。 |
| 自查-群聊持久化与 provider 覆写 | 1 / 3 | 高 | FIX | 见 SW-S5/S6 的第 1、2、4、5 条。 |

## 修复要点 / What the fixes do

**错误分类不再塌缩 / The error taxonomy no longer collapses.** 四个生成工具（图像、音频、视频、语音）现在把 provider 失败映射为 `AlephError`，走 `src/generation/error.rs` 里那套本来就写好、却零消费者的 `From<GenerationError> for AlephError`。限流回到 `RateLimited`（仓库自己的 `ToolErrorKind::is_transient` 本来就把它算作 transient），认证失败回到 `Authentication`，并且映射层会补上一句「去设置里检查 API key」这类可执行提示。

**默认值与别名真的生效了 / Defaults and aliases now land.** `DefaultsOverlay` 装饰器把配置的默认值铺在工具参数之下（工具给的仍然优先），并解析 `model_aliases`；20 个工厂分支现在都认别名。

**生成的多张图不再只交付一张 / Extra outputs are delivered.** `additional_outputs` 从此进入工具输出与媒体项；Suno 的备选音轨也一并送达。

**探测只认 2xx / The probe only trusts a 2xx.** 别名先规范化到工厂的同一张表；404/405 不再被判成「连接成功」，`verified` 徽章不再撒谎。

**群聊真的落库、真的按 persona 选 provider / Group chat persists and honours the persona provider.** 启动接线把 `state_db` 与 `multi_registry` 接进两个构造器；`Persona.provider` 现在会按**名字**严格查注册表（`ProviderRegistry::get` 也会解析模型键，不加这层判断会把拼错的 provider 名路由到无关 provider）。`src/providers/registry.rs`（一个与 `thinker::ProviderRegistry` trait 同名的 504 行重复实现）随之删除。

**名字与形状对齐 / Names and shapes aligned.** `WaitPark::Task` 不再重复携带任务 id；thinking level 白名单改读 `ThinkLevel::ALL`；`openai_compat` 的取消状态报成 `Cancelled`；`supports_image_editing()` 要求真的配了 edit endpoint。

**「设为默认」真的算数 / "Set as default" now counts.** 注册表记住每个生成类型的默认 provider（启动与热重载都会写入），四个生成工具先问默认 provider、再回退到同类型第一个。徽章与调用现在说的是同一件事。

**群聊的入口不再吞消息 / The group-chat entry point no longer swallows messages.** 启动回复带上 session id；认不出来的 `/groupchat …` 回一份用法而不是无声消失；工具元数据里的用法提示改成解析器真认的写法；`group_chat.list` 同时给 `id` 与 `session_id`。

**语音回合能吃下本地端点的默认格式 / The voice turn accepts the local endpoint's default format.** 本地语音端点默认产出 opus（`audio/ogg`），回合路径现在认它（浏览器能放），默认配置不再每一回合都失败。

## 回归守卫 / Regression guards

守卫是探测器，不是判官：它报告「没找到什么」，由人决定这意味着什么。

The guards are detectors, not judges: they report what they did **not** find, and a human decides what that means.

| 守卫 / Guard | 位置 / Location | 钉住什么 / Pins |
| --- | --- | --- |
| 群聊接线普查 / group-chat wiring census | `src/group_chat/wiring_census.rs`（新文件 / new file） | 启动接线必须出现 `orchestrator.with_database(`、`executor.with_database(`、`executor.with_provider_registry(`。 |
| 目标写入者普查 / goal writer census | `src/gateway/execution_engine/tests/goal_origin_race.rs` | 生产代码（store 模块之外）不得调用 `put_without_origin`，也不得读取 `origin_for`。 |
| 错误分类普查 / error taxonomy census | `src/builtin_tools/generation/mod.rs` | `src/builtin_tools/generation/` 下不得再出现 `ToolError`——分类塌缩一旦回来就会红。 |
| 默认 provider 普查 / default-provider census | `src/builtin_tools/generation/mod.rs` | 四个生成工具都必须先问 `default_for_type`——「设为默认」不得再退化成字母序。 |
| 回合音频容器测试 / voice-turn container test | `src/gateway/handlers/voice_turn.rs` | 回合路径认 mpeg/mp3/wav/ogg-opus，仍然拒掉 flac/pcm。 |
| 既有 / existing | `src/gateway/telecom/run_origin.rs:223` | origin 写入者恰好一个生产调用方（未改动）。 |

## 验证 / Verification

统一一次 `cargo check --all-targets`（`-j 3`，共享 `CARGO_TARGET_DIR` 复用依赖产物）覆盖全部改动；`--all-targets` 是必要的，因为本轮改动触及 `#[cfg(test)]` 模块，而 `cargo check` 不编译测试代码。

One unified `cargo check --all-targets` covered the whole change set. `--all-targets` matters here: the change set touches `#[cfg(test)]` modules, and plain `cargo check` does not compile test code.

结果 / Result: **0 errors** — `Finished 'dev' profile [unoptimized + debuginfo] target(s) in 2m 42s`（日志 `/tmp/sev-check-final3.log`，2026-11-28）。前两轮各暴露一个真实缺陷并已修掉：第 1 轮 8 个错误（`probe.rs` 的 `Option<&str>` / `Option<String>` 类型不匹配，以及 7 处漏改的 `GoalStore::put` → `put_without_origin` 调用点），第 2 轮 1 个错误（二进制 crate 不能命名私有模块 `alephcore::config`，改用公开的 `alephcore::Config`）。第 3 轮全绿。

Two earlier rounds each exposed a real defect and were fixed before the final round: round 1 had 8 errors (an `Option<&str>` vs `Option<String>` mismatch in `probe.rs`, plus 7 `GoalStore::put` → `put_without_origin` call sites the earlier rename pass missed), round 2 had 1 error (the binary crate cannot name the private `alephcore::config` module — the helper now takes the public `alephcore::Config`). Round 3 is clean.

## 未做的事 / What was not done

- 没有跑 `cargo test`：本轮只做了统一 `cargo check`。测试断言的改动（`supports_image_editing`、`openai_compat` 音色目录、`need_summary` 移除后的用例）已随代码更新，但没有执行。 / No `cargo test` run: the change set was verified with one unified `cargo check` only. The affected test assertions were updated but not executed.
- 没有实现功能缺口：`check_progress` 轮询、`cancel`、`edit_image` 工具、转写工具、群聊 Panel 界面、per-provider 限流重试。 / No feature work: progress polling, cancellation, an image-edit tool, a transcription tool, a group-chat Panel, per-provider rate-limit retries.
- 没有删除 `alephcore` 公开 API 里那套只有测试在用的访问器（`GenerationParamsBuilder`、`GenerationData` 访问器、`GenerationProgress` 构造器、`GenerationRequest::with_user_id`、`GenerationType::is_long_running`、replicate 的三个模型常量、`MockGenerationProvider`）：它们是库 crate 的连贯 API，删除属于 API 清理。 / The coherent-but-test-only public API of the library crate was left in place; cutting it is API cleanup, not a wire repair.
- 没有把 `metadata.extra` 塞进工具输出。 / `metadata.extra` was not surfaced in the tool outputs.
- 没有改 `group_chat.*` RPC 的 wire 形状（`GroupChatRequest` / `GroupChatMessage` 的 serde + JsonSchema 契约仍然无人使用）。 / The unused `group_chat.*` wire contract was left alone.
- 没有把 `method_visibility` 里那张 `group_chat.*` 可见性表提升为闸口（散落的 `stamped_owner_visible` 检查仍然是实际裁断点）。 / The curated visibility table was not promoted to a chokepoint.
- 没有补齐旧配置形状：老的 `[generation.providers.*]` 转写条目仍然解析不到（STT 只读 `transcription_providers`），`MEMBER_CARVE_OUTS` 里那条 `group_chat.start` 仍然空转。 / The legacy provider map and the inert carve-out were left alone.
- 没有删掉那三个只有配置与 Panel 认的旋钮（`output_dir` / `auto_paste_threshold_mb` / `background_task_threshold_seconds`），只把文档改写成 RESERVED。 / Three Panel-only knobs were documented as RESERVED rather than cut.
- 没有给群聊补 Panel 界面（`group_chat.*` 六个 RPC 仍无仓库内客户端）。 / Still no in-repo client for the six `group_chat.*` RPCs.
- 审计只覆盖这三个模块及其远端消费者；仓库其余部分未纳入本次扫描。 / The audit covered these three modules and their far ends only.

## 复现 / Reproducing

```bash
# 审计脚本与八份报告（已入库）/ audit scripts and the eight reports (committed)
docs/superpowers/runs/2026-11-28-severed-wire-audit-evidence/

# 守卫 / guards
cargo test -p alephcore --lib group_chat::wiring_census
cargo test -p alephcore --lib goal_origin_race
cargo test -p alephcore --lib error_taxonomy_census
```

审计期间的工作副本在 `.superpowers/sdd/2026-11-28-severed-wire-audit/`（该目录被 gitignore，只在工作机上存在；入库的是上面的 evidence 目录）。

The working copy of the audit lived in `.superpowers/sdd/2026-11-28-severed-wire-audit/` (gitignored, machine-local); what is committed is the evidence directory above.
