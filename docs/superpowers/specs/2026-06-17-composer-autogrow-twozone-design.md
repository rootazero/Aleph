# Panel Chat 输入框：自动增高 + 双区布局 + 圆角矩形

**日期**: 2026-06-17
**范围**: `interfaces/webchat`（Panel / Leptos WASM）

## 问题

Panel chat 输入框当前是固定单行设计：`<textarea>` 虽有 `rows=1` /
`resize-none` / `min-h-[32px] max-h-[140px]`，但缺少把高度贴合内容的逻辑，
导致用户输入多行时只能在单行视口内部滚动，显示效果不友好。

同时外层容器圆角为 `--radius-2xl`（20px），在 ~44px 高的单行条上呈现
"胶囊感"，希望改为"带圆角的长方形"。

附件 / 语音 / 发送等按钮当前与 textarea 同处一行（`flex items-end`），
不适配自动增高后的视觉，需要重新布局。

## 目标

1. 输入框初始单行，随文字换行自动增高，到上限后内部滚动。
2. 外层容器改为圆角矩形（12px），不再胶囊化。
3. 按钮重新布局为双区：textarea 满宽在上，工具条在下。

## 改动范围

仅 2 个文件：

1. `interfaces/webchat/src/views/chat/composer/mod.rs` — 结构 + 自动增高逻辑
2. `interfaces/webchat/styles/tailwind.css` — 圆角（1 行）

## A. 自动增高机制

- 新增 `textarea_ref = NodeRef::<leptos::html::Textarea>::new()` 绑定到
  `<textarea>`。
- 新增一个 `Effect`，**追踪 `input_text` 信号**：先将 `style.height = "auto"`，
  再读取 `scroll_height` 并写回 `style.height = "{sh}px"`。CSS 的
  `max-h-[140px]` 负责封顶，新增 `overflow-y-auto` 负责超出上限后内部滚动。
- **设计理由（追踪信号而非仅监听 DOM `on:input`）**：`input_text` 在多处
  被程序化改写（发送后清空、retry 回填、`draft_seed`、斜杠/@ 补全选中、
  清除按钮、队列回放）。监听信号让所有这些路径统一触发增高/回缩，避免
  "清空后高度不回弹"的不一致。
- 既有 `stack_ref` 的 `ResizeObserver` → `--composer-clearance` 链路会随
  composer 整体高度变化自动跟随，无需改动。

此机制是 DOM 副作用，无可宿主单测的纯逻辑（符合项目 cargo 节制约束），
验证依赖 wasm 构建 + 部署目测。

## B. 双区布局重构（`InputArea` view 末段）

容器从单行 `flex items-end gap-2 px-3 py-1.5` 改为竖向两区
`flex flex-col gap-1.5 px-3 py-2`：

```
.aleph-composer (flex flex-col gap-1.5 px-3 py-2)
├─ <input type=file hidden>           （隐藏，位置无关）
├─ <textarea w-full ...>              ← 第一区：满宽，自动增高
└─ <div flex items-center gap-2>      ← 第二区：工具条
   ├─ 📎 attach button
   ├─ 🎤 VoiceInputButton
   └─ <div ml-auto flex items-center gap-2>   ← 右簇
      ├─ <Show clear> ✕
      ├─ <Show queue> ➕
      ├─ <Show stop>  ⏹
      └─ <Show send>  ➤
```

- textarea 去掉 `flex-1`，改 `w-full`；`min-h-[32px] max-h-[140px]` 保留，
  追加 `overflow-y-auto`。
- 所有 `<Show>` 条件、`on:click`、`send_message` / `enqueue_message` /
  `on_abort` 等闭包**逻辑零改动**，仅搬运 DOM 位置。
- 图标按钮形状不动（圆形 icon button 是标准做法；"胶囊感"投诉针对外层
  容器而非按钮）。

## C. 圆角

`tailwind.css:1516`：`border-radius: var(--radius-2xl)`（20px）→
`var(--radius-lg)`（12px）。`.aleph-composer` 的边框 / 阴影 / focus 环
（`:hover`、`:focus-within`）全部不动。

## 不做（YAGNI）

- 不改按钮图标 / 形状，不改发送 / 排队 / 中止的行为逻辑。
- 不动 `--composer-clearance` 机制、附件预览条、斜杠 / @ 补全。
- 不引入新依赖。

## 验证

- `cargo build -p aleph-panel --target wasm32-unknown-unknown`（构建通过）。
- 部署后目测：
  - 空输入 → 单行高度。
  - 多行换行 → 自动增高至 140px 上限后内部滚动。
  - 发送 / 清除 / retry / draft seed → 高度正确回缩。
  - 容器呈圆角矩形（12px），工具条在 textarea 下方，左附件+语音 /
    右发送簇。
