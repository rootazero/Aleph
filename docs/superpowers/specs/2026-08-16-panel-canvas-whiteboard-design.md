# Panel 画布（Whiteboard Canvas）— 设计 Spec

> 日期：2026-08-16 · 状态：已获五节逐节批准（brainstorming 全流程）
> 参考项目：Cowart（`/Volumes/TBU4/Github/Cowart`，tldraw 5.x + React 19 + MCP widget）
> 执行协议：全量对齐 Cowart 六大能力并利用 Rust 优势超越；架构映射而非复制；熵减；全程 worktree 分支（`worktree-panel-canvas-whiteboard`），严禁触碰 main；实施用 Workflow 编排。

## 0. 决策记录（用户逐项拍板）

| 决策点 | 结论 |
|---|---|
| 功能范围 | **全量对齐并超越**：画布核心、AI 图片框、AI HTML 框、AI Slides、标注重生成、模型工具面 |
| 挂载位置 | **独立一等对象**（自有列表/库），可选关联项目房间；可见性沿用 owner + 房间名册同一套谓词 |
| 并发模型 | **乐观锁 + 实时广播**：revision 基线写入、冲突拒绝重拉、WS 事件增量广播。CRDT 明确不做 |
| 渲染路线 | **纯 Rust/Leptos 自研**：SVG DOM 形状层 + CSS transform 视口 + HTML overlay。tldraw 嵌入因商业许可（无 key 强制水印）+ 首个 JS/React 依赖 + 双状态源被否决 |

Cowart 能力 → Aleph 映射总表：

| Cowart | Aleph 对应 |
|---|---|
| tldraw 无限画布 widget | Leptos 自研 `views/canvas/` 编辑器（SVG + overlay） |
| `canvas/pages/<id>/{cowart-canvas.json, assets/}` | `<data_dir>/canvas/<id>/{doc.json, assets/<sha256>.<ext>}` |
| 10 个 MCP 工具（get/save_canvas_state、get/save_selection、save_view_state、insert_image、insert_html_draft、save_reference_image、read_page_asset、download_file） | 单个 `canvas` 多 action 内置工具（R8）+ `canvas.*` RPC 族；`save_view_state`（相机位）**刻意不做**——每设备 UI 偏好归 Panel localStorage |
| fractional-indexing (npm) | Rust 移植分数索引（~80 行） |
| html2canvas 标注截图 | WASM 内置光栅化器（SVG→Canvas2D→PNG），兼作画布 PNG 导出 |
| Codex 执行生成 | Aleph 模型经 `generate_image` + `canvas(apply)` 执行（R7：Panel 零业务逻辑，只发结构化 chat 消息） |

## 1. 总体架构与命名熵减

四层结构，依赖单向 Interface → Core → Domain（R1/R4/R6）：

```
Panel (Leptos WASM)   interfaces/webchat/src/platform/wide/views/canvas/   新白板编辑器
   ↕ JSON-RPC (WS)
Gateway RPC 面        src/gateway/handlers/canvas.rs                        canvas.* RPC 族 + 事件 topic
   ↕
Core 领域层           src/canvas/                                           文档模型 + store + 可见性
   ↕
模型工具面            src/builtin_tools/canvas.rs                           单个 canvas 多 action 工具
Wire 契约单一源       shared/protocol/src/canvas.rs                         双侧共用类型（构造而非仅解析）
```

**命名熵减（先行阶段）**：现 `views/canvas/` 是记忆星系 3D 可视化，名不副实。

1. 星系代码迁移 `views/canvas/` → `views/memory/galaxy/`（本就由 `MemoryState` 驱动）；`canvas_engine/` 中星系专属部分随迁，通用部分按实际耦合拆分或共享。
2. 新白板接管 `canvas` 命名：产品名「画布」，`views/canvas/`、`canvas.*`、工具 `canvas`、`src/canvas/` 四面一名，无第二真源。
3. 纯 move + import 修正，旧路径零残留。

**平台范围**：v1 只做 wide；phone 显示库列表 + 「请在桌面端编辑」。

## 2. 数据模型与持久化

`shared/protocol/src/canvas.rs`（serde + schemars）：

- **一张画布 = 一个无限单页文档**。不做 Cowart 的 pages 子层；画布库列表即页列表。
- `CanvasDoc { id: Ulid, title, owner_user_id, project_id: Option<_>, revision: u64, shapes: Vec<Shape>, decks: Vec<Deck>, created_at, updated_at }`
- `Shape` 枚举，公共字段 `{ id, x, y, w, h, z: FracIndex, parent_id: Option<_> }`；变体：
  `Geo`（矩形/椭圆）· `Ink`（自由笔迹点序列）· `Text` · `Note`（便签）· `Image { asset, natural_size }` · `Arrow { from, to, 可选端点绑定 shape_id }` · `Frame`（容器，可设 16:9）· `Html { asset }` · `AiImageFrame { prompt, refs, status: Draft|Pending|Done }`。旋转 v1 不做。
- **z 序分数索引**：插入/置顶不重写全表。
- **Slides = `Deck { id, title, frame_ids }`** 引用画布上的 **Frame 形状**（一页 = 一个 Frame；页内内容以 `parent_id` 挂在该 Frame 下，Image/Html 皆可）；播放不复制内容。

**写入协议 ops-based**：唯一写入口 `canvas.apply { canvas_id, base_revision, ops }`，
`CanvasOp = UpsertShape | DeleteShape | SetDocMeta | UpsertDeck | DeleteDeck`。
服务端原子应用 → `revision + 1` → 持久化 → 广播。`base_revision` 不匹配 → 专用冲突错误（携带当前 revision）；客户端重拉重放，模型工具拿紧凑结构化错误自愈（A2）。

**持久化**：`<data_dir>/canvas/<id>/{doc.json, assets/<sha256>.<ext>}`，文件即真源（素材用户可直接取用）。

- 路径一律 `utils::paths` 单一源（§5.8 判据：写者与读者必须是同一个函数；诊断/只读面不用会建目录的 helper）。
- 写入 `atomic_write_file` + store 内每画布写锁，读-改-写同临界区（`MetaGuard` 先例，创建路径同属读-改-写）。
- 列表 = 枚举目录读 doc 头；坏文件**点名 warn 后跳过**（§5.23b：跳过必须出声）。
- 素材：sha256 内容寻址、字节上限（量纲按字节）、mime 白名单（图片 + html）、保存时按 shape 引用扫孤儿回收（对齐 Cowart `getAssetIdsForShapes`）。

**可见性**：`canvas_visible_to(actor, doc)` = owner 或（`project_id` 关联 且 `roster::is_member`）——与会话同一套推导。工具面 actor 用 `visibility::ambient_actor()`（不是 `ambient_owner()`，§5.22 round-3 四谓词之坑）。

## 3. RPC 面、事件面、模型工具面

### RPC 族（`src/gateway/handlers/canvas.rs`）

| RPC | 语义 |
|---|---|
| `canvas.list` | 可见性过滤的文档头列表 |
| `canvas.create { title?, project_id? }` | 建画布 |
| `canvas.get { canvas_id }` | 整份文档 + 当前选区 |
| `canvas.apply { canvas_id, base_revision, ops }` | 唯一写入口；冲突返回专用错误码 + 当前 revision |
| `canvas.delete { canvas_id }` | owner-only |
| `canvas.asset.put / canvas.asset.get` | 素材上传/下载（base64） |
| `canvas.selection.set { canvas_id, shape_ids }` | Panel 推送选区，防抖，进程内暂存（选区本质短命，不欠 sidecar）。语义 = **该画布最近一次推送的选区**（多客户端后写胜出，够回答模型「用户选了什么」这一问） |

注册纪律：

1. 读 RPC（`get/list/asset.get`）**显式进 `gateway/lane.rs::override_for`**——名字无 `read_` 前缀，启发式认不出，漏了落 Mutate 车道被幂等键守卫拒（§6.8）。
2. 错误走三分类咽喉（not-found / 调用方可改 / 内部错），不用 `Result<_, String>`（§0）；不加 `From<String>`。
3. DTO 从 `shared/protocol` 契约类型**构造**响应（超发在编译期不可能，§0「解析只证超集」）。

### 事件面

- Topic `canvas.updated { canvas_id, revision, ops, actor }`。
- **提交在写锁内完成后原地发布**（roster 先例：变更+快照+发布同一临界区，保证事件序 = 提交序）。
- 事件臂按连接身份做 `canvas_visible_to` 过滤（§5.22：一条连接有两个方向，不能只闸请求臂）。
- 客户端收 ops 增量应用；revision 跳号 → 整拉重建。
- Panel 重连重放的 topic 清单**加上** canvas topic（§6.1 三字面量收窄之坑）。

### 模型工具面

单个 `canvas` 工具多 action（对齐 `goal`/`scratchpad`/`workspace_manage` 形状）：
`list / create / get(detail: summary|full) / apply(ops) / insert_image(路径或 asset) / insert_html(路径或内联)`。

- `get(summary)` 返回形状清单 + 包围盒 + 文本摘录，省 token；选区随 `get` 返回，不设独立 action。
- 按 action 声明元数据：`get/list` 只读；`create/apply/insert_*` 可写非破坏（Auto 档放行）；`delete`（经 apply 的 DeleteShape 不算——指整画布删除，不给工具面，仅 RPC owner 动词）。Plan 档地板自动拒 mutating（既有 `scoped/` 机制，零新闸）。
- 模式分区：`work`/`code` 可见，`chat` 不进。
- **已知代价 ①**：描述受 `catalog_description_bytes_ratchet` 棘轮（只减不增）。canvas 描述压至 ~1.2KB，同笔改动内答 3 问抬顶或删别处冗余抵账。
- **已知代价 ②**：五处登记（definitions / groups / constructor 构造段 + schema 段 / dispatch）漏一处即静默（§5.21 同款）——census 测试点名。

## 4. Panel UI 交互与 AI 内容流程

### 渲染与交互

- **三层同变换**：外层捕获 pointer/wheel → 视口 signal（translate + scale，单 CSS transform）；中层单个 `<svg>` 渲染矢量形状（`<For>` 按 shape id 键控）；上层 HTML overlay 承载文本编辑、sandboxed iframe、AI 框 prompt 输入、选择手柄。
- **交互状态机**独立模块：工具模式（选择/平移/画笔/几何/便签/箭头/框/文本）× 指针状态（idle/拖拽/框选/绘制/缩放手柄）。命中测试 = SVG 事件 + Rust bbox 框选。
- **笔迹**：Rust 移植 perfect-freehand 轮廓算法（~200 行，压力/平滑）——WASM 内联，性能超越点。
- **撤销/重做**：ops 可逆——本地逆操作栈，undo = apply 逆 ops（每客户端本地，不持久化）。
- **乐观应用**：本地先应用再发 `canvas.apply`；冲突 → 重拉 + 重放待定 ops。
- i18n 走 leptos_i18n（en/zh）；配色走主题 token（明暗双档）。

### AI 三流程（Panel 零业务逻辑，R4/R7 铁律）

1. **AI 图片框**：用户建 `AiImageFrame`（prompt + 可选参考图）→ 点「生成」→ Panel 仅向当前会话 `chat.send` 结构化消息（"为画布 X 框 Y 生成图片，prompt…，参考 assets…"）→ 模型 `canvas(get)` 读框 → `generate_image` → `canvas(apply)` 放回 Image 形状并推进状态（Draft→Pending→Done 由 ops 驱动，实时可见）。
2. **AI HTML 框**：同流程；模型产单文件 HTML 经 `insert_html` 落 asset；sandboxed iframe 渲染，默认 16:9。「按页数生成 Slides」= 模型插 N 个 16:9 Frame（各含一个 Html 子形状）+ 一个引用这些 Frame 的 Deck。
3. **标注重生成**：用户在图上画标注 → 选中「按标注重生成」→ WASM 光栅化器合成标注 PNG → `asset.put` → `chat.send` 携原图 + 标注图引用 → 模型生成干净新图插原图旁。

### Slides 播放 / 导航

- Deck 面板列 decks；全屏 overlay 按 `frame_ids` 逐帧铺满播放（←/→/点击翻页，Esc 退出）。
- 侧边栏新增「画布」入口 → 库页（标题 + 更新时间；缩略图 v1 不做，列为增强项）→ 编辑器。

## 5. 安全、错误处理、测试与 QA

### 安全

- **Html iframe**：`sandbox="allow-scripts"`，**不给** `allow-same-origin`——模型 HTML 在 opaque origin 跑，够不到 Panel RPC/storage/cookie。画布不进 transcript；对外导出 v1 仅 PNG，`session.export_html` 零 `<script>` 硬约束不受影响。
- 素材：字节上限、mime 白名单、sha256 寻址（无用户可控文件名，无路径穿越面）。
- 权限：见 §3 工具元数据；每个 RPC + 每个工具 action + 事件臂三处过同一 `canvas_visible_to`（§0「一个动词有 N 个面」）。

### 错误处理

冲突 → 专用错误码（Panel 自动重拉重放，模型自愈）；坏文件点名 warn 跳过；素材缺失 → 占位渲染不崩；光栅化失败 → toast，不上传半成品。

### 测试

- **core**：ops 应用/求逆、分数索引序（property test）、store（原子写/锁/孤儿回收/revision）、可见性三角色、冲突路径。
- **wire**：键集相等测试从契约类型派生（不写字面量清单）；Panel 侧 deserialize 对账。
- **守卫**：lane 登记、工具五处登记 census、描述棘轮、事件可见性过滤。
- **验证**：最小五条命令 + `cargo test -p aleph-panel --lib`（Panel 有改动必跑 --lib，不是 check）。
- **真机 QA**：`qa/canvas/` + chrome-devtools-mcp e2e——建画布/绘制/**双标签页实时同步**/模型工具插入/AI 图片框全流程（mock provider 配方）/标注导出/Slides 播放。每个动词有效果断言。

### 实施编排

全程本 worktree 分支；实施用 Workflow 编排（阶段：星系迁移 → core+协议+存储 → RPC+事件 → 工具面 → Panel 编辑器 → AI 流程+Slides+标注 → 真机 QA）。收尾：**FEATURE_LOCATOR.md 新 §、`docs/reference/CANVAS.md` 新建、CLAUDE.md 子系统路由表加行、文档索引加行**。

## 6. 刻意不做清单（YAGNI 记录，防重提）

- CRDT / OT 实时协同（乐观锁 + 广播已覆盖人+模型交替写的实际场景）
- Cowart pages 子层（库即页列表）
- `save_view_state` 相机同步（每设备偏好归 localStorage——多设备判据反向适用）
- 形状旋转、画布缩略图、HTML/SVG 对外导出（PNG 已覆盖标注流程）、phone 编辑、独立 CLI/TUI 客户端面
- tldraw 嵌入（商业许可 + JS 依赖 + 双状态源，已否决）

---

## 7. 实施状态与偏差记录（实施后补记）

> **状态：已完成（2026-08-17）**，worktree 分支 `worktree-panel-canvas-whiteboard`，计划 21 任务全交付。运行参考（实现后的权威文档）：[docs/reference/CANVAS.md](../../reference/CANVAS.md)；落点索引：FEATURE_LOCATOR §6.10；真机九项清单：`qa/canvas/`。以下逐条记录实施与本 spec 的偏差——每条都指得出代码锚点与测试名，按「计划期裁定」与「实施期新增」分组。

### 计划期裁定（计划自审 §4 已预告，实施证实）

1. **`canvas_format`/`canvas_io` 更名让位，不是 CUT**。spec §1 把它们列为命名让位对象时倾向清除；验证发现两者都有活消费者（`src/workflow/{store,proposal}.rs`、`src/teams/workflow_canvas.rs`、`src/tasks/cron/carryover.rs` 等七处）。落地为纯更名：`shared/protocol/src/json_canvas.rs` + `src/json_canvas_io.rs`，自带 round-trip 测试整批跟随，模块 doc 首行注明改名缘由。
2. **素材展示从「RPC base64」升级为能力 URL 字节路由**。spec §3 表里 `canvas.asset.get` 是唯一下载通道；实现另建 `GET /canvas-asset/{cap}/{canvas_id}/{asset_id}`（`src/gateway/server/canvas_asset_route.rs` + `src/gateway/security/canvas_caps.rs`，镜像 artifact 先例：浏览器缓存 + 不过 WS）。`canvas.get` 铸 10 分钟 TTL 的 canvas-scoped cap 填 `asset_base`，Panel `<image href>` = `{asset_base}/{asset_id}`（守卫 `asset_href_is_the_minted_base_plus_one_path_segment`）。`canvas.asset.get`（base64）**保留**——它是 srcdoc 文本与 AI 附件的回读通道（能力路由是给 `<image href>` 的，不作回读）。守卫：`a_valid_capability_serves_immutable_private_bytes` / `asset_route_refuses_expired_or_mismatched_cap` / `html_asset_is_served_as_plain_text` / `an_svg_asset_keeps_its_type_but_is_sandboxed`。
3. **冲突错误铸了专用码 `REVISION_CONFLICT = -32031`（而非复用 INVALID_PARAMS）——但 Panel 实际按 message 文本分支**。spec §3「客户端按专用错误码自动重拉重放」只兑现了一半：码在 wire 上（`src/gateway/protocol.rs`，映射唯一发生在 `handlers/canvas_error.rs::respond`，`apply_conflict_maps_to_revision_conflict_code`），但 `DashboardState::rpc_call` 的消息循环把 pending RPC 错误解析成 `error.message` **单独一个字符串**——码对每个方法统一掉地。Panel 因此按服务端用测试钉住的消息形状分支（服务端 `the_conflict_message_names_the_current_revision` ↔ Panel `ops.rs::is_revision_conflict` + `the_conflict_detector_matches_the_phrase_the_server_mints` 两侧对账）。**记录为已知债**（`api/canvas.rs` 模块 doc 原文）：`rpc_call` 若开始透传 code，冲突检测应改按码分支——码先于消费者存在，为的就是那一天。**→ 2026-08-17 遗留轮已还清**：消息循环保留完整错误对象（`context.rs::RpcFailure`），`rpc_call` 单点投影回 message（String 消费者字节不变），`rpc_call_with_code` 供带码消费；`REVISION_CONFLICT` 上移 `aleph_protocol::jsonrpc`（两侧共读一个常量），`api/canvas.rs::CanvasApplyError` 边界分类，`ops.rs::is_revision_conflict` 短语匹配器与两侧对账测试整体删除。

### 实施期新增（spec 未预见，committed reality）

4. **`views/canvas/toolbar.rs`（计划外文件）**：spec §4 与计划 Task 12–14 的文件清单都没有工具切换器——落地后每个创建工具（Geo/Note/Text/Frame/Draw/Arrow）都会是「无写者的信号」（capability wired ≠ capability delivered）。toolbar 是 `CanvasState::tool` 除编辑器建形后回落 Select 外的唯一写者，纯接线组件（工具模式语义由 `interaction.rs` 单测覆盖，如 `create_kind_maps_exactly_the_four_box_creation_tools`）。
5. **AI 框插入按钮**：同一失效的第二例——spec §4 流程从「用户建 `AiImageFrame`」开始，但没有任何 UI 入口能建它（只有模型经工具够得到）。落地为编辑器的 `insert_ai_frame` 回调（视口中心；坐标换算只有编辑器持有 surface rect + camera，故住 `editor.rs`）+ 纯函数 `ai.rs::insert_frame_ops`（守卫 `insert_frame_ops_creates_a_centered_draft_frame_with_a_deleting_undo`）。
6. **箭头头 = 计算出的 `<polygon>`，非 SVG `<marker>`**（计划 Task 12 草图写「箭头 marker」）：`shape_view.rs::arrow_head_points` 对退化（零长）箭头返回空串——NaN 顶点是 SVG 解析错误不是隐形三角；且同一函数被 `export.rs` 独立文档序列化共用（`<marker>` defs 在两个渲染面就是两份实现）。守卫 `arrow_head_is_symmetric_and_empty_for_a_degenerate_arrow`。
7. **`export.rs` 写死 hex 有理**（Panel「配色只用主题 token」规则的例外，模块 doc 专节论证）：导出 SVG 在脱离 DOM 的 off-screen image 里光栅化，app 的 custom properties 不存在——`var(--color-*)` 解析为空涂黑。hex 是 `shape_view::palette_var` 同槽位的 light-theme 读数，如印刷定墨。守卫 `svg_export_embeds_images_as_data_urls`（连带：image href 内联 data: URL——能力 URL 对他人 401 且外部 href 会 taint canvas 让 `to_data_url` 抛）。
8. **Deck 组建的帧序是阅读序启发式，非选区序**（spec §4 未规定序）：`decks.rs::selected_frames` 按 center-x 升序、center-y 破平、id 兜底——marquee 报的是文档序=创建序，不是用户看到的；抽屉拖拽重排是启发式误读的纠正路径。守卫 `selected_frames_keeps_only_frames_in_reading_order` / `selected_frames_ties_break_on_center_y_then_id`。
9. **`AiFrameStatus` 增 `Failed` 变体**（spec §2 写 `Draft|Pending|Done`）：生成失败要有可渲染的终态，否则框永远 Pending。锚点 `shared/protocol/src/canvas.rs`；Panel 状态推进纯函数 `ai.rs::frame_with_status` 测试覆盖 Failed 臂。
10. **工具面第 7 个 action `read_asset`**（spec §3 列 6 个）：模型标注/参考流程需要回读素材正文（text/html 返回字符串、image 返回 `_media` data URL）。锚点 `src/builtin_tools/canvas.rs`。
11. **lane 登记零改动**（spec §3 注册纪律第 1 条要求读 RPC 显式进 `lane.rs::override_for`）：实测三个读方法后缀恰是 `get`/`list`，既有 Query 启发式已覆盖——显式登记会是第二份答案。以 `handlers/canvas.rs::every_canvas_read_lands_in_query_lane` 钉住防未来改名漏 lane。同类：`method_visibility.rs` 里 `canvas.list` 是 `ListFiltered`（非全族 `KeyChecked`——list 无 addressed record），`canvas.create` 刻意缺席（`projects.create` 同裁定）。
12. **`fnv1a.rs` 落 `memory_graph/` 而非 `views/memory/galaxy/`**（计划 Task 1 草图把它分给 galaxy）：shared 半的 `category_color` 用它 hash 类别名，shared 不许伸进 view 私有模块（`memory_graph/mod.rs` doc 记录）。

### 文档收尾时顺带订正

- FEATURE_LOCATOR §0/§6.3/§2.5-邻近行里指向旧路径（`views/canvas/` 星系、`canvas_engine/`、`canvas_format.rs`、`CanvasView`）的锚点已同批订正为 `views/memory/galaxy/`、`memory_graph/`、`json_canvas.rs`、`GalaxyView`——「同一事实的两份表述，只改一份就是静默说谎」。

## 8. 真机 QA 结果（2026-08-17，qa/canvas/run.sh + chrome-devtools-mcp）

九条清单逐项（每条效果断言，非"没报错"）：

| # | 结果 | 效果断言 |
|---|---|---|
| 1 持久化 | ✅ | 矩形/便签/画笔 → 刷新重开 → doc.json 与 DOM 逐字节还原 |
| 2 双标签页广播 | ✅ | A 建形 B 实时 +1；B 移形 A 实时跟随；三方（A/B/doc.json）坐标收敛一致 |
| 3 并发竞写 | ✅（2026-08-17 遗留轮补测，冲突臂真机触发） | 首轮：A/B 交替拖同形状，两端编辑都落盘（rev 8→10）、零控制台错误、三方收敛，但真冲突窗（<100ms 帧传播）MCP 串行打不出。补测：`qa/canvas/latency_proxy.py`（tab A 上行 +2.5s、下行直通）+ 两侧页内绝对时钟编排（A 自拖 r1，B 晚 1.5s 自拖 r2）→ B 先落地、A 的 in-flight apply 带过期 rev 到达 → 代理捕获 `-32031` 下行帧（`CONFLICT FRAME SEEN`）→ A 免刷新恢复：rev 5→7，r1=A 目标、r2=B 目标，两标签页 DOM 与 doc.json 三方逐字节收敛，**两端改动全存活**。顺带钉住：in-flight batch 不被 send 之后到达的广播 rebase（下行直通正是为此） |
| 4 模型 insert_html 实时 | ✅ | `tools.invoke canvas(insert_html)` → rev 20 → Panel 实时渲染 `sandbox="allow-scripts"` iframe（无 allow-same-origin），srcdoc 经 RPC 回读 |
| 5 AI 图片框全流程 | ✅ | 生成按钮 → chat → mock 模型三步照做 → `canvas(insert_image)` rev 19 框被图替换 → Panel 实时 `<image href=能力URL>`；重复调用被拒且错误带围栏+可行动提示（A2 顺带验证） |
| 6 标注重生成 | ✅ | 笔迹标注 + 选中「按标注重新生成」→ 合成 360×240 真 PNG 上传（第三个素材）→ chat 双附件 → 模型插新图于原图右侧 |
| 7 Slides | ✅ | 3 Frame 组 deck（非 Frame 被过滤，计数 (3) 准确）→ 全屏黑边播放 → →键 1/3→2/3 → Esc 退出 |
| 8 member 角色 | ✅（2026-08-17 遗留轮真机补测） | 配方落成可执行物 `qa/canvas/member_seed.py`（loopback 播种 member/房间/两画布/ticket）。真机：TLS+0.0.0.0 重启 → 浏览器走 LAN IP + TOFU 证书 + bootstrap ticket（消费后 URL 被擦除）→ `role:"member"`。断言全过：member 库**只见**房间画布（两个 operator 私有画布 DOM 级不存在，operator 对照组三个全见）；member 凭据 wire 直调 private id → `-32009 not found` 且**与真正不存在的 id 同形**（no-oracle）；房间画布 member 画矩形落盘 rev 2、operator 画椭圆**实时**到达 member 页（双向广播、零刷新）。顺带真机复证：loopback 出示 member ticket 仍回 `role:"operator"`——信任模型语义（回环恒 operator），正是此场景必须走 LAN IP 的原因 |
| 9 PNG 导出 | ✅（修复后） | 首测失败——见下 QA-1；修复后下载触发、blob 10.2KB、PNG 魔数 `89 50 4E 47` 验证 |

### QA 抓到并修复

- **QA-1（真 bug，已修）**：导出光栅化用 `blob:` URL 加载 SVG，Panel CSP `img-src 'self' data: https:` 不含 `blob:` → 图像加载被策略阻断，导出**从未在真机成功过**（936 条单测全绿也测不到 CSP）。修复：`export.rs::rasterize_svg_to_png` 改 `data:image/svg+xml;charset=utf-8,` + `encode_uri_component`（Unicode 安全、策略白名单内、不放宽 CSP）。
- **QA-2（当时记录「预存设计、不修」——2026-08-17 遗留轮证明这条记录是错的，已修）**：症状属实（画布每次 apply 都触发 `extension.reloaded` + `tools.changed` 全量广播），归因不实。`src/extension/watcher.rs` **早就有**一张 `runtime_data_dirs` 排除表，`get_data_dir()` 正在表上，而画布根就是 `<data_dir>/canvas/` ——所以它本该被挡掉。真因是**比较的两边拼法不同**：`notify` 报的是**解析后**的路径（macOS FSEvents 把 `/var` 重写成 `/private/var`），而排除表是从未解析的 path helper 拼出来的，于是 `starts_with` **一条都不匹配、整张排除表是死的**。实测证据（探针，非推理）：事件路径 `/private/var/…/data/canvas/cv-1/doc.json`，排除项 `/var/…/data`。
  - **命中条件**：`ALEPH_HOME` 路径上任一段是符号链接——即**每一次 QA 运行**（`$TMPDIR` 在 `/var` 下）、每个 `ALEPH_HOME=/tmp/…`、以及任何 home 本身是软链的部署。默认装机（`~/.aleph` 未软链）不受影响，这也是它四轮没被发现的原因。
  - **修法**：两边都过同一个既有归一化器 `canonicalize_best_effort`——排除表在 `effective_runtime_data_dirs()` 里归一，**监视根也在构造器里归一**（只归一排除表只修好 macOS：inotify/Windows 报的是按所给根拼出来的路径，两边必须同时归一才在所有平台成立）。这正是 `notes/watcher.rs:160` 为**镜像**理由做过的事（那边未解析的根让 `strip_prefix` 把整个 vault 判成"不是笔记"）。
  - **守卫**（两条都先手工证伪过，红过才留下）：`the_runtime_data_exclusion_survives_a_symlinked_root`（纯谓词，全平台红）与 `a_canvas_write_is_dropped_while_a_skill_edit_still_reloads`（真 `notify` 事件，**两半都断言**——画布写被丢弃 *且* 真实 skill 编辑仍然到达；只断言前一半的话，一个彻底哑掉的 watcher 也会绿）。
- **QA 仪器教训**（qa/canvas/README.md 同批补记）：① 合成指针事件后**同脚本内的同步 DOM 读数发生在 Leptos 微任务刷新之前**——所有断言读数必须放到下一次 evaluate 调用或 setTimeout 之后，否则 80 个探针全部自盲；② 按钮匹配用**精确 title**，`includes('选择')` 会先命中聊天 composer 的「为本轮**选择**模型」。

### 首次 AI 图片框尝试的未决异常

第一次 Generate 的 run 内 `insert_image` 未提交（doc 停在 rev 15），当时未开 `request_log`，回执不可追溯；第二次尝试 + 手动 `tools.invoke` + wire 测试全部绿。若复发：用 `mock_anthropic.py` 第 5 参数 `request_log` 截获 tool_result（本轮已证明该 oracle 可用）。**→ 2026-08-17 遗留轮**：`run.sh` 已把 `request_log` 无条件接线到 `$QA_ROOT/request_log.jsonl`——oracle 只在已经开着时才存在，此后任何复发自带回执。

### 2026-08-17 第二遗留轮（QA-2 + 四项零散尾巴）

1. **QA-2 已修**，且推翻了自己的记录——见上方 QA-2 条目。教训是判据清单 §0 那条的又一次实证：**开工修一条记录在案的 gap，第一步是去代码里确认它还成立**。这里"成立"的只有症状，归因（"watcher 范围是预存设计"）是错的，而错的归因把一个 5 行的真 bug 归档成了"别的子系统的设计选择"。
2. **`member_seed.py` 改为幂等**（find-or-create）。原因不是洁癖：重跑会多出第二对 "Operator private" / "Room canvas"，而 item 8 的 operator 对照组断言是**数数**（"三个全见"）——一个不幂等的播种器会悄悄推翻它自己要建立的那条断言。
3. **冲突 oracle 的 chunk 边界洞已补**：`latency_proxy.py` 的 `-32031` 扫描从"每个 TCP chunk 各扫一遍"改成流式 `ConflictScanner`（跨读携带 `len(marker)-1` 字节），并带 `--self-test`。旧写法在 6 字节 needle 跨 64 KiB 边界时漏报，实证：`[b'{"code":' + m[:3], m[3:] + b'}']` 旧扫描计 0、应为 1。README 里那句"缺行不是无冲突的证明"随之收窄成只剩压缩这一种成因。
4. **Panel 的 `/favicon.ico` 404 已消**（冲突 QA 里那条"无关 404"）。根因：`index.html` 根本没有 `<link rel="icon">`，浏览器于是自动请求 `/favicon.ico`，而 dist 里没有这个文件。修在**单一源**——runtime `index.html` 是 `justfile` 里那段 heredoc 生成的（`interfaces/webchat/index.html` 是 trunk 开发变体、不是出厂的那份），三处一起改并按 heredoc 逐字节重生成 dist。用内联 `data:` SVG（ℵ 字形）而非新增 dist 资产：请求整个消失，且落在 Panel 现有 CSP `img-src 'self' data: https:` 之内——与 QA-1 用 `data:` 换掉被 CSP 挡住的 `blob:` 同一条论证。真机复证：`/favicon.ico` 单独 curl 仍 404（根因未变），而页面加载的 5 个请求全 200、其中没有 favicon 这一条。
5. **`aleph-desktop-*` 三个限肢 crate 的 check 补跑**，并清掉它暴露的一个警告：`desktop/linux/src/lib.rs` 的 `AccessibilityCapability` import 没有跟着它唯一的使用点一起 `#[cfg(target_os = "linux")]`，所以**只在非 Linux 宿主上**报 unused——正是上一轮跳过的那条命令才看得见的东西。
6. **侧栏折叠态 / tablet 宽度的画廊：真机补测，两态全 PASS**（chrome-devtools-mcp，效果断言见下表）。

| 状态 | 断言 | 结果 |
|---|---|---|
| Wide 1440 基线 | 侧栏在流内 0–225，main 225–1440，三行画廊可达 | ✅ |
| Wide 折叠 | 侧栏被裁到 width 0 / `overflow:hidden`，三行**命中测试不可达**；main 收回全宽 1440；固定 toggle 在 (10,14) **可达** | ✅ |
| Wide 折叠→展开 | 点固定 toggle → 侧栏回到 225，三行全部重新可达 | ✅ |
| Tablet 900（自动） | `ff-tablet` 上身、`sidebar_collapsed` 被强制为真；侧栏 `absolute` / 256px / `translateX(-256px)` 滑出屏外；main **未被挤** (0,900)；toggle 可达 | ✅ |
| Tablet 900 揭开 | 侧栏滑入 0–256、`z-index:60` **浮在内容之上**，main 仍是 (0,900)——即 CSS 注释声称的"不 re-cramp"确实成立；三行可达 | ✅ |
| Tablet 行点击 | 点 "Beta sketches" → 编辑器打开该画布（SVG 在场），该行变 `nav-tile-active` 高亮 | ✅ |
| 边界 1024 | 判为 Wide：侧栏回流内（main.left == sidebar.right），选中态跨带存活 | ✅ |
| 边界 1023 | 判为 Tablet：强制折叠、absolute 滑出、编辑器仍开着、toggle 可达 | ✅ |
| 控制台 | 重载后 error/warn **零条**，5 个网络请求全 200 | ✅ |

> **仪器教训（第二条，与首轮那两条同族）**：`offsetParent` 在这里**两个方向都撒谎**——对 `position:fixed` 的固定 toggle 恒为 `null`（会把"可见"读成"不可见"），对被 `overflow:hidden` 裁掉的画廊行**非 null**（会把"已裁掉"读成"仍然可见"）。第一版探针因此同时谎报了两件事，差一点让我写下"折叠态下画廊仍然显示"这个假结论。可用的判据只有**命中测试**：取元素中心点 `document.elementFromPoint`，看回来的是不是它自己或其后代。

### 2026-08-17 遗留轮收尾（本 spec 的最后三笔账）

1. **§7-3 债已还**：`rpc_call` 透传错误码，冲突检测改按码分支（见 §7-3 追记与 CANVAS.md §3）。真机 QA 的冲突恢复（上表 item 3 补测）跑的正是**新码径**——`RpcFailure` → `CanvasApplyError::Conflict` → 重拉重放，wire 级证明分类链活着。
2. **item 3 / item 8 真机缺口清零**（上表补测记录）；两件 QA 仪器落成可提交物：`qa/canvas/latency_proxy.py`（上行延迟代理 + `-32031` 帧 oracle）、`qa/canvas/member_seed.py`（member 场景播种器）。
3. **clippy `--all-targets` 的 main 预存 bench 红已修**：`benches/sandbox_performance.rs` import 的 `create_platform_driver`/`SandboxPolicy` 早已更名/移位（`create_platform_driver_from_config` + `sandbox::policy::SandboxPolicy`），对齐后 bench target clippy 绿。
