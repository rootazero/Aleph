# W3C brief — server start/handlers fixes (wave 3)

Contract: `../BRIEF_FIX.md` (same directory). Worktree: /home/zou/data/workspace/Aleph-wt-fix-wave3c (branch fix-wave3c).
Report: `/home/zou/data/workspace/Aleph-wt-fix-wave3c/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w3c.md` (uncommitted, one section per ID).

## W3C-DC-01 [P1] Dynamic `handlers_mut().register(...)` loop breaks the gateway method-census test
- location: src/bin/aleph-server/commands/start/builder/handlers/settings.rs:988-1001 (census: src/gateway/method_census.rs:606-613, failing test at 666-670)
- kind: test-break
- finding: the new "ACP verbs always answer" branch registers its ten method names through a `for method in [...]` loop whose receiver is `server.handlers_mut()` and whose first argument is not a literal. The census sweep walks every `src/**/*.rs` file and pushes into `unknown` for exactly this shape (`dynamic_rpc && rel != "src/gateway/handlers/mod.rs"`), so the census test fails.
- suggested-fix: replace the loop with ten literal registrations (`server.handlers_mut().register("acp.list", unavailable);` …). `unavailable` is a non-capturing (Copy) closure, so it can be passed repeatedly, and the census then classifies the calls as literal registrations.

## W3C-DC-02 [P3] `runtime_channel_id` still leaves same-type instances on an unread key
- location: src/bin/aleph-server/commands/start/builder/subsystems.rs:1315-1349 (used at 1058-1062 and 1159-1163)
- kind: partial-fix
- finding: the resolution collapses on the channel type: two configured instances of a type whose factory hardcodes its runtime id (discord, slack, feishu …) both resolve to that type id, so the second policy registration overwrites the first and the first instance's policy is silently lost.
- suggested-fix: make the ambiguity explicit — when more than one live channel of the type is registered, log a warning that the policies cannot be separated and register under the runtime id only when it is unambiguous; otherwise thread the instance-specific key.

## W3C-B10-S2-05 [P3] `StartupResources::cleanup` runs twice on the success path and the second failure is swallowed
- location: src/bin/aleph-server/commands/start/mod.rs:3931 and 91-93
- finding: cleanup runs both on the success path and in the error path; on success it is invoked twice and the second failure is discarded.
- suggested-fix: make cleanup idempotent (single ownership / `Once`-style guard) so it runs exactly once and its error is reported on the failure path.

## W3C-B10-S2-07 [P3] Gateway teardown owns none of the background tasks it spawns
- location: src/bin/aleph-server/commands/start/builder/subsystems.rs (teardown) + src/bin/aleph-server/commands/start/builder/handlers/config.rs
- finding: the teardown path spawns background tasks but keeps no handle to them, so the tasks are not awaited or cancelled at shutdown and may outlive the resources they use.
- suggested-fix: keep the join handles of the spawned tasks and await/cancel them in the teardown path before releasing the resources they depend on.

Scope: src/bin/aleph-server/commands/start/** only.
Read the current HEAD code before editing — wave-1 and wave-2 fixes are already merged (wave2b already touched subsystems.rs, start/mod.rs and handlers/config.rs).
