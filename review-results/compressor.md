All 121 compressor tests pass. Here's the summary:

# Module: compressor

## Summary
- Files reviewed: 7 (+ 2 test files)
- Issues found: 5
- Issues fixed: 5

## Fixes

1. **tool_truncator.rs:47** `TruncatedOutput::unchanged` — `content.len()` (bytes) used for `original_len` documented as "in characters" → Changed to `content.chars().count()`

2. **tool_truncator.rs:162,174** `truncate()` — `output.len()` (bytes) for `original_len` and `summary.len()` (bytes) mixed with `max_chars` (char count) in remaining space calculation → Changed both to `.chars().count()` for UTF-8 safety

3. **tool_truncator.rs:192** `generate_summary()` — `output.len()` (bytes) used for `{original_len}` template placeholder → Changed to `.chars().count()`

4. **smart_strategy.rs:235,265** `generate_truncation_summary()` and `truncate_output()` — Same byte/char mixing: `output.len()` and `summary.len()` used where char count is needed → Changed to `.chars().count()`, updated display format from "B" (bytes) to "chars"

5. **strategy.rs** — Dead code: `KeyInfo`, `KeyInfoExtractor`, `RuleBasedStrategy` structs unused anywhere in the codebase → Removed (per P6: "删除优于注释")

## Notes

- The `ContextStats`, `UsageBreakdown`, `CompressionFocus`, `WarningLevel`, and `CompressionPrompt` types are also unused outside the compressor module. The module doc says they're "stubbed out" and "preserved for backward compatibility". These could be candidates for future removal if no external consumers depend on them.
- No lock safety, SQL injection, `static mut`, or `unwrap`/`expect` issues found — the module is well-written with clean error handling.
- The pre-existing compilation error in `bin/aleph-server/commands/start/builder/agent_init.rs:177` is unrelated to these changes.
