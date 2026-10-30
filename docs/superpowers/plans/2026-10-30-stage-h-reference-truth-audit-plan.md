# Stage H Reference Truth Audit Plan
# Stage H Reference Truth Audit 计划

**Date / 日期:** 2026-10-30
**Status / 状态:** Planned for next session / 下一会话执行计划
**Baseline / 基线:** `e759b3601a9e6ee50c96dd283fcf105b71a85c24`

## Goal / 目标

Bring the reference documentation back to one source-backed account after Stage H. The documentation must distinguish declaration-only discovery from explicit provider activation, identify the existing lifecycle owners, and preserve the deferred boundaries for concrete providers and unproven resource classes.

在 Stage H 完成后，将 reference 文档恢复为单一、以源码为事实依据的叙述。文档必须区分只读声明发现与显式 provider activation，准确指出既有 lifecycle owner，并保留具体 provider 与未证明资源类别的 deferred 边界。

## Scope / 范围

In scope:

- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/PLUGIN_SYSTEM.md`
- Other reference files only when the stale claim census proves they repeat the same outdated Stage H fact.
- A bilingual stale-claim census and validation record.

Out of scope:

- Production Rust and tests.
- Feishu or any other concrete channel wiring.
- New capability kinds, provider catalogs, lifecycle coordinators, owner maps, generation maps, runtime tables or cleanup authorities.
- Fixing unrelated repository-wide clippy or rustfmt baseline errors.
- Native unloading, process isolation, JIT/engine reclamation, thread shutdown, RSS claims or external publication.

## Architecture / 架构事实

The canonical production ownership chain remains:

```text
read caller
  └─ ExtensionManager::ensure_discovered()
       └─ declaration rows + existing RuntimeOwner(Cold)

explicit boot/control
  └─ ExtensionManager::ensure_loaded() / set_plugin_enabled()
       └─ RuntimeOwner::activate_owner()
            └─ ProviderLifecycle + ProviderHandle
                 └─ ToolHandlerRegistry canonical publication
                      └─ ProjectionHost / ZahirFacade derived views
```

The documentation must not imply that `ProjectionHost`, `ZahirFacade`, a registry snapshot, `describe`, `resolve`, `project`, or a broadcast notification can activate a provider.

## Tech Stack / 技术栈

Markdown reference documents, existing Rust source anchors, `rg`, link/reference checks, `git diff --check`, and the repository’s existing documentation conventions. No new dependency and no Cargo build is required for a documentation-only pass unless the audit discovers a source change is necessary; in that case stop and create a separate implementation task.

## Source-backed fact matrix / 源码事实矩阵

| Topic | Source anchor | Required wording |
|---|---|---|
| Cold discovery | `src/extension/mod.rs::ensure_discovered`, `src/extension/lifecycle.rs::DiscoveryMode::DeclarationsOnly` | Scans/parses/admits and declares Cold tool-only WASM without guest/provider startup or callable publication. |
| Explicit activation | `src/extension/mod.rs::ensure_loaded`, `src/extension/plugin_ops.rs`, `src/extension/runtime_owner.rs` | Existing owner triggers activation; activation is single-flight and remains outside the load guard while retaining proof ordering. |
| Provider lifecycle | `src/capability/provider.rs`, `src/extension/runtime_owner.rs` | State, generation and call admission remain provider/owner facts. |
| Callable publication | `src/tools/registry.rs` | Canonical per-plugin publication uses the existing registry; `new_with_provider` is not a universal plugin lifecycle authority. |
| Projection | `src/capability/projection_host.rs`, `src/capability/zahir_facade.rs` | Read/projection/notification only; no implicit activation. |
| Read callers | `src/gateway/handlers/plugins/handlers/manage.rs`, `src/hub/reconcile.rs`, `src/builtin_tools/plugin_manage.rs` | Plugin list/catalog/read faces use declaration discovery; explicit boot/control faces retain activation semantics. |
| Deferred boundary | `AGENTS.md`, `docs/reference/REDLINES.md`, current Stage H design | Feishu, remaining channels, Terminal, Memory, Sandbox, other kinds and universal resource claims remain deferred. |

## Tasks / 任务

### 1. Verify baseline and load constraints

- Confirm repository root, branch, HEAD, worktree list, remote relationship and clean/dirty state.
- Read `AGENTS.md`, `docs/reference/REDLINES.md`, current Stage H design and plan.
- Confirm the source anchors in the fact matrix before editing.

Expected result: the next session starts from `e759b3601` and no production code is needed for the documentation pass.

### 2. Build the stale-claim census

Search the reference corpus for claims such as:

- “no production provider transition”;
- “`new_with_provider` has no production caller”;
- “lazy activation is design-only / not wired”;
- “read paths activate providers”;
- “all providers or channels are dynamically loaded”.

Classify each hit as:

- current and true;
- historical and clearly dated;
- stale and requiring correction;
- deferred and requiring an explicit boundary.

Record the file, section, old claim, disposition and source anchor in the task report. Do not mechanically replace historical evidence that is still true for its original date and scope.

### 3. Correct `FEATURE_LOCATOR.md`

- Replace the stale 2026-10-22 current-wiring paragraph with the Stage H production facts.
- Keep the original provider lifecycle contract history, but add the Stage H declaration-only read boundary and explicit activation boundary.
- Update B6 wording so it does not contradict Stage H or imply that every read-side path is a boot activation path.
- Preserve the Feishu and remaining-kind deferred rows.
- Keep Chinese and English mirrors semantically equivalent.

Acceptance:

- No current paragraph says production has no lifecycle transition path.
- No paragraph claims all providers are dynamically loaded.
- `ensure_discovered`, `ensure_loaded`, and `set_plugin_enabled` have distinct documented roles.

### 4. Correct cross-reference documents

- In `ARCHITECTURE.md`, retain historical B5/B6 evidence only where the date/scope makes it historical; add a short current Stage H pointer where a reader could otherwise infer stale wiring.
- In `PLUGIN_SYSTEM.md`, check Chinese/English parity for the existing RuntimeOwner and compatibility-provider wording.
- Search and update only additional reference files that repeat the same stale claim.

Acceptance:

- Cross-document ownership vocabulary is identical.
- No projection component is described as an activation authority.
- Deferred providers and unproven resource claims remain explicit.

### 5. Validate the documentation change

Run:

```bash
git diff --check
rg -n -i "no production|没有生产|dormant|未接线|只做设计|不接线|new_with_provider" docs/reference
```

Review every remaining hit manually. Remaining hits must be historical, quoted, or explicitly deferred with a clear date/scope.

Check relative links added or changed with the repository’s existing link-check convention if available. Do not invent a new checker or dependency.

### 6. Independent review and handoff

Ask an independent read-only reviewer to inspect the complete documentation diff for:

- stale claims that remain current-looking;
- Chinese/English semantic drift;
- unsupported claims beyond the bounded Stage H evidence;
- accidental architecture changes hidden in documentation.

Fix only findings within this documentation scope. If the reviewer requests production changes, record them as a separate follow-up rather than expanding this task.

Final handoff must include:

- changed files;
- stale-claim census;
- source anchors used;
- validation commands and results;
- unmodified production behavior;
- deferred provider/resource claims;
- skipped Cargo, commit, merge, push and external/destructive actions.

## Review focus / 审查重点

1. One source of truth: documentation must name the existing owners rather than inventing a new authority.
2. Cold read safety: discovery, catalog, projection and notification must not be described as activation.
3. Explicit activation: only existing ExtensionManager/RuntimeOwner control paths may start the bounded provider.
4. Scope honesty: evidence is limited to tool-only WASM; no universal provider or resource reclamation claim.
5. Historical accuracy: preserve dated Stage F/G/B5/B6 evidence without allowing it to masquerade as current Stage H wiring.
6. Bilingual parity: Chinese and English must carry the same scope, status and limitations.

## Next follow-up / 后续任务

After this documentation audit is complete, create a separate plan for the repository-wide clippy/fmt baseline remediation. That work must first reproduce the six current clippy errors and the unrelated rustfmt drift, then decide whether to fix them as a maintenance batch; it is not part of this Stage H documentation task.
