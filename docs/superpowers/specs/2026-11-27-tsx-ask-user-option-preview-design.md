# TSX Panel 对话问卷与选项预览 / Conversational questionnaires and option previews in the TSX Panel

## 目标 / Goal

在共享 TSX Chat 中接入 `ask_user`。用户可以逐题选择或自填、复核、整批提交和明确取消。用户可在选择前展开 Agent 为选项提供的配置片段、文案样例等内容。问卷确认只提交答案，执行审批沿用自己的流程。

Connect `ask_user` to the shared TSX Chat. Users can select choices or enter their own answers, review the remaining questions, submit them together, or cancel explicitly. Before choosing, they can expand examples such as configuration snippets or draft copy supplied by the agent. Confirming a questionnaire submits its answers. Execution approval retains its own flow.

## 方案 / Approaches

- **采用独立预览字段 / Use a dedicated preview field.** Agent 可为每个选项提供简短 `description` 和可展开的 `preview`。Core 为两个读入口提供同一份投影，TSX 桌面端和 Web 端共用一份界面。代价是要扩展工具入参和读侧协议，并验证旧消费者仍能处理选项。 / The agent can supply a short `description` and an expandable `preview`. Core projects them through both read paths. Desktop and web share the TSX UI. This requires extending the tool input and read contract and verifying older consumers.
- **复用选项说明 / Reuse the option description.** 可以沿用现有字段着手实现，但折叠内容和一眼能看到的说明会争用同一个字段，长片段也会流入现有文字通道。 / This can start with the existing field, but the expandable content and visible summary would compete for it, and long samples would flow into existing text channels.
- **答案批注 / Add answer notes.** 用户可随选择提交理由，适用于解释决定。它需要为答案写入与工具回执增加字段。本轮选定选项预览。 / A note lets users explain a choice and requires extending both answer submission and the tool result. The selected scope for this round is option previews.

## 协议与呈现 / Contract and presentation

- 在 `ask_user` 的展开选项对象中增加可选 Markdown `preview`。字符串选项和已有 `{label, description}` 保持兼容。空白值省略，新预览文本每次工具调用总计最多 16 KiB UTF-8，超限时在注册等待前返回可修正的错误。上限针对新字段，现有标签与说明适用原规则。 / Add an optional Markdown `preview` to detailed choices. Keep string choices and existing `{label, description}` arguments compatible. Omit blank previews and limit new preview text to 16 KiB UTF-8 per tool call. Return an actionable error before registering the wait when the limit is exceeded. Existing label and description rules still apply.
- 扩展共享 `AskUserOption`，由 `question_view` 同时为 `stream.ask_user` 和 `clarification.pending` 提供预览。答复的 `choices`、`text`、批量 `submit`、`cancel` 及工具结果沿用原形状。已有 Leptos、TUI 和 Channel 继续使用各自支持的字段。 / Extend shared `AskUserOption` and project the preview through `question_view` into `stream.ask_user` and `clarification.pending`. Retain the existing `choices`, `text`, `submit`, and `cancel` shapes and tool result. Existing Leptos, TUI, and channel views continue using their supported fields.
- TSX 在当前对话中显示问卷。单选和多选保留原始索引，自填 `1` 作为文本提交。预览有独立的展开控件，开关预览只改变本地显示。使用现有 NSH Markdown 组件，可读列表、表格和代码块，禁用 HTML、MDX、外部图像加载及链接跳转。手机宽度下预览随卡片纵向排列。敏感输入使用遮蔽控件，复核只标记已填写。 / Show the questionnaire in the active conversation. Single and multiple choice preserve original indices, and custom `1` is submitted as text. Opening a preview only changes its display. Use NSH Markdown for lists, tables, and code blocks with HTML, MDX, external image loading, and link navigation disabled. Previews stack vertically in narrow windows. Mask secret input and show only whether it was filled in during review.

## 状态与恢复 / State and recovery

TSX 通过现有 `PanelSdk` 接入。会话打开、问卷事件或连接掉帧、重连时读取 `clarification.pending`，事件用于触发读取。忽略属于旧会话、旧连接或已被更新请求取代的读结果。读取到同一请求时可保留尚未回答题目的草稿，服务端已回答的题和被替换或结束的请求清理对应草稿。按当前 `session_key`、`expected_request_id` 和 `expected_question_id` 单次提交复核后的剩余答案或明确取消。提交结果不确定时只读快照，后续写入由用户明确触发。普通聊天草稿始终独立。

Use the existing `PanelSdk`. Read `clarification.pending` when opening a conversation, receiving a question event or loss of live updates, and reconnecting. Events trigger reads. Discard responses for older sessions, connections, or superseded reads. Keep drafts only for still-unanswered questions in the same request. Clear drafts for questions answered elsewhere and for replaced or ended requests. Submit the reviewed remaining answers or explicit cancellation once, using the current `session_key`, `expected_request_id`, and `expected_question_id`. Resolve uncertain outcomes by reading the snapshot and leave any subsequent write to the user. Keep the ordinary chat draft separate.

## 验证 / Verification

验证工具入参、两条投影及旧选项兼容性。通过 TSX 测试确认预览只改变显示、确切索引与自填文本、复核遮蔽、取消、快速重复点击、切换会话、重连和过时结果。运行 TSX 类型检查与桌面/Web 构建。在真实 Gateway 上以 TSX 验证选项预览与提交，并验证断线期间结束的请求可以清理。已有 Leptos、TUI 和 Channel 的相关测试用于覆盖协议兼容性。

Verify tool input, both read projections, and compatibility with existing option forms. Test that preview only affects display, indices and custom text are exact, secret review is masked, cancellation works, duplicate clicks are guarded, and session switches, reconnects, and stale responses reconcile correctly. Run TSX type checking and desktop and web builds. Against a real Gateway, verify preview, submission, and cleanup of a request that ended while disconnected. Run the relevant existing Leptos, TUI, and channel tests for protocol compatibility.
