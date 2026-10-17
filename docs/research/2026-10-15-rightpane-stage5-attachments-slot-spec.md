# 阶段 5 Spec — 附件 Chip 升级 + LRU + 全局 Tray(右栏)

> **任务边界**:本文件是**探索 + 方案**,**不**写实际重构代码。落地属于阶段 5 实施,待用户独立批准。
>
> **系列位置**:[阶段 0 总览(2026-10-15-rightside-optimization-plan.md)](2026-10-15-rightside-optimization-plan.md) · [阶段 1 Slot 接口(2026-10-15-rightpane-stage1-slot-spec.md)](2026-10-15-rightpane-stage1-slot-spec.md) · [阶段 2 宿主切换(2026-10-15-rightpane-stage2-host-switching-spec.md)](2026-10-15-rightpane-stage2-host-switching-spec.md) · [阶段 3 子代理 slot(2026-10-15-rightpane-stage3-subagent-slot-spec.md)](2026-10-15-rightpane-stage3-subagent-slot-spec.md) · [阶段 4 能力 slot(2026-10-15-rightpane-stage4-capabilities-slot-spec.md)](2026-10-15-rightpane-stage4-capabilities-slot-spec.md) · **阶段 5 本文件**

---

## §0 TL;DR

- **草稿附件 chip 当前密度低**:`PendingAttachment` 4 字段(name / mime_type / data_base64 / size),`AttachmentPreviewBar` 只渲染 **name + size + ✕**;**无** type-icon(仅一个通用文件 SVG)、**无** 缩略图、**无** 来源 badge、**无** 错误状态(`read_file_list_into` 失败被 silent 吞掉,见 §1.1)。
- **落地层(artifacts pane)信息密度更高**:`ArtifactRow` 图片已缩略图,文本/二进制有 origin badge + size;`preview.rs` 已支持 text/markdown 内联渲染。但**草稿层与落地层完全断开** — 同一份图片,拖入时 chip 看不清,落地时却能缩略图。
- **"tray" / "global file list" 概念当前为 0**:`grep -rn 'tray\|cross.conversation\|pinned.attachment\|file.history' interfaces/webchat/src` 命中点全部为不相关(桌面托盘、notification center、palette 等);本 spec 是**新建**该概念。
- **本阶段核心动作**:
  1. **Chip 升级**:type-icon 映射 + image 缩略图(lazy) + 错误状态(失败可见) + 来源(mime 缩略字符)
  2. **LRU 策略**:草稿层 `MAX=10` 维持,但增加单条 size 限额显示;artifacts 层走 time-based + count-based 双约束
  3. **全局 tray slot**:新增 `rps.tray` slot,跨 conversation 累积 user-pinned 文件;`/dashboard/tray` 保留为全屏 deep-link(若启用)
- **3 个待拍板决策** S5-A / S5-B / S5-C,见 §3.1。

---

## §1 Aleph 现状审计(已读 5 个 composer/attachments + artifacts/ 文件)

### §1.1 草稿层 — `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs`

文件 245 行,4 个核心导出:

| 导出 | 行 | 角色 |
|---|---|---|
| `format_size(bytes: u64) -> String` | 17-26 | B/KB/MB 格式化,纯函数可测 |
| `MAX_ATTACHMENT_SIZE_BYTES: u64 = 10 MB` | 35-37 | 单条上限(Anthropic/OpenAI 图像 5MB / 文档 10MB 对齐) |
| `MAX_ATTACHMENT_COUNT: usize = 10` | 43 | 单次发送上限(base64 33% 膨胀不破网关预算) |
| `attachment_key(att) -> String` | 53-62 | DefaultHasher(name+size),stable across insert/remove/reorder |
| `read_file_list_into(FileList, RwSignal<Vec<PendingAttachment>>)` | 76-141 | FileReader → data-URL → base64 → push;失败 silent |
| `AttachmentPreviewBar(RwSignal<...>)` | 144-194 | 横向 chip 条,name + size + ✕ |

**关键事实**:
- **数据真源 = 浏览器 `FileReader` → data-URL → base64**(`attachments.rs:100-141`),base64 字符串内存中持有**整个文件**(10 MB 文件 ≈ 13 MB base64)
- **失败 silent**:`web_sys::FileReader::new()` 失败 / `result()` 失败 / base64 解析失败,**全部无 UI 反馈**(`attachments.rs:80, 100-105, 110-128`),只在控制台 warn
- **草稿不持久**:清空 textarea 不清 chip(明确注释 `attachments.rs:142-143`),但**切会话 / 刷新页面 → 草稿清零**(`PendingAttachment` 无 `localStorage` 序列化路径)
- **chip 不带 mime 信息**:`format_size(file.size)` 出现,但 `file.mime_type` **从不渲染** — 唯一 SVG 是通用文件图标
- **attachment_key 是稳定 hash**(`DefaultHasher`(name+size)):`<For>` 不会因为中间删除而错位(`attachments.rs:53-62` 注释明示)

### §1.2 `PendingAttachment` 结构(`chat/state/mod.rs:21-27`)

```rust
pub struct PendingAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,  // 整文件 base64 持有
    pub size: u64,
}
```

`Serialize + Deserialize` 已实现,但**无任何代码反序列化**(grep 0 处 `deserialize` 命中),即"持久化通道**已铺线**,**没人接**"。

### §1.3 落地层 — `interfaces/webchat/src/components/artifacts/`

| 文件 | 行 | 角色 |
|---|---|---|
| `mod.rs` | 575 | 4 来源(deliverables / inbound / outbound / workspace files),`ArtifactFilter` All/Images/Files,`origin_badge_class`,`ArtifactsSurface` 主组件,`first_unseen_deliverable`,`partition_deliverables` |
| `files.rs` | 192 | `WorkspaceFiles` — 项目树,折叠在 artifacts 底部,组件级 state(无 `WorkspaceState` 共享) |
| `row.rs` | 100 | `ArtifactRow` — image 缩略图 + name + size + origin badge;text/binary 走外链 |
| `preview.rs` | 342 | `PreviewTarget::Image` / `Text` / `for_item()`,`preview::Preview` 覆盖 pane 而非 window,markdown 走 `MarkdownRenderer`,其它 `<pre>` |
| `deliverable.rs` | 168 | `DeliverableCard` — 独立卡片(Open 按钮 + auto-open on arrival) |

**关键事实**:
- **Image 缩略图已实现**:`row.rs:60-65` `<img src=url loading="lazy" class="w-9 h-9 rounded object-cover">` — 但**只对落地后的 image**,**草稿层无**
- **Origin 三类**:`ORIGIN_INBOUND`(用户给) / `ORIGIN_EXPORT`(导出) / `ORIGIN_OUTBOUND`(agent 产出),`origin_badge_class` 给颜色 badge(`mod.rs:142-151`)
- **Deliverable auto-open**:新 deliverable 自动 `<a target="_blank">` 打开(`mod.rs:215-230`),被弹窗拦截时降级为 pane 内 banner + `set_layout(Split)`
- **`ArtifactItem` 字段**:`id, filename, mime_type, size, origin, run_id, created_at, url` — **比 `PendingAttachment` 多了 id / origin / run_id / created_at / url**
- **text/markdown 区分**:`is_text()` / `is_markdown()` 谓词(`preview.rs:60-79`),text 走 `artifacts.read_text` RPC,markdown 走 `MarkdownRenderer`
- **无 size 上限显示**:`ArtifactItem.size` 是 `u64`,渲染走 `human_size()`,**但 list 排序不按 size**

### §1.4 跨端(草稿) / 跨会话(全局)概念

- **跨端**(同一会话跨 reload):**不支持** — `PendingAttachment` 内存持有,刷新即丢
- **跨会话**(同一 user 跨 conversation):**不支持** — `ChatState` 持有 `pending_attachments`,切会话被替换
- **跨会话的 artifacts**:服务端保留(`/artifact/c/{id}/f` URL 永久),但**Panel 端无 cross-conversation 视图** — 每次进会话都重读

### §1.5 TUI 端(`interfaces/tui/src/tui/widgets/`)

- `input_area.rs` — 输入框,无 attachment 概念
- `btw_panel.rs` — "by the way" 上下文提示面板(非附件)
- `tasks_panel.rs` — 任务列表(非附件)
- **TUI 端无 attachment chip**:`grep -rln "attachment\|chip"` 在 TUI 端命中 0 处真使用(只有 `keys.rs` 键位描述、`app/events.rs` 事件名等不相关)

**结论**:TUI 端附件能力**完全空白**。本阶段**TUI 端 spec 只做接口预留**,不实施,符合"阶段 7 独立审批"约定。

### §1.6 痛点(为什么进右栏 + 升级)

1. **信息密度不均**:落地层有 type-icon / 缩略图 / origin badge,草稿层**只有** name + size;同一份图片,落地后清晰,草稿时模糊
2. **失败无感**:FileReader 错误被吞,用户不知道"我拖的 9.5MB 图为什么没出现"
3. **跨会话无记忆**:换会话后,**草稿清零**,artifacts 列表**重读**;用户无法"在会话 A 拖入,在会话 B 继续"
4. **tray 概念缺位**:用户期望"我最近给的所有文件"统一入口,当前散落在草稿 / workspace / artifacts 三处
5. **LRU 无策略**:`MAX=10` 是 count 上限,无 time-based / size-based 驱逐;artifacts 端**完全无限**

---

## §2 Chip 升级 — 三个具体方向(自写分析,非复制)

### §2.1 type-icon 映射(mime → 缩略字符/图标)

**当前**:唯一 SVG 是通用文件图标(`attachments.rs:151-160`),无法区分 image / text / code / binary

**升级方向**(基于现有 `mod.rs:142-151` `origin_badge_class` 同款 match 风格):

| Mime 模式 | 图标字符 | 颜色 |
|---|---|---|
| `image/*` | `🖼` (或 SVG) | text-info |
| `text/*` (含 `markdown`, `json`, `xml`, `csv`) | `📄` | text-secondary |
| `application/pdf` | `📕` | danger |
| `application/json` / `+json` | `{ }` | warning |
| `text/*` 含 `javascript`/`typescript` / `python`/`rust` 等 code MIME | `</>` | primary |
| `application/zip` / `application/x-tar` / `+gzip` | `🗜` | text-tertiary |
| `application/octet-stream` + 已知扩展名(`.bin`/`.dat`) | `⊘` | danger |
| 其它 | `📎`(当前通用图标) | text-tertiary |

**约束**:
- **icon 即字符**优先,SVG 备选(避免 10+ 内联 SVG 增加 bundle 体积)
- **fallback 是 mime 前缀匹配 → 扩展名 → 默认** 三段回退,失败才走通用图标
- 与 `artifacts/row.rs:60-65` 既有 image 缩略图**风格不冲突**:row 已经 `<img>`,草稿层不重复

### §2.2 image 缩略图(lazy 生成)

**当前**:草稿层 chip **无** 缩略图(`attachments.rs:144-194`)

**升级方向**:
- **Lazy 触发**:`PendingAttachment` 新增 `thumbnail_state: ThumbnailState`(Pending / Ready(data_url) / Failed(reason)),chip mount 时 Effect::new 触发生成
- **生成路径**:`mime_type.starts_with("image/")` + `size < 2 MB` → 用 `web_sys::create_image_bitmap_from_buffer` 或 `OffscreenCanvas` 缩放到 36×36,编码 data-URL
- **2 MB 阈值**:草稿层 chip 默认 36×36 PNG ≤ 5 KB;`> 2 MB` 图片不解码(避免阻塞 UI 线程),chip 走 icon + "Image" label
- **缓存**:每条 `PendingAttachment` 自己的 `thumbnail_state`,`attachments` 重排不重算(已绑定 att 对象)
- **失败兜底**:`image/*` 但 `OffscreenCanvas` 失败 → 走 icon 路径,`thumbnail_state = Failed(decoding)`,hover 显示原因
- **零引入**:**不**引新依赖,纯 `web_sys` + `OffscreenCanvas` API(浏览器原生,R10 红线守住)

### §2.3 错误状态 + 来源 badge

**当前**:失败 silent,无来源区分(草稿都是用户上传 = inbound,但无显式标记)

**升级方向**:
- **错误态**:`PendingAttachment` 增 `error: Option<String>`(FileReader 错误 / size 超限 / mime 拒绝);chip 红边 + tooltip,`✕` 改 `↻`(retry) on click
- **来源**:草稿层**只**有 inbound,无需 badge;但若未来加"从 artifacts pin 到草稿"路径,需走 `origin_badge_class` 同款 badge
- **quota 提示**:`MAX_ATTACHMENT_COUNT = 10` 已满时,新增 chip 走 disabled 样式(灰)+ tooltip "已达上限 10"

### §2.4 chip 升级总览

| 字段 | 当前 | 升级后 |
|---|---|---|
| 图标 | 通用文件 SVG | mime 映射字符(`🖼📄📕{ }` 等) |
| Image 缩略图 | 无 | lazy 36×36 data-URL,失败兜底 icon |
| Name | max-w-120px truncate | 同 |
| Size | format_size 渲染 | 同 |
| 来源 | 无 | inbound badge(隐式,留扩展位) |
| 错误态 | 无 | 红边 + tooltip + retry |
| Quota 满 | 无 | 灰 + tooltip "已达上限 10" |
| 移除 | ✕ 按钮 | 同(行为不变) |

---

## §3 决策点(已批,见决策日志 §10)

### §3.1 3 个已批决策

| ID | 决策 | 选项 | 推荐 |
|---|---|---|---|
| **S5-A** | Chip 升级范围 | **A1** 全套(icon + 缩略图 + 错误 + quota)<br>✅ **A2** 仅 icon + 缩略图,错误/quota 留 v2<br>**A3** 仅 icon,缩略图留 v2 | **已批 A2** — icon 零成本、缩略图次低成本、错误/quota 是工程量级(需要 PendingAttachment 改 + 错误传递链) |
| **S5-B** | 全局 tray 范围 | ✅ **B1** 仅 user-pinned(草稿 / artifact 行 hover "Pin to tray")<br>**B2** 全自动(草稿自动进 tray,无 pin 概念)<br>**B3** 仅 artifacts(草稿不进,artifacts 自动入) | **已批 B1** — 全自动容易让 tray 被草稿刷爆,user-pinned 是 ChatGPT 收件箱风格的成熟范式;节省 1 个决策 |
| **S5-C** | LRU 驱逐策略 | **C1** time-based(N 天未访问)<br>**C2** count-based(总条数 N)<br>✅ **C3** 双约束(time AND count) | **已批 C3** — count 给硬上限,time 给软上限;tray 容量设 50 条 + 7 天,单条大小限额 50 MB |

### §3.2 决策背后的 trade-off(供回退时查)

- **A1 vs A2**:A1 全套体验最好但**单 PR 体积膨胀**;A2 把错误/quota 拆 v2,**2 个 PR 各自 ship**
- **B1 vs B2**:B2 自动入 tray → tray 沦为"草稿影子",失去"重要文件集合"语义;B1 pin 显式手势让用户**主动**选择,语义清晰
- **C1 vs C3**:C1 单 time-based 容易被"塞 100 个老文件"攻击;C3 双约束同时防短时大量 + 长时累积

---

## §4 缩略图生成(本阶段 tactical)

### §4.1 生成时机(lazy)

```text
PendingAttachment 构造时
  └─ thumbnail_state = ThumbnailState::Pending (空)
  └─ mime_type.starts_with("image/") && size < 2 MB
      └─ web_sys::create_image_bitmap_from_buffer
          ├─ 成功: thumbnail_state = Ready(data_url)
          └─ 失败: thumbnail_state = Failed(reason)
  └─ 其它: thumbnail_state = NotApplicable (不显示缩略图,走 icon)

chip mount 时
  └─ 若 thumbnail_state == Pending, 触发上一步
  └─ 渲染按 state 分支:Ready → <img>;Failed → icon + tooltip;NotApplicable → icon
```

### §4.2 缩放参数

- 目标:36×36 px(草稿 chip 高度约 28-32px)
- 编码:`image/png`(无损,小) / `image/webp`(更小,但 Safari 旧版不支持,**不**用)
- quality:N/A(PNG 无 quality)
- 异步:`OffscreenCanvas` + `transferControlToOffscreen()`(避免主线程阻塞)

### §4.3 失败兜底(显式 4 类)

1. `mime_type` 不是 `image/*` → `NotApplicable`,走 icon
2. `size >= 2 MB` → `SkippedTooLarge`,走 icon,hover 提示"图片过大,不显示缩略图"
3. `create_image_bitmap` 失败(浏览器不支持 / OOM) → `Failed(decoding)`,走 icon,hover 显示 "解码失败"
4. `OffscreenCanvas.transferToImageBitmap()` 失败 → 同 3

### §4.4 缓存与生命周期

- `thumbnail_state` 与 `PendingAttachment` **同生命周期**:草稿清除 → 缩略图 data-URL 释放
- 草稿重排 `<For>` key 稳定 → `thumbnail_state` 不重算
- 草稿清空 → 整个 `attachments: RwSignal` reset,data-URL 字符串随 `Vec<PendingAttachment>` drop 释放

---

## §5 LRU 行为(本阶段 tactical)

### §5.1 草稿层(per-conversation)

- **count 上限**:`MAX_ATTACHMENT_COUNT = 10`(维持)
- **size 上限**:`MAX_ATTACHMENT_SIZE_BYTES = 10 MB`(维持)
- **quota 满时行为**:新增文件 silent skip + console warn(当前行为)+ **UI 显式提示**("已达上限 10 个附件")
- **不引入 LRU**:草稿是 per-conversation transient state,发送即清,**不需要** LRU 持久

### §5.2 artifacts 层(per-conversation)

- **当前**:**无** LRU,服务端 `/artifact/c/{id}/f` 永久保留
- **本阶段**:**不**改服务端永久保留(那是核心数据)
- **UI 端 LRU 渲染**:客户端列表只显示"最近 7 天 + 最多 50 条",其余折叠 "Show older" 链接
- **淘汰策略**:UI 端不删服务端数据,只折叠渲染;删除走"用户在 UI 显式删除"路径(`artifacts.delete` RPC,**本阶段不实施**,只留接口)

### §5.3 tray 层(per-user,跨 conversation)

- **本阶段新增**:`rps.tray` slot,持 `Vec<TrayItem>`
- **`TrayItem`**:派生自 `PendingAttachment` + `ArtifactItem`,新增 `pinned_at: Instant` + `last_accessed_at: Instant` + `source: TraySource`(DraftPin / ArtifactPin / UserUpload)
- **count 上限**:**50 条**(可配,默认 50)
- **time 上限**:**7 天未访问**(可配,默认 7)
- **size 限制**:**单条 50 MB**(`MAX_TRAY_SIZE_BYTES`,与草稿 10 MB 错开)
- **双约束 C3**:count 或 time 任一触发 → tray 列表折叠 + "X 条已过期 / Y 条超容量" 提示;**不**自动删除(用户显式)
- **持久化**:`localStorage["aleph.tray.v1"]` 序列化(类比 `pending_attachments` 已有 `Serialize + Deserialize` 通道)
- **跨端**:Panel 多 tab 同步走 `BroadcastChannel`(若启用);TUI 端**不**走 tray(阶段 7)

### §5.4 LRU 总览表

| 层 | 范围 | count 上限 | time 上限 | size 上限 | 行为 |
|---|---|---|---|---|---|
| 草稿 | per-conversation | 10 | (无) | 10 MB | 发送即清,无 LRU |
| artifacts 渲染 | per-conversation | 50 (UI) | 7 天 (UI) | (无) | UI 折叠,不删数据 |
| tray | per-user 跨会话 | 50 | 7 天 | 50 MB | 显式 pin,UI 折叠 + 提示 |

---

## §6 全局 Tray — 接口设计(本阶段只 spec)

### §6.1 `rps.tray` slot(新)

- id:`rps.tray`
- 位置:右栏 Tab 条,**非**默认打开(与 `rps.capabilities` 平行)
- 渲染:见 §6.2

### §6.2 组件结构

```
<RightPaneSlot id="rps.tray" />
└── TraySurface
    ├── header("Tray" + count + "Pin new" 按钮)
    ├── filter chips(All / Images / Files)
    ├── list
    │   ├── TrayRow(item, on_unpin, on_open)
    │   └── ...
    └── footer("X 已过期 / Y 超容量" 折叠)
```

### §6.3 `TrayItem` 结构(新增)

```rust
pub struct TrayItem {
    pub id: String,                  // ulid/uuid,稳定
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub origin: String,              // 与 ArtifactItem 复用
    pub source: TraySource,
    pub pinned_at: Instant,
    pub last_accessed_at: Instant,
    pub blob_ref: BlobRef,           // IndexedDB / localStorage 选择
}

pub enum TraySource {
    DraftPin,    // 草稿 chip hover "Pin to tray"
    ArtifactPin, // artifact row hover "Pin to tray"
    UserUpload,  // tray 内"上传"按钮
}

pub enum BlobRef {
    /// ≤ 1 MB 文件,base64 存 localStorage(快)
    InlineBase64(String),
    /// > 1 MB 文件,IndexedDB 存 binary blob(慢但省内存)
    IndexedDb { key: String, size: u64 },
}
```

### §6.4 Pin 入口(交互手势)

| 来源 | 触发 | 行为 |
|---|---|---|
| 草稿 chip | hover → "Pin to tray" 按钮 | `PendingAttachment` → `TrayItem`,弹 toast "Pinned to tray" |
| artifact row | hover → "Pin to tray" 按钮 | `ArtifactItem` → `TrayItem`(`url` 走 IndexedDB 拉取 + 缓存) |
| tray 自身 | "Upload" 按钮 | 走 `read_file_list_into` 同样的 FileReader 路径 |

### §6.5 Tray 容量行为(C3 双约束)

```text
on every tray insert:
  current = tray.len()
  if current > MAX_TRAY_COUNT (= 50):
    emit "tray at capacity, please unpin"
  if (now - last_accessed) > MAX_TRAY_AGE_DAYS (= 7) for any item:
    emit "X items expired"

on every tray access (read / open / re-pin):
  update last_accessed_at = now

on app start:
  re-evaluate time-based expiry
  show banner if any expired
```

**关键不变量**:**不**自动删数据,只标 "expired" + 折叠;删除走显式"Unpin"

### §6.6 与现有"双 panel" 关系

- **artifacts slot**(`rps.artifacts`):per-conversation,显示当前会话的 inbound / outbound / deliverables;**tray 不复制** artifacts 内容,而是"user-pinned 视图"
- **判据 §1 防两份表述**:tray 是**额外入口**(**非** artifacts 镜像);同一文件在 artifacts 与 tray 都有,各自独立更新(`source` 字段区分)
- **tray 详情预览**:**不**复刻 `preview.rs::Preview`(避免判据 §1);点击 tray item 走 `window.open()` 或新建 `<a target="_blank">`

### §6.7 TUI 端

- **本阶段不实施**,只留接口
- `RegionKind::RightPane(Tray)` 在 `interfaces/tui/src/tui/regions.rs` 添加 variant(若阶段 7 决定 TUI 加右列)
- TUI 端 tray 走简化版:**只读 + count + unpin**,无 blob 持久(切会话即丢)

---

## §7 验收(对齐阶段 1 spec §10 / 阶段 2 spec §8 / 阶段 3 spec §10 / 阶段 4 spec §5)

### §7.1 单元测试(`alephcore` 端 + `aleph-panel` 端)

- UT-1:`format_size` 三档(B / KB / MB)正确(`attachments.rs:215-244` 已有,本阶段不重测)
- UT-2:`attachment_key` 稳定 across insert/remove/reorder(已有,本阶段不重测)
- UT-3:新增 `mime_to_icon(mime: &str) -> IconKind` 纯函数,8 mime 模式覆盖 + fallback
- UT-4:新增 `tray_lru_decide(current: &[TrayItem], new: &TrayItem) -> Vec<ExpiredItem>` 纯函数,time + count 双约束下返回 expired 列表
- UT-5:`tray_serialize` / `tray_deserialize` round-trip(`localStorage["aleph.tray.v1"]` 路径)
- UT-6:thumbs 生成函数 `should_generate_thumbnail(mime, size) -> bool`(< 2 MB + image/*)
- UT-7:`BlobRef` 决策:`size < 1 MB` → `InlineBase64`,`size >= 1 MB` → `IndexedDb`
- UT-8:tray pin 行为:`PendingAttachment → TrayItem` 转换保留 `name / mime_type / size`,**不**保留 `data_base64`(走 IndexedDB 重新读取)

### §7.2 视觉回归(Playwright on webchat build)

- VS-1:拖入 1 个 PNG(500 KB)→ chip 立即显示缩略图(≤ 200ms)+ 名称 + 大小
- VS-2:拖入 1 个 3 MB PNG → chip **不**显示缩略图,显示 "Image" label + ⚠ tooltip "图片过大"
- VS-3:拖入 1 个 PDF → chip 显示 📕 + 名称 + 大小
- VS-4:拖入 1 个 12 MB PDF → silent skip + 控制台 warn + UI "文件过大(>10MB)已跳过" toast
- VS-5:拖入 11 个文件 → 第 11 个文件 disabled 样式 + tooltip "已达上限 10"
- VS-6:打开右栏 tray slot → 显示 0 项(首次)+ "Pin from draft or artifact to start"
- VS-7:从草稿 pin 1 个文件 → tray slot count +1,出现新行
- VS-8:tray 满 50 项后尝试 pin → 提示 "Tray 容量已满,请先 unpin"

### §7.3 端到端 smoke(走 ACP JSON + 真实 localStorage)

- E2E-1:打开会话 A,拖入 1 个 PNG,发送,草稿清空,artifact 出现该 PNG(走原流程)
- E2E-2:打开会话 A,拖入 1 个 PNG,hover chip → "Pin to tray" → tray count +1;刷新页面 → tray 仍然在(localStorage 验证)
- E2E-3:打开会话 B,tray 显示会话 A pin 的 PNG(跨会话验证)
- E2E-4:tray 满 50 + 插入第 51 → 提示"容量已满",不删旧项
- E2E-5:模拟系统时间前进 8 天 → 启动时 tray 标"X 已过期",不删数据
- E2E-6:IndexedDB 路径:拖入 1 个 5 MB PDF → tray pin → 验证 IndexedDB 有对应 entry;reload 后 tray 仍可预览

### §7.4 性能预算

- 单 chip mount(无缩略图) ≤ 5ms
- 单 chip mount(带缩略图) ≤ 50ms(`OffscreenCanvas` 缩放 + 编码)
- tray 50 项首次 paint ≤ 30ms(纯列表,无 IO)
- tray localStorage 反序列化 ≤ 10ms(50 条)
- tray IndexedDB 读取(单条) ≤ 20ms(5 MB PDF 不阻塞 UI)

### §7.5 R10 红线守住

- 新增文件:**3 个**(composer/attachments 内扩展 1 个 + right_pane/slots/tray.rs 1 个 + tray 模块文件 1 个)
- 既有文件触碰:≤ 4 处(`attachments.rs` 改 PendingAttachment 派生 + `PendingAttachment` 定义处 + `chat/state/mod.rs` + 右栏 register 启动)
- 不引新依赖(`web_sys` 已有,`OffscreenCanvas` 浏览器原生)

---

## §8 风险与红线

### §8.1 R1–R10 红线(本阶段全部守住)

| 红线 | 本阶段兑现 |
|---|---|
| R1 Core 不调平台 API | tray 模块是 I/O 层,无 platform 特定 |
| R2 UI 逻辑唯一源 | Leptos 组件,判断在 view |
| R3 核心轻量化 | 零新依赖(纯 `web_sys` + 浏览器原生) |
| R4 Interface 层禁止业务逻辑 | chip 升级是渲染层,L1 chip 不含业务 |
| R5 AI 主动到达 | N/A(本阶段不涉及 AI 触发) |
| R6 一核多端 | TUI 端**不**实施 tray(阶段 7 独立审批) |
| R7 LLM 主权 | 不替代 LLM 决策 |
| R8 工具即一切 | tray 入口走右栏 slot 注册 |
| R9 智慧在 Prompt 中 | N/A |
| R10 薄 Harness | 新增 ≤ 3 文件,既有触碰 ≤ 4 处 |

### §8.2 本阶段特有风险

| ID | 风险 | 缓解 |
|---|---|---|
| R-S5-1 | 缩略图生成阻塞主线程 | `OffscreenCanvas` 异步;失败兜底走 icon |
| R-S5-2 | IndexedDB 跨浏览器不一致 | 仅 fallback,主路径 base64 in localStorage ≤ 1 MB;> 1 MB 走 IndexedDB,显式提示用户 |
| R-S5-3 | localStorage 5 MB 上限 | 50 条 × 平均 1 MB = 50 MB,超 localStorage 限额(典型 5–10 MB);改 IndexedDB 主路径,localStorage 只存 metadata |
| R-S5-4 | tray 容量计算(50 条 vs 7 天)冲突 | C3 双约束任一触发即提示,**不**自动删 |
| R-S5-5 | pin 同一文件多次 | 检 `name + size + source` 唯一键,重复 pin 走 update `last_accessed_at` |
| R-S5-6 | 跨会话隐私:用户在会话 A pin,会话 B 看到 | **故意行为**;tray 是 per-user 不是 per-conversation;但**不**显示会话名(只显示文件本身) |
| R-S5-7 | TUI 端用户期望 tray 出现 | 阶段 7 独立审批;本阶段 TUI 走"接口预留 + 不实施" |
| R-S5-8 | mime 误判(如 `application/octet-stream` + `.png` 扩展名) | 优先 mime,扩展名 fallback;fallback 显示通用图标 |
| R-S5-9 | `read_file_list_into` silent skip 改成"显式 toast"破坏既有 UX | toast 走非阻塞,3s 自动消失,与现有 toast 风格一致 |

### §8.3 已知不做(显式 not-do)

1. **不**实施 tray 内文件全文搜索
2. **不**实施 tray 文件夹结构(扁平列表,无目录)
3. **不**实施 tray 跨设备同步(纯本地)
4. **不**实施 tray "分享"按钮
5. **不**实施 tray 拖拽到 chat(避免判据 §1 风险,跨层引用)
6. **不**实施 tray 内"重新生成缩略图"按钮(失败就失败,hover 提示)
7. **不**改服务端 `/artifact/` 永久保留策略
8. **不**改 `MAX_ATTACHMENT_SIZE_BYTES = 10 MB`(维持)
9. **不**改 `MAX_ATTACHMENT_COUNT = 10`(维持)
10. **不**实施 TUI 端 tray(阶段 7 独立审批)
11. **不**复制 ChatGPT 任何视觉元素

---

## §9 阶段路线(本 spec 自身)

| 步骤 | 范围 | 验收 |
|---|---|---|
| 9.1 决策 S5-A/B/C 用户拍板 | 3 决策 | 用户回复 |
| 9.2 `PendingAttachment` 派生 `+ thumbnail_state + error` | 1 文件 | 编译通过 |
| 9.3 `mime_to_icon` 纯函数 + chip 升级 A2 部分 | 1 文件 | VS-1/2/3/4/5 通过 |
| 9.4 `BlobRef` 决策 + IndexedDB 桥(> 1 MB 文件) | 1 文件 | E2E-6 通过 |
| 9.5 `TrayItem` 结构 + `tray_lru_decide` 纯函数 | 1 文件 | UT-4/5/7/8 通过 |
| 9.6 `rps.tray` slot 注册 + 渲染 | 1 文件 | VS-6/7/8 通过 |
| 9.7 `BroadcastChannel` 跨 tab 同步(可选) | 1 文件 | E2E-2 跨 tab 通过 |
| 9.8 验证矩阵 7.1–7.5 全跑 | 全套 | UT/VS/E2E/性能 100% |

**不**实施任何代码 — 上述步骤是**实施 spec**,待用户独立批准 §3 决策后启动。

---

## §10 file:line 速查(本阶段)

### §10.1 草稿层

| 路径 | 行 | 角色 |
|---|---|---|
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:1-15` | file header | `FileReader` → data-URL → base64 入口说明 |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:17-26` | fn | `format_size(bytes)` |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:35-37` | const | `MAX_ATTACHMENT_SIZE_BYTES = 10 MB` |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:43` | const | `MAX_ATTACHMENT_COUNT = 10` |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:53-62` | fn | `attachment_key` stable hash |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:76-141` | fn | `read_file_list_into` silent fail |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:144-194` | component | `AttachmentPreviewBar` chip 条 |
| `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:196-244` | tests | format_size / attachment_key |
| `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs:21-27` | struct | `PendingAttachment` 4 字段 |
| `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs:31-37` | struct | `QueuedPrompt` 含 attachments |

### §10.2 落地层(artifacts)

| 路径 | 行 | 角色 |
|---|---|---|
| `interfaces/webchat/src/components/artifacts/mod.rs:1-39` | file header | 4 来源说明 + 守卫 |
| `interfaces/webchat/src/components/artifacts/mod.rs:48-65` | enum | `ArtifactFilter` All/Images/Files |
| `interfaces/webchat/src/components/artifacts/mod.rs:142-151` | fn | `origin_badge_class` 3 类 |
| `interfaces/webchat/src/components/artifacts/mod.rs:155-575` | component | `ArtifactsSurface` 主组件 |
| `interfaces/webchat/src/components/artifacts/files.rs:1-100` | file header | 项目树说明 |
| `interfaces/webchat/src/components/artifacts/files.rs:32-192` | component | `WorkspaceFiles` 折叠底部 |
| `interfaces/webchat/src/components/artifacts/row.rs:42-100` | component | `ArtifactRow` image thumb + name + badge |
| `interfaces/webchat/src/components/artifacts/preview.rs:1-44` | file header | image/text viewer 原因 |
| `interfaces/webchat/src/components/artifacts/preview.rs:46-58` | enum | `PreviewTarget::Image` / `Text` |
| `interfaces/webchat/src/components/artifacts/preview.rs:60-79` | fn | `for_item` 判 mime |
| `interfaces/webchat/src/components/artifacts/preview.rs:100-342` | component | `Preview` 覆盖 pane 而非 window |
| `interfaces/webchat/src/components/artifacts/deliverable.rs:1-21` | file header | deliverable 与 material 区分 |
| `interfaces/webchat/src/components/artifacts/deliverable.rs:30-44` | fn | `partition_deliverables` |
| `interfaces/webchat/src/components/artifacts/deliverable.rs:47-65` | fn | `first_unseen_deliverable` |
| `interfaces/webchat/src/components/artifacts/deliverable.rs:74-104` | component | `DeliverableCard` 独立卡片 |

### §10.3 API 真源

| 路径 | 角色 |
|---|---|
| `interfaces/webchat/src/api/artifacts.rs` | `ArtifactItem`, `ArtifactsApi`, `TextPreview`, `ping_is_for_session`, `ARTIFACT_TOPIC` |
| `aleph_protocol::artifact` | wire 类型,`ORIGIN_INBOUND` / `ORIGIN_EXPORT` / `ORIGIN_OUTBOUND` |

### §10.4 TUI 端(无 attachment 概念)

| 路径 | 角色 |
|---|---|
| `interfaces/tui/src/tui/widgets/input_area.rs` | 输入框(无 attachment chip) |
| `interfaces/tui/src/tui/widgets/btw_panel.rs` | "by the way" 上下文(非附件) |
| `interfaces/tui/src/tui/widgets/tasks_panel.rs` | 任务列表(非附件) |

---

## §11 决策日志(本 spec)

| ID | 决策 | 状态 | 来源 |
|---|---|---|---|
| S5-A | Chip 升级范围(全套 / 仅 icon+缩略图 / 仅 icon) | ✅ **A2** icon + 缩略图 | 用户批 m00617;icon 零成本 + 缩略图次低,v2 再加错误/quota |
| S5-B | 全局 tray 范围(仅 user-pinned / 全自动 / 仅 artifacts) | ✅ **B1** 仅 user-pinned | 用户批 m00617;ChatGPT 收件箱风格成熟范式 |
| S5-C | LRU 驱逐策略(time / count / 双约束) | ✅ **C3** 双约束 50 条 + 7 天 | 用户批 m00617;count 硬上限 + time 软上限 |

**继承自上游 spec 的已批决策**(不重复拍板):
- 3.1.1 = A(flex 第三列)
- 3.1.2 = trait(可注册)
- 3.1.3 = subagent 默认 Tab,capability 次级,tray 次级
- S2-A/B/C = 阶段 2 容器过渡 + ChatOnly 分支 + tool API 直读
- S3-A/B/C = subagent 默认 Tab / 分 2 slot / 内联 drawer
- S4-A/B/C = capability 次级展开 / diagnostic 默认折叠 / 内联 drawer
- S5-A/B/C = chip 升级 icon+缩略图 / tray user-pinned / LRU 双约束(本 spec,2026-10-15 拍板)

---

## §12 引用

- **上游 spec**:
  - [2026-10-15-rightside-optimization-plan.md](./2026-10-15-rightside-optimization-plan.md) — 阶段 0 总览
  - [2026-10-15-rightpane-stage1-slot-spec.md](./2026-10-15-rightpane-stage1-slot-spec.md) — 阶段 1 slot 接口
  - [2026-10-15-rightpane-stage2-host-switching-spec.md](./2026-10-15-rightpane-stage2-host-switching-spec.md) — 阶段 2 宿主切换
  - [2026-10-15-rightpane-stage3-subagent-slot-spec.md](./2026-10-15-rightpane-stage3-subagent-slot-spec.md) — 阶段 3 子代理 slot
  - [2026-10-15-rightpane-stage4-capabilities-slot-spec.md](./2026-10-15-rightpane-stage4-capabilities-slot-spec.md) — 阶段 4 能力 slot
- **真源**:
  - `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs`
  - `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs:21-27` (`PendingAttachment`)
  - `interfaces/webchat/src/components/artifacts/{mod,files,row,preview,deliverable}.rs`
  - `aleph_protocol::artifact` wire 类型
- **未引用私有代码或 ChatGPT/Codex 视觉元素**

---

*Last updated: 2026-10-15 · 阶段 5 spec v1.1 · S5-A=A2 / S5-B=B1 / S5-C=C3 已批*
