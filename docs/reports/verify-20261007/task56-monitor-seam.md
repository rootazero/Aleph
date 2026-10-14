# Task 56 — QA monitor source seam (UNCOMPILED / UNRUN)

> **SUPERSEDED (2026-10-12, source-only, UNCOMPILED/UNRUN) / 已被取代：** the page-side `__TAURI_INTERNALS__.invoke` wrapper described below is **removed**. A real Native run showed the property is immutable (`install_failed reason:not_writable`), so it could never observe anything. Credential-execution evidence is now a **QA-build-only Rust trace** (`desktop/shell/src/qa_trace.rs`, feature `qa-owned-root`): each `panel_credential_load/save/delete` handler writes one fixed-schema `ALEPH_QA_TRACE {...}` line to the shell's own **stdout pipe** (bound to the harness-spawned child pid, monotonic `seq`, `auth`+`pid` checked, `NativeTraceStream` in `qa/native_ipc/monitor_contract.mjs`) AFTER the real operation. Production builds contain no trace code. The page WebSocket is now the DOM driver only and is never credential evidence. Historical text follows.
> 页面侧 invoke 包装已删除；凭据执行证据改为 QA 构建专用的 Rust stdout 管道 trace（固定 schema，绑定子进程 pid，`seq` 单调）。下文为历史记录。

**状态 / Status：仅源码改动，未编译、未运行、未测试。** 没有执行 Cargo、浏览器、keyring、Gateway 或任何进程；没有触碰 HOME/.aleph；没有 commit/reset。以下只是源码审计结论，不是验证通过。
Source-only change. NOT compiled, NOT run, NOT tested: no Cargo, browser, keyring, Gateway or process was executed; no HOME/.aleph access; no commit/reset. Everything below is a source audit, not a pass.

## 改了什么 / What changed

仅 3 个文件 / Exactly 3 files:

1. `desktop/shell/src/qa_root.rs`
   - 新增 `--qa-monitor-port <1024..=65535>`（默认缺省）。纯函数 `parse_monitor_port` / `parse_port_value`：只读 argv（无 env/HOME 回退）；纯十进制（无符号、无前导 0、无空白、仅 ASCII 数字）；拒绝重复、缺值、`--flag=value` 形式、越界/非数字（新增 `QaRootError::MalformedMonitorPort`）。
   - 端口只在 owned root 被证明**之后**才发布（`MONITOR_PORT: OnceLock<u16>`）；`pub(crate) fn monitor_port()` 在 root 未证明时返回 `None`。
   - 4 个纯解析测试（不碰文件系统/进程）。
   - Adds `--qa-monitor-port` (default absent). Pure parser reads argv only (no env/HOME fallback); plain decimal only; refuses duplicate, missing value, `=` form and out-of-range/non-numeric values. The port is published only after the owned root is proven; `monitor_port()` is `None` otherwise. Four pure parser tests.
2. `desktop/shell/src/main.rs`（全部在 `#[cfg(feature = "qa-owned-root")]` 下 / all behind the feature）
   - 固定常量 `QA_MONITOR_JS` + `qa_monitor_script(port: u16)`：仅把 `u16` 的十进制数字替换进一个占位符（只能产生 1–5 位数字）。
   - `build_main_window` 的 qa 块内：`monitor_port()` 为 `Some` 时追加一条 `initialization_script`。
   - 无新 Tauri command / permission / 依赖。No new commands, permissions or dependencies.
3. 本报告 / this report.

## 脚本行为 / Script behaviour

- 包装 `window.__TAURI_INTERNALS__.invoke`：以原 `this`/参数调用原函数，**返回同一个 promise**（或原样重抛同步异常）；观察用的是旁路 `p.then(ok, err)`，不改变返回值/拒绝原因。
- 只观察 3 个固定命令：`panel_credential_load|save|delete`。其余命令完全不上报。
- 只上报：固定枚举字符串、白名单内的**键名**（`credential`、`lease_id`，按白名单 `hasOwnProperty` 判断，不枚举页面数据）、结果形状标签（`lease_object|null|bool|unexpected`）、固定错误码（否则 `unexpected`）、`credentialPresent` 布尔。绝不读取/转发 credential、ticket、lease 值、DOM 文本。忽略 socket 回传的一切内容。
- 唯一端点：`ws://127.0.0.1:<port>/qa`（host/path 为编译期固定）。
- 包装器无法安装（`no_internals` / `no_invoke` / `not_writable`）→ 首个也是唯一一个事件 `install_failed {trusted:false, reason}`，**不发 `hello`**，随后关闭。
- Wraps the invoke function, forwards with original `this`/args, returns the identical promise, observes via a side `.then`. Only the 3 fixed credential commands are reported; only fixed enums, allow-listed key names, shape tags, fixed error codes and booleans are emitted; never any value; incoming messages ignored. Sole endpoint is `ws://127.0.0.1:<port>/qa`. On install failure the sole first event is an untrusted `install_failed` (no `hello`).
- 信封 / envelope：`{v:1,type,run,seq}`，`run` 为 128 位随机小写 hex，`seq` 从 0 连续，上限 512（超限发 `monitor_end{monitor_error}`）。事件类型：`hello`、`invoke_start`、`invoke_end`、`monitor_end`（`pagehide`→`page_unload`）。

## 与 harness 的已知不一致 / Known mismatches with the harness

- `qa/native_ipc/monitor_contract.mjs` 的端点校验要求同源 `/__qa_monitor/<run>`，而本任务指定 `ws://127.0.0.1:<port>/qa`（对 Tauri 页面是跨源）。按任务实现；harness 校验器/契约需后续对齐。
- `install_failed` 不在契约事件集合内：校验器会以 `unknown_type` 拒绝——这是有意的 fail-closed（不可信）。
- Contract M6 requires a same-origin `/__qa_monitor/<run>` endpoint; the task mandates `ws://127.0.0.1:<port>/qa` (cross-origin). Followed the task; the validator/contract must be reconciled later. `install_failed` is not a contract type, so the validator rejects it (intended: untrusted).

## 换行 / Line endings

任务称 CRLF，但磁盘上三个文件实测为 LF（0 个 CR），已保持 LF 不变，未引入 CR。
The task said CRLF, but the files on disk were measured as LF (0 CR); LF preserved, no CR introduced.

## 源码审计 / Source audit (diff vs pre-edit copies)

- 原始文本唯一块替换；`QA_MONITOR_JS` 内无 `"#`（仅结尾一处终止符）；占位符在模板中恰好一处（JS `var PORT = …`），`replacen(…, 1)`。
- `initialization_script(&script)`：`&String` 同时满足 `&str` 与 `impl Into<String>` 签名。
- 测试只调用纯函数 `parse_monitor_port` 与既有 helper。
- Raw-string terminator appears once; the placeholder occurs exactly once in the template; `&script` satisfies either signature; tests call only pure functions.

## 没做 / 未验证 (Negatives)

- 未编译、未 `cargo test`/clippy、未运行任何 JS（语法仅靠肉眼审计）。Not compiled; no tests/clippy; the JS was never executed (syntax only eyeballed).
- 未验证 init script 相对 Tauri 核心脚本的执行顺序，也未验证它是否应用于 iframe/子 frame。Init-script ordering vs Tauri core scripts and sub-frame coverage unverified.
- WebView2 下从页面源连 `ws://127.0.0.1` 是否被 mixed-content/CSP 拦截未验证（`csp: null`，但远程 https 源的情形未测）。Mixed-content behaviour from a remote https origin untested.
- 旁路 `.then` 会把 promise 标记为已处理：若调用方本身不处理拒绝，会抑制 `unhandledrejection`。The side `.then` marks the promise handled (suppresses `unhandledrejection` for callers that don't handle it).
- 每个 document 一个 run token/流（splash 与 Panel 各自一条）；并发 invoke 会被校验器以 overlapping_invoke 拒绝；非预期形状/错误码以 `unexpected` 上报（fail-closed）；白名单之外的多余参数键不上报。One run/stream per document; overlapping invokes get rejected by the validator; unexpected shapes/codes are reported as `unexpected`; extra arg keys outside the allow-list are not reported.
- 不发 `connect_evidence` / `dom_command` 事件。`monitor_end` 依赖 `pagehide`，Windows 上尽力而为。No `connect_evidence`/`dom_command` events; `monitor_end` via `pagehide` is best effort.
- 未修改 harness（`qa/native_ipc/*`）、未传递 `--qa-monitor-port`、未做任何真机 QA。The harness is untouched, the flag is not yet passed by anything, and no real Native QA was done.
