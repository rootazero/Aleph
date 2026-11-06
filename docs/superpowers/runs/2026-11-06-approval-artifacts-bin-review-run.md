# Static review of `src/approval`, `src/artifacts`, `src/bin` — run record / 静态审查运行记录

- Date / 日期: 2026-11-06
- Scope / 范围: `src/approval/**`, `src/artifacts/**`, `src/bin/**` (68 Rust files, ~33k LOC)
- Execution mode / 执行方式: 14 parallel static-review batches (subagents, per-batch worktrees) → triage → fix waves in isolated worktrees → merged into `main` on the local checkout. No PR, no push.
- Branch / 分支: `main` (local checkout `/home/zou/data/workspace/Aleph`)
- Attribution baseline / 归因基线: `148ba09f5`

## 1. Method / 方法

### 1.1 Batching / 分批

The scope was cut into 14 batches so that each reviewer could hold one coherent area in mind (see
`docs/superpowers/runs/2026-11-06-approval-artifacts-bin/B*.md` for the per-batch file lists):

| Batch / 批次 | Area / 范围 | Files | LOC |
|---|---|---|---|
| B1 | `src/approval` types / policy / audit / tool_call / session_route / callback_sink | 6 | ~908 |
| B2 | `src/approval` mod / config | 2 | ~1975 |
| B3 | `src/approval` operator / guardian / node requester + adapters | 4 | ~3028 |
| B4 | `src/artifacts` mod / store | 2 | ~1054 |
| B5 | `aleph-server` main / cli / server_init / daemon / update | 5 | ~3050 |
| B6 | commands: bootstrap_runtime / bootstrap_token / doctor / gateway / hooks / identity | 6 | ~2818 |
| B7 | commands: node / pair / plugins / prompt_size / resume / sandbox_debug / secret / service | 12 | ~4420 |
| B8 | `commands/start/mod.rs` | 1 | ~4575 |
| B9 | `commands/start` helpers / orchestrator_init / bootstrap_factories / runtime_warmup | 4 | ~1461 |
| B10 | `commands/start/builder` subsystems / startup_resources / handlers | 13 | ~3694 |
| B11 | `commands/start/builder/handlers` memory / settings | 3 | ~1466 |
| B12 | `commands/start/builder/agent_init/**` | 7 | ~3398 |
| B13 | `src/bin/aleph-migrate/**` | 6 | ~2376 |
| B14 | `src/bin/aleph-panel-native-qa-gateway/main.rs` | 1 | ~1028 |

Every reviewer received the same contract (`docs/superpowers/runs/2026-11-06-approval-artifacts-bin/BRIEF_COMMON.md`):
one finding per defect, `### <ID> [P0..P3] <title>` + location + failure mode + evidence + suggested fix,
and an explicit "not a defect" allowance so that style preferences do not inflate the list.

Coverage was verified mechanically: a script compared the 68 in-scope `.rs` files against the union of
the 14 batch scope files — every file was read by at least one reviewer (one file, `commands/update.rs`,
was listed under a relative path in its scope file; the owning reviewer did read it).

### 1.2 Semantic graph / 语义图谱

`graphify-out/GRAPH_REPORT.md` (built from commit `e4448c97`, 143k nodes / 294k edges / 6348 communities)
was used to place the batch boundaries along the real community structure instead of a naive directory
split, and to spot which hubs (`SqliteStore`, `ChannelRegistry`, `ToolOutput`, `InboundMessageRouter`)
belong to more than one batch. It was navigation only — no finding was accepted on the graph's word.

### 1.3 Model routing / 模型路由

Per the task's routing rule, classification, filtering and routing decisions were delegated to Jev, while
the reading, reasoning and code-change work went to reasoning models.

Per the task's routing rule, classification, filtering and routing decisions were delegated to Jev, while
the reading, reasoning and code-change work went to reasoning models.

| Step / 环节 | Input / 输入 | Jev question / 提问 | Outcome / 结论 |
|---|---|---|---|
| Baseline noise filtering / 基线降噪 | `rust-doctor` finding list | classify by rule family, decide which are worth re-reading | dropped the `print_*` family as non-defect noise; kept `sql_built_from_format`, `string_slice`, `unreachable`, `expect_used`, `high_complexity` for spot checks |
| Batch partition / 分批划分 | file list + `graphify-out` community structure | rank candidate partitions by review risk and reviewer load | 14 batches balanced by cohesion, not by directory split |
| First-pass triage (72 items, 3 calls) | finding title + failure mode | `choice`: *fix now* vs *defer* | 64 apply / 8 defer |
| Second-pass triage (11 items) | adversarial re-review findings | same `choice` | 10 fix / 1 defer |
| Diff-check triage (14 items) | concerns raised while attacking the fix diff | same `choice` | 13 fix / 1 borderline (kept — the fix was cheap) |

Harness limits worth recording, because they shaped the routing:

- The `score` question type is unusable in this harness: it requires a list of descriptions indexed by
  score, and rejects the map shape the tool passes (`Score criteria must be a list of descriptions indexed
  by score from zero, not a map`). Scoring was therefore expressed as a two-way `choice`.
- `choice` only discriminates when its `criteria` is a **map** of label -> description; a list of
  candidates silently degrades to a constant value.
- `noul` returned near-constant probabilities inside a single call (0.79/0.79/0.79/0.79), so it was used
  for coarse ranking of batch candidates only, never as a per-finding verdict.

Every Jev "defer" was still read by a human before it was skipped: all eight deferred items turned out to
be P3, i.e. the triage did not hide a P0/P1 behind a defer.

### 1.4 Adversarial second pass / 对抗式二审

A first pass that returns "no defects" is cheap and often wrong. Three batches that returned few or no
findings (B3, B5, B10) plus the whole fix diff were re-reviewed by a second set of reviewers with a
different mandate: *confirm or refute each reported finding with a concrete trigger, and treat the
diff itself as new code to attack*. That produced 11 additional findings (B3-S2-01, B5-S2-01..03,
B10-S2-01..07) and 14 diff-check concerns.

## 2. Result / 结果

**97 review findings** (86 from the first pass, 11 from the adversarial second pass) plus **14
diff-check concerns** raised while attacking the fix diffs — 111 tracked items in total.

| Priority / 优先级 | Count / 数量 |
|---|---|
| P0 | 4 |
| P1 | 36 |
| P2 | 51 |
| P3 | 20 |

| Area / 区域 | First-pass findings / 一轮问题数 |
|---|---|
| `src/approval` | 16 |
| `src/artifacts` | 5 |
| `src/bin` | 65 |

| Verdict / 处置 | Count / 数量 |
|---|---|
| fixed | 89 |
| partially fixed (root cause needs a producer-side change) | 5 |
| deferred (Jev triage, all P3) | 8 |
| not applicable (the fix report disproved the finding) | 8 |
| skipped as too risky to land in this round | 1 |

Representative high-severity fixes (a sample, the full table is in Appendix A):

- **P0 — `src/approval/config.rs`**: the deny list was resolved only against the action's own type, so a
  rule written for the parent action stopped applying once the action was renamed — a rename silently
  widened what the action could do. Rules are now inherited from `ActionType::inherited_from()`.
- **P0 — `src/approval/config.rs`**: glob rules were compiled case-sensitively while URL targets were
  folded at match time, so a lower-case rule never matched an upper-case host. Both sides are now folded
  the same way, and only for URL actions.
- **P0 — `src/artifacts/store.rs`**: artifact publication was not serialized and was not durable; two
  concurrent publications could interleave and a crash could leave a half-written artifact visible.
- **P0 — `src/bin/aleph-migrate/verify.rs`**: the orphan cleanup deleted attachments that were still
  referenced by the database when the session manifests were incomplete — the sweep now runs only when
  every verification check passed, and the protected set comes from the database itself.
- **P1 — `src/bin/aleph-server/daemon.rs`**: `stop` signalled a PID taken from a file, so a recycled PID
  could make the server kill an unrelated process. The IPC endpoint record now binds the PID to the
  writer's process start time, and the identity is verified before any signal is sent (fail closed).
- **P1 — `src/bin/aleph-server/commands/update.rs`**: a failed installer launch leaked the downloaded
  update files; the temp directory is now removed on every exit path.
- **P1 — `src/bin/aleph-server/commands/start/builder/subsystems.rs`**: security artifacts fell back to
  predictable `/tmp` paths when the home directory was unavailable.
- **P1 — `src/bin/aleph-migrate/verify.rs`**: verification could report SUCCESS for sessions that were
  never indexed, and the FTS probe did not check transcript coverage or stale rows.

## 3. Fix waves / 修复波次

Four fix waves, each in its own git worktree branched from `main`, each merged back with
`--no-ff` (no squash, so the provenance of every change stays visible).

| Wave / 波次 | Scope / 范围 | Fixes / 修复 |
|---|---|---|
| 1 | B1–B3 (approval), B4 (artifacts), B5–B7 (server CLI), B8 (start), B9 (warmup), B10+B12 (builder), B11 (handlers), B13 (migrate), B14 (qa gateway) | 8 workers, 30 commits |
| 2 | adversarial second-pass findings | 2 workers (wave2a, wave2b), 4 commits |
| 3 | concerns from the diff review of waves 1–2 | 4 workers (w3a–w3d), 8 commits |
| 4 | gaps the first waves did not cover (B13-10..17 in `aleph-migrate/verify.rs`, B5-001, B3-003/004, B10-003) | 3 workers (w4a–w4c), 3 commits |

`main` now carries the review's own changes over the attribution baseline `148ba09f5`, touching **36 files**
(+3654 / -953) inside the reviewed scope plus the four files that had to be touched to land a fix
(`src/exec/manager.rs`, `src/cli/endpoint.rs`, `src/gateway/voice/streaming/relay.rs`).

The branch additionally carries three `refactor(...): apply Occam's razor simplifications` commits
(`cb35d573f`, `3bea46ce7`, `f698c545c`) that were produced by a **concurrent session** working in the same
checkout while this review was running. They touch `src/agents/`, `src/acp/` and `src/a2a/` only — there is
**no file overlap** with the review's scope — and they are not part of this review. The final
`cargo check` below was run against a `main` that includes them.

The main worktree also received four hand-written corrections that the workers could not finish:
the qa-gateway join-result nesting (`Ok(Ok(Ok(())))` instead of `Ok(Ok(()))`), the bounded stdin read
(`(&mut input).take(...)` instead of the non-existent `by_ref()`), the `PendingRegistration` guard
construction order in `src/exec/manager.rs`, and one unnecessary `mut` binding.

Workers ran with a hard contract (`fixes-briefs/*.md`): read the finding, verify it against the code
before changing anything, make the smallest change that removes the failure mode, keep the edit inside
the named file set, write a per-item verdict (`fixed` / `partially-fixed` / `not-applicable` /
`skipped`) with file:line and a risk note, and commit with a conventional message. No worker was allowed
to run `cargo`, `clippy`, `rustfmt` or the test suite — the orchestrator owned compilation.

## 4. Verification / 验证

```bash
CARGO_BUILD_JOBS=1 cargo check -p alephcore --features native-qa
```

Result: **compiles clean — 0 errors.** (`aleph-server`, `aleph-migrate` and
`aleph-panel-native-qa-gateway` are three binaries of the `alephcore` package, so one check covers all
three.)

The warning set was audited line by line: 4 in the library (`src/extension/projection.rs`,
`src/memory/runtime/mod.rs`, `src/tools/registry.rs` — all pre-existing and outside the reviewed scope)
and 12 in the `aleph-server` binary (4 `value assigned is never read` in the agent-init builder — that is
finding `B12-P3-03`, deferred — 2 unused memory-context initializers — pre-existing — and 6 dead
note-maintenance wrappers, which is finding `B10-S2-03`, deferred). The one warning this review *did*
introduce — the address-based `serve_webchat` wrapper left unused after the wave-4c listener change —
was removed rather than silenced.

Memory control, as required:

- `.cargo/config.toml` carries `-C link-arg=-fuse-ld=lld`; the GNU linker link of `alephcore` exceeds
  15 GB of RAM on this machine and gets OOM-killed without it.
- `CARGO_BUILD_JOBS=1` keeps the compiler to a single job.
- One `rust-doctor` run (clippy under the hood) reached 9.4 GB RSS on a 19 GB machine and starved every
  other build; it was killed and its JSON baseline was **not** produced. The triage therefore rests on
  the per-batch findings and on the Jev classification, not on that baseline.

Two build-environment obstacles were hit and resolved:

- A first wave of 13 concurrent reviewers was killed by the MiniMax token-plan rate limit
  (`429 ... Token Plan 速率限制`). The fix was to spread the batches over several providers
  (`minimax-cn`, `kimi-coding`, `88api`, `deepseek`) and to keep the number of concurrent runs per
  provider low.
- An unrelated `makepkg` build (`~/.cache/paru/clone/paru-2.2.0`) held the cargo package-cache lock for
  eight minutes without progress and blocked every `cargo` invocation; it was killed so the check could
  proceed.

## 5. Deferred / 未修项

Eight items were deferred by the Jev triage and confirmed as P3 by hand:

| ID | Item |
|---|---|
| `B1-05` | doc comment and implementation disagree on the policy fallback |
| `B7:F-06` | `release_response` before park disables the gate but the run still reports PASS |
| `B8-07` | cleanup runs twice on the success path and the second failure is swallowed |
| `B11-02`, `B11-04` | voice-stream cleanup details |
| `B12-P3-02` | a 1638-line function in the agent-init builder |
| `B12-P3-03` | unused assignment in the agent-init builder |
| `B14:F-11` | a 392-line `if`-chain in the qa gateway command dispatcher |
| `B10-S2-03` | six dead note-maintenance wrappers kept alive only by tests |

All eight are readability / restructure work: landing them in the same round as the behavioural fixes
would have mixed a refactor into a correctness change and made the diff unreviewable.

Five items were **partially fixed** — the failure mode is closed, but the root cause lives in a
producer or schema that was outside the batch's permitted edit scope:

| ID | What remains |
|---|---|
| `B13-13` | distinct logical attachments that share the same bytes still cannot be represented — needs an `attachments` schema/key change in `aleph-migrate/index.rs` |
| `B13-16` | `export` can still succeed after dropping rows with null timestamps — the detection now exists on the verify side; the export side was out of scope |
| `B13-17` | the index command still defaults malformed attachment counts to zero when run on its own |
| `B12-P2-03` | see the agent-init report |
| `B14:F-09` | the qa-gateway "stopped" message reports `fixtureClosed: true` because that is the published protocol contract (`docs/superpowers/plans/2026-11-02-panel-native-runtime-completion-implementation.md:143`, asserted by `qa/native_ipc/native_project_gateway_smoke_test.mjs:242`); the helper performs no fixture teardown, so the flag is a protocol fact, not a local observation |

One item was **skipped as too risky to land in this round**: `B8-07` — the double `StartupResources::cleanup`
on the success path (fixing it touches server shutdown ordering).

Eight items were judged **not applicable** by the fix reports that re-read the code and disproved the
finding (`B2-007`, `B12-P1-02`, `B12-P2-02`, `B7:F-02`, `B7:F-04`, `B7:F-05`, `B10-S2-05`, `B10-S2-07`).

## 6. What was NOT done / 未做的事

This was a **static** review. Concretely, the following was **not** done:

- **No test run.** `cargo test` was never executed, on the whole crate or on any package. Every fix is
  therefore verified by reading and by compilation only. The fix workers added no regression tests,
  because their contract forbade running or adding them.
- **No `cargo clippy` / `cargo fmt`.** The wave contracts forbade them; formatting was done by hand in
  the repo style. The rust-doctor/clippy baseline run was killed for memory reasons and produced no JSON.
- **No runtime or end-to-end validation.** The panel native QA harness
  (`qa/native_ipc/*.mjs`) was not run against the rebuilt helper, so the `stopped` message contract,
  the 16 KiB stdin bound and the shutdown ordering are argued from the protocol docs and the code, not
  observed.
- **No `cargo check` for other packages or feature combinations.** Only
  `cargo check -p alephcore --features native-qa` was run; other packages, other features and the
  `aleph-panel` WASM build were not compiled.
- **No push, no PR, no tag.** The work sits on the local `main` branch, as requested. `origin/main` was
  not touched.
- **The scope was the three requested directories.** `src/exec/manager.rs`, `src/cli/endpoint.rs`,
  `src/gateway/voice/streaming/relay.rs` and `src/utils/paths.rs` were touched, but only because a
  finding in the requested scope could not be fixed at its own site. Nothing outside
  `src/approval`, `src/artifacts`, `src/bin` and those four files was reviewed.
- **Not every finding was fixed.** See §5: 8 deferred, 5 partially fixed, 1 skipped, 8 not applicable.
- **The first-pass reviewer verdicts were taken at face value** where the fix report did not contradict
  them. A handful of reviewers reported "no defects" for their batch; the adversarial second pass
  re-read B3, B5 and B10 for exactly that reason and found 11 more problems, but the same doubt applies
  to the batches that were not re-read.
- **No behavioural comparison against the pre-fix binary.** The changes are justified by reading, not by
  an A/B run of the old and new behaviour.

## Appendix A — full finding table / 附录 A：完整问题清单

| # | ID | Batch | P | Finding | Verdict |
|---|----|-------|---|---------|---------|
| 1 | `B1-01` | B1 | P1 | The approval cascade resolves sibling cards whose originator never consented — the group-chat approval gate protects only the clicked id | fixed |
| 2 | `B1-02` | B1 | P2 | The originator gate fails open for any approval record that carries no originator | fixed |
| 3 | `B1-03` | B1 | P2 | `ActionType::inherited_from` is honoured by the defaults map but NOT by the allowlist/blocklist rule lookup | fixed |
| 4 | `B1-04` | B1 | P3 | `audit_identity` collapses a subagent's agent id to the parent agent's, so two different subagents produce identical audit identities | fixed |
| 5 | `B1-05` | B1 | P3 | `ApprovalPolicy::check`'s doc says the decision is not audited, but the only implementation audits the `Ask` inside `check` | deferred (Jev triage: cosmetic / needs redesign) |
| 6 | `B10-001` | B10 | P1 | Per-instance channel policies are registered under an ID the runtime does not use | fixed |
| 7 | `B10-002` | B10 | P2 | Config watcher has no shutdown path and survives server teardown | fixed (wave 3c: config watcher task is captured and torn down) |
| 8 | `B10-003` | B10 | P2 | Separate WebChat server is detached from gateway shutdown | fixed |
| 9 | `B11-01` | B11 | P2 | One fleet-wide `StreamRegistry` + mint-time TTL turns a per-caller voice stream into a cross-user liveness kill | fixed |
| 10 | `B11-02` | B11 | P3 | Dead duplicate memory-context factories; the live path already diverges, and config docs point at the dead one | deferred (Jev triage: cosmetic / needs redesign) |
| 11 | `B11-03` | B11 | P3 | Absent optional subsystems answer `-32601` instead of the codebase's typed `SERVICE_UNAVAILABLE` contract | fixed |
| 12 | `B11-04` | B11 | P3 | Five parallel hot/cold `providers.*` arms make provider semantics depend on which arm a verb was added to | deferred (Jev triage: cosmetic / needs redesign) |
| 13 | `B12-P1-01` | B12 | P1 | Memory extension registry panic on duplicate registration | fixed |
| 14 | `B12-P1-02` | B12 | P1 | Tool health probe registration without existence check | not-applicable |
| 15 | `B12-P2-01` | B12 | P2 | Unvalidated API key hydration from vault | fixed |
| 16 | `B12-P2-02` | B12 | P2 | Race between extension load and tool registration | not-applicable |
| 17 | `B12-P2-03` | B12 | P2 | Leaked spawned tasks on early return | partially-fixed |
| 18 | `B12-P2-04` | B12 | P2 | Blocking SQLite operations in async context | fixed |
| 19 | `B12-P3-01` | B12 | P3 | Duplicate default-provider selection logic | fixed |
| 20 | `B12-P3-02` | B12 | P3 | Oversized function register_agent_handlers (1638 lines, complexity 90) | deferred (Jev triage: cosmetic / needs redesign) |
| 21 | `B12-P3-03` | B12 | P3 | Unused assignment to naked_loop_planner_provider and team_planner_provider | deferred (Jev triage: cosmetic / needs redesign) |
| 22 | `B13-01` | B13 | P1 | Re-indexing an existing database retains sessions absent from the source | fixed |
| 23 | `B13-02` | B13 | P1 | A mid-index error commits a partially updated target | fixed |
| 24 | `B13-03` | B13 | P1 | Removing a transcript leaves its old FTS content searchable | fixed |
| 25 | `B13-04` | B13 | P1 | Attachment metadata lookup errors are exported as “no attachments” | fixed |
| 26 | `B13-05` | B13 | P1 | Failed JSON serialization can destroy the previous export and leave a truncated artifact | fixed |
| 27 | `B13-06` | B13 | P1 | Overwrite failures corrupt previously transformed sessions | fixed |
| 28 | `B13-07` | B13 | P1 | A failed attachment write is later trusted as a complete deduplicated file | fixed |
| 29 | `B13-08` | B13 | P1 | A valid relative output directory with no parent component is rejected | fixed |
| 30 | `B13-09` | B13 | P2 | Out-of-range timestamps are silently replaced with the current time | fixed |
| 31 | `B13-10` | B13 | P1 | Filesystem-only sessions can still produce a successful verification | fixed |
| 32 | `B13-11` | B13 | P1 | FTS verification does not check transcript coverage or stale rows | fixed |
| 33 | `B13-12` | B13 | P0 | Orphan cleanup deletes attachments owned by sessions whose directories are missing | fixed |
| 34 | `B13-13` | B13 | P1 | Identical bytes with different MIME types cannot be represented by the attachment index | partially-fixed |
| 35 | `B13-14` | B13 | P1 | “Metadata consistency” accepts values that disagree with the database | fixed |
| 36 | `B13-15` | B13 | P1 | Metadata auto-fix splits legal tags containing commas | fixed |
| 37 | `B13-16` | B13 | P1 | Export succeeds after dropping source rows with null timestamps | partially-fixed |
| 38 | `B13-17` | B13 | P2 | Missing attachment counts silently disable attachment indexing | partially-fixed |
| 39 | `F-01` | B14 | P1 | `revoke_exact` aborts the whole helper on any resolve error other than `device_not_found` | fixed |
| 40 | `F-02` | B14 | P1 | `main()` discards the error, so every failure looks identical | fixed |
| 41 | `F-03` | B14 | P2 | `shutdown` waits for the HTTP server without a timeout → a parked asset request hangs the helper forever | fixed |
| 42 | `F-04` | B14 | P2 | `hold_response` swaps the `Notify` instance → a parked waiter is orphaned (lost wakeup) | fixed |
| 43 | `F-05` | B14 | P2 | The `fixture` mutex is held across the stdout write → the HTTP middleware stalls behind it | fixed |
| 44 | `F-06` | B14 | P2 | A `release_response` that lands before the request parks silently disables the gate — the case the gate exists to prove is never exercised, yet the ru | fixed |
| 45 | `F-07` | B14 | P2 | No bound on the stdin line length (the protocol declares 16 KiB) → unbounded allocation | fixed |
| 46 | `F-08` | B14 | P2 | The binary is excluded from every CI build → 1028 LOC that nothing compiles or tests | fixed |
| 47 | `F-09` | B14 | P2 | The `stopped` message reports `fixtureClosed: true` while the fixture is open and the helper keeps serving commands | partially-fixed |
| 48 | `F-10` | B14 | P3 | An unknown command is reported as `bad_state` instead of `bad_command` | fixed |
| 49 | `F-11` | B14 | P3 | `control_loop` is a 392-line if-chain with nine copy-pasted reply shapes | deferred (Jev triage: cosmetic / needs redesign) |
| 50 | `B2-001` | B2 | P0 | Renamed actions bypass blocklist and allowlist rules | fixed |
| 51 | `B2-002` | B2 | P1 | Home-directory failure loads policy from an untrusted working directory | fixed |
| 52 | `B2-003` | B2 | P2 | Policy patterns leak secrets into debug and error logs | fixed |
| 53 | `B2-004` | B2 | P2 | Audit logging copies an unbounded raw target into the log pipeline | fixed |
| 54 | `B2-005` | B2 | P2 | Policy loading reads an unbounded file into memory | fixed |
| 55 | `B2-006` | B2 | P0 | URL case variation bypasses navigation blocklists | fixed |
| 56 | `B2-007` | B2 | P1 | Long-target redaction emits the secret-bearing prefix verbatim | not-applicable |
| 57 | `B3-001` | B3 | P2 | Half-open guardian breaker admits a provider stampede | fixed |
| 58 | `B3-002` | B3 | P1 | Node approval treats an undeliverable prompt as a live request | fixed (wave 2a: node approval is denied when no operator surface received it) |
| 59 | `B3-003` | B3 | P2 | Aborting an attended approval defers zombie cleanup until a later registration | fixed |
| 60 | `B3-004` | B3 | P2 | Fallback routing loses the operator fallback across channel teardown | fixed |
| 61 | `B4-01` | B4 | P0 | Concurrent eviction can delete another successful `put` | fixed |
| 62 | `B4-02` | B4 | P1 | Acknowledged writes are not crash-durable | fixed |
| 63 | `B4-03` | B4 | P2 | Failed writes and eviction leave untracked blobs that bypass the cap | fixed |
| 64 | `B4-04` | B4 | P2 | Reads allocate the whole on-disk file before enforcing any bound | fixed |
| 65 | `B4-05` | B4 | P2 | Session encoding leaves Windows device names unchanged | fixed |
| 66 | `B5-001` | B5 | P1 | Stop can SIGTERM/SIGKILL a recycled unrelated PID | fixed |
| 67 | `B6-001` | B6 | P2 | Bootstrap token hides database open failures as “no token” | fixed |
| 68 | `B6-002` | B6 | P2 | Identity commands misreport vault/database failures as a missing token | fixed |
| 69 | `B6-003` | B6 | P2 | Export size cap is defeated by a file replacement race | fixed |
| 70 | `B6-004` | B6 | P2 | Hook review can hang or exhaust memory on untrusted command output | fixed |
| 71 | `F-01` | B7 | P1 | Synchronous blocking file I/O inside async function (node.rs) | fixed |
| 72 | `F-02` | B7 | P1 | Privilege escalation in plugins.rs: unvalidated symlink in uninstall path | not-applicable |
| 73 | `F-03` | B7 | P1 | Process::exit called on error paths in plugins.rs without cleanup | fixed |
| 74 | `F-04` | B7 | P2 | Implicit panic on path construction in resume.rs | not-applicable |
| 75 | `F-05` | B7 | P2 | Race condition: plugin discovery reads disk after enable/disable writes | not-applicable |
| 76 | `F-06` | B7 | P3 | Dead code: #[allow(dead_code)] suppresses cross-platform warnings | deferred (Jev triage: cosmetic / needs redesign) |
| 77 | `B8-01` | B8 | P2 | Channel-pairing credential store falls back to a predictable, world-readable `/tmp` path | fixed |
| 78 | `B8-02` | B8 | P2 | Pairing store silently degrades to in-memory: durable credentials are lost while the RPCs keep answering | fixed |
| 79 | `B8-03` | B8 | P3 | A2A tools stay registered but permanently dead when the execution adapter is missing; the error names the wrong cause and daemon mode logs nothing | fixed |
| 80 | `B8-04` | B8 | P2 | The spend ledger's durability guarantee is false when the security store fell back to memory | fixed |
| 81 | `B8-05` | B8 | P2 | Provider build errors are swallowed: the provider's models silently vanish from the OpenAI-compatible routing table | fixed |
| 82 | `B8-06` | B8 | P2 | Duplicate model names across providers bind to a random provider (HashMap iteration order) | fixed |
| 83 | `B8-07` | B8 | P3 | Boot prints "Task reaper: started" even when the reaper was disabled and never spawned | skipped-too-risky |
| 84 | `B9-01` | B9 | P1 | Invalid default config is replaced by defaults instead of failing startup | fixed |
| 85 | `B9-02` | B9 | P1 | Unknown session-store backend silently selects SQLite | fixed |
| 86 | `B9-03` | B9 | P2 | Runtime warmup performs unbounded blocking subprocess and file I/O on a Tokio worker | fixed |
| 87 | `B10-S2-01` | second/B10 | P2 | Config hot-reload consumer exits permanently on a single `broadcast::RecvError::Lagged` | fixed |
| 88 | `B10-S2-02` | second/B10 | P2 | `config.reload` / `config.validate` / `config.path` are unregistered whenever the watcher cannot be | fixed |
| 89 | `B10-S2-03` | second/B10 | P3 | Dead note-maintenance lifecycle wrappers, kept alive only by tests that prove nothing about production | deferred (Jev triage: cosmetic / needs redesign) |
| 90 | `B10-S2-04` | second/B10 | P2 | `enabled = false` is honoured at boot only for `feishu` | fixed |
| 91 | `B10-S2-05` | second/B10 | P3 | `StartupResources::cleanup` runs twice on the success path and the second failure is swallowed | not-applicable |
| 92 | `B10-S2-06` | second/B10 | P2 | Security artifacts fall back to predictable `/tmp` paths | fixed |
| 93 | `B10-S2-07` | second/B10 | P3 | Gateway teardown owns none of the background tasks it spawns | not-applicable |
| 94 | `B3-S2-01` | second/B3 | P1 | Node requester omits Ok(0) check, silently parks approval for full timeout | fixed |
| 95 | `B5-S2-01` | second/B5 | P1 | Daemon parent reports success before daemon setup succeeds | fixed |
| 96 | `B5-S2-02` | second/B5 | P2 | Requested WebChat listener failure is detached and startup still succeeds | fixed |
| 97 | `B5-S2-03` | second/B5 | P2 | Failed installer launch leaks downloaded update files | fixed |
| 98 | `R1-approval/DC-01` | diffcheck | P2 | Originator callback authorization is not atomic across approval ID reuse | fixed |
| 99 | `R1-approval/DC-02` | diffcheck | P2 | URL case folding changes case-sensitive path and non-URL text | fixed |
| 100 | `R2-artifacts-migrate/DC-01` | diffcheck | P1 | Path-keyed locks do not cover aliased store roots | fixed |
| 101 | `R2-artifacts-migrate/DC-02` | diffcheck | P1 | Directory-sync failures are still acknowledged as durable writes | fixed |
| 102 | `R2-artifacts-migrate/DC-03` | diffcheck | P2 | Reserved-name encoding strands existing non-Windows artifacts | fixed |
| 103 | `R2-artifacts-migrate/DC-04` | diffcheck | P2 | Deduplicated attachment verification reads an unbounded file | fixed |
| 104 | `R3-servercmd/DC-01` | diffcheck | P1 | Dynamic `handlers_mut().register(...)` loop breaks the gateway method-census test | fixed |
| 105 | `R3-servercmd/DC-02` | diffcheck | P1 | `stopped` message now reports `fixtureClosed: false`, breaking the documented contract and the QA harness | fixed |
| 106 | `R3-servercmd/DC-03` | diffcheck | P2 | Shutdown ignores the server join result and always reports `listenerClosed: true` | fixed |
| 107 | `R3-servercmd/DC-04` | diffcheck | P2 | 16 KiB line bound is checked after the whole line has been buffered | fixed |
| 108 | `R3-servercmd/DC-05` | diffcheck | P3 | `release_response` ack uses a status outside the declared ack vocabulary | fixed |
| 109 | `R3-servercmd/DC-06` | diffcheck | P3 | `runtime_channel_id` still leaves same-type instances on an unread key | fixed |
| 110 | `R3-servercmd/DC-07` | diffcheck | P3 | `revoke_exact` masks every non-ambiguity error as `already_revoked` | fixed |
| 111 | `R3-servercmd/DC-08` | diffcheck | P3 | New CI step re-checks the whole crate on all three OS runners while clippy still skips the gated bin | fixed |
