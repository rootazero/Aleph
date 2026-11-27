# 插件导入与 Capability 组合 / Plugin imports and Capability composition

日期 / Date: 2026-11-27

基线 / Baseline: `438227060`, branch `plugin-capability-composition`

## 目标 / Intent

Claude Code、Pi 和 DeepSeek Harness 的贡献应能经 Aleph 现有入口安装、发现、启用、重载和停用。空间组合要求同名的两个项目插件各自生效，且列表、调用、授权和清理都以各自的作用域为准。时间组合要求发布前验证候选版本、停用时失效旧租约、替换后把新调用交给新一代所有者。插件包提供贡献，Core 持有授权和生命周期事实，可动态发现和调用的贡献进入现有 Capability 契约。

Contributions from Claude Code, Pi, and DeepSeek Harness should go through Aleph's existing install, discover, enable, reload, and disable paths. Two projects can each use a plugin with the same name. Discovery, invocation, authorization, and cleanup follow the owning scope. Validate replacement candidates before publication, invalidate old leases on retirement, and resolve new calls against the new owner generation. Packages declare contributions, Core owns authority and lifecycle, and externally composable contributions use the existing Capability contracts.

## 四方对比 / Four-way comparison

| 维度 / Dimension | Claude Code | Pi | DeepSeek Harness | Aleph |
|---|---|---|---|---|
| 包 / Package | `.claude-plugin/plugin.json`、约定目录、marketplace | `package.json` 中的 `pi`、资源过滤和约定目录 / manifest, filters, conventions | `dsh.bundle.patch`、Cordis patch、profiles | 有 Claude/Codex/Cursor 等适配器。发现声明与挂载清单分路 / existing adapters with separate discovery and mount reads |
| 空间 / Space | 已安装来源和配置域 / installed sources and configuration scopes | 个人及项目包，项目覆盖或缩小资源集合 / personal and project resource selection | profile 启停，Cordis 上下文内注入服务 / profile activation and contextual service injection | `ScopeKey` 和可见性已存在。发现去重、运行所有者、配置和部分注册仍依赖裸 ID / scoped visibility with bare-ID paths in discovery, owners, configuration, and registration |
| 时间 / Time | `/reload-plugins` 刷新贡献 / refreshes contributions | 旧 runner 收到 `session_shutdown`，随后重新加载 / shuts down and reloads the extension runner | `inject` 等待就绪，热更新或重启取决于宿主 / readiness plus host-dependent reload or restart | `EffectScope`、预检、所有者代际和 Tool-only WASM 按需启动 / owned effects, preflight, owner generations, and tool-only lazy WASM |
| 执行 / Execution | hook、MCP 等由宿主执行 / declared hooks and MCP use host execution | 扩展工厂调用 `ExtensionAPI`；Chord 有另一套 facet 契约 / extension factories use the host API, with separate Chord facets | `apply(ctx)` 和 `!!js` 属于 Cordis 宿主 / JavaScript and Cordis services need their host | 复用静态贡献、MCP 和 WASM 路径。新增原生宿主需要独立协议设计 / existing static, MCP, and WASM paths, with future native-host contracts |

Graphify 用于导航，结论来自源码及产品文档。Claude 本地衍生仓 `963e0013` 的 `src/utils/plugins/pluginLoader.ts` 用于理解实现形态，产品行为以官方文档为准。Pi `4ea883792` 的 `packages/coding-agent/src/core/extensions/loader.ts` 在扩展工厂成功后提交注册，`packages/coding-agent/src/core/agent-session.ts` 在重载前发出关闭通知。DSH `fcc0ad4621` 的 `packages/boot/plugin-manager/src/index.ts` 管理 profile 和 bundle；`docs/cordis-tutorial/03-services.md` 描述依赖等待。Aleph 的入口和约束见下方连线。

Graphify located the implementation. Product docs and checked-out source establish behavior. The local Claude derivative illustrates loader structure, with product claims grounded in the official docs. Pi commits pending registrations after the factory succeeds and notifies the old runner before reload. DSH manages profiles and bundles and holds dependent plugins pending while their services are unavailable.

[Claude Code plugin overview](https://code.claude.com/docs/en/plugins/overview) · [Pi extensions](https://docs.pi.dev/latest/extensions) · [DSH upstream](https://github.com/deepseek-ai/deepseek-harness) · [Aleph current entry points](../../reference/FEATURE_LOCATOR.md) · [Plugin contracts](../../reference/PLUGIN_SYSTEM.md)

## 方案选择 / Approaches

- **三生态贯通，用户已选。** 先收口现有解析、挂载和所有权，修复项目实例隔离，再接入三个生态的静态贡献及已声明的 MCP 服务。每种来源都经过相同的生产入口，原生 Pi/DSH 代码执行由后续宿主切片承接。代价是作用域迁移会触达配置、管理接口和存量贡献寻址。
- **Three ecosystems first, selected by the operator.** Unify ingestion and ownership, fix project isolation, then import portable contributions and declared MCP services. Exercise the production lifecycle for each format. Native execution follows in separate host work. Scoped identity touches settings, management, and existing contribution addressing.
- **原生扩展先跑通。** 在独立进程中运行一个生态的真实扩展，验证注册、IPC 调用、停止和租约失效。优点是可以先交付可执行扩展，代价是提前锁定宿主契约，且三种格式和多项目隔离需要后续补齐。
- **Native extension first.** Run one ecosystem's real extension in an independent host and prove registration, invocation over IPC, stop, and lease invalidation. It delivers executable extensions earlier and commits the project to a host contract before broader package compatibility and scope isolation.

## 首轮格式边界 / First-release compatibility

首轮入口是操作者明确安装的本地包、现有受支持安装来源，以及现有 Aleph 插件根目录。保留 Claude 已有导入和外部缓存策略。Pi 和 DSH 的 `package.json` 须有各自的格式证据，或在已选插件根有明确的约定结构，普通应用的 `package.json` 不构成插件。发现只读清单和贡献文件。外部包的自动扫描、包管理器安装脚本、任意宿主代码、主题和 TUI、Cordis 动态组合及 Pi Chord 后续独立设计。

Use explicit local package installs, existing supported install sources, and existing Aleph plugin roots. Preserve Claude's current import and cache policy. Identify Pi and DSH packages by format-specific evidence or explicit conventions in a selected plugin root. Discovery reads manifests and contribution files. Ambient foreign package discovery, package-manager scripts, native host code, themes and TUI, Cordis dynamic composition, and Pi Chord are future work.

| 格式 / Format | 首轮处理 / First-release behavior | 验收样本 / Acceptance source |
|---|---|---|
| Claude Code | 现有 `.claude-plugin/plugin.json`、TOML 和约定目录继续走已有组件解析。保留技能、命令、Agent、受既有策略约束的 hook 与 MCP 行为。 / Reuse existing JSON, TOML, component readers, and current execution policy. | 现有 `qa/plugins/drive_cc_cache.py` 与带技能和 MCP 的包 / existing cache QA and a package with Skill and MCP |
| Pi coding-agent | 选取 `pi.skills`、`pi.prompts` 和其约定目录，复用现有 Markdown 技能、命令和模板渲染。保持资源类型各自的路径、glob、排除和排序语义。对超出支持范围的模式和模板语义给出定位诊断；含不兼容模板的命令不发布。 / Import declared or conventional skills and prompts through existing Markdown readers and command rendering. Diagnose and omit resources with unsupported patterns or template behavior. | `packages/coding-agent/docs/skills.md` 的 `SKILL.md` 形态和 `docs/prompt-templates.md` 的参数化 `/review` / portable directory skill and argument-bearing `/review` |
| DSH | 由 `package.json` 的 `dsh.bundle.patch` 定位包内 patch，接受明确的 `insert` MCP client 节点及其静态连接参数。采用 `@deepseek-ai/dsh-mcp-client` 已证实的 `stdio`、`streamable-http` 配置形态，映射到 Aleph 的 MCP 声明；读取包中约定的 `skills/**/SKILL.md`。 / Read the package-local patch. Map literal MCP client entries to Aleph's MCP declaration and load conventional directory skills. | `packages/preset/agent-preset/skills/agent-experience/SKILL.md` 与 `packages/preset/agent-preset/skills/cordis-plugin-development/templates/mcp/cordis.patch.yml`。使用测试进程和本地端点。 / upstream skill and MCP template shapes, with a test process and local endpoint. |

DSH 的字面量 `serverName`、`command`、`args`、`cwd`、`env`、HTTP `url` 及已有传输可表达的请求头沿用 Aleph 连接约束；只接受每种 transport 支持的字段。`!!js`、无法映射的配置项、未识别的 patch 操作和必需服务给出来源及位置。模板中的 `failOnStartupError: true` 针对 Cordis 启动结果，超出 Aleph 已有异步连接语义，诊断并跳过该 MCP 项。验收用形状相同的字面量副本，去掉该 Cordis 专属字段；另以原始片段验证诊断。健康状态使用现有 MCP 连接状态，插件挂载状态与 MCP 是否就绪分别读取。Pi 的 `extensions`、`themes` 和专属模板构造同样显示为尚待宿主或语义支持。能独立使用的贡献继续可用，完全没有可挂载贡献的包显示明确原因。已有安全边界拒绝整包路径逃逸、不可读清单和声明身份冲突。

Map only literal fields that the selected Aleph transport supports. Report the source and location for `!!js`, unmapped connection fields, unknown patch operators, and required Cordis services. The upstream template's `failOnStartupError: true` has Cordis-specific startup semantics. Report it and omit that MCP entry. The runnable acceptance fixture uses the same declaration shape without the Cordis-only flag. A separate fixture checks the original snippet's diagnostic. Use the existing MCP connection state for readiness and the plugin state for mounting. Report Pi extensions, themes, and unavailable template features explicitly. Mount independently supported contributions, give packages with zero mountable contributions a concrete reason, and reject package identity conflicts, unreadable manifests, and path escapes at ingestion.

## 先连通解析与挂载 / Wire ingestion and mount first

1. `src/discovery/scanner.rs` 保有来源、优先级和作用域。`src/extension/manifest/adapter.rs` 保有格式识别及贡献解析。让同一次识别生成已归一的包描述、可选现有运行说明、贡献、诊断和源文件依赖。沿用 `AdapterOutput`、`PluginManifest`、`CapabilityDeclaration` 的现有职责，逐步收拢重复读取。
2. `ExtensionManager::scan_plugins_blocking`、`prepare_mount`、安装入口及冷启动读取同一归一结果。重载用新快照预检，禁止拿到两次独立读取间混合的包身份与运行声明。历史格式的显式清单优先级和坏清单错误继续生效。缓存持有一次解析结果，命中需覆盖该适配器实际读取的 manifest、patch 与所选资源增删改；显式重载必须重新验证输入。
3. 复用现有 Skill/命令/Agent 解析、MCP transport 和 registrar、WASM 运行时及 `EffectScope`。压缩包、git、本地安装及已有启动路径都应经过同一个校验边界。解析期间不连接 MCP，不读取/执行 JS 工厂，也不执行安装脚本。外部包的相对路径、glob 结果和符号链接目标都必须通过现有或收口后的包根约束；对 DSH YAML 使用现有字面量解析，任何标签仅作为输入错误或诊断。

1. Keep origin, priority, and scope in the scanner. Reuse the adapter registry for format selection and contribution parsing. Produce one normalized package description with runtime requirements, contributions, diagnostics, and source dependencies. Reuse `AdapterOutput`, `PluginManifest`, and `CapabilityDeclaration` through the refactor.
2. Drive discovery, installation, mount preparation, and cold activation from that same normalized contract. Reload preflights a fresh candidate snapshot. Preserve precedence and malformed-manifest failures for existing formats. Cache a single parse result against its actual manifest, patch, and resource dependencies; explicit reload revalidates its inputs.
3. Reuse existing Skill, command, and Agent readers, MCP transport and registrar, WASM runtime, and `EffectScope`. All install and boot paths use the same validation boundary. Discovery makes zero connections and runs zero package code. External package paths, glob results, and symlink targets go through package-root containment. DSH YAML is data; tagged values are diagnosed without evaluation.

## 实例、授权和发布 / Identity, authorization, and publication

| 层 / Layer | 设计约束 / Design constraint |
|---|---|
| 发现与安装 / Discovery and install | Core 内部实例由 `ScopeKey` 和逻辑插件 ID 组成。仍按原优先级在同一作用域选赢家，跨项目分别发现。一次安装回执给出准确实例，重复安装或替换在同一实例上串行。 / Use `ScopeKey` and logical ID as the Core instance identity. Retain precedence within a scope and discover distinct projects independently. Install receipts identify the exact target and serialize concurrent changes to it. |
| 配置与授权 / Config and authorization | 保留 `plugins.toml` 旧格式和全局键。旧的启用和信任许可仅绑定到可唯一确认的历史实例，禁止随同名新安装扩散；旧禁用若归属不明则保留拒绝。作用域写入、配置、受信状态和授权均按实例解析。 / Read old `plugins.toml` and preserve global keys. Legacy permissions bind only to a uniquely attributable historical instance and cannot spread with later installs. Ambiguous legacy denials remain effective. Scope new writes, config, trust, and authorization to the instance. |
| 常规调用和管理 / Invocation and management | 保留现有逻辑 `id` 和无歧义裸 ID 的操作。列表附加稳定的实例 selector 和作用域。确定了调用者项目时，在可见集合内解析，项目同名优先于全局；操作者若同时管理多个同名实例，用明确 selector。无上下文的歧义请求返回可见目标及冲突原因。所有分支在 Core 先授权再定位实际拥有者。 / Preserve logical IDs and unambiguous bare-ID calls. Lists expose a stable instance selector and scope. Project resolution selects a visible project instance ahead of the global one. Multi-instance management uses explicit selectors, and context-free ambiguity returns authorized targets and a reason. Core checks authority for the chosen target. |
| 所有者与注册 / Owners and registrations | 实例键贯穿 `PluginRegistry`、`RuntimeOwner`、`EffectScope`、配置、事件和 disposer。当既有注册面只允许一种全局 provider name 时，使用由实例派生的稳定碰撞安全名称，保留既有全局 provider name。项目显示及旧调用别名通过现有作用域投影解析。调用、租约和清理使用底层实例与代际。 / Carry the instance through records, runtime owners, scopes, config, events, and disposers. For globally unique provider slots, derive collision-resistant stable names from the instance while preserving established global names. Resolve scoped display and legacy aliases through existing projections. Calls, leases, and cleanup target the actual instance and generation. |
| Capability / Capability contract | 复用 `ToolHandlerRegistry`、`OwnershipTree`、`BackendLease`、Kind 对应的投影和源注册。包实例 selector、工具 provider name 和 Capability 身份只在各自边界派生一次。现有 Skill、Agent、Hook、MCP 管道须列出其授权、发现、执行和撤回路径；对于无生产 facade 投影的贡献，设计中给出本轮实际接入面，避免只产生描述并声称已交付。 / Reuse the canonical handler registry, ownership tree, leases, and kind-specific sources. Derive each wire identifier once at its boundary. Trace authorization, discovery, invocation, and removal for existing Skill, Agent, Hook, and MCP paths, and describe the production surface proven for each. |

实现决定 / Design decisions:

- Scope canonicalization continues through `ScopeKey::project`. Persist an exact scoped preference using the existing configuration writer. Apply legacy global settings to their global identity. Bind project-only legacy permits only when the legacy target is uniquely attributable across the discovered legacy candidates, before publishing a scoped permit. Retain ambiguous legacy denials until an exact scoped operator action resolves them. Copying the directory or changing its manifest ID has no inherited project grant. Persisted selectors are opaque and reversible through existing authorized list results; no raw project path is required in a provider name or exposed to another project. Existing documents remain readable. A restart must preserve bindings and an added same-name project must not inherit a previously migrated grant.
- `CanonicalToolState::from_records_with_declarations` currently derives a unique provider-name projection. The implementation plan must account for all consumers of names, including scoped prompt and tool listings, canonical dispatch, cached leases, slash commands, agent entries, MCP actor IDs, and direct Gateway/tool operations. Exact names are deterministic and validated against existing transport length/alphabet rules, with explicit collision detection before an insertion. Display names are scoped projections of the source entries. There is no parallel authority for name ownership.
- 包、贡献和视图使用各自的状态：包挂载、贡献注册、MCP 就绪、租约有效分别由现有事实源表示。原生代码只出现在未支持的格式诊断中。 / Package mounting, contribution registration, MCP readiness, and lease validity come from their respective sources. Native code appears in compatibility diagnostics.

## 时间组合与故障 / Lifecycle and failure behavior

- 启用、停用和重载在实例粒度串行；互不关联的实例沿用现有并行资源边界。启用经过已有 trust、启用策略和依赖门禁。发现及安装验证无运行时副作用。
- 重载预检失败保留旧实例及其租约。通过预检后，撤销旧 owner，按已有 disposer 逆序收回贡献，推进代际，并发布新一代。测试重叠重载、停止和延迟回调时，旧 disposer 不能删除新注册或隔壁项目的注册。
- 预检之后才出现的运行故障通过既有挂载回滚清理候选并返回实例失败和原因。旧代际依旧失效。保留旧包字节、热切换两套同时活跃的运行时和自动回滚为独立议题。
- 用现有 `OwnershipTree` 和 lease 校验证据确认旧引用在撤销后失败；新的可见调用必须解析到新一代。MCP 工具由真实 MCP 发现和调用路径验证，异步失败用 MCP 状态反馈。现有 WASM 对精确获准调用的惰性启动保持现行契约。

- Serialize enable, disable, and reload for an instance; preserve concurrency for independent owners. Use the existing trust, activation, and dependency gates. Discovery and installation validation have zero runtime effects.
- A failed preflight preserves the incumbent and its leases. After successful preflight, revoke the old owner, unwind its effects in existing disposer order, advance generation, and publish the replacement. Overlapping reload, stop, and late callbacks must leave the replacement and other projects intact.
- Use existing mount rollback to clean up candidate effects on post-preflight runtime failures and expose the failed instance and reason. The old generation stays revoked. Retaining old package bytes, running two generations for a live switchover, and automatic restoration are separate work.
- Prove stale references fail through the existing ownership and lease checks, and new authorized calls reach the new generation. Verify MCP tools through real discovery and dispatch with current readiness feedback. Retain existing on-demand WASM activation for an exact authorized tool call.

## 实施顺序和验收 / Implementation order and verification

1. **补齐生产入口和基线验证。** 盘点安装、启动、启停、重载、可信配置、投影和调用的入口。用已有 Claude、WASM、项目可见性与 MCP QA 固定基线。合并重复的格式识别及 manifest 读取，移除失去调用者的分支。 / Inventory actual production paths, pin existing behavior with current QA, consolidate duplicate reads, and remove unused branches.
2. **贯通实例生命周期。** 修复发现 winner、记录、配置/信任、Owner、注册及管理寻址。真实 Gateway 在项目 A 和 B 安装同名包，分别返回对应贡献，项目 C 看不到 A/B；A 重载或停用时 B 能继续调用，原 A lease 失效。验证全局与项目同名、旧配置文件、授权失败、重启、歧义选择、重叠调用、预检失败和后提交失败。已有管理 RPC 与 `plugin_manage` 路由到同一服务。 / Verify actual Gateway paths, same-name projects and global/local overlap, restart and old config, ambiguous management, lost authorization, stale leases, preflight refusal, and rollback cleanup. Drive existing RPC and plugin management through the same Core service.
3. **接入格式并使用真实能力。** 通过现有安装和启动入口读取 Claude、Pi 和 DSH 的代表包。用 `qa/plugins/drive_plugins.py`、`drive_scope.py`、`drive_visibility.py`、`drive_command.py`、`drive_cc_cache.py`、`drive_trust.py` 中适用的装置，证明技能可检索、Pi 命令展开参数、MCP 连接并真实调用、停用清除可调用表、重载刷新状态。验证路径逃逸、普通 npm 包假阳性、坏 YAML、外部代码未求值、混合支持诊断及无可发布贡献的包。 / Use package shapes from each ecosystem through real install and boot. Prove retrieval, prompt expansion, actual MCP dispatch, removal, reload, and diagnostic paths. Test path escape, false-positive detection, broken YAML, and mixed support. Include fixtures that would leave an observable marker if discovery accidentally executed code.
4. **验证资源与文档。** 对比相同冷启动输入的清单读取次数、发现耗时和启用前 MCP/外部宿主启动数。记录运行环境和多次测量范围。优先消除重复 I/O，保留现有有界 blocking 工作池，按证据决定并行解析。用适用的 Rust 检查及正常入口 QA，校对 `FEATURE_LOCATOR.md`、`PLUGIN_SYSTEM.md` 及受影响的参考文档。 / Compare cold discovery reads, duration, and pre-enable process/connection counts with matched inputs. Remove duplicate I/O first, keep the existing bounded blocking work, and measure before adding parallelism. Run targeted Rust checks and the real-entry QA, then reconcile `FEATURE_LOCATOR.md`, `PLUGIN_SYSTEM.md`, and affected references.

## 实现留白 / Builder discretion

计划阶段按实测调用点确定内部类型名、解析缓存键的结构、版本化字段名、迁移实现和用例组织。选用现有 Rust 类型与依赖，保持相同的 scoped authority，避免新增独立注册表、重型依赖、宿主 DI 或 Harness 循环改动。正式计划和所有代码提交只在隔离分支。后续原生宿主若提供注册、事件或 UI，使用明确版本的 IPC 和 Core 授权，并复用此处的实例和 Capability 生命周期。

The implementation plan chooses concrete Rust types, cache key layout, versioned fields, migration mechanics, and test organization from the call-site inventory. Reuse existing types and dependencies, keep one scoped authority, and leave host DI and the harness loop alone. Keep the plan and implementation commits on the isolated branch. Any later native host introduces an explicit versioned IPC contract and Core authorization while reusing the instance and Capability lifecycle.
