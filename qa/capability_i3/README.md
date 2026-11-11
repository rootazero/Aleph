# Capability I3 production integration probe / 生产集成探针

## Execution boundary / 执行边界

**The Task7 implementation grant permits probe code and static validation only. It does not authorize running this probe.** A controller must separately approve the actual binary digest, Panel output, isolated artifact directory, process/network/browser scope and Stage B scenarios before execution. No live result is implied by this file.

**Task7 实现授权只允许编写探针及静态验证，不允许运行本探针。** 实际运行前，Controller 必须另行批准二进制 SHA、Panel 产物、隔离目录、进程/网络/浏览器边界及 Stage B 场景。本文不代表任何 live 验收结果。

The module has no import-time actions. Static checks may parse its AST and compile it to a **new SDD `.pyc` path**, without importing it, invoking `main`, starting a listener or contacting a server. No dependency install or Panel build is part of these checks.

模块 import 时不执行动作。静态检查仅解析 AST、编译到**新的 SDD `.pyc` 文件**；不得 import 执行、调用 `main`、启动监听或连接服务。不得附带安装依赖或构建 Panel。

## Separately approved invocation / 单独批准后运行

The following is a future execution interface, **not an instruction to run it under the Task7 code-only grant**. Use absolute paths and an existing artifact parent. The parent may contain an expensive build target: the probe creates a fresh child and never deletes the parent or an old evidence directory.

以下仅描述未来接口，**不是要求在 Task7 code-only 授权中执行**。使用绝对路径及已存在的证据父目录。父目录可能存放构建 target；探针只创建新子目录，不删除父目录或旧证据。

```sh
python3 /Volumes/TBU4/Workspace/Aleph/qa/capability_i3/probe.py \
  --binary /absolute/path/to/approved/aleph-server \
  --sha256 APPROVED_64_HEX_DIGEST \
  --panel-output /absolute/path/to/approved/panel-output \
  --artifacts /absolute/path/to/existing/artifacts-parent
```

Requirements / 前置条件：

- Python 3.10+ and the already available `websockets` package; the probe never installs anything. / Python 3.10+，且已有 `websockets`；不自动安装。
- POSIX signal support for the currently implemented shutdown scenario. Windows is explicitly rejected rather than labelled tested. / 当前退出场景需要 POSIX；Windows 明确拒绝，不冒充已测。
- An approved server binary; both input and private copied bytes must match `--sha256`. / 输入及私有副本 SHA 均须匹配批准值。
- Approved Panel output containing `index.html`, `build.json`, `build-inputs.json`; current file hashes and sizes enter the receipt. This is **not** a compressed HTTP/browser attestation. / Panel 必须有这三个文件；记录当前文件 SHA/大小，**不等于**压缩 HTTP/浏览器验真。

## Isolation and evidence / 隔离与证据

The probe starts its private copy with the real root flags `--config`, `--bind 127.0.0.1`, `--port`, `--webchat-dir`, followed by `start`. It uses a fresh HOME/ALEPH_HOME, a small environment allowlist, disabled background integrations, a loopback synthetic model provider and an obviously fake API key. It checks the private `.ipc-endpoint.json` PID against its own `Popen.pid` and verifies the loopback URL/port before WebSocket connection; it does not borrow a developer daemon.

探针使用真实根参数启动私有副本：`--config`、`--bind 127.0.0.1`、`--port`、`--webchat-dir`、`start`。使用全新 HOME/ALEPH_HOME、环境白名单、关闭后台集成、回环假模型及明显假 key。连接前核对私有 `.ipc-endpoint.json` 的 PID 与自有 `Popen.pid`、回环 URL/端口，不借用开发者 daemon。

Only processes it launches are signalled. The ordinary signal drain has a 45-second guard; failure is recorded and only still-running owned processes are terminated/killed in cleanup. The probe never kills a PID discovered from a port or an unrelated endpoint. It leaves its fresh artifact directory for inspection.

只向自己启动的进程发信号。正常退出保护为 45 秒；失败记录为失败，cleanup 仅终止/杀掉仍存活的自有进程。不按端口或其他 endpoint 猜测 PID，不删除新证据目录。

`receipt.json` contains check names/booleans, safe failure codes, input provenance, owned PID and event counts. Tickets, device tokens, real keys, request arguments, result bodies, UI text and raw PTY streams are not copied into it. The synthetic provider request log and process streams are discarded. A failure never becomes a skipped PASS.

`receipt.json` 只保留检查名/布尔值、安全错误码、输入来源、自有 PID 和事件计数；不写 ticket、device token、真 key、参数、结果正文、UI 文本或原始 PTY 流。假模型请求日志及进程输出丢弃。失败不转为 skipped PASS。

## Scenarios and independent oracles / 场景与独立判据

- A single reader per WebSocket dispatches replies to pending futures and preserves interleaved notifications; an in-flight RPC cannot silently consume audit frames. / 每个连接单 reader 分流 reply 并保留交错通知，等待 RPC 不吞审计事件。
- Real `users.create` → `gateway.ticket.create` → credential `connect` creates two independent operator identities and a second connection for the first identity. An unauthenticated connection remains a guest. / 使用真实用户、ticket、凭证连接路径建立两名独立 operator，以及同一用户的第二连接；另留未认证 guest。
- Real `chat.send` creates a session; UI attach/display/ack/detach checks private source commit/revoke exactly once. The same human's second connection sees audit records but not the functional display frame; the unrelated operator and guest see neither private source record. / 真实 chat 建 session，UI attach/display/ack/detach 检查 source 启用/撤销各一次；同人另一连接可看审计但不可收功能 display；无关 operator 和 guest 不可看该私有 source。
- Real Memory start/note/stop and real PTY spawn/close produce source-withdrawal frames. Approval/configuration refusal is a failure, not a reason to bypass final policy. / 真实 Memory 与 PTY 操作产生 source 撤销；审批或配置拒绝记失败，不绕过最终策略。
- Removing `capability.event`/status subscriptions while retaining the functional UI selection demonstrates that WebSocket `SubscriptionManager` selection is separate from entitlement. / 撤掉审计 topic 订阅、保留 UI 功能订阅，验证 WS selection 不等于 entitlement。
- Received sequences must be monotonic and duplicate-free; display text must not appear in capability audit frames. A bounded negative quiet window is only an observation, not a proof of every concurrent interleaving. / 序列单调无重复，功能文本不进入审计；短时阴性窗口仅是观察，不代表所有并发交错证明。
- The owned server receives SIGTERM and must exit cleanly. / 仅自有 server 接收 SIGTERM 并须正常退出。

## What this probe cannot establish / 不可由此推出的结论

There is **no invented remote audit-read endpoint**. The Rust test `gateway::capability_events_tests::production_events_reach_zahir_ws_and_same_sequence_audit` exercises actual Tool result, Memory Ready/stop, PTY child publication/close and UI mount/withdraw, comparing typed Zahir delivery, raw WebSocket bridge records and persisted records at the same sequence. WebSocket observations alone are not used to claim this equivalence.

**不虚构远程 audit endpoint**。上述 Rust 定向测试走真实四类 source，并比较 typed Zahir、WS bridge、持久化记录的同一序列；不能仅凭 WS 宣称三路一致。

Rust tests separately exercise fresh current-connection identity/role and session-store ACL, contextual resolve/prepare refusal, final domain policy, actual diagnostic mutation/audience/deduplication, no-rule admission drain, caller-cancelled UI ownership, queued/lagged/idle bridge close/join, and cancellation/re-entry of retained `StartupResources` cleanup. Their exact frozen input/log/receipt hashes belong in Task7 evidence; this README does not assign itself an independent review result.

Rust 测试另覆盖当前身份/角色/session-store ACL、contextual resolve/prepare 拒绝、最终 domain policy、真实 diagnostic mutation/audience/去重、无规则 drain、UI caller 取消后 owner、积压/lag/idle bridge close/join，以及 StartupResources 取消重入。准确输入/日志/receipt SHA 以 Task7 证据为准，本文不自评独立审查通过。

LAN/source-IP facts, browser rendering, current compressed Panel bytes, adversarial live login/roster replacement, and the entire owning `start_server` future being forcibly dropped require separately approved Stage B evidence. A clean signal result does not prove whole-future force-drop. No Task7/I3/Stage A/Stage B/merge PASS follows from static probe validation.

LAN/source-IP、浏览器渲染、当前 Panel 压缩字节、对抗性 live 登录/roster 替换、整个 start_server owning future 强制 drop，都需要另批 Stage B 证据；信号正常退出不证明整个 future force-drop。探针静态检查不推出 Task7/I3/Stage A/Stage B/merge PASS。
