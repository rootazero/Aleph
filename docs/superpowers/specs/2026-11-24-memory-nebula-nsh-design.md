# Memory Nebula on NSH / NSH Panel 记忆星云图

**状态 / Status:** 分段路线已选；独立 builder check 未发现仍需澄清的关键问题，本文待用户批准。 / Staged approach selected; independent builder check found no significant open questions. User approval pending.

## 目标、取舍与界限 / Intent, approaches, and boundaries

- 让新 NSH Panel 的星云图读到真实的 MemoryRuntime 图数据，能浏览、搜索和打开同一条笔记；也修复旧星云图丢失分区地址、搜索与当前图窗不一致、旧响应覆盖新状态的问题。 / Connect NSH to the real graph, search and note-detail path, and fix address loss, off-window selection and stale responses in Leptos.
- **已选：先只读连线、再依据真机数据优化。** 复用 Core 的处理器、传输和投影；复用现有布局计算，用 NSH Canvas 提供共享的平面呈现，而不是把平面图宣称为 3D 迁移完成。 / Chosen: wire read-only end to end first, then tune from host measurements. Share the existing Rust layout and use a planar NSH Canvas presentation, not a claim of 3D parity.
- **未选：一次交付完整 3D 和写功能。** 这会把渲染可行性、写权限、错误恢复绑成同一个验收点。嵌入旧 WebGL 页面也不选：它会复活独立的 UI 和传输。 / Rejected: one-shot 3D/write parity couples rendering, write authorization and recovery; embedding the old page would create parallel UI and transport paths.
- 本轮不替换默认 Leptos、不更改存储格式或记忆写入规则，不做跨分区画布、JSON Canvas 导入/导出、第二套浏览器客户端或新绘图库。 / No default cutover, storage migration or write-policy change, cross-partition scene, JSON Canvas file exchange, extra browser client, or rendering dependency.

## 1. 能力发布、授权与传输 / Capability, authorization, and transports

- Core 的 `panel.capabilities.snapshot` 发布内置 `aleph:memory-nebula`；在 Gateway 投影出口组合已挂载的 MemoryRuntime 图能力与现有 extension snapshot，两个 snapshot 调用路径复用同一组合。发布绑定现有运行时的注册/启动寿命，而不是靠 TSX 静态补齐、探测私有数据或维护第二份 readiness 注册表。保留现有快照 revision 的含义；此 slice 的内置能力和 Gateway 生命周期一致，不承诺热开关。如果发现 MemoryRuntime 可在同一连接代内撤销，使用现有发布 revision 变更机制，不在同一 revision 下改变声明。 / Core publishes the built-in declaration through a single composition in the existing Gateway snapshot paths. Use actual mounted runtime lifetime, not a synthesized tab, data probe or parallel readiness registry. Preserve revision semantics; same-connection revocation, if supported, must use the existing publication revision machinery.
- TSX 把声明的 key 路由到共享模块，渲染器映射不授予权限；缺失、禁用或无效声明不调用图 RPC。Leptos 保留原导航，不把 NSH 内置 entry 当 Leptos 可加载资产。 / Only map the published enabled key to shared TSX; do not mount on missing, disabled or rejected discovery or fetch an NSH entry as a Leptos asset.
- 只在 `src/gateway/server/connection/mod.rs` 的共用 v1 白名单接入 `graph.query`、`graph.search`、`graph.node_detail`。保留 IPC owner、Web 鉴权、调用者重新验证和 Core 的 `PartitionChecked` 策略；声明不是读取权限。NSH 不加入 `graph.update_note`、`graph.rename_note`、`graph.delete_note`，不回退到旧 `/ws`。拒绝/空分区不可区分的现有返回形状保持不变。 / Mount only the three reads on both v1 transports. Preserve existing caller and partition guards, denial shapes, and unmapped writes. No fallback to legacy `/ws`.

## 2. 节点地址、图窗与共享契约 / Node addresses, graph window, and shared wire types

- 共享 Rust 协议承载现有 graph 读 DTO；在 `graph.query` 返回的每个节点上加与 `graph.search` 一致的 `agent_id`，值由 Core 从实际读出的分区填入。保持原节点 ID、端点、关系、参数和旧客户端反序列化兼容；空响应不另加可用于探测的分区信息。详情和旧编辑都使用 `(agent_id, node_id)`，缺地址不猜测、不发请求。 / Consolidate read DTOs in shared Rust protocol. Add Core-supplied `agent_id` to returned graph nodes, matching search hits. Preserve IDs, endpoints and legacy decoding; empty responses disclose no address. Detail and legacy writes use exact tuple addresses, never guesses.
- 图仍是单主分区的有界窗口；旧搜索可读多个授权分区。仅在地址与当前场景节点完全匹配时聚焦；其他命中仍可按原地址查看详情，但显示“不在当前图中”，不把同 ID 的另一条笔记当命中，不扩张图窗。查询、搜索、详情分别保护请求代次；切换选择、分区、连接或卸载后，不采纳旧结果或旧错误。旧编辑成功只使对应地址的详情和相关图失效。 / Focus only exact matches in the current single-partition graph. Other authorized hits can open detail without scene selection or widening. Fence async results and errors by request/scope/connection lifetime; refresh the addressed records after legacy edits.
- NSH graph 默认 100 节点，不主动扩张请求；让相同窗口排序可复现（度数、时间之后以唯一笔记路径断平手）。现有 Web v1 的 1 MiB 帧上限保留；正常、密集和超长详情按序列化 JSON-RPC 字节数检查，超限显示明确错误且不无限重试、不静默截断正文。只在实际测到密集图超限时才加有明确元数据和测试的 Core 边投影上限；不改整条传输的帧上限。 / Default to the existing 100-node window, make tied ordering deterministic, and preserve the 1 MiB Web frame envelope. Check actual UTF-8 JSON-RPC size; surface oversize errors without silent truncation or retry loops. Only introduce an explicit Core edge projection bound if measurements require it.

## 3. 图呈现与清理 / Rendering and cleanup

- 大致布局：`Memory → [搜索] → 星云 Canvas + 节点/详情`。使用现有 NSH 设计组件呈现加载、空、失败、未在图中和连接不可用状态；可访问的节点/结果列表不依赖 Canvas 拾取。复用已经固定的 NSH 0.6.0 能力；首个验证点是该版本的 Canvas 输入与帧调度。如果某个高级手势不能跨端一致复现，保留可操作的缩放、复位和列表选择控件，不加原生/Web 专用手势桥。 / Shared NSH view pairs the canvas with search, accessible result/node controls and detail. Verify pinned Canvas input and scheduling first; if advanced gestures are unavailable, keep portable zoom/reset/list actions rather than a platform-specific bridge.
- 移出可复用的纯 Rust `ForceLayout`、必要向量/种子逻辑及图索引到现有共享 UI 逻辑，保持旧相机和 WebGL 渲染专用部分原位。绘制与拾取用同一投影。保留有向真实边和独立的相似提示；无向几何去重只发生在弹簧或绘制层，不吞关系。可见边预算优先给当前选中节点的邻边，关系详情仍能列出完整邻接。 / Reuse the pure Rust layout and graph indexing in existing shared UI logic while leaving legacy camera/WebGL code owned by Leptos. Use one rendering/hit-test projection; deduplicate geometry only, preserving directed relation facts and keeping virtual similarity distinct. Prefer selected incident edges within the draw budget; present complete adjacency in detail.
- 参考取舍：NSH 0.6.0 `examples/gallery/demos/CanvasDemo.tsx` 验证同一 draw API；JSON Canvas 和 react-jsoncanvas 提供方向、端点解析；code-call-graph-editor 提供邻接高亮；CodeGraphyV3 用 vis-network，不能作为移植 force-graph 的理由；infinite-canvas-tutorial 提供逆投影、锚点、裁剪思路。Graphify 用于定位，结论回到源码核对；官方 `/apps/` 未能直读，不作为实测。 / Reference implementations inform semantics and interaction, not dependencies or claims of runtime performance.
- 清理由共享契约替代的 DTO、重复索引/换算和无效文档引用。扫描桌面、手机和测试消费者后才删 0 引用代码；若未替代，保留旧实现。 / Remove superseded DTOs, duplicate indexing/conversions and stale doc links only after consumer census; keep live legacy paths.

## 验收与交付 / Acceptance and delivery

1. **Core/Rust:** 同路径跨分区测试必须证明查询、搜索、详情和旧编辑落在正确分区；访问不可见分区不能区分为空/不存在。验证 JSON 兼容、相反方向关系、稳定窗口、缺端点以及高亮与裁剪组合。 / Test duplicate paths across partitions, inherited denial shapes, wire compatibility, reciprocal edges, deterministic bounds, missing endpoints, and highlight versus culling.
2. **传输和 UI:** 在真实 Gateway 上从两条 v1 入口验证三条 read RPC 到达 Core、写 RPC 不挂载。跑现有 NSH Panel 检查和 host 测试，覆盖能力没有发布、切换/reconnect、旧响应不污染现态、错误及溢出。真机用正常与密集 fixture 验证搜索、图窗外详情、选中、缩放、边高亮与回包大小；记录 native/Web 帧指标和对应能力限制，记录未获得的真实平台，不用 mock 代替。 / Verify actual Gateway dispatcher behavior on both transports and exercise shared UI lifecycle and interactions with ordinary/dense fixtures. Report actual hosts, frame timing and bytes; explicitly distinguish fixtures, mocks and missing native evidence.
3. **顺序和落点:** 先 Core → 两个 v1 → NSH 真实只读路径；同步修旧地址；再调布局/交互、清理 0 引用代码，更新 `docs/reference/FEATURE_LOCATOR.md`、`docs/reference/MEMORY_SYSTEM.md`、`docs/reference/DESKTOP_SHELL.md` 中确实改变的约定。实现方案由 builder 在这些边界内安排测试和提交。只在隔离分支提交；不合并、不推送、不安装。 / Wire first, then tune and clean up, revise the affected reference docs, and commit in the isolated branch only. No merge, push or install.
