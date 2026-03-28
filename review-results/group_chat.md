All tests pass. Here's the review report:

---

# Module: group_chat

## Summary
- Files reviewed: 8 (mod.rs, channel.rs, coordinator.rs, executor.rs, protocol.rs, orchestrator.rs, session.rs, persona.rs) + 1 related (config/types/group_chat.rs)
- Issues found: 4
- Issues fixed: 4

## Fixes

1. **coordinator.rs:170** `truncate_str` byte/char semantic confusion → Removed misleading `s.len() <= max_len` early-return that compared byte length against a character limit. The `char_indices().nth()` path handles both cases correctly.

2. **coordinator.rs:131** System prompt duplication → `build_persona_prompt` embedded `persona.system_prompt` in the user message text, while `executor.rs:218` also passed it via `.with_system()`. Removed from the prompt text — system prompt is now only sent through the proper system parameter, saving LLM tokens.

3. **persona.rs:76** Non-deterministic `list_presets()` order → HashMap iteration is unordered. Added `sort_by(|a, b| a.id.cmp(&b.id))` for deterministic output. Updated test to no longer need manual sorting.

4. **config/types/group_chat.rs:130** UTF-8 safety bug → `PersonaConfig::validate()` used `.len()` (bytes) for prompt length check, meaning a 667-character Chinese prompt would hit the 2000 "character" limit. Changed to `.chars().count()` to match `Persona::validate()` which already does this correctly.

## Notes

- **Code quality is high overall** — proper error types, trait-based abstraction, thorough test coverage (66 tests), correct use of `char_indices()` in coordinator, `?` operator for error propagation, no lock poisoning risk (uses `tokio::sync::Mutex`).
- **No security vulnerabilities found** — no SQL formatting, no `static mut`, no `&s[..n]` byte slicing, no `lock().unwrap()`.
- **Pre-existing compilation error** in `bin/aleph-server/commands/start/builder/agent_init.rs:177` (unrelated `?` operator issue) — not in scope.
- **Architecture compliance**: Clean separation of concerns (protocol types, coordinator logic, executor, orchestrator, session state). Follows P1 (low coupling), P2 (high cohesion), P6 (simplicity).
