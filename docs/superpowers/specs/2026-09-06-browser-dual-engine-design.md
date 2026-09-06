# Browser Dual-Engine — obscura first-class、Chromium 逃生舱、一个 CDP 抽象层、不栅格化的页面状态树

- **日期**：2026-09-06
- **分支**：main 单分支开发；**本轮实施在用户下令后新建的 worktree 分支里进行（C2）**，按 §8 的阶段各做一次真机 QA、一次合并。
- **Status: approved design 2026-09-06** —— 五节设计逐节经用户点头（§0 D-A…D-C、路 C）；下一步是 `writing-plans` 出实施计划。范围是**一份**实施计划（§8 六个阶段），不需要拆分。
- **承接**：[`2026-09-05-browser-live-view-design.md`](2026-09-05-browser-live-view-design.md) §0 **D6**（引擎路线转向的裁定原文）；FEATURE_LOCATOR §3.12（浏览器自动化，本 spec 是它的第八轮设计输入）；plan 1（启动链翻转，已合 main `5a4a15bf1`，**不回滚**——它回答的是「谁拥有进程」，本轮回答「进程是谁」）。
- **证据**：本轮扫描与复测全文在 [`2026-09-06-browser-dual-engine-evidence/`](2026-09-06-browser-dual-engine-evidence/)（§12）。所有数字都带「测于哪个二进制、用哪条命令」。
- **参考项目**（只借设计，不搬代码）：obscura `/Volumes/TBU4/Github/obscura`（Apache-2.0）· BrowserOS `/Volumes/TBU4/Github/BrowserOS`（**AGPL-3.0，一行代码不能进 Aleph**）· browser-use/desktop `/Volumes/TBU4/Github/desktop`（MIT）· wove `/Volumes/TBU4/Github/wove`（Apache-2.0，Wave Terminal 分叉，带 NOTICE）· cef-rs `/Volumes/TBU4/Github/cef-rs`（Apache/MIT，本轮克隆）· 用户与 ChatGPT 的对话（只借三级能力枚举与失败分类，见 §2.6）。

---

## 0. 决策记录

用户在对话中按此顺序裁定，后面的设计不重开这些问题：

| # | 裁定 | 用户原话要点 / 来源 |
|---|---|---|
| **D6**（承接） | obscura 升为 first-class 默认内核；CDP 之上抽象统一 Agent Browser 接口；Chromium 降为逃生舱；**可能不需要栅格化**——把 DOM+布局转成专供模型理解的空间状态树 | 2026-09-05 晚，见上一份 spec §0 D6 |
| **C1** | **两个引擎都不进安装包**：安装时拉取、由运行时账本管理的外部运行时 | 「并不是直接将 obscura 或者 chromium 嵌入 Aleph 安装包，而是作为外部运行时在 Aleph 安装的时候拉取安装，作为运行时管理」 |
| **C2** | 用户下令前**不建 worktree**；main 正等远程合并 | 「在收到我的指令前，不要创建 worktree」 |
| **D-A** | **两引擎都上 Aleph 自己的 CDP 客户端**；playwright-cli 降为 Chromium 的备用驱动，CUT 留到真机 QA 对齐后的下一轮 | 三选一，取推荐项 |
| **D-B** | 模型的默认观察 = **缩进文本树 + 可交互节点带紧凑几何**；完整 JSON 树作附件；截图按需 | 三选一，取推荐项 |
| **D-C** | 向 obscura **上游提 PR**（`DOMSnapshot` 读真实布局、`getBoxModel` 诚实失败）；合入前用标准 CDP 过渡；不维护分叉 | 三选一，取推荐项 |
| **路 C** | CDP 传输进 `crates/aleph-cdp/`，其余进 `src/browser/`；否决 ChatGPT 的八 crate 家族（路 B）与全塞 `src/browser/` 的路 A | 「同意路 C」 |
| 参考项目增补 | cef-rs 着重页面渲染 | 「再添加一个参考项目（着重它的页面渲染）」 |

**一句话架构**：`26 个工具面（不改）→ ProfileManager::get_backend（唯一路由点，不改）→ CdpBackend（一个 BrowserBackend 实现）→ crates/aleph-cdp（一条连接、多 session）→ {obscura | chromium} 外部进程`；页面观察由 `src/browser/page_state/` 从 DOM+几何**一条推导**生成，两引擎共用；引擎切换是模型的决定、`browser_session{switch_engine}` 是它的动词、cookie+URL 是它带走的状态。

---

## 1. 背景与价值

Aleph 的浏览器子系统（FL §3.12）今天让 agent 经 26 个工具驱动一个由 Aleph 自己启动的无头 Chromium（plan 1 之后），驱动协议是 `playwright-cli attach --cdp`（`BrowserDriver::Managed`）或 `chrome-devtools-mcp`（`ExistingSession`）。三件事构成本轮的价值：

1. **驻留内存与启动**：Chromium 无头 8–10 个进程、空闲 1.4 GB（求和 RSS，上界）；obscura 单进程、空闲 20 MB、三个重页 311 MB（§2.1 M7）。Aleph 的定位是常驻后台服务（R6），这一项直接决定它能不能在用户机器上「一直开着」。
2. **模型看什么**：今天模型拿到的是驱动打印的一段 aria 快照文本（`SnapshotOutput.snapshot_text`，`types.rs:106`），仓内**零**结构化 DOM/布局/空间表示（评估报告 §4，验证过的否定）。D6 第 4 点的「空间状态树」是净新增，没有现有消费者可破坏。
3. **不与引擎绑定**：`BrowserBackend` 28 个方法里 23 个返回 `()`、已有 80% 在实质上引擎无关（`backend.rs:14`），但 `BrowserDriver` 把「驱动协议」和「引擎」混成一个 serde 枚举（`profile.rs:24-30`），两臂都通向 Chromium。要加第二个引擎就必须把这两个轴拆开。

**为什么不是 CEF**（用户要求着重看它的渲染）：CEF 要求宿主进程本身是 browser process（`cef_execute_process` 最先跑，macOS 无多线程消息循环、`NSApplication` 子类化），只有完整桌面 App 能承载（撞 R6），且 `sys/build.rs` 把 libcef 版本锁死在编译期——与 C1「安装时拉取、任何近期版本都行」的账本模型相反。它的进程内 CDP 通道（`send_dev_tools_message`，无需 websocket）是本轮记下的唯一亮点，留在 §10。

---

## 2. 证据

### 2.1 obscura v0.2.2 复测（发布于 2026-09-05，126 提交；本地克隆 `72c84ad` 只差 3 个版本号提交；二进制 sha256 已对 GitHub API `digest`）

| 项 | v0.2.1（09-05 测） | **v0.2.2（09-06 测）** | 命令 |
|---|---|---|---|
| `Runtime.evaluate("1")` github.com | 16.8 s / 12.4 s | **3 ms / 1 ms**（修好） | `node m1-isolate.mjs … 3` |
| 同一探针 Wikipedia Rust 页 | 未测 | **load 后 0.5 s 起单次 25.1 s 停顿**，3/3 复现；第二道屏障 t+26.5 s 起持续 44 s | `m1b-window.mjs`、`m12e-dense.mjs` |
| 停顿的形状（M12b，500 ms 固定节拍 400 次采样） | — | **全局屏障**：0–25.5 s 内发出的全部命令在 t≈25.75 s 同一瞬间返回（260 个）；`DOM.*`、`Page.getLayoutMetrics`、**`Target.getTargets`** 全被门住；唯一逃出的是落在两道屏障间 250 ms 缝隙里的一次 `evaluate` | `m12e-dense.mjs` |
| AX 树 229 个 `role=link` 有名字的 | 0 | **0**（`name` 键根本不存在；Chrome 198/229；`getPartialAXTree` 返回 `{}` 而非报错——fail-open） | `m3456.mjs` |
| 第二条 CDP 连接 | `getTargets=[]`、attach "Target not found" | **不变**（源码：`CdpContext` 每连接线程一份，`dispatch.rs:51-52`、`server.rs:817-829`，是 V8 每线程 isolate 的承重结构） | `m3456.mjs` |
| 多语句 `evaluate` | SyntaxError | **修好**（`"1; 2"` → 2） | `m3456.mjs` |
| `DOM.getBoxModel` | 占位四边形 | Chrome 形状；HN 五元素与 JS `getBoundingClientRect` **逐字节相同且与截图像素吻合**（M11） | `m11-geom.mjs` |
| `DOMSnapshot.captureSnapshot` | — | **合成数据**：宽全 1280、高全 18、y = 节点序号×18，3 ms 返回 Chrome 形状的 `bounds`，响应无任何标记（源码 `domsnapshot.rs:1-16, 200-236` 自述不查渲染层） | `m11-geom.mjs` |
| JS 遍历式快照（一次 `evaluate`） | — | HN 142 ms / github 1189 ms / wiki **29 s**（撞进屏障）；Chrome 12 / 21 / 71 ms；obscura `innerText` **含 `<style>/<script>` 源码**（github 3.4 M 字符） | `m2-walker.mjs` |
| `getDocument{depth:-1}` + 200×`getBoxModel` | — | 81 ms（不在屏障内时）；obscura 200/200 给盒（Chrome 拒 7 个）⇒ 「getBoxModel 失败」在 obscura 上不能当可见性信号 | `m3456.mjs` |
| RSS 空闲 / +github / +HN / +wiki / 全关 | — | **20 / 364 / 286 / 311 / 338 MB**（关闭不回落）；Chrome 求和 1381 / 1436 / 1580 / 1860 / 1111 MB（求和重复计共享页，是对 obscura 有利的上界） | `m7-mem.mjs` |
| 渲染（1280×800 截图，六站） | github 双重绘制 | HN、react.dev、x.com 近 Chrome；github/amazon 元素重叠绘制；**Wikipedia 正文列被压到 ~100 px、infobox 溢出视口**；六站零验证码 | `m10-shots.mjs` |
| `playwright-cli attach --cdp` | — | attach/goto/snapshot（608 ref、223/224 链接命名——Playwright 在页内 JS 算名）/eval/fill/screenshot/tab-new 全过；**`click <ref>` 永远解析不到；`click <selector>` 导航后永久毒化 daemon**（`utilityScript.evaluate is not a function`），两个新 daemon 复现 | 手工序列，见报告 M8 |
| 自带 MCP | — | 37 工具（`browser_interactive_elements` 等），**无几何**；`fetch`/`scrape` 走另一套网络栈，本机（fake-ip TUN）全部失败而 `serve` 正常 | `mcp-probe.sh` |

**结论**：三条阻塞项在 v0.2.2 **全部成立**（停顿变窄但未消失、AX 无名、连接隔离），另发现两条新事实（CDP 几何有真有假、playwright-cli 不是可用的直连驱动）。

### 2.2 obscura 源码事实（`72c84ad`，Apache-2.0，五平台 × 四变体发布）

- `obscura serve --port <p> [--host] [--storage-dir <dir>] [--stealth] [--allow-private-network] [--allow-file-access] [--proxy] [--v8-flags]`；**无 `--remote-debugging-port`、无 `--headless`、无 `--user-data-dir`**（`crates/obscura-cli/src/main.rs:10-198`）。cookie 持久化靠 `--storage-dir`（`obscura-browser/src/context.rs:26,41`）。
- 端点宣告：同端口 HTTP `/json/version` 返回 `webSocketDebuggerUrl: ws://127.0.0.1:{port}/devtools/browser`（`server.rs:791`，**硬编码 127.0.0.1**）；stdout 打印 `CDP server: ws://…`（`main.rs:226-236`）；**没有 `DevToolsActivePort` 文件**。`--port 0` 是否支持：**未验证**（§11）。
- 每命令看门狗 `OBSCURA_CDP_COMMAND_TIMEOUT_MS` 默认 60 000 ms，到点**杀 isolate**而不是排队（`dispatch.rs:766-780`）。
- `layout_dom(tree, viewport) -> DomLayout{rects: HashMap<NodeId, Rect>, text_runs, styles, clip_rects, transforms}` 全 `pub`（`obscura-render/src/dom.rs:298, 3207`）；`NodeId.index()` 就是 CDP `backendNodeId`（`domsnapshot.rs:16`）；`PreparedRender::layout()`（`paint.rs:986`）已存在但没有从 `Page` 暴露——**上游 PR 的落点**。
- `Page` 不是 `Send`（`Rc<RefCell<…>>`，`runtime.rs:3311`），API `pub` 但无契约（workspace 版本冻结 0.1.0）——C1 之下 Aleph **不链接它**，这两条只影响上游 PR 的写法。
- 默认发布归档含 render（`README.md:272-273`）；`no-render` 归档会把使用者扔进合成几何的世界而无警告。

### 2.3 Aleph 现状（`c049ef1ed`，评估报告 `aleph-browser.md`）

| 事实 | 锚点 | 对设计的意义 |
|---|---|---|
| `get_backend` 是两臂 match、唯一路由点；backend **按调用构造、不缓存**（因 `browser.update` 热换 SSRF 策略） | `manager.rs:383-406`、`:196-199` | CDP 连接不能按调用建 ⇒ 引擎句柄缓存在 `ProfileManager`，backend 仍按调用构造成廉价句柄克隆 |
| 26 个工具、~39 个调用点全经 `make_backend{,_and_tab,_and_tab_guarded}` | `mod.rs:216/225/249` | 新 backend 零工具面改动 |
| `BrowserDriver{Managed, ExistingSession}` 选的是驱动协议不是引擎；`BrowserType{Chromium,Chrome,Brave,Edge}` 是 Chromium 家族枚举，均 serde 暴露 | `profile.rs:24-30`、`:13-19` | 拆 `engine` × `driver` 两轴；**不**给 `BrowserType` 加 `Obscura` |
| `list_tabs -> String` + 手写 `parse_tab_line` 认两种 CLI 渲染，曾整批解析为零并静默通过 SSRF 审计 | `backend.rs:17`、`tab_registry.rs:201-246` | 改 `Vec<TabLine>`（`TabLine{id,url,selected}` 已在 `:189-195`） |
| ref 在 Rust 里**不产、不验、不映射**；`ActionTarget::{Ref{String}, Coordinates{x,y}}`，刻意无 `Selector`（`types.rs:404` 有测试钉住）；Chrome MCP 拒绝 `Coordinates` | `types.rs:10-15`、`chrome_mcp_backend.rs:40-45` | ref 格式自由；`Coordinates` 在 CDP 驱动上两引擎都真正实现 |
| 仓内唯一看快照内部的代码是 `matches("[ref=").count()` | `snapshot.rs:96` | 移到 backend 后面 |
| `SnapshotOutput.page_url/page_title` 零消费者 | `types.rs:113-116` | 本轮接上 |
| Aleph **不说裸 CDP**；`CdpEndpoint{http_url, ws_url, pid}` 的 `ws_url` 文档自述「给 plan 2 的裸 CDP 客户端」；`tokio-tungstenite 0.26` 在树 | `chromium_launch.rs:110-119`、`Cargo.toml:282` | `crates/aleph-cdp` 是它的第一个消费者 |
| `ChromiumChild::spawn`：先写 sidecar 意图戳再等端口；四结果孤儿清扫；`shutdown_browsers_global` 挂两处 daemon 退出点并有源码级守卫 | `chromium_launch.rs:316-405, 637-651`、`start/mod.rs:3672`、`helpers.rs:536, 646` | 上提为引擎无关的 `engine/process.rs`；sidecar 加 `engine` 字段 |
| `ChromiumLaunchSpec::argv()` 是顺序即契约的 Chrome 开关表，含承重的 `--use-mock-keychain` | `chromium_launch.rs:68-106` | 留在 `engine/chromium.rs` 原样不动 |
| 账本 `RuntimeSpec{…, install: &[OsInstall], post_install}`，`InstallStrategy{Shell,PowerShell,Via,NpmGlobal}`，六条目；Chromium 被刻意排除在外（不在 PATH） | `src/runtimes/specs.rs:5-44, 79, 191-195`、`runtime_manage.rs:10-16` | obscura 是单二进制，**比 Chromium 更合身**；缺一个 `GithubRelease` 安装策略 |
| `ChromiumLocator::locate()` 单数字段 | `runtime_manage.rs:112-128` | `EngineLocator::locate_all()` |
| doctor `browser/chromium-missing`、`browser/runtime` | `chromium_missing.rs:30`、`browser_runtime.rs:34` | 加 `browser/obscura-missing`，同实现按引擎参数化 |
| `pdf_generate` 自建第二个 `PlaywrightCliDriver` | `pdf_generate/browser_engine.rs`、`manager.rs:360-377` | **本轮不动**：PDF 钉 Chromium+playwright-cli，随下一轮 CUT 一起迁 |
| `evaluate` 必须返回**值**不是通话记录（`wait_probe` 哨兵回显缺陷） | `backend.rs:45-58` | CDP `returnByValue:true` 天然满足；契约写进 trait 方法文档不变 |
| 控制租约（`HumanHasControl`）**尚不存在**——是上一份 spec 规划、plan 2 才建 | 评估报告 §2 | 切换动词写成「租约落地后必须尊重」，本轮无租约可检 |
| §3.12 ㉑ 的清单已漂移：`[browser.runtime] channel` 键不存在；真实三键 `binary_path / prefer_system_browser / download_host`，嵌在 `general.browser.runtime` | `profile.rs:191-214`、`general.rs:25` | 文档纠正；只改「已成谎言」的名字 |

### 2.4 参考项目的教训（设计借用清单，代码零搬运）

| 项目 | 借 | 不借 |
|---|---|---|
| BrowserOS / BrowserClaw（AGPL） | ref 身份键 `(frame_id:loader_id, backendNodeId)`、导航即清表；点击前 `elementFromPoint` 遮挡检查并返回**具名**阻挡者；引擎 crate 不知多租户、宿主经 `InnerCallHook` 注入（Aleph 对应物是 `ProfileManager`）；tab 归属直接印在返回值里 | 两套语言各实现一遍 17 工具；rrweb 直播（依赖扩展宿主）；AX 树作模型面（obscura 上 fail-open） |
| browser-use/desktop（MIT） | 合成层**页面坐标**输入作统一原语（shadow DOM / 同源 iframe / OOPIF 一条路）；接管靠 overlay 挡输入不靠 CDP 标志（plan 2 的事）；「卡住」只测沉默、原因交给模型（R7）；`verifyCdpOwnership`——绑到端口后确认是自己的进程 | 全部会话共用一个 cookie jar；UA 伪装 Firefox；1 fps 轮询截图当直播 |
| wove（Apache，Wave 分叉） | `DOMSnapshot.captureSnapshot` 跨 frame 一次取几何（Chromium 取数器的形状）；双层点击（JS 优先、原生 CDP 兜底）；子任务独立账本共享物理 tab | 人机共用 webview 无锁；硬编码 `INTERACTIVE_TAGS` 名单（判据 §5） |
| cef-rs | 进程内 CDP 通道的存在（§10） | 全部（§1） |

### 2.5 参考实现里**没有**的东西

四个参考项目**都没有**验证码/登录墙/渲染异常的检测代码：BrowserOS 靠跑在已登录 profile 里，browser-use 靠 30 s 沉默超时 + 模型看图，wove 什么都没有。本轮同样不做检测——这不是缺口，是 R7 的共识形状。

### 2.6 ChatGPT 对话那份设计的处置

它的 `aleph-browser-core` 以**进程内 obscura 原生 API + actor/LocalSet** 为前提，与 C1 冲突，整体不采用。借三样：`CapabilityLevel{Unsupported,Partial,Supported}` 三级而非 bool；失败分类 `TargetNotFound/…/UnsupportedFeature/EngineFailure` 只有后两类才谈升级；「Capability Escalation」而非「fallback」的措辞。**CUT**：`BrowserRequirement` 静态路由（确定性路由，R7）、影子验证（双引擎跑同一 URL 比相似度，是分类器且成本翻倍）、按域名学成功率的「Browser Intelligence」（记忆层的事）。**「不栅格化 / 空间状态树」不在那份对话里**，是用户在 D6 里的原创，此处按原创记。

---

## 3. 设计第一节：架构与数据流

```
工具面 (26 个 browser_* 工具, 不改)
   │ make_backend_and_tab_guarded (mod.rs:216/225/249)
   ▼
ProfileManager::get_backend (manager.rs:383)     ← 唯一路由点，保留
   │ 解析 profile → (engine, driver)
   ├─ driver=cdp ──────► CdpBackend ─┐      ┌─ obscura  (ObscuraChild)
   ├─ driver=playwright_cli ► 现有 backend │ EngineHandle {process, conn, sessions, ref tables}
   └─ driver=chrome_mcp ─► 现有 backend   └─► └─ chromium (ChromiumChild, 从 chromium_launch 搬入)
                                          │
                     crates/aleph-cdp: CdpConnection（一条 ws，多 session，事件扇出）
                                          │
                       src/browser/page_state: RawDom ─► PageState ─► 文本树 + refs
```

### 3.1 四块新增，各一个职责

| 块 | 位置 | 做什么 | 不做什么 |
|---|---|---|---|
| CDP 传输 | `crates/aleph-cdp/` | 一条 websocket 到 `/devtools/browser`；`call(session, method, params) -> Value` 按命令超时；`Target.attachToTarget{flatten:true}` 管 session；事件 `broadcast` 扇出（滞后计数，不静默丢）；断连 ⇒ 所有挂起请求 `Err(Disconnected)` | 不生成 CDP 域类型（只有 ~30 个方法的薄包装 `aleph_cdp::methods::*`）；零 Aleph 依赖；不知道 profile / 引擎 / 工具 |
| 引擎进程 | `src/browser/engine/` | `Engine{Obscura,Chromium}`；`EngineProcess` trait；`obscura.rs` / `chromium.rs` 两个实现；`capability.rs` 能力常量表；账本 `RuntimeSpec` | 不碰 DOM；不决定用哪个引擎 |
| 统一驱动 | `src/browser/cdp_backend/` | 实现现有 `BrowserBackend` 全部 28 方法；23 个 `()` 动词直接映射 `Input.*`/`Page.*`/`DOM.*`；按 `Engine` 只在**三处**分支：几何取数路径、JS 对话框（obscura 无事件 ⇒ `UnsupportedByEngine`）、hit-test（两引擎统一走 JS `elementFromPoint`，obscura 无 `DOM.getNodeForLocation`） | 不暴露 `cdp_send` 逃生口（§3.12 第三轮 A 对 hermes 模型的否决不动） |
| 页面状态 | `src/browser/page_state/` | `RawDom`（两引擎各一个取数器）→ `PageState`（**唯一**构建器）→ 文本渲染 + ref 铸造（带代数） | 不做内容判断；不做「渲染异常」分类（R7） |

### 3.2 一次 `browser_click(ref=e12)` 的路径

工具面不变 → `get_backend` 命中 `driver=cdp` → `CdpBackend` 从 `EngineHandle` 取当前 tab 的 session → `page_state::resolve(e12)` 核对代数（不同文档 ⇒ `BrowserError::StaleRef{Navigated}`）→ `DOM.resolveNode(backendNodeId)`（失败 ⇒ `StaleRef{NodeGone}`）→ `DOM.scrollIntoViewIfNeeded` + `DOM.getBoxModel` 取中心 → JS `elementFromPoint` 遮挡检查（阻挡者具名进错误文本）→ `Input.dispatchMouseEvent` ×2（`clickCount` 对 `dblclick` 为 2——`click.rs:77-92` 「坐标双击无驱动支持」的拒绝在 playwright CUT 后解除）→ 现有 `post_nav` 等待 → `Ok(())`。

### 3.3 一次 `browser_snapshot` 的路径

Chromium 取数器 = `DOMSnapshot.captureSnapshot{computedStyles:[display,visibility,opacity,cursor,overflow], includeDOMRects:true}`：一次调用跨 frame、几何由引擎自报、**不经页面 JS**（页面无法用改写的 `getBoundingClientRect` 骗它）。obscura 过渡取数器 = `DOM.getDocument{depth:-1,pierce:true}` + 并发 `DOM.getBoxModel`（M11 证实诚实）+ 一次 `Runtime.evaluate` 按文档序取五个样式位（两次遍历用节点数 + 首尾 `backendNodeId` 校验，不一致重取一次，再不一致把 `computed` 置 `None`——**未知不写成可见**）。**上游 PR 合入并被账本钉住后删掉这个取数器**，两引擎共用 `DOMSnapshot` 一条取数路径。之后进同一个 `PageState` 构建器；返回现有 `SnapshotOutput`，`page_url`/`page_title` 一并接上。

### 3.4 三处刻意的结构决定

1. **一个 profile 一条连接、一个进程。** `EngineHandle` 缓存在 `ProfileManager`（新字段 `engines: Mutex<HashMap<ProfileName, Arc<EngineHandle>>>`），backend 仍按调用构造（保住 `manager.rs:196-199` 那条热换 SSRF 的理由——句柄克隆便宜）。直播观察者以后挂在同一条连接上——obscura 的 per-connection 隔离逼出来的形状，对 Chromium 同样成立。
2. **停顿是事实不是判断。** `call()` 超时默认 30 s（可配；**必须低于 obscura 自己 60 s 的断头台**，否则先死的是页面不是我们的等待）；到点返回 `Err(Timeout{method, waited})`，工具结果把「引擎等待了 N 秒」原样交给模型；驱动**不重发**（M12b：屏障释放时一起回来，重发只会排第二遍）。不自动切引擎（R7 / R10 第 5 不）。
3. **六个 chokepoint 不动。** `network_policy` / `secret_guard` 仍在 `&str` 上工作，`browser_exec` 仍只经 `Arc<dyn BrowserBackend>`，`make_backend_and_tab_guarded` 的重定向复检不变；CDP 驱动从它们中间走。

### 3.5 熵减（本节内）

`chromium_launch.rs` 里**引擎无关**的上提到 `engine/process.rs`：sidecar 注册目录与记录（类型改名 `EngineSidecar`、**加 `engine` 字段**——否则混合引擎的孤儿清扫认不出谁是谁）、先盖意图戳再等端口、四结果孤儿清扫（`ArgvProbe` 按 `--storage-dir=` / `--user-data-dir=` token 精确匹配）、`LaunchPolicy`、`shutdown_browsers_global` 两处退出点。**Chromium 专属**的留在 `engine/chromium.rs`：`argv()` 开关表、`DevToolsActivePort` 解析。`playwright_launch.rs` / `playwright_cli*.rs` / `chrome_mcp*.rs` **本轮不删**，降级为 `driver=playwright_cli|chrome_mcp` 的备用臂。

### 3.6 这一节让什么变难

- Aleph 从此拥有可访问名 / 可交互性 / 可见性的启发式——Playwright 今天免费给的那部分成了维护面。
- 两个驱动并存一轮，同一动词两套推导，靠 §7.4 的对齐 QA 压着；CUT 是下一轮的第一件事。
- `engine × driver` 矩阵多出要在加载期校验的非法组合。

---

## 4. 设计第二节：页面状态树（`src/browser/page_state/`）

### 4.1 三层数据，一条推导

| 层 | 谁产 | 内容 |
|---|---|---|
| `RawDom` | 每引擎一个取数器 | `engine`、`viewport{w,h,scroll_x,scroll_y,content_w,content_h,dpr}`、扁平化的各 frame（主 frame 在前，子 frame 带父偏移与 `frame_id/loader_id`）；每节点 `backend_node_id`、`parent`、tag、attrs、文本、`rect: Option<Rect>`（**`None` = 这个引擎没给盒**，计入 `no_box`；它单独**不**判定不可见——Part 5 在 obscura 上量到行内 `<a>`/`<span>` 零四边形而 M11 在 HN 上量到真实盒，两次测量不一致，可见性不能压在它上面，见 §11 U9）、`computed: Option<{display_none, visibility_hidden, opacity_zero, cursor_pointer, overflow_clip}>`、`shadow_root`、`is_clickable_hint: Option<bool>`（Chrome `DOMSnapshot` 给的） |
| `PageState` | **唯一**构建器 `PageState::build(raw, &mut RefTable)` | `engine`、`generation`、url、title、viewport、前序节点表：`role`（`role` 属性 → tag 映射表）、`name`（accname-lite：`aria-labelledby` → `aria-label` → `<label for>`/包裹 label/placeholder → `alt`/`title` → 可见后代文本 ≤80 字）、`value`、状态位（disabled/checked/expanded/selected/required/readonly/focused）、`rect`（页面坐标整数）、`interactive`、`visible`、`text`（自身文本）、`ref: Option<RefId>` |
| 模型面 | 渲染器 `render_text(&PageState)` / `to_json` | 缩进文本树（默认观察）；完整 JSON 经 `browser_snapshot{format:"json"}` 取得，走与文本同一条 `bound_content` → `redact_wrap` → `offload_full_content` 路径（offload 后可由 `ctx_search` 取回）。工具结果今天没有图片之外的附件通道，所以「附件」的诚实实现是这个第二种 `format`，不是一个没有渲染者的字段（判据 §17） |

**取数器只有两个，其中一个有死期**（§3.3）。

**可交互性是多信号提示，不是过滤器**：`role ∈ 交互集 ∨ tabindex≥0 ∨ onclick ∨ cursor:pointer ∨ contenteditable ∨ is_clickable_hint`。非交互但有文本的节点也进树、也可有 ref（供 scroll-to / 读取）。名单只覆盖立法当天的世界（判据 §5），所以它是**提示位**而不是入树门槛。

### 4.2 文本渲染

每行一个节点——**行形状** ⇒ 现有 `bound_content`（按行截断，`mod.rs:278-282`）、`redact_wrap`（注入栅栏，`mod.rs:340`）、`offload_full_content`（`mod.rs:414`）全部原样复用，**不需要第二套预算推导**。

```
# engine=obscura gen=7 url=https://news.ycombinator.com/ viewport=1280x800 scroll=0,0 doc=1280x1173 no_box=12/703 fetch=142ms
- banner
  - link "Hacker News" [ref=e1] @130,11 83x15 /url: https://news.ycombinator.com/
  - link "new" [ref=e2] @218,11 24x15 /url: newest
- main
  - listitem
    - link "Show HN: …" [ref=e5] @136,44 359x15 /url: https://…
    - text: "160 comments"
  - textbox "Search" [ref=e612] @589,1144 153x21 [placeholder=Search…]
```

几何只印在交互节点上；`visible=false` 的节点不进文本树，但 JSON 里保留并标 `visible:false`。可见性以 `computed` 为准；`computed` 缺失（取数校验失败）时才回落到 `rect.is_some()`；可见但无盒的节点照常进树、有 ref、只是不印 `@x,y wxh`。头行的 `no_box`（无盒节点/总节点）与 `fetch`（这次取数的耗时，毫秒；撞上屏障时它就是那 25 s——取数器分不出屏障与普通延迟，所以标签只说它测到了什么）是给模型的两个运行时事实，不是判断。

### 4.3 ref 与代数（约定第一次有代码）

- `RefId = "e{n}"`；身份键 `(frame_id, loader_id, backend_node_id)`；**同一文档内跨快照保持同号**（DOM 变动不改号，模型不用重学编号）；主 frame `loader_id` 变（导航）⇒ 整表清空。
- `RefTable` 住在 `EngineHandle` 的 per-tab 状态里，记每个 ref 的铸造代数；`PageState.generation` 每次捕获 +1。
- 解析：旧代数、同文档 ⇒ 允许，经 `DOM.resolveNode(backendNodeId)`；失败 ⇒ `StaleRef{ref, reason: NodeGone}`；文档已换 ⇒ `StaleRef{reason: Navigated}`。错误文本带一句「重新 snapshot」（A2：模型看见并自愈）。
- `snapshot.rs:96` 的 `matches("[ref=")` 字面量删掉，`ref_count` 由 backend 报（`SnapshotOutput` 加 `ref_count: usize`）。

### 4.4 动作目标

`ActionTarget::Coordinates{x,y}`（**已存在**，`types.rs:10-15`）在 CDP 驱动上两引擎都真正实现：页面坐标 → `Emulation`/滚动折算视口坐标 → `Input.dispatchMouseEvent`。几何进了观察面就必须有对应的动词（判据 §9），这就是那张脸。**坐标绝不跨引擎复用**：obscura 文字宽度比 Chrome 窄 ~15%（M2），换引擎即换进程、换 loader，表自然清空。`Selector` 变体**继续不存在**（`types.rs:404` 的测试不动）。

### 4.5 刻意不做

SoM 截图标注（文本树已带几何，YAGNI）；`Accessibility.*` 域一律不读（两引擎行为不一致且 obscura fail-open）；「渲染异常」判定（R7——只把 `engine`、`no_box`、`fetch` 印在头行和 JSON 里，判断留给模型）；结构 diff（BrowserOS 用行 diff，本轮连行 diff 也不做，模型自己对比）。

---

## 5. 设计第三节：逃生舱——谁决定、怎么切、带走什么

### 5.1 谁决定：模型

硬件层只做三件事——把事实摆出来、提供切换动词、搬状态。没有任何确定性代码判断「这页渲染坏了」或「这是验证码」（R7）；没有自动升级（R10 第 5 不）。这与用户在 D6 里的措辞一致：「**Agent 探测到**目标网页在 Obscura 下渲染异常……通过接口无缝切换」——探测者是 agent。

### 5.2 三类运行时事实，各有一个拥有者

| 事实 | 谁说出口 | 形态 |
|---|---|---|
| 屏障等待 | `CdpConnection::call` 超时 | `BrowserError::EngineBusy{engine, method, waited}` → 工具错误文本：「obscura 30 s 内未应答 `DOM.getDocument`；请求仍在队列中会自行完成；`browser_session{switch_engine}` 可换 chromium」。驱动**不重发** |
| 能力缺席 | `engine/capability.rs` 常量表 | `BrowserError::UnsupportedByEngine{engine, verb}`，fail-closed，报出**具体动词**。同一张表生成 `browser_session` 的 `DESCRIPTION` 里那段能力说明——**一个真源两张脸**（R9 第二把尺：这句话有一个工具拥有它） |
| 引擎死亡 | `EngineHandle` 的断连 watch | `EngineFailure{engine, reason}`；丢弃句柄；**下一次调用重启同一引擎**，不换 |

### 5.3 切换动词

`browser_session{action:"switch_engine", engine:"chromium"|"obscura", migrate:true}`；`browser_open{profile, engine?}` 接受一次性覆盖。切换对 profile 是原子的：启动目标引擎 → 迁移 → 关闭源引擎 → 返回当前 tab 的新 `PageState`（模型的旧 ref 全部失效，新树就在返回值里）。控制租约落地后（plan 2）人类持有时拒绝切换；本轮无租约可检。

### 5.4 profile 是身份，engine 是手段

一个 profile 一个 cookie/存储身份，`engine` 只是「此刻用谁跑」。`[general.browser.profiles.<n>].engine` 是默认引擎（自动注入的 `"default"` profile 改为 `engine=obscura, driver=cdp`；`"user"` 不变，仍是 Chrome `ExistingSession`）；旧配置的 `driver=managed_cli|chrome_mcp` 隐含 `engine=chromium`；`[general.browser].default_engine` 全局默认。**切换不持久化**——运行中的进程本身就是状态，进程退出即回到配置默认；想改默认走现有 `browser_profile` 工具（R8）。**不新增会话旋钮**。

### 5.5 迁移带什么（`MigrationState`，两向对称）

| 带 | 不带（写进工具 DESCRIPTION） |
|---|---|
| 全部 cookie（`Network.getAllCookies` → `Network.setCookies`，两引擎均实现） | JS 堆、进行中的表单输入、`sessionStorage` |
| 打开的 tab 的 URL 与滚动位置 | 历史栈 |
| 打开的 tab 所属 origin 的 `localStorage`（`Runtime.evaluate` 逐 origin 导出/导入，尽力而为，失败不阻断切换、写进返回值） | 其他 origin 的 localStorage、IndexedDB、Service Worker |

Chromium 的 `--user-data-dir` 与 obscura 的 `--storage-dir` 各自持久化自己的格式，互不读取。「无缝」的诚实定义：**登录态活着，页面进度死了。**

### 5.6 能力表是一张名单，所以要能被证伪

`qa/browser_dual/run.sh caps` 对真二进制逐项探测表里的每个声明（JS 对话框事件、拖拽事件、`Input.insertText`、screencast、第二连接、文件选择器…），任何一项与表不符即红。名单只覆盖立法当天的世界（判据 §5），这条守卫就是它的到期检查。表的每一行带「测于哪个版本」。

### 5.7 刻意不做

ChatGPT 的 `BrowserRequirement` 静态路由、影子验证、「Browser Intelligence」（§2.6）；按站点的引擎偏好表（那是记忆层的事，模型会记）。

---

## 6. 设计第四节：进程、账本、配置

### 6.1 账本（`src/runtimes/specs.rs`）

新增 `obscura` 的 `RuntimeSpec`：`binaries: ["obscura"]`、`version_flag: "--version"`、`version_regex` 取 `obscura (\d+\.\d+\.\d+)`（注意：源码构建报 `0.1.0`，发布版报 tag——账本只认发布版）、`min_version` = 钉住的 tag。安装策略缺一种：现有 `InstallStrategy{Shell,PowerShell,Via,NpmGlobal}` 没有「从 GitHub Release 拉一个校验过的资产」，用 `Shell` 拼 curl+tar 是第二个答案，所以加类型化的 **`GithubRelease{repo, tag, asset: AssetPattern}`**：按 `os×arch` 选资产（五平台：`{x86_64,aarch64}-{linux,macos}` + `x86_64-windows`；**无 aarch64-windows**，doctor 如实报「此平台只有 chromium」）、sha256 对 GitHub API 的 `digest` 校验、`download_host` 镜像与 Chromium 那条同名同义（本机需要它——release-assets 主机被 DNS 挡）、落到 `~/.aleph/runtimes/obscura/<tag>/`。只装 `obscura`，不装 `obscura-worker`（只有 `scrape` 用）。**变体**：默认 `default` 归档（带 render），`stealth` 是 profile 级 opt-in。tag 钉在 spec 里；上游 PR 合入后升 tag 就是账本的一次 `install`。

### 6.2 进程（`src/browser/engine/process.rs`）

```rust
#[async_trait]
pub trait EngineProcess: Send + Sync {
    fn engine(&self) -> Engine;
    async fn launch(&self, req: LaunchRequest) -> Result<Launched>;   // Launched{pid, endpoint: CdpEndpoint, sidecar: EngineSidecar}
    async fn kill(&self, pid: u32, grace: Duration) -> Result<()>;
}
```

- `ChromiumChild` 是第一个实现（plan 1 的 argv 含 `--use-mock-keychain` 原样搬入；`DevToolsActivePort` 解析留在它自己文件里）。
- `ObscuraChild` 的 argv 只有 `serve --port <p> --storage-dir <profile>/obscura [--stealth] [--allow-private-network] [--proxy <url>]`——`--allow-private-network` 只在该 profile 的 `network_policy` 允许私网时才给（obscura 自带的 SSRF 地板不比我们的松）；`--allow-file-access` **永不给**。stdio 全 null、secret env 剥离，与 `ChromiumChild::spawn` 同纪律。
- 共用机件（§3.5 上提）：`EngineSidecar{engine, pid, endpoint?, data_dir, build}`、先盖意图戳再等端点、四结果孤儿清扫、`LaunchPolicy`、两处 daemon 退出点的 `shutdown_browsers_global`（现有 census 测试扩到两引擎）；`sweep_orphaned_chromium` 改名 `sweep_orphaned_engines`，`cfg(test)` 孪生保留。
- `LaunchFailed{stage}` 的 stage 字符串加 `"obscura-exit"`、`"cdp-endpoint"`。

**就绪 ≠ 健康**（plan 1 的教训）：两引擎统一的就绪门 = `/json/version` 通 **且** `Page.navigate about:blank` + `evaluate("1")` 在 5 s 内回来；只查前者的哨兵一律不算。

**端口归属**（§11 待实测）：obscura `--port 0` 是否支持、是否在 stdout 宣告。两个备选按序落：① 支持且宣告 ⇒ 读 stdout 的 `CDP server: ws://…` 行；② 不支持 ⇒ Aleph 从临时端口段取一个、传入、启动后用 `/json/version` **加 pid 持有该监听 socket** 双重确认（browser-use 的 `verifyCdpOwnership` 思路）——不确认就可能接到别人的 obscura。

### 6.3 配置（`src/config/`）

```toml
[general.browser]
default_engine = "obscura"                 # 新；全局默认
cdp_command_timeout_secs = 30              # 新；CDP 驱动对两引擎的按命令超时；必须 < obscura 自己的 60 s 断头台

[general.browser.runtime]                  # 现有 Chromium 三键原样：binary_path / prefer_system_browser / download_host
[general.browser.obscura]                  # 新
binary_path = ""                           # 覆盖账本
variant = "default"                        # "default" | "stealth"

[general.browser.profiles.<name>]
engine = "obscura"                         # 新；缺省取 default_engine
driver = "cdp"                             # 新默认；旧值 managed_cli / chrome_mcp 继续解析且隐含 engine=chromium
```

加载期校验：`engine=obscura` × `driver≠cdp` 拒绝并指出改法；`engine=obscura` 时 `browser=<Chromium 家族>` 被忽略并在 doctor 记一条；**缺失走产品默认、损坏才收紧**（附录 D.0.76 那条闸的判据）。`BrowserType` **不加** `Obscura` 变体。

### 6.4 §3.12 ㉑ 清单的处置——只改「已经变成谎言」的名字

| 项 | 处置 |
|---|---|
| `BrowserError::ChromiumUnavailable` | → `EngineUnavailable{engine}`，消息按引擎给安装命令 |
| doctor `browser/chromium-missing` | 保留；增 `browser/obscura-missing`，同一实现按引擎参数化，各自 20 s 内部期限 |
| `ChromiumLocator::locate()` 单数 | → `EngineLocator::locate_all() -> Vec<RuntimeRow>`；`runtime_manage` 的 `capability` 值多一个 `"obscura"` |
| `ChromiumSource` 与标签 | **不改**（它描述的确实是 Chromium 的三种来源） |
| `[general.browser.runtime]` 段名 | **不改**（改名是为改名，配置兼容更贵） |
| `ChromiumSidecar` | → `EngineSidecar` + `engine` 字段（不改就是一张会说谎的记录） |
| `unsupported_in_existing_session` 的那句话 | 改为按 driver 报「哪个驱动/引擎支持此动词」 |
| ㉑ 里不存在的 `channel` 键 | 从文档删掉 |

### 6.5 三产物

`aleph-server` 独立二进制与桌面 App 内置 server 都能跑两引擎（无头、无窗口）；Panel 纯壳无关。Windows 上 obscura 是 zip 内单 exe，kill 走现有 `cfg(windows)` job 路径。

---

## 7. 设计第五节：错误、安全边界、测试与 QA

### 7.1 错误面（`BrowserError` 新增，全部 fail-closed，错误文本由驱动拥有并点名自愈动词）

| 变体 | 谁产 | 模型看到什么 |
|---|---|---|
| `EngineBusy{engine, method, waited}` | `call()` 超时 | 等了 N 秒、请求仍排队、切换动词名 |
| `UnsupportedByEngine{engine, verb}` | 能力表 | 具体动词 + 哪个引擎支持 |
| `EngineFailure{engine, reason}` | 断连 watch | 引擎已退出、下一次调用重启 |
| `EngineUnavailable{engine}` | 账本/定位 | 没装、`runtime_manage{install}` 命令 |
| `StaleRef{ref, reason: Navigated\|NodeGone}` | `RefTable` | 重新 snapshot |
| `Cdp(CdpError)` | 传输 | 协议错误原文（`code`/`message`） |

`Err` / 空列表只有资格说「我不知道」（判据 §8）：`getBoxModel` 失败 ⇒ `rect=None` 不是 0×0；样式取数校验失败 ⇒ `computed=None` 不是「可见」；`getPartialAXTree` 那种 `{}` 我们根本不读。

### 7.2 安全边界——六个 chokepoint 一个不少

- 不给模型任何裸 CDP 动词；`browser_exec` 仍只经 `Arc<dyn BrowserBackend>`。
- `network_policy`：同一函数、同一调用点（导航前的 `&str` 检查 + `make_backend_and_tab_guarded` 的重定向复检）；`Fetch.*` 子资源拦截两引擎都有但**本轮不做**——今天的契约也没管子资源，加了就是新能力不是连线。
- `secret_guard`：`type_text` / `fill_form` 同一扫描；`Coordinates` 点击不带文本，但进同一审批门（`ActionType` 表加 `BrowserSwitchEngine` 一行，**默认与 `BrowserOpen` 相同**——curated 表里是 `Ask`（`src/approval/config.rs:328`）；它启动一个新浏览器进程，与 open 同级，不更宽也不更严，别造第二道没有把手的门）。
- 页面派生的文本树与 JSON 附件都走现有注入栅栏（`ContentSource::BrowserContent`）；obscura 的 `--allow-private-network` 只由 profile 的网络策略开；`--allow-file-access` 永不给。
- 截图落盘走现有受保护位置 denylist；孤儿清扫按 argv token 精确匹配，不做子串。
- `Runtime.evaluate` 上 `returnByValue:true`：值不是通话记录（`backend.rs:45-58` 的契约天然满足）。

### 7.3 测试（最小可信验证集六条命令 + 变异纪律）

- `crates/aleph-cdp`：进程内假 CDP 服务端（tokio-tungstenite server）钉住请求关联、session 路由、超时、断连使全部挂起请求失败、事件扇出滞后计数。变异：去掉关联 id ⇒ 必红。
- `page_state`：真引擎捕获的 `RawDom` 夹具入库（HN 各引擎一份，`page_state/fixtures/`）；accname 顺序、role 映射、多信号可交互、`rect=None ⇒ hidden`、ref 跨快照同号、导航清表、渲染 golden 文件；proptest：渲染永远行形状（无节点输出换行）、任意行边界截断的前缀可解析。
- **一条推导的守卫**：census 测试断言 `PageState::build` 在生产代码里只有一个调用点，两个取数器都只产 `RawDom`（判据 §9 的机械化）。
- `testkit.rs` 加 `FakeEngineProcess`；现有「两处 daemon 退出点都调用 shutdown」的 census 扩到两引擎；配置测试：旧 driver 值隐含 chromium、非法组合拒绝、缺段走默认。
- 能力表证伪：`qa/browser_dual/run.sh caps`。

### 7.4 真机 QA（`qa/browser_dual/run.sh`，每条断言效果不断言调用）

| 阶段 | 证明什么 |
|---|---|
| `provision` | 账本从镜像拉 obscura、sha256 对上、`--version` 对上 tag |
| `open` | 就绪门（`/json/version` **且**能导航）对两引擎都成立；`--use-mock-keychain` 断言原样保留 |
| `snapshot` | 两引擎对同一本地页面产出的树在几何之外语义相同；头行 `engine` 正确 |
| `click` | ref / `Coordinates` 两张脸各自改变 URL；旧代 ref 在导航后报 `StaleRef` |
| `stall` | obscura 约 5 s 就会杀掉页面自己的忙循环任务（Part 5 实测 `autonomous browser task exceeded its task budget`），合成不出 20 s 屏障；本阶段用 `cdp_command_timeout_secs = 3` + 一个阻塞 ≥4 s 的本地页面证明「超时 → `EngineBusy` → 引擎仍活着、不重发」这条路径 |
| `switch` | 源引擎设 cookie → 切换 → 目标引擎 `getAllCookies` 里有它、当前 URL 恢复、源进程已退出 |
| `reap` | `kill -9` server 后重启，obscura 孤儿被清扫，Chromium 孤儿不误杀 |
| `caps` | 能力表逐项证伪 |

外加：`qa/browser_managed/run.sh` 中与驱动相关的场景（`open`、`tools`）在 Chromium 上以 `driver=cdp` 跑一遍（**对齐**）；全部场景以 `driver=playwright_cli` 再跑一遍（**回归**）。`ambient`/`attach`/`pdf`/`existing`/`headed` 五个场景测的是 playwright-cli 与 MCP 服务器自身，在 `cdp` 下通过等于什么都没测，脚本对它们**明确拒绝**而不是绿。

---

## 8. 交付顺序（写 plan 时的默认分期，每期一次真机 QA、一次合并）

| 期 | 交付 | 为什么这个顺序 |
|---|---|---|
| **T0** | §11 的待实测项（半天内的探针，结果写回本 spec §11） | 设计里唯一没数字的格子先填 |
| **S1** | `crates/aleph-cdp` + 假服务端测试 | 零 Aleph 依赖，可独立验证 |
| **S2** | `engine/{process,chromium}.rs` 上提 + `EngineSidecar` + 配置两轴 + 加载期校验 | 只搬不改行为；`qa/browser_managed` 八场景必须照旧全绿 |
| **S3** | `CdpBackend` 在 **Chromium** 上实现 28 方法 + `page_state`（Chromium 取数器）+ ref 表；`driver=cdp` 与 `driver=playwright_cli` 跑同一套 `qa/browser_managed` | **先在有对照物的引擎上建驱动**：playwright-cli 在同一个 Chrome 上的行为是现成的 oracle，驱动的缺陷在这里暴露比在 obscura 上便宜 |
| **S4** | obscura：`RuntimeSpec` + `GithubRelease` + `ObscuraChild` + 过渡取数器 + 能力表 + doctor；`qa/browser_dual {provision,open,snapshot,click,stall,reap,caps}` | 只加引擎适配，不再动驱动 |
| **S5** | `switch_engine` + `MigrationState` + `browser_open{engine}`；`qa/browser_dual switch` | 两引擎都能单独跑之后才有「切」 |
| **S6** | 文档：FL §3.12 新一轮 + 附录 E 实例 + `TOOL_SYSTEM.md` / `SECURITY.md` / `PROCESS_MANAGEMENT.md` 对应段 + CLAUDE.md 路由表行 + 禁用清单提议；上游 PR / issue 提交 | 与代码同一轮，不留「以后再写」 |

---

## 9. 红线与判据自查

| 条 | 本设计怎么答 |
|---|---|
| R1 | 两引擎都是外部进程、经 websocket；`src/` 无平台 API |
| R3 | 新增依赖 **零**（tokio-tungstenite、serde_json 已在树）；不链接 obscura crate、不引 chromiumoxide（生成类型数万行，只用 ~30 个方法）；「为什么不能是 Skill/MCP」：驱动浏览器是 R3 例外条款里「跑别人 agent 的运行时」的同一类——一个 CDP 客户端 ~1k 行，比 obscura 自带的 37 个 MCP 工具轻，且后者无几何 |
| R7 / R10 第 5 不 | 引擎选择、渲染异常、验证码全部是模型的判断；harness 只报事实（§5.2） |
| R8 | 引擎切换、profile 默认引擎、运行时安装都是工具（`browser_session` / `browser_profile` / `runtime_manage`） |
| R9 | 能力说明进 `browser_session` 的 `DESCRIPTION`，不进 system prompt；由能力表常量派生（一个真源） |
| R10 | `src/harness/` 零改动；棘轮不受影响（改动前后各量一次 `budget.rs::CEILING` 实测值，写进 plan 的验收） |
| 禁用清单 | 不引第二个 async runtime、不引向量库、不引平台 crate、不用正则解析自然语言（accname 只处理 DOM 属性）；**提议**新增「第二个 CDP 客户端实现——唯一真源 `crates/aleph-cdp`」 |
| 判据 §1 | 能力表 → `DESCRIPTION` 单向派生；§3.12 ㉑ 只改成谎言的名字 |
| 判据 §3 / §5 | 可交互性是提示位不是门；能力表配 `caps` 证伪装置 |
| 判据 §4 | QA 每条断言效果（URL 变了、cookie 到了、进程退了） |
| 判据 §8 | `rect=None`、`computed=None`、`Err(Timeout)` 都只说「不知道」 |
| 判据 §9 | 两引擎一个 `PageState::build`，census 钉住；`Coordinates` 与几何观察成对 |
| 判据 §13 | 30 s 命令超时 < 60 s 断头台，位置在传输层、寿命等于连接 |
| 判据 §14 | 每道闸都答「谁能打开、从哪里」：`EngineBusy` → `switch_engine`；`UnsupportedByEngine` → 另一引擎；`EngineUnavailable` → `runtime_manage{install}` |
| 判据 §15 | 意图戳先于端点（原样继承 plan 1） |
| 判据 §16 | 两引擎是孪生：能力表、就绪门、sidecar、QA 阶段都按引擎参数化，不复制两份 |
| 判据 §17 | 头行的每个字段都有渲染它的代码；JSON 附件有 `ctx_search` 消费 |
| 判据 §18 | §2.1 每个数字带二进制版本与命令；Chrome 内存明示为求和上界 |

---

## 10. 刻意不做（本轮已评估，勿重提）

- **CEF / cef-rs**：宿主必须是 browser process、只桌面 App 能承载（撞 R6）、编译期版本锁死（撞 C1）、无 codesign/notarize 支持、示例缓存了回调后即回收的纹理。唯一可能复活它的场景是**桌面 App 内的可见渲染器**（OSR 共享纹理，`was_hidden` 是真正的关闭开关），且那要先裁定「tokio 不能拥有 `main`」这条与 R1 形状不同的约束。
- **链接 obscura crate 进 aleph-server**（模式 B）：C1 否决；即便没有 C1 也撞 R3（`v8 137` + 468 包 + `!Send` 线程钉死）。
- **playwright-cli / chrome_mcp 的 CUT**：下一轮第一件事，前提是 §7.4 对齐 QA 全绿。
- **直播视图（plan 2–4）**：本轮只保证 CDP 连接是「一条连接多个消费者」的形状。
- **SoM 截图标注**、**`Accessibility.*` 域**、**结构 diff**、**`Fetch` 子资源策略**、**按站点引擎偏好**、**影子验证**、**`BrowserRequirement` 静态路由**。
- **obscura 的 `fetch` / `scrape` / MCP 面**：另一套网络栈、无几何。
- **Windows ARM 上的 obscura**：上游不发；doctor 如实报。
- **`pdf_generate` 迁引擎**：随 CUT 一起。

---

## 11. 未验证 / T0 待实测

| # | 问题 | 怎么测 | 影响 |
|---|---|---|---|
| U1 | ~~obscura `--port 0` 是否支持~~ **已答（Part 5 真机）**：绑到临时端口后 banner 与 `/json/version` 都报 `ws://127.0.0.1:0`，且 banner 在 bind 前打印（恒真）⇒ 只走「Aleph 分配端口 + 归属核验」 | 已测 | §6.2 只剩第二支 |
| U2 | obscura 对 `display:none` 元素 `getBoxModel` 返回什么（诚实失败 / 常量四边形 / 空盒） | HN 上对隐藏元素调用并比对源码 `dom.rs:322-327` 的回落分支 | 过渡取数器要不要识别常量四边形 |
| U3 | obscura `DOM.getDocument{pierce:true}` 是否带 iframe 内容 | 本地含 iframe 的页面 | 过渡取数器的 frame 扁平化 |
| U4 | Chromium `DOMSnapshot.captureSnapshot` 在 OOPIF 上的 `documents[]` 偏移 | 本地跨源 iframe 页面 | Chromium 取数器的偏移计算 |
| U5 | obscura `Input.dispatchMouseEvent` 的坐标空间是视口还是页面 | 滚动后点击一个已知 rect 的元素 | `Coordinates` 折算 |
| U6 | obscura `Network.setCookies` 对 `sameSite`/`expires` 字段的接受度 | 往返一组 cookie | 迁移保真 |
| U7 | `browser_managed` 的 `open`/`tools` 在 `driver=cdp` 下的基线（S3 前先跑一次 playwright 全场景基线记录） | 现有脚本 | 对齐 QA 的基线 |
| U8 | 上游 PR 的最小 diff（`domsnapshot.rs` 读 `PreparedRender::layout()`）能否不改 `Page` 的公开面 | 在克隆里试改并跑 `render-repros` | D-C 的落地成本 |
| U9 | obscura 对行内元素（`<a>`/`<span>`）在哪些放置下给出真实盒：body 直下 / `<p>` 内 / `<td>` 内 / 显式 block 父元素内，对照 Chrome 与 HN（M11） | T0 探针 | §4.1 的 `no_box` 语义与上游 issue 的措辞 |

---

## 12. 附录：证据文件（`2026-09-06-browser-dual-engine-evidence/`）

| 文件 | 内容 |
|---|---|
| `README.md` | 复跑方法（obscura v0.2.2 二进制来源与 sha256、端口、本地静态页） |
| `obscura-spike-v022.md` | 复测全文 M1–M12b（含每条命令与原始数字） |
| `aleph-browser-map.md` | Aleph 浏览器子系统在 `c049ef1ed` 的形状（file:line 锚点） |
| `obscura-source-survey.md` | obscura `72c84ad` 源码调查（CDP 域表、`layout_dom` API、发布矩阵） |
| `refs-browseros.md` / `refs-browser-use-desktop.md` / `refs-wove.md` / `refs-cef-rs.md` | 四个参考项目的设计借用与不借清单 |
| `chatgpt-design-summary.md` | 用户与 ChatGPT 对话的结构化摘要（哪些借、哪些 CUT） |
| `probes/` | `m1-isolate.mjs` `m1b-window.mjs` `m2-walker.mjs` `walker.js` `m3456.mjs` `m7-mem.mjs` `m10-shots.mjs` `m11-geom.mjs` `m12-lock.mjs` `m12e-dense.mjs` `mcp-probe.sh` `probe.html` |

原始 JSON / PNG（69 个文件）**不入库**，留在会话 scratchpad；数字已全部抄进复测报告。
