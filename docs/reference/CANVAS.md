# CANVAS.md — 白板画布 (Whiteboard Canvas)

> Spec 母本：[docs/superpowers/specs/2026-08-16-panel-canvas-whiteboard-design.md](../superpowers/specs/2026-08-16-panel-canvas-whiteboard-design.md)（五节逐节批准 + 实施状态/偏差记录）· 实施计划：[docs/superpowers/plans/2026-08-16-panel-canvas-whiteboard.md](../superpowers/plans/2026-08-16-panel-canvas-whiteboard.md)。本文是**实现后的运行参考**，全部内容从已提交的模块 doc 收割，不是计划的复述。
>
> 一句话：**一张画布是一份带 revision 的 JSON 文档；唯一写入口是 `canvas.apply { base_revision, ops }`，冲突拒绝重放；人（Panel）与模型（`canvas` 工具）经同一个 store、同一个可见性谓词、同一条事件广播交替编辑，实时互见。**
>
> ⚠️ 命名辨析（三个 "canvas"，只有第一个是本文）：① **白板画布** = `src/canvas/` + `views/canvas/` + `canvas.*` RPC + `canvas` 工具，四面一名；② **记忆星系** = `views/memory/galaxy/`（3D 轨道相机图谱，原名 `views/canvas/`，见 FEATURE_LOCATOR §6.3）；③ **Obsidian JSON Canvas 互换格式** = `aleph_protocol::json_canvas` + `alephcore::json_canvas_io`（原名 `canvas_format`/`canvas_io`，仅服务 workflow 画布互换）。

## 1. 四层架构

依赖单向 Interface → Core → Domain（R1/R4/R6），wire 契约单一源：

```
Panel (Leptos WASM)   interfaces/webchat/src/platform/wide/views/canvas/   编辑器（SVG 形状层 + CSS transform 视口 + HTML overlay）
   ↕ JSON-RPC (WS)          interfaces/webchat/src/{api,state}/canvas.rs
Gateway RPC 面        src/gateway/handlers/canvas.rs                        canvas.* 八方法族
Gateway 事件面        GatewayEventFrame::CanvasUpdated                      topic "canvas.updated"，ByCanvasScope 可见性分类
Gateway 字节面        src/gateway/server/canvas_asset_route.rs              GET /canvas-asset/{cap}/{canvas_id}/{asset_id}
   ↕
Core 领域层           src/canvas/                                           store + 每画布锁 + revision + 素材 + 选区
模型工具面            src/builtin_tools/canvas.rs                           单工具多 action（R8）
Wire 契约单一源       shared/protocol/src/canvas.rs                         双侧共用类型（服务端从它构造，Panel 用它解析）
```

### 代码地图

| 组件 | 位置 | 说明 |
|---|---|---|
| Wire 契约 | `shared/protocol/src/canvas.rs` | `FracIndex` / `Shape`(9 变体) / `CanvasDoc` / `CanvasOp`(5 词) / `Deck` / 全部 RPC DTO / `CanvasUpdated` 事件载荷 / 上限常量。serde 纪律照 `workspace.rs`：请求可选字段 skip-if-none；响应集合字段**无** default（缺键=协议错误）；`CanvasDoc.decks`/`owner_user_id`/`project_id` **有** default（旧文档前向兼容，`a_doc_without_decks_key_parses_as_empty_decks`） |
| Store 门面 | `src/canvas/store.rs` | `CanvasStore`：`create`/`get`/`list`/`list_entries`/`apply`/`delete` + 素材 API。一画布一目录 `<data_dir>/canvas/<id>/{doc.json, assets/}` |
| 锁纪律 | `src/canvas/doc_io.rs` | `MetaGuard` 模式（镜像 `gateway/session_store/file_backend/meta.rs`）：`write` 私有，唯一写入口 `DocGuard` 只能由「先取每画布 mutex 再读」的 `DocLocks::lock` 产出。**与孪生的刻意差异**：`commit(&mut self)` 写盘后**留锁**，事件在临界区内发布（见 §3） |
| 校验 | `src/canvas/validate.rs` | `ops_shape`（锁前纯形状闸：批量 ≤`MAX_OPS_PER_APPLY`、id 字符集 `[A-Za-z0-9_-]{1,64}`、ink ≤10 000 点）+ `apply_ops`（锁内就地应用 + `MAX_SHAPES` 后态闸；任何错误 guard 直接 drop，**拒绝的批次从不半落盘**） |
| 素材 | `src/canvas/assets.rs` | sha256 内容寻址 `assets/<sha256>.<ext>`，ext 来自 mime 白名单表**从不接受调用方文件名**（零穿越面）；孤儿回收带 `ORPHAN_GRACE`=1h 宽限（put→apply 竞速窗口），dedupe 命中 re-touch mtime 重新武装宽限 |
| 选区 | `src/canvas/selection.rs` | 进程内表（`OnceLock<Mutex<_>>`，`MAX_LIVE`=4096 满则淘汰最旧，`artifact_caps` 形状）；语义=该画布**最近一次推送**的选区，多客户端后写胜出。**刻意不欠 sidecar**：空选区是合法当前值，不是「重启后撒谎」那类（§4.13b 的反例） |
| RPC 族 | `src/gateway/handlers/canvas.rs` | 八方法；三条纪律见 §4/§7。boot 接线 `src/bin/aleph-server/commands/start/builder/handlers/canvas.rs`（store 构建于 `start/mod.rs`，与工具面共享同一枚 `Arc`） |
| 错误咽喉 | `src/gateway/handlers/canvas_error.rs` | 三分类 + `REVISION_CONFLICT`（见 §3）；源码守卫 `no_canvas_handler_writes_an_internal_error_code_of_its_own` |
| 能力表 | `src/gateway/security/canvas_caps.rs` | `CanvasCapabilities`：256-bit CSPRNG、常数时间比较、`CANVAS_CAP_TTL`=10min、**独立于** `artifact_caps` 的表（两类 cap 键空间不同，共表=互为候选密钥的混淆） |
| 字节路由 | `src/gateway/server/canvas_asset_route.rs` | 第二 ingress（明文远程拒绝 426 / Origin 403 / 自有限流桶 429 / cap 即授权）；XSS 边界见 §7 |
| 事件帧 | `src/gateway/events/frame.rs::CanvasUpdated` | 载荷 `{canvas_id, revision, ops, actor?, owner_user_id?, project_id?}`——**帧自报归属**（§4.8 地雷 H：解析句柄的安装条件不得比帧的生产条件窄），分类臂 `SessionIdentity::ByCanvasScope` 直接从帧上取 owner/project |
| 工具面 | `src/builtin_tools/canvas.rs` | 7 action：`list/create/get/apply/insert_image/insert_html/read_asset`（见 §5） |
| Panel API | `interfaces/webchat/src/api/canvas.rs` | 协议类型直用，零 `json!` 字面量零本地镜像 struct；源码守卫 `canvas_rpc_is_issued_from_api_canvas_alone`（`canvas.*` wire 串每侧一个拼写点） |
| Panel 状态 | `interfaces/webchat/src/state/canvas.rs` | `CanvasState`（Copy + 全 `RwSignal`，`app.rs::AppContent` 提供一次）；文档就是服务端的 `CanvasDoc`，**刻意不建第二份文档模型**。`rows_loaded` 是「`canvas.list` 答过没有」——空 `rows` 同时是「你没有画布」和「还没人问过」，两个消费者（侧栏列表、空态）都必须分得开 |
| Panel 画廊 | `interfaces/webchat/src/platform/wide/views/canvas/library.rs` | 左栏 `CanvasSidebar`（`ModeSidebar` 的 `PanelMode::Canvas` 臂）＋ 库动作单一源（open/create/delete/rename）＋ 纯函数 `filter_rows` / `pick_known_state` / `decide_title_edit`。见 §6 |
| Panel 编辑器 | `interfaces/webchat/src/platform/wide/views/canvas/` | 17 文件，见 §6 |
| Phone | `interfaces/webchat/src/platform/phone/canvas/mod.rs` | 只读库列表 + 「请在桌面端编辑」（v1 wide-only） |
| 集成测试 | `tests/canvas_wire.rs` | 全链 wire 对拍 + AI 模板工具名求解 + 事件三角色端到端（`--features test-helpers`） |
| 真机 QA | `qa/canvas/` | 见 §9 |

## 2. 数据模型

- **一张画布 = 一个无限单页文档**（无 Cowart pages 子层；库列表即页列表）。`CanvasDoc { id: "cv-<uuid-simple>", title, owner_user_id?, project_id?, revision: u64, shapes, decks, created_at_ms, updated_at_ms }`——时间戳一律毫秒 `_ms` 后缀（`MessageRecord.timestamp` 单位歧义教训）。
- **`Shape` tagged enum，9 变体**：`Geo`(rect/ellipse+文本) · `Ink`(点序列 `[x,y,pressure]` f32 三元组，shape-local 坐标) · `Text` · `Note` · `Image { asset_id, natural_w/h }` · `Frame`(容器，可 aspect_locked) · `Html { asset_id }` · `Arrow { start/end: ArrowEnd（可 bind 形状 id，x/y 为解绑回退）}` · `AiImageFrame { prompt, reference_asset_ids, status: Draft|Pending|Done|Failed }`。公共字段经 `#[serde(flatten)]` 摊平（`a_shape_round_trips_with_type_tag_and_flattened_common`）；未知 type 解析**失败而非静默丢弃**。`Shape::asset_ids()` 是孤儿 GC 走的引用清单。
- **z 序 = `FracIndex`**：`0-9A-Za-z` 62 进制字典序分数索引（ASCII 序即 rank 序，裸字符串比较就是 z 序），`between()` 缝隙收窄时**加长不重排**（重排会重写兄弟行）；从不铸出以最小位 `0` 结尾的索引（其下方任何长度都无空间）。守卫：`frac_index_between_is_strictly_ordered` / `frac_index_repeated_inserts_stay_bounded_and_ordered`（1000 次头插长度亚线性）/ `between_above_the_greatest_digit_still_finds_room`。
- **Slides = `Deck { id, title, frame_ids }`** 引用画布上的 `Frame` 形状；`frame_ids` 本身就是页序（**刻意不用分数索引**——列表小，整表重写）；播放不复制内容。
- **写入协议 ops-based**：`CanvasOp = UpsertShape | DeleteShape | SetDocMeta | UpsertDeck | DeleteDeck`，唯一写入口 `canvas.apply`。
- **上限常量**（全在契约模块，量纲按字节——CWE-400 教训）：`MAX_ASSET_BYTES` 10 MiB · `MAX_HTML_ASSET_BYTES` 2 MiB · `MAX_SHAPES` 5000 · `MAX_OPS_PER_APPLY` 500 · `MAX_TITLE_BYTES` 200 · `MAX_INK_POINTS` 10 000（validate.rs）。
- **标题闸 `check_title`（2026-08-17 补）**：`title` 有且只有两个写者——`canvas.create` 的可选 title 与 `SetDocMeta`——而在此之前**两个都不校验**：形状数/批量 ops/墨点/素材字节全有上限，唯一一个给人看的字符串一个都没有。闸住空白、控制字符（换行会撑破它现在赖以导航的单行行，也是在模型读回的文本里伪造结构的形状）与超 200 B。三条纪律：① 闸**住在契约里**，Panel 调的是同一个函数、按同样的理由拒同样的字符串；② 闸**只拒不改写**（`apply_ops` 存的就是收到的，规范化的闸会让盘上的值不等于调用方发的值；trim 归输入端）；③ 闸在**写**上，不在读上——盘上早于闸的长标题照常 list / get（读侧闸会让存量画布同时从每个面消失，正是 `list` 出声跳过所要避免的）。守卫：`create_refuses_an_inadmissible_title_instead_of_defaulting` · `a_set_doc_meta_over_the_title_cap_lands_nothing_from_its_batch`（含批内兄弟 op 也不落盘、不烧 revision）· `a_stored_title_that_predates_the_cap_still_lists_and_opens` · 契约侧 `the_title_cap_is_measured_in_the_unit_it_is_named_for`。**刻意不进工具 `DESCRIPTION`**：拒绝语自陈其因，模型下一轮自愈（A2），不值那几十字节。
- **持久化**：`atomic_write_file`（temp + fsync + rename）；读-改-写按构造是一个临界区（doc_io 模块边界）；创建路径**同属**读-改-写（id 先铸好走同一 `lock`）。坏 doc.json：`list` 点名 warn 跳过（跳过必须出声），`get` 报 `Internal`——「解析失败」和「没有」是两个答案（`a_corrupt_doc_json_is_skipped_loudly_by_list_but_errors_on_get`）。

## 3. 并发协议（乐观锁 + 实时广播）

**服务端**：`apply(id, base_revision, ops)` 在每画布锁内比对 `doc.revision != base_revision` → `CanvasError::Conflict { current_revision }`；通过则就地应用、`revision += 1`、原子落盘、**在同一临界区内发布事件**——`DocGuard::commit(&mut self)` 刻意与孪生 `meta.rs` 不同（那边 `commit(self)` 消费 guard 释放锁）：变更+落盘+发布同一锁作用域（roster 先例），**事件序按构造等于提交序**。守卫：`events_publish_in_revision_order_under_contention`（并发 20 次 apply 断言 revision 序列严格递增）、`concurrent_applies_serialize_one_wins_one_conflicts`、`a_dropped_guard_writes_nothing`。

**错误码**：`aleph_protocol::jsonrpc::REVISION_CONFLICT = -32031`（实现定义区间；2026-08-17 从 `src/gateway/protocol.rs` 上移到共享 crate——真源必须在被依赖的一侧，gateway 侧 re-export 保住既有 import 路径），映射唯一发生在 `canvas_error::respond`。冲突既非调用方之错也非我方失败，是三分类外的第四臂——码与它的消费者同批出生。

**Panel 按码分支（2026-08-17 还清的债）**——`DashboardState` 的消息循环曾把 pending RPC 错误解析成 `error.message` 单独一个字符串，Panel 只能按 message 文本 `contains("revision conflict")` 分支。现在消息循环保留完整错误对象（`context.rs::RpcFailure { code, message }`，纯函数 `parse_rpc_error` 单测钉住），`rpc_call` 在**唯一一处**投影回 message（150+ 处 String 消费者与 `admin_refusal` 分类器收到的字节逐字不变），带码面 `rpc_call_with_code` 供需要码的调用方使用。`api/canvas.rs::CanvasApplyError` 在 API 边界分类（`Conflict` / `Other(message)`），editor 按 enum 分支；短语匹配器 `ops.rs::is_revision_conflict` 与其源码对账测试已删。守卫：`a_revision_conflict_is_classified_by_its_wire_code`（两侧读同一个共享常量，改号即编译错）＋ `the_conflict_phrase_without_the_code_is_not_a_conflict`（负半边：文本恰好含冲突措辞的传输错误**不**触发重放——短语匹配时代做不到的断言）。本地铸造的失败（未连上/超时/通道关闭）`code` 恒 `None`，按构造不可能冒充服务端裁决。服务端消息形状照旧钉着（`the_conflict_message_names_the_current_revision`）——但那句话现在只服务人与模型，不再是协议面。

**Panel 冲突恢复**（`views/canvas/ops.rs`）：乐观应用（`apply_local` 与服务端 `validate::apply_ops` 同语义，`apply_local_matches_the_server_apply_ops_loop_verbatim` 对拍）→ 单飞发送（一次一批在途，新 ops 进队列合并，`the_queue_is_single_flight_and_acks_promote_queued_ops_in_order`）→ 冲突时 `recover` 整拉 + 队列按序重放重发（`recover_collapses_the_refused_batch_and_the_queue_in_order`）。undo/redo：ops 可逆（`invert`：upsert→前值 upsert/无前值 delete；性质测试 `apply_then_apply_inverse_round_trips_random_op_sequences`），栈上限 200。

**实时对账**（`views/canvas/reconcile.rs`，纯决策表）：收 `canvas.updated` 帧 → `revision == local+1` 增量应用；**自己的回声**按 `inflight.base_revision + 1` **且 ops 逐字节相等**识别后丢弃（revision 单独会误吞赢得同一 revision 竞速的**外来**批次；`actor` 刻意不是判据——同一用户两个标签页共享 actor，B 必须应用 A 画的）；`revision <= local` 陈旧不动；跳号整拉 `CanvasApi::get`（增量应用会静默跳过缺失 ops）。守卫：`a_foreign_batch_at_the_echo_revision_is_not_mistaken_for_the_echo` / `our_own_inflight_echo_is_dropped_by_revision_and_ops_match` / `a_revision_gap_refetches_the_whole_document`。

## 4. 可见性（一个谓词三张脸）

判据：owner 或（`project_id` 关联 且 `roster::is_member`）；`actor == None`（cron/tests）不设限——与 partition 孪生同约定。单一源 `src/gateway/visibility.rs::canvas_visible_to(owner, project, actor)`，body 只 delegate `roster::is_member` + `owner_or_legacy`，不复刻成员判断。

| 面 | 解析器 | actor 来源 |
|---|---|---|
| RPC | `canvas_visible` | `visible_owner_filter()` |
| 工具 | `ambient_canvas_visible` | `ambient_actor()`（`CALLER_USER` 在 spawn 出的 run 里恒 `None`，接 RPC 孪生=静默恒真，§5.22 round 2 ⑤） |
| 事件 | `SessionIdentity::ByCanvasScope { owner, project }` | 连接身份；臂 body 只许调 `canvas_visible_to` |

- **no-oracle**：不可见的画布与不存在的画布逐字节同形（`projects::project_not_found` 先例）；stranger 探测 `canvas.apply` 拿到的是 not-found **绝不是** conflict（revision 会成为存在性 oracle）——`apply_by_a_stranger_is_refused_as_not_found_never_conflict`。`gate_canvas` 对 store 错误也 fail-closed。
- **owner 盖章**：RPC 面 `scope::ambient_owner()`（gateway dispatch 里 `CALLER_USER` 活着）；工具面同样盖 owner——两处都登记在 `visibility.rs` 的 `AMBIENT_OWNER_CENSUS`（who-owns 问，与 who-asks 分开）。
- **`canvas.delete` 是唯一 owner-only 动词**：可见性放行 roster 成员后，owner 闸以 `PERMISSION_DENIED` 拒绝（他看得见，没有存在性可藏——诚实的拒绝自报其名）。**刻意无 org-admin override**：handlers 不携 `SecurityStore`，admin 收割去 store 根目录操作。
- `method_visibility.rs`：`canvas.list` = `ListFiltered`，其余六个 addressed 方法 = `KeyChecked`；`canvas.create` 是创建面刻意不登记（`projects.create` 同裁定，其 project 关联写入经 `project_visible` 闸——`create_with_a_foreign_project_link_is_refused_as_not_found`）。
- lane：读方法后缀恰是 `get`/`list`，Query 启发式已覆盖，`lane.rs` 零改动；`every_canvas_read_lands_in_query_lane` 钉住防未来改名漏 lane。
- 三角色端到端：`canvas_updated_admits_owner_and_roster_member_and_refuses_stranger`（event_visibility 全链）+ `tests/canvas_wire.rs::a_real_apply_broadcasts_a_frame_the_visibility_plane_scopes_correctly`。

## 5. 模型工具面（`canvas` 单工具多 action）

R8 孪生：`list / create / get(detail: summary|full) / apply(ops) / insert_image / insert_html / read_asset`。纯 I/O 翻译（R4/R7）——每个判决（可见性/冲突/素材闸/mime）都是 `CanvasStore` 的，**与 RPC 同一枚 Arc**（事件总线所在那枚，`workspace_manage` doc 教训）。

- **`apply` 不收 `base_revision`**：模型只会回声一个它上一口气读到的数。工具内部自读 revision → apply → 冲突按冲突点名的 revision 重试**一次** → 二次冲突压成紧凑错误交模型自愈（A2：让模型看见并自愈，不在 harness 里做重试矩阵）。`apply_retries_once_on_conflict`。
- **刻意无 `delete` action**：整画布删除是 RPC 面的 owner-only 动词；`apply` 的 `delete_shape` 覆盖一切编辑需要，模型侧的不可逆整文档动词只买来一个确认门问题。
- **`insert_image` 的 `location` 三形态**：`data:` URL 直接解码；本地路径只允许 `get_data_dir()` 树内或 `std::env::temp_dir()` 树内（**两边都 canonicalize 后再比**——§5.22 存储/展示形态判据；`insert_image_rejects_paths_outside_data_and_temp_roots`）；`https://` 经 reqwest 10s 超时 + Content-Type 必须 image/* + 字节上限。`frame_id` 给出 → 读框 bbox 放置 + 删除该 AiImageFrame（`insert_image_replaces_a_frame_in_place`）。
- **`insert_html`**：正文过 `MAX_HTML_ASSET_BYTES` → `put_asset(text/html)` → 16:9 Frame + Html 子形状（`parent_id`=frame），或 `frame_id` 替换模式。
- **`get(summary)`**：形状清单 + bbox + ≤80 字符文本摘录，省 token；`full` 整 doc；选区随 get 返回不设独立 action。
- **登记**：`definitions.rs`（描述指常量）、`create_tool_boxed` 臂、`groups.rs` `content_gen` 类、constructor 构造段+schema 段、dispatch 臂、`CHAT_DEFER_FAMILIES += "canvas"`（work/code 可见 chat 不进，`_` 词界连带覆盖未来 `canvas_export`）、`BuiltinToolConfig.canvas_store`（`Option<Arc<CanvasStore>>`，未接=工具不注册）。**不上 `READ_ONLY_TOOLS`**——一名多路复用读写，声明幂等等于告诉 exec tier 写是读（`file_ops` 同款裁定）。
- **棘轮账（2026-08-17，见 definitions.rs 常量 doc 的日期化注记）**：`CATALOG_DESCRIPTION_CEILING_BYTES` 102 000 → **102 955 B**（钉实测，刻意不留 slack）——基线自测 101 872 B（此前 +3 139 slack 已被兄弟轮消耗到 +128，均非本分支），canvas 自身份额 **+1 083 B** 即整份 DESCRIPTION，三问已答（运行时事实：Panel 实时联动/两个准入根/内部 revision 处理/框替换语义；强模型猜不出部署准入根；无别的工具说过）。`REGISTRY_SCHEMA_CEILING_BYTES` 92 746 → **92 798 B**（+52 B **全部**是 origin/main 语音重构合并 `5ae1814ad` 带来的无主漂移，canvas 按构造贡献零——注册门控在 `canvas_store` 上，无条件 map 够不到它）。`tools_without_an_unconditional_schema_are_pinned` 129 → **130**（+1 canvas，与 generation 工具同类）。

## 6. Panel 画廊与编辑器（`views/canvas/`，17 文件）

纪律：逻辑模块全部**无 DOM 纯函数** + native 单测（交互状态机 `interaction.rs`、ops 求逆 `ops.rs`、对账 `reconcile.rs`、视口数学 `viewport.rs`、freehand 轮廓 `freehand.rs`、deck ops `decks.rs`、素材摄取 `asset_ingest.rs`、导出序列化 `export.rs`）；组件只做接线（`editor.rs`/`mod.rs`/`toolbar.rs`/`text_edit.rs`/`present.rs`/`ai.rs` 的 view 半）。

- **画廊在左栏（2026-08-17 迁移）**：`library.rs::CanvasSidebar` 由 `ModeSidebar` 的 `PanelMode::Canvas` 臂渲染（`MemorySidebar` / `TeamsSidebar` 同惯用形，零新范式）；主区只剩编辑器与空态 `WelcomePane`。**旧形态是主区整页 `LibraryPane`，与编辑器互斥**——换一张画布要「返回库 → 找 → 进入」三步，且丢掉编辑器的相机与 undo 栈。四条纪律：
  1. **三条 liveness 线留在 `CanvasView`**（keep-alive，全程挂载一次），侧栏随分区切换挂/卸。装在侧栏＝每次进画布都重拉 + 订阅抖动，且是「库里有什么」的第二个答案；侧栏是 `rows` 的纯消费者 + `open_canvas` 的写者，写动作后的 refetch 是另一个理由。
  2. **`<Show>` 包 `<For>`，不是一个读 `rows` 的闭包包 `<For>`**——后者每帧重建它返回的东西，而它返回的正是 `For`，键控就成了装饰。
  3. **`For` 只按 id 键控，行自己用 memo 读数据**。把 `revision` 折进 key（旧库列表的做法）＝正在画的那张画布的行每批 ops 重挂一次；而 memo 读在**叶子**上（没有闭包包住整行），因为包住整行的闭包会重建它返回的子树，而子树里有重命名输入框。
  4. **三态不是两态**：`rows_loaded` 区分「还在问」/「你没有画布」/「没有匹配」——phone 端本来就有这个判据，宽端没有，于是冷加载期间对着一个还没问过的库说「还没有画布」。
- **重命名 = `SetDocMeta` 的第一个人类生产者（2026-08-17 接线）**：op、服务端 applier、模型工具面随子系统一起出厂，缺的只是人够得到的入口，所以人建的每一张画布永远叫 `Untitled`。列表化导航之后这不再是缺憾而是前提。两个面（侧栏行内 / 编辑器标题）走**同一个函数** `library::submit_title` → `rename_canvas`：同一道契约闸、同一个 no-op 跳过（改成同名也会烧一个 revision 并向每个客户端广播一帧）、同一份基准 revision 优先级（`pick_known_state`：开着的 doc 压过列表行——doc 每帧对账，rows 等一次独立的 list 往返）、**冲突重试一次**（镜像工具面纪律；重命名没有可 rebase 的东西，照原样重放就是对的）。重试读 canvas 但**刻意不 adopt 信封**——只取那一个整数，替换掉 doc 会丢掉用户正在拖的形状；新标题经 `canvas.updated` + `reconcile.rs` 到达开着的文档，与任何别的客户端的编辑同路。两条终局分岔是有意的：Enter 是「我说了算」，被拒就把红字留在输入框里；blur 是「我要走了」，把人困在一个他已经离开的红框里比丢掉一个闸本来就不会收的标题更糟。
- **搜索是客户端纯函数**（`filter_rows`，标题 + id 双匹配——id 是模型在聊天里报回来的东西）。R4：没有服务端画布搜索，加一个就是「什么匹配」的第二个答案。
- **三层同变换**：外层捕获 pointer/wheel → `Camera{x,y,zoom}` 单 CSS transform；中层单 `<svg>`（`<For>` 按 shape id 键控，z 排序按 `FracIndex` 字典序）；上层 HTML overlay（文本编辑/iframe/AI 面板/选择手柄）。`the_svg_and_css_transforms_agree`。
- **三条 liveness 线**（`mod.rs`，keep-alive 容器所以全挂 mount）：`is_connected` 门控的加载/重连 Effect（`WorkspacesView` 惯用形）；`subscribe_topic(canvas::TOPIC)` 每 mount（ledger 重连重放，**不进 `BASE_TOPICS`**）；帧消费 → 刷库行 + `reconcile.rs` 对账。
- **toolbar.rs**（计划外新增，见 spec 偏差记录）：浮动工具切换器，`CanvasState::tool` 除编辑器建形后回落 Select 外的唯一写者——计划漏了它，没有它每个创建工具都是「无写者的信号」（capability wired ≠ delivered）。
- **箭头**：绑定端点跟随目标 bbox 中心-边缘交点（`arrow_anchor` 四象限测试）；箭头头是**计算出的 `<polygon>` 而非 SVG `<marker>`**——退化（零长）箭头返回空串（NaN 顶点是 SVG 解析错误不是隐形三角），且同一函数被导出序列化器共用（`arrow_head_points`，`pub(super)`）。
- **导出**（`export.rs`）：纯字符串 SVG 序列化（镜像 `shape_view.rs` 逐臂）→ Blob → `HtmlImageElement::decode` → Canvas2D → PNG。两处刻意差异：image href 内联 data: URL（能力 URL 对别人 401，外部 href 会污染 rasterizing canvas 让 `to_data_url` 抛）；**颜色写死 hex**——主题 token 规则管的是 themed UI，导出是脱离 DOM 的独立文档，`var(--color-*)` 解析不到只会涂黑；hex 是 `palette_var` 同槽位的 light-theme 读数，如印刷定墨（模块 doc「Why this file spells hex colors」）。
- **Decks**（`decks.rs`/`present.rs`）：deck 是文档数据不是 UI 状态，经同一 ops 漏斗（可 undo、广播、revision 检查）。「组成 Slides」的帧序是**阅读序启发式**：center-x 升序、center-y 破平、id 兜底——**刻意不用选区序**（marquee 报文档序=创建序，不是用户看到的），抽屉拖拽重排是启发式误读的纠正路径。播放 `present_camera_for_frame` 纯数学 fit（信箱对称、竖帧横视口 fit 高度）。
- **AI 三流程**（`ai.rs`，Panel 零业务逻辑）：① AI 图片框——选框输 prompt 点生成，**发送前**先把框 upsert 成 Pending+参考 asset ids（每个客户端实时看到徽标翻转），参考图经 `canvas.asset.get` 拉 base64 作附件（能力字节路由是给 `<image href>` 的，不作回读通道）；② 标注重生成——合成标注 PNG（复用 export 管线）→ `asset.put` → 原图+标注图两附件，指示放 `x+w+40` 原图右侧；③ 消息模板点名 `canvas`/`image_generate` 两个真实注册名——**散文里的工具名是第二份拷贝**（§4.11 round-12），单测钉拼写、`tests/canvas_wire.rs::every_tool_the_panel_canvas_templates_name_resolves_in_the_real_tool_table` 对真工具表求解。发送走 `session_dials_for_send` 共享规则（voice 路径的教训：传 `None` 的 send 点静默忽略会话的 agent/model/tier 选择）。
- **AI 框创建入口**：编辑器 `insert_ai_frame` 回调（视口中心铸 Draft 框，`ai::insert_frame_ops` + undo）——同样是「没有它整条流程只有模型够得到」的接线。

## 7. 安全边界

1. **iframe 沙箱**：`Html` 形状经 `<iframe sandbox="allow-scripts" srcdoc=…>` 渲染，**不给 `allow-same-origin`**——模型 HTML 跑在 opaque origin，够不到 Panel 的 RPC/storage/cookie。srcdoc 经 `canvas.asset.get` 拉文本并按 asset_id 缓存；iframe 指针事件默认穿透（`pointer-events:none`），选中后才 `auto`。**源码级 census**：`the_iframe_is_sandboxed_with_scripts_only_and_never_same_origin` + `no_iframe_exists_outside_the_censused_one`（shape_view.rs）。
2. **素材闸**（全在 store，两张脸共用）：字节上限按 mime 分档（`the_asset_byte_cap_is_keyed_by_mime`）；mime 白名单单表 `ASSET_MIME_TABLE`（png/jpeg/webp/gif/svg+xml/html，大小写/空白归一化）；asset_id 解析器＝穿越闸（恰 64 hex + 白名单 ext，`parse_asset_id` 只认 `put_asset` 铸得出的，`a_path_traversal_id_is_rejected_before_touching_disk`）。
3. **能力 URL**（第二 ingress，逐条重申 `/ws` 的守卫而非继承）：cap 由已过可见性闸的 `canvas.get` 铸（TTL 10min——画布可见性是活状态，roster 一改即撤，TTL 就是容忍的判决陈旧度；素材字节反正 content-addressed `max-age=3600` 浏览器缓存）；cap 是 bearer secret 所以是**路径段永不是查询参数**（`?cap=` 会活进 access log）；URL 里的 `canvas_id` 不被信任为 scope——服务端从 cap 解析画布，URL 段必须 MATCH，不匹配=plain not-found。
4. **text/plain 裁定**：`text/html` 素材经能力 URL 必须以 `Content-Type: text/plain` 返回（裁定不是回退）——HTML 只在 sandboxed iframe srcdoc 里渲染，直开能力 URL 不能变成同源 HTML 页（那页够得到 gateway origin 的 storage 与 RPC）。`html_asset_is_served_as_plain_text`。
5. **SVG CSP**：`image/svg+xml` 保留类型（`<image href>` 渲染不了 text/plain）但携 `ARTIFACT_DOCUMENT_CSP`（`default-src 'none'` + `sandbox`）——直开时强制 opaque origin 禁脚本，子资源（图片）用法不受影响（CSP 作用于作为 document 的响应）。`an_svg_asset_keeps_its_type_but_is_sandboxed`。
6. **错误纪律**：`CanvasError` 无 `From<String>`（`?` 能自动转换的那刻，下一个调用方错误默认变回 internal）；handler 自写 `INTERNAL_ERROR` 被源码守卫拒。
7. **键集相等**：响应从契约类型**构造**（超发=编译错误），`the_canvas_responses_carry_the_contract_and_nothing_else` 的期望集从契约类型序列化派生，非字面量清单。

## 8. 刻意不做清单（YAGNI 记录，防重提）

Spec §6 原批 + 实施中新增裁定：

- **CRDT / OT 实时协同**——乐观锁 + 广播已覆盖「人+模型交替写」的实际场景。
- **Cowart pages 子层**——库即页列表。
- **`save_view_state` 相机同步**——每设备 UI 偏好归 Panel 本地（多设备判据反向适用：这个值对第二台设备**不**成立）。
- **形状旋转、画布缩略图、HTML/SVG 对外导出（PNG 已覆盖标注流程）、phone 编辑、独立 CLI/TUI 客户端面**。
- **tldraw 嵌入**——商业许可（无 key 强制水印）+ 首个 JS/React 依赖 + 双状态源，已否决。
- **工具面 `delete` action**——见 §5。
- **`canvas.delete` 的 org-admin override**——见 §4。
- **选区 sidecar**——见 §1 代码地图。
- **deck `frame_ids` 的分数索引**——小列表整写。
- **与 `artifact_caps` 共表**——见 §1。
- **SVG `<marker>` 箭头**——计算 polygon 与导出共用同一数学，见 §6。
- **`reconcile` 用 `actor` 判回声**——同用户两标签页共享 actor，见 §3。
- **`/canvas/:id` 深链路由**——「我在看哪张画布」是每设备 UI 偏好（与 `save_view_state` 同一条判据），而侧栏常驻之后换画布是一次点击；加一条路由要给 `open_canvas` 找第二个真源。
- **服务端画布搜索**——库是几十行量级，客户端纯函数过滤够用，服务端一份就是「什么匹配」的第二个答案。
- **标题闸进工具 `DESCRIPTION`**——见 §2。
- **全面类型化 `rpc_call` 的错误**——冲突分支已按码（§3，2026-08-17），但把 150+ 处 `String` 消费者整体迁到 `RpcFailure` 不做：投影面（`message` 单点派生）已保证字节不漂，整体迁移的收益只剩类型美观，代价是 `admin_refusal` 分类器全链重扫。

## 9. QA 入口

- **单测**：core `cargo test -p alephcore --lib canvas`（store/assets/selection/validate + handlers + tool + caps + route）；Panel `cargo test -p aleph-panel --lib canvas`（纯函数全家 + 源码 census）。
- **集成**：`cargo test -p alephcore --features test-helpers --test canvas_wire`——三条：全链 wire 键集对拍（`the_wire_chain_round_trips_every_canvas_response_through_the_contract`）、AI 模板工具名对真工具表求解、事件可见性三角色端到端。
- **真机**：`./qa/canvas/run.sh`——隔离 `HOME`+`ALEPH_HOME`、mock provider（api_key 内联 config 不碰 vault，**request_log oracle 无条件接线**到 `$QA_ROOT/request_log.jsonl`）、boot-and-wait 形态（驱动手是浏览器 + chrome-devtools-mcp），打印十项清单（持久化/双标签页广播/冲突恢复/工具面+事件面/AI 图片框/标注重生成/Slides/member 可见性/PNG 导出/左栏画廊），**每条带效果断言**（1–9 于 2026-08-17 全部真机 PASS；第 10 项同日真机 PASS，断言全文见 README「Item 10」）；items 4–6 的 mock 驱动配方（`tool_spec` 固定工具调用 + 两阶段拿 id）在 `qa/canvas/README.md`。前置：`just wasm`（debug server 从磁盘读 dist，空 dist 每项都为错误的理由「失败」，脚本拒绝启动）。
- **真冲突窗仪器**：`qa/canvas/latency_proxy.py`——上行 +N ms、下行直通的 TCP 代理；MCP 串行时序在 loopback 上永远输不掉乐观锁竞速（<100ms 帧传播），代理把窗口造在真 wire 上，并在 `-32031` 拒绝帧过下行时打印 `CONFLICT FRAME SEEN` 作正向 oracle。配两侧页内绝对时钟编排（跨 tab 的 MCP 往返延迟不可控，实测一次 21s——定时自拖是唯一可靠编排）。下行直通同时钉住「in-flight batch 不被 send 之后到达的广播 rebase」。
- **标题闸 wire 探针**：`qa/canvas/title_gate_probe.py`——浏览器**够不到**控制字符那条臂：`<input type="text">` 的 DOM 值净化算法直接剥掉 CR/LF，所以人在重命名框里永远提交不出换行（真机第一轮正是这么"失败"的：它把画布改名成了 `onetwo`）。那条臂是为另外两个写者（`canvas` 工具 / 任意裸 JSON-RPC 客户端）存在的，探针在真 wire 上对两个写者各跑一遍，并钉住「被拒的批次连 revision 都不烧」。12 条断言，2026-08-17 全 PASS。
- **member 场景播种器**：`qa/canvas/member_seed.py`——loopback 一发建齐 member 用户/房间名册/私有+房间双画布/bootstrap ticket 并打印 LAN member URL；浏览器半边照 README item 8 断言（含 no-oracle 同形断言）。⚠️ loopback 出示 member ticket 仍回 `operator`（信任模型），member 半边必须从 LAN IP 连。
