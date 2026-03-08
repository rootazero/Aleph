# WebChat Rust Rewrite Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Markdown rendering with syntax highlighting to the existing Panel ChatView, then delete the TypeScript WebChat.

**Architecture:** The Panel already has a complete ChatView (messages, streaming, sidebar, command palette, attachments). The only gap is Markdown/code rendering. We add `pulldown-cmark` for Markdown parsing and `syntect` for syntax highlighting, build a `MarkdownRenderer` component, and replace the plain-text rendering in `MessageBubble`.

**Tech Stack:** Leptos 0.8, pulldown-cmark, syntect (WASM-compatible), Tailwind CSS

---

### Task 1: Add Markdown dependencies

**Files:**
- Modify: `apps/panel/Cargo.toml`

**Step 1: Add pulldown-cmark and syntect to Cargo.toml**

Add after line 37 (`futures = "0.3"`):

```toml
# Markdown rendering
pulldown-cmark = { version = "0.12", default-features = false, features = ["html"] }

# Syntax highlighting (WASM-compatible subset)
syntect = { version = "5", default-features = false, features = ["default-syntaxes", "regex-onig"] }
```

**Step 2: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`

If `syntect` fails to compile for WASM (onig regex dependency), switch to:
```toml
syntect = { version = "5", default-features = false, features = ["default-syntaxes", "regex-fancy"] }
```

If still fails, use `default-fancy` or drop syntect and use CSS-class-based highlighting:
```toml
syntect = { version = "5", default-features = false, features = ["default-fancy"] }
```

**Step 3: Commit**

```bash
git add apps/panel/Cargo.toml
git commit -m "panel: add pulldown-cmark and syntect dependencies"
```

---

### Task 2: Create Markdown renderer component

**Files:**
- Create: `apps/panel/src/components/markdown.rs`
- Modify: `apps/panel/src/components/mod.rs` (add `pub mod markdown;`)

**Context:** This component takes a `&str` of Markdown content and renders it as styled HTML. It uses `pulldown-cmark` to parse Markdown and `syntect` to highlight code blocks. The output is set via `innerHTML` on a container div (pulldown-cmark outputs trusted HTML from our own content, not user-supplied external HTML).

**Step 1: Create the markdown module**

Create `apps/panel/src/components/markdown.rs`:

```rust
//! Markdown rendering with syntax highlighting for chat messages.

use leptos::prelude::*;
use pulldown_cmark::{Parser, Options, Event, Tag, TagEnd, CodeBlockKind};
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;
use std::sync::LazyLock;

/// Shared syntax/theme sets (loaded once, reused across renders).
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Render Markdown text to HTML with syntax-highlighted code blocks.
fn render_markdown(input: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(input, opts);

    let mut html_output = String::new();
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let l = lang.to_string();
                        if l.is_empty() { None } else { Some(l) }
                    }
                    CodeBlockKind::Indented => None,
                };
                code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                let highlighted = highlight_code(&code_block_content, code_block_lang.as_deref());
                let lang_label = code_block_lang.as_deref().unwrap_or("");
                html_output.push_str(&format!(
                    "<div class=\"code-block-wrapper relative group my-3\">\
                     <div class=\"flex items-center justify-between px-3 py-1.5 \
                          bg-surface-sunken/50 rounded-t-lg border border-b-0 border-border \
                          text-xs text-text-tertiary\">\
                       <span>{lang_label}</span>\
                       <button class=\"copy-btn opacity-0 group-hover:opacity-100 transition-opacity \
                                px-2 py-0.5 rounded text-text-secondary hover:text-text-primary \
                                hover:bg-surface-raised\" \
                               onclick=\"navigator.clipboard.writeText(this.closest('.code-block-wrapper')\
                               .querySelector('code').textContent)\">\
                         Copy\
                       </button>\
                     </div>\
                     <pre class=\"rounded-b-lg border border-border bg-surface-sunken \
                          overflow-x-auto p-3 text-sm leading-relaxed\">\
                       <code>{highlighted}</code>\
                     </pre>\
                     </div>"
                ));
                code_block_lang = None;
            }
            Event::Text(text) if code_block_lang.is_some() || !code_block_content.is_empty() => {
                // Inside a code block — accumulate text
                // Note: need to check if we're actually in a code block
                code_block_content.push_str(&text);
            }
            _ => {
                // For all other events, use pulldown-cmark's built-in HTML renderer
                // by collecting remaining events
                pulldown_cmark::html::push_html(
                    &mut html_output,
                    std::iter::once(event),
                );
            }
        }
    }

    html_output
}

/// Highlight a code snippet using syntect. Falls back to escaped plain text.
fn highlight_code(code: &str, lang: Option<&str>) -> String {
    let ss = &*SYNTAX_SET;
    let ts = &*THEME_SET;

    // Try to find syntax by language token
    let syntax = lang
        .and_then(|l| ss.find_syntax_by_token(l))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    // Use a dark-friendly theme
    let theme = ts.themes.get("base16-ocean.dark")
        .unwrap_or_else(|| ts.themes.values().next().unwrap());

    highlighted_html_for_string(code, ss, syntax, theme)
        .unwrap_or_else(|_| html_escape(code))
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}

/// Leptos component that renders Markdown content.
#[component]
pub fn MarkdownRenderer(
    /// Raw Markdown text to render.
    content: String,
) -> impl IntoView {
    let html = render_markdown(&content);
    // Use innerHTML since pulldown-cmark output is trusted (our own content)
    view! {
        <div class="markdown-body prose prose-sm dark:prose-invert max-w-none" inner_html=html />
    }
}
```

**Step 2: Register the module**

Add to `apps/panel/src/components/mod.rs`:
```rust
pub mod markdown;
```

**Step 3: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`

**Step 4: Commit**

```bash
git add apps/panel/src/components/markdown.rs apps/panel/src/components/mod.rs
git commit -m "panel: add MarkdownRenderer component with syntax highlighting"
```

---

### Task 3: Integrate MarkdownRenderer into MessageBubble

**Files:**
- Modify: `apps/panel/src/views/chat/view.rs:91-177`

**Context:** Replace the plain-text `<div class="whitespace-pre-wrap ...">` in `MessageBubble` with `<MarkdownRenderer content=... />`. Only render Markdown for assistant messages; user messages stay as plain text.

**Step 1: Add import**

At top of `view.rs`, add:
```rust
use crate::components::markdown::MarkdownRenderer;
```

**Step 2: Replace plain text rendering**

In `MessageBubble` component (around line 164-167), replace:

```rust
// Message content
<div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
    {content}
</div>
```

With:

```rust
// Message content
{if is_user {
    view! {
        <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
            {content.clone()}
        </div>
    }.into_any()
} else {
    view! {
        <MarkdownRenderer content=content.clone() />
    }.into_any()
}}
```

**Step 3: Verify compilation and test**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`

Then build and test visually:
```bash
cd apps/panel && npx @tailwindcss/cli -i styles/tailwind.css -o dist/tailwind.css --minify
trunk build --release
```

**Step 4: Commit**

```bash
git add apps/panel/src/views/chat/view.rs
git commit -m "panel: render assistant messages as Markdown with syntax highlighting"
```

---

### Task 4: Add Markdown/code block CSS styles

**Files:**
- Modify: `apps/panel/styles/tailwind.css`

**Context:** Add styles for Markdown prose elements (headings, lists, links, tables, blockquotes) and code blocks that work with both light and dark themes.

**Step 1: Add markdown styles**

Append to `apps/panel/styles/tailwind.css` (after existing content):

```css
/* ── Markdown prose ────────────────────────────────────── */

.markdown-body h1 { @apply text-lg font-bold mt-4 mb-2; }
.markdown-body h2 { @apply text-base font-bold mt-3 mb-2; }
.markdown-body h3 { @apply text-sm font-bold mt-2 mb-1; }
.markdown-body p { @apply my-1.5 leading-relaxed; }
.markdown-body ul { @apply list-disc pl-5 my-1.5; }
.markdown-body ol { @apply list-decimal pl-5 my-1.5; }
.markdown-body li { @apply my-0.5; }
.markdown-body a { @apply text-primary underline hover:text-primary/80; }
.markdown-body blockquote {
  @apply border-l-2 border-border pl-3 my-2 text-text-secondary italic;
}
.markdown-body table { @apply border-collapse my-2 text-sm; }
.markdown-body th { @apply border border-border px-3 py-1.5 bg-surface-sunken font-medium text-left; }
.markdown-body td { @apply border border-border px-3 py-1.5; }
.markdown-body code:not(pre code) {
  @apply px-1.5 py-0.5 rounded bg-surface-sunken text-xs font-mono;
}
.markdown-body hr { @apply border-border my-3; }
.markdown-body img { @apply rounded-lg max-w-full my-2; }

/* Task list checkboxes */
.markdown-body input[type="checkbox"] {
  @apply mr-1.5 accent-primary;
}

/* Code block wrapper — syntax highlighting overrides */
.code-block-wrapper pre {
  @apply !bg-surface-sunken;
}
.dark .code-block-wrapper pre {
  @apply !bg-[#2b303b];
}
```

**Step 2: Rebuild Tailwind**

```bash
cd apps/panel && npx @tailwindcss/cli -i styles/tailwind.css -o dist/tailwind.css --minify
```

**Step 3: Commit**

```bash
git add apps/panel/styles/tailwind.css apps/panel/dist/tailwind.css
git commit -m "panel: add Markdown prose and code block styles"
```

---

### Task 5: Add auto-scroll to bottom on new messages

**Files:**
- Modify: `apps/panel/src/views/chat/view.rs` (MessageList component, lines 67-89)

**Context:** When new messages arrive or content is streaming, the message list should auto-scroll to the bottom. Use a `NodeRef` on the scroll container and an `Effect` that watches message changes.

**Step 1: Add auto-scroll logic**

In the `MessageList` component, add a `NodeRef` and scroll effect:

```rust
#[component]
fn MessageList() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let scroll_ref = NodeRef::<leptos::html::Div>::new();

    // Auto-scroll to bottom when messages change or during streaming
    Effect::new(move || {
        // Track message list changes (triggers on any message update)
        let _msgs = chat.messages.get();
        let _phase = chat.phase.get();
        // Scroll to bottom
        if let Some(el) = scroll_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            el.set_scroll_top(el.scroll_height());
        }
    });

    view! {
        <div node_ref=scroll_ref class="flex-1 overflow-y-auto px-4 py-6 space-y-4">
            // ... existing For loop and Thinking indicator ...
        </div>
    }
}
```

**Step 2: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`

**Step 3: Commit**

```bash
git add apps/panel/src/views/chat/view.rs
git commit -m "panel: auto-scroll chat to bottom on new messages"
```

---

### Task 6: Build, test, and rebuild WASM dist

**Files:**
- Modify: `apps/panel/dist/` (rebuilt artifacts)

**Step 1: Full build**

```bash
just wasm
```

Or manually:
```bash
cd apps/panel
npx @tailwindcss/cli -i styles/tailwind.css -o dist/tailwind.css --minify
trunk build --release
```

**Step 2: Test locally**

Start the server:
```bash
cargo run --bin aleph
```

Open the Panel dashboard, navigate to Chat, send a message that includes Markdown (code blocks, lists, bold text). Verify:
- [ ] Markdown renders correctly (headings, lists, bold, italic)
- [ ] Code blocks have syntax highlighting
- [ ] Copy button appears on hover over code blocks
- [ ] Auto-scroll works during streaming
- [ ] Dark mode renders correctly
- [ ] Existing features still work (send, abort, attachments, command palette)

**Step 3: Commit rebuilt dist**

```bash
git add apps/panel/dist/
git commit -m "panel: rebuild WASM dist with Markdown rendering"
```

---

### Task 7: Delete TypeScript WebChat

**Files:**
- Delete: `apps/webchat/` (entire directory)

**Step 1: Verify webchat is not referenced anywhere**

```bash
grep -r "webchat" --include="*.rs" --include="*.toml" --include="*.yml" core/ apps/ .github/
```

If any references exist, update them first.

**Step 2: Delete the directory**

```bash
rm -rf apps/webchat/
```

**Step 3: Update workspace Cargo.toml if needed**

Check if `apps/webchat` is listed as a workspace member:
```bash
grep -n "webchat" Cargo.toml
```

Remove if present.

**Step 4: Commit**

```bash
git add -A apps/webchat/
git commit -m "cleanup: remove TypeScript webchat (replaced by Panel /chat)"
```

---

## Summary

| Task | What | LOC |
|------|------|-----|
| 1 | Add dependencies | ~3 |
| 2 | MarkdownRenderer component | ~120 |
| 3 | Integrate into MessageBubble | ~15 |
| 4 | CSS styles | ~30 |
| 5 | Auto-scroll | ~15 |
| 6 | Build and test | 0 |
| 7 | Delete webchat | -993 |
| **Total** | **Net change** | **~-810** |

The project gains Markdown rendering and syntax highlighting while **deleting more code than it adds**.
