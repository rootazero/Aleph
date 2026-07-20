# Aleph Panel Phase 2 · iPhone 移动端三修复 — Design Spec

> 2026-06-25。延续 Phase 1 移动适配（[[project-aleph-panel-phase1-mobile-adaptation]]）。
> 首次真机 390px QA 暴露三个缺口，本 spec 定义其修复设计。
> crate = `aleph-panel`（`interfaces/webchat/`），纯 Leptos/WASM + Tailwind CSS。

## Goal

修复 390px iPhone 竖屏 QA 发现的三个移动端缺口：① settings master-detail 双栏在手机上横向溢出、右侧配置栏不可达；② `--aleph-content-top` 移动档偏移不足导致顶部内容被 MobileTopBar 遮挡；③ chat 顶栏 agent 名重复。

## Architecture / 范围约束（Global Constraints）

- **设备范围 = iOS-first**：本轮只为 **iPhone 竖屏（`max-sm`, <640px）** 实施。**iPad / 桌面 ≥640px 维持现有桌面双栏布局，字节级不变**（红线）。
- **断点即分离**：现有单断点 CSS 门控（`max-sm` = 移动壳；`≥640` = 桌面）已天然分离 iPhone 与 iPad。三个修复全部落在 `max-sm` 一侧；iPad 不经过这些代码路径。
- **Android 将来迁移**：所有改动是 CSS 断点驱动（非 iOS 专有 API），Android 手机同宽度自动套用，无需现在做额外工作、也不被锁死。
- **零 core `src/` 改动**：仅改 `interfaces/webchat`（panel crate）。
- **零新依赖**。
- **单一 UI 源（R2）**：不复制独立 iPhone 组件树；iPhone 行为锁在 `max-sm:` 类 / `is_mobile` 信号门控中，与桌面变体共用同一组件。

## 根因摘要（QA ground-truth）

| # | 现象 | 根因（已实测） |
|---|------|----------------|
| 1 | 嵌入/生成/重排序/ACP/LLM 提供商/搜索/频道平台页在 iPhone 上右栏不可达 | 各页用桌面 `w-5/12 min-w-[400px]` + `w-7/12 min-w-[320px]`（或 `w-56` 侧栏）双栏；390px 下左栏≥400px 已超视口，右栏被父层 `overflow-hidden` 裁掉。Phase 1 的 reflow 只动了**表单内部** `.grid-responsive`，没动**页面级双栏外壳**。 |
| 2 | 各非-chat tab 顶部标题（"基础"/"嵌入提供商"/"记忆库"/agents H1）被 43px 顶栏压住 | `tailwind.css:1964` 移动档 `--aleph-content-top = calc(safe-area-top + 0.85rem)`（≈13.6px），只避让了 notch（safe-area），**漏算了坐在 notch 下方的 43px `MobileTopBar` 覆盖层**（`absolute top-0 z-20`，`app.rs:412`）。 |
| 3 | chat 顶栏 agent 名重复 | `chat/view.rs:242` 的 `MobileTopBar` 同时 `title = agent名` 与 `left = 含 agent名的 pill`。 |

> 注：agents 页"顶栏过挤"是 #2 的症状——body H1（"Main Agent / 默认 / 删除"）被偏移不足顶进 43px 栏内、与全局 title "智能体" 相撞。**#2 修好后自动消失**，无需单独处理。

## 设计

### Fix #2 — 顶栏内容偏移（先做，根因修复，收益最广）

**单点 CSS 改动**，`interfaces/webchat/styles/tailwind.css` 移动档 `:root`（L1964）：

```css
/* 当前 */
@media (max-width: 639px) {
  :root { --aleph-content-top: calc(var(--safe-area-top) + 0.85rem); }
}
/* 改为：清掉 notch + 43px MobileTopBar 覆盖层 + ~5px 间隙 */
@media (max-width: 639px) {
  :root { --aleph-content-top: calc(var(--safe-area-top) + 3rem); }
}
```

- `3rem` = 48px：实测 `.mobile-top-bar` 非-safe 总高 43px（`pt: safe+0.5rem` + 内容 + `pb-2`），48px 清掉 bar 并留 ~5px 间隙。**精确值实测验证后可微调 ±0.25rem**。
- 同步更新 L1960-1962 注释：内容须避让 `safe-area + MobileTopBar`，而非只 notch。
- **不碰** `:root` 桌面档 `0.85rem`（L1932）与 `html[data-platform=macos]` `2.45rem`（L1945）——`@media (max-width:639px)` 不影响 ≥640。
- 收益：`.aleph-content-top` 被所有 section page 根容器消费（settings/memory/agents/teams/extensions），一处修复全部归位；agents "过挤" 一并解决。

### Fix #3 — chat 顶栏去重（小，仅 `chat/view.rs`）

- `chat/view.rs:242` 的 `MobileTopBar` **去掉冗余的中间 `title`**（传空 `Signal::derive(|| String::new())`），保留左 pill（avatar + agent名 + chevron，可点切换 agent，信息更全且可交互）+ 右铃铛默认。
- `MobileTopBar` 组件本身（§11 钉死接口）不改；空 title 即中间不渲染文字。
- 桌面不受影响（`.mobile-top-bar` 是 `max-sm:` only）。

### Fix #1 — settings 双栏钻入式折叠（主体）

**统一 `max-sm` 钻入模式**，逐页套用（各页手写 flex 无共享组件，但套同一套类规则）。

**涉及页面**（master-detail 双栏外壳）：

| 文件 | 双栏结构 | 子型 |
|------|----------|------|
| `settings/embedding_providers/mod.rs` | L83 左 `w-5/12 min-w-[400px]` + L277 右 `w-7/12 min-w-[320px]` | 硬溢出 |
| `settings/generation_providers/mod.rs` | L161 左 `min-w-[400px]` + L339 右 `min-w-[320px]` | 硬溢出 |
| `settings/reranking_providers/mod.rs` | L114 左 `min-w-[400px]` | 硬溢出 |
| `settings/acp_harnesses/mod.rs` | L102 左 `min-w-[400px]` | 硬溢出 |
| `settings/providers/mod.rs` | L82 左 `w-5/12 min-w-0` + L126 右 `w-7/12 min-w-0` | 过窄（不溢出但 162+228px 没法填） |
| `settings/search.rs` | L183 左 `w-5/12 min-w-0` + L230 右 `w-7/12 min-w-0` | 过窄 |
| `settings/channels/platform_page.rs` | L213 `w-56` 侧栏 + L303 `flex-1` 详情 | 侧栏变体 |

**钻入模式（§ Interface，对每页统一）：**

记 `detail_active = selected_id.is_some() || show_add_form`（用各页既有信号；platform_page 用 `selected_id`，add-form 状态按页实际）。

1. **左列表 wrapper**：追加 `max-sm:w-full max-sm:min-w-0`；可见性 `class=move || if detail_active() { "max-sm:hidden" } else { "" }`（详情激活时移动端隐藏列表，桌面 ≥640 不受 `max-sm:hidden` 影响仍并排）。
2. **右详情 wrapper**：追加 `max-sm:w-full max-sm:min-w-0`；可见性 `class=move || if detail_active() { "" } else { "max-sm:hidden" }`（无选中时移动端隐藏详情→只剩全宽列表；桌面仍显示 EmptyState）。
3. **移动返回**：详情区顶部加一个 `max-sm:`-only `‹返回` 按钮（桌面 `hidden`，移动 `max-sm:flex`），点击 `set_selected_id(None)` + `set_show_add_form(false)`，回到全宽列表。
4. **列表优先**：各页 mount 时的 auto-select-active（如 embedding/generation 自动选中当前激活 provider）用 `!is_mobile`（`expect_context::<ViewportState>().is_mobile`）门控——移动端落在列表，不直接钻进详情。
5. `platform_page.rs` 同理（`w-56` 侧栏 → `max-sm:w-full` + 详情同上），返回按钮清 `selected_id`。

**为什么钻入而非堆叠**：iOS 原生 settings 心智；复用既有 `selected_id` 信号，纯 `max-sm:` 可见性切换 + 一个返回按钮，无需新状态；与 landing→详情的钻入一致。

## Testing

- **既有单测保留**：`mobile_landing` 的 group/tab 数量断言、pinch、status_matches、compute_depths 等不动。
- **新纯函数单测（若引入）**：本设计以 CSS 类 + 既有信号门控为主，预计不新增可单测纯逻辑；若某页折叠需要派生 helper（如 `detail_active`），就地加最小单测。
- **视觉验证**：`just wasm` 重建 dist → 重编 `aleph-server` 热换运行中 daemon（rust_embed 编译期嵌入）→ chrome-devtools 390px 设备仿真逐页复看（嵌入详情可达、顶栏不遮挡、chat 不重复）。
- **桌面回归**：≥640px 截图比对，确认双栏/顶栏/偏移字节不变。

## 实施顺序

1. Fix #2（1 行 CSS，根因，先验证遮挡全消 + agents 过挤消失）
2. Fix #3（chat 去重，小）
3. Fix #1（逐页钻入折叠，主体；硬溢出 4 页优先，过窄 2 页 + platform_page 跟上）
4. 末尾一次 `just wasm` + 重编 + 390px 全量复看

## Non-Goals（YAGNI）

- iPad 专属布局/触控优化（iPad 用桌面双栏即可，非本轮范围）。
- Android 专项（同断点免费迁移，不单独做）。
- 堆叠式折叠、横向滚动方案（已否决）。
- 重构各页为共享 master-detail 组件（三次法则未到，逐页套规则即可；如未来第 N 页再抽象）。
