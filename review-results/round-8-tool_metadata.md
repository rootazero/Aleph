# Logic Review Report — src/tool_metadata
**Module**: src/tool_metadata
**Scope**: full module (23 .rs files)
**Date**: 2026-08-29
**Mode**: strict

## Findings

### [Critical] Production wiring gap: `with_safety_level` and `with_requires_confirmation` are unreachable from any production caller
- **Location**: `src/tool_metadata/types/unified/builders.rs:51` (`.with_requires_confirmation`), `src/tool_metadata/types/unified/builders.rs:58` (`.with_safety_level`)
- **Trigger condition**: any tool is registered through `ConflictResolver::register_with_conflict_resolution`, which calls `infer_visible_channels(&tool)` (`registry/conflict.rs:152-157`).
- **Expected behavior**: the `IrreversibleHighRisk` arm (panel/cli only) and the `_ if tool.requires_confirmation` arm (panel/telegram/discord/cli, no iMessage) of `infer_visible_channels` are intended to be the canonical place where channel visibility is derived from a tool's own declared safety/confirmation metadata. Every call site that builds an `UnifiedTool::new(...)` for a destructive operation should chain `.with_requires_confirmation(true)` or `.with_safety_level(ToolSafetyLevel::IrreversibleHighRisk)` so the registry hides the tool from iMessage / dangerous ops are panel/CLI-only.
- **Actual behavior**: grep across the entire crate finds exactly **two** uses of these builders, both inside `src/tool_metadata/types/unified/tests.rs` (lines 49 and 103). Zero production call sites. Across 112 `UnifiedTool::new` constructions in `src/` (including the 100+ sites in `executor/builtin_registry/builder/constructor/*.rs`), no tool sets `requires_confirmation = true` or `safety_level != ReadOnly` on the catalog side. Consequence: `infer_visible_channels` always falls through to `Vec::new()` ("visible to all channels") for every catalog entry, so the whole high-risk/confirming-tool gating logic in `registry/conflict.rs:19-34` is dead on arrival. An MCP tool that carries `destructiveHint = true` (e.g. `github__delete_repo`) is registered into the catalog with `requires_confirmation = false`, gets `visible_channels = []`, and shows up in iMessage even though the handler-side `requires_confirmation()` will still gate the call — a UX/state-divergence bug between the catalog layer and the execution layer.
- **Suggested fix**: either (a) wire `requires_confirmation` from the handler-side flag at registration time (the same source `tools/handlers/registration.rs:101` already reads for `with_flags`), or (b) delete `requires_confirmation` and `safety_level` from `UnifiedTool` and collapse `infer_visible_channels` to a no-op so the dead code doesn't masquerade as a safety boundary. Add a unit test that asserts at least one registration sets these to non-default values; today that test would fail to compile.

### [Critical] `UnifiedTool::is_active` is read on every query but never mutated in production
- **Location**: `src/tool_metadata/types/unified/mod.rs:72` (definition, `pub is_active: bool`); `src/tool_metadata/registry/query.rs:24,34,46,60,189,258,312,325,355,374,391,417` (every read site)
- **Trigger condition**: any `ToolCatalog` list/filter operation — `list_all`, `list_builtin_tools`, `list_all_for_ui`, `list_for_channel`, `resolve_command`, `find_best_match`, `is_namespace`, `list_namespace_children`, `list_root_commands`, `list_by_mcp_server`, `search`, `count`, `active_count`.
- **Expected behavior**: `is_active` exists so operators can hot-pause a tool (and the LLM stops seeing it) without un-registering. A `set_inactive`/`pause`/`unregister_tool` method on `ToolCatalog` should be the surface for that, and the filter `.filter(|t| t.is_active)` should actually remove something.
- **Actual behavior**: `is_active` is initialised to `true` in `UnifiedTool::new` and never assigned anywhere in production. The only mutations in the crate are inside tests (`src/tools/adapters/registry_adapter.rs:1131`). The field is `pub`, so technically anyone can write it from outside, but the lock-protected accessor path doesn't exist, and no code path does. Every list operation pays the cost of `filter(|t| t.is_active)` without ever excluding a tool. A grep for `unregister_tool` / `deactivate_tool` / `pause` in `src/tool_metadata/` returns zero matches.
- **Suggested fix**: either provide a `ToolCatalog::set_inactive(&self, id: &str, active: bool)` that holds the write lock and bumps `ToolHealthCache.invalidate_all()` (the same pattern the other mutation methods follow), or mark `is_active` private and drop the filter. Either fix must be paired with a test that exercises the new path.

### [Warning] 9 of 11 constants in `tool_metadata/constants.rs` are dead code
- **Location**: `src/tool_metadata/constants.rs:16-34`
- **Trigger condition**: any consumer of `DEFAULT_MAX_FILE_SIZE`, `DEFAULT_SANDBOX_ENABLED`, `DEFAULT_ALLOW_NETWORK`, `DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE`, `DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE`, `DEFAULT_FILE_OPS_ENABLED`, `DEFAULT_CODE_EXEC_ENABLED`, `DEFAULT_CODE_EXEC_RUNTIME`, `DEFAULT_PASS_ENV`.
- **Risk**: silently drifting from the real config defaults. The module comment claims these are "Security-enforced constants that are not user-configurable" and that "their only readers were the `default_*` fns behind `[agent] require_confirmation` / `max_parallelism`", but the constants are still re-exported via `pub use constants::*` in `mod.rs:13` and continue to compile as part of the public `alephcore` API surface.
- **Current impact**: low (no behaviour bug today — the values are not read), high long-term drift risk because `[sandbox]` / `[policies]` defaults live in `config/types/*` and the values in `constants.rs` no longer match the real defaults after the `require_confirmation`/`max_parallelism` retire pass.
- **Suggestion**: either delete the unused constants or wire them as the *fallback* defaults at the same call sites that read `[sandbox] enabled`, `[policies] require_confirmation`, etc. — whichever matches the real config layer's behaviour. Do not leave them as a stale second source of truth.

### [Warning] Loom tests do not model the actual concurrency patterns in the module
- **Location**: `src/tool_metadata/loom_concurrency.rs` (entire file, 119 lines)
- **Trigger condition**: `cargo test --features loom` against this module.
- **Expected behaviour**: per `sync_primitives.rs` lock hierarchy (`Level 2: ToolCatalog, ChannelRegistry`), the loom tests should pin invariants the module *actually* relies on: (1) `ToolHealthCache` lock-free reads via `ArcSwap::load` while a writer is doing `rcu`, (2) `tokio::sync::OnceCell` single-flight coalescing when N threads call `refresh(name)` concurrently, (3) `DashMap<String, Arc<OnceCell<...>>>` slot leak protection under cancellation (the `InflightGuard` in `health.rs:228`), (4) `Arc<AtomicU64>` monotonic `generation` counter with `Acquire`/`Release` synchronisation across the cache-snapshot pair, (5) `AsyncRwLock<HashMap>` write-lock held across the conflict-detection/rename/insert critical section in `register_with_conflict_resolution`.
- **Actual behaviour**: the four existing loom tests cover entirely synthetic patterns: `AtomicBool` pause/resume/cancel (`loom_engine_pause_resume_cancel`), `AtomicU64` `fetch_add` (`loom_atomic_counter_monotonic`), and a `(u32, u32)` tuple `RwLock` (`loom_registry_concurrent_read_write`, `loom_progress_snapshot`). None of those primitives is used in the production module — `AtomicBool` appears nowhere in `tool_metadata/`, and the `RwLock<HashMap<String, u64>>` test never reaches the `Arc<AsyncRwLock<HashMap<String, UnifiedTool>>>` shape. The test that is closest to a real pattern (`loom_engine_pause_resume_cancel`) even self-asserts `assert!(cancelled.load(...))` — which is meaningless because only one thread writes `cancelled = true` and the assertion always holds regardless of any race the loom model was supposed to catch.
- **Risk**: medium-high. The single most subtle concurrency primitive in the module — `ToolHealthCache::refresh`'s `OnceCell` single-flight with RAII guard — has zero loom coverage, and the loom tests that exist will not catch a regression there.
- **Suggestion**: rewrite `loom_concurrency.rs` to model the actual `health.rs` state machine (see Suggested Test below), and at minimum cover the concurrent-register / concurrent-unregister races that `register_with_conflict_resolution` and `remove_by_mcp_server` are designed to handle.

### [Warning] `register_with_conflict_resolution` silently overwrites a tool with a duplicate id (last-writer-wins)
- **Location**: `src/tool_metadata/registry/conflict.rs:255-263`
- **Trigger condition**: two registrations whose `tool.id` collisions — examples: two skills with the same `id` (the id is keyed only on `skill:{id}` per `ToolSource::format_tool_id`), two plugin manifests declaring the same `tool_name` under the same `plugin_id`, or a hot-reload that forgot to call `ToolCatalog::clear`. The bug is *not* the warning log itself (which is fine), it is that the conflict check above only inspects `tool.name.to_lowercase()`, not `tool.id`, so two tools with the same id but different names slip past the conflict block and hit the bare `HashMap::insert`.
- **Expected behavior**: an id collision should either (a) refuse the second registration with an explicit error, or (b) route through the same priority/rename logic as a name collision. A warning + silent overwrite is a third option but it requires the id check to be the gating event, not the name check.
- **Actual behavior**: line 257 `tools.insert(id.clone(), tool)` returns `Some(prev)` on duplicate id; the comment correctly identifies this; the resolution is `last-writer-wins` (the `prev` value is dropped on the floor after a single `tracing::warn!`). The "first registered" tool is gone and any caller holding an `Arc<UnifiedTool>` to it now holds a stale handle. The `was_renamed` flag is *not* set on the new tool, so a future code reader cannot tell from the tool's own metadata that it displaced something.
- **Suggestion**: extend the conflict check to also search for an existing tool with the same `id` (not just the same `name`), and run it through `resolve_conflict` the same way. If a deliberate "id-wins last-writer" policy is desired, add a `was_renamed = true` and an explicit `original_id` field on `UnifiedTool` so the audit trail is symmetric with name-based renaming.

### [Warning] `register_with_conflict_resolution` display-name format is inconsistent between rename arms
- **Location**: `src/tool_metadata/registry/conflict.rs:202-208` (`RenameExisting` arm) vs `src/tool_metadata/registry/conflict.rs:224-225` (`RenameNew` arm)
- **Risk**: cosmetic, but every consumer that surfaces `display_name` (e.g. `/help` renderer, `commands.list` RPC response) will see two different suffixes for the same kind of event: `search-mcp (renamed)` for the loser-when-existing-wins path and `search-mcp (MCP)` for the loser-when-new-wins path.
- **Current impact**: low (UX consistency only).
- **Suggestion**: pick one format and use it in both arms — the source-label format is richer, so prefer `format!("{new_name} ({})", losing_source.label())` and apply it in both branches.

### [Warning] `extract_command_name` only handles three regex prefixes — silently wrong on real-world patterns
- **Location**: `src/tool_metadata/registry/helpers.rs:13-22`
- **Trigger condition**: a `RoutingRuleConfig.regex` containing anything other than `^/`, `(?i)`, or `(` in that exact order — e.g. `^/(?-i)translate`, `^/(?P<n>foo)\s+`, `^/(?i:translate)\s+`, `(?i)^/foo`.
- **Expected behaviour**: the documented contract ("Extract command name from regex pattern") should handle any sane pattern that `RoutingRuleConfig` is willing to accept, since the rule's regex is parsed and stored as a string by the config layer with no validation.
- **Actual behaviour**: the function strips the listed prefixes one by one then takes alphanumeric/`-`/`_` characters and stops at the first regex metacharacter. `^/(?-i)translate` produces `?-i` (the `-` is taken, then `)` is the stop char), so the custom command ends up named `?-i` instead of `translate`. There is no error path; the `warn!` upstream in `register_custom_commands` (`registration.rs:228`) is only triggered if the result is empty. The risk is a silent misregistration: the user's `^/(?-i)translate` rule becomes a `/ ?-i` slash command.
- **Suggestion**: parse with the `regex` crate the codebase already depends on (used in `registration.rs:194` for the routing regex itself), extract the first literal token, and fall back to a default. The current implementation is a textbook example of the "two regexes stay in sync only by hope" anti-pattern called out in `CODE_ORGANIZATION.md` §5.

### [Warning] `is_active`, `requires_confirmation`, and `safety_level` are `pub` fields on `UnifiedTool`
- **Location**: `src/tool_metadata/types/unified/mod.rs:72,76,82`
- **Risk**: anyone outside the module (e.g. `tools/adapters/registry_adapter.rs:1131`) can poke these fields directly, bypassing the `Arc<AsyncRwLock>` that guards the rest of the catalog. A `disp.list_all()` caller may see one `is_active` value and a `disp.list_for_channel(ch)` caller sees another if a concurrent write happens between — there is no happens-before edge.
- **Current impact**: low (no in-tree mutation outside tests today, per the Critical finding above), but the API surface invites future writers to introduce exactly the kind of inconsistency the read-lock pattern is supposed to prevent.
- **Suggestion**: make the fields `pub(crate)` and add `UnifiedTool::set_active(&mut self, bool)` / `set_requires_confirmation(&mut self, bool)` / `set_safety_level(&mut self, ToolSafetyLevel)` constructors; pair each with a builder method that is the only public mutation path.

### [Warning] `register_plugin_tools` is referenced only in `btw_wire_tests.rs` documentation, never in production
- **Location**: `src/tool_metadata/registry/registration.rs:160-189` (`register_plugin_tools` definition)
- **Trigger condition**: any plugin manifest is discovered by the boot path. Grep for `register_plugin_tools(` outside `tool_metadata/` and `btw_wire_tests.rs:2277` (a doc comment) returns zero matches.
- **Risk**: medium. The code is exercised only by a test that calls `register_plugin_tools` indirectly through other helpers, and the boot path's plugin-registration loop in `agent_init/mod.rs` does not call this method. A user with a plugin manifest under `~/.aleph/plugins/*/aleph.plugin.toml` will not see those tools in `/commands.list` even though the function exists.
- **Current impact**: low (no in-tree plugin consumers yet), high latent risk.
- **Suggestion**: either wire `register_plugin_tools` into the actual plugin boot path in `agent_init/mod.rs`, or move the function behind a `#[cfg(feature = "plugins")]`. The current "exists but is never called" state is exactly the kind of dead wiring this audit is supposed to surface.

### [Warning] `UnifiedTool::search` on `ToolCatalog` is only used by tests
- **Location**: `src/tool_metadata/registry/query.rs:382-405` (`search` definition)
- **Trigger condition**: any caller invokes `ToolCatalog::search(query)`. Grep returns only `src/tool_metadata/registry/tests.rs:150` (a test that registers a custom command and then searches for its own name — trivially successful).
- **Risk**: medium. Production callers (`commands.list`, "did you mean?" replies) all use `suggest_commands` or `resolve_command`, never `ToolCatalog::search`. The function silently exists, returns name-matches-then-description-matches, and may give different answers from `suggest_commands` (which uses Levenshtein). Either consolidate the two or delete the unused one.
- **Suggestion**: delete `ToolCatalog::search` if `suggest_commands` is the canonical surface; otherwise document why both exist.

### [Warning] `loom_concurrency.rs` uses `Ordering::SeqCst` exclusively; the production code uses `Acquire`/`Release` for the same purpose
- **Location**: `src/tool_metadata/loom_concurrency.rs:51,52,57,63,76,79,85` (SeqCst) vs `src/tool_metadata/registry/health.rs:144,160,193,267` (`Release`/`Acquire`)
- **Risk**: the loom test is a stronger model than the production code — SeqCst is more expensive and a stricter ordering than the `Release`/`Acquire` pairs used by `generation` in `ToolHealthCache`. A loom test that pins a property under SeqCst does **not** prove the same property under the weaker ordering the runtime uses. If the property really requires SeqCst, the runtime is using insufficient ordering and has a data race; if the property holds under Acquire/Release, the loom test wastes CPU enumerating ordering combinations that the runtime will never take.
- **Current impact**: low (the loom test happens to be too strong, so no false-negative — but the test is meaningless as a safety check for the real code).
- **Suggestion**: rewrite the counter test to use `Acquire`/`Release` and assert the same property the production code claims — i.e. that a `load(Acquire)` after a `fetch_add(Release)` on another thread sees the bumped value (rather than the trivially true `assert_eq!(counter.load(...), 2)`).

### [Warning] `register_with_conflict_resolution` infers visibility before checking name conflicts — ordering mismatch with the alias-shadow warning loop
- **Location**: `src/tool_metadata/registry/conflict.rs:152-167` (visibility inference + alias-shadow warn) vs `src/tool_metadata/registry/conflict.rs:179-247` (name conflict block)
- **Risk**: the alias-shadow warning iterates `tools.values().filter(|t| t.aliases.iter().any(|a| a.to_lowercase() == name_lower))` — but at this point in execution, `tool` (the candidate) has *not* been inserted, so this loop is looking for tools whose alias equals the candidate's canonical name. If no such tool exists (the typical case) the warn is skipped. Fine. But if the candidate's canonical name collides with an existing tool's alias *and* the existing tool has the same canonical name (both happening), the name-conflict block renames the existing tool to `{name}-{existing.suffix()}` while the new tool's aliases (which is what the shadow warning was about) are unchanged. After insertion, the renamed existing tool's alias is still discoverable as a Tier-1 match in `find_best_match`, which is correct but is worth a comment given how many subtle invariants live in this function.
- **Current impact**: low (behaviour is correct, just undocumented).
- **Suggestion**: add a one-paragraph comment to the function explaining the post-rename invariant ("after `RenameExisting`, the loser keeps its aliases and they resolve via Tier 1/2 of `find_best_match`; after `RenameNew`, the winner keeps its canonical name and the loser's aliases are dead because its canonical name no longer matches"). The test that already pins this (`two_tools_may_share_an_alias_and_the_loser_keeps_it_as_a_fallback`) is enough; it just deserves a back-reference in the doc comment.

### [Warning] `HealthSnapshot::unhealthy_iter` rebuilds the `Instant::now()` for every item — quadratic if many entries are unhealthy
- **Location**: `src/tool_metadata/registry/health.rs:331-339`
- **Risk**: micro-perf — `unhealthy_iter` calls `Instant::now()` *inside* the `filter_map` closure, which is called once per entry. With N entries it makes N syscalls. With the current code (a few probes per tool) this is invisible; with a future expansion to thousands of probes it becomes noticeable.
- **Current impact**: low.
- **Suggestion**: capture `let now = Instant::now();` once outside the closure and reuse.

### [Suggested Test] Loom test for `ToolHealthCache::refresh` single-flight under concurrent cancellation
```rust
//! Verify that N concurrent `refresh(name)` calls coalesce to one probe
//! invocation, and that dropping a future mid-flight does NOT leak the
//! single-flight slot (so a subsequent refresh runs a fresh probe).

use crate::tool_metadata::ToolHealthCache;
use crate::tool_metadata::ProbeResult;
use crate::tool_metadata::ToolHealthProbe;
use std::sync::atomic::{AtomicU32, Ordering as AOrdering};
use std::sync::Arc;
use std::time::Duration;

struct CountingProbe {
    count: Arc<AtomicU32>,
}
#[async_trait::async_trait]
impl ToolHealthProbe for CountingProbe {
    async fn probe(&self) -> ProbeResult {
        // Hold the probe long enough that concurrent calls must coalesce.
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.count.fetch_add(1, AOrdering::SeqCst);
        ProbeResult::Healthy
    }
}

#[tokio::test]
async fn concurrent_refresh_coalesces_and_does_not_leak_under_cancellation() {
    let cache = Arc::new(ToolHealthCache::new());
    let count = Arc::new(AtomicU32::new(0));
    cache.register_probe("x", Arc::new(CountingProbe { count: count.clone() }));

    // Fire five concurrent refreshes.
    let mut handles = Vec::new();
    for _ in 0..5 {
        let c = Arc::clone(&cache);
        handles.push(tokio::spawn(async move { c.refresh("x").await }));
    }
    for h in handles { h.await.unwrap(); }

    // Single-flight: only ONE probe ran despite 5 callers.
    assert_eq!(count.load(AOrdering::SeqCst), 1,
        "concurrent refresh must coalesce");

    // Now cancel mid-flight by spawning a refresh and aborting it.
    let c2 = Arc::clone(&cache);
    let h2 = tokio::spawn(async move { c2.refresh("x").await });
    h2.abort();
    let _ = h2.await; // Err(JoinError::Cancelled) is fine.

    // Slot must be freed so a *new* refresh actually runs.
    cache.refresh("x").await;
    assert_eq!(count.load(AOrdering::SeqCst), 2,
        "cancellation must free the inflight slot for a fresh probe");
}
```

### [Suggested Test] Loom test for concurrent register/unregister with shared alias
```rust
//! Verify `register_with_conflict_resolution` does not deadlock or corrupt
//! state when one thread is registering a builtin and another is calling
//! `remove_by_mcp_server` for an MCP server whose name aliases the builtin.
//! Models: registry/mod.rs concurrent CRUD + alias-shadow warning.

use loom::sync::Arc;
use loom::thread;
use crate::tool_metadata::{ToolCatalog, UnifiedTool, ToolSource};

#[test]
fn loom_registry_concurrent_register_and_remove() {
    loom::model(|| {
        let catalog = Arc::new(ToolCatalog::new());
        let mut handles = Vec::new();

        for i in 0..3 {
            let c = Arc::clone(&catalog);
            handles.push(thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all().build().unwrap();
                rt.block_on(async {
                    let tool = UnifiedTool::new(
                        format!("mcp:server{i}:git"),
                        "git",
                        format!("git from server{i}"),
                        ToolSource::Mcp { server: format!("server{i}") },
                    );
                    c.register_with_conflict_resolution(tool).await;
                });
            }));
        }

        // Concurrent unregister of one MCP server while the others register.
        let c2 = Arc::clone(&catalog);
        handles.push(thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            rt.block_on(async {
                c2.remove_by_mcp_server("server1").await;
            });
        }));

        for h in handles { h.join().unwrap(); }
        // After dust settles, no duplicate `git` canonical names.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().unwrap();
        let tools = rt.block_on(catalog.list_all());
        let gits: Vec<_> = tools.iter().filter(|t| t.name == "git").collect();
        assert!(gits.len() <= 1,
            "conflict resolution must leave at most one canonical `git`");
    });
}
```

### [Suggested Test] Production-wired test that `with_safety_level` / `with_requires_confirmation` actually drive `infer_visible_channels`
```rust
#[tokio::test]
async fn infer_visible_channels_reflects_declared_safety_and_confirmation() {
    let registry = ToolCatalog::new();

    // High-risk tool: must be visible to Panel/CLI only.
    let destructive = UnifiedTool::new(
        "builtin:purge",
        "purge",
        "Destructive op",
        ToolSource::Builtin,
    )
    .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);
    registry.register_with_conflict_resolution(destructive).await;

    let telegram = registry.list_for_channel(ChannelType::Telegram).await;
    assert!(!telegram.iter().any(|t| t.name == "purge"),
        "IrreversibleHighRisk tool must NOT appear on Telegram");
    let panel = registry.list_for_channel(ChannelType::Panel).await;
    assert!(panel.iter().any(|t| t.name == "purge"),
        "IrreversibleHighRisk tool MUST appear on Panel");

    // Confirming tool: must be visible to Panel/Telegram/Discord/CLI, NOT iMessage.
    let confirming = UnifiedTool::new(
        "builtin:write_file",
        "write_file",
        "Mutating op",
        ToolSource::Builtin,
    )
    .with_requires_confirmation(true);
    registry.register_with_conflict_resolution(confirming).await;

    let imessage = registry.list_for_channel(ChannelType::IMessage).await;
    assert!(!imessage.iter().any(|t| t.name == "write_file"),
        "Confirming tool must NOT appear on iMessage (no confirm UI)");
}
```
This test will fail today — exactly the audit surface the Critical finding above is asking for.

## Cross-Module Findings

### [Warning] `register_with_conflict_resolution` ↔ `tools/handlers/registration.rs:113` (MCP tool unification)
- **Location**: `src/tools/handlers/registration.rs:97-132` (`register_mcp_tools`)
- **Issue**: the handler side reads the MCP `requires_confirmation` flag from `tool.requires_confirmation` (`McpTool`) and threads it into `McpHandler::with_flags` (`registration.rs:101`). The catalog side constructs `UnifiedTool::new(...)` *without* forwarding that flag. So the catalog and the handler disagree about whether a tool requires confirmation. A future `list_for_channel` filter that respects catalog-side `requires_confirmation` (which is what `infer_visible_channels` was written for) will hide a tool from iMessage while the handler is still happy to dispatch it — or vice versa.
- **Suggestion**: pass `requires_confirmation` from `McpTool` into the `UnifiedTool` builder at line 116 of `registration.rs` and apply the same forwarding for the plugin/skills registration paths.

### [Warning] `ToolHealthCache` ↔ `tools/scoped/builder.rs:509` (`requires_confirmation` lookup)
- **Location**: `src/tools/scoped/builder.rs:509` reads `tool.requires_confirmation()` from a separate `LoopTool::requires_confirmation()` trait method, which is set per-tool on the handler side. The `ToolHealthCache` (catalog side) tracks *availability* (probe health), not *declaration* (requires_confirmation). Both layers converge on whether the model sees a tool, but they track independent facts. Currently there is no integrity check that the two flags agree on the same tool.
- **Suggestion**: in `tools/handlers/registration.rs:130` (where the MCP catalog entry is built), assert in debug builds that the catalog entry's `requires_confirmation` matches the `LoopTool::requires_confirmation()` returned by the handler, so a future divergence fails the boot path early rather than producing silent UX confusion.

### [Warning] `aliases.rs` ↔ `gateway/execution_engine/slash_command.rs:32`
- **Location**: `src/gateway/execution_engine/slash_command.rs:32` (`pub(crate) use crate::tool_metadata::aliases::is_shorthand_alias;`) and `src/tool_metadata/mod.rs:17` (`pub use aliases::{...}`)
- **Issue**: `aliases::is_shorthand_alias` is re-exported in two places — `tool_metadata::is_shorthand_alias` (public, via `pub use aliases::*` in `mod.rs:17`) and `gateway::execution_engine::is_shorthand_alias` (crate-internal). The cross-table executability guard (`every_shorthand_target_is_executable`, `aliases.rs:196`) is a unit test on the alias table itself; there is no integration test that runs the boot path (which builds the alias table by reading `BUILTIN_TOOL_DEFINITIONS`) and asserts the table still matches. If `BUILTIN_TOOL_DEFINITIONS` changes without an alias-table update, the test would catch it, but only if it is wired into CI as a boot smoke test, not as a unit test on the constants.
- **Suggestion**: add an integration test under `tests/` that boots a minimal `ToolCatalog` (via `register_builtin_tools` + the `tool_catalog_init` defs loop), then asserts every `SHORTHAND_ALIASES` row maps to a resolvable tool. The round-3 fix (`aliases.rs:7-22` comment) said exactly this would have caught the `session_set_topic` regression, but the test is currently only static.

## Summary

| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 11 |
| Suggested Test | 3 |

**Total: 16 findings.**

### Key invariants confirmed
- Lock hierarchy respected: no `std::sync::*` import in `src/tool_metadata/`; all sync primitives come from `crate::sync_primitives`. The module sits at Level 2 (per `sync_primitives.rs:23-31`) and does not acquire Level 0/1/3 locks. `ArcSwap::rcu` and `tokio::sync::RwLock` write paths hold their critical sections without `.await`.
- No `.unwrap()` / `.expect()` / `panic!()` in production paths of `src/tool_metadata/`. Every `unwrap` is inside `#[cfg(test)]` or inside `loom::model` (where it is justified).
- Atomic ordering is correct: `Release` on the writer and `Acquire` on the reader for `ToolHealthCache::generation` (`health.rs:144,160,181,193,267`) is the textbook pair for an epoch counter. The loom test that models the same pattern uses `SeqCst`, which is stricter than necessary but not wrong.
- Single-flight `OnceCell` + `InflightGuard` RAII pattern in `health.rs:225-271` is sound against panic and cancellation.
- `register_with_conflict_resolution` correctly takes a single `write` lock for the inline conflict-check + rename + insert triple (no TOCTOU).

### Key invariants broken
- The two `UnifiedTool` declaration fields (`requires_confirmation`, `safety_level`) that drive `infer_visible_channels` are never set in production — the channel-visibility inference is effectively a no-op.
- The `is_active` filter on every list path is effectively a no-op for the same reason.
- The Loom suite does not exercise the concurrency patterns the module actually relies on (ArcSwap refresh, OnceCell single-flight, DashMap leak protection), so it gives false confidence.

### What this audit did NOT cover (declared out of scope)
- The two unrelated `ToolDefinition` structs in `tools/service.rs`, `tools/runtime.rs`, and `mcp/protocol.rs` — these are sibling types, not part of `src/tool_metadata/`, and were excluded.
- Boot-path wiring in `bin/aleph-server/commands/start/builder/agent_init/*` — these consumers were used as evidence (calling pattern, registration of `ToolHealthCache`), but their internal correctness is the subject of a separate audit round.
- Provider-side `ToolDefinition` schema correctness (`src/providers/protocols/openai_*/prompt_cache.rs`) — referenced only to confirm `tool_metadata::ToolDefinition` flows correctly into `RequestPayload::tools`.
