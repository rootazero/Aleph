# S1-C Web v1 verification / Web v1 验证回执

## Scope / 范围

Native Gateway TLS mounts [`POST /v1/session` and `GET /v1/ws`](../../../src/gateway/server/connection/web_v1.rs). Plaintext and wrapper routers do not mount them. Present one active Panel device token or one one-time ticket explicitly bound to an active Core user. The exchange yields an empty `204` and a five-minute `__Host-aleph-v1` opaque Cookie, with `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`, and no `Domain`. The Cookie is not credential material for legacy `/ws` and does not change the Core principal.

显式绑定票据在一次 SecurityStore 写事务里验证活跃用户、标记消费、创建设备并发证；写入失败时整组回滚。旧配对与 `/ws` 首帧 `connect` 保留原语义。Web v1 在升级前和分发前复核原 token、设备及活跃用户，空闲连接每秒复核。到期后必须重新提交有效显式凭据，不会自动续期。Cookie 不按端口隔离，也不绑定浏览器实例：部署时需独用可信主机名，不与同主机名下不可信的 HTTPS 服务共用。

The `aleph.protocol.v1` subprotocol accepts one length-prefixed UTF-8 payload per binary WebSocket message, at most 1 MiB. After `system.hello`, `health` and `sessions.list` go through the existing Gateway executor, one request at a time. Unsupported methods have their own rate bucket and cannot consume the legacy Auth bucket. The Web v1 listener limits admission to eight sockets and the exchange holds at most 256 live handles. A Web v1 Upgrade reserves a shared connection row before `101`, including while waiting for hello. This is **not** an atomic mixed-transport cap: legacy `/ws` retains its two-stage check and registration. No snapshot, replay, binary channel, or automatic retry is advertised.

## Evidence / 验证证据

Verified code commit `a48aec7cb6b0632250182aee488ad0d6f757475d`, source tree `1da6288f66b15bd592c4b7fa8abdfa29db85799f`; implementation plan [S1-C Web v1](../plans/2026-11-23-nsh-panel-s1c-web-v1.md). macOS 27.2.1 arm64, Rust/Cargo 1.99.1, libprotoc 36.2, Chrome 154.0.8037.93, Node 26.13.0. Tests were serial, `--locked`, and host-memory gated, with temporary `ALEPH_HOME` and fixture-generated credentials and sessions. The pre-existing macOS compact-unwind linker warning did not prevent the test binary from running.

- **Mounted exchange and binding:** 3 passed. Prior to implementation the mounted exchange returned `405` instead of `204`. The final fixture proves single-use explicit pairing, exact credential shape, TLS mount selection, and exact token/device/user binding, including expiry and revocation. The security suite separately exercises actual atomic rollback at token issuance.
- **Real Chrome via native TLS:** 1 passed, run explicitly. Chromium used two ephemeral `127.0.0.1` HTTPS ports and a generated certificate with public-key pinning scoped to the probe. The [browser driver](../../../qa/browser_live/web-v1.cjs) verified Cookie attributes and script invisibility, pre-hello denial, actual `health`, the original member's `sessions.list` visibility, unsupported-method denial and legacy `connect`/`health`. The [mounted fixture](../../../src/gateway/server/connection/web_v1/tests.rs) observed `503` while pre-hello sockets filled the configured cap, and `403` when a cross-port browser handshake offered the exact issued Cookie. The observer does not log or authorize credentials. Revoking the token released the idle connection's shared row before any subsequent RPC and refused another upgrade. The TLS fixture shut down and left no connection rows.
- **Gateway server:** 240 passed. The Chrome fixture is ignored in the broad suite and was run separately as above.
- **Gateway security:** 103 passed, including `bound_exchange_rolls_back_ticket_and_device_if_issuance_fails`. A fixture-induced token write failure proves that the ticket remains redeemable and that no device row was left behind.
- **Legacy ingress integration:** `gateway_ui_ingress` passed (1 test). The serial verification exited `0`; Rust formatting, browser-script syntax, and `git diff --check` also passed.

Initial read-only review `d8f430c8-f5dd-4851-b25c-81877bcbb2cf` blocked on incomplete browser evidence, shared capacity and rate isolation, non-atomic ticket exchange, idle revocation, and credential shape. Follow-up `b256e08a-0191-4141-8bd4-609e5c39fb4c` closed those findings and noted pre-hello close ordering. Final delta review `4f908579-878f-4d0f-91b7-dc29723e47f7` confirmed that normal pre-hello close releases the row first, with asynchronous cleanup as fallback. No Critical or Important finding remained in the reviewed tree.

## Replay / 复跑

Use an isolated checkout with an existing workspace Playwright installation and Chrome/Chromium. No installed service or production credentials are needed.

```bash
export ALEPH_HOME="$(mktemp -d)"
export ALEPH_BROWSER_LIVE_CHROME_PATH="/path/to/Chrome"
python qa/browser_live/check_memory.py && cargo test --locked -p alephcore --lib gateway::server::connection::web_v1::tests::web_socket_requires_hello_and_keeps_member_scope -- --ignored --nocapture
python qa/browser_live/check_memory.py && cargo test --locked -p alephcore --lib gateway::server::
python qa/browser_live/check_memory.py && cargo test --locked -p alephcore --lib gateway::security::
python qa/browser_live/check_memory.py && cargo test --locked -p alephcore --test gateway_ui_ingress
```

## Not delivered / 未交付

现有 NSH WebView、Leptos/WASM Panel 和 Tauri 不换线；未部署、未切默认、未发布。未交付共享 TSX、自定义呈现迁移及 S4 持久快照与补读；未验证代理终止 TLS、公开证书部署或其他浏览器和平台。Cookie 到期在绑定层验证，没有在 Chromium 等五分钟。S1-B Linux 真机验证与 Windows Named Pipe 仍未完成；不把 S1-C 证据算作桌面平台验收。
