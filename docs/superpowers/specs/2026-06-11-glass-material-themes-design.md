# 玻璃材质主题体系设计（Glass Material Themes）

- **日期**: 2026-06-11
- **状态**: 设计已获用户确认
- **前序**: 本设计是 Panel 玻璃谱系第 5 轮，前四轮见
  `2026-06-09-panel-glass-material-design.md`（chrome 玻璃地基）、
  `2026-06-09-panel-glass-refinement-design.md` + `2026-06-09-panel-glass-theme-restore-design.md`（精修与主题恢复）、
  `2026-06-10-chat-message-visual-refresh-design.md`（消息流仿玻璃）、
  `2026-06-11-sidebar-menu-glass-design.md`（侧栏导航瓦片）。

## 1. 背景与目标

四轮玻璃化后，用户评价仍未达到"惊艳"。本轮用三个方向的情绪板（亮/暗成对）做了口味校准，
用户决定**三个方向全部保留**，作为可切换的"材质"主题。审美硬标准：**加强效果，但保持优雅**——
追求纵深感与工艺感，不是更吵的颜色或更花的特效。

三种材质：

| 材质 | 英文名 | 性格 |
|------|--------|------|
| 奢华磨砂（默认） | Quiet Luxury / `luxe` | 背景克制，工艺细节拉满：镜面发丝线、三层阴影、细粒面、精确 1px 边缘 |
| 液态玻璃 | Liquid Glass / `liquid` | 表面更透、镜面边缘最亮、画布光场鲜艳——玻璃像真实透镜 |
| 极光浓雾 | Aurora Frost / `aurora` | 画布全幅饱和极光是主角，表面是浓奶雾，色彩从雾里渗出 |

同时修复一个根本问题：**现有 chrome 真模糊背后是平滑渐变画布，模糊了等于没模糊**。
本轮给模糊"可见的结构"（画布粒面 + 悬浮输入框让消息从背后流过），让已付出的模糊成本真正可见。

## 2. 决策记录（用户逐项确认）

1. **三种材质全部成为可选主题**（不是三选一）。
2. **正交三旋钮模型**：材质 × 亮暗 × 强调色。现有「玻璃」主题退役，迁移到 液态玻璃+暗。
3. **极光色彩 = 强调色派生**：极光主色随 `--color-primary` 变（选森林→绿极光），配固定反色辉光避免单调。与现有辉光架构一致。
4. **悬浮输入框**：滚动区延伸到底部，输入框浮在消息流之上（真模糊），消息滚动时从磨砂背后流过。会话标签条同样处理。
5. **原色层 token 重构**：消灭现有"每组 token 手写 4 份分支"的重复，改为 原色层（9 小块）+ 派生层（只写一次）。

## 3. 主题模型

外观 = **材质 × 亮暗 × 强调色**，三旋钮全正交（3 × 3 × 5 = 45 组合全部成立）：

| 旋钮 | 取值 | DOM 载体 | localStorage |
|------|------|----------|--------------|
| 材质（新增） | `luxe`（默认）/ `liquid` / `aurora` | `<html data-material="liquid\|aurora">`，缺省 = luxe（镜像 accent 缺省 = mauve 的惯例） | `aleph-material` |
| 亮暗 | 亮 / 暗 / 跟随系统 | 现有 `.light` / `.dark` 类 | `aleph-theme`（现有） |
| 强调色 | 现有 5 色 | 现有 `data-accent` | `aleph-accent`（现有） |

### 3.1 Material enum

`interfaces/webchat/src/appearance.rs` 新增 `Material` enum，**完全镜像 `Accent` 的结构**：
`ALL` / `label()` / `id()` / `storage_value()` / `from_storage()` / 预览色板方法，
localStorage key `aleph-material`，应用函数设置/清除 `data-material` 属性。

### 3.2 ThemeMode::Glass 退役与迁移

- `ThemeMode::ALL` 变为 `[System, Light, Dark]`（Glass 从选择列表移除）。
- `ThemeMode::from_storage` 保留对旧值 `"glass"` 的解析：映射为 `Dark`，同时迁移逻辑把
  `aleph-material` 写为 `"liquid"`、`aleph-theme` 回写为 `"dark"`（一次性，幂等）。
- 迁移挂在既有引导路径上：`lib.rs:46` 的 `appearance::init_appearance()` 在 mount 前重放
  所有外观轴；无独立 inline 引导脚本，无 FOUC 新增风险。
- CSS 中全部 `html.glass` 选择器随原色层重构删除。

## 4. Token 架构：原色层 + 派生层

### 4.1 现状问题

`styles/tailwind.css` 中每组 token（atmosphere 画布、msg-glass、glass utility、surface-raised…）
手写 4 份分支（亮 / `.dark` / `html.glass` / 系统暗 media-query），同值复制。直接乘 3 材质会爆到 ~12 份。

### 4.2 新结构

**原色层（9 个小块）**：每个 材质×亮暗 组合一块原色定义——
3 材质 × {亮, 暗} = 6 块，外加 3 块系统跟随（media-query 内镜像各材质暗值，只镜像原色）。
选择器形态：`:root`（luxe 亮，默认）、`.dark`（luxe 暗）、`[data-material="liquid"]`、
`[data-material="liquid"].dark`…… 系统跟随沿用现有 `:root:not(.light)` 模式。

每块原色约 15~20 个变量，族谱：

- **画布场**: `--mat-solid-ground`、`--mat-canvas-base`、辉光强度（强调色 mix 百分比 ×3 档）、
  `--mat-glow-counter`（固定反色，极光材质为此值最浓）、`--mat-grain-opacity`
- **表面墨**: `--mat-ink-raise`（气泡/chrome 填充基色）、`--mat-ink-sink`（inset/下沉面基色）——
  亮色态二者极性相反（白墨 vs 深墨），故原色携带完整颜色而非纯透明度
- **填充浓度**: `--mat-fill-chrome` / `--mat-fill-bubble` / `--mat-fill-inset`（三档百分比）
- **边缘与光**: `--mat-edge`（发丝线）、`--mat-edge-top`（镜面顶边）、`--mat-spec`（inset 高光）、
  `--mat-sheen`（斜向光泽强度）
- **深度**: `--mat-shadow-ink` + 阴影深度档位
- **模糊档**: `--mat-blur-transient` / `--mat-blur-chrome` / `--mat-saturate`（替代现 `--glass-blur*` 三档）

**派生层（只写一次）**：`msg-glass` 系、`glass-inset`、`nav-tile`、`.aleph-sidebar`、
输入框/标签条、`.glass` 弹层、`--code-header-bg`/`--code-pre-bg`、`--color-surface-raised` 等
全部表面 token 由原色经 `color-mix(in oklch, …)` 派生——沿用代码库既有惯用法
（`color-mix` 的百分比参数可由 `var()` 提供）。用户气泡继续从 `--color-primary` 派生，
混合百分比与外发光强度参数化为原色（`--mat-user-fill` / `--mat-user-glow`）。

### 4.3 收益

- 现有四份手写重复顺势消灭；三处材质微调不再需要同步三处。
- 未来加第四种材质 = 加一块原色，派生层零改动（OCP）。
- 强调色 5 色对全部材质自动生效（派生链上游就是 `--color-primary`）。

## 5. 材质配方（视觉规格）

定性规格如下表；精确数值在实现时以情绪板
（`docs/superpowers/specs/assets/2026-06-11-glass-material/direction.html`）
的六个 skin 为锚点校准，并受 §5.3 对比度红线约束。

| 维度 | 奢华磨砂 luxe | 液态玻璃 liquid | 极光浓雾 aurora |
|------|--------------|----------------|----------------|
| 画布场强度 | 克制（现状微深化） | 鲜艳（辉光浓度显著上调） | 全幅饱和（画布是主角） |
| 表面透明度 | 中（接近现状） | 最透（亮态 ~0.4x，暗态 ~0.07x 量级） | 最浓奶雾（色彩靠画布透出） |
| 边缘性格 | 精确发丝线 + 亮 spec 线 | 镜面边缘最亮（透镜感） | 最柔（边缘融进雾里） |
| 阴影性格 | 三层精确堆叠（紧贴+中距+环境） | 中等 + 强调色外发光（用户气泡） | 大而软的单层 |
| 模糊档（chrome/瞬态） | ≈ 现状 16/20px | 最高（≈ 现 glass 主题 24/34px） | 高（雾感靠浓填充而非极限模糊） |
| 粒面 | 细而克制 | 中 | 低（雾要顺滑） |

### 5.1 画布粒面层（structure for blur）

`.aleph-shell::before` 在既有 4 层 radial-gradient 之上叠一层静态细粒面
（data-URI `feTurbulence`，与 `.glass::after` 同源），**绘制一次、零每帧成本**。双重作用：

1. 给 chrome 真模糊"可见的结构"——粒面在侧栏/输入框背后被冰平，磨砂差异在边缘立现，
   根治"模糊了等于没模糊"。
2. 抖掉大面积柔和渐变的色带（banding）。

粒面浓度是原色（`--mat-grain-opacity`），按材质分档。

### 5.2 消息气泡

三材质都**保持仿玻璃配方**（零 backdrop-filter 红线不动）：半透填充 + 160° 光泽 + 亮顶边 + 阴影，
差异全在 token 数值（经原色层自动分化）。`reduced-transparency` 坍缩行为延续。

### 5.3 对比度红线

正文文字对正文气泡填充的对比度 ≥ **4.5:1**（WCAG AA）。校准点是 液态·亮（填充最稀）：
若情绪板数值不达标，提高 `--mat-fill-bubble` 直到达标，透明感损失由 sheen/边缘补偿。

## 6. 布局改动：悬浮输入框

### 6.1 结构（`views/chat/view.rs:136-149`）

现状：`SessionTabs` / `MessageList` / `InputArea` 为 flex 纵向三段，互不重叠。
改为：消息滚动区延伸到底部，`InputArea` 绝对定位悬浮其上（左右与底部留边）：

- `MessageList` 的滚动容器（`messages.rs:164` `absolute inset-0 overflow-y-auto`）不变，
  其滚动**内容**追加底部 padding ≥ 输入框高度 + 间距，保证末条消息能完整滚出输入框上缘。
- 输入框高度会随 queue_bar / 附件条 / 多行输入变化：padding 策略候选
  （CSS var + ResizeObserver 同步实测高度，或宽松固定值），在实现计划中定夺；
  验收标准是"任何输入框高度下，滚到底时末条消息完整可见"。
- `InputArea` 容器升级为**真模糊面**（`--mat-blur-chrome` 档 + 材质原色派生的填充/边缘/阴影）。
  queue_bar、附件条在容器内随浮，无单独处理。

### 6.2 顶部对称处理

- 滚动内容上缘加渐隐带：优先 `mask-image: linear-gradient(…)` 作用于滚动容器；
  在 WKWebView 实测滚动性能，若有可感知代价则**砍渐隐、保悬浮**（渐隐是增强项不是必需品）。
- `SessionTabs`（≥2 会话时条件渲染）同样升级真模糊面，消息从其渐隐带下穿过。

## 7. 设置 UI

- `views/settings/appearance.rs`：外观页新增"材质"行——三个带微缩预览的选择块
  （预览块用对应材质的原色渲染小样），交互模式与现有亮暗/强调色行一致。
- `components/theme_toggle.rs`：弹层新增材质行；材质切换复用既有 `apply` 路径，
  圆形揭幕动画（View Transition）照常工作。
- 两处共享 `appearance::Material` 的读写函数（单一来源）。

## 8. 资源红线（正式化）

1. 常驻真模糊面 ≤ **3**：侧栏 + 悬浮输入框 + 会话标签条（第三个为条件渲染）；瞬态弹层（`.glass`）照旧。
2. 消息气泡 / 导航瓦片**永不** backdrop-filter。
3. 画布静态**零动画**；粒面层绘制一次零每帧成本；不恢复已移除的 drift 动画。
4. 模糊档位上限不超过现 glass 主题已验证的 24px（chrome）/ 34px（瞬态）。
5. 验收时用 Activity Monitor 对比空闲 CPU/GPU 与现版本基线（参照 bridge 瘦身轮的方法）。

## 9. 无障碍与回退

- `prefers-reduced-transparency`：延续既有坍缩块——全部材质坍缩为对应**实色**
  （材质仍可影响实色色相，但零透明、零模糊、隐藏粒面与渐隐带）。
- `prefers-reduced-motion`：本轮零新增动画，既有全局抑制继续覆盖。
- 无 `backdrop-filter` 的环境：表面仍有半透明填充与边缘光，可读性不受影响（渐进增强）。
- 外观"字号/圆角"旋钮（`--control-ui-*`）与材质正交，不受影响。

## 10. 验收方式

1. **截图矩阵**（沿用 standalone HTML 直载编译后 `dist/tailwind.css` + chrome-devtools 截图法）：
   3 材质 × 亮/暗 = 6 张基准；强调色抽查 2 张（森林×极光、日落×极光）；
   悬浮输入框滚动中态 1 张（消息半压输入框）。
2. **迁移验证**：localStorage 预置 `aleph-theme=glass` → 启动后呈现 液态·暗，
   且 storage 已回写为 `dark` + `liquid`。
3. **Rust 单测**：`Material` 的 from_storage/storage_value/ALL 测试镜像 `Accent` 现有测试；
   ThemeMode glass 迁移路径测试。新测试不得引入 `web_sys` 依赖路径（host-test 红线）。
4. **构建**：panel 改动必须通过 **wasm target 构建**（不止 native `cargo check`）；
   `just wasm` + 重编 `aleph-server` 烧入 `rust_embed` 后替换部署验收。
5. **性能**：§8.5 的空闲基线对比；滚动时帧率目测无可感知退化。

## 11. 范围外（明确不做）

工作区面板重设计、设置页面重排、字体更换、WASM 体积优化、新增动画、移动端适配、
`.glass` 瞬态弹层的视觉重设计（自动继承材质原色即可）。

## 12. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 默认材质（luxe）重构后与现观感漂移 | 截图矩阵以现版本截图为对照基线，逐像素目测对齐后再调档 |
| 液态·亮 文字对比度不足 | §5.3 红线：fill 上调 + sheen/边缘补偿 |
| 顶部 mask-image 渐隐的滚动开销 | WKWebView 实测；超预算则砍渐隐保悬浮 |
| `color-mix` 百分比来自 `var()` 的兼容性 | 目标环境（WKWebView / 现代浏览器）已支持；截图验收天然覆盖 |
| 输入框高度变化导致末条消息被遮 | §6.1 padding 跟踪策略 + 显式验收标准 |
| 材质切换 View Transition 异常 | 复用既有 apply 路径；验收时三材质间互切目测 |
| `html.glass` 选择器残留 | 重构时全仓 grep `\.glass\b` 与 `html.glass` 清点（CSS 与 Rust 两侧） |

## 13. 实现锚点

| 位置 | 内容 |
|------|------|
| `interfaces/webchat/src/appearance.rs` | `Material` enum 新增；`ThemeMode` 退役 Glass + 迁移（KEY 常量区 L21-24、ThemeMode L32-77、Accent 模板 L83-150、init_appearance L397+） |
| `interfaces/webchat/src/lib.rs:46` | 引导重放路径（已存在，迁移挂此） |
| `interfaces/webchat/styles/tailwind.css` | 原色层重构主战场：@theme L12-132、暗色块 L233-308、accent 块 L310-432、`.glass` L434-508、msg-glass L510-639、reduced-transparency L641-695、atmosphere L908-1050 |
| `interfaces/webchat/src/views/chat/view.rs:136-149` | 悬浮输入框结构改动 |
| `interfaces/webchat/src/views/chat/messages.rs:164` | 滚动容器（底部 padding / 顶部渐隐） |
| `interfaces/webchat/src/views/settings/appearance.rs` | 材质选择行 |
| `interfaces/webchat/src/components/theme_toggle.rs` | 弹层材质行 |
| 情绪板 | `docs/superpowers/specs/assets/2026-06-11-glass-material/direction.html`（六 skin 数值锚点）、同目录 `composer-float.html`（悬浮布局锚点）——已从临时 brainstorm 目录复制入库 |
