# Phase11 NSH Cutover Contract / Phase11 NSH 切换契约

**Status / 状态:** implemented foundation slices, NOT P11-A–H acceptance / 基础切片已实施，不是全期验收
**Version / 版本:** `2026-11-21.2`
**Scope / 范围:** official platform entry selection and fixed credential transport / 官方平台入口选择与固定凭据传输

## 1. Authority and non-goals / 权威与不做事项

The binding authority is [the approved Phase11 plan](../plans/2026-11-21-phase11-nsh-cutover.md) and [R2 choice A](../runs/2026-11-16-phase9-nsh-r2-boundary.md). This contract does not waive any gate.

- Leptos/WASM Panel remains the sole business UI. `interfaces/webchat` stays; no native Chat rewrite, second Gateway client, permission system, projection, registry, or arbitrary invoke tunnel.
- Core remains platform-independent. Host code is native I/O and lifecycle glue using existing protocol/client owners.
- Tauri remains the active default and rollback path. No default/release switch or deletion is allowed before the approved gates; the common dual-track interval starts only after the last of three platforms qualifies and requires at least 14 real calendar days.
- No remote push/tag/release, signing-key change, destructive credential migration, toolkit patch, or old fork is part of this delivery.

本次是可验证的基础增量，不是以“计划完成”代替产品实施，也不是提前把入口可编译当成跨平台替代完成。

## 2. Source ownership and callsite inventory / 所有权与调用点

The versioned [inventory](../runs/2026-11-21-phase11-nsh-inventory.json) lists concrete registered commands, consumers, source owners, destinations and unresolved checks. Line numbers are discovery anchors, not a generated ABI.

| Surface / 能力 | Existing authority / 现有权威 | Current result / 当前结果 |
| --- | --- | --- |
| Window, tray, hotkey, URL intent, notification | `desktop/shell/src/{main.rs,tray.rs,hotkey.rs,deeplink.rs,notify.rs,menu.rs}`; existing NSH Phase10 APIs | Same NSH Panel host selected for three targets; only historical macOS scope accepted. New native platform regression still required. |
| Panel document/request fence | `shared/protocol/src/panel_shell.rs`, `desktop/nsh-shell/src/panel_bridge.{rs,js}`, `interfaces/webchat/src/app/shell_bridge.rs` | Existing Probe/OpenChat/Notify behavior retained; typed credential messages added, not an invoke compatibility layer. |
| Credential storage and DTOs | `shared/client` `DeviceCredentialStore`; `shared/protocol/src/native_credentials.rs` | NSH adapter calls the existing store; no new persistent store or identity authority. Tauri implementation remains active. |
| Native handshake and attempt fence | `interfaces/webchat/src/api/native_credentials.rs`, `interfaces/webchat/src/context.rs` | `NativeIpc` explicitly selects NSH or the existing Tauri transport; lease/attempt and Gateway identity rules unchanged. |
| Target, full/lite, discovery, certificate trust | `desktop/shell/src/{connection.rs,connect_setup.rs,gateway_probe.rs,cert_trust/}` | Not migrated. NSH still has an immutable admitted local endpoint, not production remote/full/lite support. |
| Daemon startup/recovery/supervision | `desktop/shell/src/{main.rs,daemon.rs}` and existing server service commands | Not migrated. Do not introduce a competing supervisor or infer readiness from a TCP-only check. |
| Autostart, updater, package/release | `desktop/shell/src/update.rs`, Tauri plugin/configs, `justfile`, `.github/workflows/aleph-app-release.yml` | Not migrated; all active rollback/build paths remain. |
| Directory/project filesystem actions | `interfaces/webchat/src/api/fs.rs` and Gateway JSON-RPC | Not native-shell IPC; retain existing Gateway authority. |
| Native styling, microphone and platform markers | `interfaces/webchat/src/platform_host.rs`, voice `audio.rs`, Panel CSS/baseline | Tauri-specific assumptions remain; audit and native parity are open. A new `aleph-nsh` marker does not itself migrate these consumers. |

## 3. Official platform feasibility / 官方平台可行性

`desktop/nsh-shell/Cargo.toml` and `Cargo.lock` now select official crates.io `nsh-platform-{macos,linux,windows} =0.6.0`. `src/platform.rs` reexports the selected backend's `AppOptions`, `AppSource`, `Root`, `run_app`. Common host code uses shared NSH `quit`, not a macOS-only quit call. `keep_alive_on_close` and the historical phase9 native clock remain macOS-gated because the other platform option structs/clock do not implement that contract.

The research checkout `../nsh-phase10` at `644a31f0cd1cc35855b6241aedec90a2eda9f222` has local macOS experiments. It was inspected, not edited or linked. Same-version index/lock pins are the dependency identity; checkout/API inspection is not native acceptance.

- Linux: `webview` feature and GTK4/WebKitGTK 6 are required. The offline target check is blocked by uncached `cairo-rs v0.22.11`; X11/Wayland native runs are NOT TESTED.
- Windows: a real Windows/WebView2 runner is required. No Windows target is installed here; native compile/launch is NOT TESTED.
- macOS: host tests and a unique-namespace OS-keyring roundtrip passed; the changed host's WebView/native IPC and full desktop regression remain NOT TESTED.

取舍：一个 cfg facade 复用 Panel 宿主，避免三个业务 UI；代价是显式承受各平台原生依赖与行为差异，不能凭共同 API 签名宣布平台对等。

## 4. Fixed credential boundary / 固定凭据边界

1. `HostSettings::from_inputs` still admits only default loopback or paired debug loopback endpoint + 64-lowercase-hex nonce. No production remote target was added.
2. The wire has only `CredentialCall::{Load {}, Save { lease_id, credential }, Delete { lease_id }}` and closed `CredentialReply`/`CredentialError` enums. `deny_unknown_fields` and canonical DTOs reject extra keys; credential Debug output remains redacted. No `String command + Value args` crosses the Rust host wire.
3. The JavaScript adapter exposes only the existing three fixed credential commands. It does not create `__TAURI_INTERNALS__`. Top-frame/exact-origin installation, random document ID, native source/origin checks, request matching, one pending request, bounded messages, 15-second timeout and `pagehide` rejection are enforced.
4. Calls wait for document-specific `CredentialsReady` after the existing probe. Save ACK is explicitly `null`; nullable credential payloads do not erase the ACK/outcome distinction.
5. `NativeIpc` preserves `BridgeIpc`, strict response decoding, endpoint match, attempt receipts, no-localStorage fallback and Gateway-owned identity. A present but malformed NSH bridge must not fall back to Tauri.
6. `CredentialService` captures the admitted immutable Gateway URL, owns one lease gate and one worker permit, and calls canonical `DeviceCredentialStore`. A named OS thread owns actual blocking work; no entered Tokio runtime is assumed. Overlap rejects rather than queues.
7. Navigation increments a lock-free generation without blocking the UI on OS I/O. Worker admission and result delivery check generation; the host additionally checks document and loading state. A synchronous OS effect already admitted cannot be undone by navigation or timeout; its late result and lease are rejected. Future target mutation must join this authority boundary before being enabled.
8. Debug QA endpoints never select the production keyring namespace: without explicit `qa-service`, credential operations fail closed. With debug `qa-service`, the canonical client's nonce-derived isolated namespace is used. An opt-in ignored test writes and cleans only a random QA namespace.

限制：Node VM tests model the JavaScript browser seams; Rust memory-store tests model the external store. The real macOS isolated-store test proves OS persistence/deletion and revoked-lease effects, not WebView→host IPC, full pairing, or platform UI parity. A hung OS-keyring call retains the one permit until its blocking body exits; it is not forcibly cancelled.

## 5. SPEC → QUALITY matrix / 规格到质量矩阵

| ID | Predicate / 判据 | Current evidence and limit / 证据及限界 |
| --- | --- | --- |
| Q1 | Official `=0.6.0`, no fork or toolkit patch | Manifest/lock audit: 33 package identities added, zero removed; Linux/Windows registry pins present. |
| Q2 | Three real platform cfgs, no macOS-only failure stub | Source/API review and macOS compilation pass; Linux cache-blocked, Windows native unavailable. NOT platform acceptance. |
| Q3 | Closed credential wire and preserved Panel intent boundary | Protocol 415 unit + 11 integration tests pass; Node bridge 8 pass. Native WebView delivery not tested. |
| Q4 | No second business UI/store/client/permission authority | Source review: existing Panel, client DTO/store and Gateway handshake retained. Full capability regression not rerun. |
| Q5 | Immutable local endpoint; stale/overlap/timeout fail closed | NSH 11 unit + 2 integration tests pass; feature build 11 pass + 1 opt-in ignored; explicit real OS test separately passed. |
| Q6 | Tauri full/lite rollback survives | No source/config/build path deleted; no package identities removed. Tauri full/lite build/regression NOT rerun. |
| Q7 | Native platform/effect evidence is not inferred from API or test counts | OS-store roundtrip is separately labeled; three-platform launches/performance/packaging remain open. |
| Q8 | No release/default/signing/credential migration | Diff and local Git inspection; only isolated QA credentials were created/deleted. No publish/tag/push. |
| Q9 | Existing Panel attempt fence stays effective | 24 native credential unit tests pass; WASM check passes. Plain native tests do not exercise the WASM selector. |

The original contract preceded the first platform slice. Its inventory was only module-level; the credential exploratory draft preceded complete callsite review. That ordering deviation is recorded, not retroactively described as SPEC→QUALITY approval. Subsequent self SPEC and self QUALITY findings/checks are recorded in the inventory. No independent reviewer was dispatched and no independent approval is claimed. P11-A remains OPEN pending complete per-caller dispositions and the required ordered review.

原任务列表曾过早把基础摸底标为完成；这不构成 P11-A 通过。新增的 P11-A review task 保持进行中；其余迁移/平台/双轨/删除闸门不自动推进。

## 6. Updater trust boundary / 更新信任边界

Same-version source inspection found a format/contract mismatch, not a drop-in updater:

- `tauri-plugin-updater-2.14.0/src/updater.rs:1893-1916` decodes a minisign public key/signature and verifies the artifact, then checks signed-version policy. `desktop/shell/src/update.rs:323-378` owns the existing check/download/install/restart path. `desktop/shell/tauri.conf.json` supplies the current public trust anchor; full/lite use their existing endpoints. No key value is reproduced here.
- `nsh-updater-0.6.0/src/lib.rs:522-545` expects base64 raw 32-byte Ed25519 public keys and a signature over context-framed manifest bytes. Its native installer is a separate contract. Reusing its API does not preserve the existing minisign update feed automatically.
- The current Aleph updater config does not explicitly set `requireSignedVersion`; this observation is not proof that all historical artifacts carry signed version metadata, and this slice does not change policy.

**Recommendation / 建议:** preserve the existing minisign trust anchor/feed and verification semantics in a narrowly scoped host updater adapter, with invalid-signature/wrong-version/rollback/package-manager tests before enablement. An alternative is the official NSH signed-manifest feed, but it needs an explicit trust/feed migration decision and new verifiable signing/rollback receipts. Neither route is implemented or approved by this foundation slice. Do not silently generate a new key, reinterpret the old key as an NSH key, enable unsigned updates, or run an installer.

这一缺口不阻止独立的入口/固定凭据测试，但阻止声称 updater 已迁移或执行默认切换；具体安装/重启/daemon 生命周期还需下一阶段规格冻结。

## 7. Open gates and next executable slice / 未完成与下一切片

Finish the P11-A consumer/ownership dispositions and review first; next transfer target/connection/service ownership using existing types and security gates, then platform OS integrations and updater/packaging. NSH current local bootstrap is not a replacement for production full/lite/remote products.

P11-B–H remain open. No default switch, rollback rehearsal, common dual-track start or Tauri deletion has occurred. Phase10 original matrix remains `VALID / NOT_ACCEPTED`; only the separate Quit median/p95 `1.30x` amendment accepted its retained observations. Physical multi-display remains deferred/NOT TESTED, and a notification feature-disable/zero-resource contract remains undefined.
