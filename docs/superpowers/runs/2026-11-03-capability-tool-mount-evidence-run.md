# Capability Tool Mount Evidence Run / Capability Tool Mount 证据运行记录

- Date / 日期: 2026-11-03
- Plan / 计划: `docs/superpowers/plans/2026-11-03-capability-tool-mount-evidence-plan.md`
- Spec / 设计: `docs/superpowers/specs/2026-11-03-capability-tool-mount-evidence-design.md`
- Execution mode / 执行方式: Native execution on local `main`; main agent is the sole writer. No fetch, push, merge, force, reset, or unrelated branch switch.
- Scope / 范围: Verify the existing bounded Tool-only production mount and its real cleanup/resource effects; align documentation. No new lifecycle architecture.

## Evidence categories / 证据分类

This record keeps four kinds of evidence separate:

1. **Production source wiring / 生产源码接线** — what the implementation currently calls and shares.
2. **Static guards / 静态接线守卫** — source-level or compile-time tests that reject bypasses.
3. **Real effect tests / 真实效果测试** — tests that observe delivery, readiness, invalidation, resource discovery/resolution, or cleanup through real stores, hosts, channels, or notifications.
4. **Historical results / 历史结果** — prior run results retained for context; they do not count as this run’s fresh evidence.

## Preflight / 前置状态

- Git root / Git 根目录: `/Volumes/TBU4/Workspace/Aleph`
- Branch / 分支: `main`
- Initial HEAD / 起始 HEAD: `063345f2f` (`Merge remote-tracking branch 'origin/main'`)
- Baseline / 基线: `4b6da265a` resolves and is an ancestor of HEAD.
- Initial worktree / 初始工作区: clean.
- The requested `task-start` helper was absent; the available `task-brief` helper created the Task 1 brief. This is an environment mismatch only; the plan and evidence order are unchanged.

## Current source wiring / 当前源码接线

`src/bin/aleph-server/commands/start/mod.rs:242-305` creates one shared `Arc<OwnershipTree>` and one canonical `Arc<ToolHandlerRegistry>`, passes them to `ProjectionHost::mount_with_memory`, installs the host, and awaits applied readiness before diagnostics and later consumers. `src/capability/projection_host.rs:562-662` constructs `ZahirFacade::new_with_memory`, subscribes to registry and ownership streams before reading the initial snapshot, queues that snapshot for the default consumer, and starts the source/applier workers. `src/capability/projection_host.rs:703-723` gates readiness on applied state and fails closed after close. `src/bin/aleph-server/commands/start/builder/startup_resources.rs:25-55` stops Memory before `ProjectionHost::close_and_await`; `src/capability/projection_host.rs:1248-1298` owns the shared cancellation-safe teardown and both worker joins.

The host is a derived projection/notification surface. Callable Tool authority remains `ToolHandlerRegistry`; ownership, provider, and generation authorities remain in their existing sources. The concrete Memory Resource is source-backed and is projected separately from fake Tool entries.

## Evidence matrix / 证据矩阵

| Claim / 事实 | Source / 源码 | Static guard / 静态守卫 | Effect test / 效果测试 | Result / 结果 | Gap / 缺口 |
|---|---|---|---|---|---|
| Canonical registry and shared OwnershipTree reach the production host | `src/bin/aleph-server/commands/start/mod.rs:242-297` creates one `Arc<OwnershipTree>`, one `Arc<ToolHandlerRegistry>`, installs the registry for the request engine, and passes the same registry/tree to `ProjectionHost::mount_with_memory` | `boot_mounts_projection_host_from_canonical_registry_and_awaits_readiness` checks mount, global host install, readiness, and ordering after `set_mcp_tool_registry` | The same test is a source-backed boot gate; concrete host effects are covered by the real applier/resource tests below | PASS; source wiring and guard confirmed | No universal mount for deferred capability kinds is claimed |
| Readiness gates diagnostics/first consumers | `src/bin/aleph-server/commands/start/mod.rs:289-305` installs the host and awaits `wait_until_ready()` before diagnostics control; `src/capability/projection_host.rs:703-723` waits on applied state, not publisher state | `diagnostics_registration_follows_canonical_mount_readiness` checks readiness → diagnostics → registration ordering and shared handles | `initial_delivery_via_real_applier_and_fails_closed` proves readiness comes from the real default applier | PASS; 1 static guard + 1 real applier effect | No claim beyond bounded startup consumer ordering |
| Initial Memory delivery uses real applier and fails closed | `src/capability/projection_host.rs:562-662` queues the initial snapshot before spawning source/applier workers; `:703-723` exposes applied state and returns `false` after close | Existing assertions distinguish applied state from publisher snapshot | `initial_delivery_via_real_applier_and_fails_closed` observed one applied entry, then host close made `current_snapshot()` return `None` | PASS; real host/applier effect | No sleep-based race or publisher-state shortcut used |
| Ownership bump invalidates without registry revision | `src/capability/projection_host.rs:562-662` subscribes to ownership changes and rebuilds snapshots from the same facade | Assertions require `Invalidated` then replacement with unchanged cursor | `ownership_bump_invalidates_without_registry_revision` observed `Invalidated`, replacement snapshot, and unchanged `Cursor(1)` | PASS; real channel/applier effect | Does not prove every future writer path |
| Memory Resource is discoverable without a fake Tool entry | `src/capability/projection_host.rs:582-586` constructs `ZahirFacade::new_with_memory` when Memory is supplied; the Memory Resource is projected separately from the Tool registry | `resource_is_discoverable_without_fake_tool_entry` asserts empty Tool entries and unchanged registry revision | `src/capability/projection_host_memory_tests.rs:10-59` observed one Resource, zero fake Tool entries, unchanged cursor, and real start/stop transitions | PASS; real Resource projection effect | Does not imply a universal Resource backend |
| Resource resolve preserves canonical scope/tree authority | `src/capability/zahir_facade_memory_tests.rs:38-83` exercises the source-backed Memory Resource with the supplied tree | The test rejects custom-principal authority minting, foreign-tree resolution, and post-stop lease validation | `resource_resolve_does_not_mint_custom_scope_or_accept_another_tree` passed against real `MemoryRuntime`, `OwnershipTree`, and `ZahirFacade` | PASS; real lease/authority effect | No second ownership authority introduced |
| Cleanup stops Memory before host close and joins owned workers | `src/bin/aleph-server/commands/start/builder/startup_resources.rs:25-55` calls `memory.stop()` before `host.close_and_await()`; `src/capability/projection_host.rs:818-829,1248-1298` owns the shared completion boundary and both worker joins | `boot_closes_projection_host_before_registry_scope_dispose` checks common funnel ordering and `close_and_await` presence | Six cleanup effects passed: registration error drains Memory/host; original error survives cleanup; cancelled waiter does not detach teardown; idle close joins both workers; concurrent close is idempotent; early startup cleanup preserves state | PASS; 6/6 cleanup tests | No claim that cleanup closes shared registry/tree authority |

## Historical evidence / 历史证据

- Prior Tool activation evidence proves the bounded Cold/Ready Tool slice, not this production mount/readiness/resource/cleanup task.
- Prior whole-diff review and baseline checks are retained as history only; fresh results below are the evidence for this run.

## Task 1 source trace / Task 1 源码追踪

Fresh source tracing confirmed the production path described above. The first named consumer after readiness is diagnostics setup in `src/bin/aleph-server/commands/start/mod.rs:301-305`; the static guard also checks later agent registration ordering. `ProjectionHost::mount_sources` subscribes before the snapshot, so the initial applied state and subsequent live changes share the source boundary.

## Task 1 tests / Task 1 测试

All six named Task 1 commands selected exactly one test and passed: 6 passed, 0 failed, 0 ignored, 0 measured. Every command was preceded by `python qa/browser_live/check_memory.py`, returned `ALLOW`, and used `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; the lowest observed available-bytes reading across Task 1 and Task 2 was `6505725952`, above the required `4294967296` threshold. The startup tests are static source guards; the other four are real host/applier/resource/lease effects. Cargo emitted only existing warnings, including dead-code warnings and the macOS linker `__eh_frame section too large` warning.

Named results:

- `boot_mounts_projection_host_from_canonical_registry_and_awaits_readiness`: 1 passed.
- `diagnostics_registration_follows_canonical_mount_readiness`: 1 passed.
- `initial_delivery_via_real_applier_and_fails_closed`: 1 passed.
- `ownership_bump_invalidates_without_registry_revision`: 1 passed.
- `resource_is_discoverable_without_fake_tool_entry`: 1 passed.
- `resource_resolve_does_not_mint_custom_scope_or_accept_another_tree`: 1 passed.

## Task 2 cleanup evidence / Task 2 清理证据

All six named cleanup commands selected exactly one test and passed: 6 passed, 0 failed, 0 ignored, 0 measured. Every command passed the memory gate and used serial Cargo settings.

- `boot_closes_projection_host_before_registry_scope_dispose`: 1 static guard passed.
- `early_registration_error_drains_memory_and_its_projection_host`: 1 real startup cleanup effect passed; Memory reached `Off` and host reached `Closed`.
- `early_startup_failure_preserves_original_error_after_cleanup`: 1 real cleanup/error-preservation effect passed; original startup error remained visible and Memory reached `Off`.
- `cancelled_first_waiter_does_not_detach_teardown`: 1 real teardown effect passed; the second waiter observed both workers joined.
- `idle_close_wakeup_joins_both_workers`: 1 real teardown effect passed; registry close woke source shutdown and both workers joined.
- `concurrent_close_is_idempotent`: 1 real teardown effect passed; concurrent waiters observed the same successful outcome.

## Documentation alignment / 文档校准

Updated `AGENTS.md` and `CLAUDE.md` together so the Capability Phase 4 entry records the existing production `ProjectionHost::mount_with_memory` / `ZahirFacade::new_with_memory` path, shared canonical registry/tree, readiness wait, and Memory-before-host cleanup. Added current bounded closure entries to `docs/reference/ARCHITECTURE.md` and `docs/reference/FEATURE_LOCATOR.md` with exact source paths, source-versus-effect distinctions, run-record evidence, and deferred boundaries. Existing `docs/reference/MEMORY_SYSTEM.md` and `docs/reference/TOOL_SYSTEM.md` wording remains valid and was not broadened.

## Validation and review / 验证与审查

Documentation checks passed:

- `python qa/docs/check_markdown_links.py --self-test`: PASS.
- `python qa/docs/check_markdown_links.py`: PASS; 1,640 Markdown files scanned, 908 valid repository links, 65 external/protocol links, 157 code/example links, 0 missing repository targets.
- `python qa/browser_live/check_docs.py --self-test`: PASS (fixture self-test; this task did not change browser-live entry blocks).
- `git diff --check` and `git diff --cached --check`: PASS.
- `AGENTS.md` and `CLAUDE.md` Concrete host entries are identical after the edit.

No Rust production or test source changed, so the plan’s conditional `cargo check`/clippy steps were not run. The 12 focused runtime/effect tests above are the applicable fresh Rust evidence; Linux/Windows native execution remains unrun under the authorized exemption.

Fresh whole-diff review was attempted through the supported Pi read-only review paths but was unavailable in this environment:

- `gstack_pi_review` with `openai-codex/gpt-6.1-sol`: failed because the model is unsupported with the current ChatGPT account.
- `gstack_pi_review` with `openai-codex/gpt-6-codex`: failed because the model is unsupported with the current ChatGPT account.
- `gstack_pi_review` with the bridge default (`model:""`): timed out after 300 seconds without a report.
- A supported Pi ACP `reviewer` child: timed out after 10 minutes without a report.

None of these attempts produced a verdict, so this run records review as **unavailable**, not Ready/APPROVE. The main agent performed the final source/diff sanity checks described above; no Rust production/test source changed and no concrete review finding is available to fix.

## Deferred boundaries / 延后边界

This run does not claim universal all-writer stop, native unload, RSS reduction, process isolation, external exactly-once, universal scheduler, Feishu/Channel/Terminal/MCP lazy integration, or EffectClaim producer integration. Remaining capability kinds and production Feishu → Zahir integration remain deferred.
