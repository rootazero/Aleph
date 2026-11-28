# Phase9 runtime QA / 运行时 QA

This suite opens real macOS native windows hosting the production Panel. The deterministic model endpoint replaces only the model protocol; it does not supply Gateway RPC replies, artifact values, or UI projections. / 本套件实际打开 macOS 原生窗口并托管生产 Panel；模型夹具只替代模型协议，Gateway、存储与 UI 仍走生产实现。

## Inputs and ownership / 输入与所有权

Build the NSH host and `aleph-server` into the same task target on macOS. Use the memory gate before each Cargo command, and use `--offline --locked --jobs 1` with the task-specific `CARGO_TARGET_DIR`. / 在 macOS 上把 NSH 宿主和 `aleph-server` 构建到同一任务目录。每次 Cargo 调用先过内存门禁，并使用任务专用目录离线串行构建。

The old-receipt challenge requires an explicitly separate Panel bundle: build it with `ALEPH_PANEL_RUNTIME_QA=1 just wasm` before building the Gateway used by this suite. The optional `runtime-qa` feature retains the current and previous connection receipts and calls the production notification ingress. Normal `just wasm` keeps this feature off. Record both bundles' sources and digests separately. / 先单独构建带旧 receipt 探针的 QA Panel，再构建 Gateway。默认生产构建关闭该注入入口，两种产物分别记录来源与摘要。

```sh
ALEPH_PANEL_RUNTIME_QA=1 just wasm
python3 qa/browser_live/check_memory.py --required-bytes 4294967296 &&
  CARGO_TARGET_DIR="$PWD/.phase9-target" cargo build --offline --locked --jobs 1 \
    -p alephcore --bin aleph-server -p aleph-nsh-shell --bin aleph-nsh-shell
```

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

NSH receives debug-only `ALEPH_NSH_QA_URL` and `ALEPH_NSH_QA_NONCE`. The collector binds stdout observations to the nonce. / NSH 通过仅调试构建支持的环境变量接收隔离地址和 nonce，采集器用 nonce 关联标准输出。

## Evidence boundaries / 证据边界

The shared driver submits through the existing composer. It observes actual `connect`, `sessions.list`, `chat.history`, `chat.send`, `artifacts.list` and wire stream events. `session.artifact` remains an invalidation; refreshed data must arrive through `artifacts.list`. Queued is never counted as accepted. / 共用脚本只观察并操作真实输入框，不编造 RPC 成功。

After a real disconnect/reconnect and fresh snapshots, the old-epoch challenge invokes the retained stale receipt through the same Panel notification ingress used by production. It also invokes a current-receipt positive control. Foreign-session, malformed-topic and duplicate frames reach the actual current WebSocket event handlers. Rendered Chat/artifact values are compared across two animation frames. A changed projection records `projection-violation`; neither a closed socket with no handler nor the observer's own filtering can satisfy this gate. / 旧 epoch 必须实际调用持有旧 receipt 的入口，并通过当前 receipt 的正向控制；不能用无接收器的 socket no-op 冒充拒绝证据。

The Gateway fixture also retains the actual identity-encoded HTTP bytes for `index.html`, `aleph_panel.js`, `aleph_panel_bg.wasm` and `tailwind.css`, then verifies the same digests after the native runs. This is required because a debug RustEmbed Gateway serves worktree assets rather than binding them exclusively into the executable. The live verifier checks those retained payload hashes in addition to binary/log hashes. Raw served payloads are local evidence, not a downloadable release bundle. / debug Gateway 的二进制摘要不能替代实际 HTTP 产物摘要；运行前后资源变化必须失败。

The collector launches the NSH host through public NSWorkspace in its own application coalition, saves each raw LaunchServices listing and the exact per-PID RSS/thread measurements, and verifies aggregate sums. Bare coalition IDs denote singleton membership; a controlled fork/join experiment must precede accepting that output shape. Host identity is published through a nonce-bound QA PID rendezvous and independently matched to the NSWorkspace result. A first real resource sample releases the debug-only startup gate; native events and samples use public `CLOCK_MONOTONIC` nanoseconds so startup and terminal coverage share one time axis. Samples remain subject to the unchanged 50–175ms interval/coverage checks. / NSH 宿主通过公开 NSWorkspace 独立启动；coalition 决定归属，名称只检查角色完整性。保留原始成员、逐 PID 数值和统一单调时间轴，不以全机差值或父进程树替代 WebKit 归属。

Public kqueue `NOTE_EXITSTATUS` records the actual native host status, separately from helper exit. All observed coalition PIDs must reach terminal state; settlement is observed immediately after native closure, before waiting for the helper's own bookkeeping. The helper, owned Gateway/provider, and fixture root must also settle normally. Native capture files are written to local OS temp and archived after shutdown to avoid external-volume I/O blocking the application. Saved events must exactly match nonce/run-bound markers reparsed from verified stdout. / 宿主的真实退出码、全部已观测成员终态、helper 和 fixture 清理分别验真；不把 helper 成功当宿主成功，也不回填假终态采样。

`runtime-valid.json` is a **protocol fixture**, never live proof. `--runtime-json` validates a saved observation but requires verified capture provenance for a live PASS. / 纯夹具只验证格式与关联，不能冒充开窗证据。

## Metrics / 指标

The current launcher records reconnect timing through the history application witness and subsequent rendered frames, peak aggregate RSS, maximum aggregate threads, and time to all-member settlement. Use one isolated run for smoke verification. For release acceptance, use the [R7 cutover receipt](../docs/superpowers/runs/2026-11-27-nsh-r7-cutover-receipt.md). / 单次隔离运行适用于冒烟检查，采集历史应用与渲染见证、RSS 峰值、线程峰值和全成员退出用时。发布验收见 R7 回执。

The retired dual-host comparison used 10 warmups and 30 measured iterations per host. The [archived Phase 9 guide](https://github.com/rootazero/Aleph/blob/fe979a1d7e4dea2653698df6443e6e138a065434/qa/phase9_runtime.md) preserves the matrix procedure and original performance thresholds. / 历史双宿主对比为每端 10 轮预热和 30 轮测量，归档指南保留原流程与阈值。
