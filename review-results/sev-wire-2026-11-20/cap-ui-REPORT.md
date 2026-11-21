# Capability terminal / UI severed-wire static audit

- HEAD: `d57791367`
- Slice: `src/capability/terminal.rs`, `src/capability/ui.rs`, `src/capability/ui_surface.rs`
- Method: full-file reads, whole-repository symbol/route searches, and call-site confirmation in the canonical gateway/facade path. No source edits and no `cargo` invocation.
- Result: **2 个 no-consumer 候选，1 个非连线类竞态缺陷**。没有发现 name/path drift、never-compiled far-end 或 AGENTS.md 已声明 deferred 项的新缺陷。

## 结论摘要

| ID | 类型 | 结论 | 置信度 |
|---|---|---|---|
| CUI-01 | no-consumer | 整个 legacy `TerminalCapability` / `TerminalHandler` 注册链只在本文件及测试中出现；CUT | HIGH |
| CUI-02 | no-consumer | 整个 legacy in-memory `UiCapability` / `UiHandler` 注册链只在本文件及测试中出现；CUT | HIGH |
| CUI-D1 | other-static-defect | display 发出后到 `pending.sent = true` 之间存在合法 ACK 被拒的竞态；CUT | MEDIUM |

CUI-01/02 **不代表当前 canonical terminal/UI 功能整体不可用**：生产 PTY 路径走 `src/capability/tool_adapters.rs::TerminalCapabilitySource`，生产 UI 路径走 `UiSurface` + `src/gateway/capability_ui.rs`。两份 legacy 实现是仓库内未接线的另一套实现。

## Severed-wire candidates

### CUI-01 - legacy terminal handler 从未接入生产注册表
- Form: no-consumer
- Producer: `src/capability/terminal.rs:36` - `TerminalCapability`；`src/capability/terminal.rs:117` - `TerminalHandler`；`src/capability/terminal.rs:236` - `register_handler`
- Consumer: NONE FOUND: `rg -l --glob '*.rs' '\bTerminalCapability\b|\bTerminalHandler\b|terminal::register_handler|\bTERMINAL_NAMESPACE\b|\bMAX_READ_BYTES\b|\bTerminalError\b' src desktop plugins crates tests`
- Live caller search: 上述精确词边界搜索只返回 `src/capability/terminal.rs`。文件内 `register_handler` 的调用点为 `src/capability/terminal.rs:352,377,409,426`，全部位于 `#[cfg(test)] mod tests`（从 `src/capability/terminal.rs:243` 开始）。`TerminalCapability::system` 也没有生产调用点。搜索时特别使用词边界，避免把生产中的 `TerminalCapabilitySource` 误算成 `TerminalCapability` 调用。
- Symptom: 仓库内生产启动不会构造此 capability，也不会把描述符 `aleph/terminal` / `io`（`src/capability/terminal.rs:216-219`）注册到 legacy registry；因此该实现暴露的 `read_stdin` / `write_stdout` / `write_stderr` handler 不可能由仓库内生产路径到达。当前真实 PTY 功能由另一套 canonical adapter 提供，所以症状是这份公开实现及其常数/错误类型成为暗代码，而不是现有 PTY 工具整体失效。
- Hypothesis: CUT — producer、handler 和注册函数完整存在，但生产注册/调用臂完全缺失。
- Confidence: HIGH

### CUI-02 - legacy in-memory UI handler 从未接入生产注册表
- Form: no-consumer
- Producer: `src/capability/ui.rs:64` - `UiCapability`（文件注释明确称其为 in-memory bridge stub）；`src/capability/ui.rs:130` - `UiHandler`；`src/capability/ui.rs:222` - `register_handler`
- Consumer: NONE FOUND: `rg -l --glob '*.rs' '\bUiCapability\b|\bUiHandler\b|\bUiElement\b|ui::register_handler' src desktop plugins crates tests`
- Live caller search: 上述精确词边界搜索只返回 `src/capability/ui.rs`。`register_handler` 的调用点为 `src/capability/ui.rs:325,348,359`，全部位于 `#[cfg(test)] mod tests`（从 `src/capability/ui.rs:229` 开始）。`display_snapshot`、`push_input`、`capture_input` 同样只有该文件内测试调用。`UI_NAMESPACE`、`UiError`、`schema()`、`descriptor()` 不在此候选内：它们被 canonical gateway 复用（见“已核实连接”）。
- Symptom: 生产不会注册 legacy 固定 ID `aleph/ui` / `surface`（`src/capability/ui.rs:202-205`）的这个 handler，其 display snapshot 与内存 input queue 只被测试观察。canonical UI 使用每 attachment 一个 `UiSurface`，所以这不是 canonical renderer 路径整体断线，而是 legacy UI bridge 的公开生产入口缺失。
- Hypothesis: CUT — in-memory producer 和 registry registration 两端代码都在，但生产只复用其 schema/descriptor，从不构造或注册该 handler。
- Confidence: HIGH

## 非连线类静态缺陷

### CUI-D1 - 快速合法 render ACK 会在 `sent` 翻转前被拒，display 最终超时
- Form: other-static-defect
- Producer: `src/gateway/capability_ui.rs:159-174` - send closure 通过 `publish_json` 发布 `UiDisplayMessage`；`src/capability/ui_surface.rs:202` - `(self.send)(message)` 返回后，直到 `src/capability/ui_surface.rs:213` 才执行 `pending.sent = true`
- Consumer: `src/gateway/capability_ui.rs:284-300` - `handle_ack` / `surface.ack`；`src/capability/ui_surface.rs:230-247` - `UiSurface::ack` 在 `src/capability/ui_surface.rs:243` 要求 `pending.sent`
- Live caller search: `rg -n 'capabilities\.ui\.ack|handle_ack|surface\.ack' src/gateway tests/gateway_ui_ingress.rs qa/capability_i3/probe.py`。生产 handler 注册于 `src/gateway/server/mod.rs:891-893`；真实集成调用在 `tests/gateway_ui_ingress.rs`，QA renderer 流程在 `qa/capability_i3/probe.py:432`。display 在 `src/gateway/server/connection/acknowledged_effects.rs:128` 的 off-I/O-loop effect worker 中执行，而 ACK 走独立 RPC dispatch，因此 socket 串行处理不能消除这个交错。
- Symptom: 事件已经 publish、renderer 也已收到并立即 ACK 时，如果 ACK 在 display worker 执行 `pending.sent = true` 之前到达，`ack` 的 filter 失败并返回 `UiError::StaleAttachment`。此时 resolution 没有被取走，合法 ACK 也不会排队或自动重试；客户端看到 ACK RPC 失败，已经渲染的 display 仍可能一直等到 `UI_ACK_TIMEOUT_MS`（5000 ms）后超时。
- Hypothesis: CUT — display→ACK 两端都存在且已接线，但“事件已对客户端可见”与“ACK 可被接受”之间缺少同步；`sent` 在 publish 后才置位形成一个可交错的拒绝窗口。
- Confidence: MEDIUM — 静态交错明确，但按任务约束未运行竞态复现。决定性验证是用 barrier 强制在 send 返回后、`pending.sent = true` 前调用同 connection/同 request 的 ACK，并断言 ACK 应被接受且 display 成功完成。

## 已核实连接（避免误报）

- `src/capability/ui.rs:174` 的 `schema()` 与 `src/capability/ui.rs:198` 的 `descriptor()` 有生产消费者：`src/gateway/capability_ui.rs:194`、`:206`。`UI_NAMESPACE` 在 `src/gateway/capability_ui.rs:440` 用于每 surface 的 capability ID。
- `UiSurface::new` 在 `src/gateway/capability_ui.rs:176` 被生产 attach 路径构造；`UiSurface::display` / `input` 通过 `src/capability/ui_surface.rs:375-390` 的生产 `CapabilityHandler::invoke` 到达。
- `UiSurface::bind_policy_source` **不是** test-only：`src/capability/zahir_facade.rs:897` 在 `special_policy_facts` 中生产调用它；`close` 在 `src/capability/ui_surface.rs:280-289` 撤回对应 fact source。
- `owner_connection()` 在 `src/capability/ui_surface.rs:116` 和 `src/gateway/capability_ui.rs:382` 有生产读取；`is_closed()` 在 `src/capability/ui_surface.rs:350` 被 `validate_attachment` 读取；`validate_attachment` 被 gateway invoke/ack/input/detach 与 event admission 复用。
- `capabilities.ui.attach` / `invoke` / `ack` / `submit_input` / `detach` 均注册于 `src/gateway/server/mod.rs:881-900`，路由字面量与 `src/gateway/method_visibility.rs:544-548`、`src/gateway/method_census.rs:124-128` 一致。
- `capabilities.ui.display` 在 `src/gateway/capability_ui.rs:52,161` 发射，在 `src/gateway/event_visibility.rs:450` 与 `src/gateway/server/connection/mod.rs:1549,1637-1648` 有生产可见性/订阅转发逻辑；`qa/capability_i3/probe.py:26,430-432` 和 `tests/gateway_ui_ingress.rs` 实际订阅并 ACK。因此未把“desktop 目录没有具体 renderer”误报成事件总线断线。
- 当前 canonical terminal 消费者是 `TerminalCapabilitySource`（`src/capability/tool_adapters.rs:44`），生产构造点包括 `src/bin/aleph-server/commands/start/mod.rs:1168`；它不是 CUI-01 中 legacy `TerminalCapability` 的消费者。
- stub sweep：三个 slice 文件无 `TODO` / `todo!` / `unimplemented!`；除测试模块外没有条件编译手臂。`UiCapability` 的 stub 属性已按其实际 no-consumer 状态归入 CUI-02，未另造重复候选。

## 导出项覆盖清单

| 文件 | 导出项 | 审计结果 |
|---|---|---|
| `terminal.rs` | `TERMINAL_NAMESPACE`, `MAX_READ_BYTES`, `TerminalError`, `TerminalCapability` 及全部方法, `TerminalHandler`, `register_handler` | 全部归入 CUI-01；仅本文件/测试 |
| `ui.rs` | `UI_NAMESPACE`, `UiError`, `schema`, `descriptor` | CONNECT：canonical gateway 复用 |
| `ui.rs` | `UiElement`, `UiCapability` 及全部方法, `UiHandler`, `register_handler` | 全部归入 CUI-02；仅本文件/测试 |
| `ui_surface.rs` | `UiSurface::new`, `owner_connection`, `owner_context`, `bind_policy_source`, `display`, `input`, `ack`, `submit_input`, `close`, `is_closed`, `validate_attachment`, `CapabilityHandler::invoke` | CONNECT：gateway/facade/special registry 均有生产路径；另见 CUI-D1 的 ACK 时序缺陷 |

## 既有报告交叉核对

- `review-results/severed-wire-2026-08-17/capability/REPORT.md` 讨论旧 `mod.rs` / `census.rs`，没有 terminal/UI surface 条目可直接复验。
- `review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md` 的 terminal 命中是 effect-claim “terminal receipt”，不是本 slice 的 terminal I/O；其中旧 ZahirFacade mount 叙述已由当前生产 mount 取代。
- 因而本轮没有把任何条目标成“回归/未修”。

## 未覆盖的接缝

- 按 brief 未运行 `cargo`、单测或真机 QA；CUI-D1 只有静态交错证明，没有运行时复现。
- 搜索覆盖仓库内 `src/`, `desktop/`, `plugins/`, `crates/`, `tests/`，并补查 `qa/`；无法排除仓库外下游 crate 直接调用 public legacy `TerminalCapability` / `UiCapability`，所以 CUI-01/02 的结论限定为“本仓库生产路径未接线”。
- 未完整审计 PTY manager、canonical tool adapter、event bus 内部实现或所有外部 renderer；只核实它们是否构成 slice 的真实消费者。AGENTS.md 明确 deferred 的 EffectClaim producer integration、Safe Replay、external-effect exactly-once、universal durable scheduler、完整 ACP server gate 和其余 CapabilityKind backends 均未作为缺陷报告。
