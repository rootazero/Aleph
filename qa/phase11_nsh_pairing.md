# Protected NSH Panel pairing / NSH Panel 受保护配对

## Scope / 范围

Run the real shared Leptos/WASM Panel in the real macOS NSH WebView. Reuse the existing `aleph-panel-native-qa-gateway` and its canonical SecurityStore, DeviceTokenManager and `handle_connect`; this is **not** the full production server. No substitute connect reply, fake native bridge or new identity authority is used. / 真实 UI、原生 IPC 和规范鉴权实现，但不冒称完整服务验收。

The fixed debug+qa-service observer enters an isolated one-time ticket in the actual token wall, waits for the real save ACK and Gateway-derived Alice identity, reloads the document, then requires a stored native credential and protected authentication without sending another ticket. Only booleans and document IDs persist across reload. / 票据、设备令牌不进日志、回执或 sessionStorage。

After authentication, explicit QA-only load/delete/load calls clean the random canonical OS namespace. This is not a Panel deletion UI. The native process must return naturally; the helper must independently confirm pairing and report `listenerClosed` **and** `fixtureClosed`, then exit after control-pipe EOF. Served assets must remain unchanged and the owned root must disappear. / 原生、鉴权服务和数据清理均需真实结束。

## Run / 运行

First build the helper and host serially, running the documented 4 GiB memory gate before **each** Cargo command:

```sh
python3 qa/browser_live/check_memory.py --required-bytes 4294967296
cargo build --bin aleph-panel-native-qa-gateway --features native-qa --offline --locked --jobs 1
python3 qa/browser_live/check_memory.py --required-bytes 4294967296
cargo build -p aleph-nsh-shell --features qa-service --offline --locked --jobs 1
uv run --offline --no-project --no-python-downloads --python python3 --with jsonschema==4.25.1 python qa/phase11_nsh_pairing.py --exe target/debug/aleph-nsh-shell --helper-binary target/debug/aleph-panel-native-qa-gateway --receipt <new-evidence-directory>/receipt.json
```

Use a new receipt path. Preserve the real native login HOME; native profile/ALEPH_HOME, helper root and random credential service are isolated. Do not run native QA concurrently with compilation or asset builds. / 新建证据路径，不改真实服务或生产凭据。

## Failed-run recovery / 失败恢复

Acceptance never substitutes fallback cleanup for a native delete ACK. Two early fixture failures used `{}` instead of the fixed bridge's `null` load argument; their failed receipts remain. Their exact nonce-owned items were removed by metadata-only OS lookup plus exact service/account deletion, and absence was independently verified; **no password flag or token read** was used. Recovery receipts are separate from native acceptance. A private `*.cleanup-context.json` now preserves routing metadata (nonce and loopback origin, no ticket/token) for any future failed run. / 失败项可独立清理，但不能追认为原生删除成功；不要触碰非本次随机命名空间。

The initial helper shutdown failure also included a runner bug: its real camelCase wire fields were read as snake_case. A negative-direction evaluator test now binds the actual fields; no criterion was relaxed. / 保留反例，不改旧回执。

## Not proven / 未证明

Full production-server protected pairing, TLS/remote, mutable targets, other-platform native behavior, autostart writes, service/default/rollback, all-helper census, performance, real 14-day dual operation and Tauri removal remain outside this receipt. See the [Phase11 contract](../docs/superpowers/specs/2026-11-21-phase11-nsh-cutover.md) and [checkpoint receipt](../docs/superpowers/runs/2026-11-22-phase11-protected-panel-receipt.json).
