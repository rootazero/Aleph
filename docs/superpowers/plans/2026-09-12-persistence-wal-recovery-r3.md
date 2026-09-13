# Persistence & Crash-Reduction Round 3 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every fact that crosses a side-effect durable BEFORE the effect, every multi-step durable operation atomic, every "I don't know" unreadable as "no", and every boot re-reduction bounded — so a `kill -9` at any instruction leaves a log the reducer reads truthfully and a resume that never loops, never widens scope, and never tells the model a parked call ran.

**Architecture:** One write path (`SessionEventStore::append_batch` — one `BEGIN IMMEDIATE` per batch, retire inside the transaction, per-transaction `PRAGMA synchronous=FULL` for the four Barrier events) under one policy table (`durability_of` / `ignorable`). Intent stamps (`ResumeAttempted`, `ToolCallParked`) and a fourth disposition (`Unanswered`) let the existing reducer (`reduce_run` / `reduce_disposition` / `boundary_repair`) answer the windows it could not see. Per-row decode isolation makes a bad record refuse one session, not the scan. The boot scan fans out under a `JoinSet` bounded by the already-existing `[resume] max_concurrent`. The process journal grows a `kind` and a probed two-arm tombstone so a job or terminal orphaned by a restart answers "exited" / "still running (pid N)" instead of "not found".

**Tech Stack:** Rust (tokio, rusqlite `Transaction`, `tokio::task::JoinSet` + `Semaphore`, `sysinfo` 0.39), SQLite WAL, `shared/protocol` wire types, Leptos Panel locales (`locales/{en,zh}.json`), TUI, Node ≥ 22.5 QA fixtures (`node:sqlite`).

**Spec:** `docs/superpowers/specs/2026-09-12-persistence-wal-recovery-r3-design.md` — the plan argues from it; read §1 (the 15 defects), §3 (rulings U1–U7), §4–§9 (the six design sections), §11 (verification), §12 (entropy), §13 (not-doing). The four scan reports it derives from are in `docs/superpowers/specs/2026-09-12-persistence-r3-scans/`.

## Global Constraints

- **Branch isolation**: everything happens in worktree `D:\Workspace\Aleph\.claude\worktrees\persistence-r3` (Bash `/d/Workspace/Aleph/.claude/worktrees/persistence-r3`), branch `worktree-persistence-r3`, base `5e85060b8`. **Never touch `main`**, never `git checkout main`, never edit in `D:\Workspace\Aleph`. **No merge to main** (U7): commit on the branch and report.
- **R10**: ZERO changes under `src/harness/` (`src/harness/tests/budget.rs::CEILING` must not move; `src/harness/agent/{act,think,prompt}.rs` are read-only). The harness calls `emit_event`; durability policy lives in the store. **R7**: reduction states facts, never chooses a re-run for the model. **Criterion #10**: wire key sets live in `shared/protocol` and the server constructs responses from those types — no `json!` literals of a wire shape.
- **U3 durability**: Barrier = exactly `ToolCallRequested` / `RunStarted` / `ResumeAttempted` / `UserMessage` (per-transaction `PRAGMA synchronous=FULL`, restored to `NORMAL` after commit AND after rollback). `ToolCallParked` is Normal — losing it reads as "outcome unknown", the safe direction. Pinned by a source-derived census (T1).
- **U4**: a parked call is only TOLD to the model (fourth repair arm); no approval / clarification redelivery. **U5**: orphan OS processes get pid + creation time + a two-arm tombstone; never killed by the server.
- **Criterion #6**: before changing a fact's writers or readers, COUNT them (comments stripped) and list them in the task. **Criterion #17**: a new wire field / status word names its renderer(s) in the same task or is not added. **Census assertions are EQUALITY**, derived from the owning type or dispatch site — never a hand-written list of what someone remembers.
- **P7**: locks `unwrap_or_else(|e| e.into_inner())`; string slicing via `char_indices()` / `.get(..n)`. **Never** `unwrap_or` / `.ok()` / `unwrap_or_default()` on a `reduce_run(` result — `Err` only has the right to say "I don't know" (criterion #8).
- **Panel text goes through `locales/{en,zh}.json`** (the i18n gate forbids inline Chinese); TUI text lives in the function (that crate has no locale table).
- **No production failpoints**: QA stages open crash windows with configurable hooks, a stalled mock embedding provider, or the approval gate (`bash = "ask"`). Nothing under `src/` reads a QA env var.
- **Windows host**: `python` / `python3` are WindowsApps stubs (exit 49) — every fixture is Node `.mjs`; Node here is v24.13.0 (`node:sqlite` needs ≥ 22.5; T0 re-checks).
- **Entropy**: every "delete" listed in a task is really deleted; deleting a `pub fn` / field means running the `--lib` test BUILD in the same commit (`cargo check` cannot see `#[cfg(test)]`).
- **Commit messages**: English, `<scope>: <description>`. **THIS round DOES end every commit with the line `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`** — the r2 plan's "no trailer" rule (2026-09-02) is superseded: every commit of 2026-09-12 on this branch's history carries it, and the user read the spec commit with it and raised no objection. Do not report this as a conflict. Comments and identifiers English; spec/plan Chinese.
- **Spec deviations settled while drafting (code wins over the spec's wording; the spec is NOT edited for these):**
  - §4.3 `/undo`: the closer the existing `close_open_run_after_retire` emits is `RunFinished { outcome: Cancelled }` (`marker_balance.rs:64-70` says why: the user cut it; `Abandoned` is the resume coordinator's word). Every task says `Cancelled`.
  - §5.5: the fold can only give TOKENS — `AssistantMessage.usage` is a `TokenBreakdown` (no model, no price). `AssistantRunMeta` loses `input_tokens` / `output_tokens` only; `cost_usd` / `model` / `model_provider` stay on the meta, so session DOLLARS still ride the post-run event (closing that needs per-call cost on the message ⇒ harness emits it ⇒ R10 ⇒ Phase 2 F).
  - §8.1: `[resume] max_concurrent` already exists (`src/config/types/resume.rs:22-25`, default 4, `#[serde(default)]`) with ZERO readers — a severed wire (criterion #7). T15 connects it and lowers the default to the spec's 2 (the 4 was never exercised, so it carries no evidence).
  - §9: the journal writers are `record_spawn` / `record_settled` (not `record_start`), the record type is `JobRecord`, bash actions are `poll|wait|kill|list` (no `status`), and the 7-day retention sweep already exists (`RECORD_RETENTION_MS`) — no `MAX_TOMBSTONES` is added.
  - §6.1: `ToolCallParked` has NO `tool_name` (the dispatch it pairs with owns it; `clarification::ask` cannot know its caller's name). `PreHook` means "parked on a card a `BeforeToolCall` hook raised", NOT "the hook script was executing" — a crash inside a hook script stays OUTCOME UNKNOWN because no release fact exists.
  - §11: the `BeforeAgentStart` sleeper opens the PRE-seed window (§8.2(b)); the seed→RunStarted window is opened by a stalled embedding provider (T10).
  - §12's "四条撒谎注释": `start/mod.rs:391` is corrected by T4 (it is on that task's path); T17 corrects the other three.

---

## 0. 本机验证配方（每个任务都适用；r2 plan §0 原样，路径已改）

### 本机 cargo 配方（Windows；alephcore lib-test 一次完整编译实测 16m30s，超过 Bash 工具 10 min 上限）

所有 cargo 命令必须带这两个 env（值不可变，否则 fingerprint 失配全量重编）：

```
CARGO_TARGET_DIR=D:/Workspace/Aleph/target
CARGO_PROFILE_TEST_DEBUG=line-tables-only
```

| 目的 | 命令 | 怎么跑 |
|---|---|---|
| 快速类型检查（不含 tests） | `cargo check -p alephcore` / `cargo check -p aleph-protocol` | Bash 前台，timeout 600000（1–4 min） |
| 运行某模块单测 | `cargo test -p alephcore --lib <module::path>` ；**多个**过滤器必须写在 `--` 之后（`--lib -- a b`），并排写会以 `error: unexpected argument` 空跑退出 | **分离式**（见下）＋**前台轮询**等待（Monitor 会截断本回合，见下段） |
| 集成面类型检查（替代 `--test '*' --no-run`） | `cargo check -p alephcore --features test-helpers --all-targets` | 分离式（约 8 min） |
| protocol / tui / cli / bins | `cargo test -p aleph-protocol`；`cargo test -p aleph-tui`；`cargo test -p aleph-cli`；`cargo test -p alephcore --bins` | Bash 前台 timeout 600000 通常够；超时就改分离式 |
| Panel（宿主测试） | `cargo test -p aleph-panel --lib`（harness 在**第一个**失败处中止——用 `-- --skip <name>` 看其余） | 分离式 |
| Panel 出厂形态 | `just wasm`（**必须用 Bash 工具**跑 just，PowerShell 缺 cygpath） | Bash 前台 timeout 600000 |
| clippy | `just _stage-shell-placeholders && cargo clippy --workspace --all-targets`（Windows 上排除 `-p aleph-desktop-macos -p aleph-desktop-linux` 若报错） | 分离式（~6 min+） |

**分离式启动**（PowerShell 工具）——把 `<filter>` 与 `<name>` 换掉：

```powershell
$S='C:\Users\zou\AppData\Local\Temp\claude\D--Workspace-Aleph\63e9bfd1-ae20-4e05-9159-6aadc0934652\scratchpad'; $out="$S\<name>.txt"; Remove-Item -Force -ErrorAction SilentlyContinue $out,"$out.done"
Start-Process pwsh -ArgumentList "-NoProfile","-Command","`$env:CARGO_TARGET_DIR='D:/Workspace/Aleph/target'; `$env:CARGO_PROFILE_TEST_DEBUG='line-tables-only'; cargo test -p alephcore --lib <filter> *> '$out'; 'EXIT='+`$LASTEXITCODE | Out-File '$out.done'" -WorkingDirectory 'D:\Workspace\Aleph\.claude\worktrees\persistence-r3' -WindowStyle Hidden
```

**等待**——**前台 Bash 轮询，`timeout: 600000`**，一次最多等 9.5 分钟；没等到就**再调一次同样的命令**，直到 `.done` 出现。**绝对不要用 Monitor 等**：Monitor 要结束本回合才能收到事件，而 Workflow agent 一结束回合就会被强制交最终报告——本轮三个 agent 都是这样被截断的（2026-09-02 实测）。也不要用 `run_in_background` 再 `TaskOutput` 之外的任何"回头再看"方式。

```bash
S=/c/Users/zou/AppData/Local/Temp/claude/D--Workspace-Aleph/63e9bfd1-ae20-4e05-9159-6aadc0934652/scratchpad
for i in $(seq 1 28); do [ -f "$S/<name>.txt.done" ] && break; sleep 20; done
[ -f "$S/<name>.txt.done" ] && { echo "DONE $(cat "$S/<name>.txt.done")"; grep -E "^test result:|^error(\[|:)|panicked at|FAILED" "$S/<name>.txt" | head -20; } || echo "STILL RUNNING — call this again"
```

完成后用 Read / `sed -n` 读 `<name>.txt` 看细节。**永远不要 kill 一个在跑的 cargo**（会毁掉增量产物，下一次更慢）；一次只跑一个 cargo（共享 target dir 会串行化，且本机 RAM 撑不住两个 rustc）——启动前 `tasklist //FI "IMAGENAME eq rustc.exe"` 确认没有别的 rustc 在跑。先用 `cargo check` 消灭非测试编译错误，再上分离式测试构建。

**基线失败名单**（改动前，18 条，全部环境/上游）在 `<scratchpad>/baseline_failures.txt`；全量 `--lib` 跑完后用 `comm -3` 按**名字**比对，多出来的才是你的。


### 0.1 上游转发给下游的约束（**你的任务在下表里的话，这是必读项**）

> 每个任务的 review 只发给本任务的 fixer，够不到下一个任务的 agent。凡是「上游发现、下游才能修」的
> 东西一律搬到这里——这本身就是判据 #7（两端完整而中间没线）在本流程上的形态。五份起草稿的跨组冲突
> 已经在任务正文里改掉（见 spec deviations 与下表的「已解决」列）；这里留的是**执行时仍要看一眼**的约束。

| 给谁 | 约束 | 出处 |
|---|---|---|
| **T1** | `SessionEvent::ResumeAttempted { target: EventSeq, attempt: u32 }` 只在 T1 定义一次（Barrier census 需要它）。T6 只消费：`load_run_markers` 的 `IN (...)` 列表、reducer 臂、producer。 | A1/A2 交叉检查 (a) |
| **T1 → T11** | `durability_of` 的 `match` 没有通配臂——T11 加 `ToolCallParked` 变体时必须同时把它加进 `Normal` 组，并给 `fixtures::sample_of_every_kind` 加样本；否则 T1 的 census 拒绝编译。 | (b) |
| **T1 → T14** | T1 的私有 `encode_payload` 是**接缝**：T14 用 pub `encode_row` 取代并删掉它（一个编码器），`events::ignorable` 由此获得唯一消费者。若 T14 没做，`ignorable` 是零消费者的 pub fn，必须 CUT。 | (c) |
| **T6** | 只用 T1 的 `fixtures::sample_of_every_kind()` 做枚举采样，**不要**再造第二个 `one_of_each_variant`（同一张表两份）。 | (b)/(j) |
| **T7 → T15** | T7 的活动窗口循环在 T15 搬进 `launch_resume`（循环体不变）；T15 的 `ResumeLaunch::pending` 刻意**过包含**（扫描要访问的每个会话），因为 `Unanswered` 对 marker-only 切片不可见。 | (g) |
| **T9** | `ErrorKind::HookStop` 由 **T9 自己**加（T1 不动 `ErrorKind`）；加完看哪个 `match ErrorKind` 编译失败——那就是渲染器 census（`session/projection.rs::project_row`）。 | A1/A2 交叉检查 |
| **T11** | r2 的每个 QA dangle 都停在 `ask` 门 ⇒ T11 落地后 `knobs` 阶段的 `"OUTCOME UNKNOWN"` 探针变红。`REPAIR_MARKERS` / `carriesRepair` 的 `drive_r2.mjs:805` 改动**随 T11 同一提交**落地；T13 复用。 | (f) |
| **T11 → T14** | `LogContradiction` 闭集编号：T6 `ResumeWithoutTarget` = 9（`KIND_COUNT` 10）→ T11 `ParkedWithoutRequest` = 10（11）→ T14 `UndecodableRecord` = 11（12，REJECT 集 `0 \| 1 \| 11`）。后落地者按此编号，别各自从 9 起。 | (j) |
| **T15 → T16** | T15 的 `ResumeReport::absorb` **不含** `notified`，`ResumeLaunch::settle` **不调** `adjudicate_orphaned_tasks`——两者都由 T16 在加字段时补上（T15 的代码块里留了注释标记）。 | (j) |
| **T16** | 8.2(b)「请重发」臂必须跳过恢复重触发留下的 `tasks` 行：它们的 `task_prompt == ""`（`retrigger` 发 `input: String::new()`），且 `ratchet` 阶段每次 boot 都留一行。T16 有专门的测试钉它。 | A2 open #6 (e) |
| **T16 → T17** | `messages` 表写者 census（`the_projector_is_the_only_production_writer_of_the_messages_table`）住在 **T17**（那里 `AgentInstance::add_message*` 被删，census 才绿）；T17 紧跟 T16。 | (j) |
| **T17** | `SESSION_SERVICE_READERS` 在提交时**重新推导**：T8 把 fast-path 的读者搬到 `slash_command.rs`，T9 加了 `run_loop/mod.rs`，T11 把 `dispatch.rs` 的读者搬进 `session/call_log.rs`。测试是仪器，列表是预期。 | A2/B 交叉检查 |
| **T10 / T13 / T18 / T22** | `qa/resume_boundary/run.sh` 的 stage `case` 列表与 `drive_r2.mjs` / `mock_r2.mjs` / `patch_r2.mjs` 由四个任务**累加**编辑：T13 加 `parked`，T10 加 `unanswered\|ratchet`，T18 加 `parallel\|undecodable\|attribute` 并删 Python，T22 加 `tombstone`。`patch_r2.mjs` 的 argv[7] 属 T10（`embed-stall`）；T18 用 env `QA_MAX_CONCURRENT`。`mock_r2.mjs` 的 `MARKER` 正则：T18 加 `spawn`，T22 加 `bg\|poll\|kill` 且**保留** `spawn`。 | (j)/(m) |
| **T10** | Step 0 是前置飞行：`mock.log` 里必须看到 `embeddings request; stalling` 落在 `user_message` 行与 kill 之间；`drive window` 的 `INSTRUMENT FAILURE` 退出 1——静默未命中不算绿。 | (m) |
| **T13** | `LastRunState::NEVER_RAN`、`dangling()` 在 `!inspected` 时答 `None`（r2 ⑮）：`never_ran_count()` 对目录面必须是 `None`，不是 0。 | B 草稿 |
| **T14** | `get_events` 遇坏行返回 `Err` 的那条路径在 `src/harness/agent/think.rs:326` **之外**处理（`orchestrator/harness_bridge/error.rs::classify_harness_error`）——R10。拒绝句指向 `diagnostics.run` / `doctor` 工具的 `core/session-log fix=true`，**不是** `aleph-server doctor`（冷进程不注册 `with_session_log_check`）。 | C 草稿 |
| **T15** | 重注入**不是** lane-deferred：等信号量的候选还没拿到引擎槽（`gate.rs::admit_run` → `try_claim`），幸存者会先被准入。所以分两趟：非 pending 的在扇出后立刻重注入，pending 的在 `settle` 之后。 | C 草稿 |
| **T18** | `attribute` 不能用 `ask` 仪器（B 之后停车调用得第四臂）：用前台 `subagent` 子，其 provider 挂 120 s，父的派发才真在飞。 | C 草稿 |
| **T19–T21** | 与 A/B/C 的 `src/` 文件**完全不相交**（见 File Structure），可并行；**T22 除外**（共享 QA 文件，排在 T18 之后）。 | (j) |
| **T22** | Windows 上 `[sandbox.windows]` 三个开关打开时 `child.id()` 是 `sandbox-init-windows` 启动器，且 `KILL_ON_JOB_CLOSE` 会随服务器死亡杀掉子进程——阶段用 `QA_WINDOWS_SANDBOX_OFF=1` 关掉它们；作业若 2 s 内 settle ⇒ 退出 **78**（仪器不可用），账本记 **UNRUN** 不记 PASS。 | D 草稿 |
| **T5a/T5b** | 出错 / 取消的 run 从不发 `AssistantRunMeta`（`execute.rs:955` 只在 `Ok` 臂）——T5b 在 boot 补账；live 路径仍不计。记录为 #11 的兄弟，本轮不改。 | A1 open #5 |
| **T23** | `cargo clippy --all-targets` 会把 `target/debug` 里每个链接好的二进制留成 **0 字节**（仍可执行）——QA 阶段要在 clippy **之前**跑，或跑完 clippy 重建。 | r2 T8 实测 |

### 0.2 你的第一条命令是 `git status --porcelain`（**每个 agent，无例外**）

本轮已经发生**三次**：一个 agent 死在半路，留下未提交的改动，而**接手的那个 agent 拿到的是全新
prompt，不知道那些改动存在**。三次的死因各不相同，形状完全一样：

| 何时 | 死因 | 留下了什么 |
|---|---|---|
| T2 第一次 | harness 判为 `[Request interrupted by user]` | `boundary_repair.rs` + `marker_balance.rs` 共 605 行 |
| T2 第二次 | 账号 session 上限（`resets 7pm`） | 11 个文件 508 行，含已写好的 commit message |
| T4 续做 1 | `API Error: UNKNOWN_CERTIFICATE_VERIFICATION_ERROR`（本机 schannel TLS 抖动，见 [[windows-git-schannel-tls-flake]]） | `runner_impl.rs` + `reduction.rs` + `tests/resume_coordinator_integration.rs`，+160/−11，**恰好是计划要的那批测试** |

所以：

1. **第一条命令是 `git status --porcelain`**，不是读计划。脏 ⇒ **有人死在这里**。
2. 接着 `git diff --stat` 与 `git diff`，**读完再判断**。这批改动往往正是计划要而提交里没有的部分。
3. **不许 `git checkout --` 掉它**，除非你已经证明它编译不过且修不动；要丢也先 `cp` 到
   `<scratchpad>/` 再丢。默认动作是**验证后提交**（`<scope>: <task> part N — <what>`）。
4. 报告里 `tree_clean` 只有在你自己跑过 `git status --porcelain` 且为空时才填 `true`。
   **上一个 agent 说过 clean 不算**——T4 的 impl 报的就是 `tree_clean: true`，而 review 到达时树是脏的。

---


### 0.3 orchestrator 与 agent 同时改一个文件时，`git add <path>` 会把对方的在飞改动一起提交

2026-09-03 实测，**是 orchestrator 干的**：我在补 `core/session-log` 时 `git add
src/builtin_tools/doctor.rs`，而 T5 的续做 agent 正在同一个文件里改 doc 和加测试。它的改动被
`9e6c83002` 一并提交，而那条 commit message 一个字都没提到它们。

它诊断得比症状准，值得抄下来：**`git status` 干净、`git diff` 为空，而磁盘上的文件明显和你刚写的
不一样**——因为别人已经把你的改动提交了。分辨方法是
`git hash-object <path>` 与 `git rev-parse HEAD:<path>` 比对，相等就说明「你的改动还在，只是
已经在 HEAD 里了」，不是「你的改动没了」。

规矩（对 orchestrator）：

1. **`git add` 只列自己创建的新文件，或先确认那个路径的 diff 只有自己那几行**（`git diff --stat
   <path>` 对一遍行数）。`git add -A` 在这个工作树里等于替所有在跑的 agent 做决定。
2. 要改的文件如果在**当前任务的 Files 列表**里，就别在任务跑着的时候改——等任务间隙。
3. 真的扫进去了：**别 revert**（对方的工作在你的 commit 里是安全的），在下一条 commit 的正文里
   写清楚哪些行不是这条 commit 的。`23d855f53` 就是这么做的。


---

## File Structure（本轮触碰面）

| 文件 | 任务 | 职责 |
|---|---|---|
| `src/session/events.rs` | T1 T5a T8 T9 T11 T12 | `Retire` / `Durability` / `durability_of` / `ignorable` 策略表；`ResumeAttempted`、`ToolCallParked`、`ParkReason`、`ErrorKind::HookStop`；`RunEnvelopeSnapshot` 两键；`user_turn` 种子对；`AssistantRunMeta` 去计数 |
| `src/session/store.rs` | T1 T6 T14 | `append_batch` + 事务内 retire + Barrier；`MARKER_EVENT_TYPES`；逐行 `decode_row` / `encode_row` / `MarkerSlice` / `retire_record` |
| `src/session/actor.rs` | T2 T14 | `EmitBatch` 臂；`replay` 改 `load_head_seq` |
| `src/session/service.rs` `in_process.rs` | T2 T14 T17 | `emit_batch`（提供方法，默认 `Err`）；`SessionError::UndecodableRecord`；`SESSION_SERVICE_READERS` census |
| `src/session/reduction.rs` | T6 T7 T11 T14 | `attempts`、`Unanswered`、`is_disposition_bearing`；`parked`；`ResumeWithoutTarget` / `ParkedWithoutRequest` / `UndecodableRecord`；`reduce_marker_slice` |
| `src/session/boundary_repair.rs` | T11 | 第四臂 |
| `src/session/marker_balance.rs` · `src/gateway/handlers/{mod,chat}.rs` · `handlers/session/db_handlers/modify.rs` | T3 | `/undo` 与 `session.truncate` 一批 |
| `src/session/call_log.rs`（新） | T11 | 审批门与停车的唯一写者 |
| `src/session/usage_fold.rs`（新） | T5a | `session_usage_totals` / `run_usage_totals` |
| `src/context/compact/manual.rs` · `builtin_tools/sessions/compact_tool.rs` | T3 | `/compact` 一批 |
| `src/context/compact/session_split.rs` | T4 | 父批 → 子批 → epoch |
| `src/gateway/projection_reconciler.rs` | T4 T5b T14 T17 | epoch heal 臂；缺 meta 合成戳；`reduce_marker_slice`；注释纠正 |
| `src/gateway/session_projector.rs` | T1 T5a T5b T14 T17 | `append_batch` mock；从折叠计费；`stamps_synthesized`；写者 census |
| `src/gateway/resume_coordinator.rs` | T6 T7 T12 T14 T15 T16 | `ResumeAttempted` 戳；`Unanswered` 臂 + 活动窗口；两键回放；坏行 refused；`launch_resume` / `settle` / `absorb`；`adjudicate_orphaned_tasks` / `system_note` |
| `src/gateway/session_snapshot.rs` · `shared/protocol/src/session_thread.rs` | T7 T12 T13 T14 | `Unanswered` 面；`RUN_ENVELOPE_FACT_KEYS`；`parked` 面；`last_run_from_markers(Result)` |
| `interfaces/webchat/src/components/chat_sidebar.rs` · `locales/{en,zh}.json` · `interfaces/tui/src/tui/commands.rs` | T7 T13 | 两条新文案各两张脸 |
| `src/gateway/execution_engine/{fast_path,slash_command,slash_skill_scope}.rs` · `run_loop/{mod,inner}.rs` · `src/thinker/context.rs` · `orchestrator/harness_bridge/{runner_impl,session_seed}.rs` | T8 T9 T12 | fast-path run 形状；hook-stop 回执；envelope 两键的生产者 |
| `src/tools/scoped/{dispatch,tests}.rs` · `src/clarification/ask.rs` | T11 | 停车写点；身份 census |
| `src/gateway/btw/mod.rs` | T12 | 恢复的侧问不扇出 |
| `src/config/types/resume.rs` · `src/gateway/busy_queue/durable.rs` · `src/bin/aleph-server/commands/start/mod.rs` · `start/builder/agent_init/mod.rs` · `start/helpers.rs` | T4 T15 T16 T17 | `max_concurrent` 接线；两趟重注入；`orphan_notice` CUT；注释纠正 |
| `src/gateway/orphan_notice.rs` | T16 | **删除** |
| `src/resilience/database/{migration,tasks}.rs` · `state_database/{mod,schema}.rs` | T16 | `adjudicated_at_ms` |
| `src/diagnostics/checks/session_log.rs` | T14 | `session-log-undecodable-record` + `--fix` retire |
| `src/orchestrator/harness_bridge/error.rs` | T14 | 坏行的拒绝句 |
| `src/gateway/agent_instance.rs` · `continuation_lifecycle.rs` | T17 | 死写者删除；census 地板 |
| `src/agents/subagent_spawner/fork.rs` `fork/tests.rs` · `src/agents/subagent_tool/recovery.rs` | T1 T11 T13 | 穷举 match 臂；`parked` 孪生 |
| `src/builtin_tools/process_journal.rs` · `utils/process_alive.rs` · `sandbox/live_tail.rs` · `sandbox/platforms/common.rs` · `builtin_tools/process_registry.rs` | T19 T20 | kind / pid / 探活 / 墓碑；pid 经 LiveTail 到 journal |
| `src/gateway/pty/{session,manager}.rs` · `builtin_tools/{bash_exec,terminal}.rs` · `gateway/handlers/pty.rs` | T20 T21 | PTY 进 journal；三张工具脸的墓碑 |
| `tests/resume_coordinator_integration.rs` · `tests/parked_gate_integration.rs`（新） | T6 T7 T11 T15 T16 | 集成面 |
| `qa/resume_boundary/{run.sh,drive_r2.mjs,mock_r2.mjs,patch_r2.mjs}` · `assert_repairs.py` `drive_dangle.py`（删） | T10 T11 T13 T18 T22 | 六个新阶段 + `attribute` 移植 |
| `docs/reference/FEATURE_LOCATOR.md` · `SESSION_KNOBS.md` · `SESSION_SERVICE.md` · `qa/README.md` · `CLAUDE.md` | T24 | 文档 |

**任务顺序（依赖驱动；每个文件内的任务按此顺序串行）：**
`T0 → T1 → T2 → T6 → T7 → T3 → T4 → T8 → T9 → T11 → T12 → T13 → T14 → T15 → T16 → T17 → T5a → T5b → T10 → T18 → T22 → T23 → T24`
— **T19 → T20 → T21 独立**，`src/` 文件与 A/B/C 完全不相交，可与 T3–T18 并行跑（三个任务之间串行）；**T22 共享 QA 文件，必须排在 T18 之后**。与最初拟定的 `… T16 → T5a → T5b → T17` 相比，T17 提前到 T16 之后：`messages` 写者 census 在 T17 才绿，隔三个提交的红是一棵坏树。

---

### Task 0: 基线——在分支基点量一次红名单

**Files:**
- Create: `<scratchpad>/baseline_failures.txt`（不进 git）
- Test: 无代码变更

**Interfaces:**
- Produces: 基线名单文件；`node --version` 的记录。

- [ ] **Step 1: `git status --porcelain`** 必须为空；`git rev-parse HEAD` 必须是 `db5e5ddd9`（spec 提交）或其后仅含 docs 的提交。
- [ ] **Step 2: 跑全量 `--lib`（分离式，见 §0）**

`<name>` = `baseline`，`<filter>` 留空。等到 `.done`，然后：
```bash
S=/c/Users/zou/AppData/Local/Temp/claude/D--Workspace-Aleph/63e9bfd1-ae20-4e05-9159-6aadc0934652/scratchpad
grep -E '^test .* \.\.\. FAILED$' "$S/baseline.txt" | sed 's/^test //; s/ \.\.\. FAILED$//' | sort > "$S/baseline_failures.txt"
wc -l "$S/baseline_failures.txt"; grep -E '^test result:' "$S/baseline.txt"
```
Expected: 一份**名字**清单（记忆 `windows-alephcore-lib-baseline-2026-09-02` 说是 18 条；数目变了不是问题，**名字**变了才是——把差异写进验证记录）。一个 `--lib` 跑可能 WEDGE（pass-branch 死锁，见记忆 `windows-ci-parity-full-verify`）：`.done` 超过 25 分钟不出现，看 `tasklist //FI "IMAGENAME eq alephcore-*.exe"`，杀掉测试二进制（不是 cargo）再跑一次带 `--test-threads=1`。
- [ ] **Step 3: 环境**
```bash
node --version          # 必须 ≥ v22.5（node:sqlite）；本机 v24.13.0
tasklist //FI "IMAGENAME eq rustc.exe" | head -3   # 没有别的 rustc 在跑
```
- [ ] **Step 4: 记录**——把 Step 2 的名单条数与 `test result:` 行、Step 3 的版本写进本文末尾「验证记录 › T0」。不提交任何东西。

---

### Task 1: `append_batch` + `Retire` + durability policy table (store)

**Files:**
- Modify: `src/session/events.rs:44-48` (`ErrorKind` untouched; add after `SessionEventRecord` :451), `:230-449` (add `ResumeAttempted` variant after `RunFinished` :257)
- Modify: `src/session/store.rs:38-162` (trait), `:299-350` (append → append_batch), `:430-510` (retire_from/through share `retire_in_txn`), `:653-704` (two match arms), tests `:878+`
- Modify: `src/agents/subagent_spawner/fork.rs:216-241` (arm), `src/agents/subagent_spawner/fork/tests.rs:163-311` (delete `sample_of_every_kind`, import the shared one)
- Modify: `src/gateway/session_projector.rs:1231-1240` (`UnreadableRetirement::append` → `append_batch`), `src/session/actor.rs:464-486` (`CollideOnceStore::append` → `append_batch`, keep the collide-once behaviour)
- Test: `#[cfg(test)] mod tests` in `events.rs` and `store.rs` (both already exist)

**Interfaces:**
- Consumes: `SessionEventStore` (`store.rs:39`), `session_id_to_string/extract_turn_id/event_type_tag/render_event_text` (`store.rs:648-743`), `crate::utils::source_scan::{production_text, strip_comment_lines}` (`source_scan.rs:163,690`).
- Produces (events.rs):
  ```rust
  pub enum Retire { Through(EventSeq), From(EventSeq) }            // Copy, Debug, PartialEq, Eq
  pub enum Durability { Normal, Barrier }                            // Copy, Debug, PartialEq, Eq, PartialOrd, Ord (Normal < Barrier)
  pub const fn durability_of(event: &SessionEvent) -> Durability;
  pub fn batch_durability<'a>(events: impl Iterator<Item = &'a SessionEvent>) -> Durability;
  pub const fn ignorable(event: &SessionEvent) -> bool;              // always false this round
  SessionEvent::ResumeAttempted { target: EventSeq, attempt: u32 }
  #[cfg(test)] pub(crate) mod fixtures { pub(crate) fn sample_of_every_kind() -> Vec<(&'static str, SessionEvent)> }
  ```
  (store.rs):
  ```rust
  async fn append_batch(&self, session_id: &SessionId, first_seq: EventSeq, events: &[(SessionEvent, i64)],
                        retire: Option<Retire>, durability: Durability) -> Result<(), SessionError>;   // required
  async fn append(&self, ..)  // default = append_batch(sid, seq, &[(event.clone(), ts)], None, durability_of(event))
  fn retire_in_txn(conn: &Connection, session_key: &str, retire: Retire, at: i64) -> rusqlite::Result<usize>; // private
  fn encode_payload(event: &SessionEvent) -> Result<String, SessionError>;  // private seam: today serde_json::to_string; T14 replaces it with the pub `encode_row` (delete `encode_payload` then — ONE encoder)
  ```

- [ ] **Step 1: Write the failing tests** (`store.rs` tests module; helpers `make_store/sample_session_id/turn_started/run_started/user_message` exist at :979-1198)

```rust
#[tokio::test]
async fn a_batch_whose_third_row_collides_leaves_nothing_behind() {
    // Atomicity without an injected failure: pre-seed seq 3, then batch [1,2,3] + Retire::Through(0).
    let store = make_store();
    let sid = sample_session_id();
    let tid = uuid::Uuid::new_v4();
    let at = now_ms();
    store.append(&sid, 3, &turn_started(tid, at), at).await.unwrap();
    let batch = vec![(turn_started(tid, at), at), (user_message(tid, "x", at), at), (turn_started(tid, at), at)];
    let err = store.append_batch(&sid, 1, &batch, None, Durability::Normal).await.unwrap_err();
    assert!(matches!(err, SessionError::Storage(_)), "{err:?}");
    let live: Vec<_> = store.load_all_events(&sid).await.unwrap().into_iter().map(|r| r.seq).collect();
    assert_eq!(live, vec![3], "rows 1 and 2 must have rolled back with row 3");
}

#[tokio::test]
async fn retire_and_insert_are_one_transaction() {
    let store = make_store();
    let sid = sample_session_id();
    let tid = uuid::Uuid::new_v4();
    let at = now_ms();
    for seq in 1..=3 { store.append(&sid, seq, &user_message(tid, "old", at), at).await.unwrap(); }
    store.append(&sid, 5, &turn_started(tid, at), at).await.unwrap(); // makes seq 5 collide below
    let batch = vec![(run_finished("r", at), at), (turn_started(tid, at), at)];
    store.append_batch(&sid, 4, &batch, Some(Retire::From(2)), Durability::Normal).await.unwrap_err();
    let live: Vec<_> = store.load_all_events(&sid).await.unwrap().into_iter().map(|r| r.seq).collect();
    assert_eq!(live, vec![1, 2, 3, 5], "a failed batch must not have retired anything either");
    // And the successful shape: the batch's own rows are live, the retired range is not.
    let batch = vec![(run_finished("r", at), at)];
    store.append_batch(&sid, 6, &batch, Some(Retire::From(2)), Durability::Normal).await.unwrap();
    let live: Vec<_> = store.load_all_events(&sid).await.unwrap().into_iter().map(|r| r.seq).collect();
    assert_eq!(live, vec![1, 6]);
    assert!(store.search_events(&sid, "old", 10).await.unwrap().is_empty(), "From deletes the BM25 mirror like retire_from");
}

#[tokio::test]
async fn barrier_restores_normal_after_success_and_after_failure() {
    let store = make_store();
    let sid = sample_session_id();
    let at = now_ms();
    let sync = |s: &SqliteEventStore| async move {
        s.conn.lock().await.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0)).unwrap()
    };
    store.append_batch(&sid, 1, &[(run_started("r", at), at)], None, Durability::Barrier).await.unwrap();
    assert_eq!(sync(&store).await, 1, "NORMAL restored after a Barrier commit");
    store.append_batch(&sid, 1, &[(run_started("r", at), at)], None, Durability::Barrier).await.unwrap_err();
    assert_eq!(sync(&store).await, 1, "NORMAL restored after a Barrier rollback");
}

#[tokio::test]
async fn an_empty_batch_with_nothing_to_retire_is_refused() {
    let store = make_store();
    let err = store.append_batch(&sample_session_id(), 1, &[], None, Durability::Normal).await.unwrap_err();
    assert!(matches!(err, SessionError::Other(_)));
}
```

(`events.rs` tests module) — the census, derived from the enum source and from the real policy function:

```rust
fn variant_names_from_source() -> Vec<String> {
    let src = crate::utils::source_scan::production_text(std::path::Path::new(file!()), include_str!("events.rs"));
    let src = crate::utils::source_scan::strip_comment_lines(&src);
    let body = src.split("pub enum SessionEvent {").nth(1).expect("enum present");
    let body = body.split("\n}").next().expect("enum closes");
    body.lines()
        .filter(|l| l.starts_with("    ") && !l.starts_with("     ") && !l.trim_start().starts_with('#'))
        .filter_map(|l| l.trim().split([' ', '{', '(']).next().map(str::to_string))
        .filter(|n| n.chars().next().is_some_and(char::is_uppercase))
        .collect()
}

#[test]
fn durability_barrier_set_is_exactly_the_four_ruled_events() {
    let sample = fixtures::sample_of_every_kind();
    let mut sampled: Vec<&str> = sample.iter().map(|(n, _)| *n).collect();
    sampled.sort_unstable();
    let mut declared = variant_names_from_source();
    declared.sort_unstable();
    assert_eq!(sampled, declared, "sample_of_every_kind() must construct every variant");
    let mut barrier: Vec<&str> = sample.iter().filter(|(_, e)| durability_of(e) == Durability::Barrier).map(|(n, _)| *n).collect();
    barrier.sort_unstable();
    assert_eq!(barrier, ["ResumeAttempted", "RunStarted", "ToolCallRequested", "UserMessage"]);   // U3
    assert!(sample.iter().all(|(_, e)| !ignorable(e)), "no event is ignorable this round");
    let src = crate::utils::source_scan::production_text(std::path::Path::new(file!()), include_str!("events.rs"));
    let fn_body = src.split("fn durability_of").nth(1).unwrap().split("\n}").next().unwrap();
    assert!(!fn_body.contains("_ =>"), "durability_of must force a decision on every new variant");
    assert_eq!(batch_durability([&sample[0].1, &SessionEvent::UserMessage { turn_id: uuid::Uuid::new_v4(), content: MessageContent { text: "u".into(), blocks: vec![], thinking: None, thinking_signature: None }, at: 0, synthetic: false, author_user_id: None }].into_iter()), Durability::Barrier);
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::store::tests session::events::tests` → `error[E0425]: cannot find function \`append_batch\`` / `error[E0433]: failed to resolve: use of undeclared type \`Durability\``.

- [ ] **Step 3: Minimal implementation**

`events.rs` (after `RunFinished` at :257):
```rust
    /// Intent stamp written by `ResumeCoordinator` BEFORE it re-triggers a run
    /// (§5.1). `target` = seq of the `RunStarted` being resumed, or of the
    /// unanswered `UserMessage`. Marker-class: no turn, not prompt-bearing.
    ResumeAttempted { target: EventSeq, attempt: u32 },
```
`events.rs` (after `SessionEventRecord`):
```rust
/// What a batch retires in the same transaction as its inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retire { Through(EventSeq), From(EventSeq) }

/// Commit durability. `Barrier` fsyncs the WAL at commit (`PRAGMA synchronous=FULL`
/// for that one transaction); `Normal` is the store's resting level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Durability { Normal, Barrier }

/// THE durability policy table (U3). One function; no wildcard arm on purpose —
/// a new variant must state its column here or the crate does not compile.
pub const fn durability_of(event: &SessionEvent) -> Durability {
    match event {
        SessionEvent::ToolCallRequested { .. }
        | SessionEvent::RunStarted { .. }
        | SessionEvent::ResumeAttempted { .. }
        | SessionEvent::UserMessage { .. } => Durability::Barrier,
        SessionEvent::SessionWoken { .. }
        | SessionEvent::RunFinished { .. }
        | SessionEvent::TurnStarted { .. }
        | SessionEvent::AssistantMessage { .. }
        | SessionEvent::AssistantRunMeta { .. }
        | SessionEvent::SystemMessage { .. }
        | SessionEvent::ToolCallApproved { .. }
        | SessionEvent::ToolCallDenied { .. }
        | SessionEvent::ToolResult { .. }
        | SessionEvent::ToolError { .. }
        | SessionEvent::SubagentSpawned { .. }
        | SessionEvent::SubagentReturned { .. }
        | SessionEvent::CompactionPerformed { .. }
        | SessionEvent::SessionForked { .. }
        | SessionEvent::Error { .. } => Durability::Normal,
        // T11 adds `| SessionEvent::ToolCallParked { .. }` to THIS group (U3: Normal) and its
        // sample to `fixtures::sample_of_every_kind` — the census below refuses to compile until it does.
    }
}

/// A batch is as durable as its most durable member.
pub fn batch_durability<'a>(events: impl Iterator<Item = &'a SessionEvent>) -> Durability {
    events.map(durability_of).max().unwrap_or(Durability::Normal)
}

/// Second column of the same table (§7.3): may an old binary skip this row
/// unread? Nothing declares it this round; the column exists so the envelope
/// writer (group C) has one place to ask.
pub const fn ignorable(event: &SessionEvent) -> bool {
    let _ = event;
    false
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    /// One constructed event per variant — moved here from `fork/tests.rs` so
    /// the fork guard and the durability census share ONE list. Completeness is
    /// pinned against the enum source in `tests::durability_barrier_set_is_exactly_the_four_ruled_events`.
    pub(crate) fn sample_of_every_kind() -> Vec<(&'static str, SessionEvent)> {
        // body = fork/tests.rs:166-311 verbatim, with `user/assistant/call/result` helpers inlined,
        // plus: ("ResumeAttempted", SessionEvent::ResumeAttempted { target: 1, attempt: 1 }),
    }
}
```
`fork/tests.rs`: delete its `sample_of_every_kind` (:163-311), add `use crate::session::events::fixtures::sample_of_every_kind;`. `fork.rs:233`: add `| SessionEvent::ResumeAttempted { .. }` to the `false` arm. `store.rs:670`: add `| SessionEvent::ResumeAttempted { .. }` to the `None` arm; `:685` add `SessionEvent::ResumeAttempted { .. } => "resume_attempted",`.

`store.rs` trait:
```rust
    async fn append_batch(&self, session_id: &SessionId, first_seq: EventSeq, events: &[(SessionEvent, i64)],
                          retire: Option<Retire>, durability: Durability) -> Result<(), SessionError>;

    /// Single append = a batch of one. Kept for direct-store writers and tests.
    async fn append(&self, session_id: &SessionId, seq: EventSeq, event: &SessionEvent, created_at_ms: i64) -> Result<(), SessionError> {
        let one = [(event.clone(), created_at_ms)];
        self.append_batch(session_id, seq, &one, None, durability_of(event)).await
    }
```
`store.rs` impl (replaces `append`, and `retire_from`/`retire_through` bodies):
```rust
struct EncodedRow { seq: i64, turn_id: Option<String>, event_type: &'static str, payload: String, created_at: i64, fts_body: Option<String> }

fn encode_payload(event: &SessionEvent) -> Result<String, SessionError> { Ok(serde_json::to_string(event)?) }

fn retire_in_txn(conn: &Connection, session_key: &str, retire: Retire, at: i64) -> rusqlite::Result<usize> {
    match retire {
        Retire::From(from_seq) => {
            let from_val = i64::try_from(from_seq).unwrap_or(i64::MAX);
            let n = conn.execute("UPDATE session_events SET retired_at = ?3 WHERE session_id = ?1 AND seq >= ?2 AND retired_at IS NULL", params![session_key, from_val, at])?;
            conn.execute("DELETE FROM session_events_fts WHERE session_id = ?1 AND seq >= ?2", params![session_key, from_val])?;
            Ok(n)
        }
        Retire::Through(through_seq) => {
            let through_val = i64::try_from(through_seq).unwrap_or(i64::MAX);
            conn.execute("UPDATE session_events SET retired_at = ?3 WHERE session_id = ?1 AND seq <= ?2 AND retired_at IS NULL", params![session_key, through_val, at])
        }
    }
}

fn write_batch(conn: &mut Connection, session_key: &str, rows: &[EncodedRow], retire: Option<Retire>, at: i64) -> Result<(), SessionError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| SessionError::Storage(format!("append_batch BEGIN IMMEDIATE failed: {e}")))?;
    // Retire FIRST so the batch's own rows, appended after, stay live.
    if let Some(r) = retire { retire_in_txn(&tx, session_key, r, at).map_err(|e| SessionError::Storage(e.to_string()))?; }
    for row in rows {
        tx.execute("INSERT INTO session_events (session_id, seq, turn_id, event_type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_key, row.seq, row.turn_id, row.event_type, row.payload, row.created_at])
            .map_err(|e| SessionError::Storage(e.to_string()))?;
    }
    tx.commit().map_err(|e| SessionError::Storage(format!("append_batch COMMIT failed: {e}")))  // drop without commit = rollback
}

async fn append_batch(&self, session_id: &SessionId, first_seq: EventSeq, events: &[(SessionEvent, i64)], retire: Option<Retire>, durability: Durability) -> Result<(), SessionError> {
    if events.is_empty() && retire.is_none() {
        return Err(SessionError::Other("append_batch: empty batch with nothing to retire".into()));
    }
    let session_key = session_id_to_string(session_id)?;
    let mut rows = Vec::with_capacity(events.len());
    for (i, (event, at)) in events.iter().enumerate() {
        let seq = first_seq.checked_add(i as u64).ok_or_else(|| SessionError::Storage("seq overflow".into()))?;
        let seq = i64::try_from(seq).map_err(|_| SessionError::Storage(format!("seq {seq} exceeds i64::MAX")))?;
        rows.push(EncodedRow { seq, turn_id: extract_turn_id(event).map(|u| u.to_string()), event_type: event_type_tag(event),
                               payload: encode_payload(event)?, created_at: *at, fts_body: render_event_text(event) });
    }
    let at = crate::session::events::now_ms();
    let mut conn = self.conn.lock().await;
    if durability == Durability::Barrier {
        conn.execute_batch("PRAGMA synchronous=FULL").map_err(|e| SessionError::Storage(format!("PRAGMA synchronous=FULL: {e}")))?;
    }
    let written = write_batch(&mut conn, &session_key, &rows, retire, at);
    if durability == Durability::Barrier {
        if let Err(e) = conn.execute_batch("PRAGMA synchronous=NORMAL") {
            // Stuck at FULL is slower, never less durable.
            tracing::warn!(error = %e, "append_batch: could not restore PRAGMA synchronous=NORMAL");
        }
    }
    written?;
    // BM25 mirror: best-effort, outside the transaction (unchanged contract).
    for row in rows.iter().filter(|r| r.fts_body.is_some()) {
        if let Err(e) = conn.execute("INSERT INTO session_events_fts (body, session_id, seq, event_type, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![row.fts_body, session_key, row.seq, row.event_type, row.created_at]) {
            tracing::debug!(error = %e, "session_events_fts index insert failed; session_search degraded");
        }
    }
    Ok(())
}

async fn retire_from(&self, session_id: &SessionId, from_seq: EventSeq) -> Result<usize, SessionError> {
    let session_key = session_id_to_string(session_id)?;
    let at = crate::session::events::now_ms();
    let mut conn = self.conn.lock().await;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| SessionError::Storage(format!("retire_from BEGIN failed: {e}")))?;
    let n = retire_in_txn(&tx, &session_key, Retire::From(from_seq), at).map_err(|e| SessionError::Storage(e.to_string()))?;
    tx.commit().map_err(|e| SessionError::Storage(format!("retire_from COMMIT failed: {e}")))?;
    Ok(n)
}
// retire_through: identical shape with Retire::Through(through_seq).
```
`session_projector.rs:1232` and `actor.rs:465`: replace the `append` impl with an `append_batch` impl of the same behaviour (CollideOnceStore asserts `first_seq == 1` then `== 2`).

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::store::tests session::events::tests agents::subagent_spawner::fork gateway::session_projector::tests session::actor::tests`
- [ ] **Step 5: Mutation check** (§11 ledger "4.1 retire out of the transaction") — move the `retire_in_txn` call in `write_batch` to after `tx.commit()` on `conn` directly ⇒ `retire_and_insert_are_one_transaction` RED (`[1, 3, 5]` ≠ `[1, 2, 3, 5]`). Restore.
- [ ] **Step 6: Commit** — `git add src/session/events.rs src/session/store.rs src/agents/subagent_spawner/fork.rs src/agents/subagent_spawner/fork/tests.rs src/gateway/session_projector.rs src/session/actor.rs` · `session: append_batch with in-transaction retire and barrier durability` + `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`

---

### Task 2: `ActorCommand::EmitBatch` + `SessionService::emit_batch`

**Files:**
- Modify: `src/session/actor.rs:21-37` (enum), `:69-139` (`finish_emitted` loses `reply`), `:160-202` and `:232-268` (both arms route through one writer), tests `:306+`
- Modify: `src/session/service.rs:38-61` (trait default), `src/session/in_process.rs:326-347` (+ `emit_batch`), tests `:495+` (+ `install_test_session_service`)
- Test: existing `#[cfg(test)] mod tests` in `actor.rs` and `in_process.rs`

**Interfaces:**
- Consumes: T1 `append_batch`, `batch_durability`, `Retire`; `SessionEventObserver::on_appended` (`observer.rs`).
- Produces:
  ```rust
  ActorCommand::EmitBatch { events: Vec<SessionEvent>, retire: Option<Retire>, reply: oneshot::Sender<Result<Vec<EventSeq>, SessionError>> }
  // SessionService (trait, DEFAULT = Err — see corrections):
  async fn emit_batch(&self, id: &SessionId, events: Vec<SessionEvent>, retire: Option<Retire>) -> Result<Vec<EventSeq>, SessionError>
  #[cfg(test)] pub(crate) fn install_test_session_service() -> Arc<InProcessActorSessionService>   // in_process.rs; wraps install_test_event_store()
  ```

- [ ] **Step 1: Write the failing tests** (`in_process.rs` tests; `fresh_service/sample_id` exist at :497-506; `Counter` observer shape at :680)

```rust
#[tokio::test]
async fn a_batch_is_contiguous_observed_in_order_and_not_refired_on_replay() {
    struct Seen(std::sync::Mutex<Vec<EventSeq>>);
    impl crate::session::observer::SessionEventObserver for Seen {
        fn on_appended(&self, _id: &SessionId, rec: &SessionEventRecord) { self.0.lock().unwrap_or_else(|e| e.into_inner()).push(rec.seq); }
    }
    let seen = Arc::new(Seen(std::sync::Mutex::new(vec![])));
    let store = test_store().await;                       // as fresh_service() builds it
    let svc = InProcessActorSessionService::new(store).with_observer(seen.clone());
    let id = sample_id("batch");
    let t = uuid::Uuid::new_v4();
    let seqs = svc.emit_batch(&id, vec![turn_started(t), user_msg(t, "hi"), run_started("r")], None).await.unwrap();
    assert_eq!(seqs, vec![1, 2, 3]);
    assert_eq!(*seen.0.lock().unwrap(), vec![1, 2, 3]);
    svc.detach(&id).await.unwrap();
    svc.attach(id.clone()).await.unwrap();                // replays; observer must NOT fire again
    assert_eq!(seen.0.lock().unwrap().len(), 3);
    assert_eq!(svc.get_events(&id, None, None).await.unwrap().len(), 3);
}

#[tokio::test]
async fn a_batch_with_retire_from_keeps_its_own_rows_live() {
    let svc = fresh_service().await;
    let id = sample_id("retire-batch");
    let t = uuid::Uuid::new_v4();
    for _ in 0..3 { svc.emit_event(&id, turn_started(t)).await.unwrap(); }
    let seqs = svc.emit_batch(&id, vec![run_finished("r")], Some(Retire::From(2))).await.unwrap();
    assert_eq!(seqs, vec![4]);
    let live: Vec<_> = svc.get_events(&id, None, None).await.unwrap().into_iter().map(|r| r.seq).collect();
    assert_eq!(live, vec![1, 4]);
}

#[tokio::test]
async fn an_empty_batch_is_refused_before_the_store() {
    let svc = fresh_service().await;
    assert!(matches!(svc.emit_batch(&sample_id("empty"), vec![], None).await, Err(SessionError::Other(_))));
}
```
(`actor.rs`: adapt `actor_self_heals_seq_after_append_collision` to send `EmitBatch` with two events; `CollideOnceStore::append_batch` asserts `first_seq == 1` on call 0 and `== 2` on call 1 — the retry re-sends the WHOLE batch.)

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::in_process::tests session::actor::tests` → `error[E0599]: no method named \`emit_batch\`` / `no variant named \`EmitBatch\``.

- [ ] **Step 3: Minimal implementation**

`actor.rs`:
```rust
pub enum ActorCommand {
    EmitEvent { event: SessionEvent, reply: oneshot::Sender<Result<EventSeq, SessionError>> },
    EmitBatch { events: Vec<SessionEvent>, retire: Option<Retire>, reply: oneshot::Sender<Result<Vec<EventSeq>, SessionError>> },
    GetEvents { .. }, Subscribe { .. }, Shutdown { .. },
}

impl SessionActor {
    /// The one store call. Seqs `head+1..`; on failure resync `head_seq` and retry
    /// the WHOLE batch once (the txn wrote nothing, so a retry cannot double-write).
    async fn write_batch(&mut self, events: &[(SessionEvent, i64)], retire: Option<Retire>) -> Result<EventSeq, SessionError> {
        let durability = batch_durability(events.iter().map(|(e, _)| e));
        let mut first = self.head_seq + 1;
        let mut result = self.store.append_batch(&self.id, first, events, retire, durability).await;
        if result.is_err() {
            if let Ok(stored_head) = self.store.load_head_seq(&self.id).await {
                self.head_seq = stored_head;
                first = stored_head + 1;
                result = self.store.append_batch(&self.id, first, events, retire, durability).await;
            }
        }
        result.map(|()| first)
    }

    async fn handle_emit_batch(&mut self, events: Vec<SessionEvent>, retire: Option<Retire>, reply: oneshot::Sender<Result<Vec<EventSeq>, SessionError>>) -> bool {
        if events.is_empty() && retire.is_none() {
            let _ = reply.send(Err(SessionError::Other("emit_batch: empty batch with nothing to retire".into())));
            return false;
        }
        let at = now_ms();
        let pairs: Vec<(SessionEvent, i64)> = events.into_iter().map(|e| (e, at)).collect();
        match self.write_batch(&pairs, retire).await {
            Ok(first) => {
                let seqs: Vec<EventSeq> = (0..pairs.len() as u64).map(|i| first + i).collect();
                for (i, (event, at)) in pairs.into_iter().enumerate() { self.finish_emitted(first + i as u64, event, at); }
                let _ = reply.send(Ok(seqs));
                true
            }
            Err(e) => { let _ = reply.send(Err(e)); false }
        }
    }

    async fn handle_emit_one(&mut self, event: SessionEvent, reply: oneshot::Sender<Result<EventSeq, SessionError>>) -> bool {
        let at = now_ms();
        let pair = [(event, at)];
        match self.write_batch(&pair, None).await {
            Ok(first) => { let [(event, at)] = pair; self.finish_emitted(first, event, at); let _ = reply.send(Ok(first)); true }
            Err(e) => { let _ = reply.send(Err(e)); false }
        }
    }
}
```
`finish_emitted(&mut self, seq, event, at)` = existing body minus the `reply.send`. Hot arm: `EmitEvent` ⇒ `if self.handle_emit_one(event, reply).await { idle_deadline = ... }`; `EmitBatch` ⇒ same with `handle_emit_batch`. Drain arm: both call the handlers (delete the duplicated self-heal at :237-267).

`service.rs` trait (after `emit_event`):
```rust
    /// Append `events` and apply `retire` in ONE store transaction (§4.1).
    /// Default is a refusal, not a loop of `emit_event`: a service that cannot
    /// commit atomically must say so rather than report a batch that can tear.
    async fn emit_batch(&self, id: &SessionId, events: Vec<SessionEvent>, retire: Option<Retire>) -> Result<Vec<EventSeq>, SessionError> {
        let _ = (id, events, retire);
        Err(SessionError::Other("this SessionService cannot append atomically (only InProcessActorSessionService can)".into()))
    }
```
`in_process.rs`:
```rust
    async fn emit_batch(&self, id: &SessionId, events: Vec<SessionEvent>, retire: Option<Retire>) -> Result<Vec<EventSeq>, SessionError> {
        let sender = match self.sender_for(id).await { Some(s) => s, None => self.spawn_actor(id, None).await? };
        let (tx, rx) = oneshot::channel();
        sender.send(ActorCommand::EmitBatch { events, retire, reply: tx }).await
            .map_err(|e| { tracing::warn!(session_id = ?id, error = %e, "EmitBatch send failed"); SessionError::ActorShutdown })?;
        rx.await.map_err(|e| { tracing::warn!(session_id = ?id, error = %e, "EmitBatch reply dropped"); SessionError::ActorShutdown })?
    }
    async fn emit_event(&self, id: &SessionId, event: SessionEvent) -> Result<EventSeq, SessionError> {
        let seqs = self.emit_batch(id, vec![event], None).await?;
        seqs.first().copied().ok_or_else(|| SessionError::Other("emit_batch: one event in, no seq out".into()))
    }

#[cfg(test)]
pub(crate) fn install_test_session_service() -> Arc<InProcessActorSessionService> {
    static SVC: std::sync::OnceLock<Arc<InProcessActorSessionService>> = std::sync::OnceLock::new();
    let svc = SVC.get_or_init(|| Arc::new(InProcessActorSessionService::new(crate::session::store::install_test_event_store()))).clone();
    crate::session::service::set_global_session_service(svc.clone());
    svc
}
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::in_process::tests session::actor::tests`
- [ ] **Step 6: Commit** — `git add src/session/actor.rs src/session/service.rs src/session/in_process.rs` · `session: EmitBatch actor arm and SessionService::emit_batch` + Claude-Session line.

---

### Task 6: `ResumeAttempted` — the ratchet moves to the intent side

**Files:**
- Modify (after T1/T2 — locate by symbol, lines have moved): `src/session/reduction.rs:54-135` (enum + `tag` + `Display`), `:176-191` (`RunDisposition`), `:299-333` (`is_marker`, `reduce_disposition`), `:378-483` (scan), tests `:804-852, 1480-1491, 1526-1540, 1618-1661`
- Modify: `src/session/store.rs:537-548` (`load_run_markers` SQL), `:683-703` (`event_type_tag` gets the arm A1 added — verify), tests
- Modify: `src/gateway/resume_coordinator.rs:164-200` (`ResumeRefusal`), `:831-834`, `:910-1070` (`handle_interrupted`), tests `:1441-1481`
- Modify: `src/gateway/session_snapshot.rs:164-171, 215-221`; `shared/protocol/src/session_thread.rs:436-440` (doc only)
- Modify: `src/session/marker_balance.rs:149,180` (test literals)
- Test: `tests/resume_coordinator_integration.rs` (+1 test; rewrite `:697-859`)

**Interfaces:**
- Consumes: `SessionEvent::ResumeAttempted { target, attempt }` (A1), `SessionEventStore::append` (A1 default method), `reduce_run`/`reduce_disposition` (existing).
- Produces: `RunDisposition::Interrupted { attempts: u32 }`; `pub(crate) fn is_marker(&SessionEvent) -> bool`; `LogContradiction::ResumeWithoutTarget { seq }` (REPORT, tag `session-log-resume-without-target`); `store::MARKER_EVENT_TYPES: [&str; 3]`; `ResumeRefusal::IntentStampFailed(String)` (reason `intent_stamp_failed`); `ResumeCoordinator::stamp_resume_attempt(&self, &SessionId, target: EventSeq, attempt: u32) -> Result<(), SessionError>` (private).

- [ ] **Step 1: Write the failing tests** (reduction.rs `mod tests`; add helper next to `finished`)

```rust
fn attempted(target: EventSeq, attempt: u32) -> SessionEvent {
    SessionEvent::ResumeAttempted { target, attempt }
}

#[test]
fn attempts_count_intent_stamps_not_trailing_starts() {
    // Three boots that each stamped intent and crashed before RunStarted.
    let markers = vec![
        rec(1, started("a")), rec(2, attempted(1, 1)), rec(3, attempted(1, 2)), rec(4, attempted(1, 3)),
    ];
    assert_eq!(reduce_disposition(&markers), Ok(RunDisposition::Interrupted { attempts: 3 }));
    // Two RunStarted with no stamp between them: the old counter said 2, the
    // ratchet says 0 — nobody has *tried* to resume this yet.
    let markers = vec![rec(1, started("a")), rec(2, started("b"))];
    assert_eq!(reduce_disposition(&markers), Ok(RunDisposition::Interrupted { attempts: 0 }));
    // A RunFinished resets the count.
    let markers = vec![rec(1, started("a")), rec(2, attempted(1, 1)), rec(3, finished("a")), rec(4, started("b"))];
    assert_eq!(reduce_disposition(&markers), Ok(RunDisposition::Interrupted { attempts: 0 }));
}

#[test]
fn a_stamp_with_nothing_to_resume_is_reported_and_ignored() {
    let events = vec![rec(1, started("a")), rec(2, finished("a")), rec(3, attempted(1, 1))];
    let r = reduced(&events);
    assert_eq!(r.disposition, RunDisposition::Clean);
    assert_eq!(tags(&r), vec!["session-log-resume-without-target"]);
}
```

In `mod g1`, widen `event_for` to 6 tags (`5 => attempted(seq, 1)`, `tag % 6`) and `markers_of` to `is_marker`; add the §5.6 property:

```rust
proptest! {
    #[test]
    fn attempts_are_the_stamps_since_the_last_finish(tags in prop::collection::vec(0u8..6, 0..40)) {
        let events: Vec<SessionEventRecord> = tags.iter().enumerate()
            .map(|(i, t)| rec(i as EventSeq + 1, event_for(*t, i as EventSeq + 1))).collect();
        let since_finish = events.iter()
            .rposition(|r| matches!(r.event, SessionEvent::RunFinished { .. }))
            .map_or(0, |i| i + 1);
        let expected = events[since_finish..].iter()
            .filter(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. })).count() as u32;
        if let Ok(RunDisposition::Interrupted { attempts }) = reduce_disposition(&markers_of(&events)) {
            prop_assert_eq!(attempts, expected);
        }
    }
}
```

`kind_index`: `ResumeWithoutTarget { .. } => 9`, `KIND_COUNT = 10`, add a sample to `one_of_each` (B/C append theirs after — the exhaustive match is the census, not the number). Update `disposition_counts_the_trailing_starts` → expects `attempts: 0` and rename it; `a_dangling_call_under_a_finished_run_is_reported_as_earlier` → `attempts: 0`; marker_balance.rs:149,180 → `attempts: 0`.

store.rs tests (reuse the file's `run_started`/`run_finished` helpers):

```rust
#[tokio::test]
async fn load_run_markers_returns_resume_attempted_as_a_marker() {
    let store = mk_store(); let sid = SessionKey::main("m"); let at = 1_700_000_000_000;
    store.append(&sid, 1, &run_started("r1", at), at).await.unwrap();
    store.append(&sid, 2, &SessionEvent::ResumeAttempted { target: 1, attempt: 1 }, at + 1).await.unwrap();
    store.append(&sid, 3, &SessionEvent::SystemMessage { turn_id: uuid::Uuid::new_v4(), content: "x".into(), at }, at + 2).await.unwrap();
    let groups = store.load_run_markers().await.unwrap();
    let seqs: Vec<u64> = groups[0].1.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![1, 2]);
}

/// The SQL IN-list and the reducer's `is_marker` are two spellings of one set.
#[test]
fn marker_event_types_are_exactly_the_reducers_marker_set() {
    // ONE sampler for the whole enum: T1's `events::fixtures::sample_of_every_kind()`, whose
    // completeness is pinned against the enum source there. A second sampler here would be
    // the same list twice (criterion #1).
    let all: Vec<SessionEvent> = crate::session::events::fixtures::sample_of_every_kind().into_iter().map(|(_, e)| e).collect();
    let derived: std::collections::BTreeSet<&str> =
        all.iter().filter(|e| crate::session::reduction::is_marker(e)).map(|e| event_type_tag(e)).collect();
    let declared: std::collections::BTreeSet<&str> = MARKER_EVENT_TYPES.iter().copied().collect();
    assert_eq!(derived, declared);
}
```

(The sampler is T1's `fixtures::sample_of_every_kind()`; a variant T11 adds without a sample reddens T1's census, which is the point.)

Integration test (tests/resume_coordinator_integration.rs):

```rust
/// §5.1 / §5.6: three boots whose retrigger never reaches `RunStarted` — the
/// adapter records the call and writes nothing, i.e. a crash in admit / hook /
/// seed. Under the old counter `trailing_starts` never moved and this looped
/// forever; the intent stamp counts the ATTEMPT, not the run's own marker.
#[tokio::test]
async fn a_retrigger_that_never_starts_is_capped_by_the_intent_stamp() {
    let store = store();
    let sid = SessionKey::main("ratchet-agent");
    seed_interrupted_run(&store, &sid).await;
    let cfg = ResumeConfig { max_attempts: 2, ..ResumeConfig::default() };
    let adapter = Arc::new(RecordingAdapter::new());
    let calls = adapter.calls.clone();
    let registry = registry_with_agent(sid.agent_id()).await;
    let boot = |n: usize| async {
        // each boot is a fresh coordinator over the SAME log
        let c = ResumeCoordinator::new(store.clone(), cfg.clone(), adapter.clone() as Arc<dyn ExecutionAdapter>, registry.clone(), sessions(), test_bus());
        let r = c.resume_interrupted_runs().await;
        (n, r)
    };
    let (_, r1) = boot(1).await; assert_eq!((r1.resumed, r1.abandoned), (1, 0));
    let (_, r2) = boot(2).await; assert_eq!((r2.resumed, r2.abandoned), (1, 0));
    let (_, r3) = boot(3).await; assert_eq!((r3.resumed, r3.abandoned), (0, 1), "attempts == max_attempts: abandon, no retrigger");
    let (_, r4) = boot(4).await; assert_eq!((r4.resumed, r4.abandoned, r4.skipped), (0, 0, 1));
    assert_eq!(calls.lock().await.len(), 2, "exactly two retriggers were ever dispatched");
    let stamps: Vec<(u64, u32)> = store.load_all_events(&sid).await.unwrap().iter()
        .filter_map(|r| match &r.event { SessionEvent::ResumeAttempted { target, attempt } => Some((*target, *attempt)), _ => None }).collect();
    assert_eq!(stamps, vec![(3, 1), (3, 2)], "each stamp names the RunStarted (seq 3) and its ordinal");
}
```

Rewrite `crash_loop_cap_abandons_instead_of_retriggering` (`:704-732` and the passive-store half `:811-840`) to seed `RunStarted` + three `ResumeAttempted { target: 1, attempt: 1..=3 }` — the fixture used to manufacture a state production never writes (three bare `RunStarted`); after this task that state reads `attempts: 0` and the test would retrigger.

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::reduction session::store::tests::marker_event_types` → `error[E0026]: variant RunDisposition::Interrupted does not have a field named attempts` / `no variant ResumeWithoutTarget`; `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1 a_retrigger_that_never_starts` → compile error on `attempts`.

- [ ] **Step 3: Minimal implementation**

reduction.rs:

```rust
    /// A `ResumeAttempted` with nothing to resume: no run is open and no
    /// user message is unanswered at that point. Reading: ignored — the
    /// disposition is what it would be without the stamp.
    ResumeWithoutTarget { seq: EventSeq },
// tag(): Self::ResumeWithoutTarget { .. } => "session-log-resume-without-target",
// Display: write!(f, "ResumeAttempted at seq {seq} names no open run and no unanswered message")

pub enum RunDisposition {
    Clean,
    /// A `RunStarted` after the last `RunFinished`; `attempts` counts the
    /// `ResumeAttempted` stamps since that finish — the crash-loop ratchet,
    /// written by the coordinator BEFORE each retrigger, so a crash anywhere
    /// before the resumed run's own `RunStarted` still counts (§5.1).
    Interrupted { attempts: u32 },
}

pub(crate) fn is_marker(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::RunStarted { .. } | SessionEvent::RunFinished { .. } | SessionEvent::ResumeAttempted { .. })
}

pub fn reduce_disposition(markers: &[SessionEventRecord]) -> Result<RunDisposition, LogContradiction> {
    validate_slice(markers)?;
    if let Some(stray) = markers.iter().find(|r| !is_marker(&r.event)) {
        return Err(LogContradiction::NonMarkerInMarkerSlice { seq: stray.seq });
    }
    let since_finish = markers.iter()
        .rposition(|r| matches!(r.event, SessionEvent::RunFinished { .. }))
        .map_or(0, |i| i + 1);
    let tail = &markers[since_finish..];
    if !tail.iter().any(|r| matches!(r.event, SessionEvent::RunStarted { .. })) {
        return Ok(RunDisposition::Clean);
    }
    let attempts = tail.iter().filter(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. })).count();
    Ok(RunDisposition::Interrupted { attempts: u32::try_from(attempts).unwrap_or(u32::MAX) })
}
```

In `reduce_run`'s scan add the arm (T7 extends its predicate with `pending_unanswered`):

```rust
            SessionEvent::ResumeAttempted { .. } => {
                if open_run.is_none() {
                    contradictions.push(LogContradiction::ResumeWithoutTarget { seq: record.seq });
                }
                markers.push(record.clone());
            }
```

store.rs:

```rust
/// The `event_type_tag` of every variant `reduction::is_marker` accepts —
/// the one list `load_run_markers` selects by. Pinned equal by test.
pub(crate) const MARKER_EVENT_TYPES: [&str; 3] = ["run_started", "run_finished", "resume_attempted"];
// load_run_markers: replace the literal IN (...) with
let in_list = MARKER_EVENT_TYPES.iter().map(|t| format!("'{t}'")).collect::<Vec<_>>().join(", ");
let sql = format!("SELECT session_id, seq, payload_json, created_at FROM session_events WHERE event_type IN ({in_list}) AND retired_at IS NULL ORDER BY session_id, seq ASC");
let mut stmt = conn.prepare(&sql).map_err(|e| SessionError::Storage(e.to_string()))?;
```

resume_coordinator.rs:

```rust
    /// The stamp did not land, so the retrigger must not happen: a resume
    /// without its intent stamp is exactly the unbounded loop §5.1 closes.
    IntentStampFailed(String),
// reason(): Self::IntentStampFailed(_) => "intent_stamp_failed"; detail(): | Self::IntentStampFailed(e) => e.clone()

    /// §5.1: write the intent BEFORE the action. `append` (A1) makes this a
    /// Barrier commit, so a crash one instruction later still counts.
    async fn stamp_resume_attempt(&self, session_id: &SessionId, target: EventSeq, attempt: u32)
        -> Result<(), crate::session::service::SessionError> {
        let seq = self.next_seq(session_id).await?;
        self.event_store.append(session_id, seq, &SessionEvent::ResumeAttempted { target, attempt }, now_ms()).await
    }
```

`handle_interrupted(&self, session_id, markers, attempts: u32, report)`: cap check becomes `if attempts >= self.config.max_attempts`; between the repair and `retrigger`:

```rust
        let Some(target) = reduction.run_anchor else {
            report.refused.push((session_id.clone(), ResumeRefusal::IntentStampFailed("interrupted run has no RunStarted anchor".into())));
            return;
        };
        if let Err(e) = self.stamp_resume_attempt(session_id, target, attempts + 1).await {
            tracing::warn!(session = ?session_id, error = %e, "resume: intent stamp failed; not retriggering");
            report.refused.push((session_id.clone(), ResumeRefusal::IntentStampFailed(e.to_string())));
            return;
        }
```

`resume_from_markers:831` → `Ok(RunDisposition::Interrupted { attempts }) => self.handle_interrupted(session_id, markers, attempts, report).await`. Unit tests `:1441-1481` → `attempts: 0`; delete `classify_counts_consecutive_trailing_starts` (its premise is gone) and add `classify_counts_stamps_since_last_finish` mirroring `attempts_count_intent_stamps_not_trailing_starts`. session_snapshot.rs:165-167, 216-218 → `RunDisposition::Interrupted { attempts } => (LastRunState::INTERRUPTED, attempts)`. protocol `LastRunState.trailing_starts` (`:436-440`): wire key unchanged (§5.1 "不加 wire 字段"), doc becomes "Resume attempts stamped since the last `RunFinished` — the crash-loop counter. The key keeps its historical spelling; no client renders it (only fixtures construct it)".

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::reduction session::store session::marker_balance gateway::resume_coordinator gateway::session_snapshot` then `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1`; `cargo test -p aleph-protocol`.
- [ ] **Step 5: Mutation check** — move the `stamp_resume_attempt` call to AFTER `self.retrigger(...)` ⇒ `a_retrigger_that_never_starts_is_capped_by_the_intent_stamp` RED (four boots retrigger, `calls.len() == 4`); T10 `ratchet` RED. Restore.
- [ ] **Step 6: Commit** — `git add src/session/reduction.rs src/session/store.rs src/session/marker_balance.rs src/gateway/resume_coordinator.rs src/gateway/session_snapshot.rs shared/protocol/src/session_thread.rs tests/resume_coordinator_integration.rs` — `session: count resume attempts from ResumeAttempted stamps, not trailing starts` + `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`

---

### Task 7: `Unanswered` — the seed→RunStarted window becomes a disposition

**Files:**
- Modify: `src/session/reduction.rs` (`RunDisposition`, `reduce_disposition`, scan), tests (`legal_shapes` + prefix test)
- Modify: `src/gateway/resume_coordinator.rs:678-713` (`resume_interrupted_runs`), `:759-851` (`resume_from_markers`), `:881-900` (`resume_session`), new `check_unanswered`/`handle_unanswered`
- Modify: `src/gateway/session_snapshot.rs:164-171, 215-221`; `src/gateway/projection_reconciler.rs:149-151`
- Modify: `shared/protocol/src/session_thread.rs:466-537` (+ tests `:596-609`)
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:348-393, 403-445`; `interfaces/webchat/locales/en.json:2702-2706, 305-307`; `interfaces/webchat/locales/zh.json` (same keys)
- Modify: `interfaces/tui/src/tui/commands.rs:543-553, 1075-1108`
- Test: census in `src/session/reduction.rs` tests (uses `crate::utils::source_scan::{rust_sources_under, production_text, code_text}`); integration test in `tests/resume_coordinator_integration.rs`

**Interfaces:**
- Consumes: T6 names; `SessionStore::list_sessions(SessionFilter { active_minutes, .. })` (`src/gateway/session_store/mod.rs:46`, semantics `session_manager/ops/crud.rs:91-98` bumps `last_active_at` on every `get_or_create`, which `execute()` calls before seeding); `SessionEventStore::load_events_range`; `ResumePlan::default()` (`resume_coordinator.rs:343`).
- Produces: `RunDisposition::Unanswered { user_seq: EventSeq, attempts: u32 }`; `pub(crate) fn is_disposition_bearing(&SessionEvent) -> bool`; `pub(crate) fn unanswered_eligible(&SessionId) -> bool`; `LastRunState::UNANSWERED = "unanswered"`, `LastRunDisposition::Unanswered`; `RunBadge::Unanswered`; locale keys `narration.last_run_unanswered`, `chat.run_badge_unanswered`.

**Decision on how the reducer sees the tail (prompt option a vs b): (b).** `load_run_markers` (`store.rs:537`) is one index-served scan of ~2 rows per run; widening it to `user_message`/`assistant_message` makes every boot and every `sessions.list` row (`last_run_from_markers`) read most of every transcript. Instead `reduce_disposition` accepts a **disposition-bearing** slice (markers ∪ `UserMessage` ∪ `AssistantMessage`) so there is still ONE derivation; the coordinator feeds it markers + a bounded tail `load_events_range(from = last_marker_seq + 1, to = None)` filtered to bearing events, only for candidates whose marker slice reduced `Clean`, plus every marker-less session in the activity window (`list_sessions { active_minutes }` — the same window `ProjectionReconciler::candidates` uses at `projection_reconciler.rs:180-188`). The list face (`last_run_from_markers`) stays marker-only and therefore never says `unanswered` — it already refuses to speak about the non-marker tail (`inspected: false`); the attach face does.

**T15 builds on this decision.** Because `Unanswered` is invisible to a marker-only slice, T15's `ResumeLaunch::pending` is deliberately OVER-inclusive (every session the scan visits: marker groups ∪ activity window) — holding a queued survivor a little longer is the safe direction; releasing it into a session the scan is about to act on is the collision T15 documents. T15 also moves this task's activity-window loop from `resume_interrupted_runs` into `launch_resume` (the loop body is unchanged; only its home moves).

- [ ] **Step 1: Write the failing tests** (reduction.rs)

```rust
#[test]
fn a_seeded_message_with_no_run_started_is_unanswered() {
    let bearing = vec![rec(1, started("a")), rec(2, finished("a")), rec(3, user("hi again"))];
    assert_eq!(reduce_disposition(&bearing), Ok(RunDisposition::Unanswered { user_seq: 3, attempts: 0 }));
    let stamped = vec![rec(1, user("hi")), rec(2, attempted(1, 1))];
    assert_eq!(reduce_disposition(&stamped), Ok(RunDisposition::Unanswered { user_seq: 1, attempts: 1 }));
    // Answered by an assistant row (simple engine / fast path) → Clean.
    let answered = vec![rec(1, user("hi")), rec(2, assistant("yo"))];
    assert_eq!(reduce_disposition(&answered), Ok(RunDisposition::Clean));
    // A harness-authored message is never "the user waiting".
    let synthetic = vec![rec(1, started("a")), rec(2, finished("a")), rec(3, SessionEvent::synthetic_user(TurnId::new_v4(), "nudge".into()))];
    assert_eq!(reduce_disposition(&synthetic), Ok(RunDisposition::Clean));
    // Still rejects a raw log: a tool dispatch bears on no disposition.
    assert_eq!(reduce_disposition(&[rec(1, user("hi")), rec(2, requested("c1"))]),
        Err(LogContradiction::NonMarkerInMarkerSlice { seq: 2 }));
}

#[test]
fn reduce_run_reads_the_unanswered_tail_from_a_full_log() {
    let events = seq_log(vec![turn_started(), user("hi"), started("r1"), assistant("ok"), finished("r1"), run_meta("r1"), turn_started(), user("second")]);
    assert_eq!(reduced(&events).disposition, RunDisposition::Unanswered { user_seq: 8, attempts: 0 });
}
```

Add to `legal_shapes`: `"seed then crash before RunStarted"` = `[turn_started(), user("hi")]`, `allowed: &[]`; and `"unanswered then abandoned closer"` = `[turn_started(), user("hi"), attempted(2,1), attempted(2,2), finished_as("abandoned-1", RunOutcome::Abandoned)]`, `allowed: &[FINISH_WITHOUT_START]` (extend the `exhibited` list in `the_finish_without_start_allowance_is_exercised_where_the_shape_produces_it` with this name). The prefix test then pins every prefix `Ok`. G1: `markers_of` → filter by `is_disposition_bearing`; `event_for` gains `6 => user("u")` (`tag % 7`).

Census (reduction.rs tests; the reason: §5.2's premise "the only writer of a `UserMessage` outside `[RunStarted, RunFinished]` on a resumable session is `seed_session`"):

```rust
/// Every production construction of `SessionEvent::UserMessage` — a
/// construction supplies all five fields and therefore never contains `..`;
/// a pattern always does. Equality, so a sixth producer is a red test.
#[test]
fn user_message_producers_are_the_known_set() {
    use crate::utils::source_scan::{code_text, production_text, rust_sources_under};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: std::collections::BTreeMap<String, usize> = Default::default();
    for (path, src) in rust_sources_under(&root) {
        let code = code_text(&production_text(std::path::Path::new(&path), &src));
        for body in code.split("SessionEvent::UserMessage {").skip(1) {
            let mut depth = 1usize; let mut end = 0usize;
            for (i, ch) in body.char_indices() { match ch { '{' => depth += 1, '}' => { depth -= 1; if depth == 0 { end = i; break; } } _ => {} } }
            let fields = body.get(..end).unwrap_or("");
            if !fields.contains("..") { *found.entry(path.replace('\\', "/").rsplit("src/").next().unwrap_or(&path).to_string()).or_default() += 1; }
        }
    }
    let expected: std::collections::BTreeMap<String, usize> = [
        ("agents/subagent_spawner/mod.rs", 1),   // child seed — excluded by `unanswered_eligible` (Subagent/Ephemeral keys)
        ("gateway/execution_engine/simple.rs", 1), // Simulated engine: user then assistant, no markers
        ("gateway/execution_engine/steering.rs", 1), // steer: only into a RUNNING session
        ("gateway/openai_api/completions/agent.rs", 1), // client history replay, Ephemeral key
        ("orchestrator/harness_bridge/backfill.rs", 1), // legacy transcript backfill, followed by a run
        ("orchestrator/harness_bridge/session_seed.rs", 2), // THE producer (multimodal + emit_message; history uses user_turn)
        ("session/events.rs", 2),               // synthetic_user (in-run) + user_turn (T8/seed constructor)
    ].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    assert!(found.get("orchestrator/harness_bridge/session_seed.rs").is_some(), "the scan found no seed — blind, not clean");
    assert_eq!(found, expected, "a new UserMessage producer must be classified against §5.2 (inside a run / child / ephemeral / seed)");
}
```

(Counts are what the reads at `5e85060b8` + T8 give; the implementer re-derives the map on the commit and writes the classification comment per row — the test is the instrument, the numbers here are not.)

resume_coordinator integration test:

```rust
#[tokio::test]
async fn an_unanswered_seed_is_stamped_and_retriggered_without_repair() {
    let store = store();
    let sid = SessionKey::main("unanswered-agent");
    let tid = TurnId::new_v4(); let at = now_ms();
    store.append(&sid, 1, &SessionEvent::TurnStarted { turn_id: tid, trigger: alephcore::session::events::TurnTrigger::UserMessage, at }, at).await.unwrap();
    store.append(&sid, 2, &SessionEvent::UserMessage { turn_id: tid, content: alephcore::session::events::MessageContent { text: "hello?".into(), blocks: vec![], thinking: None, thinking_signature: None }, at: at + 1, synthetic: false, author_user_id: None }, at + 1).await.unwrap();
    let sessions = sessions();
    sessions.get_or_create(&sid).await.unwrap(); // the row `execute()` creates before seeding — what puts it in the window
    let adapter = Arc::new(RecordingAdapter::new()); let calls = adapter.calls.clone();
    let c = ResumeCoordinator::new(store.clone(), ResumeConfig::default(), adapter as Arc<dyn ExecutionAdapter>, registry_with_agent(sid.agent_id()).await, sessions, test_bus());
    let r = c.resume_interrupted_runs().await;
    assert_eq!((r.scanned, r.resumed, r.unsnapshotted), (1, 1, 0), "not counted as unsnapshotted: there was no RunStarted to snapshot");
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));
    let all = store.load_all_events(&sid).await.unwrap();
    assert!(matches!(all.last().map(|r| &r.event), Some(SessionEvent::ResumeAttempted { target: 2, attempt: 1 })));
    assert!(!all.iter().any(|r| matches!(r.event, SessionEvent::ToolError { .. })), "no boundary repair: nothing dangled");
}
```

Protocol/Panel/TUI tests: extend `every_word_the_server_writes_has_a_distinct_reading` with `(LastRunState::UNANSWERED, LastRunDisposition::Unanswered)`; session_snapshot: `an_unanswered_log_is_reported_as_such` (`[started, finished, user]` ⇒ `view.disposition() == Unanswered`, `trailing_starts == 0`, `inspected`); Panel `chat_sidebar.rs` tests: `last_run_notice` for `disposition: "unanswered"` returns `Some(_)` containing the en string, `run_badge` → `Some(RunBadge::Unanswered)`; TUI: `last_run_mark` → `Some("  [unanswered]")`.

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::reduction` → `no variant named Unanswered`; `cargo test -p aleph-protocol` → `no associated item UNANSWERED`; `cargo test -p aleph-panel --lib`, `cargo test -p aleph-tui` → same.

- [ ] **Step 3: Minimal implementation**

reduction.rs:

```rust
pub enum RunDisposition {
    Clean,
    Interrupted { attempts: u32 },
    /// A real `UserMessage` after the last `RunFinished` (or the log start)
    /// with no `RunStarted` and no `AssistantMessage` after it: the crash
    /// landed in the seed→RunStarted window (§5.2). `user_seq` is that
    /// message's seq — the `ResumeAttempted.target` and the recency anchor.
    Unanswered { user_seq: EventSeq, attempts: u32 },
}
// (rewrite the enum doc: three variants; the fourth arrives with its consumer)

/// What `reduce_disposition` may be handed: the run markers and the two
/// message kinds that decide "was the user answered". Anything else in a
/// slice is the raw-log-by-mistake shape and is refused.
pub(crate) fn is_disposition_bearing(event: &SessionEvent) -> bool {
    is_marker(event) || matches!(event, SessionEvent::UserMessage { .. } | SessionEvent::AssistantMessage { .. })
}

pub fn reduce_disposition(markers: &[SessionEventRecord]) -> Result<RunDisposition, LogContradiction> {
    validate_slice(markers)?;
    if let Some(stray) = markers.iter().find(|r| !is_disposition_bearing(&r.event)) {
        return Err(LogContradiction::NonMarkerInMarkerSlice { seq: stray.seq });
    }
    let since_finish = markers.iter().rposition(|r| matches!(r.event, SessionEvent::RunFinished { .. })).map_or(0, |i| i + 1);
    let tail = &markers[since_finish..];
    let attempts = u32::try_from(tail.iter().filter(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. })).count()).unwrap_or(u32::MAX);
    if tail.iter().any(|r| matches!(r.event, SessionEvent::RunStarted { .. })) {
        return Ok(RunDisposition::Interrupted { attempts });
    }
    let last_user = tail.iter().rposition(|r| matches!(r.event, SessionEvent::UserMessage { synthetic: false, .. }));
    match last_user {
        Some(i) if !tail[i + 1..].iter().any(|r| matches!(r.event, SessionEvent::AssistantMessage { .. })) => {
            Ok(RunDisposition::Unanswered { user_seq: tail[i].seq, attempts })
        }
        _ => Ok(RunDisposition::Clean),
    }
}
```

`reduce_run`: push `UserMessage`/`AssistantMessage` records onto `markers` (rename the local to `bearing`), track `pending_unanswered: bool` (set on a non-synthetic `UserMessage`, cleared on `AssistantMessage`/`RunStarted`/`RunFinished`), and the T6 arm's predicate becomes `open_run.is_none() && !pending_unanswered`. Doc of `NonMarkerInMarkerSlice`: "carries an event that bears on no disposition".

resume_coordinator.rs:

```rust
/// Sessions the Unanswered arm may retrigger: not a scheduler-owned unit
/// (they re-run by their own rule) and not a sub-agent child or an ephemeral
/// side session (A7: children are reported, never re-driven; an ephemeral
/// session has no user waiting on it).
pub(crate) fn unanswered_eligible(key: &SessionId) -> bool {
    !has_own_scheduler(key) && !matches!(key, SessionId::Subagent { .. } | SessionId::Ephemeral { .. })
}

impl ResumeCoordinator {
    /// The Clean arm's second question, and the whole question for a session
    /// with no markers: is there a user message nobody answered? One bounded
    /// read past the last marker; the tail is filtered to what
    /// `reduce_disposition` accepts so the derivation stays the reducer's.
    /// Returns whether an unanswered tail was found (and acted on).
    async fn check_unanswered(&self, session_id: &SessionId, markers: &[SessionEventRecord], report: &mut ResumeReport) -> bool {
        if !unanswered_eligible(session_id) { return false; }
        let from = markers.last().map(|m| m.seq + 1);
        let tail = match self.event_store.load_events_range(session_id, from, None).await {
            Ok(t) => t,
            Err(e) => { tracing::warn!(session = ?session_id, error = %e, "resume: tail read failed; cannot tell whether the last message was answered");
                        report.refused.push((session_id.clone(), ResumeRefusal::BoundaryRepairFailed(e.to_string()))); return true; }
        };
        let mut bearing: Vec<SessionEventRecord> = markers.to_vec();
        let user_at: std::collections::HashMap<EventSeq, i64> = tail.iter().map(|r| (r.seq, r.created_at_ms)).collect();
        bearing.extend(tail.into_iter().filter(|r| crate::session::reduction::is_disposition_bearing(&r.event)));
        match reduce_disposition(&bearing) {
            Ok(RunDisposition::Unanswered { user_seq, attempts }) => {
                let at = user_at.get(&user_seq).copied().unwrap_or(0);
                self.handle_unanswered(session_id, user_seq, at, attempts, report).await; true
            }
            Ok(_) => false,
            Err(c) => { report.refused.push((session_id.clone(), ResumeRefusal::LogInconsistent(c))); true }
        }
    }

    /// `Interrupted` minus the boundary repair: nothing dangled, so nothing is
    /// owed a receipt; recency is the message's own recording time; the plan
    /// is empty because no `RunStarted` ever froze an envelope — that is NOT
    /// `unsnapshotted` (the counter is about markers that exist).
    async fn handle_unanswered(&self, session_id: &SessionId, user_seq: EventSeq, user_at: i64, attempts: u32, report: &mut ResumeReport) {
        if user_at == 0 { report.skipped_unknown_age += 1; return; }
        let age_ms = now_ms().saturating_sub(user_at);
        if age_ms > (self.config.max_age_secs as i64).saturating_mul(1000) {
            self.abandon(session_id, "the unanswered message was too old to resume safely").await; report.abandoned += 1; return;
        }
        if attempts >= self.config.max_attempts {
            self.abandon(session_id, "it kept crashing before the run could start").await; report.abandoned += 1; return;
        }
        if let Err(e) = self.stamp_resume_attempt(session_id, user_seq, attempts + 1).await {
            report.refused.push((session_id.clone(), ResumeRefusal::IntentStampFailed(e.to_string()))); return;
        }
        match self.retrigger(session_id, &ResumePlan::default()).await {
            Ok(()) => report.resumed += 1,
            Err(refusal) => report.refused.push((session_id.clone(), refusal)),
        }
    }
}
```

`resume_from_markers` Clean arm: `Ok(RunDisposition::Clean) => { if !self.check_unanswered(session_id, markers, report).await { report.skipped += 1; } }`; add arm `Ok(RunDisposition::Unanswered { .. }) => { report.skipped += 1; }` with the comment "unreachable from a marker-only slice — `check_unanswered` is the reader that can see it; counted honestly if a caller ever hands a wider slice". `resume_interrupted_runs`: collect `seen: HashSet<SessionId>` from the marker groups; then

```rust
        let active_minutes = u32::try_from(self.config.max_age_secs.div_ceil(60).max(1)).unwrap_or(u32::MAX);
        match self.session_store.list_sessions(crate::gateway::session_store::types::SessionFilter { active_minutes: Some(active_minutes), ..Default::default() }).await {
            Ok(rows) => for meta in rows {
                let Some(id) = SessionId::from_key_string(&meta.key) else { continue };
                if seen.contains(&id) { continue; }
                let Some(_slot) = self.try_claim_resume(&id) else { report.busy += 1; continue; };
                if self.check_unanswered(&id, &[], &mut report).await { report.scanned += 1; }
            },
            Err(e) => tracing::warn!(error = %e, "resume: activity-window listing failed; marker-less unanswered sessions not scanned"),
        }
```

`resume_session` (`:887`): when the session has no marker group, call `check_unanswered(session_id, &[], &mut report)` under a claimed slot instead of returning the zero report. `abandon()` for an Unanswered writes the same `RunFinished { Abandoned }` closer (it reads as `FinishWithoutStart`, allowed by the new legal shape). `handle_interrupted`'s cap message is unchanged.

Faces: session_snapshot.rs both matches add `RunDisposition::Unanswered { attempts, .. } => (LastRunState::UNANSWERED, attempts)`; projection_reconciler.rs:149-151 add `Ok(RunDisposition::Unanswered { .. }) => {}` (a candidate). Protocol: `pub const UNANSWERED: &'static str = "unanswered";` + `Self::UNANSWERED => LastRunDisposition::Unanswered` + variant doc "`[`LastRunState::UNANSWERED`]` — the last user message reached the log and no run answered it". Panel `last_run_notice`: `D::Unanswered => Some(td_string!(locale, narration.last_run_unanswered).to_string())`; `run_badge`: `D::Unanswered => Some(RunBadge::Unanswered)`; `RunBadge::Unanswered => t_string!(i18n, chat.run_badge_unanswered).to_string()`. en.json: `"last_run_unanswered": "Your last message was not answered — the run stopped before it could start; recovery will retry it"`, `"run_badge_unanswered": "unanswered"`; zh.json: `"上一条消息没有得到回答 — 运行在开始前就中断了，恢复会重试"`, `"未回答"`. TUI `last_run_mark`: `LastRunDisposition::Unanswered => Some("  [unanswered]")`; `last_run_notice`: `LastRunDisposition::Unanswered => Some("上一条消息没有得到回答 — 运行在开始前就中断了，恢复会重试".to_string())`.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::reduction gateway::resume_coordinator gateway::session_snapshot gateway::projection_reconciler`; `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1`; `cargo test -p aleph-protocol`; `cargo test -p aleph-panel --lib`; `cargo test -p aleph-tui`; `just wasm`.
- [ ] **Step 5: Mutation check** — in `reduce_disposition` drop the `synthetic: false` guard ⇒ `a_seeded_message_with_no_run_started_is_unanswered` RED; in `resume_interrupted_runs` delete the activity-window loop ⇒ `an_unanswered_seed_is_stamped_and_retriggered_without_repair` RED and T10 `unanswered` RED.
- [ ] **Step 6: Commit** — `git add src/session/reduction.rs src/gateway/resume_coordinator.rs src/gateway/session_snapshot.rs src/gateway/projection_reconciler.rs shared/protocol/src/session_thread.rs interfaces/webchat/src/components/chat_sidebar.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json interfaces/tui/src/tui/commands.rs tests/resume_coordinator_integration.rs` — `session: reduce an unanswered seed as its own disposition and resume it` + Claude-Session line.

---

### Task 3: manual `/compact` and rewind/truncate become one batch each

**Files:**
- Modify: `src/context/compact/manual.rs:20-37` (doc), `:306-325` (signature: drop `store`), `:459-499` (batch), `:518-526` (log), tests `:900-1000` (`StoreBackedService` + `CountingStore`), `:1013-1063`
- Modify: `src/builtin_tools/sessions/compact_tool.rs:115-138` (drop the `store` lookup + argument)
- Modify: `src/session/marker_balance.rs` (whole file: builder + one-call API), `src/session/mod.rs:33` (re-export)
- Modify: `src/gateway/handlers/mod.rs:150-200` (`retire_events_and_balance` replaces `balance_run_markers_after_retire`), `src/gateway/handlers/chat.rs:815-831`, `src/gateway/handlers/session/db_handlers/modify.rs:736-753`, its test `:1089-1098` (+ `install_test_session_service()`)
- Modify: `src/session/events.rs:416-421` (`summary_ref` doc), `src/session/store.rs` (+ `#[cfg(test)] pub(crate) mod test_support { CountingStore }`)
- Test: `manual.rs`, `marker_balance.rs`, `modify.rs` test modules

**Interfaces:**
- Consumes: T2 `emit_batch`; `reduce_run` (`reduction.rs:361`), `RunReduction.open_run: Option<RunStartFacts>` (`:264`), `AgentRunManager::running_sessions` (`handlers/agent.rs:446`).
- Produces:
  ```rust
  // manual.rs
  pub async fn compact_session(service: &dyn SessionService, summarizer: Option<&ContextCompactor>, session_id: &SessionId, opts: &ManualCompactOptions) -> anyhow::Result<ManualCompactOutcome>
  // marker_balance.rs
  pub struct RetireOutcome { pub retired: usize, pub closed_run: Option<String> }
  pub fn open_run_after_retire(events: &[SessionEventRecord], from_seq: EventSeq) -> Result<Option<String>, SessionError>   // the batch BUILDER
  pub async fn retire_from_and_close_run(service: &dyn SessionService, session: &SessionId, from_seq: EventSeq, is_running: impl Fn(&SessionId) -> bool) -> Result<RetireOutcome, SessionError>
  // handlers/mod.rs
  pub(crate) async fn retire_events_and_balance(session_key: &SessionId, from_seq: EventSeq, run_manager: Option<&Arc<agent::AgentRunManager>>) -> Result<usize, SessionError>
  // store.rs test_support
  pub(crate) struct CountingStore { pub inner: Arc<SqliteEventStore>, pub append_batches: AtomicUsize, pub retire_froms: AtomicUsize, pub retire_throughs: AtomicUsize }  // impl SessionEventStore by delegation
  ```

- [ ] **Step 1: Write the failing tests**

`manual.rs` (replace `StoreBackedService.store` with `Arc<CountingStore>`; `emit_batch` allocates `first = *next_seq; *next_seq += events.len()`, calls `inner.append_batch`; `emit_event` = `emit_batch(vec![e], None)`; `seeded_session` returns `(StoreBackedService, Arc<CountingStore>, SessionId)`):
```rust
#[tokio::test]
async fn manual_compact_is_one_store_transaction() {
    let (service, store, sid) = seeded_session(40).await;
    let before = store.append_batches.load(Ordering::SeqCst);
    let out = compact_session(&service, None, &sid, &ManualCompactOptions::default()).await.unwrap();
    assert!(out.compacted);
    assert_eq!(store.append_batches.load(Ordering::SeqCst) - before, 1, "summary + checkpoint + retire = ONE append_batch");
    assert_eq!(store.retire_throughs.load(Ordering::SeqCst), 0, "no second step");
    let after = service.get_events(&sid, None, None).await.unwrap();
    let summary = after.iter().find(|r| matches!(&r.event, SessionEvent::SystemMessage { content, .. } if content.starts_with(SUMMARY_MARKER))).expect("summary live");
    let ckpt = after.iter().find(|r| matches!(r.event, SessionEvent::CompactionPerformed { .. })).expect("checkpoint live");
    assert_eq!(ckpt.seq, summary.seq + 1, "same batch ⇒ adjacent seqs");
    let (SessionEvent::SystemMessage { turn_id, .. }, SessionEvent::CompactionPerformed { summary_ref, to_seq, .. }) = (&summary.event, &ckpt.event) else { unreachable!() };
    assert_eq!(summary_ref, &turn_id.to_string(), "summary_ref names the summary by turn_id (known before the batch commits)");
    assert!(after.iter().all(|r| r.seq > *to_seq), "prefix retired in the same step");
}
```
Update `compaction_actually_shrinks_what_the_prompt_is_rebuilt_from` (:1036-1063): compare `checkpoint.1` with the summary's `turn_id.to_string()`, not its seq. Every `compact_session(&service, store.as_ref(), ..)` call drops the second argument.

`marker_balance.rs` (replace the four tests; `store()/run_started/run_finished/seed/disposition` helpers stay, `store()` now builds `Arc<CountingStore>`, `service()` wraps it in `InProcessActorSessionService`):
```rust
#[tokio::test]
async fn a_rewind_that_cuts_the_finish_closes_the_run_in_the_same_call() {
    let store = store();
    let svc = InProcessActorSessionService::new(store.clone());
    let sid: SessionId = SessionKey::ephemeral("balance-rewind");
    seed(&store, &sid, &[run_started(10), run_finished(20)]).await;
    let n = store.append_batches.load(Ordering::SeqCst);
    let out = retire_from_and_close_run(&svc, &sid, 2, |_| false).await.expect("rewind");
    assert_eq!(store.append_batches.load(Ordering::SeqCst) - n, 1, "retire + closer = ONE call");
    assert_eq!(store.retire_froms.load(Ordering::SeqCst), 0, "no window between retire and closer");
    assert_eq!((out.retired, out.closed_run.as_deref()), (1, Some("run-10")));
    assert_eq!(disposition(&store, &sid).await, RunDisposition::Clean);
    let live: Vec<_> = store.load_all_events(&sid).await.unwrap().into_iter().map(|r| r.seq).collect();
    assert_eq!(live, vec![1, 3], "the closer is live at seq 3; seq 2 is gone");
}
#[tokio::test] async fn a_running_session_is_retired_but_not_closed() { /* is_running = true ⇒ out.closed_run == None, still retired 1, still ONE append_batch (retire-only) */ }
#[tokio::test] async fn a_balanced_cut_appends_no_closer() { /* seed [RS, RF], retire from 3 ⇒ closed_run None, one append_batch, log len 2 */ }
#[tokio::test] fn open_run_after_retire_reduces_the_surviving_prefix() { /* events [RS(seq1), RF(seq2)]: from 2 ⇒ Some("run-10"); from 3 ⇒ None; from 1 ⇒ None */ }
```
`modify.rs:1089`: add `let _svc = crate::session::in_process::install_test_session_service();` after the store install; assertion unchanged (2 surviving).

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- context::compact::manual session::marker_balance gateway::handlers::session::db_handlers::modify::tests::truncate_retires` → `E0061 this function takes 4 arguments but 5 were supplied` / `E0425 cannot find function retire_from_and_close_run`.

- [ ] **Step 3: Minimal implementation**

`manual.rs:459-499` → 
```rust
    let summary_turn = uuid::Uuid::new_v4();
    let batch = vec![
        SessionEvent::SystemMessage { turn_id: summary_turn, content: format!("{SUMMARY_MARKER}\n{SUMMARY_FRAMING}\n\n{body}"), at: crate::session::events::now_ms() },
        SessionEvent::CompactionPerformed {
            from_seq,
            to_seq: cut_seq,
            // The summary's turn_id: the one reference known BEFORE the batch
            // commits (its seq is allocated by the actor inside the commit).
            summary_ref: summary_turn.to_string(),
            at: crate::session::events::now_ms(),
        },
    ];
    // Summary, checkpoint and retire commit together or not at all (§4.1).
    service.emit_batch(session_id, batch, Some(Retire::Through(cut_seq))).await.map_err(|e| {
        tracing::warn!(?session_id, error = %e, "manual compaction: transaction did not commit; context unchanged");
        anyhow::Error::from(e)
    })?;
```
Delete the `store` parameter and `retired` from the info line; module doc steps 4–5 become "4. One transaction: append the summary `SystemMessage` + `CompactionPerformed`, retire the prefix (`Retire::Through`)"; fn doc "Ordering is fail-safe by construction…" → "All-or-nothing: a crash leaves either the old log or the compacted one." `compact_tool.rs:115-116,134`: delete the store lookup and argument. `events.rs:419` doc: `/// Turn id of the summary SystemMessage written in the same batch (before 2026-09-12: its seq). No production reader resolves it; a future one must accept both.`

`marker_balance.rs` (replaces `close_open_run_after_retire`):
```rust
pub struct RetireOutcome { pub retired: usize, pub closed_run: Option<String> }

/// The BUILDER: which run would `Retire::From(from_seq)` leave open?
pub fn open_run_after_retire(events: &[SessionEventRecord], from_seq: EventSeq) -> Result<Option<String>, SessionError> {
    let surviving: Vec<SessionEventRecord> = events.iter().filter(|r| r.seq < from_seq).cloned().collect();
    let reduction = reduce_run(&surviving).map_err(|c| SessionError::Other(c.to_string()))?;
    Ok(reduction.open_run.map(|o| o.run_id))
}

/// Retire `>= from_seq` and, in the SAME transaction, close the run that retire
/// would leave open (`Cancelled`: the user cut it, recovery did not give up).
/// `is_running` must fail closed (doc unchanged from the old closer).
/// A log the reducer refuses is an `Err` BEFORE anything is retired.
pub async fn retire_from_and_close_run(service: &dyn SessionService, session: &SessionId, from_seq: EventSeq, is_running: impl Fn(&SessionId) -> bool) -> Result<RetireOutcome, SessionError> {
    let events = service.get_events(session, None, None).await?;
    let retired = events.iter().filter(|r| r.seq >= from_seq).count();
    let closed_run = if is_running(session) { None } else { open_run_after_retire(&events, from_seq)? };
    let closer = closed_run.iter().map(|run_id| SessionEvent::RunFinished { run_id: run_id.clone(), outcome: RunOutcome::Cancelled, at: now_ms() }).collect();
    service.emit_batch(session, closer, Some(Retire::From(from_seq))).await?;
    Ok(RetireOutcome { retired, closed_run })
}
```
`session/mod.rs:33` → `pub use marker_balance::{open_run_after_retire, retire_from_and_close_run, RetireOutcome};`.

`handlers/mod.rs` (replaces `balance_run_markers_after_retire`, doc kept minus "the retire has already committed"):
```rust
pub(crate) async fn retire_events_and_balance(session_key: &crate::session::service::SessionId, from_seq: crate::session::events::EventSeq, run_manager: Option<&Arc<agent::AgentRunManager>>) -> Result<usize, crate::session::service::SessionError> {
    let Some(service) = crate::session::service::global_session_service() else { return Ok(0); }; // no event log in this process (CLI one-shot, tests)
    let running: Vec<String> = run_manager.map(|rm| rm.running_sessions()).unwrap_or_default();
    let knows_runs = run_manager.is_some();
    let out = crate::session::marker_balance::retire_from_and_close_run(service.as_ref(), session_key, from_seq,
        |key| !knows_runs || running.iter().any(|k| k == &key.to_key_string())).await?;
    if let Some(run_id) = &out.closed_run { tracing::debug!(session = %session_key.to_key_string(), run_id, "retire closed the run it would have left open"); }
    Ok(out.retired)
}
```
`chat.rs:815-831` → `let retired = match super::retire_events_and_balance(&session_key, params.seq, run_manager.as_ref()).await { Ok(n) => n, Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to rewind session event log: {e}")) };` (delete the balance call). `modify.rs:740-753` → same shape (`"Failed to retire session event log: {e}"`). `retire_live_events` stays for `chat.clear` / `sessions.reset` / `sessions.delete`.

`store.rs`:
```rust
#[cfg(test)]
pub(crate) mod test_support {
    /// A real SQLite store that counts its write entry points, so a test can
    /// assert "one transaction" as a NUMBER instead of trusting the caller.
    pub(crate) struct CountingStore { pub inner: Arc<SqliteEventStore>, pub append_batches: AtomicUsize, pub retire_froms: AtomicUsize, pub retire_throughs: AtomicUsize }
    impl CountingStore { pub(crate) fn in_memory() -> Arc<Self> { /* open_in_memory + migrate + SqliteEventStore::new */ } }
    #[async_trait] impl SessionEventStore for CountingStore { /* append_batch/retire_from/retire_through: fetch_add then delegate; every other method: delegate */ }
}
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- context::compact::manual session::marker_balance gateway::handlers::session::db_handlers::modify builtin_tools::sessions::compact_tool`
- [ ] **Step 5: Mutation check** (§11 "4.1 retire out of the transaction", compact face) — replace the `emit_batch` in `compact_session` with two `emit_event`s + `store.retire_through` ⇒ `manual_compact_is_one_store_transaction` RED (`3 ≠ 1`). Restore.
- [ ] **Step 6: Commit** — `git add src/context/compact/manual.rs src/builtin_tools/sessions/compact_tool.rs src/session/marker_balance.rs src/session/mod.rs src/session/events.rs src/session/store.rs src/gateway/handlers/mod.rs src/gateway/handlers/chat.rs src/gateway/handlers/session/db_handlers/modify.rs` · `context: manual compact and rewind commit as one batch each` + Claude-Session line.

---

### Task 4: session-split as two batches, epoch after, boot heal arm

**Files:**
- Modify: `src/context/compact/session_split.rs:105-199` (order), `:25-33` (`SplitError` doc), tests `:355-413` (`RecordingSessionService` + `emit_batch`), `:419-441` (registrar shares a trace), `:505-612`
- Modify: `src/gateway/projection_reconciler.rs:56-78` (`ReconcileReport` + 2 fields), `:80-104` (5th ctor arg), `:108-138` (log line), `:142-225` (`heal_split_epochs` before the disposition loop), tests
- Modify: `src/bin/aleph-server/commands/start/mod.rs:386-395` (clone registrar before the move at :1774), `:2990-2995` (pass it), `:3008-3018` (log line)
- Test: `session_split.rs` and `projection_reconciler.rs` test modules

**Batch order (decided): parent `[RunFinished{Completed}]` FIRST, child `[SessionForked, SystemMessage, tail…, RunStarted]` SECOND, `register_epoch` THIRD.** Why: the heal arm's premise is "a `SessionForked` in the log ⇒ the parent side is done". Child-first would leave a crash window where BOTH parent and child read `Interrupted` and both get resumed (double execution), and the heal could not fix it without a boot redo (rejected, U2). Parent-first torn states: (i) after the parent batch only — parent `Clean`, no child, routing on parent: the next turn re-splits; no double work. (ii) after both batches, before epoch — log says split, routing says parent: the heal arm registers the epoch at boot, before the resume pass in the same detached task. In-process failure of the child batch returns `Err` and the caller (`directive.rs:196`) falls back to compact-to-fit on the parent; the run's own `RunFinished` then lands after the split closer (`FinishWithoutStart`, REPORT class — readable, counted).

**Interfaces:**
- Consumes: T2 `emit_batch`; `SessionEpochRegistrar` (`epoch_registrar.rs:14`); `SessionStore::get_current_epoch` (`session_store/mod.rs:410`); `SessionKey::{epoch, base_key_pattern}` (`session_key.rs:360,446`).
- Produces:
  ```rust
  pub struct ReconcileReport { .., pub epochs_healed: usize, pub epoch_heal_skipped: usize }
  ProjectionReconciler::new(event_store, session_store, projector, max_age_secs, epoch_registrar: Option<Arc<dyn SessionEpochRegistrar>>)
  async fn heal_split_epochs(&self, groups: &[(SessionId, Vec<SessionEventRecord>)], report: &mut ReconcileReport)   // private
  ```

- [ ] **Step 1: Write the failing tests**

`session_split.rs` (fakes share `trace: Arc<Mutex<Vec<String>>>`; `RecordingSessionService::emit_batch` pushes `format!("batch:{}:{}", id.to_key_string(), events.len())` and records `(id, Vec<SessionEvent>, Option<Retire>)`; `emit_event` delegates; `RecordingRegistrar::register_epoch` pushes `format!("register_epoch:{}", key.to_key_string())`):
```rust
#[tokio::test]
async fn split_commits_parent_then_child_then_registers_the_epoch() {
    let parent = SessionKey::Main { agent_id: "agent-a".into(), main_key: "main".into(), epoch: 0 };
    let child = parent.with_next_epoch();
    let events = vec![user_record(1, "pre-tail 1"), user_record(2, "pre-tail 2"), user_record(3, "fresh tail")];
    let trace = Arc::new(Mutex::new(vec![]));
    let session = RecordingSessionService::with_trace(trace.clone());
    let registrar = RecordingRegistrar::with_trace(trace.clone());
    let compactor = ContextCompactor::new(AlephArc::new(MockProvider::new("S")), CompactorConfig::default());
    perform_session_split(session.as_ref(), registrar.as_ref(), &compactor, &parent, &events, 2).await.unwrap();
    assert_eq!(*trace.lock().await, vec![
        format!("batch:{}:1", parent.to_key_string()),
        format!("batch:{}:4", child.to_key_string()),
        format!("register_epoch:{}", child.to_key_string()),
    ]);
    let batches = session.batches().await;
    assert!(matches!(batches[0].1.as_slice(), [SessionEvent::RunFinished { outcome: RunOutcome::Completed, .. }]));
    assert!(matches!(batches[1].1.as_slice(), [SessionEvent::SessionForked { .. }, SessionEvent::SystemMessage { .. }, SessionEvent::UserMessage { .. }, SessionEvent::RunStarted { .. }]));
    assert!(batches.iter().all(|(_, _, retire)| retire.is_none()));
}
```
(Rewrite `split_seeds_child_with_forked_summary_and_fresh_tail` :505-612 against `batches()`; the content assertions on `[Context Summary]`, the parent key string and "fresh tail message" stay.)

`projection_reconciler.rs`:
```rust
#[tokio::test]
async fn a_forked_child_the_routing_table_never_learned_is_registered_at_boot() {
    let event_store = own_event_store();
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(SessionManager::new(SessionManagerConfig { db_path: temp.path().join("heal.db"), ..Default::default() }).unwrap());
    let parent = SessionKey::Main { agent_id: "a".into(), main_key: "k".into(), epoch: 0 };
    manager.get_or_create(&parent).await.unwrap();
    let child = parent.with_next_epoch();
    let now = crate::session::events::now_ms();
    for (seq, ev) in [
        (1, SessionEvent::SessionForked { parent_session_id: parent.to_key_string(), at: now }),
        (2, SessionEvent::SystemMessage { turn_id: uuid::Uuid::new_v4(), content: "[Context Summary]".into(), at: now }),
        (3, SessionEvent::RunStarted { run_id: "split".into(), at: now, project_root: None, envelope: None }),
    ] { event_store.append(&child, seq, &ev, now).await.unwrap(); }
    let session_store: Arc<dyn SessionStore> = manager.clone();
    assert_eq!(session_store.get_current_epoch(&parent.base_key_pattern()).await.unwrap(), 0, "precondition: routing still says epoch 0");
    let reconciler = ProjectionReconciler::new(event_store.clone(), session_store.clone(),
        MessageProjector::with_event_store(session_store.clone(), None, Some(event_store.clone())), 86_400, Some(manager.clone() as Arc<dyn SessionEpochRegistrar>));
    let report = reconciler.reconcile_candidates().await;
    assert_eq!(report.epochs_healed, 1);
    assert_eq!(session_store.get_current_epoch(&parent.base_key_pattern()).await.unwrap(), 1, "routing now resolves to the child");
    assert_eq!(reconciler.reconcile_candidates().await.epochs_healed, 0, "idempotent");
}

#[tokio::test]
async fn without_a_registrar_the_heal_is_counted_not_faked() {
    // same log on the FILE backend (temp_file_store()), registrar None ⇒ epoch_heal_skipped == 1, epochs_healed == 0
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- context::compact::session_split gateway::projection_reconciler` → `E0061 this function takes 4 arguments but 5 were supplied` / `no field epochs_healed`; the order test panics on the trace vector (`register_epoch` first).

- [ ] **Step 3: Minimal implementation**

`session_split.rs:116-199` →
```rust
    // 3. Close the parent FIRST: a `SessionForked` on the child must imply the
    //    parent is finished — the boot heal (`heal_split_epochs`) relies on it.
    let at = crate::session::events::now_ms();
    let split_run_id = uuid::Uuid::new_v4().to_string();
    session.emit_batch(parent_session_id, vec![SessionEvent::RunFinished { run_id: split_run_id.clone(), outcome: RunOutcome::Completed, at }], None)
        .await.map_err(|e| SplitError::Failed(anyhow::anyhow!("emit parent RunFinished: {e}")))?;
    // 4. Seed the child in ONE transaction: fork marker, summary, verbatim tail, open run.
    let mut child_batch = Vec::with_capacity(events.len() - tail_start + 3);
    child_batch.push(SessionEvent::SessionForked { parent_session_id: parent_session_id.to_key_string(), at });
    child_batch.push(build_summary_event(summary_text));
    child_batch.extend(events[tail_start..].iter().map(|r| r.event.clone()));
    child_batch.push(SessionEvent::RunStarted { run_id: split_run_id, at, project_root: None, envelope: None });
    session.emit_batch(&child, child_batch, None).await.map_err(|e| SplitError::Failed(anyhow::anyhow!("seed child: {e}")))?;
    // 5. Routing LAST. From here the log is authoritative; a failed registration
    //    is healed at the next boot (log leads, cache follows), so it is not an Err
    //    — an Err would make the caller continue on a parent whose run is closed.
    match epoch_registrar.register_epoch(&child).await {
        Ok(()) => epoch_registrar.retire_superseded(parent_session_id).await,
        Err(e) => tracing::error!(parent = ?parent_session_id, child = ?child, error = %e,
            "session split committed but epoch registration failed; routing resolves to the parent until the boot heal"),
    }
    Ok(SplitOutcome { child_session_id: child })
```
`SplitError::Failed` doc → "Summarization or event emission failed (epoch registration is not fatal: see step 5)."

`projection_reconciler.rs`:
```rust
    async fn heal_split_epochs(&self, groups: &[(SessionId, Vec<SessionEventRecord>)], report: &mut ReconcileReport) {
        let horizon = crate::session::events::now_ms().saturating_sub(i64::try_from(self.max_age_secs.saturating_mul(1000)).unwrap_or(i64::MAX));
        for (id, markers) in groups {
            if id.epoch() == 0 || !markers.last().is_some_and(|m| m.created_at_ms >= horizon) { continue; }
            let current = match self.session_store.get_current_epoch(&id.base_key_pattern()).await {
                Ok(e) => e,
                Err(e) => { tracing::warn!(session = ?id, error = %e, "epoch heal: get_current_epoch failed"); report.errored += 1; continue; }
            };
            if current >= id.epoch() { continue; }
            let head = match self.event_store.load_events_range(id, Some(1), Some(2)).await {
                Ok(h) => h,
                Err(e) => { tracing::warn!(session = ?id, error = %e, "epoch heal: could not read seq 1"); report.errored += 1; continue; }
            };
            if !matches!(head.first().map(|r| &r.event), Some(SessionEvent::SessionForked { .. })) {
                tracing::warn!(session = ?id, current, "epoch heal: log at a higher epoch than routing, but seq 1 is not SessionForked; left alone");
                report.errored += 1;
                continue;
            }
            let Some(registrar) = &self.epoch_registrar else { report.epoch_heal_skipped += 1; continue; }; // file backend: no registrar
            match registrar.register_epoch(id).await {
                Ok(()) => { tracing::info!(session = ?id, from = current, "epoch heal: registered a forked child the routing table never learned"); report.epochs_healed += 1; }
                Err(e) => { tracing::warn!(session = ?id, error = %e, "epoch heal: register_epoch failed"); report.errored += 1; }
            }
        }
    }
```
In `candidates()`: bind `let groups = match self.event_store.load_run_markers().await {..}`; call `self.heal_split_epochs(&groups, report).await` before the disposition loop. Add `epochs_healed = report.epochs_healed, epoch_heal_skipped = report.epoch_heal_skipped` to the `tracing::info!` at :131 and `start/mod.rs:3010`. `start/mod.rs:391`: `let epoch_registrar_for_reconcile = epoch_registrar_for_orchestrator.clone();` immediately after the binding; pass it at :2994; fix the lying comment at :391 ("split degrades to FinalReply" → "split degrades to compact-to-fit, `directive.rs:207-226`").

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- context::compact::session_split gateway::projection_reconciler` then `cargo check -p alephcore --bins`
- [ ] **Step 5: Mutation check** — move `register_epoch` back above the parent batch ⇒ `split_commits_parent_then_child_then_registers_the_epoch` RED. Restore.
- [ ] **Step 6: Commit** — `git add src/context/compact/session_split.rs src/gateway/projection_reconciler.rs src/bin/aleph-server/commands/start/mod.rs` · `gateway: session split commits parent then child, epoch registered after and healed at boot` + Claude-Session line.

---

### Task 8: the L0 fast path takes the run's shape

**Files:**
- Modify: `src/session/events.rs:69-95` (add `SessionEvent::user_turn`); `src/orchestrator/harness_bridge/session_seed.rs:118-152` (`seed_history` uses it)
- Modify: `src/gateway/execution_engine/fast_path.rs` (journal struct; delete the two `emit_event` pairs `:43-90`, `:163-209`)
- Modify: `src/gateway/execution_engine/slash_command.rs:314-420` (`execute_direct_tool`), `:435-528` (`slash_gate_reason` → `Result<RunEnvelopeSnapshot, String>`)
- Test: `fast_path.rs` (new `#[cfg(test)] mod tests`), `slash_command.rs` tests (census)

**Interfaces:**
- Consumes: `SessionService::emit_batch` (A1); `crate::session::service::global_session_service()`; `RunEnvelopeSnapshot` (`events.rs:174`; B adds `allowed_tools`/`btw` — the fast path leaves both `None`, it never runs a `/<skill>`); `ExecTier::id()`, `SessionMode::id()`, `ThinkLevel::id()`, `MemoryMode::id()` (as `runner_impl.rs:1617-1631` uses them).
- Produces: `SessionEvent::user_turn(turn_id: TurnId, content: MessageContent, author_user_id: Option<String>) -> [SessionEvent; 2]`; `pub(super) struct FastPathJournal { svc: Arc<dyn SessionService>, session: SessionId, turn_id: TurnId, run_id: String, call_id: String }` with `open(&self, text, author, envelope, tool, input) -> Result<Vec<EventSeq>, SessionError>`, `close_ok(&self, result: Value, reply: String)`, `close_err(&self, error: String, reply: String)`.

Why the journal lives in `execute_direct_tool` and not the finalizers: the dispatch table `execute_slash_command_fast_path:195-306` has exactly ONE arm that executes anything (`"direct_tool"` → `execute_direct_tool`; `skill`/`mcp`/`custom` fall through, `_` fails before any effect), and the open batch must be written after the gate (`slash_gate_reason`) says the call may run — a gate that says Fallthrough hands the turn to the full loop, which seeds its own pair. The three pre-dispatch `Failed` arms (invalid mode JSON `:184`, missing `tool_id` `:322`, unknown mode type `:303`) are unreachable through `try_resolve_slash_command` / the channel router's `serialize_parsed_command` and after this task leave no transcript row (they still reach the user via `ResponseChunk` + `RunComplete`) — a *missing* row, never a wrong one (criterion #17).

- [ ] **Step 1: Write the failing tests** (fast_path.rs `mod tests`; real `InProcessActorSessionService` over an in-memory `SqliteEventStore`, as `execution_engine/tests.rs:1398-1402` does)

```rust
    fn svc() -> (Arc<dyn SessionService>, Arc<dyn SessionEventStore>) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        (Arc::new(InProcessActorSessionService::new(store.clone())), store)
    }

    #[tokio::test]
    async fn the_open_batch_is_durable_before_the_tool_runs_and_reads_interrupted_if_it_never_closes() {
        let (svc, store) = svc(); let sid = SessionKey::main("fp");
        let j = FastPathJournal::new(svc, sid.clone(), "select_model");
        let seqs = j.open("/model x".into(), None, RunEnvelopeSnapshot::default(), serde_json::json!({"model": "x"})).await.unwrap();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        let log = store.load_all_events(&sid).await.unwrap();
        let kinds: Vec<&str> = log.iter().map(|r| crate::session::store::event_type_tag(&r.event)).collect();
        assert_eq!(kinds, ["turn_started", "user_message", "run_started", "tool_call_requested"]);
        // <- the process dies here: the same shape the harness leaves.
        let r = reduce_run(&log).unwrap();
        assert_eq!(r.disposition, RunDisposition::Interrupted { attempts: 0 });
        assert_eq!(r.dangling.len(), 1);
        assert_eq!(r.dangling[0].tool_name, "select_model");
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::ThisRestart);
    }

    #[tokio::test]
    async fn a_closed_fast_path_reduces_clean_with_a_receipt_and_an_answer() {
        let (svc, store) = svc(); let sid = SessionKey::main("fp2");
        let j = FastPathJournal::new(svc, sid.clone(), "session_rename");
        j.open("/rename x".into(), None, RunEnvelopeSnapshot::default(), serde_json::json!({"topic": "x"})).await.unwrap();
        j.close_ok(serde_json::json!({"message": "renamed"}), "renamed".into()).await.unwrap();
        let log = store.load_all_events(&sid).await.unwrap();
        let kinds: Vec<&str> = log.iter().map(|r| crate::session::store::event_type_tag(&r.event)).collect();
        assert_eq!(&kinds[4..], ["tool_result", "assistant_message", "run_finished"]);
        let r = reduce_run(&log).unwrap();
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert!(r.dangling.is_empty() && r.contradictions.is_empty());
        assert_eq!(r.progress.tool_calls_answered, 1);
    }
```

(`event_type_tag` becomes `pub(crate)`.) `close_err` test: same with `close_err("boom".into(), "❌ boom".into())` ⇒ `tool_error`, `RunFinished { Errored }`, Clean. slash_command.rs census:

```rust
    /// §5.3: every L0 arm that executes a tool journals the dispatch first.
    /// Derived from the dispatch table — the ONE `execute_tool(` call in this
    /// file's production text — and equality on its count.
    #[test]
    fn every_fast_path_dispatch_is_journaled_before_it_runs() {
        use crate::utils::source_scan::{code_text, production_prefix};
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gateway/execution_engine/slash_command.rs")).unwrap().replace('\r', "");
        let code = code_text(&production_prefix(&src));
        let dispatch_sites = code.matches(".execute_tool(").count();
        assert_eq!(dispatch_sites, 1, "the fast path has one dispatch site; a second must journal too");
        let body = code.split("async fn execute_direct_tool").nth(1).expect("execute_direct_tool exists");
        let open = body.find("journal.open(").expect("the open batch is written in execute_direct_tool");
        let run = body.find(".execute_tool(").expect("dispatch is in execute_direct_tool");
        assert!(open < run, "the open batch must precede the dispatch");
        assert!(body.get(run..).is_some_and(|after| after.contains("journal.close_ok(") && after.contains("journal.close_err(")), "both closes follow the dispatch");
    }
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- gateway::execution_engine::fast_path gateway::execution_engine::slash_command::tests::every_fast_path_dispatch` → `cannot find type FastPathJournal` / `journal.open(` not found.

- [ ] **Step 3: Minimal implementation**

events.rs (next to `synthetic_user`):

```rust
    /// The seed pair every user-driven run opens with: `TurnStarted` then the
    /// `UserMessage`, sharing one `turn_id`. Two writers (the bridge's
    /// `seed_history` and the L0 fast path) used to spell it by hand.
    #[must_use]
    pub fn user_turn(turn_id: TurnId, content: MessageContent, author_user_id: Option<String>) -> [SessionEvent; 2] {
        let at = now_ms();
        [
            SessionEvent::TurnStarted { turn_id, trigger: TurnTrigger::UserMessage, at },
            SessionEvent::UserMessage { turn_id, content, at, synthetic: false, author_user_id },
        ]
    }
```

session_seed.rs `seed_history:118-152`: replace the two `emit_event` blocks with `for ev in SessionEvent::user_turn(turn_id, MessageContent { text: prompt, blocks: Vec::new(), thinking: None, thinking_signature: None }, crate::scope::ambient_room_author()) { service.emit_event(session_id, ev).await.map_err(|e| FlowError::Internal(format!("session seed: {e}")))?; }` (two emits as today — seed stays two `emit_event`s; A1's batch is not owed here).

fast_path.rs:

```rust
/// The L0 fast path's session journal — the same shape the harness leaves:
/// `[TurnStarted, UserMessage, RunStarted{envelope}, ToolCallRequested]`
/// BEFORE the tool runs (one Barrier batch), `[ToolResult|ToolError,
/// AssistantMessage, RunFinished]` after. A crash between the two reads
/// `Interrupted` with one dangling call, and the existing three-arm repair
/// tells the model the outcome is unknown (§5.3) — no second run shape.
pub(super) struct FastPathJournal {
    svc: Arc<dyn crate::session::service::SessionService>,
    session: SessionId,
    turn_id: TurnId,
    run_id: String,
    call_id: String,
    tool: String,
}

impl FastPathJournal {
    pub(super) fn new(svc: Arc<dyn SessionService>, session: SessionId, tool: &str) -> Self {
        Self { svc, session, turn_id: TurnId::new_v4(), run_id: format!("slash-{}", uuid::Uuid::new_v4()), call_id: format!("slash-{}", uuid::Uuid::new_v4()), tool: tool.to_string() }
    }

    pub(super) async fn open(&self, text: String, author_user_id: Option<String>, envelope: RunEnvelopeSnapshot, input: serde_json::Value) -> Result<Vec<EventSeq>, SessionError> {
        let [turn, user] = SessionEvent::user_turn(self.turn_id, MessageContent { text, blocks: Vec::new(), thinking: None, thinking_signature: None }, author_user_id);
        let events = vec![
            turn, user,
            SessionEvent::RunStarted { run_id: self.run_id.clone(), at: now_ms(), project_root: None, envelope: Some(envelope) },
            SessionEvent::ToolCallRequested { turn_id: self.turn_id, call_id: self.call_id.clone(), name: self.tool.clone(), input, at: now_ms() },
        ];
        self.svc.emit_batch(&self.session, events, None).await
    }

    pub(super) async fn close_ok(&self, result: serde_json::Value, reply: String) -> Result<Vec<EventSeq>, SessionError> {
        let receipt = SessionEvent::ToolResult { turn_id: self.turn_id, call_id: self.call_id.clone(), output: ToolOutput { value: result, metadata: Default::default() }, at: now_ms() };
        self.close(receipt, reply, RunOutcome::Completed).await
    }

    pub(super) async fn close_err(&self, error: String, reply: String) -> Result<Vec<EventSeq>, SessionError> {
        let receipt = SessionEvent::ToolError { turn_id: self.turn_id, call_id: self.call_id.clone(), error, at: now_ms() };
        self.close(receipt, reply, RunOutcome::Errored).await
    }

    async fn close(&self, receipt: SessionEvent, reply: String, outcome: RunOutcome) -> Result<Vec<EventSeq>, SessionError> {
        let events = vec![
            receipt,
            SessionEvent::AssistantMessage { turn_id: self.turn_id, content: MessageContent { text: reply, blocks: Vec::new(), thinking: None, thinking_signature: None }, usage: None, at: now_ms() },
            SessionEvent::RunFinished { run_id: self.run_id.clone(), outcome, at: now_ms() },
        ];
        self.svc.emit_batch(&self.session, events, None).await
    }
}
```

Delete the `if let Some(svc) = global_session_service() { … } else { warn!(…) }` blocks in both finalizers (`:46-90`, `:165-209`) — their two rows are now the journal's; keep everything else. Leave the `service.rs:67-80` count comment alone — T17 replaces it with a tree-pinned census that will list `slash_command.rs` (this task's new reader) instead of `fast_path.rs`.

slash_command.rs `slash_gate_reason` → `async fn slash_gate_reason(..) -> Result<RunEnvelopeSnapshot, String>`: bind the four resolvers (`let mode = self.resolve_turn_mode(request).await; let think = …; let memory = …;` — the census `every_stamp_on_carry_resolver_runs_on_the_fast_path` only requires the calls), every `return Some(reason)` becomes `return Err(reason)`, the final `None` becomes:

```rust
        Ok(RunEnvelopeSnapshot {
            exec_tier: Some(exec_tier.id().to_string()),
            session_mode: Some(mode.id().to_string()),
            think_level: think.map(|l| l.id().to_string()),
            memory_mode: Some(memory.id().to_string()),
            // No LLM call on this path: the run served on no model. `None`
            // is "the writer could not name the model", which is the truth.
            model: None, model_provider: None,
            ..RunEnvelopeSnapshot::default()   // B's allowed_tools / btw stay None here
        })
```

`execute_direct_tool:331-419`:

```rust
        let envelope = match self.slash_gate_reason(tool_id, &arguments, request, agent).await {
            Ok(env) => env,
            Err(reason) => return Err(ExecutionError::Fallthrough { reason }),
        };
        // §5.3: the fact lands before the effect. No handle (tests, the
        // simple engine) = no log to be durable into, the pre-existing
        // degraded shape; a handle whose write FAILS is a refusal — running
        // the tool over an unrecorded dispatch is the defect this closes.
        let journal = crate::session::service::global_session_service()
            .map(|svc| super::fast_path::FastPathJournal::new(svc, request.session_key.clone(), tool_id));
        if let Some(journal) = journal.as_ref() {
            journal.open(request.input.clone(), crate::scope::room_author_from_metadata(&request.metadata), envelope, arguments.clone()).await
                .map_err(|e| ExecutionError::Failed(format!("session log write failed before dispatch: {e}")))?;
        } else {
            warn!(session_key = %request.session_key.to_key_string(), "session/service capability absent; fast path runs unjournaled — see `aleph doctor`");
        }
        // … existing Reasoning emit + fs_scope + execution.await …
        match execution.await {
            Ok(result) => {
                // … existing harvest + extract …
                if let Some(journal) = journal.as_ref() {
                    if let Err(e) = journal.close_ok(result.clone(), response.clone()).await { warn!(error = %e, "fast path: close batch failed; the dispatch stays open for the boot repair"); }
                }
                // … existing ResponseChunk emit … Ok(response)
            }
            Err(e) => {
                let msg = format!("Tool '{tool_id}' execution failed: {e}");
                if let Some(journal) = journal.as_ref() {
                    if let Err(e2) = journal.close_err(msg.clone(), format!("❌ {msg}")).await { warn!(error = %e2, "fast path: close batch failed"); }
                }
                Err(ExecutionError::Failed(msg))
            }
        }
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- gateway::execution_engine::fast_path gateway::execution_engine::slash_command orchestrator::harness_bridge session::events`; `cargo test -p alephcore --features test-helpers --test 'gateway_chat_*' -j 1` (slash-command fixtures under `tests/gateway_chat_common`).
- [ ] **Step 5: Mutation check** — swap `journal.open(...)` to after `execution.await` ⇒ `every_fast_path_dispatch_is_journaled_before_it_runs` RED (`open < run` fails).
- [ ] **Step 6: Commit** — `git add src/session/events.rs src/orchestrator/harness_bridge/session_seed.rs src/gateway/execution_engine/fast_path.rs src/gateway/execution_engine/slash_command.rs src/session/service.rs` — `gateway: journal the slash fast path as a run before the tool runs` + Claude-Session line.

---

### Task 9: hook stop writes a receipt instead of erasing the turn

**Files:**
- Modify: `src/session/events.rs:44-49` (`ErrorKind` gains `HookStop` — T1 leaves `ErrorKind` untouched, so this task adds it; any exhaustive `match` on `ErrorKind` that fails to compile afterwards is a renderer to update — `session/projection.rs::project_row` is the known one)
- Modify: `src/gateway/execution_engine/run_loop/mod.rs:483-526` (the `BeforeAgentStart` match) — NOTE: the prompt's path `src/orchestrator/harness_bridge/run_loop/mod.rs` does not exist; the site is under `execution_engine`
- Modify: `src/session/projection.rs:94-99` (`project_row` `Error` arm), tests `:266-279`
- Test: `src/gateway/execution_engine/run_loop/mod.rs` (new `#[cfg(test)] mod hook_stop_tests` — the directory's `tests.rs` is the sibling module for integration-shaped tests; a pure builder test belongs next to the builder), `projection.rs` tests

**Interfaces:**
- Consumes: `SessionService::emit_batch` (A1); `RunRequest::is_resume()` (`execution_engine/mod.rs:409`); `SessionEvent::user_turn` (T8); `crate::scope::room_author_from_metadata` (`scope/mod.rs:222`).
- Produces: `ErrorKind::HookStop` (`src/session/events.rs`); `pub(super) fn hook_stop_receipt(request: &RunRequest, outcome: RunOutcome, text: &str) -> Vec<SessionEvent>`; `pub(super) async fn journal_hook_stop(request: &RunRequest, outcome: RunOutcome, text: &str)`.

- [ ] **Step 1: Write the failing tests**

run_loop/mod.rs:

```rust
#[cfg(test)]
mod hook_stop_tests {
    use super::*;
    use crate::session::events::{RunOutcome, SessionEvent, SessionEventRecord};
    use crate::session::reduction::{reduce_run, RunDisposition};

    fn request(input: &str, resume: bool) -> RunRequest {
        let mut r = RunRequest::new_for_test(input); // exists? — if not: build the literal as `resume_coordinator.rs:1234-1248` does
        if resume { r.metadata.insert("resume".into(), "true".into()); }
        r
    }
    fn seq_log(events: Vec<SessionEvent>) -> Vec<SessionEventRecord> {
        events.into_iter().enumerate().map(|(i, e)| SessionEventRecord { seq: i as u64 + 1, event: e, created_at_ms: (i as i64 + 1) * 10 }).collect()
    }

    /// §5.4: the log says "a run happened and the hook stopped it" — five
    /// events, reducing Clean, so §5.2 does not read it as an unanswered
    /// message and retrigger it three times.
    #[test]
    fn a_stopping_hook_leaves_five_events_that_reduce_clean() {
        let evs = hook_stop_receipt(&request("hi", false), RunOutcome::Cancelled, "halted by policy");
        let kinds: Vec<&str> = evs.iter().map(crate::session::store::event_type_tag).collect();
        assert_eq!(kinds, ["turn_started", "user_message", "run_started", "error", "run_finished"]);
        assert!(matches!(&evs[3], SessionEvent::Error { kind: crate::session::events::ErrorKind::HookStop, message, .. } if message == "halted by policy"));
        let r = reduce_run(&seq_log(evs)).expect("legal");
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert!(r.dangling.is_empty() && r.contradictions.is_empty());
    }

    /// A resumed run already holds its user message; the receipt must not
    /// seed a second, empty one.
    #[test]
    fn a_hook_stopped_resume_writes_no_seed_pair() {
        let evs = hook_stop_receipt(&request("", true), RunOutcome::Errored, "denied");
        let kinds: Vec<&str> = evs.iter().map(crate::session::store::event_type_tag).collect();
        assert_eq!(kinds, ["run_started", "error", "run_finished"]);
    }

    /// Both stop arms owe the receipt — count the `return`s inside the
    /// BeforeAgentStart match against the journal calls in the same block.
    #[test]
    fn both_before_agent_start_exits_journal_the_stop() {
        use crate::utils::source_scan::{code_text, production_prefix};
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gateway/execution_engine/run_loop/mod.rs")).unwrap().replace('\r', "");
        let code = code_text(&production_prefix(&src));
        let block = code.split("HookEvent::BeforeAgentStart, ctx)").nth(1).and_then(|rest| rest.split("Ok(_) => {}").next()).expect("the BeforeAgentStart match");
        assert_eq!(block.matches("return ").count(), 2, "two stop exits");
        assert_eq!(block.matches("journal_hook_stop(").count(), 2, "each exit journals before it returns");
    }
}
```

projection.rs: `project_row_labels_a_hook_stop_receipt` — `Error { kind: ErrorKind::HookStop, message: "halted" }` ⇒ `row.role == "system"`, `row.text == "Stopped by hook: halted"`.

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- gateway::execution_engine::run_loop::hook_stop_tests session::projection` → `cannot find function hook_stop_receipt`; `project_row` fails to compile (`ErrorKind::HookStop` not covered — that non-exhaustive error IS the renderer census).

- [ ] **Step 3: Minimal implementation**

events.rs (`ErrorKind`, after `Guardrail`):
```rust
    /// A `BeforeAgentStart` hook refused or stopped the run before it started
    /// (§5.4). The receipt closes the run in the same batch, so the log says
    /// "a run happened and the hook stopped it" — never `Unanswered`.
    HookStop,
```

run_loop/mod.rs, above `impl … ExecutionEngine`:

```rust
/// §5.4: what a `BeforeAgentStart` stop leaves on the log. The seed pair is
/// written here because the bridge (`runner_impl.rs:342`) never runs; a
/// resume already holds its message (`is_resume`) and gets only the run
/// bracket. `Error { HookStop }` reuses the guardrail receipt's projection
/// row (`session::projection::project_row`) and is NOT prompt-bearing.
pub(super) fn hook_stop_receipt(request: &RunRequest, outcome: RunOutcome, text: &str) -> Vec<SessionEvent> {
    let turn_id = TurnId::new_v4();
    let mut events = Vec::with_capacity(5);
    if !request.is_resume() {
        events.extend(SessionEvent::user_turn(turn_id, MessageContent { text: request.input.clone(), blocks: Vec::new(), thinking: None, thinking_signature: None }, crate::scope::room_author_from_metadata(&request.metadata)));
    }
    events.push(SessionEvent::RunStarted { run_id: format!("hookstop-{}", uuid::Uuid::new_v4()), at: now_ms(), project_root: request.workspace_override.as_ref().map(|p| p.display().to_string()), envelope: None });
    events.push(SessionEvent::Error { turn_id: Some(turn_id), kind: ErrorKind::HookStop, message: text.to_string(), recoverable: false, at: now_ms() });
    events.push(SessionEvent::RunFinished { run_id: events.iter().find_map(|e| match e { SessionEvent::RunStarted { run_id, .. } => Some(run_id.clone()), _ => None }).unwrap_or_default(), outcome, at: now_ms() });
    events
}

/// One batch, best-effort: a stop that could not be journaled still stops
/// the run (the user already saw the text); the warn is the trace.
pub(super) async fn journal_hook_stop(request: &RunRequest, outcome: RunOutcome, text: &str) {
    let Some(svc) = crate::session::service::global_session_service() else {
        warn!(session_key = %request.session_key.to_key_string(), "session/service capability absent; hook stop not journaled"); return;
    };
    if let Err(e) = svc.emit_batch(&request.session_key, hook_stop_receipt(request, outcome, text), None).await {
        warn!(error = %e, "hook stop receipt append failed");
    }
}
```

In the match (`:491-522`): deny arm — before `return Err(…)` insert `journal_hook_stop(request, RunOutcome::Errored, &reason).await;`; prevent_continuation arm — before `return Ok(stop_msg)` insert `journal_hook_stop(request, RunOutcome::Cancelled, &stop_msg).await;`. (`envelope: None` is the honest value: this writer resolved no knobs; the run is closed in the same batch so nothing ever replays it.) projection.rs:96: `ErrorKind::HookStop => "Stopped by hook",`.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- gateway::execution_engine::run_loop session::projection`.
- [ ] **Step 5: Mutation check** — omit `journal_hook_stop` from the deny arm ⇒ `both_before_agent_start_exits_journal_the_stop` RED. (T10 `unanswered` variant 2 stays SKIP until T14; the hook receipt is observed by `ratchet`'s log shape only indirectly — see T10.)
- [ ] **Step 6: Commit** — `git add src/gateway/execution_engine/run_loop/mod.rs src/session/projection.rs` — `gateway: journal a BeforeAgentStart stop as a closed run with a HookStop receipt` + Claude-Session line.

---

### Task 11: `ToolCallParked` — the park is a fact before the park

**Files:**
- Modify: `src/session/events.rs:44-49` (add `ParkReason` after `ErrorKind`), `:384-390` (variant after `ToolCallDenied`)
- Modify: `src/session/store.rs:657-675`, `:695-712`; `src/agents/subagent_spawner/fork.rs:238-243`
- Create: `src/session/call_log.rs` (+ `pub mod call_log;` in `src/session/mod.rs`)
- Modify: `src/tools/scoped/dispatch.rs:955-962`, `:1108-1115` (doc), `:1159-1200`
- Modify: `src/clarification/ask.rs:376-381`
- Modify: `src/session/reduction.rs:54-135`, `:220-234`, `:335-345`, `:450-460`, `:502-509`; tests `:803-850`, `:1249`
- Modify: `src/session/boundary_repair.rs:87-121`, `:138-157`; tests `:340-375`
- Modify: `src/agents/subagent_tool/recovery.rs:1507` (test fixture gains `parked: None`)
- Modify: `src/session/events.rs` `durability_of` (T1 — add `| SessionEvent::ToolCallParked { .. }` to the `Durability::Normal` group; the match has no wildcard on purpose) and `fixtures::sample_of_every_kind` (T1 — add `("ToolCallParked", SessionEvent::ToolCallParked { turn_id: TurnId::new_v4(), call_id: "c".into(), reason: ParkReason::Approval })`; T1's census `durability_barrier_set_is_exactly_the_four_ruled_events` and T6's `marker_event_types_are_exactly_the_reducers_marker_set` both read this list)
- Modify: `qa/resume_boundary/drive_r2.mjs:805` — the r2 `knobs` stage's dangle parks at the `ask` gate, so after this task its repair text is the FOURTH arm and the `"OUTCOME UNKNOWN"` probe goes red. The probe change lands HERE (same commit), not in T13: `const REPAIR_MARKERS = ["OUTCOME UNKNOWN", "NOT EXECUTED"]; const carriesRepair = (text) => REPAIR_MARKERS.some((m) => text.includes(m));` and at `:805` replace `userText(r.body).includes("OUTCOME UNKNOWN")` with `carriesRepair(userText(r.body))`. T13's `parked` stage reuses `REPAIR_MARKERS`.
- Test: `src/tools/scoped/tests.rs` (the crate's sibling tests module for `scoped`); `tests/parked_gate_integration.rs` (new — the global session service is a process-wide slot, only an integration binary may install it)

**Interfaces:**
- Consumes: `SessionService::emit_event(&self, id: &SessionId, event: SessionEvent) -> Result<EventSeq, SessionError>` (`service.rs:49`), `global_session_service()` (`:119`), `approval::current_call_identity() -> Option<CallIdentity>` (`approval/tool_call.rs:55`), `GateRule::HookRequested.id()` (`gate_chain.rs:181`), `ApprovalAction.rule_id: Option<&'static str>` (`sandbox/exec_approval/action.rs:98`), `TurnContext.session_key: SessionKey` (`turn_context.rs:23`).
- Produces: `pub enum ParkReason { Approval, Clarification, PreHook }` — `pub const fn as_str(self) -> &'static str` (= serde word) + `impl Display` (= clause); `SessionEvent::ToolCallParked { turn_id: TurnId, call_id: String, reason: ParkReason }` (**no `tool_name` — Corrections #1**); `pub async fn session::call_log::emit_for_ambient_call(session: &SessionId, tool: &str, what: &'static str, make: impl FnOnce(TurnId, String) -> SessionEvent) -> bool`; `DanglingCall { .., parked: Option<ParkReason> }`; `LogContradiction::ParkedWithoutRequest { seq: EventSeq, call_id: String }` (REPORT, tag `session-log-parked-without-request`); `boundary_repair_text(tool: &str, provenance: DanglingProvenance, denied: bool, parked: Option<ParkReason>, degrade: Option<&DegradeNote>) -> String`. `durability_of(ToolCallParked) == Normal` (A1's default arm; U3).

- [ ] **Step 1: Write the failing tests**

`reduction.rs` tests (fixtures `rec`/`requested`/`denied`/`started`/`result_for`/`reduced`/`tags` at :621-790):

```rust
    fn parked(call: &str, reason: ParkReason) -> SessionEvent {
        SessionEvent::ToolCallParked { turn_id: TurnId::new_v4(), call_id: call.to_string(), reason }
    }
    fn approved(call: &str) -> SessionEvent {
        SessionEvent::ToolCallApproved { turn_id: TurnId::new_v4(), call_id: call.to_string(),
            by: crate::session::events::ApprovalSource::User, at: 4 }
    }
    #[test]
    fn a_parked_dangling_call_carries_its_reason() {
        let r = reduced(&[rec(1, started("a")), rec(2, requested("c1")), rec(3, parked("c1", ParkReason::Approval))]);
        assert_eq!(r.dangling[0].parked, Some(ParkReason::Approval));
        assert!(!r.dangling[0].denied && tags(&r).is_empty());
    }
    #[test]
    fn an_answered_gate_ends_the_park() {
        let r = reduced(&[rec(1, started("a")), rec(2, requested("c1")),
            rec(3, parked("c1", ParkReason::Approval)), rec(4, approved("c1"))]);
        assert_eq!(r.dangling[0].parked, None, "approved ⇒ it went on to run ⇒ unknown, not never-ran");
        let r = reduced(&[rec(1, started("a")), rec(2, requested("c1")),
            rec(3, parked("c1", ParkReason::PreHook)), rec(4, denied("c1"))]);
        assert!(r.dangling[0].parked.is_none() && r.dangling[0].denied);
    }
    #[test]
    fn a_park_pairs_with_the_nearest_unanswered_dispatch_of_its_id() {
        let r = reduced(&[rec(1, started("a")), rec(2, requested("c1")), rec(3, result_for("c1")),
            rec(4, requested("c1")), rec(5, parked("c1", ParkReason::Clarification))]);
        assert_eq!((r.dangling.len(), r.dangling[0].seq, r.dangling[0].parked), (1, 4, Some(ParkReason::Clarification)));
    }
    #[test]
    fn a_park_without_a_dispatch_is_reported_and_ignored() {
        let r = reduced(&[rec(1, started("a")), rec(2, parked("ghost", ParkReason::Approval))]);
        assert!(r.dangling.is_empty());
        assert_eq!(tags(&r), vec!["session-log-parked-without-request"]);
    }
```
`kind_index`: add `LogContradiction::ParkedWithoutRequest { .. } => 10` (T6 took 9 for `ResumeWithoutTarget`), `KIND_COUNT = 11`, add `ParkedWithoutRequest { seq: 1, call_id: "c".into() }` to `one_of_each` (the exhaustive match refuses to compile otherwise). Add a `LegalShape { name: "approval-parked call, approved, then result", events: seq_log(vec![turn_started(), user("hi"), started("r1"), requested("c1"), parked("c1", ParkReason::Approval), approved("c1"), result_for("c1"), assistant("done"), finished("r1"), run_meta("r1")]), allowed: &[] }`.

`boundary_repair.rs` — rename `the_three_arms…` → `the_four_arms_are_four_different_sentences` and append:
```rust
        let parked = boundary_repair_text("bash_exec", DanglingProvenance::ThisRestart, false, Some(ParkReason::Approval), None);
        assert_shared_points(&parked, "bash_exec");
        assert!(parked.contains("never ran") && parked.contains("operator approval"), "{parked}");
        assert!(!parked.contains("may have completed") && parked.contains("no file writes"), "{parked}");
        assert!(parked != denied && parked != restart && parked != earlier);
        for (reason, clause) in [(ParkReason::Approval, "operator approval"),
            (ParkReason::Clarification, "the answer to your question"), (ParkReason::PreHook, "a pre-tool hook")] {
            assert!(boundary_repair_text("t", DanglingProvenance::EarlierRun, false, Some(reason), None).contains(clause));
        }
```
plus `a_parked_dangling_call_is_answered_with_the_parked_arm` (log `run_started(10)`, `tool_requested("c1")`, `ToolCallParked{c1, Approval}` → `repairs_for` gives one `ToolError` containing `"never ran"`, not `"OUTCOME UNKNOWN"`). Existing `boundary_repair_text(.., false, None)` calls gain a `None` before `degrade`.

`events.rs`:
```rust
    #[test]
    fn park_reason_wire_word_is_the_serde_name_and_display_is_the_clause() {
        for r in [ParkReason::Approval, ParkReason::Clarification, ParkReason::PreHook] {
            assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(r.as_str()));
            assert_ne!(r.as_str(), r.to_string());
        }
        let ev = SessionEvent::ToolCallParked { turn_id: TurnId::new_v4(), call_id: "c".into(), reason: ParkReason::PreHook };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"tool_call_parked\"") && json.contains("\"reason\":\"pre_hook\""), "{json}");
        let _: SessionEvent = serde_json::from_str(&json).unwrap();
    }
```

`src/tools/scoped/tests.rs` — §6.2 census (equality, derived from the dispatch sites; `walk` as in `slash_skill_scope.rs:430-442`):
```rust
/// A file that DEFINES `execute_with_cancel` forwards or implements it (trait
/// default, scoped service, allowlist/MCP-scope decorators); ORIGINATORS only
/// call it. Today that set is the harness Act phase alone, each call inside a
/// `with_call_identity(..)` scope — which is what makes the gate's no-identity
/// branch a `debug_assert!` instead of a silent return (spec §6.2).
#[test]
fn every_production_dispatch_into_the_scoped_gate_is_scoped_by_a_call_identity() {
    use crate::utils::source_scan::{code_text, production_text};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut originators: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for file in files {
        let rel = file.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap_or(&file).to_string_lossy().replace('\\', "/");
        let Ok(src) = std::fs::read_to_string(&file) else { continue };
        let code = code_text(&production_text(&file, &src));
        if code.contains("fn execute_with_cancel(") { continue; }
        let calls = code.matches(".execute_with_cancel(").count();
        if calls > 0 { originators.insert(rel, (calls, code.matches("with_call_identity(").count())); }
    }
    assert_eq!(originators.keys().collect::<Vec<_>>(), vec!["src/harness/agent/act.rs"],
        "a new originator must scope a CallIdentity around its dispatch, or not reach the gate: {originators:?}");
    let (calls, scoped) = originators["src/harness/agent/act.rs"];
    assert!(calls >= 1, "self-protection: the scan found the Act phase");
    assert_eq!(calls, scoped, "every dispatch in act.rs is wrapped by exactly one identity scope");
}
```

`tests/parked_gate_integration.rs` — `NeedsConfirmation` + `chat_tier_turn` copied verbatim from `tests/agent_ledger_gate_refusals.rs:31-88`; session service as `tests/cancellation_chain.rs:42-47`:
```rust
//! §6.1 effect test: the park row is durable BEFORE anyone is asked. In
//! `tests/` because `set_global_session_service` is process-wide (service.rs:98).
struct NeverAnswers { asked: Arc<tokio::sync::Notify> }
#[async_trait::async_trait]
impl ApprovalRequester for NeverAnswers {
    async fn request_approval(&self, _a: &ApprovalAction) -> ApprovalResponse {
        self.asked.notify_one();
        std::future::pending().await
    }
}
#[tokio::test]
async fn a_gate_park_is_in_the_log_before_the_requester_is_reached() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let sessions: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
    set_global_session_service(sessions.clone());
    let turn = chat_tier_turn("parked-gate");
    let session = turn.session_key.clone();
    sessions.attach(session.clone()).await.unwrap();
    let asked = Arc::new(tokio::sync::Notify::new());
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(NeedsConfirmation));
    let tools = Arc::new(ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(turn).with_confirmation(Arc::new(NeverAnswers { asked: asked.clone() })));
    let identity = CallIdentity { turn_id: TurnId::new_v4(), call_id: "toolu_parked".into() };
    let run = tokio::spawn(with_call_identity(Some(identity), async move { tools.execute("needs_confirmation", json!({})).await }));
    asked.notified().await; // the gate reached the requester: the park is happening NOW
    let rows = sessions.get_events(&session, None, None).await.unwrap();
    let parks = rows.iter().filter(|r| matches!(&r.event,
        SessionEvent::ToolCallParked { call_id, reason: ParkReason::Approval, .. } if call_id == "toolu_parked")).count();
    assert_eq!(parks, 1, "one park row, landed before the card was raised");
    run.abort();
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::reduction session::boundary_repair session::events tools::scoped::tests::every_production` → `no variant named ToolCallParked` / `cannot find type ParkReason`; `cargo test -p alephcore --features test-helpers --test parked_gate_integration -j 1` → same.

- [ ] **Step 3: Minimal implementation**

`events.rs` after `ErrorKind`:
```rust
/// Why a dispatched call stopped at a gate instead of running. Serde name = the
/// wire word on `DanglingCallView::parked`; `Display` = the clause the repair
/// text reads to the model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    /// A confirmation / operator card (`confirm_with_memory`).
    Approval,
    /// `ask_user` waiting for the person's answer.
    Clarification,
    /// A card a `BeforeToolCall` hook's `Ask` raised. NOT "the hook script was
    /// running" — a crash there stays OUTCOME UNKNOWN (there is no release fact).
    PreHook,
}
impl ParkReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self { Self::Approval => "approval", Self::Clarification => "clarification", Self::PreHook => "pre_hook" }
    }
}
impl std::fmt::Display for ParkReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Approval => "operator approval",
            Self::Clarification => "the answer to your question",
            Self::PreHook => "a pre-tool hook",
        })
    }
}
```
Variant after `ToolCallDenied`:
```rust
    /// Written by the gate BEFORE it parks (§6.1). Normal durability (U3): lost
    /// = "outcome unknown", the safe direction. The tool name lives on the
    /// dispatch this pairs with, not here.
    ToolCallParked { turn_id: TurnId, call_id: String, reason: ParkReason },
```
`store.rs:657`: `| SessionEvent::ToolCallParked { turn_id, .. }` in the `Some(*turn_id)` group; `:695`: `SessionEvent::ToolCallParked { .. } => "tool_call_parked",`. `fork.rs:238`: `| SessionEvent::ToolCallParked { .. }` in the `false` group. `events.rs` `durability_of`: `| SessionEvent::ToolCallParked { .. }` in the `Normal` group (U3; lost = "outcome unknown", the safe direction) and the `fixtures::sample_of_every_kind` entry named in Files. `qa/resume_boundary/drive_r2.mjs`: the `REPAIR_MARKERS` / `carriesRepair` change named in Files.

`src/session/call_log.rs`:
```rust
//! The one writer for "a session-log row about the call being dispatched right
//! now" — the approval decision and the park both ride it.
use std::sync::atomic::{AtomicU64, Ordering};
use crate::approval::{current_call_identity, CallIdentity};
use crate::session::events::{SessionEvent, TurnId};
use crate::session::service::{global_session_service, SessionId};

/// Reported on the warning line below, which is the count's reader.
static DROPPED_FOR_MISSING_IDENTITY: AtomicU64 = AtomicU64::new(0);

/// Emit `make(turn_id, call_id)` for the ambient call; `false` = not written (why is logged).
pub async fn emit_for_ambient_call(
    session: &SessionId, tool: &str, what: &'static str,
    make: impl FnOnce(TurnId, String) -> SessionEvent,
) -> bool {
    let Some(svc) = global_session_service() else {
        tracing::warn!(tool = %tool, session = %session, what, "session/service capability absent; not persisted — see `aleph doctor`");
        return false;
    };
    let Some(CallIdentity { turn_id, call_id }) = current_call_identity() else {
        let dropped = DROPPED_FOR_MISSING_IDENTITY.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(tool = %tool, what, dropped_so_far = dropped, "no ambient call identity at the approval gate; not persisted");
        debug_assert!(false, "approval gate reached without a CallIdentity (tool `{tool}`, {what}); \
            every production dispatch is scoped by the harness Act phase — see the census in tools/scoped/tests.rs");
        return false;
    };
    match svc.emit_event(session, make(turn_id, call_id)).await {
        Ok(_) => true,
        Err(e) => { tracing::warn!(tool = %tool, what, error = ?e, "failed to persist to the session log"); false }
    }
}
```

`dispatch.rs`: replace `:1159-1200` (from `let Some(session_svc) = …` to the fn's end) with
```rust
        crate::session::call_log::emit_for_ambient_call(&turn.session_key, name, "approval decision",
            |turn_id, call_id| match decision.denial_reason() {
                Some(reason) => SessionEvent::ToolCallDenied { turn_id, call_id, reason: reason.to_string(), at: now_ms() },
                None => SessionEvent::ToolCallApproved { turn_id, call_id, by: decision.approval_source(), at: now_ms() },
            }).await;
```
add beside it
```rust
    /// §6.1 — the park is a fact BEFORE the park; same anchor as the decision.
    async fn record_parked(&self, name: &str, reason: crate::session::events::ParkReason) {
        let Some(turn) = self.turn_context.as_ref() else { return };
        crate::session::call_log::emit_for_ambient_call(&turn.session_key, name, "park",
            |turn_id, call_id| crate::session::events::SessionEvent::ToolCallParked { turn_id, call_id, reason }).await;
    }
```
and in `confirm_with_memory` right before `let asked_at = std::time::Instant::now();` (:961):
```rust
        // Which gate raised the card is on the action (`gated_by`): a hook's
        // card reads "pre-tool hook", every other card "operator approval".
        let reason = if action.rule_id == Some(super::gate_chain::GateRule::HookRequested.id()) {
            crate::session::events::ParkReason::PreHook
        } else {
            crate::session::events::ParkReason::Approval
        };
        self.record_parked(name, reason).await;
```
Rewrite the doc at `:1108-1115`: the `None` case is not "direct `tools.invoke`" (it bypasses this service) — it is "a dispatch nobody scoped, which `call_log` reports".

`ask.rs`, after the `info!(… "ask: question delivered — awaiting reply")` (:376):
```rust
    // §6.1: written only once delivery is proven — an undeliverable question
    // fails fast above and never parks.
    crate::session::call_log::emit_for_ambient_call(&turn.session_key, "ask_user", "clarification park",
        |turn_id, call_id| crate::session::events::SessionEvent::ToolCallParked {
            turn_id, call_id, reason: crate::session::events::ParkReason::Clarification }).await;
```

`reduction.rs`: `Dispatch` gains `parked: Option<ParkReason>` (init `None` at :450); `DanglingCall` gains
```rust
    /// Parked at a gate when the log ends and nothing answered the gate: it
    /// never ran. Cleared by a later `ToolCallApproved` / `ToolCallDenied`.
    pub parked: Option<ParkReason>,
```
the Denied arm (:453) also sets `d.parked = None;`; new arms:
```rust
            SessionEvent::ToolCallParked { call_id, reason, .. } => {
                match dispatches.iter_mut().rev().find(|d| d.call_id == call_id && d.answered.is_none()) {
                    Some(d) => d.parked = Some(*reason),
                    None => contradictions.push(LogContradiction::ParkedWithoutRequest { seq: record.seq, call_id: call_id.clone() }),
                }
            }
            SessionEvent::ToolCallApproved { call_id, .. } => {
                if let Some(d) = dispatches.iter_mut().rev().find(|d| d.call_id == call_id && d.answered.is_none()) {
                    d.parked = None;
                }
            }
```
`:502` push adds `parked: d.parked,`. Variant + `tag` + Display:
```rust
    /// A `ToolCallParked` with no unanswered dispatch of its `call_id` before
    /// it. Reading: ignored — it names nothing the log can pair it with.
    ParkedWithoutRequest { seq: EventSeq, call_id: String },
// tag(): Self::ParkedWithoutRequest { .. } => "session-log-parked-without-request",
// Display: "park for call_id `{call_id}` at seq {seq} names no unanswered dispatch"
```
The doc line "the same nine words" (:45) loses its number.

`boundary_repair.rs:87`:
```rust
pub fn boundary_repair_text(
    tool: &str, provenance: DanglingProvenance, denied: bool,
    parked: Option<ParkReason>, degrade: Option<&DegradeNote>,
) -> String {
    let body = if denied {
        /* unchanged */
    } else if let Some(reason) = parked {
        format!(
            "NOT EXECUTED — this `{tool}` call never ran: when the server restarted it was still \
             waiting for {reason}. Nothing it would have done has happened: no file writes, no \
             commands, no network calls, no change to external state. If you still need it, call \
             it again — it will go through the gate again. {VERIFY_CLOSE}"
        )
    } else {
        /* the two provenance arms, unchanged */
    };
```
`repairs_for` passes `call.parked` fourth; doc "Three arms" → "Four arms".

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session:: tools::scoped::tests approval::` (T1's durability census and T6's marker census must be green — they are what pins the two seams above); `SKIP_BUILD=1 bash qa/resume_boundary/run.sh knobs` after the binary is rebuilt (the probe change); `cargo test -p alephcore --features test-helpers --test parked_gate_integration --test agent_ledger_gate_refusals -j 1` (the second binary proves the refactored decision path still signs the ledger).
- [ ] **Step 5: Mutation check** — move `self.record_parked(name, reason).await;` AFTER `request_approval(...).await`: `a_gate_park_is_in_the_log_before_the_requester_is_reached` RED (`parks == 0`); QA `parked` (T13) RED at `dangle-parked`. Make the Approved arm not clear `parked`: `an_answered_gate_ends_the_park` RED.
- [ ] **Step 6: Commit** — `git add src/session/events.rs src/session/call_log.rs src/session/mod.rs src/session/store.rs src/session/reduction.rs src/session/boundary_repair.rs src/tools/scoped/dispatch.rs src/tools/scoped/tests.rs src/clarification/ask.rs src/agents/subagent_spawner/fork.rs src/agents/subagent_tool/recovery.rs tests/parked_gate_integration.rs qa/resume_boundary/drive_r2.mjs` · `session: record ToolCallParked before the gate parks and answer it with a never-ran arm` + `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`

---

### Task 12: envelope carries `allowed_tools` / `btw`; resume replays them on the snapshot rung only

**Files:**
- Modify: `src/session/events.rs:174-217`; tests `:562-580`, `:617-647`
- Modify: `src/gateway/session_snapshot.rs:59-66` (add `RUN_ENVELOPE_FACT_KEYS`)
- Modify: `src/thinker/context.rs:118-201`
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:1398-1420`
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs:1617-1630`; test `:1680-1700`
- Modify: `src/gateway/execution_engine/slash_skill_scope.rs:47-58`
- Modify: `src/gateway/resume_coordinator.rs:344-350`, `:420-427`, `:1276-1290`; test fixture `:1572-1580`
- Modify: `src/gateway/btw/mod.rs:296-301`
- Test: `#[cfg(test)]` modules of each file

**Interfaces:**
- Consumes: `slash_skill_scope::{SLASH_SKILL_ALLOWED_TOOLS_KEY, from_metadata, stamp_from_mode}` (:38-58), `btw::{BTW_METADATA_KEY, PROMOTE_STAMP}` (`btw/mod.rs:41,55`), `ResumePlan.knobs` + `plan_resume` (:344-427), the `metadata.extend(plan.knobs)` merge (:1232), `busy_queue/durable.rs:460` (the side-question fan-out rule).
- Produces: `RunEnvelopeSnapshot { .., allowed_tools: Option<Vec<String>>, btw: Option<String> }`; `TurnEnvelope { .., allowed_tools: Option<Vec<String>>, btw: Option<String> }`; `pub const session_snapshot::RUN_ENVELOPE_FACT_KEYS: [&str; 2] = ["allowed_tools", "btw"]`; `pub(crate) fn slash_skill_scope::stamp_list(metadata: &mut HashMap<String, String>, tools: &[String])`.

Counted (判据 #6, comments stripped): `RunEnvelopeSnapshot {` literals = 6 (prod `runner_impl.rs:1622`; tests `events.rs:568,622`, `resume_coordinator.rs:1573` list every field → add two; `reduction.rs:1177,1181` use `..Default::default()`). `TurnEnvelope {` literals = 4 (prod `inner.rs:1398`; three tests use `..default()`). Skill-key writer 1 (`stamp_from_mode`), reader 1 (`from_metadata`), stripper 1.

- [ ] **Step 1: Write the failing tests**

`events.rs`:
```rust
    #[test]
    fn an_old_run_started_without_the_fact_keys_decodes_to_none_and_none_stays_off_the_wire() {
        let old = r#"{"type":"run_started","run_id":"r","at":1,"envelope":{"exec_tier":"ask"}}"#;
        let SessionEvent::RunStarted { envelope: Some(env), .. } = serde_json::from_str(old).unwrap() else { panic!() };
        assert_eq!((env.allowed_tools.as_ref(), env.btw.as_ref()), (None, None));
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("allowed_tools") && !json.contains("btw"), "{json}");
        let scoped = RunEnvelopeSnapshot { allowed_tools: Some(vec![]), btw: Some("q?".into()), ..Default::default() };
        assert!(serde_json::to_string(&scoped).unwrap().contains("\"allowed_tools\":[]"), "an empty list is a declaration");
        assert!(!scoped.is_empty());
    }
```
Census `the_envelope_carries_exactly_the_published_knob_keys`: `all` gains `allowed_tools: Some(vec!["g".into()]), btw: Some("h".into())`; `want` = `RUN_ENVELOPE_KNOB_KEYS.iter().chain(RUN_ENVELOPE_FACT_KEYS.iter())`, sorted.

`runner_impl.rs`:
```rust
    #[test]
    fn the_marker_records_the_per_run_facts_the_turn_is_running_under() {
        let env = TurnEnvelope { allowed_tools: Some(vec!["file_read".into()]), btw: Some("why?".into()), ..TurnEnvelope::default() };
        let snap = run_envelope_snapshot(&env, None, None);
        assert_eq!(snap.allowed_tools.as_deref(), Some(&["file_read".to_string()][..]));
        assert_eq!(snap.btw.as_deref(), Some("why?"));
    }
```
`slash_skill_scope.rs`:
```rust
    #[test]
    fn stamp_list_round_trips_through_from_metadata_including_the_empty_declaration() {
        for tools in [vec![], vec!["grep".to_string(), "file_read".to_string()]] {
            let mut meta = HashMap::new();
            stamp_list(&mut meta, &tools);
            assert_eq!(from_metadata(&meta), Some(tools.iter().cloned().collect::<HashSet<_>>()));
        }
    }
```
`resume_coordinator.rs` (fixtures `facts`/`envelope_with`/`pinnable` at :1560-1585; `envelope_with` gains `allowed_tools: None, btw: None`):
```rust
    #[test]
    fn a_snapshot_scope_reaches_the_request_and_an_absent_one_stamps_nothing() {
        use crate::gateway::execution_engine::slash_skill_scope::from_metadata;
        let mut env = envelope_with(Some("gpt-5.6"), Some("openai"), Some("full"));
        env.allowed_tools = Some(vec!["file_read".to_string()]);
        let plan = plan_resume(Some(&facts(None, Some(env))), Some(&pinnable(&["openai"])), &|_| true);
        assert_eq!(from_metadata(&plan.knobs), Some(["file_read".to_string()].into_iter().collect()));
        assert!(!plan.degraded && !plan.unsnapshotted);
        let plan = plan_resume(Some(&facts(None, Some(envelope_with(Some("gpt-5.6"), Some("openai"), Some("full"))))),
            Some(&pinnable(&["openai"])), &|_| true);
        assert_eq!(from_metadata(&plan.knobs), None, "no declaration ⇒ full surface");
        assert!(!plan.degraded, "an optional fact never degrades a resume");
    }
    #[test]
    fn a_btw_question_is_replayed_but_a_promote_sentinel_is_not() {
        use crate::gateway::btw::{BTW_METADATA_KEY, PROMOTE_STAMP};
        let mut env = envelope_with(Some("m"), Some("openai"), Some("ask"));
        env.btw = Some("is it green?".into());
        let plan = plan_resume(Some(&facts(None, Some(env.clone()))), Some(&pinnable(&["openai"])), &|_| true);
        assert_eq!(plan.knobs.get(BTW_METADATA_KEY).map(String::as_str), Some("is it green?"));
        env.btw = Some(PROMOTE_STAMP.into());
        let plan = plan_resume(Some(&facts(None, Some(env))), Some(&pinnable(&["openai"])), &|_| true);
        assert_eq!(plan.knobs.get(BTW_METADATA_KEY), None, "a promote is not a run; replaying it would hit the promote arm");
    }
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::events resume_coordinator slash_skill_scope run_envelope_snapshot_tests` → `no field allowed_tools` / `cannot find function stamp_list`.

- [ ] **Step 3: Minimal implementation**

`events.rs` `RunEnvelopeSnapshot`, after `model_provider`:
```rust
    /// `/<skill>` `allowed-tools` this run executed under. `Some(vec![])` is
    /// deny-all, `None` "declared nothing" (the `slash_skill_scope` tri-state).
    /// A per-run FACT, not a knob: on resume it has one rung — this snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// The `/btw` stamp exactly as `btw::BTW_METADATA_KEY` carried it, so a
    /// resumed side question keeps its read-only ceiling (`turn_permissions.rs:326`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btw: Option<String>,
```
`is_empty` adds `&& self.allowed_tools.is_none() && self.btw.is_none()`.

`session_snapshot.rs`, after `RUN_ENVELOPE_KNOB_KEYS`:
```rust
/// The per-run FACTS the envelope also freezes — not knobs: no `custom` twin,
/// no session/global rung, so `plan_resume` replays them from the snapshot or
/// not at all. Request-metadata spellings belong to `slash_skill_scope` and
/// `btw`; these are the marker's field names. The census in `session::events`
/// asserts the envelope's key set == KNOB ∪ FACT.
pub const RUN_ENVELOPE_FACT_KEYS: [&str; 2] = ["allowed_tools", "btw"];
```
`context.rs` `TurnEnvelope`: `pub allowed_tools: Option<Vec<String>>, pub btw: Option<String>` (doc: "carried for the `RunStarted` envelope; no prompt layer renders either"). `inner.rs:1398` literal adds:
```rust
                    // §6.3: the SAME scope that narrowed the surface above, and
                    // the SAME stamp `turn_permissions` reads.
                    allowed_tools: slash_skill_scope.as_ref().map(|s| { let mut v: Vec<String> = s.iter().cloned().collect(); v.sort(); v }),
                    btw: request.metadata.get(crate::gateway::btw::BTW_METADATA_KEY).cloned(),
```
`runner_impl.rs:1622` adds `allowed_tools: envelope.allowed_tools.clone(), btw: envelope.btw.clone(),`.

`slash_skill_scope.rs` — one encoder:
```rust
/// Write an explicit declaration (an empty one included) under the key.
pub(crate) fn stamp_list(metadata: &mut HashMap<String, String>, tools: &[String]) {
    if let Ok(encoded) = serde_json::to_string(tools) {
        metadata.insert(SLASH_SKILL_ALLOWED_TOOLS_KEY.to_string(), encoded);
    }
}
```
and `stamp_from_mode` ends in `stamp_list(metadata, &tools);`.

`resume_coordinator.rs` `plan_resume`, after the knob loop (:427):
```rust
    // §6.3 per-run facts: snapshot rung only. Absent = "declared nothing" —
    // not a degrade, and never a session/global fallback.
    if let Some(tools) = env.allowed_tools.as_deref() {
        crate::gateway::execution_engine::slash_skill_scope::stamp_list(&mut plan.knobs, tools);
    }
    match env.btw.as_deref() {
        Some(stamp) if stamp == crate::gateway::btw::PROMOTE_STAMP => {
            tracing::warn!("resume: marker carries a promote stamp; a promote is not a run and is not replayed");
        }
        Some(stamp) => { plan.knobs.insert(crate::gateway::btw::BTW_METADATA_KEY.to_string(), stamp.to_string()); }
        None => {}
    }
```
`ResumePlan.knobs` doc: "…plus the two per-run facts (skill scope, btw stamp)". At `:1276`, mirror `durable.rs:460`: compute `let is_side_question = metadata.contains_key(crate::gateway::btw::BTW_METADATA_KEY);` before `metadata` moves into the request, and `if is_side_question { base } else { match channel_registry() { … } }` — a re-delivered side answer must not land on the origin channel unmarked. `btw/mod.rs:299-301` paragraph becomes: "`resume_coordinator.rs` — `input` is `String::new()`; the stamp is replayed from the `RunStarted` envelope (§6.3) and the site answers as `busy_queue/durable.rs` does: a stamped resume skips the fan-out and rides the bus alone." The origin-fanout census (`origin_fanout.rs:385-415`) keeps its file set.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::events resume_coordinator slash_skill_scope run_envelope_snapshot_tests session_snapshot origin_fanout btw`
- [ ] **Step 5: Mutation check** — not in the §11 ledger (delete the `stamp_list` call in `plan_resume` → `a_snapshot_scope_reaches…` RED).
- [ ] **Step 6: Commit** — `git add src/session/events.rs src/gateway/session_snapshot.rs src/thinker/context.rs src/gateway/execution_engine/run_loop/inner.rs src/orchestrator/harness_bridge/runner_impl.rs src/gateway/execution_engine/slash_skill_scope.rs src/gateway/resume_coordinator.rs src/gateway/btw/mod.rs` · `gateway: freeze skill scope and btw stamp on RunStarted and replay them on resume` + Claude-Session line.

---

### Task 13: faces (`parked` on the wire, Panel/TUI wording, sub-agent twin) + QA stage `parked`

**Files:**
- Modify: `shared/protocol/src/session_thread.rs:541-567`, `:485-520`; tests `:645-662`
- Modify: `src/gateway/session_snapshot.rs:232-243`; `src/agents/subagent_tool/recovery.rs:508-521`, `:593-612` (twin, 判据 #16)
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:348-393`; tests `:2155-2165`, `:2185-2200`; `interfaces/webchat/locales/en.json:2706`, `zh.json:2706`
- Modify: `interfaces/tui/src/tui/commands.rs:1075-1108`; tests `:2358-2372`, `:2451-2470`
- Modify: `qa/resume_boundary/run.sh:52`, `:255-262`, `:266`; `qa/resume_boundary/drive_r2.mjs:304-320`, `:1040-1078` (the `:805` probe change and `REPAIR_MARKERS` landed in T11)
- Test: protocol `#[cfg(test)]`; Panel/TUI in-file test modules; the QA stage

**Interfaces:**
- Consumes: `DanglingCall.parked`, `ParkReason::as_str()` (T11); `LastRunState::dangling() -> Option<&[DanglingCallView]>` (`session_thread.rs:515`). A2 renames the `Interrupted { trailing_starts }` arms at `session_snapshot.rs:163-170,216-222`; this task touches only `dangling_view`.
- Produces: `DanglingCallView { .., #[serde(default, skip_serializing_if = "Option::is_none")] parked: Option<String> }`; `DanglingCallView::never_ran(&self) -> bool` (= `denied || parked.is_some()`); `LastRunState::never_ran_count(&self) -> Option<usize>`; locale key `narration.last_run_parked`; QA stage `parked` (floor below); `REPAIR_MARKERS` in `drive_r2.mjs`.

Counted: `DanglingCallView {` literals = 5 (prod `session_snapshot.rs:233`; tests `session_thread.rs:648`, `commands.rs:2363,2454`, `chat_sidebar.rs:2157`) — all gain `parked: None`. Readers of `.denied` = 3 (`session_snapshot.rs:241`, `recovery.rs:519`, `:594`); every face that renders a sentence from it gets the fourth bucket.

- [ ] **Step 1: Write the failing tests**

`session_thread.rs`:
```rust
    #[test]
    fn a_parked_dangling_call_reads_as_never_ran_and_an_old_view_reads_as_unknown() {
        let parked = DanglingCallView { call_id: "c".into(), tool_name: "bash".into(),
            provenance: DanglingCallView::THIS_RESTART.into(), denied: false, parked: Some("approval".into()) };
        assert!(parked.never_ran());
        let old: DanglingCallView = serde_json::from_value(serde_json::json!({"call_id":"c","tool_name":"bash"})).unwrap();
        assert!(old.parked.is_none() && !old.never_ran(), "absent is 'cannot vouch', never 'never ran'");
        assert!(!serde_json::to_value(&old).unwrap().as_object().unwrap().contains_key("parked"));
        let s = LastRunState { inspected: true, dangling: vec![parked, old], ..LastRunState::from_markers(LastRunState::INTERRUPTED, None, 1) };
        assert_eq!(s.never_ran_count(), Some(1));
        assert_eq!(LastRunState::from_markers(LastRunState::INTERRUPTED, None, 1).never_ran_count(), None, "the list face cannot vouch");
    }
```
`session_snapshot.rs` `last_run_tests`: log `run_started`, `requested("c1")`, `ToolCallParked{c1, Clarification}` → `last_run_from_events(..).dangling[0].parked == Some("clarification".into())`, and `serde_json::to_value(ParkReason::Clarification) == json!("clarification")` (wire word IS the serde word).

`recovery.rs` (beside :1604-1620): a `DanglingCall` with `parked: Some(ParkReason::Approval)` → `interrupted_note` contains `"never ran"` and `"waiting at a gate"`, its name not in the "outcome is unknown" list; `in_flight_json` row carries `"parked": "approval"`.

Panel (`dangling(n)` fixture gains `parked: None`):
```rust
    #[test]
    fn parked_calls_are_counted_as_never_ran_not_unknown() {
        let mut lr = interrupted();
        lr.dangling[0].parked = Some("approval".into());
        let notice = last_run_notice(&lr, Locale::default()).expect("news");
        assert!(notice.contains("2/5") && notice.contains('1'), "2 unknown, 1 never ran: {notice}");
        assert_ne!(notice, last_run_notice(&interrupted(), Locale::default()).unwrap());
        let all_parked = LastRunState { disposition: LastRunState::NEVER_RAN.into(), inspected: true,
            dangling: lr.dangling.iter().map(|d| DanglingCallView { parked: Some("clarification".into()), ..d.clone() }).collect(),
            ..LastRunState::default() };
        let n = last_run_notice(&all_parked, Locale::default()).expect("never-ran calls are news too");
        assert!(n.contains('3') && !n.contains('0'), "{n}");
    }
```
TUI: `parked_calls_are_reported_as_never_ran` — `interrupted()` with `parked: Some("approval".into())` → the line contains `"0 次结果未知"` and `"1 次工具调用从未执行"`.

QA `run.sh`: add `parked` to the `case` at :52; `FLOOR` arm `parked) FLOOR=11 ;;` (**designed** 2+1+3+5 — replace with the printed count on the first green run, same commit, as :251-255 requires); stage arm mirrors `denied`:
```bash
    parked)
      # Resume OFF so the receipt is the only pass. The dangle is the r2
      # instrument (BASH_POLICY=ask parks at the gate); new here: the kill
      # waits for the PARK ROW, not just the dispatch.
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" >/dev/null || exit 1
      start_server || exit 1
      drive dangle-parked qa-dangle || { echo "instrument failure: no parked dangle" >&2; RC=1; }
      hard_kill_server
      [ "$RC" = "0" ] && { drive assert-dangling 1 || RC=1; }
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { drive parked wire || RC=1; }
      if [ "$RC" = "0" ]; then
        say "aleph-server resume --json"
        "$BIN" resume --json "$(cat "$SESSION_FILE")" >"$RECEIPT" 2>"$QA_ROOT/resume.err"
        echo "resume rc=$? receipt:"; cat "$RECEIPT"
        drive parked model "$RECEIPT" || RC=1
      fi
      ;;
```
`drive_r2.mjs`:
```js
/** Dangling call ids that also have a `tool_call_parked` row — the park as a FACT, not a card. */
const parkedIds = () => {
  const open = new Set(danglingIds());
  return eventsOf().filter((r) => r.event_type === "tool_call_parked")
    .map((r) => { try { return JSON.parse(r.payload_json).call_id; } catch { return null; } })
    .filter((id) => id && open.has(id));
};
async function cmdDangleParked(marker = "qa-dangle") {
  await cmdDangle(marker);
  const landed = await until(() => (parkedIds().length > 0 ? parkedIds() : null), 60_000, 300);
  check(Boolean(landed), "a tool_call_parked row landed for the dangling call BEFORE the kill", show(eventsOf().slice(-4)));
  if (!landed) process.exit(1);
  const conn = new Conn("driver");
  await conn.open();
  const pending = (await conn.attempt("exec.approvals.pending")).result?.pending ?? [];
  check(pending.some((p) => p.record?.tool_call_id === landed[0]),
    "and exec.approvals.pending holds that call id — the gate is parked on it, not merely logged", show(pending));
  conn.close();
}
// `REPAIR_MARKERS` / `carriesRepair` exist since T11.
async function cmdParked(sub, receiptFile) {
  const key = readSession();
  if (sub === "model") {
    const hit = await until(() => requests().find((r) => userText(r.body).includes("never ran")) || null, 180_000, 1000);
    check(Boolean(hit), "the model's next request says the call NEVER RAN", `${requests().length} requests, none carrying the phrase`);
    const text = hit ? userText(hit.body) : "";
    check(text.includes("operator approval"), "and names what it was waiting for", text.slice(0, 400));
    check(text.includes("bash"), "and names the tool", text.slice(0, 400));
    check(!requests().some((r) => userText(r.body).includes("OUTCOME UNKNOWN")), "and no request calls that parked call's outcome UNKNOWN");
    let receipt = null;
    try { receipt = JSON.parse(fs.readFileSync(receiptFile, "utf8")); } catch { /* asserted below */ }
    check(receipt?.resumed === 1, "the receipt counts one resumed run", show(receipt));
    return;
  }
  const conn = new Conn("driver");
  await conn.open();
  const { lastRun } = await lastRunOf(conn, key);
  const d = (lastRun?.dangling || [])[0] || {};
  check(d.parked === "approval", "the wire face says the dangling call was parked awaiting approval", show(lastRun?.dangling));
  check(d.denied === false, "and not denied", show(d));
  check(lastRun?.disposition === "interrupted", "and still reads the run as interrupted", show(lastRun?.disposition));
  conn.close();
}
```
Dispatch arms: `case "dangle-parked": await cmdDangleParked(REST[0]); break;` · `case "parked": await cmdParked(REST[0] ?? "wire", REST[1]); break;`. (The `:805` `knobs` probe already reads `carriesRepair` since T11.)

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p aleph-protocol` → `no field parked`; `cargo test -p aleph-panel --lib -- chat_sidebar` / `cargo test -p aleph-tui -- last_run` → same; `SKIP_BUILD=1 bash qa/resume_boundary/run.sh parked` → `unknown stage: parked`.

- [ ] **Step 3: Minimal implementation**

`session_thread.rs`:
```rust
    /// Why the call was parked at a gate when the log ends (`"approval"` /
    /// `"clarification"` / `"pre_hook"`, the core's `ParkReason` words): it
    /// never ran. Absent = not parked, or an older core; neither is "never ran".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked: Option<String>,
```
```rust
impl DanglingCallView {
    /// The log proves this call never started: refused by the gate, or still
    /// parked at one. One predicate for every face (Panel, TUI, sub-agent note).
    #[must_use]
    pub fn never_ran(&self) -> bool { self.denied || self.parked.is_some() }
}
impl LastRunState {
    /// Of the dangling calls, how many never ran; `None` on the list face (see `dangling`).
    #[must_use]
    pub fn never_ran_count(&self) -> Option<usize> {
        self.dangling().map(|d| d.iter().filter(|c| c.never_ran()).count())
    }
}
```
`session_snapshot.rs::dangling_view`: `parked: call.parked.map(|r| r.as_str().to_string()),`. `recovery.rs`: `in_flight_json` adds `"parked": c.parked.map(ParkReason::as_str),`; `interrupted_note` partitions three ways — `denied` (sentence unchanged), `parked` (`" These calls were waiting at a gate (approval or a question) when the child stopped and never ran: [{}]."`), `unknown` (the rest).

`chat_sidebar.rs::last_run_notice`:
```rust
    // One split, from the protocol's own predicate: the calls the log proves
    // never started, and the rest, whose outcome nobody can vouch for.
    let counts = last_run.dangling().map(|d| {
        let never_ran = d.iter().filter(|c| c.never_ran()).count();
        (d.len() - never_ran, never_ran)
    });
    let parked_line = |m: usize| td_string!(locale, narration.last_run_parked, parked = m as i64).to_string();
    let with_parked = |base: String, m: usize| if m > 0 { format!("{base} {}", parked_line(m)) } else { base };
    match last_run.disposition() {
        D::LogInconsistent => { /* unchanged */ }
        D::Interrupted => Some(match (last_run.progress, counts) {
            (Some(p), Some((unknown, never_ran))) => with_parked(
                td_string!(locale, narration.last_run_interrupted, answered = i64::from(p.tool_calls_answered),
                    dispatched = i64::from(p.tool_calls_dispatched), unknown = unknown as i64).to_string(),
                never_ran),
            _ => td_string!(locale, narration.last_run_interrupted_plain).to_string(),
        }),
        D::Unrecognized => { /* unchanged */ }
        D::Clean | D::NeverRan => match counts {
            Some((unknown, never_ran)) if unknown > 0 => Some(with_parked(
                td_string!(locale, narration.last_run_dangling, count = unknown as i64).to_string(), never_ran)),
            Some((0, never_ran)) if never_ran > 0 => Some(parked_line(never_ran)),
            _ => None,
        },
    }
```
`en.json:2706` (+1 line): `"last_run_parked": "{{ parked }} of its tool calls never ran — they were waiting for approval or for your answer when the server stopped, or were denied"`; `zh.json`: `"last_run_parked": "其中 {{ parked }} 次工具调用从未执行 — 服务停止时它们还在等审批 / 等回答，或已被拒绝"`.

`commands.rs::last_run_notice`: same shape; `parked_line = |m| format!("其中 {m} 次工具调用从未执行 — 服务停止时还在等审批 / 等回答，或已被拒绝")`; `Interrupted` prints `{unknown} 次结果未知`; the `Clean | NeverRan` arm mirrors the Panel's three cases. The badge rule at `:537` is unchanged (it keys on the word).

- [ ] **Step 4: Run, expect PASS** — `cargo test -p aleph-protocol`; `cargo test -p alephcore --lib -- session_snapshot recovery`; `cargo test -p aleph-panel --lib`; `just wasm`; `cargo test -p aleph-tui`; `node --version` (≥ 22.5 for `node:sqlite`; this host: v24.13.0); `bash qa/resume_boundary/run.sh parked` (builds once), then `SKIP_BUILD=1 bash qa/resume_boundary/run.sh knobs` (probe change) and `SKIP_BUILD=1 bash qa/resume_boundary/run.sh denied` (parked-then-denied still reads denied).
- [ ] **Step 5: Mutation check** — spec §11 "6.1 `Parked` 写点挪后 ⇒ `parked` 红": move `record_parked` after `request_approval` (T11) → `dangle-parked` prints `FAIL a tool_call_parked row landed …`, exit 1. Flip `never_ran()` to `self.denied` only → Panel/TUI parked tests RED while `parked wire` stays green (the wire is honest; the faces are what the flip breaks).
- [ ] **Step 6: Commit** — `git add shared/protocol/src/session_thread.rs src/gateway/session_snapshot.rs src/agents/subagent_tool/recovery.rs interfaces/webchat/src/components/chat_sidebar.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json interfaces/tui/src/tui/commands.rs qa/resume_boundary/run.sh qa/resume_boundary/drive_r2.mjs` · `protocol: carry parked on DanglingCallView and say never-ran on every face; qa parked stage` + Claude-Session line.

---

### Task 14: per-row decode isolation, `undecodable-record` REJECT, doctor `--fix`

**Files:**
- Modify (after T1/T6 — locate by symbol): `src/session/store.rs:36-141` (trait), `:301-350` (encode), `:359-406` (range read), `:537-585` (markers), tests `#[cfg(test)] mod tests`
- Modify: `src/session/service.rs:14-30` (`SessionError`)
- Modify (after T6/T7/T11): `src/session/reduction.rs:54-135` (variant/tag/rejects/Display), `:801-871` (closed-set tests: `kind_index`, `KIND_COUNT`, `one_of_each`, `the_two_reject_kinds_…`)
- Modify (after T2): `src/session/actor.rs:141-147` (`replay` → `load_head_seq`)
- Modify (after T6/T7/T12): `src/gateway/resume_coordinator.rs:686-695`, `:759-848` (`resume_from_markers`), `:886-889`, `:920-948` (`handle_interrupted` read arm)
- Modify (after T4/T7): `src/gateway/projection_reconciler.rs:146-167`; `src/gateway/session_snapshot.rs:198-228` + 3 test call sites `:424,437,459`; `src/gateway/handlers/session/db_handlers/query.rs:65-79,131-134`; `src/gateway/handlers/chat.rs:629-644`
- Modify: `src/gateway/session_projector.rs:1272`, `src/session/actor.rs:507` (mock impls)
- Modify: `src/orchestrator/harness_bridge/error.rs:33-42`
- Modify: `src/diagnostics/checks/session_log.rs:83-266` + tests
- Test: same-file `#[cfg(test)] mod tests` in each

**Interfaces:**
- Consumes: `SessionEventStore` (`store.rs:39`), `reduce_run`/`reduce_disposition` (`reduction.rs:316,361`), `HealthCheck`/`Finding::{problem,repairable,with_repair}` (`diagnostics/check.rs:54`, `finding.rs:97-111`), `classify_harness_error` (`harness_bridge/error.rs:33`), A1's `ignorable(&SessionEvent) -> bool`.
- Produces (store.rs): `pub const SESSION_EVENT_SCHEMA_VERSION: u16 = 1`; `pub struct UndecodableRecord { pub seq: EventSeq, pub kind_tag: Option<String>, pub error: String }` (+`Display`); `pub enum DecodedRow { Event(SessionEventRecord), Skipped { seq: EventSeq, kind_tag: String }, Undecodable(UndecodableRecord) }`; `pub type MarkerSlice = Result<Vec<SessionEventRecord>, UndecodableRecord>`; `pub fn encode_row(event: &SessionEvent) -> Result<String, SessionError>`; `pub fn decode_row(seq: EventSeq, created_at_ms: i64, json: &str) -> DecodedRow`; `pub fn fold_strict(rows: Vec<DecodedRow>) -> Result<Vec<SessionEventRecord>, UndecodableRecord>`; trait: `async fn load_rows(&self, session_id: &SessionId) -> Result<Vec<DecodedRow>, SessionError>` (default `Err(Storage)`), `async fn retire_record(&self, session_id: &SessionId, seq: EventSeq) -> Result<bool, SessionError>` (default `Err(Storage)`), `async fn load_run_markers(&self) -> Result<Vec<(SessionId, MarkerSlice)>, SessionError>`.
- Produces (service.rs): `SessionError::UndecodableRecord(UndecodableRecord)`.
- Produces (reduction.rs): `LogContradiction::UndecodableRecord { seq: EventSeq }` (REJECT, tag `session-log-undecodable-record`), `impl From<&UndecodableRecord> for LogContradiction`, `pub fn reduce_marker_slice(slice: &MarkerSlice) -> Result<RunDisposition, LogContradiction>`.
- Produces (session_snapshot.rs): `pub fn last_run_from_markers(markers: Result<&[SessionEventRecord], &UndecodableRecord>) -> LastRunState`.

- [ ] **Step 1: Write the failing tests** (store.rs tests; `make_store`, `sample_session_id`, `user_message` exist at `:979-1195`)

```rust
fn raw_insert(store: &SqliteEventStore, sid: &SessionId, seq: i64, event_type: &str, json: &str) {
    let conn = store.conn.blocking_lock();
    conn.execute(
        "INSERT INTO session_events (session_id, seq, event_type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, 1)",
        params![session_id_to_string(sid).unwrap(), seq, event_type, json],
    ).unwrap();
}

#[test]
fn every_row_carries_the_schema_version_and_omits_ignorable_when_false() {
    let json = encode_row(&user_message(uuid::Uuid::new_v4(), "hi", 1)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["v"], SESSION_EVENT_SCHEMA_VERSION);
    assert!(v.get("ignorable").is_none(), "absent, never `false`: {json}");
    assert!(matches!(decode_row(1, 1, &json), DecodedRow::Event(_)));
}

#[test]
fn an_unknown_variant_is_undecodable_unless_the_row_says_ignorable() {
    assert!(matches!(decode_row(7, 1, r#"{"type":"from_the_future","v":9}"#),
        DecodedRow::Undecodable(UndecodableRecord { seq: 7, kind_tag: Some(t), .. }) if t == "from_the_future"));
    assert!(matches!(decode_row(8, 1, r#"{"type":"from_the_future","ignorable":true}"#),
        DecodedRow::Skipped { seq: 8, kind_tag } if kind_tag == "from_the_future"));
    // a missing key reads as false; a KNOWN variant that will not parse is corruption, not skippable
    assert!(matches!(decode_row(9, 1, r#"{"type":"from_the_future","ignorable":"yes"}"#), DecodedRow::Undecodable(_)));
    assert!(matches!(decode_row(10, 1, r#"{"type":"tool_call_requested","ignorable":true}"#), DecodedRow::Undecodable(_)));
    assert!(matches!(decode_row(11, 1, "{not json"), DecodedRow::Undecodable(UndecodableRecord { kind_tag: None, .. })));
}

#[tokio::test]
async fn one_bad_row_refuses_only_its_own_session() {
    let store = make_store();
    let (a, b) = (SessionKey::main("a"), SessionKey::main("b"));
    for sid in [&a, &b] {
        store.append(sid, 1, &run_started("r", 1), 1).await.unwrap();
    }
    raw_insert(&store, &a, 2, "from_the_future", r#"{"type":"from_the_future"}"#);
    let err = store.load_all_events(&a).await.unwrap_err();
    assert!(matches!(err, SessionError::UndecodableRecord(UndecodableRecord { seq: 2, .. })), "{err}");
    assert_eq!(store.load_all_events(&b).await.unwrap().len(), 1);
    // the marker scan: a bad MARKER row refuses only that session's slice
    raw_insert(&store, &a, 3, "run_finished", r#"{"type":"run_finished","outcome":"???"}"#);
    let groups = store.load_run_markers().await.unwrap();
    assert!(matches!(&groups.iter().find(|(s, _)| *s == a).unwrap().1, Err(u) if u.seq == 3));
    assert!(matches!(&groups.iter().find(|(s, _)| *s == b).unwrap().1, Ok(m) if m.len() == 1));
    // retire_record is the single-record exit doctor --fix takes
    assert!(store.retire_record(&a, 2).await.unwrap());
    assert!(!store.retire_record(&a, 2).await.unwrap(), "idempotent");
}

#[tokio::test]
async fn an_ignorable_row_is_skipped_by_readers_and_counted_by_load_rows() {
    let store = make_store();
    let sid = sample_session_id();
    store.append(&sid, 1, &run_started("r", 1), 1).await.unwrap();
    raw_insert(&store, &sid, 2, "from_the_future", r#"{"type":"from_the_future","ignorable":true}"#);
    assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
    let rows = store.load_rows(&sid).await.unwrap();
    assert_eq!(rows.iter().filter(|r| matches!(r, DecodedRow::Skipped { .. })).count(), 1);
}

#[test]
fn the_event_table_migration_only_ever_adds_columns() {
    let src = crate::utils::source_scan::production_code_lines(include_str!("store.rs"));
    let alters: Vec<&str> = src.lines().filter(|l| l.contains("ALTER TABLE session_events")).collect();
    assert!(!alters.is_empty() && alters.iter().all(|l| l.contains("ADD COLUMN")), "{alters:?}");
    assert!(!src.contains("user_version"), "a schema gate is fail-dead (spec 7.4)");
}
```

events.rs tests: `fn every_envelope_field_is_defaultable() { let e: RunEnvelopeSnapshot = serde_json::from_str("{}").unwrap(); assert!(e.is_empty()); }` — `{}` parses iff every field is `#[serde(default)]`; a field added without it turns this red.

reduction.rs closed-set tests: add `LogContradiction::UndecodableRecord { seq: 1 }` to `one_of_each`, index `11` in `kind_index` (T6 took 9, T11 took 10), `KIND_COUNT = 12`, and in `the_two_reject_kinds_…` change `matches!(kind_index(&c), 0 | 1)` → `0 | 1 | 11` and rename to `the_three_reject_kinds_…`. Add:

```rust
#[test]
fn a_marker_slice_that_did_not_decode_is_refused_under_its_own_kind() {
    let slice: MarkerSlice = Err(UndecodableRecord { seq: 4, kind_tag: None, error: "x".into() });
    assert_eq!(reduce_marker_slice(&slice), Err(LogContradiction::UndecodableRecord { seq: 4 }));
    assert_eq!(reduce_marker_slice(&Ok(vec![rec(1, started("a"))])), Ok(RunDisposition::Interrupted { attempts: 0 }));
}
```

session_log.rs test (fixture: in-memory `SqliteEventStore` + `raw_insert` as above, `SessionLogCheck::new(None, Some(store))`):

```rust
#[tokio::test]
async fn an_undecodable_row_is_named_and_fix_retires_exactly_that_record() {
    let (store, sid) = seeded_store_with_bad_row(7).await; // RunStarted@1, bad row @7
    let check = SessionLogCheck::new(None, Some(store.clone() as Arc<dyn SessionEventStore>));
    let f = &check.run(Posture::Inspect).await[0];
    assert!(f.detail.contains("session-log-undecodable-record") && f.detail.contains("seq 7"), "{}", f.detail);
    assert!(f.repairable);
    let f = &check.run(Posture::Fix).await[0];
    assert!(matches!(f.repair_outcome, Some(RepairOutcome::Repaired { .. })));
    assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1, "only the bad record was retired");
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::store::tests session::reduction::tests diagnostics::checks::session_log` → `error[E0425]: cannot find function encode_row`, `no variant named UndecodableRecord`.

- [ ] **Step 3: Minimal implementation**

store.rs:
```rust
pub const SESSION_EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndecodableRecord { pub seq: EventSeq, pub kind_tag: Option<String>, pub error: String }
impl std::fmt::Display for UndecodableRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "seq {} (type `{}`) could not be decoded by this build: {}",
            self.seq, self.kind_tag.as_deref().unwrap_or("?"), self.error)
    }
}
#[derive(Debug, Clone)]
pub enum DecodedRow { Event(SessionEventRecord), Skipped { seq: EventSeq, kind_tag: String }, Undecodable(UndecodableRecord) }
pub type MarkerSlice = Result<Vec<SessionEventRecord>, UndecodableRecord>;

/// The one encoder: `v` on every row, `ignorable` only when the policy table says so.
pub fn encode_row(event: &SessionEvent) -> Result<String, SessionError> {
    let mut v = serde_json::to_value(event)?;
    let serde_json::Value::Object(m) = &mut v else {
        return Err(SessionError::Storage("event did not serialize to an object".into()));
    };
    m.insert("v".into(), SESSION_EVENT_SCHEMA_VERSION.into());
    if crate::session::events::ignorable(event) { m.insert("ignorable".into(), true.into()); }
    Ok(v.to_string())
}

/// Unknown variant + `"ignorable": true` ⇒ Skipped; a missing key reads as false; anything else ⇒ Undecodable.
pub fn decode_row(seq: EventSeq, created_at_ms: i64, json: &str) -> DecodedRow {
    let err = match serde_json::from_str::<SessionEvent>(json) {
        Ok(event) => return DecodedRow::Event(SessionEventRecord { seq, event, created_at_ms }),
        Err(e) => e,
    };
    let raw: Option<serde_json::Value> = serde_json::from_str(json).ok();
    let kind_tag = raw.as_ref().and_then(|v| v["type"].as_str()).map(str::to_string);
    let unknown_variant = err.to_string().starts_with("unknown variant"); // serde's fixed machine text; pinned by test
    let ignorable = raw.as_ref().is_some_and(|v| v["ignorable"] == serde_json::Value::Bool(true));
    match (kind_tag, unknown_variant && ignorable) {
        (Some(kind_tag), true) => DecodedRow::Skipped { seq, kind_tag },
        (kind_tag, _) => DecodedRow::Undecodable(UndecodableRecord { seq, kind_tag, error: err.to_string() }),
    }
}

/// Strict fold for the model path: Skipped rows are dropped (debug-traced), the first Undecodable wins.
pub fn fold_strict(rows: Vec<DecodedRow>) -> Result<Vec<SessionEventRecord>, UndecodableRecord> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        match row {
            DecodedRow::Event(r) => out.push(r),
            DecodedRow::Skipped { seq, kind_tag } => tracing::debug!(seq, kind_tag, "session_events: ignorable row skipped"),
            DecodedRow::Undecodable(u) => return Err(u),
        }
    }
    Ok(out)
}
```
Trait additions (defaults `Err(SessionError::Storage("this event store cannot expose raw rows".into()))` / `"…cannot retire a single record"`), `load_run_markers` return type widened. `SqliteEventStore`: private `fn query_rows(conn, key, from, to) -> Result<Vec<DecodedRow>, SessionError>` (today's SELECT at `:378-405`, mapping each row through `decode_row`); `load_events_range` = `fold_strict(query_rows(..)?).map_err(SessionError::UndecodableRecord)`; `load_rows` = `query_rows(conn, key, 0, i64::MAX)`; `load_run_markers` keeps the SQL, groups `DecodedRow`s per session, then `(sid, fold_strict(rows))`; `retire_record`: `UPDATE session_events SET retired_at = ?1 WHERE session_id = ?2 AND seq = ?3 AND retired_at IS NULL` → `Ok(changed == 1)`. T1's private `encode_payload` is DELETED and its one call site in `append_batch` becomes `encode_row(event)?` — one encoder, and `events::ignorable` (T1's second policy column) gets its one and only consumer here. service.rs: `#[error("undecodable session record: {0}")] UndecodableRecord(crate::session::store::UndecodableRecord)`. actor.rs `replay`: `self.head_seq = self.store.load_head_seq(&self.id).await?;` (the actor must still boot so `/undo` and doctor's retire can reach the log; `load_head_seq` never decodes and is the seq allocator's own definition — `replay_rebuilds_head_seq` stays green).

reduction.rs:
```rust
/// A row of this slice did not decode. REJECT — the reducer never saw the record, so no reading exists.
UndecodableRecord { seq: EventSeq },
// rejects(): | Self::UndecodableRecord { .. }; tag(): "session-log-undecodable-record";
// Display: "the record at seq {seq} could not be decoded by this build; run the doctor (core/session-log, fix=true) to retire that one record, or /undo to before it"
impl From<&crate::session::store::UndecodableRecord> for LogContradiction {
    fn from(u: &crate::session::store::UndecodableRecord) -> Self { Self::UndecodableRecord { seq: u.seq } }
}
pub fn reduce_marker_slice(slice: &MarkerSlice) -> Result<RunDisposition, LogContradiction> {
    match slice { Ok(markers) => reduce_disposition(markers), Err(u) => Err(LogContradiction::from(u)) }
}
```

Consumers: `resume_from_markers(&self, session_id, slice: &MarkerSlice, report)` — `report.scanned += 1; let markers: &[_] = slice.as_deref().unwrap_or(&[]); match reduce_marker_slice(slice) { … existing arms … }`; `resume_interrupted_runs`/`resume_session` pass `&slice`. `handle_interrupted` `load_all_events` `Err` arm splits: `Err(SessionError::UndecodableRecord(u)) => report.refused.push((sid.clone(), ResumeRefusal::LogInconsistent(LogContradiction::from(&u))))`, other errors unchanged. `projection_reconciler::candidates`: `match reduce_marker_slice(&slice)`. `query.rs`: map keeps `MarkerSlice`; call `last_run_from_markers(by_session.get(&m.key).map_or(Ok(&[][..]), |s| s.as_deref()))` where `last_run_from_markers` starts `let verdict = match markers { Ok(m) if m.is_empty() => return NEVER_RAN…, Ok(m) => reduce_run(m), Err(u) => Err(LogContradiction::from(u)) };`. `chat.rs:634`: `Err(SessionError::UndecodableRecord(u)) => Some(LastRunState { disposition: LOG_INCONSISTENT.into(), contradictions: vec![LogContradiction::from(&u).tag().into()], inspected: true, ..Default::default() })` — the attach face names it (criterion #9), other errors stay `None`. Mocks: return `Ok(Vec::new())` unchanged in body, type widened.

harness_bridge/error.rs, first arm of `classify_harness_error` (structural, never re-dispatched):
```rust
if let HarnessError::Session(SessionError::UndecodableRecord(u)) = &err {
    return FlowError::Internal(format!(
        "this session's log holds a record this build cannot read ({u}). Run the doctor \
         (`core/session-log`, fix=true) to retire that one record, or `/undo` to before it."));
}
```

session_log.rs: per session `events.load_rows(&id)`; walk rows: first `Undecodable(u)` ⇒ `bad.push(Contradicting { key, refused: true, tags: vec![LogContradiction::from(&u).tag()], undecodable: Some(u) })`, `Skipped` ⇒ `skipped += 1`, else `reduce_run(&events)` as today. Detail names `seq N type T` for undecodable entries and appends `"; {skipped} ignorable row(s) skipped"` (also on the OK finding). When any `undecodable.is_some()`: `.repairable()` + hint "…`fix=true` retires ONLY the undecodable record(s); contradictions stay report-only"; `posture.allows_repair()` ⇒ `events.retire_record(&id, u.seq)` per entry → `RepairOutcome::{Repaired{detail}, Failed{error}}`. Update the `every_contradiction_tag_belongs_to_this_checks_namespace` list with the new variant.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session:: gateway::resume_coordinator gateway::projection_reconciler gateway::session_snapshot diagnostics::checks::session_log orchestrator::harness_bridge` and `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1`.
- [ ] **Step 5: Mutation check** (§11 ledger 7.1): in `load_run_markers` replace `fold_strict(rows)` with `Ok(rows.into_iter().filter_map(|r| match r { DecodedRow::Event(e) => Some(e), _ => None }).collect())` ⇒ `one_bad_row_refuses_only_its_own_session` RED (session `a` reads `Ok`); in `decode_row` drop the `unknown_variant &&` guard ⇒ `an_unknown_variant_is_undecodable_unless_the_row_says_ignorable` RED (row 10).
- [ ] **Step 6: Commit** (three): `git add src/session/store.rs src/session/service.rs src/session/actor.rs src/session/reduction.rs src/session/events.rs` → `session: decode session_events per row and refuse only the session that holds a bad one`; `git add src/gateway/resume_coordinator.rs src/gateway/projection_reconciler.rs src/gateway/session_snapshot.rs src/gateway/handlers/session/db_handlers/query.rs src/gateway/handlers/chat.rs src/gateway/session_projector.rs src/orchestrator/harness_bridge/error.rs` → `gateway: name an undecodable record on every face instead of skipping the scan`; `git add src/diagnostics/checks/session_log.rs` → `diagnostics: core/session-log names and retires an undecodable record`. Each ends with `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`.

---

### Task 15: bounded-parallel resume, reinjection split around it

**Files:**
- Modify: `src/config/types/resume.rs:22-25,40-42,64-72` — the field `max_concurrent` ALREADY EXISTS (default 4) and has ZERO readers (criterion #7: a config field nothing reads); this task CONNECTS it and lowers the default 4 → 2 per spec §8.1 (the 4 was never exercised, so it carries no evidence); `defaults_are_sane` asserts 2
- Modify (after T7): `src/gateway/resume_coordinator.rs` — T7 put the activity-window loop in `resume_interrupted_runs`; it moves into `launch_resume` below, body unchanged
- Modify: `src/gateway/resume_coordinator.rs:83-120` (`ResumeReport::absorb`), `:672-712` (`resume_interrupted_runs` → `launch_resume` + `ResumeLaunch`), `:764-775` (comment), `:881-897` (`resume_session` takes the permit), `:1198-1210` (`retrigger` drops its acquire)
- Modify: `src/gateway/busy_queue/durable.rs:401-425` (`select` predicate)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:3044-3090`
- Modify: `tests/resume_coordinator_integration.rs` — every `let coordinator = ResumeCoordinator::new(` → `Arc::new(…)` (grep count first: `grep -c 'ResumeCoordinator::new(' tests/resume_coordinator_integration.rs`)
- Test: `tests/resume_coordinator_integration.rs` (the `RecordingAdapter`, `registry_with_agent`, `store`, `sessions`, `test_bus`, `seed_interrupted_run` fixtures live there — unit tests in `resume_coordinator.rs` have no adapter)

**Interfaces:**
- Consumes: `ResumeCoordinator::{resume_from_markers, semaphore, config}`, `reinject_survivors` (`durable.rs:401`), `has_own_scheduler`, `SessionRunRegistry::try_claim` (`session_run_registry.rs:108`) via `gate.rs::admit_run:105`.
- Produces: `pub struct ResumeLaunch { pub pending: HashSet<String>, .. }` with `pub async fn settle(self) -> ResumeReport`; `pub async fn launch_resume(self: &Arc<Self>) -> ResumeLaunch`; `pub async fn resume_interrupted_runs(self: &Arc<Self>) -> ResumeReport` (= launch + settle); `impl ResumeReport { pub fn absorb(&mut self, other: ResumeReport) }`; `pub async fn reinject_survivors(adapter, registry, bus, cfg, select: &(dyn Fn(&SessionKey) -> bool + Sync)) -> usize`.

**Admission finding (asked for).** A reinjected survivor is admitted by `deliver_with_ticket` → `adapter.execute` → `ExecutionEngine::admit_run` (`gate.rs:105`) → `SessionRunRegistry::try_claim`; a refusal is `ExecutionError::AgentBusy` and the ticket parks. That gate knows nothing about a resume that has not yet reached `retrigger`: with `max_concurrent = 2` the third candidate holds no engine slot while it waits on the semaphore, so a survivor for it would be admitted first and the later resume would append its boundary repair into a live run and then be refused `retrigger_failed`. The spec's "lane-deferred" assumption does not hold, so reinjection is split: survivors for sessions the scan will act on (`ResumeLaunch::pending`) re-enter after `settle()`, all others immediately after launch.

- [ ] **Step 1: Write the failing test** (`tests/resume_coordinator_integration.rs`; `SlowAdapter` wraps `RecordingAdapter` and sleeps 2 s when the session key contains `b`, recording `(key, Instant)` on entry and exit)

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_interrupted_sessions_resume_two_at_a_time_and_the_slow_one_does_not_block_the_rest() {
    let store = store();
    let keys = ["a", "b", "c"].map(SessionKey::main);
    for k in &keys { seed_interrupted_run(&store, k).await; }
    let adapter = Arc::new(SlowAdapter::new("b", Duration::from_secs(2)));
    let registry = registry_with_agents(&["a", "b", "c"]).await;
    let cfg = ResumeConfig { max_concurrent: 2, ..ResumeConfig::default() };
    let coordinator = Arc::new(ResumeCoordinator::new(store, cfg, adapter.clone(), registry, sessions(), test_bus()));
    let t0 = Instant::now();
    let report = coordinator.resume_interrupted_runs().await;
    assert_eq!((report.scanned, report.resumed), (3, 3), "{report:?}");
    let done = adapter.exits(); // Vec<(String, Instant)>
    let at = |k: &str| done.iter().find(|(s, _)| s.contains(k)).unwrap().1;
    assert!(at("a") < at("b") && at("c") < at("b"), "a and c must finish before the 2 s session");
    assert!(t0.elapsed() < Duration::from_millis(3500), "serial would be ≥ 2 s + everything else");
    assert_eq!(adapter.max_in_flight(), 2, "the cap bounds the burst");
}
```
Plus in `busy_queue/durable.rs` tests: `reinject_survivors(.., &|k| k.to_key_string() != "agent:main:b")` leaves `b`'s record journaled and re-queues the rest (assert via `survivors()` after the call).

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1 -- three_interrupted` → today: `assertion failed: at("a") < at("b")` (serial: b finishes before c starts) or `max_in_flight == 1`.

- [ ] **Step 3: Minimal implementation**

```rust
/// A launched boot scan: `pending` is every session the scan will ACT on (claimed
/// verdict Interrupted / Unanswered), so a caller can hold that session's queued
/// input back until `settle`; everything else is safe to re-deliver at once.
pub struct ResumeLaunch {
    pub pending: std::collections::HashSet<String>,
    tasks: tokio::task::JoinSet<(usize, ResumeReport)>,
    report: ResumeReport,
    me: Arc<ResumeCoordinator>,
}
impl ResumeLaunch {
    pub async fn settle(mut self) -> ResumeReport {
        let mut parts = Vec::new();
        while let Some(joined) = self.tasks.join_next().await {
            match joined { Ok(p) => parts.push(p), Err(e) => tracing::warn!(error = %e, "resume: candidate task panicked") }
        }
        parts.sort_by_key(|(i, _)| *i); // candidate order, so `refused` reads like the scan walked
        for (_, part) in parts { self.report.absorb(part); }
        // T16 inserts `self.me.adjudicate_orphaned_tasks(&mut self.report).await;` HERE.
        tracing::info!(scanned = self.report.scanned, resumed = self.report.resumed, abandoned = self.report.abandoned,
            skipped = self.report.skipped, delegated = self.report.delegated, busy = self.report.busy, "resume scan complete");
        self.report
    }
}
impl ResumeCoordinator {
    pub async fn launch_resume(self: &Arc<Self>) -> ResumeLaunch {
        let mut launch = ResumeLaunch { pending: Default::default(), tasks: tokio::task::JoinSet::new(),
            report: ResumeReport::default(), me: Arc::clone(self) };
        if !self.config.enabled { tracing::debug!("resume disabled ([resume] enabled = false); skipping scan"); return launch; }
        let groups = match self.event_store.load_run_markers().await {
            Ok(g) => g,
            Err(e) => { tracing::warn!(error = %e, "resume scan failed; skipping resume"); return launch; }
        };
        // `pending` is deliberately OVER-inclusive: every session the scan will VISIT (not only
        // the ones a marker-only slice already calls Interrupted). An `Unanswered` verdict is
        // invisible to a marker slice (T7 reads it from a bounded tail inside the task), so a
        // narrower set would release a survivor into a session the scan is about to act on.
        let mut seen: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
        let group_count = groups.len();
        for (i, (sid, slice)) in groups.into_iter().enumerate() {
            seen.insert(sid.clone());
            if !has_own_scheduler(&sid) { launch.pending.insert(sid.to_key_string()); }
            let (me, sem) = (Arc::clone(self), Arc::clone(&self.semaphore));
            launch.tasks.spawn(async move {
                let mut part = ResumeReport::default();
                match sem.acquire_owned().await {
                    Ok(_permit) => me.resume_from_markers(&sid, &slice, &mut part).await,
                    Err(e) => part.refused.push((sid, ResumeRefusal::RetriggerFailed(format!("resume semaphore closed: {e}")))),
                }
                (i, part)
            });
        }
        // T7's activity window: sessions with NO markers at all (a seed that never reached RunStarted).
        let active_minutes = u32::try_from(self.config.max_age_secs.div_ceil(60).max(1)).unwrap_or(u32::MAX);
        match self.session_store.list_sessions(crate::gateway::session_store::types::SessionFilter { active_minutes: Some(active_minutes), ..Default::default() }).await {
            Ok(rows) => for (j, meta) in rows.into_iter().enumerate() {
                let Some(id) = SessionId::from_key_string(&meta.key) else { continue };
                if seen.contains(&id) || !unanswered_eligible(&id) { continue; }
                launch.pending.insert(id.to_key_string());
                let (me, sem) = (Arc::clone(self), Arc::clone(&self.semaphore));
                let ordinal = group_count + j;
                launch.tasks.spawn(async move {
                    let mut part = ResumeReport::default();
                    match sem.acquire_owned().await {
                        Ok(_permit) => {
                            let Some(_slot) = me.try_claim_resume(&id) else { part.busy += 1; return (ordinal, part); };
                            if me.check_unanswered(&id, &[], &mut part).await { part.scanned += 1; }
                        }
                        Err(e) => part.refused.push((id, ResumeRefusal::RetriggerFailed(format!("resume semaphore closed: {e}")))),
                    }
                    (ordinal, part)
                });
            },
            Err(e) => tracing::warn!(error = %e, "resume: activity-window listing failed; marker-less unanswered sessions not scanned"),
        }
        launch
    }
    pub async fn resume_interrupted_runs(self: &Arc<Self>) -> ResumeReport { self.launch_resume().await.settle().await }
}
impl ResumeReport {
    pub fn absorb(&mut self, o: Self) {
        self.scanned += o.scanned; self.resumed += o.resumed; self.abandoned += o.abandoned; self.skipped += o.skipped;
        self.delegated += o.delegated; self.busy += o.busy; self.skipped_unknown_age += o.skipped_unknown_age;
        self.contradictions += o.contradictions; self.degraded += o.degraded; self.unsnapshotted += o.unsnapshotted;
        self.refused.extend(o.refused);   // T16 adds `self.notified += o.notified;` when it adds the field
    }
}
```
`resume_session`: `let _permit = self.semaphore.clone().acquire_owned().await.map_err(|e| SessionError::Other(format!("resume semaphore closed: {e}")))?;` before `resume_from_markers`; `retrigger` loses its `acquire_owned` + `drop(permit)` (the permit now spans repair + retrigger, held by the two entry points). Comment at `:764-775` ("The boot scan never exposed this (it walks sessions in a sequential loop)") → "the boot scan fans out but claims one slot per session, so two candidates never collide; `busy` still counts an on-demand resume racing the scan". `resume.rs`: `default_resume_max_concurrent() -> 2`, doc "default: 2". `durable.rs`: `for payload in survivors { … if !select(&session_key) { continue; } …}` after the key parse. Boot:
```rust
let launch = coordinator.launch_resume().await;
let pending = launch.pending.clone();
let scan = tokio::spawn(launch.settle());
let early = reinject_survivors(a.clone(), r.clone(), b.clone(), cfg.clone(), &|k| !pending.contains(&k.to_key_string())).await;
let report = scan.await.unwrap_or_default();
tracing::info!(scanned = report.scanned, resumed = report.resumed, abandoned = report.abandoned, skipped = report.skipped,
    "ResumeCoordinator boot scan finished");   // T16 adds `notified = report.notified`
let late = reinject_survivors(a, r, b, cfg, &|k| pending.contains(&k.to_key_string())).await;
```
(the `!auto_scan` branch calls once with `&|_| true`).

- [ ] **Step 4: Run, expect PASS** — the integration binary (`-j 1`) + `cargo test -p alephcore --lib -- gateway::busy_queue config::types::resume` + `cargo test -p alephcore --bins`.
- [ ] **Step 5: Mutation check**: `Semaphore::new(permits)` → `Semaphore::new(1)` ⇒ `max_in_flight == 2` RED and the elapsed bound RED; move `sem.acquire_owned()` back into `retrigger` while keeping the fan-out permit ⇒ the test hangs at `max_concurrent = 1` — run it once with `cfg.max_concurrent = 1` in a scratch copy to see the deadlock the two-acquire shape produces.
- [ ] **Step 6: Commit**: `git add src/config/types/resume.rs src/gateway/resume_coordinator.rs tests/resume_coordinator_integration.rs` → `resume: fan the boot scan out under a JoinSet bounded by [resume] max_concurrent`; `git add src/gateway/busy_queue/durable.rs src/bin/aleph-server/commands/start/mod.rs` → `gateway: re-deliver queued survivors around the resume scan instead of after it`. Trailer as above.

---

### Task 16: `orphan_notice.rs` CUT; the notice arm moves into `ResumeCoordinator`

**Files:**
- Delete: `src/gateway/orphan_notice.rs`; Modify: `src/gateway/mod.rs:42`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1346-1375` (keep `reconcile_orphaned_tasks` + the per-task `warn!`; delete the notify call), `start/mod.rs:3030` (`.with_state_database`)
- Modify: `src/resilience/database/migration.rs` (new `migrate_add_agent_tasks_adjudicated_at`), `state_database/mod.rs:136-139,199-202` (both invocation lists), `state_database/schema.rs:194-218` (`adjudicated_at_ms INTEGER` in the CREATE), `tasks.rs` (two methods)
- Modify: `src/gateway/resume_coordinator.rs` (`state_database` field + builder, `system_note`, `adjudicate_orphaned_tasks`, `abandon`, `announce_degrade`, `ResumeReport::notified`)
- Test: `tasks.rs` tests, `tests/resume_coordinator_integration.rs` (the `messages`-writer census lives in T17, which lands immediately after this task — a census that is red across commits is a broken tree)
- Modify (after T15): `src/gateway/resume_coordinator.rs` `ResumeReport::absorb` gains `self.notified += o.notified;`; `ResumeLaunch::settle` gains `self.me.adjudicate_orphaned_tasks(&mut self.report).await;` before its log line; `start/mod.rs`'s boot log line gains `notified = report.notified`

**Interfaces:**
- Consumes: `StateDatabase::reconcile_orphaned_tasks` (`tasks.rs:353`), `AgentTask { id, parent_session_id, task_prompt, created_at (unix s), lane, .. }` (`types.rs:196`), `MessageProjector::request_repair` (`session_projector.rs:294`), `global_message_projector()` (`:89`), `reduce_run`, A2's `RunDisposition::Unanswered`.
- Produces: `StateDatabase::unadjudicated_interrupted_tasks(&self, since_secs: i64) -> Result<Vec<AgentTask>, AlephError>` (`status='interrupted' AND lane='main' AND adjudicated_at_ms IS NULL AND created_at >= ?`), `StateDatabase::mark_task_adjudicated(&self, task_id: &str, at_ms: i64) -> Result<(), AlephError>`; `ResumeCoordinator::with_state_database(self, db: Arc<StateDatabase>) -> Self`; `ResumeReport.notified: usize` (renderer: the boot log line in `start/mod.rs`, T15); `pub fn lost_input_notice(created_at_secs: i64, prompt: &str) -> String`.

**Facts settled by reading.** The `agent_tasks` row DOES carry the input: `task_prompt = request.input` (`persistence.rs:33-38`), plus `created_at` in unix **seconds** taken before the seed (`execute.rs:482` runs before the orchestrator seeds), `parent_session_id` = the wire key, `metadata_json` = `{run_id, session_key, channel_id, sender_id, conversation_id, source}`. `id` is the gateway `RunRequest.run_id`, which is NOT the `RunStarted.run_id` (A5) — so "no events for that task" is derived by time, not by id: a `UserMessage` with `created_at_ms >= task.created_at * 1000` means the seed reached the log. A resume request has `input: ""` (`retrigger`), so an empty `task_prompt` is never a lost message. Column name is `adjudicated_at_ms`, not `notified_at`: it is stamped in all three arms (a: nothing to say, b: notice written, c: closed by `abandon`), and a column named "notified" that is set when nothing was notified is a #17 label lie.

- [ ] **Step 1: Write the failing tests**

tasks.rs:
```rust
#[tokio::test]
async fn unadjudicated_interrupted_tasks_are_listed_once() {
    let db = StateDatabase::in_memory().unwrap();
    insert_with_status(&db, "run-1", TaskStatus::Running).await;
    db.reconcile_orphaned_tasks().await.unwrap();
    // `task()` builds lane Subagent; flip one to Main the way the engine does
    db.with_conn(|c| Ok(c.execute("UPDATE agent_tasks SET lane='main'", []).map(|_| ())?)).await.unwrap();
    assert_eq!(db.unadjudicated_interrupted_tasks(0).await.unwrap().len(), 1);
    db.mark_task_adjudicated("run-1", 5).await.unwrap();
    assert!(db.unadjudicated_interrupted_tasks(0).await.unwrap().is_empty(), "stamped rows drop out");
    assert!(db.unadjudicated_interrupted_tasks(i64::MAX).await.unwrap().is_empty(), "the window bounds it");
}
```
integration (fixtures as T15; `state_db = Arc::new(StateDatabase::in_memory()?)` with a Main-lane `running` task for session `s` whose `task_prompt = "buy milk"`; log for `s` holds only `SessionWoken`):
```rust
#[tokio::test]
async fn a_task_whose_seed_never_landed_gets_exactly_one_resend_notice_across_two_boots() {
    let (store, db, sid) = orphan_fixture("buy milk").await;
    db.reconcile_orphaned_tasks().await.unwrap();
    let boot = || Arc::new(ResumeCoordinator::new(store.clone(), ResumeConfig::default(), adapter(), registry(), sessions(), test_bus())
        .with_state_database(db.clone()));
    assert_eq!(boot().resume_interrupted_runs().await.notified, 1);
    assert_eq!(boot().resume_interrupted_runs().await.notified, 0, "idempotent across boots");
    let notes: Vec<String> = store.load_all_events(&sid).await.unwrap().into_iter()
        .filter_map(|r| match r.event { SessionEvent::SystemMessage { content, .. } => Some(content), _ => None }).collect();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("lost before it was recorded") && notes[0].contains("buy milk") && notes[0].contains("re-send"), "{}", notes[0]);
}
#[tokio::test]
async fn a_task_whose_session_has_an_open_run_is_left_to_the_resume_arm() { /* seed_interrupted_run(&store,&sid) first ⇒ notified == 0, resumed == 1, and the row IS stamped */ }
#[tokio::test]
async fn an_abandoned_run_gets_a_system_note_after_its_closer() { /* max_age_secs: 0 ⇒ abandoned == 1; last two events are RunFinished{Abandoned} then SystemMessage containing "abandoned" */ }
```
**(e) the `is_resume()` guard, as a test** — a retriggered run's `tasks` row has `task_prompt = ""` (`retrigger` sends `input: String::new()`), and the ratchet stage leaves such rows with NO events every boot; they must be adjudicated silently, never as "please re-send":
```rust
#[tokio::test]
async fn a_resume_retrigger_row_with_an_empty_prompt_is_adjudicated_without_a_notice() {
    let (store, db, _sid) = orphan_fixture("").await;          // empty prompt = the retrigger's shape
    db.reconcile_orphaned_tasks().await.unwrap();
    let boot = Arc::new(ResumeCoordinator::new(store.clone(), ResumeConfig::default(), adapter(), registry(), sessions(), test_bus()).with_state_database(db.clone()));
    assert_eq!(boot.resume_interrupted_runs().await.notified, 0);
    assert!(db.unadjudicated_interrupted_tasks(0).await.unwrap().is_empty(), "stamped, so it is not re-examined every boot");
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- resilience::database::tasks` → `no method named unadjudicated_interrupted_tasks`; the integration binary → `no method named with_state_database`.

- [ ] **Step 3: Minimal implementation**

migration.rs — copy `migrate_add_group_chat_owner` (`:559-604`) verbatim with `agent_tasks` / `adjudicated_at_ms` / savepoint `migration_agent_tasks_adjudicated`; add the column to the CREATE in `schema.rs` and the call to both lists in `state_database/mod.rs`. tasks.rs:
```rust
pub async fn unadjudicated_interrupted_tasks(&self, since_secs: i64) -> Result<Vec<AgentTask>, AlephError> {
    self.with_conn(move |conn| {
        let mut stmt = conn.prepare("SELECT id, parent_session_id, agent_id, task_prompt, status, risk_level, lane, \
            checkpoint_snapshot_path, last_tool_call_id, recursion_depth, parent_task_id, created_at, updated_at, \
            started_at, completed_at, metadata_json FROM agent_tasks WHERE status = 'interrupted' AND lane = 'main' \
            AND adjudicated_at_ms IS NULL AND created_at >= ?1 ORDER BY created_at ASC")
            .map_err(|e| AlephError::config(format!("Failed to prepare unadjudicated query: {e}")))?;
        let rows = stmt.query_map(params![since_secs], agent_task_from_row)
            .map_err(|e| AlephError::config(format!("Failed to run unadjudicated query: {e}")))?
            .collect::<Result<Vec<_>, _>>().map_err(|e| AlephError::config(format!("Failed to collect: {e}")))?;
        Ok(rows)
    }).await
}
pub async fn mark_task_adjudicated(&self, task_id: &str, at_ms: i64) -> Result<(), AlephError> {
    let id = task_id.to_string();
    self.with_conn(move |conn| {
        conn.execute("UPDATE agent_tasks SET adjudicated_at_ms = ?1 WHERE id = ?2", params![at_ms, id])
            .map_err(|e| AlephError::config(format!("Failed to stamp adjudicated_at_ms: {e}")))?;
        Ok(())
    }).await
}
```
resume_coordinator.rs:
```rust
pub fn with_state_database(mut self, db: Arc<crate::resilience::StateDatabase>) -> Self { self.state_database = Some(db); self }

/// The one way this coordinator tells the user something in-band: a SystemMessage appended
/// at the head, then the projector asked to paint it now rather than at the next boot.
async fn system_note(&self, session_id: &SessionId, content: String) -> Result<(), SessionError> {
    let seq = self.next_seq(session_id).await?;
    let ev = SessionEvent::SystemMessage { turn_id: TurnId::new_v4(), content, at: now_ms() };
    self.event_store.append(session_id, seq, &ev, now_ms()).await?;
    if let Some(p) = crate::gateway::session_projector::global_message_projector() { p.request_repair(session_id).await; }
    Ok(())
}

pub fn lost_input_notice(created_at_secs: i64, prompt: &str) -> String {
    let when = chrono::DateTime::from_timestamp(created_at_secs, 0).map_or_else(|| created_at_secs.to_string(), |t| t.to_rfc3339());
    let head: String = prompt.chars().take(80).collect();
    format!("A message you sent at {when} was lost before it was recorded: «{head}». Please re-send it.")
}

async fn adjudicate_orphaned_tasks(&self, report: &mut ResumeReport) {
    let Some(db) = self.state_database.as_ref() else { return };
    let since = (now_ms() / 1000).saturating_sub(self.config.max_age_secs as i64);
    let rows = match db.unadjudicated_interrupted_tasks(since).await {
        Ok(r) => r, Err(e) => { tracing::warn!(error = %e, "resume: orphaned task rows unreadable"); return; }
    };
    for task in rows {
        let Some(key) = SessionId::from_key_string(&task.parent_session_id) else {
            tracing::warn!(task_id = %task.id, "resume: orphaned task has an unparseable session key"); continue;
        };
        // A refused / unreadable log is not adjudicated: the doctor is its exit and the window bounds the retries.
        let Ok(events) = self.event_store.load_all_events(&key).await else { continue };
        let Ok(reduction) = reduce_run(&events) else { continue };
        let open = !matches!(reduction.disposition, RunDisposition::Clean);
        let seeded = events.iter().any(|r| matches!(r.event, SessionEvent::UserMessage { .. }) && r.created_at_ms >= task.created_at.saturating_mul(1000));
        if !open && !seeded && !task.task_prompt.trim().is_empty() {
            match self.system_note(&key, lost_input_notice(task.created_at, &task.task_prompt)).await {
                Ok(()) => report.notified += 1,
                Err(e) => { tracing::warn!(task_id = %task.id, error = %e, "resume: lost-input notice append failed"); continue; }
            }
        }
        if let Err(e) = db.mark_task_adjudicated(&task.id, now_ms()).await { tracing::warn!(task_id = %task.id, error = %e, "resume: adjudicated stamp failed"); }
    }
}
```
`abandon()`: after the `RunFinished{Abandoned}` append succeeds, `let _ = self.system_note(session_id, format!("The run interrupted by a restart was abandoned ({reason}); nothing after its last recorded step was done.")).await;` (best-effort, logged). `announce_degrade` body → `if let Err(e) = self.system_note(session_id, note.sentence.clone()).await { warn }`. `ResumeReport` gains `pub notified: usize` (doc: "8.2(b) lost-input notices written this pass"). `agent_init/mod.rs`: delete lines 1359-1370 (the `notified` call + its `info!` collapses to `tracing::info!(count = orphans.len(), "Reconciled orphaned agent tasks")`); `start/mod.rs`: `ResumeCoordinator::new(..)` → chain `.with_state_database(db)` when `agent_result.state_db` is `Some` (clone it beside `resume_collaborators`). Delete `orphan_notice.rs` and `pub mod orphan_notice;`.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- resilience::database gateway::session_projector gateway::resume_coordinator`, the integration binary, `cargo check -p alephcore --bins`.
- [ ] **Step 5: Mutation check** (§11 ledger 8.2 — the `orphan_notice` resurrection mutation is T17's, where the census lives): flip `!seeded` → `seeded` ⇒ `a_task_whose_seed_never_landed…` RED (`notified == 0`).
- [ ] **Step 6: Commit** (two): `git add src/resilience/database/migration.rs src/resilience/database/state_database/mod.rs src/resilience/database/state_database/schema.rs src/resilience/database/tasks.rs` → `resilience: adjudicated_at_ms on agent_tasks so a boot notice is written once`; `git add src/gateway/resume_coordinator.rs src/gateway/mod.rs src/bin/aleph-server/commands/start/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs tests/resume_coordinator_integration.rs` + `git rm src/gateway/orphan_notice.rs` → `gateway: fold the orphan-task notice into the resume arm and cut the second messages writer`. Trailer as above.

---

### Task 17: four lying comments → facts; "nine sites" → census; dead `AgentInstance` writers deleted

**Files:**
- Modify: `src/bin/aleph-server/commands/start/helpers.rs:327-334`; `src/gateway/projection_reconciler.rs:29-31`; `src/session/service.rs:67-92` (+ tests)
- Modify: `src/gateway/agent_instance.rs:452-500` (`add_message`, `add_message_with_run_id`), `:543-582` (`reset_session`), tests `:1015-1060` (the two tests that call them); `src/gateway/continuation_lifecycle.rs:830-840` (census floor)
- Test: `service.rs` `#[cfg(test)] mod tests`; `src/gateway/session_projector.rs` tests (the `messages`-writer census, moved here from T16)

**Interfaces:**
- Produces: `pub(crate) const SESSION_SERVICE_READERS: &[(&str, &str)]` in `service.rs` (file, what a missing handle reads as there).

Callers counted first (comments stripped): `AgentInstance::add_message*` — 0 production, 2 tests (`agent_instance.rs:1023,1054`); `AgentInstance::reset_session` — 0 production, 1 test (`:1032`); `build_message_metadata` stays (`session_projector.rs:791` reads it). `global_session_service()` — 9 sites in 7 files at `5e85060b8` (`compact_tool.rs`, `execute.rs`, `fast_path.rs`×2, `run_loop/inner.rs`, `simple.rs`×2, `openai_api/completions/agent.rs`, `tools/scoped/dispatch.rs`). By this task's turn T8 moved the fast-path reader into `slash_command.rs` (`fast_path.rs` has none), T9 added `run_loop/mod.rs` (`journal_hook_stop`), and T11 moved `dispatch.rs`'s reader into `session/call_log.rs`. **Re-derive the set on the commit** (`rg -n 'global_session_service\(\)' src --glob '!**/tests.rs'`, comments stripped) — the list below is the EXPECTED set; the test is the instrument. Deleting `AgentInstance::reset_session` removes `agent_instance.rs` from `continuation_lifecycle.rs`'s epoch-seam census (its `self.session_store.reset_session(` site counts as non-self) — the floor `with_markers.len() >= 8 && sites >= 12` (`:834`) goes RED; re-measure and lower to 7/11 in the same commit, stating why.

- [ ] **Step 1: Write the failing tests**

session_projector.rs tests (the census that goes RED if `orphan_notice.rs` is restored):
```rust
#[test]
fn the_projector_is_the_only_production_writer_of_the_messages_table() {
    use crate::utils::source_scan::{production_text, rust_sources_under, strip_comment_lines};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let writers: std::collections::BTreeSet<String> = rust_sources_under(&root.join("src")).into_iter()
        // the store's own tree defines and forwards the method; it originates no row
        .filter(|(rel, _)| !rel.starts_with("src/gateway/session_store/"))
        .filter(|(rel, text)| strip_comment_lines(&production_text(&root.join(rel), text)).contains(".append_message("))
        .map(|(rel, _)| rel).collect();
    assert_eq!(writers, std::collections::BTreeSet::from(["src/gateway/session_projector.rs".to_string()]),
        "a second `messages` writer bypasses the SSOT: route it through the event log and `request_repair`");
}
```
(Measured at `5e85060b8` the set is `{orphan_notice.rs, agent_instance.rs, session_projector.rs}` — T16 removed the first, this task removes the second, so the census is green on this task's commit. Mutation (§11 ledger 8.2): `git checkout 5e85060b8 -- src/gateway/orphan_notice.rs` + restore `pub mod orphan_notice;` ⇒ RED naming the file; restore.)

(service.rs)
```rust
#[test]
fn the_reader_census_matches_the_tree_in_both_directions() {
    use crate::utils::source_scan::{production_code_lines, rust_sources_under};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found: std::collections::BTreeSet<String> = rust_sources_under(&root).into_iter()
        .filter(|(rel, _)| rel != "src/session/service.rs")
        .filter(|(_, t)| production_code_lines(t).contains("global_session_service()"))
        .map(|(rel, _)| rel).collect();
    let listed: std::collections::BTreeSet<String> = SESSION_SERVICE_READERS.iter().map(|(f, _)| (*f).to_string()).collect();
    assert_eq!(found, listed, "a reader of the session-service handle appeared or vanished; update SESSION_SERVICE_READERS with what a missing handle reads as there");
}
```
- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::service::tests gateway::session_projector::tests::the_projector_is_the_only` → `cannot find value SESSION_SERVICE_READERS`; census: `assertion failed … {"src/gateway/agent_instance.rs", "src/gateway/session_projector.rs"}`.
- [ ] **Step 3: Minimal implementation**
```rust
/// Every production reader of the handle and what an uninstalled handle reads as THERE.
/// Not a count: the number this used to quote rotted twice. Pinned to the tree by test.
pub(crate) const SESSION_SERVICE_READERS: &[(&str, &str)] = &[
    ("src/builtin_tools/sessions/compact_tool.rs", "an AlephError to the model"),
    ("src/gateway/execution_engine/execute.rs", "a dropped session_events append; the projector never sees it"),
    ("src/gateway/execution_engine/slash_command.rs", "the L0 fast path runs unjournaled (T8: warn + `aleph doctor`)"),
    ("src/gateway/execution_engine/run_loop/mod.rs", "a BeforeAgentStart stop is not journaled (T9: warn)"),
    ("src/gateway/execution_engine/run_loop/inner.rs", "the legacy event-log backfill is skipped"),
    ("src/gateway/execution_engine/simple.rs", "a dropped session_events append (two sites)"),
    ("src/gateway/openai_api/completions/agent.rs", "a dropped session_events append"),
    ("src/session/call_log.rs", "the park / approval decision is not persisted (T11: warn + `aleph doctor`)"),
];
```
The static's doc becomes: "`ConsumerDecides`: the readers do not converge — see [`SESSION_SERVICE_READERS`] for each one's reading; the list is pinned to the tree by `the_reader_census_matches_the_tree_in_both_directions`." Comment rewrites (`start/mod.rs:391` was already corrected by T4): `helpers.rs:327-334` → "Opens a dedicated connection … Returns `None` on any failure — then no event log exists for this run, `decline_global_session_service` says so, and the `messages` projection has nothing to project from" (drop "dual-write"/"authoritative"); `projection_reconciler.rs:29-31` → "sessions that emit no run markers — background sub-agent sessions (`sub-bg-*`). Cron and heartbeat DO emit them (the bridge writes `RunStarted` unconditionally, `runner_impl.rs`); `has_own_scheduler` closes theirs." Delete `add_message`, `add_message_with_run_id`, `reset_session` and their tests; keep `build_message_metadata`. `continuation_lifecycle.rs:834`: `>= 7 && sites >= 11` with a one-line note "(2026-09-12: `AgentInstance::reset_session`, zero callers, deleted)".
- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::service gateway::agent_instance gateway::continuation_lifecycle gateway::session_projector` + `cargo check -p alephcore --bins`.
- [ ] **Step 5: Mutation check** (§11 ledger 8.2): `git checkout 5e85060b8 -- src/gateway/orphan_notice.rs` + restore `pub mod orphan_notice;` in `src/gateway/mod.rs` ⇒ `the_projector_is_the_only_production_writer_of_the_messages_table` RED naming the file. Restore.
- [ ] **Step 6: Commit** (three): `git add src/session/service.rs` → `session: replace the hard-coded reader count with a tree-pinned census`; `git add src/bin/aleph-server/commands/start/helpers.rs src/gateway/projection_reconciler.rs` → `docs: correct two comments that described behaviour the code does not have`; `git add src/gateway/agent_instance.rs src/gateway/continuation_lifecycle.rs src/gateway/session_projector.rs` → `gateway: delete AgentInstance message writers that had no production caller`. Trailer as above.

---

### Task 5a: `session_usage_totals` fold; `AssistantRunMeta` drops its token counters

**Files:**
- Create: `src/session/usage_fold.rs`; Modify: `src/session/mod.rs:10-22` (`pub mod usage_fold;`)
- Modify: `src/session/events.rs:329-360` (delete `input_tokens`, `output_tokens`; rewrite the doc), `src/gateway/execution_engine/helpers.rs:51-79` (delete the two fields), `:395-420` (literal), `src/gateway/execution_engine/execute.rs:966-999` (literal ×2), `src/gateway/session_projector.rs:769-846` (meta arm bills from the fold), tests `:1165-1192`, `:1423-1466`, `:1534-1617`
- Modify (compile-only, drop two fields): `src/gateway/agent_instance.rs:1060-1068`, `:1113-1122`; `src/session/reduction.rs:764-776`; `src/agents/subagent_spawner/fork/tests.rs` (now the shared fixture in `events.rs`)
- Test: `usage_fold.rs` own `#[cfg(test)] mod tests`; projector tests

**Interfaces:**
- Consumes: `TokenBreakdown { input, output, cache_read, cache_creation, reasoning }` (`orchestrator/dispatch.rs:349`), `SessionStore::update_session_usage` (`session_store/mod.rs:507`), `ProjectionCtx { events, run_start, .. }` (`session_projector.rs:666`).
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
  pub struct UsageTotals { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_creation: u64, pub reasoning: u64, pub with_usage: usize, pub without_usage: usize }
  pub fn session_usage_totals(log: &[SessionEventRecord]) -> UsageTotals
  pub fn run_usage_totals(slice: &[SessionEventRecord]) -> Option<UsageTotals>   // anchored on the LAST RunStarted in `slice`; None = unanchored
  SessionEvent::AssistantRunMeta { turn_id, run_id, context_tokens, context_window, total_tokens, cost_usd, model, model_provider, at }
  RunContextOccupancy { context_tokens, context_window, total_tokens, cost_usd, model, model_provider }
  ```

- [ ] **Step 1: Write the failing tests**

`usage_fold.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, RunOutcome, SessionEvent, SessionEventRecord};
    use crate::orchestrator::dispatch::TokenBreakdown;
    fn rec(seq: u64, event: SessionEvent) -> SessionEventRecord { SessionEventRecord { seq, event, created_at_ms: 0 } }
    fn asst(usage: Option<TokenBreakdown>) -> SessionEvent {
        SessionEvent::AssistantMessage { turn_id: uuid::Uuid::new_v4(), content: MessageContent { text: "a".into(), blocks: vec![], thinking: None, thinking_signature: None }, usage, at: 0 }
    }
    fn tb(input: u32, output: u32) -> TokenBreakdown { TokenBreakdown { input, output, cache_read: 0, cache_creation: 0, reasoning: 0 } }
    fn rs(id: &str) -> SessionEvent { SessionEvent::RunStarted { run_id: id.into(), at: 0, project_root: None, envelope: None } }
    fn rf(id: &str) -> SessionEvent { SessionEvent::RunFinished { run_id: id.into(), outcome: RunOutcome::Completed, at: 0 } }

    #[test]
    fn session_totals_sum_every_priced_message_and_count_the_unpriced() {
        let log = [rec(1, rs("a")), rec(2, asst(Some(tb(100, 10)))), rec(3, asst(None)), rec(4, rf("a")), rec(5, rs("b")), rec(6, asst(Some(tb(5, 1))))];
        let t = session_usage_totals(&log);
        assert_eq!((t.input, t.output, t.with_usage, t.without_usage), (105, 11, 2, 1));
    }
    #[test]
    fn run_totals_anchor_on_the_last_run_started() {
        let log = [rec(1, rs("a")), rec(2, asst(Some(tb(100, 10)))), rec(3, rf("a")), rec(4, rs("b")), rec(5, asst(Some(tb(5, 1))))];
        assert_eq!(run_usage_totals(&log).map(|t| (t.input, t.output)), Some((5, 1)));
        assert_eq!(run_usage_totals(&log[1..3]), None, "no RunStarted in the slice ⇒ refuse rather than bill the whole slice");
    }
}
```
Projector: rewrite `replaying_one_run_meta_bills_once` (:1567) to pin an event store — `own` in-memory `SqliteEventStore` seeded `[(1, RunStarted r_a), (2, assistant_msg_billed(tid, 45, 25)), (3, RunFinished), (4, run_meta(tid, "run_a"))]`, `ctx.events: Some(&events)`, `run_start: 1`; assertions unchanged (`(45, 25)`, second pass `Nothing`). `run_meta(tid, run)` fixture loses its two token args; `projector_stamps_run_meta_on_assistant_row` and `a_run_meta_with_no_row_in_range_defers_and_does_not_bill` get the same seeded log. New:
```rust
#[tokio::test]
async fn an_unanchored_meta_stamps_but_does_not_bill() {
    // events pinned, but the slice [0, meta) holds NO RunStarted ⇒ Stamped { billed: false } and session counters stay 0.
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- session::usage_fold gateway::session_projector::tests` → `E0433 could not find usage_fold` (then, after the module exists, the projector test asserts `(45, 25)` but gets `(0, 0)` because the meta no longer carries tokens).

- [ ] **Step 3: Minimal implementation**

`usage_fold.rs`:
```rust
//! Token totals folded from `AssistantMessage.usage` — the ONE derivation of
//! "what this session / run spent" (§5.5). `AssistantRunMeta` no longer carries
//! counters; it keeps only what a fold cannot give (run_id join, occupancy,
//! cost, model). Discarded-retry calls are not in the log and are not counted
//! here; the per-call `SpendLedger` is the other fact and is untouched.
use crate::session::events::{SessionEvent, SessionEventRecord};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub input: u64, pub output: u64, pub cache_read: u64, pub cache_creation: u64, pub reasoning: u64,
    /// Assistant messages whose provider reported usage.
    pub with_usage: usize,
    /// Assistant messages with `usage: None` — absent, NOT zero. A consumer that
    /// shows a total with `without_usage > 0` is showing a floor, and must say so.
    pub without_usage: usize,
}

impl UsageTotals {
    fn fold(mut self, event: &SessionEvent) -> Self {
        if let SessionEvent::AssistantMessage { usage, .. } = event {
            match usage {
                Some(u) => {
                    self.input += u64::from(u.input); self.output += u64::from(u.output);
                    self.cache_read += u64::from(u.cache_read); self.cache_creation += u64::from(u.cache_creation);
                    self.reasoning += u64::from(u.reasoning); self.with_usage += 1;
                }
                None => self.without_usage += 1,
            }
        }
        self
    }
}

#[must_use]
pub fn session_usage_totals(log: &[SessionEventRecord]) -> UsageTotals {
    log.iter().fold(UsageTotals::default(), |acc, r| acc.fold(&r.event))
}

/// Totals of the run opened by the LAST `RunStarted` in `slice`. `None` when
/// the slice holds none: an unanchored fold would bill a whole session to one run.
#[must_use]
pub fn run_usage_totals(slice: &[SessionEventRecord]) -> Option<UsageTotals> {
    let start = slice.iter().rposition(|r| matches!(r.event, SessionEvent::RunStarted { .. }))?;
    Some(slice[start..].iter().fold(UsageTotals::default(), |acc, r| acc.fold(&r.event)))
}
```
`events.rs:329-360`: delete `input_tokens`/`output_tokens`; doc → `/// Stamped after the run. Carries what the usage fold (\`session::usage_fold\`) cannot derive: the run_id join, context-window occupancy, the priced cost and the serving model. Token counters were removed 2026-09-12 — they are folded from \`AssistantMessage.usage\`.` (old rows still deserialize: serde ignores unknown fields by default on this enum — verify `deny_unknown_fields` is absent, it is at :225-229).
`helpers.rs`: delete the two fields + their docs; `:405` literal drops them (keep the `spent` gate — `cost_usd` still rides). `execute.rs:970-975,984-995`: drop the two fields.
`session_projector.rs:769-846` meta arm: `input_tokens, output_tokens` leave the pattern; the `Stamped` arm becomes `Projected::Stamped { billed: bill_run_from_fold(id, rec.seq, ctx, run_id, *cost_usd, model.as_deref(), model_provider.as_deref()).await }` with:
```rust
/// Accumulate the run's spend onto the session row from the usage fold —
/// exactly once, guarded by the stamp that just landed. `false` = not billed,
/// which is "I could not tell", never "it was zero".
async fn bill_run_from_fold(id: &SessionId, meta_seq: EventSeq, ctx: &ProjectionCtx<'_>, run_id: &str, cost_usd: Option<f64>, model: Option<&str>, provider: Option<&str>) -> bool {
    let Some(events) = ctx.events else { tracing::warn!(session = ?id, run_id, "projector: no event log; run spend not accumulated"); return false; };
    let slice = match events.load_events_range(id, Some(ctx.run_start), Some(meta_seq)).await {
        Ok(s) => s, Err(e) => { tracing::warn!(session = ?id, error = %e, "projector: usage fold read failed"); return false; }
    };
    let Some(totals) = crate::session::usage_fold::run_usage_totals(&slice) else {
        tracing::warn!(session = ?id, run_id, "projector: run meta with no RunStarted before it; spend not accumulated"); return false;
    };
    if totals.input == 0 && totals.output == 0 && cost_usd.is_none() { return false; }
    match ctx.store.update_session_usage(id, i64::try_from(totals.input).unwrap_or(i64::MAX), i64::try_from(totals.output).unwrap_or(i64::MAX), cost_usd.unwrap_or(0.0), model, provider).await {
        Ok(()) => true,
        Err(e) => { tracing::warn!(error = %e, "projector: session usage accumulation failed"); false }
    }
}
```
(`ctx.run_start == 0` ⇒ the slice starts at the log head and `run_usage_totals` anchors on the last `RunStarted` it finds — the restarted-drain case.)

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- session::usage_fold gateway::session_projector gateway::agent_instance session::reduction gateway::projection_reconciler` + `cargo check -p alephcore --bins`
- [ ] **Step 6: Commit** — `git add src/session/usage_fold.rs src/session/mod.rs src/session/events.rs src/gateway/execution_engine/helpers.rs src/gateway/execution_engine/execute.rs src/gateway/session_projector.rs src/gateway/agent_instance.rs src/session/reduction.rs` · `session: fold session spend from AssistantMessage.usage; AssistantRunMeta drops its counters` + Claude-Session line.

---

### Task 5b: the reconciler synthesizes the stamp for a run whose meta never landed

**Files:**
- Modify: `src/gateway/session_projector.rs:117-150` (`RepairReport.stamps_synthesized`), `:475-590` (`heal_session` collects run spans; post-walk synthesis, `WholeSession` only), `src/gateway/projection_reconciler.rs:56-78,108-138` (`ReconcileReport.stamps_synthesized` + log), `src/bin/aleph-server/commands/start/mod.rs:3008-3018` (log)
- Test: `projection_reconciler.rs` tests (helpers `own_event_store/temp_file_store/assistant/append_all/reconciler` at :232-330)

**Interfaces:**
- Consumes: T5a `run_usage_totals`; `build_message_metadata(Some(run_id), None)` (`agent_instance.rs:188`); `SessionStore::stamp_assistant_metadata_in_range` (`session_store/mod.rs:489`), `StampOutcome`.
- Produces: `RepairReport { .., pub stamps_synthesized: usize }`, `ReconcileReport { .., pub stamps_synthesized: usize }`.

- [ ] **Step 1: Write the failing test**
```rust
/// #11: the run finished, the process died before `AssistantRunMeta` — the
/// session's counters under-counted forever. Boot folds the run's own messages.
#[tokio::test]
async fn a_finished_run_with_no_meta_is_billed_from_its_messages_at_boot() {
    let event_store = own_event_store();
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(SessionManager::new(SessionManagerConfig { db_path: temp.path().join("nometa.db"), ..Default::default() }).unwrap());
    let session_store: Arc<dyn SessionStore> = manager.clone();
    let id = SessionKey::ephemeral("nometa");
    session_store.get_or_create(&id).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    append_all(&event_store, &id, &[
        (1, SessionEvent::RunStarted { run_id: "r1".into(), at: 1, project_root: None, envelope: None }),
        (2, SessionEvent::TurnStarted { turn_id: tid, trigger: TurnTrigger::UserMessage, at: 2 }),
        (3, assistant(tid, 300, 40, 3)),
        (4, SessionEvent::RunFinished { run_id: "r1".into(), outcome: RunOutcome::Completed, at: 4 }),
    ]).await;
    let r = reconciler(&event_store, &session_store).reconcile_candidates().await;   // reconciler() gains a `None` registrar arg from T4
    assert_eq!((r.stamps_synthesized, r.usage_rebilled), (1, 1));
    let meta = session_store.get_metadata(&id).await.unwrap().unwrap();
    assert_eq!((meta.input_tokens, meta.output_tokens), (300, 40));
    let row = session_store.get_history(&id, None).await.unwrap().into_iter().find(|m| m.role == "assistant").unwrap();
    assert_eq!(row.metadata.unwrap().get("run_id").and_then(|v| v.as_str()), Some("r1"));
    let again = reconciler(&event_store, &session_store).reconcile_candidates().await;
    assert_eq!((again.stamps_synthesized, again.usage_rebilled), (0, 0), "the stamp is the idempotence guard");
    assert_eq!(session_store.get_metadata(&id).await.unwrap().unwrap().input_tokens, 300);
}
```
Also assert in `each_assistant_row_carries_its_own_calls_tokens` (:595, a run WITH a meta) that `stamps_synthesized == 0`.

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- gateway::projection_reconciler` → `no field stamps_synthesized`.

- [ ] **Step 3: Minimal implementation** (`heal_session`)
```rust
struct RunSpan { run_id: String, start: EventSeq, end: Option<EventSeq>, assistant_rows: usize, meta: bool }
// inside the walk, before project_event:
match &rec.event {
    SessionEvent::RunStarted { run_id, .. } => spans.push(RunSpan { run_id: run_id.clone(), start: rec.seq, end: None, assistant_rows: 0, meta: false }),
    SessionEvent::AssistantMessage { .. } => if let Some(s) = spans.last_mut().filter(|s| s.end.is_none()) { s.assistant_rows += 1 },
    SessionEvent::RunFinished { .. } => if let Some(s) = spans.last_mut() { s.end.get_or_insert(rec.seq); },
    SessionEvent::AssistantRunMeta { run_id, .. } => if let Some(s) = spans.iter_mut().rev().find(|s| &s.run_id == run_id) { s.meta = true },
    _ => {}
}
// after the walk — only a whole-session pass can know a meta is MISSING rather than not-yet-drained:
if scope == HealScope::WholeSession {
    for span in spans.iter().filter(|s| s.assistant_rows > 0 && !s.meta) {
        let Some(end) = span.end else { continue };                       // still open: its meta may still come
        let Some(meta) = crate::gateway::agent_instance::build_message_metadata(Some(&span.run_id), None) else { continue };
        match store.stamp_assistant_metadata_in_range(id, span.start, end, &meta).await {
            Ok(StampOutcome::Stamped) => {
                report.stamps_synthesized += 1;
                let slice: Vec<SessionEventRecord> = events.iter().filter(|r| r.seq >= span.start && r.seq <= end).cloned().collect();
                if let Some(t) = crate::session::usage_fold::run_usage_totals(&slice) {
                    if (t.input > 0 || t.output > 0) && store.update_session_usage(id, i64::try_from(t.input).unwrap_or(i64::MAX), i64::try_from(t.output).unwrap_or(i64::MAX), 0.0, None, None).await.is_ok() {
                        report.usage_rebilled += 1;   // cost stays 0.0: unpriced, and the fold cannot price
                    }
                }
            }
            Ok(StampOutcome::AlreadyStamped | StampOutcome::NoRowInRange) => {}
            Err(e) => { tracing::warn!(session = ?id, run_id = %span.run_id, error = %e, "heal: synthesized stamp failed"); report.errored = true; }
        }
    }
}
// up_to_date: add `&& report.stamps_synthesized == 0`
```
`request_repair` → `reconcile_candidates` sums `stamps_synthesized`; both log lines gain it.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p alephcore --lib -- gateway::projection_reconciler gateway::session_projector` + `cargo check -p alephcore --bins`
- [ ] **Step 6: Commit** — `git add src/gateway/session_projector.rs src/gateway/projection_reconciler.rs src/bin/aleph-server/commands/start/mod.rs` · `gateway: boot heal stamps and bills a finished run whose AssistantRunMeta never landed` + Claude-Session line.

---

### Task 10: QA stages `unanswered` and `ratchet`

**Files:**
- Modify: `qa/resume_boundary/run.sh:50-54` (stage list), `:203-211` (FLOOR table), `:213-330` (case arms)
- Modify: `qa/resume_boundary/patch_r2.mjs` (+ optional 6th arg `embed-stall` that enables memory with the mock as embedding provider; + `[resume] max_attempts` via env `QA_MAX_ATTEMPTS`)
- Modify: `qa/resume_boundary/mock_r2.mjs` (+ `POST /v1/embeddings` handler that stalls `QA_EMBED_STALL_MS` then answers a valid 8-dim vector)
- Modify: `qa/resume_boundary/drive_r2.mjs` (+ commands `hooks`, `window`, `unanswered`, `ratchet`)
- Test: the stages themselves (`bash qa/resume_boundary/run.sh unanswered|ratchet`, `SKIP_BUILD=1` when no `.rs` changed)

**Interfaces:**
- Consumes: T6–T9 behaviour; hook file format `~/.aleph/hooks.json` = `{"hooks": {"BeforeAgentStart": [{"hooks": [{"type": "command", "command": "...", "timeout_secs": 600}]}]}}` (`src/extension/hooks/user_settings.rs:18-33, 80-110`; global layer at `get_config_dir()/hooks.json` = `$ALEPH_HOME/hooks.json`, `:117-126`); shell-hook consent registry `$ALEPH_HOME/shell-hooks-allowlist.json` (`consent.rs:112-125`, entry fingerprint = first 16 hex of `sha256(plugin_name ++ "\0" ++ command)` with `plugin_name = "user:global"`, `consent.rs:104-110`, `user_settings.rs:288`); hooks run under `cmd /C` on Windows (`executor.rs:549-556`) so the sleeper is `node -e "setTimeout(()=>{},N)"` on both hosts; embeddings go to `<api_base>/embeddings` with `timeout_ms` (`memory/embedding_provider.rs:143,172,230`; config `[[memory.embedding.providers]]` `src/config/types/memory/embed.rs:48-75,184-189`).
- Produces: stages `unanswered` (FLOOR measured on first green run — declare provisional 9) and `ratchet` (provisional 8). Node ≥ 22.5 already required (`drive_r2.mjs:38` imports `node:sqlite`; host is v24.13.0).

**Which lever opens which window (correction to spec §11 / the prompt):** `BeforeAgentStart` runs at `run_loop/mod.rs:485-526`, BEFORE the orchestrator seeds (`runner_impl.rs:342`), so a kill inside it leaves NO user message — that is the 8.2(b)/T14 window. The seed→RunStarted window (`runner_impl.rs:342→978`) contains exactly two awaits: routing recall and `build_system_prompt` (`:516-551`), whose only remote call is the memory hybrid recall's query embedding (`prompt_build.rs:376-384` → `embedding_provider.rs:230`). So `unanswered` stretches the window with memory ON and the mock as the embedding provider stalling `/v1/embeddings` for 10 s; `ratchet` uses the `BeforeAgentStart` sleeper because after `ResumeAttempted` is stamped any crash before the resumed run's `RunStarted` must count. No production failpoint either way.

- [ ] **Step 0: Pre-flight — prove the stall is on the seed→RunStarted path before trusting any green.** `KEEP=1 QA_EMBED_STALL_MS=10000 bash qa/resume_boundary/run.sh unanswered` once the fixture below exists, then read `$QA_ROOT/mock.log`: an `embeddings request; stalling 10000ms` line must sit between the `user_message` row's timestamp and the kill. If it never arrives, `drive window` exits 1 with `INSTRUMENT FAILURE` and the stage is NOT green — the fallback lever is `routing_recall` (`runner_impl.rs:516-523`, also embedding-backed). A silent miss must never read as a pass.
- [ ] **Step 1: Fixture code**

patch_r2.mjs (after the `for (const [section, key, value] of […])` loop):

```js
const embedStall = process.argv[7] === "embed-stall";
if (process.env.QA_MAX_ATTEMPTS) src = setKey(src, "resume", "max_attempts", process.env.QA_MAX_ATTEMPTS);
if (embedStall) {
  src = setKey(src, "memory", "enabled", "true");
  src = setKey(src, "memory.embedding", "active_provider_id", '"qa-embed"');
  src += `
[[memory.embedding.providers]]
id = "qa-embed"
name = "QA embed (stalls)"
preset = "custom"
api_base = "http://127.0.0.1:${mockPort}/v1"
api_key = "qa-dummy"
models = ["qa-embed"]
dimensions = 8
timeout_ms = 60000
`;
}
```

mock_r2.mjs (inside the request handler, before the messages route):

```js
if (req.method === "POST" && req.url.endsWith("/embeddings")) {
  const stall = Number(process.env.QA_EMBED_STALL_MS || 0);
  log(`embeddings request; stalling ${stall}ms`);
  setTimeout(() => {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ object: "list", data: [{ object: "embedding", index: 0, embedding: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8] }], model: "qa-embed", usage: { prompt_tokens: 1, total_tokens: 1 } }));
  }, stall);
  return;
}
```

drive_r2.mjs commands:

```js
import crypto from "node:crypto";
const ALEPH_HOME = path.join(QA_ROOT, "home", ".aleph");

/** Install an approved BeforeAgentStart command hook that sleeps `ms`. */
function cmdHooks(ms) {
  const command = `node -e "setTimeout(()=>{}, ${Number(ms)})"`;
  fs.writeFileSync(path.join(ALEPH_HOME, "hooks.json"), JSON.stringify({ hooks: { BeforeAgentStart: [{ hooks: [{ type: "command", command, timeout_secs: 600 }] }] } }, null, 2));
  const fingerprint = crypto.createHash("sha256").update("user:global").update(Buffer.from([0])).update(command).digest("hex").slice(0, 16);
  const now = Math.floor(Date.now() / 1000);
  fs.writeFileSync(path.join(ALEPH_HOME, "shell-hooks-allowlist.json"), JSON.stringify({ version: 1, entries: [{ fingerprint, plugin_name: "user:global", command, event: "BeforeAgentStart", status: "approved", first_seen: now, approved_at: now }] }, null, 2));
  log(`hook installed: ${command} (fp ${fingerprint})`);
}

const kinds = (key) => eventsOf(key).filter((r) => r.retired_at === null).map((r) => r.event_type);
const countKind = (key, k) => kinds(key).filter((t) => t.includes(k)).length;
const abandonedCount = (key) => eventsOf(key).filter((r) => r.event_type.includes("run_finished") && /"abandoned"/.test(r.payload_json)).length;
const bootLine = (serverLog, nth) => {
  const lines = fs.readFileSync(serverLog, "utf8").split("\n").filter((l) => l.includes("ResumeCoordinator boot scan finished"));
  const l = lines[nth - 1] ?? ""; const num = (k) => Number((l.match(new RegExp(`${k}=(\\d+)`)) || [])[1] ?? -1);
  return { resumed: num("resumed"), abandoned: num("abandoned"), skipped: num("skipped"), raw: l };
};

/** Send a turn and return once the seed is durable but no RunStarted is — the window this stage kills in. */
async function cmdWindow(marker = "qa-unanswered") {
  const conn = new Conn("driver"); await conn.open();
  const started = await sendTurn(conn, `${marker} hello, are you there`, null, null, null);
  fs.writeFileSync(SESSION_FILE, started.session_key);
  const opened = await until(() => countKind(started.session_key, "user_message") >= 1 && countKind(started.session_key, "run_started") === 0, 60_000, 100);
  conn.close();
  if (!opened) { console.error("INSTRUMENT FAILURE: the seed→RunStarted window never opened (embedding stall not on the path?)"); process.exit(1); }
  log("window open: user_message durable, no run_started");
}

async function cmdUnanswered(serverLog, phase) {
  const key = readSession();
  if (phase === "after-kill") {
    check(countKind(key, "user_message") === 1 && countKind(key, "run_started") === 0, "kill landed inside the seed→RunStarted window", kinds(key).join(","));
    return;
  }
  const conn = new Conn("driver"); await conn.open();
  const { lastRun } = await lastRunOf(conn, key);
  if (phase === "before-resume") { check(lastRun?.disposition === "unanswered", "chat.history reads the session as `unanswered`", show(lastRun)); conn.close(); return; }
  const b = bootLine(serverLog, 1);
  check(b.resumed === 1, "the boot scan resumed the unanswered message (resumed=1)", b.raw);
  check(countKind(key, "resume_attempted") === 1, "exactly one ResumeAttempted stamp", kinds(key).join(","));
  check(countKind(key, "tool_error") === 0, "no boundary repair was written (nothing dangled)", kinds(key).join(","));
  const answered = await until(() => kinds(key).filter((t) => t.includes("assistant_message")).length >= 1 && kinds(key).at(-1)?.includes("run_finished"), 120_000);
  check(Boolean(answered), "the transcript ends with an assistant answer and a RunFinished", kinds(key).join(","));
  const rq = requests(); check(rq.some((r) => userText(r).includes("hello, are you there")), "the original message reached the model", String(rq.length));
  const settled = await lastRunOf(conn, key); check(settled.lastRun?.disposition === "clean", "last_run settles to clean", show(settled.lastRun));
  const h = await conn.attempt("chat.history", { session_key: key });
  const rows = h.result?.messages ?? [];
  check(rows.at(-1)?.role === "assistant", "chat.history's last row is the assistant", show(rows.at(-1)));
  conn.close();
}

/** Variant 2: no seed at all (needs T14). Not counted toward the floor. */
function cmdNoticeSkip() { console.log("SKIP  needs T14 (8.2(b) SystemMessage notice for a kill before the seed)"); }

async function cmdRatchet(serverLog, boot) {
  const key = readSession(); const n = Number(boot); const b = bootLine(serverLog, n);
  if (n <= 2) {
    check(b.resumed === 1 && b.abandoned === 0, `boot ${n}: resumed=1 abandoned=0`, b.raw);
    check(countKind(key, "resume_attempted") === n, `boot ${n}: ${n} ResumeAttempted stamp(s) on the log`, kinds(key).join(","));
    check(countKind(key, "run_started") === 1, `boot ${n}: the resumed run never reached RunStarted (hook held it)`, kinds(key).join(","));
  } else if (n === 3) {
    check(b.resumed === 0 && b.abandoned === 1, "boot 3: attempts == max_attempts(2) → abandoned=1, resumed=0", b.raw);
    check(abandonedCount(key) === 1, "one RunFinished{abandoned} closer", kinds(key).join(","));
    check(countKind(key, "resume_attempted") === 2, "no third stamp", kinds(key).join(","));
  } else {
    check(b.resumed === 0 && b.abandoned === 0, "boot 4: nothing to resume, nothing abandoned", b.raw);
  }
}
// main(): case "hooks": cmdHooks(REST[0] ?? 10000); case "window": await cmdWindow(REST[0]); case "unanswered": await cmdUnanswered(REST[0], REST[1]);
//         case "notice-skip": cmdNoticeSkip(); case "ratchet": await cmdRatchet(REST[0], REST[1]);
```

run.sh: stage list accumulates (T13 already added `parked`): `claims|denied|rewind|knobs|holes|parked|unanswered|ratchet`; FLOOR `unanswered) FLOOR=9 ;; ratchet) FLOOR=8 ;;` (provisional — replace with the count the first green run prints, same commit); arms:

```bash
    unanswered)
      # Memory ON with the mock as embedding provider: the query embedding is
      # the only remote call between the seed (runner_impl.rs:342) and
      # RunStarted (:978), so a 10 s stall there is the window. Resume ON.
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" embed-stall >/dev/null || exit 1
      kill -9 "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null
      QA_EMBED_STALL_MS=10000 node "$HERE/mock_r2.mjs" "$MOCK_PORT" "$R2_REQUESTS" >"$QA_ROOT/mock.log" 2>&1 &
      MOCK_PID=$!; sleep 1
      start_server || exit 1
      drive window qa-unanswered || { echo "instrument failure: window never opened" >&2; RC=1; }
      hard_kill_server
      [ "$RC" = "0" ] && { drive unanswered "$QA_ROOT/server.log" after-kill || RC=1; }
      # Boot with resume OFF first: the wire face must say `unanswered` on its own.
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" embed-stall >/dev/null || exit 1
      [ "$RC" = "0" ] && { start_server || exit 1; drive unanswered "$QA_ROOT/server.log" before-resume || RC=1; hard_kill_server; }
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" embed-stall >/dev/null || exit 1
      kill -9 "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null
      QA_EMBED_STALL_MS=0 node "$HERE/mock_r2.mjs" "$MOCK_PORT" "$R2_REQUESTS" >"$QA_ROOT/mock.log" 2>&1 &
      MOCK_PID=$!; sleep 1
      [ "$RC" = "0" ] && { start_server || exit 1; drive unanswered "$QA_ROOT/server.log" after-resume || RC=1; }
      drive notice-skip
      ;;
    ratchet)
      # max_attempts=2: boot1 (0 stamps → stamp #1), boot2 (1 → #2), boot3 (2 ≥ 2 → abandon), boot4 clean.
      QA_MAX_ATTEMPTS=2 node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1
      start_server || exit 1
      drive dangle qa-dangle || { echo "instrument failure: no dangle" >&2; RC=1; }
      hard_kill_server
      # Installed AFTER the first crash so the dangle turn itself was not held.
      drive hooks 600000 || exit 1
      for n in 1 2 3 4; do
        [ "$RC" = "0" ] || break
        start_server || exit 1
        # The boot scan is detached; wait for its line, then kill mid-hook.
        wait_for_text "$QA_ROOT/server.log" "ResumeCoordinator boot scan finished" 120 || { echo "boot $n: no scan line" >&2; RC=1; }
        [ "$n" -le 2 ] && sleep 2
        hard_kill_server
        [ "$RC" = "0" ] && { drive ratchet "$QA_ROOT/server.log" "$n" || RC=1; }
      done
      # Orphaned sleepers self-terminate (600 s); belt and braces:
      command -v taskkill >/dev/null 2>&1 && taskkill //F //IM node.exe //FI "WINDOWTITLE eq setTimeout*" >/dev/null 2>&1 || true
      ;;
```

- [ ] **Step 2: Run, expect FAIL on the base commit** — `SKIP_BUILD=1 bash qa/resume_boundary/run.sh unanswered` on `5e85060b8`'s binary: `chat.history reads the session as unanswered` FAIL and `resumed=1` FAIL (the old scan skips a marker-less session); `ratchet`: boot 4 still `resumed=1` (old counter never moved) FAIL.
- [ ] **Step 3: Run on the branch, expect PASS** — `bash qa/resume_boundary/run.sh unanswered && bash qa/resume_boundary/run.sh ratchet` (KEEP=1 on first run; record the printed `assertions:` counts into the FLOOR table in the same commit).
- [ ] **Step 4: Mutation check** — the §11 ledger: move the `stamp_resume_attempt` call after `retrigger` (T6) ⇒ `ratchet` boot 4 `resumed=1` RED; skip the activity-window loop (T7) ⇒ `unanswered` `resumed=1` RED. Record both observed reds in the plan's ledger.
- [ ] **Step 5: Commit** — `git add qa/resume_boundary/run.sh qa/resume_boundary/patch_r2.mjs qa/resume_boundary/mock_r2.mjs qa/resume_boundary/drive_r2.mjs` — `qa: resume_boundary unanswered + ratchet stages` + Claude-Session line.

---

### Task 18: QA stages `parallel`, `undecodable`, Node `attribute`; Python stages deleted

**Files:**
- Modify: `qa/resume_boundary/run.sh` (header, `case` list `:47-51`, r2 guard `:195`, floors `:239-246`, new arms; delete `:386-514`)
- Modify: `qa/resume_boundary/drive_r2.mjs` (`sendTurn` explicit key, `cmdParallel`, `cmdUndecodable`, `cmdAttribute`, `eventsOf` per-session filter), `mock_r2.mjs` (`qa-spawn` arm, `slow` delay), `patch_r2.mjs` (env `QA_MAX_CONCURRENT` → `[resume] max_concurrent`; NOT a 6th positional — T10 already uses argv[7] for `embed-stall`)
- Delete: `qa/resume_boundary/assert_repairs.py`, `qa/resume_boundary/drive_dangle.py`

Host facts: `node --version` = v24.13.0 ≥ 22.5, and `drive_r2.mjs:38` already imports `DatabaseSync` from `node:sqlite` — no package needed (the older-Node fallback in the contract does not apply; `qa/` has no `package.json`). The r2 dangle instrument is `bash = "ask"` (a long command cannot dangle on this host, `patch_r2.mjs` header §3). After group B a parked call is answered by the fourth arm, so `attribute` — which asserts the `ThisRestart` / `EarlierRun` wording of an IN-FLIGHT call — needs a call that is genuinely in flight: `mock_r2` gains `qa-spawn` → `tool_use subagent {action:"run", task:"qa-child-slow: wait"}`; the child's own request carries `qa-child-slow`, which the mock holds for 120 s, so the parent's `subagent` dispatch is durable and unanswered when the kill lands. `parallel` and `undecodable` keep `ask` (the repair wording is irrelevant to them; the mock ends any turn whose marker it already answered).

- [ ] **Step 1: Write the stages**

run.sh: the stage list ACCUMULATES — by this task T13 (`parked`) and T10 (`unanswered|ratchet`) are already in it: `case "$STAGE" in claims|denied|rewind|knobs|holes|parked|unanswered|ratchet|parallel|undecodable|attribute) ;;`; the r2 guard becomes unconditional (drop the `if [ "$STAGE" != "crash" ] …` wrapper, delete everything from `# The round-1 stages need a real interpreter` to EOF, rewrite the header's usage lines); floors `parallel) FLOOR=7 ;; undecodable) FLOOR=9 ;; attribute) FLOOR=6 ;;` — **measured**: run each stage once green, replace with the printed count, and say so in the commit. Arms:
```bash
parallel)
  QA_MAX_CONCURRENT=2 node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1
  start_server || exit 1
  for s in a b c; do drive dangle "qa-dangle:slow-$s" "" "" "agent:main:qa-$s:s1" || RC=1; done
  hard_kill_server
  [ "$RC" = "0" ] && { drive assert-dangling 3 || RC=1; }
  [ "$RC" = "0" ] && { start_server || exit 1; }
  [ "$RC" = "0" ] && { drive parallel "$QA_ROOT/server.log" || RC=1; } ;;
undecodable)
  node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1
  start_server || exit 1
  drive dangle qa-dangle:x "" "" "agent:main:qa-x:s1" || RC=1
  drive dangle qa-dangle:y "" "" "agent:main:qa-y:s1" || RC=1
  hard_kill_server
  [ "$RC" = "0" ] && { drive undecodable forge agent:main:qa-x:s1 || RC=1; }   # server DOWN: the only safe writer
  [ "$RC" = "0" ] && { start_server || exit 1; }
  [ "$RC" = "0" ] && { drive undecodable refused agent:main:qa-x:s1 agent:main:qa-y:s1 || RC=1; }
  hard_kill_server
  [ "$RC" = "0" ] && { drive undecodable mark-ignorable agent:main:qa-x:s1 || RC=1; }
  [ "$RC" = "0" ] && { start_server || exit 1; }
  [ "$RC" = "0" ] && { drive undecodable skipped agent:main:qa-x:s1 agent:main:qa-y:s1 || RC=1; } ;;
attribute)
  node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false allow >/dev/null || exit 1
  start_server || exit 1
  drive dangle qa-spawn:1 || RC=1
  hard_kill_server
  [ "$RC" = "0" ] && { start_server || exit 1; }          # resume still OFF: dangle #1 survives
  [ "$RC" = "0" ] && { drive dangle qa-spawn:2 || RC=1; }
  hard_kill_server
  [ "$RC" = "0" ] && { drive assert-dangling 2 || RC=1; }
  node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true allow >/dev/null || exit 1
  [ "$RC" = "0" ] && { start_server || exit 1; }
  [ "$RC" = "0" ] && { drive attribute || RC=1; } ;;
```
drive_r2.mjs (`check`, `until`, `withEvents`, `requests`, `lastRunOf`, `Conn` exist):
```js
// session_events keys rows by the SERIALISED SessionId; resolve the wire key to it once.
const sidJson = (wire) => { const m = /^agent:([^:]+):([^:]+):s(\d+)$/.exec(wire);
  return JSON.stringify({ type: "main", agent_id: m[1], main_key: m[2], epoch: Number(m[3]) }); };
const eventsOfKey = (wire) => withEvents((db) => db ? db.prepare(
  "SELECT seq, event_type, payload_json, retired_at FROM session_events WHERE session_id = ? ORDER BY seq ASC").all(sidJson(wire)) : []);
// sendTurn(conn, text, sessionKey, model, tier) unchanged; cmdDangle gains a 4th arg `key` passed as sessionKey.

async function cmdParallel(serverLog) {
  const keys = ["a", "b", "c"].map((s) => `agent:main:qa-${s}:s1`);
  const conn = new Conn("driver"); await conn.open();
  let maxInFlight = 0; const seen = new Set();
  const allClean = await until(async () => {
    const m = await conn.attempt("gateway.metrics.run_concurrency", {});
    const running = m.result?.running_sessions ?? [];
    maxInFlight = Math.max(maxInFlight, running.length); running.forEach((k) => seen.add(k));
    const states = await Promise.all(keys.map(async (k) => (await lastRunOf(conn, k)).lastRun?.disposition));
    return states.every((d) => d === "clean");
  }, 90_000, 150);
  conn.close();
  check(!!allClean, "all three sessions settled to clean");
  check(maxInFlight === 2, "at most two resumed runs in flight at once, and two at least once", `${maxInFlight}`);
  keys.forEach((k) => check(seen.has(k), `resumed run observed running: ${k}`));
  const log = fs.readFileSync(serverLog, "utf8");
  check(/ResumeCoordinator boot scan finished.*resumed=3/.test(log), "boot line reports resumed=3");
  check(!/refused|retrigger_failed/.test(log.split("ResumeCoordinator boot scan finished")[0] ?? ""), "no candidate refused");
}

function forgeRow(wire, payload) {
  const db = new DatabaseSync(EVENTS_DB); try {
    const { m } = db.prepare("SELECT COALESCE(MAX(seq),0) AS m FROM session_events WHERE session_id = ?").get(sidJson(wire));
    db.prepare("INSERT INTO session_events (session_id, seq, event_type, payload_json, created_at) VALUES (?, ?, 'from_the_future', ?, ?)")
      .run(sidJson(wire), m + 1, JSON.stringify(payload), Date.now());
    return m + 1;
  } finally { db.close(); }
}
async function cmdUndecodable(sub, x, y) {
  if (sub === "forge") { const seq = forgeRow(x, { type: "from_the_future", v: 99 }); check(seq > 1, "future row appended after the dangle", `seq ${seq}`); return; }
  if (sub === "mark-ignorable") {
    const db = new DatabaseSync(EVENTS_DB); try {
      const n = db.prepare("UPDATE session_events SET payload_json = json_set(payload_json, '$.ignorable', json('true')) WHERE event_type = 'from_the_future' AND session_id = ?").run(sidJson(x)).changes;
      check(n === 1, "exactly one row marked ignorable");
    } finally { db.close(); } return;
  }
  const conn = new Conn("driver"); await conn.open();
  const yClean = await until(async () => (await lastRunOf(conn, y)).lastRun?.disposition === "clean", 90_000);
  check(!!yClean, `the clean session ${y} was resumed`);
  const xr = await lastRunOf(conn, x);
  const doc = await conn.attempt("diagnostics.run", { only: ["core/session-log"] });
  const detail = JSON.stringify(doc.result?.findings ?? []);
  if (sub === "refused") {
    check(xr.lastRun?.disposition === "log_inconsistent", "attach face refuses the session with the bad row", show(xr.lastRun));
    check((xr.lastRun?.contradictions ?? []).includes("session-log-undecodable-record"), "…under its own tag");
    check(detail.includes("session-log-undecodable-record"), "doctor names the record", detail.slice(0, 300));
    check(detail.includes("qa-x"), "doctor names the session");
  } else {
    const xClean = await until(async () => (await lastRunOf(conn, x)).lastRun?.disposition === "clean", 90_000);
    check(!!xClean, "the ignorable row no longer refuses the session; it resumed");
    check(!detail.includes("undecodable"), "doctor no longer names an undecodable record");
    check(/1 ignorable row/.test(detail), "doctor counts the skipped row", detail.slice(0, 300));
  }
  conn.close();
}
const FIVE = ["OUTCOME UNKNOWN", "NOT a report that the call failed", "side effects", "Verify the current state before deciding", "`subagent`"];
async function cmdAttribute() {
  const texts = await until(() => { const t = repairTexts(); return t.length >= 2 ? t : null; }, 120_000);
  check(!!texts, "two repair texts reached the model", `${repairTexts().length}`);
  for (const t of texts ?? []) for (const p of FIVE) check(t.includes(p), `repair text carries ${p}`);
  check((texts ?? []).some((t) => t.includes("an earlier run in this session")), "the older dangle is attributed to an earlier run");
  check((texts ?? []).some((t) => t.includes("the server restarted")), "this run's dangle is attributed to the restart");
}
// repairTexts(): every content block of every logged request body containing "OUTCOME UNKNOWN" (port of assert_repairs.py::repair_texts)
```
mock_r2.mjs: `MARKER = /qa-(dangle|burst|spawn)(?::([\w-]+))?/g`; new arm `if (verb === "spawn") return { kind: "tool", name: "subagent", input: { action: "run", task: "qa-child-slow: wait for the operator" } };`; before the marker scan: `if (userSide.includes("qa-child-slow")) { await sleep(120_000); return { kind: "end", text: "child done" }; }`; and when every hit is already answered and some tag starts with `slow`, `await sleep(Number(process.env.QA_SLOW_MS || 4000))` before the `end` (`decide` becomes async — its one caller awaits it). patch_r2.mjs: `if (process.env.QA_MAX_CONCURRENT) src = setKey(src, "resume", "max_concurrent", process.env.QA_MAX_CONCURRENT);` beside T10's `QA_MAX_ATTEMPTS` line (argv[7] stays T10's `embed-stall`).

- [ ] **Step 2: Run, expect FAIL on the pre-C tree** — `SKIP_BUILD=1 bash qa/resume_boundary/run.sh parallel` on `5e85060b8`'s binary ⇒ `maxInFlight === 2` red (serial: 1); `undecodable refused` ⇒ `y` never settles (`resume scan failed; skipping resume`) — the two §11 "observed red" lines.
- [ ] **Step 3: Implement** — the code above.
- [ ] **Step 4: Run, expect PASS** — `bash qa/resume_boundary/run.sh parallel`, `… undecodable`, `… attribute`, then the five r2 stages with `SKIP_BUILD=1`; record each printed `assertions:` count as its floor.
- [ ] **Step 5: Mutation check** (§11 ledger): T14's `fold_strict` → lossy filter ⇒ `undecodable refused` RED (`y` resumes but so does `x`, and `log_inconsistent` never appears); T15's semaphore → 1 permit ⇒ `parallel` RED at `maxInFlight === 2`.
- [ ] **Step 6: Commit**: `git rm qa/resume_boundary/assert_repairs.py qa/resume_boundary/drive_dangle.py` + `git add qa/resume_boundary/run.sh qa/resume_boundary/drive_r2.mjs qa/resume_boundary/mock_r2.mjs qa/resume_boundary/patch_r2.mjs` → `qa: parallel + undecodable stages, attribute ported to node, python stages deleted`. Trailer as above.

---

> **Group D（T19–T22）**：T19–T21 的 `src/` 文件与 A/B/C 完全不相交（`process_journal.rs` `process_alive.rs` `live_tail.rs` `platforms/common.rs` `process_registry.rs` `pty/{session,manager}.rs` `bash_exec.rs` `terminal.rs` `handlers/pty.rs`），可与 T3–T18 **并行**（三个之间串行）；**T22 共享 `qa/resume_boundary/*`，排在 T18 之后**。并行前 orchestrator 用 `git diff --stat` 核一遍两边的文件集不相交。

### Task 19: journal records kind / pid / creation time and boots a two-arm tombstone

**Files:**
- Modify: `src/builtin_tools/process_journal.rs:1-48` (module doc), `:224-305` (types), `:415-520` (reconcile), `:664-705` (`record_spawn`), `:1315-1330` `:1394-1408` (test literals)
- Modify: `src/utils/process_alive.rs:112` (`default_refresh_kind` → `pub(crate)`)
- Test: same file, existing `mod tests` (`test_gate()` / `enable_for_test`)

**Interfaces:**
- Consumes: `process_alive::{with_process_specifics, default_refresh_kind, process_start_time}`; `sysinfo::Process::{start_time, status}` (0.39.3).
- Produces: `pub enum JournalKind { Bash, Pty }`; `pub enum Tombstone { ExitedDuringRestart, StillRunningUnattached { pid: u32 } }`; `pub enum Liveness { Exited, StillRunning, Unknown }`; `pub fn probe_liveness(pid: u32, process_created_at_ms: Option<u64>) -> Liveness`; `JobRecord { .., pub kind, pub pty_session_id: Option<String>, pub pid: Option<u32>, pub process_created_at_ms: Option<u64>, pub tombstone: Option<Tombstone> }`; `pub(crate) fn record_child(id: u64, pid: u32)`; `#[cfg(test)] pub(crate) fn init_and_reconcile_with_probe(dir, probe: &dyn Fn(u32, Option<u64>) -> Liveness) -> usize`; `settled_label` arms `exited_during_restart` / `still_running_unattached`.

`JobRecord {` constructors (comments stripped): 5 — `:451` `:471` (struct-update, untouched), `:684`, tests `:1316` `:1394` (gain the five fields).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_old_row_without_the_new_fields_still_decodes_as_a_bash_row() {
    let r: JobRecord = serde_json::from_str(r#"{"id":7,"owner":"o","command":"c","started_ms":1,"phase":"running"}"#).unwrap();
    assert_eq!((r.kind, r.pid, r.process_created_at_ms, r.tombstone, r.pty_session_id), (JournalKind::Bash, None, None, None, None));
}

#[test]
fn record_child_needs_the_intent_row_and_stamps_pid_plus_creation_time() {
    let _g = gate();
    let tmp = tempfile::tempdir().unwrap();
    enable_for_test(tmp.path().to_path_buf());
    let me = std::process::id();
    record_child(5, me);                                   // before the intent: no-op
    assert!(lookup(5, Some(OWNER)).is_none());
    record_spawn(5, "sleep 300", Some(OWNER));
    assert_eq!(lookup(5, Some(OWNER)).unwrap().record.pid, None, "the intent row carries no pid");
    record_child(5, me);
    let on_disk: JobRecord = serde_json::from_slice(&std::fs::read(tmp.path().join("job-5").join(STATE_FILE)).unwrap()).unwrap();
    assert_eq!((on_disk.pid, on_disk.kind), (Some(me), JournalKind::Bash));
    assert!(on_disk.process_created_at_ms.is_some(), "sysinfo must report this process's start time");
    disable_for_test();
}

#[test]
fn probe_liveness_answers_all_three_ways() {
    let me = std::process::id();
    let created = crate::utils::process_alive::process_start_time(me as i32).map(|s| s * 1000);
    assert_eq!(probe_liveness(me, created), Liveness::StillRunning);
    assert_eq!(probe_liveness(me, created.map(|c| c + 60_000)), Liveness::Exited, "a recycled pid is not this process");
    assert_eq!(probe_liveness(me, None), Liveness::Unknown, "present but unverifiable is not a verdict");
    assert_eq!(probe_liveness(u32::MAX - 7, Some(1)), Liveness::Exited);
}

fn boot_with(dir: &std::path::Path, answer: Liveness) -> JobRecord {
    init_and_reconcile_with_probe(dir.to_path_buf(), &move |_, _| answer);
    lookup(9, Some(OWNER)).expect("row survives").record
}

#[test]
fn reconcile_writes_the_probed_arm_and_reasks_a_still_running_one() {
    let _g = gate();
    let tmp = tempfile::tempdir().unwrap();
    enable_for_test(tmp.path().to_path_buf());
    record_spawn(9, "sleep 300", Some(OWNER));
    record_child(9, std::process::id());
    disable_for_test();
    let r = boot_with(tmp.path(), Liveness::StillRunning);
    assert_eq!((r.phase, r.tombstone), (JobPhase::Interrupted, Some(Tombstone::StillRunningUnattached { pid: std::process::id() })));
    assert_eq!(settled_label(&r), "still_running_unattached");
    let stamp = r.ended_ms;
    let r = boot_with(tmp.path(), Liveness::Exited);          // the orphan died between boots
    assert_eq!((r.tombstone, settled_label(&r), r.ended_ms), (Some(Tombstone::ExitedDuringRestart), "exited_during_restart", stamp));
    let r = boot_with(tmp.path(), Liveness::StillRunning);    // exited is final: not re-asked
    assert_eq!(r.tombstone, Some(Tombstone::ExitedDuringRestart));
    disable_for_test();
}

#[test]
fn an_unknown_probe_and_a_pidless_row_keep_the_liveness_unknown_wording() {
    let _g = gate();
    let tmp = tempfile::tempdir().unwrap();
    enable_for_test(tmp.path().to_path_buf());
    record_spawn(9, "sleep 300", Some(OWNER));
    record_child(9, std::process::id());
    record_spawn(10, "sleep 300", Some(OWNER));               // never got a pid
    disable_for_test();
    let r = boot_with(tmp.path(), Liveness::Unknown);
    assert_eq!((r.tombstone, settled_label(&r)), (None, "interrupted_by_restart_liveness_unknown"));
    let ten = lookup(10, Some(OWNER)).unwrap().record;
    assert_eq!((ten.phase, ten.tombstone), (JobPhase::Interrupted, None));
    for l in ["exited_during_restart", "still_running_unattached", settled_label(&ten)] { assert!(!l.contains("fail"), "{l}"); }
    disable_for_test();
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- builtin_tools::process_journal::tests` (detached pwsh + poll) → `cannot find type JournalKind` / `cannot find function record_child`.

- [ ] **Step 3: Minimal implementation**

```rust
// process_alive.rs:112 — `pub(crate) fn default_refresh_kind()` (body unchanged)

// process_journal.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind { #[default] Bash, Pty }

/// What boot learned about a `Running` row's OS process. `None` on an
/// `Interrupted` row = no pid, or the probe could not answer: today's
/// "liveness unknown" wording stays as the third arm (criterion #8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tombstone { ExitedDuringRestart, StillRunningUnattached { pid: u32 } }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness { Exited, StillRunning, Unknown }

/// sysinfo reports start times in whole seconds on every platform.
const CREATION_TIME_TOLERANCE_MS: u64 = 2_000;

/// Pure boundary: one sysinfo refresh of `pid`. `Unknown` whenever the
/// instrument cannot answer — it cannot see THIS process, the pid is present
/// with no creation time to compare, or sysinfo reports 0. `Exited` only when
/// no process has the pid, it is a zombie, or it started at another time.
pub fn probe_liveness(pid: u32, process_created_at_ms: Option<u64>) -> Liveness {
    use crate::utils::process_alive::{default_refresh_kind, with_process_specifics};
    use sysinfo::ProcessStatus::{Dead, Zombie};
    let facts = |p: &sysinfo::Process| (p.start_time(), p.status());
    if with_process_specifics(std::process::id(), default_refresh_kind(), facts).is_none() {
        return Liveness::Unknown;
    }
    match (with_process_specifics(pid, default_refresh_kind(), facts), process_created_at_ms) {
        (None, _) | (Some((_, Zombie | Dead)), _) => Liveness::Exited,
        (Some((0, _)), _) | (Some(_), None) => Liveness::Unknown,
        (Some((start_s, _)), Some(created)) if (start_s * 1000).abs_diff(created) <= CREATION_TIME_TOLERANCE_MS => Liveness::StillRunning,
        (Some(_), Some(_)) => Liveness::Exited,
    }
}

// JobRecord — five new fields, each `#[serde(default)]`, Options also `skip_serializing_if = "Option::is_none"`:
pub kind: JournalKind, pub pty_session_id: Option<String>, pub pid: Option<u32>,
pub process_created_at_ms: Option<u64>, pub tombstone: Option<Tombstone>,
// record_spawn's literal: `kind: JournalKind::Bash, pty_session_id: None, pid: None, process_created_at_ms: None, tombstone: None`.

// settled_label — Interrupted arm:
JobPhase::Interrupted => match record.tombstone {
    Some(Tombstone::ExitedDuringRestart) => "exited_during_restart",
    Some(Tombstone::StillRunningUnattached { .. }) => "still_running_unattached",
    None => "interrupted_by_restart_liveness_unknown",
},

/// Third write of a job's life: the driver has the OS child. The creation
/// time comes off the SAME routine the boot probe compares against. No-op
/// without an intent row — the reconcile can only tombstone what was written
/// before the crash, so the intent must come first.
pub(crate) fn record_child(id: u64, pid: u32) {
    let Some(dir) = store_dir() else { return };
    let created = crate::utils::process_alive::process_start_time(i32::try_from(pid).unwrap_or(-1)).map(|s| s.saturating_mul(1000));
    let record = {
        let mut index = index_lock();
        let Some(r) = index.get_mut(&id) else { return };
        r.pid = Some(pid); r.process_created_at_ms = created; r.clone()
    };
    write_state(&dir, &record);
}

pub fn init_and_reconcile(dir: PathBuf) -> usize { reconcile_with(dir, &probe_liveness) }
#[cfg(test)]
pub(crate) fn init_and_reconcile_with_probe(dir: PathBuf, probe: &dyn Fn(u32, Option<u64>) -> Liveness) -> usize { reconcile_with(dir, probe) }

fn tombstone_for(record: &JobRecord, probe: &dyn Fn(u32, Option<u64>) -> Liveness) -> Option<Tombstone> {
    let pid = record.pid?;
    match probe(pid, record.process_created_at_ms) {
        Liveness::Exited => Some(Tombstone::ExitedDuringRestart),
        Liveness::StillRunning => Some(Tombstone::StillRunningUnattached { pid }),
        Liveness::Unknown => None,
    }
}

// reconcile_with(dir, probe) = today's init_and_reconcile body; the `Running` arm becomes:
if record.phase == JobPhase::Running {
    let tombstone = JobRecord { phase: JobPhase::Interrupted, ended_ms: Some(now), tombstone: tombstone_for(&record, probe), ..record };
    write_state(&dir, &tombstone); tombstoned += 1; index.insert(tombstone.id, tombstone);
} else if matches!(record.tombstone, Some(Tombstone::StillRunningUnattached { .. }))
    && tombstone_for(&record, probe) == Some(Tombstone::ExitedDuringRestart)
{
    // Re-ask an orphan that outlived one restart. Only a definite `Exited`
    // rewrites; `Unknown` keeps the previous answer. `ended_ms` is NOT
    // re-stamped — it dates the restart that orphaned it.
    let record = JobRecord { tombstone: Some(Tombstone::ExitedDuringRestart), ..record };
    write_state(&dir, &record); index.insert(record.id, record);
} else if is_undelivered_completion(&record, now) { /* unchanged */ }
```

Module doc (判据 #1): "exactly twice per job — spawn and terminal" → "three times — intent at spawn, pid once the driver has the child, terminal"; replace the "**1. There is no pid…decided against for this round**" paragraph with three sentences naming `record_child`, `probe_liveness`, the three arms.

- [ ] **Step 4: Run, expect PASS** — same command; every pre-existing test stays green (`boot_reconcile_tombstones_orphans…` spawns no pid ⇒ still `interrupted_by_restart_liveness_unknown`).
- [ ] **Step 5: Mutation check** — ledger "9.1": turn `record_child`'s `get_mut` into an upsert → `record_child_needs_the_intent_row…` RED at `is_none()`. Map `Liveness::Unknown => Some(ExitedDuringRestart)` → `an_unknown_probe…` RED.
- [ ] **Step 6: Commit** — `git add src/builtin_tools/process_journal.rs src/utils/process_alive.rs` · `process_journal: record the child pid and probe liveness at boot into a two-arm tombstone` · `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`

---

### Task 20: pid reaches the journal (bash via LiveTail, PTY via PtyManager); PTY rows join the journal

**Files:**
- Modify: `src/sandbox/live_tail.rs:150-206`; `src/sandbox/platforms/common.rs:430`; `src/builtin_tools/process_registry.rs:371-378`; `src/builtin_tools/process_journal.rs` (`Verdict::Exited`, `PTY_INDEX`, `record_dir`, PTY writers/reader); `src/gateway/pty/session.rs:295-300` `:691-737`; `src/gateway/pty/manager.rs:452-486` `:560-593`
- Test: each file's `mod tests` (`common.rs`'s existing drain tests are all `#[cfg(unix)]` — the new one must not be)

**Interfaces:**
- Consumes: T19; `PtySession::with_screen(Screen::visible_text)` (`session.rs:444`); registry test helpers `ProcessRegistry::new_journaled` :1211, `live_handle()` :771, `unwrap_id` :780.
- Produces: `LiveTail::{set_child_pid(&self, u32), on_child_pid(&self, Box<dyn FnOnce(u32) + Send>), child_pid(&self) -> Option<u32>}`; `Verdict::Exited` (label `"exited"`, `settled_label` arm `"exited"`); `pub(crate) fn record_pty_spawn(session_id: &str, shell: &str, cwd: &str, created_by: Option<&str>)`; `pub(crate) fn record_pty_child(session_id: &str, pid: u32)`; `pub(crate) fn record_pty_settled(session_id: &str, verdict: Verdict, exit_code: Option<i32>, last_screen: &str)`; `pub(crate) fn lookup_pty(session_id: &str) -> Option<RecoveredJob>` (unscoped — each face applies its own predicate in T21); `pub(crate) fn PtySession::shell_pid(&self) -> Option<u32>`.

PTY lifecycle writers counted: spawn 1 (`manager.rs:455`), kills 3 (`close` :569, `close_all` :587, eviction :481), natural exit 1 (`settle_exit`). Row dir `<root>/pty-<uuid>/state.json` beside `job-<id>` (`highest_dir_id` strips only `job-`, `read_all` reads any subdir). A `Pty` row's `id` is `0` and never reaches a bash face: `lookup`/`list_for_scope` read `INDEX`, which holds only `Bash` rows (pinned below).

- [ ] **Step 1: Write the failing tests**

```rust
// live_tail.rs
#[test]
fn the_child_pid_fires_the_hook_whichever_side_arrives_first() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (t, t2) = (LiveTail::new(), LiveTail::new());
    let s = seen.clone(); t.on_child_pid(Box::new(move |p| s.lock().unwrap().push(p))); t.set_child_pid(41);
    t2.set_child_pid(42); let s = seen.clone(); t2.on_child_pid(Box::new(move |p| s.lock().unwrap().push(p)));
    t2.set_child_pid(43);                                   // one child per tail: ignored
    assert_eq!((&*seen.lock().unwrap(), t.child_pid(), t2.child_pid()), (&vec![41, 42], Some(41), Some(42)));
}

// common.rs (runs on Windows AND Unix)
#[tokio::test]
async fn run_child_with_drain_publishes_the_child_pid_into_the_scoped_tail() {
    use crate::sandbox::context::LIVE_TAIL;
    let spawn = || {
        let (prog, args) = if cfg!(windows) { ("cmd", ["/C", "echo hi"]) } else { ("sh", ["-c", "echo hi"]) };
        tokio::process::Command::new(prog).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped()).kill_on_drop(true).spawn().expect("spawn")
    };
    let child = spawn();
    let expected = child.id().expect("a just-spawned child has a pid");
    let tail = Arc::new(LiveTail::new());
    LIVE_TAIL.scope(tail.clone(), async { run_child_with_drain(child, None, Duration::from_secs(20), 1024).await.expect("exit"); }).await;
    assert_eq!(tail.child_pid(), Some(expected));
    run_child_with_drain(spawn(), None, Duration::from_secs(20), 1024).await.expect("exit"); // no scope ⇒ foreground untouched
}

// process_registry.rs (beside `a_second_boot_never_re_issues…` :1203)
#[tokio::test]
async fn attaching_a_tail_wires_the_child_pid_into_the_journal() {
    let _g = process_journal::test_gate();
    let tmp = tempfile::tempdir().unwrap();
    process_journal::init_and_reconcile(tmp.path().to_path_buf());
    let reg = ProcessRegistry::new_journaled();
    reg.seed_id_floor(process_journal::id_floor());
    let owner = Some("boot-owner".to_string());
    let id = unwrap_id(reg.register_running("sleep 300", owner.clone(), live_handle().await));
    let tail = Arc::new(LiveTail::new());
    reg.attach_live(id, tail.clone());
    tail.set_child_pid(std::process::id());
    assert_eq!(process_journal::lookup(id, owner.as_deref()).expect("row").record.pid, Some(std::process::id()));
    process_journal::disable_for_test();
}

// process_journal.rs
#[test]
fn a_pty_row_lives_beside_bash_rows_and_never_answers_on_a_bash_face() {
    let _g = gate();
    let tmp = tempfile::tempdir().unwrap();
    enable_for_test(tmp.path().to_path_buf());
    record_pty_spawn("u-1", "pwsh", "C:/w", Some("alice"));
    record_pty_spawn("u-2", "pwsh", "C:/w", None);          // unowned: refused, like bash
    record_pty_child("u-1", std::process::id());
    let r = lookup_pty("u-1").expect("pty row").record;
    assert_eq!((r.kind, r.id, r.pty_session_id.as_deref(), r.pid), (JournalKind::Pty, 0, Some("u-1"), Some(std::process::id())));
    assert!(lookup_pty("u-2").is_none() && tmp.path().join("pty-u-1").join(STATE_FILE).exists());
    assert!(lookup(0, Some("alice")).is_none() && list_for_scope(Some("alice"), &[]).is_empty(), "bash faces never see a Pty row");
    record_pty_settled("u-1", Verdict::Exited, Some(3), "last screen");
    let j = lookup_pty("u-1").unwrap();
    assert_eq!((j.record.phase, settled_label(&j.record), j.record.exit_code), (JobPhase::Settled, "exited", Some(3)));
    assert!(j.recorded_output.contains("last screen"), "{:?}", j.recorded_output); // `[screen]` header rides along like `[stdout]`
    record_pty_settled("u-1", Verdict::Killed, None, "");   // second verdict loses
    assert_eq!(settled_label(&lookup_pty("u-1").unwrap().record), "exited");
    disable_for_test();
}

#[test]
fn a_running_pty_row_is_tombstoned_at_boot_like_a_bash_row() {
    let _g = gate();
    let tmp = tempfile::tempdir().unwrap();
    enable_for_test(tmp.path().to_path_buf());
    record_pty_spawn("u-3", "pwsh", "", Some("alice"));
    record_pty_child("u-3", std::process::id());
    disable_for_test();
    init_and_reconcile_with_probe(tmp.path().to_path_buf(), &|_, _| Liveness::StillRunning);
    assert_eq!(lookup_pty("u-3").unwrap().record.tombstone, Some(Tombstone::StillRunningUnattached { pid: std::process::id() }));
    disable_for_test();
}

// pty/manager.rs
#[test]
fn spawning_a_pty_journals_intent_then_pid_and_its_exit_settles_the_row() {
    use crate::builtin_tools::process_journal as j;
    let _g = j::test_gate();
    let tmp = tempfile::tempdir().unwrap();
    j::enable_for_test(tmp.path().to_path_buf());
    let (cmd, args) = if cfg!(windows) { ("cmd.exe", vec!["/C".into(), "exit 3".into()]) } else { ("sh", vec!["-c".into(), "exit 3".into()]) };
    let sid = PtyManager::new().spawn(&SpawnOptions { command: Some(cmd.into()), args, created_by: Some("u-alice".into()), ..Default::default() }).expect("spawn").session_id;
    let row = j::lookup_pty(&sid).expect("journaled").record;
    assert!(row.kind == j::JournalKind::Pty && row.pid.is_some(), "{row:?}");
    let settled = (0..100).find_map(|_| { std::thread::sleep(std::time::Duration::from_millis(100));
        let r = j::lookup_pty(&sid)?.record; (r.phase == j::JobPhase::Settled).then_some(r) }).expect("exit reaches the journal within 10 s");
    assert_eq!((settled.exit_code, settled.outcome.as_deref()), (Some(3), Some("exited")));
    j::disable_for_test();
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- sandbox::live_tail sandbox::platforms::common builtin_tools::process_registry builtin_tools::process_journal gateway::pty::manager` → `no method named set_child_pid` / `cannot find function record_pty_spawn`.

- [ ] **Step 3: Minimal implementation**

```rust
// live_tail.rs
struct ChildSlot { pid: Option<u32>, hook: Option<Box<dyn FnOnce(u32) + Send>> }
pub struct LiveTail { stdout: Mutex<Ring>, stderr: Mutex<Ring>, child: Mutex<ChildSlot> }   // + in new()/with_capacity()
impl LiveTail {
    /// The driver's one report of the OS child. Fires the hook from
    /// [`Self::on_child_pid`]; a second report is ignored (one child per tail).
    pub fn set_child_pid(&self, pid: u32) {
        let hook = { let mut c = self.child.lock().unwrap_or_else(|e| e.into_inner());
            if c.pid.is_some() { return; } c.pid = Some(pid); c.hook.take() };
        if let Some(h) = hook { h(pid); }
    }
    /// Run `hook` once with the child's pid — immediately if already known.
    pub fn on_child_pid(&self, hook: Box<dyn FnOnce(u32) + Send>) {
        let mut c = self.child.lock().unwrap_or_else(|e| e.into_inner());
        match c.pid { Some(p) => { drop(c); hook(p); } None => c.hook = Some(hook) }
    }
    #[must_use] pub fn child_pid(&self) -> Option<u32> { self.child.lock().unwrap_or_else(|e| e.into_inner()).pid }
}

// common.rs :430 — after `let live = current_live_tail();`
if let (Some(tail), Some(pid)) = (&live, child.id()) { tail.set_child_pid(pid); }

// process_registry.rs attach_live — store `live.clone()`, then outside the table lock:
if self.journaled { live.on_child_pid(Box::new(move |pid| process_journal::record_child(id, pid))); }

// process_journal.rs
pub(crate) enum Verdict { Completed, Killed, Exited }        // label "exited"; settled_label + the label census test gain the arm
const PTY_DIR_PREFIX: &str = "pty-";
static PTY_INDEX: LazyLock<Mutex<HashMap<String, JobRecord>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn pty_index_lock() -> MutexGuard<'static, HashMap<String, JobRecord>> { PTY_INDEX.lock().unwrap_or_else(|e| e.into_inner()) }
/// One row, one directory: `job-<id>` for Bash, `pty-<uuid>` for Pty.
fn record_dir(dir: &Path, record: &JobRecord) -> PathBuf {
    match (record.kind, record.pty_session_id.as_deref()) {
        (JournalKind::Pty, Some(sid)) => dir.join(format!("{PTY_DIR_PREFIX}{sid}")),
        _ => job_dir(dir, record.id),
    }
}
// `job_dir(dir, record.id)` → `record_dir(dir, record)` at :356 :363 :445 :991; `append_block` takes `&JobRecord` instead of `id`
// (callers :834 :835 pass the record they already hold). reconcile: `index.insert(..)` → by kind into `index` / `pty_index`;
// `*pty_index_lock() = pty_index;`; `enable_for_test`/`disable_for_test` clear it.

pub(crate) fn record_pty_spawn(session_id: &str, shell: &str, cwd: &str, created_by: Option<&str>) {
    let Some(dir) = store_dir() else { return };
    let Some(owner) = created_by.filter(|o| !o.is_empty()) else { return };
    let record = JobRecord { id: 0, kind: JournalKind::Pty, pty_session_id: Some(session_id.to_string()), owner: owner.to_string(),
        command: mask_block(&format!("{shell} (cwd {cwd})")), started_ms: now_ms(), phase: JobPhase::Running, ended_ms: None,
        outcome: None, exit_code: None, output_file: Some(OUTPUT_FILE.to_string()), partial_file: None, announce_attempts: 0,
        announced_boot: None, pid: None, process_created_at_ms: None, tombstone: None };
    write_state(&dir, &record);
    pty_index_lock().insert(session_id.to_string(), record);
}
pub(crate) fn record_pty_child(session_id: &str, pid: u32) { /* record_child's body over pty_index_lock().get_mut(session_id) */ }
pub(crate) fn record_pty_settled(session_id: &str, verdict: Verdict, exit_code: Option<i32>, last_screen: &str) {
    let Some(dir) = store_dir() else { return };
    let record = { let mut ix = pty_index_lock(); let Some(r) = ix.get_mut(session_id) else { return };
        if r.phase != JobPhase::Running { return; }             // first verdict wins: close() then settle_exit()
        r.phase = JobPhase::Settled; r.ended_ms = Some(now_ms()); r.outcome = Some(verdict.label().to_string()); r.exit_code = exit_code; r.clone() };
    append_block(&dir, &record, "screen", last_screen);
    write_state(&dir, &record);
}
#[must_use]
pub(crate) fn lookup_pty(session_id: &str) -> Option<RecoveredJob> {
    let dir = store_dir()?; let record = pty_index_lock().get(session_id).cloned()?; Some(hydrate(&dir, record))
}

// session.rs — replace the CUT comment :295-300 with the accessor and its caller:
/// portable-pty's pid for the shell. Reader: `PtyManager::spawn` → `process_journal::record_pty_child`.
pub(crate) fn shell_pid(&self) -> Option<u32> { self.shell_pid }
// settle_exit, before `super::manager().remove(&session.id)`:
let screen = session.with_screen(super::screen::Screen::visible_text);
crate::builtin_tools::process_journal::record_pty_settled(&session.id, crate::builtin_tools::process_journal::Verdict::Exited, i32::try_from(exit_code).ok(), &screen);

// manager.rs spawn — intent BEFORE PtySession::spawn, pid right after:
let id = uuid::Uuid::new_v4().to_string();
process_journal::record_pty_spawn(&id, opts.command.as_deref().unwrap_or("<default shell>"), opts.cwd.as_deref().unwrap_or(""), opts.created_by.as_deref());
let session = PtySession::spawn(id.clone(), opts, bus)?;
if let Some(pid) = session.shell_pid() { process_journal::record_pty_child(&id, pid); }
// close / close_all / eviction: `process_journal::record_pty_settled(&<id>, Verdict::Killed, None, "")` before `.kill()`.
```

- [ ] **Step 4: Run, expect PASS** — Step 2 command, plus `-- builtin_tools::bash_exec::tests::every_process_action_face_reaches_the_journal_resolver`.
- [ ] **Step 5: Mutation check** — delete the `record_pty_spawn` line in `manager.rs` → `spawning_a_pty_journals_intent…` RED at `expect("journaled")` (no intent ⇒ `record_pty_child` is a no-op: the same intent-first pin as T19). Delete `tail.set_child_pid(pid)` → `run_child_with_drain_publishes…` RED.
- [ ] **Step 6: Commit** — `git add src/sandbox/live_tail.rs src/sandbox/platforms/common.rs src/builtin_tools/process_registry.rs src/builtin_tools/process_journal.rs src/gateway/pty/session.rs src/gateway/pty/manager.rs` · `process_journal: pty sessions join the journal and the bash child pid rides the live tail` · session trailer.

---

### Task 21: the tombstone reaches the faces — bash poll/wait/kill, terminal read/wait/explain, pty.* RPC

**Files:**
- Modify: `src/builtin_tools/process_journal.rs` (`TombstoneReport`, `tombstone_report`); `src/builtin_tools/bash_exec.rs:790-800` `:858-916` `:967-989`; `src/builtin_tools/terminal.rs:181-186` `:247-292` `:379-411` `:587` `:675`; `src/gateway/handlers/pty.rs:425-436`
- Test: each file's `mod tests`; bash extends `every_process_action_face_reaches_the_journal_resolver` :2085

**Interfaces:**
- Consumes: T19/T20; `RecoveredJob` (all fields pub); `pty::owner_admits`; `terminal_admits`; `JsonRpcResponse::error(id, code, msg)` (`gateway/protocol.rs:149`); `handlers/pty.rs` test helper `req(method, params)` :461 and `CALLER_USER.scope(Some(user), fut)` (:1235 shape).
- Produces: `pub struct TombstoneReport { pub kind: &'static str, pub text: String, pub pid: Option<u32>, pub stop_command: Option<String> }`; `pub fn tombstone_report(job: &RecoveredJob) -> Option<TombstoneReport>` (`Some` iff `phase == Interrupted`, three arms); `TerminalOutput { .., #[serde(skip_serializing_if = "is_false")] pub lost_with_restart: bool }`; bash JSON keys `lost_with_restart: true` / `pid` / `stop_command` on Interrupted rows only.

Renderers (判据 #17): bash JSON and `TerminalOutput` → the model; `pty.*` → JSON-RPC `error.message`, which the Panel already renders for every `pty.*` error (no `error.data` — nothing reads it). Not-found consumers: bash 3 faces via one resolver (:770-800); terminal 3 via `owned_session_id`; RPC 4 via `require_owned` (census-pinned `pty.rs:1418`).

- [ ] **Step 1: Write the failing tests**

```rust
// process_journal.rs
fn interrupted_job(tombstone: Option<Tombstone>, output: &str) -> RecoveredJob {
    RecoveredJob { record: JobRecord { id: 12, owner: OWNER.into(), command: "sleep 300".into(), started_ms: 1_000, phase: JobPhase::Interrupted,
        ended_ms: Some(9_000), outcome: None, exit_code: None, output_file: None, partial_file: None, announce_attempts: 0, announced_boot: None,
        kind: JournalKind::Bash, pty_session_id: None, pid: Some(4321), process_created_at_ms: Some(2_000), tombstone },
        recorded_output: output.into(), output_is_live_capture: true, last_activity_ms: 5_000 }
}

#[test]
fn the_report_has_three_arms_and_none_for_a_live_row() {
    let exited = tombstone_report(&interrupted_job(Some(Tombstone::ExitedDuringRestart), "built 3 crates")).unwrap();
    assert_eq!((exited.kind, exited.stop_command.as_deref()), ("exited_during_restart", None));
    for s in ["Job #12", "previous server process", "has EXITED", "exit code unknown", "built 3 crates"] { assert!(exited.text.contains(s), "{s}: {}", exited.text); }
    let running = tombstone_report(&interrupted_job(Some(Tombstone::StillRunningUnattached { pid: 4321 }), "")).unwrap();
    let cmd = running.stop_command.clone().unwrap();
    assert_eq!(cmd, if cfg!(windows) { "taskkill /PID 4321 /T /F" } else { "kill 4321" });
    for s in ["STILL RUNNING", "pid 4321", "cannot re-attach", "no output was recorded", cmd.as_str()] { assert!(running.text.contains(s), "{s}: {}", running.text); }
    let unknown = tombstone_report(&interrupted_job(None, "")).unwrap();
    assert!(unknown.text.contains("did NOT check") && unknown.kind == "interrupted_by_restart_liveness_unknown", "{}", unknown.text);
    for r in [&exited, &running, &unknown] { assert!(!r.text.contains("fail"), "{}", r.text); }
    let mut live = interrupted_job(None, ""); live.record.phase = JobPhase::Running;
    assert!(tombstone_report(&live).is_none());
    let mut pty = interrupted_job(Some(Tombstone::ExitedDuringRestart), ""); pty.record.kind = JournalKind::Pty; pty.record.pty_session_id = Some("u-9".into());
    assert!(tombstone_report(&pty).unwrap().text.starts_with("Terminal u-9"));
}

// bash_exec.rs — appended inside every_process_action_face_reaches_the_journal_resolver after the `for face` loop
// (that loop's `interrupted_by_restart_liveness_unknown` stays: 4242 has no pid)
process_journal::enable_for_test(tmp.path().to_path_buf());
process_journal::record_spawn(4244, "sleep 300", Some(&owner));
process_journal::record_child(4244, std::process::id());
process_journal::disable_for_test();
process_journal::init_and_reconcile_with_probe(tmp.path().to_path_buf(), &|_, _| process_journal::Liveness::StillRunning);
let v: serde_json::Value = serde_json::from_str(&handle_process_action("poll", Some(4244), None).await.stdout).unwrap();
assert_eq!((&v["status"], &v["lost_with_restart"], &v["pid"]), (&json!("still_running_unattached"), &json!(true), &json!(std::process::id())));
assert!(v["advisory"].as_str().unwrap().contains(&format!("pid {}", std::process::id())));
let k: serde_json::Value = serde_json::from_str(&handle_process_action("kill", Some(4244), None).await.stdout).unwrap();
let note = k["skipped"].as_str().unwrap();
assert!(note.contains("kill was NOT attempted") && note.contains(v["stop_command"].as_str().unwrap()), "{note}");
process_journal::enable_for_test(tmp.path().to_path_buf());        // a settled row stays byte-identical: no key, not `false`
process_journal::record_spawn(4245, "echo", Some(&owner));
process_journal::record_settled(4245, Verdict::Completed, Some(0), "", "");
assert!(!handle_process_action("poll", Some(4245), None).await.stdout.contains("lost_with_restart"));
process_journal::disable_for_test();

// terminal.rs
#[test]
fn a_tombstoned_terminal_answers_its_owner_and_nobody_else() {
    use crate::builtin_tools::process_journal as j;
    let _g = j::test_gate();
    let tmp = tempfile::tempdir().unwrap();
    j::enable_for_test(tmp.path().to_path_buf());
    j::record_pty_spawn("t-1", "pwsh", "", Some("alice")); j::record_pty_child("t-1", 777);
    j::disable_for_test();
    j::init_and_reconcile_with_probe(tmp.path().to_path_buf(), &|_, _| j::Liveness::StillRunning);
    match owned_session_id(Some("t-1"), Some("alice"), "read") { Err(TerminalRefusal::LostWithRestart(r)) => assert!(r.text.contains("pid 777")), o => panic!("{o:?}") }
    assert!(matches!(owned_session_id(Some("t-1"), Some("bob"), "read"), Err(TerminalRefusal::Message(m)) if m == pty::no_such_session("t-1")));
    assert!(matches!(owned_session_id(Some("t-1"), None, "read"), Err(TerminalRefusal::Message(_))), "actor-less sees nothing");
    let out = serde_json::to_value(TerminalOutput { success: false, message: "x".into(), data: None, lost_with_restart: false }).unwrap();
    assert!(out.get("lost_with_restart").is_none(), "false is skipped: existing envelopes byte-identical");
    j::disable_for_test();
}

// handlers/pty.rs
#[tokio::test]
async fn require_owned_returns_the_tombstone_text_to_its_owner_only() {
    use crate::builtin_tools::process_journal as j;
    let _g = j::test_gate();
    let tmp = tempfile::tempdir().unwrap();
    j::enable_for_test(tmp.path().to_path_buf());
    j::record_pty_spawn("t-2", "pwsh", "", Some("alice")); j::record_pty_child("t-2", 778);
    j::disable_for_test();
    j::init_and_reconcile_with_probe(tmp.path().to_path_buf(), &|_, _| j::Liveness::Exited);
    let r = req("pty.close", json!({ "session_id": "t-2" }));
    let as_user = |u: &str| { let r = r.clone(); let u = u.to_string();
        crate::gateway::caller_identity::CALLER_USER.scope(Some(u), async move { require_owned(&r, "t-2") }) };
    let msg = as_user("alice").await.unwrap_err().error.unwrap().message;
    assert!(msg.contains("has EXITED") && msg.contains("Terminal t-2"), "{msg}");
    assert_eq!(as_user("bob").await.unwrap_err().error.unwrap().message, pty::no_such_session("t-2"));
    j::disable_for_test();
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p alephcore --lib -- process_journal::tests::the_report bash_exec::tests::every_process_action terminal::tests::a_tombstoned handlers::pty::tests::require_owned_returns` → `cannot find function tombstone_report` / `cannot find type TerminalRefusal`.

- [ ] **Step 3: Minimal implementation**

```rust
// process_journal.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TombstoneReport { pub kind: &'static str, pub text: String, pub pid: Option<u32>, pub stop_command: Option<String> }

/// The sentence a face gives for an id the previous server owned. Pure —
/// reads only the recovered row. Every arm says what the model can do next;
/// none kills anything (U5). `None` for any non-`Interrupted` row.
#[must_use]
pub fn tombstone_report(job: &RecoveredJob) -> Option<TombstoneReport> {
    let r = &job.record;
    if r.phase != JobPhase::Interrupted { return None; }
    let subject = match (r.kind, r.pty_session_id.as_deref()) {
        (JournalKind::Pty, Some(sid)) => format!("Terminal {sid} (`{}`)", r.command),
        _ => format!("Job #{} (`{}`)", r.id, r.command),
    };
    let head = format!("{subject} was started by a previous server process, which stopped before {} (unix ms); this server holds no handle to it and cannot re-attach.", r.ended_ms.unwrap_or(0));
    let output = if job.recorded_output.is_empty() { "no output was recorded".to_string() } else {
        format!("last recorded output (as of {}{}): {}", job.last_activity_ms, if job.output_is_live_capture { ", a mid-run snapshot" } else { "" }, job.recorded_output) };
    let started = r.process_created_at_ms.map_or(String::new(), |t| format!(", started {t}"));
    let pid_s = r.pid.map_or("?".to_string(), |p| p.to_string());
    Some(match r.tombstone {
        Some(Tombstone::ExitedDuringRestart) => TombstoneReport { kind: "exited_during_restart",
            text: format!("{head} Its OS process (pid {pid_s}{started}) has EXITED — exit code unknown. {output}."), pid: r.pid, stop_command: None },
        Some(Tombstone::StillRunningUnattached { pid }) => {
            let cmd = if cfg!(windows) { format!("taskkill /PID {pid} /T /F") } else { format!("kill {pid}") };
            TombstoneReport { kind: "still_running_unattached",
                text: format!("{head} Its OS process is STILL RUNNING (pid {pid}{started}) as of the last server start; kill was NOT attempted and will not be. To stop it run `{cmd}`. {output}."),
                pid: Some(pid), stop_command: Some(cmd) }
        }
        None => TombstoneReport { kind: "interrupted_by_restart_liveness_unknown",
            text: format!("{head} Aleph did NOT check whether the OS process is still alive — it may still be running, finished, or have died with the server. Nothing about the command failed; check yourself (`ps` / `tasklist`) before re-running work that may already be done. {output}."),
            pid: r.pid, stop_command: None },
    })
}

// bash_exec.rs — `advisory` becomes one function over the job; the old `(Interrupted, _)` literal is DELETED
// (its wording now lives in tombstone_report's third arm — one owner, 判据 #1). Settled arms unchanged.
fn advisory(job: &RecoveredJob) -> String {
    if let Some(report) = process_journal::tombstone_report(job) { return report.text; }
    match (job.record.phase, job.record.outcome.as_deref()) {
        /* three Settled arms verbatim from today */
        (JobPhase::Interrupted | JobPhase::Running, _) => "This job's journal row is not terminal, but this process holds no handle for it. Aleph did NOT check whether the OS process is still alive.",
    }.to_string()
}
// recovered_row:
obj.insert("advisory".into(), serde_json::json!(advisory(job)));
if let Some(report) = process_journal::tombstone_report(job) {
    obj.insert("lost_with_restart".into(), serde_json::json!(true));
    if let Some(pid) = report.pid { obj.insert("pid".into(), serde_json::json!(pid)); }
    if let Some(cmd) = report.stop_command { obj.insert("stop_command".into(), serde_json::json!(cmd)); }
}
// kill arm (:795):
KillOutcome::NotFound => {
    let note = resolve_forgotten(Some(id), caller.as_deref(), &[]).first().and_then(process_journal::tombstone_report).and_then(|r| r.stop_command)
        .map_or_else(|| "kill was NOT attempted: this process holds no handle for this job. If its OS process is still alive, terminate it yourself (e.g. `pkill -f`).".to_string(),
                     |cmd| format!("kill was NOT attempted: this process holds no handle for this job. Its OS process is still running; to stop it run `{cmd}`."));
    recovered_or_unknown(id, caller.as_deref(), Some(&note))
}

// terminal.rs
#[derive(Debug)]
enum TerminalRefusal { Message(String), LostWithRestart(crate::builtin_tools::process_journal::TombstoneReport) }
impl From<String> for TerminalRefusal { fn from(m: String) -> Self { Self::Message(m) } }
pub struct TerminalOutput { pub success: bool, pub message: String, pub data: Option<serde_json::Value>,
    /// True only for a session a previous server process owned (journal tombstone). Skipped when false so every pre-existing envelope stays byte-identical.
    #[serde(skip_serializing_if = "is_false")] pub lost_with_restart: bool }
#[allow(clippy::trivially_copy_pass_by_ref)] fn is_false(b: &bool) -> bool { !*b }
fn owned_session_id<'a>(session_id: Option<&'a str>, actor: Option<&str>, action: &str) -> std::result::Result<&'a str, TerminalRefusal> {
    let session_id = session_id.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("{action} requires `session_id`"))?;
    if owner_record_admits(&pty::manager().owner_of(session_id), actor) { return Ok(session_id); }
    // Not live. The journal may know it — answered with the SAME predicate this tool uses for live rows,
    // so a stranger still reads `no_such_session` (the oracle this file documents).
    if let Some(job) = crate::builtin_tools::process_journal::lookup_pty(session_id) {
        if terminal_admits(Some(job.record.owner.as_str()), actor) {
            if let Some(report) = crate::builtin_tools::process_journal::tombstone_report(&job) { return Err(TerminalRefusal::LostWithRestart(report)); }
        }
    }
    Err(TerminalRefusal::Message(pty::no_such_session(session_id)))
}
// read_session / wait_for_session / explain_session return `Result<Value, TerminalRefusal>` (`?` on String errors converts).
// call(): `List`/`Status` arms `.map_err(TerminalRefusal::from)`; Err arms:
Err(TerminalRefusal::Message(message)) => { notify_tool_result(Self::NAME, &message, false); Ok(TerminalOutput { success: false, message, data: None, lost_with_restart: false }) }
Err(TerminalRefusal::LostWithRestart(r)) => { notify_tool_result(Self::NAME, &r.text, false);
    Ok(TerminalOutput { success: false, message: r.text, data: Some(serde_json::json!({ "tombstone": r.kind, "pid": r.pid, "stop_command": r.stop_command })), lost_with_restart: true }) }
// every other `TerminalOutput { .. }` literal (2 in call(), tests) gains `lost_with_restart: false`.

// handlers/pty.rs require_owned — between the admits check and the refusal:
if matches!(pty::manager().owner_of(session_id), pty::SessionOwner::Unknown) {
    if let Some(job) = crate::builtin_tools::process_journal::lookup_pty(session_id) {
        if pty::owner_admits(Some(job.record.owner.as_str()), actor.as_deref()) {
            if let Some(report) = crate::builtin_tools::process_journal::tombstone_report(&job) {
                return Err(JsonRpcResponse::error(request.id.clone(), INVALID_PARAMS, report.text));
            }
        }
    }
}
```

- [ ] **Step 4: Run, expect PASS** — Step 2 command, then `-- builtin_tools::bash_exec builtin_tools::terminal gateway::handlers::pty builtin_tools::process_journal`.
- [ ] **Step 5: Mutation check** — drop the ownership test around the journal lookup in either face → the "bob" assertions RED. Insert `lost_with_restart` unconditionally in `recovered_row` → the 4245 assertion RED.
- [ ] **Step 6: Commit** — `git add src/builtin_tools/process_journal.rs src/builtin_tools/bash_exec.rs src/builtin_tools/terminal.rs src/gateway/handlers/pty.rs` · `builtin_tools: answer a tombstoned job or terminal with what happened to its process, not not-found` · session trailer.

---

### Task 22: QA stage `tombstone` — one background job across two boots

**Files:**
- Modify (after T18 — this is the ONE group-D task that shares files with A/B/C; run it last): `qa/resume_boundary/run.sh:52-56` (stage list — accumulates: append `tombstone` to T18's list), `:262-268` (floor), `:269-395` (new arm), `cleanup()` :112
- Modify: `qa/resume_boundary/mock_r2.mjs:60-97`; `qa/resume_boundary/patch_r2.mjs:99-110`; `qa/resume_boundary/drive_r2.mjs` (new commands)
- Test: `SKIP_BUILD=1 bash qa/resume_boundary/run.sh tombstone`

**Interfaces:**
- Consumes: T21 bash JSON; journal file `<QA_ROOT>/home/.aleph/data/background_processes/job-<N>/state.json`; existing `sendTurn` / `requests()` / `until` / `check` in `drive_r2.mjs`.
- Produces: stage `tombstone`; mock markers `qa-bg`, `qa-poll:<N>-<tag>`, `qa-kill:<N>-<tag>`; `patch_r2.mjs` env `QA_WINDOWS_SANDBOX_OFF=1`.

Host facts: `node` v24.13.0; the bash tool's Windows shell is the probed PowerShell (`utils/shell.rs:352`) where `sleep` aliases `Start-Sleep`, so `sleep 300` is one spelling on both hosts. `patch_r2.mjs`'s 2026-09-03 measurement (every bash call dies in ~240 ms under the restricted token) predates the PowerShell-shell round; this stage re-measures it as a pre-flight and boots with `[sandbox.windows] use_restricted_token=false use_app_container=false use_job_object=false` so (a) `child.id()` is the shell, not the launcher, and (b) `kill -9` of the server does not close a KILL_ON_JOB_CLOSE job. If the job still settles within 2 s the stage exits **78** (instrument unavailable, reason printed) — never green.

- [ ] **Step 1: Write the failing fixture** (RED on the pre-T19 tree: no `pid` on the row, poll answers without `lost_with_restart`)

```js
// mock_r2.mjs
const MARKER = /qa-(dangle|burst|spawn|bg|poll|kill)(?::([\w-]+))?/g;   // T18 added `spawn`; keep it
// decide(), after `answered.add(whole)`; `[whole, verb, arg] = pending.at(-1)`; "12-a" → 12 (the suffix makes each turn's marker unique)
const pid = Number.parseInt(String(arg ?? ""), 10);
if (verb === "bg") return { kind: "tools", calls: [{ name: "bash", input: { cmd: "sleep 300", background: true } }] };
if (verb === "poll") return { kind: "tools", calls: [{ name: "bash", input: { process_action: "poll", process_id: pid } }] };
if (verb === "kill") return { kind: "tools", calls: [{ name: "bash", input: { process_action: "kill", process_id: pid } }] };

// patch_r2.mjs — after the setKey loop
if (process.env.QA_WINDOWS_SANDBOX_OFF === "1")
  for (const k of ["use_restricted_token", "use_app_container", "use_job_object"]) src = setKey(src, "sandbox.windows", k, "false");

// drive_r2.mjs
const JOBS_DIR = path.join(QA_ROOT, "home", ".aleph", "data", "background_processes");
const JOB_FILE = path.join(QA_ROOT, "job.json");
const readJob = (id) => { try { return JSON.parse(fs.readFileSync(path.join(JOBS_DIR, `job-${id}`, "state.json"), "utf8")); } catch { return null; } };
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch (e) { return e.code === "EPERM"; } };
const job = () => JSON.parse(fs.readFileSync(JOB_FILE, "utf8"));
/** The request-log line carrying process N's tool_result (JSON inside a JSON string ⇒ `\"key\":value`). */
const toolResultFor = (id) => requests().reverse().map((r) => JSON.stringify(r.body)).find((l) => new RegExp(`\\\\"process_id\\\\":${id}\\b`).test(l)) ?? null;
const hasKey = (line, key, val) => new RegExp(`\\\\"${key}\\\\":${val}`).test(line);

async function cmdBg() {
  const conn = new Conn("driver"); await conn.open();
  const started = await sendTurn(conn, "qa-bg start the long job", fs.existsSync(SESSION_FILE) ? readSession() : null);
  fs.writeFileSync(SESSION_FILE, started.session_key);
  const seen = []; let id = null; const end = Date.now() + 120_000;      // every DISTINCT row, polled at 5 ms: the first is the intent
  while (Date.now() < end) {
    for (const d of fs.existsSync(JOBS_DIR) ? fs.readdirSync(JOBS_DIR).filter((d) => d.startsWith("job-")) : []) {
      const row = readJob(d.slice(4));
      if (row && JSON.stringify(row) !== JSON.stringify(seen.at(-1))) { seen.push(row); id = row.id; }
    }
    if (seen.at(-1)?.pid) break;
    await sleep(5);
  }
  conn.close();
  const first = seen[0], last = seen.at(-1);
  check(first?.phase === "running" && first?.kind === "bash", "the intent row lands first, as a bash row", show(first));
  check(first && first.pid === undefined, "the FIRST row seen carries no pid (intent precedes the child)", show(seen));
  check(typeof last?.pid === "number", "the pid arrives on the row", show(last));
  check(typeof last?.process_created_at_ms === "number", "the creation time arrives beside it", show(last));
  await sleep(2_000);
  const later = readJob(id);
  if (later?.phase !== "running") { console.error(`INSTRUMENT UNAVAILABLE: job settled within 2 s (${show(later)}) — the shell cannot sleep here; see patch_r2.mjs point 3`); process.exit(78); }
  check(alive(last.pid), `the recorded pid ${last.pid} is a live process`);
  fs.writeFileSync(JOB_FILE, JSON.stringify({ id, pid: last.pid }));
}
async function cmdBgAlive(expect) { const { pid } = job(); check(alive(pid) === (expect === "yes"), `orphan ${pid} alive == ${expect}`); }
async function cmdTomb(arm, tag) {                                        // arm: still | exited
  const { id, pid } = job();
  const kind = arm === "still" ? "still_running_unattached" : "exited_during_restart";
  const row = readJob(id);
  check(row?.phase === "interrupted" && row?.tombstone?.kind === kind, `state.json carries the ${kind} tombstone`, show(row));
  const conn = new Conn("driver"); await conn.open();
  await sendTurn(conn, `qa-poll:${id}-${tag} what happened to it`, readSession());
  const line = await until(() => { const l = toolResultFor(id); return l && /lost_with_restart|no background process|liveness_unknown/.test(l) ? l : null; }, 120_000, 300);
  conn.close();
  check(!!line, "the poll's tool result reached the model", "no request carried it");
  check(line && hasKey(line, "lost_with_restart", "true"), "lost_with_restart: true", line);
  check(line && hasKey(line, "status", `\\\\"${kind}\\\\"`), `status == ${kind}`, line);
  if (arm === "still") check(line && line.includes(`pid ${pid}`) && /taskkill \/PID|kill \d+/.test(line), "the text names the pid and the stop command", line);
  else check(line && /has EXITED/.test(line), "the text says the process exited", line);
}
async function cmdKillTomb() {
  const { id, pid } = job();
  const conn = new Conn("driver"); await conn.open();
  await sendTurn(conn, `qa-kill:${id}-k stop it`, readSession());
  const line = await until(() => { const l = toolResultFor(id); return l && /kill was NOT attempted/.test(l) ? l : null; }, 120_000, 300);
  conn.close();
  check(line && hasKey(line, "lost_with_restart", "true") && /taskkill \/PID|kill \d+/.test(line), "kill answers with the command, not a pretend kill", line ?? "no result");
  check(alive(pid), `U5: the orphan ${pid} was NOT killed by the server`);
}
async function cmdKillSleep() {
  const { pid } = job();
  try { process.kill(pid, "SIGKILL"); } catch (e) { console.log("kill:", e.message); }
  check((await until(() => (alive(pid) ? null : true), 20_000, 200)) === true, `the fixture killed ${pid}`);
}
// dispatch: "bg" | "bg-alive" | "tomb" | "kill-tomb" | "kill-sleep" beside the existing cases; each ends with the
// existing `${PASS} passed, ${FAIL} failed` line and exits non-zero on FAIL.
```

```bash
# run.sh — add `tombstone` to the stage `case` at :54; floor `tombstone) FLOOR=15 ;;` (15 = the check() calls above;
# replace with the number the first green run prints, in the same commit — the file's rule).
tombstone)
  # `allow`: the job must actually run. Windows sandbox primitives off: with them
  # on, `child.id()` is the sandbox-init-windows launcher and the server's death
  # closes a KILL_ON_JOB_CLOSE job, so the "still running" arm is unreachable.
  QA_WINDOWS_SANDBOX_OFF=1 node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true allow >/dev/null || exit 1
  start_server || exit 1
  drive bg; rc=$?; [ "$rc" = "78" ] && exit 78; [ "$rc" = "0" ] || RC=1
  hard_kill_server
  [ "$RC" = "0" ] && { drive bg-alive yes || RC=1; }
  [ "$RC" = "0" ] && { start_server || exit 1; }
  [ "$RC" = "0" ] && { drive tomb still a || RC=1; }
  [ "$RC" = "0" ] && { drive kill-tomb || RC=1; }
  [ "$RC" = "0" ] && { drive kill-sleep || RC=1; }
  hard_kill_server
  [ "$RC" = "0" ] && { start_server || exit 1; }
  [ "$RC" = "0" ] && { drive tomb exited b || RC=1; }
  ;;
# cleanup(): `[ -f "$QA_ROOT/job.json" ] && node -e 'try{process.kill(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).pid,"SIGKILL")}catch{}' "$QA_ROOT/job.json"`
# so a failed run leaves no `sleep 300` behind. `BASH_POLICY` is forced to `allow` in the arm (the `holes` pattern).
```

- [ ] **Step 2: Run, expect FAIL** — on the tree before T19–T21 (binary built from this worktree, detached): `SKIP_BUILD=1 bash qa/resume_boundary/run.sh tombstone` → `FAIL  the pid arrives on the row`, then `FAIL  lost_with_restart: true`.
- [ ] **Step 3: Minimal implementation** — the fixture above is the implementation; nothing under `src/` changes here.
- [ ] **Step 4: Run, expect PASS** — after T19–T21 + rebuild: `bash qa/resume_boundary/run.sh tombstone` → `assertions: N (floor 15)`, `verdict: rc=0`; write N into the floor. On exit 78 record the printed reason in the verification ledger as **UNRUN**, not PASS — the T19–T21 unit tests are then the only witnesses.
- [ ] **Step 5: Mutation check** — ledger "9.1 意图挪到 spawn 后": move `record_spawn` in `register_running` to `attach_live` → `the FIRST row seen carries no pid` RED (the intent-to-pid gap is a whole process spawn against a 5 ms poll; T19's `record_child_needs_the_intent_row…` is the deterministic twin). Stop re-asking `StillRunningUnattached` rows in `reconcile_with` → `state.json carries the exited_during_restart tombstone` RED.
- [ ] **Step 6: Commit** — `git add qa/resume_boundary/run.sh qa/resume_boundary/drive_r2.mjs qa/resume_boundary/mock_r2.mjs qa/resume_boundary/patch_r2.mjs` · `qa: tombstone stage — a background job outlives two server boots and reads still-running then exited` · session trailer.

---

### Task 23: 全量验证集 + 十一个真机阶段 + 变异账本

**Files:**
- Modify: 本文末尾「验证记录」（观测值）；`qa/resume_boundary/run.sh` 的 FLOOR 表（把首次绿跑印出的 `assertions:` 数写成地板——各阶段任务已经这么要求；这里是复核）
- Test: 全部

**Interfaces:**
- Consumes: T1–T22 的全部产物。
- Produces: 验证记录（本文末尾）填满；每条 §11 变异有**观测到的**红。

- [ ] **Step 1: `git status --porcelain`** 为空；`tasklist //FI "IMAGENAME eq rustc.exe"` 无 rustc。
- [ ] **Step 2: 先跑 QA，后跑 clippy**（clippy `--all-targets` 会把 `target/debug` 的二进制留成 0 字节）。QA 一律从本 worktree 的树构建一次（第一个阶段不带 `SKIP_BUILD`），其余 `SKIP_BUILD=1`：

```bash
cd /d/Workspace/Aleph/.claude/worktrees/persistence-r3
bash qa/resume_boundary/run.sh parked                       # builds
for s in unanswered ratchet parallel undecodable attribute tombstone claims denied rewind knobs holes; do
  SKIP_BUILD=1 bash qa/resume_boundary/run.sh "$s" | tee "$S/qa-$s.txt" | grep -E '^(PASS|FAIL|SKIP|OBSERVATION|assertions:|verdict:|INSTRUMENT)' ; echo "== $s rc=${PIPESTATUS[0]}"
done
```
Expected: 11 个阶段 `verdict: rc=0`，每个的 `assertions: N (floor F)` 满足 `N >= F`；`tombstone` 若退出 78，账本记 **UNRUN + 打印的原因**，不记 PASS。`unanswered` 的 `notice-skip` 行必须已经从 SKIP 变成真实断言（T16 落地后 T10 的第二变体解锁——若仍是 SKIP，回 T10 把它接上再算绿）。

- [ ] **Step 3: 六条最小可信集 + 客户端 crate**（每条分离式，一次一个）

| # | 命令 | 期望 |
|---|---|---|
| 1 | `cargo test -p alephcore --lib`（全量） | 红名单 `comm -3` 基线 ⇒ **空**（新红逐条解释或修掉；数目不作数，名字作数） |
| 2 | `cargo test -p alephcore --bins` | 绿（含钉 boot 无条件 `install_policy`/`install_ledger` 的 census） |
| 3 | `cargo test -p alephcore --features test-helpers --test '*' -j 1`（约 36 min；先 `--no-run` 再跑 `resume_coordinator_integration parked_gate_integration` 两个二进制，再全量） | 绿 |
| 4 | `cargo test -p aleph-protocol && cargo test -p aleph-tui && cargo test -p aleph-cli` | 绿 |
| 5 | `cargo test -p aleph-panel --lib`（harness 在第一个失败处中止——用 `-- --skip <name>` 看其余） | 绿 |
| 6 | `just wasm`（Bash 工具；worktree 里先补 `node_modules` junction：`cmd //c mklink //J interfaces\\webchat\\node_modules D:\\Workspace\\Aleph\\interfaces\\webchat\\node_modules`；跑完 `git checkout -- interfaces/webchat/dist` 还原产物） | 编译通过；`dist/` 不进任何提交 |
| 7 | `just _stage-shell-placeholders && cargo clippy --workspace --all-targets`（Windows 上若报错排除 `-p aleph-desktop-macos -p aleph-desktop-linux`） | 0 error；warning 只允许出现在本轮 hunk 之外（对 `git diff 5e85060b8 --stat` 逐个核） |

- [ ] **Step 4: 变异账本——每一行都要实际翻转、实际看到红、再还原**（`git stash push -u -m persistence-r3-mutation-<n>` → 翻转 → 跑 → `git stash apply <sha>` 还原；或直接编辑再 `git checkout -- <file>`，先 `cp` 到 scratchpad）：

| # | 变异 | 翻转在哪 | 预期变红的测试 / 阶段 | 观测（执行者填） |
|---|---|---|---|---|
| 4.1 | retire 出事务 | T1 `write_batch`：`retire_in_txn` 挪到 `tx.commit()` 之后 | `retire_and_insert_are_one_transaction`；`manual_compact_is_one_store_transaction` | |
| 5.1 | `ResumeAttempted` 写点挪后 | T6 `handle_interrupted`：`stamp_resume_attempt` 挪到 `retrigger` 之后 | `a_retrigger_that_never_starts_is_capped_by_the_intent_stamp`；QA `ratchet` boot 4 `resumed=1` | |
| 5.2 | 活动窗口循环删除 | T15 `launch_resume` 的 `list_sessions` 块 | `an_unanswered_seed_is_stamped_and_retriggered_without_repair`；QA `unanswered` `resumed=1` | |
| 5.3 | fast-path 先跑后记 | T8：`journal.open` 挪到 `execution.await` 之后 | `every_fast_path_dispatch_is_journaled_before_it_runs` | |
| 5.4 | hook-stop 少一臂回执 | T9：deny 臂删掉 `journal_hook_stop` | `both_before_agent_start_exits_journal_the_stop` | |
| 6.1 | `Parked` 写点挪后 | T11：`record_parked` 挪到 `request_approval` 之后 | `a_gate_park_is_in_the_log_before_the_requester_is_reached`；QA `parked` 的 `dangle-parked` | |
| 6.1b | Approved 不清 parked | T11 reducer 的 Approved 臂 | `an_answered_gate_ends_the_park` | |
| 7.1 | 逐行 `Result` 改回整批丢弃 | T14 `load_run_markers`：`fold_strict` → lossy filter | `one_bad_row_refuses_only_its_own_session`；QA `undecodable refused` | |
| 7.3 | `ignorable` 闸删除 | T14 `decode_row`：去掉 `unknown_variant &&` | `an_unknown_variant_is_undecodable_unless_the_row_says_ignorable` row 10 | |
| 8.1 | 信号量退化为 1 | T15 `Semaphore::new(1)` | `three_interrupted_sessions_resume_two_at_a_time…`；QA `parallel` `maxInFlight === 2` | |
| 8.2 | `orphan_notice` 复活 | T17：`git checkout 5e85060b8 -- src/gateway/orphan_notice.rs` + `pub mod` | `the_projector_is_the_only_production_writer_of_the_messages_table` | |
| 8.2b | seeded 判反 | T16 `!seeded` → `seeded` | `a_task_whose_seed_never_landed_gets_exactly_one_resend_notice…` | |
| 9.1 | 意图挪到 spawn 后 | T19 `record_child` 改 upsert；T20 删 `record_pty_spawn` | `record_child_needs_the_intent_row…`；`spawning_a_pty_journals_intent…`；QA `tombstone` 首行无 pid | |
| 9.2 | Unknown 读成 Exited | T19 `Liveness::Unknown => Some(ExitedDuringRestart)` | `an_unknown_probe_and_a_pidless_row_keep_the_liveness_unknown_wording` | |
| 4.4 | epoch 先于父批 | T4：`register_epoch` 挪回父批之前 | `split_commits_parent_then_child_then_registers_the_epoch` | |

- [ ] **Step 5: 记录**——每条命令的 `test result:` 行、每个阶段的 `assertions:`/`verdict:` 行、每条变异的红名单，**逐条注明测于哪个 commit**（`git rev-parse --short HEAD`），写进「验证记录」。不提交代码；FLOOR 表若有改动随 T18/T22 的提交或单独 `qa: floors measured` 提交。

---

### Task 24: 文档——FEATURE_LOCATOR §4.13a round-3 · 附录 D/E · SESSION_KNOBS · SESSION_SERVICE · qa/README · CLAUDE.md 路由行

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.13a 末尾新增 `#### round-3（2026-09-12）` 小节；附录 D.0 / D.4 各加条目；附录 E.0 / E.4 各加触发器）
- Modify: `docs/reference/SESSION_KNOBS.md`（`RunEnvelopeSnapshot` 的两条**事实键** `allowed_tools` / `btw`：只有快照一根 rung，无会话/全局回退；「加一根新旋钮要动的每一处」表加一行「事实键走 `RUN_ENVELOPE_FACT_KEYS` 不走 `KNOB_KEYS`」）
- Modify: `docs/reference/SESSION_SERVICE.md`（`append_batch` / `Retire` / `Durability` 策略表；`emit_batch` 是提供方法且默认 `Err`；行信封 `v` / `ignorable`；逐行解码与 `UndecodableRecord`）
- Modify: `qa/README.md`（`qa/resume_boundary/run.sh` 条目：六个新阶段各在证明什么 + 地板；`attribute` 已是 Node；`tombstone` 的 78 = UNRUN；Python 阶段已删）
- Modify: `CLAUDE.md`（**只改**子系统路由表 `src/gateway/` 行的「真机 QA」列：`qa/resume_boundary/run.sh {claims,denied,rewind,knobs,holes,parked,unanswered,ratchet,parallel,undecodable,attribute,tombstone}`；判据索引**不动**——见下）
- Test: `grep -c` 每个新阶段名在 `qa/README.md` 出现 ≥ 1；`grep -n 'trailing_starts' docs/reference/*.md` 为空（旧名已改）

**Interfaces:**
- Consumes: T1–T23 的实际落地形状（读代码写文档，不从 spec 抄——spec 与代码分叉的地方以代码为准并在 §4.13a 小节里说出分叉）。

**FL 写入纪律（CLAUDE.md 🚦）**：代码话术（案例叙述、来龙去脉、被推翻的假设）进 FL：**触发器**进附录 E，**全文**进附录 D。CLAUDE.md 判据索引只在出现**一个新形状**时增一行。本轮候选新形状，**由 orchestrator 裁定，本任务不加进 CLAUDE.md**：

> 候选 #19：「**棘轮数的是被它约束的那个动作自己留下的标记 ⇒ 它只数成功**」——crash-loop 上限用被重触发 run 自己的 `RunStarted` 计数，于是在 `RunStarted` 之前崩溃的每一次都不计（#2）。判据 #13 说「上限的位置与寿命决定它约束什么」，#15 说「跨越之前先盖意图戳」；候选 #19 是两者交点上一个具体的形状：**计数器必须挂在意图上，不能挂在结果上**。若 orchestrator 认为它已被 #13/#15 覆盖，则只进附录 E.0 作 #13 的新实例，不进索引。

- [ ] **Step 1: FEATURE_LOCATOR §4.13a `#### round-3（2026-09-12）` 小节**，与 round-2 的 ⑩–⑯ 同一格式（每条一个加粗编号 + 机制 + 锚点 + 「⚠️」的陷阱 + 「打磨话术」段），条目：⑰ `append_batch` 与四种 Barrier 事件（`durability_of` 一张表；`PRAGMA synchronous` 逐事务切换；`/compact` `/undo` split 各一批；split 的 epoch 是唯一两步 + boot heal）· ⑱ `ResumeAttempted` 棘轮（`trailing_starts` 已删；`attempts`；`ResumeWithoutTarget`）· ⑲ `Unanswered` 判决（marker 切片看不见它；活动窗口；四张脸的 `unanswered`）· ⑳ fast-path 与 hook-stop 的 run 形状（`FastPathJournal`；`Error{HookStop}` 五条一批）· ㉑ `ToolCallParked` 与第四臂（三种 `ParkReason`；`PreHook` 的准确含义；`DanglingCallView.parked` + `never_ran()`；两张脸的文案）· ㉒ envelope 两条事实键（`RUN_ENVELOPE_FACT_KEYS`；恢复的 `/btw` 不扇出）· ㉓ 逐行解码（`v` / `ignorable` 信封；`UndecodableRecord` 第三 REJECT；`get_events` fail-closed；doctor `--fix` retire 单条；不加 `user_version` 门的理由）· ㉔ `launch_resume` / `settle` 与两趟重注入（`pending` 过包含的理由；`max_concurrent` 曾是零读者的断线）· ㉕ `orphan_notice` CUT 与 `adjudicate_orphaned_tasks` 三臂（`adjudicated_at_ms`；`task_prompt == ""` 是重触发）· ㉖ `usage_fold`（token 折叠；`cost_usd` 仍在 meta 的原因；boot 合成戳）· ㉗ process journal 的 `kind` / pid / 探活 / 两臂墓碑（`probe_liveness` 的 `Unknown` 不读作 Exited；`record_child` 无意图行是 no-op；PTY 与 bash 同表；三张脸；不杀）· ㉘ 六个新阶段各证明什么 + `attribute` 的 `subagent` 仪器 + `tombstone` 的 78。末尾「打磨话术」：「重启后一条消息没人回」→ `Unanswered` 与活动窗口；「审批时崩了恢复后说可能执行了」→ 第四臂；「某个会话日志读不出来整台不恢复」→ 逐行隔离 + doctor `fix=true`；「恢复一个一个来太慢」→ `[resume] max_concurrent`；「后台作业重启后说不存在」→ 墓碑 + `lost_with_restart`。
- [ ] **Step 2: 附录 D（全文）** — D.0 加「棘轮挂在结果上只数成功」（#2 的全文，含 `trailing_starts` 为什么看起来在工作）；D.4 加「两条 boot 臂两个判决」（`orphan_notice` vs resume）与「多步持久操作在 SQLite 上本可一事务」（三家 JSONL 做不到，Aleph 三写/七写却没用）。
- [ ] **Step 3: 附录 E（触发器）** — E.0 加两条：「你在给一个上限计数——它数的是**意图**还是**结果**？数结果的上限对"开始前就崩"盲」· 「你在写一个多步的持久操作——它在**同一个连接**上吗？是就进一个事务；不是就先写日志侧、后写缓存侧、boot 用日志 heal 缓存」；E.4 加三条：「一个悬空调用与一个停在门前的调用在日志里同形吗？——`ToolCallParked` 是那条区分它们的事实」· 「`load_run_markers` 整体失败 ⇒ 整台不恢复：解码失败要隔离到行」· 「boot 顺序里有没有一个 `await` 整个 run 的循环？」。每条带 FL §4.13a 的条目号指针。
- [ ] **Step 4: SESSION_KNOBS.md / SESSION_SERVICE.md / qa/README.md / CLAUDE.md 路由行**，按 Files 所列。CLAUDE.md 的其它任何行**不动**。
- [ ] **Step 5: 自查** — `grep -n 'trailing_starts\|orphan_notice\|close_open_run_after_retire\|balance_run_markers_after_retire' docs/reference/*.md CLAUDE.md qa/README.md` 只允许出现在「已删除 / 曾叫」的句子里；每个 `file:line` 锚点在当前 HEAD 上 `sed -n` 能看到它说的那个符号（判据 #18：数字带着它测于哪个 commit）。
- [ ] **Step 6: Commit** — `git add docs/reference/FEATURE_LOCATOR.md docs/reference/SESSION_KNOBS.md docs/reference/SESSION_SERVICE.md qa/README.md CLAUDE.md` · `docs: persistence r3 — FEATURE_LOCATOR §4.13a round-3, criteria appendices, knobs/service/qa docs` + `Claude-Session: https://claude.ai/code/session_01CUFAFPb3J96PtqLRLnPDq4`

---

## 验证记录（实测，逐条注明测于哪个 commit；执行者填，只写观测到的）

> 规则（r2 §0.1 原样）：一条「Verified:」必须能在你自己的 transcript 里找到那条真实的 cargo / bash 调用；数字带着它测于哪个 commit；变异先看红的**名字**是不是预期的那一份，再看条数；一次并行绿不算修好一个间歇性测试。

### T0 — 基线（commit: 00f38fc31）
- `--lib` 红名单条数 / 与记忆名单的名字差异：**51 条**（`test result: FAILED. 18493 passed; 51 failed; 17 ignored; finished in 683.84s`）。全名单已存 `<scratchpad>/baseline_failures.txt`（不进 git）。对比磁盘上找到的 2026-09-03 旧名单（`/c/Users/.../327cc4c4-.../scratchpad/baseline_failures.txt`，**17 行**——记忆 `windows-alephcore-lib-baseline-2026-09-02` 写的「18 条」与磁盘实测有一位漂移，以磁盘为准）：8 条两边都在（`acp::session::tests::test_spawn_and_drop_kills_child`、`harness::tests::budget::the_harness_line_budget_does_not_grow`、`mcp::transport::stdio::tests::{test_request_timeout_returns_mcp_timeout,test_spawn_echo_server,test_stdio_as_trait_object,test_stdio_implements_mcp_transport,test_timeout_configuration}`、`sandbox::worktree::tests::worktree_sandbox_executes_at_worktree_path`）；9 条只在旧名单（此次未复现，视为已解决）；**43 条新名字**，旧名单没有。
  - 43 条新增里至少 4 条可归因**本 worktree 未 `git submodule update --init`**（`git submodule status` 对 `skills`/`plugins` 均显示前导 `-`）：`config::tests::skill_doc_drift::{no_bundled_snippet_hardcodes_the_aleph_config_path,the_bundled_self_skill_names_no_phantom_config_keys}`、`extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`、`gateway::execution_engine::btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question`——panic 消息原话都含「the skills/ submodule is not checked out」。5 条 `builtin_tools::pdf_generate::tests::*` panic 于「output_path escapes the workspace output dir」/ engine fallback，像是本 worktree 路径场景相关（未深挖，超出 T0 范围）。
  - **非环境、疑似真实红，如实转述供后续任务参考**（T0 不做诊断，也不修）：`thinker::layers::extra_files::tests::blocks_prompt_injection_patterns`、`thinker::layers::identity_files::tests::blocks_prompt_injection_patterns`、`thinker::layers::soul::tests::workspace_soul_is_sanitized_against_injection`（三条都是 prompt-injection 防护断言失败）、`mcp::redact::tests::conservative_redact_is_case_insensitive_on_substring`（脱敏断言失败）、`gateway::pty::tests::a_write_reaches_a_real_subscriber_over_the_pty_screen_topic`（geometry 断言 `None` vs `Some((10,40))`）。`capability::census::tests::every_installed_global_is_a_capability_slot`（`50` vs 期望 `49`）与 `executor::builtin_registry::definitions::tests::catalog_description_bytes_ratchet`（超字节棘轮上限）两条棘轮测试本身注释已说明是已知漂移类型。**这些全部计入基线**——T1 起的红名单 diff 只应比对相对本文件这份基线**新增**的名字，以上条目不算本轮 r3 工作引入。
  - 未发生 WEDGE（`.done` 在第 3 轮 9.67 分钟轮询内出现，未触发 `--test-threads=1` 重跑）。
- `node --version`：`v24.13.0`（≥ v22.5 要求满足）；`tasklist //FI "IMAGENAME eq rustc.exe"` 启动前确认无其它 rustc 在跑。

### T1 — （commit: a4fac37bd · fix round 1: 72f888fd9）
- **RED**（改动前，只加了 brief 的测试）：`cargo check -p alephcore --tests` → `error: could not compile alephcore (lib test) due to 20 previous errors`，全部指向缺失的新 API：`E0599 no method named append_batch`×6、`E0433 cannot find type Durability`×8 / `Retire`×2 / `module fixtures`×1、`E0425 cannot find function durability_of / batch_durability / ignorable`×3。
- **GREEN**（a4fac37bd）：`cargo test -p alephcore --lib -- session::store session::events agents::subagent_spawner::fork gateway::session_projector session::actor` → `test result: ok. 85 passed; 0 failed; 0 ignored; 18481 filtered out`。
- **变异**（§11「4.1 retire out of the transaction」，各跑 `--lib -- session::store::tests`，30 条）：A = retire 挪到 `tx.commit()` 之后直接打 `conn` → 红名单恰 `retire_and_insert_are_one_transaction`（1/30），红在**第二条**断言 `[1] != [1, 6]`（成功批次的 commit 后 retire 把批次自己的 seq 6 也退休了）；brief 预测的 `[1,3,5] != [1,2,3,5]` 不是这条变异的形状——retire 在 commit 之后时，失败批次根本走不到 retire。B = retire 挪到 `BEGIN` 之前直接打 `conn` → 同一条测试红在**第一条**断言 `[1] != [1, 2, 3, 5]`（失败批次已经退休了 2/3/5）。两条断言各守一个方向。`git checkout -- src/session/store.rs` 还原，`grep -c MUTATION` = 0。
- **全量 `--lib`**（a4fac37bd）：`test result: FAILED. 18495 passed; 54 failed; 17 ignored; 0 measured; 0 filtered out; finished in 681.19s`。按名字 `comm -13 baseline_failures.txt t1_failures.txt`：**新增 3 条**，基线 51 条一条未少。三条全在本任务**没碰**的模块，且 `rg` 证明它们不引用 `session::{store,events}` / `SessionEventStore`：`skill::usage::tests::concurrent_bumps_do_not_lose_counts`（98/100）、`tools::usage::store::tests::concurrent_records_do_not_lose_counts`（99/100）、`thinker::runtime_context::tests::cached_repo_root_releases_lock_before_filesystem_io`（20 ms sleep 竞争，该测试自己的注释写明「under the full suite's parallel load」敏感）。单独重跑三条：`test result: ok. 3 passed; 0 failed`。前两条是记忆 `windows-alephcore-lib-baseline-2026-09-02` 记的 `with_file_lock` 争用家族（379c5bb1e 修复、T0 并行基线里也绿）——本轮读作**并行负载下的间歇**，不读作 T1 引入；但那条记忆也警告 Windows 共享 fs 基础设施的红要当 bug report 看，**转给控制者裁定**，不在 T1 范围内修。
- **A1 #3**：`let mut conn = self.conn.lock().await; conn.transaction_with_behavior(Immediate)` 与 `write_batch(&mut conn, …)`（形参 `&mut Connection`）在 `cargo check -p alephcore` 下直接通过——`tokio::sync::MutexGuard` 的 `DerefMut` 够用，无 E0596，不需要 `&mut *conn`。
- **A1 #6**：`a_batch_whose_third_row_collides_leaves_nothing_behind` 绿——第三行 PK 冲突后 `Transaction` 未 commit 即 drop，live 只剩预置的 `[3]`，即 rusqlite 默认 `DropBehavior::Rollback` 成立。
- **brief 测试的一处偏差**（实现未改）：`retire_and_insert_are_one_transaction` 末条断言 `search_events("old").is_empty()` 在 brief 自己的夹具下**不可能成立**——seq 1 是 `user_message("old")` 且 `Retire::From(2)` 不退休它，它合法地仍可搜到。改为断言命中恰为 `[1]`（退休的 2/3 已从 BM25 镜像删除、未退休的 1 未被误删），比 `is_empty` 更强。另 `barrier_restores_normal_after_success_and_after_failure` 里 brief 的 `sync` 闭包（返回借用实参的 async 块）编译报 `lifetime may not live long enough`，改为同体的嵌套 `async fn`，断言不变。
- `impl SessionEventStore` 计数：**3 处**（`store.rs` 生产实现、`actor.rs::CollideOnceStore`、`session_projector.rs::UnreadableRetirement`），`rg "SessionEventStore for"` 在 `src/` + `tests/` 下无第四处。`SessionEvent` 穷举 `match` 计数：**3 处**（`extract_turn_id`、`event_type_tag`、`fork.rs::is_prompt_bearing`），均已加 `ResumeAttempted` 臂。
- `fork/tests.rs` 里 `sample_of_every_kind` 实际范围 163–305（brief 写 311，尾行漂 6 行，首行准确）；删 163–306。
- **Fix round 1（72f888fd9）**：审查发现 Barrier 测试只读 commit **之后**的 `PRAGMA synchronous`，删掉 `=FULL` 的 raise 全绿（判据 #4）。改法：raise/restore 收进私有 `with_synchronous_full(conn, f) -> Result<T, SessionError>`（`f` 返回裸值，内部任何 `?` 都跳不过 restore；唯一提前返回是 raise 本身没生效——签名比裁定多包一层 `Result`，因为 raise 失败必须有出口，静默降到 NORMAL 却报 Barrier 是 fail-open）。新测试：`barrier_raises_full_for_the_transaction_and_only_for_it`（先把连接压到 NORMAL——裸内存连接默认就是 FULL，不压则删 raise 也读 2；闭包内跑真 `write_batch` 并读到 **2**，返回后读到 **1**，行已落库）与 `a_normal_batch_leaves_the_pragma_alone`（先 `=OFF`，Normal 批次后仍 **0**；再一个 Barrier 批次后 **1**——回的是 NORMAL 不是"之前的值"）。RED：`cargo check -p alephcore --tests` → `E0425 cannot find function with_synchronous_full`。GREEN：`cargo test -p alephcore --lib -- session::store::tests session::events::tests` → `test result: ok. 46 passed; 0 failed`。**变异 C**（删掉 raise，只留 restore）→ `--lib -- session::store::tests`：红名单恰 `barrier_raises_full_for_the_transaction_and_only_for_it`（1/32），`the transaction ran under FULL: left 1, right 2`；其余 31 条（含旧的 after-batch 断言）全绿——正是审查所指的盲区。`retire_in_txn` 改收 `&rusqlite::Transaction<'_>`：**变异 B′**（retire 挪到 `BEGIN` 前直接打 `conn`）现在是编译错误 `E0308 expected &Transaction<'_>, found &mut Connection`（`cargo check -p alephcore`），不再靠测试兜。`Retire::Through/From` 的 pub 文档写明闭区间与 `From` 同事务删 FTS 镜像。两次变异都 `git checkout --` 还原，`grep -c MUTATION` = 0；还原后 `cargo check -p alephcore` 绿。
### T2 — （commit: 7fda33366 · 控制者裁定 CUT `EmitEvent`: 44487baaa · fix round 1: a02908e3e）
- **RED**（改动前，只加了 brief 的测试 + 本地夹具）：`cargo check -p alephcore --tests` → `error: could not compile alephcore (lib test) due to 4 previous errors`，全部指向缺失的新 API：`E0599 no method named emit_batch`×3（in_process 三条新测试）、`E0599 no variant named EmitBatch`×1（actor 自愈测试改发批次）。
- **GREEN**（7fda33366）：`cargo test -p alephcore --lib -- session::actor session::in_process session::service` → `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 18551 filtered out; finished in 0.13s`（4 条新/改测试按名字都在：`actor_self_heals_seq_after_append_collision`、`a_batch_is_contiguous_observed_in_order_and_not_refired_on_replay`、`a_batch_with_retire_from_keeps_its_own_rows_live`、`an_empty_batch_is_refused_before_the_store`、`install_test_session_service_is_one_instance_and_fills_the_slot`）。
- **全量 `--lib`**（7fda33366 的工作树内容，提交前跑）：`test result: FAILED. 18503 passed; 52 failed; 17 ignored; 0 measured; 0 filtered out; finished in 681.05s`。按名字 `comm -13 baseline_failures.txt t2_failures.txt`：**新增 1 条** `skill::usage::tests::concurrent_bumps_do_not_lose_counts`（`98 != 100`——T1 全量跑记的三条 `with_file_lock` 争用家族之一，形状相同；另两条本次未复现），基线 51 条一条未少、一条未多；`session::*` 零红（21 条全绿）。按控制者指示不修、只转述。
- **`impl SessionService for` 计数：9 处**（`rg -n "impl SessionService for" src tests`）——1 处生产（`in_process.rs`）+ 6 处 `src/harness/tests/`（`act.rs` ×2：`MockSession`、`ToolErrorFailingSession`；`think.rs`、`task10_wiring/mod.rs`、`reactive_compaction.rs`、`guardrails.rs` 各 1）+ 2 处 `src/context/compact/`（`session_split.rs::RecordingSessionService`、`manual.rs::StoreBackedService`）。8 处非生产实现全靠 trait 默认 `emit_batch`（fail-closed `Err`）编译——lib test 构建绿即证明；`src/harness/` **零改动**（`git show --stat 7fda33366` 只有 `src/session/{actor,service,in_process}.rs`）。
- **夹具策略（控制者决定 2）**：`store.rs` 的 `turn_started/run_started/run_finished/user_message` 都带 `at: i64` 形参且在其私有 `mod tests` 内，与 brief 的 `turn_started(t)` / `user_msg(t, "hi")` 签名不同——改可见性也要改签名，不是一行的事 ⇒ 在 `in_process.rs` 的 test module 里**本地**加了 `test_store/turn_started/user_msg/run_started/run_finished`（`fresh_service` 改为包 `test_store`）。
- **对 brief 测试文本的三处偏差**：① 测试里锁的 `.unwrap()` 统一为 `.unwrap_or_else(|e| e.into_inner())`（P7；brief 自己的 observer impl 就这么写）；② `CollideOnceStore::append_batch` 两次调用都断言 `events.len() == 2`——brief 说「the retry re-sends the WHOLE batch」，只断 `first_seq` 证不出这句，行数才证得出；第二次 `head.store(3)` 而非 2（两行落地后 head 是 3）。③ 加了一条 `install_test_session_service_is_one_instance_and_fills_the_slot`（`Arc::ptr_eq` 两次调用 + `std::ptr::addr_eq` 对比槽里那份的数据地址）：该 helper 本任务内没有调用者（T3/T4/T8/T9 才用），不加测试就是一条 `dead_code` 警告，加 `#[allow]` 是零消费者抽象的 tell；这条测试钉的是 doc 里「同一实例」的承诺。lib 二进制内目前只有它一处往 `session/service` 槽装东西（生产 `start/mod.rs:476` 不在 lib test 里）。
- **未做**：brief 没给 T2 变异，本任务未跑变异。`an_empty_batch_is_refused_before_the_store` 只断 `Err(Other(_))`——`SqliteEventStore::append_batch` 对空批次也返回同一变体，这条测试**分不出**「actor 拒」和「store 拒」，名字比断言强（判据 #17 的形状）；按 brief 原样保留，转控制者裁定要不要换成计数 store。`ActorCommand::EmitEvent` 在生产侧**没有发送者了**（`in_process::emit_event` 走 `emit_batch`），只剩 `actor.rs` 测试发它——是留还是 CUT，转控制者。
- **CUT `EmitEvent`（44487baaa，控制者裁定：零消费者的臂按 Global Constraints 熵规则 CUT，压过 brief 的保留）**：删掉变体、`handle_emit_one`、热臂与 drain 臂两条分派；`actor.rs` 三条测试改发 `EmitBatch { events: vec![event], retire: None, reply }`，原来断单个 seq 的地方改断一元素 `Vec<EventSeq>`（`vec![1]` / `.len() == 1` / `vec![4]`）；`in_process.rs:995` 一条提到 `EmitEvent` 的 doc 注释改写。`rg "EmitEvent|handle_emit_one" src tests` → 零命中。`cargo check -p alephcore --tests` 绿；`cargo test -p alephcore --lib -- session::actor session::in_process session::service` → `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 18551 filtered out; finished in 0.12s`。按控制者指示未重跑全量 `--lib`（`EmitBatch` 是唯一写命令，`in_process.rs` 生产路径没变）。`an_empty_batch_is_refused_before_the_store` 的名字问题留给审查者。
- **Fix round 1（a02908e3e）**：审查发现空批次测试跑在 `SqliteEventStore` 上分不出「actor 拒」和「store 拒」（判据 #4：删掉 actor 闸测试照绿）。改法：`actor.rs` 测试模块一个 `ScriptedStore` 替代 `CollideOnceStore`——`append_batch` 把每次调用记成 `(first_seq, rows)`、前 N 次以编号 `Storage` 错误失败（每次失败 head+1，模拟直写者抢走那个 seq）、之后成功（head += 行数）；断言全在测试里对录音做，不在 double 里 panic。三条测试：① `an_empty_batch_is_refused_before_the_store`（actor 层）断 `Err(Other)` **且**录音为空**且** observer 计数 0；② `actor_self_heals_seq_after_append_collision` 断回复 `[2,3]`、录音恰 `[(1,2),(2,2)]`（一次冲突一次重试、两次都是整批）、observer 2 次；③ 新 `a_second_failure_is_the_callers_error_and_nothing_fires`：两次都失败 → 回复是**第二次**的 `Storage("scripted failure #2")` 原文、录音恰两条（有界）、observer 0、broadcast `try_recv` 为 `Empty`。服务面那条改名 `an_empty_batch_error_reaches_the_service_caller`（它能证的只是 `Err` 穿透到调用者）。另两条 minor：`handle_emit_batch` 的 seq 只推导一次（`seqs.iter().copied().zip(pairs)`，判据 #12）；新 `a_retire_only_batch_appends_nothing_and_shrinks_the_live_log`（预置 3 行，`emit_batch(vec![], Some(Retire::From(2)))` → `Ok(vec![])`、live `[1]`、下一次 append 落在 4——用 `From(2)` 而非控制者写的 `From(1)`，留一行活着才证得出边界）。GREEN：`cargo test -p alephcore --lib -- session::actor session::in_process` → `test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 18553 filtered out; finished in 0.14s`。**变异**（删掉 `handle_emit_batch` 的空批次闸，跑同一过滤器）：红名单恰 `session::actor::tests::an_empty_batch_is_refused_before_the_store`（1/22，`assertion failed: matches!(rrx.await.unwrap(), Err(SessionError::Other(_)))`——double 对空批次回 `Ok`，第一条断言就红）；改名后的服务面测试在变异下**仍绿**（真 store 拒了），正是它证不了那句话的实证。`git checkout -- src/session/actor.rs` 还原，`grep -c MUTATION` = 0，`git status --porcelain` 空，`cargo check -p alephcore --tests` 绿。未重跑全量 `--lib`（本轮只动测试与一处等价重写）。
### T6 — （commit: 45448caba · fix round 1: 9a47a4344）
- **RED**（改动前，只加了 brief 的测试 + 夹具）：`cargo test -p alephcore --lib -- session::reduction session::store::tests::marker_event_types` → `EXIT=101`，`error: could not compile alephcore (lib test) due to 21 previous errors`，全部指向缺失的新 API：`E0559 variant RunDisposition::Interrupted has no field named attempts`×12（另 `E0026`/`E0027` 各 1，g1 的模式匹配）、`E0599 no variant named ResumeWithoutTarget`×3、`E0599 … IntentStampFailed for enum ResumeRefusal`×2、`E0603 function is_marker is private`×1、`E0425 cannot find value MARKER_EVENT_TYPES`×1。
- **类型检查**：`cargo check -p alephcore` → `Finished 1m 08s`（4 条 `never used` 警告全在未触碰文件：`generation/providers/replicate/builder.rs`、`mcp/manager/handle.rs`、`pii/engine.rs`、`verification/tool_loop_verifier.rs`）；`cargo check -p aleph-protocol` → `Finished 7.85s`，零警告。
- **GREEN**：`cargo test -p alephcore --lib -- session::reduction session::store::tests session::marker_balance session::events gateway::resume_coordinator gateway::session_snapshot gateway::handlers::resume` → `test result: ok. 146 passed; 0 failed; 0 ignored; 0 measured; 18435 filtered out; finished in 1.07s`（新/改测试按名字都在：`attempts_count_intent_stamps_not_trailing_starts`、`a_stamp_with_nothing_to_resume_is_reported_and_ignored`、`bare_run_starts_after_a_finish_are_interrupted_with_no_attempts`、`g1::attempts_are_the_stamps_since_the_last_finish`、`store::tests::{load_run_markers_returns_resume_attempted_as_a_marker, marker_event_types_are_exactly_the_reducers_marker_set}`、`resume_coordinator::tests::classify_counts_stamps_since_last_finish`、`events::tests::resume_attempted_event_round_trips_through_json`）。`cargo test -p aleph-protocol` → `360 passed; 0 failed`。
- **集成二进制**：`cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1` → 编译并真跑（无 E0786）：加 witness 测试前 `17 passed; 0 failed`，最终 `test result: ok. 18 passed; 0 failed; 0 ignored; finished in 0.29s`（`a_retrigger_that_never_starts_is_capped_by_the_intent_stamp`、`the_intent_stamp_is_durable_before_the_engine_is_handed_the_run`、`crash_loop_cap_abandons_instead_of_retriggering` 都 ok）。
- **变异 M1**（brief 第 5 步：`stamp_resume_attempt` 挪到 `self.retrigger(...)` **之后**）：跑集成二进制 → **`17 passed; 0 failed`，brief 预测的红（`calls.len() == 4`）没有发生**——`RecordingAdapter` 立即返回 `Ok`，戳照样落地（只是晚了一条指令），棘轮照样爬：三次 boot 后仍 abandon、`calls.len() == 2`。brief 自己的测试守的是戳的**数目**，不是戳相对 retrigger 的**顺序**（判据 #4）。补 `StampWitnessAdapter`（在 `execute` 内部读日志、记下此刻已落地的 `ResumeAttempted` 数）+ `the_intent_stamp_is_durable_before_the_engine_is_handed_the_run`（两次 boot 断 `[1, 2]`）：M1 下 → `test result: FAILED. 17 passed; 1 failed`，红名单恰这一条，`left: [0, 1]  right: [1, 2]`；其余 17 条（含 brief 那条）仍绿。还原后 `grep -c MUTATION` = 0，集成二进制 `18 passed; 0 failed`。T10 `ratchet` 阶段尚未建，未测。
- **变异 M2**（额外，lib 级：`reduce_disposition` 改回旧的 trailing-`RunStarted` 计数）：`--lib -- session::reduction session::store::tests session::marker_balance gateway::resume_coordinator gateway::session_snapshot`（113 条）→ `102 passed; 11 failed`，红名单全是 intent 侧：`reduction::tests::{attempts_count_intent_stamps_not_trailing_starts, bare_run_starts_after_a_finish_are_interrupted_with_no_attempts, a_dangling_call_under_a_finished_run_is_reported_as_earlier, g1::attempts_are_the_stamps_since_the_last_finish}`、`resume_coordinator::tests::{classify_counts_stamps_since_last_finish, classify_interrupted_single_dangling_start, classify_interrupted_when_no_finish_at_all}`、`session_snapshot::last_run_tests::{an_interrupted_log_carries_the_reducers_numbers, the_list_face_agrees_on_the_word_without_claiming_to_have_looked}`、`marker_balance::tests::{a_rewind_that_cut_the_finish_leaves_a_clean_log, an_open_run_on_a_running_session_is_left_alone}`；`a_stamp_with_nothing_to_resume_is_reported_and_ignored` 如预期仍绿（它钉的是 `reduce_run` 的报告，旧计数对那条日志也读 `Clean`）。还原，`grep -c MUTATION` = 0。
- **全量 `--lib`**（最终工作树内容，提交前跑）：`test result: FAILED. 18513 passed; 51 failed; 17 ignored; 0 measured; 0 filtered out; finished in 681.31s`。按名字 `comm -13 baseline_sorted.txt t6_failures.txt` → **空**；`comm -23` → **空**：与 T0 基线 51 条逐名相同，零新增零消失（本次连 `with_file_lock` 争用家族那三条都没复现）。
- **读者普查（判据 #6）**：`RunDisposition::Interrupted` 生产读者 **7 处**（定义、`reduce_disposition` 构造、`session_snapshot` 两张脸、`resume_coordinator` 主臂——5 处改；`resume_coordinator.rs` delegated 臂与 `projection_reconciler.rs` 用 `{ .. }` 不动）；测试字面量 9 处全部改 `attempts: 0`；wire 键 `LastRunState.trailing_starts` 只有 `session_snapshot` 两处生产者 + protocol 定义 + `interfaces/{tui,webchat}` 两处**测试夹具**构造，`rg "\.trailing_starts" interfaces shared` 零读者——没有客户端渲染它。
- **通配臂审计**：66 个含 `SessionEvent::` 的文件、每个 `_ =>` 臂逐一看；`ResumeAttempted` 在 `projection.rs::project_row`（`messages` 投影）、`fork.rs::is_prompt_bearing`（fork 种子）、`store.rs::render_event_text`（FTS）、`session_split.rs`（摘要输入）、`harness/agent/prompt.rs:239`（模型上下文，R10 只读）全部落进忽略臂——它只到 `load_run_markers → reduce_*`。全表在 `task-6-report.md`。brief 之外**零**产品改动。
- **对 brief 文本的偏差**：① 集成测试的 `boot` 闭包——brief 的 `|n| async { … }` 返回借用闭包捕获物的 future，改为闭包同步建 coordinator、返回 `async move { c.resume_interrupted_runs().await }`，断言不变；② 第 5 步变异的预测不成立（见 M1），补了 witness 测试；③ 额外测试：`legal_shapes()` 加「crash-loop with an intent stamp」（T6 之后生产真正写出的序列）、`events.rs` 的 JSON 往返、`handlers/resume.rs` 手写名单加 `IntentStampFailed`；④ `reduction.rs` 模块/枚举 doc 里「seven REPORT」「nine words」「a tenth variant」三个数字**删掉**而非改成八/十——T11、T14 还要各加一种；⑤ `session_snapshot.rs:323,427` 两条 wire 值断言 `1 → 0`（值的含义变了）；⑥ `shared/protocol/src/resume.rs` 的 reason 词表 doc 补 `intent_stamp_failed`（文件不在 brief 名单，纯 doc）。
- **未做 / 转控制者**：`every_refusal_has_its_own_reason_word` 与 `the_other_refusals_do_not_claim_an_inconsistent_log` 都是手写变体名单（判据 #5），新变体不加进去什么都不红；`FEATURE_LOCATOR.md:2448`「七种 REPORT」现已是八种，未改（文档归 T17/T24）；边界修复**过程中**的崩溃仍不计数（戳按 brief 放在修复之后）；`rustfmt --check` 只对本次十个叶子文件（2 处自己 hunk 内的格式差异手工改平，复查 0），`cargo fmt --check -p alephcore` 的大量既有差异未动。
- **Fix round 1（9a47a4344）**：审查三条 Important + 两条 minor。① `ResumeWithoutTarget` 的 doc 写的「ignored — the disposition is what it would be without the stamp」与 `reduce_disposition` 的计数读法相反（`[RunFinished a, ResumeAttempted, RunStarted b]` → 报告**且** `attempts: 1`，g1 属性钉的正是计数）：裁定计数读法为准，改 doc（「若其后、下一个 `RunFinished` 之前有 `RunStarted`，它仍算一次 attempt；`reduce_disposition` 只看 marker；报告告诉操作者写它时没有对象」），测试改名 `a_stamp_with_no_open_run_is_reported_and_counts_only_if_a_run_follows`，两种读法并排钉住（`[started, finished, attempted]` → `Clean` + 报告；`[finished, attempted, started]` → `Interrupted { attempts: 1 }` + `[finish-without-start, resume-without-target]`）。② 戳刷新了「最后活着」时间：`last_alive_at` 取 `max(progress.last_activity_at, 最后一个 marker.created_at_ms)`，两者现在都看到最新的 `ResumeAttempted`，`max_age_secs` 变成从**上一次尝试**量而非从中断量；`handle_interrupted` 里「The dangling RunStarted is the last marker」已是假话。裁定协调器的 intent 记录不是 run 的活动：`reduction.rs` 的 `last_activity_at` 排除 `ResumeAttempted`；`handle_interrupted` 改从**同一份 reduction** 的 `run_anchor` 在 `events` 里找到 `RunStarted` 记录（`run_started`），它同时给 `last_alive_at` 计时与给戳当 `target`；`markers` 形参删掉（唯一读者就是那句假话），`last_alive_at` 形参改名 `run_started` 并写明理由；「no RunStarted anchor」的拒绝随之前移到 reduction 之后（变体与措辞不变，审查已列为 deferred）。新测试：`reduction::tests::last_activity_at_is_not_refreshed_by_an_intent_stamp`（`[started, requested, attempted]` → `Some(20)`；只有戳 → `None`）、`resume_coordinator::tests::recency_is_not_refreshed_by_an_intent_stamp`（戳 900_000 之后仍答 2_000 / 只戳时答 1_000）、集成 `a_fresh_stamp_does_not_resurrect_a_run_interrupted_too_long_ago`（两天前的 `RunStarted` + 刚刚的戳 → `(abandoned, resumed) == (1, 0)`，adapter 零调用）。③ fail-closed 臂零测试：集成测试加 `StampRefusingStore`（包一个真 `SqliteEventStore`，批次含 `ResumeAttempted` 时 `append_batch` 返回 `Storage("disk full: stamp refused")`，其余全部透传）+ `a_stamp_that_does_not_land_refuses_the_resume_without_retriggering`：`(scanned, resumed, abandoned) == (1, 0, 0)`、`refused == [(sid, IntentStampFailed(_))]`、`reason() == "intent_stamp_failed"`、adapter 零调用、边界修复的 `ToolError` 已落地、无 `RunFinished{Abandoned}`、日志里无 `ResumeAttempted`。④ `attempts + 1` → `attempts.saturating_add(1)`。⑤ 本 header 改写 SHA。**类型检查**：`cargo check -p alephcore` → `Finished`（仍只有那 4 条既有警告）。**GREEN**（最终内容，rustfmt 手工整平后重跑）：`cargo test -p alephcore --lib -- session::reduction gateway::resume_coordinator` → `test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 18518 filtered out; finished in 1.09s`；`cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1` → `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s`（新测试按名字都在）。**变异 M4**（审查要求：`Err` 臂只 warn、落到 `retrigger`）→ 集成二进制 `19 passed; 1 failed`，红名单恰 `a_stamp_that_does_not_land_refuses_the_resume_without_retriggering`（`left: (1, 1, 0)  right: (1, 0, 0)`——resumed 从 0 变 1）。**变异 M3**（`last_activity_at` 的排除去掉）→ `--lib -- session::reduction gateway::resume_coordinator` `63 passed; 2 failed`，红名单恰 `last_activity_at_is_not_refreshed_by_an_intent_stamp`（`Some(30) != Some(20)`）与 `recency_is_not_refreshed_by_an_intent_stamp`（`900000 != 2000`）；集成那条未在 M3 下单独跑（同一机制）。两次都还原，`grep -c MUTATION` = 0。① 是 doc + 钉现有行为，没有 RED 可取；② 的 RED 由 M3 充当。未重跑全量 `--lib`（改动限于 `reduction.rs` 一处谓词、`resume_coordinator.rs` 的 anchor 读取与集成测试）。
### T7 — （commit: 72c9a63a3 · fix round 1: 7f8d1a32c）
- **RED**（改动前，只加了 brief 的测试 + 本地夹具）：`cargo check -p alephcore --tests` → `error: could not compile alephcore (lib test) due to 11 previous errors`，全部指向缺失的新 API：`E0599 no variant named Unanswered found for enum reduction::RunDisposition`×5、`E0599 … TailReadFailed for enum ResumeRefusal`×2、`E0425 cannot find function unanswered_eligible`×2、`E0599 … Unanswered for enum LastRunDisposition`×1、`E0425 cannot find function is_disposition_bearing`×1；`cargo check -p aleph-protocol --tests` → 2 errors（`no associated … UNANSWERED`、`no variant … Unanswered`）；`cargo check -p aleph-tui --tests` → 3 errors（同上 ×2 + 测试模块未 import `last_run_notice`）。
- **类型检查**（改动后）：`cargo check -p alephcore` / `--tests` / `-p aleph-protocol --tests` / `-p aleph-tui --tests` 全部 `Finished`（alephcore 仍只有 T6 记的那 4 条既有 `never used` 警告）。
- **GREEN**（72c9a63a3 内容）：`cargo test -p alephcore --lib -- session::reduction gateway::resume_coordinator gateway::session_snapshot gateway::projection_reconciler gateway::handlers::resume session::marker_balance session::boundary_repair` → `test result: ok. 122 passed; 0 failed; 0 ignored; 0 measured; 18467 filtered out; finished in 4.03s`（新/改测试按名字都在：`reduction::tests::{a_seeded_message_with_no_run_started_is_unanswered, unanswered_attempts_count_only_the_stamps_after_the_message, reduce_run_reads_the_unanswered_tail_from_a_full_log, user_message_producers_are_the_known_set, every_prefix_of_every_legal_shape_is_green, the_finish_without_start_allowance_is_exercised_where_the_shape_produces_it, g1::*}`、`resume_coordinator::tests::{every_refusal_has_its_own_reason_word, unanswered_eligibility_excludes_scheduled_child_and_ephemeral_sessions}`、`session_snapshot::last_run_tests::an_unanswered_log_is_reported_as_such`、`handlers::resume::tests::the_other_refusals_do_not_claim_an_inconsistent_log`）。`cargo test -p aleph-protocol` → `360 passed; 0 failed`。`cargo test -p aleph-tui` → `435 passed; 0 failed`（`picker_marks_unanswered_and_the_notice_names_the_fact` ok）。`cargo test -p aleph-panel --lib`（分离式，两次：改动后与 rustfmt 整平后各一次）→ 均 `test result: ok. 1272 passed; 0 failed`（`last_run_face_tests::an_unanswered_last_run_is_news_and_badges_the_row` ok）。`just wasm`（Bash 工具）→ `Finished wasm-release … 7m 26s` + `✓ panel dist OK`（**首跑失败**于 tailwind `Can't resolve 'tailwindcss'`：recipe 只把主检出的 `.bin` 放进 PATH，但 tailwind v4 从 worktree 的 `styles/` 解析 `@import "tailwindcss"`，worktree 无 `node_modules`——建了一个 gitignored 的目录 junction 指向主检出的 `node_modules` 后重跑绿；`just wasm` 会改写**被 git 跟踪的** `interfaces/webchat/dist/*`，验证后 `git checkout -- interfaces/webchat/dist/` 还原，未入提交）。集成二进制 `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1` → `test result: ok. 23 passed; 0 failed`（20 旧 + 3 新：`an_unanswered_seed_is_stamped_and_retriggered_without_repair`、`an_unanswered_seed_behind_its_own_stamp_is_seen_on_the_next_boot`、`an_unanswered_seed_is_capped_by_its_own_stamps`）。
- **变异 M1**（brief 第 5 步：`reduce_disposition` 去掉 `synthetic: false` 守卫）→ 同一 `--lib` 过滤器 `121 passed; 1 failed`，红名单恰 `a_seeded_message_with_no_run_started_is_unanswered`（`left: Ok(Unanswered { user_seq: 3, attempts: 0 })  right: Ok(Clean)`——合成消息被读成"用户在等"）。**变异 M2**（brief 第 5 步：删掉 `resume_interrupted_runs` 的活动窗口循环）→ 集成二进制 `22 passed; 1 failed`，红名单恰 `an_unanswered_seed_is_stamped_and_retriggered_without_repair`（`left: (0, 0, 0)  right: (1, 1, 0)`——无 marker 的会话谁都不看）；T10 `unanswered` 阶段尚未建，未测。**变异 M3**（自加：`check_unanswered` 的 tail 读改回 brief 原文的「上一个 **marker** 之后」）→ 集成二进制 `21 passed; 2 failed`，红名单恰 `an_unanswered_seed_behind_its_own_stamp_is_seen_on_the_next_boot`（第二次 boot `(scanned, resumed, skipped) = (1, 0, 1)`，被盖了戳的种子被归成 `skipped`＝`already_finished`）与 `an_unanswered_seed_is_capped_by_its_own_stamps`（`abandoned 0`，戳数永远读不到）。三次都 `cp` 备份还原，`grep -c MUTATION` = 0，还原后 `cmp` 与备份逐字节相同。
- **全量 `--lib`**（72c9a63a3 的工作树内容，提交前跑）：`test result: FAILED. 18521 passed; 51 failed; 17 ignored; 0 measured; 0 filtered out; finished in 664.01s`。按名字 `comm -13 baseline_sorted.txt t7_failures.txt` → **空**；`comm -23` → **空**：与 T0 基线 51 条逐名相同，零新增零消失（`skill::usage::tests::concurrent_bumps_do_not_lose_counts` 本次未复现）。
- **判据 #6 读者普查**（`rg "reduce_disposition\(|reduce_run\(" src tests`，剥注释）：生产调用 **11 处**。`reduce_disposition(` ×4：`reduction.rs::reduce_run`（**bearing**，本任务改）、`resume_coordinator.rs::resume_from_markers`（marker，`load_run_markers`）、`resume_coordinator.rs::check_unanswered`（**bearing**，新增：finish 前的 markers ∪ 过滤后的 tail）、`projection_reconciler.rs::candidates`（marker）。`reduce_run(` ×7：`resume_coordinator.rs::handle_interrupted`（全日志）、`session_snapshot.rs::last_run_from_events`（全日志 ⇒ 能说 `unanswered`）、`session_snapshot.rs::last_run_from_markers`（marker 切片 ⇒ **永远说不出** `unanswered`，臂仍渲染以防更宽的切片折成 `clean`）、`boundary_repair.rs:243`（全日志，只读 `dangling`/`open_run`）、`marker_balance.rs:56`（全日志，只读 `open_run`）、`diagnostics/checks/session_log.rs:169`（全日志，只读 `contradictions`）、`agents/subagent_tool/recovery.rs:786`（子日志 own-scope，只读 `progress`/`dangling`/`contradictions`）。`RunDisposition` 穷举 `match` 生产 4 处全部加臂（`session_snapshot` ×2、`resume_coordinator::resume_from_markers`、`projection_reconciler::candidates`）。
- **判据 #17 渲染者**：wire `shared/protocol/src/session_thread.rs:484`（`UNANSWERED`）`:512`（`disposition()` 臂）`:542`（变体 + doc）；生产者 `src/gateway/session_snapshot.rs:169`（attach 面）`:222`（list 面，不可达）；Panel 通知 `interfaces/webchat/src/components/chat_sidebar.rs:389` → `narration.last_run_unanswered`（`en.json:2708` / `zh.json:2708`）；Panel 徽章 `chat_sidebar.rs:416` → `RunBadge::Unanswered`（`:437`）→ `label` `:448` → `chat.run_badge_unanswered`（`en.json:308` / `zh.json:308`）；TUI 标记 `interfaces/tui/src/tui/commands.rs:549`（`"  [unanswered]"`）、TUI 通知 `:1102`；协议 doc 同上；**子代理面无渲染者且不需要**——`unanswered_eligible` 排除 `Subagent`/`Ephemeral` 键，`subagent_tool::recovery` 不读 `disposition`（`rg "\.disposition\b" src` 证）。拒绝词 `tail_read_failed` 是 wire 上的透传字符串，无客户端 switch，只补了 `shared/protocol/src/resume.rs` 的词表 doc。
- **A2 #3**：是。`execute.rs:497` `ensure_session_under_request_scope` → `AgentInstance::ensure_session` → `SessionStore::get_or_create`（file backend `file_backend/mod.rs:484-501` 已存在分支 `last_active_at = now`；sqlite `session_manager/ops/crud.rs:91-98`），在 fast path（`execute.rs:787`）与 agent loop（`:885` → `harness_bridge::runner_impl.rs:342 seed_session`）**之前**；`SimpleExecutionEngine`（`simple.rs:92`）同样在其 `UserMessage` emit（`:146`）之前。steer（`steering.rs:752`）只写进正在运行的会话；`completions/agent.rs`（Ephemeral）与 `subagent_spawner`（Subagent）被 `unanswered_eligible` 排除；`backfill.rs` 后必有 run。
- **A2 #5**：**否**——`exec_adapter` 只在真实执行分支赋值（`agent_init/mod.rs:1496`），Simulated 回退分支只建 `SimpleExecutionEngine` 给 `AgentRunManager`（`:1889-1899`）、`exec_adapter` 保持 `None`；`start/mod.rs:3016` `if let (Some(exec_adapter), Some(registry))` 不成立 ⇒ **不构造 `ResumeCoordinator`**，走 `decline_global_resume_coordinator`。且 `simple.rs` **不读 `is_resume()`**（零处 `resume`），一旦有人把它接上，`FlowInput::Resume` 的空 `input` 会被当成一条新 `UserMessage` 落盘。按指示未改。
- **A2 #7**：普查图在本 commit 由测试实测得出：`agents/subagent_spawner/mod.rs: 1, gateway/execution_engine/fast_path.rs: 2, gateway/execution_engine/simple.rs: 1, gateway/execution_engine/steering.rs: 1, gateway/openai_api/completions/agent.rs: 1, orchestrator/harness_bridge/backfill.rs: 1, orchestrator/harness_bridge/session_seed.rs: 3, session/events.rs: 1`（与控制者裁定一致；brief 的 `events.rs: 2` / `session_seed.rs: 2` / 无 `fast_path.rs` 是 T8 之后的树）。**仪器盲区**：第一次跑普查多出 `harness/tests/{act,agent,guardrails,prompt,reactive_compaction,stability,task10_wiring/mod,think}.rs` 与 `guardrails/tests/registry.rs` 共 9 个**纯测试文件**——它们由祖父 `mod.rs` 里**内联的** `#[cfg(test)] mod tests { mod act; … }` 块声明，`source_scan::declared_as_a_test_module` 只认三种形状（`dir/mod.rs`、`dir.rs` 里的 `mod x;`），对这第四种整文件返回 production；其自测 `production_text_empties_whole_file_test_modules_and_only_those` 的推导共享同一盲区（它把 `mod act;` 解析成 `src/harness/act.rs`）。**未改 `source_scan.rs`**（brief 之外，且改它会移动其它 census 的计数）；在本普查内用仪器自己的 `cfg_test_portion` 加了局部 `under_inline_cfg_test_block`（祖先目录逐级问父模块的 cfg-test 部分是否开了 `mod <dir> {`），带自检（它必须真的跳过了 `harness/tests/act.rs` 且 `production_text` 对该文件仍返回非空——仪器学会这形状那天自检变红、删助手）。转控制者：要不要把第四种形状教给 `declared_as_a_test_module` 本身。
- **A2 #8**：**没有** Rust 测试钉 `en.json` ⇄ `zh.json` 键对等（`rg "zh.json" interfaces/webchat/src` 零命中）。唯一的检查是 `build.rs` 里 `leptos_i18n_build` 对两份 locale 的解析——非默认 locale 缺键在 0.6 是 build **warning**（回退到 `en`），不是红。两个键都写进了两份文件。
- **对 brief 文本的偏差**（实现侧，全部有测试钉住）：① `check_unanswered` 的 tail 读起点是上一个 **`RunFinished`** 之后而非上一个 marker 之后（M3 实证 brief 原文让被盖过戳的种子从第二次 boot 起永远 `skipped`）；prefix 取 `seq < from` 的 markers、无 finish 时 prefix 为空，两半按 seq 不交。② `Unanswered.attempts` 只数**该消息之后**的戳（控制者裁定；`unanswered_attempts_count_only_the_stamps_after_the_message` 钉住 `[finished, attempted, user]` → 0）。③ `resume_from_markers` 不加单独的 `Unanswered => skipped += 1` 臂，而是 `Clean | Unanswered` 共臂走 `check_unanswered`——marker 切片说不出它，但如果哪天有人递宽切片，把它记成 `skipped`（渲染为 `already_finished`）正是判据 #17 的错标签；共臂让答案从 tail 重新推导。④ `handle_unanswered` 的 `user_at` 是 `Option<Timestamp>`（在 bearing 里按 seq 找、0 视为未知）而非 brief 的 `unwrap_or(0)` 再判 0。⑤ `unmarked_activity_reads_as_earlier_run_with_no_open_run`（③-D2 `[finished, user, requested]`）的断言从 `Clean` 改为 `Unanswered { user_seq: 4, attempts: 0 }`——brief 自己的 reducer 对这个形状就是这么读的（dispatch 不 bearing），brief 未列此测试。⑥ Panel/TUI 里两条**旧**测试的 `unanswered` 一词原指"服务端什么都没说"，与新词撞名：改名 `a_silent_or_clean_row_carries_no_badge`、`a_clean_or_silent_row_is_unmarked`、`an_absent_last_run_says_nothing`（断言不变）。⑦ `shared/protocol/src/session_thread.rs` 两处计数散文（「four constants」「three above」）删数字。
- **未做 / 转控制者**：(a) ③-D2 形状（RunStarted append 失败、run 已派发工具、崩溃）现在读作 `Unanswered`，Unanswered 臂**不做边界修复**（brief 设计），重触发后 replay 里那条悬空 `tool_use` 由 `harness/agent/prompt.rs` 的 orphan-drop 静默丢掉——与今天 `EarlierRun` 悬空调用的既有行为相同（Clean 会话本来就无人修复），但模型不会被告知"结果未知"；要修就得让 Unanswered 臂跑 `reduce_run` 全日志，是设计变更。(b) 同一形状在 attach 面会同时带 `unanswered` 与非空 `dangling`，Panel/TUI 的 Unanswered 句子**不渲染** dangling 计数（brief 的句子无计数位）。(c) 无 marker 的种子重触发时 `ResumePlan::default()`——崩溃前请求的 `project_root`/knobs 没有任何 marker 冻结，run 回到 agent 默认工作区且**没有 degrade note 告诉模型**（Interrupted 臂有）；brief 接受"plan 为空"，但这条静默值得记。(d) `abandon()` 的用户通知文案仍是「An interrupted run … could not be resumed」，reason 括号里才说"the unanswered message was too old"——brief 说"同一个 closer"，文案未改。(e) `just wasm` 在 worktree 里的 tailwind 解析问题是 recipe 缺陷（只修了 `.bin`，没修 `@import` 解析），本轮用 junction 绕过、未改 `justfile`。(f) 未跑 clippy（不在 T7 验证集）。
- **Fix round 1（7f8d1a32c）**：审查四条 Important + 七条 minor。① `Interrupted` 压过 `Unanswered` 的优先级在全日志层面没有 pin（把 `reduce_disposition` 两个检查互换，整套绿）：加 `reduction::tests::a_seed_a_run_picked_up_is_interrupted_not_unanswered`（`[turn_started, user, started, requested]` → `Interrupted { attempts: 0 }`；bearing 切片 `[user, started]` 同；种子前有戳 `[started a, finished a, attempted, turn_started, user, started b]` → `Interrupted { attempts: 1 }`）与 `session_snapshot::last_run_tests::a_seed_a_run_picked_up_is_interrupted_on_the_attach_face`（同一日志 → `Interrupted`、`run_id = run-a`、dangling `[call-1]`）。**变异 SWAP**（用户扫描挪到 `RunStarted` 检查之前）→ `--lib -- session::reduction gateway::resume_coordinator gateway::session_snapshot gateway::projection_reconciler gateway::handlers::resume session::marker_balance session::boundary_repair`：`123 passed; 2 failed`，红名单恰这两条（`left: Unanswered { user_seq: 2, attempts: 0 }  right: Interrupted { attempts: 0 }` / `left: Unanswered  right: Interrupted`），其余 123 条全绿——审查说的"整套绿"实证。② `TailReadFailed` 零行为测试：T6 的 `StampRefusingStore` 泛化成 `FaultingStore { inner, fault: Fault::{StampAppend, TailRead} }`（一份委托、一次一个故障），新集成测试 `a_tail_that_cannot_be_read_refuses_without_stamping_or_retriggering`：`(scanned, resumed, skipped, abandoned) == (1, 0, 0, 0)`、`refused == [(sid, TailReadFailed(_))]`、`reason() == "tail_read_failed"`、adapter 零调用、零戳、日志仍 2 条。③ on-demand 面无 marker 分支零测试：`an_on_demand_resume_of_a_marker_less_unanswered_seed_stamps_and_retriggers`（`resume_session` on `[turn_started, user]` → `(scanned, resumed, skipped) == (1, 1, 0)`、戳 `[(2, 1)]`、一次 `resume=true` 调用）。**变异 A+B**（同一次构建：A = 该分支退回零报告；B = tail 读 `Err` 折成空 tail）→ 集成二进制 `23 passed; 2 failed`，红名单恰 ③ 与 ②（各自 `(0,0,0) != (1,1,0)` / `(0,0,0,0) != (1,0,0,0)`）。④ `abandon()` 对无 run 的会话说「An interrupted run … was abandoned」：加私有 `enum Abandoned { InterruptedRun, UnansweredMessage }`，`notice(reason)` / `goal_note(reason)` 各两句真话（unanswered 臂：「Your last message … was never picked up by a run, and after a restart it could not be retried (…); it was abandoned — send it again if you still need it」），closer 不变；`the_abandon_sentences_name_the_thing_that_was_abandoned` 钉两臂互异、unanswered 句不含 "interrupted run"、都带 reason 与「Re-set the goal」。Minor：⑤ `stamp_resume_attempt` doc 写两种 target；⑥ Panel/TUI 两条测试改名为 `…_the_row_badge_arm_is_pinned_though_unreachable` / `…_the_picker_mark_arm_is_pinned_though_unreachable` 并在 doc 里写明 list 面 marker-only、行今天带不了这个词；⑦ 活动窗口循环对解析不了的 key 改为 `warn!(key)`（**没加计数器**：`ResumeReport` 每个计数都经 `receipt_from_report` 的穷举解构上 wire，一行无 session 可名的计数要动 protocol + handler + CLI 三面，超出 minor；twin 的 `errored` 报告没有 wire 面）；⑧ `check_unanswered` 的 `Ok(_)` 拆成 `Clean => false` 与 `Interrupted { .. } => { warn!; false }`；⑫ `unanswered_eligible` 改穷举 `match`（`Subagent | Ephemeral => false`，其余四种 → `!has_own_scheduler`）；⑬ `scanned` doc 补「被拒（TailReadFailed / LogInconsistent）的无 marker 会话也计」。**GREEN**（7f8d1a32c 内容）：`--lib -- session::reduction gateway::resume_coordinator gateway::session_snapshot gateway::projection_reconciler gateway::handlers::resume` → `test result: ok. 112 passed; 0 failed`；集成二进制 → `test result: ok. 25 passed; 0 failed`（23 + 2 新）；`cargo test -p aleph-panel --lib -- chat_sidebar` → `18 passed; 0 failed`；`cargo test -p aleph-tui` → `435 passed; 0 failed`。三次变异都从备份 `cp` 还原，`grep -c MUTATION` = 0。未重跑全量 `--lib`（改动限于测试、一处枚举穷举化、一处 warn、`abandon` 的文案参数）。deferred 未动：#9 prefix clone、#10 两条 ladder 的 DRY、#11 本地 `strip_visibility`。
### T3 — （commit: 6fdf76f6d · fix round 1: 9e24d936c）
- **RED**（改动前：brief 的测试 + `store::test_support::CountingStore` + 整文件重写的 `marker_balance.rs` 已就位，旧调用点未动）：`cargo check -p alephcore` → `error[E0432]: unresolved import marker_balance::close_open_run_after_retire --> src\session\mod.rs:33:9` + `error[E0425]: cannot find function close_open_run_after_retire in module crate::session::marker_balance --> src\gateway\handlers\mod.rs:179:50`，`could not compile alephcore (lib) due to 2 previous errors`——两条都指名被删的项。brief 写的 `E0061`（`compact_session` 参数数）住在 test target 里，`cargo check` 看不见，未另花一次 test-target 构建去采它。
- **类型检查**（改动后）：`cargo check -p alephcore` → `Finished`（仍只有 T6 记的那 4 条既有 `never used` 警告）。`rustfmt --check --edition 2021` 对每个改动的叶文件 clean（`manual.rs` / `marker_balance.rs` / `chat.rs` 三个就地格式化过；三者都不声明外部子模块）。
- **GREEN**（6fdf76f6d 的工作树内容）：`cargo test -p alephcore --lib -- context::compact::manual session::marker_balance gateway::handlers::session::db_handlers::modify gateway::handlers builtin_tools::sessions::compact_tool session::store::test_support` → `test result: ok. 1050 passed; 0 failed; 0 ignored; 0 measured; 17544 filtered out; finished in 13.07s`。新/改测试按名字都在：`manual::tests::{manual_compact_is_one_store_transaction, compaction_actually_shrinks_what_the_prompt_is_rebuilt_from}`（后者改为拿 `summary_ref` 比对 summary 的 `turn_id`）、`marker_balance::tests::{a_rewind_that_cuts_the_finish_closes_the_run_in_the_same_call, a_running_session_is_retired_but_not_closed, a_balanced_cut_appends_no_closer, open_run_after_retire_reduces_the_surviving_prefix, an_unreadable_prefix_is_an_error_not_a_no_op}`（brief 里三条一行注释骨架按裁定 1 写成真测试；第五条自加，判据 #8：乱序切片是调用方唯一递得进来的 REJECT 种类——`FinishWithoutStart` 是 REPORT 级，**不会**让 `reduce_run` 返回 `Err`，第一版拿它当 REJECT 写是错的，实测前改掉）、`modify::tests::truncate_retires_the_event_log_not_just_the_projection`（加 `install_test_session_service()`；投影断言从 `len() == 2` 收紧为「投影行的 source seq 集合 == 日志幸存 seq 集合」）。**第一次**跑同一过滤器 `1048 passed; 2 failed`：① `manual_compact_is_one_store_transaction` 红于 `Some("conversation already fits the verbatim tail budget")`——brief 的测试文本用 `ManualCompactOptions::default()`（20k 默认预算），40 回合夹具在这个预算下本来就装得下、什么都不写；改用同文件其它端到端测试的 `keep_tokens: Some(MIN_KEEP_TOKENS)`。② `gateway::handlers::fs::tests::every_credential_denylist_entry_is_refused_by_the_rpc_face`（不在 T0 基线，依赖本机 `~/.aleph/skills` 存在，与本任务路径无关）：单跑 `ok`，同过滤器重跑 `ok`——宽过滤器下的并行顺序相关，**未修**、列出。
- **变异 M1**（brief 第 5 步，compact 面：`compact_session` 的那一次 `emit_batch` 换成逐行 `emit_event` + 一次 retire-only `emit_batch(vec![], Some(Retire::Through))`）→ 同过滤器 `46 passed; 1 failed`，红名单恰 `context::compact::manual::tests::manual_compact_is_one_store_transaction`（`left: 3  right: 1`）。**变异 M2**（自加，/undo 面：`retire_from_and_close_run` 先 retire-only 批、closer 再单独 `emit_event`）→ `--lib -- session::marker_balance gateway::handlers::session::db_handlers::modify`：`21 passed; 1 failed`，红名单恰 `session::marker_balance::tests::a_rewind_that_cuts_the_finish_closes_the_run_in_the_same_call`（`left: 2  right: 1`）。两次都 `cp` 备份还原，`cmp` 逐字节相同，`grep -c MUTATION` = 0。
- **全量 `--lib`**（6fdf76f6d 的工作树内容，提交前跑）：`test result: FAILED. 18526 passed; 51 failed; 17 ignored; 0 measured; 0 filtered out; finished in 668.53s`。按名字 `comm -13 baseline_sorted.txt t3_failures.txt` → **空**；`comm -23` → **空**：与 T0 基线 51 条逐名相同，零新增零消失（`skill::usage::tests::concurrent_bumps_do_not_lose_counts` 本次未复现）。删掉的 `pub fn close_open_run_after_retire` / `balance_run_markers_after_retire` 与 `compact_session` 的 `store` 参数都在这个 `--lib` 构建里被编译过（同一 commit）。
- **判据 #6 调用者普查**（`rg -n "balance_run_markers_after_retire|compact_session\(|close_open_run_after_retire" src tests --glob '!**/target/**'`，剥注释，测于 23d4ca0c0 改动前）：`compact_session(`（manual 那个；`SessionManager::compact_session` 是另一个类型上的同名方法，未动）生产调用 **1 处** `builtin_tools/sessions/compact_tool.rs:132`（`run_manual_compaction`，工具面与 `session.compact` RPC `db_handlers/modify.rs:626` 共用）+ 测试 **9 处**（`manual.rs:1013,1075,1119,1152,1260,1263,1306,1344,1349`），10 处全部去掉 `store.as_ref()`；`balance_run_markers_after_retire` 定义 `handlers/mod.rs:164` + 调用 **2 处**（`chat.rs:831`、`db_handlers/modify.rs:752`），全部换成 `retire_events_and_balance`；`close_open_run_after_retire` 定义 `marker_balance.rs:47` + 再导出 `session/mod.rs:33` + 生产调用 **1 处** `handlers/mod.rs:179` + 测试 4 处，全部替换。改完后 `rg` 再扫多抓出一处普查漏掉的 `///` 文档引用 `resume_coordinator.rs:752`（改名）；此后 `rg -n "balance_run_markers_after_retire|close_open_run_after_retire" src tests` → **零命中**。`docs/reference/FEATURE_LOCATOR.md:2429,2442` 与 r2 spec/plan、r3 scans 仍写着旧名——文件清单之外，留给 T24。`compact_tool.rs` 不再读 `global_session_event_store()` ⇒ `store.rs` 该 slot 的读者表删掉这一行、硬编码的「five」改成「recount」（判据 #1）。
- **裁定 2 投影机制**：retire-only 批**不触发** `on_appended`（`actor.rs::finish_emitted` 每 appended 行跑一次，读代码确认）；退休从来不是经 observer 到达 `messages` 表的——① `chat.rewind` 靠显式的投影侧写 `delete_messages_from_seq(key, seq)`（按 source seq），批提交后仍调用；② `session.truncate` 同理靠 `truncate_messages(key, keep_count)`；③ 投影器写入时自查 `store.is_retired(id, seq)`（`project_event`），挡住排在 drain 队列里、被批退休的事件；④ closer `RunFinished { Cancelled }` 是批里的 appended 行、**会**触发 observer，但 `projection::project_row` 对 `RunFinished` 返回 `None`，不产生任何行。改动前 closer 是 `store.append` 直写（绕过 actor ⇒ 绕过 observer 与 actor 的 `head_seq`）；现在经 actor，投影器看得见它、actor 头不再失步。证明在 `modify.rs` 那条测试的收紧断言（投影幸存 source seq == 日志幸存 seq，`[1, 2]`）。`chat.rs` 没有 rewind 的 happy-path 测试（只有 `chat_rewind_denies_cross_user_as_not_found`，到不了 retire），未加。
- **裁定 3/4/6**：批内两行共一个 `created_at_ms`（`manual_compact_is_one_store_transaction` 断言 summary 与 checkpoint 的 `created_at_ms` 相等，顺序只看 seq）；`retire_events_and_balance` 无 service 时 `Ok(0)`，doc 句与 `retire_live_events` 同（boot 侧 `start/mod.rs:463-478` 两个 slot 由同一个 `Option` 安装，service 装 ⇔ store 装）；`global_session_service()` 调用点上方一行「A counted reader … (T17 census)」注释。
- **行为变化（点名）**：balance 以前是 best-effort（失败只 log、RPC 照样成功，因为 retire 已落盘）；现在批要么整个提交、要么 RPC 失败且**什么都没退休**——投影对齐因此永远不会对着一个日志没有真正切过的位置执行。`RetireOutcome.retired` 是批之前活日志里 `seq >= from_seq` 的计数，不是 UPDATE 行数——无并发写者时相等。
- **未做**：`--test '*'` 与 clippy 未跑（`rg` 证实集成测试无被删名字的调用者；`--lib` 构建已覆盖删除项）；`--lib` 的 `unix2dos` 只动工作树换行（index 是 LF、`core.autocrlf=true`，提交内容不受影响）。
- **Fix round 1（9e24d936c）**：审查一条 Important（`store.rs` 的 `retire_through` trait doc 仍在描述被删掉的两步 `/compact`——「the caller appends its summary first…」；`retire_from` doc 说它「backs chat.rewind」而 rewind 已在批内走 `Retire::From`）+ 控制者熵裁定（`retire_through` 生产调用者 **0**：`rg "retire_through\("` 改前只剩 trait def `store.rs:134`、impl `:636`、两个测试委托 `CountingStore` / `tests/resume_coordinator_integration.rs::FaultingStore`、三条 store 测试）+ 四条 minor。① **CUT** `SessionEventStore::retire_through`：trait、`SqliteEventStore` impl、`CountingStore`（连 `retire_throughs` 计数一起删——「一次事务」现在读作 `append_batches == 1 && retire_froms == 0`）、`FaultingStore` 四处全删；改后 `rg -n "retire_through|retire_throughs" src tests` 只剩 `store.rs` 里三条**测试名**与 `retire_through_batch` 测试助手（名字说的是 `Through` 语义，正是它们现在经 `append_batch(&sid, head+1, &[], Some(Retire::Through(n)), Durability::Normal)` 驱动的东西）。② 三条 store 测试改走 retire-only 批：`<=` 边界改为直接读私有连接的 `retired_at`（seq 1、2 `is_some()`、3 `is_none()`）；幂等改用**哨兵**（先把 seq 1 的 `retired_at` 改成 4242，再跑一次同样的 `Through(2)` 批，断言仍是 4242）——批不回报计数，而两次调用落在同一毫秒时「时间戳相等」是空断言，哨兵不是；FTS 保留与跨会话隔离两条原样。③ docs：`retire_from`（哪几个动词走独立退休、哪几个走批内 `retire` 参数、`Through` 没有独立方法）、`append_batch`（`/compact` = 摘要行 + 检查点行 + `Retire::Through(cut)` 一次调用；rewind/truncate = `Retire::From(seq)` + closer；空事件 + retire 合法）、`retire_in_txn`（原来指向 `retire_through` 的那句改成自述 FTS 保留理由）、`session_projector.rs:637` 的 retired-gate doc（两个写者改按 `Retire::From` / `Retire::Through` 命名）。④ minors：`manual.rs` 两个载荷共用一个 `at`（一批一个瞬间）；`manual_compact_is_one_store_transaction` 的 `created_at_ms` 相等断言注明它钉的是**测试替身**的一批一戳（actor 那半在 T2 的 actor 测试里）；`manual.rs:275`「`retire_through` boundary」→「`Retire::Through` boundary」；`modify.rs:716` 注释 `retire_live_events` → `retire_events_and_balance`。**验证**（9e24d936c 的工作树内容）：`cargo check -p alephcore` → `Finished`（仍只有既有 4 条警告）；`cargo test -p alephcore --lib -- session::store context::compact::manual session::marker_balance gateway::session_projector session::actor session::in_process gateway::handlers::session::db_handlers::modify` → `test result: ok. 122 passed; 0 failed; 0 ignored; 0 measured; 18472 filtered out; finished in 0.45s`（三条改写的 `retire_through_*` 测试按名字都在且 `ok`；改动文件零新警告）；删 trait 方法 ⇒ 集成二进制同一 commit 内构建：`cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1 --no-run` → `Finished test profile … in 6m 08s`，`Executable tests\resume_coordinator_integration.rs`（5 条警告全在别的文件）。`rustfmt --check` 对 5 个改动文件 clean。**未改**：`docs/reference/SESSION_SERVICE.md:58-62` 与 `FEATURE_LOCATOR.md:281,283,291,769,5532` 仍把 `retire_through` 写成 `/compact` 的原语——控制者已路由给 T24；`open_run_after_retire` 的前缀 clone（`partition_point` + slice）与 `RetireOutcome.retired` 的读前计数按裁定留到终审。
### T4 — （commit: b901c418c）
- **起点**：`git status --porcelain` 空，HEAD `b11463cb3`（T1/T2/T3/T6/T7 已落地）。`cargo check -p alephcore` → `Finished`（仅既有 4 条警告）。
- **RED**（测试先写）：`cargo test -p alephcore --lib -- context::compact::session_split gateway::projection_reconciler` 第一次 → 编译失败 `E0061: this function takes 4 arguments but 5 were supplied` ×3、`E0609: no field epochs_healed / epoch_heal_skipped` ×8（11 errors，与 brief 第 2 步预期形状一致）。只补结构（5 参 ctor + 两个字段，不加 heal、不改顺序）再跑 → `test result: FAILED. 12 passed; 8 failed`，8 条红名单 = 8 条新/改测试全部**运行时**红：`split_commits_parent_then_child_then_registers_the_epoch` 的 trace 实测为 `["register_epoch:…:s1", "retire_superseded:…", "batch:…:s1:1" ×3, "batch:…:main:1", "batch:…:s1:1"]`（旧序：先注册、再五个单行批），期望 `["batch:parent:1", "batch:child:4", "register_epoch:child", "retire_superseded:parent"]`。
- **GREEN**（b901c418c 的工作树内容）：同过滤器 + `gateway::continuation_lifecycle` → `test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 18575 filtered out; finished in 1.18s`。新/改测试按名字：`session_split::tests::{split_commits_parent_then_child_then_registers_the_epoch, split_seeds_child_with_forked_summary_and_fresh_tail（改为对 batches() 断言，[Context Summary] / 父键串 / "fresh tail message" 三条内容断言保留，另加 closer 与 opener 的 run_id 相等）, a_refused_epoch_registration_is_not_a_failed_split, a_refused_parent_batch_fails_the_split_before_anything_else, a_refused_child_batch_fails_the_split_without_registering}`；`projection_reconciler::tests::{a_forked_child_the_routing_table_never_learned_is_registered_at_boot（含幂等：第二次 healed 0 / skipped 0）, without_a_registrar_the_heal_is_counted_not_faked（裁定 1：真测试，FILE backend，skipped == 1 && healed == 0）, the_heal_touches_only_an_unregistered_fork, the_healed_child_inherits_attribution_and_the_parents_side_session_is_retired}`。中途两次 GREEN 试跑各暴露一个**夹具**问题（非产品缺陷）：① `without_a_registrar…` 的「routing 不变」断言在 FILE backend 上为假——`FileSessionStore::get_current_epoch` 读目录树，而同一次扫描里投影修复为子会话建了目录 ⇒ routing 经**另一条**boot pass 的副作用解析到子会话；改为按名字断言该副作用（判据 #1：计数器只陈述 heal 自己做了什么，日志句「routing stays on the parent」同步删掉）；② `the_heal_touches_only…` 的 odd 夹具经 `append_all` 落盘、`created_at_ms = seq`（1970），在活动窗之外 ⇒ heal 根本没看它；改为 `now` 戳。
- **brief 之外、由守卫/阅读抓出的两处补齐（同一 commit）**：(a) 首次全量 `--lib` 把 `gateway::continuation_lifecycle::tests::every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement` 打红——heal 里新增的 `register_epoch` 站点没有配套 `retire_superseded`：boot 侧只重放了 step 5 的一半，父会话的 `/btw` 侧线程会成为无人能寻址的孤儿。现 heal 在注册成功后 `retire_superseded(parent)`（parent 由 `SessionForked.parent_session_id` 解析），与进程内路径同序。(b) `register_epoch` = `get_or_create`，其 `stamp_attribution` 读 scope task-local，boot task 里为 `None` ⇒ 修好的子会话行 owner/scope 为 NULL，而随后的 resume pass 从**子会话自己的列**（`resume_coordinator::resume_metadata` → `from_persisted`）给续跑的 run 定 scope ⇒ 未盖戳的子会话会**无 scope 续跑**、记忆落到 base 分区（那段 doc 描述的正是这个洞）。现以父行的 `from_persisted` 属性 `with_scope` 包住 `register_epoch`；legacy（pre-P1）父行没有属性时什么都不盖，与 resume 同一 carve-out。
- **变异**（各自 `cp` 备份、还原后 `cmp` 逐字节相同、`grep -c MUTATION` = 0）：**M1**（brief 第 5 步：`register_epoch` 挪回父批之前）→ `17 passed; 3 failed`，红名单 `split_commits_parent_then_child_then_registers_the_epoch` + `a_refused_parent_batch_fails_the_split_before_anything_else` + `a_refused_child_batch_fails_the_split_without_registering`（后两条钉的正是「批失败时 routing 不得已学到子会话」）。**M2**（子批先于父批）→ `17 passed; 3 failed`：`split_commits_parent_then_child_then_registers_the_epoch` + `split_seeds_child_with_forked_summary_and_fresh_tail` + `a_refused_child_batch_fails_the_split_without_registering`。**M3**（heal 注册时不套父 scope）→ `--lib -- gateway::projection_reconciler gateway::continuation_lifecycle`：`17 passed; 1 failed`，恰 `the_healed_child_inherits_attribution_and_the_parents_side_session_is_retired`（`left: None right: Some("u-alice")`）。**M4**（heal 去掉 `retire_superseded`）→ `16 passed; 2 failed`：同一条行为测试（"the parent's side session was retired…"）+ `continuation_lifecycle` 那条源码级 census。
- **`--bins`**：`cargo test -p alephcore --bins` → `test result: ok. 94 passed; 0 failed`（`aleph-server` unittests；`start/mod.rs` 的 5 参调用与两条新日志字段在这个构建里编译）。
- **判据 #6 ctor 调用点普查**（`rg -n "ProjectionReconciler::new(" src tests --glob '!**/target/**'`，改前）：**2 处**——生产 `src/bin/aleph-server/commands/start/mod.rs:2990`（现 :3000，传 `epoch_registrar_for_reconcile`）+ 测试助手 `src/gateway/projection_reconciler.rs:326`（现 :568，传 `None`；T5b 依赖它）；其余 8 个文件里的 `ProjectionReconciler` 命中全是注释/doc 引用。改后 5 处（+3 处是新测试各自构造带 `Some(registrar)` 的实例）。
- **reconciler 与 resume 的调用顺序**（`start/mod.rs`）：同一个 `tokio::spawn`（:3018）内顺序执行——:3019 `reconciler.reconcile_candidates().await`（heal 在 `candidates()` 内、`load_run_markers` 之后、disposition 循环**之前**）→ :3041 `ResumeCoordinator::new` → :3057 `set_global_resume_coordinator` → `auto_scan` 分支 `wait_for_channel_config_snapshot` → :3068 `coordinator.resume_interrupted_runs().await`。reconciler 无条件跑，resume 扫描受 `[resume] enabled` 门控 ⇒ heal 不依赖 resume 开关。
- **heal 如何发现未注册的 fork / marker 切片是否够用**：子批最后一行恒为 `RunStarted`（marker）⇒ 每个 fork 出的子会话都在 `load_run_markers` 的分组里，分组键（`SessionId`，带 epoch）+ 末 marker 的 `created_at_ms`（活动窗）就是 heal 需要的全部；`SessionForked` 不是 marker，靠 `load_events_range(id, Some(1), Some(2))` 一次定点读取 seq 1。判据链：`id.epoch() == 0` 跳过 → 末 marker 早于 horizon 跳过 → `get_current_epoch(base) >= id.epoch()` 跳过（幂等）→ seq 1 非 `SessionForked` ⇒ `errored`（"I don't know"，判据 #8）→ 无 registrar ⇒ `epoch_heal_skipped` → 注册（父 scope 内）→ `retire_superseded(parent)`。
- **全量 `--lib`**（b901c418c 的工作树内容）：`test result: FAILED. 18533 passed; 52 failed; 17 ignored; 0 measured; 0 filtered out; finished in 714.44s`。按名字 `comm -13 baseline_sorted.txt t4_failures2.txt` → **恰 1 条**：`harness::tests::task10_wiring::extras::split_session_directive_continues_run_in_child_session`；`comm -23` → 空。（首次全量在补 (a)(b) 之前跑：`18530 passed; 54 failed`，多出的 3 条 = 这条 + `continuation_lifecycle` 那条 census + 已知 flaky `skill::usage::tests::concurrent_bumps_do_not_lose_counts`；后两条在终跑里分别被修掉 / 未复现。）
- **已知新红，待控制者裁定（已发 NEEDS_CONTEXT）**：`src/harness/tests/task10_wiring/mod.rs::MockSession` 没有 `emit_batch` 覆写（T2 留在 fail-closed 默认上，`src/harness/` 零改动），而 `extras.rs` 的两条 split 测试**经这个 mock**驱动 `perform_session_split`：① `split_session_directive_continues_run_in_child_session` 现在在**父批**就吃到 `Err` → 退回 compact-to-fit → `registrar.called` 断言红；② `split_session_failsoft_compacts_and_continues` 用 `FailRegistrar` 断言「注册失败 ⇒ 分裂失败」，这正是本任务刻意反转的契约，它此刻仍绿只因失败提前发生在批那一步。在 `perform_session_split` 里退回逐行 `emit_event` 不是选项（那就是本任务要消灭的撕裂）。三个选项已列给控制者：A 允许 `src/harness/tests/` 的纯测试改动（给 MockSession 加 `emit_batch`、把 ② 改到新契约；不动 `budget.rs::CEILING`，`src/harness/tests/` 按构造在预算之外）/ B 保持零改动、按名列出 / C 另裁。
- **与 brief 的偏差（点名）**：① trace 期望向量比 brief 多一项 `retire_superseded:<parent>`，把「只在注册成功之后才退休」这层顺序也钉住；② `build_summary_event` 多收一个 `at`，一批一个瞬间（T3 fix round 的同一裁定）；③ heal 多做了上面 (a)(b) 两件事；④ `SplitError::Failed` doc 与 `session_split.rs` 模块头、`projection_reconciler.rs` 模块头（原句「a driver and nothing else」现已不真）同步改写。
- **未做**：`--test '*'` 与 clippy 未跑（本任务未删任何 `pub` 项；`--lib` 构建已覆盖 `#[cfg(test)]`）；`FEATURE_LOCATOR.md` / `SESSION_SERVICE.md` 里描述七次单写分裂与「注册失败 ⇒ 分裂失败」的散文未改——文件清单之外，留给 T24；`rustfmt` 对 `start/mod.rs` 只 `--check`（它是 `mod.rs`，就地格式化会递归改到 `helpers.rs` 里一处既有 diff）。
- **Fix round 1（cb5869f6d）— R10 例外（仅测试）**：控制者裁定 **A**：CLAUDE.md 定义的 R10 锁的是 harness 的 **12 个文件**与 `budget.rs::CEILING` 棘轮；`src/harness/tests/` 按构造在 ceiling 之外（`budget.rs:705`）、也不在 12 个文件里，本 plan 的「`src/harness/` 零改动」是更严的自设表述，护的是循环不是它的测试替身；分支上留一条已知红（选项 B）不可接受，生产侧退回逐行 `emit_event` 是本轮要消灭的撕裂、排除。改动**恰两个文件**：① `src/harness/tests/task10_wiring/mod.rs::MockSession` 加 `emit_batch` 覆写（对每一行做 `emit_event` 做的事、一把锁、回 seq；`assert!(retire.is_none())`——split 从不退休），并加 `with_refused_batches` 构造子（`emit_batch` 回 `SessionError::Storage`、`emit_event` 照常，harness 自己的逐回合写入不受影响）；② `src/harness/tests/task10_wiring/extras.rs`：`split_session_directive_continues_run_in_child_session` **零改动自行转绿**；`split_session_failsoft_compacts_and_continues` 改名 `split_session_survives_a_failed_epoch_registration` 并改到新契约（`FailRegistrar` 不再让分裂失败：两批落盘、`final_session_id == parent.with_next_epoch()`、日志里有 `SessionForked`、`!hit_limit`、provider 恰 1 次调用）；新增 `split_session_failsoft_on_a_refused_batch_compacts_and_continues`（幸存的契约：批被拒 ⇒ 退回 compact-to-fit、`final == parent`、registrar **未被触及**）。`git diff --stat src/harness/tests/budget.rs` 空，`CEILING = 5239` 未动。**验证**：`cargo test -p alephcore --lib -- harness::tests::budget harness::tests::task10_wiring` → `25 passed; 1 failed`，唯一红 = `harness::tests::budget::the_harness_line_budget_does_not_grow`——T0 基线第 21 条，且报文逐字节相同（`grew to 5250 … ceiling of 5239`，基线 / b901c418c 全量 / 本次三处一致；`src/harness/` 生产文件本轮零改动）。两个 harness 测试文件 `rustfmt --check` clean。**全量 `--lib`（cb5869f6d 的工作树内容）**：`test result: FAILED. 18535 passed; 51 failed; 17 ignored; 0 measured; 0 filtered out; finished in 677.50s`，按名字 `comm -13` / `comm -23` 对 T0 基线**双向为空**——与基线 51 条逐名相同，零新增零消失；三条 split 测试按名字 `ok`。
### T8 — （commit: a8693e1b6 · fix round 1: 136c32f66）
- **判据 #6 种子对生产者普查**（改前；`rg -n "SessionEvent::TurnStarted \{" src --glob '!**/target/**'` + `UserMessage {` 字面量，剥注释、按各文件首个 `#[cfg(test)]` 行号剥测试段）：生产侧**构造** `TurnStarted {` 恰 **3 处**——① `session_seed.rs:118`（`seed_history`，trigger `UserMessage` + 1 条 `UserMessage`：**唯一的"种子对"**，本任务改经 `user_turn`）；② `session_seed.rs:160`（`seed_multimodal`，trigger `UserMessage` + **N** 条 `UserMessage` 循环——同触发器、非 1+1 形状，`user_turn` 装不下，**未改、点名**）；③ `agents/subagent_spawner/mod.rs:675`（trigger `SubagentRequest` + 1 条 `UserMessage(author None)`——另一个触发器，`user_turn` 硬编码 `TurnTrigger::UserMessage`，**未改、点名**）。`fast_path.rs` 改前**没有** `TurnStarted`（只有两条孤立 `UserMessage` + `AssistantMessage`，写在工具**之后**），不是对生产者。其余 `TurnStarted {` 命中全是模式（`{ .. }`）或测试模块（逐个核对：`harness/agent/prompt.rs:749`、`subagent_tool/recovery.rs:1427`、`tools/gather_budget.rs:315`、`tools/attempt_summary.rs:316`、`session_snapshot.rs:298`、`projection_reconciler.rs:522` 均在各自 `#[cfg(test)]` 之后）。改后：`events.rs::user_turn` 是种子对唯一构造子，`seed_history` 与 L0 fast path 都经它；生产侧 `TurnStarted {` 仍 3 处（`user_turn` / multimodal / subagent）。
- **RED**（改动前，只加了 brief 的测试）：`cargo check -p alephcore --tests` → `error[E0432]: unresolved import super::FastPathJournal --> src\gateway\execution_engine\fast_path.rs:266:9`、`error[E0603]: function event_type_tag is private --> fast_path.rs:269:33`，`could not compile alephcore (lib test) due to 2 previous errors`（census 测试只做字符串扫描、能编译，运行时红于 `journal.open(` 缺失——第二轮实测到）。
- **类型检查**（改动后）：`cargo check -p alephcore` → `Finished`，仍只有 T6 记的那 4 条既有 `never used` 警告。改动的 6 个叶文件 `rustfmt --edition 2021 --check` 全 clean（`fast_path.rs` / `slash_command.rs` 就地格式化过；其余四个无 diff；无一声明外部子模块）。
- **GREEN 第一轮**（实现后）：`cargo test -p alephcore --lib -- gateway::execution_engine::fast_path gateway::execution_engine::slash_command orchestrator::harness_bridge::session_seed session::events session::reduction::tests::user_message_producers` → `27 passed; 2 failed`：① `user_message_producers_are_the_known_set` 红——**预期**（控制者裁定 1），扫描打印 `{"agents/subagent_spawner/mod.rs": 1, "gateway/execution_engine/simple.rs": 1, "gateway/execution_engine/steering.rs": 1, "gateway/openai_api/completions/agent.rs": 1, "orchestrator/harness_bridge/backfill.rs": 1, "orchestrator/harness_bridge/session_seed.rs": 2, "session/events.rs": 2}`（`fast_path.rs` 消失、`session_seed.rs` 3→2、`events.rs` 1→2），期望表照抄这份；② `every_fast_path_dispatch_is_journaled_before_it_runs` 红于 `the open batch is written in execute_direct_tool`——rustfmt 把 `journal.open(` 链拆成 `journal` / `.open(` 两行，brief 的字面锚点在格式化后的文本里不存在；改测试为在**逐行 trim 后无分隔拼接**的视图上匹配锚点（顺序断言不变），不为一个文本锚点改生产代码。
- **GREEN 第二轮**（a8693e1b6 的内容）：同一过滤器 → `test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 18579 filtered out; finished in 4.24s`。按名字：`fast_path::tests::{the_open_batch_is_durable_before_the_tool_runs_and_reads_interrupted_if_it_never_closes（seqs == [1,2,3,4]、kinds == [turn_started,user_message,run_started,tool_call_requested]、reduce_run ⇒ Interrupted{attempts:0} + 1 条悬空 call（tool_name / call_id / ThisRestart）, the_open_batch_carries_one_turn_the_envelope_and_the_input, a_closed_fast_path_reduces_clean_with_a_receipt_and_an_answer（close 批 seqs == [5,6,7]、Completed、answered == 1）, a_failed_fast_path_closes_errored_and_reduces_clean（tool_error / Errored / Clean）}`、`slash_command::arg_mapping_tests::every_fast_path_dispatch_is_journaled_before_it_runs`、`every_stamp_on_carry_resolver_runs_on_the_fast_path`（四个 resolver 仍在 gate 内被调用，值现在上了 envelope）、`session::events::tests::*` 14 条（含 `durability_barrier_set_is_exactly_the_four_ruled_events`）、`session::reduction::tests::user_message_producers_are_the_known_set`。⚠️ `orchestrator::harness_bridge::session_seed` 过滤器**零命中**（seed 的测试住在 `orchestrator::harness_bridge::tests`），变异轮改用整个 `orchestrator::harness_bridge`。
- **变异**（brief 第 5 步：`journal.open(...)` 搬到 `execution.await` 之后）：`cargo test -p alephcore --lib -- gateway::execution_engine::fast_path gateway::execution_engine::slash_command orchestrator::harness_bridge session::events session::reduction::tests::user_message_producers` → `128 passed; 1 failed`，红名单**恰** `every_fast_path_dispatch_is_journaled_before_it_runs`（`the open batch must precede the dispatch`）。四条 `fast_path::tests` 在变异下仍绿——它们测 journal 本身不测调用点，这正是 census 存在的理由。还原后 `slash_command.rs` SHA-256 与变异前副本逐字节相同（`709221B3…1E672`）。`orchestrator::harness_bridge::tests::*` 全绿（含 `history_input_does_not_reseed_when_log_nonempty`）⇒ `seed_history` 改经 `user_turn` 后行为不变。
- **全量 `--lib`**（a8693e1b6 的工作树内容，提交前跑）：`test result: FAILED. 18538 passed; 53 failed; 17 ignored; 0 measured; 0 filtered out; finished in 672.85s`。按名字与 T0 基线 51 条比对：新增 **2 条**——`skill::usage::tests::concurrent_bumps_do_not_lose_counts`（控制者点名的已知 flaky，`98 != 100`）、`thinker::runtime_context::tests::cached_repo_root_releases_lock_before_filesystem_io`（`sleep(20ms)` 后 `try_lock` 采样的计时测试，本任务未触及 `thinker/`）；两条在已建好的二进制上单独重跑 → `2 passed; 0 failed`（`Finished in 3.08s`，无重编译）⇒ 负载型 flaky，按名列出、未修。消失 **0 条**。`harness::tests::budget::the_harness_line_budget_does_not_grow` 红 = 基线第 21 条（`src/harness/` 零改动）。本任务的 9 条相关测试在全量里按名字全 `ok`。
- **判据 #17 渲染者**：本任务不加 wire 字段/状态词。新落盘的行全由既有渲染者消费：`RunStarted{envelope}` → `reduction::RunStartFacts.envelope` → `resume_coordinator::plan_resume`；`ToolCallRequested` / `ToolResult` / `ToolError` → `session::projection::project_row` + `boundary_repair`（悬空调用的三臂修复）；`UserMessage` / `AssistantMessage` → `MessageProjector` → `messages` → `chat.history`。**三个前置 `Failed` 臂**（无效 mode JSON / 缺 `tool_id` / 未知 mode type，经 `try_resolve_slash_command` 与 `serialize_parsed_command` 不可达）与 **open batch 被拒**（一个事务、什么都没落）的情形不留任何 transcript 行——缺行不是错行，`finalize_fast_path_error` 站点已注明；用户仍经 `ResponseChunk` + `RunComplete` 收到回执。
- **`/compact` 交互**（同一条 fast path）：open batch 现在在 `session_compact` 跑之前就在日志里。`manual.rs::select_cut` 从尾部向前保留（四行 open batch 很便宜、必在 kept tail 里），`snap_out_of_open_run` 只管 cut **之后**有 `RunFinished` 的 run（本 run 此刻未关），`task_focus` 已过滤 `/compact` 头——最终尾部 `[…, RunStarted, ToolCallRequested(session_compact), SystemMessage(summary), CompactionPerformed, ToolResult, AssistantMessage, RunFinished]` 与模型工具调用 `/compact` 今天留下的形状**同形**，不是第二种形状。
- **与 brief 文本的偏差（点名）**：① `close_err` 的回复行用 `format!("❌ {ExecutionError::Failed(msg)}")` 而非 brief 的 `format!("❌ {msg}")`——`execute.rs` 交给 `finalize_fast_path_error` 的是 `e.to_string()`（前缀 `Execution failed: `），照 brief 写，transcript 里的回复会和用户看到的那行差一个前缀（判据 #1）；`ToolError.error` 仍是 `msg`。② `RunEnvelopeSnapshot` 构造不显式写 `model: None, model_provider: None`——六字段全列再 `..default()` 触发 `clippy::needless_update`（工作区零警告），两个 `None` 改由控制者要求保留的 `..default()` 供给，语义相同、T12 加字段后仍留 `None`。③ census 锚点在逐行拼接视图上匹配（GREEN 第一轮 ②）。④ `reduction.rs` 不在 brief 第 6 步的 `git add` 清单里但进了本 commit（普查表重推导；控制者裁定 1）；`service.rs` 未改故未 stage（裁定 2）。⑤ `fast_path.rs` 多一条 `the_open_batch_carries_one_turn_the_envelope_and_the_input`（钉 turn_id 共享 / envelope / input / author 落盘，brief 的两条只看 kinds 与 reduce）。
- **未做**：brief 第 4 步的 `cargo test --features test-helpers --test 'gateway_chat_*' -j 1` 未跑——`rg "SLASH_COMMAND_MODE_KEY|slash|fast_path" tests/` **零命中**（`tests/gateway_chat_common` 没有 slash-command 夹具，brief 那句是假设），且本任务未删任何 `pub` 项、`user_turn` 是纯新增、`--lib` 构建已覆盖 `#[cfg(test)]`；clippy 未跑（偏差 ② 是分析不是仪器，留给 review）；`seed_multimodal` 与 `subagent_spawner` 两处手写 `TurnStarted`（普查 ②③）未改；`docs/reference` 无描述旧 transcript 形状的散文可改（`service.rs:67-80` 的 `fast_path ×2` 计数注释留给 T17）。
- **Fix round 1（136c32f66）**：审查 1 条 Important + Minor #1/#4。**Important**——本任务存在的两条性质（open batch 在工具体开始前已落盘；open 被拒 ⇒ 工具不跑）只由文本 census 钉着，而它在 `let _ = journal.open(..).await;` 下照样绿（判据 #4：扔掉返回值仍绿 ⇒ 守的是调用不是效果）。**缝**：`execute_slash_command_fast_path` 解析一次 `global_session_service()`、把 `Option<Arc<dyn SessionService>>` 传给 `execute_direct_tool`（改 `pub(super)`，`execution_engine::tests` 才能塞进拒绝的 double）；生产行为不变。**两条行为测试**（`execution_engine/tests.rs`）：(a) `the_open_batch_is_in_the_log_when_the_tool_body_runs_and_carries_the_gates_knobs`——`WitnessToolRegistry` 在**工具体内部**经 `install_test_session_service` 的全局句柄读日志，看到恰 `[turn_started, user_message, run_started, tool_call_requested]`；`RunStarted.envelope` 与请求携带的四个 knob 逐字段相等（`exec_tier=full / session_mode=code / think_level=high / memory_mode=off`，各用该 knob 自己的 `.id()` 拼——四个值互异，`session_mode`/`memory_mode` 互换过不了）；工具后同一日志接 `[tool_result, assistant_message, run_finished]`。(b) `a_refused_open_batch_means_the_tool_never_runs`——`RefusingSessionService`（`emit_batch` 回 `Storage("refused")`）⇒ `ExecutionError::Failed("session log write failed before dispatch: …")` 且 `CountingToolRegistry.calls() == 0`。**GREEN**：`cargo test -p alephcore --lib -- gateway::execution_engine::fast_path gateway::execution_engine::slash_command gateway::execution_engine::tests orchestrator::harness_bridge session::events session::reduction::tests::user_message_producers` → `test result: ok. 167 passed; 0 failed; 0 ignored; 0 measured; 18443 filtered out; finished in 5.89s`（原 5 条 gate 测试与 `every_stamp_on_carry_resolver_runs_on_the_fast_path` 仍绿；`slash_engine` 助手泛化为 `R: ToolRegistry`）。**变异**（`let _ = journal.open(..).await;`）：`-- gateway::execution_engine::fast_path gateway::execution_engine::slash_command gateway::execution_engine::tests` → `50 passed; 1 failed`，红名单**恰** `a_refused_open_batch_means_the_tool_never_runs`（`a dispatch the log refused to record must not run: "ran"`）；同一变异下 `every_fast_path_dispatch_is_journaled_before_it_runs` **仍绿**——审查点名的盲区实测成立。还原后 `slash_command.rs` SHA-256 与变异前副本相同（`B4CAFDD6…DDACB`）。**Minor #1（一批一个瞬间）**：`SessionEvent::user_turn` 多收一个 `at`（T4 `build_summary_event` 同一裁定；brief 签名的点名偏差）；`FastPathJournal::open` / `close` 各只 mint 一次 `now_ms()` 盖满整批（`close_ok`/`close_err` 把 receipt 改成 `FnOnce(Timestamp) -> SessionEvent` 交给 `close` 在同一瞬间构造）；`fast_path::tests` 三条各加 `assert_one_instant`（open 4 行 / close 3 行 payload `at` 全等）。**Minor #4**：`user_turn` doc 改为「the 1+1 seed pair a single-text user turn opens with」。**未跑**：本轮未重跑全量 `--lib`——改动的两个签名（`user_turn` +`at`、`execute_direct_tool` +`session`）的全部调用者都在覆盖过滤器内（`--lib` 构建本身已在覆盖轮编译通过）；clippy 仍未跑。Minor #2（共享 `RunEnvelopeSnapshot::from_knobs`）→ T12、Minor #3（fast path 落盘未经 `apply_result_budget` 的原始 registry 值）留档、跨任务「model not recorded」句子 → T12、`service.rs` 计数注释 → T17（控制者路由）。
### T9 — （commit: 37fb7e965 · twin round: 2d122ac41 · fix round 1: ba3cabaab）
- **前置**：`git status --porcelain` 空，HEAD `4ac4052ba`。`tasklist //FI "IMAGENAME eq rustc.exe"` 无进程。
- **判据 #17 / 裁定 5 渲染者普查**（改动前只加了 `ErrorKind::HookStop`）：`cargo check -p alephcore` → `error[E0004]: non-exhaustive patterns: &session::events::ErrorKind::HookStop not covered --> src\session\projection.rs:95:31`，`could not compile alephcore (lib) due to 1 previous error` ⇒ 整个 lib 里对 `ErrorKind` 的穷尽 `match` **恰 1 处**：`session/projection.rs::project_row`（现渲染 `"Stopped by hook: {message}"`，`system` 行）。`rg "ErrorKind::Guardrail|events::ErrorKind"` 其余命中全是构造/断言（`events.rs:768` 的 `sample_of_every_kind` 夹具、`harness_bridge/callback.rs:135` 生产者、`harness_bridge/tests.rs:340`、`session_projector.rs:1758` 测试、`projection.rs:271` 测试），`shared/protocol` 无镜像枚举（criterion #10 不涉）。无 `_ =>` 臂。
- **RED**：① 上面那条 E0004；② 加了 `project_row` 臂后 `cargo check -p alephcore --tests` → `error[E0425]: cannot find function hook_stop_receipt in this scope` ×4（`run_loop/mod.rs:736/760/812/845`）+ 3 条级联 `E0614`（`evs` 类型未知时 let-else 绑定模式推不出，函数存在后自行消失，未改测试）。
- **类型检查**（实现后）：`cargo check -p alephcore --tests` → `Finished`，lib 侧仍只有既有 4 条 `never used`（`replicate/builder.rs:108`、`mcp/manager/handle.rs:417`、`pii/engine.rs:14`、`tool_loop_verifier.rs:86`）；`rustfmt --edition 2021 --check` 三个文件 clean（`mod.rs` 的两处 diff 就地改掉：import 单行、kinds 数组竖排；只看 `mod.rs` 自己的 diff，rustfmt 会顺着 mod 树报子文件）。
- **裁定 3 census 锚点**（从真实代码推导，不用 brief 的字面量）：production 半段经 `code_text(&production_prefix(include_str!("mod.rs")))`（同目录 `author_census.rs` 的写法，编译期读、不依赖 cwd），锚点 `execute_interceptors(HookEvent::BeforeAgentStart,`（派发行；注释里的 `BeforeAgentStart` 与 `warn!` 字符串被 `code_text` 剥掉，满足不了它），先断言锚点在 production 半段**恰 1 次**，再取到第一个 `Ok(_) => {}`（穿透臂）为块。块按 `"return "` 切段：**恰 2 个出口**，且**每个出口前面那一段** `journal_hook_stop(` 恰 1 次、最后一个 `return` 之后 0 次——比 brief 的「块内数 2」多一层**顺序**断言（journal 必须在它自己的 return 之前）。
- **GREEN 第一轮**：`cargo test -p alephcore --lib -- gateway::execution_engine::run_loop session::projection session::events session::reduction` → `test result: ok. 126 passed; 0 failed; 0 ignored; 0 measured; 18490 filtered out; finished in 4.26s`。按名字：`hook_stop_tests::{a_stopping_hook_leaves_five_events_that_reduce_clean（kinds == [turn_started,user_message,run_started,error,run_finished]、Error{HookStop,recoverable:false}、reduce_run ⇒ Clean、dangling/contradictions 空）, the_receipt_is_one_turn_one_run_one_instant（种子对共 turn_id、text=="hi"、synthetic:false、RunStarted/RunFinished 同一 `hookstop-` id、envelope None、Error.turn_id == Some(seed turn)、outcome Cancelled、五行 at 全等）, a_hook_stopped_resume_writes_no_seed_pair（kinds == [run_started,error,run_finished]、Error.turn_id None、Errored、Clean）, both_receipt_shapes_derive_a_barrier_from_their_members（两种形状经真 `batch_durability` 都是 Barrier——U3 推导不硬编码）, both_before_agent_start_exits_journal_the_stop}`、`projection::tests::project_row_labels_a_hook_stop_receipt`；T8 的 `session::reduction::tests::user_message_producers_are_the_known_set` 仍绿（本任务经 `user_turn`，无 `UserMessage {` 字面量生产者）。
- **GREEN 第二轮**（自审判据 #4 后加的效果测试 `journal_hook_stop_appends_the_receipt_to_the_session_log`：经 `install_test_session_service` 真跑 `journal_hook_stop`，用 `SessionKey::ephemeral` 独占键，`get_events` 读回 5 行同序、真 reducer ⇒ Clean）：`-- gateway::execution_engine::run_loop session::projection session::events session::reduction session::in_process` → `test result: ok. 143 passed; 0 failed; 0 ignored; 0 measured; 18474 filtered out; finished in 4.35s`。
- **变异**（每次还原后 `touch mod.rs`，SHA-256 与变异前副本逐字节相同 `cc1f2e26…3bb68`，`git diff --stat` 不变）：
  - ① brief 第 5 步：deny 臂删掉 `journal_hook_stop` → `-- gateway::execution_engine::run_loop session::projection`：`65 passed; 1 failed`，红名单**恰** `both_before_agent_start_exits_journal_the_stop`（`exit 0 must journal exactly once before it returns`）。
  - ② 控制者点名「回执写在报告给用户**之后**」：prevent_continuation 臂把 `journal_hook_stop` 挪到 `return Ok(stop_msg)` 之后（`#[allow(unreachable_code)]` 块）→ `65 passed; 1 failed`，红名单**恰**同一条（`exit 1 must journal exactly once before it returns`）——brief 原版「块内数到 2」对这个变异是**绿**的（调用还在块里），顺序断言是它红的原因。
  - ③ builder 丢掉 `RunFinished` → `63 passed; 3 failed`，红名单 `a_stopping_hook_leaves_five_events_that_reduce_clean` / `the_receipt_is_one_turn_one_run_one_instant` / `a_hook_stopped_resume_writes_no_seed_pair`（三条都红在 kinds 断言，**没跑到** `reduce_run` 断言）。如实转述：形状被钉住了，但 reducer 断言的「变红情形」不是 builder 变异——kinds 不变而 reduce 读法变（reducer 侧改动）才是它的红；本轮未变异 `reduction.rs` 去证明它。
- **全量 `--lib`**（37fb7e965 的工作树内容，提交前跑）：`test result: FAILED. 18549 passed; 51 failed; 17 ignored; 0 measured; 0 filtered out; finished in 594.69s`。按名字与 T0 基线 51 条 `comm -3`：新增 **0 条**、消失 **0 条**（控制者点名的三条 flaky 本轮均未出现）。本任务 7 条测试在全量里按名字全 `ok`。未 WEDGE（第 2 轮轮询内出现 `.done`）。
- **与 brief 文本的偏差（点名）**：① resume 臂的 `Error.turn_id` 是 `None` 不是 brief 的 `Some(turn_id)`——那一臂不写 `TurnStarted`，`store::extract_turn_id` 会把 `Error.turn_id` 索引到 per-turn replay，一个指向不存在的 turn 的 id 是判据 #17 那种「占位符替字段说出一个具体的谎」；`seeded_turn = (!is_resume()).then(TurnId::new_v4)` 让 Error 的 turn 恰等于「本批打开的那个 turn 或没有」。② `run_id` 一次绑定、两个 marker 共用（裁定 1，不用 `find_map(..).unwrap_or_default()`）。③ 一批一个 `now_ms()`（brief 三次；控制者「一批一个瞬间」）。④ census 用 `include_str!("mod.rs")` + 派发行锚点 + 逐出口顺序断言（裁定 3）。⑤ 多 3 条测试（`the_receipt_is_one_turn_one_run_one_instant` / `both_receipt_shapes_derive_a_barrier_from_their_members` / `journal_hook_stop_appends_the_receipt_to_the_session_log`）。⑥ `events.rs` 进了 commit（brief 第 6 步的 `git add` 漏了它，而 `HookStop` 就加在那里）。
- **孪生点名（判据 #16，未改、交控制者裁）**：`run_loop/inner.rs:397-425` 的 `UserPromptSubmit` deny / `prevent_continuation` 与本任务的站点**同形**——同样在 bridge seed 之前 `return`，同样抹掉整个 turn；`journal_hook_stop` 原样可用，但 spec §5.4 / brief 只点名 `BeforeAgentStart`，census 也只盖这一个 match，本任务不扩。
- **未做**：`UserPromptSubmit` 孪生（上条）；`docs/reference/SESSION_SERVICE.md:47` 「today `Guardrail`」那句现在少了 `HookStop`——文档归 T24（plan §文件表 L188），未动；未经真 hook 走一遍 `run_agent_loop`（extension manager 是进程全局，`execution_engine/tests` 没有 BeforeAgentStart 夹具；三段连线各自有测试：builder 形状 / journal 到 store 的效果 / 两个出口都在 return 前调 journal）；clippy 未跑；`reduction.rs` 未变异（变异 ③ 的说明）。
- **Twin round（2d122ac41，控制者裁定：判据 #16 孪生同笔修）**：`run_loop/inner.rs` 的 `UserPromptSubmit` deny / `prevent_continuation` 两个出口接同一个 `journal_hook_stop`（deny → `Errored`、prevent_continuation → `Cancelled`，同一批一个瞬间、同一条 resume 不重播种规则）。**属于哪种情形**（读调用顺序）：`inner.rs` 里 `execute_interceptors(HookEvent::UserPromptSubmit,` 在 `:394`，而 bridge 的 `seed_history` 跑在 `run_dispatch_and_drain_classified(`（`:1447`）派发的 orchestrator 里——`fn run_agent_loop_inner<` 到该锚点之间 `rg "emit_event|emit_batch|user_turn|seed_history|seed_session|UserMessage \{"` **零命中**（`prepare_history` 可能压缩，但种子对构造子由 T8 的 `user_message_producers_are_the_known_set` 钉死，压缩器里没有）⇒ **PRE-SEED**，与 `BeforeAgentStart` 同一情形，回执**要**播种（resume 不播）。站点注释与 `ErrorKind::HookStop` / `project_row` / `hook_stop_receipt` 三处只点名 `BeforeAgentStart` 的 doc 一并改成点名一对（判据 #1：否则它们就是说谎的那一份）。**Census 改成推导**：`every_pre_seed_hook_exit_journals_the_stop`——语料 = 两个循环文件（`mod.rs` / `inner.rs`，`code_text(production_prefix(include_str!))`）各带一个**交棒标记**（`.run_agent_loop_inner(` / `run_dispatch_and_drain_classified(`，断言在 production 半段恰 1 次）；站点 = 交棒之前的每一个 `execute_interceptors(HookEvent::` 派发；出口 = 该派发**括号配平**的 block 里的每个 `return `（`dispatch_body`，在剥掉字面量的文本上配平，与 arm 拼法无关）；等式 = `journal 调用数 == 出口数`（每个出口前一段恰 1 次、最后一个 return 之后 0 次），外加 `出口数 > 0` 的空集守卫——**不手写 3**（推导表用一条临时 `eprintln!` 探针 + `--nocapture` 单测实测：`PROBE derived sites: [("run_loop/mod.rs", "BeforeAgentStart", 2), ("run_loop/inner.rs", "SessionStart", 0), ("run_loop/inner.rs", "UserPromptSubmit", 2)]`，探针还原后 `touch`、SHA-256 `db6934e5…4096` 与提交文件相同；交棒之后的 `AgentEnd` 按位置落在规则外，将来它若有 `return` 不会误报）。**Pin**：`the_twin_hook_seams_fire_before_anything_seeds_the_turn` 点名两个孪生（点名是刻意的：它钉两个事实，不数任何东西）——各自的派发在 `fn run_agent_loop<` / `fn run_agent_loop_inner<` 之内、交棒之前，且函数签名到派发之间无种子写者拼法。**类型检查**：`cargo check -p alephcore --tests` → `Finished`；四个文件 `rustfmt --check` clean（`mod.rs` 两处就地改）。**GREEN**：`-- gateway::execution_engine::run_loop session::projection session::reduction` → `test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 18505 filtered out; finished in 4.30s`（7 条 `hook_stop_tests` + `project_row_labels_a_hook_stop_receipt` + `user_message_producers_are_the_known_set` 按名字 ok）。**变异**（`inner.rs` deny 臂删 `journal_hook_stop`）：同过滤器 `112 passed; 1 failed`，红名单**恰** `every_pre_seed_hook_exit_journals_the_stop`，消息 `run_loop/inner.rs: HookEvent::UserPromptSubmit exit 0 must journal exactly once before it returns` ⇒ 推导确实走到了 `inner.rs`。还原后 `touch inner.rs`，SHA-256 `f9516374…9026` 与变异前副本相同。**还原后更宽一轮**：`-- gateway::execution_engine session::projection session::reduction session::events` → `414 passed; 1 failed`，唯一红 `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` = T0 基线名单第 18 条（环境）；`execution_engine` 下全部既有 census（`no_reader_under_execution_engine_takes_the_uncorrected_scope_stamp`、`flow_scope_census::tests::*`、`RunRequest {` 字面量普查）全绿。**未做**：本轮未重跑全量 `--lib`——改动限于 `run_loop/{mod,inner}.rs` 与三处 doc 注释，覆盖过滤器已含整个 `gateway::execution_engine`，且 lib 测试二进制在本轮两次构建里都整程编译通过；clippy 仍未跑。
- **Fix round 1（ba3cabaab）**：审查 1 条 Important（注释级）+ minors (a)–(e)。**Important**——`journal_hook_stop` doc 与两臂注释里的「the user already saw the text」对 **deny** 臂为真（`Err` → `execute.rs:1201` 发 `RunError`），对 **prevent_continuation** 臂为假（两个孪生都是）：该臂 `return Ok(stop_msg)`，`execute.rs:956` 以 `Ok(_response)` 绑定后丢弃（`execute.rs` 里零处 `ResponseChunk`，`RunComplete` 单一源是没跑的 orchestrator drain）⇒ 那一臂上回执的投影行是**唯一**报告，批被拒/服务缺席 = 静默停止（回执前的行为）。裁定：保持 best-effort（brief 契约；升级是行为改动，不在 T9），把不对称**如实**写在 `journal_hook_stop` doc、`mod.rs` 两臂、`inner.rs` 站点注释与 prevent 臂、`project_row` 臂注释五处。`journal_hook_stop` 拆成「解析全局句柄 + 委托」与 `journal_hook_stop_with(Option<&dyn SessionService>, ..)`（返回 `()`——best-effort 作为类型：没有任何臂能让自己的 `return` 依赖回执结果）；census 仍在四个出口数 `journal_hook_stop(`。**Minors**：(a) `hook_stop_receipt` doc 声明种子 text only（`blocks: Vec::new()`，被停的附件回合 reload 无图；两个 seam 都在媒体解析之前开火）；(b) `dispatch_body` doc 点名它只认**一种**出口形状（第一个括号块内词法上的 `return `）与看不见的三种（hoisted result 的下一语句 `return`、`?`、`return;`）；(c) `events.rs` / `projection.rs` / `hook_stop_receipt` 三处按名列孪生的 doc 改为「pre-seed hook seams——集合由 `every_pre_seed_hook_exit_journals_the_stop` 推导」；(d) 测试 `a_refused_or_absent_journal_returns_normally`（T8 的 `RefusingSessionService` 改 `pub(super)`，`journal_hook_stop_with(Some(&refusing))` 与 `(None)` 都正常返回；变红情形 = 有人在里面 `expect` 批结果）；(e) 测试 `a_hook_stopped_resume_reduces_clean_in_both_real_shapes`——**审查给的字面形状 `[…UserMessage, RunStarted, RunFinished, ResumeAttempted, …]` 不是真实形状**（`RunFinished` 在 stamp 之前 ⇒ reducer 报 `ResumeWithoutTarget`）；读 `resume_coordinator.rs::stamp_resume_attempt`（`:1575-1589`）：stamp 的 target 是**开着的** `RunStarted` 或未答的 `UserMessage`，retrigger 前不写 `RunFinished`。两种真实形状各带对照：`[TurnStarted, UserMessage, RunStarted(crashed), ResumeAttempted{3}]` 无回执 ⇒ `Interrupted{attempts:1}`、接 resume 臂回执 ⇒ `Clean` 零矛盾；`[TurnStarted, UserMessage, ResumeAttempted{2}]` 无回执 ⇒ `Unanswered{user_seq:2, attempts:1}`、接回执 ⇒ `Clean` 零矛盾。**类型检查**：`cargo check -p alephcore --tests` → `Finished`；五个文件 `rustfmt --check` clean（`mod.rs` 两处就地改）。**GREEN**：`cargo test -p alephcore --lib -- gateway::execution_engine::run_loop gateway::execution_engine::tests session::reduction` → `test result: ok. 140 passed; 0 failed; 0 ignored; 0 measured; 18480 filtered out; finished in 5.78s`（9 条 `hook_stop_tests` + T8 的 `a_refused_open_batch_means_the_tool_never_runs` / `the_open_batch_is_in_the_log_…` 按名字 ok）。**未做**：本轮无变异（改动是注释 + 测试 pin + 一个纯委托拆分，(e) 自带「无回执 ⇒ Interrupted/Unanswered」对照）；未重跑全量 `--lib`；clippy 未跑；未经真 hook 走 `run_agent_loop`（extension manager 是进程全局 `CapabilitySlot`，首装即锁，lib 二进制内装不了带 stop hook 的第二份）——「臂仍返回其 Err/Ok」由类型担保（`journal_hook_stop_with` 返回 `()`，臂里没有可分支的值）而非行为测试。
### T11 — （commit: 86b427d64 · fix round 1: 22b4b0233）
- **前置**：`git status --porcelain` 空，HEAD `d454702c8`。`tasklist //FI "IMAGENAME eq rustc.exe"` 无进程。
- **穷举 match 普查**（只加 `ParkReason` + `ToolCallParked` 变体后）：`cargo check -p alephcore` → `error[E0004]` **恰 4 处**：`agents/subagent_spawner/fork.rs:217`（`is_prompt_bearing`）、`session/events.rs:586`（`durability_of`）、`session/store.rs:784`（`extract_turn_id`）、`store.rs:819`（`event_type_tag`）——与 brief 文件表一致，全部加**真话臂**（fork → `false`；durability → `Normal`；turn_id → `Some`；tag → `"tool_call_parked"`），无 `_ =>`。`rg "ToolCallDenied|tool_call_denied" shared interfaces` **零命中** ⇒ 事件集在 `shared/protocol` 无镜像（criterion #10 不涉）；矛盾 tag 集经 `session_snapshot.rs` 以 `c.tag().to_string()` 开放词汇透传，无镜像表。`DanglingCallView.parked` 线面**不在本任务**（plan §文件表 L172 归 T13），brief 里 `ParkReason` doc 那句「the wire word on `DanglingCallView::parked`」没抄——那个字段今天不存在（criterion #17）。
- **`boundary_repair_text` 调用者计数**（`rg -n "boundary_repair_text\(" src tests` 全仓，含多行）：**4 处**——1 处生产（`boundary_repair.rs::repairs_for`，传 `call.parked` 第 4 参）+ 3 处测试（同文件 `the_three_arms…`，已改名 `the_four_arms_are_four_different_sentences`，各补 `None`）；`session/mod.rs:26` 是 re-export 不是调用。`DanglingCall {` 构造点 **2 处**：`reduction.rs::reduce_run`（传 `d.parked`）+ `subagent_tool/recovery.rs:1507` 测试夹具（`parked: None`）。
- **RED**（`cargo check -p alephcore --tests`，测试先写）：E0004 ×4（上）、`E0061: this function takes 4 arguments but 5 arguments were supplied` ×5（`boundary_repair.rs:346/353/360/405/432`）、`E0599: no variant named ParkedWithoutRequest` ×2（`reduction.rs:965/997`）、`E0609: no field parked on type DanglingCall` ×4（`reduction.rs:1298/1314/1324/1340`）。
- **待核实项 B #1**（`code_text` 是否保住多行 `with_call_identity(\n Some(`）：普查 `every_production_dispatch_into_the_scoped_gate_is_scoped_by_a_call_identity` 绿——`act.rs:631` 的 `crate::approval::with_call_identity(` 与 `:955` 的 `with_call_identity(Some(identity), async move {` 两处 token 都在各自行内完整，`calls == scoped == 2`；originator 集恰 `["src/harness/agent/act.rs"]`。**裁定 1**照办：用 `utils::source_scan::rust_sources_under` + `production_text` + `code_text`，没抄 walker。
- **待核实项 B #2**（harness 之外的生产 `.execute(` 发起者）：`rg -n "\.execute\(\s*(&?name|name|&?tool)" src --glob '!**/tests.rs' --glob '!**/tests/**'` **6 命中**，逐条：`tools/service.rs:275` = trait 默认 `execute_with_cancel` 转发（定义方）；`tools/mcp_scope_view.rs:51` = `McpScopedToolService` 装饰器转发；`tools/scoped/dispatch.rs:670` = `self.inner.execute(&name_owned, input, cancel)` 是 `LoopToolRegistry` 的 3 参 execute（scoped 服务自身内部）；`agents/allowlist_tool_service.rs:60` = 装饰器转发；`:236/:247` = 该文件内联 `#[cfg(test)]` 测试。另扫 `\.execute\(\s*&?\w*(\.name|_name|name)\b` 多出 `tasks/heartbeat/probe.rs:122` = `ProbeExecutor::execute`（`executor::ToolRegistry` 面，不是 `ToolService`）。⇒ **零个** harness 之外的 `ToolService::execute` 发起者，全是转发/装饰/别的 trait。`tools.invoke` RPC（`gateway/handlers/tools_invoke.rs:258`）走 `ToolRegistry::execute_tool`，**不经** `ScopedToolService`——`record_approval_decision` 旧 doc 里「`None` outside harness dispatch (direct `tools.invoke` RPC, tests)」那半句已改写（它把一条绕开本服务的路说成了本服务的一种形状）。
- **待核实项 B #3/#4**：`InProcessActorSessionService::emit_batch` 在 `rx.await` 上等 actor 回执（`in_process.rs:348`）⇒ `emit_for_ambient_call(..).await` 返回时行已 append；`get_events` 有活 actor 时经 actor 串行读。效果由 `tests/parked_gate_integration.rs::a_gate_park_is_in_the_log_before_the_requester_is_reached` 钉住：`NeverAnswers` requester 在 `request_approval` 入口 `notify_one` 后 `pending()`，测试在该瞬间 `get_events` 读到**恰 1 行** `ToolCallParked{call_id:"toolu_parked", reason:Approval}`。
- **类型检查**：`cargo check -p alephcore --tests` → `Finished`（lib 侧仍只有既有 4 条 `never used`）；`cargo check -p alephcore --features test-helpers --test parked_gate_integration` → `Finished`（3m24s）。`rustfmt --edition 2021 --check` 11 个触及文件：自己的区域全 clean（`events.rs`/`reduction.rs`/`boundary_repair.rs` 三个叶文件跑了 `rustfmt`，随后把它写出的 LF 还原成 CRLF——它们都不声明子 `mod`，安全）；`dispatch.rs:1780` 一处 diff 是**既有**代码（`looks_like_cancellation`，未动）。`node --check qa/resume_boundary/drive_r2.mjs` OK。
- **GREEN 第一轮**：`cargo test -p alephcore --lib -- session::reduction session::boundary_repair session::call_log session::events session::store::tests tools::scoped::tests clarification::ask agents::subagent_spawner::fork agents::subagent_tool::recovery` → `test result: ok. 256 passed; 0 failed; 0 ignored; 0 measured; 18371 filtered out; finished in 4.26s`。按名字：`reduction::tests::{a_parked_dangling_call_carries_its_reason, an_answered_gate_ends_the_park, a_park_pairs_with_the_nearest_unanswered_dispatch_of_its_id, a_park_without_a_dispatch_is_reported_and_ignored, every_prefix_of_every_legal_shape_is_green（新增 LegalShape「approval-parked call, approved, then result」）, tags_are_derived_from_the_serde_kind, the_two_reject_kinds_are_exactly_out_of_order_and_non_marker（KIND_COUNT 11，REJECT 仍 `0|1`）, user_message_producers_are_the_known_set}`、`boundary_repair::tests::{the_four_arms_are_four_different_sentences, a_parked_dangling_call_is_answered_with_the_parked_arm}`、`events::tests::{park_reason_wire_word_is_the_serde_name_and_display_is_the_clause, durability_barrier_set_is_exactly_the_four_ruled_events}`（T1 普查：Barrier 集不变，`sample_of_every_kind` 加了 `ToolCallParked`）、`store::tests::marker_event_types_are_exactly_the_reducers_marker_set`（T6）、`fork::tests::fork_kinds_track_the_prompt_builder`、`tools::scoped::tests::every_production_dispatch_into_the_scoped_gate_is_scoped_by_a_call_identity` 全 ok。
- **集成面**：`cargo test -p alephcore --features test-helpers --test parked_gate_integration --test agent_ledger_gate_refusals -j 1` → 两个二进制各 `test result: ok. 1 passed; 0 failed`（`a_gate_park_is_in_the_log_before_the_requester_is_reached` / `a_gate_that_refuses_without_an_approval_channel_still_signs_a_record`——重构后的决策路径仍签 ledger）。
- **GREEN 第二轮**（自审后加的 in-crate 效果测试 `tools::scoped::tests::an_answered_gate_leaves_park_then_decision_in_the_session_log`：`install_test_session_service` + `SessionKey::ephemeral` 独占键 + `with_call_identity` 包住 `svc.execute`，`FakeRequester(Denied)`，`get_events` 读回 `event_type_tag` 序列**恰** `["tool_call_parked","tool_call_denied"]`，首行 reason `Approval`、call_id 为环境 id）：同过滤器 + `session::in_process` → `test result: ok. 273 passed; 0 failed; 0 ignored; 0 measured; 18355 filtered out; finished in 4.28s`。
- **变异**（每次还原后 `touch`，SHA-256 与变异前副本逐字节相同；`git diff --stat` 不变）：
  - **A**（一次 lib 构建，两处并列）：reducer 的 `ToolCallApproved` 臂不再清 `parked` + `boundary_repair_text` 第四臂改为不可达（`parked.filter(|_| false)`）→ `-- session::reduction session::boundary_repair`：`56 passed; 3 failed`，红名单**恰** `reduction::tests::an_answered_gate_ends_the_park`（`left: Some(Approval) right: None`）、`boundary_repair::tests::the_four_arms_are_four_different_sentences`（`:442`，文本落回 OUTCOME UNKNOWN 臂）、`boundary_repair::tests::a_parked_dangling_call_is_answered_with_the_parked_arm`（`:499`）。还原 SHA `96c26297…7857`（reduction.rs）/ `1eb8badd…293c`（boundary_repair.rs）。
  - **B**（brief 第 5 步）：`self.record_parked(name, reason).await` 挪到 `request_approval(..).await` **之后** → `--test parked_gate_integration -j 1`：`0 passed; 1 failed`，红**恰** `a_gate_park_is_in_the_log_before_the_requester_is_reached`（`left: 0 right: 1`）。还原 SHA `866e1a72…aa08b`（dispatch.rs）；还原后重跑同两个集成二进制：输出里 `Compiling alephcore` 出现（确实重编了），两条各 `1 passed`。
  - QA `parked` stage（T13）本轮**未跑**（本机无完整 server 构建；`knobs` 探针改动只做了 `node --check`——**unexercised**，T23 跑）。
- **全量 `--lib`**（86b427d64 的工作树内容，提交前跑）：`test result: FAILED. 18559 passed; 52 failed; 17 ignored; 0 measured; 0 filtered out; finished in 651.93s`。按名字与 T0 基线 51 条 `comm -3`：新增 **1 条** `skill::usage::tests::concurrent_bumps_do_not_lose_counts`（控制者点名的三条 flaky 之一，未修）、消失 **0 条**。本任务 15 条测试在全量里按名字全 `ok`；`harness::tests::budget::the_harness_line_budget_does_not_grow` 红在基线里，`git status` 下 `src/harness/` **零改动**。未 WEDGE（第 2 轮轮询内出现 `.done`）。
- **与 brief / 裁定的偏差（点名）**：① **裁定 2 的 `debug_assert!(false, …)` 没放**：`tools/scoped/tests.rs` 里 **24 条**单测带 `with_turn_context` + requester、直接 `svc.execute` 不包 `with_call_identity`；同一 lib 二进制里 **3 条**测试调 `install_test_session_service`（`run_loop/mod.rs:1015`、`execution_engine/tests.rs:1953`、`db_handlers/modify.rs:1095`）填进程级槽——线程交错决定那 24 条里哪条先看见 `Some(svc)` 再撞 `None` identity，`debug_assert!` 就是一个「提交源码产生不了」的随机红。改为 `warn!` + `DROPPED_FOR_MISSING_IDENTITY` 计数（第一次开口报出的数就是那一类的真实大小，criterion #11）；§6.2 由普查以等式钉住，`debug_assert!` 不多钉任何东西。② 第四臂 lead **按 provenance 分两句**（`ThisRestart` = brief 原句「when the server restarted it was still waiting for {reason}」；`EarlierRun` = 「an earlier run in this session ended while it was still waiting for {reason}」）——一条被 `/undo`/closer 关掉的 run 留下的 parked dangle 不是「服务器重启时」，与既有两臂同一理由（「the difference between a true sentence and a false one」）；测试在 `EarlierRun` 上多断言 `!contains("the server restarted")`。③ `ask.rs` 的 log 标签写 `"clarification::ask"` 不是 brief 的 `"ask_user"`——scratchpad 计划门也经它 park，标签只是 warn 上下文，事件不带名。④ `ParkReason` 没加进 `session/mod.rs` 的 re-export（brief 未要求，路径 `session::events::ParkReason`）。⑤ 多 1 条测试（`an_answered_gate_leaves_park_then_decision_in_the_session_log`）。
- **未做**：QA `knobs`/`parked` 真机（上）；`PreHook` 推导（`action.rule_id == HookRequested.id()` 两行 `if`）无独立行为测试——hook 测试全 `cfg(unix)`，本机 Windows；`docs/reference` 事件表 / `SESSION_SERVICE.md` 归 T24 未动；clippy 未跑；`DanglingCallView.parked` 线面归 T13。
- **Fix round 1（22b4b0233）**：审查 0 Critical / 4 Important / 11 Minor；前置 `git status --porcelain` 空、HEAD `25e7e1cf9`、无 rustc。
  - **I1 第三个 park 站点（裁定：盖章）**：`sandbox/workspace/mod.rs` 的能力提升卡（shell 工具自己的 `execute` 内、`ToolCallRequested` 之后、harness 的 `with_call_identity` 作用域内）经同一 writer 写 `ToolCallParked{Approval}`，`site = cmd.tool_name`，在 `request_approval_for_action(&action).await` **之前** await 到 store，park 不依赖写入。**后果（自己发现、自己处理、点名交裁）**：`bash_exec::spawn_background`（`:466/:502`）刻意在分离任务里重进 call identity，所以**后台作业**的能力卡会在派生它的 `bash` 调用已拿到回执（`ToolResult`）之后落 `ToolCallParked` —— 按 86b427d64 的 reducer 这是一条 `ParkedWithoutRequest`（对一个**设计如此**的形状报矛盾 = 会被当证据引用的误报，判据 #3）。改法：`ToolCallParked` 臂三分——最近的未答且未拒派发 ⇒ parked；**同 id 的派发全已答** ⇒ 分离作业的卡，**不标不报**（与 `ToolCallApproved`/`ToolCallDenied` 回执之后的既有静默读法同一条）；无派发或最近未答的已被拒 ⇒ `ParkedWithoutRequest`（Minor #3 裁定的 `&& !d.denied`）。测试：`a_park_after_the_receipt_marks_nothing_and_reports_nothing`（dangling 空、tags 空、answered==1）、`a_park_after_a_denial_is_reported_and_the_denial_kept`（tags **恰** `[parked-without-request, dangling-denied-call]`、`denied && parked.is_none()`）、LegalShape「background job's capability card after the spawn call returned」（`requested→result→parked→approved`，allowed 空）。`ParkedWithoutRequest` doc / `DanglingCall.parked` doc（「never `Some` on a call whose `denied` is set」现在由臂保证）同笔改真。效果测试 `sandbox::workspace::tests::the_capability_card_is_in_the_log_before_the_requester_is_reached`（`NeverAnswers` requester `notify_one` 后 `pending()`，`install_test_session_service` + `sid()` ephemeral 键 + `with_call_identity`，在 requester 入口那一瞬 `get_events` ⇒ **恰 1 行**且只有这 1 行 `ToolCallParked{toolu_elevated, Approval}`，随后 abort）。**普查** `tools::scoped::tests::every_production_approval_park_is_stamped_or_named_exempt`：语料 `rust_sources_under` + `production_text` + `code_text`；排除定义 `fn request_approval(`/`fn request_approval_for_action(` 的文件（requester 面的实现/转发：`gate.rs`、guardian/adapters/operator requesters、`cluster/node_approval.rs`；`scratchpad.rs::request_approval` 是**同名无关方法**、经 `clarification::ask` park 并在那里盖章——doc 里点名它是被同一规则捎带排除的，不是分类）；站点 = 每个 `.request_approval(`/`.request_approval_for_action(` 行；等式 `[failover/provider.rs, sandbox/workspace/mod.rs, tools/scoped/dispatch.rs]`；两个 STAMPED 文件的每个站点在其上方 `WINDOW_LINES = 20` 行内、无 `fn ` 头穿过（同函数）须有 `record_parked(`/`emit_for_ambient_call(`（实测距离 **5**（dispatch.rs:969→974）与 **17**（workspace/mod.rs:343→360），窗口=实测+余量，doc 写明）；`failover/provider.rs` 是**具名豁免**（THINK 阶段 park、无 call_id，`escalation_allowed`）且断言它**没有**盖章调用。盲区照 §6.2 普查 doc 写明（也定义同名方法的 originator 看不见）。
  - **I2（裁定）**：`emit_for_ambient_call` 返回 `()`；四个调用点本就丢弃；doc 改成「best-effort as a type」。**I3**：三处说谎注释按审查措辞改（call_log.rs 4-6「One site used to …; the park would have been two more copies」；23-24 删判据 #11 那句、改成「flood vs one-off」；集成测试 13-14 改成「进程级槽只认第一次安装，集成二进制能装自己的 in-memory 服务；lib 二进制的槽由先装者共享，in-crate 孪生经 ephemeral 键读它」）。**I4（裁定）**：`Clarification` 专用第四臂正文——`"NOT ANSWERED — this `{tool}` call delivered its question, but no answer was recorded before {ended}; {reason} is still missing. If you still need the answer, ask again. {VERIFY_CLOSE}"`（`ended` 按 provenance = 「the server restarted」/「an earlier run in this session ended」；`{reason}` 让 `Display` 三个变体都有读者）；Approval/PreHook 正文不变（lead 改写成 `"{ended} while it was still waiting for {reason}"`，语义同前）。**加了前缀词 `NOT ANSWERED`**（与其余三臂同一「大写 lead — 正文」形状），`drive_r2.mjs` 的 `REPAIR_MARKERS` 同笔加第三项（T13 若在 `ask_user` 上 park 直接可用；`knobs` 仍在 approval 门 park，探针语义不变）。测试：`the_four_arms_are_four_different_sentences` 改成——Approval(ThisRestart) 含 `never ran`；Clarification(ThisRestart) 含 `delivered its question`/`no answer was recorded`/`ask again`，**不**含 `never ran`/`Nothing it would have done`/`the gate`/`OUTCOME UNKNOWN`；六对不等式（parked/unanswered 各对 denied/restart/earlier 及互相）；`for reason in ParkReason::ALL`（EarlierRun）：含各自 `Display` 子句、`contains("never ran") == !Clarification`、不含 `the server restarted`。
  - **Minors**：#1 §6.2 普查 doc 加一句「看不见 2 参 `.execute(` originator；`.execute(` 名字太常见无法文本普查；今天唯一生产 2 参 `ToolService::execute` 是 `AllowlistToolService` 转发——是测量不是钉」；#3（上）；#4 doc 改「four arms (the parked and unknown arms each carry two provenance leads)」；#5 `ParkReason::Clarification` doc 点名 scratchpad 计划门、并说明「question WAS shown」；#7 `ParkReason::ALL: [ParkReason; 3]` + `park_reason_all_lists_every_variant_once`（穷尽 `match` 派 index + `seen` 数组：新变体先编译失败、再越界红）；两处手写数组改成迭代 `ALL`；#8 集成测试 `:62` 继承注释改真（confirm 门由 `requires_confirmation()` 触发，角色只为与 ledger 测试夹具一致）；#9 参数改名 `site`。**未动**（控制者 deferred）：#2 `declared_as_a_test_module` 对 `src/harness/tests` 盲（T17）、#6 `as_str` 消费者（T13）、#10 33 条门测试的 warn! 噪音、#11 文件体积。
  - **类型检查**：`cargo check -p alephcore --tests` → `Finished`（一次 E0502 借用冲突就地改成先算 `any_of_id`）；`rustfmt --check` 8 个触及文件 clean（`boundary_repair.rs`/`scoped/tests.rs` 跑了 `rustfmt`，LF 还原成 CRLF）；`node --check drive_r2.mjs` OK。
  - **GREEN**：`cargo test -p alephcore --lib -- session::reduction session::boundary_repair session::call_log session::events tools::scoped::tests sandbox::workspace clarification::ask` → `test result: ok. 228 passed; 0 failed; 0 ignored; 0 measured; 18405 filtered out; finished in 4.36s`（12 条本轮新增/改动测试按名字全 ok）。集成：`--test parked_gate_integration --test agent_ledger_gate_refusals -j 1` → 各 `1 passed; 0 failed`。
  - **变异**（沙箱盖章整段注掉；还原后 `touch`，SHA-256 `93628187…5385` 与变异前副本相同，`grep MUTATION` 0）：`-- tools::scoped::tests::every_production_approval_park sandbox::workspace::tests::the_capability_card` → `0 passed; 2 failed`，红名单**恰** `sandbox::workspace::tests::the_capability_card_is_in_the_log_before_the_requester_is_reached`（`left: (0, 0) right: (1, 1)`）与 `tools::scoped::tests::every_production_approval_park_is_stamped_or_named_exempt`（`src/sandbox/workspace/mod.rs:350 awaits a requester with no ToolCallParked stamp within 20 lines of the same function`）。还原后同过滤器重跑：输出含 `Compiling alephcore`（确实重编），`228 passed`；集成两条各 `1 passed`。
  - **未做**：本轮无全量 `--lib`（控制者：不碰其他模块穷举的 match 则不需要；本轮没动 `SessionEvent` 枚举、没动任何穷举 match）；clippy 未跑；后台作业「卡在回执之前落地」的竞态形状（`requested→parked→result`）只由既有 reducer 规则覆盖（parked 后 answered ⇒ 不 dangling），未单独钉。
### T12 — （commit: 4f300b435）
- **前置**：`git status --porcelain` 空，HEAD `4c9ba0699`。`tasklist //FI "IMAGENAME eq rustc.exe"` 无进程。全部改动一条提交（brief 正文 + 四个 carried items）。
- **重数（判据 #6，去注释，`src/` + `tests/` + `src/harness/tests/`）**：`RunEnvelopeSnapshot {` 构造字面量 **10 处**（brief 写 6；T8/T11 之后多 4）——列全字段 5 处（`runner_impl.rs::run_envelope_snapshot` 生产、`events.rs` 往返测试、`events.rs` 普查 `all`、`resume_coordinator.rs` 夹具 `envelope_with`、`tests/resume_coordinator_integration.rs:378`）全加两字段；`..Default::default()` 5 处（`slash_command.rs` 生产 → 改走 `from_knobs`、`fast_path.rs:436`、`execution_engine/tests.rs:2004`、`reduction.rs:1591/1595`）不动。`TurnEnvelope {` **4 处**（与 brief 同）：`inner.rs` 生产加两字段，三处测试 `..default()`。`src/harness/` 两个类型都不构造（grep 0）。skill 键：writer 1（`stamp_list`，被 `stamp_from_mode` 与 `plan_resume` 喂）、reader 1、stripper 1。
- **RED**（`cargo test -p alephcore --lib -- session::events resume_coordinator slash_skill_scope run_envelope_snapshot_tests session_split task10_wiring`）：`could not compile alephcore (lib test) due to 27 previous errors`，全是本任务引入的名字：`E0425 RUN_ENVELOPE_FACT_KEYS`、`E0560 RunEnvelopeSnapshot has no field allowed_tools/btw`（多处）、`E0560 TurnEnvelope has no field allowed_tools/btw`、`E0599 no variant NoOpenRun … SplitError`、`E0599 from_knobs` ×2、`E0425 stamp_list`、`E0425 retrigger_emitter` ×2。
- **类型检查**：`cargo check -p alephcore` → `Finished dev … 3m 27s`（只有既有 4 条 `never used`）。`cargo fmt -- --check` 触及的 15 个文件：本任务区域 1 处 diff（新断言换行），已改；仓库其他既有 fmt diff 未动。
- **GREEN**（`… --lib -- session::events resume_coordinator slash_skill_scope run_envelope_snapshot_tests session_split session_snapshot origin_fanout btw task10_wiring execution_engine::tests fast_path slash_command`）：`test result: FAILED. 251 passed; 1 failed; 0 ignored; 0 measured; 18390 filtered out; finished in 6.17s`——唯一红 `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` 在基线里（本工作树 submodule 未初始化，「parsed 0 skill(s) and 0 plugin director(ies)」）。按名字全 ok：`events::tests::{an_old_run_started_without_the_fact_keys_decodes_to_none_and_none_stays_off_the_wire, from_knobs_spells_each_knob_by_its_id_and_leaves_the_rest_unset, the_envelope_carries_exactly_the_published_knob_keys（want = KNOB ∪ FACT）, run_started_with_an_envelope_round_trips}`、`runner_impl::run_envelope_snapshot_tests::the_marker_records_the_per_run_facts_the_turn_is_running_under`、`slash_skill_scope::tests::stamp_list_round_trips_through_from_metadata_including_the_empty_declaration`、`resume_coordinator::tests::{a_snapshot_scope_reaches_the_request_and_an_absent_one_stamps_nothing, a_btw_question_is_replayed_but_a_promote_sentinel_is_not, a_stamped_resume_skips_the_origin_fan_out_and_an_unstamped_one_takes_it, an_envelope_that_never_named_the_model_resumes_degraded_and_says_so（新句正向三段 + 反向 `!contains("was not recorded")`）}`、`session_split::tests::{the_child_opener_carries_the_parents_envelope_and_project_root（经 recording service `emitted()` 读回）, a_parent_without_an_open_run_is_refused_before_any_batch（无 marker / 已关闭的 run 两形状，`Err(NoOpenRun)`、零 batch、registrar 空）}` + 六条改形夹具、`task10_wiring::extras::split_session_{directive_continues_run_in_child_session, survives_a_failed_epoch_registration, failsoft_on_a_refused_batch_compacts_and_continues}`（三条 split 测试的 `MockSession` 种子加 `run_started_event()`）、`origin_fanout::tests::every_fan_out_construction_site_is_answered_in_format_side_answers_doc`（改名，文件集不变）、`btw::guard_tests::{btw_is_not_filed_with_the_five_session_knobs, only_the_shared_resolver_decides_what_a_side_question_is}`、`fast_path::tests::the_open_batch_carries_one_turn_the_envelope_and_the_input`、`btw_wire_tests::the_router_claims_btw_ahead_of_a_catalog_that_could_resolve_it`。
- **fast path 核实（carried item 2）**：`/<skill>` **到不了** fast path——`slash_command.rs` 的 `"skill"` 臂恒 `Fallthrough`，`direct_tool` 的 mode JSON 不带 `allowed_tools` ⇒ `allowed_tools: None` 是真话。`/btw` **在 RPC 面能到**（非出厂世界）：channel 面由 `inbound_router/mod.rs:884` 抢在解析器前认领（`the_router_claims_btw_ahead_of_a_catalog_that_could_resolve_it` 钉）；RPC 面 `stamp_slash_mode` 先盖 btw 戳**再**问解析器，出厂目录无 `btw` 命令词（`no_shipped_command_word_resolves_as_a_side_question` 钉，它自己的文案就说「`stamp_slash_mode` would stamp SLASH_COMMAND_MODE_KEY and the engine's fast path would run it」），但**用户自装**的 `btw` 同名工具不在那张普查里 ⇒ fast path 现在从 request 复制 `btw` 戳（与 `run_loop` 同一推导），注释写明「出厂恒 `None`，非出厂如实记录」。同时暴露的**既有**洞：那个世界里 RPC 面会**无闸**跑那个工具（`slash_gate_reason` 有意丢 `side_question`）——本任务不修，报控制者。
- **变异**（一次 lib 构建、四处独立变异各对一条具名测试；测于已提交的 `4f300b435`，`git checkout --` 还原 + `touch`，还原后 `git status --porcelain` 空）：`test result: FAILED. 120 passed; 4 failed`，红名单**恰**——A `plan_resume` 删 `stamp_list` 调用 → `a_snapshot_scope_reaches_the_request_and_an_absent_one_stamps_nothing`；B `RUN_ENVELOPE_FACT_KEYS` 忘掉 `btw` 键（`[&str; 1]`）→ `the_envelope_carries_exactly_the_published_knob_keys`（控制者要的「omit `btw` from `all`」在全字段字面量上是编译错误不是红测，故变异的是普查真正要抓的形状：字段在、键忘了）；C split 子 opener 改回 `envelope: None` → `the_child_opener_carries_the_parents_envelope_and_project_root`；D `retrigger_emitter` 去掉 side-question 早返回 → `a_stamped_resume_skips_the_origin_fan_out_and_an_unstamped_one_takes_it`。
- **全量 `--lib`**（`4f300b435`，变异还原后）：`test result: FAILED. 18572 passed; 53 failed; 17 ignored; 0 measured; 0 filtered out; finished in 656.37s`。按名字与基线 51 条 `comm`：新增 **2** — `skill::usage::tests::concurrent_bumps_do_not_lose_counts`、`tools::usage::store::tests::concurrent_records_do_not_lose_counts`（控制者点名的 flaky，未动）；消失 **0**。本任务 20 条测试全量里按名字全 ok。`harness::tests::budget::the_harness_line_budget_does_not_grow` 红在基线里且文案逐字相同（`5250 … ceiling of 5239`）——`src/harness/tests/task10_wiring/` 的 tests-only 改动不计入预算；`src/harness/agent/*`、`budget.rs::CEILING` 零改动。未 WEDGE。
- **集成面**：`cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1` → `EXIT=0`，`Finished test … 27m 35s`，**139 个** `Executable`，零 `error`，含 `tests\resume_coordinator_integration.rs`（本任务唯一触及的集成文件，只加两个 `None` 字段）。未运行。
- **与 brief / 裁定的偏差（点名）**：① **`RUN_ENVELOPE_FACT_KEYS` 住在 `session/events.rs`，不在 `session_snapshot.rs`，且 `btw` 用 `crate::gateway::btw::BTW_METADATA_KEY` 拼**——两条既有守卫让 brief 的位置在任何拼法下都红：`btw_is_not_filed_with_the_five_session_knobs` 对 `session_snapshot.rs` 生产源里**任何**含 `btw`（不分大小写，常量名也算）的行报红；`only_the_shared_resolver_decides_what_a_side_question_is` 对全仓任何非 `const …: &str =` 定义行上的 `"btw"` 字面量报红（且只准一条定义）。`btw/mod.rs` 模块 doc 本就写着「does NOT appear … in `session_snapshot.rs`」。普查谓词不变（KNOB ∪ FACT == 结构体键集，等式，两个数组都具名），且在忘键时红过。`session_snapshot.rs` 的 KNOB_KEYS doc 改指向兄弟数组。用戳的常量拼是绑定不是巧合：改戳键名 ⇒ 普查红 ⇒ marker 字段必须跟着改。② `SplitError::NoOpenRun` 是新变体不是 `Failed(anyhow!)`——按约拒绝是独立分类，测试 `matches!` 它；`Failed` 保留兜底语义（doc 加了 `reduce_run` 的 `Err` 臂）；唯一生产调用者 `directive.rs` 只 `%e` 并回退 compact-to-fit，不用加臂。③ `origin_fanout.rs` 普查改名（`the_fan_out_construction_sites_are_still_the_four_that_cannot_carry_a_side_question` → `every_fan_out_construction_site_is_answered_in_format_side_answers_doc`），doc 与 assert 文案去掉「four」——resume 成了按规则的站点后，名字里的数目就是会烂的名单（判据 #16）；`ANSWERED` 文件集不变。④ `busy_queue/durable.rs` 那句「the four pre-existing sites are all unreachable by side questions」改真（它描述的是 resume，本次之后为假，判据 #1）；只改注释。⑤ 扇出测试是 in-crate、测抽出来的 `retrigger_emitter`，不是端到端穿 `retrigger`——`ResumeCoordinator` 只在集成二进制里构造、channel registry 是进程级 `OnceLock`；抽取保留 `OriginFanoutEmitter::new(` 在 `resume_coordinator.rs`（普查文件集不动），测试证效果（假 channel 对盖戳的那份看到 0 条、对未盖戳的看到回复）。⑥ fast path 填 `btw`（brief 留 `None`）——按 carried item「only if that is true」与上面的核实。⑦ `is_promote` / `PROMOTE_STAMP` doc 点名新读者（旧文说 `is_promote` 是哨兵唯一读者，现 `plan_resume` 经常量比对以拒绝重放 promote）。⑧ T8 留在 `snapshot_model_pair` 上的「④ Freeze…」doc 块搬回 `run_envelope_snapshot`；`is_empty` doc 里「split / compaction / sub-agent」那张名单改成不列举（split 现在会捕获 envelope）。
- **未做 / 顾虑**：RPC 面 `/btw` 走 fast path 无闸（上，既有，`stamp_slash_mode` 在已盖戳时跳过解析器一行可关，未动）；首轮 split 会把父 run 自己的 `RunStarted` 随 tail 逐字复制进子会话、再追加 split 自己的 opener（既有形状，`reduce_run` 取最后一条 ⇒ 本任务让两条答案一致，重复 marker 本身未处理）；`retrigger` 现在无条件 `await agent.origin_route(..)`（此前只在有 registry 时）——便宜的元数据读；`--bins` / clippy / tui / cli / panel 未跑（未触及）；无真机 QA 阶段覆盖两事实的重放（`qa/resume_boundary` 无 `/btw`、`/<skill>` 阶段）；集成测试只加了两个 `None` 字段、不端到端走事实；`docs/reference` 归 T24。
- **Fix round 1（de774d9b9）**：审查 Approved，0 Critical / 2 Important / 8 Minor；前置 `git status --porcelain` 空、HEAD `ca9f2e122`、无 rustc。
  - **I1（谎言机制）**：`plan_resume` 的 promote 拒绝注释与 `a_btw_question_is_replayed_but_a_promote_sentinel_is_not` 的断言文案原写「会落进 promote 臂」——假：promote 臂（`execute.rs:430-436`）由 `btw_main_session.is_some()` 门住，只有从 **主键** 重定向才产生，而每条 `RunStarted { btw: Some(_) }` 都写在侧会话上、resume 又寻址到那个侧会话 ⇒ 永远到不了那条臂；真实后果是在只读天花板下跑一轮无意义的侧会话 turn。两处改成真话，裁决不变（不重放）。
  - **I2（夹具说谎 + 生产顺序夹具）**：`parent_log` doc 原称「harness bridge 会产生的顺序：opener 在前」——生产顺序是 message-then-opener（`reduction.rs:69-70`；bridge 在 gateway 播种 turn 之后于 `runner_impl.rs:978` 发 `RunStarted`）。doc 改为「opener 放在消息之前是为了让被摘要的那一半拥有它；bridge 的真实顺序相反」，并新增 `a_parents_opener_copied_inside_the_tail_still_yields_the_split_opener_last`：生产顺序 `[user, assistant, user, RunStarted]`、`tail_start=2` ⇒ 父 opener 落在被逐字复制的 tail 里；经 service `emitted()` 读回子会话按发射顺序**恰 2 条** `RunStarted`——第一条是复制的 `run-parent`，**最后一条**是 split 自铸的 run_id（`assert_ne!`）且携带父 envelope + `PARENT_ROOT`。这就是顾虑 B 依赖的「最后一条为准」读法，现在有夹具钉住。
  - **Minor #4（裁定：常量搬家）**：`RUN_ENVELOPE_FACT_KEYS` 从 `session/events.rs` 搬到 `gateway/resume_coordinator.rs`、紧挨 `plan_resume`（唯一生产读者），仍用 `btw::BTW_METADATA_KEY` 拼；doc 写明为什么不在 `RUN_ENVELOPE_KNOB_KEYS` 旁边（guard 6 禁止 `session_snapshot.rs` 里出现任何 `btw` 文本）、为什么不在 `session::events`（让 `session` 不依赖 `gateway`）。`events.rs` 的普查测试 `use crate::gateway::resume_coordinator::RUN_ENVELOPE_FACT_KEYS`；`events.rs` / `session_snapshot.rs` 的 doc 链接改指新址。**核实**：`grep -rn "crate::gateway" src/session/` 去掉注释行后 **5 命中**，全部在 `events.rs` 第 1124–1187 行、位于 `#[cfg(test)]`（第 907 行）之下 ⇒ **`src/session/` 的生产代码对 `crate::gateway` 的引用 = 0**；`src/session/` 其他文件 `gateway::` 代码引用 0。
  - **Minor #3**：`btw/mod.rs` 的 `is_promote` doc 与 `format_side_answer` doc 把 `contains_key` 读者改成 `ResumeCoordinator::retrigger`（读的是它，`resume_coordinator.rs` 的 `is_side_question` 那行），`retrigger_emitter` 只收 bool。
  - **Minor #5**：`MockProvider` 没有调用计数、也不想为一个消费者给生产用 mock 加字段 ⇒ 在 `session_split` 测试模块内加 `CountingSummarizer`（`AtomicUsize` + `AiProvider` 三个必需方法）。拒绝测试两个形状各断言 `calls() == 0`；**正向对照**：同一个 double、有 open run 的 `parent_log` 夹具（1 条 pre-tail 消息，`summarize_slice` 只对空列表短路）⇒ `calls() == 1`，让 0 是测量而不是一个到不了的 mock（判据 #2）。
  - **Minor #6**：`retrigger_emitter` 的 `route` 参数改成 `impl Future<Output = Option<(String, String)>>`、函数变 async；只有「非侧问题 **且** 有 registry」时才 `.await` 它——恢复 T12 之前「有 registry 才查路由」的惰性，并让侧问题也不付这笔查找；决定点仍只有一个（没有在 `retrigger` 里再写一遍 `if is_side_question`）。测试用带 `AtomicBool` 的 route future 钉住：盖戳那份 **从未被 poll**（`!looked_up`），未盖戳那份 poll 了且扇出到假 channel。`btw/mod.rs` 与 `format_side_answer` doc 提到的机制未变。
  - **Minor #7**：`directive.rs` 的回退日志改成「session split refused or failed; falling back to compact-to-fit」（`%e` 文本本就区分；仓内无测试钉旧文案）。
  - **Minor #8（报告失实）**：报告里「`src/harness/` 构造两个类型都为 0（grep）」是改动前的话——改动后 `task10_wiring/mod.rs::run_started_event()` 构造了一个 `RunEnvelopeSnapshot`（tests-only）；报告已改正。
  - **类型检查**：`cargo check -p alephcore` → `Finished dev … 1m 03s`（既有 4 条 `never used`）。`cargo fmt -- --check` 本轮 6 个文件：1 处 diff（新 `assert_ne!` 换行）已改。EOL 逐字节审计：6 个文件全 CRLF、0 bare LF。
  - **GREEN**：`cargo test -p alephcore --lib -- session::events resume_coordinator session_split slash_skill_scope btw task10_wiring session_snapshot origin_fanout compact::directive` → `test result: FAILED. 189 passed; 1 failed; 0 ignored; 0 measured; 18453 filtered out; finished in 2.63s`——唯一红仍是基线 `no_shipped_command_word_resolves_as_a_side_question`（submodule）。本轮新增/改动测试按名字全 ok：`session_split::tests::{a_parents_opener_copied_inside_the_tail_still_yields_the_split_opener_last, a_parent_without_an_open_run_is_refused_before_any_batch, the_child_opener_carries_the_parents_envelope_and_project_root}`、`resume_coordinator::tests::{a_stamped_resume_skips_the_origin_fan_out_and_an_unstamped_one_takes_it, a_btw_question_is_replayed_but_a_promote_sentinel_is_not}`、`events::tests::the_envelope_carries_exactly_the_published_knob_keys`、`btw::guard_tests::{btw_is_not_filed_with_the_five_session_knobs, only_the_shared_resolver_decides_what_a_side_question_is}`、`origin_fanout::tests::every_fan_out_construction_site_is_answered_in_format_side_answers_doc`、`task10_wiring::extras::split_session_*` 三条。
  - **变异**（控制者点名的那一条，在 `de774d9b9` 上做）：普查 `all` 里的 `btw: Some("h")` 换成 `..Default::default()` → `-- session::events`：`18 passed; 1 failed`，红**恰** `the_envelope_carries_exactly_the_published_knob_keys`（`left` 缺 `btw`、`right` 有）。`git checkout --` 还原 + `touch`；`git hash-object` 与 `HEAD:src/session/events.rs` 同为 `ee1c403ab…084ac`，`grep MUTANT` 0；重跑 `-- session::events resume_coordinator session_split` 输出含 `Compiling alephcore`（确实重编）→ `test result: ok. 61 passed; 0 failed`。
  - **未做**：无全量 `--lib`（本轮没动结构体、枚举或穷举 match）；集成二进制未重编（`RUN_ENVELOPE_FACT_KEYS` 在 `tests/` 下零引用，grep）；顾虑 A（用户自装 `btw` 同名工具在 RPC 面无闸走 fast path）与顾虑 B 的重复 opener 本身按控制者指示**未动**（后续清单）。
### T13 — （commit: d32083dfc · fix round 1: 2722d53e0）
- **前置**：`git status --porcelain` 空，HEAD `7cbca5bcc`。`tasklist` 无 `rustc.exe` / `cargo.exe`。共享 target 里已有 debug `aleph-server.exe`（Sep 13 10:20）。Node v24.13.0。全部改动一条提交。
- **重数（判据 #6，去注释，`shared/` + `src/` + `interfaces/` + `tests/`）**：`DanglingCallView {` 构造字面量 **5 处**（brief 写 5，一致）——生产 `session_snapshot.rs::dangling_view` 1 处；测试 `session_thread.rs`、`chat_sidebar.rs::dangling(n)`、`commands.rs` ×2。全部加 `parked`（生产处是真映射，测试处 `parked: None`）。核心 `DanglingCall.denied` 读者 **4 处**（`dangling_view`、`recovery.rs::in_flight_json`、`recovery.rs::interrupted_note`、`boundary_repair.rs:190`——第四处是审查补上的，首报写 3，正是判据 #6「数错永远少一个」的方向），每处同时读 `.parked`（`boundary_repair.rs` 自 T11 起就读）。`rg in_flight_calls src tests` 只命中 `recovery.rs`——没有别处断言那一行的形状。
- **RED**：`cargo test -p aleph-protocol` → `E0560 DanglingCallView has no field named parked` ×2、`E0599 never_completed` ×2、`E0599 never_completed_count` ×2，`could not compile aleph-protocol (lib test) due to 7 previous errors`。`cargo check -p alephcore`（protocol 字段落地、核心未动）→ `E0063 missing field parked in initializer of DanglingCallView --> src\gateway\session_snapshot.rs:237:5`。把 `dangling_view` 先桩成 `parked: None` 后跑 `--lib -- session_snapshot subagent_tool::recovery` → `test result: FAILED. 36 passed; 2 failed`，红名单**恰**是两条新测试：`a_parked_dangling_call_carries_the_reasons_wire_word`（`left: None / right: Some("approval")`）、`a_parked_call_lands_in_exactly_one_sentence_per_reason`（`left: ["their outcome is unknown"] / right: ["waiting at a gate"]`）。
- **类型检查**：`cargo check -p alephcore` → `Finished dev … 57.18s`，只有既有 4 条 `never used`。`rustfmt --check` 五个 `.rs`：两处闭包换行已改，其余干净。EOL 逐字节审计：每个文件保持改前约定（`session_snapshot.rs` / `recovery.rs` / 两份 locale / `run.sh` / `drive_r2.mjs` 全 CRLF；`session_thread.rs` / `chat_sidebar.rs` / `commands.rs` 改前就是全 LF，改后仍全 LF）。
- **GREEN**：`cargo test -p aleph-protocol` → `test result: ok. 361 passed`。`--lib -- session_snapshot subagent_tool::recovery session::reduction session::events` → `test result: ok. 108 passed; 0 failed; 18537 filtered out; finished in 4.20s`（那次构建的 11 条 `does not need to be mutable` 全在 `group_chat/executor.rs` / `providers/registry.rs`，既有）。`cargo test -p aleph-tui` → `436 passed`（`parked_calls_are_reported_as_never_completed ok`）。`cargo test -p aleph-panel --lib` → `1273 passed`（`parked_calls_are_counted_as_never_completed_not_unknown ok`，i18n 普查在内）。`just wasm`（Bash 工具）→ `Finished wasm-release 7m 21s`、终稿再跑 `7m 04s`，`✓ panel dist OK`；每次之后 `git checkout -- interfaces/webchat/dist`。`node --check drive_r2.mjs` / `bash -n run.sh` 通过。tui / panel / wasm 在终稿（`never_completed_count` 接上消费者之后）各重跑一次，数字同上。
- **变异**：① `never_completed()` 改成只看 `self.denied`：protocol `360 passed; 1 failed`（恰 `a_parked_dangling_call_reads_as_never_completed_and_an_old_view_reads_as_unknown`）；`aleph-tui -- last_run_face_tests` `11 passed; 1 failed`（恰 `parked_calls_are_reported_as_never_completed`）；`aleph-panel --lib -- last_run_face_tests` `10 passed; 1 failed`（恰 `parked_calls_are_counted_as_never_completed_not_unknown`）。Edit 还原 + `touch`，`grep MUTANT` 0，下一次 protocol 跑印 `Compiling aleph-protocol` → `361 passed`；tui / panel 宿主二进制重编重跑绿，共享 target 不留变异体。② `record_parked` 挪到 `request_approval` 之后（`dispatch.rs`，QA 级）：`run.sh parked`（重编 2m30s）→ `FAIL  a tool_call_parked row landed for the dangling call BEFORE the kill` → `instrument failure: no parked dangle` → `assertions: 0 (floor 11)`，`verdict: rc=1`。`git checkout --` + `touch`，`git hash-object` 与 `HEAD:src/tools/scoped/dispatch.rs` 同为 `b11c2709e…`；带构建重跑（2m26s）→ 11/11 rc=0。
- **真机 QA `parked`**：第一次（带构建，`qa_build` 8m47s）`dangle-parked` 两条 PASS 后 Node 在 `process.exit` 里 abort：`Assertion failed: !(handle->flags & UV_HANDLE_CLOSING), file src\win\async.c, line 76` → shell 读成 `instrument failure`，rc=1——**driver 缺陷，不是产品缺陷**：brief 的 `cmdDangleParked` 在同一进程里开了第二条 WebSocket（每个绿命令都只开一条）。改成 `dangleOn(conn, …)`（从 `cmdDangle` 抽出）、两半共用一条连接，driver 里留了这条实测。第二次（`SKIP_BUILD=1`）2+1+3+5 全 PASS，receipt `resumed: 1`，`=== assertions: 11 (floor 11) ===`、`verdict: rc=0`；模型实际收到的修复文本：`NOT EXECUTED — this \`bash\` call never ran: the server restarted while it was still waiting for operator approval. …`。`knobs` → `10 (floor 10)` rc=0；`denied` → `5 (floor 5)` rc=0。**FLOOR**：首次绿跑印出 11 = 设计值 2+1+3+5，`FLOOR=11` 不变，同一提交里删掉「designed, not yet measured」注释。
- **徽标规则核实（裁定 2）**：`commands.rs::last_run_mark`（`:537` 那条）键在**处置词** `LastRunDisposition` + `dangling().is_some_and(|d| !d.is_empty())` 上，从不读句子——「结果未知」/「从未执行」都不曾是键。停车的调用仍在 `dangling` 里 ⇒ 只剩停车调用的运行仍标 `[interrupted]`，与 T13 之前完全一致；新句子既不点亮也不熄灭徽标。Panel `run_badge` 同一规则。这是 T7 的本意（无回执的调用需要关注），未动。
- **与 brief / 裁定的偏差（点名）**：① 裁定 1–3 照做（改名 `never_completed` / `never_completed_count`、四处面文案、Panel 测试名、四路孪生、`ParkReason::ALL` 走遍）。② **`never_completed_count()` 有了消费者**——Panel 与 TUI 都用 `dangling().zip(never_completed_count())`；brief 的两段面代码各自内联重算一遍，会让 protocol 方法零生产消费者、且同一推导两份（判据 #1）。③ `cmdDangleParked` 单连接（见 QA 第一次）。④ 测试形状：`session_snapshot.rs` 走 `ParkReason::ALL`、用 `json!(view.parked) == to_value(reason)` 把 view 的字串钉到 serde 词上（brief 只写一个 Clarification 字面量）；`recovery.rs` 期望桶用穷举 `match`，不手列。⑤ 注释卫生：`drive_r2.mjs` 头不再数「five stages」；`run.sh` FLOOR 注释改成「(the r2 rows on 2026-09-03)」；python 拒绝提示的清单加 `parked`。⑥ Panel/TUI 的测试级 RED 由变异 ① 证明，没有单独跑改前一次（一次只跑一个 cargo，当时核心构建在跑）。
- **未做 / 顾虑**：全量 `--lib` 未跑（没改核心 struct / enum；`interrupted_note` 新增一个对既有 `ParkReason` 的穷举 `match`，不是改既有 match）；单独的停车句在 Clean/NeverRan 面上以「其中」/「of its」开头而没有前句可指（裁定原文照录，产品层面可再议）；Interrupted 面上全部停车时会印 `0 次结果未知`（brief 的 TUI 测试就这么断言，是「看过了、为零」而非「没丢」的零，但和停车句并排读着别扭）；driver 的 `Conn.close()` 仍不等 socket `close` 事件就 `process.exit`（所有既有命令同形状、一直绿，未扩大改动）；clippy 未跑（会清空 `target/debug` 的二进制，T23 管顺序）；`/tmp/aleph-qa-resume-{uyYBqs,xCyuFl}` 两个 KEEP=1 的产物目录在仓库外，可删。
- **Fix round 1（2722d53e0）**：审查 Approved，0 Critical / 0 Important / 6 Minor；落 M1 M2 M4 M6（代码）+ M3（本记录）；M5 按指示不动。前置 `git status --porcelain` 空、HEAD `63d27eb0a`、无 rustc。
  - **M4（裁定，覆盖此前逐字文案）**：停车句自带主语——en `"{{ parked }} of the previous run's tool calls never completed — …"`，zh `"上一轮有 {{ parked }} 次工具调用未完成 — 服务停止时它们还在等审批 / 等回答，或已被拒绝"`，TUI `parked_line` 用**同一句** zh（孪生之间「它们」的漂移消除）；拼接改成分句边界：Panel `format!("{base}; {parked}")`、TUI `format!("{base}；{parked}")`。既有测试不变仍绿（TUI 断言 `"1 次工具调用未完成"`）。
  - **M1**：Panel `parked_calls_are_counted_as_never_completed_not_unknown` 改断言数字**顺序**（`notice` 的 ASCII 数字串 == `"2521"`），`all_parked` 半段改成与 `td_string!(Locale::default(), narration.last_run_parked, parked = 3_i64)` **精确相等**（对调后会印 `last_run_dangling` 的「3 …no receipt」句，同样只含一个 3——所以比数字不够，比整句才够）。**变异**：把分裂处的元组对调 `(never, d.len() - never)` → `-- last_run_face_tests`：`10 passed; 1 failed`，红**恰** `parked_calls_are_counted_as_never_completed_not_unknown`（`left: "2512" / right: "2521"`）。Edit 还原 + `touch`，`grep MUTANT` 0，之后全量 `-p aleph-panel --lib` 印 `Compiling aleph-panel` → `1273 passed`。
  - **M2**：`events.rs` `ParkReason::ALL` doc「for the two tests that walk them」→「for the tests that walk them」（T13 之后走遍它的测试已是 4 条 + 1 条钉子）。**M6**：`session_thread.rs` `parked` doc 不再手列三个词，改「the core's `ParkReason` serde words, e.g. `"approval"`」。**M3**：上文读者数 3 → 4（`boundary_repair.rs:190`）。
  - **验证**：`cargo test -p aleph-panel --lib -- chat_sidebar` → `19 passed`；全量 `-p aleph-panel --lib` → `1273 passed; 0 failed`；`just wasm` → `Finished wasm-release 7m 12s`、`✓ panel dist OK`，之后 `git checkout -- interfaces/webchat/dist`；`cargo test -p aleph-tui -- last_run` → `12 passed`（`parked_calls_are_reported_as_never_completed ok`）；`cargo test -p aleph-protocol` → `361 passed`；`cargo check -p alephcore` → `Finished 1m 06s`（既有 4 条 warning）。`rustfmt --check` 三个 `.rs` 干净；EOL 逐字节：六个文件各保持原约定。
  - **未做**：M5（Interrupted 臂 `progress` 为 `None` 时的 `_` 回退丢掉停车句）按指示不动；真机 `parked` 阶段没有重跑（本轮只改了面文案与测试，`drive_r2.mjs` 的探针读的是模型侧修复文本与 wire 字段，都没动）。
### T14 — （commit: 851fdd135 · 52a98daf1 · 7079e2314 · 集成测试修补: 886cc55d9 · fix round 1: 0b99fde99）
- **前置**：`git status --porcelain` 空，HEAD `cc0a66da1`。`tasklist` 无 `rustc.exe` / `cargo.exe`。改动前 EOL：14 个目标文件全 CRLF。
- **重数（判据 #6）**：`impl SessionEventStore for` 在 `src/` + `tests/` + `src/harness/tests/` 共 **5 处**（brief 数了 2 个 mock，控制者点名 4 个替身；实际 1 生产 + 4 替身）：`SqliteEventStore`、`store::test_support::CountingStore`（委托）、`actor.rs::ScriptedStore`（mock）、`session_projector.rs::UnreadableRetirement`（mock）、`tests/resume_coordinator_integration.rs::FaultingStore`（委托）——全部随拓宽的 `load_run_markers` 类型改动并编译；`src/harness/tests/` 无。`load_run_markers()` 调用点 **8 处**（去注释）：`session_log.rs`、`query.rs`、`projection_reconciler.rs` ×3（1 生产 2 测试）、`resume_coordinator.rs` ×2、store 测试若干——全部按 `MarkerSlice` 改。`src/bin/…/start/helpers.rs` 只以 `Arc<dyn SessionEventStore>` 引用（无 impl），`--bins` 仍跑。消费者：`reduce_marker_slice` 生产 1（`projection_reconciler::candidates`）+ 测试 2；`fold_strict` 生产 2；`From<&UndecodableRecord>` 8 处；`load_rows` / `retire_record` 各生产 1（doctor）；`events::ignorable` 生产 1（`encode_row`）——T1 的零消费者列有了唯一消费者，私有 `encode_payload` **已删**（`grep` 0）。
- **RED**：`cargo test -p alephcore --lib --no-run`（只写测试）→ `could not compile alephcore (lib test) due to 36 previous errors; 11 warnings emitted`，错误全是预期的未定义符号：`encode_row` / `decode_row`×6 / `reduce_marker_slice`×2 / `MarkerSlice` / `DecodedRow`×7 / `UndecodableRecord`×4（struct）+×4（`LogContradiction` 变体）+×1（`SessionError` 变体）/ `SESSION_EVENT_SCHEMA_VERSION` / `UNKNOWN_VARIANT_PREFIX`×3 / `RepairOutcome` / `retire_record`×2 / `load_rows`，外加两条 `E0308`（marker 扫描断言 `expected Vec<SessionEventRecord>, found Result<_, _>`）。11 条 warning 是既有的 `does not need to be mutable`。
- **类型检查**：`cargo check -p alephcore` 两次（commit-1 范围后 `3m 28s`、三个面后 `49.63s`）→ `Finished`，只有既有 4 条 `never used`。`rustfmt --check` 14 个文件：我的 7 处已改；`events.rs:1049` / `:1172` 两处是既有格式漂移，未动。**EOL 逐字节审计**（`wc -c` − `tr -d '\r' | wc -c` = CR 数，对 `\n` 数）：14 个文件全 CR==LF。⚠️ `projection_reconciler.rs` 在一次 Edit 后变成**全 LF**（CR=0），用 `awk … printf "%s\r\n"` + `touch` 还原，`git diff --stat` 仍只有我的 89/15 行；**仪器注**：msys 下 `grep -c $'\r$'` 对那个全 LF 文件报 `CRLF=1390 LF-only=0`——它撒谎，字节数才是仪器。
- **GREEN**（提交前树，`--lib -- session:: gateway::resume_coordinator gateway::projection_reconciler gateway::session_snapshot diagnostics::checks::session_log orchestrator::harness_bridge`）→ `test result: FAILED. 436 passed; 1 failed; 0 ignored; 0 measured; 18220 filtered out; finished in 4.89s`——那 1 条红是基线的 `acp::session::tests::test_spawn_and_drop_kills_child`（被 `session::` 子串过滤器扫进来）。新测试逐名 `ok`：store 6 条（含 `the_unknown_variant_guard_keys_on_serdes_own_wording`——对**真实**的 unknown-variant 错误与真实的 known-variant 错误各钉一次前缀，判据 #18）、reduction 2 条、events 1、actor `replay_rebuilds_head_seq`、reconciler PF-3 测试、session_snapshot 列表面测试、error.rs 闸测试（夹具 `kind_tag = "connection_opened"`，先断言 `is_transient_harness_message` 为 **真**——按措辞它本会被重派——再断言结构臂给出 `Internal` 并点名 `seq 9` 与 `core/session-log`）、doctor `an_undecodable_row_is_named_and_fix_retires_exactly_that_record`。`no_caller_swallows_a_refused_reduction` 加了第三根针 `reduce_marker_slice(` 仍绿。无 warning 指向改动文件。
- **变异**（§11 账本 7.1，各单独一次构建，`--lib -- session::store::tests`）：① `load_run_markers` 的 `fold_strict(rows)` 换成 `Ok(filter_map(Event ⇒ Some))` → `39 passed; 1 failed`，红**恰** `one_bad_row_refuses_only_its_own_session`，panic 在 `Err(u) if u.seq == 3` 断言（会话 `a` 读成 `Ok`）。② `decode_row` 去掉 `unknown_variant &&` 守卫 → `39 passed; 1 failed`，红**恰** `an_unknown_variant_is_undecodable_unless_the_row_says_ignorable`，panic 在第 10 行（`tool_call_requested` + `ignorable:true` 被读成 Skipped）。两次都 Edit 还原 + `touch`，`cmp` 与变异前副本逐字节相同，`grep MUTANT` 0。
- **全量 `--lib`**（`7079e2314`，分离式）→ `test result: FAILED. 18589 passed; 51 failed; 17 ignored; 0 measured; 0 filtered out; finished in 668.46s`；按名字对 `baseline_failures.txt`（51 条）：**NEW 无，GONE 无**（T0 基线 18493 passed，T11–T14 累计 +96）。四条已知抖动一条都没出现。
- **集成 / bins**：`--features test-helpers --test resume_coordinator_integration -j 1` 在 `7079e2314` **编译不过**（`E0425 cannot find value inner / sid` @ `:2013`）——我插入新测试时把上一条测试的最后一句断言切进了新测试；`886cc55d9` 修（两条测试各自完整，断言零增减）→ `test result: ok. 26 passed; 0 failed`，`an_undecodable_marker_row_refuses_only_its_own_session_at_the_resume_face … ok`（真 store、文件库、第二条连接裸写 seq 99 的 `run_finished{outcome:"from_the_future"}`：`(scanned, resumed, skipped) == (2, 1, 0)`，`refused == [(bad, LogInconsistent(UndecodableRecord{seq:99}))]`，adapter 只派发了邻居）。`--test '*' --no-run -j 1`（`886cc55d9`）→ `Finished … 25m 53s`，139 个可执行文件，EXIT=0。`cargo test -p alephcore --bins` → `94 passed; 0 failed`。
- **各面点名同一矛盾（判据 #9）**：resume（`resume_from_markers` 先 `match slice`，`Err` 在任何臂读到 markers **之前**以 `LogInconsistent(UndecodableRecord)` 拒绝；`handle_interrupted` 的 `load_all_events` 与 `check_unanswered` 的 tail 读各拆一臂）、reconciler（`candidates` 走 `reduce_marker_slice`，`Err` 臂 `kind = tag` + `errored`；`heal_split_epochs` 对 `Err` 切片 `continue`——PF-3）、列表面（`query.rs` 把 `Err` 原样交给 `last_run_from_markers(Result<&[_], &UndecodableRecord>)` ⇒ `log_inconsistent` + tag，**永不** `never_ran`）、attach 面（`chat.rs` ⇒ `last_run_refused`，`inspected: true`）、harness 拒绝（`classify_harness_error` 第一臂结构匹配）、doctor（`load_rows` 逐行，命名 `seq N type T`，`fix=true` 走 `retire_record` 只退那几条）。**客户端**：TUI `commands.rs:1096` 与 Panel `chat_sidebar.rs:375` 都是 `contradictions.join(..)` 泛渲染，`interfaces/` `shared/` 里唯一的 `session-log-` 字面量是 `duplicate-dispatch` 夹具 ⇒ 新 tag **不需要客户端改动**（判据 #17）。diff 里 `unwrap_or(&[])` / `unwrap_or_default()` 0 处。
- **与 brief 的偏差（点名）**：① **提交切分**——brief 的三刀按范围切，commit 1 单独编不过（拓宽的返回类型压着 4 个 gateway 读者），故 4 个读者随替身进 commit 1，commit 2 只剩 trait 不强迫的两张脸（attach + harness）；lib / lib-test 在每个提交都能编（论证：中间树跑过 `cargo check`；commit-1 文件里新加的测试只引用 commit-1 的符号；HEAD 的 chat.rs / error.rs / session_log.rs 对拓宽后的 trait 原样可编——臂是 `Err(e)` 泛型与 `for (id, _)`），**没有**逐提交真构建；集成二进制在 1–3 号提交编不过，`886cc55d9` 才好。② **拒绝句不再承诺「`/undo` to before it」**——`/undo`（`marker_balance::retire_from_and_close_run`）经 `get_events` 读**整份**日志，遇坏行现在就是 `Err(UndecodableRecord)`，那句承诺是描述另一个子系统而它为假（判据 #1 第四形态）；Display 与 harness 文案只指 doctor。③ `resume_from_markers` **先 match 切片**再 `reduce_disposition`，而不是 `match reduce_marker_slice(slice)`：Clean 臂要用 markers，元组匹配得多一条永远到不了的 `(Ok, Err)` 臂；`Err→矛盾` 的推导仍是同一个 `From`。④ `check_unanswered` 的 tail 读也拆 `UndecodableRecord` 臂（brief 只拆了 `handle_interrupted`）。⑤ 新私有 `refuse_log` 合并了 coordinator 里四处手抄的 warn+push（三条不同的日志句并成一条，带 `kind = tag`）。⑥ `raw_insert` 是 **async**（brief 的 `blocking_lock()` 在 tokio 运行时里 panic），落在 `SqliteEventStore::insert_raw_row_for_test`（`#[cfg(test)] pub(crate)`），store / reconciler / doctor 三处测试共用一个夹具。⑦ `decode_row` 末尾 match 绑定 `kind_tag` 而非 `(kind_tag, _)` 通配。⑧ doctor 每会话收集**全部**坏行（`Vec`）、逐条命名逐条退（brief 的 `Option` 对 N 条坏行要跑 N 次 doctor）。⑨ `; N ignorable row(s) skipped` 只在 N > 0 时附加。⑩ 抽出 `session_snapshot::last_run_refused`，attach 面的两种拒绝一个形状。⑪ brief 之外的测试：serde 措辞钉子、reconciler PF-3、列表面、harness 闸、集成 resume 面。⑫ `every_contradiction_tag_belongs_to_this_checks_namespace` 仍是手写清单（+1 变体；`one_of_each` 是 `reduction::tests` 私有的）——判据 #5 债，未改。
- **未做 / 顾虑**：坏行会话上 **`/undo` 会失败**（RPC 报错带 undecodable 句，指向 doctor）——要修得让 `retire_from_and_close_run` 只读幸存前缀并另找不解码的 `retired` 计数源，`marker_balance.rs` / `handlers/mod.rs` 不在 T14 文件表；今天唯一出口是 doctor `fix=true`。`doctor` 工具 `DESCRIPTION` 手列「fix=true applies safe mechanical repairs (missing data dir, stale lock)」没提退坏行——拒绝句已告诉模型该跑什么，未动（会烂的清单，判据 #5），点名。`retire_record` 不动 BM25 镜像行（trait doc 写明）。PF-3 在 heal 侧「跳过」与「读成空」在报告上不可分（都不 heal），reconciler 测试钉的是「无 heal + `errored ≥ 1` + 路由不动」。`candidates` 对坏行会话记一次 `errored`，projector 修复读同一行再失败又记一次——REJECT 类既有形状，未改。clippy 未跑（T23）；`just wasm` / Panel / TUI 未跑（无客户端改动）；QA `undecodable` 阶段属 T18。`session::mod.rs` 未加 re-export。
- **Fix round 1（0b99fde99）**：审查 0 Critical / 2 Important / 7 Minor；落 I1 I2 M1–M5，M6 M7 按指示不动。前置 `git status --porcelain` 空、HEAD `781f31b9e`、无 rustc。
  - **I1（裁定：本轮可动 brief 之外的文件）**：`builtin_tools/doctor.rs` `DESCRIPTION` 剪掉「(missing data dir, stale lock)」（−34 B；ratchet 是 `<=`，无精确串钉子——`grep "fix=true\|Self-diagnose Aleph" src/bin` 只命中一句注释；`catalog_description_bytes_ratchet` 本就在基线红名单里，与本改动无关）；注释加一句 2026-09-13 剪的理由。`cli.rs:84` 的 `aleph-server doctor` 文档同样手列「(recreate data dir, clear a stale lock)」——冷进程确实退不了记录，那句今天不假，**未动**，点名。
  - **I2**：roll-call 先列**全部**带坏行的会话（不封顶），再列封顶 `NAMED_LIMIT` 的普通矛盾会话；hint 改成「retires ONLY the {undecodable_total} undecodable record(s) named above (every one is listed; the cap applies to the other sessions)」，`undecodable_total` 有了消费者；`name_entry` 抽出。测试 `an_undecodable_entry_is_named_past_the_roll_call_cap_and_counted`：11 个 `a-plain-NN`（各一条 `RunFinished` ⇒ `FinishWithoutStart`）按键序排在 `doctor-undecodable` 之前，断言 detail 含该键与 `seq 7`、含「… and 1 more」、hint 含「the 1 undecodable record(s) named above」（先断言键序前提）。
  - **M1**：守卫改键在**外层** tag——`names_unknown_outer_variant(err, kind_tag) = err.starts_with("unknown variant `{kind_tag}`")`，`decode_row` 与钉子测试共用它；`an_unknown_variant_is_undecodable_unless_the_row_says_ignorable` 加第 12 行 `{"type":"run_finished",…,"outcome":"from_the_future",…,"ignorable":true}` ⇒ `Undecodable{kind_tag:"run_finished"}`；钉子测试加内层用例（先断言它也以 `unknown variant` 开头——前缀本身分不开，再断言外层判据不认它）；`UNKNOWN_VARIANT_PREFIX` doc 改说「任何深度的任何枚举」。**残留**：内层未知词恰好等于本行的 `type` 字面（如 `outcome:"run_finished"`）仍会被读成 Skipped——需要 `type` 本身是未知、且行标 ignorable、且内层词与外层同名，实际不可达，记下不修。
  - **M2**：`session::actor::tests::the_actor_boots_on_a_log_with_a_row_it_cannot_read`——decodable 行 @1 + 裸坏行 @2，spawn actor，`EmitBatch` 回 `Ok([3])`（计数器看见了坏行），`GetEvents` 回 `Err(UndecodableRecord{seq:2})`。**变异**：`replay` 体还原成 `load_all_events` 循环 → `--lib -- session::actor`：`6 passed; 1 failed`，红**恰**是它，panic 在 `expect("the actor is alive: its reply channel was not dropped")`。Edit 还原 + `touch`，`cmp` 逐字节相同，`grep MUTANT` 0；之后 `--lib -- session::actor` 印 `Compiling alephcore` → `7 passed`（共享 target 不留变异体）。
  - **M3**：`SqliteEventStore::insert_raw_row_for_test` 改 `#[cfg(any(test, feature = "test-helpers"))] pub`；集成测试删掉第二条连接的裸 INSERT、改用它（不再需要文件库，回到内存库）。**M4**：`reduction::fixtures::{kind_index, KIND_COUNT, one_of_each_kind}`（`#[cfg(test)] pub(crate) mod`，与 `events::fixtures` 同形）；`reduction::tests` 与 doctor 的 `every_contradiction_tag_belongs_to_this_checks_namespace` 都走它，后者先断言 `len() == KIND_COUNT`（12）。**M5**：store 模块 doc 加一句：经 `serde_json::Value`（工作区无 `preserve_order`）行内键按字母序，解码与序无关、无 SQL 按文本读 `payload_json`（`grep "payload_json LIKE\|json_extract"` 只命中别的表的 `event_json`）。
  - **验证**：`cargo check -p alephcore --features test-helpers --tests` → `Finished 6m 18s`，改动文件无 warning；`--lib -- session::store::tests session::actor diagnostics::checks::session_log session::reduction` → `test result: ok. 103 passed; 0 failed`（新/改测试逐名 ok）；`--features test-helpers --test resume_coordinator_integration -j 1` → `26 passed; 0 failed`；`--bins` **未跑**（`src/bin` 下没有 DESCRIPTION 普查）；全量 `--lib` 未跑（无 struct / enum / 穷举 match 改动——`kind_index` 只是搬家）。`rustfmt --check` 六个文件干净；EOL 逐字节六个文件 CR==LF。
### T15 — （commit: d1e105bb9 · fa29f1d01 · fix round 1: 70cbc8ff6）
- **前置**：`git status --porcelain` 空，HEAD `b968e0938`。`tasklist` 无 `rustc.exe` / `cargo.exe`。改动前 EOL：5 个目标文件全 CRLF（`tr -cd '\r' | wc -c` == `wc -l`），每次 Edit 后复核，提交时仍全 CR==LF。
- **重数（判据 #6）**：`ResumeCoordinator::new(` 在集成测试里 **27 处**（brief 说先 grep：实测 27，加本任务新测试 = 28）：**22 处**改成 `Arc::new(..)`（它们的 coordinator 调 `resume_interrupted_runs`，现收 `self: &Arc<Self>`），**4 处**只调 `resume_session`（`&self` 不变）原样不动，**2 处**本来就是 `Arc`（既有 `concurrent_resumes_of_one_session_repair_the_boundary_once` + 新测试）。`resume_interrupted_runs` 在 `src/` 的调用点 **1 处**（boot），`reinject_survivors` 调用点 **1 → 3 处**（early / late / `!auto_scan` 的 `&|_| true`）。信号量 acquire 点 **1 → 3 处**（`launch_resume` 的 marker 任务、activity-window 任务、`resume_session`），`retrigger` **0 处**。`ResumeReport` 字段 **11 个**（10 个计数器 + `refused`）——`absorb` 穷举解构无 `..`，T16 加 `notified` 时是编译错误不是漏加。**PF-9 措辞**：配置 doc / 注释 / commit message 一律写「the knob bounded only `retrigger`, and the scan itself was serial, so contention never occurred」，没有「zero readers」/「severed」。
- **RED**（改动前，只加了 `SlowAdapter` + `registry_with_agents` + 新测试；`Arc::new` 在旧签名下经 auto-deref 照编）：`--features test-helpers --test resume_coordinator_integration -j 1 -- three_interrupted` → `FAILED. 0 passed; 1 failed; … finished in 2.07s`，panic 在 `:2218` 第一条顺序断言 `a and c must finish before the 2 s session`——串行：`a` 出 `t=356940.569`，`b` 出 `+2.002s`，`c` 出 `b` 之后 **1 ms**。这次 RED 同时证明 brief 的 **elapsed 界不能区分形状**（只有一个慢会话时串行 2.07 s、扇出 2.03–2.05 s，都 < 3.5 s），故测试 doc 写明它只是 sanity 上限，真判据是 `max_in_flight == 2` 与顺序断言。
- **GREEN**（`d1e105bb9`）：同一二进制 `-j 1` 全跑 → `test result: ok. 27 passed; 0 failed; … finished in 2.28s`（26 旧 + 1 新）。单跑新测试三次：`finished in 2.03s / 2.04s / 2.05s`。`cargo check -p alephcore`（lib + bins）→ `Finished 3m 10s`，只有既有 4 条 `never used`。`--lib -- config::types::resume gateway::resume_coordinator gateway::handlers::resume` → `52 passed; 0 failed`（`absorb_sums_every_counter_and_appends_every_refusal`、`defaults_are_sane`（断 2）、`every_counter_the_report_carries_reaches_the_wire_with_its_value` 逐名 ok）；lib-test 的 21 条 warning 没有一条指向改动文件。
- **变异**（各单独构建，还原后 `cmp` 逐字节相同 + `touch`，`grep MUTATION|SCRATCH` = 0，还原后重跑印出 `Compiling alephcore` 再绿）：**A** `Semaphore::new(permits)` → `Semaphore::new(1)` → 新测试红在 `:2219` 顺序断言（串行，`c` 又在 `b` 之后 1.2 ms）；把 `max_in_flight` 断言临时挪到最前（scratch，未提交）再跑 → `assertion left == right failed … left: 1 right: 2`——brief 预测的「elapsed 界也红」**没有发生**（2.05 s），原因同上。**B（死锁探针）**：`retrigger` 里放回 `acquire_owned`（扇出的 permit 保留 = 两次 acquire）+ scratch 测试 `max_concurrent: 1`，`--no-run` 构建后直接跑测试二进制 `timeout 40 …exe three_interrupted --test-threads=1` → 打印 `test … ...` 后**不再有任何输出**，`exit=124`（被 timeout 杀）——第一个任务握着唯一的 permit 在 `retrigger` 里等自己，`b`/`c` 排在后面。**C**（commit 2）`if !select(..)` 改成恒假 → `--lib -- gateway::busy_queue::durable`：`6 passed; 1 failed`，红**恰** `a_deselected_survivor_stays_journaled_while_the_rest_are_re_delivered`，panic 在 `reinjected` 断言 `left: 3 right: 2`（被 deselect 的那条也送进引擎了）。这条测试没有自己的 RED（改动前是缺参数 = 编译错误），变异 C 是它第一次被证伪。
- **集成 / bins / no-run**（`fa29f1d01` 前树 = 两个提交的内容）：`--lib -- gateway::busy_queue config::types::resume gateway::resume_coordinator` → `86 passed; 0 failed`（rustfmt 修一处测试断言折行后重跑仍 `86 passed`）；`cargo test -p alephcore --bins` → `94 passed; 0 failed`（`Finished 5m 52s`）；`--features test-helpers --test '*' --no-run -j 1` → `Finished … 29m 00s`，**139 个可执行文件**（与 T14 相同），EXIT=0，只有既有 4 条 lib warning。`rustfmt --check` 五个文件干净（`start/mod.rs` 那次列出的唯一 diff 是既有的 `helpers.rs:552`，未动）。**全量 `--lib` 未跑**：没有既有 struct / enum / 穷举 match 改动（`ResumeLaunch` 是新类型，`ResumeReport` 字段未变），两处签名改动由 `--no-run` + `--bins` + `cargo check` 覆盖。
- **与 brief 的偏差（点名）**：① `ResumeLaunch` **没有 `me: Arc<ResumeCoordinator>` 字段**——T15 里零消费者（只有 T16 的 `adjudicate_orphaned_tasks` 会读它），会是 `dead_code` warning；转发注释改成「T16 adds a `me` field and inserts … HERE」。② 加了私有 `walked: bool`：`settle` 只在 marker 扫描真跑过时打 `resume scan complete`——否则 `load_run_markers` 失败会先 warn「scan failed」再 info「scan complete scanned=0」，把 `Err` 读成空结果（判据 #8）。③ 测试里 brief 的 `at = |k| done.iter().find(|(s, _)| s.contains(k))` **对 `"a"` 恒命中第一条**（`agent:b:main` / `agent:c:main` 都含 `a`），改成整键相等 `*s == SessionKey::main(k).to_key_string()`；`SlowAdapter::new` 收 `&SessionKey` 而不是子串标记（PF-17 的精神）。④ 多一条断言 `entered_at("c") < exited_at("b")`（`c` 必须在 `b` 还在跑时**进入**引擎——顺序断言的另一面），`SlowAdapter` 因此同时记 entries 与 exits。⑤ `registry_with_agents(&[..])` 加在 `registry_with_agent` 旁边、单数版委托给它（一个 helper）。⑥ `resume_session` 在 `load_run_markers` 之后、**两条臂之前**取 permit（brief 只写「before `resume_from_markers`」）：marker-less 臂的 `check_unanswered` 也会 retrigger，permit 只盖一条臂等于给 on-demand 留了一条不受限的 retrigger 路。⑦ `durable.rs` 的 `select` 测试要等 spawned 的投递写完 tombstone（≤ 5 s 轮询 `survivors().len()`），并额外用 `RecordingAdapter` 断言恰是 `a`/`c` 到了引擎。⑧ `ResumeLaunch` 没加进 `gateway/mod.rs` 的 re-export（boot 不点名这个类型，只调 `launch_resume()` / `.settle()`）。⑨ `reinject_survivors` doc 里「the two never overlap」那句改写：幸存者不是被恢复的那次 run 这一半仍真，但**同一会话**的幸存者会在准入门撞上，这正是拆两趟的理由。
- **未做 / 顾虑**：**`pending` 幸存者要等最慢的那条恢复 run 跑完**——`ExecutionEngine::execute` 是整个 run（`resume_from_markers` 持 permit 到 `execute` 返回），`settle` 等所有任务，所以会话 X 的排队消息要等会话 Y 的恢复 run 结束才重注入；按会话逐个放行需要每个任务完成时的信号（不在 T15 范围，点名给控制者）。任务 panic 的候选**哪个计数器都不进**（只 warn；`JoinError` 里没有会话名，报告也没有「扫描本身失败」的臂）——与 brief 一致，记下。`unanswered_eligible` 提前 `continue` 让 Subagent / Ephemeral / scheduler-owned 的窗口会话不再 claim slot，因此一个正被 on-demand resume 占着的这类会话不再计 `busy`（brief 要求的过滤，行为差异良性）。clippy 未跑（T23）；QA `parallel` 阶段属 T18（`QA_MAX_CONCURRENT`）；Panel / TUI 无改动未跑。
- **Fix round 1（70cbc8ff6）**：审查 0 Critical / 1 Important / 5 Minor，全部落地。前置 `git status --porcelain` 空、HEAD `120981b20`、无 rustc；四个文件改动前后 CR==LF。
  - **I1（PF-16 的孪生行）**：`walked` 只拦住了 `settle` 自己那条「resume scan complete」，boot 的「ResumeCoordinator boot scan finished scanned=0」在 `load_run_markers` 失败 / `enabled = false` 之后照发。现 `ResumeLaunch::walked` 是 `pub`，boot 在 `tokio::spawn(launch.settle())` **之前**读进局部，`Ok(report) if walked` 才 `info!`（带 T16 的 `notified` 转发注释），否则 `debug!`「did not walk the marker log; no report to print」；`late` 重注入两种情况都照跑。
  - **M1** `#[must_use = "dropping a ResumeLaunch aborts every resume it launched; call settle()"]`。**M2** 代码改成与 doc 同序（先 `has_own_scheduler` 再 `select`——scheduler-owned 幸存者无论哪趟都报同一句 debug），per-survivor 的编号清单改成散文。**M4** 两处「deadlocks at `max_concurrent = 1`」改成「as soon as `max_concurrent` candidates each hold a permit and wait for a second」。**M5** `ResumeLaunch.sessions: HashMap<tokio::task::Id, SessionId>`，`JoinSet::spawn` 返回的 `AbortHandle::id()` 配 `JoinError::id()`（本机 tokio 1.52 两者都有），warn 改成「candidate task did not complete; its verdict is unknown」带 `session = ?`（`JoinError` 也可能是 cancel，不再只说 panicked）。
  - **M3**：`SlowAdapter::new(slow, slow_delay, floor)`，每个会话至少睡 `floor`（300 ms），`b` 睡 2 s——不论哪两个候选先拿到 permit，300 ms 后两者都还在 `execute` 里，`max_in_flight == 2` 由构造保证而不靠 `ORDER BY session_id`。**实测**（`--test resume_coordinator_integration -j 1 -- three_interrupted` 单跑三次）：`finished in 2.07s / 2.06s / 2.09s`；全二进制 `27 passed; 0 failed; finished in 2.46s`。**变异 A 重跑**（`Semaphore::new(1)`）：`FAILED … finished in 2.70s`，panic 在 `:2250` = `max_in_flight` 断言——这次串行下 permit 顺序跟着调度器走（`c` 先于 `b`），两条顺序断言**都过了**，只有 `max_in_flight` 红：正是 M3 说的那件事（顺序断言是较弱的一对），数字与观察写进了测试 doc。Edit 还原 + `touch`，`cmp` 逐字节相同，`grep MUTATION` 0，重跑印出 `Compiling alephcore` 后绿。
  - **验证**：`cargo check -p alephcore`（lib + bins）`Finished 1m 00s` 只有既有 4 条 warning；`--lib -- gateway::busy_queue gateway::resume_coordinator` → `83 passed; 0 failed`（改动文件无 warning）；`cargo test -p alephcore --bins` → `94 passed; 0 failed`；`cargo check -p alephcore --tests` → `Finished 5m 58s`，0 error，改动文件无 warning。`rustfmt --check` 四个文件干净（`start/mod.rs` 那次唯一的 diff 仍是既有 `helpers.rs:552`）。全量 `--lib` / `--test '*' --no-run` 未重跑（无签名 / struct 字段集变化对外部可见：`ResumeLaunch` 只多了 `pub walked` 与私有 `sessions`，唯一构造点在本文件）。
### T16 — （commit: ）
### T17 — （commit: ）
### T5a — （commit: ）
### T5b — （commit: ）
### T10 — （commit: ）
### T18 — （commit: ）
### T19 — （commit: ）
### T20 — （commit: ）
### T21 — （commit: ）
### T22 — （commit: ）
### T23 — 全量验证集与变异账本（commit: ）
### T24 — 文档（commit: ）

---

## 附录：各组起草时留下的「待核实项」（Open verifications，原样保留——执行到该任务时先核，核的结果写进验证记录）
### A1（T1–T5b） — 起草时核实的事实

Worktree `D:\Workspace\Aleph\.claude\worktrees\persistence-r3`, base `5e85060b8`. First command of every task: `git status --porcelain`.
Every "Run" step is `cargo test -p alephcore --lib -- <filter>`; the first compile after a change is ~16 min — run it detached (`Start-Process pwsh -ArgumentList '-c','cargo test -p alephcore --lib --no-run *> D:\Workspace\Aleph\target\a1-build.log'`) and poll the log; subsequent runs are incremental.

**Facts settled by reading:**
- Event store = ONE `Connection` under `Arc<tokio::sync::Mutex<_>>` (`store.rs:280-296`), opened by `open_sqlite_safe` at `helpers.rs:336-372` (the only production `SqliteEventStore::new`, :363); the guard is held for a method's whole body ⇒ the per-txn `PRAGMA synchronous` toggle is private to the batch. `transaction_with_behavior(Immediate)` precedent: `teams/messages/store.rs:285`.
- `sessions` (epoch) = `SessionManager.conn`, a separate `std::sync::Mutex<Connection>` (`ops/identity.rs:112`; `start/mod.rs:426-452`). **Different connections ⇒ epoch cannot enter the txn ⇒ T4 = batch first, epoch after, boot heal arm.**
- `SessionService` implementors: 1 prod (`in_process.rs:286`) + 7 mocks, five under `src/harness/tests/` (R10) ⇒ `emit_batch` is a provided method with a fail-closed `Err` default. `SessionEventStore` doubles: `actor.rs:464`, `session_projector.rs:1231` (switch to `append_batch`).
- Exhaustive `match`es over `SessionEvent`: `store.rs:655,683`, `fork.rs:216`; T1 adds `durability_of` and the `ResumeAttempted` variant (see corrections).
- `/undo` closer is `RunOutcome::Cancelled` (`marker_balance.rs:64-70`), not `Abandoned`. Its twin `session.truncate` (`modify.rs:752`) migrates too (criterion #16).
- `AssistantMessage.usage` is a `TokenBreakdown` (no model/price) ⇒ the fold gives tokens, not `cost_usd`. `AssistantRunMeta` census (comments stripped): producer `execute.rs:984`; spend reader `session_projector.rs:769-846`; pass-through `store.rs:668,691`, `fork.rs:236`; test ctors `session_projector.rs:1179`, `reduction.rs:764`, `fork/tests.rs:244` ⇒ ≤6, split T5a/T5b by test cycle only. `SessionEvent` has no `deny_unknown_fields` ⇒ old rows with the deleted fields still decode.

---

### A1（T1–T5b） — 待核实项
1. **Group C's envelope writer consumes `ignorable`** — drafted with `encode_payload` as the seam. If C does not wire it, `ignorable` is a zero-consumer pub fn and must be CUT. Settle at cross-check: grep C's draft for `ignorable(`.
2. **A2 does not re-add `ResumeAttempted`** (T1 adds it; A2 owns the `load_run_markers` `IN (...)` list, reducer arms, producer). Settle at cross-check: grep A2's draft for `ResumeAttempted {` under a "Create/Modify events.rs" line.
3. **`tokio::sync::MutexGuard<Connection>` derefs to `&mut Connection` for `transaction_with_behavior`** — precedent at `teams/messages/store.rs:285` is on a `std` guard. Settle: `cargo check -p alephcore` after T1 Step 3 (a deref error is E0596 and trivially fixed with `&mut *conn`).
4. **`reconciler()` test helper arity + `MessageProjector::new` billing on the live drain** — T4 adds a 5th ctor arg; T5a's live billing needs `resolve_events` to find the global store (installed at `start/mod.rs:463`). Settle: `cargo test -p alephcore --lib -- gateway::projection_reconciler gateway::session_projector`.
5. **Errored/cancelled runs never emit `AssistantRunMeta`** (`execute.rs:955` emits only in the `Ok` arm) — T5b bills them at boot; the live path stays unbilled until then. Not changed here; a #11 sibling for the orchestrator.
6. **`rusqlite` `Transaction` drop = rollback** (`DropBehavior::Rollback` default) — drafted as true. Settle: T1's `a_batch_whose_third_row_collides_leaves_nothing_behind` is exactly this assertion.

### A2（T6–T10） — 起草时核实的事实

Assumes group A1 has landed: `SessionEvent::ResumeAttempted { target: EventSeq, attempt: u32 }`, `ErrorKind::HookStop`, `Durability`/`durability_of` (Barrier for `ResumeAttempted`), `SessionEventStore::append` as the default single-event wrapper of `append_batch`, `SessionService::emit_batch(&self, id, Vec<SessionEvent>, Option<Retire>) -> Result<Vec<EventSeq>, SessionError>`. Every path below is relative to `D:\Workspace\Aleph\.claude\worktrees\persistence-r3`. Unit tests live in each file's existing `#[cfg(test)] mod tests` unless stated. First command of every task: `git status --porcelain`.

---

### A2（T6–T10） — 待核实项
1. **Does the hybrid recall embed the query on every turn when the memory store is empty?** Drafted assuming yes (`prompt_build.rs:376-384` → `build_memory_user_message` → query embedding). Settles it: `KEEP=1 QA_EMBED_STALL_MS=10000 bash qa/resume_boundary/run.sh unanswered` and read `mock.log` for `embeddings request` between the `user_message` row and the kill; if it never arrives, the fallback lever is `routing_recall` (`runner_impl.rs:516-523`, also embedding-backed) and the stage's `INSTRUMENT FAILURE` says so.
2. **Does `[memory] enabled = true` with a `custom` embedding provider boot without a vault/other service?** Command: `node patch_r2.mjs <cfg> 18831 18832 true ask embed-stall && "$BIN" --port 18831 start` and check `/health`.
3. **Does `get_or_create` stamp `last_active_at` on the row before the seed for every producer** (the activity window that finds marker-less unanswered sessions)? Read `src/gateway/execution_engine/execute.rs` for the `ensure_session_under_request_scope` call site relative to `run_agent_loop`; `grep -n "ensure_session" src/gateway/execution_engine/execute.rs`.
4. **`RunRequest` test constructor** — T9's test assumes a `RunRequest::new_for_test` or builds the literal as `resume_coordinator.rs:1234-1248`; `grep -rn "fn new_for_test\|impl RunRequest" src/gateway/execution_engine/mod.rs`.
5. **Simulated mode**: does a `ResumeCoordinator` exist when only `SimpleExecutionEngine` runs (its `UserMessage`+`AssistantMessage` with no markers can read `Unanswered` mid-crash)? `grep -n "execution_adapter" src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`; if yes, the simple engine must answer `is_resume()` (it does not today: `grep -n resume src/gateway/execution_engine/simple.rs` is empty).
6. **T14 interplay**: the ratchet's hook-held resumed runs leave `tasks` rows with no events; group C's 8.2(b) arm must skip `is_resume()` requests or the `ratchet` stage will grow a "please resend" notice per boot — the stage's assertions count only `resume_attempted` / abandoned closers so they stay green either way, but the notice would be a wrong sentence to a user.
7. **UserMessage census counts** in T7 are derived from reads at `5e85060b8` (+T8); re-derive on the commit — the test is the instrument.
8. **Locale parity guard**: assumed a test pins `en.json` ⇄ `zh.json` key sets (`grep -rn "zh.json" interfaces/webchat/src/i18n`); if none, add both keys anyway.

### B（T11–T13） — 起草时核实的事实

Findings (file:line at `5e85060b8`):

- Approval park = `requester.request_approval(action).await`, `src/tools/scoped/dispatch.rs:962`, inside `confirm_with_memory` (:769; callers: operator gate :460, confirmation gate :573, hook `Ask` :1287). The rule that raised the card is on the action (`action.rule_id`; `GateRule::HookRequested.id() == "hook_requested"`, `gate_chain.rs:198`) ⇒ `ParkReason` derived there. `PreHook` = **"parked on a card a `BeforeToolCall` hook raised"**; a crash while the hook *script* runs (:1236) stays OUTCOME UNKNOWN — no fact is written when a hook passes, so a Parked before the script would be a false "never ran" for every call that then crashed mid-body (判据 #17). Deliberate limit.
- Clarification park = `tokio::time::timeout(.., parked.receiver()).await`, `src/clarification/ask.rs:381`; the fact goes after `delivered` (:363).
- Un-park is an existing fact: `ToolCallApproved`/`Denied` are written by the same gate after the park ends (:1088/:1005); the reducer clears `parked` on either.
- The silent skip (:1173-1180) is reachable only with the global session service installed (one caller: `start/mod.rs:476`). Production originators of `execute_with_cancel` on the scoped service: `src/harness/agent/act.rs:636`/`:956`, both inside `with_call_identity(Some(..))` (:631/:955); `tools.invoke` bypasses `ScopedToolService` (`handlers/tools_invoke.rs:42-45`); sole production constructor `tool_service_builder.rs:169`. The claim is TRUE today; the census pins it, so the `debug_assert!` is safe.
- QA: every r2 dangle already parks at the `ask` gate (`run.sh` `BASH_POLICY="ask"`); after T11 they get the fourth arm ⇒ `knobs`'s `"OUTCOME UNKNOWN"` probe (`drive_r2.mjs:805`) must accept either marker, and the kill must wait for the `tool_call_parked` row.
- Exhaustive `SessionEvent` matches needing an arm: `src/session/store.rs:657`, `:695`, `src/agents/subagent_spawner/fork.rs:238` (A1/A2 touch the same three — append).

---

### B（T11–T13） — 待核实项
1. **`code_text` and the multi-line `with_call_identity(\n Some(`** at `act.rs:631` — the census counts `with_call_identity(`, not `…(Some(`, for this reason; assumes `crate::utils::source_scan::code_text` leaves that token intact. Settles: `cargo test -p alephcore --lib -- tools::scoped::tests::every_production_dispatch` (must see `calls == scoped == 2`).
2. **Production `.execute(` originators** — grep (comments stripped) found none outside the harness, but the census pins only `execute_with_cancel`. Settles: `rg -n "\.execute\(\s*(&?name|name|&?tool)" src --glob '!**/tests.rs' --glob '!**/tests/**'` — every hit must be a `LoopTool`/registry `execute`, not a `ToolService` one.
3. **`InProcessActorSessionService`: `attach` then `emit_event` then `get_events` sees the row** — assumed; settles: `cargo test -p alephcore --features test-helpers --test parked_gate_integration -j 1`.
4. **`asked.notified()` vs. the park write** — the emit awaits the actor's append before `request_approval` is entered, so the row is durable when `notify_one` fires; a flaky run means the write point is wrong, not the test. Same command as (3).
5. **Leptos-i18n default locale is `en`** — the Panel test asserts digits only, like its neighbours; settles: `cargo test -p aleph-panel --lib -- chat_sidebar::tests::parked_calls`.
6. **`exec.approvals.pending` item shape `{ record: { tool_call_id } }`** (`exec/manager.rs:263-266`, `:94`) reaches the wire unrenamed — settles: the `dangle-parked` phase's second check on a real run.
7. **A resumed `/btw` run's `origin_route`** — assumed `None` for a side session, so the fan-out skip is belt-and-braces; settles: `cargo test -p alephcore --lib -- btw_wire_tests resume_coordinator` + `rg -n "origin_route" src/gateway/btw`.
8. **Round-1 Python `crash` deletion / `attribute` port (spec §11)** — not in this group; assumed to land with A2's `unanswered` stage. `crash` uses `bash=allow`, so T11 does not change its text.

### C（T14–T18） — 起草时核实的事实

Base `5e85060b8`, worktree `persistence-r3`. Every agent's first command: `git status --porcelain`. Unit test command throughout: `cargo test -p alephcore --lib -- <path>` (detached pwsh + poll for a cold build). Consumes from A1: `durability_of`, `ignorable`, `append_batch`; from A2: `RunDisposition::{Interrupted{attempts}, Unanswered{..}}`.

**Carrier decision (T14).** The reducer is a pure function over decoded events and cannot observe a row that did not decode, so the undecodable fact is raised at the **read boundary** and carried in the reducer's closed set: (1) marker face — `load_run_markers` widens to `Vec<(SessionId, MarkerSlice)>`, `MarkerSlice = Result<Vec<SessionEventRecord>, UndecodableRecord>`, reduced by one new fn `reduce_marker_slice`; (2) full-log face — `load_events_range`/`load_all_events` return `Err(SessionError::UndecodableRecord(rec))`, which the three consumers that *name* it (`handle_interrupted`, doctor, bridge) match structurally. A synthetic marker slot was rejected: a non-event in `SessionEvent` needs arms in `extract_turn_id`, `event_type_tag`, `render_event_text`, `durability_of`, `ignorable`, `project_row` and the R10-frozen prompt builder. Readers of `load_run_markers` (counted, comments stripped): `resume_coordinator.rs:686,886`, `projection_reconciler.rs:146`, `diagnostics/checks/session_log.rs:121` (ids only — unchanged), `handlers/session/db_handlers/query.rs:67`; impls: `store.rs:537`, mocks `session_projector.rs:1272`, `actor.rs:507`. Readers of `load_all_events`/`load_events_range`: 17 files, all already propagate `Err` — only the three that must *name* the variant change.

---

### C（T14–T18） — 待核实项
1. `serde_json` renders an unknown internally-tagged variant as a message starting `unknown variant` — settle: `cargo test -p alephcore --lib -- store::tests::an_unknown_variant_is_undecodable` (the test pins it; if red, match on `serde_json::Error::classify() == Category::Data` plus a `known_tags` check derived from `event_type_tag`).
- 2. `chat.send` with an unseen explicit `session_key` (`agent:main:qa-b:s1`) mints that Main session — settle: `KEEP=1 bash qa/resume_boundary/run.sh parallel`, then `SELECT DISTINCT session_id FROM session_events` in `$QA_ROOT/home/.aleph/data/sessions.db` must show three ids.
3. The `subagent` tool is on the QA agent's tool surface and a foreground `run` child whose provider hangs keeps the parent's dispatch in flight — settle: `KEEP=1 … attribute` and check `drive assert-dangling 2` names `subagent` call ids (`danglingIds` output).
4. A2's `Unanswered` verdict is derivable from `reduce_marker_slice` (i.e., A2 widens the marker query to include `user_message`); if it is only derivable from the full log, `ResumeLaunch::pending` under-counts and the late reinjection pass must fall back to `pending = every session with any marker` — settle by reading A2's final `load_run_markers` SQL.
5. `MessageProjector::request_repair` paints a SystemMessage appended directly to the store while the session's actor is not running — settle: after T16's integration test, `store.get_history(&sid)` on the projection store shows the notice row (add that assertion if it does; if it does not, `system_note` must go through `global_session_service().emit_event` instead).
6. The boot scan actually fires in the QA fixture with no channels configured (`wait_for_channel_config_snapshot(30s)` times out and proceeds) — the deleted Python `crash` stage relied on it on the Mac; settle on the first `parallel` run (`ResumeCoordinator boot scan finished` in `server.log` within 40 s).

### D（T19–T22） — 起草时核实的事实

Anchors (worktree `db5e5ddd9`): `process_journal.rs` — `JobRecord` :288, `JobPhase` :224, `settled_label` :256, `Verdict` :270, `init_and_reconcile` :415 (retention sweep :440 = `RECORD_RETENTION_MS` 7 days :121 — **a cleanup rule exists, no `MAX_TOMBSTONES`**), `init_and_announce` :525, **`record_spawn(id, command, owner)` :664** (the prompt's `record_start`), `record_settled` :819, `lookup` :887, `list_for_scope` :899, `write_state` :991, `read_all` :1167 (reads every `<dir>/*/state.json`); root `<ALEPH_HOME>/data/background_processes/` (`paths.rs:373`, enabled at `start/mod.rs:1663`). The `tokio::process::Child` exists only inside `sandbox/platforms/common.rs:383 run_child_with_drain`, which already reads the `LIVE_TAIL` task-local (:430); `ProcessRegistry::attach_live` :371 is the tool-side end; `register_running` calls `record_spawn` (:345) before `spawn_background` releases the id to the detached task (`bash_exec.rs:562`) — the child cannot exist before the intent row. `utils/process_alive.rs`: `process_start_time(i32) -> Option<u64>` (seconds), `with_process_specifics` (`pub(crate)`), `default_refresh_kind` (private → `pub(crate)`). PTY: `PtyManager::spawn` :452 mints the uuid; `session.rs:213` stores `child.process_id()` as private `shell_pid` (accessor CUT :295 for zero callers); `settle_exit` :691; `close` :560; ownership `owner_admits` :194 (RPC) / `terminal_admits` (`terminal.rs:313`). Not-found faces: bash `recovered_or_unknown` :844 → `recovered_row` :858 / `advisory` :967 (actions are `poll|wait|kill|list` — **no `status`**); RPC `require_owned` (`handlers/pty.rs:425`); tool `owned_session_id` (`terminal.rs:397`). `tools.invoke` denies `bash` and has no session scope ⇒ QA drives a real turn through the mock. Windows sandbox: with `use_restricted_token || use_app_container` the child is the `sandbox-init-windows` launcher (`windows/driver.rs:270`), and `use_job_object` (default true) sets `KILL_ON_JOB_CLOSE` (`job.rs:60`) — a `kill -9` of the server kills the child.

---

### D（T19–T22） — 待核实项
1. **sysinfo `start_time()` is non-zero on Windows for a single-pid refresh with `default_refresh_kind()`** (drafted: yes — `process_matches_falls_back_and_detects_reuse` already relies on it) — `cargo test -p alephcore --lib -- utils::process_alive` and T19's `probe_liveness_answers_all_three_ways`.
2. **`sysinfo::ProcessStatus::{Zombie, Dead}` compile on Windows in 0.39.3** (drafted: yes — `diagnostics/checks/duplicate_instance.rs:47` matches both) — `cargo check -p alephcore` after T19.
3. **A background `sleep 300` actually sleeps on this host with the three `[sandbox.windows]` flags off** (drafted: yes — plain `CreateProcessW` of pwsh; the 2026-09-03 "dies in 240 ms" was git-bash under the restricted token) — the stage's 2-second pre-flight; exit 78 says otherwise.
4. **Node `process.kill(pid, 0)` / `process.kill(pid, "SIGKILL")` act on a non-child Windows pid** (drafted: yes — TerminateProcess) — `node -e 'process.kill(<pid of a Start-Sleep pwsh>,0)'`.
5. **`kill -9` from Git Bash on `"$BIN" start` leaves a non-job-object child alive** (drafted: yes — TerminateProcess does not touch children; `kill_on_drop` never runs) — the stage's `bg-alive yes` check.
6. **`serial_test::parallel(pty_global_manager)` is needed on the new `handlers/pty.rs` test** (drafted: no — it only reads `manager().owner_of`; the journal singleton is gated by `test_gate`) — the file's tests at `:1230` carry the attribute; add it if the test races.

