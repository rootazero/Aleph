# Aleph Panel 光学界面与思考阅读器 / Optical Panel and Thought Reader

状态 / Status: Provisional design for approval · 2026-11-27

## 目标 / Intent

在目前发版使用的 Leptos Panel 中实现原创光学界面、统一阻尼动效、上下文浅水小瓶、像素伙伴和双模式思考阅读器。用户可以在现有会话里查看真实的上下文占用、核对模型已展示的原文和译文，并按需关闭动效或伙伴。现行 Web 与桌面 WebView 共用同一呈现源，宽屏与手机布局共用组件。

Deliver Aleph's own optical design, damped motion, context vial, pixel companion and dual-mode thought reader in the shipping Leptos Panel. The current Web and desktop WebView mounts share that source. The responsive chat layouts share components and Core keeps authority for context data, session access and translation execution.

## 方案与取舍 / Approaches and trade-offs

| 方案 / Approach | 取舍 / Trade-off |
| --- | --- |
| **现行 Leptos 首发 / Ship in Leptos first**（用户已选择 / selected） | 默认入口直接受益，复用已准入的上下文和思考流。未来 TSX 需要自己的呈现适配，可以复用类型、投影与 Core 服务。 / Ships to today's users via existing admitted streams. TSX will need a renderer adapter and can reuse the projection types and Core service. |
| TSX 首发 / Start in TSX | 直接建设迁移目标。v1 需先新增并验证思考数据投影，成果只出现在显式启用的入口。 / Builds the migration target directly. It needs a new v1 thought projection and verification, and reaches opt-in mounts. |
| 双轨同轮 / Ship on both tracks | 两条入口共同交付，要同时维护两个动画和组件适配并扩大验收面。 / Delivers both mounts together and requires two renderer integrations and their verification matrix. |

参考 `obsidian-prism` `e016f5e3ae607ced36dfd6e5685778dd3d99ae7f` 的分层、运动降级和双语核对方式。Aleph 原创实现、配色、角色和资产保持仓库 MIT 许可。 / The reference informs separation of concerns, motion fallbacks and bilingual verification. Aleph uses original code, palettes, characters and assets under its MIT license.

## 界面与交互 / Interface and interaction

- **光学层 / Optical surfaces.** 扩展现有颜色、层级和密度 token，为导航选中项、聊天输入区、上下文明细和思考卡提供固定的高光、边界及焦点状态。深浅主题各自保持文字对比，窄屏保留布局与阅读空间。 / Build on existing theme and density tokens. Apply bounded highlights to selected navigation, the composer, the context detail and thought card. Preserve contrast and focus in both schemes and usable layouts at narrow widths.
- **阻尼运动 / Damped motion.** 用共享弹簧参数和可中断的生命周期控制承载浮层、展开和状态切换。减少动态效果时直接提交静态终态。绘制预算随实际可见性和停止条件回收。从已改造组件移除被替换的样式和计时路径。 / Share spring parameters, interruption and cleanup across the upgraded surfaces. Reduced motion commits the final state immediately. Visibility and settling stop the frame loop. Remove superseded animation paths in the migrated surfaces.
- **上下文小瓶 / Context vial.** 点按或键盘激活小瓶打开 Leptos 明细浮层，复用已经服务 TSX 与 TUI 的 `context.breakdown` 契约和共享明细投影。数字和可访问名称随数据更新。把 `stream.context_gauge`、终态摘要和授权历史投影归并到同一占用状态，沿用现有会话路由与刷新隔离。每个使用量对应其产生时的真实模型窗口。`512K` 作为真实容量或测试样例出现，模型切换、缺数、缩减和恢复各有显式状态。浅水渲染采用上限明确的逐帧计算，减少动态效果或绘制资源不可用时显示静态水位。 / Open a Leptos detail popover using the `context.breakdown` contract and shared projection already used by TSX and TUI. Merge live occupancy, terminal summaries and authorized history into one projection keyed to the actual window for the measured turn. Cover model changes, unknown values, compaction and restore. Support a 512K fixture and a static accessible level indicator.
- **像素伙伴 / Pixel companion.** 原创像素角色位于输入区边缘，有独立偏好开关，只响应已有空闲、处理和完成状态。它有固定布局上限，移动端可隐藏装饰。切换会话、隐藏页面、静置和卸载释放计时。 / Use an original pixel companion with an appearance preference and bounded placement. Existing idle, active and complete states drive it. Stop timers on session change, hidden pages, settling and teardown.
- **思考阅读器 / Thought reader.** 继续实时显示现有原文，按可识别的思考块组织呈现。保留原文查看，提供沉浸式译文及原文与译文并列的双语模式。当前块失败、模型没有可展示思考或引擎未配置时，保留原文并给出对应状态。思考强度仍走现有 `thinking` 会话旋钮。 / Keep the original live readout, organize it into identifiable thought blocks, and offer translated and bilingual reading layouts. Keep the original reachable through failures and unavailable engines. Preserve the existing session thinking control.

```text
宽屏 / Wide    导航 | 会话 · 原文 / 译文 / 双语
                        输入区                       [小瓶] [发送] [伙伴]
窄屏 / Narrow       会话 · 思考阅读选项
                        输入区                 [小瓶] [发送]
```

## 契约与数据边界 / Contracts and data boundaries

1. 会话与 run 归属、模型窗口和 provider 用量沿用 Core。Panel 共享 Rust 逻辑只合并已授权值，明确区分实测窗口、占用未知和最后一次测量。`context.breakdown` 是明细事实源，各分桶保留测量或估算语义。只有占用快照与明细能通过同一请求标识关联时才合并，其他情况独立展示。 / Keep session and run ownership, model window and measured usage in Core. The shared Rust projection distinguishes a known capacity, unknown occupancy and the last measurement. Detail rows from `context.breakdown` preserve measurement and estimate labels. Combine them with occupancy only when both carry the same request identity.
2. 翻译是 Core 的显式能力。输入定位到会话、run 和一个已完成且原本可向该调用者展示的模型思考块，通过现有 owner/read 检查并从记录解析出源内容。新 RPC 只做类型化接入与投影，复用现有 Capability 发现、授权和调用路径。工具摘要、命令、附件、未发布的推理内容和系统提示均在源选择之外。旧记录或只有临时输出的块仍可阅读原文。 / Translation is an explicit Core capability. Resolve one settled, already-displayable thought block through the existing owner-scoped trace path. The RPC adapts the typed request and response to the existing capability contract. Source selection excludes tool summaries, commands, attachments, unpublished reasoning and system prompts. Older records or transient-only blocks remain readable in the original.
3. **机器 / Machine** 使用操作者配置并授予调用权限的结构化翻译工具 Capability，复用 MCP 工具通路。**AI** 使用已配置的 provider 和 model，复用文本生成流程，启用有界流式输出，固定任务提示和无工具调用。引擎可用性由 Core 显式报告。每个块由用户触发翻译，同轮同引擎去重，切换会话或取消时丢弃晚到结果，设置变化后按新授权重新检查。译文只在当前展示生命周期持有。 / A configured, authorized MCP translation capability supplies machine translation. The AI mode uses a configured model/provider and bounded streaming text generation with a fixed task prompt and tools disabled. Core describes engine availability. Translation is an explicit action on a block, with in-flight deduplication, bounded input/output, cancellation and stale-result rejection. The displayed translation is ephemeral.
4. 两个引擎共用当前 Panel 界面语言作为目标语言。目标语言纳入类型化请求及在途去重键。切换界面语言时取消请求、清除旧译文并拒收迟到结果。展示偏好复用 Panel 现有客户端偏好。引擎选择、凭据与授权复用 Core 配置、provider 和 Capability 边界，按启用的引擎标明接收方。 / Both engines translate into the current Panel interface language. Include the target locale in the typed request and in-flight deduplication key. A locale change cancels pending work, clears displayed translations and discards late results. Local presentation preferences follow existing Panel settings. Engine configuration, credentials and access live in Core. The enabled engine exposes its destination before use.

## 实施顺序与验收 / Delivery and acceptance

1. **连线 / Wire existing facts.** 验证现有实时占用、历史、明细、模型切换和思考来源，收敛会话投影并连接两个响应式入口。 / Trace the live usage, history, detail, model switch and thought sources, merge the projection and wire both responsive entry points.
2. **呈现 / Render.** 加上统一光学与运动层，小瓶替换旧环，接入像素伙伴和设置。保持焦点、点击目标和文本选择。 / Apply the optical and motion layers, replace the ring with the vial and mount the companion and settings. Preserve keyboard focus, touch targets and text selection.
3. **翻译 / Translate.** 接通真实配置的两个引擎，并用本地受控端点覆盖成功、无权限、取消、失败与流式增量。用同一授权入口验证其他会话的 run 无法访问。 / Wire both configured engines and verify success, permission denial, cancellation, failure and streaming against controlled local fixtures. Verify that another session's run remains inaccessible through the same entry point.
4. **清理和回归 / Clean up and verify.** 清理旧环、被取代的动效和重复的会话派生逻辑，保留仍有调用方的缓存及费用显示。执行 `cargo test -p aleph-panel --lib`、Rust/WASM 检查、相关测试和实际本机 WebView 或浏览器验证，记录明暗、窄宽、减少动态、重连、运行切换、未知数据和静置停止绘制的证据。校对 `FEATURE_LOCATOR.md`、转录与桌面相关参考文档并提交工作分支。 / Delete the replaced ring and redundant motion and projection logic while preserving cache and cost consumers. Run Rust and WASM checks, targeted tests and native WebView or browser QA for both schemes, viewports, reduced motion, reconnection, run change, unknown data and idle rendering. Update the locator and relevant references, then commit the work branch.

迁移入口、发版切换和宿主包装沿用已定路线。聊天和展示增强通过既有部署入口交付，性能比较以本次可复现的实测为准。 / The migration, release switch and host packaging follow the existing roadmap. The delivery uses the current entry points and reports only reproducible performance measurements.

## 代码依据 / Source anchors

- 宿主与当前挂载 / Host and current mount. [Desktop Shell](../../reference/DESKTOP_SHELL.md#nsh-panel-migration--panel-迁移) · [Feature Locator](../../reference/FEATURE_LOCATOR.md#6-ui--panel)
- 当前会话呈现与数据 / Current chat rendering and data. [Context gauge](../../../interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs) · [Context measurements](../../../src/gateway/handlers/context_breakdown.rs) · [Session history](../../../src/gateway/handlers/chat.rs)
- 思考源与工具权限 / Thought source and tool authority. [Reasoning card](../../../interfaces/webchat/src/platform/wide/views/chat/reasoning.rs) · [Scoped trace reader](../../../src/gateway/handlers/trace_replay.rs) · [Capability facade](../../../src/capability/facade.rs)
- 参考源码与许可证 / Reference implementation and license. [Prism source and notices](https://github.com/RMvdesi/obsidian-prism/tree/e016f5e3ae607ced36dfd6e5685778dd3d99ae7f) · [Aleph MIT license](../../../LICENSE)
