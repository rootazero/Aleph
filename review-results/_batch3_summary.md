# Review Summary — batch-3

**Date**: 2026-07-21
**Modules reviewed**: 8 (`src/extension`, `src/gateway`, `src/generation`, `src/group_chat`, `src/guardrails`, `src/harness`, `src/init_unified`, `src/loop_graph`)
**Reviewer**: static (rust-logic-audit five-phase checklist, parallel subagent dispatch)
**Branch**: `review/batch-3`
**Worktree**: `/tmp/aleph-review-batch-3`

## Module Totals

| Module                 | Files |      LOC | Critical | Warning | Suggested Test |
|------------------------|------:|---------:|---------:|--------:|----------------:|
| src/extension          |    69 |   24,661 |    **12**|       9 |               4 |
| src/gateway (core)     |    85 |   ~26k   |        0 |       4 |               3 |
| src/gateway (handlers) |   130 |   45,568 |        1 |       2 |               3 |
| src/gateway (interfaces) | 193 | 49,723 |    **11**|       4 |               0 |
| src/gateway (engine)   |    84 |   ~30k   |        1 |       4 |               2 |
| src/gateway (rest)     |   ~95  |  ~25k   |        2 |       6 |               3 |
| src/generation         |    79 |   23,143 |        0 |       7 |               4 |
| src/group_chat         |     8 |    3,114 |        1 |       6 |               3 |
| src/guardrails         |     9 |    1,382 |        0 |       6 |               3 |
| src/harness            |    26 |   17,466 |        1 |       2 |               3 |
| src/init_unified       |     3 |      658 |        1 |       5 |               3 |
| src/loop_graph         |     5 |    1,440 |        2 |       3 |               2 |
| **TOTAL**              | ~786 |  ~248k   |    **32**|      58 |              33 |

## Critical Findings (fixed)

### src/extension
1. `config/mod.rs:44-67` — ConfigManager only merged `aleph.jsonc`/`aleph.json`; `aleph.toml` silently dropped. Fixed: load all three, dispatch by extension.
2. `manifest/adapters/{auto_discover,cursor,aleph_toml}.rs` — `commands/` parsed as `SkillType::Skill`, registered as model-auto-invocable. Fixed: switch to `SkillType::Command`.
3. `manifest/parsers.rs:166`, `scan_component_dir` — manifest-declared paths accepted without `is_path_inside` check (path traversal). Fixed.
4. `manifest/types.rs:382` — `entry_path` allowed absolute entries and silently returned unverified path when canonicalize failed. Fixed: reject absolute, require both canonicalize calls succeed.
5. `manifest/adapters/{auto_discover,cursor}.rs` — `plugin_id` raw dir name, no sanitize. Fixed.
6. `registrar/api.rs:46` — `CapabilityApi::register_capability` didn't check plugin ownership of capability. Fixed.
7. `registry/plugin_registry/mod.rs:76` — `register_plugin` silently replaced existing entries without unregistering stale capabilities. Fixed.
8. `mod.rs:469`, `plugin_ops.rs:282` — `set_plugin_enabled(false)` only flipped status; runtime kept serving. Fixed.
9. `marketplace/{github_source,installer,mod}.rs` — marketplace name validation + symlink rejection. Fixed.
10. `hooks/executor.rs:498,612,627,780`; `runtime/wasm/mod.rs:175` — debug logs echoed full resolved command, stderr, hook body, WASM input. Fixed: structured fields only.
11. `hooks/executor.rs:45,744,770` — `read_capped` and HTTP body loop silently swallowed I/O errors as truncation. Fixed.
12. `registry/plugin_registry/mod.rs:256` — `ServiceRegistration` keyed by bare `service.id`, allowing collision. Fixed: namespace by `plugin_id:service_id`.

### src/gateway (interfaces)
13. `interfaces/feishu/.../webhook_server.rs:118-127` — Feishu webhook accepted requests without full signature/timestamp/token verification (replay-prone). Fixed: enforce all three + token.
14. `interfaces/line/mod.rs:127-138` — LINE webhook task not cancelled on stop, port held. Fixed.
15. `interfaces/discord/mod.rs:243,335,480` — DM / slash command bypassed allowed_channels check when empty. Fixed.
16. `interfaces/whatsapp/mod.rs:231-233` + `config.rs:74-75` — `NeedsPairing` silently dropped; defaults too permissive. Fixed.
17. `interfaces/msteams/auth.rs:357-381` — `serviceUrl` validation bypass via `userinfo`/non-https/subdomain trickery. Fixed.
18. `interfaces/msteams/streaming.rs:362-364` — StreamCoalescer text buffer accumulated duplicate content on every update. Fixed.
19. `interfaces/nostr/message_ops/ops.rs:152-178` — Nostr relay accepted unsigned/mismatched-id events. Fixed.
20. `interfaces/wechat/runtime.rs:75-90,137-145,155` — sync cursor advanced on failure, token never persisted, context_token never cached. Fixed.
21. `interfaces/qq/api.rs:38-41` — `Duration` underflow panic on clock skew. Fixed with `saturating_duration_since`.
22. `interfaces/qq/delivery.rs:51` — `ReplyTracker` first insert returned wrong "passive reply" hint. Fixed.
23. `interfaces/{irc,xmpp}/mod.rs` — `send()` read-lock handling was analyzed as "lock held across await" by subagent but the analysis was wrong; subagent's `.clone()` fix broke the type contract. **Reverted**: kept original `as_ref().ok_or_else()`.
24. `interfaces/plugin.rs:34-40` — `channel_types()` output order non-deterministic. Fixed with `sort_unstable()`.

### src/gateway (handlers)
25. `handlers/mod.rs:157,161` — `make_runtime_ledger` used `std::sync::Arc` / `tokio::sync::RwLock` directly. Fixed via `crate::sync_primitives`.

### src/gateway (engine)
26. `inbound_router/command_handler.rs:280-282` — `/btw` injected non-mode JSON into `SLASH_COMMAND_MODE_KEY`, breaking the engine's slash-command fast path. Fixed: route through `execute_for_context` (regular path).

### src/gateway (rest)
27. `reply_emitter/emitter/helpers.rs:74-140` — Voice auto-disable state never persisted to registry, defeating the 3-strike mechanism. Fixed.
28. `security/store/tokens.rs:54-68` — `get_shared_token_plaintext` returned `Some("")` for empty plaintext, disagreed with `read_current_token_readonly`. Fixed.

### src/group_chat
29. `orchestrator.rs:176-178` — `end_session` silently dropped `session.end()` on `try_lock` contention. Fixed: `match` with debug log.

### src/harness
30. `agent/think.rs:1121` — `race_llm_call` timeout was re-armed per retry (fresh `budget` from "now" instead of `started + budget`), allowing a turn to overrun by up to `(retries + 1) × budget`. Fixed with `sleep_until(tokio::time::Instant::from_std(started) + budget)`.
31. `deps.rs:137` — `std::sync::Arc` used directly. Fixed via `crate::sync_primitives`.

### src/init_unified
32. `coordinator.rs:117,118,119,132` — `INITIALIZING` used `std::sync::atomic::*` directly. Fixed via `crate::sync_primitives`.

### src/loop_graph
33. `store.rs:19,56,67,71` + `service.rs:34,46` — `std::sync::Mutex` / `MutexGuard` used directly. Fixed via `crate::sync_primitives`.
34. `store.rs:291-303` — Dangling-edge lint finding double-counted when both endpoints vanished. Fixed.

## Compile-Errors Fixed After Subagent Work

After all 11 subagent commits, `cargo check -p alephcore` flagged 4 errors that were introduced by subagent fixes. Fixed in `5d2f4c898`:
- `gateway/interfaces/wechat/inbound/mapper.rs:90` — subagent accidentally removed `to_user_id` local binding. Restored.
- `harness/agent/think.rs:1121` — `tokio::time::sleep_until` requires `tokio::time::Instant`, not `std::time::Instant`. Wrapped with `tokio::time::Instant::from_std()`.
- `gateway/interfaces/{irc,xmpp}/mod.rs` — subagent's `.clone()` "fix" for the (incorrectly diagnosed) lock-across-await issue broke the `&Sender<String>` type contract. Reverted to `.as_ref().ok_or_else()?`.

## Architecture Compliance Snapshot

| Redline | Status across the 8 modules |
|---------|------------------------------|
| **R1** (no platform APIs in core) | clean |
| **R8** (regex only for machine formats) | clean |
| **R10** (intelligence in prompts) | clean |
| Sync primitives via `crate::sync_primitives` | **5 violations fixed**: extension (config), gateway-core (event_emitter RwLock), gateway-handlers (mod.rs Arc/RwLock), init_unified (INITIALIZING atomic), loop_graph (store + service Mutex) |

## Categories Summary (across all 8 modules)

- **Critical**: 32 (all fixed)
- **Warning**: 58 (mostly recorded, surgical fixes only)
- **Suggested Test**: 33 (no tests added; documented for follow-up)
- **Compile fixes post-cargo-check**: 4

## Fix Strategy

Critical + surgical fixes land as separate commits per module on `review/batch-3` branch. No `cargo check` mid-flight. Single `cargo check -p alephcore` after all fixes verified clean.

Final verification: `cargo check -p alephcore --message-format=short` → `EXIT=0`, 7m 47s.