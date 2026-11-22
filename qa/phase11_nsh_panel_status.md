# Production Panel native status QA / 生产 Panel 原生状态验证

This opt-in macOS fixture opens the **real Leptos/WASM Panel** in NSH against a freshly owned production Gateway. It does not replace RPC responses or call the credential/status bridge on behalf of the component. / 本夹具打开真实生产组件，不伪造 RPC，也不替组件调用原生方法。

## Build / 构建

Run the canonical `just wasm` recipe, enforcing the required 4 GiB memory gate before **every** Cargo invocation and `--offline --locked --jobs 1` (metadata has no jobs option). Use a local Cargo wrapper as described in [the runtime QA guide](phase9_runtime.md). Preserve existing dist bytes before rebuilding. The normal build is sufficient; `runtime-qa` is not required. / 正常 Panel 构建即可；保留旧资产，不需要旧 epoch 专用特性。

Build `aleph-server` and `aleph-nsh-shell --features qa-service` with the same memory/offline/locked/serial discipline. Do not run native QA concurrently with compilation or asset generation.

```sh
uv run --offline --no-project --no-python-downloads --python python3 --with jsonschema==4.25.1 \
  python qa/phase11_nsh_panel_status.py \
  --exe target/debug/aleph-nsh-shell \
  --gateway-binary target/debug/aleph-server \
  --receipt .phase10-evidence/NEW-OWNED-DIRECTORY/receipt.json
```

The receipt and its Gateway-output directory must be new. The fixture owns the provider/Gateway children, their private HOME, and native `data_dir`/`ALEPH_HOME`. The native process retains its **real login HOME** because replacing it made the real macOS OS-store probe fail. Credentials are isolated using the canonical random QA service namespace, not by pretending a fake HOME is an OS sandbox. No production login item, installed service or production credential namespace is modified. / 原生进程保留真实登录上下文；凭据由随机 QA 服务名隔离。

## Required effects / 必须到达的效果

1. The production Panel receives an empty native credential lease and a real credentialless authorized Gateway `connect` response. This is **not** protected-user or pairing evidence.
2. The observer navigates the actual SPA to `/settings/general`; the production `DesktopAutostartSection` performs its fixed document-bound native read itself.
3. The visible component displays the canonical translated unavailable/read-only text, no checkbox, and actual `Unavailable(NotAppBundle)`, not false/disabled. The locale sources own the expected text.
4. The host emits ordered fixture/close/native-return observations and exits zero. The served HTTP asset identities remain unchanged; the owned Gateway/provider are joined without forced kill and their private fixture is removed.

The observer's 45-second deadline records incomplete effects and closes; it does not convert a timeout into success. Native process timeout fails too. Diagnostics contain only fixed asset names, counters, error classes and booleans, never credentials or page text. / 超时保留失败；诊断不输出凭据、页面正文或拒绝 URL。

## Scope / 范围

The source contract is [Phase11 §15](../docs/superpowers/specs/2026-11-21-phase11-nsh-cutover.md). The current scoped result is bound in [the production Panel receipt](../docs/superpowers/runs/2026-11-22-phase11-production-panel-receipt.json). Earlier failed runs and the controlled HOME experiment are retained separately; none is rewritten as PASS.

This does **not** prove protected pairing/persistence, TLS/remote, native login-item writes, all-helper lifecycle census, release performance, Windows/Linux parity, full/lite service ownership, default/rollback, 14-day dual operation or Tauri removal. / 以上仍须独立完成，所有 Phase11 总闸门保持未验收。
