> Independent reviewer / 独立审查 `0eab2546-4aff-40f3-b1ee-97599a24f6fd`; raw SHA-256 `1567d865a19bdc4bc4ea213c3d2ba67b6fa43cbce41456ddb134a9935b9db66d`. Repository prefix normalized only; findings/verdict unchanged. / 仅路径归一化。

# Phase9 Independent QUALITY Review

## Review

**独立 QUALITY 结论：BLOCK / NOT COMPLETE。** 发现一项 P1 实现缺陷：新增的 idle reconnect 历史恢复可能覆盖同一会话内刚发送或刚投影的消息。此外，实际最终 SPEC 报告保留的 D 资源归属与有效 10+30 性能验收阻塞仍然成立。两者需要分别处理，不能把 D 的证据不足解释为性能阈值失败。

审查基线为 `3892804c112fd6e574166a4c03a7c40a77d0b239`，分支为 `runtime-proof`。审查使用指定的最终补丁 `quality-final-current.patch`，其提供的 SHA-256 为 `9d0d4d535f80932ae97ff29580b4f288e09e26769febe58f5e71ac1d6a4f71ef`；43 文件 source manifest 提供的 SHA-256 为 `10d6f3148c1fbe2d1457226217632418178830f0e4fda2f3a7edb527f191b4c5`。本次没有自行重新计算这些摘要。

已先阅读 QUALITY brief、实际最终 SPEC 报告和指定的父会话 graphify 输出，再检查源码。当前只读工具没有执行能力，因此**没有执行 graphify**；采用的是 brief 明确允许的 `spec-final-graph.txt`。该输出自身标明结果截断，以及 graphify package/skill 版本差异，不能视为完整调用图审计。

### Correct

- **R2 托管边界保持清楚。** [NSH main.rs](desktop/nsh-shell/src/main.rs:1) 和 [shell_host.rs](desktop/nsh-shell/src/shell_host.rs:1) 将新入口限定为现有 Panel 的 WebView host；没有挂载历史 native Chat/Artifacts 业务模块，也没有增加第二套 Gateway 业务 transport。QA endpoint 要求 debug build、严格 loopback origin 和配对 nonce。

- **Tauri 观察桥有明确的权限边界。** [main.rs](desktop/shell/src/main.rs:55) 将桥接模块限制在 `qa-owned-root + debug_assertions`；[phase9_qa.rs](desktop/shell/src/phase9_qa.rs:27) 限制 capability 的 remote origin 和 main window，命令还检查当前来源、owned-root nonce、run identity 和事件大小。未发现本次变更放宽普通业务 handler 的具体问题。

- **旧 receipt 测试触及生产 ingress。** [context.rs](interfaces/webchat/src/context.rs:29) 的共享 dispatcher 在 subscriber dispatch 前检查 receipt 和 Gateway URL。[回归测试](interfaces/webchat/src/context.rs:4057) 同时断言旧 receipt 没有 subscriber effect、当前 receipt 有 effect。optional QA ingress 与生产 loop 使用同一 dispatcher，符合实际最终 SPEC 对旧 closed-socket no-op finding 的关闭结论。

- **HTTP provenance 与性能证据边界没有混淆。** [verified_capture](qa/nsh_panel_launcher.py:147) 检查 retained HTML/JS/WASM/CSS、可执行文件和日志摘要，并从 stdout 重建 nonce/run 匹配的 marker sequence。runtime15 文档记录两窗口正常退出、served assets 前后稳定及 owned Gateway fixture 清理；同时明确保留 `resource_complete:false`。[metric_document](qa/nsh_panel_launcher.py:267) 要求完整 40 runs，并拒绝不完整资源范围。

- **测试闭合改动集中在夹具与同步。** discovery 测试使用组合 HOME/ALEPH_HOME 隔离，路径断言比较派生结果，live-tail 测试以 release file 同步。这些改动没有显示出修改生产 discovery policy 的具体问题。实际最终 SPEC 接受 A/B；本次没有重新执行或独立复现其全量测试结果。

### Fixed

无。本次为只读独立审查，没有修改任何文件。

### Finding: P1 — idle reconnect 的历史恢复会覆盖同会话的新消息

**主要位置：** [sessions.rs:577](interfaces/webchat/src/state/sessions.rs:577)。

本次变更新增 `!live.contains(&key) => Some(key)`，使已连接的 idle 会话也进入 `hydrate_and_follow`。该路径由 [reattach_after_connect](interfaces/webchat/src/state/reattach.rs:150) 调用。

历史恢复的失效条件不足以保护这个新入口：

- [chat_sidebar.rs:123](interfaces/webchat/src/components/chat_sidebar.rs:123) 的 `current` 只比较 connection epoch 和 session key。
- 历史响应之后，还可能等待 `trace.by_runs`；随后 [chat_sidebar.rs:175](interfaces/webchat/src/components/chat_sidebar.rs:175) 直接执行 `chat.messages.set(Vec::new())`，再重放旧历史。
- Composer 的 [can_send](interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:1213) 只检查 draft/attachments 和 `is_sending`，没有等待 reconnect hydration。发送时会在 [composer/mod.rs:287](interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:287) 立即插入用户消息。

因此存在以下源码可确定的执行顺序，本次未实际运行该复现：

1. 同一 idle 会话重连成功，开始请求历史快照。
2. 旧历史返回，恢复逻辑继续等待该历史对应的 traces。
3. 用户在已连接的同一会话发送新消息；用户 bubble、accepted 或部分 stream 已投影。
4. traces 返回。epoch 和 session key 都没有改变，guard 通过。
5. 恢复逻辑清空当前 transcript，用旧快照替换它，覆盖步骤 3 的新消息或流内容。

这项风险由本次新增的 idle reconnect hydration 入口带入原本不执行该恢复的场景。跨 epoch/session 的 fence 不能识别同一会话已经前进。

**最小修复方向：** 给 destructive hydration 增加同会话进展检查。已有 `ChatState::sends` 可识别本地新发送；替换前还应识别期间到达的 live projection 更新，过期时放弃旧结果并按需要重新获取。无需改变 Gateway authority 或增加业务 transport。

**必要回归：** 保持同 epoch、同 session，延迟旧 history/traces 响应，在其间插入新发送及 stream，再释放旧响应，断言新用户 bubble 和新流内容保留。现有静态 epoch/session guard 及 idle `rejoin_target` 测试不能证明这一点。

### Finding: P1 — D 仍缺完整资源归属与有效 10+30 验收

这是**已知的外部证据阻塞**，独立于上面的实现缺陷。

[性能规格](docs/superpowers/specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md:76) 要求完整资源采样及 10 warmup + 30 measured 对照。[资源归属调查](docs/superpowers/runs/2026-11-16-phase9-resource-ownership-investigation.md:3) 记录当前仅能可靠采样直接启动的 native child，尚不能证明 launchd 所属 WebKit XPC 进程的每实例归属。

runtime15 明确保留不完整资源范围，`performance.status` 为 `OPEN`。现有结果不能证明完整 RSS/thread 阈值通过，也不能据此判定阈值失败或 `NOT_ACCEPTED`。

后续需要取得可验证的 WebKit 实例归属与清理证据，再执行有效的 10+30 对照。取得之前，D 应继续保持 `OPEN / BLOCKED`。

### 验证与未执行事项

读取了实际最终 SPEC、execution/closeout/integrity 记录、相关补丁与源码、runtime15 文档及部分 raw markers、最终 QA 索引和原始测试日志。最终原始日志分别显示 runner 13、lock 3、runtime/HTTP 18、Node 3 个测试通过。

Core `22,402/0/20`、Panel `1,562/0/0`、Tauri 190、NSH 4 和 API 1 均作为父会话执行并记录的结果引用，**不是本 reviewer 执行的测试**。

本次没有：

- 运行 Cargo、Python、Node 测试或原生应用；
- 修改、暂存、提交或推送文件，启动或终止进程；
- 重新计算 manifest、binary、日志或 HTTP payload 摘要；
- 读取受保护的 `.claude/settings.json`、根 `config.lock` 或根 `config.toml`；
- 执行完整 WebKit 资源归属验证、有效 10+30、release performance 或跨平台验收。

### Merge verdict: BLOCK / NOT COMPLETE

实际最终 SPEC 的结论保持为 **BLOCK / NOT COMPLETE**；其对 A/B/C 和旧 ingress、HTTP provenance findings 的判断没有被改写。本次 QUALITY 另发现上述同会话 hydration 竞态。现有 runtime15 行为证据仍有其限定价值，但不足以覆盖该并发顺序；D 也尚未闭合。当前不能声明整体 Phase9 PASS。