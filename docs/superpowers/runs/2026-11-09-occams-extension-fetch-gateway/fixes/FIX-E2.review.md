# Fix review — E2

| ID | Verdict | Comment |
|---|---|---|
| E2-01 | FIXED | Line 720 now propagates manifest parse errors with `.map_err(...)? ` instead of `if let Ok`, blocking invalid settings when manifest is unreadable. |
| E2-02 | FIXED | Lines 17 and 46 acquire `load_guard` before registration lookup, serializing manual start/stop with mount/unmount lifecycle (lines 120–121 disposer). |

## New problems

None.

## Batch verdict

Safe to commit. Both failure modes eliminated: E2-01 now fails closed on manifest parse errors; E2-02 serializes manual service control with mount lifecycle using the existing load_guard.