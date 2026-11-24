# FIX-GX2 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- GX2-08: FIXED
- GX2-01: FIXED
- GX2-05: FIXED

## GX2-DUPLICATE-TOPIC — FIXED — 前台入口复用现有 helper，统一提示词、LLM 调用和解析 / Foreground `generate_session_topic` delegates to the shared background helper; one prompt/LLM/parser implementation remains.
## GX2-REDUNDANT-KEY-CLONE — FIXED — 直接移动旧会话键，删除多余克隆 / Removed `old_key_for_topic`; the spawned task captures and borrows `old_key_resolved` for generation and persistence.

验证 / Verification: Added one focused source-structure `#[test]` per finding in `command_handler.rs`. Their equivalent source assertions failed before the fixes and passed afterward. `rustfmt --edition 2021 --check` and targeted `git diff --check` passed.

边界与未做事项 / Limits and omissions: No cargo commands, Rust test execution, live LLM calls, commits, or unrelated edits. The foreground path now resolves locale before reading history; the locale fallback, 20-message history limit, two-message minimum, 100-character excerpts, response parsing and warnings are unchanged. Existing entries above are previous-wave records, not reverified in this pass.
