# Tool 首调用激活与文档边界运行记录 / Tool First-Call Activation and Documentation Boundary Run

**日期 / Date:** 2026-11-03
**基线 / Baseline:** `main` at `78763a6cefc2132ed1465e4d9a2c099503f4a2f7`, clean before this phase
**执行方式 / Execution:** Native main-agent execution; one writer; no push or merge

## 目标 / Goal

本阶段闭合一个已由源码证明的契约缺口：Cold tool-only WASM declaration 以前只存在于 discovery metadata，canonical callable state 没有可激活 handler，而 `PluginActivationAuthority` 与 `ReadySnapshot` 只接受 Ready publication，因此 exact permitted first Tool call 无法完成 Cold→Ready 激活。

This phase closes a source-proven contract gap: Cold tool-only WASM declarations were visible only as discovery metadata, with no callable activation handler in canonical state. Because `PluginActivationAuthority` and `ReadySnapshot` require Ready publication, an exact permitted first Tool call could not activate a Cold owner.

## Rust changes / Rust 修改

- `src/tools/registry.rs`: added explicit `ColdSnapshot`, Cold entry state, unified Cold/Ready publication identity, Cold batch receipt, and atomic Cold→Ready replacement using the existing registry receipt authority.
- `src/extension/runtime_owner.rs`: publishes trusted Tool declarations as metadata-only Cold entries, replaces the exact Cold receipt during mount, and retains Failed residuals on publication collision.
- `src/extension/canonical_handler.rs`: added retained `PluginColdHandler`; it validates owner incarnation, awaits existing single-flight activation, captures the same registry's Ready entry, and re-enters the registry-owned Ready handler under sealed publication proof. It never calls WASM directly or fabricates Ready proof.
- `src/tools/scoped/dispatch.rs` and `src/tools/dispatch_verdict.rs`: rebind the original Cold capture to the post-activation Ready entry while preserving the original permission/gate verdict.
- `src/extension/lifecycle.rs`: updated Cold metadata and residual semantics and added/updated real-WASM lifecycle coverage.
- `src/extension/mod.rs`, `src/extension/projection.rs`, and `src/gateway/execution_engine/tool_refresh.rs`: include admitted Cold declarations in the existing derived tool metadata projection; this does not activate or authenticate a provider.
- `src/orchestrator/harness_bridge/replay_adapter.rs`: refuses Safe Replay preparation for Cold declarations before a replay claim can be consumed.

Provider generation remains in `ProviderHandle`; owner incarnation remains `RuntimeOwner`'s `NEXT_OWNER_GENERATION` identity; `OwnershipTree` and `MemoryRuntime` remain separate authorities. Transport compatibility remains mounted-only. Read-side catalog/describe/resolve/project/observer/doctor calls do not activate.

## Rust evidence / Rust 证据

Every Cargo command was preceded by `python qa/browser_live/check_memory.py`, returned `ALLOW` with at least 4 GiB available, and used `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`.

- Registry RED: missing `PluginColdBatch`, `PluginColdBatchEntry`, `publish_plugin_cold_batch`, and `CanonicalEntryLease::is_cold_declaration`.
- Registry GREEN: `cargo test -p alephcore --lib tools::registry::tests::` — 58 passed, 0 failed.
- Additional registry receipt tests: `cold_` — 2 passed, 0 failed (unrelated mutation does not stale a Cold receipt; name-only unregister cannot remove it).
- Replay safety test: `cold_safe_tool_is_refused_before_replay_claim` — 1 passed, 0 failed.
- `cargo test -p alephcore --lib stage_f_ -- --nocapture` — 10 passed, 0 failed.
- `cargo test -p alephcore --lib stage_h_ -- --nocapture` — 6 passed, 0 failed.
- Coverage includes Cold no-start metadata reads, first-call activation, concurrent single-flight, permission denial, activation/build/publication/disposal failure residuals, stale generation fencing, discovery, enable, reload and concurrent discovery.
- Warnings were unrelated `PluginProjection.panel_snapshot` dead code and a linker warning.
- Whole-repository `cargo fmt -- --check` remains blocked by pre-existing formatting differences outside this change; unrelated files were not reformatted.

## Locator and link cleanup / Locator 与链接清理

`docs/reference/FEATURE_LOCATOR.md` now has explicit bilingual `Current state / 当前现状`, `Deferred boundaries / Deferred 边界`, and `Historical dictionary / 历史词典` layers. Historical round entries remain evidence, and current source plus owning reference documents win when dated history conflicts with present behavior. The current Tool/Plugin section records metadata-only Cold discovery and exact permitted canonical first-call activation, while preserving the mounted-only transport boundary and deferred non-Tool kinds.

The repository link classifier is `qa/docs/check_markdown_links.py`. It uses only the Python standard library, skips fenced and inline-code examples, reports external/protocol and machine-private links separately, and fails only on unresolved repository-relative targets. Repository-verifiable links were repaired or migrated to surviving evidence; deleted/private history and machine-local/external paths were marked as unavailable or explanatory non-links.

The initial broad scan produced 228 raw relative-link findings. The final parser report is disjoint by recognized link shape; the editorial categories below describe repair decisions and may overlap at the file level:

| Editorial category / 编辑分类 | Count | Meaning / 含义 |
| --- | ---: | --- |
| `repairable-current` / 可修复当前链接 | 15 files | Relative paths repaired to repository-verifiable current files. |
| `historical-migrate` / 历史迁移 | 9 files | Historical links moved to surviving source/plan/spec/run evidence. |
| `historical-unavailable` / 历史不可用 | 11 files | Deleted/private historical artifacts marked unavailable without invented targets. |
| `code/example` / 代码示例 | 157 links | Fenced or inline examples excluded from the repository link contract. |
| `external/placeholder` / 外部或占位 | 65 external/protocol links, 0 placeholders | Preserved as non-repository references; no placeholder target remained. |

These are file/link decision counts from the repaired diff, while the parser totals below are the reproducible final scan. The scanner intentionally does not fully parse nested, multiline, escaped, or angle-bracket Markdown syntax; the zero unresolved count therefore applies to its recognized repository-relative subset.

Final classified scan:

- `doc_files=1629`
- `valid=904`
- `external_or_protocol=65`
- `absolute_or_private=0`
- `placeholder=0`
- `code_or_example=157`
- `missing_repository_targets=0`

## Review findings and fixes / Review 发现与修复

The first whole-diff review did not reach a Clean/APPROVE verdict: its read-only process was SIGKILLed at the 30-minute limit. Its actionable source findings were independently confirmed and fixed before the final review:

- Cold receipt validation no longer depends on an unrelated global registry revision; exact descriptor and Cold identity remain required.
- Name-only `unregister` refuses Cold producer entries, so Cold teardown remains receipt-owned.
- `with_current_capture` compares Cold identities instead of treating `(ready=None, cold=None)` as every valid capture.
- The Cold handler verifies the post-activation Ready entry still belongs to the original plugin and owner incarnation.
- A failed Cold receipt revoke is restored to the owner residual for retry/diagnostics.
- Safe Replay refuses Cold declarations before claim consumption, because the replay token cannot be safely rebound by ordinary dispatch identity replacement.
- The agent metadata projection now includes Cold declarations from the existing RuntimeOwner source, and the locator's historical table header and plan/run bookkeeping were corrected.

- The first fresh review then identified a Cold completion rebind race, publication-only residual cleanup, stale design status, and incomplete link ledger. The fixes retained original plugin/owner identity through outer rebind, made publication-only Failed owners cleanable, updated the design status, and recorded raw/editorial scanner categories.
- The second fresh review identified MCP exposure of uncallable Cold metadata, disabled Cold owners remaining in the owner map, and activation errors being overwritten by post-call rebind. The fixes added a mounted-only plugin snapshot for MCP, remove Cold/Disabled owners after successful cleanup, and return the original failed invocation before rebind validation. The Cold metadata test now asserts canonical visibility and mounted-only exclusion separately.

- A final fresh read-only whole-diff review returned **Clean / APPROVE** with no P0–P2 findings. It inspected all 55 tracked changed files and 4 untracked files, including Rust authority paths, documentation repairs, the scanner, design, plan and run record. It confirmed Cold/Ready proof separation, receipt ownership, provider/owner generation fencing, canonical versus mounted-only routing, Safe Replay refusal, and scanner consistency.
- The reviewer did not run Cargo, as required; the Cargo evidence above is from the main agent under the memory gate.

The review gate and local conventional commit are complete (`4b6da265a`). No push or merge was performed. / 审查与本地提交已完成（`4b6da265a`），未推送或合并。

## Scope limits / 范围边界
- This record does not claim universal Capability lifecycle ownership, a universal scheduler, native unload, RSS reduction, process isolation, or exactly-once external effects. Feishu→Zahir, MCP lazy activation outside the Tool slice, EffectClaim production, Safe Replay, remaining Capability kinds and other deferred integrations remain deferred/NO-GO. Historical validation claims in old documents were not rerun merely because their links were repaired.

本记录不声称所有 Capability 都由同一生命周期拥有，不声称 universal scheduler、native unload、RSS reduction、process isolation 或 exactly-once external effects。Feishu→Zahir、Tool slice 之外的 MCP lazy activation、EffectClaim production、Safe Replay、其余 Capability kind 与其他 deferred integration 仍保持 deferred/NO-GO。旧文档中的历史验证结果不会因为链接修复而被重新声称为本轮验证。

## Validation complete / 验证完成

- `python qa/docs/check_markdown_links.py --self-test` — PASS.
- `python qa/docs/check_markdown_links.py` — `doc_files=1629`, `valid=904`, `external_or_protocol=65`, `absolute_or_private=0`, `placeholder=0`, `code_or_example=157`, `missing_repository_targets=0`.
- UTF-8 and changed-file structural checks passed; `git diff --check` passed after removing trailing whitespace from newly added records and the touched historical note.
- `cargo check -p alephcore --tests` — PASS with existing warnings.
- `cargo check -p alephcore --bin aleph-server` — PASS with existing warnings.
- `cargo clippy -p alephcore --lib --tests -- -A clippy::large_enum_variant` — PASS with existing warnings.
- Whole-repository `cargo fmt -- --check` was not green because of unrelated pre-existing formatting differences; no unrelated formatting was changed.

A fresh read-only whole-diff review returned **Clean / APPROVE** with no P0–P2 findings. The final review was read-only and did not run Cargo; Cargo evidence above was executed by the main agent under the required memory gates. The scanner limitation remains documented: nested, multiline, escaped, and angle-bracket Markdown syntax is outside the recognized subset.

The final local conventional commit was recorded after this review. No push or merge is authorized.

## Remaining validation / 范围边界（历史字段）

The integrated Cargo checks, structural documentation validation, and final read-only review are complete. The local conventional commit is recorded; no push or merge is authorized.
