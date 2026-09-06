# ChatGPT 分享 6a9d165a — Aleph 内嵌浏览器设计摘要

来源：`chatgpt-conversation.md`（stream 顺序 MSG 0–4，非聊天顺序）+ 14 条短 CJK 字符串（多为用户轮次/分享页摘录）。按要求忽略"Chromium/CEF/Codex Desktop 内嵌浏览器"早期部分，聚焦用户表明在做 Aleph 之后的设计。

## 1. 聊天顺序还原

用 "Aleph" 出现次数做锚点：MSG3、MSG4 全文 0 次提及 Aleph（完全通用，不知道 Aleph 存在）；MSG0（38 次）、MSG1（34 次）、MSG2（48 次）都大量提及。结合内容前后引用关系还原顺序：

1. **S13**（用户）问 GitHub 上有无内嵌浏览器 AI agent 项目可参考 → **MSG4**：给出 BrowserOS 等 8 个项目的排序对比，末尾主动提出做"技术栈地图"。*[忽略]*
2. **S9/S11**（用户，二者内容高度重叠）要求逐层对比这些项目 → **MSG3**：给出 L0–L8 八层技术栈地图，推荐 Electron+Chromium+CDP+Browser Harness+Wove。*[忽略，0 次提及 Aleph]*
3. **[未被抓取的用户轮次]**：收窄为"Aleph 怎样把网页渲染进自己的 Rust Desktop UI" → **MSG0**：重排为 CEF/cef-rs 第一、BrowserOS 第二、Browser Use Desktop 第三，**明确排除 Obscura**作为渲染层参考。*[忽略——纯渲染视角，与后续 obscura-first 决策相反]*
4. **S7**（用户，唯一明确点名 "Aleph" 的轮次）："我正在开发一个叫 Aleph 的 AI agent，使用 rust 开发，目前基本成熟。现在想给 Aleph 添加内嵌浏览器功能。我设想是使用 obscura 作为 first-class 浏览器底层，使用 chromium 作为兜底策略。你根据我的需求，将上面的方案重新整合成一个完整的方案给我。" → **MSG2**：开篇"你的约束一变，前面的方案确实应该重构"，修正 MSG0，改为 Obscura=Agent Execution Engine / Chromium=Full-Web Compatibility Engine；结尾主动提出落成 trait/API 设计。
5. **S6**（近乎逐字复述 MSG2 结尾提议，疑似分享页摘录）/ **S4**（用户更短复述同一请求）→ **MSG1**：交付接近可编译的 `aleph-browser-core` 设计。
6. **S3**（"把 Obscura 视为单线程、不满足 Send 的引擎……核心采用 actor 模型"）内容与 MSG1 §9-11/§29-31 高度重合，判断为 **MSG1 的摘录片段**，非独立轮次。

**结论**：需要的"后期"内容 = **MSG2（重构判断）→ MSG1（落地设计）**；MSG0/MSG3/MSG4 属用户要求忽略的"渲染优先/CEF/Codex Desktop"部分。

---

## 2. 提出的架构（逐字引用关键代码，来自 MSG2 → MSG1）

### 2.1 MSG2 的初版四层 trait（后被 MSG1 取代）

MSG2 先给了一版简化 trait：`trait Browser { launch, close }` / `trait BrowserSession { new_page, pages, close, capabilities }` / `trait Page { goto, snapshot, click, type_text, press, screenshot, evaluate }`，配扁平的 `Target { semantic_id, role, name, locator }`。`BrowserCapabilities` 是纯 **bool** 字段集，`BrowserSupervisor` 持有具体类型 `obscura: ObscuraBackend, chromium: ChromiumBackend`，用 if/else 二选一——**这套 bool + if/else 设计在 MSG1 中被推翻**（见 2.4/2.8）。另提出 `BrowserPolicy` 与 `TrustLevel{Trusted,Untrusted,UserConfirmed}` + `UserInstruction/BrowserContent/ToolOutput` 三个字符串 newtype 阻止网页内容隐式变成用户指令（详见第 7 节安全模型）。

### 2.2 MSG1：workspace 与核心原则

> "aleph-browser-core 是同步/异步 Agent-facing protocol；所有具体浏览器都藏在 Actor 后面。"

```text
aleph/
├── crates/
│   ├── aleph-core/
│   ├── aleph-agent/
│   ├── aleph-browser-core/
│   ├── aleph-browser-runtime/
│   ├── aleph-browser-obscura/
│   ├── aleph-browser-chromium/
│   ├── aleph-browser-cdp/
│   ├── aleph-browser-policy/
│   ├── aleph-browser-observation/
│   └── aleph-browser-migration/
└── apps/
    └── aleph-desktop/
```

`Cargo.toml` 依赖刻意精简：`async-trait/serde(derive)/serde_json/thiserror/url(serde)/uuid(v4,serde)/bytes`——建议若 Aleph 已有统一的 `Error/Result/Id/Event/Cancellation` 就直接复用。对象模型：`BrowserManager → Session → Context → Page → Frame`（第一版简化为 `Manager → Session → Page`）。

### 2.3 ID 类型（宏生成 newtype，防止跨类型误传）

```rust
macro_rules! define_id { ($name:ident) => {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct $name(pub Uuid);
    impl $name { pub fn new() -> Self { Self(Uuid::new_v4()) } }
}; }
```
生成：`BrowserId, SessionId, PageId, FrameId, TargetId, ElementId, ActionId`（防止如 `PageId` 误传成 `SessionId`）。

### 2.4 Engine / Capability（升级为三级，而非 bool）

`pub enum BrowserEngine { Obscura, Chromium }`。`pub struct BrowserCapabilities`，全部字段类型均为 `CapabilityLevel`：`javascript, dom, accessibility, screenshots, pdf, uploads, downloads, websocket, service_worker, webgl, webgpu, webrtc, extensions, iframe, network_interception, cookies, local_storage, session_storage`；`pub enum CapabilityLevel { Unsupported, Partial, Supported }`。理由原文："WebGL = Partial 和 WebGL = Unsupported 对于 routing 是完全不同的。" `pub struct BrowserRequirement`，全部字段类型均为 `bool`：`visual, javascript, uploads, downloads, webgl, webrtc, extensions, network_interception`。

### 2.5 Handle / Actor 边界（核心机制）

`trait BrowserManager: Send + Sync` 方法：`capabilities() -> BrowserCapabilities`、`create_session(config) -> Result<BrowserSessionHandle>`、`close_session(session: SessionId) -> Result<()>`。**关键点：返回 `Handle` 而非真实 backend 对象**：`BrowserSessionHandle { id: SessionId, tx: mpsc::Sender<SessionCommand> }`、`PageHandle { id: PageId, tx: mpsc::Sender<PageCommand> }`（均 `#[derive(Clone)]`）。链路：`Agent → PageHandle → mpsc::Sender<BrowserCommand> → ObscuraActor → obscura::Page`——因 Obscura Page 受 `!Send` 与单 V8 isolate 约束，禁止 `Arc<Mutex<obscura::Page>>` 直接穿透 Agent runtime。

`trait BrowserSession: Send + Sync` 方法：`id() -> SessionId`、`pages() -> Result<Vec<PageInfo>>`、`new_page() -> Result<PageHandle>`、`close() -> Result<()>`、`storage_state() -> Result<StorageState>`、`apply_storage_state(state: StorageState) -> Result<()>`。

`trait Page: Send + Sync` 方法（Agent 真正操作的对象，第一版只做 primitive）：`id() -> PageId`、`url() -> Result<Url>`、`title() -> Result<String>`、`goto(url: Url, options: NavigateOptions) -> Result<NavigationResult>`、`reload(options: NavigateOptions) -> Result<NavigationResult>`、`back() -> Result<NavigationResult>`、`forward() -> Result<NavigationResult>`、`snapshot(options: SnapshotOptions) -> Result<PageSnapshot>`、`click(target: Target) -> Result<ActionResult>`、`type_text(target: Target, text: String) -> Result<ActionResult>`、`press(target: Option<Target>, key: Key) -> Result<ActionResult>`、`scroll(direction: ScrollDirection, amount: f64) -> Result<ActionResult>`、`screenshot(options: ScreenshotOptions) -> Result<Image>`、`evaluate(expression: String) -> Result<serde_json::Value>`、`wait(condition: WaitCondition) -> Result<()>`。

Target/Locator/Role（**不向 Agent 暴露 CSS selector**）：`pub enum Target { Element(ElementId), Semantic { role: Option<Role>, name: Option<String>, nth: Option<u32> }, Locator(Locator), Coordinates { x: f64, y: f64 } }`；`pub enum Locator { Css(String), XPath(String), Text(String) }`；`pub enum Role { Button, Link, Textbox, Checkbox, Radio, Combobox, Listbox, Menu, MenuItem, Tab, Heading, Image, Generic }`。

### 2.6 Snapshot / Element（Agent 的核心输入）

`pub struct PageSnapshot` 字段：`page_id: PageId, url: Url, title: String, accessibility: Option<AccessibilityTree>, interactive_elements: Vec<InteractiveElement>, text: Option<String>, frames: Vec<FrameInfo>, screenshot: Option<ImageRef>, timestamp_ms: u64`。`pub struct InteractiveElement` 字段：`id: ElementId, role: Option<Role>, name: Option<String>, value: Option<String>, placeholder: Option<String>, disabled: bool, checked: Option<bool>, editable: bool, visible: bool, bounds: Option<Rect>, locator: Option<Locator>`。

ElementId 是 **session-local**，防止 Agent 用"三分钟前的 DOM node"点击：`pub struct ElementRef { snapshot_id: u64, element_id: ElementId }`，或直接 `Target::SnapshotElement { snapshot: SnapshotId, element: ElementId }`。

`ActionResult` 直接携带 `new_snapshot`，把 click→DOM 变化→重新 snapshot 合并成一次 round-trip：`pub struct ActionResult { action_id: ActionId, success: bool, page_changed: bool, navigation: Option<NavigationResult>, new_snapshot: Option<PageSnapshot>, diagnostics: Vec<Diagnostic> }`。

导航/等待条件抽象掉引擎具体名字（不写 `ObscuraNetworkIdle2`）：`pub struct NavigateOptions { wait_until: WaitUntil, timeout: DurationMs }`；`pub enum WaitUntil { Commit, DomContentLoaded, Load, NetworkIdle }`；`pub struct DurationMs(pub u64)`（不在 serde 结构里直接用 `std::time::Duration`）。

### 2.7 Event / Command / Actor 运行模型

`pub enum BrowserEvent` 变体：`SessionCreated{session_id,engine}`、`SessionClosed{session_id}`、`PageCreated{session_id,page_id}`、`PageClosed{page_id}`、`NavigationStarted{page_id,url}`、`NavigationCommitted{page_id,url}`、`NavigationFinished{page_id,url}`、`DomChanged{page_id}`、`Console{page_id,level:LogLevel,message:String}`、`Request{page_id,request:NetworkRequest}`、`Response{page_id,response:NetworkResponse}`、`Download{page_id,download:DownloadInfo}`、`PermissionRequested{page_id,permission:BrowserPermission}`、`EngineFailure{session_id,error:BrowserError}`。

配套的 `enum PageCommand` 每个变体形如 `Navigate { url, options, reply: oneshot::Sender<Result<NavigationResult>> }`（`Snapshot/Click/TypeText/Screenshot/Evaluate/Close` 各一个变体）：handle 侧发命令、actor 侧执行、`oneshot` 回传。

Obscura 专属线程 + `LocalSet`（S3 概括的正是这一段）：`pub struct ObscuraActor { rx: mpsc::Receiver<BrowserCommand>, browser: obscura::Browser }`。

```rust
pub fn spawn_obscura(config: ObscuraConfig) -> ObscuraRuntime {
    let (tx, rx) = mpsc::channel(256);
    std::thread::Builder::new().name("aleph-obscura".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let local = tokio::task::LocalSet::new();
            local.run_until(async move {
                let browser = obscura::Browser::builder().stealth(config.stealth).build()
                    .expect("failed to create Obscura");
                run_obscura_actor(browser, rx).await;
            }).await;
        });
    }).expect("failed to spawn Obscura thread");
    ObscuraRuntime { command_tx: tx }
}
```

原文强调：**不要"一 Session 一个线程"**——多个 page 共享一个 V8 isolate，一个 engine actor 管多个 session 更合理；Chromium/CEF backend 不受此限，但同样遵循 `Handle → Actor → backend`。

### 2.8 统一 Backend / Supervisor（取代 MSG2 的 bool + if/else）

`trait BrowserBackend: Send + Sync` 方法：`engine() -> BrowserEngine`、`capabilities() -> BrowserCapabilities`、`create_session(config) -> Result<BackendSession>`、`migrate_in(state: MigrationState) -> Result<BackendSession>`、`health() -> Result<HealthStatus>`。`pub struct BrowserSupervisor { obscura: Arc<dyn BrowserBackend>, chromium: Arc<dyn BrowserBackend>, sessions: SessionRegistry, policy: BrowserPolicy, events: BrowserEventBus }`；核心方法 `create_session(requirement, config)` 内部先 `let engine = self.select_engine(&requirement).await?`，再 `match engine { Obscura => self.obscura.create_session(config).await, Chromium => self.chromium.create_session(config).await }`。`select_engine(requirement, obscura_caps, chromium_caps) -> BrowserEngine`：`if satisfies(obscura, requirement) { Obscura } else if satisfies(chromium, requirement) { Chromium } else { Chromium }`。原则："先找最轻量、最便宜、最快的满足者。"

### 2.9 Policy / Permission / Network

`BrowserPolicy { network, downloads, uploads, clipboard, navigation, permissions }`，`NavigationPolicy { allowed_schemes, blocked_hosts, allowed_hosts: Option<HashSet<String>> }`；`enum BrowserPermission { Camera, Microphone, Geolocation, Notifications, ClipboardRead, ClipboardWrite, Downloads, Uploads, Popups }`（core 只定义枚举，具体实现留给各 backend）。网络层：`trait NetworkController { set_rules(rules: Vec<NetworkRule>) -> Result<()> }`，`NetworkRule { pattern: UrlPattern, action: NetworkAction }`，`enum NetworkAction { Allow, Block, Redirect{url}, Fulfill{response} }`。

### 2.10 Migration（Session ID 跨引擎不变）

`pub struct MigrationState { session_id: SessionId, current_url: Option<Url>, cookies: Vec<Cookie>, local_storage: HashMap<String, HashMap<String, String>>, session_storage: HashMap<String, HashMap<String, String>>, pages: Vec<PageMigrationState> }`；`pub struct PageMigrationState { page_id: PageId, url: Url, scroll_x: f64, scroll_y: f64 }`。明确**不**迁移：JS heap、DOM 内部指针、service worker、WebRTC state、canvas state（"那些基本属于 engine-specific state，不应该成为 Migration contract"）。`SessionId = 42` 在 Obscura→Chromium 升级前后保持不变，Agent 不感知；`engine` 只暴露在 `SessionInfo { id, engine, capabilities }` 诊断结构里。

### 2.11 Health / Error / Recovery

`pub enum HealthStatus { Healthy, Degraded, Unresponsive, Crashed }`；`pub struct SessionHealth { last_success: Instant, consecutive_failures: u32, last_error: Option<BrowserError> }`；`pub enum BrowserError { NavigationTimeout, TargetNotFound, StaleTarget, UnsupportedCapability(BrowserCapability), EngineCrashed, EngineUnavailable, PermissionDenied, NetworkBlocked, JavaScriptError, PolicyViolation, MigrationFailed, Timeout, Internal(String) }`；`pub enum RecoveryAction { Retry, ReSnapshot, ReResolveTarget, Reload, ReopenPage, EscalateToChromium, Fail }`。

### 2.12 Facade / CDP 抽象 / 最终公开 API

Agent 最终只依赖一个 facade：`struct Browser { supervisor: Arc<BrowserSupervisor> }`，方法 `open(config)` 与 `open_with_requirement(requirement, config)` 均返回 `Result<BrowserSessionHandle>`，内部转发给 `supervisor.create_session`。统一 CDP 抽象：`trait CdpTransport { send(method, params) -> Result<Value>; next_event() -> Result<CdpEvent>; }`。CDP 分工：Obscura 默认走 native API、CDP 只做特殊能力/调试；Chromium 用 CEF native + CDP 做 Agent inspection/control。

MSG1 §74 把最终公开 API 收敛为 4 个 trait（`BrowserManager/BrowserSession/Page/BrowserBackend`）+ 上文已列出的全部 struct/enum，不再新增类型（"这组类型足够支撑第一版，又没有把你锁死在 Obscura 或 Chromium 的实现细节上"）。§76 给出写作顺序：先写 `aleph-browser-core/src/{lib,ids,error,engine,capability,requirement,session,page,target,action,navigation,snapshot,event,storage,migration,policy}.rs`，再写 `aleph-browser-runtime/src/{supervisor,selector,session_registry,recovery,migration,event_bus}.rs`，最后才是 `aleph-browser-obscura/`、`aleph-browser-chromium/`。结尾建议：先落一个"接近可编译"的 core（配一个**假的 InMemoryBackend** 让它先编译、跑通测试），再接 `ObscuraBackend`。

---

## 3. 关于 Obscura 实际接口的事实性主张（CLAIM，待与真实源码核实）

以下均标记 **[KNOWN]**（ChatGPT 自己的标注，不代表已验证），附其原始 `citeturn...` 引用标记（未核实真实性）：

- Obscura 提供原生 Rust `Browser/Page/Element` API；native API 调用**不需要经过 CDP round-trip**。`citeturn841300search0`
- Obscura 当前**所有页面共享单个 V8 isolate**，V8 工作**全局锁串行化**；官方要求用 `LocalSet` 驱动，对象存在 **`!Send`** 约束。`citeturn841300search0/search2`
- 页面生命周期状态机：`init → commit → domcontentloaded → load → networkidle2 → networkidle0`。`citeturn841300search2`
- 已提供 **request interception**（continue/modify-fulfill/fail），文档强调 **URL rewrite 必须做 SSRF validation**。`citeturn841300search5/search6`
- 同时暴露 native Rust API 与 CDP 两条通道，"不要为了接口统一强迫 Obscura backend 全部走 CDP client"。`citeturn841300search0`
- （MSG2，措辞更保守）当前定位仍是 **headless browser engine**，"不是 CEF 那种可直接作为 UI surface 嵌入桌面应用的浏览器"；可被 Playwright/Puppeteer 通过 CDP 驱动，定位为 AI agents/scraping 用途，"不是普通用户的完整桌面浏览器体验"。`citeturn0search4/1search0`
- （第三方口碑，单一来源）"Obscura 宣传指标激进……第三方实测在部分 React/Cloudflare 站点上兼容性不及 README"；ChatGPT 自己注明这是单一测试，不能据此否定 Obscura。`citeturn0reddit48`

⚠️ MSG1 与 MSG2 对 Obscura"是否走 CDP"表述有张力：MSG1 §57 说 native API 优先、CDP 只做"特殊能力/调试"，但 §49 技术选型表又把 "Browser protocol" 统一写成 **CDP**（未区分引擎）——ChatGPT 自身论述不一致，核实时需注意。

---

## 4. "无需光栅化 / 空间 JSON 树" 的主张

**在本对话（MSG0–4）中未找到**明确的"不需要光栅化，用 DOM+布局生成空间状态 JSON 交给模型"的完整设计。全文对"几何/坐标"的唯一具体落点是 `InteractiveElement.bounds: Option<Rect>` 这一个字段，且未说明其来源（CDP 的 `getBoxModel`/`DOM.getContentQuads` 还是 Obscura 原生 API）、坐标系（viewport/page/device pixel），也没有独立的"空间 JSON 树"schema。`PageSnapshot` 里 `accessibility/interactive_elements/screenshot` 三者并列，且 `screenshot` 明确"不要默认包含"，只在 canvas/图表/布局歧义等场景用于 "Visual"/"Hybrid" 两种 Observation Mode（默认是 "Semantic" = DOM+A11y+Text）。**这更接近"默认不截图、按需截图"，而非"根本不需要光栅化"这一更强主张** — 后者如存在应来自本对话之外，建议核实时明确标注"本对话未涵盖"。

---

## 5. Escape-hatch（Obscura → Chromium）触发逻辑

MSG2 明确反对"简单失败即 fallback"：`match obscura.goto(url).await { Ok(_) => …, Err(_) => chromium.goto(url).await }` —— **"这是不够的"**，因为"页面'加载失败' ≠ 浏览器'不支持'"，且若 Obscura 已执行到一半（登录、填表 5 步），此时切 Chromium 会**丢失整个 state**。正确机制分三层：

1. **能力声明 + 静态路由**：Task 提交 `BrowserRequirement`，Supervisor 用 `select_engine` 按"最轻量满足者优先"选择引擎，而非运行时才试错。
2. **失败分类 + 仅特定类型才升级**：failure taxonomy 为 `TargetNotFound/TargetNotVisible/NavigationTimeout/JSException/UnsupportedFeature/BrowserCrash/PermissionDenied/NetworkError`，**只有 `UnsupportedFeature` 或 `EngineFailure` 才升级**，正名为 **"Capability Escalation"**（能力升级）而非 "fallback"（降级）——Obscura 是 "fast agent browser"，Chromium 是 "full compatibility browser"，方向是升级。
3. **迁移保状态**：升级时用 `MigrationState` 把会话状态搬到 Chromium，`SessionId` 不变，Agent 无感知。

MSG2 额外提出两个 **[INFERRED]** 高级机制（均未给出判定阈值算法）：**"影子验证"**——同 URL 同时跑两个引擎，比较 DOM/文本/标题相似度、元素数量、截图相似度得出置信分数，低于阈值才升级；**"Browser Intelligence"**——长期记录每个 `site`/`domain+task_type` 的成功率与迁移率，让 Aleph 自己学出路由表（如 "github.com/read → Obscura，/edit → Chromium"）。

---

## 6. [INFERRED] vs [KNOWN] 标记盘点

**[KNOWN]** 几乎全部集中在第 3 节列出的 Obscura/CEF 事实性描述（后者属已忽略的渲染部分）。**[INFERRED]** 占绝大多数篇幅——crate 目录划分、四层对象模型、`BrowserCapabilities` 三级枚举设计、Handle+Actor+mpsc 实现形态、`spawn_obscura` 骨架（原文自认"具体代码需按 Aleph 当前 runtime 和 Obscura 最新 API 调整"）、`BrowserSupervisor`/`select_engine`、`MigrationState` 字段集、Recovery 分类、"影子验证"/"Browser Intelligence"、Permission Broker/`TrustLevel`、Observation 的 relevance scoring/token budget、以及几乎全部 ASCII 架构图。核实阶段应假定**除第 3 节外，其余设计细节均为 ChatGPT 原创建议、需独立验证可行性**，而非验证"转述是否准确"。

---

## 7. ChatGPT 自己提出的开放问题 / 保留意见

- **Obscura 成熟度保留**：它"仍是相对年轻的独立浏览器引擎"，第三方测试显示真实站点兼容性或不如 README 宣传；结论是 Obscura-first + Chromium escalation 而非 Obscura-only。
- **建议建 Benchmark**：100–300 个真实网站/任务同时跑两引擎，记录成功率/延迟/内存/token/迁移率/失败率，以后自动决定 Obscura 是否仍有资格做 first-class engine。
- **代码骨架非最终实现**：线程/LocalSet 边界才是架构重点，`spawn_obscura` 等函数需按 Aleph 实际 runtime 与 Obscura 最新 API 调整。
- **安全模型需独立对待**：网页内容→LLM→受信任工具构成间接 prompt injection 攻击链；引用 "ceLLMate" 研究（限制 Agent 的 ambient authority）；建议第一天做 Permission Broker + 类型级信任区分。
- **CEF/Rust 生态成熟度保留**（已忽略部分）：`cef-rs` 这层 Rust binding "远没有 Chromium/CEF C++ 原生生态成熟"，建议严格封装不泄漏。
- **MVP 范围声明**：MSG1 §68 建议"第一版砍掉 50% 接口"，只做 open/close/goto/back/forward/reload/snapshot/click/type/press/scroll/screenshot/evaluate/wait + cookies/storage/download，DOM/A11y/Network 先内部实现不对外暴露。
