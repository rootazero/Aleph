# Gateway PTY test isolation (#10) / Gateway PTY 测试隔离（#10）

Linux x86_64, on `gateway-suite-recovery` after #9 (`2e05ecdd2`). / 在 #9 提交 `2e05ecdd2` 之后的 `gateway-suite-recovery` 分支验证。

The stale-generation handler fixture bound its Terminal source to the process-global `PtyManager`. The test lock orders access but cannot undo that one-shot binding. The PTY exit reader can retain a source after its facade is gone; a subsequent spawn may then fail registration. If the receiving test panics before deactivation, its Tokio runtime cancels the retained flush driver, and the next fixture sees a failed join. This sequence fits the source and paired failures; the old runs did not trace each intermediate state. / 过期代际用例把 Terminal 源绑定到进程全局 `PtyManager`。共享测试锁只能约束顺序，不能撤销一次性绑定。PTY 退出读线程可能在 facade 销毁后仍持有源，后续 spawn 可能注册失败。若用例在停用前 panic，其 Tokio runtime 会取消仍留在 manager 上的 flush driver，下一用例就可能遇到 join 失败。此因果链与源码和成对失败一致；旧日志未逐步观测中间状态。

The fixture now makes the same real `pty.attach` calls in an exact-match child. The parent requires both a successful exit and the fixture's own receipt after driver shutdown. No production lifecycle or owner policy changed. / 用例在精确选择的子进程内继续走同一真实 `pty.attach` 路径；父进程要求子进程正常退出，并核对 driver 停止后发出的专属回执。未修改生产生命周期或所有权规则。

| Linux verification / Linux 验证 | Result / 结果 |
| --- | --- |
| Old binary: stale handler + close-at-capacity + resize, `--test-threads=3` / 旧制品三用例并发 | 20 of 50 runs failed, each with both downstream failures / 50 轮中 20 轮失败，每轮均出现两项下游失败 |
| Old binary control: close + resize without the source-binding fixture / 旧制品移除绑定用例的两用例对照 | 50/50 passed / 通过 |
| Fixed binary: same three tests and concurrency, check stale-handler receipt each round / 修复制品原组合原并发、逐轮校验回执 | 50/50 passed / 通过 |
| `cargo test -j1 --locked -p alephcore --lib gateway::` | 5,168 passed, 0 failed, 3 pre-existing ignores / 5,168 通过，0 失败，3 项原有忽略 |
| `just check-wiring`, file-scoped rustfmt check, `git diff --check` / 接线、局部格式和差异检查 | Passed / 通过 |

Independent static review found no blocker. Other platforms and the full workspace suite were not run. / 独立静态复核无阻塞项。未运行其他平台或全工作区套件。
