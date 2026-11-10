# Fix review — G4

| ID | Verdict | Comment |
|---|---|---|
| G4-01 | NOT_FIXED | No diff for `sanitize.rs`; the livelock in `strip_tags_code_aware` and `split_reasoning` remains unaddressed. |
| G4-02 | NOT_FIXED | No diff for `sanitize.rs`; the case-sensitive quick probe still bypasses uppercase `<MEMORY>` tags. |
| G4-03 | NOT_FIXED | No diff for `idempotency.rs`; stale slots can still complete or discard replacements by key alone. |
| G4-04 | NOT_FIXED | No diff for `agent_instance.rs`; failed lazy construction still removes the registry entry permanently. |
| G4-05 | NOT_FIXED | The diff only updates a test import path (`SessionFilter` → `types::SessionFilter`); no reservation logic added to `router.rs` or `session_store`. |
| G4-06 | NOT_FIXED | No diff for `streaming.rs`; terminal suffixes are still appended to `self.buffer` rather than the controller's live tail. |
| G4-07 | NOT_FIXED | No diff for `streaming.rs`; notice insertion before the `text.is_empty()` check still suppresses summary fallback. |
| G4-08 | NOT_FIXED | No diff for `config.rs`; malformed legacy config is still accepted as defaults. |
| G4-09 | NOT_FIXED | No diff for `streaming.rs`; `StreamAction::Done` still skips settling edits, leaving cursors visible. |
| G4-10 | NOT_FIXED | No diff for `pairing_store.rs`; expired pending requests are still reused without TTL validation. |

## New problems

- `src/gateway/router.rs:281` — The test import path change (`SessionFilter` → `types::SessionFilter`) suggests a module reorganization that is not shown in the diff. If `types` is a new submodule, this change will break compilation unless the corresponding `mod types;` declaration and file exist. This is a build-breaking regression unrelated to any finding in G4.

## Batch verdict

**Needs complete rework.** The diff contains only a single test import path change that does not address any of the ten findings and introduces a potential build break. None of the described fixes (acquisition fencing, epoch reservation, suffix routing, cursor settling, TTL validation, etc.) are present in the submitted diff. The worktree appears to contain either the wrong diff or an incomplete extraction.