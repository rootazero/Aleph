# Phase7 acceptance and local-main handoff / Phase7 验收与 main 交接

Date / 日期: 2026-11-15.

**Disposition / 判定: approved bounded Phase7 PASS WITH RECORDED LIMITS.** Current-source QUALITY is PASS WITH RECORDED P2 LIMITS; terminal-evidence QUALITY is PASS WITH INDEPENDENT CORRECTIONS. No substantiated remaining source P0/P1 or failed required threshold. / 批准范围已完成，不等同于所有 backend、原生桌面、合并后的发布或任意下一阶段实施验收。

The user explicitly authorized writing the next-session prompt/plan, committing this completed task and merging it into **local main**, then restarting from main. This supersedes the earlier no-commit/no-merge restriction only for this closeout. It does not authorize push/publication, deletion of old resources or unrelated remote integration. / 本次提交与本地合并已有明确授权；保留原分支、原始证据和构建目录。

## Restart entry points / 重启入口

- **Copyable prompt / 可复制 prompt:** [2026-11-15-phase8-nsh-main-restart.md](../prompts/2026-11-15-phase8-nsh-main-restart.md).
- **Next-session plan / 下一会话计划:** [2026-11-15-phase8-integration-security-and-nsh-restart.md](../plans/2026-11-15-phase8-integration-security-and-nsh-restart.md).
- **NSH master / NSH 总纲（全文必读）:** [2026-11-05-nsh-desktop-refactor-design.md](../specs/2026-11-05-nsh-desktop-refactor-design.md).
- [Approved continuation / 已批准后续阶段表](../plans/2026-11-12-session-ui-readiness-and-phase7-continuation.md), [Capability Phase7 master](../plans/2026-11-05-phase7-advanced-capabilities.md), [I3 plan](../plans/2026-11-07-phase7-i3-production-integration.md).

Find the actual containing closeout commit after merge with `git log -1 --format='%H %s' -- docs/superpowers/runs/2026-11-15-phase7-acceptance-and-main-handoff.md`. Check `git merge-base --is-ancestor <that-commit> main`. Do not copy the pre-commit baseline below as the new HEAD. / 以 Git 查询真实交接提交及 main 祖先关系，不在本文自引用尚未生成的提交号。

## Frozen source and portable evidence / 冻结源码与随仓证据

Original feature branch: `fix/session-ui-readiness`; pre-closeout main/HEAD `d5e835d18ff6329ee31629548661224d33d579c9`. One checkout. Final accepted source manifest has 178 relative Rust paths; its SHA is an inventory identity, not a claim that 178 paths were changed by one patch. / 原始 main 为验收基线，提交后 HEAD 会前移。

| Evidence / 证据 | Repository copy / 随仓副本 | SHA-256 / 身份 |
|---|---|---|
| Final independent review | [independent-review.md](2026-11-15-phase7-closeout/independent-review.md) | `e99bb5292273482bf3bc07e90dd0c5b1a0d79351b703ae73ad3181ec44892d0a` |
| Exact v26 source manifest | [source-manifest.json](2026-11-15-phase7-closeout/source-manifest.json) | `0aca3c9595789c8a295edc48d901daa2b4d2dc77497a787025bba2fe9164c36c` |
| NSH v0.5 upstream audit | [nsh05-audit.md](2026-11-15-phase7-closeout/nsh05-audit.md) | `0be93eedcf3e002d4dc0852e971754f3d1ab8773db2268f59f3b7052e71d1290` |
| Independently corrected production coverage | [coverage-reconciliation.json](2026-11-15-phase7-closeout/coverage-reconciliation.json) | Preserves original report/analysis/LCOV hashes; only retained LCOV path is made repository-relative |

The first three copies are byte-identical to the original files. Large raw logs, LLVM profiles/LCOV, binaries, targets and sandbox homes remain local under `.superpowers/sdd/2026-11-12-session-ui-readiness-and-phase7-continuation/`; they are not added to Git. On another machine, absence of those files is missing raw evidence, not permission to fabricate a runtime receipt. / 核心审查、清单、覆盖率及审计可随 Git 保留，原始大体积/私密运行材料仍留原处。

Supplemental source hashes:

- `interfaces/webchat/scripts/build-panel-tabs.mjs`: `8737677cf2fc1f568c845edb5202fbe78c10018b5a155cb56430338b27b30fde`.
- `interfaces/webchat/tests/panel-build.test.mjs`: `4d08a89dfeb766b34077fd0b7f9625211f73ac8c952527b3918d8283e993679d`.
- Final server binary: `56427f1a9b300dc20b8dfd1e17915abfbaaffc5ea54afef5c1611d65eb52a9d8`.
- Final Panel package build ID: `a0550c3d0d2b4c69`; index decoded SHA `e50ef6479741e97e44c51408777855cffef4ebbdae9586e79fe30ddf858aa9b5` (2358 bytes; gzip1273/br964).

## Resulting behavior / 本次结果

Contract A is implemented through one canonical fallible session-preparation path: actual `chat.send`/`agent.run` prepare addressed metadata and validate returned incumbent visibility before enqueue/success; existing execution-key preparation and `/btw` response-versus-side-key semantics remain explicit. Valid rows may remain after later failure. No ready latch/map, second SessionStore or fabricated inline branch was introduced. / 保留 canonical authority，未把 metadata readiness 扩大成模型或渲染器就绪承诺。

Other accepted fixes include owned PTY Close remaining reachable when new terminal work is disabled; actual production static service choosing packaged `.br/.gz` sidecars; whole frontend build provenance; per-store metadata file-write serialization; real FIFO pre-open rejection; approval cleanup/generation correctness and marketplace traversal rejection; strict lint fixes. Public layout changes `PolicyDenial::PrincipalMismatch.actual: Box<OwnerRef>` and `GrantChange::Created.grant: Arc<Grant>` were explicitly approved. / 公开字段变化已授权，不是下一会话待决事项。

## Terminal gate accounting / 终态门槛

| Gate / 门槛 | Actual final result / 真实结果 | Original local evidence label / 本地标签 |
|---|---|---|
| Strict lint | workspace/all-targets Clippy `-D warnings`, natural0,392.507s | `p7-strict-lint-workspace-v26` |
| Complete default-feature workspace | natural0,2200.686s; **27,649 passed/0failed/56ignored/0measured/0filtered**,213 outer footers | `p7-workspace-current-source-v6`; 3 nested probes and9 custom harnesses separate |
| Formal server build | natural0,385.601s | `p7-final-server-build-v2` |
| Fresh replacement verification check | workspace/all-targets offline/locked/jobs1 natural0,489.268s | `p7-final-direct-verify-v3-result.json`; all seven recorded checks true, coverage interpretation corrected below |
| Production resources and PTY | 16 real static gzip/br responses match decoded disk hashes; actual attach/input→shell output/resize/close; late resize rejected `-32099`; server0/group empty | `p7-production-browser-final-v1/result.json` |
| Same-binary I3 |25/25, **PASS_FOR_LISTED_SCENARIOS_ONLY**; server/provider joined, owned groups empty | `p7-final-runtime-v1-result.json` binds I3 receipt |
| Policy and emitter controls | Each baseline1pass→compiled/executedRED1fail→exact restore/GREEN1pass; no compiler RED substitution | `p7-policy-behavior-final-v1/result.json`, `p7-emitter-behavior-final-v1/result.json` |
| Renderer ACK control | Final package1pass→retained built ACK mutant1fail→final package1pass; explicitly a replay | `p7-renderer-retained-replay-v1-result.json` |
| Test provenance / 四项交付 |112 distinct current passing new-name cases vs pre-Task1 `d54cedf65255c51b140d392f5807fbe88cbff9ae`, ignored excluded; activation/constraints, special UI/PTY, events/audit, optimization all accepted | `p7-phase7-test-provenance-v26-v2.json`; count supplemented by semantic review/execution |
| Frontend tests |44passed/0failed/1optional generated glue skipped (`ALEPH_PANEL_BRIDGE_GLUE` absent) | Final Panel package-v2 provenance suite |

All cited final Cargo commands used immediate4GiB memory gates, isolated targets,offline/locked/jobs1 and owned bounded process groups; natural terminal results and cleanup are recorded. No paid external LLM or Simulated substitute was used for production runtime proof. / 未把取消标志、旧 binary 或 QA 代理压缩当作生产验收。

### Coverage correction / 必须沿用的覆盖率更正

**Valid final production coverage: 66 executable files,14,311/16,193=88.37769406533687%,missing0, unchanged threshold80% PASS.** Original artifact scanner mishandled Rust lifetimes, `cfg(test)` item/field delimiters and test-only `census.rs`; its **90.724%,19,385/21,367,67 production-file** claims are withdrawn. Raw retained LLVM data is usable. Parent reran the independent review appendix read-only, exactly reproducing corrected values on the unchanged178-source manifest. The appendix is scoped frozen-source analysis, not a certified general Rust parser. / 旧 checker 待修；后续新源码覆盖率不能继续使用旧错误算法。

The unlinked/incompatible orphan `src/capability/session.rs` and declaration-only `src/tools/scoped/traits.rs` retain explicit non-executable scope dispositions, not fake mapped passes. Original checker, receipts and thresholds remain unmodified. / 不把未挂载孤儿或声明文件虚报已覆盖。

### Performance / 性能

Actual release `capability_resolver` benchmark,1024tools/32fields,natural0,1072.319s,empty group: warm438.67ns (437.47–439.74ns); invalidate/cold7.3915μs (7.3738–7.4096); namespace describe7.5071ms (7.4718–7.5356); fresh resolve7.0745μs (7.0384–7.1041); LRU128over64 point7.5600μs; FIFO point7.5273μs. These are different workloads, not an old/new A/B speedup or invented release threshold. / 各场景单独报告，不跨工作量相除。

## P2s and unperformed scope / 遗留与未验证范围

1. **Server-reachable simulated transcript failure:** `SimpleExecutionEngine` is constructed in the real no-provider branch at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569`, erased at`:1572`,mounted in fallback GatewayContext at`:1618`. Installed-service `emit_event` failures in `src/gateway/execution_engine/simple.rs:147` and`:235` may still reach RunComplete at`:265`. Earlier “no server constructor” assertion is withdrawn. P2 remains; Contract A's canonical metadata check at`:89–101` is distinct from simulated transcript durability. Future fixes need real installed-error/terminal/idle/cleanup tests without global singleton poisoning.
2. Metadata mutex is per store instance/process, not cross-instance/process exclusion; PTY error presentation remains P2. Coverage parser requires repair before future automated acceptance.
3.56ignored and1optional glue skip are unexecuted, not passes. No all-features, native Linux/Windows, remote deployment, built-in same-port candidate routing, every-kind/backend, SafeReplay, external-effect exactly-once or universal scheduler acceptance.
4. Configured raw targets keep canonical fail-closed admission; unconfigured legacy raw behavior remains as approved. Renderer display ACK does not create a new PTY ACK schema. D-T=A is PTY attach/input/resize/close, not separate stdin/stdout/stderr streaming.

## NSH master and distinct next gate / NSH 总纲与独立门槛

The NSH master linked above is394lines, still draft and based on0.4 APIs. Historical local clone paths, line estimates and “validated” claims are not current0.5 evidence. Actual handoff inventory found `desktop/nsh-shell/` and `desktop/nsh-spike/` present, `interfaces/nsh-desktop/` absent; no `docs/adr/` directory at that location. Locate the actual ADR index and current upstream checkout rather than declaring them unavailable. / 不盲目重做已合入阶段，不将目录存在当端到端完成。

Pinned target v0.5.0: release `2026-11-11T12:04:05Z`,tag object `729ea4c6810112160e890684e8a8d34528ec6da9`,commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`. Official0.3/0.4/0.5 are not yet in the permanent compatibility record.0.5 replaces window APIs with WindowRef/Window/window kinds; validate matching CLI/crates/runtime/platform AppView/JS types,host channel/Tokio handle,shutdown/media grants/activation policy. Floors Rust1.97,macOS14+,Windows10 1809+,LinuxGTK4.14+/WebKitGTK6.0(2.40+). No local0.5compile/native launch was performed. / 上游审计不是本地原生兼容验收。

NSH Phases0–6:0compatibility;1shared transport/hello/events/sessions/supervisor;2OS integration;3real canonical chat;4full UI parity;5separate PluginRuntime after ADR prerequisites (especially006WASM/008trust);6Tauri removal only after Phase4 and two-week dual-run. Swift screenshot/voice/camera and i18n remain separately scoped. Full replacement direction is retained without early Tauri deletion. Check the explicit R2/native-UI ADR transition before changing the constitutional UI single-source rule. / 不把 Capability Phase7 通过冒充 NSH parity 或 PluginRuntime 批准。

**Next permitted work / 下一步:** execute the new plan's source-backed inventory and author real Capability Phase8 Integration & Security and separate NSH slice designs/plans, independent SPEC/QUALITY review, then human approval of concrete production changes. No Phase8/NSH production implementation is included in this closeout. Generic repeat confirmation of Contract A, execution method or the explicitly authorized local merge is unnecessary.

**Not done / 未做:** push/publication, Phase8 production implementation, local NSH0.5native verification/migration, Tauri removal, global installation/trust/checker/threshold changes or protected-config access. Original raw evidence and resources remain local and preserved. / 新会话按具体门槛继续，不继承无限制发布授权。
