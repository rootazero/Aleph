# Aleph Panel 浏览器工作区与设计 Companion 架构规格
# Aleph Panel Browser Workspace and Design Companion Architecture Specification

- **状态 / Status:** 已完成设计审批，等待用户审阅规格；未授权实施
- **日期 / Date:** 2026-10-07
- **分支 / Branch:** `feature/panel-browser-workspace-20261007-202532-13256`
- **范围 / Scope:** Panel Chat 右侧浏览器工作区、TUI 独立 viewer、开发预览网络边界、设计 companion、资源连线与旧路径清理

> 本文把已在对话中批准的四节设计固化为实现前规格。它不是实现计划，也不授权写产品代码。实现必须在本文获用户审阅批准后，另行通过 `writing-plans` 产生计划。
>
> This document records the four approved design sections. It is not an implementation plan and does not authorize product-code changes. After user approval, a separate implementation plan must be produced through `writing-plans`.

## 1. 目标与问题 / Goal and Problem

### 1.1 用户目标 / User goal

Aleph 的浏览器能力应成为 Chat 任务的一部分，而不是把用户从任务上下文送到另一个浏览器：

- Panel Chat 中打开的网页、前端开发实时预览、设计阶段的 mockup／对比图，都在右侧栏的浏览器工作区显示。
- GPT／Claude 前端设计过程中常见的 browser companion 也必须进入同一右栏，包括候选窗布局、横排／多屏定位和动效状态。
- TUI 是明确例外：通过用户机器默认浏览器或用户指定的 Chrome 打开独立 viewer，但该 viewer 仍展示同一个 Gateway 浏览器会话，而不是新建第二套会话。
- Gateway 是浏览器动作和预览资源的执行位置；Panel 与 TUI 是展示／交互端。

Aleph 需要解决的不是“再加一个 iframe”，而是把浏览器会话、任务上下文、设计产物、开发服务、跨端 viewer 和授权边界连成一条可验证的资源链。

### 1.2 成功标准 / Success criteria

1. Panel 内所有 Aleph 管理的网页、开发预览和设计 companion 都进入当前任务的右栏。
2. 右栏消费结构化资源，不从聊天文字猜 URL。
3. TUI 可以打开本机默认浏览器或指定 Chrome，展示同一 Gateway session／target，不跳回 Panel。
4. Panel、TUI 和设计 companion 共享浏览器 target 与授权生命周期；不复制浏览器控制逻辑。
5. 开发预览的网络权限是任务级、来源级和必要的端口级约束；在证据不足时拒绝宣称安全隔离已完成。
6. 打开失败、授权过期、断连和进程退出都真实可见，不允许“报成功的 no-op”。
7. 过时的裸 URL、重复 iframe、直达系统浏览器路径在迁移后清理，或有明确兼容层和删除条件。

## 2. 设计约束与非目标 / Constraints and Non-goals

### 2.1 硬约束 / Hard constraints

- 遵守 Aleph R1：Rust Core 不直接调用平台 API；平台动作通过既有 Bridge／interface 边界完成。
- 遵守 R2/R4：复杂 UI 逻辑仍由 Leptos/WASM／Panel 承担，Bridge 与 interface 只做 I/O 和能力桥接。
- 遵守 R7/R8/R9：不使用确定性中间件替代 LLM 判断；可配置动作必须有工具入口；不能用 prompt 文案冒充强制路由或安全拦截。
- 所有操作在独立 worktree 分支中进行；不得直接修改 main。
- 参考项目只用于抽取模式，不能复制实现；最终边界必须映射到 Aleph Rust Core。
- Gateway 侧执行不等于把用户本机端口暴露到公网；本轮不新增通用隧道。

### 2.2 非目标 / Non-goals

- 不保证任意外部 GPT／Claude CLI 或脚本的 `open`、`--open`、系统浏览器调用都能被 Aleph 拦截或重定向。
- 不新增一个本机执行 Bridge 来捕获任意外部脚本。
- 不通过全局关闭私网保护来支持开发预览。
- 不把任意用户本机服务透明暴露给远端 Gateway。
- 不在未验证 redirect、子资源、iframe、fetch、WebSocket 和引擎差异前承诺完整 SSRF 隔离。
- 不维护 Panel iframe 预览、TUI 浏览器和设计 companion 各自独立的 session 状态。

## 3. 现状事实与 Gap Analysis / Current Facts and Gap Analysis

| 能力 / Capability | 参考项目可借鉴模式 / Reference pattern | Aleph 当前事实 / Aleph fact | 缺口与决策 / Gap and decision |
|---|---|---|---|
| 浏览器工作区 | BrowserOS／desktop 类项目把浏览器视图当作工作区，而不是普通链接 | 已有 browser/CDP、Gateway 与 Panel/TUI 入口，但右栏统一资源链尚未闭合 | 以 Gateway session/target 为唯一执行对象，Panel/TUI 只投影同一对象 |
| 多引擎抽象 | cef-rs 强调引擎能力与生命周期隔离 | Chromium 与 obscura 的能力声明存在，真实 Fetch 覆盖仍不完整 | 能力必须以实测 capability gate 决定，不能按静态枚举承诺 |
| 设计预览 | 参考项目和外部工作流常用 browser companion 展示候选状态 | 未证实 Aleph 有统一 companion 启动入口；`artifact_publish` 已有结构化通知基础 | 把 companion 作为 `design_companion` 资源，通过 Aleph 工具发布到右栏 |
| 资源通知 | wove 等工作流可把产物附着于当前任务上下文 | `artifact_publish` 可读取当前 turn 的 `session_key` 并发布 artifact 通知 | 扩展为任务绑定的页面／预览资源，不把 URL 当成唯一契约 |
| 模型／CLI边界 | 外部代理工具可能自行打开浏览器 | GPT／Claude 在源码中是 provider 体系；未证实外部 CLI 是 Aleph 可控执行器 | 仅对 Aleph 管理的工具路径提供右栏保证，外部 `open` 标记为不受控 |
| 网络安全 | 浏览器 Fetch 拦截可作为逐请求拒绝点 | `network_policy` 目前无 task/origin/port 维度；mock worker 才启用 Fetch；obscura Fetch 不可靠 | 先做丢弃式 Spike；未通过则拒绝该引擎的精确预览授权 |

### 3.1 已核查的源码事实 / Verified source facts

- `src/browser/network_policy.rs:149-244` 当前有全局 `block_private`、域名表与 HTTP(S) 检查，不足以表达 task/origin/port grant。
- `crates/aleph-cdp/src/methods/fetch.rs:1-49` 提供 Fetch 能力；`src/browser/cdp_backend/routes.rs:373-533` 可在暂停请求时执行 SSRF 检查和 `Fetch.failRequest(BlockedByClient)`。
- `src/browser/cdp_backend/routes.rs:550-624` 显示上述 worker 当前随 mock rule 生命周期启动；无规则时关闭，不能直接宣称是持续安全边界。
- `src/browser/cdp_backend/navigate.rs:186-244` 与 `src/browser/post_nav.rs:1-130` 主要覆盖工具主导航前检和最终落点审计；页面自导航、普通子资源与 WS 尚未得到完整持续保护证据。
- `src/browser/engine/capability.rs:260-294` 记录 obscura v0.2.2 的 Fetch 拦截不可靠；`src/browser/engine/obscura.rs:153-219` 的私网开关是进程启动级选项。
- `src/builtin_tools/artifact_publish.rs:33,143` 已有当前 turn／`session_key` 到 artifact 通知的接线基础。
- `src/agents/runtime.rs:16` 与 `src/providers/adapter.rs:44` 证明 provider 运行时存在；未证明 GPT／Claude 是 Aleph 可管理的外部 CLI。

## 4. 目标架构 / Target Architecture

### 4.1 核心对象 / Core objects

实现阶段应把下列概念作为同一资源模型的一部分，具体 Rust 类型名由计划阶段根据现有领域模型确定：

- **BrowserSession**：Gateway 上实际执行的浏览器会话。
- **BrowserTarget**：session 中的页面／target；Panel、TUI 和 companion 不创建重复 target。
- **PreviewResource**：任务向展示端发布的结构化资源。
- **ViewerGrant**：授予 Panel 或 TUI viewer 访问指定 session/target 的短期授权。
- **PreviewNetworkGrant**：开发预览可访问的 scheme、host、port、origin 与必要协议范围。
- **CompanionState**：设计候选状态及其资源元数据，不拥有独立的浏览器会话。

`PreviewResource` 至少应表达：任务标识、session、target、资源类型、展示策略、来源、过期时间和敏感字段脱敏状态。资源类型包括：

- `page`：普通网页；
- `dev_preview`：开发服务页面；
- `design_companion`：mockup、布局对比、多屏定位或动效状态；
- `artifact`：静态设计产物、截图或其他可展示文件。

不得让展示端通过聊天文本猜 URL，也不得让裸 URL 成为唯一的授权凭据。

### 4.2 数据流 / Data flow

```text
Agent / Tool
    │  structured preview resource
    ▼
Gateway task + BrowserSession + BrowserTarget
    │  resource notification (task/session/target/grant)
    ├──────────────► Panel Chat right rail
    │
    └──────────────► TUI viewer handoff
                         │
                         └── local default browser / selected Chrome
```

Panel 右栏消费当前任务资源通知并绑定 viewer；TUI 消费同一资源并兑换受限 ViewerGrant。两者不能通过 URL 自行推导另一个任务的 target，也不能调用通用 Gateway RPC。

### 4.3 Panel 右栏 / Panel right rail

Panel 的职责是：

- 展示当前任务绑定的 browser target；
- 渲染页面、开发预览和 design companion；
- 展示候选布局／多屏／动效状态的切换；
- 发送显式的导航、接管、交回、刷新等用户动作；
- 呈现授权、断连、加载和错误状态。

Panel 不负责：

- 解析聊天文本中的隐含 URL；
- 自己创建第二个浏览器会话；
- 直接执行桌面系统打开命令；
- 把 mockup 状态存在独立于任务的临时 session 中；
- 关闭全局网络安全策略来满足预览。

### 4.4 TUI 独立 viewer / TUI standalone viewer

TUI 通过 interface 层使用既有平台打开能力：

- 默认打开用户机器默认浏览器，或显式指定且已安装的 Chrome；
- 打开的是短期、单任务、单 target 的 viewer 入口；
- viewer 兑换后从地址栏／日志中清除敏感交接凭据；
- 可查看画面，并按显式权限决定是否允许接管；
- 打开失败检查进程退出状态，并返回可手动操作的安全入口；
- 绝不回退为打开 Panel Chat。

ViewerGrant 必须由服务端校验主体、task、session、target、权限、有效期、一次性交接和撤销状态。它不能兑换成通用 Gateway RPC 权限，也不能订阅其他任务的帧。

## 5. 设计 Companion / Design companion

### 5.1 统一入口 / Unified entry

Panel 设计流程中的 mockup／对比图不得通过外部 `--open` 作为主要入口。Aleph 管理的 companion 流程应：

1. 创建或发现 Gateway 可访问的设计预览服务；
2. 以 `design_companion` 发布结构化资源；
3. 由当前 turn／task 的资源通知将它送入右栏；
4. 在同一个 BrowserTarget 内展示候选窗口布局、横排／多屏定位和动效状态；
5. 通过 HMR 或刷新更新同一 target，而非创建副本。

### 5.2 外部脚本边界 / External-script boundary

只读调查未发现 Aleph 已有统一 visual companion 启动器、`--open` 接管器或系统级 `open` 拦截链。GPT／Claude provider 的存在不能证明外部脚本受 Aleph 控制。因此：

- Aleph 工具发布的 companion：必须进入右栏；
- 未托管脚本自行执行系统 `open`：标记为外部不受控路径，不伪装为右栏已接管；
- 后续若需覆盖外部 CLI，必须单独设计、评估平台权限和安全边界，不隐含在本规格中。

## 6. 开发预览网络边界 / Development-preview network boundary

### 6.1 授权模型 / Grant model

目标是最小授权：按 task 绑定开发服务的 scheme、host、port 和必要协议，限制跨 origin redirect、子资源、iframe、fetch 及 HMR WebSocket。授权应随 task/session 生命周期撤销，而不是修改全局 `block_private`。

### 6.2 Spike gate

实现产品代码前，必须获得单独授权执行丢弃式安全 Spike。Spike 使用隔离 Chromium profile、本机测试 HTTP/WS 服务和隔离进程，验证：

1. 授权 document、子资源、iframe、fetch、redirect 和 HMR WS；
2. 未授权私网、metadata、跨 origin redirect 是否真正到达目标服务；
3. mock rule 增删是否改变 Fetch 拦截生命周期；
4. Chromium 与 obscura 的实际行为和能力差异。

Spike 不读取日常浏览器 profile、不访问真实内网、不修改 main，探针代码不进入产品。若某引擎不能持续执行逐请求边界，则该引擎不得以“精确预览授权”模式默认启用；不能以全局放开私网作为替代。

### 6.3 已知限制 / Known limitations

当前源码不能证明单一任务可同时保证：精确 scheme+host+port、redirect hop、子资源、跨 origin、WS/HMR 和 obscura 支持。因此规格只定义安全门槛，不预先承诺 Spike 通过后的具体引擎数量。

## 7. 双线实施边界与合并顺序 / Two-lane implementation boundary

### 7.1 线一：Gateway/Core/Panel / Lane 1

负责 BrowserSession／BrowserTarget／PreviewResource／ViewerGrant 契约、Gateway 生命周期、artifact 资源投影、Panel 右栏展示及 Chromium 端能力接线。不得写 TUI 平台打开实现。

### 7.2 线二：TUI/外部 viewer/文档 / Lane 2

负责 TUI viewer 交接、默认浏览器／指定 Chrome 的失败反馈、跨端 viewer 状态和相关文档。不得修改线一的核心 session 契约；如需契约变更，先停线并通过接口评审。

### 7.3 合并顺序 / Merge order

1. 先确定并评审资源／授权契约；
2. 线一完成并验证核心生命周期；
3. 线二基于稳定契约接线；
4. 每完成 2–3 个独立任务再统一编译，避免无意义重复构建；
5. 任何一线发现安全边界或共享文件冲突，停止竞争修改，先合并契约变更。

## 8. 迁移与清理 / Migration and cleanup

实施时必须定位并清理：

- Panel 内重复 iframe／裸 URL 预览入口；
- Panel 直接执行系统浏览器的路径；
- 设计 companion 自己维护的临时 session；
- 只发裸 URL、无 task/session/target 绑定的 artifact 通知；
- 未检查进程退出状态却报告打开成功的路径。

若旧入口仍有外部消费者，短期保留兼容适配器，但它必须转换成 `PreviewResource`，并记录迁移和删除条件。不得保留两套长期真相源。

## 9. 错误处理与可观测性 / Error handling and observability

错误必须沿资源链返回并可定位到 task/session/target：

- 资源无效或过期：拒绝展示并提示重新发布；
- 授权不匹配：服务端拒绝，不仅隐藏 UI；
- 浏览器断连：右栏／viewer 显示断连状态，不创建隐式副本；
- TUI 打开失败：记录真实退出状态，返回安全手动入口；
- 网络授权不足：拒绝请求并记录命中规则；
- 未知引擎能力：能力门控拒绝，不静默降级为全局放开；
- companion 服务退出：显示服务状态，不报告页面仍可用。

日志和通知不得泄露 ViewerGrant、一次性交接凭据或未脱敏的内部地址。

## 10. 验收与验证 / Acceptance and verification

### 功能验收 / Functional acceptance

- [ ] Panel 打开普通网页、开发预览和 design companion 均进入右栏。
- [ ] mockup、候选布局、横排／多屏定位和动效状态可在右栏展示。
- [ ] HMR／刷新沿用同一个 BrowserTarget。
- [ ] TUI 打开默认浏览器或指定 Chrome，展示同一 Gateway session/target。
- [ ] TUI 不回退打开 Panel；接管、交回、撤销状态可见。
- [ ] 外部脚本系统 `open` 未被误报为已接入。

### 安全验收 / Security acceptance

- [ ] Spike 以目标服务是否收到请求为判据，而非只检查调用是否发生。
- [ ] document、子资源、iframe、fetch、redirect、HMR WS 的结果均有记录。
- [ ] 未授权私网和 metadata 请求被验证为拒绝，或能力被明确禁用。
- [ ] mock 生命周期不会无意关闭必要的安全拦截；若无法证明，则不启用该组合。
- [ ] 未使用全局关闭私网保护替代任务级授权。

### 工程验收 / Engineering acceptance

- [ ] Rust compile/test/clippy 按项目最小可信验证集通过。
- [ ] Panel/WASM 与 TUI 的真实交互验证完成。
- [ ] 至少验证默认浏览器和已安装 Chrome 的打开成功／失败路径。
- [ ] 旧入口、死代码和重复资源状态已清理或有迁移记录。
- [ ] `docs/reference/FEATURE_LOCATOR.md` 与相关 reference 文档已补充入口、约束和验证方法。
- [ ] 变更在 worktree 分支提交，未修改 main。

## 11. 待用户确认的后续门槛 / Remaining gates

1. 用户审阅并批准本文规格；
2. 单独明确是否批准安全可行性 Spike；
3. 若 Spike 结果改变引擎或网络边界，回写本文并重新审阅受影响章节；
4. 规格批准后调用 `writing-plans` 生成实施计划；
5. 实施、测试、reference 文档补充和最终 review 分开验收。

在上述门槛完成前，不得把本文描述的目标架构写成“已实现”。

## 12. 实现后明确不做的事情 / Explicitly not done by this specification

本规格没有实现产品代码、没有安装依赖、没有启动浏览器或测试服务、没有执行安全 Spike、没有声称外部 GPT／Claude CLI 可被拦截，也没有承诺 obscura 或 Chromium 已经满足完整的任务级网络隔离。上述事项属于后续批准后的计划和验证范围。
