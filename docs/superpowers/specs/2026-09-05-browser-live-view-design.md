# Browser Live View — 人看见 agent 的浏览器，并能随时接手（Chromium 先行，obscura 候补）

- **日期**：2026-09-05
- **分支**：main（单分支开发；实施时按 §6.5 的四步各开一个 worktree，每步一次真机 QA、一次合并）
- **Status: approved design 2026-09-05** —— 四节设计逐节经用户点头；下一步是 `writing-plans` 出实施计划。范围是**一份**实施计划的大小（四步交付顺序见 §6.5），不需要拆分。
- **承接**：FEATURE_LOCATOR §3.12（浏览器自动化，本 spec 是它的第七轮设计输入）与 §6.11（内嵌终端，本 spec 的视图面是它的孪生）。
- **证据**：两个丢弃式 spike 的读数与脚本在 [`2026-09-05-browser-live-view-evidence/`](2026-09-05-browser-live-view-evidence/)（§10）。
- **参考项目**：Codex CLI（`/Volumes/TBU4/Github/codex`，闭源桌面版的协议痕迹）· obscura（`/Volumes/TBU4/Github/obscura`）· Skyvern · steel-browser（只借设计，不搬代码；许可证未查，见 §9）。

---

## 0. 决策记录

用户在对话中按此顺序裁定，后面的设计不重开这些问题：

| # | 裁定 | 用户原话要点 |
|---|---|---|
| D1 | 首要场景是 **③ 旁观并随时介入 agent 的浏览**；① 登录接手、② 前端开发指元素、④ 服务器只读引擎都不是第一版的目标 | 「3 是最重要的场景」「可视化是为人类设计的，尤其是随时接入，与 agent 更好的交互」 |
| D2 | 接管语义取 **(a) 显式接管/交回 + 交回时给 agent 一份介入摘要** | 「按 (a) 加介入摘要定」 |
| D3 | **Chromium 先行，obscura 候补**；后又收紧为本轮**不做 obscura 设计** | 「obscura 对比 chromium 除了体积和内存使用小一点外，也没多大优势……把力量集中到 chromium」 |
| D4 | Chromium **不进 Aleph 安装包**，作为外部运行时在安装时下载 | 「通过外部运行时在 Aleph 安装时下载安装」 |
| D5 | 浏览器视图**放在 Panel 右侧工作区栏（`WorkspacePanel`）里、与当前对话联动**，不建独立的浏览器 tab | 「放在 panel 右侧工作区栏，和对话窗口联动，而不是单独建立浏览器 tab，从而在操作上产生割裂感」 |

---

## 1. 背景与价值

Aleph 今天的浏览器子系统（FEATURE_LOCATOR §3.12）让 agent 经 26 个工具驱动一个无头 Chromium（`BrowserDriver::Managed`，`src/browser/profile.rs:24-30`）或附着到用户自己的 Chrome（`ExistingSession`）。人看不见 agent 在做什么，也无法在同一个会话里插手。本 spec 补的就是这两件事。

**三个价值场景**，共同前提是**人和 agent 操作同一个浏览器会话**：

1. **旁观与介入**（D1 首要）：看着 agent 浏览，觉得不对就接手，做完交回，agent 知道发生了什么。
2. **登录/验证码接手**：agent 卡住 → 请人 → 人在同一会话里登录 → 交回。这一条由 §3.3 的 `browser_control{request}` 顺手覆盖。
3. **指着元素说话**：人点页面上的东西，agent 拿到一份能直接消费的元素描述（§5）。

**为什么不是壳里嵌一个原生 WebView**（Codex 桌面版看起来的做法）：
- 会话不共享——人在 WKWebView 里登录，agent 的 Chromium 是另一个 cookie jar，场景 1、2 直接失效。
- 要让 agent 也能驱动那个 WebView，就得在 JS 注入之上再造一个 backend（没有网络拦截、没有 dialog、没有下载），是禁用清单里「第二个 VT」那一类。
- 只有桌面 App 有，违 R6 一核多端。
- 而且 Aleph 的壳是 Tauri：macOS WKWebView、Linux WebKitGTK 都不说 CDP，「像 Codex 那样嵌」在 Aleph 不存在，除非换壳。

所以可视面只能是**远程帧缓冲视图**：服务端把 agent 那个浏览器的画面推给 Panel，Panel 把鼠标键盘送回去。这与内嵌终端（§6.11：服务端 VT 出帧 → `pty.screen` → Panel canvas → `pty.input`）是同一个形状。

**Codex CLI 的证据**（`/Volumes/TBU4/Github/codex` @ `28327355b8`，2026-08-30）：核心里零浏览器代码；`features/src/lib.rs` 有三道独立的门 `in_app_browser`（人用的面板）、`browser_use`（agent 集成，副门 `browser_use_full_cdp_access` / `browser_use_external`）、`computer_use`；agent 那半是名为 `node_repl`/`cua_repl` 的 MCP actor server（`protocol/src/mcp.rs:38`），core 只往请求 `_meta` 塞 confirmation-policy markdown 并把返回截图收为 Guardian 审查证据；策略按 origin 分 `access / downloads / uploads / full_cdp_access / auto_review / persistent_approval / access_approval_lifetime(turn|thread)`。「面板与 agent 共用同一个 Chromium」是**推断**（依据是给面板配的 `full_cdp_access` 与「导入外部浏览器设置」），不是读到的。借它的：两轴拆分（人的面板 vs agent 集成）、共享会话、按 origin 的策略词汇。不借的：REPL 执行模型与 Guardian（Aleph 的补偿是六道确定性咽喉 + 审批门）。

---

## 2. 证据

### 2.1 obscura spike（发布版 v0.2.1，2026-08-23；本地源码 `72c84ad`，2026-09-04）

| 项 | 读数 |
|---|---|
| 跨连接 | **target 按 CDP 连接隔离**（`CdpContext` 每 socket 一份）：第二条连接收到 `Target.targetCreated` 广播，但 `getTargets` 为 `[]`、`attachToTarget` 报 `Target not found`，先连 / `setDiscoverTargets` 都无效 |
| 单 V8 isolate | github.com 上 `Runtime.evaluate("1")` 往返 **16.8 s**（加载后）/ **12.4 s**（静置 8 s 后）；AX 树本身拿到锁后 18–39 ms |
| screencast（单连接，1280×800 JPEG q60） | 静态页 3 s **0 帧**；动画页约 31 fps；28–33 KB/帧；点击→下一帧 28/13/13/14/13 ms；连续三次 `Page.navigate` 后仍出帧 |
| 输入 | `keyDown`+text 出 ASCII；`char` 事件插入中文（`ab中`）；Backspace 需 `rawKeyDown`+`windowsVirtualKeyCode=8`；`mouseWheel` 使 scrollY=600；`Input.insertText` 发布版**不存在**（源码 `e4814b4` 2026-09-04 已加）；无 `Page.javascriptDialogOpening`、无拖拽事件 |
| AX 树 | 每节点带 `backendDOMNodeId`、`ignored=0`；HN 1305 / GitHub 5551 / Wikipedia 4004 节点；**可访问名不从内容计算**：HN 229 个 link 零个有 name |
| 命中测试 | `elementFromPoint`、`DOM.querySelector`+`getBoxModel` 可用 |
| 私网地板 | 不带 `--allow-private-network` 时 `Page.navigate` 到 127.0.0.1 报 `Access to private/internal IP address 127.0.0.1 is not allowed`，页面停在 about:blank；flag 是进程级 |
| `Runtime.evaluate` | 只收表达式：`1; 2` 报语法错；IIFE / 逗号表达式 / `awaitPromise` 正常 |
| 保真度 | HN、Wikipedia 接近 Chrome；GitHub 有导航标签重影、幻影 tooltip、提交列空、README 未画 |
| 其它 | 非 stealth 构建也自报 `Chrome/145` + Linux UA；二进制 94 MB；内存未量 |

注意：本机走 fake-ip TUN 代理（`github.com → 198.18.0.9`），导航耗时受网络主导，不可归因；`evaluate("1")` 的等待纯粹是锁。发布版与源码相差 12 天、差出一个 `insertText`。

### 2.2 Chrome spike（Google Chrome 152 无头；playwright-cli 0.1.8 / playwright-core 1.60.0-alpha）

| 项 | 读数 |
|---|---|
| 多客户端 | 第二、第三条连接都看见并附着到第一条连接正在驱动的页面（7 个 target 含它），无人被踢 |
| 旁观 screencast | 静置页 **0 帧/3 s**（example.com 上连续 16 s 零帧）；动画页 60 fps；10.3–10.4 KB/帧（HN 16.5 KB，导航中一帧 89 KB）；同源与跨源导航都不断流、同一 session id、无 `detachedFromTarget` |
| 延迟（静置页，按需出帧） | 驱动方点击→旁观者收帧 **11–16 ms**；旁观者点击→旁观者收帧 **13–19 ms** |
| 干扰 | 旁观者拉 60 fps 时驱动方 `Runtime.evaluate` 往返 **0 ms ×5**（两次全量跑分别 2 ms、1 ms） |
| 旁观者注入 | 点击把页面计数 5→6；`Input.insertText` 落入 `中文 hello`；`mouseWheel` 使 scrollY=600；旁观者断开后驱动方照常 |
| playwright-cli 起的 Chrome | argv 里**已有**随机 `--remote-debugging-port=<n>` 与 `--remote-debugging-pipe` 并存；用户给的 `=0` 被它自己的覆盖（last wins）；**不写** `DevToolsActivePort`；`playwright-cli list` 不打印端点——只能刮 `ps` |
| `cdpEndpoint` 接入 | `open --config {"browser":{"cdpEndpoint":"http://…"}}` 被接受、复用现有页面，但 `open` 会对该页 `goto('about:blank')`；**`attach --cdp`**（http 与 ws 两种形式）不清页 |
| 生命周期 | `cdpEndpoint` 下 `playwright-cli close` 后 9 个 Chrome 进程原样、端点仍服务、页面仍在 example.com；重新 `attach` 找回同一页；两个 CLI session 可共用一个 Chrome；外部连接把页面导航走之后 `tab-list` 仍正确 |
| AX 名字 | `Accessibility.getFullAXTree` 给 229 个 link 中 198 个命名 |
| 内存 | 8 个进程 RSS 求和 **1175 MB**——macOS 对共享页重复计数，只能当**上界**；「一个 tab 八九个进程」是真的 |

注意：延迟是 CDP 事件延迟，不是 glass-to-glass，不含编码与传到 Panel 的那段；帧只计数未解码。

### 2.3 参考实现的教训（Skyvern · steel-browser）

搜索了 openclaw / hermes-agent / pi-mono 三个生态：全是「接用户自己 Chrome」的 relay 扩展或「容器里 Chromium + noVNC」，没有一家做过自家 UI 里的实时视图加接管。做过的是 Skyvern（VNC → CDP screencast 迁移，留有计划文档与实测）和 steel-browser（纯 CDP cast）。借来的判据：

- 旁观连接**必须独立且不带任何拦截**：Skyvern 复用驱动方状态后给 agent 正在操作的页面装上了第二个下载拦截器。
- 接管是显式的 `take-control` / `cede-control`，服务端维护 `interactor ∈ {agent, user}`，非持有者的输入**丢弃**。
- 只放一个「接管」按钮时，五分之一用户在画面上点了约 470 次无效点击，于是改成「画面上点任意处即接管」。
- 人驱动的导航过与 agent 同一道闸，且**重定向链复检**、**后退/前进重放的历史条目也复检**（被拦在半路的重定向会留在 back 栈里）。
- 非打印键要带 `windowsVirtualKeyCode` 配 `rawKeyDown`，否则是 no-op（本仓 spike 在 Backspace 上复现）。
- 流的生命周期：终态集合、指数退避、「重连超过几次还挂着旧帧就是在撒谎」（即判据 §17）。
- steel 的视图是 iframe，不合 R2；只借它的服务端形状（按 tab 各开一路 cast、tab 列表发现、心跳）。

许可证未查（记忆里 Skyvern 是 AGPL、steel 是 Apache-2.0）——**只借设计，不搬代码**。

---

## 3. 设计第一节：driver 边界与接管状态机

### 3.1 谁起 Chrome、谁接入

**Aleph 起 Chromium，playwright-cli 经 `attach --cdp` 接入。** 理由是 §2.2：playwright-cli 起的 Chrome 虽有随机调试口，但不可钉、不可发现、不写 `DevToolsActivePort`，那不是契约。

- `Managed` 这个 driver 名与 profile 配置**不变**（`src/browser/profile.rs:24-30`、`:34-107`），变的是启动链：`SessionLaunch` 的五个字段（`src/browser/playwright_launch.rs:35-41`：headless / browser / user_data_dir / proxy / extra_args）改为拼进 Aleph 自己 spawn 的 Chrome argv，外加 `--remote-debugging-port=0`；端点从 `<user_data_dir>/DevToolsActivePort` 读出，带 deadline 轮询。二进制由 §6.1 的顺序解析（`src/browser/discovery.rs:125` `find_chromium_preferred` 已在）。
- `launch_config_json`（`playwright_launch.rs:178-200`）只剩 `outputDir` 与 `allowUnrestrictedFileAccess` 两键；`open_argv`（`:220-234`）改为 `attach_argv`。旧的 CLI 启动路径**删除**，不留第二个答案（P6）。今天 `cdpEndpoint` 只在 `:243` 的注释里出现过，从未被写过。
- **生命周期归 Aleph**（§2.2）：`close` 只是断开。`reap_idle`（`src/browser/manager.rs:320`）对 Managed 改为终结 Aleph 的 Chrome 子进程；playwright-cli 崩了或被收割只需重新 `attach`——现有「CLI 自己说没开才惰性开」的咽喉改成惰性 attach。
- 一定用 `attach` 不用 `open`：`open` 会对复用的页面 `goto('about:blank')`。

### 3.2 视图 = 第二条连接

Aleph 内一个只盖十几个方法的 CDP 客户端（`tokio-tungstenite 0.26` 已在工作区依赖），作为**旁观者**接同一个端点，flatten 附着到活动 tab 的 target。它只做：`Page.enable`、screencast 三件（`startScreencast` / `screencastFrameAck` / `stopScreencast`）、`Input.dispatch{Mouse,Key}Event` + `Input.insertText`、人的导航三件（`Page.navigate` / `navigateToHistoryEntry` / `reload`）、`Target.getTargets` 拿 tab 条、监听 `Page.frameNavigated`。**永远不开 `Fetch` / `Network` 拦截**（§2.3 第一条）。

它住在 `src/browser/live/`，**不进 `BrowserBackend` trait**（`src/browser/backend.rs:14`）——视图不是工具能力，是网关面；trait 与 `FakeBackend` census 零改动。`ProfileManager` 只多一个 `live_endpoint(profile)`。视图**只对 Managed profile 存在**：`ExistingSession` 是用户自己的 Chrome，人本来就看得见。

### 3.3 接管状态机

状态挂在 **profile** 上而不是会话上（一个 profile 可被多个会话驱动），住在 `ProfileManager` 进程内存里——它是一张**租约**，持有者是一条活着的 Panel 连接，连接死则租约死，这正是它该在内存里的理由（判据 §15 针对的是耐久事实，租约不是）。

| 状态 | 进入 | 离开 |
|---|---|---|
| `Agent`（默认） | 启动；`cede`；持有者的 WS 断开 | `take` |
| `Human{holder, since, tab}` | `take`（operator 门控 RPC） | `cede` / 持有者断开 |

- **对 agent 的效果**：闸放在 `get_backend`（`src/browser/manager.rs:284-303`）——所有工具和收割器都经过它（每次调用现建 backend）。`Human` 态下返回新的结构化错误 `BrowserError::HumanHasControl{since}`，经 `backend_error_text` 咽喉（`src/builtin_tools/browser_tools/mod.rs:148-157`）渲染成模型能读的一句：「人从 T 起接管了 profile X，等待交回或在对话里问」。模型看见并自愈（A2），harness 不替它挑策略（R10 第 5 不）。收割器（`manager.rs:422-469`）被同一道闸挡住是对的：人在操作时不该有 tab 被回收。
- **对人的效果**：`Agent` 态下人的输入**丢弃**（§4.1 的 `input` 回执会说丢了几条），画面上点任意处 = `take`（§2.3 第三条），且那一下不转发。多个 Panel 可同时看，只一个能持有，其余看到「X 正在操作」。
- **新工具 `browser_control`**，三个动作：`status` / `wait`（阻塞到回到 `Agent`，受 180 s 工具预算）/ `request{reason}`（agent 请人接手，Panel 在画面上弹提示）。第三个动作把场景 2 顺手做了。它不常驻——落在 `BROWSER_RESIDENT_CORE`（`src/config/types/policies/session_mode.rs:163`）之外的延迟分区；不需要审批，所以不进 `approval_wiring_census`（`browser_tools/mod.rs:965`）。
- **介入摘要**：`Human` 期间 driver 记录顶帧导航 URL 序列、输入计数、起止时间、最终 URL 与标题；`cede` 时**一份**摘要两处消费——`browser.live.state` 事件给 Panel 显示，以及作为前缀塞进该 profile **下一次**工具结果里，保证模型一定读到，并明写「T 之前的 refs 已失效，重拍 snapshot」。给模型的那份经 `redact_and_wrap`（`browser_tools/mod.rs:366-383`）。

### 3.4 人驱动的导航过同一道闸

地址栏与前进/后退/刷新在派发前走 `check_navigation`（`src/browser/manager.rs:511-513`；历史条目也复检，§2.3 第四条）；页内点击引发的导航无法预检，由旁观连接的 `frameNavigated` 触发**现有的** `post_nav` 审计，违规则同样隔离到 about:blank 并通知视图。agent 和人共用同一份推导（判据 §9）。

### 3.5 门控与安全

`browser.live.` 加进 `ADMIN_PREFIXES`（`src/gateway/method_admin.rs:101`，`"pty."` 在 `:259` 是先例）与 `EventScopeGuard` 的 admin 前缀（`src/gateway/event_scope.rs:24-31`），姿态与 `pty.` 一致：浏览器 profile 是宿主机资源，member 在 v1 什么都看不到。调试口绑 loopback 但无认证，本机任何进程都能驱动它——今天 playwright-cli 起的 Chrome 已经如此（§2.2），不是回归，但要写进 SECURITY.md。

### 3.6 这一节让什么变难了

- Aleph 从此在三个平台上**拥有 Chrome 进程**：退出时杀、崩溃后按 user_data_dir 标记收孤儿、boot 时清上次的残留。`src/sandbox` 有子进程管理的先例可循，但 Windows 那半没人跑过。
- Managed 的八场景真机 QA（`qa/browser_managed/run.sh`）必须整套重跑；`playwright_cli.rs` 里「没开」的错误分类锚点会随 `open`→`attach` 变化——FEATURE_LOCATOR 附录 D.9.13 记录过的形状。
- `attach` 是否接受 `--config`（`outputDir` 还要不要得到）spike 没测，实施第一步先验（§9）。

---

## 4. 设计第二节：帧与输入协议、Panel 视图

### 4.1 线上契约：`shared/protocol/src/browser_live.rs`

照 `pty.rs` 的纪律（`shared/protocol/src/pty.rs:1-6`）：两端同一个 crate，服务端**用这些类型构造**响应，配一条 key-set 相等测试；服务端注册时用字面量而不是常量（`pty.rs:19-27` 说明了 `method_census` 只认字面量）。前缀 `browser.live.`。

**两个事件主题，刻意分开**：帧只有像素，其余状态另走一路。

- `browser.live.frame`：`{profile, tab, seq, width, height, format:"jpeg", data:base64}`。`seq` 按 profile 单调；客户端见 `seq != last+1` 即重附着（总线是有界广播，落后者会丢，同 `pty.rs:136-138`）。
- `browser.live.state`：`{profile, state_seq, control:{mode: Agent|Human, holder, since}, tabs:[{id,url,title,active}], viewport:{w,h}, intervention: Option<摘要>, request: Option<{reason, since}>}`。**每次都发整份**而不是补丁——它很小，而 `Option` 的「没变」与「不知道」在 pty 那边已证明是两个坑。`intervention` 只在 cede 那一发上带值。
- `browser.live.closed`：`{profile, reason}`，Chrome 死了或被收割。

**RPC**，全部 operator 门控（§3.5）：

| 方法 | 入参 | 回 | 要持有租约 |
|---|---|---|---|
| `browser.live.attach` | `{profile}` | `{viewer_id, state, frame: Option}`——一次调用同时给状态和当前帧，没有「先拿状态再等第一帧」的窗口（`PtyAttachResponse` 的理由，`pty.rs:159-165`） | 否 |
| `browser.live.detach` | `{profile}` | ok | 否 |
| `browser.live.input` | `{profile, tab, events:[…]}` | `{accepted, dropped, reason}`——非持有者的输入被丢，但**丢了要说** | 是 |
| `browser.live.control` | `{profile, action: Take\|Cede}` | state；`Take` 撞上别人持有 → 显式错误 `held_by` | — |
| `browser.live.navigate` | `{profile, tab, action: Url\|Back\|Forward\|Reload}` | `Ok \| Blocked{reason}`，先过 `check_navigation` | 是 |
| `browser.live.tabs` | `{profile, action: Select\|New\|Close}` | state | 是 |
| `browser.live.hit_test` / `browser.live.pick` | 见 §5 | 见 §5 | 否 |

`InputEvent` 是带标签的枚举：`MouseMove / MouseDown / MouseUp {x, y, button, click_count, modifiers}`、`Wheel {x, y, dx, dy, modifiers}`、`KeyDown / KeyUp {key, code, vk, text, modifiers}`、`InsertText {text}`（IME commit 与粘贴都走它，§2.2 证明中文经它落地）。坐标一律是 **viewport CSS 像素**。

**viewport 归 profile，不归 viewer**：agent 的截图与 snapshot 依赖它，只有 `browser_resize` 工具能改；viewer 只做 letterbox 缩放并把坐标映射回去。这样就没有 pty 那种「最小者胜」的多客户端尺寸协商（`pty.rs:143-150`）。同理**一个 profile 一路 screencast、始终对着活动 tab**，`Select` 就是 `switch_tab` 的人类脸，所以要租约；活动 tab 的唯一真源仍是 `tab_registry::active_tab_id`（`src/browser/tab_registry.rs:263-291`）。

### 4.2 服务端：`src/browser/live/`

- `cdp.rs`：极小 CDP 客户端，`call` 带独立 deadline、事件回调、flatten session。只此一处。
- `session.rs`：每 profile 一个 `LiveSession`——持有旁观连接、screencast 引用计数（首个 viewer 开、末个关）、帧 `seq`、状态、租约、介入记录器、输入派发。收到 `screencastFrame` → 发布 → 立刻 ack，永不因为 viewer 慢而卡住 Chrome。
- 发布走 pty 同一条线：`TopicEvent::new(BROWSER_LIVE_FRAME_TOPIC, …)` → `GatewayEventBus::publish`（`src/gateway/event_bus.rs:464`），`attach_event_bus` 在 `build_router` 挂一次（同 `src/gateway/pty/manager.rs:402`、发布点 `:774-775`）。
- 过滤只有**角色一项**：`EventScopeGuard` 给 `browser.live.` 配 admin。**不做** pty 那种按创建者的归属收窄（`src/gateway/event_visibility.rs:498-499` 的 `ByPtySession`）——profile 是宿主机共享资源，任何 operator 都在用同一个浏览器，工具面已经如此，视图面不该更严。这一点与 pty 明写不同。
- 帧率与画质是**全局配置**（§6.3），经 `startScreencast` 的 `everyNthFrame` / `maxWidth` 交给 Chrome 自己降；viewer 不协商。手机端带宽不是 v1。
- `frameNavigated` 在此处接 `post_nav` 审计（§3.4）。
- 给模型的介入摘要经 `redact_and_wrap`（URL 里可能带 token）；给 Panel 的 state 不脱敏——那是 operator 自己的浏览器地址栏。
- 处理器放 `src/gateway/handlers/browser_live.rs`，在 `src/gateway/handlers/mod.rs:436-440` 那种 `registry.register("<literal>", fn)` 的位置注册。

### 4.3 Panel：`interfaces/webchat/src/platform/wide/views/browser/`，终端视图的孪生

- `mod.rs`：挂载时**先** `subscribe_topic_ephemeral` 两个主题**再** `attach`（终端视图 `mod.rs:316` 那条「先订阅再列表」的教训），`seq` 断档 → 重新 `attach` 拿新帧。
- `render.rs`：canvas（同终端 `render.rs:9`、`:245`）；JPEG 交给浏览器原生解码（`Blob → createImageBitmap → drawImage`），不在 WASM 里解码。
- `input.rs`：照 Skyvern 的映射搬设计不搬代码——letterbox 坐标换算、修饰键位掩码、非打印键必须带 `windowsVirtualKeyCode`、滚轮累积与 rAF 合并、鼠标移动合并、`compositionend` / 粘贴 → `InsertText`。键盘只在容器聚焦时捕获，监听器挂元素不挂 window（Leptos 0.8 `window_event_listener` 不注册清理，记忆 `project-providers-preset-picker` / `project-channel-reachability-and-phone-i18n`）。
- `chrome.rs`：地址栏（仅持有时可编辑）、前进/后退/刷新、tab 条（来自 state）、持有者横幅、「点击画面任意处即接管」覆盖层、介入摘要提示、agent 的 `request` 提示。
- 位置（D5）：**不是独立视图，是右侧工作区栏的一个体**。宽屏聊天面已有 `WorkspacePanel`（`interfaces/webchat/src/components/workspace_panel.rs:1-21`）：`LayoutMode::Split` 时在右侧打开、聊天面让出约 40% 宽度（`views/chat/view.rs:189-200`、`:256-257`）；单 agent 模式的体是 `ArtifactsSurface`，team 模式是 deliverables / tasks 页签；`WorkspaceState`（`state/layout.rs:81`）已经记录每次工具调用的参数与结果（`record_tool_args` `:183`、`get_tool_payload` `:202`）。浏览器视图作为这个面板的又一个体接入：单 agent 模式加一个 Artifacts / Browser 的体切换，team 模式加一个 Browser 页签。上面四个文件的实现不变，只是挂载点从「并列视图」改为工作区体。手机端不做。
- 与对话联动的五条规则（在对话里只定了「联动」二字，以下细则是**待用户确认的默认值**，plan 3 执行前须过目）：
  - L1 **profile 跟随会话**：视图显示的是**当前会话的 agent 正在用的 profile**，取自 `WorkspaceState` 里记录的最近一次 `browser_*` 调用参数（缺省 `default`），不做全局 profile 选择器。
  - L2 **自动出现**：当前会话的 run 首次调用 `browser_*` 且该 profile 有活会话（`session_active`）时，若布局是 `ChatOnly` 则切到 `Split` 并选中 Browser 体；用户手动收起后本 run 内不再自动弹出。agent 的 `browser_control{request}` **必弹**并高亮提示。
  - L3 **transcript 可达**：聊天记录里的 `browser_*` 工具行可点击，点击 = 打开 Browser 体（不做「回到当时那一帧」）。
  - L4 **选取器产物落在当前会话的 composer**（§5.1 的芯片），不落到别的会话。
  - L5 **介入摘要同时是当前会话 transcript 里的一条系统行**（Panel 从 `browser.live.state` 的 `intervention` 渲染），与工具结果里那份同源同文。
  - 非 operator 的用户：`WorkspacePanel` 不提供 Browser 体（同 `components/admin_refusal.rs` 的口径），而不是提供后再被 RPC 拒绝。

### 4.4 变难的与不做的

- 剪贴板只做粘贴进（`InsertText`），不做从页面拷出（steel 用 iframe postMessage 桥，v1 不要）。
- 多 viewer 各看不同 tab：不做，一路 screencast。
- headed profile 也能挂视图，但人的物理输入与注入输入交错未测（§9），文档写明 v1 只对 headless 承诺。
- 帧是页面内容，敏感度等同 cookie；admin-only 之外没有更多防线，像素无法脱敏。

---

## 5. 设计第三节：元素选取器与「选中元素」附件

模型今天只能用两种方式指认元素：`ActionTarget::Ref{ref_id}` 与 `ActionTarget::Coordinates{x,y}`（`src/browser/types.rs:10-15`），没有 selector 臂。所以选取器的产物要同时喂两种消费者：「在页面上动它」靠坐标与角色名，「在代码里改它」靠 selector、HTML 与源码位置。

### 5.1 交互：选取不需要租约

选取是只读的命中测试，不改页面——旁观者也能指。Panel 视图加一个「选取」模式：

- 进入后，鼠标移动经 rAF 节流调 `browser.live.hit_test {profile, tab, x, y}` → `{box, role, name, tag}`，Panel 在 canvas 覆盖层画框和标签（本机往返十几毫秒，§2.2）。
- 点击调 `browser.live.pick {profile, tab, x, y}` → 完整描述符 + 元素裁剪图；**这一下不转发为输入**，选取模式吞掉所有鼠标事件。
- 描述符回到 Panel 后插进聊天输入框，显示成一枚芯片「选中元素：button "Submit"」，用户接着打字。可以连点几次，几枚芯片。

### 5.2 服务端：`src/browser/live/pick.rs`，走旁观连接

Chrome 这一侧原语齐全：`DOM.getNodeForLocation` → `backendNodeId`；`DOM.describeNode` 拿标签与属性；`Accessibility.getPartialAXTree` 拿 role 与 name；`DOM.getBoxModel` 拿包围盒；`DOM.getOuterHTML` 拿外层 HTML；`Runtime.callFunctionOn` 在节点上生成唯一 selector（优先 `#id` / `[data-testid]` / `[name]` / `[aria-label]`，再 role+文本，最后 nth-child 路径；**不用**其它带值的属性，那里可能有 token）；`Page.captureScreenshot{clip}` 裁一张元素图。

**源码位置是可选的加分项**：在节点上尽力读 `data-source-loc`、React fiber 的 `_debugSource`、Vue 的组件文件——本地 dev server 的页面常带这些。读不到就没有这个字段，不猜。

跨源 iframe：v1 顶层文档内解析；命中的是 OOPIF 的 `<iframe>` 元素时描述符只说「位于 iframe <src>」。Playwright 的 snapshot 能穿透 OOPIF（§3.12 第五轮 ⑧），选取器暂不。

### 5.3 附件形状：沿用扁平 `Attachment`，加一个自有 mime

现状：RPC 侧 `Attachment{name, mime_type, data}`（`src/gateway/handlers/agent.rs:24-31`，`chat.rs:47-49` 的 `SendParams.attachments`），无标签枚举；`shared/protocol` 的瘦客户端类型刻意不含附件（`shared/protocol/src/session_thread.rs:97-100`）；`agent.rs:1016-1021` 把它转成通道侧的第二个 `Attachment` 类型交给媒体处理器。两条路里选**复用**：

- `mime_type = "application/vnd.aleph.browser-element+json"`，`data` 是 `SelectedElement` 的 JSON。类型定义在 `shared/protocol/src/browser_live.rs`（`pick` 的响应就是它），Panel 只原样回传，不手写 JSON；服务端在 `agent.rs:1016-1021` 那个转换点识别这个 mime，**从同一个类型渲染**成给模型的文本块。一份形状，两个消费者（判据 §1）。
- 元素裁剪图作为第二个 `image/png` 附件，走现有的图片路径进视觉块，零新代码。
- 不引入带标签枚举：那是横跨 `chat.rs`、`agent.rs`、`gateway::channel` 的破坏性改动，而收益只是一个变体。

给模型的文本块：

```
<selected_element url="…" title="…" viewport="1280x800">
role=button name="Submit" tag=button#submit.btn
center=(412,377) box=(380,360,64,34)
selector=#submit
text="Submit"
source=src/components/Form.tsx:42        # 仅在读到时出现
html=<button id="submit" class="btn">Submit</button>   # 有界、脱敏、围栏
</selected_element>
```

`center` 让模型**现在**就能用 `Coordinates` 点它；`role+name` 让它重拍 snapshot 后按名字找到 ref；`selector / html / source` 是给改代码用的。「早先的 refs 可能失效」这句只写进渲染器一次，不进 system prompt（R9 第二把尺）。

### 5.4 出口纪律

外层 HTML 与文本经现有 `redact_wrap`（`browser_tools/mod.rs:340-343`：脱敏 + `ContentSource::BrowserContent` 围栏），上限 1 KB；selector 生成器不碰带值属性；裁剪图走图片附件的既有体积预算。

### 5.5 不做的

- 选取器不产 ref：ref 是 Playwright 每次 snapshot 的产物，服务端算不出模型手里那一份。
- 不做 OOPIF 内部选取、不做多选框选、不做「选中后自动生成修改 prompt」——用户自己打字。
- agent 正在 `browser_exec` 中途时也允许选取，但页面可能在你指的时候变了；描述符带 URL，模型照例重拍。

---

## 6. 设计第四节：运行时供给、错误处理与降级、配置、QA、交付顺序

### 6.1 Chromium 作为外部运行时（D4）

playwright-cli 本身就不在安装包里，是运行时台账经 fnm 装的 Node 拉下来的（`src/browser/playwright_cli.rs:126` `ensure_capability("playwright-cli", …)`）。Chromium 走同一张台账，三产物保持 Chromium-free。

- **供给器用 Playwright 自己的**：`playwright install chromium`。它下载的是与 playwright-core 钉在一起的 Chromium 构建，版本确定，落在 `~/.cache/ms-playwright/`。不自写下载器（Chrome for Testing 的 JSON API 是可选源，但那是 core 里的第二个下载器，不要）。
- **二进制解析顺序**：配置里的 `binary_path` 显式钉住 > 系统已装的 Chromium 系浏览器（`src/browser/discovery.rs:125` 现有的 Chrome / Chromium / Brave / Edge 顺序）> 台账下载的 Playwright Chromium。**有系统浏览器就不下载**——Windows 几乎必有 Edge，macOS 多半有 Chrome；只有干净的 Linux 服务器真会走到下载。§2.2 用系统 Chrome 152 配 playwright-core 1.60 跑通，跨版本不是问题。
- **时机**：安装时尝试一次，失败不阻塞安装；首次用到 Managed profile 时再试一次；仍然没有就 fail-closed——工具返回「Chromium 未安装，运行 X 或让我安装」，doctor 加 `browser/chromium-missing` 哨兵，并按 R8 暴露成工具（`runtime_install{chromium}`），让模型能在对话里完成安装。
- **镜像**：Playwright 的下载走它的 CDN，在国内网络下会像 GitHub release 资产一样被挡。台账透传 `PLAYWRIGHT_DOWNLOAD_HOST`（npmmirror 有对应镜像），作为配置项而不是让人去 export。
- **Linux 系统依赖**：`--with-deps` 要 root；没有 root 时用 `chromium-headless-shell`（依赖少、体积小）作为只无头的降级选项。

### 6.2 错误处理与降级

每一条都要答「模型/人看到什么」，不能静默：

| 故障 | 处置 | 谁看见 |
|---|---|---|
| 端口文件超时没出现 | 杀子进程，`BrowserError::LaunchFailed{stage:"devtools-port"}` | 工具错误 + 日志 |
| Chrome 中途死 | `browser.live.closed{reason}`；租约作废回 `Agent`；下次工具调用惰性重启 | Panel 横幅 + 工具错误 |
| playwright-cli 崩 / 被收割 | 惰性 `attach` 重连，Chrome 与页面状态都还在（§2.2） | 无感，日志一行 |
| 总线丢帧（`seq` 断档） | 客户端重 `attach` 拿新帧 | 无感 |
| viewer 断线 | 租约释放、screencast 引用计数减一、末个关流 | agent 侧下次工具结果带介入摘要 |
| 人导航被闸 | `Blocked{reason}`；页内点击导航由 `post_nav` 事后隔离到 about:blank | Panel 提示 |
| Chromium 未装 | §6.1 的 fail-closed 三件套 | 工具错误 / doctor / 可安装 |
| `attach` 时 profile 不是 Managed | 显式拒绝「ExistingSession 无视图」 | RPC 错误 |
| `take` 撞上他人持有 | 显式错误 `held_by` | Panel 提示 |

原则照判据 §8：`Err` 只有资格说「我不知道」，不当放行读；「还没准备好」（端口未出）不答成「失败了」。

### 6.3 配置

```toml
[browser]                      # 现有（src/browser/profile.rs:245-261）
[browser.live]                 # 新
enabled = true
max_fps = 30
quality = 60
max_width = 1280
[browser.runtime]              # 新，台账用
download_host = ""             # PLAYWRIGHT_DOWNLOAD_HOST 镜像
prefer_system_browser = true
```

`[browser.live]` 走 `browser.update` 的 `reload_impact` 同一套裁决（live 还是 restart 是验证出来的，§3.12 第三轮 ⑥）。不加会话旋钮：接管是租约不是旋钮（旋钮的读写形状在 `src/gateway/execution_engine/turn_mode.rs:67-93`、`:96-113`，这里刻意不用）。

### 6.4 QA 装置：`qa/browser_live/run.sh`

真机、真 Chrome、真 playwright-cli，每个阶段证明一句话：

- `launch`：Aleph 起 Chrome、端口文件出现、`attach --cdp` 接上、`browser_snapshot` 有 `[ref=eN]`；`close` 后 Chrome 仍活。
- `stream`：旁观连接在 agent `browser_exec` 期间收到帧，`seq` 连续；静态页零帧。
- `takeover`：`take` 后 agent 工具返回 `HumanHasControl`；人输入落地；`cede` 后下一次工具结果带介入摘要且 URL 序列正确。
- `floor`：人导航到私网被 `Blocked`；点击跳转到私网被 `post_nav` 隔离。
- `pick`：命中测试返回的 `center` 用 `Coordinates` 点一下确实触发了那个元素。
- `reap`：viewer 断线后租约回 `Agent`，idle 收割能杀 Chrome。

Managed 的现有八场景（`qa/browser_managed/run.sh`）整套重跑。**变异验证**至少两处：去掉 `get_backend` 的闸 → `takeover` 必红；去掉历史条目复检 → `floor` 必红。

### 6.5 交付顺序

1. 启动链翻转（§3.1 + §6.1）+ 八场景回归。这一步单独可合，没有视图也已把 Chrome 的所有权理顺。
2. CDP 客户端 + `LiveSession` + RPC + 事件门控 + `browser_control` 工具（§3.2–3.5、§4.1–4.2）。
3. Panel 视图（§4.3）+ `qa/browser_live` 前四个阶段。
4. 选取器与附件（§5）+ `pick` 阶段。

每步一个 worktree、一次真机 QA、一次合并；没有真机证据的步骤不算交付（这一族的单测对 `FakeBackend` 结构性失明，FEATURE_LOCATOR §3.12 第四、五轮记录在案）。

---

## 7. 红线与判据自查

| 条目 | 一句话 |
|---|---|
| R1 | Chrome 是子进程、CDP 是 WebSocket，与现有 playwright-cli / chrome-devtools-mcp 同类；`src/` 不碰平台 API |
| R2 | 视图、选取器、芯片全在 Leptos Panel；壳零改动 |
| R3 | 无新 crate（`tokio-tungstenite` 已在）；Chromium 是运行时依赖不是构建依赖 |
| R4 | Panel 只订阅、绘制、回传事件；租约、闸、摘要全在服务端 |
| R6 | 视图走网关事件总线，桌面壳 / 浏览器标签 / 局域网同一份 |
| R7 | driver 只做 I/O 映射；介入摘要是事实记录不是判断 |
| R8 | `browser_control`、`runtime_install{chromium}` 都是工具，对话即管理 |
| R9 | 「refs 已失效」与「等待交回」只写进各自的渲染器一次，不进 system prompt |
| R10 | `src/harness/` 零改动；接管时模型看见拒绝并自愈，harness 不替它挑策略 |
| §1 单一推导 | `SelectedElement` 一个类型两个消费者；介入摘要一份两处 |
| §7 两端无线 | `cdpEndpoint` 今天只在注释里，本 spec 把它接上并删掉旧路 |
| §8 fail-closed | `input` 丢弃要回报数目；端口未出是「未就绪」不是「失败」 |
| §9 多脸同推导 | 人与 agent 的导航共用 `check_navigation` + `post_nav`；`Select` 与 `switch_tab` 共用 `active_tab_id` |
| §12 同处派生 | 坐标一律 viewport CSS 像素，viewport 只有 `browser_resize` 一个写者 |
| §14 谁能开闸 | `Human` 态的出口有两个（cede / 持有者断开），被闸住的 agent 有 `browser_control{wait}` 与超时回报可走，不是 fail-dead |
| §17 展示要有渲染者 | 每个 `state` 字段与芯片都指得出 `chrome.rs` 里画它的那处，否则 CUT |

---

## 8. 刻意不做（本轮已评估，勿重提）

- **obscura**（D3）。三条阻塞项：① target 按 CDP 连接隔离（旁观连接附不上）；② 可访问名不从内容计算（snapshot 要自算 accname）；③ 单 V8 isolate，脚本重的页面上每条 CDP 命令等十几秒。它真正的优势（30 MB 声称、单二进制、秒开、反检测）属于场景 ④「服务器上只读、批量、无人看」。**重评配方**：用 §10 的 `probe.mjs` / `probe2.mjs` / `probe3.mjs` 对新版本重量四个读数——第二条连接能否附着并收帧、HN 上 link 的 name 覆盖率、github.com 上 `evaluate("1")` 的往返、`Input.insertText` 是否存在；四项都过再谈 driver。
- 壳内原生 WebView（§1 三条理由）。
- 独立的浏览器 tab / 顶层视图（D5：与对话割裂）。
- VNC / noVNC（openclaw / hermes 生态的做法：Linux 容器专属、对语义一无所知）。
- WebRTC / neko（kernel-images 的做法：画质最好，但要改 X 输入驱动）。
- `ExistingSession` 的视图：用户本来就看得见自己的 Chrome。
- OOPIF 内部选取；多选框选；自动生成修改 prompt。
- 手机 Panel 的视图与带宽自适应。
- 从页面拷出到剪贴板。
- 租约 TTL（连接死即释放已足够；TTL 是 YAGNI）。
- viewer 协商画质/帧率（全局配置即可）。
- 带标签的附件枚举。

---

## 9. 未验证

来自两个 spike 与设计假设，实施前逐条补：

- `playwright-cli attach` 是否接受 `--config`（`outputDir` / `allowUnrestrictedFileAccess` 还要不要得到）。
- 现有运行时台账是否已经会跑 `playwright install`——默认 `BrowserType::Chromium` 意味着 playwright-cli 起的是 Playwright 自带的 Chromium，若没人装它，今天的默认路径靠系统 Chrome 兜着。
- headed profile 下人的物理输入与 CDP 注入输入的交错（`isTrusted`、焦点）。
- Windows / Linux（两个 spike 都只在 macOS arm64）。
- 两条连接**同时**派发输入的竞争（spike 交替进行，未同时）。
- `DevToolsActivePort` 出现的时间窗（spike 事后读取，未量）。
- 长时间流与慢 viewer 的背压（最长连续 30 s、及时 ack）。
- Chrome spike 从未解码过一帧像素（只计数与字节）；延迟不含编码与传输到 Panel。
- Chrome 的实际工作集（1175 MB 是 RSS 求和上界）。
- playwright-cli 未经提示就带 `--remote-debugging-port` 是否跨版本稳定——本 spec **不依赖**它，只记录。
- Skyvern / steel-browser 的许可证。

---

## 10. 附录：证据文件

目录 [`2026-09-05-browser-live-view-evidence/`](2026-09-05-browser-live-view-evidence/)：

- `obscura-spike-findings.md` —— §2.1 的原始记录（探针输出的逐项读数）。
- `chrome-spike-findings.md` —— §2.2 的原始记录（含每一步的命令与逐字错误文本）。
- `probe.mjs` / `probe2.mjs` / `probe3.mjs` —— obscura 探针；`probe-chrome.mjs` —— Chrome 双连接探针；`probe.html` —— 本地测试页。
- `README.md` —— 怎么重跑。

这些脚本是丢弃式 spike 产物，只为 §8 的 obscura 重评能原样再跑一遍而保留；不是测试，不进 `qa/`。
