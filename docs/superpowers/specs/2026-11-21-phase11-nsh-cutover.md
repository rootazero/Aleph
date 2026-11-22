# Phase11 NSH Cutover Contract / Phase11 NSH 切换契约

**Status / 状态:** implemented foundation slices, NOT P11-A–H acceptance / 基础切片已实施，不是全期验收
**Version / 版本:** `2026-11-22.7`
**Scope / 范围:** official platform entry selection, fixed credential transport and immutable launch targets / 官方平台入口选择、固定凭据传输与不可变启动目标

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
- macOS: host tests, a unique-namespace OS-keyring roundtrip, and the changed host's real WebView → typed IPC → isolated OS-store fixture passed. This is not production Panel WASM, pairing or full desktop regression; those remain NOT TESTED.

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

限制：Node VM tests model JavaScript browser seams; Rust memory-store tests model the external store. The separate real macOS fixture (`qa/phase11_nsh_credentials.py` / `.html`) now proves the existing WebView bridge reaches the canonical isolated OS store: ten effect checks across two actual document identities, superseded/navigation lease rejection, wrong-document no-effect, deletion/absence and natural host return. It is not production Panel WASM, full pairing or platform parity. A hung OS-keyring call retains the one permit until its blocking body exits; it is not forcibly cancelled. Fixture failures/timeouts never count as natural exit or guaranteed isolated-store cleanup.

## 5. SPEC → QUALITY matrix / 规格到质量矩阵

| ID | Predicate / 判据 | Current evidence and limit / 证据及限界 |
| --- | --- | --- |
| Q1 | Official `=0.6.0`, no fork or toolkit patch | Manifest/lock audit: 33 package identities added, zero removed; Linux/Windows registry pins present. |
| Q2 | Three real platform cfgs, no macOS-only failure stub | Source/API review and macOS compilation pass; Linux cache-blocked, Windows native unavailable. NOT platform acceptance. |
| Q3 | Closed credential wire and preserved Panel intent boundary | Protocol 415 unit + 11 integration tests pass; Node bridge 8 pass. Separate real macOS fixture passes ten effects across two documents; production Panel WASM delivery remains open. |
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

**Decision / 已确认:** the user selected preserving the existing minisign trust anchor/feed and verification semantics in a narrowly scoped host updater adapter. Invalid-signature/wrong-version/rollback/package-manager tests precede enablement. This does not authorize new keys, switching to the official NSH signed-manifest feed, publishing or running an installer. The adapter is not implemented. Never reinterpret the old public key as an NSH raw Ed25519 key or enable unsigned updates.

这一缺口不阻止独立的入口/固定凭据测试，但阻止声称 updater 已迁移或执行默认切换；具体安装/重启/daemon 生命周期还需下一阶段规格冻结。

## 7. Open gates and next executable slice / 未完成与下一切片

Finish the P11-A consumer/ownership dispositions and review first; next transfer target/connection/service ownership using existing types and security gates, then platform OS integrations and updater/packaging. NSH current local bootstrap is not a replacement for production full/lite/remote products.

P11-B–H remain open. No default switch, rollback rehearsal, common dual-track start or Tauri deletion has occurred. Phase10 original matrix remains `VALID / NOT_ACCEPTED`; only the separate Quit median/p95 `1.30x` amendment accepted its retained observations. Physical multi-display remains deferred/NOT TESTED, and a notification feature-disable/zero-resource contract remains undefined.

## 8. Production ownership freeze / 生产所有权约束

These are implementation requirements, not claims that the handlers or platform packages already exist. Missing official platform proof still blocks P11-A acceptance; the existing local credential boundary can be tested independently. / 以下是待实施约束，不是已完成声明。

| Product / 产品 | Startup and ownership / 启动与所有权 | Failure and exit / 失败与退出 |
| --- | --- | --- |
| Full + local | Resolve the packaged server first; reuse existing readiness/launch operations. A healthy existing Gateway is adopted, not replaced merely because the shell version changed. A booting `/ready` 503 is waited on; a foreign listener fails closed. | Close hides; ordinary Quit leaves the server alive. Only a distinct explicit Quit-and-stop action may request the existing server stop command. No automatic legacy-service removal during overlap. |
| Lite + local | No bundled daemon acquisition, takeover, stop or recovery loop. Use the selected existing server and expose unavailable state in shared Panel setup. | Failure remains actionable; never silently turn lite into full. Ordinary Quit affects only the host. |
| Either + remote | Preserve canonical HTTP(S) target parsing/default ports, persisted-state compatibility, TLS/certificate approval and Gateway identity rules. Remote targets never trigger local launch/stop or import local credentials. | On denial/unreachable/failed persistence retain the prior committed target. No navigation or success event before commit. No raw rejected URL or credentials in errors. |
| Standalone server | Existing `aleph-server` service/CLI remains the owner; distribution and platform helpers survive desktop replacement. | No new service registry or desktop business logic inside Core. |

**Source-backed service distinction / 服务区分:** `src/bin/aleph-server/commands/service/mod.rs::handle_service_command` dispatches Install/Uninstall/Enable/Disable/Status; it is not a start/stop API. Per-user launchd/systemd-user/Task Scheduler own server login-start behavior. Explicit server stop is the top-level CLI command. `desktop/shell/src/daemon.rs::reconcile_for_version` currently removes legacy autostart and stops a running server; copying it into NSH would violate healthy-service preservation during dual operation, so it is not the new startup path.

**Target transaction / 目标事务:** preserve `connection::serialize_target_switch` and `keyring_bridge::commit_target_change` semantics when transferring ownership. Parse/probe and explicit certificate decisions precede mutation. Target persistence, credential association and lease invalidation share the owning boundary; only then emit/reconnect/navigate. No NSH mutable-target feature may bypass that boundary. Canonical parser extraction may be shared, but a second persistent target store or second credential owner is forbidden.

**Autostart / 自启动:** current Panel `plugin:autostart` controls desktop-host login startup, not `aleph-server service enable`. Preserve this distinction. Read existing host startup state before proposing an OS-specific replacement; do not install/remove login items or change system defaults as a build/test side effect. Three-platform host autostart and single-instance activation implementations remain open, not mapped to unrelated server controls.

**Updater / 更新:** preserve the approved minisign trust/feed, current full/lite separation, managed/package-manager behavior, version checks, one apply at a time and recoverable installation. Verification must precede irreversible replacement. Restart must obey the same target/service ownership rules; failed verify/download/install must not report success. Actual platform installation/rollback and release artifacts still require tests; no generic `invoke` or official NSH manifest switch fills this gap.

**Single instance and URL / 单实例与链接:** a second launch forwards a bounded validated intent to the existing host; it never acquires a second service owner. Retain exact `aleph://open` matching, show/focus before Panel intent and document/request confirmation. Window close, app Quit, URL activation and explicit server stop remain separate actions. Actual Windows/Linux registration/activation and multi-display are untested.

### Ordered contract self-review / 有序契约自审

1. **SPEC:** checked the above products against Task3/5, current server dispatcher and old daemon/target owners. Resolved the updater route; explicitly excluded automatic service takeover and host/server-autostart conflation. The inventory names proposed boundaries rather than claiming missing handlers exist.
2. **QUALITY (distinct pass after SPEC):** checked effect/no-op and negative directions: failed target commit cannot navigate, rejected stale delete must leave stored value unchanged, Quit cannot imply server stop, missing native platform cannot imply support, and updater verification cannot imply installation. The new fixture observes actual store effects and native return; its six reducer tests reject missing/duplicate/reordered/false observations and absent shutdown acknowledgement. Full production Panel, service processes, signed installer rollback and native Windows/Linux remain unverified.

These are author self-reviews, not independent approval. P11-A remains OPEN because platform experiments and remaining concrete handler/OS feasibility dispositions are incomplete. / 自审不冒充独立批准，闸门仍未关闭。

## 9. Immutable launch target slice / 单进程不可变连接目标切片

This slice is a production launch entry, not full/lite/service or mutable-target acceptance. / 本切片只交付生产启动参数，不冒充产品与服务迁移完成。

- **Caller and authority:** process arguments `--connect <origin>` select one target for the lifetime of this NSH process. `aleph-client::ConnectionTarget` owns parsing, defaults, origin projection and canonical credential transport admission. The old Tauri module reexports the same value type; its persistence/credential transaction gate remains unchanged.
- **Admission:** accept a bare host/host:port or root HTTP(S) origin. Reject userinfo, query/fragment bootstrap credentials and non-root paths without echoing input. Reject plaintext non-loopback credential transport using the existing client policy. Preserve default `http://127.0.0.1:18790/` when no argument is supplied. `--help` is side-effect-free. Unknown/missing/duplicate arguments fail before creating a window or opening a store.
- **QA separation:** production arguments and QA endpoint/nonce are mutually exclusive. Existing debug QA isolation remains mandatory; a production argument cannot redirect the nonce store or make a fixture use the production namespace. Non-Unicode process arguments or QA environment values fail closed without echoing their bytes; `--help` returns before reading configuration.
- **Mounted handler:** `HostSettings` derives Panel root and WS/WSS credential endpoint once; `main` mounts that exact root and passes the admitted endpoint to the existing `CredentialService`. The bootstrap binds to that root's origin. No new arbitrary invoke, TLS exception, certificate approval or native transport client is added.
- **Effects and lifecycle:** this slice writes no target marker, changes no service/autostart state and never launches/stops a local daemon. A remote launch has no local service acquisition path. Existing OS-store operations retain document/generation/lease/attempt checks. Runtime target change, persisted compatibility, pairing-ticket handoff, setup UI and supervision remain OPEN.
- **Verification:** RED/GREEN shared URL/transport tests; argument admission and secret-free error tests; retained Tauri connection transactions; both NSH feature modes; WASM build; real isolated native credential fixture. HTTPS remote TLS/production pairing and native Windows/Linux are NOT TESTED.

**SPEC pass (before boundary implementation):** checked immutable selection against the existing target transaction owner; deliberately no mutable write or legacy ticket migration. Existing credential transport policy is reused rather than reimplemented. **QUALITY pass (after SPEC):** checked both directions of QA isolation, rejected argument no-effect, sanitized origin projection and WSS mapping; native readiness/service/recovery cannot be inferred from a parsed target. These are separate author passes, not independent approval. / 本节为实施前两次自审，不改变 P11-A–H 状态。

### Verified launch checkpoint / 已验证启动切片

The [machine receipt](../runs/2026-11-22-phase11-launch-receipt.json) records exact source/native identities and limitations. / 机器回执记录源码、原生身份及验证边界。

- Shared client: 57 tests passed. Retained Tauri full: 167 tests passed; lite with isolated QA feature: 190 passed. These are Rust tests, not installed-product lifecycle proofs. / 保留旧壳单测，不等于安装产品验收。
- NSH QA feature: 13 unit, five real-process argument tests and two window contract tests passed; one real-store test ignored by default. Malformed QA environment first failed a safe RED case, then passed after removing silent `var(...).ok()` downgrade. / 非 Unicode QA 配置先 RED 后修正，拒绝降级到生产命名空间。
- Current binary passed the existing real macOS WebView credential fixture: ten effects, two actual documents, isolated final absence and native return. Raw receipt: `.phase10-evidence/phase11-launch-credentials.EU3xi4/receipt.json`. This revalidates the changed endpoint wiring, not HTTPS remote production pairing. / 真实隔离回归覆盖新端点接线，不代表远程生产配对。
- Strict client/NSH clippy and Panel WASM check passed; WASM retains three existing dead-code warnings. Default switch, daemon takeover, performance rerun and Tauri removal were not performed. / 未切默认、未接管服务、未重跑性能、未删 Tauri。

## 10. Pinned host API gaps / 固定版本宿主 API 缺口

The exact 0.6.0 source, not current website HEAD, defines these limits. / 以下以锁定版本源码为准。

- `nsh-core-0.6.0/src/element/desktop.rs:1082` exposes `launch_at_login() -> impl Future<Output = bool> + 'static`; it returns false for disabled, approval pending, unsupported contexts and read errors. `src/element/desktop.rs:1101` exposes fallible writes, but no public detailed status getter was located. `src/desktop/login_item.rs` has richer backend statuses; importing a type is not access to the installed backend. Do not expose false as a confirmed Disabled status. / 布尔读取丢失状态，不能据此宣称已禁用。
- `nsh-core-0.6.0/src/single_instance.rs` assigns platform uniqueness/forwarding to release runners. A Rust-hosted `run_app` plus an event listener does not prove duplicate-process exclusion or intent forwarding. / Rust 宿主的单实例仍需真实证明。
- The user approved alternative (A), a narrow same-identity OS status adapter retaining official 0.6.0; section 11 implements its read-only macOS path. An official API upgrade remains unselected. No toolkit patch, fork, global service change or production login-item mutation was made. / 用户已批准同身份窄只读适配器，不升级、不修改生产登录项。
- Windows/Linux native runners and Linux's cached GTK dependencies remain absent. No cross-compilation/source review result counts as their native acceptance. / 平台机器与离线依赖缺口不冒充验收。

## 11. Read-only login-item adapter / 只读登录项适配器

The user approved a narrow OS adapter, retaining official NSH 0.6.0. / 用户批准窄 OS 适配器路线，不改官方版本。

- **First caller:** `aleph-nsh-shell --autostart-status`, returning one closed JSON status without opening a WebView. This diagnostic is not the Panel toggle migration. The fixed command rejects extra arguments and QA URL mixing. / 首个真实调用方为只读诊断命令，不冒充 Panel 开关完成。
- **Authority:** on macOS read `SMAppService.mainAppService.status`, the same source as the pinned NSH backend. First verify the current executable belongs to an `.app` bundle with the expected application identifier. Share `APP_ID` with window startup. An unbundled binary or identity mismatch is Unavailable, never Disabled. / 与官方写端使用相同 OS 所有者，先核对宿主身份。
- **Status contract:** preserve Enabled, Disabled and RequiresApproval; represent unsupported contexts, missing API and NotFound as explicit Unavailable reasons. Unknown future OS values/read failures are Failed, never an enabled/disabled answer. / 未知与失败不降级成布尔值。
- **QA identity:** only debug `qa-service` builds may read a random nonce-derived `com.aleph.nsh-shell.qa.<nonce>` bundle through the existing QA nonce environment. No arbitrary bundle query or production namespace fallback. / 隔离 QA 身份不允许任意查询或生产回退。
- **Lifecycle/effects:** synchronous bounded local status read before native startup; no worker, persistent registry, setter, launchd/systemd/registry/task-scheduler mutation or server-service command. No registration/unregistration is called. / 只读，不修改系统登录项或服务器服务。
- **Platform limits:** current exact research sources did not locate a Linux/Windows login-item backend. Return UnsupportedPlatform until a concrete same-identity OS adapter and native effects are validated; do not invent NSH registration paths. / 其他平台显式不可用，不能编造注册位置。
- **Verification:** RED/GREEN for all status mappings, identity and QA admission; real process CLI tests; real nonce-isolated ad-hoc signed macOS bundle read; wrong-identity/unbundled no-effect controls. No production login-item writes. / 验证映射、身份、真实隔离包及拒绝方向。

**Panel read path / Panel 只读接入:** fixed `autostart-request {request, document}` and `autostart-reply {request, document, status}`; no app id, enabled flag, server target or arbitrary invoke in the call. Reuse launch prefix, probe-ready, document/generation fences and 15-second JS timeout. A separate one-slot status request avoids interfering with credential admission. One host-owned worker permit bounds OS reads across navigation. The Panel displays the exact status, never an unchecked checkbox for unknown/failed state; NSH writes remain unavailable. Existing Tauri toggle remains on its original path. / 固定只读类型，状态读取不抢凭据槽；未知态不画成关闭，写端不冒充完成。

**SPEC author pass:** verified the current OS owner, same application identity and fixed document-bound read; no setter or arbitrary target is admitted. **QUALITY author pass after SPEC:** checked unknown/not-found/pending, invalid QA, early readiness, separate bounded requests and late-navigation replies. Neither pass is independent review. Production Panel rendering, setters and Windows/Linux native acceptance remain OPEN. / 两次有序自审不等于独立审阅或全期闸门通过。

### Verified read-only checkpoint / 已验证只读切片

The [machine receipt](../runs/2026-11-22-phase11-autostart-receipt.json) binds current source and native identities. / 机器回执绑定当前源码与原生身份。

- Real macOS WebView status reads passed on two actual document identities, alongside all ten isolated credential effects, final store absence, native return and process exit 0. Both unbundled reads explicitly returned Unavailable/NotAppBundle, not Disabled. / 两个真实文档均收到宿主不可用原因，不冒充关闭。
- An independently invoked nonce-isolated ad-hoc signed `.app` read the actual OS status twice as Unavailable/NotFound. Wrong-identity and unbundled refusal controls passed. No login-item registration/unregistration, production identity read, service operation or default change was performed. / 隔离包真实查询及身份拒绝验证通过，未改生产系统状态。
- Protocol 416 unit + 11 integration; NSH default and QA each 19 unit + seven real-process launch + two window tests; Node bridge 11; top-level Python QA 88 passed. Strict NSH clippy and Panel WASM check passed; WASM keeps its three existing dead-code warnings. / 定向回归通过，不等于三平台验收。
- RequiresApproval, Enabled, Disabled and unknown-state preservation are mapping/contract tests, not changed real OS login-item states. Production Panel rendering, NSH setters, Windows/Linux, installer/updater, performance/default/rollback and 14-day dual operation are NOT TESTED by this checkpoint. / 未改变真实登录项来制造状态，也未越过后续闸门。

## 12. Shared local-server source extraction / 本地服务源代码抽取

Before NSH acquires any service ownership, extract the existing Tauri-independent local-server I/O into a lightweight internal `desktop/host` library. This is a source-owner refactor, not a second controller or NSH startup acceptance. / NSH 接管前先抽取唯一来源；此切片不冒充新宿主服务迁移完成。

- **Owner and actual caller:** `desktop/shell/src/daemon.rs` reexports the same ensure/readiness/relaunch/wait operations from `aleph-desktop-host::local_server`. Its existing callers and product selection stay unchanged. Only one implementation resolves/spawns a local daemon or probes its readiness. / 旧壳真实调用方不变，启动与探测实现只有一份。
- **Retained old-shell effects:** keep `stop_daemon`, version marker and `reconcile_for_version` plus legacy autostart removal inside the Tauri shell. The new library cannot remove launchd/systemd/Task Scheduler registrations or reconcile installed versions. NSH does not yet call this library. / 破坏性旧版本接管不随通用启动代码迁移，NSH 暂不挂载。
- **Boundary and dependency:** the new internal, unpublished crate depends on Tokio/tracing only, not Core/Server/Tauri/NSH or native capture backends; the retained shell enables it only with `embedded-core`. It creates no runtime, worker or process until an existing caller invokes an operation. Cost: one additional workspace member and build dependency; benefit: later NSH lifecycle wiring cannot silently diverge from the retained baseline. / 增加一项 full 专用内部构建依赖，避免两套生命周期语义。
- **Extraction-time blocking gaps:** checkpoint `2026-11-22.6` retained the synchronous Unix launcher wait and an unbounded preliminary connect. Sections 12.1/12.2 now bound those operations, but external cancellation is still not an awaited drain and no NSH supervisor/runtime owner is mounted. These local bounds are not a whole-startup deadline. / 抽取时遗留的两项单操作边界现已补齐，仍不冒充取消排空、NSH 接管或全启动时限。
- **Readiness meaning:** the inherited `/ready` 200/503 classification is an HTTP readiness observation, not proof of binary identity or version and not permission to stop an existing service. Connection/read/parser bounds and fragmented replies need actual loopback-socket tests, not only status-enum tests. No installed default endpoint is used by those tests. / 就绪不等于进程身份；用自有临时端口检验真实网络边界。
- **Verification and limits:** preserve old-shell tests, add real isolated TCP regression cases and run strict lint. No production daemon/service command, default change or installed-product takeover is permitted by this refactor. Full/lite NSH startup, supervisor runtime ownership, explicit stop and process-level recovery remain separate pending implementation. / 保留旧回归，禁止用抽取或 socket 测试冒充产品生命周期验收。

**SPEC author pass:** checked old call sites, neutral I/O dependencies and the forbidden reconcile transfer. **QUALITY author pass after SPEC:** checked that a 200 reply cannot establish identity, extraction cannot authorize stopping, and tests must exercise actual fragmented network input without using port 18790. These are author passes, not independent approval. / 先明确意义与拒绝方向，再实施；不关闭全期闸门。

### 12.1 Bounded Unix launcher follow-up / Unix 启动器有界等待

The next source change replaces the inherited synchronous Unix launcher wait with Tokio's owned child wait. Reuse the existing `READY_TIMEOUT` (40 seconds) for the launcher operation; this is **not** a claim that the whole startup takes at most 40 seconds. On timeout, await kill/reap before returning a timeout error; nonzero launcher exit must fail rather than look like successful startup. Windows' detached server is not a short-lived Unix launcher and must not use this wait/kill policy. / 复用既有时限，只约束短命 Unix launcher；Windows 分离服务进程不套用该策略。

Tests use only owned `/bin/sh` exit/sleep children, with explicit cleanup even in RED, and assert actual child settlement rather than whether kill was called. They do not execute `aleph-server`, prove a detached server's recovery, or authorize changing an installed service. If the surrounding future is externally dropped, kill-on-drop is only a fallback, not an awaited drain receipt; the future NSH runtime owner must still join its operations. The preliminary port connect gap also remains open for a separate fail-closed change. / 子进程测试不冒充真实服务验收，外部取消和探测边界仍须由后续 owner 接线闭合。

**SPEC author follow-up:** checked Unix/Windows lifetime differences and exact old caller signatures. **QUALITY author follow-up after SPEC:** checked real child exit assertions, RED cleanup, nonzero refusal and the distinction between timeout cleanup and external future cancellation. / 作者按顺序自审，不冒充独立审核。

### 12.2 Fail-closed preliminary connect / 连接前探测失败闭合

Bound the preliminary loopback TCP connect by the existing two-second `PROBE_TIMEOUT`. Only an actual `ConnectionRefused` is a refused-listener observation; it is not a reservation or proof that a subsequent bind will succeed. Timeout, permission/network error and other unknown outcomes remain Foreign/unknown and never authorize a launch. `Some(false)` means positively refused, `Some(true)` connected, and `None` unknown; both consumers preserve this distinction and the unknown error message does not claim a confirmed foreign process. / 明确拒绝只是未监听观察，不保证随后 bind 成功；未知不得许可启动，也不能伪称已识别外来进程。

Verification uses pure I/O-error mapping plus an actual owned listener and a separately reserved, bound/non-listening TCP socket. On this machine the latter can time out rather than refuse, so a standard-library TCP probe supplies the independent OS observation and the async probe must preserve its classification. The fixture keeps the socket bound and does not assume that dropping a listener immediately produces `ConnectionRefused`. Diagnostic controls reproduced that old assumption failing during concurrent child creation; socket-only and isolated controls did not reproduce it. This does not establish a kernel, Tokio or inherited-FD cause. No permission-error injection or Windows/Linux native fault handling is claimed. The legacy unit-return port-close wait remains not a new NSH stop receipt. / 用真实 TCP 及独立标准库观察校验，明确拒绝与未知分开；修正测量夹具而非放宽生产判据，不把未证实的底层归因写成事实。

**SPEC author pass:** checked both callers (`probe_port` and legacy port-close waiting). **QUALITY author pass after SPEC:** checked unknown in both directions: neither launch permission nor proof of close. / 两个方向都保持拒绝，作者自审不等于独立审核。

The [bounded-host checkpoint](../runs/2026-11-22-phase11-bounded-host-receipt.json) records the final-source checks and the rejected fixture assumptions. The earlier source-extraction receipt remains historical and unchanged. Neither receipt is NSH full/lite startup, native cross-platform acceptance, service recovery or Phase11 completion. / 新回执单列有界操作验证，旧回执不改写；仍不关闭产品与跨平台闸门。
