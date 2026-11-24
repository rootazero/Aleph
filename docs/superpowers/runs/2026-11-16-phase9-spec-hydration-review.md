> Independent reviewer: `8315699d-cf9c-4a8c-8710-9b2273ce091b`. Actual verdict retained: **BLOCK / NOT COMPLETE**. Only repository-absolute path prefixes below are normalized for portability; findings are unchanged. Original raw SHA-256: `daf37b935d4e297bf8fd28b2cd8cff348f3f75481448831d334e974d67d14e7f`. / 独立原文仅将仓库绝对路径转为相对路径，未改 verdict。

# Phase9 独立 SPEC 复审：hydration race 修复

审查基线：`3892804c112fd6e574166a4c03a7c40a77d0b239`  
审查对象：`interfaces/webchat/src/components/chat_sidebar.rs` 最终 hydration 修复补丁  
最终 verdict：**BLOCK / NOT COMPLETE**

## Correct

- **QUALITY 报告中的同会话覆盖 P1 已被当前补丁闭合。**

  `await_current_history` 在等待历史和 trace 前保存 connection epoch、session key、`sends`、完整 `messages`、`active_run_id`、phase、reasoning 和 `history_limit`，并在异步返回后逐项比较；任何 signal 被 dispose 或发生变化时都会 fail closed。证据：[interfaces/webchat/src/components/chat_sidebar.rs:31](interfaces/webchat/src/components/chat_sidebar.rs:31)、[interfaces/webchat/src/components/chat_sidebar.rs:36](interfaces/webchat/src/components/chat_sidebar.rs:36)、[interfaces/webchat/src/components/chat_sidebar.rs:41](interfaces/webchat/src/components/chat_sidebar.rs:41)。

- **破坏性 transcript 替换现在发生在完整历史与 trace 获取之后，并且受 checkpoint 保护。**

  `ChatApi::history` 和 `TraceApi::by_runs` 都位于同一个受保护的加载 future 中；只有 `await_current_history` 接受结果后，代码才执行 `chat.messages.set(Vec::new())` 及历史重放。证据：[interfaces/webchat/src/components/chat_sidebar.rs:295](interfaces/webchat/src/components/chat_sidebar.rs:295)、[interfaces/webchat/src/components/chat_sidebar.rs:346](interfaces/webchat/src/components/chat_sidebar.rs:346)。

- **可选 context estimate 不再阻塞初始 transcript 呈现。**

  历史元数据、消息、plan 和 last-run notice 先同步提交；只有没有持久化 occupancy 时才请求 estimate。该请求使用 replay 后的新 checkpoint，若期间出现新发送、stream、phase 或其他受保护状态变化，则直接返回而不消费陈旧结果。证据：[interfaces/webchat/src/components/chat_sidebar.rs:482](interfaces/webchat/src/components/chat_sidebar.rs:482)、[interfaces/webchat/src/components/chat_sidebar.rs:486](interfaces/webchat/src/components/chat_sidebar.rs:486)。

- **回归测试覆盖了原 QUALITY P1 的实际交错顺序。**

  五个测试使用真实 `ChatState` effects 和 owned oneshot barriers，覆盖新 send 加 stream、无本地 send 的 live chunk、新 send 尚未形成 bubble、扩大 history window，以及 unchanged positive control。证据：[interfaces/webchat/src/components/chat_sidebar.rs:66](interfaces/webchat/src/components/chat_sidebar.rs:66)、[interfaces/webchat/src/components/chat_sidebar.rs:109](interfaces/webchat/src/components/chat_sidebar.rs:109)、[interfaces/webchat/src/components/chat_sidebar.rs:150](interfaces/webchat/src/components/chat_sidebar.rs:150)、[interfaces/webchat/src/components/chat_sidebar.rs:164](interfaces/webchat/src/components/chat_sidebar.rs:164)、[interfaces/webchat/src/components/chat_sidebar.rs:185](interfaces/webchat/src/components/chat_sidebar.rs:185)。

- 父会话保存的 v2 receipt 报告 Panel 测试为 **1,567 passed / 0 failed / 0 ignored**；canonical default 和 QA build 均为 `PASS`。这些是已记录的执行证据，不是本 reviewer 亲自运行的测试。[2026-11-16-phase9-panel-hydration-v2-green.json](docs/superpowers/runs/2026-11-16-phase9-panel-hydration-v2-green.json:1)、[2026-11-16-phase9-panel-hydration-v2-default.json](docs/superpowers/runs/2026-11-16-phase9-panel-hydration-v2-default.json:1)、[2026-11-16-phase9-panel-hydration-v2-qa.json](docs/superpowers/runs/2026-11-16-phase9-panel-hydration-v2-qa.json:1)。

- 终态 runtime16 receipt 现已存在并已读取。Tauri 和 NSH 两个 surface 均为 `PASS`，served Panel assets 与新 QA WASM 对应，Gateway、model、fixture cleanup 均记录为正常完成；因此当前 patch 的 runtime replay/provenance 证据已经替换了旧 runtime15 的同类证据。[2026-11-16-phase9-runtime-attempt16.json](docs/superpowers/runs/2026-11-16-phase9-runtime-attempt16.json:1)

- 新 default bundle 的 byte-exact restoration receipt 为 `PASS`，且 `qa_ingress_name_present:false`，所以 QA ingress 没有留在 canonical default 产物中。[2026-11-16-phase9-hydration-v2-default-restoration.json](docs/superpowers/runs/2026-11-16-phase9-hydration-v2-default-restoration.json:1)

## Fixed

### 原 QUALITY P1：同 epoch、同 session 的历史恢复覆盖新 send/stream

**状态：已修复。**

最小修复是当前 `await_current_history` checkpoint，而不是增加新的 counter、transport、Gateway authority 或第二套状态源。它能拒绝：

- history/traces 等待期间出现的新 send；
- 没有增加 `sends`、但已有 assistant bubble 新 chunk 的 stream；
- 尚未形成 bubble 的 send progress；
- history window 被更新后的旧请求。

这符合原 QUALITY 报告要求的同会话并发回归，并保持了新消息优先于过期持久化 snapshot 的行为。

## Finding

### P1 — D 的完整性能验收仍然 OPEN / BLOCKED

**位置：**

- [docs/superpowers/runs/2026-11-16-phase9-runtime-attempt16.json:1](docs/superpowers/runs/2026-11-16-phase9-runtime-attempt16.json:1)
- [docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:33](docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:33)
- [docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:35](docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:35)
- [docs/superpowers/runs/2026-11-16-phase9-closeout.md:17](docs/superpowers/runs/2026-11-16-phase9-closeout.md:17)

runtime16 的两个 surface 都明确记录 `resource_complete:false`，其 `performance.status` 仍为 `OPEN`。当前采样只证明 direct native child 的退出和行为相关证据，不能证明 launchd-owned WebKit XPC 进程的每实例归属、完整 RSS/thread 范围或清理关系。

因此 runtime16 关闭了新 sidebar patch 的行为与产物回归证据缺口，但没有关闭 D。SPEC 要求的有效 **10 次 warmup + 30 次 measured**、完整 WebKit 资源归属和 parity threshold acceptance 仍未完成。该 finding 是外部验收证据阻塞，不应解释为已证明的性能阈值失败，也不应改写为 `NOT_ACCEPTED`。

**最小后续动作：**取得可验证的每实例 WebKit ownership 与 cleanup 证据后，按既定 Tauri-first / NSH-second 顺序重新执行完整 10+30 对照。现有阈值和 reducer 不应改变。

## 验证边界与跳过项

本次复审保持只读：

- 未修改、暂存、提交或推送任何文件；
- 未运行 Cargo、Python、Node 测试；
- 未启动或终止进程；
- 未重新计算补丁、source manifest、WASM、binary、日志或 HTTP payload hash；
- 未读取受保护的 `.claude/settings.json`、根目录 `config.lock` 或根目录 `config.toml`；
- 未执行完整 WebKit XPC ownership 验证；
- 未执行新的 10 warmup + 30 measured 性能验收；
- 未宣称 release-profile、跨平台 NSH、Tauri/WebChat 移除、detached daemon ownership、Safe Replay、external-effect exactly-once 或 universal scheduler 已完成。

graphify executable/tool 在本受限 reviewer 环境中不可用，因此没有重新执行 `graphify`。按任务要求先读取了已有的父会话 graph 输出 `hydration-race-graph.txt`；该输出明确显示为 955 节点中的截断视图，只展示约 53 个节点，并带有 graphify package/skill 版本差异警告。随后仅围绕该输出定位的 `await_current_history`、`ChatState`、`hydrate_session_history` 和 `hydrate_and_follow` seam 读取原始源码。因此本次没有把该截断 graph 当作完整调用图审计。

## Merge verdict: BLOCK / NOT COMPLETE

当前代码层面的 QUALITY P1 已闭合，v2 Panel 测试、default/QA build、runtime16 和 default restoration 均有终态记录。整体 Phase9 仍为 **BLOCK / NOT COMPLETE**，唯一保留的本次审查阻塞是 D：完整 WebKit 资源归属和有效 10+30 性能验收仍为 `OPEN / BLOCKED`。