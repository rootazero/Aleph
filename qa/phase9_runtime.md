# Phase9 runtime QA / 运行时 QA

This suite opens real macOS native windows hosting the production Panel. The deterministic model endpoint replaces only the model protocol; it does not supply Gateway RPC replies, artifact values, or UI projections. / 本套件实际打开 macOS 原生窗口并托管生产 Panel；模型夹具只替代模型协议，Gateway、存储与 UI 仍走生产实现。

## Inputs and ownership / 输入与所有权

Build the debug binaries `aleph-server`, `aleph-desktop-shell` and `aleph-nsh-shell` into the same task target. Tauri must use `--no-default-features --features aleph-desktop-shell/qa-owned-root`; this is the debug lite baseline. Before **every Cargo command**, require `python3 qa/browser_live/check_memory.py --required-bytes 4294967296` to return exit 0 / ALLOW, then use `--offline --locked --jobs 1` and the task-specific `CARGO_TARGET_DIR`. / Cargo 必须逐次过内存闸门，构建与测试由同一个 owner 串行执行，不隐式联网。

The old-receipt challenge requires an explicitly separate Panel bundle: build the canonical recipe with `ALEPH_PANEL_RUNTIME_QA=1 just wasm`, then embed that bundle in the Gateway used by this suite. The optional `runtime-qa` feature retains the current and previous connection receipts and calls the same production notification ingress. Normal `just wasm` keeps this feature off. The task's Cargo wrapper still enforces the memory gate and `--offline --locked --jobs 1` for both the build and metadata invocations; send gate output to stderr so Cargo metadata remains JSON. / 旧 receipt 实验使用显式 QA 产物；默认生产构建不包含 JS 注入入口。两种产物必须分别记录来源与摘要。

The Python schema dependency is `jsonschema==4.25.1`. Once explicitly fetched, use the offline environment:

```sh
uv run --offline --no-project --with jsonschema==4.25.1 python qa/nsh_panel_launcher.py \
  --repo-root . --target-dir .phase9-target \
  --fixture-root qa/fixtures/nsh-panel-parity \
  --gateway-binary .phase9-target/debug/aleph-server \
  --timeout-seconds 90 --runs 1 \
  --receipt docs/superpowers/runs/phase9-runtime-local.json
```

`--gateway-binary` owns a new loopback Gateway, a model endpoint and their private filesystem root. Both children are joined during cleanup. An alternative `--gateway-receipt` must identify an already isolated loopback Gateway and its binary digest; it does not transfer lifecycle ownership of that external process. / 自动模式只管理本次新建的子进程；传入现有回执不会接管外部 daemon。

Tauri receives its existing `--qa-root` / `--qa-nonce` argv. The root is an OS-temp direct `aleph-shell-qa-*` child with a 32-byte lowercase-hex nonce and `.qa-owned` containing nonce plus newline. NSH receives debug-only `ALEPH_NSH_QA_URL` / `ALEPH_NSH_QA_NONCE`; no NSH QA CLI is invented. Both use nonce-bound stdout observations. / 两个壳保留各自真实边界，不能伪造 NSH CLI。

## Evidence boundaries / 证据边界

The shared driver submits through the existing composer. It observes actual `connect`, `sessions.list`, `chat.history`, `chat.send`, `artifacts.list` and wire stream events. `session.artifact` remains an invalidation; refreshed data must arrive through `artifacts.list`. Queued is never counted as accepted. / 共用脚本只观察并操作真实输入框，不编造 RPC 成功。

After a real disconnect/reconnect and fresh snapshots, the old-epoch challenge invokes the retained stale receipt through the same Panel notification ingress used by production. It also invokes a current-receipt positive control. Foreign-session, malformed-topic and duplicate frames reach the actual current WebSocket event handlers. Rendered Chat/artifact values are compared across two animation frames. A changed projection records `projection-violation`; neither a closed socket with no handler nor the observer's own filtering can satisfy this gate. / 旧 epoch 必须实际调用持有旧 receipt 的入口，并通过当前 receipt 的正向控制；不能用无接收器的 socket no-op 冒充拒绝证据。

The Gateway fixture also retains the actual identity-encoded HTTP bytes for `index.html`, `aleph_panel.js`, `aleph_panel_bg.wasm` and `tailwind.css`, then verifies the same digests after the native runs. This is required because a debug RustEmbed Gateway serves worktree assets rather than binding them exclusively into the executable. The live verifier checks those retained payload hashes in addition to binary/log hashes. Raw served payloads are local evidence, not a downloadable release bundle. / debug Gateway 的二进制摘要不能替代实际 HTTP 产物摘要；运行前后资源变化必须失败。

The collector launches each surface through public NSWorkspace into a separate application coalition, saves each raw LaunchServices listing and the exact per-PID RSS/thread measurements, and verifies aggregate sums. Bare coalition IDs denote singleton membership; a controlled fork/join experiment must precede accepting that output shape. Host identity is published through a nonce-bound QA PID rendezvous and independently matched to the NSWorkspace result. A first real resource sample releases the debug-only startup gate; native events and samples use public `CLOCK_MONOTONIC` nanoseconds so startup and terminal coverage share one time axis. Samples remain subject to the unchanged 50–175ms interval/coverage checks. / 两端通过公开 NSWorkspace 独立启动；coalition 决定归属，名称只检查角色完整性。保留原始成员、逐 PID 数值和统一单调时间轴，不以全机差值或父进程树替代 WebKit 归属。

Public kqueue `NOTE_EXITSTATUS` records the actual native host status, separately from helper exit. All observed coalition PIDs must reach terminal state; settlement is observed immediately after native closure, before waiting for the helper's own bookkeeping. The helper, owned Gateway/provider, and fixture root must also settle normally. Native capture files are written to local OS temp and archived after shutdown to avoid external-volume I/O blocking the application. Saved events must exactly match nonce/run-bound markers reparsed from verified stdout. / 宿主的真实退出码、全部已观测成员终态、helper 和 fixture 清理分别验真；不把 helper 成功当宿主成功，也不回填假终态采样。

`runtime-valid.json` is a **protocol fixture**, never live proof. `--runtime-json` validates a saved observation but requires verified capture provenance for a live PASS. / 纯夹具只验证格式与关联，不能冒充开窗证据。

## Metrics / 指标

Use `qa/nsh_panel_matrix.py --runs 40` (with `--repo-root`, `--target-dir`, `--gateway-binary` and `--receipt`) for acceptance: it creates a fresh Gateway/provider/session fixture for every iteration, collects Tauri first and NSH second, and retains 10 warmups plus 30 measured samples for each. The lower-level launcher's multi-run mode is not the fresh-fixture acceptance matrix. Reconnect timing ends only after the production history application witness and subsequent rendered frames. Each run retains peak aggregate RSS and maximum aggregate threads; close timing includes all-member settlement. Export requires every runtime gate and complete resource ownership. The value-only metric files then enter the existing `nsh_panel_parity.py` reducer. Its p95 is one-based rank 29; latency median/p95 limits are 1.10/1.20, RSS median 1.10, and median threads baseline+2. A valid threshold miss remains `VALID / NOT_ACCEPTED`, exit 3. Debug evidence never proves release-profile performance. / 正式矩阵每轮重置服务与会话；运行事件、原始资源、指标值与既有阈值 reducer 分层验真。无效整块保留，不拼接成功轮。
