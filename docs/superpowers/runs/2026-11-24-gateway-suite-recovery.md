# Gateway regression follow-up #9 / Gateway 回归修复 #9

Linux x86_64, 2026-11-24. The isolated `gateway-suite-recovery` branch starts at `0e39f7b2a`, the cold-declaration checkpoint. This is a run receipt, not the Gateway architecture reference. / 这是以冷声明检查点 `0e39f7b2a` 为基线的 Linux x86_64 验证回执，不是当前 Gateway 架构参考。

| Original failure / 原失败 | Repair / 修复 |
| --- | --- |
| `issuer_rpc_owned_issue_and_revoke_enforce_identity_without_oracle` | Check the permissions actually requested and independently confirm a Frames-only request grants no control. / 校对请求与实际颁发权限，并单独验证只请求 Frames 不获得控制权限。 |
| `the_cli_carve_out_delivers_everything_an_unsubscribed_socket_did_but_reasoning` | Include braced constant arms in the topic census; retain the reasoning opt-out assertion. / 主题普查识别带花括号的常量分支，保留 reasoning 不下发的断言。 |
| `pairing_drops_dm_from_unpaired_number` | Opt the fixture into Pairing and supply phone-number JIDs; verify both the drop and the paired delivery. / fixture 显式启用 Pairing，使用电话号码形态的 JID，同时验证拒收及已配对消息到达。 |
| `oversized_incomplete_stanza_closes_connection` | Skip scans that cannot find a newly completed stanza; retain the size limit and close the old receive-loop transport before the reconnect wait. / 不能产生新完整 stanza 时跳过扫描；保留大小上限，接收循环退出后先关闭旧连接再等重连。 |
| `every_production_caller_of_ambient_actor_is_classified` | Classify the three extant PTY calls as ownership comparisons; the retired compose call is no longer counted. / 将现有三个 PTY 调用归类为所有权比较，移除已退役的身份构造调用预期。 |

## Verification / 验证

- `cargo test -j1 --locked -p alephcore --lib gateway::` compiled the final Rust sources. All five tracked tests passed; two other PTY tests failed once: `full_pty_can_close_and_old_capture_cannot_restart_it` reported `terminal provider is unavailable`; `stage_a_resize_uses_real_connection_minimum_and_disconnect_growth` reported a cancelled driver join. Both passed individually from the same binary. / 最终源码编译成功，五项均转绿。另有两项 PTY 测试曾失败，同一制品单独运行均通过。
- Ran the same Cargo-built test binary, `alephcore-70d71350e55c8e18`, again with the full `gateway::` filter, default test concurrency and no skips. Exit 0: **5,168 passed, 0 failed, 3 ignored**. Both PTY tests also passed on this run. / 同一制品、同一全量过滤器、默认并发、不跳项重跑，退出码 0，5,168 通过、0 失败、3 项原有忽略；两项 PTY 测试也都通过。
- `just check-wiring` and `git diff --check` passed. An independent static review found no blocker; the XMPP socket regression still uses a wall-clock deadline. / 两项检查均通过。独立静态审查无阻塞项；XMPP socket 回归测试仍使用真实时钟。

Not covered: PTY test isolation, external chat services, other platforms, or the entire workspace suite. No authorization rules, channel policy defaults, or reconnect intervals were changed. / 未修复 PTY 测试隔离，未验证外部聊天服务、其他平台或全工作区套件；未改变授权规则、通道默认策略或重连间隔。
