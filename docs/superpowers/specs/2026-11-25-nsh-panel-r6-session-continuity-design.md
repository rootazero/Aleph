# NSH Panel 第六轮：会话恢复与历史翻页 / Session continuity and older history

## 目标 / Intent

在同一 NSH TSX Panel（macOS IPC 与同源 Web）中，让用户能回到刚打开或新建的会话，并继续阅读超出初始窗口的历史。第四、五轮后的壳、偏好、富转录、运行详情和权限判断继续使用现有实现。先修会话选择，再接 Core 的转录顺序，最后连到现有列表。/ Keep the same selected conversation across view remounts and same-identity reconnects, then let users read beyond the initial history window. Reuse the existing shell, rich transcript, detail reads and Core access decisions.

不做 / Non-goals: 本地持久保存会话键或草稿、持久事件 replay、自动重试发送、任意消息跳转或全文搜索、终端接线、Capability 快照热刷新、默认宿主切换。/ No persistence of session selection or drafts across shell reloads, durable event replay, automatic resend, arbitrary message jumps or search, terminal mount, live Capability refresh or default-host cutover.

## 对照与选项 / Comparison and alternatives

| 方案 / Approach | 源码依据与代价 / Evidence and trade-off |
|---|---|
| **用户选择：会话恢复 + 可续读历史 / Selected: continuity and older history** | Noop `app/App.tsx` 的 `toCard` 及时间线把消息身份与窗口位置分开。NSH `examples/bench/data-table` 使用有限窗口，`examples/rust-exports` 将 Rust 对象寿命与视图订阅分开。Aleph 已有稳定投影顺序、NSH 键控列表和共享 Rust reducer；缺的是会话选择生命周期、底层行身份和可续读边界。代价是两种存储和两种宿主都要验证。/ Reuse projection order, the keyed list and shared reducer. Verify both stores and both hosts. |
| **未选：Capability 快照热刷新 / Not selected: live Capability refresh** | ExtensionManager 已输出带修订号的可见快照；可利用失效通知重取，但不会解决会话选择与历史截断。通知不能取代快照或授权。/ Reuse the scoped snapshot, but this would not fix conversation continuity or truncation. |
| **未选：只读终端历史 / Not selected: read-only terminal history** | Termlevel 的会话独立于视图，这一原则值得参考。Core 已有 `pty.history`；TSX v1 不挂载 PTY。只读也要先设计可见性与授权，不改 v1 白名单绕过。/ Core has history, but v1 deliberately does not expose PTY. A separate access design is required. |

## 1. 先连通会话选择 / Wire session selection first

- **只有 Shell 保存跨挂载的选择，且只存在内存中。** 复用现有 Shell/SDK 导航与 generation 守卫。已连接的 `(endpoint, non-empty subject)` 是跨连接复用意图的条件，不是授权凭据。断线或重连期间不读取旧意图；再次握手确认同一对值才恢复，endpoint 改变、握手确认主体改变或缺失则清空。没有 `subject` 的兼容宿主仍能在当前 generation 内打开会话。旧 generation / 已卸载模块不能写回选择。/ Keep one in-memory selection path through Shell/SDK. Hold it inaccessible during reconnect and restore it only after hello confirms the same endpoint and nonempty subject. Without a subject, selection is usable within the current generation only. A stale generation cannot write back. The Core still authorizes every call.
- Sessions 打开行时，先确认布局允许选择 Chat，再一起提交选中页签和会话意图。若 Chat 已隐藏，显示可理解的不可打开反馈，不偷偷记录目标，也不擅自取消隐藏。Chat 首次发送成功，用 Core 回传的规范会话键更新同一选择，不重挂载正接收流的视图。/ Commit an allowed tab selection and its intent together. A hidden Chat remains hidden, with feedback. A successful first send updates the same selection with Core's canonical key without remounting the live reader.
- 新挂载从 `chat.history` 恢复当前尾窗口及 Core 确认的活动运行，不从旧 UI 搬运转录、游标、草稿或活动状态。失败与空转录分开；已有目标在首次读取成功前，或收到不存在/不可见后，不允许发送到该键（旧 `chat.send` 可能创建不存在的键），也不自动降级成无键发送。用户可重试读取或显式选择新会话。新会话清除选择并启动空视图，不发 RPC；第一次手动发送走现有无 `session_key` 路径。离开旧会话不停止后台运行。/ Remount via an authoritative tail read; distinguish error from empty. Do not send to a selected key until the initial read succeeds or after a denial; retry the read or explicitly start a new conversation. Starting over neither sends nor aborts.
- 保留“结果未知、不自动重发”的语义。若发送尚未收到回执即卸载或失联，同主体回到 Chat 时显示一条不含内容、可关闭的结果未知提示，指向 Sessions 查找；不保存草稿、未确认键或迟到的回执，不试图从最新会话列表自动认领写入。仅保留内存提示，换主体清除。/ On unmount or disconnect before acknowledgement, show one dismissible, in-memory outcome-unknown notice when the same subject returns to Chat and direct them to Sessions. Do not store drafts, invent a key, adopt a late reply, or infer which session accepted a send.

## 2. 沿用 Core 顺序 / Page in canonical Core order

- 扩展现有 `chat.history` 的**可选、受限滚页模式**，不新增 RPC 或短路 `visibility::session_visible`。旧调用的 `before`、输出与异常降级语义不变；新模式不接受同时指定时间过滤与行边界。共享 Rust wire 类型描述滚页边界、返回的行标识和续页信息。新路径将边界当作不透明、限长且绑定会话的行引用，不能授权或替代 `session_key`；格式错误是参数错误，授权成功后才可检查该引用是否仍存在。老 Core 没有续页字段时仍可读尾窗口，Panel 不用时间戳猜下一页。/ Add an opt-in bounded read on the existing method. Older callers retain `before` and their output; older servers still provide a readable tail. Cursor validation does not change visibility or widen method admission.
- SQLite 复用 [`TRANSCRIPT_ANCHOR_SQL`](../../../src/session/projection.rs) 计算完整单会话的 `(anchor, id)` 顺序，随后用该顺序向前读取；不能先移除边界外的行再计算锚点。文件后端复用 `order_by_source_seq`，在同一次已排序读取里定位边界并截取。两端返回同方向、排除边界的页，且以多读一行或等价判据确定是否真正有更早行。行、边界和续页判据来自同一次存储读取；不让客户端根据 total、窗长或可见正文行推导。/ Apply an exclusive backward bound in the same ordering as delivery, after computing anchors. Derive continuation from the same read, not from counts, timestamps or the UI filter.
- 不承诺跨请求的冻结快照：新尾部增长不能改变分页方向，恢复插入在尚未读取的范围内可被后续请求读到，但已经读过的范围可能需要重取。失效边界（例如被截断的边界行）明确返回“重新读取最新”状态，不默默返回尾页伪装上一页，也不承诺识别所有仍保留边界 ID 的历史改写。已发布 Core 可能忽略新参数；新版 Panel 只在响应明确支持后才发送续页，并校验响应确实对应所请求的边界，不能把老 Core 意外返回的尾窗口当作历史页。/ Stable-store traversal is gap-free; no cross-request snapshot isolation. A removed boundary is an explicit restart. The client only requests continuation after support is signaled and checks the response matches the requested boundary; an old server's tail is never treated as an older page. Retained-ID rewrites are not universally detected.

## 3. 接回共享转录与现有列表 / Reuse the shared transcript and list

- 在 `TranscriptList` 顶部提供“加载更早消息”；结果在同一列表前插，视口不因前插跳回顶部或尾部。无更早消息、请求中、可重试错误和需重取最新是不同状态。无自动无限滚动；单次显式加载最多取当前尾窗口同等的 200 条投影行，历史列表依旧虚拟化，已经加载的数据保留在本次挂载内，内存按实际读入行数增长。/ Explicit load, capped at the existing 200-row tail budget per request; preserve the visible anchor, distinguish end, retry and stale, and keep existing virtualization. Loaded data lives only for the mounted view and costs memory proportional to what was loaded.
- 可滚页正文以 Core 行标识键控及去重；共享 Rust `chat_panel` reducer 是转录行、工具组和 live 行合并的唯一实现。仅保留对旧协议尾窗口的 `(run, role, timestamp, occurrence)` 键兼容路径；新版不能拿位置或时间当身份。复用 `chat.transcript_details` 和工具输出读取，单页最多 12 个运行详情；按运行去重，页边界把同一运行拆开时不重复渲染工具组。保留现有运行归属校验（最近 500 条投影行）；超出该范围或详情失败仍显示正文，明确详情未提供，不扩大授权或伪造工具状态。/ Replace occurrence-based keys for the new wire shape, not the renderer. Reuse scoped detail lookups, deduplicate split runs, and keep body available when details fail or the run falls outside the current 500-row attribution scan.
- 只有当前挂载、选择和请求仍有效才应用 RPC 结果。向前翻页不改变活动运行、停止目标或发送状态。实时 reconcile 仍更新权威尾部；旧页与尾页通过稳定行重叠安全合并，无重叠且无法证明连续时，丢弃旧窗口、显示原因并以权威尾部恢复，而不是画出貌似连续的历史。断线重挂载、切换会话和新会话均重建窗口；不承诺恢复旧滚动位置。/ Keep historical prepend separate from live status and reconcile the tail through existing logic. Merge only across confirmed overlap; on discontinuity reset visibly. Remount does not restore scroll position.

实现者决定 / Builder discretion: 游标序列化细节、现有 SDK 最小接口调整、可用错误呈现样式与测试夹具；不引入新 registry、传输协议、渲染器、存储索引或主机专用逻辑。替换跨身份的旧意图读写和现有仅尾部的加载分支，不保留双份新路径。/ Choose encoding, minimal SDK API and fixtures. Remove displaced state paths; keep compatibility only where there is an existing caller.

## 验收 / Acceptance

1. **选择 / Selection:** 从 Sessions 打开、首次发送后进出 Settings、同主体重连都回到同一 Core 会话；不同或无主体、不同 endpoint 与迟到的旧请求不能恢复或修改新选择。隐藏页签不吞操作；“新会话”不发写入或停止，第一次显式发送不带旧键。
2. **存储与授权 / Stores and admission:** SQLite 与文件后端分别构造同时间戳、补写乱序、遗留行、空窗口、边界被删以及跨会话边界夹具；按位置遍历与整段规范顺序一致且不重不漏；验证老 `before` 和 scoped Desktop/Web 可见性未改变。额外包含翻页时尾部增长而没有重复、不可见会话不泄漏边界检查结果。
3. **表现 / Presentation:** 共享 reducer + NSH headless 覆盖翻页与迟到详情/实时流交错、工具组只出现一次、退出 reader 的焦点不回归。复用 `qa/nsh_s3_chat`，在本机 macOS IPC 与同源 Chromium 中验证重挂载、翻页锚点和实际渲染；标明未跑平台和未量化性能，不将 headless 当真机。
4. **文档 / References:** 用实现和验收结果校对 `docs/reference/FEATURE_LOCATOR.md`、`docs/reference/DESKTOP_SHELL.md` 及受影响的 reference 段落。解释权限、兼容、已验证范围。变更仅提交到 `nsh-panel-r6`。
