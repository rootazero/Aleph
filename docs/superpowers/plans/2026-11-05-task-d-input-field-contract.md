# Task D: Input Field Component Contract (Corrected)

**Date:** 2026-11-05  
**Clarification:** Task D asked for disabled state and API corrections

---

## Corrected Signature

```rust
pub fn input_field(
    value: impl IntoReactive<String>,
    disabled: impl IntoReactive<bool>,
    on_change: impl IntoListener<String, M>,
    on_submit: impl IntoListener<(), M>,
) -> impl Element
```

Or builder pattern if preferred by task-d.

---

## Key Points

1. **Disabled state:** Tracks `SessionManager.is_generating` during response
2. **NSH APIs:** Use actual `IntoListener<E, M>` generic (not simplified spellings)
3. **Scope:** Includes `main.rs` integration + message model changes
4. **Compilation:** Will uncomment/enable `mod components` in main.rs

---

## Integration Points

- `desktop/nsh-shell/src/components/input_field.rs`
- `desktop/nsh-shell/src/components/message_list.rs`
- `desktop/nsh-shell/src/components/mod.rs`
- `desktop/nsh-shell/src/main.rs` (enable components module)
- `desktop/nsh-shell/src/session.rs` (Message struct if needed)

All in task-d's scope.
