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
