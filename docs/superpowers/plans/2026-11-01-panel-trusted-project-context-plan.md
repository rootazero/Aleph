# Panel trusted project context — next phase plan
# Panel 可信 Project Context 下一阶段计划

Date: 2026-11-01
Base: `main` at `c385d131e649f0856c055101190ae129278d8c3b`
Previous delivery: authenticated canonical HTTP publication and real Gateway/Chrome delivery (#2–#5).

## Purpose / 目标

The current publication path is production-proven but deliberately Global-only: contextless `panel.capabilities.snapshot` discovery and `/tabs/...` delivery both use `VisibilityCtx::default()`. Project-scoped Panel records remain in Core but cannot be reached through the HTTP/browser ingress. This phase should add one trusted project context that is derived from authenticated server state and used consistently by discovery and delivery.

当前 production publication 已通过真实 Gateway/Chrome 验收，但 discovery 与 HTTP delivery 都故意只使用 Global-only context。Core 中的 project-scoped Panel record 仍存在，却不能通过 HTTP/browser ingress 读取。本阶段为两条路径增加同一个由服务端认证状态派生的可信 project context。

## Goals / 目标范围

- Define one typed, server-owned project authorization context shared by snapshot discovery and asset delivery.
- Resolve the selected project through the existing authenticated caller identity and `ProjectStore`/project authorization APIs.
- Make invalid, missing, cross-user, inactive or inaccessible project claims fail closed; never fall back to Global when a project was explicitly claimed.
- Return project-filtered snapshots and serve project-scoped exact bytes through the existing Core `visible_to` and `panel_ui_asset` paths.
- Preserve Global behavior when no project is selected.
- Keep Core as the only declaration, bytes, scope and revision authority.
- Keep HTTP route state narrow; do not put a project registry or Panel publication map in `GatewaySharedState`.
- Add real RED→GREEN evidence for RPC discovery, HTTP delivery, scope isolation, project switching, stale revision fencing and browser selection.

## Non-goals / 非目标

- No install/update/uninstall, marketplace, signatures, default-release switching, live notifications, background refresh, multi-file assets, external WASM or Zahir runtime mount.
- No raw filesystem roots, arbitrary project paths, untrusted CWD inference or client-supplied authorization booleans.
- Do not weaken the existing Global-only route or browser credential checks while adding project selection.
- Do not claim native WebView QA or full workspace validation unless those commands actually run.

## Constraints / 约束

- Start from the merged local `main` at the base above. Create an isolated short branch/worktree; do not work directly in shared `main`.
- Read `AGENTS.md`, `docs/reference/FEATURE_LOCATOR.md` §6.11, `docs/reference/SECURITY.md`, `docs/reference/WORKSPACE.md`, `docs/reference/TOOL_SYSTEM.md`, `docs/reference/PROJECT_TOPOLOGY.md`, `docs/reference/MEMORY_SYSTEM.md`, the prior HTTP report/ledger, and the current project authorization APIs before coding.
- Keep `PanelUiCapabilitySnapshot` wire compatibility unless a design-approved field is necessary. Do not serialize raw `PathBuf` roots.
- Reuse `resolve_client`, explicit bearer authentication, current identity resolution, `OriginPolicy`, `refuse_insecure_remote`, private rate limiting and the Core `visible_to` derivation.
- The existing `authz::project_for(..., actor=None)` unrestricted compatibility path is not acceptable for an HTTP project claim. A claimed project must be checked against the authenticated actor.
- Before every Cargo command, run `python qa/browser_live/check_memory.py` from the worktree root. Run Cargo serially from its owning package directory with `CARGO_BUILD_JOBS=1`; prefer the independent integration target if the monolithic Core harness hits the known LLVM OOM.
- Use English code comments and bilingual repository documentation. Preserve ignored evidence and SDD ledgers.

## Design gate / 设计闸门

Before implementation, perform a focused audit and write a short design under `docs/superpowers/specs/` answering:

1. What is the canonical project selector: authenticated connection state, an explicitly requested project ID, or an existing server-side project/session binding?
2. How is the selector bound to the current authenticated user/device, and where is the proof created?
3. How do RPC discovery and HTTP asset delivery receive the same project context without duplicating authority?
4. What does no claim, invalid claim, project switch and revoked project access do to discovery, asset reads and current PanelShell state?
5. How are project replacement and source revision fenced across the two ingress paths?

Run a fresh builder check after writing the design. Obtain explicit design approval before implementation. Then use the writing-plans and executing-plans workflows with one RED→GREEN task at a time.

## Implementation sequence / 实施顺序

### Task 1 — Authenticated project context contract

- Add the smallest failing tests for a typed project context and explicit claim refusal.
- Prove that loopback, shared token, device token and stale WS state cannot manufacture project access.
- Prove that a missing claim means Global-only, while an invalid or unauthorized explicit claim refuses rather than falling back.

### Task 2 — Core and RPC discovery

- Add a crate-internal Core snapshot accessor accepting the approved trusted context, reusing the existing publication state.
- Extend `panel.capabilities.snapshot` only through the authenticated handler path; never accept a bare filesystem root or unauthenticated project ID.
- Test Global, Project A, Project B, absent project, invalid project and cross-user project cases.
- Keep discovery passive: no fetch, import or mount on snapshot arrival.

### Task 3 — HTTP route project delivery

- Pass the same server-derived context into the narrow Panel route state.
- Keep canonical URI matching and exact Core identity/entry/revision derivation unchanged.
- Test project A/B isolation, switching, missing records, replacement/retraction, token revocation, origin/TLS/proxy guards and generic responses.
- Preserve unauthenticated built-in Terminal delegation and exact `private, no-store` behavior.

### Task 4 — Browser project selection and lifecycle

- Add the smallest host-owned project selection state needed to bind discovery and delivery to the same context.
- Ensure a project switch fences old snapshots, asset requests, descriptor state and late loader results.
- Test explicit project selection, invalid project refusal, disconnect/reconnect, replacement/retraction and one-shot cleanup with the real `PanelLoader`/`PanelShell`.
- Keep project identifiers out of module/SDK credential arguments and never expose project roots.

### Task 5 — Real integration evidence and documentation

- Extend the independent production-library Gateway target with two real projects and separate scoped Panel records.
- Exercise actual authenticated `/ws` snapshot and `/tabs` requests through the production router; do not add a public test shim or fake JSON-RPC endpoint.
- Run the fresh production-feature WASM/Chrome fixture for project switching and late-result fencing where the environment supports it.
- Update §6.11, `TERMINAL_RUNTIME.md`, the validation report and a new run ledger with exact pass/fail counts and deferred limits.
- Run a fresh read-only whole-branch review focused on actor binding, project claim fallback, duplicate authorities, scope isolation, revision fencing and Terminal preservation.

## Acceptance / 验收标准

- No explicit project claim can read another user’s or another project’s Panel bytes.
- Invalid explicit project claims refuse generically and never expose Global assets as a fallback.
- No claim preserves the current Global-only behavior.
- Discovery and delivery use the same authenticated project context and Core publication state.
- Existing authenticated Global route, built-in Terminal assets, passive discovery, exact-byte SRI loader and one-shot cleanup remain green.
- The real production-library Gateway integration and browser fixture provide fresh evidence for the new project path, or the report records the exact environmental blocker.
- Worktree is tracked-clean and no merge/push occurs until explicitly requested.

## Deferred after this phase / 本阶段之后仍延后

Native WebView runtime QA, install/update/uninstall, marketplace/signatures, default release policy, live notifications, background refresh, multi-file graphs, external WASM and Zahir runtime mount remain separate phases.
