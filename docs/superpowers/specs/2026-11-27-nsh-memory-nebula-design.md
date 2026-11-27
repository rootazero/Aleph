# NSH 记忆星云图重构 / NSH Memory Nebula redesign

日期 / Date · 2026-11-27

状态 / Status · 已批准 / Approved

## 目标与现状 / Goal and current behavior

让 NSH 用户从星云图查找、浏览和管理记忆笔记。桌面原生宿主与 TSX Web 共用组件和呈现逻辑，Core 继续决定可见范围并维护笔记事实。现有 TSX 星云固定查询 100 个节点，支持点选和按钮缩放，详情关系为纯文字。经典 Leptos 星系有三维轨道相机、邻域与编辑，并仍服务桌面和手机入口。

Let NSH users find, explore, and manage memory notes from the graph. Native desktop and TSX Web share the presentation, and Core owns visibility and note persistence. The current TSX graph queries a 100-note window, supports picking and button zoom, and displays relationships as text. The classic Leptos graph offers a 3D orbit camera, neighborhoods, and editing and still serves desktop and phone entry points.

## 方案选择 / Approaches

| 方案 / Approach | 取舍 / Trade-off |
| --- | --- |
| **NSH 原生 2D，用户选定 / NSH native 2D, selected** | 复用现有 NSH Canvas、共享 Rust 布局和跨端输入。二维交互可以用现有画布 API 验收，轨道观察需要另一套渲染实现。 / Reuse NSH Canvas, shared Rust layout, and cross-platform input. Existing canvas APIs cover 2D navigation, while orbit viewing requires another rendering implementation. |
| 首发原生 3D / Native 3D in the first release | 保留空间体验，同时增加三维场景、深度拾取、GPU 生命周期和 Web fallback 验证。 / Preserve the spatial experience with additional work on scene representation, depth picking, GPU lifecycle, and Web fallback. |

参考设计来自 Graphify 的关系来源、CodeGraphyV3 的稳定布局配置、JSON Canvas 的显式端点、react-jsoncanvas 的 ID 锚定、X6 的焦点样式和 infinite-canvas-tutorial 的缩放与裁剪。Aleph 已有 `NoteAddress`、`NebulaIndex`、`ForceLayout` 和 JSON Canvas 协议类型。交换格式可复用现有类型。此设计的交付对象是记忆浏览和既有管理能力，专用导入、导出和独立连边编辑属于后续功能范围。

Reference patterns include relation provenance from Graphify, deterministic layout settings from CodeGraphyV3, explicit endpoints from JSON Canvas, ID-based anchors from react-jsoncanvas, focus styling from X6, and zoom and culling from infinite-canvas-tutorial. Aleph already has `NoteAddress`, `NebulaIndex`, `ForceLayout`, and JSON Canvas protocol types. The existing types are the basis for future interchange. The scope here is browsing and existing note management.

| 参考实现 / Reference | Aleph 差距与落点 / Gap and implementation |
| --- | --- |
| Graphify · [关系来源 / Edge provenance](https://github.com/Graphify-Labs/graphify/blob/0f22ae762e3e3d0b689fc7ad2b8192740628413b/graphify/build.py) | Aleph 已有命名、生命周期与置信度字段。详情使用 Core 提供的活动目标和状态，画布区分显式关系与派生相似性。 / Detail uses Core’s live targets and lifecycle status. The canvas distinguishes explicit relations from derived similarity. |
| CodeGraphyV3 · [布局随机种子 / Layout seed](https://github.com/joesobo/CodeGraphyV3/blob/04a4032d663e07c97e249ea79477585725e4c517/src/view/Graph.vue#L80) | 复用共享 Rust 的稳定种子和有界布局，场景内手势复用同一组坐标。 / Reuse the shared Rust seed and bounded layout. Gestures reuse the scene coordinates. |
| JSON Canvas · [边端点 / Edge endpoints](https://github.com/obsidianmd/jsoncanvas/blob/456f843cb293df4f4ab1763e22ccb46a80b307c8/spec/1.0.md) | 视图和关系跳转统一使用 Core 精确地址，方向保留在绘制和详情里。 / Navigation uses Core’s exact addresses. Drawing and detail preserve direction. |
| react-jsoncanvas · [锚点解析 / Anchor lookup](https://github.com/Digital-Tvilling/react-jsoncanvas/blob/a51eee5a8e401456ae72e8136a6d9b5ee895507f/src/components/Edges.tsx#L18) | 复用 `NebulaIndex` 解析端点，绘制和拾取共享同一投影。 / Resolve endpoints through `NebulaIndex` and share the projection for drawing and hit testing. |
| code-call-graph-editor · [状态样式 / State styles](https://github.com/dearmydear/code-call-graph-editor/blob/1ab3d42907b2c83e440ff7c90b43005a6ef1a1c5/src/webview-x6/index.ts#L130) | 优先绘制选中笔记的邻边，并在 Core 有数据时显示社区、桥接节点和异常边。 / Prioritize selected note links and display communities, bridge notes, and surprising links when Core provides them. |
| infinite-canvas-tutorial · [摄像机 / Camera](https://github.com/xiaoiver/infinite-canvas-tutorial/blob/aafefcee1de41651aafcb6f6bdd05a7c896617e8/packages/lesson_006/src/plugins/CameraControl.ts#L137) · [裁剪 / Culling](https://github.com/xiaoiver/infinite-canvas-tutorial/blob/aafefcee1de41651aafcb6f6bdd05a7c896617e8/packages/ecs/src/systems/ViewportCulling.ts#L200) | NSH Canvas 使用指针锚定缩放，并按视口裁剪节点和边。 / NSH Canvas anchors zoom under the pointer and clips nodes and links to the viewport. |

## 第一阶段 · 浏览与视口 / Phase 1 · Browsing and viewport

1. **先接数据 / Connect data first.** 复用 `aleph:memory-nebula`、`graph.query`、`graph.search`、`graph.node_detail`，以 `(agent_id, node_id)` 串起画布、列表、搜索和详情。旧响应缺少精确分区时仍可展示条目。可解析的活跃关系和反向链接能打开目标，悬空和墓碑关系显示其状态。只有 Core 明确提供目标地址的关系才作为导航入口。 / Reuse the capability and three read operations. Join canvas, list, search, and detail by exact note address. Old rows missing a verified partition can still be displayed. Active links and backlinks navigate when Core supplies the target address. Dangling and tombstoned rows retain their status.
2. **聚焦图外笔记 / Focus a note outside the window.** 全局概览继续使用有界窗口。图外搜索结果可以直接查看详情，再用显式聚焦动作请求当前笔记及一跳邻域。先复用 SQLite NoteStore 的分区读取和 `get_neighbors` 的遍历思路。聚焦窗口只计入可见、仍存在的活跃关系端点，严格限制大小并提供稳定顺序。必要时给 `graph.query` 增加可选的精确 `focus` 地址和可验证的聚焦响应。Core 明确过滤两端并验证目标可见性。UI 保留返回概览的动作，查询失败可以重试并保留已有画面。 / Keep a bounded overview. Open off-window search hits directly in detail, with an explicit action for a one-hop view. Reuse the scoped SQLite store and the traversal pattern in `get_neighbors`. The focused window counts visible, existing endpoints of active links, with a strict size bound and deterministic ordering. Add an optional exact focus and an explicit response marker to `graph.query` if needed. Core verifies visibility and filters endpoints. Preserve the current view on failed requests and offer a return to overview.
3. **共享场景 / Share the scene.** 共享 Rust 重用关系索引和力导向算法，构建有稳定标识的有界布局。NSH TSX 持有交互状态并绘制。画布支持拖拽平移、指针锚定缩放、适配全图、选中聚焦、节点标签和关系密度。拖拽跨过阈值后按移动处理，释放点选使用实际绘制的投影。画布以外的列表和按钮提供键盘可达的路径。冷缓存下仍能浏览，社区和洞察在已有数据时呈现。 / Reuse the Rust relation index and force layout for bounded, stable scene geometry. NSH TSX owns interaction state and drawing. Provide panning, pointer-anchored zoom, fit, focus, labels, and edge density. A drag threshold separates motion from click, and picking shares the drawing projection. Lists and controls provide a keyboard path. The graph works with a cold cache and presents optional communities and insights when present.
4. **控制交互成本 / Bound interaction cost.** 图窗口变化时重新布局。平移、缩放和选择复用布局与邻接索引，合并重绘并裁剪不可见几何。边预算优先保留选中邻边。先测现有 Rust 布局和 Canvas，再根据瓶颈调整。首版保留 100 节点默认概览，500 节点用于密集场景验证和上限试验。 / Recalculate geometry when the graph window changes. Reuse the layout and adjacency during input, coalesce redraws, and cull offscreen geometry. Budget edges with selection neighborhoods first. Measure the existing Rust layout and Canvas before tuning. Keep the 100-node overview default and use a 500-node dense case for validation and sizing.
5. **处理连接状态 / Handle connection state.** 区分加载、空图、模块不可用、失败和断连。各读请求按连接代次与选择判定结果是否仍有效。刷新保留仍存在的选择和视口，请求失败保留最后成功画面。使用分区过滤的 Core 判据约束每次读。 / Distinguish loading, empty, unavailable, error, and disconnected states. Discard results from stale connections or selections. Refresh preserves valid selection and camera state, and a failed request keeps the last successful view. Each read goes through Core partition visibility.

## 第二阶段 · 接通记忆管理 / Phase 2 · Connect note management

- **管理动作 / Actions.** 接通已有的编辑正文、改名、删除。编辑以现有笔记的完整 Markdown 为单位，显式关系仍由笔记内容表达。派生相似边只用于浏览。删除沿用现有墓碑语义。 / Connect existing Markdown editing, rename, and delete. The note remains the persistence unit for explicit links. Similarity edges stay in the read projection. Deletion keeps the current tombstone behavior.
- **Core 契约 / Core contract.** 在现有 Memory runtime 和笔记索引器上收口兼容的精确路径写入。共享写请求携带精确 `NoteAddress` 和从同一份读出内容派生的修订标记。Core 复用现有 `lock_note_corpus` 作为写入互斥边界，在锁内校验版本、同名碰撞和工作分区，入口继续使用 Core 的调用者权限判定。并发约束覆盖经过该写入边界的 Core 操作。磁盘与索引部分成功需要可识别、可恢复的结果，UI 以 Core 返回状态为准。 / Build exact-path mutations on the existing runtime and indexer. Requests carry the exact address and a revision derived from the note snapshot used for editing. Core uses the existing `lock_note_corpus` for the mutation boundary, checks the revision, destination collision and corpus inside the guard, and uses the existing Core caller authorization at the entry point. Concurrency guarantees apply to Core writers through that boundary. Report and reconcile partial file and index outcomes explicitly.
- **能力与入口 / Capability and entry points.** 从同一 Core renderer 声明派生管理动作的可用性，接入现有 v1 桌面与 Web 写准入、身份校验和幂等策略。读能力降级时能独立工作。失败时保留编辑草稿，写成功后按 Core 返回的新地址或删除结果刷新。旧客户端仍按原契约调用。 / Derive action availability from Core's renderer declaration and integrate with the existing v1 desktop and Web identity and idempotency policy. Reads remain usable when management is unavailable. Keep edits on failure, and refresh from the returned address or deletion outcome. Legacy callers keep their existing contract.

## 清理与验收 / Cleanup and acceptance

- 迁移后删除 NSH 旧的固定画布高度、中心缩放与重复投影、重复 DTO 适配和已被替换的刷新分支。经典画布及宿主的清理以真实入口和迁移验证为依据。目前经典入口仍由已发布宿主和手机使用。对照 `FEATURE_LOCATOR.md` §6.3、`DESKTOP_SHELL.md` 和 `MEMORY_SYSTEM.md` 校对职责、能力边界与验证步骤。 / Remove the old NSH fixed canvas height, center-only zoom, superseded projections and DTO adapters, and stale refresh branches. Use live entry points and migration evidence to decide when classic code can be removed. Released hosts and phone still use that path. Align the feature locator, desktop shell, and memory reference docs with the verified paths.
- 在 macOS 原生与 TSX Web 重放同一有界数据，检查加载、图外搜索、聚焦、关系导航、拖拽、缩放后点选、宽度变化、断连重连和撤销挂载。测试同路径跨分区、失效关系、写版本冲突、权限拒绝、同名改名和索引失败。复用 `qa/nsh_s2_shell`，记录 100 和 500 节点的首个可交互画面耗时、交互帧 p95、峰值内存及基线环境。 / Replay the same bounded fixtures on native macOS and TSX Web. Check load, off-window search, focus, navigation, drag, picking after zoom, resize, reconnect, and unmount. Cover same-path cross-partition notes, stale relations, conflicts, permissions, rename collisions, and index failures. Extend `qa/nsh_s2_shell` to record first interactive frame, interaction p95, peak memory, and baseline conditions for 100 and 500 notes.
