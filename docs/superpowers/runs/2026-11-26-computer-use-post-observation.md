# Scoped Computer Use post-observation: verification / 定向动作后观测验收

Branch: `refactor/computer-use`, based on `2ffce02fd`. [Design](../specs/2026-11-26-computer-use-post-observation-design.md) · [Plan](../plans/2026-11-26-computer-use-post-observation.md)

## Reference decisions / 对标取舍

| Reference | Pattern / 设计 | What Aleph reused / Aleph 取舍 |
|---|---|---|
| `agent-desktop` (`ab7dfb21`) | Actable snapshot refs retain surface and owner identity. / 可操作引用绑定表面与所有者。 | Carry execution evidence through the existing rail and element-token resolver; do not introduce a parallel ref registry. / 复用投递轨与元素令牌，不另建引用表。 |
| `lcu` (`0dbcb13`) | Host approval remains outside the model's control. / 宿主控制审批，模型不能代批。 | Leave Core approvals unchanged; do not adopt the closed runtime or its adapters. / 保留 Core 审批，不接入其运行时。 |
| `cua` (`93c9b500a0`) | A valid request, a delivered action, and verified state are distinct. / 请求有效、动作投递和状态证实是不同事实。 | Preserve `effect` and `verify_state`; post-observation failure cannot fail or replay an action. / 保留效果和验证语义，不因观测失败回滚判定或重放。 |
| `open-codex-computer-use` (`fccf49a`) | Capture and geometry are attached to a concrete window. / 图像和几何信息绑定具体窗口。 | Require a window ID and owner PID on the same ScreenCaptureKit object; reuse normal image processing. / 同一窗口对象校验 ID 与 PID，复用截图处理。 |

The built-in `desktop` path consumes this context in `src/builtin_tools/desktop/native.rs`. The obsolete observer in `src/builtin_tools/desktop/mod.rs` no longer reissues a scope-less screenshot. / 现有内置工具直接消费执行上下文，移除旧的无作用域回读路径。

## Evidence / 验证证据

- **RED, Core:** seven fake-platform tests ran against the old implementation and failed on the expected missing or incorrect scoped fields. / 旧路径的七条回归断言失败，证明测试覆盖实际接线缺口。
- **Bridge:** the Rust protocol tests (2) and macOS fake-RPC test (1) passed. The fake helper returned method-not-found and recorded only `screen.capture_owned_window`. The Swift 6.4.2 synthetic suite (2) passed after making optional JSON decoding explicit. / 旧 helper 不触发旧 RPC；Swift 合成用例验证严格参数和同一窗口对象的所有者判据。
- **Integrated Rust:** `cargo check --offline -p alephcore -p aleph-desktop-macos` passed. The Core desktop suite passed **293/293**, and its focused `post_observation` rerun passed **8/8**. The latter covers dispatch scope, refusal to widen, failed capture, partial batches and image extraction. The final protocol (2) and macOS fake-RPC (1) reruns also passed. / Core 与 macOS limb 集成检查通过；桌面回归 293 项通过，其中定向观测 8 项独立重跑通过，协议与旧 helper 回归复测通过。
- **Format:** `git diff --check 2ffce02fd` passed. `cargo fmt --all --check` reports 100 pre-existing hunks in 37 unchanged files; none in the branch's changed files. / 全仓格式门禁仍报告既有问题，本分支改动文件无格式差异。
- **Independent review:** checked the Swift owner predicate and no-fallback path, resolved Core PID, refusal to widen a scoped read, and batch/legacy compatibility. Review requested explicit handling of optional JSON values; the synthetic Swift tests passed with that change. / 独立审阅没有发现其他作用域扩张路径；已按建议显式处理缺失字段并复测。

No live UI, ScreenCaptureKit capture, TCC changes, or Windows/Linux platform run: the existing opt-in macOS fixture needs Accessibility permission and real CGEvents, and its visible tier needs a logged-in GUI session. No Zahir mount or interactive approval work. / 未向真机桌面发送事件、未实拍、未修改 TCC，未运行 Windows/Linux；未扩展 Zahir 投影或交互审批。
