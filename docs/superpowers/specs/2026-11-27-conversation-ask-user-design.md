# 对话内 `ask_user`：有身份的回答与提交 / Addressed replies and submission for conversational `ask_user`

## 目标与边界 / Intent and scope

让 Agent 在对话中提出一到四题，用户可以单选、多选或自填，在确认后把准确的答案交给原来等待的工具调用；也可以取消。即使不同设备同时开着，旧卡、重复点击和迟到确认也不能回答另一题。首批接通现役 Leptos Panel、TUI 和 Channel；不接迁移中的 TSX Panel，不复制 rpiv-mono 的预览、批注或键盘布局。问卷确认只是提交答案，不是执行授权；不改 `exec` 审批、scratchpad 计划决策规则或 workflow `clarify` 的持久任务语义。

Let a parked tool receive the user's exact choices or custom text, after deliberate submission, or receive a cancellation. A stale card must not answer another question. Ship on the current Leptos Panel, TUI, and channels. Do not port TSX, previews, notes, or the reference UI. Submitting a questionnaire does not grant permission. Keep execution approvals, scratchpad decision rules, and durable workflow clarification separate.

## 对比与方案 / Reference and approaches

参考 [rpiv-mono `7c669d0`](https://github.com/juicesharp/rpiv-mono/tree/7c669d0837cd08497dc34c53a7a17fdfa1a949ee/packages/rpiv-ask-user-question)。其思路是先开等待者再展示、按工具调用定位、校验答案、显式提交或取消。Aleph 已有 `ask_user` Tool Capability、`ClarificationManager`、`clarification.pending`、run 事件和一次性等待者，不需要另设注册表。

| 对比 / Area | rpiv-mono | Aleph 需要改变的点 / Aleph change |
|---|---|---|
| 选择和自填 / Choice and custom text | 独立表达 / Distinct inputs | 保留选项索引或文本，不再经由同一数字字符串猜测 / Stop encoding both as strings |
| 多题 / Questionnaire | 审阅后提交，外部批量答案先校验 / Review and validate before submission | 复用已有 cursor；富客户端原子提交剩余题，纯文本继续逐题 / Reuse cursor for atomic rich submission and sequential text |
| 并发 / Concurrency | 按工具调用寻址 / Addressed by tool call | 用现有请求和当前题身份拦住旧卡 / Enforce existing identities consistently |
| 取消与无 UI / Cancel and no UI | Dismiss or fail early | 复用等待者、超时、停止和终止事件；增加主动取消 / Reuse lifecycle and expose deliberate cancel |

**选择（用户确认） / Selected:** 现役三端共用一个 Core 契约，先复用并接通，再加富端审阅。兼容读侧投影和带双重守卫的旧回答；未升级的缓存 Panel 若不带题 ID，必须刷新再答，不为兼容而放行。**备选 / Alternative:** 同次接 TSX，覆盖更完整但要独立验证另一套 UI 的收发、重连和敏感输入，容易延误现役修复。只补旧字符串判断则无法确知数字自填与选项的区别，不能达到目标。

## Core 与协议 / Core and wire contract

- **事实源 / Authority.** 保留 `src/clarification/ask.rs` 的注册→送达→`ToolCallParked(Clarification)`→等待路径、600 秒时限、不可送达与任务丢弃的收尾。唯一在世记录仍在 `src/clarification/session.rs`；注册新请求替换旧请求；不新建调度器、持久回放、授权账本或 answer channel。`ask_user` 保持现有 Capability 注册；Gateway 继续按会话可见性授权 `clarification.pending/resolve`，不从客户端收 agent/run 路由或选项 value。 / Keep the existing registration, delivery, park evidence, timeout, cleanup, and sole live entry. Keep capability registration and existing session visibility controls; never trust a client-supplied value or route.
- **有类型的动作 / Typed actions.** `shared/protocol` 提供一个 serde 契约。例：`{"session_key":"…","expected_request_id":"…","expected_question_id":"q2","action":{"kind":"submit","answers":[{"question_id":"q2","answer":{"kind":"text","text":"1"}},{"question_id":"q3","answer":{"kind":"choices","indices":[0,2]}}]}}`。`submit` 按当前顺序完整覆盖所有未答题。`answer` （`{"kind":"answer","answer":{…}}`）只回答当前一题，可推进；供逐题接线或即时输入用，富客户端问卷卡最终按 `submit` 提交。`cancel`（`{"kind":"cancel"}`）弃掉整项请求，不交给模型任何部分答案，不停止整轮。三种动作外层均要求非空请求 ID 和当前题 ID；`submit` 须有题 ID，不接收客户端提供的标签、value 或 `answered` 计数。 / All actions require both current identities. Submit covers exactly the ordered unanswered suffix; answer advances one question; cancel discards the whole request without stopping the run.
- **类型与边界验证 / Validation.** `choices` 用 0-based 索引。单选恰好一个；多选至少一个且不允许重复；越界或对无选项题提交 choices 都无效。`text` 不走选项解析，拒绝纯空白，保留用户输入原文。富端多选与自填是二选一，不合并为含歧义的字符串。Core 按注册的 options 映射索引和值，保留现有模型输出 `answer`、`selected_index`、`answers[].selected_indices` 和 `withheld`；不增加模型可伪造的用户确认字段。 / Selection cardinality, uniqueness, range, and nonblank custom text are checked in Core. Core derives model-facing values; the question UI chooses between custom text and choices.
- **锁与回执 / Atomicity and acknowledgement.** 在 manager 原有写锁内检查身份、生存期、当前题和整组答案，先构造并验证全部回答再修改。身份不符（含重复旧题提交）返回 `stale`，不消费、不前进；当前目标匹配但内容非法返回 `INVALID_PARAMS`，不消费、不前进。只有 oneshot 确认送达，才回 `answered` 或 `cancelled`；等待者竞态关闭时回 `stale` 并收尾。推进回 `advanced`。用 `status` 表达这四种结果；保留旧 `resolved`（仅 `advanced/answered`）和 `pending_questions/stale` 作为从同一结果投影的兼容字段。事件仍在锁外发。没有新请求时、过期和已放弃沿用现有回收及原因。 / Check and apply under the existing lock, and validate the full batch before mutating. Stale is not invalid. Terminal success requires delivery to the waiter. Compatibility fields are derived, not independent facts.
- **兼容 / Compatibility.** 在同一 RPC 解码旧 `reply`/`answers`（互斥且只接受显式非空数据），两项 guard 同样必需；旧字符串 `answers` 仍按当前题顺序推进，可交部分但不能超额，整个旧批次也先校验再应用。`action` 与旧字段不可混用。只有 Channel 的普通文本与受守卫的旧 RPC 继续走原有数字/标签/逗号解析；不改 workflow `clarify` 的用法。不接受旧无身份按钮执行写操作。`clarification.pending` 和事件上的旧 flat 投影仅保留读取兼容。 / Keep guarded legacy replies and read projections; remove unguarded gateway mutations. Plain text and the old guarded format retain their legacy interpretation.
- **工具入参 / Tool arguments.** 保留 `question/choices` 和 `questions` 两种生产形状及现有上限；拒绝混用 flat 与 grouped 字段、空 prompt/ID、重复 ID、空选项标签和值、同题大小写折叠后的重复标签或由两个选项共享的文字别名。验证贴近 `ask_user` 输入边界，避免收紧共享的 scratchpad/workflow 构造器。secret 隔离、withheld 说明和无交互快速失败不退化。 / Validate the two existing argument shapes at the tool boundary without changing the other producers.

## 三端接线 / Surface behavior

**Panel（Leptos）和 TUI。** 共享协议 DTO；各自保留现有窗口、modal 或 chat card、输入和可达性逻辑。选择只更新本地草稿，不发 RPC；可以在未提交题之间编辑。预览示意（敏感题的回顾只标“已填写”）：

```text
部署范围 / Scope    [x] 测试环境   [ ] 生产环境
交付说明 / Note     自填：保留旧版本 / Keep the previous version
凭据 / Credential   ••••••••（已填写 / Entered）
[提交答案 / Submit answers]    [取消提问 / Cancel question]
```

使用当前 `request_id`、`question_id` 和 per-question draft 发一次 guarded submit/cancel。发送期间同步防重复，并禁用所有提交触发器。取消是明确按钮或可见按键；TUI Esc 保留既有隐藏/可重开语义，不把临时关窗当作取消，运行停止仍走原路径。普通聊天 composer 不再暗中代答或把拒答回落为 chat.send；有未答提问时引导到卡片，不清空原普通聊天草稿。敏感输入只在对应的掩码控件，不回显到聊天、审阅文字、错误或日志；错配或结果未知也不能自动发普通消息/自动重试。保留既有模型可见的回答边界，不承诺改造持久存储或对模型隐藏秘密。

**Panel and TUI.** Use the shared DTO, keep the existing rendering and interaction owners, stage all outstanding answers locally, make the user submit once, and offer explicit cancellation. Keep Esc-as-hide in TUI. Do not divert the chat composer into a question reply or reclassify a failed reply as chat. Mask sensitive drafts and summaries; preserve the existing tool-result boundary, not a promise about transcript retention inside Core.

**投影与重连 / Projection and reconnect.** push 是通知，`clarification.pending` 是当前状态的权威快照。已知同一请求的 cursor 只前进；旧终止事件只影响精确匹配的 `(session_key,request_id)`，不按 session 全删；空身份终止、冲突 ID、迟到 ack 和网络不确定都走 reconcile。现有卡片控制器对事件之后返回的旧 fetch 回执做代际校验；终止通知不要被迟到的同 ID AskUser 复活。重连订阅后拉快照，保留同身份、未前进题的本地草稿；目标已经推进或替换则丢弃不适用的草稿。失联期间不重放 mutation；快照确认还在原 cursor 后，用户可以重新提交。 / Pending snapshots reconcile pushes and uncertain delivery. Match terminal frames exactly, ignore regressing cursors and out-of-order fetch results, discard obsolete drafts, and never automatically replay a mutation.

**Channel。** 保留现有文字渲染与逐题回复。inline 回调从 `clarify:<index>` 变为 `clarify:<32-hex-request-uuid>:<cursor>:<1-based-index>`，不塞入任意长度的模型题 ID；Core 同锁检查 UUID 和 cursor，按位置选择，不经 `interpret_reply`，有 UI 限制的通道仍能装进 64 字节。旧数字回调可提示按钮已过期但不得写入或当普通消息。没有菜单或不支持键盘时，仍用文字菜单；多选仍用逗号或自填。保留并发文本消息无法区分引用哪条旧消息的限制：普通回复只针对收到时在世的当前题，不声称具备旧卡保护。Channel 支持明确的 `/cancel-question`（只取消当下会话的等待项，不等价 `/stop`）与 `/question`（重新查看当前非敏感题，弥补下一题送达失败；不消耗回答，不自动重试）。在已有互动/命令入口中接这两个精确命令，不重写斜杠命令分派；帮助文字让用户知道恢复路径。不得把仅限 Panel/TUI 的敏感题发布到 Channel 或把拦下的敏感答复落为普通聊天。 / Keep sequential text fallback. Use compact request/cursor callbacks and direct indexed selection. Consume stale legacy callbacks without action. Provide explicit cancel and re-display commands, with the known limitation that unaddressed plain text targets the currently live question. Never expose a secret question or a rejected secret reply to chat.

## 预计删除、验证和交付 / Removal, verification, and delivery

- 删除客户端逗号字符串选择编解码、Panel 重复手写字符串 resolve DTO 和 composer 隐式回复/回落分支、裸数字 `clarify:` 回调及先查 pending 后按裸数字 resolve 的分叉。保留生产挂载、纯文本解析、当前卡片结构、审批与计划 gate；不清理与交付无关的历史事件。 / Remove dead encoders, duplicate DTOs, composer interception/fallback and identity-less callback parsing; preserve mounts, text interpretation and distinct approval paths.
- 回归覆盖：数字/逗号自填与索引分离；重复、越界、缺失/错序/超额答案无局部提交；两客户端同答只推进一次；旧请求/旧 cursor 的按钮与取消不影响后继；只有发送到等待者才算终止成功；终止帧只收精确匹配的卡；断线不重发、掩码答案不回显或回落；Channel 能逐题推进、重显与取消；非交互、超时、withheld、计划 gate 和审批无回归。至少一条真实 Gateway 驱动验证工具的等待者收到正确结果，不能只断言调用发生。更新现有 `qa/human_gate` 与 `qa/plan_handoff` 驱动，避免旧无题 ID 的请求在新版被拒。 / Exercise exact waiter results, negative and racing inputs, recovery, each surface, and separation from permissions. Update the existing QA drivers.
- 执行 `just test-shared`、Core 相关单测与 `cargo test -p alephcore --lib --no-run`、`cargo test -p aleph-panel --lib`、Panel WASM 构建和可用的运行态 QA；记录无法访问的第三方 UI/真机，不把源码探针当成真机验收。更新 `docs/reference/FEATURE_LOCATOR.md` §5.3，并按实际改动校对互动/工具/QA 入口，区分历史证据与当前实现。全程只在 `refactor/conversation-ask-user`，不在主分支工作。 / Run shared, Core, Panel, WASM and available runtime gates. Report missing real-device evidence. Update the current reference and QA docs, then commit on the isolated branch only.

实现计划决定具体 Rust 类型和本地草稿存储位置、语言串与按键、测试装置与工作分拆；不允许改变上面的身份、确认、取消、可见性或实测要求。 / The plan may choose type layout, draft storage, UI details, and test mechanics; it may not weaken the contracts above.
