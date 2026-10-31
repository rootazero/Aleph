# Next-session prompt — trusted project-scoped Panel publication
# 下一会话 Prompt — 可信 project-scoped Panel publication

继续实施 Aleph Panel 社区 UI 的下一阶段：在当前已合并到 `main` 的 production publication 基础上，为 discovery 与 HTTP asset delivery 增加经过认证的 project context。

## Starting point

- Repository: `D:/Workspace/Aleph`
- Current local `main`: `c385d131e649f0856c055101190ae129278d8c3b`
- Previous commits: `227a8e002` authenticated canonical route; `320c21737` browser credential/lifecycle; `c385d131e` validation/reference docs.
- Previous phase #2–#5 is complete and locally validated:
  - production-router integration: 2 passed, 0 failed;
  - native Panel/Terminal regression: 1603 passed, 0 failed;
  - fresh production-feature WASM/glue loader: 14 passed, 0 skipped;
  - real Gateway/Chrome fixture: 1 passed, 0 failed, driver exit 0.
- Read first: `docs/superpowers/plans/2026-11-01-panel-trusted-project-context-plan.md`, `docs/reports/panel-community-live-publication-validation.md`, `docs/superpowers/runs/2026-10-31-panel-http-publication.md`, `docs/reference/FEATURE_LOCATOR.md` §6.11, `docs/reference/TERMINAL_RUNTIME.md`.

## Objective

Replace the current deliberate Global-only limitation with a single trusted project context shared by authenticated `panel.capabilities.snapshot` discovery and `/tabs/...` delivery. Core remains the only declaration, immutable bytes, trust, scope and source-revision authority.

A missing project claim continues to mean Global-only. An explicit invalid, inaccessible, cross-user or revoked project claim must fail closed and must never fall back to Global. Do not infer a project from daemon CWD, filesystem root, URL path, query string, arbitrary header or client boolean.

## Hard boundaries

- Do not create a second Panel registry, bytes map, trust model, revision counter or project authority in Gateway or browser code.
- Do not use `authz::project_for(..., actor=None)` for a claimed HTTP project.
- Bind project access to the current authenticated user/device through existing server-side identity and project authorization APIs.
- Reuse `resolve_client`, `refuse_insecure_remote`, `OriginPolicy`, explicit bearer validation, current identity/wall checks and private rate limits.
- Keep discovery passive. Snapshot arrival must never fetch, import or mount.
- Preserve built-in Terminal public assets through `ControlPlaneAssets`; preserve exact `private, no-store` community responses and generic refusal.
- Keep project roots internal. Never serialize `PathBuf` roots to protocol or browser state.
- No install/update/uninstall, marketplace/signatures, default release switching, notifications, background refresh, multi-file assets, external WASM, native WebView QA or Zahir runtime mount in this phase.
- Before every Cargo command run `python qa/browser_live/check_memory.py` from the worktree root; run Cargo serially with `CARGO_BUILD_JOBS=1`. Prefer the independent production-library integration target if the monolithic Core lib-test harness hits the known LLVM OOM.
- Use an isolated short branch/worktree, English code comments, bilingual docs, RED before production code, and no merge/push unless explicitly requested.

## Required workflow

1. Use brainstorming first. Audit the existing ProjectStore/authz/connection/project APIs and the current RPC/HTTP route. Present two or three viable context-binding approaches, recommend one, and ask for the choice.
2. After the approach is chosen, write the bilingual design doc under `docs/superpowers/specs/`, run a fresh builder check, and obtain explicit design approval before code.
3. Use writing-plans, then executing-plans. Implement one task at a time with a meaningful RED test, production code, GREEN verification, ledger update and conventional commit.
4. Run a fresh read-only whole-branch review before final delivery. Disclose any SIGTERM/timeout and do not call it a clean review.

## Suggested task order

1. Typed authenticated project-context contract: explicit claim refusal, no-claim Global behavior, actor binding and fail-closed tests.
2. Core/RPC snapshot context: one Core accessor, authenticated server-derived context, Global/Project A/Project B/cross-user tests, passive discovery.
3. HTTP route context: same context into narrow route state, exact canonical URI and Core asset reads, project isolation/switch/revocation/replacement tests.
4. Browser selection and lifecycle: host-owned context, project switch fences snapshot/descriptor/asset/late result, real PanelLoader/PanelShell cleanup.
5. Real Gateway/browser fixture, docs, review and tracked-clean delivery.

## Acceptance

The phase is complete only when explicit project claims cannot cross project/user boundaries, invalid claims do not fall back to Global, discovery and delivery use the same trusted context, current Global behavior and Terminal remain green, and real production-library Gateway/browser evidence is recorded. If the existing APIs cannot provide a trustworthy actor-bound project proof, stop with an audit/design finding instead of adding a weaker endpoint.
