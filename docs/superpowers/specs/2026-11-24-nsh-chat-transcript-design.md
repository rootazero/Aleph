# NSH Chat：授权呈现流 / Authorized Transcript Stream

状态 / Status: independent builder check completed; not approved for implementation.

## 目标与范围 / Intent and scope

把 NSH 聊天中的实时正文、工具活动和最终历史变成同一份可读转录。默认看摘要而不是整块输出；需要时从同一张卡展开，或打开独立全屏阅读视图。补齐 Markdown、Edit/Write 实际改动和最近一次模型请求的 Context 计量。参考终端项目的交互，不复制终端鼠标坐标、工具包装或估算逻辑。目标是 NSH 原生 TSX 与 `/v1/panel/` 的同源 Web fallback。保留默认挂载、发版和切换门禁；不改 Tauri、旧 Leptos UI、TUI 行为、工具执行和模型上下文。不生成额外 LLM 摘要，不新增通用事件存储、Capability registry 或外部服务。

Make live text, tool activity and persisted history one readable transcript. Show compact summaries by default, then reveal detail in place or in an immersive reader using the same cards. Support Markdown enhancements, actual Edit/Write changes and last-request Context accounting. Scope: native NSH TSX and its same-source Web fallback. No default mount or release cutover, legacy client rewrite, execution changes, extra model call, event store or registry.

## 方案选择 / Approaches

1. **授权呈现流（已选）/ Authorized presentation stream (selected).** Gateway 负责授权和数据最小化；同一个 Rust UI 模块消费有界的类型化数据；TSX 持有短命交互状态。比直接开 topic 多一个契约，但不用复制 reducer，也不会把 raw 参数变成客户端 API。 / Gateway owns authorization and minimization; shared Rust derives rows, TSX owns ephemeral interaction. The extra contract prevents raw execution payloads becoming client APIs.
2. **Gateway 投影行 / Gateway-rendered rows.** 客户端更薄，但服务端必须理解界面行生命周期，视图改动会牵动 Core。 / A thinner client at the cost of server-side UI row lifecycle and coupling.
3. **只做历史增强 / Snapshot-only upgrade.** 改动面最小，但工具不能即时交错呈现，不能满足任务。 / Lower change surface; misses live tool chronology and activity.

证据 / Evidence: `pi-cc-extensions` (`5e3ef6b`) 的工具摘要、点开卡片、限量展开和流结束后制图值得借鉴；它通过 wrapping 捕获 Write 旧内容，Aleph 不需要。`pi-claude-code-tui` (`e85276b`) 的短行摘要值得借鉴；它继承 Pi 的展开行为，不实现 Write before/after 和分项 Context。两者都不是 Aleph 的授权或数据源。 / Borrow compact rows, interactive expansion and deferred diagram layout; use Aleph's existing authorization and tool facts instead of wrapping tool execution or importing terminal renderers.

## 契约与数据流 / Contract and data flow

- **授权 / Authority.** 在 `src/gateway/server/connection/event_admission.rs` 的共享 IPC/WS 边界使用既有 owner、project、allowed run 和连接代次检查。历史、详情、Context 复用既有 handler 的地址解析与读权限，只有明确批准的 RPC/字段进入 V1；不改为 V0 或加 admin 旁路。相同的 UI 呈现安全规则覆盖实时、回放、错误与分页：默认拒绝 raw 参数、模型推理文本、内部 trace 和任意 JSON；工具显示参数只从安全字段生成摘要，未识别工具显示名称/状态，正文和详情沿现有安全边界。脱敏在裁剪、渲染及日志之前；不得先切页再遗漏跨页秘密。先验收另一会话、无授权项目、连接重绑与秘密样本都不可见，再开放入口。 / The shared IPC/WS boundary enforces owner/project/run/generation policy; existing addressed handlers remain authoritative. Promote only proven-safe RPC/fields. Preserve user-visible text, not raw execution payloads or private reasoning. Apply masking and bounds before disclosure, across live, replay and detail.
- **身份和顺序 / Identity and order.** 逻辑载荷为授权会话、run、现有源身份/顺序、类型化 payload：可见正文增量、工具开始/进度/结束、运行生命周期。一个工具活动只认一条实时来源（不同时消费语义重复的 `Tool*` 与 `AgentTrace`）；工具行按 `(run_id, tool_call_id)` 更新，正文按 run 内段落身份更新。不要把源 `seq` 跳号当成丢帧：过滤后的序列可能天然不连续。重复增量不能拼两次，完成状态不能被晚到开始覆盖。未等到 send 应答时沿用早到缓冲，但设大小/寿命上限；只有确认 session/run 归属才合并，超限则对账。切会话、断线和身份换代清空未确认数据并丢弃旧异步响应；未知 send 结果不自动重发，不伪造失败。 / Use stable run/call identity and source order. Consume one tool event source, dedupe deltas, keep terminal tool state monotone, and buffer early send events only within caps. Gaps in a filtered source sequence are not proof of loss. Fence all delayed replies and never blindly retry an unknown send.
- **历史收敛 / Reconciliation.** 现有 `chat.history` 是持久化消息事实源，`trace.by_runs` 仅补授权 run 的细节。按 history 实际返回的 run 身份做有限批量回填；同 run 多段正文不因回放重复，历史正文与 trace 矛盾时采用历史正文，旧条目缺 run 则保留简单正文。活动 run 的 live staging 不因旧 snapshot 丢失；完成、已知流损失、重连后刷新，以原子换窗方式替换已持久化部分。`trace` 查不到或超出预算，仍显示正文；未落盘、丢失或被遮盖的细节不能靠 UI 推测。 / Persisted history wins for final messages; trace enriches owned runs without duplicating text. Refresh after completion, interruption or reconnect, prefer atomic window replacement, retain in-flight rows until proven persisted, degrade to text when trace is unavailable.
- **共享 UI / Shared UI.** 在 `interfaces/panel` 现有 Rust/WASM UI set 中桥接 `shared/ui_logic/src/transcript/{reducer,view_model,summarize,diff_view,md_enhance,context}.rs`。若 reducer 目前只接受 raw 参数才能生成摘要，抽出接受已授权 `CallSummary` 的类型化入口，而非展宽 wire；同一个 `summarize` 在授权边界复用，不在 TS 重写。保持每会话/页面生命周期自己的模型；增量更新只回传变更的行，不为每 chunk 跨边界重传整个历史。TSX 负责视图组成、焦点、折叠、滚动、分页请求；Rust/TS 任一侧都不是第二业务/授权权威。 / Bridge shared transcript logic via the existing Rust/WASM UI set. Accept pre-authorized summaries where necessary; avoid full-history serialization per chunk or a duplicate TS reducer. TSX owns layout and interaction only.

### Builder check：实施边界 / Implementation rulings

- **V1 只增加展示字段 / V1 display only.** 不直接开放 `trace.by_runs` 的原响应（其中 attended `result.output` 不保证脱敏）。做面向 V1 的类型化呈现读取，复用 existing handler 的 session/run ownership 查询与 trace 事实源；现有 V0/CLI/TUI 原契约不变。实时与回放共用一份工具字段投影：状态、耗时、受限的摘要和脱敏 typed diff；工具进度只有状态，不透传自由文本；仅确认安全的摘要字段可入选，未识别或含敏字段退为工具名/状态。普通工具文本走已脱敏的 `trace.tool_output` 按需分页；历史和实时预览不另带 raw output、内部错误、原始参数或推理文本。分页按现有 masked byte offsets；`total_bytes` 变化时重启，不拼接不同代的页面，expired 有明确提示。 / No raw trace RPC exposure or legacy contract change. One tool projection for live and replay, sanitized typed diffs, status-only progress, and paged masked output on explicit demand. Restart paging when the masked length changes; label expired data.
- **正文语义不变 / Existing assistant text contract.** `stream.response_chunk` 与 `chat.history` 的用户可见正文沿用当前 S3 政策；本次不增加逐 chunk masking、跨 chunk 扫描或延迟缓冲。这里的字段最小化与脱敏约束针对本次新增的工具/trace 数据，不宣称模型生成的正文对秘密有新的安全保证。若另立正文 DLP 需求，必须同时设计实时缓存和历史一致性。 / No new content-DLP guarantee or buffering for already admitted assistant text; the new disclosure rules govern tool and trace data only.
- **run 级对账 / Run-level replacement.** 保证 live 的到达顺序、tool call key 和最终正文一致，不承诺跨 live 与 snapshot 的文字段落有相同 identity 或像素级顺序。当前 history 只给 `run_id`，没有可证据化的逐消息 join key：用可靠 trace 时保留已知顺序，无法可靠归因时保留所有历史正文，不模糊匹配、不为此新增执行事件或持久键。捕获/恢复读者锚点是交互保证，不变成正文身份保证。 / Preserve observed live order and call identity; later replace the completed run atomically. No fabricated per-message join across history and live.
- **重新附着不补未落盘帧 / No unpersisted catch-up.** 打开活跃会话或重连后显示已提交快照和后续已授权帧，明确标记此前实时片段可能不完整。终态、重连和已知溢出触发对账；没有终态时保留诚实的中断或部分状态。没有新增 durable cursor，也不为“立即补齐”缓存私有运行内容。 / On reattach, show persisted content and newly admitted activity, visibly mark any missing live prefix, and reconcile at completion. No durable cursor, private buffer or invented terminal state.

## 用户可见行为 / UX contract

| 功能 / Feature | 验收 / Acceptance |
|---|---|
| 摘要与折叠 / Summaries | 工具开始即有可点击的简洁行，完成就地更新状态和耗时；默认折叠，连续工具可以按共享分组规则呈现，失败不会藏在成功摘要里。键盘可达；无输出或已裁剪须有明确状态。 / Compact actionable row on start, update in place, sensible grouping, keyboard action, explicit empty/truncated states. |
| Edit / Write | UI 消费 `aleph_protocol::file_change::Presentation::FileChanges` 的文件、hunk、行与已有截断标记；用共享 inline span 做增删及行内高亮。无 typed diff 则提示并展示可用的普通输出，不重读文件、不重算伪 diff。 / Render actual typed changes; never recreate a diff from the user filesystem or guessed inputs. |
| Markdown | 流式正文走 NSH `TextView`，保留代码块、链接与硬换行。复用 shared enhancer 的识别/归一化和 NSH 自带 alerts；同一提示块不做两次改写。Bare HTTP(S) 仅在合法 Markdown 上下文链接化，禁止脚本协议及任意 HTML。Mermaid 使用可信解析产生的类型化区块，不从正文识别可伪造的 HTML placeholder；只在封闭且稳定的 fence 后处理；首版支持 `flowchart`/`graph` 的 TD/LR 基础节点与连线，使用有界的共享纯数据布局和 NSH 原生视图；不支持/超限/失败时显示标注过的源码。禁止 CDN、外部资源、回调、HTML labels。 / Streaming TextView, shared safe enhancement, bounded basic Mermaid flowcharts with a readable source fallback. |
| 全屏 / Fullscreen | 在 Panel 内容中打开沉浸式转录阅读器，不改变 OS 窗口；与内嵌视图共用行组件、行身份与展开状态。按钮/Escape 返回，恢复发起控件焦点，阅读位置不强制跳到最新。 / Immersive panel reader, not OS fullscreen; same cards, expansion state and keyboard interaction. |
| 长输出 / Long output | 用同一个 NSH 虚拟 `List` 渲染 live 和历史。底部附近持续跟随；用户离底后保持锚点，显示“回到底部”，点击后恢复跟随。仅展开时请求 `trace.tool_output` 分页，禁止全量预取；切换/收起后忽略过时结果。Hover/键盘 focus 突出可操作行，diff/代码保持语法或语义高亮；不新增全文检索。 / One anchored list, follow only at tail, explicit jump and lazy detail pages. Highlight hover/focus and diff/code; no search feature. |
| Context | `/context` 概念采用可发现的 Panel 入口，读取 `context.breakdown` 最近一次已记录请求。System Prompt 显示总量与分层；Skill 从已记录的 `skill_instructions` 等可证实层中拆列为 Prompt 子项，不重复加总；Tools 显示工具 schema 各项与总量，区别于对话中的工具结果。provider usage 与本地估算明确标识：RPC 不填 `provider_reported`，仅可在能证明 `ContextGauge` 与 breakdown 属于同一请求时补实际用量，否则总量/占比显示未知。系统动态半段总量以 `dynamic_bytes_sent` 为准，trim 前的分层归因不能冒充实际发送字节；未采集/不适用不伪装成零。 / Last-request accounting; skill subtotal belongs to System Prompt; tool schemas differ from tool-result messages. Use observed provider usage only when correlated with the measured request, otherwise leave it unknown. Distinguish pre-trim attribution from bytes actually sent. |

## 交付顺序与证据 / Delivery order and evidence

1. 先连通授权实时/回放/详情/Context 契约与渲染前的脱敏；对 IPC 和 Web WS 各验证正反两侧。再接共享 reducer，测早到、重复、多消息 run、终态、断线结果未知、角色/会话切换；演示普通聊天和 stop 仍正常。 / Wire existing sources, prove both transport boundaries and ordering before adding visuals.
2. 让 live 与 history 共用一份列表，再挂接 summary/diff、页面详情、全屏和滚动，最后接增强 Markdown 与 Context。移除 `interfaces/panel/modules/Chat.tsx` 的独立 live Text 渲染和已被新模型替代的拼接/重复状态；保留发送、停止、composition 与未知结果语义。 / Connect features to one list; remove superseded preview/string-assembly paths without regressing send/stop/IME.
3. 单测/集成：Rust 权限与 transcript 契约、TSX 原生测试、Web host 测试、`nsh check`。真机：NSH 原生与浏览器使用同一会话场景，工具开始→正文→完成/断线后不重影、Edit/Write 真 diff、长输出锚点、全屏/焦点、Mermaid 成功与回退、正常用户与外会话拒绝、Context 来源标识。测较长脚本与数轮流式，记录行数/响应字节/变更批次；确认没有每个 chunk 重传已完成历史。未运行的环境必须标明。 / Prove actual presentation and denial, not only emitted calls, in native NSH and its Web fallback; record performance/size bounds and untested platforms.
4. 更新 `docs/reference/FEATURE_LOCATOR.md`、`docs/reference/TRANSCRIPT_RENDERING.md` 与受影响的 `docs/reference/DESKTOP_SHELL.md`、`qa/nsh_s3_chat/README.md`；记录新证据，不改写历史 receipt。全部提交在 `chat-stream-experience`，不合并或推送 `main`。 / Update references and QA evidence and commit only in the isolated branch.

## 实施者可裁定 / Builder discretion

可选择 Rust UI set 的句柄形式、稳定 row key 的编码、确切组件拆分和预算值，但需明确定义内存/字节/分页预算、可观察的截断行为和测试。对不支持 Mermaid 语法优先明示回退；若 `List` 的原生与 Web 行为不同，优先用同一 TSX 控件通过 NSH 既有接口兼容；不可另写一套 Web business renderer。任何授权策略变更、默认挂载切换或显著新依赖不在授权范围。 / Builders may choose handles, key encoding, component boundaries and explicit budget values backed by tests, but not loosen access, switch the default mount, add major dependencies or fork rendering semantics.
