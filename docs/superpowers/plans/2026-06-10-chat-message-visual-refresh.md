# Chat 消息流视觉刷新 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 聊天消息流（气泡/markdown/代码块/工具卡/思考面板/步骤条/空状态/分隔线/错误）从平面旧风格升级为与 chrome 一致的玻璃设计语言。

**Architecture:** 仿玻璃（零新增 `backdrop-filter`）：新增 `--msg-glass-*` / `--glass-inset-*` CSS token（四主题状态：light 默认 / `.dark` / `html.glass` / System 模式媒体查询）+ 四个组件类（`.msg-glass` / `.msg-glass-user` / `.msg-glass-danger` / `.glass-inset`），Rust 侧只做定点 class 替换；代码块内联 Tailwind 类收敛为语义类（`render_markdown` / `render_streaming` 两处同步）。

**Tech Stack:** Leptos 0.8 (WASM CSR)、Tailwind CSS 4.2（`@tailwindcss/cli` 编译）、pulldown-cmark + syntect（不动）。

**Spec:** `docs/superpowers/specs/2026-06-10-chat-message-visual-refresh-design.md`

---

## 全局注意事项（每个任务都适用）

1. **Worktree 隔离**（用户要求）：全部开发在 git worktree 内进行。⚠️ 项目雷区：worktree 会话内**只合并不删除**——`git worktree remove` 会永久损坏 Shell CWD；清理留给新会话。
2. **fmt hook 雷区**（来自项目经验）：本机 PostToolUse hook 可能对全仓跑 `cargo fmt`。每次提交前 `git status` 核对——若出现与本任务无关的文件 churn，用 `git checkout -- <无关文件>` 还原后再 `git add` 指定文件（**禁用 `git add -A`**）。
3. **测试 host 安全红线**：`markdown.rs` 中 `highlight_code(lang 非空)` 会调 `is_dark_mode()` → `web_sys::window()`，在宿主机测试中 **panic**。所有新测试只能用**无语言围栏**（` ``` ` 裸三反引号）测 `render_markdown`，转义回归用 `render_streaming`（全程不碰 web_sys）。
4. **CSS 构建**：改完 `styles/tailwind.css` 或 Rust 中的类名后需重编 CSS：`cd interfaces/webchat && npm run build:css`。`dist/tailwind.css` 是 `rust_embed` 的嵌入源，**生成产物也要提交**。
5. Panel crate 名为 `aleph-panel`（测试命令 `cargo test -p aleph-panel --lib` 在宿主机跑纯逻辑测试）。

---

### Task 0: Worktree 准备

**Files:** 无代码改动。

- [ ] **Step 1: 用 superpowers:using-git-worktrees 技能创建隔离 worktree**

分支名：`chat-message-visual-refresh`，基于本地 `main`（⚠️ 经验教训：若工具基于 `origin/main` 创建而落后本地 main，需 `git reset --hard main`）。

- [ ] **Step 2: 验证基线**

```bash
cargo check -p aleph-panel
```
Expected: 编译通过（基线干净）。

---

### Task 1: 玻璃 token + 四个组件类（CSS）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`
  - 在 `.aleph-blur-subtle` 规则（~:508）之后插入新 section
  - 在 `@media (prefers-reduced-transparency: reduce)` 块（~:516-540）内追加回退

- [ ] **Step 1: 确认依赖 token 存在**

```bash
cd interfaces/webchat && grep -n -- "--color-danger:" styles/tailwind.css && grep -n -- "--color-success:" styles/tailwind.css && grep -n -- "--radius-sm:" styles/tailwind.css
```
Expected: 三个 token 均有定义行。若缺失，在新 section 中先补（按既有 oklch 风格）。

- [ ] **Step 2: 插入消息玻璃 section**

在 `.aleph-blur-subtle { ... }` 规则结束之后、`prefers-reduced-transparency` 注释块之前插入：

```css
/* ============================================================
   Message glass — faux-glass materials for the chat message flow.
   NO backdrop-filter by design: bubbles sit in the scroll flow over a
   static panel background, so blur would cost compositor time for an
   invisible effect (hundreds of bubbles × per-frame cost). The look is
   layered instead: translucent fill + 160° sheen + bright top edge +
   soft shadow. Real backdrop-filter stays reserved for transient
   surfaces (.glass on modals/menus).
   Token states: light default → .dark / html.glass overrides → a
   media-query branch for System mode (no class on <html>), mirroring
   the reduced-transparency block below.
   ============================================================ */
:root {
  --msg-glass-bg: oklch(1 0 0 / 0.62);
  --msg-glass-sheen: linear-gradient(160deg, oklch(1 0 0 / 0.35), transparent 42%);
  --msg-glass-border: oklch(0.40 0.02 295 / 0.14);
  --msg-glass-border-top: oklch(0.40 0.02 295 / 0.20);
  --msg-glass-shadow: 0 4px 16px oklch(0.35 0.02 295 / 0.10);
  --glass-inset-bg: oklch(0.45 0.02 295 / 0.06);
  --glass-inset-border: oklch(0.40 0.02 295 / 0.12);
  --glass-inset-border-top: oklch(0.40 0.02 295 / 0.16);
  --code-header-bg: oklch(0.45 0.02 295 / 0.08);
  --code-pre-bg: oklch(0.45 0.02 295 / 0.05);
}
.dark {
  --msg-glass-bg: oklch(0.24 0.02 310 / 0.75);
  --msg-glass-sheen: linear-gradient(160deg, oklch(1 0 0 / 0.05), transparent 42%);
  --msg-glass-border: oklch(1 0 0 / 0.10);
  --msg-glass-border-top: oklch(1 0 0 / 0.22);
  --msg-glass-shadow: 0 4px 16px oklch(0 0 0 / 0.25);
  --glass-inset-bg: oklch(0.19 0.02 310 / 0.7);
  --glass-inset-border: oklch(1 0 0 / 0.07);
  --glass-inset-border-top: oklch(1 0 0 / 0.13);
  --code-header-bg: oklch(0.19 0.02 310 / 0.9);
  --code-pre-bg: oklch(0.15 0.02 310 / 0.85);
}
/* Glass theme: dark-adjacent values with a brighter specular top edge. */
html.glass {
  --msg-glass-bg: oklch(0.24 0.02 310 / 0.65);
  --msg-glass-sheen: linear-gradient(160deg, oklch(1 0 0 / 0.07), transparent 42%);
  --msg-glass-border: oklch(1 0 0 / 0.12);
  --msg-glass-border-top: oklch(1 0 0 / 0.35);
  --msg-glass-shadow: 0 4px 16px oklch(0 0 0 / 0.30);
  --glass-inset-bg: oklch(0.19 0.02 310 / 0.6);
  --glass-inset-border: oklch(1 0 0 / 0.09);
  --glass-inset-border-top: oklch(1 0 0 / 0.18);
  --code-header-bg: oklch(0.17 0.02 310 / 0.85);
  --code-pre-bg: oklch(0.14 0.02 310 / 0.8);
}
/* System mode (no explicit class) + OS dark — same values as .dark. */
@media (prefers-color-scheme: dark) {
  :root:not(.light):not(.dark):not(.glass) {
    --msg-glass-bg: oklch(0.24 0.02 310 / 0.75);
    --msg-glass-sheen: linear-gradient(160deg, oklch(1 0 0 / 0.05), transparent 42%);
    --msg-glass-border: oklch(1 0 0 / 0.10);
    --msg-glass-border-top: oklch(1 0 0 / 0.22);
    --msg-glass-shadow: 0 4px 16px oklch(0 0 0 / 0.25);
    --glass-inset-bg: oklch(0.19 0.02 310 / 0.7);
    --glass-inset-border: oklch(1 0 0 / 0.07);
    --glass-inset-border-top: oklch(1 0 0 / 0.13);
    --code-header-bg: oklch(0.19 0.02 310 / 0.9);
    --code-pre-bg: oklch(0.15 0.02 310 / 0.85);
  }
}

@layer components {
  /* Assistant bubble — faux glass. Text colour comes from the caller's
     text-* utility; this class owns only the material. */
  .msg-glass {
    background-image: var(--msg-glass-sheen);
    background-color: var(--msg-glass-bg);
    border: 1px solid var(--msg-glass-border);
    border-top-color: var(--msg-glass-border-top);
    box-shadow: var(--msg-glass-shadow);
  }
  /* User bubble — accent-tinted glass. Every colour derives from
     --color-primary via color-mix so the four accent palettes
     (ocean/forest/sunset/rose) follow automatically. */
  .msg-glass-user {
    color: white;
    background-image: linear-gradient(160deg, oklch(1 0 0 / 0.10), transparent 45%);
    background-color: color-mix(in oklch, var(--color-primary) 68%, transparent);
    border: 1px solid color-mix(in oklch, var(--color-primary) 45%, transparent);
    border-top-color: color-mix(in oklch, white 28%, var(--color-primary));
    box-shadow: 0 4px 14px color-mix(in oklch, var(--color-primary) 25%, transparent);
  }
  /* Errored final answer — danger-tinted glass (same recipe, danger hue). */
  .msg-glass-danger {
    background-image: linear-gradient(160deg, oklch(1 0 0 / 0.06), transparent 45%);
    background-color: color-mix(in oklch, var(--color-danger) 16%, transparent);
    border: 1px solid color-mix(in oklch, var(--color-danger) 35%, transparent);
  }
  /* Inset surface — the lightest tier, for second-level surfaces nested
     inside bubbles (tool cards, reasoning panel, step strip, date pill,
     hero chips). Material hierarchy: panel atmosphere → msg-glass →
     glass-inset. */
  .glass-inset {
    background-color: var(--glass-inset-bg);
    border: 1px solid var(--glass-inset-border);
    border-top-color: var(--glass-inset-border-top);
  }
}
```

- [ ] **Step 3: 在 reduced-transparency 块内追加回退**

在现有 `@media (prefers-reduced-transparency: reduce) { ... }` 块（`html.glass { ... }` 子规则之后、块结束 `}` 之前）追加：

```css
  /* Message-flow faux glass → opaque solids. */
  .msg-glass {
    background-color: var(--color-surface-raised) !important;
    background-image: none !important;
    box-shadow: none !important;
  }
  .msg-glass-user {
    background-color: var(--color-primary) !important;
    background-image: none !important;
    box-shadow: none !important;
  }
  .msg-glass-danger {
    background-color: var(--color-danger-subtle) !important;
    background-image: none !important;
  }
  .glass-inset { background-color: var(--color-surface-sunken) !important; }
```

⚠️ 若 Step 1 发现 `--color-danger-subtle` 不存在（只有 `bg-danger-subtle` 工具类映射其他 token 名），用 grep 找到 `bg-danger-subtle` 实际映射的变量名替换。

- [ ] **Step 4: 重编 CSS 并验证类已生成**

```bash
cd interfaces/webchat && npm run build:css && grep -c "msg-glass" dist/tailwind.css
```
Expected: 构建成功，grep 计数 ≥ 4。

- [ ] **Step 5: Commit**

```bash
git status   # 核对只有 tailwind.css + dist/tailwind.css 变更
git add interfaces/webchat/styles/tailwind.css interfaces/webchat/dist/tailwind.css
git commit -m "panel: add message glass tokens and component classes"
```

---

### Task 2: 代码块语义类收敛（markdown.rs，TDD）

**Files:**
- Modify: `interfaces/webchat/src/components/markdown.rs:55-57`（render_markdown 的 HTML 字符串）、`:178-180`（render_streaming 的 HTML 字符串）
- Test: 同文件新增 `#[cfg(test)] mod tests`（文件末尾）

- [ ] **Step 1: 写失败测试**

在 `markdown.rs` 文件末尾追加（私有函数同文件测试模块可直接访问）：

```rust
#[cfg(test)]
mod tests {
    use super::{render_markdown, render_streaming};

    // ⚠️ Host-test safety: render_markdown with a *language-tagged* fence
    // calls is_dark_mode() → web_sys::window(), which panics off-wasm.
    // Markdown-side tests therefore use bare ``` fences only (the empty
    // lang takes highlight_code's early escape path); the info-string
    // escape regression is covered via render_streaming, which never
    // touches web_sys.

    #[test]
    fn markdown_code_block_emits_semantic_classes() {
        let html = render_markdown("```\nlet x = 1;\n```");
        assert!(html.contains(r#"<div class="code-block-wrapper">"#));
        assert!(html.contains(r#"<div class="code-block-header">"#));
        assert!(html.contains(r#"<button class="copy-btn""#));
        assert!(html.contains("<pre><code>"));
        // legacy inline utility soup must be gone
        assert!(!html.contains("bg-surface-sunken"));
    }

    #[test]
    fn streaming_code_block_matches_semantic_classes() {
        let html = render_streaming("```rust\nlet x = 1;\n");
        assert!(html.contains(r#"<div class="code-block-wrapper">"#));
        assert!(html.contains(r#"<div class="code-block-header">"#));
        // streaming variant has no copy button
        assert!(!html.contains("copy-btn"));
        // unclosed fence is auto-closed
        assert!(html.ends_with("</code></pre></div>"));
    }

    #[test]
    fn streaming_escapes_fence_info_string() {
        let html = render_streaming("```<script>alert(1)</script>\ncode\n```");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p aleph-panel --lib markdown
```
Expected: 3 个新测试 FAIL（断言 `code-block-header` 不存在等）。

- [ ] **Step 3: 替换 render_markdown 的 HTML 字符串**

`markdown.rs:55-57` 的 `html_output.push_str(&format!(...))` 整体替换为（onclick JS 原样保留）：

```rust
                html_output.push_str(&format!(
                    r#"<div class="code-block-wrapper"><div class="code-block-header"><span>{lang_label}</span><button class="copy-btn" onclick="navigator.clipboard.writeText(this.closest('.code-block-wrapper').querySelector('code').textContent);var b=this;if(b._t)clearTimeout(b._t);b.textContent='Copied!';b.classList.add('copied');b._t=setTimeout(function(){{b.textContent='Copy';b.classList.remove('copied')}},1500)">Copy</button></div><pre><code>{highlighted}</code></pre></div>"#,
                ));
```

- [ ] **Step 4: 替换 render_streaming 的 HTML 字符串**

`markdown.rs:178-180` 的 `html.push_str(&format!(...))` 整体替换为：

```rust
                html.push_str(&format!(
                    r#"<div class="code-block-wrapper"><div class="code-block-header"><span>{lang_label}</span></div><pre><code>"#,
                ));
```

- [ ] **Step 5: 跑测试确认通过**

```bash
cargo test -p aleph-panel --lib markdown
```
Expected: 3 PASS。

- [ ] **Step 6: 补充代码块语义 CSS**

`tailwind.css` 中现有 `/* Code block - syntect inline styles handle colors, we just handle layout */` 注释 + `.code-block-wrapper pre { overflow-x: auto; }`（markdown prose 区末尾，~:678-679）整体替换为：

```css
/* ── Code block — refined header bar on faux glass ── */
.code-block-wrapper {
  position: relative;
  margin: 0.75rem 0;
  border-radius: 10px;
  border: 1px solid var(--glass-inset-border);
  border-top-color: var(--glass-inset-border-top);
  overflow: hidden;
}
.code-block-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.3rem 0.75rem;
  background-image: linear-gradient(180deg, oklch(1 0 0 / 0.05), transparent);
  background-color: var(--code-header-bg);
  border-bottom: 1px solid var(--glass-inset-border);
  font-size: 0.75rem;
  color: var(--color-text-tertiary);
}
.code-block-wrapper pre {
  margin: 0;
  padding: 0.75rem;
  background: var(--code-pre-bg);
  overflow-x: auto;
  font-size: 0.875rem;
  line-height: 1.5;
}
.copy-btn {
  opacity: 0;
  padding: 0.125rem 0.5rem;
  border-radius: var(--radius-full);
  border: 1px solid var(--glass-inset-border);
  color: var(--color-text-secondary);
  transition: opacity 0.2s, color 0.2s, background-color 0.2s;
}
.code-block-wrapper:hover .copy-btn,
.copy-btn:focus-visible { opacity: 1; }
.copy-btn:hover { color: var(--color-text-primary); background-color: var(--glass-inset-bg); }
.copy-btn.copied { color: var(--color-success); }
```

- [ ] **Step 7: 重编 CSS + 全量测试**

```bash
cd interfaces/webchat && npm run build:css && cd ../.. && cargo test -p aleph-panel --lib
```
Expected: CSS 构建成功；全部测试 PASS（含既有 tool_card / reasoning / run_id 测试）。

- [ ] **Step 8: Commit**

```bash
git status   # 核对无 fmt hook 带来的无关 churn
git add interfaces/webchat/src/components/markdown.rs interfaces/webchat/styles/tailwind.css interfaces/webchat/dist/tailwind.css
git commit -m "panel: consolidate code block markup into semantic glass classes"
```

---

### Task 3: Markdown 排版刷新（纯 CSS）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:628-676`（`.markdown-body` 全部规则，保留 :628 的 min-width 约束注释与规则）

- [ ] **Step 1: 替换 `.markdown-body` 规则块**

保留开头的容器约束（注释 + `.markdown-body { min-width: 0; max-width: 100%; }`），其后到 `input[type="checkbox"]` 为止整体替换为：

```css
.markdown-body h1,
.markdown-body h2 {
  /* gradient hairline under the two top heading levels */
  background-image: linear-gradient(90deg, var(--color-border), transparent 70%);
  background-repeat: no-repeat;
  background-size: 100% 1px;
  background-position: 0 100%;
  padding-bottom: 0.35rem;
}
.markdown-body h1 { font-size: 1.25rem; font-weight: 700; letter-spacing: -0.01em; margin-top: 1.1rem; margin-bottom: 0.5rem; }
.markdown-body h2 { font-size: 1.0625rem; font-weight: 600; margin-top: 0.85rem; margin-bottom: 0.45rem; }
.markdown-body h3 { font-size: 0.9375rem; font-weight: 600; margin-top: 0.6rem; margin-bottom: 0.3rem; }
.markdown-body p { margin: 0.375rem 0; line-height: 1.625; }
.markdown-body ul { list-style-type: disc; padding-left: 1.25rem; margin: 0.375rem 0; }
.markdown-body ol { list-style-type: decimal; padding-left: 1.25rem; margin: 0.375rem 0; }
.markdown-body li { margin: 0.125rem 0; }
.markdown-body a {
  color: var(--color-primary);
  text-decoration: underline;
  text-decoration-color: color-mix(in oklch, var(--color-primary) 40%, transparent);
  text-underline-offset: 2px;
  transition: text-decoration-color 0.15s;
}
.markdown-body a:hover { text-decoration-color: var(--color-primary); }
.markdown-body blockquote {
  /* accent-tinted bar + wash; no italics (poor CJK rendering) */
  border-left: 3px solid color-mix(in oklch, var(--color-primary) 55%, transparent);
  border-radius: 2px;
  padding: 0.25rem 0 0.25rem 0.75rem;
  margin: 0.5rem 0;
  color: var(--color-text-secondary);
  background: linear-gradient(90deg, color-mix(in oklch, var(--color-primary) 7%, transparent), transparent 70%);
}
/* Tables scroll horizontally inside the bubble rather than overflowing it:
   `display:block` turns the table box into its own scroll container, capped at
   the bubble width. Long unbreakable cell content (paths/URLs) also wraps.
   Modern grid: rounded hairline shell, header fill, row separators only. */
.markdown-body table {
  display: block;
  max-width: 100%;
  overflow-x: auto;
  border-collapse: separate;
  border-spacing: 0;
  margin: 0.5rem 0;
  font-size: 0.875rem;
  border: 1px solid var(--glass-inset-border);
  border-radius: 8px;
}
.markdown-body th {
  padding: 0.375rem 0.75rem;
  background: var(--glass-inset-bg);
  font-weight: 600;
  text-align: left;
  border-bottom: 1px solid var(--color-border);
}
.markdown-body td {
  padding: 0.375rem 0.75rem;
  border-bottom: 1px solid var(--color-border-subtle);
}
.markdown-body tr:last-child td { border-bottom: none; }
.markdown-body th,
.markdown-body td { overflow-wrap: anywhere; word-break: break-word; }
.markdown-body code:not(pre code) {
  padding: 0.125rem 0.375rem;
  border-radius: var(--radius-sm);
  background: var(--glass-inset-bg);
  border: 1px solid var(--glass-inset-border);
  font-size: 0.8125rem;
  font-family: var(--font-mono);
  font-feature-settings: "calt", "zero";
}
.markdown-body hr {
  border: none;
  height: 1px;
  margin: 0.85rem 0;
  background: linear-gradient(90deg, transparent, var(--color-border), transparent);
}
.markdown-body img {
  border-radius: 0.5rem;
  max-width: 100%;
  margin: 0.5rem 0;
  border: 1px solid var(--glass-inset-border);
  box-shadow: 0 2px 10px oklch(0 0 0 / 0.12);
}
.markdown-body input[type="checkbox"] { margin-right: 0.375rem; accent-color: var(--color-primary); }
```

- [ ] **Step 2: 重编 CSS**

```bash
cd interfaces/webchat && npm run build:css
```
Expected: 构建成功。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css interfaces/webchat/dist/tailwind.css
git commit -m "panel: refresh markdown prose typography for glass language"
```

---

### Task 4: messages.rs 定点 class 替换

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs`（5 处：气泡样式 :339-356、流式光标 :420、日期分隔线 :297-306、ChatHero chips :71-74、StepStrip :656）

- [ ] **Step 1: 气泡样式三处替换（:339-356 的 `bubble_style`）**

```rust
    let bubble_style = if is_user {
        "min-w-0 max-w-[80%] rounded-2xl px-3.5 py-2 msg-glass-user"
    } else if in_strip {
        // Intermediate step inside the run's step strip — no bubble.
        if has_error {
            "min-w-0 w-full px-2 py-1 text-danger border-l-2 border-danger/40"
        } else if message.is_intermediate {
            "min-w-0 w-full px-2 py-0.5 text-text-secondary text-sm"
        } else {
            "min-w-0 w-full px-2 py-1 text-text-primary"
        }
    } else if has_error {
        // Standalone final answer that errored — keep the bubble.
        "min-w-0 max-w-[80%] rounded-2xl px-4 py-3 msg-glass-danger text-danger"
    } else {
        // Standalone final answer — the conversational reply keeps its bubble.
        "min-w-0 max-w-[80%] rounded-2xl px-4 py-3 msg-glass text-text-primary"
    };
```

（改动点：`bg-primary text-white` → `msg-glass-user`；`bg-danger-subtle text-danger border border-danger/20` → `msg-glass-danger text-danger`；`bg-surface-raised` → `msg-glass`。in_strip 三分支不动。）

- [ ] **Step 2: 流式光标（:418-424 的 `streaming_cursor`）**

```rust
    let streaming_cursor = if is_streaming {
        Some(view! {
            <span class="inline-block w-[3px] h-4 rounded-full bg-gradient-to-b from-primary to-primary/40 animate-pulse ml-0.5 align-text-bottom"></span>
        })
    } else {
        None
    };
```

- [ ] **Step 3: 日期分隔线（:296-306 的 `DaySeparator` view）**

```rust
    view! {
        <div class="flex items-center gap-3 py-1.5 select-none">
            <span class="flex-1 h-px bg-gradient-to-r from-transparent to-border/60"></span>
            <span class="px-2.5 py-0.5 rounded-full text-[10px] font-medium uppercase tracking-wider
                         text-text-tertiary glass-inset">
                {label}
            </span>
            <span class="flex-1 h-px bg-gradient-to-l from-transparent to-border/60"></span>
        </div>
    }
```

- [ ] **Step 4: ChatHero 启动器 chips（:71-74 的 button class）**

```rust
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs
                                       text-text-secondary glass-inset
                                       hover:text-text-primary hover:bg-surface-raised
                                       transition-colors"
```

（`bg-surface-raised/70 border border-border/60` 与 `hover:border-border` 由 `glass-inset` 接管。）

- [ ] **Step 5: StepStrip 容器（:656）**

```rust
            <div class="w-full rounded-lg glass-inset">
```

（替换 `"w-full rounded-lg border border-border/40 bg-surface-sunken/25"`。）

- [ ] **Step 6: 编译 + 测试 + 重编 CSS**

```bash
cargo check -p aleph-panel && cargo test -p aleph-panel --lib && cd interfaces/webchat && npm run build:css
```
Expected: 编译通过、测试全 PASS、CSS 构建成功（Tailwind 需扫描到新工具类 `w-[3px]`、`bg-gradient-to-b` 等）。

- [ ] **Step 7: Commit**

```bash
git status   # 核对无 fmt hook 无关 churn
git add interfaces/webchat/src/views/chat/messages.rs interfaces/webchat/dist/tailwind.css
git commit -m "panel: move chat bubbles and message-flow chrome to glass classes"
```

---

### Task 5: 工具卡片 + 思考面板换 glass-inset

**Files:**
- Modify: `interfaces/webchat/src/components/tool_card.rs`（4 处：:310 容器、:394 diff_view、:566/:575 default_body details）
- Modify: `interfaces/webchat/src/views/chat/reasoning.rs`（1 处：:57 容器）

- [ ] **Step 1: ToolCard 容器（tool_card.rs:310）**

```rust
        <div class="rounded-lg glass-inset hover:bg-surface-raised/30 transition-colors">
```

（替换 `"rounded-md hover:bg-surface-raised/30 transition-colors"`。状态色、折叠逻辑、headline 全部不动。）

- [ ] **Step 2: diff 容器（tool_card.rs:394）**

```rust
        <div class=format!("{MONO_BLOCK} rounded-md glass-inset overflow-x-auto")>
```

（替换 `format!("{MONO_BLOCK} rounded border border-border/60 overflow-x-auto")`。）

- [ ] **Step 3: default_body 两个 details 容器（tool_card.rs:566 与 :575）**

两处 `class="rounded-md border border-border/60 bg-surface-sunken/60"` 均替换为：

```rust
                    <details class="rounded-md glass-inset">
```

（:575 的那处保留其 `open=true` 属性。）

- [ ] **Step 4: ReasoningPanel 容器（reasoning.rs:57）**

```rust
                <div class="rounded-xl glass-inset overflow-hidden">
```

（替换 `"rounded-xl border border-border bg-surface-raised/40 overflow-hidden"`。脉冲点、tail 预览、展开逻辑全部不动。）

- [ ] **Step 5: 编译 + 测试**

```bash
cargo check -p aleph-panel && cargo test -p aleph-panel --lib
```
Expected: 编译通过、全部 PASS（tool_card 纯逻辑测试不受 class 改动影响）。

- [ ] **Step 6: Commit**

```bash
git status
git add interfaces/webchat/src/components/tool_card.rs interfaces/webchat/src/views/chat/reasoning.rs
git commit -m "panel: move tool cards and reasoning panel to glass-inset surface"
```

---

### Task 6: 全量验证 + 视觉验收 + 合并

**Files:** 无新改动（验证轮）。

- [ ] **Step 1: 全量静态检查**

```bash
cargo test -p aleph-panel --lib && cargo clippy -p aleph-panel -- -D warnings && just wasm
```
Expected: 测试全 PASS、clippy 零警告、WASM 构建成功。

- [ ] **Step 2: 按刷新链部署到本地 daemon**

⚠️ Panel 资源是编译期 `rust_embed` 嵌入——只跑 `just wasm` 看不到效果：

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
```

- [ ] **Step 3: 三主题视觉验收**

用 chrome-devtools（MCP）打开 Panel，发一条覆盖全元素的测试消息（含 h1/h2/h3、表格、带语言代码块、无语言代码块、引用、任务列表、链接、行内代码、hr），并触发一次带工具调用 + 思考过程的运行。在 **dark / light / glass** 三主题下分别截图核对：

- [ ] 助手气泡呈玻璃材质（光泽 + 亮顶边 + 柔影），文字对比度正常
- [ ] 用户气泡 accent 着色玻璃；切换 accent 色板（ocean→sunset）气泡跟随
- [ ] 代码块：玻璃标题栏 + 语言标签 + hover 出现 Copy、点击有 "Copied!" 反馈、语法高亮正常
- [ ] 表格圆角外框、无竖线、表头有填充；超宽表格在气泡内横向滚动
- [ ] 引用块 accent 缘线 + 无斜体；链接低透明度下划线 hover 变实
- [ ] 工具卡片 / 思考面板 / 步骤条 / 日期胶囊 / 空状态 chips 均为 glass-inset 材质
- [ ] 流式输出：光标为圆角渐变细条；流式→完成切换无视觉跳变
- [ ] 系统"减少透明度"开启后全部回退为不透明实底（macOS 辅助功能设置验证）

发现视觉问题 → 只调 CSS token 值（不改结构），修后重走 Step 1-2。

- [ ] **Step 4: 合并回 main（worktree 会话内只合并）**

```bash
git -C /Volumes/TBU4/Workspace/Aleph merge --no-ff chat-message-visual-refresh -m "panel: chat message flow visual refresh (glass language)"
```

合并前先 `git log --oneline main..chat-message-visual-refresh` 与 `git diff main...chat-message-visual-refresh --stat` 核对范围；若 main 有并发推进，先 merge main 入分支解冲突再 --no-ff 回 main。

⚠️ **不要在本会话执行 `git worktree remove`**（Shell CWD 损坏雷区）——worktree 清理留给新会话。

---

## Self-Review 记录

- Spec 覆盖：A 气泡材质→Task 1+4；B markdown 排版→Task 3；C 代码块→Task 2；D 嵌入表面→Task 4(分隔线/chips/StepStrip)+Task 5(工具卡/思考面板)；E 动效→Task 4 Step 2（其余"不动"项无需任务）；F 回退→Task 1 Step 3，验证→Task 6；错误消息 danger 玻璃→Task 1(.msg-glass-danger)+Task 4 Step 1。
- 类型/类名一致性：`.msg-glass` / `.msg-glass-user` / `.msg-glass-danger` / `.glass-inset` / `.code-block-header` / `.copy-btn` 在 CSS 定义与 Rust 使用处拼写一致。
- 无占位符：所有代码步骤给出完整代码；唯一条件分支（`--color-danger-subtle` 是否存在）给出了明确的处置方法。
