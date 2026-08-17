# Panel 画布（Whiteboard Canvas）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Aleph Panel 新增全功能白板画布子系统（Cowart 全量对齐并超越）：无限画布编辑器 + 服务端持久化 + 乐观锁实时广播 + 模型工具面 + AI 图片框/HTML 框/Slides/标注重生成。

**Architecture:** 四层：`shared/protocol/src/canvas.rs`（wire 契约单一源）→ `src/canvas/`（core store：文件持久化 + 每画布锁 + revision 乐观并发 + 事件发布）→ `src/gateway/handlers/canvas.rs`（RPC 族）+ `GatewayEventFrame::CanvasUpdated`（可见性过滤广播）+ `src/builtin_tools/canvas.rs`（单工具多 action）→ Panel `views/canvas/`（SVG DOM 形状层 + CSS transform 视口 + HTML overlay，纯 Leptos 零 JS 依赖）。

**Tech Stack:** Rust workspace（tokio / serde / schemars / uuid v4 / tempfile）、Leptos 0.8 CSR WASM、Tailwind v4 主题 token、无新第三方依赖（分数索引与 freehand 轮廓算法手写移植）。

**Spec:** `docs/superpowers/specs/2026-08-16-panel-canvas-whiteboard-design.md`（已批准；本计划从它论证，执行者两份都读）

## Global Constraints

- **全程在 worktree 分支 `worktree-panel-canvas-whiteboard` 提交，严禁触碰 main**（用户执行协议）。
- 提交信息英文，格式 `<scope>: <description>`（如 `canvas: add protocol contract types`）。
- **不引入任何新第三方依赖**：id 用既有 `uuid` v4（画布 id 形如 `format!("cv-{}", Uuid::new_v4().simple())`，对齐 `p-` 前缀家族）；分数索引、freehand 轮廓手写（R3）。
- 时间戳字段一律毫秒并以 `_ms` 后缀命名（`created_at_ms: i64`）——`MessageRecord.timestamp` 单位歧义教训。
- 路径只经 `utils::paths` 新增的单一源 helper；守卫 `no_hand_rolled_aleph_home_outside_the_allowlist` 会按文件红。
- 文档写盘一律 `atomic_write_file` / 新增的 `atomic_write_bytes`；读-改-写在每画布锁的同一临界区（`MetaGuard` 模式：私有 write + guard 唯一入口）。
- 工具面 actor 一律 `crate::gateway::visibility::ambient_actor()`；RPC 面 `visible_owner_filter()`；事件面从帧载荷取归属交给同一个谓词——三面一个推导。
- 错误三分类（not-found / caller-fixable / internal），禁止 handler 自写 `INTERNAL_ERROR`（include_str! 源码守卫），禁止 `From<String>`。
- 源码级守卫先 `.replace('\r', "")` 再 split（CRLF 判据）；`reg(` 单独一行 + 名字字面量在下一行（census 扫描格式约束，rustfmt 不许收拢）。
- Panel：i18n en/zh 两份 locale 键必须同批加（build 会因缺键失败）；配色只用 `--color-*` 主题 token（galaxy 的硬编码 hex 是反例）；不许 phone 屏 `provide_context` 任何 wide 视图读的类型（`context_ownership` 守卫）；`spawn_local` 内 `.await` 之后只用 `try_get_untracked()`（`disposed_reads` 守卫）。
- 验证命令：core 改动跑 `cargo test -p alephcore --lib`；Panel 有任何改动跑 `cargo test -p aleph-panel --lib`（**不是 check**）；每阶段收尾跑最小五条验证集。
- 每个 Task 一次提交（或按 Step 标注多次），TDD：先写红测试再实现。

## File Structure（全景）

```
shared/protocol/src/canvas.rs                  新：白板契约（FracIndex/Doc/Shape/Op/Deck/DTO）
shared/protocol/src/json_canvas.rs             更名自 canvas_format.rs（Obsidian JSON Canvas）
src/json_canvas_io.rs                          更名自 canvas_io.rs
src/canvas/mod.rs                              新：CanvasStore 门面 + re-exports
src/canvas/store.rs                            新：文档 CRUD + apply + revision + 锁
src/canvas/doc_io.rs                           新：私有读写（MetaGuard 模式的模块边界）
src/canvas/assets.rs                           新：内容寻址素材 + 孤儿回收
src/canvas/selection.rs                        新：进程内选区表（OnceLock + 上限淘汰）
src/canvas/validate.rs                         新：ops/形状/上限校验
src/utils/atomic_write.rs                      改：新增 atomic_write_bytes
src/utils/paths.rs                             改：get_canvas_root()
src/gateway/visibility.rs                      改：canvas_visible_to + 两个 resolver 包装
src/gateway/handlers/canvas.rs                 新：RPC 族
src/gateway/handlers/canvas_error.rs           新：三分类咽喉 + 源码守卫
src/gateway/events/frame.rs                    改：CanvasUpdated 变体
src/gateway/event_visibility.rs                改：分类臂
src/gateway/server/canvas_asset_route.rs       新：素材能力 URL 字节路由（镜像 artifact_route.rs）
src/gateway/protocol.rs                        改：REVISION_CONFLICT 错误码
src/gateway/method_visibility.rs               改：canvas.* KeyChecked pin
src/gateway/lane.rs                            不改（get/list 后缀启发式已覆盖全部读 RPC）
src/builtin_tools/canvas.rs                    新：canvas 工具（单工具多 action）
src/executor/builtin_registry/…               改：12 处登记（见 Task 10 清单）
src/config/types/policies/session_mode.rs      改：CHAT_DEFER_FAMILIES += "canvas"
src/bin/aleph-server/commands/start/…          改：store 构建 + handler 注册 + BuiltinToolConfig
interfaces/webchat/src/platform/wide/views/memory/galaxy/   迁移自 views/canvas/
interfaces/webchat/src/memory_graph/           迁移自 canvas_engine/（共享半）
interfaces/webchat/src/api/canvas.rs           新：CanvasApi（协议类型直用）
interfaces/webchat/src/state/canvas.rs         新：CanvasState
interfaces/webchat/src/platform/wide/views/canvas/          新：白板编辑器（见 Task 12+）
interfaces/webchat/src/platform/phone/canvas/mod.rs         新：手机库列表 stub
interfaces/webchat/locales/{en,zh}.json        改：canvas.* 键
qa/canvas/                                     新：真机 QA 夹具与清单
docs/reference/CANVAS.md                       新：参考文档
docs/reference/FEATURE_LOCATOR.md              改：新 §
CLAUDE.md                                      改：子系统路由表一行
```

依赖方向：Task 编号即建议执行序；Phase 内部分任务可并行（见每个 Task 的 Interfaces 块）。

---

## Phase 0 — 熵减与命名让位

### Task 1: Panel 星系画布迁移（释放 canvas 命名）

**Files:**
- Move: `interfaces/webchat/src/platform/wide/views/canvas/` → `interfaces/webchat/src/platform/wide/views/memory/galaxy/`（21 文件整体平移）
- Move: `interfaces/webchat/src/canvas_engine/{fnv1a.rs,interaction.rs}` → `interfaces/webchat/src/platform/wide/views/memory/galaxy/`（仅星系消费）
- Move: `interfaces/webchat/src/canvas_engine/{adapter.rs,markdown_excerpt.rs,category_color.rs,mod.rs}` → `interfaces/webchat/src/memory_graph/`（api/ 与 memory 视图共享）
- Modify: `interfaces/webchat/src/platform/wide/views/mod.rs`（删 `pub mod canvas;`——canvas 名留给 Task 11 的新白板）
- Modify: `interfaces/webchat/src/platform/wide/views/memory/mod.rs`（加 `pub mod galaxy;`）
- Modify: `interfaces/webchat/src/lib.rs`（`pub mod memory_graph;`，删 `pub mod canvas_engine;`）
- Modify（import 重指向，逐个 grep）: `src/api/graph.rs`、`src/api/memory.rs`、`src/platform/wide/views/memory/cards.rs`、`src/platform/wide/views/memory/drawer.rs`、`src/platform/phone/memory/detail.rs`、`src/platform/wide/views/memory_hub/mod.rs`（挂载点 `use crate::views::canvas::CanvasView` → `use crate::views::memory::galaxy::GalaxyView`）、`src/platform/phone/memory/graph.rs`（同）

**Interfaces:**
- Consumes: 无（纯迁移）
- Produces: `crate::views::memory::galaxy::GalaxyView`（原 `CanvasView` 更名）；`crate::memory_graph::{adapter, markdown_excerpt, category_color}`。**`views::canvas` 与 `canvas_engine` 两个路径此后不存在**——Task 11/12 重建 `views/canvas/` 为白板。

- [ ] **Step 1: 执行 git mv 并更名组件**

```bash
cd interfaces/webchat/src
git mv platform/wide/views/canvas platform/wide/views/memory/galaxy
mkdir memory_graph
git mv canvas_engine/adapter.rs canvas_engine/markdown_excerpt.rs canvas_engine/category_color.rs memory_graph/
git mv canvas_engine/fnv1a.rs canvas_engine/interaction.rs platform/wide/views/memory/galaxy/
git rm canvas_engine/mod.rs
```

新建 `memory_graph/mod.rs`：

```rust
//! Memory-graph support shared by `api/{graph,memory}.rs`, the memory table
//! views, and the galaxy renderer. Split out of the old `canvas_engine/` when
//! the `canvas` name was handed to the whiteboard (see views/canvas/).
pub mod adapter;
pub mod category_color;
pub mod markdown_excerpt;
```

在 `views/memory/galaxy/mod.rs` 顶部补 `mod fnv1a; mod interaction;` 并把原 `pub fn CanvasView()` 更名为 `pub fn GalaxyView()`（保留内部 `GalaxyCanvasView` 不动）。

- [ ] **Step 2: 全量重指向 import 并编译**

```bash
cd interfaces/webchat
grep -rln "canvas_engine\|views::canvas\|views/canvas" src/ | xargs sed -i '' \
  -e 's/crate::canvas_engine::adapter/crate::memory_graph::adapter/g' \
  -e 's/crate::canvas_engine::markdown_excerpt/crate::memory_graph::markdown_excerpt/g' \
  -e 's/crate::canvas_engine::category_color/crate::memory_graph::category_color/g' \
  -e 's/crate::canvas_engine::fnv1a/super::fnv1a/g' \
  -e 's/crate::canvas_engine::interaction/super::interaction/g' \
  -e 's/crate::views::canvas::CanvasView/crate::views::memory::galaxy::GalaxyView/g'
cargo check -p aleph-panel
```

sed 后必须逐文件人工复核（galaxy 内部相对路径 `super::` 层级可能不同——`gl/` 子模块里是 `super::super::`）。修到 `cargo check -p aleph-panel` 零错误零警告。

- [ ] **Step 3: 跑 Panel 测试确认零回归**

Run: `cargo test -p aleph-panel --lib`
Expected: PASS（迁移前后测试数相同；`context_ownership` 与 `disposed_reads` 源码守卫扫的是新路径，天然跟随）

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "panel: move memory galaxy out of views/canvas, split canvas_engine into memory_graph"
```

### Task 2: core 侧 Obsidian canvas 命名让位

**Files:**
- Move: `shared/protocol/src/canvas_format.rs` → `shared/protocol/src/json_canvas.rs`
- Move: `src/canvas_io.rs` → `src/json_canvas_io.rs`
- Modify: `shared/protocol/src/lib.rs`（`pub mod json_canvas;`）、`src/lib.rs`（`pub mod json_canvas_io;`）
- Modify（消费者 import）: `src/teams/workflow_canvas.rs`、`src/builtin_tools/team/workflow_canvas.rs`、`src/tasks/cron/carryover.rs`、`src/workflow/store.rs`、`src/workflow/proposal.rs`、`src/builtin_tools/workflow_tool.rs`、`src/teams/templates/loader.rs`

**Interfaces:**
- Produces: `aleph_protocol::json_canvas`（Obsidian JSON Canvas 1.0 互换类型，原样）与 `alephcore::json_canvas_io`。`canvas` 名在 protocol 与 core 两侧从此专属白板。

- [ ] **Step 1: git mv + 全量 import 更名**

```bash
git mv shared/protocol/src/canvas_format.rs shared/protocol/src/json_canvas.rs
git mv src/canvas_io.rs src/json_canvas_io.rs
grep -rln "canvas_format\|canvas_io" src/ shared/ --include="*.rs" | xargs sed -i '' \
  -e 's/canvas_format/json_canvas/g' -e 's/canvas_io/json_canvas_io/g'
```

两个文件的模块 doc 第一行补一句：`//! (renamed from canvas_format/canvas_io when the whiteboard took the `canvas` name — see src/canvas/)`。

- [ ] **Step 2: 编译 + 测试**

Run: `cargo check -p aleph-protocol && cargo test -p alephcore --lib json_canvas`
Expected: PASS（`json_canvas` 自带 10 条 round-trip 测试全绿）

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "core: rename canvas_format/canvas_io to json_canvas{,_io}, freeing the canvas name"
```

---

## Phase 1 — 协议与核心域

### Task 3: `aleph_protocol::canvas` 契约模块

**Files:**
- Create: `shared/protocol/src/canvas.rs`
- Modify: `shared/protocol/src/lib.rs`（`pub mod canvas;` 按字母序插入）

**Interfaces:**
- Consumes: 无
- Produces（后续所有 Task 依赖的确切形状）:
  - `FracIndex`（`pub struct FracIndex(String)`，`between(lo: Option<&FracIndex>, hi: Option<&FracIndex>) -> FracIndex`）
  - `ShapeCommon { id: String, x: f64, y: f64, w: f64, h: f64, z: FracIndex, parent_id: Option<String> }`
  - `Shape`（tagged enum，见下）、`GeoForm`、`ShapeStyle`、`SizeKind`、`ArrowEnd`、`AiFrameStatus`
  - `Deck { id: String, title: String, frame_ids: Vec<String> }`
  - `CanvasDoc { id, title, owner_user_id: Option<String>, project_id: Option<String>, revision: u64, shapes: Vec<Shape>, decks: Vec<Deck>, created_at_ms: i64, updated_at_ms: i64 }`
  - `CanvasOp`（tagged enum：`UpsertShape{shape}` / `DeleteShape{id}` / `SetDocMeta{title}` / `UpsertDeck{deck}` / `DeleteDeck{id}`）
  - RPC DTO：`CanvasCreateParams { title, project_id }`、`CanvasRef { canvas_id }`、`CanvasApplyParams { canvas_id, base_revision, ops }`、`CanvasApplyResult { revision }`、`CanvasRow { id, title, revision, shape_count, project_id, updated_at_ms }`、`CanvasList { canvases }`、`CanvasEnvelope { canvas, selection: Vec<String>, asset_base: Option<String> }`、`AssetPutParams { canvas_id, mime_type, data }`、`AssetPutResult { asset_id }`、`AssetGetParams { canvas_id, asset_id }`、`AssetGetResult { mime_type, data }`、`SelectionSetParams { canvas_id, shape_ids }`
  - 事件载荷 `CanvasUpdated { canvas_id, revision, ops: Vec<CanvasOp>, actor: Option<String> }`（Panel 解析事件 data 用；服务端帧多发 owner/project 字段，serde 容忍）
  - 常量：`pub const TOPIC: &str = "canvas.updated";`、`MAX_ASSET_BYTES: usize = 10 * 1024 * 1024`、`MAX_SHAPES: usize = 5000`、`MAX_OPS_PER_APPLY: usize = 500`、`MAX_HTML_ASSET_BYTES: usize = 2 * 1024 * 1024`

- [ ] **Step 1: 写红测试（同文件 `mod tests`）**

关键测试（照 `workspace.rs` 的散文命名风格）：

```rust
#[test]
fn frac_index_between_is_strictly_ordered() {
    let a = FracIndex::first();
    let b = FracIndex::between(Some(&a), None);
    let c = FracIndex::between(Some(&a), Some(&b));
    assert!(a < c && c < b, "between() must land strictly inside the gap");
}

#[test]
fn frac_index_repeated_inserts_stay_bounded_and_ordered() {
    // 1000 次头部插入不退化成无序、长度亚线性增长
    let mut hi = FracIndex::first();
    let mut prev_len = 0usize;
    for _ in 0..1000 {
        let lo = FracIndex::between(None, Some(&hi));
        assert!(lo < hi);
        prev_len = prev_len.max(lo.as_str().len());
        hi = lo;
    }
    assert!(prev_len < 200, "index length must not explode: {prev_len}");
}

#[test]
fn a_shape_round_trips_with_type_tag_and_flattened_common() {
    let s = Shape::Note {
        common: ShapeCommon { id: "n1".into(), x: 1.0, y: 2.0, w: 200.0, h: 200.0,
                              z: FracIndex::first(), parent_id: None },
        style: ShapeStyle::default(), text: "hi".into(),
    };
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["type"], "note");
    assert_eq!(v["id"], "n1");            // flatten 把 common 摊平到顶层
    let back: Shape = serde_json::from_value(v).unwrap();
    assert_eq!(back, s);
}

#[test]
fn an_unknown_shape_type_fails_to_parse_rather_than_silently_dropping() {
    let v = serde_json::json!({"type":"hologram","id":"x","x":0,"y":0,"w":1,"h":1,"z":"a1"});
    assert!(serde_json::from_value::<Shape>(v).is_err());
}

#[test]
fn absent_optionals_are_omitted_rather_than_sent_as_null() {
    let p = CanvasCreateParams { title: None, project_id: None };
    assert_eq!(serde_json::to_value(&p).unwrap(), serde_json::json!({}));
}

#[test]
fn a_doc_without_decks_key_parses_as_empty_decks() {
    // 向前兼容：旧 doc.json 无 decks 键
    let v = serde_json::json!({"id":"cv-1","title":"t","revision":1,
        "shapes":[],"created_at_ms":0,"updated_at_ms":0});
    let d: CanvasDoc = serde_json::from_value(v).unwrap();
    assert!(d.decks.is_empty() && d.owner_user_id.is_none());
}
```

Run: `cargo test -p aleph-protocol canvas` → Expected: FAIL（模块不存在）

- [ ] **Step 2: 实现**

要点（完整照抄下列骨架，省略处按同型补全）：

```rust
//! Whiteboard canvas wire contract — the single shape shared by the gateway
//! handlers (which BUILD responses from these types), the Panel, and the
//! `canvas` builtin tool. NOT `json_canvas.rs` (Obsidian interchange).
//! Key-set-equality tests live server-side in src/gateway/handlers/canvas.rs.
use serde::{Deserialize, Serialize};

pub const TOPIC: &str = "canvas.updated";
pub const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_HTML_ASSET_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SHAPES: usize = 5000;
pub const MAX_OPS_PER_APPLY: usize = 500;

/// Lexicographic fractional index over `0-9A-Za-z`. `between(None, None)`
/// yields the midpoint "U". Digits append when the gap closes — length grows
/// O(inserts-at-same-gap), never rebalances (rebalance would rewrite sibling
/// rows, defeating the point).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FracIndex(String);

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

impl FracIndex {
    #[must_use] pub fn first() -> Self { Self("U".to_string()) }
    #[must_use] pub fn as_str(&self) -> &str { &self.0 }
    /// Midpoint strictly between `lo` and `hi` (either side open).
    #[must_use]
    pub fn between(lo: Option<&Self>, hi: Option<&Self>) -> Self {
        let a = lo.map_or("", |f| f.0.as_str()).as_bytes();
        let b = hi.map_or("", |f| f.0.as_str()).as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let da = a.get(i).map_or(0i32, |c| digit_of(*c));      // 下界缺位补 0
            let db = b.get(i).map_or(62i32, |c| digit_of(*c));     // 上界缺位补 62（超上限哨兵）
            if db - da > 1 {
                out.push(DIGITS[((da + db) / 2) as usize]);
                return Self(String::from_utf8(out).expect("ascii"));
            }
            out.push(DIGITS[da.max(0) as usize]);
            i += 1;
        }
    }
}
fn digit_of(c: u8) -> i32 { DIGITS.iter().position(|d| *d == c).map_or(0, |p| p as i32) }
```

注意 `between` 的循环不变量：`out` 已含公共前缀 + 下界位；`da==db` 时继续下钻；`db-da==1` 时押下界位继续。实现后**手动验证** `between(Some("U"), Some("V"))` 产出 `"UU"` 之类且 `"U" < "UU" < "V"`（ASCII 序上 `"U" < "UU"` 成立因为前缀更短）。上界缺位补 62 而不是 61，否则 `between(Some(z), None)` 到不了 `z` 之后。

形状与文档（`json_canvas.rs` 的 `tag + flatten` 先例，无自定义 deserializer 所以 flatten 安全）：

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapeCommon {
    pub id: String,
    pub x: f64, pub y: f64, pub w: f64, pub h: f64,
    pub z: FracIndex,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeoForm { Rect, Ellipse }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeKind { Small, #[default] Medium, Large }

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ShapeStyle {
    /// Named palette slot ("default","red","orange","yellow","green","blue","violet")
    /// — resolved to theme tokens Panel-side; never a hex literal on the wire.
    #[serde(default)] pub color: String,
    #[serde(default)] pub fill: bool,
    #[serde(default)] pub size: SizeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrowEnd {
    pub x: f64, pub y: f64,
    /// Bound shape id; when present x/y are the recomputed fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub bind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiFrameStatus { Draft, Pending, Done, Failed }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Geo    { #[serde(flatten)] common: ShapeCommon, form: GeoForm, #[serde(default)] style: ShapeStyle, #[serde(default)] text: String },
    Ink    { #[serde(flatten)] common: ShapeCommon, #[serde(default)] style: ShapeStyle, points: Vec<[f32; 3]> },
    Text   { #[serde(flatten)] common: ShapeCommon, #[serde(default)] style: ShapeStyle, text: String },
    Note   { #[serde(flatten)] common: ShapeCommon, #[serde(default)] style: ShapeStyle, #[serde(default)] text: String },
    Image  { #[serde(flatten)] common: ShapeCommon, asset_id: String, #[serde(default)] natural_w: f64, #[serde(default)] natural_h: f64 },
    Frame  { #[serde(flatten)] common: ShapeCommon, #[serde(default)] title: String, #[serde(default)] aspect_locked: bool },
    Html   { #[serde(flatten)] common: ShapeCommon, asset_id: String },
    Arrow  { #[serde(flatten)] common: ShapeCommon, start: ArrowEnd, end: ArrowEnd, #[serde(default)] style: ShapeStyle, #[serde(default)] label: String },
    AiImageFrame { #[serde(flatten)] common: ShapeCommon, prompt: String,
                   #[serde(default)] reference_asset_ids: Vec<String>, status: AiFrameStatus },
}

impl Shape {
    #[must_use] pub fn common(&self) -> &ShapeCommon { /* match 全变体返回 &common */ }
    #[must_use] pub fn id(&self) -> &str { &self.common().id }
    /// Asset ids this shape references (orphan-GC walks this).
    #[must_use] pub fn asset_ids(&self) -> Vec<&str> {
        match self {
            Self::Image { asset_id, .. } | Self::Html { asset_id, .. } => vec![asset_id],
            Self::AiImageFrame { reference_asset_ids, .. } =>
                reference_asset_ids.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    }
}
```

`Deck`、`CanvasDoc`、`CanvasOp`、全部 RPC DTO、`CanvasUpdated` 按 Interfaces 清单写全；serde 纪律照 `workspace.rs`：请求可选字段 `#[serde(default, skip_serializing_if = "Option::is_none")]`，响应集合字段**不加** default（`CanvasList.canvases` 缺键=错误），`CanvasDoc.decks`/`owner_user_id`/`project_id` **加** default（老文档前向兼容）。

- [ ] **Step 3: 测试转绿**

Run: `cargo test -p aleph-protocol canvas` → Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add shared/protocol && git commit -m "protocol: whiteboard canvas contract (doc model, ops, frac index, rpc dtos)"
```

### Task 4: utils 补件 — `atomic_write_bytes` 与 `get_canvas_root`

**Files:**
- Modify: `src/utils/atomic_write.rs`
- Modify: `src/utils/paths.rs`
- Test: 两文件各自 `mod tests`

**Interfaces:**
- Produces: `pub async fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<(), AlephError>`；`pub fn get_canvas_root() -> Result<PathBuf>`（= `get_data_dir()?.join("canvas")`，**会建目录**——它是写路径 helper，诊断不要调它）

- [ ] **Step 1: 红测试**

```rust
#[tokio::test]
async fn atomic_write_bytes_round_trips_binary() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.bin");
    let payload = [0u8, 159, 146, 150, 255];          // 非 UTF-8
    atomic_write_bytes(&p, &payload).await.unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), payload);
}
```

- [ ] **Step 2: 实现** —— 把现有 `atomic_write_file` 的 body 抽成字节版（temp 同目录 + 写 + `sync_all` + 权限位拷贝 + rename 全部保留），`atomic_write_file` 变为 `atomic_write_bytes(path, content.as_bytes()).await` 的一行转发。`get_canvas_root` 挨着 `get_workspaces_dir` 加，doc 注明"creates the directory — not for diagnostics"。

- [ ] **Step 3: 验证** — `cargo test -p alephcore --lib atomic_write && cargo test -p alephcore --lib paths` → PASS（含既有 `no_hand_rolled_aleph_home` 守卫仍绿）

- [ ] **Step 4: Commit** — `git commit -m "utils: atomic_write_bytes + canvas root path helper"`

### Task 5: `src/canvas/` 文档存储（锁 + revision + apply）

**Files:**
- Create: `src/canvas/mod.rs`、`src/canvas/store.rs`、`src/canvas/doc_io.rs`、`src/canvas/validate.rs`
- Modify: `src/lib.rs`（`pub mod canvas;` 按字母序）
- Test: `src/canvas/store.rs::tests`（用 `tempfile::tempdir` 做 root，**guard 不许提前 drop**——绑定到测试函数局部并活过全部断言）

**Interfaces:**
- Consumes: Task 3 类型、Task 4 helpers
- Produces:

```rust
pub enum CanvasError {
    NotFound(String),
    Invalid(String),
    Conflict { current_revision: u64 },
    Internal(String),
}   // 无 From<String>；Display 用 thiserror 或手写

pub struct CanvasStore { /* root: PathBuf, locks: DocLocks, event_bus: Option<Arc<GatewayEventBus>> */ }
impl CanvasStore {
    pub fn new(root: PathBuf) -> Self;
    #[must_use] pub fn with_event_bus(self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) -> Self;
    pub async fn create(&self, title: Option<String>, project_id: Option<String>,
                        owner_user_id: Option<String>) -> Result<CanvasDoc, CanvasError>;
    pub async fn get(&self, id: &str) -> Result<CanvasDoc, CanvasError>;
    pub async fn list(&self) -> Vec<CanvasRow>;                 // 坏文件点名 warn 跳过
    pub async fn apply(&self, id: &str, base_revision: u64, ops: Vec<CanvasOp>,
                       actor: Option<String>) -> Result<u64, CanvasError>;  // 新 revision
    pub async fn delete(&self, id: &str) -> Result<(), CanvasError>;
}
```

- [ ] **Step 1: 红测试（行为全覆盖）**

```rust
#[tokio::test]
async fn apply_with_stale_revision_returns_conflict_with_current() {
    let dir = tempfile::tempdir().unwrap();
    let store = CanvasStore::new(dir.path().to_path_buf());
    let doc = store.create(Some("t".into()), None, Some("u1".into())).await.unwrap();
    let op = upsert_note("n1");                                  // 测试 helper：造一个 Note UpsertShape
    let r1 = store.apply(&doc.id, doc.revision, vec![op.clone()], None).await.unwrap();
    let err = store.apply(&doc.id, doc.revision, vec![op], None).await.unwrap_err();
    assert!(matches!(err, CanvasError::Conflict { current_revision } if current_revision == r1));
}

#[tokio::test]
async fn concurrent_applies_serialize_one_wins_one_conflicts() {
    // 两个任务同 base_revision 并发 apply：恰好一个 Ok 一个 Conflict，且落盘 revision == Ok 那个
}

#[tokio::test]
async fn delete_shape_op_removes_it_and_upsert_replaces_in_place() { /* ops 语义 */ }

#[tokio::test]
async fn a_corrupt_doc_json_is_skipped_loudly_by_list_but_errors_on_get() {
    // 手写坏 JSON 进 <root>/cv-x/doc.json：list() 不含它且不 panic；get 返回 Internal（不是 NotFound——
    // "解析失败"和"没有"是两个答案）
}

#[tokio::test]
async fn shape_count_over_cap_is_rejected_as_invalid() { /* MAX_SHAPES */ }

#[tokio::test]
async fn create_persists_owner_and_project_scope() { /* 重开 store 读回 owner/project */ }
```

Run: `cargo test -p alephcore --lib canvas::` → FAIL

- [ ] **Step 2: 实现 doc_io.rs（MetaGuard 模式，照抄 session_store/file_backend/meta.rs 的结构）**

```rust
//! Read-modify-write for doc.json is one critical section BY CONSTRUCTION:
//! `write` is private; only a `DocGuard` (from `DocLocks::lock`, which takes
//! the per-canvas mutex THEN reads) can commit. Mirrors
//! gateway/session_store/file_backend/meta.rs — read that module doc first.
pub(super) struct DocLocks { slots: std::sync::Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>> }
pub(super) struct DocGuard { path: PathBuf, doc: Option<CanvasDoc>, _permit: tokio::sync::OwnedMutexGuard<()> }
impl DocLocks {
    pub(super) async fn lock(&self, id: &str, path: PathBuf) -> Result<DocGuard, CanvasError>;
}
impl DocGuard {
    pub(super) fn existing_mut(&mut self) -> Option<&mut CanvasDoc>;
    pub(super) fn insert(&mut self, doc: CanvasDoc) -> &mut CanvasDoc;
    pub(super) async fn commit(self) -> Result<CanvasDoc, CanvasError>;   // atomic_write_file(serde_json::to_string_pretty)
}
async fn read(path: &Path) -> Result<Option<CanvasDoc>, CanvasError>;      // 缺文件 Ok(None)；解析失败 Err(Internal)
async fn write(...)   // private — 父模块够不到
```

槽表带 `PRUNE_AT = 128` 清 dead Weak（同 meta.rs）。**创建路径同属读-改-写**：`create` 也走 `lock`（id 先铸好），guard `insert` + `commit`。

- [ ] **Step 3: 实现 store.rs + validate.rs**

`apply` 的骨架（事件发布在锁内、commit 后）：

```rust
pub async fn apply(&self, id: &str, base_revision: u64, ops: Vec<CanvasOp>, actor: Option<String>)
    -> Result<u64, CanvasError>
{
    validate::ops_shape(&ops)?;                       // 条数上限、id 字符集 [A-Za-z0-9_-]{1,64}、点数上限
    let mut guard = self.locks.lock(id, self.doc_path(id)).await?;
    let doc = guard.existing_mut().ok_or_else(|| CanvasError::NotFound(id.to_string()))?;
    if doc.revision != base_revision {
        return Err(CanvasError::Conflict { current_revision: doc.revision });
    }
    validate::apply_ops(doc, &ops)?;                  // 就地应用 + MAX_SHAPES 校验（失败即整批拒绝，guard drop 不写盘）
    doc.revision += 1;
    doc.updated_at_ms = now_ms();
    let new_revision = doc.revision;
    let committed = guard.commit().await?;            // 原子落盘
    self.emit_updated(&committed, new_revision, ops, actor);  // 锁内发布（guard 已 consume 但 per-canvas mutex 由 _permit 决定——
                                                      // 注意：commit(self) 会 drop _permit！改法见下
    Ok(new_revision)
}
```

⚠️ **顺序陷阱（判据 §5.22 roster：变更+发布同一临界区）**：`commit(self)` 消费 guard 会释放锁，发布挪到锁外就可能与下一次 apply 的发布乱序。解法：给 `DocGuard::commit` 改签名为 `commit(&mut self)`（写盘、留锁），store 在同一作用域里发布完事件再 drop guard。测试 `events_publish_in_revision_order_under_contention` 钉住：并发 20 次 apply，收 typed 订阅断言 revision 序列严格递增。

`emit_updated` 照 `agent_env/mod.rs::emit_change` 形状：`let Some(bus) = &self.event_bus else { return };` 构造 `GatewayEventFrame::CanvasUpdated{..}`（Task 8 先以 `#[allow(dead_code)]` 打桩、Task 9 接真帧——**不行，帧变体必须先存在**。调整：Task 5 的 `emit_updated` 留 `fn emit_updated(&self, ...)` 空实现 + `// wired in Task 9`，Task 9 填 body 并在同笔改动里加帧变体与分类臂，避免半接线状态）。

- [ ] **Step 4: 全绿 + Commit**

Run: `cargo test -p alephcore --lib canvas::` → PASS
`git commit -m "canvas: file-backed store with per-doc locks and revision-checked ops apply"`

### Task 6: 素材层 + 选区表

**Files:**
- Create: `src/canvas/assets.rs`、`src/canvas/selection.rs`
- Modify: `src/canvas/store.rs`（挂 assets API）
- Test: 同文件 tests

**Interfaces:**
- Produces:
  - `CanvasStore::put_asset(&self, id: &str, mime: &str, bytes: &[u8]) -> Result<String, CanvasError>`（sha256 内容寻址、`MAX_ASSET_BYTES`/`MAX_HTML_ASSET_BYTES` 字节闸、mime 白名单 `image/png image/jpeg image/webp image/gif image/svg+xml text/html`）
  - `CanvasStore::read_asset(&self, id: &str, asset_id: &str) -> Result<(String, Vec<u8>), CanvasError>`（asset_id 即 `<sha256>.<ext>`，校验字符集拒路径穿越）
  - `CanvasStore::sweep_orphan_assets(&self, id: &str) -> Result<usize, CanvasError>`（doc 引用之外**且 mtime 早于 1 小时**的删除——宽限期挡住 put→apply 竞速窗口；apply 成功后顺手调用）
  - `selection::set(canvas_id, shape_ids)` / `selection::get(canvas_id) -> Vec<String>`（`OnceLock<Mutex<HashMap>>`，`MAX_LIVE = 4096` 满则淘汰最旧，artifact_caps.rs 形状）

- [ ] **Step 1: 红测试** — `put_asset` 去重（同字节两次 put 同 id）、超限拒绝、mime 拒绝、`read_asset("../../etc/passwd")` 拒绝、GC 宽限期（新孤儿不删、老孤儿删、被引用的不删）。
- [ ] **Step 2: 实现**（sha256 用既有依赖——grep `sha2` 在 Cargo.toml 已有，hub 校验在用；ext 由 mime 映射表推导，**不接受调用方文件名**）。
- [ ] **Step 3: 绿 + Commit** — `git commit -m "canvas: content-addressed assets with orphan sweep, in-memory selection table"`

### Task 7: 可见性谓词三形态

**Files:**
- Modify: `src/gateway/visibility.rs`
- Test: 同文件 `mod tests`

**Interfaces:**
- Consumes: `projects::roster::is_member`、`owner_or_legacy`、`visible_owner_filter`、`ambient_actor`
- Produces:

```rust
/// Whiteboard visibility: the owner sees it; a project-linked canvas is
/// visible to every roster member. `actor == None` (cron/tests) is
/// unrestricted, same convention as the partition twins above.
#[must_use]
pub fn canvas_visible_to(owner_user_id: Option<&str>, project_id: Option<&str>, actor: Option<&str>) -> bool;
#[must_use] pub fn canvas_visible(owner_user_id: Option<&str>, project_id: Option<&str>) -> bool;          // RPC 面：actor = visible_owner_filter()
#[must_use] pub fn ambient_canvas_visible(owner_user_id: Option<&str>, project_id: Option<&str>) -> bool;  // 工具面：actor = ambient_actor()
```

- [ ] **Step 1: 红测试** — owner 可见 / roster 成员可见 / 外人不可见 / `actor=None` 放行 / **delete 不在此谓词内**（owner-only 判定留 handler 的 `require_owner` 型 gate，测试注释点名）。roster 测试拿 `projects::roster::TEST_GUARD` 再 `publish`。
- [ ] **Step 2: 实现**（`canvas_visible_to` 的 body 只许 delegate `roster::is_member` + `owner_or_legacy`，不复刻成员判断——`visibility.rs:71` 的裁定原文照办）。
- [ ] **Step 3: 绿 + Commit** — `git commit -m "gateway: canvas visibility predicate with rpc/tool resolver twins"`

---

## Phase 2 — Gateway RPC 与事件

### Task 8: RPC 族 + 错误咽喉 + boot 接线

**Files:**
- Create: `src/gateway/handlers/canvas.rs`、`src/gateway/handlers/canvas_error.rs`
- Modify: `src/gateway/handlers/mod.rs`（两个 `pub mod` + `HandlerRegistry::new` 里的 phase-1 默认注册——照 `projects.*` 用 `CanvasStore` 进程共享实例？**不**：canvas store 无 `shared()`，phase-1 注册 `service_unavailable` 占位，boot 时真接）
- Modify: `src/gateway/protocol.rs`（`pub const REVISION_CONFLICT: i32 = -32031;` 带 doc：实现定义区间、Panel 按码分支自动重拉）
- Modify: `src/gateway/method_visibility.rs`（`canvas.*` 全族 `KeyChecked` pin）
- Create: `src/bin/aleph-server/commands/start/builder/handlers/canvas.rs`（`register_canvas_handlers`）
- Modify: `src/bin/aleph-server/commands/start/mod.rs`（构建 `Arc<CanvasStore>`（root=`utils::paths::get_canvas_root()`）`.with_event_bus(bus)`，调注册函数；Arc 同时留给 Task 10 的 `BuiltinToolConfig`）
- Test: `handlers/canvas.rs::tests`（进程内直调 handler 函数）

**Interfaces:**
- Consumes: Task 3 DTO、Task 5/6 store、Task 7 谓词
- Produces（方法名 → handler，全部 `pub async fn handle_*(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse`）:
  - `canvas.create` / `canvas.list` / `canvas.get` / `canvas.apply` / `canvas.delete` / `canvas.asset.put` / `canvas.asset.get` / `canvas.selection.set`
  - **lane 检查**：读方法后缀恰是 `get`/`list` → Query 启发式已覆盖，`lane.rs` 零改动；测试 `every_canvas_read_lands_in_query_lane` 断言 `Lane::for_method("canvas.get"/"canvas.list"/"canvas.asset.get") == Query` 钉住（防未来改名漏 lane）。

- [ ] **Step 1: canvas_error.rs（先行，红测试）**

照 task_error.rs 形状：

```rust
pub fn respond(id: Option<Value>, context: &str, error: &CanvasError) -> JsonRpcResponse {
    let code = match error {
        CanvasError::NotFound(_) => RESOURCE_NOT_FOUND,
        CanvasError::Invalid(_) => INVALID_PARAMS,
        CanvasError::Conflict { .. } => REVISION_CONFLICT,
        CanvasError::Internal(_) => INTERNAL_ERROR,
    };
    // Conflict 的 message 必须带 current_revision 数字（Panel/模型都靠它重放）
    JsonRpcResponse::error(id, code, format!("{context}: {error}"))
}
```

tests：每变体→码断言 + include_str! 守卫 `no_canvas_handler_writes_an_internal_error_code_of_its_own`（`.replace('\r',"")` + split `#[cfg(test)]` 取生产前缀 + 断言含 `canvas_error::respond` 且不含 `INTERNAL_ERROR`，自保断言非空）。

- [ ] **Step 2: handlers 红测试**

关键用例（每个 handler 至少一正一负；owner/成员/外人三角色借 `CALLER_USER` task-local scope 包裹直调）：

```rust
#[tokio::test]
async fn get_is_refused_for_a_stranger_as_not_found() { /* 拒绝与不存在同形（no-oracle） */ }
#[tokio::test]
async fn apply_conflict_maps_to_revision_conflict_code() { /* code == REVISION_CONFLICT 且 message 含数字 */ }
#[tokio::test]
async fn delete_by_a_roster_member_who_is_not_owner_is_refused() { /* owner-only 动词 */ }
#[tokio::test]
async fn the_canvas_responses_carry_the_contract_and_nothing_else() {
    // 键集相等：emitted keys == serde round-trip(CanvasEnvelope/CanvasList/CanvasApplyResult) 的 keys，
    // 期望集从契约类型自身派生（workspace.rs:1020 同款，勿写字面量清单）
}
```

- [ ] **Step 3: 实现 handlers**

body 形状照 `projects.rs::handle_member_list`：`parse_params` → gate（`store.get` + `canvas_visible` → 不可见一律 `canvas_error::respond(NotFound)`，与不存在同形）→ 委托 store → 从**契约类型构造**响应。`canvas.get` 响应叠 `selection::get` 与 `asset_base`（Task 9 之前恒 `None`，Task 9 填）。`canvas.create` 的 owner 取 `crate::scope::ambient_owner()`（projects.rs:318 同款）。`canvas.selection.set` 校验可见性后写 `selection::set`，返回 `{}`。

- [ ] **Step 4: boot 接线**（`register_handler!` 宏 + `start/mod.rs` 调用 + 非 daemon 打印方法清单，projects 同款）
- [ ] **Step 5: 绿 + Commit**

Run: `cargo test -p alephcore --lib "handlers::canvas"` → PASS
`git commit -m "gateway: canvas.* rpc family with revision-conflict code and boot wiring"`

### Task 9: CanvasUpdated 事件帧 + 素材能力 URL 路由

**Files:**
- Modify: `src/gateway/events/frame.rs`（变体 + `topic_name()` 臂；**不加** `stream_method()` 臂）
- Modify: `src/gateway/event_visibility.rs`（`session_identity_of` 臂 + `every_frame_variant_is_classified` 的 `expected()` 臂）
- Modify: `src/canvas/store.rs`（填 Task 5 留空的 `emit_updated`）
- Create: `src/gateway/server/canvas_asset_route.rs`（镜像 `artifact_route.rs` + `security/artifact_caps.rs` 的 cap 表模式——**先读这两个文件再动笔**）
- Modify: `src/gateway/handlers/canvas.rs`（`canvas.get` 铸 cap 填 `asset_base`）
- Test: 各文件 tests

**Interfaces:**
- Produces:

```rust
// frame.rs
CanvasUpdated {
    canvas_id: String,
    revision: u64,
    ops: Vec<aleph_protocol::canvas::CanvasOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")] actor: Option<String>,
    // 归属自报（§4.8 地雷 H：解析句柄的安装条件不得比帧的生产条件窄——帧自带归属，不依赖任何索引播种）
    #[serde(default, skip_serializing_if = "Option::is_none")] owner_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] project_id: Option<String>,
},
// topic_name(): Self::CanvasUpdated { .. } => "canvas.updated"
```

分类臂：新 `SessionIdentity::ByCanvasScope { owner: Option<String>, project: Option<String> }`，`event_admits_for` 的臂 body **只许**：

```rust
SessionIdentity::ByCanvasScope { owner, project } =>
    crate::gateway::visibility::canvas_visible_to(owner.as_deref(), project.as_deref(), caller_user),
```

（同一个谓词的第三面——事件面不许自己 re-derive 成员判断。）默认臂是 `Global` 广播，忘写分类=向所有连接泄漏，`every_frame_variant_is_classified` 无通配 match 会编译期强制。

素材路由：`GET /canvas-asset/{cap}/{canvas_id}/{asset_id}`，cap 由 `canvas.get` 铸（TTL 10 分钟，绑 canvas_id，表复用 artifact_caps 的 OnceLock+上限+过期淘汰形状但**独立表**——两类 cap 不共表）；响应带 `Content-Type` 与 `Cache-Control: private, max-age=3600`（asset 内容寻址不可变）。**`text/html` 素材经此路由必须以 `Content-Type: text/plain` 返回**（HTML 渲染只发生在 Panel 的 sandboxed iframe srcdoc，能力 URL 直开不能变成同源 HTML 页——这是 XSS 面）。

- [ ] **Step 1: 红测试**
  - `canvas_updated_admits_owner_and_roster_member_and_refuses_stranger`（走 `event_admits_for` 全链）
  - `every_frame_variant_is_classified`（编译期强制，加臂即修）
  - `asset_route_refuses_expired_or_mismatched_cap`、`html_asset_is_served_as_plain_text`
  - store 侧 `events_publish_in_revision_order_under_contention`（Task 5 预埋的钉子此时转正）
- [ ] **Step 2: 实现 + 绿**
- [ ] **Step 3: Commit** — `git commit -m "gateway: canvas.updated frame with scope classification, capability-url asset route"`

---

## Phase 3 — 模型工具面

### Task 10: `canvas` 内置工具

**Files:**
- Create: `src/builtin_tools/canvas.rs`
- Modify（12 处登记清单，逐项打勾）:
  1. `src/builtin_tools/mod.rs` — `pub mod canvas;` + `pub use canvas::{CanvasToolArgs, CanvasTool};`
  2. `src/executor/builtin_registry/config.rs` — `pub canvas_store: Option<Arc<crate::canvas::CanvasStore>>,`
  3. `src/executor/builtin_registry/definitions.rs` — `BuiltinToolDefinition { name: "canvas", description: <…CanvasTool as crate::tools::AlephTool>::DESCRIPTION, requires_config: true }`（**指常量不写字面量**）
  4. 同文件 `create_tool_boxed` 加臂（config→canvas_store→Box::new）
  5. `src/executor/builtin_registry/groups.rs` — `"canvas"` 进 `content_gen` 类
  6. `src/executor/builtin_registry/builder/constructor/mod.rs` — 构造段 `let canvas_tool = config.canvas_store.as_ref().map(|s| CanvasTool::new(Arc::clone(s)));`
  7. `src/executor/builtin_registry/registry/struct_def.rs` — `pub(crate) canvas_tool: Option<…>,` + 构造末尾初始化
  8. `src/executor/builtin_registry/builder/optional_tools.rs` — 配置门控 `reg(` 块（**`reg(` 独占一行、名字在下一行**）
  9. `src/executor/builtin_registry/registry/tool_registry_impl.rs` — dispatch 臂
  10. `src/config/types/policies/session_mode.rs` — `CHAT_DEFER_FAMILIES` += `"canvas"`（work/code 可见、chat 不进；`_` 词界匹配连带覆盖未来 `canvas_export`）
  11. `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` — `BuiltinToolConfig` 字面量填 `canvas_store: canvas_store.clone(),`
  12. `definitions.rs` tests — 跑 `catalog_description_bytes_ratchet` / `registry_schema_bytes_ratchet` / `tools_without_an_unconditional_schema_are_pinned`，按打印实测值重钉常量并附**答完三问的日期化注记**（三问：运行时事实？强模型能否自推？别的工具已说过？）
- Test: `src/builtin_tools/canvas.rs::tests` + `registry_adapter.rs` 的两向断言（canvas **不上** `READ_ONLY_TOOLS`——多路复用读写，file_ops 同款裁定）

**Interfaces:**
- Consumes: `CanvasStore`（与 RPC 同一枚 Arc——事件总线所在的那枚，`workspace_manage` 的 doc 教训）、`ambient_canvas_visible`、`selection::get`
- Produces（args；action 用 enum 防模型造词）:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanvasToolAction { List, Create, Get, Apply, InsertImage, InsertHtml, ReadAsset }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanvasToolArgs {
    pub action: CanvasToolAction,
    #[serde(default)] pub canvas_id: Option<String>,
    /// get: "summary" (default) | "full"
    #[serde(default)] pub detail: Option<String>,
    #[serde(default)] pub title: Option<String>,
    #[serde(default)] pub project_id: Option<String>,
    #[serde(default)] pub ops: Option<Vec<aleph_protocol::canvas::CanvasOp>>,
    /// insert_image: data: URL | 本地文件路径 | https URL（generation 工具的三种产出形态）
    #[serde(default)] pub location: Option<String>,
    /// insert_html: 单文件 HTML 内联正文
    #[serde(default)] pub html: Option<String>,
    /// insert_*: 目标框——给了就按框 bbox 放置并删除该 AiImageFrame
    #[serde(default)] pub frame_id: Option<String>,
    #[serde(default)] pub x: Option<f64>, #[serde(default)] pub y: Option<f64>,
    #[serde(default)] pub w: Option<f64>, #[serde(default)] pub h: Option<f64>,
    #[serde(default)] pub asset_id: Option<String>,
}
```

行为要点：
- 每个 action 先 `ambient_canvas_visible` 闸（list 过滤）；`get(summary)` 返回 `{id,title,revision,selection,shapes:[{id,type,x,y,w,h,text_excerpt(≤80 chars),parent_id}],decks}`——不含 points/style，省 token；`full` 返回整 doc。
- `apply` 不要求模型带 `base_revision`：工具内部 get→apply，Conflict 重试一次，再冲突把 `Conflict{current_revision}` 压成紧凑错误交模型（A2）。
- `insert_image` 的 `location` 三形态：`data:` 直接解码；本地路径**只允许**位于 `get_data_dir()` 树内或 std::env::temp_dir() 树内（canonicalize 后 starts_with，两边同侧转换——§5.22 存储形态判据），拒绝一切其它根；`https://` 用 workspace 里既有的 HTTP client（grep `reqwest` 既有用法照抄其 builder），10s 超时 + `MAX_ASSET_BYTES` 流式截断 + 响应 Content-Type 必须 image/*。放置：frame_id 给出 → 读框 bbox → `UpsertShape(Image)` + `DeleteShape(frame)`；否则用 x/y/w/h 或默认 100,100,512,512。
- `insert_html`：正文过 `MAX_HTML_ASSET_BYTES` → put_asset(text/html) → Frame(16:9, aspect_locked) + Html 子形状（parent_id=frame），或 frame_id 模式替换。
- `read_asset`：text/html 返回正文字符串；image 返回 `_media` 单元素（`MediaItem` data URL）+ 提示字段。
- `DESCRIPTION` ≤1.2KB，只写 schema 说不出的运行时事实（工具↔Panel 实时联动、frame 替换语义、summary/full 取舍、ops 与 RPC `canvas.apply` 同构）。

- [ ] **Step 1: 红测试**（每 action 一正一负；`stranger_gets_not_found_shape`; `insert_image_rejects_paths_outside_data_and_temp_roots`; `apply_retries_once_on_conflict`）
- [ ] **Step 2: 实现（模板：workspace_manage 的文件/登记形状 + scratchpad 的 action enum/typed output）**
- [ ] **Step 3: 12 处登记逐项过 + 棘轮重钉**

Run: `cargo test -p alephcore --lib -- definitions groups session_mode registry_adapter builtin_tools::canvas`
Expected: PASS（含 census `every_registered_core_tool_is_accounted`、分组三测试、`chat_mode_defers_heavy_families`）

- [ ] **Step 4: Commit** — `git commit -m "tools: canvas builtin tool (list/create/get/apply/insert_image/insert_html/read_asset)"`

---

## Phase 4 — Panel 白板编辑器

> Panel 每个 Task 收尾都跑 `cargo test -p aleph-panel --lib`。逻辑模块（交互状态机、ops 求逆、frac 排序、视口数学）全部写成**无 DOM 纯函数**并单测；组件只做接线。

### Task 11: 导航 + API + 状态 + 画布库页 + phone stub + i18n

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs`（`PanelMode::Canvas` + `from_path("/canvas")` + `ModeSidebar` 臂 + 两个枚举测试）
- Modify: `interfaces/webchat/src/components/nav_menu.rs`（`ALL_MODES` `[PanelMode; 7]`→`[PanelMode; 8]`、`route_of`→`"/canvas"`、`label_of`→`t_string!(i18n, nav.canvas)`、`icon_of` 24×24 SVG）
- Modify: `interfaces/webchat/src/components/command_palette.rs`（新 `mk_nav("nav.canvas", …, &["canvas","whiteboard","画布","白板"], "/canvas")`；**把 memory 条目关键词里的 "canvas" 移除**）
- Modify: `interfaces/webchat/src/app.rs`（`MainContent` 新 keep-alive 容器：Phone→`PhoneCanvas`，否则→`CanvasView`）
- Create: `interfaces/webchat/src/api/canvas.rs`（`CanvasApi`，**只用 `aleph_protocol::canvas` 类型**，workspace.rs 模式：`encode` helper + 每方法 `state.rpc_call`；含源码守卫测试 `canvas_rpc_is_issued_from_api_canvas_alone`）
- Create: `interfaces/webchat/src/state/canvas.rs`（见 Produces）
- Create: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`（本任务先渲染库页 + 空编辑器占位）
- Create: `interfaces/webchat/src/platform/phone/canvas/mod.rs`（`PhoneCanvas`：PhoneShell + 列表只读 + "请在桌面端编辑"）
- Modify: `interfaces/webchat/src/platform/phone/more.rs`（••• 菜单行）+ `mode_sidebar.rs::under_more`
- Modify: `interfaces/webchat/locales/{en,zh}.json`（`nav.canvas` + `canvas.*` 全部本任务用到的键，两份同批）

**Interfaces:**
- Produces:

```rust
// state/canvas.rs — 全 RwSignal、Copy（provide_context 于 app.rs AppContent，句点注明类型名）
#[derive(Clone, Copy)]
pub struct CanvasState {
    pub open_canvas: RwSignal<Option<String>>,          // 当前打开的 canvas_id；None=库页
    pub rows: RwSignal<Vec<aleph_protocol::canvas::CanvasRow>>,
    pub doc: RwSignal<Option<aleph_protocol::canvas::CanvasDoc>>,
    pub asset_base: RwSignal<Option<String>>,
    pub selection: RwSignal<Vec<String>>,               // 本端选区（shape ids）
    pub tool: RwSignal<CanvasTool>,                     // enum Select/Pan/Draw/Geo(GeoForm)/Note/Text/Frame/Arrow
    pub camera: RwSignal<Camera>,                       // struct Camera { x: f64, y: f64, zoom: f64 }
    pub pending_conflict: RwSignal<bool>,
    pub load_error: RwSignal<Option<String>>,
}
// api/canvas.rs
impl CanvasApi {
    pub async fn list(state:&DashboardState) -> Result<Vec<CanvasRow>, String>;
    pub async fn create(state:&DashboardState, title:Option<String>, project_id:Option<String>) -> Result<CanvasDoc, String>;
    pub async fn get(state:&DashboardState, id:&str) -> Result<CanvasEnvelope, String>;
    pub async fn apply(state:&DashboardState, id:&str, base_revision:u64, ops:Vec<CanvasOp>) -> Result<u64, String>;   // Err 携带原始 message（含 conflict revision）
    pub async fn delete(state:&DashboardState, id:&str) -> Result<(), String>;
    pub async fn asset_put(state:&DashboardState, id:&str, mime:&str, data_base64:String) -> Result<String, String>;
    pub async fn selection_set(state:&DashboardState, id:&str, shape_ids:Vec<String>) -> Result<(), String>;
}
```

- [ ] **Step 1: 逻辑红测试**（`from_path` 分类、`under_more` 包含 Canvas、i18n 键存在性由 build 兜、`screen` 纯函数如有）
- [ ] **Step 2: 实现库页**：挂载 Effect 里 `subscribe_topic("canvas.updated")`（ledger 自动重连重放，**不动 `BASE_TOPICS`**）+ `subscribe_events` 收帧刷新；列表跟 workspaces.rs 同款"重连后刷新"Effect（gate `is_connected`）；行点击 → `open_canvas.set(Some(id))` + `CanvasApi::get` 填 doc；「新建画布」按钮；删除带确认。视觉全部主题 token（`bg-surface-raised text-text-primary border-border`）。
- [ ] **Step 3: 验证** — `cargo test -p aleph-panel --lib` PASS（`context_ownership` 对新 provide_context 的类型解析要求：调用处写 `provide_context(CanvasState::new());` 带类型名）；`cargo check -p aleph-panel` 零警告。
- [ ] **Step 4: Commit** — `git commit -m "panel: canvas section (nav, api, state, library page, phone stub)"`

### Task 12: 编辑器壳 — 视口 + SVG 只读渲染

**Files:**
- Create: `views/canvas/editor.rs`（组件：三层容器）、`views/canvas/viewport.rs`（纯数学 + wheel/pointer 接线）、`views/canvas/shape_view.rs`（每形状 SVG 渲染）
- Modify: `views/canvas/mod.rs`（open_canvas 非空时渲染 `<CanvasEditor/>`）

**Interfaces:**
- Consumes: `CanvasState`、Task 3 类型
- Produces:
  - `Camera { x, y, zoom }` + 纯函数 `screen_to_world(cam, sx, sy) -> (f64,f64)`、`world_to_screen`、`zoom_at(cam, cursor, factor) -> Camera`（光标为锚缩放）
  - `ShapeView` 组件：`Geo`（rect/ellipse + 内嵌文本）、`Text`、`Note`（圆角矩形+文本）、`Image`（`<image href={asset_base}/{asset_id}>`）、`Frame`（描边矩形+标题）、`Ink`（`<path d=…>` 由 points 生成）、`Arrow`（线段+箭头 marker+标签）、`Html`（此任务先占位框，Task 16 换 iframe）、`AiImageFrame`（虚线框+prompt 摘要+状态徽标）

- [ ] **Step 1: 纯函数红测试**（`zoom_at` 保持光标下世界点不动：`screen_to_world(zoom_at(c,p,f),p) == screen_to_world(c,p)` 浮点容差；`ink_path_d` 空点集/单点不 panic）
- [ ] **Step 2: 实现**：外层 `<div style="touch-action:none" on:wheel on:pointerdown…>`；wheel 归一化**逐字照抄** galaxy `pointer_input.rs` 的 `delta_mode` 三臂（LINE=16px/PAGE=400px）；Ctrl/⌘+wheel 或 trackpad pinch = 缩放，裸 wheel = 平移；空格或中键拖 = 平移。内层 `<svg class="absolute inset-0 w-full h-full">` + `<g transform=move||format!("translate({} {}) scale({})", …)>` + `<For each=shapes key=|s| s.id().to_owned()>`；HTML overlay `<div>` 同 transform（`transform-origin: 0 0`）。z 排序：`shapes.sorted_by(z)` 的 Memo。
- [ ] **Step 3: 验证 + Commit** — `cargo test -p aleph-panel --lib canvas` PASS；`git commit -m "panel: canvas editor shell with viewport math and svg shape rendering"`

### Task 13: 交互核心 — 选择/移动/缩放 + ops 乐观应用 + undo/redo + 冲突恢复

**Files:**
- Create: `views/canvas/interaction.rs`（**纯状态机**，零 DOM）、`views/canvas/ops.rs`（乐观应用/求逆/重放，纯函数）
- Modify: `views/canvas/editor.rs`（接线）

**Interfaces:**
- Produces（纯逻辑，全部单测）:

```rust
pub enum ToolMode { Select, Pan, Draw, Geo(GeoForm), Note, Text, Frame, Arrow }
pub enum Drag { None, Marquee{origin:(f64,f64)}, Move{start:(f64,f64), ids:Vec<String>},
                Resize{handle:Handle, origin_bbox:Bbox, ids:Vec<String>}, Drawing{points:Vec<[f32;3]>},
                ArrowDraft{start:(f64,f64)} }
pub struct InteractionState { pub drag: Drag, /* … */ }
impl InteractionState {
    pub fn pointer_down(&mut self, world:(f64,f64), hit:Option<&Shape>, tool:ToolMode, shift:bool) -> Vec<Effect>;
    pub fn pointer_move(&mut self, world:(f64,f64)) -> Vec<Effect>;
    pub fn pointer_up(&mut self, world:(f64,f64)) -> Vec<Effect>;   // Effect = SetSelection/EmitOps(Vec<CanvasOp>)/BeginTextEdit(id)/…
}
// ops.rs
pub fn invert(doc_before:&CanvasDoc, ops:&[CanvasOp]) -> Vec<CanvasOp>;   // upsert→前值 upsert / 无前值 delete；delete→upsert 前值
pub fn apply_local(doc:&mut CanvasDoc, ops:&[CanvasOp]);                  // 与服务端 validate::apply_ops 同语义（对拍测试钉住）
pub struct UndoStack { /* Vec<(redo:Vec<CanvasOp>, undo:Vec<CanvasOp>)>, cap 200 */ }
```

冲突恢复协议（Panel 侧）：`CanvasApi::apply` Err 且 message 含 conflict → `CanvasApi::get` 整拉 → 把**本地未确认 ops 队列**按序 `apply_local` 重放 → 重发。发送为串行单飞（一次一批在途，返程前的新 ops 进队列合并）。

- [ ] **Step 1: 红测试** — 状态机（点选/shift 加选/空点击清选/marquee 命中 bbox 相交/move 产出 upsert ops/resize 8 手柄各方向 bbox 数学/Esc 取消回滚）；`invert(apply(doc,ops)) 再 apply == 原 doc`（性质测试跑随机 op 序列）；undo/redo 往返；冲突重放（模拟 get 返回新 doc + 队列重放序）。
- [ ] **Step 2: 实现 + 接线**（键盘：Delete/Backspace 删、⌘D 复制、方向键 nudge 1px/shift 10px、⌘Z/⇧⌘Z；selection 变化 debounce 300ms → `selection_set`）
- [ ] **Step 3: 验证 + Commit** — `git commit -m "panel: canvas selection/move/resize with optimistic ops, undo, conflict replay"`

### Task 14: 创建工具 + 文本编辑 overlay

**Files:**
- Create: `views/canvas/text_edit.rs`（overlay `<textarea>`：双击 Text/Note/Geo 进入，blur/⌘Enter 提交 UpsertShape，Esc 放弃；世界坐标定位 = overlay 层已同 transform）
- Modify: `interaction.rs`（Geo/Note/Text/Frame 工具的拖拽建形：down 定原点、move 画预览、up 生成 shape + 切回 Select；shape id 铸造 `mint_shape_id()`）
- Create: `views/canvas/id_mint.rs`：

```rust
/// 128-bit random hex via js_sys::Math::random (panel has no uuid/rand dep).
/// Collision domain: shapes within one canvas — negligible.
pub fn mint_shape_id() -> String {
    let mut s = String::with_capacity(16);
    for _ in 0..4 {
        s.push_str(&format!("{:04x}", (js_sys::Math::random() * 65536.0) as u16));
    }
    s
}
```

- [ ] Steps: 红测试（建形 ops 的 bbox 归一化——反向拖拽 w/h 为正；文本提交产出 upsert；空文本新建即弃）→ 实现 → `cargo test -p aleph-panel --lib` → Commit `panel: canvas shape creation tools and text editing overlay`

### Task 15: 画笔 + 箭头

**Files:**
- Create: `views/canvas/freehand.rs`（**纯算法**：perfect-freehand 轮廓移植 ~150 行——输入 `&[[f32;3]]`（x,y,pressure）输出闭合轮廓点集；参数 size/thinning/smoothing/streamline 常量化）
- Modify: `interaction.rs`（Draw 工具：move 累点（min 距离 2px 抽稀）、up 归一化到 bbox 相对坐标生成 Ink shape；Arrow 工具：up 时对命中形状 `bind`，move 中端点吸附高亮）
- Modify: `shape_view.rs`（Ink 用 freehand 轮廓 fill path；Arrow 绑定端点跟随目标形状 bbox 中心-边缘交点，纯函数 `arrow_anchor(bbox, toward:(f64,f64)) -> (f64,f64)`）

- [ ] Steps: 红测试（freehand：两点输入产闭合非自交轮廓、零点/单点不 panic；`arrow_anchor` 四象限交点）→ 实现 → 验证 → Commit `panel: canvas freehand ink and bindable arrows`

### Task 16: 图片与 HTML 形状

**Files:**
- Modify: `views/canvas/editor.rs`（拖放/粘贴图片：`DragEvent`/paste → FileReader→base64（composer attachments.rs 同款）→ `asset_put` → 光标处 UpsertShape(Image)，natural size 由 `HtmlImageElement` onload 读）
- Modify: `views/canvas/shape_view.rs`（Image：`<image>` href = `{asset_base}/{canvas_id}/{asset_id}`；Html：overlay 层 `<iframe sandbox="allow-scripts" srcdoc=…>`——srcdoc 内容经 `CanvasApi` 新增 `asset_get` 拉文本（RPC base64→String），缓存于 `StoredValue<HashMap<String,String>>`；**iframe 指针事件默认穿透**（`pointer-events:none`），选中该形状后才 `auto`——否则画布拖拽会被 iframe 吃掉）
- Modify: `api/canvas.rs`（`asset_get`）

- [ ] Steps: 红测试（asset URL 拼接、srcdoc 缓存键、`sandbox` 属性字面量 census——一条源码级测试断言 iframe 行含 `sandbox="allow-scripts"` 且不含 `allow-same-origin`）→ 实现 → 验证 → Commit `panel: canvas image upload/render and sandboxed html frames`

### Task 17: 实时同步

**Files:**
- Modify: `views/canvas/mod.rs`（收 `canvas.updated`：`serde_json::from_value::<aleph_protocol::canvas::CanvasUpdated>(evt.data)`；`revision == local+1` → `apply_local(ops)`（**跳过 actor==自己已乐观应用的批次**：本端在途批次记 `(base_revision, ops_hash)`，回声按 revision 匹配丢弃）；跳号 → 整拉 `CanvasApi::get`）
- Modify: `state/canvas.rs`（`inflight: RwSignal<Option<InflightBatch>>`）

- [ ] Steps: 红测试（纯函数 `reconcile(local_rev, frame) -> Reconcile::{ApplyOps, Refetch, DropEcho}` 三臂）→ 实现 → 验证 → Commit `panel: canvas realtime ops broadcast reconciliation`
  - **真机双标签页验证留 QA 阶段**（Task 20 的清单项，含效果断言：A 标签画一笔，B 标签 1s 内出现同笔）。

### Task 18: AI 三流程 + 光栅化导出 + 标注

**Files:**
- Create: `views/canvas/ai.rs`（AiImageFrame 的 overlay UI：prompt 输入、参考图选择（画布上已选 Image 形状注入 reference_asset_ids）、「生成」按钮；模板函数——**模型面英文**：）

```rust
pub fn generation_message(canvas_id:&str, frame:&Shape) -> String {
    // "[canvas] Generate an image for frame {fid} on canvas {cid}.\nPrompt: {prompt}\n
    //  Reference images are attached. Steps: 1) call canvas(action='get', canvas_id, detail='summary')
    //  2) generate with image_generate 3) call canvas(action='insert_image', canvas_id, frame_id, location=<image_location>)."
}
```

- Modify: `views/canvas/mod.rs`（发送：`ChatApi::send` 携该消息 + 参考图 assets 转 `PendingAttachment`（asset_get→base64）投当前会话；发送同时 ops 置 frame status=Pending）
- Create: `views/canvas/export.rs`（光栅化：选中形状集 → 生成独立 `<svg>` 字符串（内嵌 Image 为 data URL，经 asset_get）→ `Blob`→`HtmlImageElement`→`CanvasRenderingContext2d::draw_image`→`to_data_url("image/png")`；导出 PNG 下载复用 transcript.rs 的 blob 下载 idiom）
- 标注流程按钮（选中 Image + 覆盖其上的标注形状时出现）：composite PNG → `asset_put` → `ChatApi::send`（"[canvas] Regenerate this image following the annotations…" + 原图与标注图两个附件 + 指示 `insert_image` 放原图右侧 x+w+40）

- [ ] Steps: 红测试（消息模板含三步指示与确切工具名 `canvas`/`image_generate`——**源码级守卫**：模板里反引号点名的工具名逐个对 `BUILTIN_TOOL_DEFINITIONS`+registry 名单求解（§4.11 round-12 判据，防散文里的工具名成为第二份拷贝）→ 该守卫写在 alephcore 侧还是 panel 侧？panel 依赖不了 alephcore——**写成 qa/canvas 的服务端集成测试**，Task 20）；`svg_export_embeds_images_as_data_urls` 纯字符串断言 → 实现 → 验证 → Commit `panel: canvas ai frames, rasterizing export, annotation regen flow`

### Task 19: Decks（Slides）+ 播放

**Files:**
- Create: `views/canvas/decks.rs`（右侧抽屉：deck 列表、选中 Frame 后「组成 Slides」（UpsertDeck ops）、排序拖拽（frac 无关——deck.frame_ids 就是序）、播放按钮）
- Create: `views/canvas/present.rs`（全屏 overlay：当前帧内容按 Frame bbox 裁剪缩放铺满（camera 数学复用 `fit`）；←/→/点击翻页、Esc 退出；进度点）

- [ ] Steps: 红测试（`present_camera_for_frame(frame_bbox, viewport) -> Camera` 纯数学；deck ops 生成）→ 实现 → 验证 → Commit `panel: canvas decks and fullscreen presentation`

---

## Phase 5 — QA 与文档

### Task 20: 服务端集成测试 + 真机 QA 夹具

**Files:**
- Create: `tests/canvas_wire.rs`（alephcore 集成测试，`--features test-helpers`）：
  - 全链 wire 对拍：直调 handler 产出的 JSON 被 `aleph_protocol::canvas` 契约类型解析且键集相等
  - **AI 消息模板工具名求解守卫**：读 Panel 源文件 `views/canvas/ai.rs`（`include_str!` 相对路径不可达——改为 `std::fs::read_to_string("interfaces/webchat/src/…")`，仓库根相对，CI cwd 即 workspace 根；找不到文件即 fail）抽反引号内工具名，逐个断言存在于 `BUILTIN_TOOL_DEFINITIONS` ∪ registry 名单
  - 事件可见性三角色端到端（owner/成员/外人各开一条 typed 订阅）
- Create: `qa/canvas/README.md` + `qa/canvas/run.sh`（起 server（mock provider 配方：api_key 内联 config 不碰 vault、`ALEPH_HOME` 与 `HOME` 双隔离）+ 打印手动清单）。真机清单（chrome-devtools-mcp 执行，**每条带效果断言**）:
  1. 建画布→画矩形/便签/画笔→刷新页面→内容还在（持久化）
  2. 双标签页：A 画一笔 B 实时出现；B 移动形状 A 实时跟随（广播）
  3. A/B 同时拖同一形状→一端收冲突→自动重拉不丢另一端改动（乐观锁）
  4. 对话里让模型 `canvas(action='create')` + `insert_html`→Panel 实时弹出新画布内容（工具面+事件面）
  5. AI 图片框全流程（mock provider 返回固定 data URL 图）→框被图替换
  6. 标注重生成：标注→提交→模型插新图于原图旁
  7. Slides：三帧组 deck→播放→翻页→Esc
  8. member 角色（0.0.0.0 + 自签 TLS + 局域网 IP，配方见 memory）看不到 operator 的私有画布；房间画布双方可见可编辑
  9. PNG 导出落文件且可打开

- [ ] Steps: 写集成测试（红→绿）→ 写 qa 夹具 → 跑最小五条验证集 + `cargo test -p aleph-panel --lib` + `cargo test -p aleph-tui -p aleph-cli`（wire 契约触碰 protocol crate，两客户端 crate 必须同批验证——§10 判据）→ Commit `canvas: wire integration tests and real-machine qa fixtures`

### Task 21: 文档收尾

**Files:**
- Create: `docs/reference/CANVAS.md`（子系统参考：架构四层图、数据模型、并发协议、可见性、工具面、安全边界（iframe sandbox/asset 闸/能力 URL text/plain）、刻意不做清单（从 spec §6 迁移并补实施中新增的裁定）、QA 入口）
- Modify: `docs/reference/FEATURE_LOCATOR.md`（新 § 按现有编号规则续；逐 Task 落点与守卫清单）
- Modify: `CLAUDE.md`（①子系统路由表加行：`src/canvas/` `interfaces/webchat/src/platform/wide/views/canvas/` → CANVAS.md + qa/canvas；②文档索引表加 CANVAS.md 一行；**不在 Tier-1 写任何细节**）
- Modify: `docs/superpowers/specs/2026-08-16-panel-canvas-whiteboard-design.md`（头部补一行实施状态与偏差记录——若实施中有与 spec 的偏差，逐条列明何以偏离）

- [ ] Steps: 写三份 → 自查（每个"已交付"声明能指出代码锚点与测试名）→ Commit `docs: canvas subsystem reference, feature locator entry, routing table row`

---

## Self-Review 记录（计划作者已跑）

1. **Spec 覆盖**：spec §1 架构/命名→T1/T2/T11；§2 数据模型/持久化/可见性→T3-T7；§3 RPC/事件/工具→T8-T10；§4 渲染/交互/AI 三流程/Slides/导航→T12-T19、phone→T11；§5 安全（iframe/素材/档位/三面同谓词）→T6/T9/T10/T16、错误处理→T5/T8/T13、测试→各 Task+T20、真机 QA→T20、文档→T21；§6 刻意不做→T21 迁移。无缺口。
2. **占位符扫描**：无 TBD/TODO；"同型补全"仅指 serde 派生重复体，其形状在同 Task 内已有完整示例。
3. **类型一致性**：`FracIndex`/`ShapeCommon`/`CanvasOp`/`CanvasError::Conflict{current_revision}`/`CanvasApi` 签名在 T3 定义后各 Task 引用一致；`emit_updated` 的半接线状态在 T5/T9 有显式交接说明；工具 args 用 `CanvasToolArgs`（T10）与 Panel `CanvasState`（T11）无名字冲突。
4. **计划内裁定与 spec 的三处偏差**（已在任务中标注理由）：① `canvas_format`/`canvas_io` 由"CUT 候选"改为**更名让位**（验证发现均有活消费者）；② 素材展示从"RPC base64"升级为**能力 URL 路由**（repo 既有 artifacts 先例，浏览器缓存 + 不过 WS）；③ 冲突错误从"复用 INVALID_PARAMS"改为**新码 REVISION_CONFLICT**（Panel 按码分支自动恢复，消费者与码同批出生）。三处均写进 T21 的 spec 偏差记录。
