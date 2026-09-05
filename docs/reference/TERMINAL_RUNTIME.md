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

   ⚠️ 渗进来的**不总是**规规矩矩的 `VAR=value`。`exec npx pi` 在本机的逐字读数是
   `npm exec pi ZSH_AI_PROMPT_EXTEND=Always prefer modern CLI tools like ripgrep, fd, and bat.
   CLAUDE_CODE_MESSAGING_TOKEN=…`——一个**值里带空格**的导出变量把 `prefer` / `modern` / `like`
   这样的**裸词**撒进了程序名该在的位置。它们无害只因为一个结构性理由：**argv 排在环境之前**，
   所以 operand 总是先被读到，而这条链取的是**第一个** operand，不是「扫到一个能识别的为止」。
   改成扫描的那一刻，操作员 prompt 字符串里随便哪个词都能变成 agent 名。

**这张表只覆盖它列出的那些形状**（判据 §18）：`launcher_spec` 是一份名单，名单只覆盖立法当天的世界
（判据 §5）。不认识的包装器答 `None` 并把包装器本身报成 program——弱答案，不是错答案。

真机装置：`qa/terminal/run.sh real` 用**PATH 上真的 agent 二进制**跑上面第 3、6 行（本机挑中 `pi`，
因为它带 shebang——替身伪造不了的那种形状），`qa/terminal/run.sh tui` 用**真的 `aleph-tui`**
证明 TUI 的 agent 面板显示的值来自活 socket。两者 2026-09-05 全绿。

#### 3.2.2 Windows 真机（2026-09-05，Windows 11 Pro 10.0.28000）

上表整张是 **macOS** 的读数：那条路走 `tcgetpgrp`。Windows 走的是**另一半代码**——
`leader_from_terminal` 在 `cfg(not(unix))` 下按构造恒 `None`，所以每一次探测都落到
`deepest_newest_descendant`。**这一半在 2026-09-05 之前从没在 Windows 硬件上执行过一次**：
实现刻意做成平台无关（见该函数的文档），而它的三条 exerciser 全都戴着 `#[cfg(unix)]`，
被门在了**不需要它的那个平台**上。三条现已平台无关，读数如下。

| 守卫 | Windows 上实测 |
|---|---|
| `foreground::the_descendant_walk_finds_a_child_this_test_started` | `cmd /c ping` 两级树：`root=2344(cmd.exe) → 13088(PING.EXE)`，descend 成功 |
| `foreground::a_real_child_is_reported_as_the_foreground_program` | 生产连线答 `pid=1772 name="PING.EXE" argv0="ping" cwd="C:\Users\zou\"`——主语是**孙**进程，`leader_from_terminal` 结构上说不出它 |
| `runtime::a_real_agent_started_after_spawn_is_identified` | `agent: Some("claude") · program: Some("claude.exe") · cwd: "C:\Users\zou\"`，spawn 标签仍是 `cmd.exe` |

**证伪**（把 `deepest_newest_descendant` 中和成 `Some(shell_pid)`，重编译后跑）：三条**全红**，
且失败消息各自点出走树在承载哪一句——`left: 5224 right: 5224`（没 descend）／拿到 `cmd.exe`
而不是 `ping`／`agent: None`。在 Unix 上同一个变异只红第一条，后两条由 `tcgetpgrp` 兜住——
**这个不对称本身就是「Windows 上这三句由走树承载」的证明**。

三件量出来、和 macOS 不一样的事：

1. **`program` 在两个平台上是两个字符串**：`sysinfo` 在 Windows 报 `claude.exe`，
   `normalized_agent_lookup_name` 剥掉扩展名去识别 **agent**（✅ `Some("claude")`），而
   `normalized_program_name` 回的是它**看的那个 token**，于是面板在 macOS 印 `claude`、
   在 Windows 印 `claude.exe`。不是谎，但也不是同一句话。**刻意没有在这一轮抹平**——
   抹平会改掉每个平台印什么，那是产品裁决不是测试该做的决定。
2. **`cwd` 在 Windows 带尾分隔符**（`C:\Users\zou\`），macOS 不带。三个 cwd 生产者的
   派生点是一处（判据 §12），但**归一化不在那一处**。
3. **一次全表走树实测 5.30 ms**——谓词：**debug** 构建、171 个活进程、这台机器、2026-09-05
   （数值印在 `the_descendant_walk_finds_a_child_this_test_started` 的 `--nocapture` 输出里，
   **刻意不断言**：那是硬件，一个别人复现不了的上限比没有上限更糟，判据 §13）。对照
   `PROBE_MIN_INTERVAL_MS = 500`，一个满速产帧的会话每秒最多付 2 次 ⇒ 约 1% 的一个核；
   它没有推翻「走树是第二选择」这个定位，但也不是免费的——**代价随会话数线性叠加**，
   因为每个会话各自 `System::new()` + 各自全表刷新，之间不共享快照。

#### 3.2.3 `pty.exit` 曾在 Windows 上永远不发（同轮量到、同轮修掉）

**症状**：Windows 上每一个**程序已退出**的终端会话，永远留在 `pty.list` 和 agent 面板里——
正是 `manager.rs::owner_of` 自己写下的那句「a client that never learns its shell died shows a
live terminal forever」。

**机制**：`spawn_reader` 的整条收尾（`child.wait()` / `closed` / `pty.exit` /
`manager().remove` / `runtime::agents().remove`）全在读循环 `break` 之后，而 break 条件是
`Ok(0)`。判据 §6 的形状：**「孩子退出了」和「终端到了 EOF」是两个事实，代码只给了它们一个
推导者，而这个平台不提供被拿来当推导来源的那一个**。

**两个数字是量的不是推的**（一次性探针，直接对 `portable-pty`，本机 2026-09-05）：

| 问 | 答 |
|---|---|
| 孩子退出时 `child.wait()` 会不会及时返回？ | **2.07 s 返回 code=0**（孩子恰好 ~2 s 退出）⇒ 等孩子这条路在 Windows 上完全正常 |
| 卡住的 `read` 靠什么解开？ | 孩子退出后**仍卡 ≥3 s**；**drop 掉 master 后 1.94 ms** 拿到 EOF |

**修法**：读线程只喂屏幕、不再收尾；新的 waiter 线程持有 `child`，`child.wait()` 返回后调
`settle_exit`——**一个事实一个推导者，而且是这个平台真的提供的那一个**。

⚠️ **修复本身带一个回归口，值得单记**：新的触发点（孩子退出）**早于**旧的（EOF = 数据已排空）。
照直写就会在 reader 把尾部输出喂进屏幕之前把会话从 manager 摘掉——**Unix 上本来好好的输出
会被截掉**。所以 `settle_exit` 先给 reader 一个有界的 `READER_DRAIN_GRACE`（500 ms）自己结束；
**只有仍然卡着的 reader 才会被抽走 master**。于是 EOF 正常的平台上一个字节都没变（master 根本
不被碰），而这根杠杆只作用在「这个终端确实没有 EOF」的情形——顺带也治了 Unix 上孙进程握着
slave fd 导致同样卡死的那一类。

**守卫** `session::a_child_that_exits_settles_the_session_without_needing_terminal_eof`，
两半分开断言，因为它们会分开失败：① `pty.exit` 发出来了；② 会话的 `Arc` 强引用降回调用方
自己那一份——**删掉抽 master 那一步，①仍然绿而 reader 线程连同整块屏幕与 scrollback 泄漏
到进程结束**。Windows 实测 `pty.exit at 2.6076167s, strong_count=1`：2.07 + 0.5 ≈ 2.61，
**这个算术本身就证明走的是「reader 仍卡住 → 拉杠杆」那条臂**。
`a_session_that_exits_leaves_the_table` 随之转绿。

**这一节不承诺的**：`qa/terminal/run.sh` 的六个阶段在 Windows 上**仍然是 UNRUN 而不是 PASS**
——装置是 python 的，而本机 `python3` 是 WindowsApps 存根（同 `qa/spend_budget`，见
[`qa/README.md`](../../qa/README.md)）。上面三行是**进程内**守卫，走的是真 PTY、真进程表、
生产调用序，但不是发货二进制上的 RPC 往返。

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

#### 3.2.4 Windows 第二轮：三个缺陷，其中两个是**同一次有损往返**（2026-09-05）

上一轮把 §7.3 的两条 Windows 缺口写成「刻意不修 / 演示不出红」。第二轮把它们都关了，并且在
路上撞出一个**没人在找、比那两条都贵**的第三个。

**1. `C:\Program Files\…` 让面板把程序名印成 `Program`，并让整条 launcher 链失效。**

用一个独立的 `sysinfo` 0.39.6 探针（scratchpad，一次性）在本机实测：Windows 上 `cmd()` 返回的是
**正经 argv 向量**，元素 0 是**含空格的完整映像路径**。

```
argv0              = "C:\Program Files\Git\bin\bash.exe"
join(" ")          = "C:\Program Files\Git\bin\bash.exe -c ..."
split_whitespace[0] = "C:\Program"
```

顺着当时的真实代码推到底，**两处独立失效**：

- `normalized_program_name` 的兜底是 `path_basename(first_word(argv0))` ⇒ **`"Program"`**。
  `C:\Program Files\` 下的**每一个**程序在面板上都叫 `Program`（判据 §17：错的标签比缺的贵）。
- `agent_token_in_cmdline` 的 `tokens[0]` 是 `"C:\Program"`——不是 agent、不是 launcher、
  也不是 generic runtime ⇒ 直接 `None`。**launcher chain 分析对任何路径带空格的进程整条死掉**，
  而 `C:\Program Files\nodejs` 正是 Windows 上 node 的默认安装位置，即 node 装的 agent
  开箱识别不了。

**根因不是 tokenizer，是 `fact_for_pid` 先 `join(" ")` 压扁、`agent_token_in_cmdline` 再
`split_whitespace` 拆回来的那次有损往返**（判据 §1：同一事实的两份表述，其中一份是另一份的
削弱版）。所以修在往返上——`ForegroundFact` 改成携带 `argv: Vec<String>`（顺带删掉 `argv0`，
它本来就是 `cmdline` 的第一个 token，是第二份表述），`agent_detect` 的两个入口改吃 argv 切片。

拆开两种「含空格的元素」只需要**一条**规则，因为要分辨的只有一件事：

| 元素 | basename | 判定 |
|---|---|---|
| `C:\Program Files\Git\bin\bash.exe` | `bash.exe`（无空格） | 真 argv 元素，**整个**是一个 token |
| `npm exec claude TERM_PROGRAM=…` | 就是它自己（有空格） | 被改写的进程**标题**，按空格拆 |
| `pi TERM_PROGRAM=Apple_Terminal` | 就是它自己（有空格） | 同上 |

同一条规则也把 `first_word` / `path_basename` 的**顺序**定死了：**先 basename 再取首词**。
守卫 `an_argv_element_splits_only_when_its_basename_still_has_a_space` 两半都断言（只钉
Windows 那半的话，一个「不再拆标题」的改动会让它绿）。**两半各自证伪过**：把顺序换回去 ⇒
`left: "Program"`；把 `argv_tokens` 改成永远拆 ⇒ `left: None right: Some(Claude)`。

**2. 「最深的后代」不是「拥有这个终端的程序」——已修**（详见 §7.3）。

**3. 单调性闸——已加，并且改掉了「演示不出红就先不加」这个判断**（详见 §7.3）。
本机连续三次实测：`total=174 dangling_ppid=10 inverted_start_time=0 self_parent=0`。

##### 装置：整套驱动从 Python 换成 Node

`qa/terminal/run.sh` 在 Windows 上一直是 **UNRUN**，原因不是设计而是**语言的意外**：驱动是
Python，而这台主机上没装解释器。**Windows 恰好是前台探测没有 `tcgetpgrp`、
`foreground_fact_for_shell` 是全部答案的那个平台**，所以「装置在那里跑不了」是它最不该跑不了的地方。

⚠️ **这段的第一稿把理由写错了，而写错的理由待在注释里是最贵的那一类（判据 §1）**。初稿写的是
「这台机器上唯一的 `python3` 是 WindowsApps 存根」，被用户当场纠正。实测的准确版本：
`python`/`python3` 在 PATH 上**确实**是 WindowsApps 存根（不运行、exit 49），但**这不等于
「没有别的路」**——`uv` 装着，而且 `uv` 正是 Aleph 自己 `bootstrap-runtime` 的
`DEFAULT_TARGETS` 之一、也是 `prompt_build.rs` 引导模型去用的那个（"letting it invoke
`uv run` / a managed interpreter instead of bare python"）。此刻它只是**还没装解释器**
（`uv python find` exit 2），`uv python install 3.12` 就能有一个。判据：**「这台机器上没有 X」
和「PATH 上第一个 X 不能用」是两句话**，而前者需要把这个仓自己提供 X 的那条路也查过；
`run.sh` 的 `PY_CMD` 解析因此按 Aleph 自己的顺序找（真 `python3` → `uv run`），并且
**刻意不替操作员下载**（`uv run` 会去拉一个运行时，一个中途悄悄下载运行时的装置是它自己的隐患）。
⚠️ 这**不改变** `real`/`tui` 的结论：CPython 的 `pty` 只在 Unix 上有，所以那两个阶段与解释器
从哪来无关。

现在 `identify` / `wait` / `quiet` / `cwd` 与 `panel` 的布板走 Node（`drive_terminal.mjs` ·
`derive_chrome.mjs` · `derive_agent_bins.mjs` · `patch_config.mjs` · `toml_min.mjs`），
假 agent 从 bash 换成 `fake-claude.cjs`（装成**无扩展名**的 `claude`——`claude.js` 会让
`program` 在两个平台上是两个字符串）。`real` 与 `tui` **没跟着搬**，理由是结构性的而非移植
偷懒：`probe_alive.py` 和 `drive_tui.py` 都用 `pty.fork` 驱动一个程序，Node 没有原生模块就
没有 pty；它们在跑不了的地方**响亮地 SKIP 而不是报 pass**（判据 §2）。

⚠️ **Windows 上 shell 交互是另一种拼写，这一点没有被藏进各调用点的分支**：`drive_terminal.mjs`
把它收成一个 `SHELL` kit（`cmd.exe` / `\r\n` / `set "PATH=…"` / `set K=V` 各自成句）。
逐站点写 `cfg` 分支正是「Windows 那条臂安静地不再敲 agent、而阶段照样报出对照会话的行」的形状。

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
- **上一版这里挂着的两个 Windows 缺口，2026-09-05 第二轮都关掉了**（见 §3.2.4）。历史留在
  这里，因为**关掉它们的过程本身**改掉了三个判断：
  - **「最深」不等于「那个 agent」——已修**。`tcgetpgrp` 给的是**进程组组长**，走树给的是**最深的
    后代**，所以 `claude` 每跑一个工具子进程，Windows 上被报成前台程序的就是**那个工具**。现在
    `foreground::pick_foreground` 先在候选里取**最浅的、识别得出 agent 的那个**，取不到才回落
    deepest-newest；agent 的工具永远比 agent 深，所以这一条是充分的。**「最浅」不是「最深」的
    反面而是 launcher 链的头**：`npx claude` 把 launcher 摆在 agent 上面，而 launcher 自己的
    命令行已经点名了它的操作数（`agent_detect::identify_agent_from_process`）。分层代价（把识别
    搬进 `gateway::pty`）已由用户裁决接受。
    ⚠️ **修它的前提是先修 `agent_detect` 的空格路径**（§3.2.4 第一条）：在那之前
    `identify_agent_from_process` 对 `C:\Program Files\nodejs\node.exe` 恒答 `None`，
    agent 优先这条臂**一次都不会触发**，而它会安静地永远回落到 deepest-newest。
  - **上一版把这条缺口的证据认错了，那才是本轮最贵的一课**。原文写着「它已经在让
    `a_changed_sample_…` 变红，因为 `changed` 谓词含 `program`」，并用一次独立的
    `Win32_Process` 走树复测支持它。复测本身没错，**结论的作用域错了**：把混淆项从 fixture 里
    移掉之后（`ping` → 只用 cmd builtin），那条测试**照样红**，而诊断打出来的翻转字段
    **不是 `program` 而是 `label`**——`label` 是 OSC 标题（非空时压过 spawn 标签），
    而 `cmd.exe` 启动后会把控制台标题设成**自己的映像路径**，那条 OSC 恰好落在两帧之间。
    判据：**一次测量证明了「A 会变」，不等于证明了「A 是那条断言变红的原因」**——谓词是五项的
    析取，只有把其余四项按住才谈得上归因。现在 fixture 自己 `echo` 一条 OSC 抢下标题、并且
    **等到那个标题真的落在行上**才开始第二次观测（写成「flush 到不再变化」会在第一个没产帧的
    tick 上退出，那是个不可能失败的谓词，判据 §2）。
  - **Windows 的 ppid 会说谎——单调性闸已加**。Windows 没有 reparent-to-init，孤儿保留**已死
    父进程的 pid**，而 pid 会回收。2026-09-05 本机实测（连续三次）：174 个活进程里
    **10 个的 ppid 指向已死 pid**，**0 个已经成真的假边**（`inverted_start_time=0`）。
    上一版据此**没加**这个闸，理由是「演示不出红」——那个理由现在被替换掉了：闸本身没问题，
    问题是**当时的形状让它无法被证伪**。把走树拆成纯函数（`ProcRow` + `descendants_of`）之后，
    一张合成表就能确定性地让它变红（`a_parent_pointer_that_predates_its_child_is_not_an_edge`，
    中和后实测 `left: [(1,1100,200), (2,5,900)]`）。判据：**一条在你这台机器上演示不出红的规则，
    不是「先不加」的理由，而是「它现在挂错地方了」的信号**——把它挪到一个吃合成输入的纯函数上，
    它就有红了。同一次重构顺带给 64 层界与环路保护第一份覆盖，并加了一道 `seen` 去重（单调性闸
    看不见「两行报同一个 start_time」的环）。
- 装置自己写下的边界（`qa/terminal/run.sh` 头部）：**第三层 cwd（spawn 目录）够不到**
  （需要一次**失败**的探测，从 wire 上安排不出来）；`program: null` 同理；manifest 表只端到端跑了 `claude` 一个（装置头部自己写下这条边界：
  其余的由 `agent-detect` 自己的套件在进程内覆盖，一个画二十屏的装置只是在用最慢的仪器重测规则引擎）。
