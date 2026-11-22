# Phase 4: NSH Input Limitations Found

**Discovery:** Task D identified NSH rev 2ab3b6bc27 limitations

---

## Limitations

1. **No disabled() prop**
   - NSH has `readonly(bool)` only
   - Workaround: `readonly(is_generating)`
   - Loss: No platform-native disabled styling/a11y

2. **No per-input keydown events**
   - Keyboard shortcuts only via global `on_key_with("mod+enter", ...)`
   - Workaround: Guard submission in handler
   - Loss: Can't disable keyboard-only in some scenarios

---

## Implementation Strategy

- Use `readonly(is_generating)` + submission guard
- Global keyboard handler with focus check
- Document as Phase 4 tech debt

---

## Future Upstream Work

File NSH issue for native `disabled()` prop with proper a11y/styling.

---

## Impact on Phase 4

- UI functional ✅
- A11y incomplete ⚠️  
- User experience acceptable for MVP
