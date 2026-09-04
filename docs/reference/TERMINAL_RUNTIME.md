# TERMINAL_RUNTIME.md — 内嵌终端与运行时 agent 面板

> **Tier 2**。总入口 [FEATURE_LOCATOR.md](FEATURE_LOCATOR.md)：落点索引在 **§6.11**（终端面：PTY / VT / Panel tab / TUI）
> 与 **§6.12**（运行时面：前台探测 / 识别 / `RuntimeAgents` / 工具面 / 真机装置）。
> spec 母本：[第 1 期](../superpowers/specs/2026-09-01-herdr-runtime-phase1-design.md) ·
> [第 2 期](../superpowers/specs/2026-09-04-terminal-round2-design.md) ·
> [Panel 内嵌终端](../superpowers/specs/2026-08-29-panel-embedded-terminal-design.md) ·
> [0-A VT 缺口清单](../superpowers/plans/2026-09-03-0a-vt-capability-gaps.md)。
> 授权与信任模型全文在 [SECURITY.md](SECURITY.md#embedded-terminal)，工具条目在 [TOOL_SYSTEM.md](TOOL_SYSTEM.md)。
>
> ⚠️ **本文不复制代码拥有的事实**。每个常量、每个字段、每条顺序都带着**它的所有者**（符号 + 文件）出现，
> 读到与代码不一致时**代码是权威**（判据 §1：同一事实的两份表述，只改一份就是静默说谎）。
> 参照实现 herdr 0.8.2（Apache-2.0，`/Volumes/TBU4/Github/herdr`）的行号是**对别人文件的断言**，最会腐烂，
> 引用前重读；本文只在 §6 集中引用，且逐条标注**哪个数字是 herdr 的、哪个是 Aleph 自己的**。

---

## 0. 这个子系统在解决什么

Aleph 的内嵌终端是**给人用的**：Panel 里开一个 PTY，人在里面交互式地启动别人的 agent（`claude`、`codex`、
`gemini` …）。运行时 agent 面板回答的是**「哪个终端里的 agent 现在卡住了在等我」**——它不发号施令，只观察。

**第 1 期（2026-09-01）交付的面板在生产上从未识别过任何一个 agent。** 识别的输入是
`PtySession::shell`——**spawn 时**记下的 `$SHELL` 标签；而人是先开 shell、再敲 `claude` 的，所以那个标签
永远是 `zsh`，`identify_agent("zsh")` 永远答 `None`，检测引擎在读第一条规则之前就早退成 `Unknown`。
`Agent::SCREEN_MANIFEST_AGENTS` 那一整表 manifest、规则引擎、idle-hold、OSC 9;4 全部正确且全部不可达；每一条测试都把 agent 的名字**自己传进去**，
所以没有一条能看见它（判据 §2：问的不是「它对不对」，是「它在什么情况下会变红」）。

第 2 期（2026-09-04）接的就是这条线：**识别源改成前台进程事实**（spec R2-1），并由
`qa/terminal/run.sh identify` 在一台真机的发货二进制上证明它。

---

## 1. 架构：两层，一条线把它们连起来

```
                         ┌──────────────── 终端面（§6.11）────────────────┐
   PTY 子进程 ──字节──►  src/gateway/pty/session.rs   spawn_reader
                         src/gateway/pty/screen/      vte::Perform → Grid
                         src/gateway/pty/manager.rs   start_flush_loop (16 ms，唯一时钟)
                         └───────────────────────────────────────────────┘
                                        │                      │
                       (a) PtyScreenPatch│          (b) 前台探测│(c) 采样
                                        ▼                      ▼
                    pty.screen / pty.exit          ┌─── 运行时面（§6.12）───┐
                    Panel views/terminal/          │ pty/foreground.rs      │
                    （tab 条 · 粘贴 · 光标）        │ crates/agent-detect/   │
                                                   │ gateway/runtime/       │
                                                   └────────────────────────┘
                                                              │
                                       runtime.agents.changed │ runtime.agents.list
                                                              ▼
                                     Panel 侧栏 agent 面板 · TUI `/agents` · terminal 工具
```

**边界**（也是两节 FL 的分界）：**终端面**拥有字节、网格与像素——`src/gateway/pty/{session,manager,screen}.rs`、
`shared/protocol/src/pty.rs`、`interfaces/webchat/src/platform/wide/views/terminal/`。
**运行时面**拥有「这个终端里跑的是什么、它现在什么状态」——`src/gateway/pty/foreground.rs`、
`crates/agent-detect/`、`src/gateway/runtime/`、`shared/protocol/src/runtime.rs`、
`src/builtin_tools/terminal.rs`、两端的 agent 面板与 `shared/ui_logic/src/state/agent_panel.rs`。

`src/gateway/pty/manager.rs::flush_session` 是**唯一同时站在两边**的函数：它取帧、驱动探测、组装
`SampleInput`、把两个结果一起交回 `start_flush_loop`。

---

## 2. 数据流（一次 16 ms tick）

`src/gateway/pty/manager.rs::start_flush_loop` 是**唯一时钟**——探测、idle-hold 释放、quiet 标记
共用同一个 `now`（unix 毫秒，每 tick 取一次），所以「同一遍里被碰到的每一行带的是同一个瞬间」，
采样器自己**从不读时钟**（判据 §12）。

1. `PtySession::feed_and_take_frame()` — 把 PTY 读线程攒下的字节喂进 `screen/`，取走一个 `PtyScreenPatch`。
2. `PtySession::maybe_probe_foreground(now, frame.is_some(), agents.agent_known(id))` — 三条闸见 §3.1。
   返回值是**「前台程序变了吗」**。
3. **没有帧且程序没变 ⇒ 本 tick 到此为止。** 程序变了但屏幕没动也要继续——这正是第一次真机跑
   `a_real_agent_started_after_spawn_is_identified` 抓到的：探测已经看见 `/bin/sh …/claude`，
   而表里还写着 `program: "sh", agent: None`，陈旧 521 ms。
4. `session.with_screen(|screen| …)` — **一次**取屏幕锁，在闭包里同时算 cwd 与采样
   （第二次 `with_screen` 只为问一句 cwd，既是 PTY 读线程热路径上的又一次取锁，也可能答的是另一块屏幕）。
5. **live cwd 的权威顺序，在这一个地方派生**（判据 §12）：
   `Screen::cwd()`（OSC 7，空串算缺席）› 前台进程自己的 `cwd`（探测结果）› `session.cwd`（spawn 目录，永不变）。
   守卫 `gateway::runtime::tests::cwd_prefers_osc7_then_foreground_then_spawn`。
6. `RuntimeAgents::sample(SampleInput { … })` — 见 §3.2，返回 `changed: bool`。
7. `start_flush_loop` 按两个结果分别发 `pty.screen` 帧与 `runtime.agents.changed`（空载荷）。

**空载荷是有意的**：`runtime.agents.changed` 只说「表动了」，客户端拿它去调 `runtime.agents.list`。
把行塞进帧就要在每条连接上重跑一遍可见性投影。

---

## 3. 仲裁与常量

> 每个数字都写着它的所有者。**改数字改代码那一处**，本文这一行会跟着腐烂——发现不一致时以代码为准。

### 3.1 前台探测（`src/gateway/pty/foreground.rs`）

| 常量 | 值 | 所有者 | 归属 |
|---|---|---|---|
| `PROBE_MIN_INTERVAL_MS` | 500 | `src/gateway/pty/foreground.rs` | **herdr 的数字**（`PROCESS_ACQUISITION_FAST_RECHECK`）。**逐行出处只在那个文件的许可头里写一次**——上游行号是对别人文件的断言，本文再抄一份就是同一事实的第二份表述（判据 §1），而两份里只有一份记着读的是哪个版本 |
| `PROBE_RECHECK_MS` | 3 000 | 同上 | **Aleph 自己的裁定**。herdr 干这活的同类是 `PROCESS_RECHECK_IDENTIFIED` = 5 s。⚠️ herdr 里**确实有** 3 s（`AGENT_STARTUP_GRACE_WINDOW`），只是另一件事——细节同样只在那个许可头里 |
| `PROBE_MISSES_TO_FORGET` | 6 | 同上 | **herdr 的数字**（`AGENT_MISS_CONFIRMATION_ATTEMPTS`） |

**三条闸，全部住在 `foreground::probe_due`**（纯函数；`PtySession::maybe_probe_foreground` 只负责在问它之前
先把这一 tick 的帧记进 `ForegroundState::note_frame`，见下）：

① **从没探过就探**（`last_probe_at == None`）——第一次识别全靠它，少了它下面两条都够不到；
② **距上次探测以来有过帧**，**且**距上次探测 ≥ `PROBE_MIN_INTERVAL_MS`；
③ 已识别到 agent 且距上次 ≥ `PROBE_RECHECK_MS`，**有没有帧都探**（**没有这条，最要紧的那个情形永远够不到闸**：
agent 退出后 shell 回到前台且什么都不画，只看帧的闸再也不会回头看，面板会永远显示一个已经跑完的 agent）。

⚠️ ② 的主语是「**自上次探测以来**」，不是「本 tick」，这个差别就是这条规则本身。本文上一版写的正是「本 tick 有帧」
——那是本轮**修掉**的缺陷：一个启动后画一次就安静下来的程序，那一帧整个落在上次探测的 500 ms 阴影里被丢掉，
此后再没有帧，而 ③ 因为什么都还没识别出来也帮不上忙，「未识别」于是成了一个**吸收态**
（`a_real_agent_started_after_spawn_is_identified` 第一次真跑就抓到）。粘住那一帧的是
`ForegroundState::note_frame`，它在闸说「不」的时候照样记。

（「从不在屏幕锁内探测」不是闸的一条，是锁纪律——见下方**锁纪律**段。上一版把它数进「三条闸」，于是三条里
一条不是闸、真正的第一条闸反而漏了：判据 §6，数错的方向永远是少一个。）

**滞后是不对称的**：命中立刻生效，连续 `PROBE_MISSES_TO_FORGET` 次探不到才把 `program` 退回 shell 标签。
探不到说的是「**我没能看**」，不是「那里什么都没跑」（判据 §8）。

**锁纪律**：一次探测是**三个函数**，不是一个——`leader_from_terminal`（唯一碰 master 的，一次 `tcgetpgrp` ioctl，
在锁内）、`deepest_newest_descendant`（非 Unix 兜底，全表刷新，在**所有锁外**）、`fact_for_pid`（单 pid，
在所有锁外）。它们曾被合成一个 `foreground_leader(master, shell_pid)`，那会逼调用方在 Windows 上**每次探测**
都把 master 锁跨过整次全表刷新，而两条 doc 同时断言相反的事。守卫
`no_process_table_read_happens_under_the_master_lock` 钉住这条边界。

**成本守卫**（herdr 的计数式架构测试形状）：`ForegroundState` 自带探测计数器，
`gateway::runtime::tests::probe_count_is_bounded_at_fifteen_sessions` 断言 15 个会话 × 100 tick 的探测次数
落在上界内，`foreground.rs::probe_count_can_reach_one` 证明计数器不是恒零（判据 §2）。

### 3.2 识别与状态（`crates/agent-detect/` + `src/gateway/runtime/mod.rs`）

`RuntimeAgents::sample` 的输入是 `SampleInput`（结构体而非九个位置参数——其中四个是相邻的
`Option<&str>`/`&str`，正是「换一对也能编译然后开始说谎」的形状）。

- **一次推导两个字段**：`agent_detect::normalized_program_name(name, argv0, cmdline)` 的答案同时铸出
  `program`（叫它什么）与 `agent`（它是哪个 agent），所以两者不可能描述不同的 token（判据 §1）。
  内核的原始 name **不能单独发布**——macOS 把一个叫 `claude` 的 `#!/bin/sh` 脚本报成 `bash`；
  而 `claude` 本身是 Node 脚本，进程名是 `node`，只有命令行认得出它。
- **识别排在 `screen.visible_text()` 之前**（成本，S3）：没识别出 agent 时引擎根本不读那段文本，
  先建就是每会话每帧一次视觉网格大小的分配然后丢弃。守卫
  `identify_runs_before_the_screen_text_is_built` + 计数器 `RuntimeAgents::visible_text_builds`。
- **探测答不出时退回 shell 标签**：那是**更弱的答案，不是错的答案**（`pty.spawn` 带显式 `command` 时
  agent 名字确实在那里）；`program` 保持 `None`——「我们没能看」和「那里跑的是 shell」是两句话。

#### 3.2.1 包装器：一条 launcher 链，不是一次进程树遍历

一条命令行是**一串 launcher 最后落到一个程序**。`agent_token_in_cmdline` 按这条链走：每一步要么
自己就是 agent（结束），要么把活交给下一个 launcher（`sudo` / `npx` / `uv tool run` …），要么是个
通用运行时、它的脚本就是那个程序（`node …/cli.js`）。链**有界**（`MAX_LAUNCHER_LAYERS = 3`），
越界答 `None`。

下表每一行都是 **2026-09-05 在真机上量出来的**，不是推的：用 `pty.fork` 起进程、`tcgetpgrp` 取前台
进程组组长、再对那个 pid 读 `sysinfo` 的 `name` / `cmd[0]` / `cmd.join(" ")`——正是
`foreground::fact_for_pid` 收集的同三个事实。

| 起法 | 组长的 `name` / `argv0` / `cmdline` | 修前 | 修后 |
|---|---|---|---|
| 叫 `claude` 的 shell 脚本 | `bash` / `/bin/bash` / `/bin/bash …/claude` | ✅ `claude` | ✅ |
| `env FOO=1 claude` | 同上——**`env` 会 exec，进程表里根本不出现** | ✅ `claude` | ✅ |
| 真 `pi`（node `cli.js`，设了 `process.title`） | `node` / `pi` / `pi TERM_PROGRAM=Apple_Terminal` | ✅ `pi` | ✅ |
| `claude-code`（node bin 软链） | `node` / `node` / `node …/bin/claude-code` | ✅ `claude` | ✅ |
| **`node …/node_modules/@anthropic-ai/claude-code/cli.js`** | `node` / `node` / `node …/cli.js` | ❌ `node` / `None` | ✅ `claude-code` / Claude |
| **`npx claude`** | `node` / `npm exec claude` / `npm exec claude TERM_PROGRAM=… SHELL=…` | ❌ `"npm exec claude"` / `None` | ✅ `claude` / Claude |
| **`uvx <agent>`** | `uv` / `/opt/homebrew/bin/uv` / `uv tool uvx --offline --from <pkg> <cmd>` | ❌ `uv` / `None` | ✅ `<cmd>` |
| `sudo claude` | 本机无免密 sudo，**未量到**；按 launcher 表处理 | ❌ | ✅（单测形状） |

三个由此定下来的事：

1. **`env` 从来就是好的。** 遗留清单把它列进「识别不了」是错的——它 exec 掉自己，进程表里没有它。
2. **`npx` / `uvx` 的 agent 是组长的孩子**，和组长同一个 pgid。所以 `engine.rs` 里那句
   「Aleph 只探一个进程，`tcgetpgrp` 只给组长的 pid」**作为不移植 herdr `identify_agent_in_job`
   打分半边的理由是假的**（判据 §1：一句承重的注释错了）。仍然不移植，但理由换了：**量到的每一个
   包装器都把自己的 operand 写在组长自己的命令行里**，而后代遍历要付一次**全量进程表刷新**——
   每一次探测、每一个闲置 shell 都要付（`deepest_newest_descendant` 的文档说了这就是它是第二选择的
   原因）。哪天出现一个把 operand 藏起来的包装器，那时候才需要它。
3. **macOS 的 `cmd()` 会把环境变量渗进 argv。** 一个重写了标题的进程（每个 Node CLI 都会）让
   `sysinfo` 从 argv 区读进 env 区，所以真实读数是 `pi TERM_PROGRAM=Apple_Terminal` 和
   `npm exec claude TERM_PROGRAM=… SHELL=…`。因此 (a) `VAR=value` 形状的 token 没有资格被当成
   程序名，(b) `normalized_program_name` 的兜底取 `argv[0]` 的**第一个空白分隔词**——程序名不含空格，
   把整条标题（或粘着环境变量的标题）交给面板是**一个具体的谎**（判据 §17）。

**这张表只覆盖它列出的那些形状**（判据 §18）：`launcher_spec` 是一份名单，名单只覆盖立法当天的世界
（判据 §5）。不认识的包装器答 `None` 并把包装器本身报成 program——弱答案，不是错答案。

| 常量 | 值 | 所有者 | 说明 |
|---|---|---|---|
| `IDLE_HOLD_MS` | 700 | `src/gateway/runtime/mod.rs` | Working→Idle 的去抖。herdr 用**计数**确认（`src/pane.rs:727` 缩短轮询让确认累积），Aleph **刻意只用墙钟**：这里的帧只在屏幕变化时存在，一个跑完就安静下来的 agent 不再产帧，数一个不保证会到的再观察等于让它永远卡在 Working |
| `QUIET_AFTER_MS` | 30 000 | 同上 | 连续这么久没有帧 ⇒ 发布 `quiet_since` |

**`quiet_since` 不是状态转移，也永远不许变成状态转移（spec R2-3）。** 一个思考五分钟的 agent 什么都不发；
让时钟把 `Working` 变成 `Idle` 是在**伪造证据**（判据 §8）。herdr 没有对应物——它靠**排序**让停滞的 agent 可见；
Aleph 两端的面板是列表，所以发布一个时长更便宜。

`changed` 谓词（决定发不发 `runtime.agents.changed`）包含 state / agent / program / label / cwd 与
**`quiet_since` 的 None↔Some 翻转**——不是它的值：值每 tick 都在变老，把它算进去就是每 16 ms 一个事件。

**只有真帧能结束一个 quiet 标记**（`SampleInput::frame_produced`）。这个字段曾被省略，理由是「到得了
`sample` 就说明有帧」——而那句话在唯一的生产调用点是假的（§2 第 3 步：程序变了、屏幕没动也会采样），
于是一个安静的 agent 只要 `chdir` 一次就被重新发布成不安静。守卫
`a_program_change_without_a_frame_does_not_clear_quiet_since`。

### 3.3 工具面等待（`src/builtin_tools/terminal.rs`）

| 常量 | 值 | 所有者 | 说明 |
|---|---|---|---|
| `WAIT_DEFAULT_TIMEOUT_MS` | 60 000 | `src/builtin_tools/terminal.rs` | 与 `bash_exec` 的 `process_action: "wait"` 同值同理由 |
| `WAIT_MAX_TIMEOUT_MS` | 150 000 | 同上 | **派生自** `bash_exec` 的 `WAIT_MAX_TIMEOUT_SECS`（170 s）所受的同一个约束：阻塞调用必须在 harness 的 180 s 前台工具预算内返回（R10，别去扩预算）。守卫 `the_wait_ceiling_stays_under_the_foreground_tool_budget` 钉的是**那个常量**，不是这个数字的第二份拷贝 |
| `WAIT_DEFAULT_UNTIL` | `[blocked, idle]` | 同上 | 「告诉我它什么时候需要我」；`working`/`unknown` 合法但从不是那句话的意思 |
| `EXPLAIN_SCREEN_TAIL_LINES` | 12 | 同上 | 只是回显给人看的窗口。**引擎吃的是整屏**，与采样器一致 |

超上限的请求**被夹紧，不被拒绝**：要等十分钟的调用方想的是尽量久地等，答「不行」只换来一次重试。
空的 `until: []` 反过来**被拒绝**——一个显式的空集永远到不了，诚实地拒绝比默默替换成默认集好。

---

## 4. 闸

内嵌终端有**三张脸**，每张都要单独关（判据 §9）。全文与信任模型见
[SECURITY.md](SECURITY.md#embedded-terminal)，这里只给落点。

| 面 | 闸 | 落点 |
|---|---|---|
| RPC（`pty.*` / `runtime.*`） | operator-only | `src/gateway/method_admin.rs::ADMIN_PREFIXES` |
| 事件（`pty.screen` / `pty.exit` / `runtime.agents.changed`） | operator-only | `src/gateway/event_scope.rs::default_rules` |
| 工具（`terminal`） | operator-only **两道** | `src/gateway/method_authz.rs::OPERATOR_TOOLS` + `src/builtin_tools/terminal.rs::caller_is_operator` 内联 |
| 子系统开关 | `[policies.terminal] enabled` | `src/config/types/policies/terminal.rs::TerminalConfig`；执行点 `src/gateway/handlers/pty.rs::handle_spawn`（每次 spawn 读新值）；关掉会杀掉在飞会话（`live_apply` 的 `"policies.terminal"` 臂调 `PtyManager::close_all`） |
| spawn 目录 | 工作区根内 | `src/gateway/pty/jail.rs::resolve_spawn_cwd`。**只管起点**——终端里的 `cd` 不受约束，别把它当隔离引用 |

**归属过滤在这个文件里只有一个谓词**：`src/builtin_tools/terminal.rs::terminal_admits`。
`list` 直接拿它比 `SessionInfo::created_by`；`read` / `wait` / `explain` 经 `owner_record_admits` +
`PtyManager::owner_of` 走同一个函数体，所以五张镜头不可能对「哪些行你能看」悄悄给出不同答案。

**零身份臂（spec D7）是刻意收窄的**：`actor == None` 时只放行 `created_by == None` 的会话，
而**生产上每一次 spawn 都盖 actor**（loopback 的 operator 解析出 `Some(OWNER_USER_ID)`——
`src/gateway/handlers/connect.rs` 的 loopback 臂），所以一个没有身份的调用方（cron / A2A / 内部接线）
**什么都看不见**。
这是有意的 fail-closed；要重新打开它，开口在**身份系统**（让那次运行带上身份），不在这个工具。
守卫 `an_actorless_caller_sees_only_unowned_sessions` + `a_loopback_operator_is_not_an_actor_less_caller`。

不属于你的会话一律答 `no_such_session`，与「不存在」逐字节同形——一句「这不是你的」会把每个动词
变成枚举别人 session id 的 oracle。

⚠️ **一个已记录在案的缺口**（不是本轮引入，也不是本轮修的）：`ScopedToolService` 路径上，
一个 chat 档位/member 调用方会先触发 `OPERATOR_TOOLS` 的审批卡，**而即使人批准了，内联检查仍然拒绝**——
批准只翻转派发管线的 `authorized`，没有任何东西重盖 `TurnContext`。决定的修法是一条**接缝**
（把 `check_operator_gate` 已经算出的 `approved_by_operator_gate` 传下来），那条缝还不存在。
**别用「删掉内联检查」来修它**：审批卡的文案今天写着 "…which changes Aleph's own configuration"，
对一个只读工具是假的，删掉内联检查等于让一张贴错标签的卡真的授出一次别人终端屏幕的读取。
全文在 `src/builtin_tools/terminal.rs` 的模块 doc 与 `src/gateway/method_authz.rs` 的 `terminal` 条目。

---

## 5. wire 契约

**服务端响应一律用 `shared/protocol` 的类型构造**，不手搓 `json!`（判据 §10：一个只读自己刚写下的
字面量的断言测的是 serde，永远绿）。

`shared/protocol/src/pty.rs`
- `PTY_LIST_METHOD` / `PTY_SCREEN_TOPIC` / `PTY_EXIT_TOPIC`
- `PtySessionInfo { session_id, shell, cwd, created_at, closed }` + `PtyListResponse { sessions }`
  ——**刻意不带 `created_by`**（零读者）。键集相等由 `pty_list_response_round_trips_and_pins_its_key_set`
  与 `gateway::handlers::pty::list_response_is_built_from_the_protocol_type` 两侧各钉一次
  （解析只能证明超集，永远证不出相等）。
- `PtyScreenPatch` 第 2 期新增 `cursor_visible` / `bracketed_paste` / `cwd`，都是 `Option<_>` +
  `skip_serializing_if`：**只有变化时才 `Some`**。
- **清空在 wire 上拼作 `Some("")`**（`screen::perform::published_clear`）。客户端规则一句话：
  **空串或缺席都读作「没有」**。`TabModel::derive_title` 与 `flush_session` 的 cwd 顺序各自实现了这条规则
  的同一种拼法。

`shared/protocol/src/runtime.rs`
- `RUNTIME_AGENTS_LIST_METHOD` / `RUNTIME_AGENTS_CHANGED_TOPIC`
- `RuntimeAgentEntry { session_id, label, cwd, agent, program, state, updated_at, quiet_since }`
  - `program: Option<String>` —— `None` = **本平台/本会话探不到**，不是「没有程序在跑」。
  - `quiet_since: Option<i64>` —— 只有资格说「安静了多久」，不说「它闲了」。
  - `cwd` —— live cwd，来源顺序见 §2 第 5 步。
- `terminal{wait}` 的载荷直接用 `RuntimeAgentEntry`，所以等待者拿回的行与 `status`、
  `runtime.agents.list` 是**同一种拼法**（判据 §10）。

---

## 6. herdr 对照（0.8.2 · Apache-2.0）

> **许可边界**：herdr 派生的代码只允许住在 `crates/agent-detect/`（机器可读的 Apache-2.0 区，第 1 期 spec）。
> `src/gateway/pty/foreground.rs` **不是移植**——它是本仓自己的 MIT 代码，从 herdr 拿的只有
> **两个数字和一个想法的形状**，逐处标注（见 §3.1 与该文件的头部）。

| 维度 | herdr | Aleph | 裁定 |
|---|---|---|---|
| VT | libghostty-vt（vendored Zig + 352 KB bindgen），`src/pane/terminal.rs` 是 6 735 行**包装层** | `src/gateway/pty/screen/`，基于 `vte` crate 的输出向文本采样器 | **不引第二个 VT**（CLAUDE.md 禁用清单）；缺口一律**扩容 `screen/`**。0-A 清单是那次定价 |
| agent 识别 | 前台进程组 + argv/cmdline 归一化（`src/detect/mod.rs:243` `identify_agent_in_job`） | `portable-pty` 的 `process_group_leader()` + `sysinfo` 单 pid 刷新 | **同一个想法，不同的实现**：herdr 走自己的 `crate::platform` FFI，R1 够不到 |
| 状态源 | 四源：屏幕兜底 / hook 权威 / 进程事实做闸 / metadata | 屏幕 + 进程事实 | hook 源是第 3 期（要往用户的 `~/.claude/settings.json` 写东西，须用户点头） |
| 静默的 agent | 无时间衰减；靠 `seen`/`Done` + 注意力排序让它可见 | 无时间衰减；发布 `quiet_since` | 两边都**不**用时钟伪造 idle。可见性机制不同（排序 vs 时长）因为面不同（树 vs 列表） |
| 多窗口 | workspace → tab → BSP 树，pane 是指向 terminal 的布局槽 | tab 条（第 2 期） | BSP 分屏 → 第 3 期 |
| 工具面 | `agent {get,read,wait,explain,prompt,send-keys,start}` | `terminal {list,read,status,wait,explain}` | **只读**。写动词是授权架构的决定 → §7 |
| 协议层授权 | 无：0600 socket，同用户即全权 | operator 闸 ×3 + 单源归属谓词 + Ed25519 身份账本 | Aleph **更强**，这是写动词若要做的地基 |
| 持久化 / handoff | 快照存 cwd/argv/session-ref，fd 经 `SCM_RIGHTS` 交接 | 无（重启即丢） | 第 3 期 |
| agent hooks | 20 个集成资产走同一 socket；6 个全生命周期 | 无 | 第 3 期，需用户点头 |

---

## 7. 刻意不做

> 来源是第 2 期 spec 的 **§7 DECIDE**（需要用户裁定、本轮不实施）与 **§8 明确不做**（第 3 期候选）。
> 别从记忆里重新推导这两张清单——去 spec 读。

### 7.1 需要用户裁定（spec §7）

- **`terminal` 的写动词**（`spawn` / `send` / `keys` / `close`）。理由不是「难」：PTY **不经
  `[sandbox.command_policy]` 也不经 exec tier**，今天这条路只对**人**开放。把它给 LLM 就是给 Aleph 自己的
  模型一个绕过全部命令闸的 shell——这是**授权架构的决定，不是一个功能**。
  若裁定「做」，推荐形状全文在 spec §7.1（第一版只做 `send`/`keys`、写前五项前置检查、
  只对 `owner_admits` 为真的会话开放、每次写入进签名账本、加一根 `terminal_write: off|ask|on` 旋钮
  且 `ask` 之前要先修 R11-14 的审批卡标签缺陷）。
- **BSP 分屏**（08-29 spec Phase 6）。tab 之后的自然下一步；不做的唯一理由是 Panel 端零测试环境 + 规模。
- **agent hooks 状态源**（herdr §3）。要往用户的配置文件里写 hook，是对用户环境的写入。

### 7.2 第 3 期候选（spec §8）

BSP 分屏 · scrollback 读取 RPC + 滚轮 · 鼠标模式 · kitty 键盘 · 会话持久化/恢复与 fd 交接 ·
agent hooks · `seen`/`Done` 第五态与注意力流（需要写入面）· worktree 模型 · manifest 远端热更新 ·
`AgentPanelState` 的 leptos 信号面 · TUI 拖拽分割条。

### 7.3 本文件不承诺的覆盖

- **真机只覆盖 RPC 往返**。`qa/terminal/run.sh` 的四个阶段全绿于一台真机（四次手工证伪变异各自把
  它声称覆盖的断言变红），但**每一条断言都是 RPC 往返**：Panel 的 tab 条、面板行点击跳转、粘贴、
  光标可见性**从未对着一个跑起来的 server 验过**（stream C 没有真机阶段）。
- **Windows 没有真机**。非 Unix 的前台进程只有「最深、最新的后代」启发式，
  `cargo check -p aleph-desktop-windows` 只能证明它编译。
- 装置自己写下的边界（`qa/terminal/run.sh` 头部）：**第三层 cwd（spawn 目录）够不到**
  （需要一次**失败**的探测，从 wire 上安排不出来）；`program: null` 同理；manifest 表只端到端跑了 `claude` 一个（装置头部自己写下这条边界：
  其余的由 `agent-detect` 自己的套件在进程内覆盖，一个画二十屏的装置只是在用最慢的仪器重测规则引擎）。
