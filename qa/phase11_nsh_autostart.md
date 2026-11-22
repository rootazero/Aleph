# Phase11 read-only host autostart QA / 宿主自启动只读验证

This verifies the current macOS application's login-item **status**, not server-service autostart and not the NSH setter. / 验证宿主登录项读取，不测试服务器服务或设置写入。

## Build / 构建

```sh
python3 qa/browser_live/check_memory.py --required-bytes 4294967296
# Only after ALLOW; run serially, before native QA.
cargo build -p aleph-nsh-shell --features qa-service --offline --locked --jobs 1
```

## Real signed-app status / 真实隔离应用状态

```sh
python3 qa/phase11_nsh_autostart.py \
  --exe target/debug/aleph-nsh-shell \
  --receipt .phase10-evidence/phase11-autostart-new/receipt.json
```

Use a new evidence directory. The runner copies the executable into a nonce-isolated `.app`, writes that QA identity's Info.plist, ad-hoc signs and verifies it, and invokes `--autostart-status` directly. It does **not** use `open`, register the application/login item, launch a server or change production identity/configuration. / 使用新目录，直接运行隔离包，不注册应用或登录项，不修改生产配置。

Acceptance requires the unbundled NotAppBundle refusal, the wrong-identity IdentityMismatch refusal, and two equal isolated OS reads of Disabled or explicit Unavailable/NotFound. RequiresApproval/Enabled/Failed or a generic unavailable answer cannot stand in for those observations. All four processes must exit 0 with empty stderr, and the signed binary must remain unchanged. / 接受条件包括四次真实进程、身份拒绝与精确 OS 状态，不能用泛化不可用掩盖错误。

## Real WebView read path / 真实 WebView 读取通路

```sh
python3 qa/phase11_nsh_credentials.py \
  --exe target/debug/aleph-nsh-shell \
  --receipt .phase10-evidence/phase11-status-bridge-new/receipt.json \
  --observe-autostart
```

The opt-in macOS extension requires actual `autostartStatus()` replies bound to both fixture documents, each Unavailable/NotAppBundle for this deliberately unbundled executable. It also retains the original ten credential effects, isolated store cleanup, native return and process exit requirements. The default credential-only evaluator is unchanged; no historical receipt is rewritten. / 扩展模式同时验证两个文档的真实宿主回复与原凭据效果；默认历史评估器保持不变。

```sh
(cd qa && python3 -m unittest test_phase11_nsh_autostart test_phase11_nsh_credentials -v)
node --test desktop/nsh-shell/tests/panel_bridge.test.mjs
```

The receipt evaluator tests are not native evidence. Node tests cover closed state shapes, approval-state preservation, readiness, separate bounded credential/status admission, timeouts, pagehide and wrong-document/source/origin/request refusal. / 单测不冒充原生证据，JS 测试覆盖固定契约、期限与文档边界。

## Limits / 限界

No production login-item read/write, real enabled/disabled/approval transition, production Panel WASM rendering, Windows/Linux native result, updater/installer, performance, default switch, rollback or 14-day dual-operation claim is made. The existing Tauri writable toggle is retained. / 未验证以上生产与跨平台行为，保留旧壳写端。

Current source/native identities and scoped observations are recorded in [the machine receipt](../docs/superpowers/runs/2026-11-22-phase11-autostart-receipt.json). / 当前源码和原生身份见机器回执。
