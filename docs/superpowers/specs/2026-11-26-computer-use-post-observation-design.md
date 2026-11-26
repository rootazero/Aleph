# Computer Use: scoped post-action observation / 按目标回传动作后观测

## Intent / 意图

让 `desktop` 的主动观测回答「刚才的动作送到了哪里、那里现在是什么状态」，而不是「用户的前台现在是什么」。修复在后台输入后可能返回另一应用焦点、甚至整屏像素的连线缺陷。保留显式独立截图和验证命令，以及 Core 的审批权威、Bridge 的平台执行边界。

`desktop` post-observation should describe the subject of the completed action, not the user's foreground session. Keep standalone capture and verification, Core approval, and Bridge platform execution as separate contracts.

## Approaches / 备选方案

| Approach / 方案 | Trade-off / 取舍 |
|---|---|
| Focus-only / 仅修焦点 | 最小改动，但会失去定向后截图。 / Smallest change, but removes targeted post-action images. |
| **Scoped observation / 按目标观测（选定）** | 复用现有投递、AX 查询和截图处理；有可信窗口才捕获图像。增加一个窄的 Bridge 内部契约，换取旧 helper 也不会静默放宽截图范围。 / Reuse delivery, AX, and image processing; require a proven window for images. A narrow Bridge contract prevents older helpers from silently weakening the capture constraint. |
| Scoped observation + Zahir projection / 同时接入 Zahir | 能扩展发现路径，但须独立证明同一 handler、审批及所有权，不应和观测修复一起改变。 / Extends discovery, but requires a separate proof of handler, approval, and ownership reuse. |

参考设计：`agent-desktop` 的窗口/所有者引用，`open-codex-computer-use` 的窗口快照与几何约束，`cua` 的动作与验证分离，以及 `lcu` 的模型外审批。借鉴契约，不引入参考项目的运行时或依赖。 / Adopt the contracts, not the reference runtimes or dependencies.

## Execution evidence / 执行上下文

- 在 `src/builtin_tools/desktop/native.rs` 中，从实际选定的投递路径、元素令牌展开后的 locator 或平台动作结果产生内部观测目标。只为成功、请求观测、且已在现有观测资格内的叶动作使用它。不要从原始参数、公开 JSON 回执或再次匹配应用名称推测投递目标。让生产调用消费这份类型化上下文，不在 `mod.rs` 保留第二份推导。 / Derive an internal typed observation subject from the rail selected for actual dispatch, the resolved AX locator, or authoritative action results. Use it only for successful eligible leaf actions that opted in; do not reconstruct it from raw arguments, public JSON, or a second app-name lookup in `mod.rs`.
- **定向输入 / Targeted input:** 以 `Rail::Targeted(pid)` 为准。可以携带该次动作的显式窗口 ID；窗口 ID 本身不证明它属于该 PID。元素令牌只有 PID 时，不猜窗口、不选用户前台。 / Use the delivered PID. A window ID supplied for that action is only a candidate; a PID-only element token does not justify picking a window.
- **AX 写入 / AX writes:** `set_value` / `ax_action` 以实际 locator PID 或成功结果的 `matched.pid` 为准；若两者都有但不一致，或没有可证明的 PID，不观测。窗口必须来自本次操作的显式候选，并通过同样的归属校验。不会为观测改写 AX 动作现有的定位规则。 / Use the effective locator PID or successful matched PID; conflicting evidence or an unproven PID means unavailable. An explicit candidate window still needs owner validation. Do not change AX action selection.
- **窗口动作 / Window mutations:** `move_window` / `resize_window` 在请求观测时，可从现有 `window_lookup` 获取该动作窗口的 PID；查不到则观测不可用，不影响动作。 / Reuse `window_lookup` for the selected window when observation is requested; lookup failure only disables observation. Carry the owner from before dispatch, not a newly discovered owner after settlement.
- **全局与无法归因 / Global and unattributable:** 保留真正无目标、全局投递输入以及 `clipboard_write` 的原有全局观测。带作用域声明但实际全局投递时，不把前台或整屏当作该目标的回执；生命周期动作没有可信的执行 PID 时同样明确不可用。不要为本轮重新定义 `display_id`、坐标或其他选择器的动作语义。 / Preserve legacy global observation for genuinely unscoped global input and clipboard writes. Global delivery despite a named process/window and lifecycle mutations without execution-owned identity get an unavailable result, not a wider read. Do not change display/coordinate selector semantics.
- 保留 `execute_batch` 的现有继承、逐项调用及失败停止机制。每项只观测自己的目标；不新增 batch 快照、不跨项复用目标，不更改现有审批、Escape 检查、保护规则和会话锁。 / Preserve existing batch inheritance, per-item dispatch and stop behavior, approval, Escape, safety guards, and session locks. No shared batch snapshot or cross-item target reuse.

## Post-state and pixels / 后置状态与像素

- `src/builtin_tools/desktop/observe.rs` 保留一次 300 ms settle。定向状态用 `QueryFocusedParams { pid: Some(pid) }` 并在 Core 拒绝非该 PID 的返回；不查询或附加系统前台应用。沿用安全值脱敏和 200 字符上限。全局保留 `frontmost_app` / `focused_element`。 / Preserve one 300 ms settle and secure-value redaction with the existing 200-character bound. Use PID-bound AX focus, check the returned element belongs to that PID, and omit the system frontmost app for targeted state. Preserve existing global state fields.
- 保留 `post_state`、成功时的 `post_screenshot`、`success`、`message`、`effect` 的现有含义。给 `post_state` 添加 `scope: global | process | unavailable`，进程观测附 `target_pid`；无法证明焦点时显式标记 `focus_status`（`not_found` 区别于 `unavailable`），并省略焦点元素。请求了图像但未拿到时，以 `post_screenshot_unavailable` 返回稳定原因（至少区分未确认作用域/窗口、窗口捕获失败），不塞入假图。观测既不能提升效果等级，也不能触发动作重放或失败。 / Keep the existing result keys and effect semantics. Add a scope and PID, distinguish missing focus from unavailable focus, and use a separate screenshot-unavailable reason instead of a fake image. Observation cannot promote an effect, trigger a retry, or fail a completed action.
- 进程截图必须携带 `(window_id, expected_pid)`。Core 通过 `desktop/shared/src/traits/screen.rs` 的窄类型化方法访问；缺省实现不支持，不能转调用普通截图。新增独立内部 RPC `screen.capture_owned_window`，沿用 `CaptureResult`；旧 helper 未注册此方法就拒绝，不调用旧 capture 捕获图像。macOS 在 `ScreenCaptureHandlers.swift` 用**同一个** `SCWindow` 对象核对 PID 并创建独立窗口的 filter。ID/PID 无效、找不到窗口、缺少所有者或不匹配时，在捕获前拒绝；定向截图不显示指针，不允许 display/region fallback。 / An owned-window capture takes `(window_id, expected_pid)` through a typed `ScreenCapability` method, unsupported by default. Use a separate internal RPC with the same capture result so older helpers fail closed. On macOS the exact `SCWindow` whose owning application PID was validated supplies the capture filter. Reject invalid IDs or absent/mismatched owners before capture. No cursor or display/region fallback.
- 复用截图编码、空帧拒绝、1568 宽度限制、图像预算、窗口坐标元数据和图像结果通道。只调整用于复用的截图分支，不移动平台 API 到 Core、不新增图像格式或隐式重试。独立 `screenshot` RPC 和公开行为保持兼容。移除 `src/builtin_tools/desktop/mod.rs` 忽略目标重造全局 `screenshot` 参数的旧路径，`observe.rs` 不再无条件读取全局焦点。 / Reuse the screenshot pipeline, blank-frame rejection, 1568 width limit, image budget, coordinate metadata, and image ingress. Remove the old unconditional observation path; standalone screenshots retain their contract.

## Non-goals / 非目标

不做 Zahir 投影挂载、交互审批接线、授权扩张、自动重放、全局快照调度或性能换代。`verify_state` 的稳定采样和效果分级不变。第一阶段不实现在 Windows/Linux 上的所有者约束截图，也不宣称跨平台实拍一致；不会用 PID 承诺跨进程退出/重启的身份连续性。 / No Zahir projection, interactive approval, broader authority, automatic replay, new snapshot scheduler, or performance redesign. Verification and effect grading stay intact. No new owned capture backend on Windows/Linux or assertion of identity continuity across process lifetimes.

## Proof and cleanup / 验证与清理

1. 先写具体回归：模拟定向输入和不同前台，断言 AX 参数和返回内容；窗口 PID/ID 随同一个动作传到 Bridge 捕获边界；无窗口、拒绝、所有者不符、AX 返回外部 PID、旧 helper 或不支持均不可宽化（记录并断言全局捕获次数为零）。再覆盖无目标全局兼容、启用与未启用观测、成功但观测不可用、批次逐项结果/停止以及嵌套图像仍可被提取。 / Test actual boundary calls and outputs, including zero global captures for scoped failure cases, global compatibility, opt-in, preserved success/effect, per-item batch behavior, and nested image ingress.
2. 为新 RPC 验证严格输入解析、实际所有者判据、缺失所有者和无回退；执行 Rust 与 Swift 的定向构建/测试。可用且已经授权时运行现有 macOS fixture 检查后台 AX 和窗口捕获，否则注明 TCC/真机验证缺口。 / Verify strict RPC validation, the owner predicate, no fallback, and focused Rust/Swift checks. Run the existing macOS fixture only if authorized and prerequisites are present; report gaps.
3. 更新 `src/builtin_tools/desktop/perm.rs` 的平台中立工具描述，不更改权限动作。校对 `docs/reference/FEATURE_LOCATOR.md` 对应段落及 `docs/reference/DESKTOP_BRIDGE.md`，区分现有工具入口和未挂载投影，记下平台差异。提交仅在 `refactor/computer-use`，不操作 `main`。 / Correct permission tool wording without behavior changes, update the relevant reference docs, and commit only on the isolated branch.

实现者可决定内部类型、诊断枚举的命名和测试夹具布局；不得放宽目标或观察边界。 / Internal type names, diagnostic enum spellings and fixture layout are implementation choices; scope constraints are not.
