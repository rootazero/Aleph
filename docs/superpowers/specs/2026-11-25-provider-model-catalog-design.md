# Provider/model catalog and selection wiring / 供应商与模型目录连线

**Status / 状态:** Approved · 已确认 · 2026-11-25

## Intent / 意图

用户需要在已配置的供应商上找到最新、可用的模型；选中的供应商和模型必须作为一对到达执行，运营配置里的模型属性也应真正影响运行。刷新目录不能只显示成功而不改变选模结果，同名模型不能因客户端丢失供应商而被拒绝或错路由。

Make recent models discoverable on configured providers, honor operator metadata at runtime, and preserve the exact provider/model binding from picker to transport. A successful refresh must change what selection can see; a duplicate model ID must never erase provider identity.

## Approaches / 方案比较

| Approach / 方案 | Trade-off / 代价 |
| --- | --- |
| **Selected: connect first, then curate / 已选：连线＋策展更新** | Reuse Aleph's static facts, operator definitions, endpoint discovery, and selectable Capability. Model data still needs explicit maintainer verification. / 复用现有能力；目录仍需维护者逐条核验。 |
| Connect + offline importer / 连线＋离线导入 | Reproducible bulk updates, but adds source mapping and release tooling before the selection path is sound. / 批量更新更快，但需维护导入与映射规则。 |
| Connect + live public overlay / 连线＋运行时覆盖 | Pi-style faster updates, but adds remote trust, rollback, and cache isolation. / 更新更快，但扩大运行时依赖与信任边界。 |

Pi's provider-scoped catalog and separate availability projection (`packages/ai/src/model-catalog.ts`, `packages/coding-agent/src/core/model-runtime.ts`) are useful patterns. OpenCode's decoder skips incompatible API entries and retains a bundled baseline (`packages/core/src/models-dev.ts`). Aleph will adopt the separation of concerns, not their runtime sources or protocol inventories.

Pi 的按供应商组织目录、独立发布可用性，以及 OpenCode 对协议兼容性和随包基线的处理，值得参考。Aleph 借鉴职责划分，不直接引入其运行时目录源或供应商列表。

## Design / 设计

### 1. One selectable publication / 一份可选模型发布

- Reuse `SelectableModelsSnapshot` and its existing Capability slot. Publish from boot, provider config changes, and a successful explicit `providers.modelsRefresh`. Keep routability, enabled state and usable authentication checks; discovery cannot grant readiness or create a provider transport. No independent registry or credentials in the snapshot. / 复用现有快照和槽位；启动、配置变更及成功的显式刷新均接同一发布路径。发现不能绕过启用、可路由及凭据状态检查，不另建注册表、不持有凭据。
- Reuse the discovery path for both `providers.modelsRefresh` and `list_models { refresh: true }`: a newly fetched, accepted inventory is published in both cases, not deferred until a restart. The tool keeps its present TTL and permission limits; only the admin RPC can force a fetch. A tool cache hit is a read, not a write. / 两个入口都即时发布新取得且通过检查的清单；工具保留现有 TTL 和权限，只有管理员 RPC 可强制刷新；工具命中缓存不写入。
- Publish against current provider configuration and readiness, not the pre-I/O clone. Bind fetch, stale fallback and cache usability to provider name and effective listing endpoint, protocol and credential scope. Endpoint/protocol/credential changes and provider removal invalidate old inventory. Check the captured config revision before disk commit and live publication so an in-flight A → B → A result cannot return as current; if durable compatibility cannot be established, cold-start discovery is a miss. No raw credential in cache metadata, protocol, errors or logs. Preserve single-flight without serving a different scope's winning request. / 抓取、旧清单回退及缓存复用必须匹配供应商、实际清单端点、协议和凭据作用域；变更须失效，并在落盘和发布时检查配置版本挡住 A → B → A。不能证明持久缓存匹配则冷启动不使用；凭据不得写入缓存或泄露。复用 single-flight，不串用不同作用域的结果。
- A recognized listing array that is literally empty removes discovery-only entries, not operator entries. If a nonempty array decodes to no usable rows, treat it as a shape failure. Any failed refresh keeps last known good for that same scope, reports stale status where the contract supports it, and never grants entries. No network I/O or disk cache read on picker or turn lookup. / 只有已识别协议中的实际空数组才清除发现项；非空数组全不可解析按形状失败处理。失败保留同作用域旧清单并如实标记；聊天查询不读盘或访问网络。
- Add a member-readable, read-only `chat.models.list` projection of usable choices with exact provider ID, model ID and display labels. The result contains no endpoints, keys, auth material, disabled credentials, or unrelated settings. Register with Gateway policy/census and shared wire types; keep `providers.*` admin-only. Web chat and a dedicated member TUI picker (e.g. `/pick-model [query]`) fetch on open and preserve both IDs. Keep the existing TUI `/providers [query]` admin browser and refresh actions, including browsing unconfigured presets; fix bindings from that overlay too. Render loading, empty and error separately. / 新建成员可读的最小聊天投影；Web 与 TUI 成员入口打开即刷新，保留两个身份。原 `/providers` 仍为管理浏览和刷新入口，继续展示未配置预设，也修正其选中绑定。加载、空和错误分开呈现。

### 2. Bind the exact pair / 精确绑定供应商和模型

- Keep the public `ModelOverride` qualified/raw wire shapes. For explicit user overrides on `chat.send` and `agent.run`, validate through the selectable resolver used by `select_model`. An explicit pair matches the exact provider and model. `Raw` is an opaque model ID, including `/`; only the slash-command grammar interprets `provider/model` shorthand. A raw ID must match exactly one provider in the configured snapshot before readiness is checked, so disabling a same-ID provider cannot silently change where a raw request goes. Ambiguity asks the caller to send a qualified pair without disclosing inaccessible provider names. Refuse invalid explicit input as a model-selection error, before queue admission or run effects. Re-check the routable provider at final dispatch, including already-queued work; refuse rather than applying the requested model to the default provider. A rejected request does not mutate the session pin. / 保留 wire 形状；明确绑定精确匹配。RPC 的 Raw 把 `/` 当模型 ID 的一部分，只有斜线命令解释限定简写；先对配置快照判唯一再看就绪，避免同名供应商状态变化导致静默改投。歧义提示使用明确绑定，不泄露不可访问的供应商名。入队和运行副作用前拒绝无效输入；实际派发再挡住消失的供应商，绝不把旧模型发给默认供应商，不改会话 pin。
- TUI sends the qualified selection through the existing `/model` path. Web keeps its existing two actions: the composer choice stays client-local and is sent as an explicit turn override; the session action still invokes the existing durable server pin. Keep priority: explicit turn > valid durable session pin > agent default. A stale persisted pin is ignored for the run without rewriting the user's saved preference; make the fallback visible to the user. Keep the existing internal voice defaults and replay migration policy; an explicit qualified internal binding still may not degrade to another provider with the same model. Do not migrate the preference store or change automatic fallback membership. / TUI 沿现有 `/model` 发送带供应商的身份，Web 普通选择仍留在当前客户端并随单轮请求发送；另有独立动作写入持久的会话 pin。旧 pin 失效时整对忽略、告知用户但不偷偷改写；既有语音与恢复策略保持，明确绑定不得拆开回退。

### 3. Reuse model resolution / 复用模型事实合并

- Make the existing `ModelRecord::resolve` and roster composition honor the exact provider's matching `model_defs` for preset as well as custom rows. Share existing candidate assembly across admin catalog, model tool and selectable projection where the semantics match. Preserve intentional differences in auxiliary rows, readiness, ordering and fallback eligibility. Remove only equivalent legacy branches; no second metadata authority. / 让现有合并点覆盖预设与自定义供应商的逐模型配置；只合并同义候选逻辑，保留管理、辅助、可选和自动回退的不同语义。
- Use the same configured model window in budget initialization and serving-model refinement; apply the same lookup to the summary route where it resolves a configured model. Existing provider-level positive context override remains first, then positive matching model definition, then static fact, then existing conservative unknown-model behavior. An explicit token budget still wins. Per-serving refinement can constrain the precomputed chain budget, not expand it; unknown, zero or another provider's override cannot increase it. / 预算初始化、实际服务模型及能解析到配置的总结模型统一查窗口；保留供应商级正数覆盖优先，其次逐模型正数覆盖、静态事实、保守未知值。显式 token 预算优先，实际服务窗口只能收紧链预算。

### 4. Curate verified data / 更新经核验的目录数据

- Prioritize current OpenAI, Anthropic/Claude and DeepSeek Flash on Aleph's existing direct configured transports. Require an authoritative model/endpoint source and verified exact wire ID, protocol availability, region where applicable, context/output limits and lifecycle for each edited field. Validate price against the matching official rate card or leave it unknown; never infer a number from an adjacent model. Keep provenance in docs or the change evidence. Pi/OpenCode and search snippets are leads, not provider authority. / 优先现有直连的 OpenAI、Anthropic/Claude、DeepSeek Flash。记录供应商来源与端点、确切 ID、协议、适用地域、窗口/输出上限和生命周期；价格必须匹配正式价目，否则留未知。参考目录只作线索。
- Do not silently promote an entry to default/fallback, remove an active binding, extend the DeepSeek non-Flash ladder, or add a new transport. Where one curated list currently doubles as a ladder, make the distinct policy explicit rather than changing the ladder by adding metadata. An unverifiable candidate stays unshipped with the reason documented. / 不自动更新默认/回退、删除现有绑定、扩充 DeepSeek 非 Flash 或新增协议；目录和回退虽可能共享原表，但扩目录不能改变自动路由。无法核验的候选不进入目录。

## Verification and rollout / 验证与交付

1. Prove refresh-to-member-picker and refresh-to-resolver with a deterministic endpoint fixture, including stale failure, successful empty, endpoint replacement, and readiness denial. Exercise member access and admin denial independently. / 用确定性的端点夹具验证刷新传到客户端和解析器，以及成员权限、失败、空结果和端点替换。
2. Prove two providers with the same ID remain distinct through both pickers, explicit invalid input is refused, stale preferences use complete default binding, and no provider receives another provider's requested model. Check queued/recovery and no-provider branches wherever they share the contract. / 验证同名双供应商、无效请求、陈旧 pin、队列/恢复及无供应商路径。
3. Compare the same preset/custom `model_defs` row in roster, tool output and serving budget; prove explicit override and chain caps; regression-check discovery does not alter fallback ordering. Verify new IDs are usable only via matching transports and supported metadata is not invented. / 对照预设和自定义逐模型覆盖，验证实际预算、链上限和回退顺序，新数据只用于匹配协议。
4. Run targeted Rust and client checks, then workspace/integration checks appropriate to affected crates. Update `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/MODEL_CATALOG.md` and other current references whose contracts changed. Commit on the isolated branch; report validation not run and unverified model data. / 完成针对性和集成验证，校对现行参考文档，仅在隔离分支提交，注明未验证项。

**Builder decisions / 实施者决定:** Internal helper boundaries, fixture placement and smallest compatible UI representation. Do not build a second catalog, provider registry, scheduler, or redesign the picker. / 内部 helper、测试落点与最小 UI 表示由实施者定；不新增目录、注册表、调度器或重做选模界面。
