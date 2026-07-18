# Composer 自动增高 + 双区布局 + 圆角矩形 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel chat 输入框从固定单行改为随文字自动增高的圆角矩形，并把附件/语音/发送按钮重排为「textarea 在上、工具条在下」的双区布局。

**Architecture:** 纯 Panel (Leptos/WASM) 前端改动。自动增高用一个追踪 `input_text` 信号的 `Effect` 把 `<textarea>` 高度设为 `scroll_height`，CSS `max-h` 封顶 + `overflow-y-auto` 滚动；布局把单行 flex 容器改为竖向两区；圆角在 CSS 源里把 `--radius-2xl` 降到 `--radius-lg`。

**Tech Stack:** Rust + Leptos 0.7 (`NodeRef` / `Effect` / `web_sys`)、Tailwind CSS。

## Global Constraints

- 不引入新依赖（serde/tokio 全栈锁定；前端无新 crate）。
- 行为逻辑零改动：`send_message` / `enqueue_message` / `on_abort` / 所有 `<Show>` 条件与 `on:click` 闭包仅搬运 DOM 位置，签名与语义不变。
- 无可宿主单测的纯逻辑（DOM 副作用）；验证 = `cargo build -p aleph-panel --target wasm32-unknown-unknown` 通过 + 部署目测。遵循项目 cargo 节制：每个任务至多一次 wasm 构建。
- `docs/superpowers/` 被 `.gitignore` 忽略，spec/plan 仅落盘不提交。
- 代码注释用英文。

---

### Task 1: 圆角矩形（CSS 1 行）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:1516`

**Interfaces:**
- Consumes: 既有 `--radius-lg`（12px）token（同文件 line 95 定义）。
- Produces: 无（纯样式）。

- [ ] **Step 1: 改 border-radius**

把 `.aleph-composer` 的圆角从 2xl 降到 lg。当前（line 1512–1517）：

```css
.aleph-composer {
  background-color: var(--color-surface-raised);
  background-image: linear-gradient(to bottom, var(--aleph-sheen), transparent 52%);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-2xl);
  box-shadow: var(--shadow-md), inset 0 1px 0 var(--aleph-sheen);
```

把这一行：

```css
  border-radius: var(--radius-2xl);
```

改为：

```css
  border-radius: var(--radius-lg);
```

`.aleph-composer` 的 `:hover` / `:focus-within` / 边框 / 阴影规则一律不动。

- [ ] **Step 2: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: composer corner radius 2xl->lg (rounded rectangle)"
```

---

### Task 2: 自动增高 + 双区布局

**Files:**
- Modify: `interfaces/webchat/src/views/chat/composer/mod.rs`（`InputArea` 组件：~82 行附近加 ref，~90 行附近的 Effect 区加自动增高 Effect，741–881 行替换 view 块）

**Interfaces:**
- Consumes: 既有 `input_text: RwSignal<String>`（line 57）、既有所有按钮闭包与 `<Show>` 条件。
- Produces: 无（组件内部）。

- [ ] **Step 1: 新增 textarea NodeRef**

在 `file_input_ref` / `stack_ref` 声明旁（当前 line 82–83）：

```rust
    let file_input_ref = NodeRef::<leptos::html::Input>::new();
    let stack_ref = NodeRef::<leptos::html::Div>::new();
```

追加一行：

```rust
    let textarea_ref = NodeRef::<leptos::html::Textarea>::new();
```

- [ ] **Step 2: 新增自动增高 Effect**

紧跟在 `stack_ref` 的 `ResizeObserver` Effect（当前在 line 110 的 `}` 后）之后，插入：

```rust
    // Auto-grow the composer textarea to fit its content. We track the
    // `input_text` signal (not just the DOM `input` event) so every
    // programmatic rewrite — send-clear, retry refill, draft seed, slash/@
    // completion, clear button, queue replay — resizes too. Set height to
    // `auto` first so the box can shrink, then to `scroll_height`; CSS
    // `max-h-[140px]` caps it and `overflow-y-auto` scrolls beyond the cap.
    Effect::new(move |_| {
        let _ = input_text.get();
        if let Some(ta) = textarea_ref.get() {
            let style = ta.style();
            let _ = style.set_property("height", "auto");
            let _ = style.set_property("height", &format!("{}px", ta.scroll_height()));
        }
    });
```

- [ ] **Step 3: 替换 view 中的 composer 块**

把当前 741–881 行的整个 `<div class="aleph-composer ...">...</div>` 块替换为下面的双区版本。改动点：①容器类 `flex items-end gap-2 px-3 py-1.5` → `flex flex-col gap-1.5 px-3 py-2`；②`<textarea>` 提到按钮之前、`flex-1 min-w-0` → `w-full`、加 `overflow-y-auto`、加 `node_ref=textarea_ref`；③attach/voice + 右簇（clear/queue/stop/send）包进工具条 `<div>`，右簇用 `ml-auto`。所有内部元素、SVG、闭包逐字保留。

```rust
                // Composer card — two zones: full-width auto-grow textarea
                // on top, a toolbar row below (attach + voice on the left,
                // clear / queue / abort / send on the right). The textarea
                // grows up to 140px then scrolls internally.
                <div class="aleph-composer flex flex-col gap-1.5 px-3 py-2">
                    // Hidden file input. `accept` is a *hint* — the OS
                    // picker defaults to images, common video, plain
                    // text / markdown / pdf / json. Users can still
                    // switch to "All files" for niche types.
                    <input
                        type="file"
                        multiple=true
                        class="hidden"
                        accept="image/*,video/mp4,video/webm,video/quicktime,text/*,application/pdf,application/json,.md,.csv"
                        node_ref=file_input_ref
                        on:change=on_file_change
                    />

                    <textarea
                        class="w-full resize-none overflow-y-auto bg-transparent px-1 py-[6px] text-sm leading-snug
                               text-text-primary placeholder:text-text-tertiary
                               focus:outline-none min-h-[32px] max-h-[140px]"
                        placeholder=move || t_string!(i18n, chat.send_placeholder).to_string()
                        rows=1
                        node_ref=textarea_ref
                        prop:value=move || input_text.get()
                        on:input=move |ev| {
                            let val = event_target_value(&ev);
                            input_text.set(val.clone());
                            update_palette(&val);
                            // Read the caret from the underlying textarea DOM node.
                            let caret = ev
                                .target()
                                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                                .and_then(|ta| ta.selection_start().ok().flatten())
                                .unwrap_or(val.len() as u32) as usize;
                            let at = update_mention_palette(
                                &val,
                                caret,
                                chat.team_id.get_untracked(),
                                &chat.team_members.get_untracked(),
                                show_mention,
                                mention_members,
                                mention_selected,
                            );
                            mention_at.set(at);
                        }
                        on:keydown=on_keydown
                    />

                    // Toolbar row — left: attach + voice; right cluster: the
                    // conditional clear / queue / abort / send buttons.
                    <div class="flex items-center gap-2">
                        <button
                            class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                                   hover:bg-surface-sunken transition-colors flex-shrink-0"
                            title=move || t_string!(i18n, chat.attach).to_string()
                            on:click=on_attach_click
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5"
                                 viewBox="0 0 20 20" fill="currentColor">
                                <path fill-rule="evenodd"
                                      d="M15.621 4.379a3 3 0 0 0-4.242 0l-7 7a3 3 0 0 0 4.241 4.243h.001l.497-.5a.75.75 0 0 1 1.064 1.057l-.498.501-.002.002a4.5 4.5 0 0 1-6.364-6.364l7-7a4.5 4.5 0 0 1 6.368 6.36l-3.455 3.553A2.625 2.625 0 1 1 9.52 9.52l3.45-3.451a.75.75 0 1 1 1.061 1.06l-3.45 3.451a1.125 1.125 0 0 0 1.587 1.595l3.454-3.553a3 3 0 0 0 0-4.242Z"
                                      clip-rule="evenodd" />
                            </svg>
                        </button>

                        // Voice loop — record → STT → send → spoken reply.
                        <voice::VoiceInputButton
                            disabled=Signal::derive(move || is_sending.get())
                        />

                        <div class="ml-auto flex items-center gap-2">
                            // Clear-draft ✕ — visible only when text exists.
                            // Wipes text + closes palette + exits namespace in
                            // one click. Attachments are left alone (own ✕).
                            <Show when=move || !input_text.get().trim().is_empty()>
                                <button
                                    class="w-8 h-8 rounded-full text-text-tertiary hover:text-text-primary
                                           hover:bg-surface-sunken flex items-center justify-center
                                           transition-colors flex-shrink-0"
                                    title=move || t_string!(i18n, chat.clear).to_string()
                                    on:click=move |_| {
                                        input_text.set(String::new());
                                        show_palette.set(false);
                                        current_namespace.set(None);
                                        show_mention.set(false);
                                        mention_at.set(None);
                                    }
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
                                    </svg>
                                </button>
                            </Show>

                            // Queue button — only while a run is active. Lets the user
                            // line up a follow-up that auto-sends when the turn settles.
                            <Show when=move || chat.active_run_id.get().is_some()>
                                <button
                                    class="w-8 h-8 rounded-full bg-surface-sunken text-text-secondary
                                           flex items-center justify-center hover:bg-surface-raised
                                           hover:text-text-primary disabled:opacity-35
                                           disabled:cursor-not-allowed transition-colors flex-shrink-0"
                                    title=move || t_string!(i18n, chat.queue).to_string()
                                    disabled=move || !has_draft.get()
                                    on:click=move |_| enqueue_message()
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path fill-rule="evenodd"
                                              d="M10 3a.75.75 0 0 1 .75.75v5.5h5.5a.75.75 0 0 1 0 1.5h-5.5v5.5a.75.75 0 0 1-1.5 0v-5.5h-5.5a.75.75 0 0 1 0-1.5h5.5v-5.5A.75.75 0 0 1 10 3Z"
                                              clip-rule="evenodd" />
                                    </svg>
                                </button>
                            </Show>

                            <Show when=move || chat.active_run_id.get().is_some()>
                                <button
                                    class="w-8 h-8 rounded-full bg-danger/15 text-danger flex items-center
                                           justify-center hover:bg-danger/25 transition-colors flex-shrink-0"
                                    title=move || t_string!(i18n, chat.stop).to_string()
                                    on:click=on_abort
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <rect x="4" y="4" width="12" height="12" rx="2" />
                                    </svg>
                                </button>
                            </Show>

                            <Show when=move || chat.active_run_id.get().is_none()>
                                <button
                                    class="w-8 h-8 rounded-full bg-primary text-white flex items-center
                                           justify-center shadow-sm hover:bg-primary-hover
                                           disabled:opacity-35 disabled:cursor-not-allowed
                                           disabled:shadow-none transition-all flex-shrink-0"
                                    disabled=move || !can_send.get()
                                    on:click=move |_| send_message()
                                >
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                         stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                                         class="w-4 h-4">
                                        <path d="M12 19V5" />
                                        <path d="M5 12l7-7 7 7" />
                                    </svg>
                                </button>
                            </Show>
                        </div>
                    </div>
                </div>
```

- [ ] **Step 4: 构建验证**

Run: `cargo build -p aleph-panel --target wasm32-unknown-unknown`
Expected: 编译通过（无 `textarea_ref` 未用 / 类型错误 / 未闭合标签警告）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/composer/mod.rs
git commit -m "panel: auto-grow composer textarea + two-zone toolbar layout"
```

---

## 部署后目测（非任务，交付后人工）

- 空输入 → 单行高度（~32px）。
- 连续换行 → 容器自动增高，至 140px 上限后 textarea 内部滚动。
- 发送 / 点 ✕ 清除 / retry / 空态建议 chip 注入 → 高度正确回缩到单行。
- 容器为 12px 圆角矩形；工具条在 textarea 下方，左附件+语音、右发送簇（运行中显示排队+停止）。
- 走 `just wasm` → 重编 `aleph-server` → 替换运行中 binary 后才能看到效果（Panel 资源编译期嵌入）。

## Self-Review

- **Spec coverage:** A 自动增高 → Task 2 Step 1-2;B 双区布局 → Task 2 Step 3;C 圆角 → Task 1。全覆盖。
- **Placeholder scan:** 无 TBD/TODO；所有步骤含完整代码。
- **Type consistency:** `textarea_ref` 声明（`NodeRef::<leptos::html::Textarea>`）与 Effect / `node_ref=textarea_ref` 一致;`input_text` / 闭包名沿用既有。
