# Gateway cold declarations — Linux validation / Gateway 冷声明 Linux 验证

**Point-in-time evidence, not the current Gateway reference. / 时点证据，不替代当前 Gateway 参考文档。**

## Build and verification / 构建与验证

- Linux x86_64, Rust 1.99.1; implementation `9976b7840` on `gateway-cold-declaration`, forked at `4f966eaf6`. The tested binary was built from the worktree with this implementation, not taken from the base revision. / Linux x86_64，Rust 1.99.1；实施提交 `9976b7840` 位于从 `4f966eaf6` 分出的 `gateway-cold-declaration` 工作树。二进制由该工作树构建，不是基线制品。
- `cargo check -j1 --locked -p alephcore --lib --bin aleph-server`: exit 0. `cargo build -j1 --locked -p alephcore --bin aleph-server`: exit 0. Linux binary SHA-256: `01fe98a910d719b3a59cae8a58271d5256aa4ebeec544aa85fe78b581f6f343e`.
- `python3 qa/channels/cold_declarations.py "$CARGO_TARGET_DIR/debug/aleph-server"`: exit 0 on the same binary. First run used `channel_id` instead of `id` for `channel.delete` and received `-32602, Missing 'id' field`; only the driver changed for the rerun. / 同一二进制复验通过；首次驱动给 `channel.delete` 传了 `channel_id` 而不是 `id`，复验只修正了驱动参数。
- `just check-wiring`: exit 0; 9 advisories and 34 known path-backlog entries, no errors. / 通过，9 条提示、34 项已登记路径欠账，无错误。
- `cargo test -j1 --locked -p alephcore --lib gateway::`: **exit 101**, 5,163 passed, 5 failed, 3 ignored. The same Cargo-built test binary passed 351/351 with the six targeted module filters (`channel_policy`, `channel_registry`, `handlers::channel`, `inbound_router`, `interfaces::telegram`, `security::shared_token`). / 全量套件未全绿；同一 Cargo 测试二进制按上述六个模块定向复跑，351 项全部通过。
- The five failures reproduced individually with the same assertion or timeout in the current binary and a local test artifact built on 2026-11-22, before this worktree: browser viewer owner scopes, CLI event topic census, WhatsApp paired-DM receive, XMPP incomplete-stanza close, and the `ambient_actor` caller census. The older artifact is evidence of prior occurrence, not a fresh build of the exact fork revision. None of those test files changed here. / 五项失败在当前二进制和本工作树建立前的 2026-11-22 旧制品上，逐条复现了相同断言或超时。涉及浏览器权限、CLI 事件主题、WhatsApp 已配对消息、XMPP 残缺报文和 `ambient_actor` 调用清点。旧制品证明此前已出现，不等于对准确基线提交重新构建；本轮没有改这些测试文件。

## Native observations / 原生观测

| Scenario / 场景 | Observed / 观测 |
| --- | --- |
| Explicitly false at boot / 启动时显式禁用 | Signal had zero HTTP-client constructions, vault hydration calls, and upstream connections before activation. Cold Telegram/Feishu had zero observed credential hydration; disabled iMessage/BlueBubbles did not contact the loopback upstream. All projected `disabled`. / 观测到 Signal 未构造 HTTP 客户端、未从 vault 补齐凭据，也未连上游；Telegram/Feishu 未补齐凭据，BlueBubbles 未连测试上游，状态均为 `disabled`。 |
| True and omitted flag controls / `true` 与省略字段的对照 | At boot and create, each of the four Signal controls constructed once and performed a loopback SSE GET. / 启动和创建中的四个 Signal 对照各构造一次，并连接回环 SSE。 |
| RPC cold create, list, send, delete / RPC 冷创建、枚举、发送、删除 | No constructor or network work before explicit start; duplicate create and cold send rejected. List reconciled 10 distinct rows, 6 disabled. Config-only deletion removed the row without activation. / 显式启动前无构造或网络活动；拒绝重复创建与冷发送；10 条唯一记录中 6 条禁用；纯配置删除未触发激活。 |
| Explicit activation / 显式激活 | Malformed latest policy was rejected before construction. Corrected boot row and post-boot create row each constructed once; the first inbound group slash command got the policy denial through the real outbound API. Both persisted `enabled=false`. / 错误策略在构造前被拒绝；修复的启动声明与启动后创建的声明各构造一次；首条群组斜杠命令经实际出站接口收到拒绝；两条配置均保留 `enabled=false`。 |

The whole native probe used a disposable home, an authenticated Gateway WebSocket connection, and loopback HTTP only. It did not connect to live messaging services. This proves no construction on the observed cold paths, not memory reclamation after stop or native code unlinking. No desktop, macOS, or Windows QA was performed. / 探针只使用临时 home、Gateway WebSocket 握手与回环 HTTP，不连接真实通信服务。证据证明观测到的冷路径没有构造实例，不证明停止后回收内存或从二进制移除原生代码。未进行桌面、macOS 或 Windows QA。
