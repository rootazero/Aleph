# Phase11 isolated NSH credential IPC / 隔离原生凭据通路

This opt-in fixture runs the **real debug NSH host**, its injected bridge, typed Rust handler and canonical OS credential store. Two fixture documents replace the production Panel only for the test; no mock credential result is supplied. / 此测试使用真实宿主与 OS store，但测试页不是生产 Panel。

## Build and run / 构建与运行

From the repository root, keep builds serial and separate from native QA:

```sh
python3 qa/browser_live/check_memory.py --required-bytes 4294967296
# Only after ALLOW:
cargo build -p aleph-nsh-shell --features qa-service --offline --locked --jobs 1

# Use a NEW evidence path; existing receipts/profiles are never overwritten.
python3 qa/phase11_nsh_credentials.py \
  --exe target/debug/aleph-nsh-shell \
  --receipt .phase10-evidence/phase11-native-credentials-example/receipt.json

(cd qa && python3 -m unittest test_phase11_nsh_credentials -v)
```

Adjust the executable path for a configured target directory/Windows suffix. Windows/Linux execution is not proven by the macOS result. / 其他平台必须在真机分别运行。

The runner admits only a random `127.0.0.1` port, a random nonce-derived QA keyring service and a new WebView data directory. It does not alter HOME, default credentials, installed services, startup registrations, update trust or release paths. The summon hotkey is disabled for this fixture. The host still runs its ordinary window/tray/notification initialization; no zero-resource feature-disable claim is made.

## Required observations / 必须观察的效果

1. The initially empty store loads through actual IPC.
2. Save ACK is exactly null and a subsequent load reads the synthetic credential back.
3. Superseded lease deletion rejects and a subsequent load proves the value still exists.
4. After actual navigation, the prior document's lease deletion rejects and the value remains.
5. A wrong-document raw request has no deletion effect.
6. Current-lease deletion reports existence; the final load is empty.
7. Exactly two distinct document identities and ten ordered successful effect checks are observed.
8. The host receives close-requested, returns from the native engine, emits owned-drained and exits successfully. Timeout/kill is never a successful exit observation.

The evaluator unit tests reject incomplete, duplicate, reordered, non-boolean, one-document, failed-cleanup and missing-shutdown observations. Tests never substitute for a real host invocation. The opt-in macOS `--observe-autostart` extension additionally requires actual status replies on both documents; its [read-only status QA guide](phase11_nsh_autostart.md) documents scope and refusal controls. The original credential-only evaluator and default acceptance remain unchanged. / 可选状态扩展不改变原凭据评估器或历史回执。

## Limits and failure / 限界与失败

The receipt is **not** production Panel WASM/attempt-selector/pairing proof; neither is it a Gateway/service test, platform parity, signing/updater/installer proof, performance sample, default switch or dual-track activity. Ordinary host return is not a global census of every OS helper process.

On fixture assertion failure, the page attempts load/delete/read-empty cleanup in its isolated service before requesting host shutdown. On a hung/crashed host or OS-keyring call, cleanup may remain unknown; the receipt fails and does not claim otherwise. The runner retains its profile/evidence for diagnosis. It never cleans arbitrary user namespaces to conceal a failed run. / 失败时保留真实未完成状态，不为变绿触碰生产凭据。

The 2026-11-22 macOS run and exact source/binary identity are recorded in [`2026-11-22-phase11-native-credentials.json`](../docs/superpowers/runs/2026-11-22-phase11-native-credentials.json).
