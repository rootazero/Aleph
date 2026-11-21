# src/clarification severed-wire 静态审计

- 审计对象：`src/clarification/{mod,session,ask,render}.rs`，worktree HEAD `d57791367`。
- 方法：完整阅读 slice；用 `graphify query "how does clarification ask and session answer flow" --budget 600` 定向；对导出项、RPC、事件、配置与 stub 做全仓库 `rg` 消费者复核。未运行 cargo，未修改源码。
- 既往报告：复核 `review-results/severed-wire-2026-08-17/clarification/REPORT.md` 与 `review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md`。旧的 `ClarificationOptionView` 外露、`DEFAULT_QUESTION_ID` 外露及 2026-10-30 clarification 缺陷均已修复，本轮未发现回归。

## 已确认的主链路（非候选）

- 唯一生产 `ClarificationManager` 在 `src/bin/aleph-server/commands/start/mod.rs:3569` 构造；同一实例在 `:3667` 注册 RPC，并在 `:3893-3901` 注入工具。
- `clarification.resolve` / `clarification.pending` 在 `src/gateway/handlers/clarification.rs:41,:50` 注册，分别在 `:104,:164` 到达 `resolve_many_expected` / `list_pending`，无 client ghost。
- 通道回复在 `src/gateway/inbound_router/mod.rs:1288-1289,:1330-1331` 到达 `has_pending` / `resolve`；`resolve` 在生产内调用 `resolve_many`，所以后者不是死代码。
- 工具入口 `src/clarification/ask.rs:333` 调 `register_with_identity`，`:388` 调 `cancel_if_request`，`:240` 调 `cancel_abandoned_if_request`，`:446` 调 `cleanup_expired`。
- `DEFAULT_CLARIFY_TIMEOUT` 在 `src/clarification/ask.rs:297-300` 与工具预算共同决定实际等待时间，不是 inert config。
- `ask_user` / `clarification_ended` 有生产订阅者；lane 与 visibility 也分别覆盖 `clarification.pending`（`src/gateway/lane.rs:120`）和两个 RPC（`src/gateway/method_visibility.rs:590-591`）。
- 未发现 `TODO`、`todo!()`、`unimplemented!()`、空成功 handler 或未声明 feature 守卫的远端。

## Severed-wire 候选

### SW-CLAR-01 - 公开 `register` 仅有测试调用
- Form: no-consumer
- Producer: `src/clarification/session.rs:236` - `ClarificationManager::register`
- Consumer: NONE FOUND（生产）。测试调用位于 `src/clarification/session.rs:868` 起的测试模块及 `src/gateway/handlers/clarification.rs:251` 起的测试；生产注册统一走 `src/clarification/ask.rs:333` 的 `register_with_identity`。
- Live caller search: `rg -n --glob '*.rs' 'ClarificationManager::(register|cancel|cancel_abandoned)\b|\.(register|cancel|cancel_abandoned)\(' src interfaces shared desktop plugins crates tests 2>/dev/null`；`register` 的 clarification 命中均为文档或 `#[cfg(test)]` 内调用，未见生产调用。
- Symptom: 当前生产流程无可见故障；该公开便捷入口在本仓库只被测试使用，不能作为生产 registration wire 的证据。
- Hypothesis: DECIDE — 若它是对外兼容 API 可保留并标注 test-only 覆盖，否则可删除或收紧可见性；仓外调用者不在本次可搜索范围内。
- Confidence: HIGH

### SW-CLAR-02 - 无 request 身份的 `cancel` / `cancel_abandoned` 仅有测试调用
- Form: no-consumer
- Producer: `src/clarification/session.rs:554` - `ClarificationManager::cancel`；`src/clarification/session.rs:593` - `ClarificationManager::cancel_abandoned`
- Consumer: NONE FOUND（生产）。生产分别使用 `src/clarification/ask.rs:388` 的 `cancel_if_request` 与 `src/clarification/ask.rs:240` 的 `cancel_abandoned_if_request`；无身份版本仅见 `src/clarification/session.rs:1109,:1124,:1129,:1137,:1164` 测试。
- Live caller search: `rg -n --glob '*.rs' 'ClarificationManager::(register|cancel|cancel_abandoned)\b|\.(cancel|cancel_abandoned)\(' src interfaces shared desktop plugins crates tests 2>/dev/null`；两个方法的 clarification 调用均位于 `src/clarification/session.rs:828` 开始的测试模块，其它命中是无关类型的同名方法。
- Symptom: 当前生产取消/回收路径正常；但这两个公开、无身份防护的入口没有生产消费者，测试对它们的通过不能证明实际 identity-scoped wire。
- Hypothesis: DECIDE — 像兼容便捷 API 则可保留，否则删除/收紧以避免未来绕过 request 身份；不能直接判为生产功能断裂。
- Confidence: HIGH

### SW-CLAR-03 - `ClarificationRequest::questions` 仅有测试调用
- Form: no-consumer
- Producer: `src/clarification/mod.rs:213` - `ClarificationRequest::questions`
- Consumer: NONE FOUND（生产）。仅 `src/builtin_tools/ask_user.rs:504,:508-510` 使用，而测试模块从 `src/builtin_tools/ask_user.rs:376-377` 开始；生产直接读取同模块可见的 `questions` 字段。
- Live caller search: `rg -n --glob '*.rs' '\.questions\(\)|ClarificationRequest::questions' src interfaces shared desktop plugins crates tests 2>/dev/null`；只命中上述定义、注释和测试调用。
- Symptom: 无当前用户可见故障；这是只被测试使用的公开 accessor，而非缺失的回答/渲染链路。
- Hypothesis: DECIDE — 对外 API 兼容与最小公开面之间取舍；本仓库没有必须保留它的生产消费者。
- Confidence: HIGH

## 非连线类静态缺陷

### SD-CLAR-01 - Webchat 丢弃 request 身份，旧回答可落到同 session 的新请求
- Form: other-static-defect
- Producer: `src/clarification/session.rs:267` 生成每请求唯一 `request_id`；`:339` 在 pending 视图、`:749` 在 ask frame、`:785` 在 ended frame 发布；`:433-443` 在写锁内用 expected request/question 身份拒绝 stale answer。
- Consumer: `interfaces/webchat/src/api/clarification.rs:88-103` 从 pending 构造视图时未保留 `request_id`；`interfaces/webchat/src/state/notifications.rs:86-103` 的 `PendingAskView` 没有该字段；`interfaces/webchat/src/platform/wide/views/chat/events.rs:1071-1112` 同样丢弃 ask frame 身份。回答在 `interfaces/webchat/src/api/clarification.rs:136-166` 只发送 `session_key` + reply/answers，`:56-62` 又忽略响应里的 `stale`。
- Live caller search: `rg -n 'request_id|expected_question_id' src/clarification/session.rs src/clarification/ask.rs shared/protocol/src/clarification.rs interfaces/webchat/src/api/clarification.rs interfaces/webchat/src/platform/wide/views/chat/events.rs interfaces/webchat/src/state/notifications.rs`；协议在 `shared/protocol/src/clarification.rs:51-55` 已提供两个可选身份字段，TUI 在 `interfaces/aleph-tui/src/app/clarification.rs:90-95,:137-153` 使用 request generation 和 tombstone，Webchat 上述路径没有对应读取。
- Symptom: 同一 session 的第二次 `ask_user` 会 supersede 第一次。若旧卡片回答已在飞行中，新请求注册后该回答因 `expected_request_id=None` 可回答新请求；Core 返回 `stale` 时 Webchat 也显示不了 stale。另一个竞态是 `interfaces/webchat/src/platform/wide/views/chat/events.rs:1003-1009` 仅按 `session_key` 删除 ended 卡片，延迟的旧 generation ended 可移除 successor 卡片。
- Hypothesis: CUT — 身份防护和 stale 响应都已存在，但 Webchat 消费端未接线；这是 Webchat 特定缺陷，不代表 TUI 或 Core 全局未使用 request id。
- Confidence: HIGH

### 文档漂移（不单独计候选）

`src/clarification/session.rs:70-75` 与 `:236-239` 仍说 register sweep 是 abandoned entry 唯一实际回收点，但 `src/clarification/session.rs:209-224` 的 Drop 回收及 `src/clarification/ask.rs:237-245` 的 timeout 回收已经存在。行为未断，仅注释过时。

## 未覆盖的接缝

- 未运行 cargo、浏览器/WASM 或真机通道，按 brief 仅做静态审计；上述竞态未做动态时序复现。
- 仓外 crate 对公开 Rust API 的调用不可见，因此 SW-CLAR-01/02/03 均为 DECIDE，而非直接删除建议。
- TUI 仅用于交叉确认 request identity 的消费，未对其全部 clarification UI 做独立完整审计；Webchat 其它非 clarification 事件也未展开。
- 未逐一审计所有第三方 Channel 插件的内部实现；本报告覆盖 Core inbound router 与仓库内 clarification 消费者。
