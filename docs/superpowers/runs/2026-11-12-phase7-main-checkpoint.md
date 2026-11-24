# Phase 7 Main Checkpoint / 主线检查点

## Integration intent / 合并意图

The user explicitly requested a local merge to `main` before further session-publication/UI-attach investigation and a restart handoff. This is a development checkpoint, **not** a release, ship approval, or successful Phase 7 acceptance.

用户明确要求先将本会话工作合入本地 `main`，再调查 session publication / UI attach 并编写重启交接。这是开发检查点，**不是**发布授权、验收通过或 Phase 8-ready 声明。

## Included work / 纳入范围

- Existing Phase 7 commits and the accumulated I3 integration: shared Policy and special capability registry, actual PTY/UI ingress, scoped event/audit wiring, owned operation settlement, lifecycle fixes and focused regression/QA fixtures.
- The approved I3 design and implementation plan, preserving their historical status.
- No diagnostic-only external observer snapshot is copied into production source.

纳入已有 Phase 7 提交及 I3 的实际实现和测试；保留批准设计与实施计划。外部诊断变体及其额外观察器代码不回灌生产代码。

## Known non-green gates / 已知未通过项

1. E2's corrected diagnostic COPY server built successfully, with warnings. Its unchanged original `chat.send → immediate capabilities.ui.attach` scenario reached and failed with `-32099: session is unavailable`.
2. Normal 45-second stop and later UI/Memory/PTY acceptance were not reached in that failed scenario; failure-cleanup server exit `-9` is not graceful shutdown.
3. Previous full Core failures, strict lint, coverage, real wire-sever mutation checks, served compressed Panel identity and invalidated global-verification evidence remain open, failed or unmeasured as individually recorded. A fresh checkpoint is not proof they disappeared.
4. Application-wide state-machine census remains partial: 105 referenced application file containers, 3425 remaining paths, no semantically exhausted files. These are not machine counts or test coverage.
5. NSH 0.5.0 is the next compatibility target, not an already completed dependency upgrade or native build. E2's Core compile closure excludes NSH.

上述失败及未验证项全部保留；本合并不将旧失败回填为通过，不修改测试、门槛、权限或 checker 以取得绿灯。

## Verification boundary / 验证边界

The fresh declared runner detection reports `cargo check --all-targets`, `cargo clippy --all-targets`, pregate selection against `origin/main`, and **TRUST untrusted**. No trust record, threshold, repo context or policy is changed. Do not report the runner as green without a genuine current-tree result. The required pre-commit `/verify` invocation and its real outcome must be reported separately.

当前 runner detect 为 untrusted；不会自动 trust、伪造 Git 上下文或修改阈值。提交和合并是用户要求的已知未验收检查点，不是成功完成通用 finishing 工作流。不得伪称全量测试已绿。

## Preserved local state / 保留的本地状态

- Exclude the pre-existing `.claude/settings.json` modification.
- Preserve and do not commit, delete or overwrite test-generated root `config.toml` and `config.lock`.
- Preserve ignored `.superpowers/sdd/` history, external evidence, targets, feature branches and the existing clean `nsh-phase0-spike` worktree.
- No push, pull, remote publication, dependency installation or worktree deletion is part of this merge.

本地用户设置、测试副产物和所有历史证据均保留；只合并属于本会话的实现、测试和文档。不 push、不 pull、不删除 worktree。

## NSH branch inventory / NSH 分支全量核对

- All local branch refs were inspected: `main`, `phase7-task2-special-capabilities`, `worktree-nsh-phase0-spike`; both existing worktrees were inspected. The spike worktree is clean and its tip is the current feature HEAD ancestor.
- A fresh read-only `git ls-remote --heads origin` found only `refs/heads/main`, no additional remote NSH task branch. Its observed tip was `bd0daaab437afbadc5d0bde68b6ca96990a7f26f`. This is not a pull, synchronization or remote-content review; do not claim local `main` is up to date with that remote tip.
- NSH-related reflog commit subjects were checked against local `main`. All fourteen non-stash historical NSH commits checked were already ancestors, including Phase 0–4 and connection fixes. Reflog subjects are not a complete proof of every historical deleted branch or remote history.
- Preserved `stash@{0}` (`02d0eecded00c434996b473411529d73f81a7b05`, on historical `nsh-desktop-phase1`) contains only two tracked changes. Its final blobs already exactly match the current checked-in files: `shared/protocol/src/system_hello.rs` = `935a459725607e43467e94cfe79de86b8e1ac10b`; `src/gateway/handlers/system_hello.rs` = `5e24ba59af5414677cdd115e4b553b5ef048fb90`. Do not apply or drop this already-integrated stash, or merge the stash object merely to manufacture ancestry.
- After the merge, verify both current feature branch tips are ancestors of local `main` and the two stash blob identities remain present. Preserve branch refs, stash and worktree directories.

已检查所有当前本地分支、worktree、实际远程分支名及 NSH 相关 reflog。历史 NSH 实现已在主线祖先中；唯一 NSH stash 的两个文件内容也已完整纳入，不重复应用。合并后仍须实查当前两个 feature tip 均被本地 main 包含。没有远程任务分支被遗漏，但不声称远程 main 已同步，也不声称穷尽了已删除且不可恢复的历史引用。

## Evidence pointers / 证据入口

- Historical local controller report: `.superpowers/sdd/2026-11-07-phase7-i3-production-integration/e2-census3-unified-report-20261112.txt` (ignored; not guaranteed in a fresh clone).
- Portable follow-up design, implementation plan and restart prompt are authored after this checkpoint merge. Their status is planning only; this session must not implement the proposed readiness fix.

完整本地收据仍在 ignored / 外部证据目录中；后续交接文档必须携带足够事实，不能依赖新 clone 自动拥有本机证据。

## Completed local integration / 实际本地合并

The actual checkpoint commit is `fa996e6407c1ba7adbdf871b9b4185a0257dafd8`, `feat(capability): checkpoint Phase 7 I3 production integration` (101 files). Local `main` fast-forwarded to it; merging the spike task branch reported already up to date. Both task tips are ancestors. No branch, worktree or stash was deleted and no push occurred.

Immediately before that product checkpoint commit, the required `/verify` runner exited 3 with `CHECKS_CONSENT_REQUIRED`: the detected commands were untrusted and **nothing was run**. No trust/configuration/policy was changed; the user-requested known-non-green checkpoint commit and local merge succeeded. This is not a fresh passing build/test/lint or completion of the standard all-green finishing workflow.

源码检查点已按要求先合入本地 main，再做只读调查；没有为了通过提交而绕过 hook、伪造 Git 或修改授权。已有 branch-switch/post-commit hooks 曾自动启动 graph rebuild，完成状态不作为源码或验收证据。

## Restart handoff / 重启交接入口

1. Design / 修复方案: `docs/superpowers/specs/2026-11-12-session-ui-readiness-design.md`.
2. Concrete task plan and canonical later roadmap / 逐任务计划与后续路线: `docs/superpowers/plans/2026-11-12-session-ui-readiness-and-phase7-continuation.md`.
3. Copyable request / 新会话复制正文: `docs/superpowers/prompts/2026-11-12-phase7-main-restart.md`.

The design proposes A, with **one unapproved product semantic choice**: legitimate metadata can commit for a no-run inline success and before a later queue/spend/run denial. A covers every addressed-key success from the two real handlers, not only their pre-spawn branches. The new session must confirm this choice and the plan before code. A document check is not human approval. No readiness implementation, new fixture compilation, model/service run, full-suite rerun or NSH dependency migration occurred while authoring these documents.

Capability Phase7 remaining acceptance, the actually named Phase8 Integration & Security planning gate, and the distinct NSH Phase0–6 roadmap are explicitly separated in the plan. NSH 0.5.0 is the preferred new compatibility target; current Core-only E2 evidence does not prove native NSH compatibility. GPUI remains deferred. Do not invent Phase8/9 implementation details or remove Tauri before the master roadmap's gates.
