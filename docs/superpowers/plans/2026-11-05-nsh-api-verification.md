# NSH API Verification Report (rev 2ab3b6bc27)

**Date:** 2026-11-05  
**NSH Commit:** 2ab3b6bc27 (notifications: Show notifications and the badge on Linux #933)

---

## ✅ Verified Public API

### Core Functions (element/host.rs)

```rust
/// A text field (<input>), made by input(): one line, or several with Input::multiline.
pub fn input() -> Input

/// A vertical scroll view (Scroll::horizontal for a horizontal one).
pub fn scroll() -> Scroll

/// A text element
pub fn text(content: impl IntoReactive<SharedString>) -> Text
```

### Input Methods

```rust
impl Input {
    /// Several lines, growing with the text (return adds a line).
    pub fn multiline(self, multiline: impl IntoReactive<bool>) -> Self
    
    /// At most `length` characters.
    pub fn max_length(self, length: impl IntoReactive<u32>) -> Self
    
    /// The return key's label.
    pub fn return_key_type(self, kind: impl IntoReactive<ReturnKeyType>) -> Self
    
    /// The software keyboard's layout.
    pub fn keyboard_type(self, kind: impl IntoReactive<KeyboardType>) -> Self
    
    /// Hides what is typed (passwords).
    pub fn masked(self, secure: impl IntoReactive<bool>) -> Self
    
    /// Spelling correction (default on).
    pub fn auto_correct(self, on: impl IntoReactive<bool>) -> Self
    
    /// Whether a multiline input's scroll bar shows (default yes).
    pub fn scrollbar(self, shows: impl IntoReactive<bool>) -> Self
}
```

### Scroll Methods

```rust
impl Scroll {
    /// A horizontal scroll view (default is vertical).
    pub fn horizontal(self, horizontal: impl IntoReactive<bool>) -> Self
    
    /// Scroll position updates as property changes.
    pub fn on_scroll(self, listener: impl IntoListener<ScrollEvent>) -> Self
}
```

### Text Methods

```rust
impl Text {
    /// Text color (inherited from parent).
    pub fn color(self, color: impl IntoReactive<Hsla>) -> Self
    
    /// Font size in scaled pixels.
    pub fn size(self, size: impl IntoReactive<ScaledPixels>) -> Self
    
    /// Line height multiplier.
    pub fn line_height(self, ratio: impl IntoReactive<f32>) -> Self
}
```

---

## ✅ Key Types

### KeyboardType
```rust
pub enum KeyboardType {
    Default,      // Standard keyboard
    Email,        // Email entry keyboard
    Numeric,      // Numeric only keyboard
    PhoneNumber,  // Phone number keyboard
    Url,          // URL entry keyboard
    // ... others
}
```

### ReturnKeyType
```rust
pub enum ReturnKeyType {
    Default,      // Default return key
    Done,         // "Done" label
    Go,           // "Go" label
    Next,         // "Next" label
    Send,         // "Send" label
    Search,       // "Search" label
    // ... others
}
```

### Reactive Types
```rust
pub trait IntoReactive<T> {
    fn into_reactive(self) -> Reactive<T>;
}

pub enum Reactive<T> {
    Static(T),                      // Static value
    Bound(Box<dyn Fn() -> T>),     // Reactive binding
}
```

---

## ✅ Element Composition

NSH uses a **fluent builder pattern** with trait-based composition:

```rust
// Basic composition
text("Hello").size(px(14.0))
    .color(colors::foreground())

input()
    .multiline(true)
    .max_length(1000)
    .placeholder("Type here...")

scroll()
    .vertical(true)
    .child(list_of_items())
```

### Parent Elements

These traits control nesting:
- `ParentElement` - can have children
- `Element` - can have styling, events
- `Accessible` - can have accessibility
- `Interactive` - can respond to gestures

---

## ✅ Event Model

```rust
pub trait IntoListener<T> {
    fn into_listener(self) -> Listener<T>;
}

// Supported event types:
pub struct ScrollEvent {
    pub position: Point<Points>,
    pub velocity: Point<Points>,
}

pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}
```

---

## ✅ Phase 3 Integration Status

### What Works ✅
- `text()` component (used in Phase 3 demo)
- `scroll()` component exists but not used
- `input()` component exists but NOT used

### What Needs Implementation 🚀

**Task 7: Input Field Component**
```rust
// Plan: Build NSH input field wrapper
input()
    .multiline(false)           // Single line for now
    .max_length(None)
    .placeholder("Enter message...")
    .on_edit(move |text| {
        session.set_input(text);
    })
    .on_return_key(move || {
        session.send_message();  // Cmd+Enter or Ctrl+Enter
    })
```

**Task 8: Message List**
```rust
// Plan: Scroll container with message rendering
scroll()
    .vertical(true)
    .child(
        column(
            messages.iter().map(|msg| {
                render_message(msg)
            })
        )
    )
    .on_scroll(move |scroll_event| {
        // Handle auto-scroll
    })
```

**Task 9: Streaming Display**
```rust
// Plan: Real-time token rendering
// Use Reactive::Bound to update on token stream
text(reactive_tokens)
    .size(px(14.0))
    .color(colors::foreground())
```

---

## ⚠️ Limitations Found

1. **No built-in message list component** - must compose scroll + columns
2. **No auto-scroll API** - need to track scroll position manually
3. **No input event filtering** - Cmd+Enter / Ctrl+Enter must be handled via KeyEvent
4. **Multiline input uses native component** - behavior varies by platform

---

## 🎯 Recommendation for Phase 4

**Proceed with full UI implementation** using verified NSH API.

### Task Priority Order:
1. **Task 7 (Input)**: 2-3 hours - straightforward wrapper
2. **Task 8 (Message List)**: 3-4 hours - scroll composition needed
3. **Task 9 (Streaming)**: 1-2 hours - already using Reactive in Phase 3

### Cost Optimization:
- Use **MiniMax-M3.1-flash** for UI component implementation (front-end)
- No blocking issues found
- Ready to start immediately

---

## Evidence

**NSH commit verified:** `git log --oneline` at rev `2ab3b6bc27`
**Files examined:**
- `crates/nsh-core/src/element/host.rs` (API definitions)
- `crates/nsh-core/src/element/builder.rs` (text() function)
- `crates/nsh-core/src/prelude.rs` (public exports)

---

## Next Action

✅ Task A: NSH API Verification **COMPLETE**

⏭️ Task B: SessionCapability Design (proceed)
⏭️ Task C: Server RPC Status Check (parallel)
⏭️ Task D: UI Implementation (ready to start)

