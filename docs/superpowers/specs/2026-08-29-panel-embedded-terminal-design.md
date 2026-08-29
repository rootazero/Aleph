# Panel 内嵌终端 — 设计（Orca 结构布局对标）

**日期**: 2026-08-29
**分支**: `worktree-panel-embedded-terminal`
**参照项目**: `/Volumes/TBU4/Github/Orca`（Electron + React + xterm.js + node-pty）
**状态**: 设计已逐段确认，待实施

---

## 0. 一句话

Aleph 的 `pty.*` 子系统**服务端已完整实现且零客户端**。本设计不是从零建子系统，是**接通一条断线**，并在接通的同一笔里把仿真器搬到服务端 —— 用架构换掉 Orca 花了一整个子系统做的三件事（重连恢复、背压、多客户端共屏）。

---

## 1. 现状扫描

### 1.1 Aleph 既有资产（`src/gateway/pty/`，474 行）

| 组件 | 锚点 | 状态 |
|---|---|---|
| PTY 会话 | `pty/session.rs::PtySession`（`portable-pty 0.8`，Unix + Windows ConPTY） | 完整 |
| 全局注册表 | `pty/manager.rs::PtyManager`（`LazyLock`，FIFO cap 64） | 完整 |
| RPC 面 | `pty.spawn/input/resize/close/list`（`handlers/mod.rs:362-366`） | 已注册 |
| 事件面 | `pty.output` / `pty.exit`（base64，8 KiB/帧） | 已发布 |
| 权限 | `method_admin::ADMIN_PREFIXES` 含 `"pty."` + `event_scope::default_rules` 双面 admin 闸 | 完整 |
| **客户端** | — | **全仓零个** |

零客户端经精确 grep 确认：`interfaces/`、`shared/`、`desktop/` 中无一处出现 `pty.spawn` / `"pty.` / `pty.output`。

### 1.2 已确认的缺口

| 缺口 | 事实 | 后果 |
|---|---|---|
| 无回放 | 字节 fire-and-forget 上总线 | 掉线即永久丢失 |
| 无背压 | 每 8 KiB 一个事件直发共享总线 | `yes` 打爆 WS |
| **总线会丢帧** | `GatewayEventBus` = `tokio::sync::broadcast`，容量 1024，慢订阅者 `Lagged`（`event_bus.rs:405-417`） | **原始字节流丢一块即永久乱码，零报错** |
| 无 cwd 授权 | `SpawnOptions.cwd` 客户端自填 | 可在任意目录开 shell |
| 无归属 | `SessionInfo` 无 owner 字段 | 问责对"哪个人"沉默 |
| 无持久性 | 纯内存 `LazyLock` | 重启后对已存在过的会话答"从来没有过" |
| 绕过命令闸 | `method_admin.rs:247` 自陈 "strictly more dangerous, not equally protected by a different layer" | 接线让这个休眠缺口当场承重 |

### 1.3 Orca 侧（不移植的部分）

Orca terminal ≈ 160 文件；`TerminalPane.tsx` 单文件 3361 行；含 park/reveal、OSC 52、kitty keyboard、agent 感知 tab、Quick Commands、worktree 绑定、WebGL/ligatures、IME e2e workflow、perf gate。**1:1 不可行，也不必要。**

值得对标的是它的**结构**：`TabGroupLayoutNode = leaf | split{direction, first, second, ratio}` 二叉树 + tab 条 + 每 leaf 一个 PTY。

---

## 2. 四个已确认的架构决策

| # | 决策 | 取值 |
|---|---|---|
| D1 | 复刻档位 | **B 档**：tab 条 + 二叉树分屏 + 每 pane 一 PTY + 布局持久化 + 搜索 + 链接点击 |
| D2 | 仿真器归属 | **服务端持屏 + 网格差分** |
| D3 | 安全粒度 | **会话粒度闸 + cwd jail**（命令粒度在 PTY 上结构性做不到） |
| D4 | 开关默认值 | **默认开 + Panel 设置页可关**（避免 fail-dead） |

### D2 的理由（为什么不是 xterm.js，也不是客户端 vte）

| 性质 | 服务端持屏 | 客户端 vte | xterm.js |
|---|---|---|---|
| 重连恢复屏幕 | ✅ 架构送（attach 回快照） | ❌ 另做 | ❌ 靠 serialize addon 自己做 |
| 背压 | ✅ 架构送（一屏有上界） | ❌ 另做流控 | ❌ 另做流控 |
| 多客户端共屏 | ✅ 架构送 | ❌ 各自解析会分叉 | ❌ |
| 单测环境 | core（强） | wasm（弱） | JS |
| 新依赖 | 无（`vte 0.14.1` 已在 `alephcore` 依赖树，经 `strip-ansi-escapes`） | 同左 | npm 运行时依赖 + bundler |
| 构建形态 | 不变 | 不变 | Panel 从"纯 wasm + tailwind"变成带 JS 供应链 |

CSP 不是障碍（`script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' blob:` 允许同源 JS）；挡住 xterm.js 的是构建纪律与 `webview-baseline.json` 的 wasm 围栏旁多一条无围栏的 JS 线。

---

## 3. 后端设计

### 3.1 屏幕状态机（新增 `src/gateway/pty/screen.rs`）

```
PtySession
 ├─ reader thread（已存在，OS 线程，阻塞读）
 │    read() → screen.feed(&bytes)      ← vte::Parser 就地解析，不占 tokio blocking pool
 │             screen.mark_dirty()
 └─ flush task（新增，tokio interval 16ms）
      screen.take_diff() → publish("pty.screen", {session_id, seq, patch})
```

`Grid`：`rows × cols` 的 `Cell{ch, fg, bg, attrs}` + 光标 + **备用屏**（`\e[?1049h`，不做则 vim/htop/opencode 全废）+ 有界 scrollback。

行用 trim-trailing-blank 的 `Vec<Cell>` 存；**scrollback 默认 1000 行/会话**（可配）。按 200 列估算 ≈ 3 MB/会话，64 会话上限约 200 MB 最坏。

### 3.2 三个取舍（已确认接受）

1. **16ms 合流是背压的全部实现。** 输出延迟固定 +≤16ms。这让 `pty.output` 的 8 KiB/事件洪水路径失去存在理由。
2. **服务端持 scrollback 让重连恢复完整**，代价是内存 + "终端历史住在服务器上"（诊断/审计可见 → SECURITY.md 要记一笔）。
3. **core 多一个状态机**：`src/gateway/pty/` 从 474 行 → 约 1200 行。R3 顾虑不成立（`vte` 已在树，`Grid` 是自有代码，不是重库）。

### 3.3 这个设计关掉的门

终端的**真实字节流此后没有任何面能看到**（只有网格）。将来若要"把终端输出喂给 agent"或做终端录制，需要重新引一条旁路。判定为 YAGNI（R10），但如实记录。

---

## 4. wire 契约与重同步

契约住在 `shared/protocol/src/pty.rs`（`aleph_protocol::pty`）。

### 4.1 帧形状（行级差分 + 同风格 run 折叠）

```rust
PtyScreenFrame { session_id, seq: u64, patch: ScreenPatch }

ScreenPatch {
    rows: Vec<RowPatch>,        // 只发脏行，整行重发；行内按相同 SGR 折叠成 run
    cursor: Option<CursorState>,
    alt_screen: Option<bool>,
    title: Option<String>,      // OSC 0/2 → tab 标签
    bell: bool,
}
```

**行级而非单元格级**是有意的：单元格差分省带宽，但换来一整类"客户端某个格子没更新"的 bug；一行只有 200 格。行级的性质是**每次重发即自愈**。

### 4.2 seq 纪律（因为总线会丢帧，这是地板不是加固）

- `seq` per-session 单调，每次 publish 递增。
- 客户端收到 `seq != last + 1` ⇒ 调 `pty.attach` 拉全量。
- **`pty.attach` 必须是一个快照**：`{seq, grid, title, cursor, alt_screen, scrollback_len}` 一次返回。分两次调用会开出"拿着屏幕却拿着另一份光标"的窗口。
- **快照在途期间的差分必须缓冲后重放**：快照取于 seq N，响应回来时 N+1/N+2 可能已发出 ⇒ 客户端在 attach 期间缓冲所有帧，回来后丢弃 `≤ N`、重放 `> N`。**不做这一步屏幕会静默错位且零报错。**
- `pty.spawn` 的响应同批带初始 `seq`，否则 spawn→首帧之间是同一个窗口。

### 4.3 RPC 面变更

| 方法 | 变更 |
|---|---|
| `pty.attach {session_id}` | **新增** — 全量快照 + seq |
| `pty.scrollback {session_id, from, to}` | **新增** — 翻历史 |
| `pty.spawn` | 响应加 `seq` |
| `pty.input` / `pty.resize` / `pty.close` / `pty.list` | 保留（`list` 增 `attached_count`） |
| topic `pty.output` | **CUT** |
| topic `pty.screen` | **新增** |
| topic `pty.exit` | 保留 |

### 4.4 对账纪律

两侧各一条对账测试，断言**键集相等**而非包含 —— 解析只能证明超集，超发就住在那个缝里。handler 要**用契约类型构造响应**，不是构造 `json!` 再让契约去解析。

### 4.5 多客户端 resize（被迫回答的问题）

服务端持屏让"两个客户端看同一块屏"变成免费的，但**两个客户端视口不一样大时 PTY 只有一个尺寸**。

**取 min-size across attached viewports**（tmux 共享会话的既有约定；确定性、不抖动）。因此需要一张 per-session 的 attach 表（`conn_id → 视口尺寸`），断开即释放约束。非驱动方 letterbox。

这不是可选机件 —— 第二个客户端接上的那一刻必须有人回答这个问题。

### 4.6 这一段关掉的门

`seq` 让 PTY 从无状态广播变成有状态会话：`pty.*` 此后每加一个动词都要问"它动不动 seq"。attach 表把连接生命周期引进 PTY 子系统：客户端崩溃而 WS 未关时，其尺寸约束滞留到心跳超时。

---

## 5. 两道闸

### 5.1 闸 ① cwd jail

**真源是 `AgentEnvStore`**（`workspace.list/get` 那张表，operator 注册过的目录）。

> 为什么不是 `EXEC_WORKSPACE`：它是 `tokio::task_local`，属于 agent run 的上下文，而 `pty.spawn` 是 RPC handler，**结构性拿不到它**（同"工具面取不到 task-local"那一类）。
>
> 为什么 `AgentEnvStore` 是对的：CLAUDE.md 已判定 `workspace_path` 是运行时权力（成为 run 的 cwd），它的每个写入者已经是权限授予点。复用它是**同一套推导**，不是第二个答案。

规则：

1. 客户端给的 cwd **只是一个申请**；闸 `canonicalize` 后要求它落在某个已注册工作区内（或 `sandbox.workspace_root` 下）。
2. **两侧用同一个函数归一化**，只比规范形式。Windows 的 `\\?\C:\` 出线转换是部分的，两边各转一次会让 `starts_with` 从放行翻成拒绝。
3. **省略 cwd 时不许回落到守护进程 cwd**（一个缺省值如果回答的是另一个问题，它就不是缺省值，是谎话）。回落到 `AgentEnvStore` 的默认工作区；一个都没注册时**响亮拒绝并点名怎么办**。

### 5.2 闸 ② 会话开关

配置 section 的完整字段（**这是它的唯一声明处，别在别处再开一份**）：

```toml
[gateway.terminal]
enabled = true            # 闸 ②。默认开；关掉会杀掉在飞的会话
scrollback_lines = 1000   # §3.1 的服务端 scrollback 上限，每会话
max_sessions = 64         # 沿用 manager.rs::MAX_SESSIONS，本次搬到配置里
```

1. **spawn 时现读，不在 boot 快照**（对标 `read exec_tier live instead of snapshotting before the dispatch`）。
2. 声明进 `LIVE_SUBSECTIONS`（形状对标 `policies.spend`，因为 `[gateway]` 的 host/port 是 Restart 档），且**声明必须有真句柄背书** —— 恒真的声明等于没声明。
3. **关掉它要杀掉在飞的会话**，不只挡新 spawn。界限要在执行时刻成立；只在入队处判等于没判。

### 5.3 闸 ② 的写入面必须举卡

`self_config` 写 `gateway.terminal.enabled` 要触发 `src/tools/scoped/gate_chain.rs` 的 `DestructiveArguments`。闸的范围必须覆盖能把这个闸拿掉的那个动词，否则"两步都合法、合起来等价"就是绕闸路径。

### 5.4 问责（记录，不是闸）

`SessionInfo` 加 `created_by`（**人**，不只是身份 —— `ambient_actor()` 已能答这一问），spawn 时落审计记录。

单层信任模型下所有 operator 共享 `["*"]`，能互相看见并 attach 彼此的会话 —— **这是有意的，写出来而不是碰巧如此**。

### 5.5 措辞的三份拷贝

代码地板、`[gateway.terminal]` 的 doc、`self_config` 的 `DESCRIPTION` 必须同批改。最贵的那份是发给模型的。

### 5.6 这道闸买到的和没买到的

cwd jail 只管**起点**。终端内部的 `cd` 不受限（命令粒度在 PTY 上做不到）。

**它买到的是"起点可枚举、可审计"，不是"终端不能离开工作区"。** 这个界限必须如实写进 SECURITY.md，否则下一个人会把它当成隔离来引用。

---

## 6. Panel 设计

### 6.1 挂载点

新 nav 视图 `interfaces/webchat/src/platform/wide/views/terminal/`，与 `canvas/`（10 945 行）同级，后者可作范式参照。

**服务端持屏白送的第四项收益**：视图卸载/重挂是**无损**的 —— 离开导航停止渲染，回来 `pty.attach` 拿全量快照。所以订阅用 `subscribe_topic_ephemeral`（不进 ledger），生命周期跟视图走，**不需要 park/reveal 那一整套**。

### 6.2 布局模型

```rust
enum PaneNode {
    Leaf { pane_id: PaneId },
    Split { dir: SplitDir, first: Box<PaneNode>, second: Box<PaneNode>, ratio: f32 },
}

struct TermTab {
    id: TabId,
    title: String,                  // OSC 0/2 实时更新
    custom_title: Option<String>,   // 用户重命名后钉住
    pinned: bool,
    layout: PaneNode,
    active_pane: PaneId,
}
```

`pane_id → pty session_id` 一一映射。

### 6.3 持久化边界

**会话列表是服务端真源（`pty.list`），布局是每设备的（`localStorage`）。**

判据「多设备共享的事实不能住在 localStorage」问的是"这个值对第二台设备还成立吗"：
- 会话**成立** → 服务端
- 排布**不成立**（第二台屏幕尺寸不同）→ localStorage

新设备打开时从 `pty.list` 重建成每会话一个 leaf，所以不会出现"第二个成员进来什么也没找到"那类缺陷。

### 6.4 渲染

canvas2d 网格：
- 字体用 `index.html` 已加载的 JetBrains Mono；`measure_text` 量一次得 cell 尺寸。
- 按 `devicePixelRatio` 缩放（不做就是糊的）。
- 每帧只重绘脏行，同 SGR 的 run 合并成一次 `fill_text`。
- **web-sys features 预期零新增**（`CanvasRenderingContext2d` / `TextMetrics` 已在）。

### 6.5 两个 Panel 特有的坑（预先绕开）

1. **rAF 回调里不许 `NodeRef::get_untracked()`** —— 晚一帧执行足够组件卸载，unwrap 就是整页崩。测量收进一个私有函数，**只有一种拼法**（不是加一条只认得内联 `move ||` 的半盲守卫）。
2. **`<Show when=…>` 的守卫与 body 是两个反应式作用域** —— 别在 body 里 `expect("visible implies Some")`。单次读 + `Option` 视图。

### 6.6 输入

`keydown` → VT 字节编码（方向键 / 功能键 / Ctrl / Alt 组合）独立成 `keymap.rs`。

**IME 如实降级声明**：隐藏 `<textarea>` 承接 composition、`compositionend` 提交。够中文日常输入，**不声称与 Orca 等价**（Orca 为 IME 建了专门的 e2e workflow + ibus/hangul 脚本）。

### 6.7 选区 / 复制 / 搜索

全在客户端 —— 网格文本本来就在本地，不需要新 RPC。粘贴走 `pty.input{base64}`。

### 6.8 规模估算

| 层 | 新增 |
|---|---|
| Panel `views/terminal/`（layout / tabs / render / keymap / session / search） | ≈ 2200 行 |
| core `pty/screen.rs` + manager/session 改造 | ≈ 750 行 |
| `shared/protocol/src/pty.rs` | ≈ 200 行 |
| **合计** | **≈ 3150 行** |

### 6.9 这一段关掉的门

网格渲染器是自有的，`vim` / `htop` / `opencode` 这类 TUI 的正确性**没有 xterm.js 十年打磨兜底**。用真机 QA 装置顶住，但第一版一定有渲染差异要修 —— 这是选 D2 的确定代价，不是意外。

---

## 7. 实施顺序

### Phase 0 — 探针（必须第一步，无代码产出）

对一个真跑起来的 server **真发一次 `pty.spawn` / `pty.input`，看 `pty.output` 到不到**。

> 「零客户端有两种成因，处置相反，而分辨只要真发一次那个调用」——"没人需要它"是 CUT 候选，"**没有人可能用得了它**"是缺陷，而后者会伪装成前者（找不到客户端的原因恰恰是它不工作）。

可能改变后续所有阶段的发现：`attach_event_bus` 从没被调到、base64 帧格式对不上、admin 闸把 Panel 自己挡住等。

**产出**：一个结论，写进本文件的 §12。

### Phase 1 — 后端持屏

`pty/screen.rs`（vte + Grid + 备用屏 + scrollback）、16ms flush、`pty.screen` topic、CUT `pty.output`。

**第一件事是一个 20 行的局部实验验证 `vte 0.14` 的 `Perform` trait 形状**（不确定就先做小实验，不猜）。

core 测试环境强，这一层把断言吃饱。

### Phase 2 — 协议与重同步

`aleph_protocol::pty` + seq + attach 快照 + 在途缓冲重放契约 + 两侧**键集相等**对账。

### Phase 3 — 两道闸

cwd jail、会话开关、`gate_chain` 接线、审计、SECURITY.md 段落。

### Phase 4 — Panel 最小渲染

单 pane，无 tab 无分屏。**第一个端到端时刻，要尽快到达。**

### Phase 5 — Tab 条

create / close / rename / reorder / pin + OSC title 实时更新。

### Phase 6 — 分屏树

split h/v、ratio 拖拽、close-pane、布局序列化。

### Phase 7 — 搜索 / 选区 / 链接 + 真机装置

新建 `qa/terminal/run.sh`，逐动词带效果断言；vim / htop / opencode 三个硬考题各一条。

### Phase 8 — 文档

FEATURE_LOCATOR 新章 + 判据清单新条 + SECURITY.md + 子系统路由表。

**顺序理由**：后端先行是因为**它的测试环境强、Panel 的弱**；Panel 那半靠真机 QA 顶。

---

## 8. 风险与缓解

| 风险 | 缓解 |
|---|---|
| `vte 0.14` 的 `Perform` trait 形状未验证 | Phase 1 第一件事是 20 行局部实验，跑通再往下 |
| broadcast ring 仍可能 Lagged（64 会话 × 60fps ≈ 3840 事件/秒 对 1024 环） | 重同步已是地板，最坏表现是"卡一下然后正确"而非永久错位；Phase 1 实测该数并决定是否给 `pty.screen` 单独提高容量 |
| TUI 渲染正确性无 xterm.js 兜底 | `qa/terminal/run.sh` 逐动词效果断言 |
| IME 不与 Orca 等价 | 已如实降级声明 |
| 断而未关的客户端滞留 min-size 约束 | attach 表绑 WS 连接生命周期，心跳超时释放；`pty.list` 暴露 `attached_count` 便于诊断 |

### 8.1 worktree 环境的两个已知坑

- **worktree 里 submodule 是空的**，而 `skills/` `plugins/` 经 `include_dir!`（编译期宏）嵌入 ⇒ 目录缺失直接编译失败。实施前需 `git submodule update --init --recursive`。
- **fresh worktree 无 `node_modules`** ⇒ `just wasm` 死在 tailwind。需先在 `interfaces/webchat/` 跑一次 `npm install`。

---

## 9. 熵减清理清单

| 位置 | 动作 |
|---|---|
| `src/gateway/pty/session.rs` | 删除 `pty.output` 的 base64 编码与 publish 整段 |
| `src/gateway/event_scope.rs` | `pty.output` → `pty.screen` 的 topic 改名（两处：`default_rules` 与其孪生 pin 测试） |
| `src/gateway/handlers/pty.rs` | 模块 doc 里残留的 LAN-trust 表述随闸改写 |
| 全局 | 接线完成后跑一遍 severed-wire 扫描，确认没留下第二条半接的路 |

---

## 10. 验证集

```
# CLAUDE.md §10 的六条
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-{macos,windows,linux}
cargo clippy --workspace --all-targets          # 先 just _stage-shell-placeholders

# 本设计追加
cargo test -p aleph-panel --lib                 # 真跑，不是 --no-run；check 看不见它的测试模块
just wasm                                       # 唯一编译出厂形态的命令
qa/terminal/run.sh                              # 真机装置
```

---

## 11. 明确不做的（B 档划线之外）

park/reveal 保活调度 · OSC 52 剪贴板 · kitty 键盘协议 · agent 感知 tab 标题 · Quick Commands · worktree 绑定 · 退出原因分类 · WebGL 渲染 · ligatures · 终端原始字节旁路（YAGNI，见 §3.3）。

---

## 12. Phase 0 结果

**VERDICT: WORKS — 零客户端是"没人需要"（真实命令输出经 `pty.output` 送达，字节符合预期）**

`pty.*` 子系统本身没有缺陷：`connect` → `events.subscribe{topics:["pty.*"]}` →
`pty.spawn` → `pty.input` → `pty.output` 事件回传 → `pty.close` 全链路一次真实
调用全部走通，字节内容是真实 shell 的输出而非空响应或错误。零客户端的成因是
"没人接线"，不是"接不上线"——Task 1 之后的阶段划分**不需要重排**，可以按计划
把持屏/多路复用这类"锦上添花"留在后面阶段，先把 Panel 端 UI 接上这条已经工作的
后端线路。

### 探针环境（隔离，未碰操作者真实 `~/.aleph`）

按 `qa/lib/scratch_home.sh::qa_redirect_home` 的纪律，在
`/private/tmp/claude-502/.../scratchpad/pty-probe-qa` 下起了一个全新 `HOME`/
`ALEPH_HOME`（`cargo` 仍用真实 `HOME` 编译，避免 rustup 在 scratch 里重装一整套
工具链）。基线 commit `9b7feda4e`（`cargo build --bin aleph-server` 编译通过，
`Finished dev profile ... in 2m 50s`）。服务端绑定 `127.0.0.1:18790`（默认
host/port，未改配置）。探针脚本为一次性文件
（`pty_probe.py` + `pty_probe_run.sh`），未入库，符合 Task 0 交付物要求。

探针实现上有一处自我修正：brief 里给的示例脚本按"发一个请求、`recv()` 一次"的
顺序读取，而实测中 `connect` 之后网关会在同一个 socket 上插播一条不带 `id` 的
`presence.joined` 事件通知，把后续的顺序读位错位一格（把 `events.subscribe`
的响应错认成 `pty.spawn` 的响应，导致误判"`pty.spawn` 失败"）。最终版按
`id` 匹配响应、把没有匹配 `id` 的帧当通知单独处理，问题消失——这是探针本身的
bug，不是网关的 bug（记录在此，供 Task 1+ 的 Panel 端 WS 客户端代码参考：
读循环必须按 `id` 分发，不能假设响应严格按请求顺序单一到达）。

第一版探针在 spawn 后立刻写入，命中一次 zsh 提示符主题初始化与我们键入字节
之间的时序竞争（用户态 shell 还没跑完自己的启动重绘），产生的字节流可读但
不易人工确认"命令真的执行了"还是"只是键入回显"。加了 1.5s 结算延迟、并把
捕获窗口从"看到一次命中就停"改成"固定 6 秒窗口，去重复统计字符串出现次数
+ 用 `(?<!echo )ALEPH_PROBE_OK\r?\n` 排除掉键入回显那一次"之后，转录清晰、
可人工验证。

### 四问逐一回答（均为**直接观察**，非推断）

**1. `pty.spawn` 返回 session_id 了吗？**

是。原始响应（`connect`/`subscribe`/`spawn`/`input`/`close` 均按 JSON-RPC id
配对读取，非位置猜测）：

```
spawn -> {'id': 3, 'jsonrpc': '2.0', 'result': {'session_id': 'b6b03597-8b4b-41c1-8f52-8cc5ec4d073d', 'shell': '/bin/zsh'}, 'traceparent': '00-20a50e97456f4d5aa6a42de65ae82692-b19bc0b8506d4185-01'}
```

**2. `pty.output` 帧到达了吗？base64 解出来是不是预期字节？**

是，且内容是真实 shell 交互，不是空字节或垃圾。累计收到 662 字节
（多帧 `pty.output`，`topic` 与 `session_id` 逐帧核对匹配），完整解码后
（原始 repr，含控制字符）：

```
'\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m                                                                               \r \r\x1b]7;file://Mac-Mini-M4.local/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/74e2ecf9-7762-4147-9495-822b693479f1/scratchpad/pty-probe-qa/home\x07\r\x1b[0m\x1b[27m\x1b[24m\x1b[Jzouguojun@Mac-Mini-M4 ~ % \x1b[K\x1b[?2004he\x08echo ALEPH_PROBE_OK\x1b[?2004l\r\r\nALEPH_PROBE_OK\r\n\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m                                                                               \r \r\x1b]7;file://Mac-Mini-M4.local/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/74e2ecf9-7762-4147-9495-822b693479f1/scratchpad/pty-probe-qa/home\x07\r\x1b[0m\x1b[27m\x1b[24m\x1b[Jzouguojun@Mac-Mini-M4 ~ % \x1b[K\x1b[?2004h'
```

ANSI/OSC 转义剥离后的人读视图：

```
%                                                                                zouguojun@Mac-Mini-M4 ~ % eecho ALEPH_PROBE_OK
ALEPH_PROBE_OK
%                                                                                zouguojun@Mac-Mini-M4 ~ % 
```

这里 `ALEPH_PROBE_OK` 出现两次：第一次是键入回显（跟在 zsh 提示符与
`echo ` 之后，属于终端本地回显），第二次是**独立成行、不跟在 `echo ` 后面**
的一行——即 `echo` 命令真正执行后的 stdout，随后跟着一个全新的 zsh 提示符
`zouguojun@Mac-Mini-M4 ~ % `。这证明字节确实往返穿过了一个真实的子进程
（shell 执行了命令、打印了输出、打印了新提示符），而不是某种 loopback 或
echo-only 的假象。`\x1b]7;...file://.../pty-probe-qa/home\x07`（OSC 7 上报
cwd）也确认了这个 shell 的工作目录正是探针起的 scratch home，与
`SpawnOptions.cwd` 未指定时的默认行为一致。

**3. `attach_event_bus` 真被调到了吗？**

是——静态确认 + 动态确认双重印证。静态：`src/gateway/server/mod.rs:753` 在
`build_router()` 里无条件调用
`crate::gateway::pty::attach_event_bus(self.event_bus.clone())`，没有
feature flag 或配置开关包裹。动态：如果这条线没接上，`events.subscribe`
成功之后 `pty.output` 事件就不会经这个 event bus 广播到订阅连接——而探针
确实收到了帧（见问题 2），所以这条线在运行时真的通了，不只是"代码里写了
调用"。

**4. loopback 连接过得了 `"pty."` 的 admin 闸吗？**

是。`connect` 响应里 `role` 直接是 `"operator"`（loopback 免 token 自动
operator，符合 `src/gateway/CLAUDE.md` 记录的信任模型），随后
`pty.spawn`/`pty.input`/`pty.close` 三次调用全部返回 `result`（无
`INVALID_PARAMS`/权限错误），`events.subscribe{topics:["pty.*"]}` 也成功
订阅并收到了 `pty.` 前缀的事件——两个面（`method_admin::ADMIN_PREFIXES` 的
RPC 闸、`event_scope::EventScopeGuard::default_rules` 的事件闸）在 loopback
operator 身份下都放行了，与 `src/gateway/handlers/pty.rs` 模块 doc 描述的
"operator-only, on both faces"一致（loopback 连接本身就是 operator，所以
两道闸都天然通过，未额外测试"非 operator 连接被两道闸拒绝"这一负面路径——
该负面路径的正确性由既有单测覆盖，Phase 0 的目标只是确认正面路径可达）。

### 探针产物（未入库）

- `/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/74e2ecf9-7762-4147-9495-822b693479f1/scratchpad/pty_probe.py`
- `/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/74e2ecf9-7762-4147-9495-822b693479f1/scratchpad/pty_probe_run.sh`
- 服务端与探针完整日志：同目录 `pty-probe-qa/`（scratch `ALEPH_HOME`，含
  `server.log`）与 `final_run.log`
