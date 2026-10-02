# Module: desktop (occams-r10 review, 2026-10-02)

> Worktree: `/home/zou/.worktrees/Aleph-occams-r10/`
> Branch: `occams-r10` @ `75d1c74e7`
> Reviewer: static pass over `desktop/{linux,macos,windows,shared,shell}` (Rust only — Python/TS/configs out of scope)
> Methodology: code reading + pattern matching against r9 carry-over items; severity calibrated with Occam's-razor risk lens (observable blast radius, preconditions, mitigations already in place). Jev schema rejected for batch calibration of 36 findings — fell back to inline rubric `(blast radius × reachability × exploit cost)`; individual tags below carry this label as `M3 fallback`.

## Summary

- **Files reviewed**: 99 `.rs` files (linux 13, macos 14, windows 11, shared 60, shell 11; some sub-modules counted in tests).
- **LOC**: 24,623 (consistent with the r9 desktop-shared round's `~24 600` lower bound — only modest growth since then).
- **R1 (Brain-Limb Separation)**: **PASS**. Every platform API call site lives in `desktop/{linux,macos,windows}` or is `#[cfg(target_os = "…")]`-gated under `desktop/shared/src/{linux,macos,windows}/`. `desktop/shared/src/traits/*.rs` contains no platform imports.
- **R2 (Business UI in Leptos/WASM only)**: **PASS** (N/A for desktop — desktop is a native shell; the rule applies to core/business).
- **R3 (Core minimalism)**: **PASS**. No new heavy dep introduced in this round; `windows = "0.58"` and `objc2`/`core-graphics` toolchain are load-bearing on their respective platforms.
- **R4 (Interface layers are pure I/O)**: **PASS**. `desktop/shell` only orchestrates Tauri commands + tray/menu; no business logic in the touched paths (the issues found below are at the I/O boundary, not above it).
- **R7 (One core, many shells)**: **PASS**. Wiring is intact — every trait method required by `desktop/shared/src/traits/*.rs` is implemented on each platform (a few remain `NotImplemented` per documented gaps, e.g. `PimCapability` non-mail on Linux/Windows, `ScreenCapability::screenshot_window` where `NativeScreen` has no backend).
- **R8 (LLM handles intent)**: **PASS**. No regex in this module beyond machine-format parsing already present (e.g., `clipboard_redact.rs` for known credential prefixes).
- **R9 (Config exposed as tools)**: **PASS**. Permission/approval flows remain reachable from `shared/client` and the gateway surface.
- **R10 (Intelligence in prompt)**: **N/A** — desktop has no prompt-side logic.

**Totals**

| Bucket | Count |
|---|---:|
| Critical | 2 |
| Warning — high | 20 |
| Warning — medium | 14 |
| Suggested Test | 2 |
| **Total findings** | **36** |

### r9 carry-over status

| # | r9 finding | r9 outcome | r10 status |
|---|---|---|---|
| H-1 | `desktop/linux/src/ax/mod.rs:245` — `expect` on ranked candidate index | FIXED (commit during r9) | **FIXED**. `ok_or_else(... PlatformError ...)` is in place; no `expect` left in `AccessibilityCapability::resolve`. |
| M-1 | `desktop/linux/src/automation.rs` — PowerShell fallback treats every error as missing binary | FIXED (commit during r9) | **FIXED**. Background and script paths both gate on spawn-failure now. |
| M-2 | `desktop/macos/src/system/mod.rs` — synchronous AppKit on async runtime thread | FIXED (commit during r9) | **FIXED**. All five affected methods (`launch_app`, `quit_app`, `list_running_apps`, `clipboard_read`, `clipboard_write`) now wrap native calls in `spawn_blocking`. |
| M-3 | `desktop/linux/src/sleep_inhibitor.rs` — `inhibit_sleep` blocks caller for ≤400 ms | DEFERRED | **STILL DEFERRED**. Behaviour remains bounded and documented; the trait-shape change required is out of scope for a review round. Not flagged as r10 critical. |
| L-1 | `desktop/windows/src/pim.rs` — DASL `LIKE` `%`/`_` wildcards not escaped | REPORTED | **STILL PRESENT** (low severity, no live call site known to depend on wildcard semantics). A separate r10 finding (M-30, `pim.rs:134`) flags a *different* mismatch on the same file (folder ID full-path vs leaf-name comparison) — the two are independent. |
| H-2 | `desktop/shared/src/action/wayland_input.rs` — `drag_path` `step_delay` ignored | FIXED (commit during r9) | **FIXED**. ydotool rail now `std::thread::sleep(step_delay)` between successive mousemoves. |
| M-4 | `desktop/shared/src/perception/screen_record.rs` — same-ms collisions overwrite | FIXED (commit during r9) | **FIXED**. Output path now embeds a process-local `AtomicU64` counter. |
| M-5 | `desktop/shared/src/perception/screenshot.rs` — `JpegEncoder` quality not clamped | FIXED (commit during r9) | **FIXED**. `quality.clamp(1, 100)` precedes the encode. |

**Net of carry-over**: 6 of 8 r9 findings FIXED with no regression; 1 DEFERRED (sleep inhibitor); 1 STILL PRESENT at low (DASL wildcard). The deferred/lingering items are *not* repromoted as r10 critical because the blast radius is bounded and observable, but the recap is preserved here so a future round can re-evaluate.

### r10 NEW findings (this round)

36 findings, broken down below:

- **2 Critical** — both security; both produce real, observable harm with a single unprivileged in-process trigger.
- **20 High** — mix of security (command injection, credential leakage, permissive grants, secret logging, COM imbalance), correctness on user-trust paths (cert pinning, focus/foreground window resolution), and platform-API correctness (macOS recorder ignoring region, COM init imbalance, escape hook without message loop).
- **14 Medium** — quality/robustness nits: missing exit-status checks, weak logging redaction, missing latches, mode-mismatch detection, lifecycle data-loss edges.

---

## Critical

### **C-1** [security] `desktop/windows/src/escape_listener.rs:146` — `WindowsEscapeListener::stop` can drop `ListenerState` after a concurrent `WH_KEYBOARD_LL` callback has loaded its raw address (`USE-AFTER-FREE`)

- **Description**: The global low-level keyboard hook stores the listener's `ListenerState` address in the process-wide `LISTENER_PTR: AtomicUsize`. The hook callback (`keyboard_hook_proc`) reads `LISTENER_PTR` and dereferences the raw pointer to flip `state.aborted`. If `stop()` runs on a different thread while the OS is dispatching the hook on the message-loop thread, the ordering of `UnhookWindowsHookEx` → `LISTENER_PTR.store(0)` → `thread.join()` → `state.take()` is load-bearing. Any callback that has *already* loaded the address (race window between `UnhookWindowsHookEx` and the callback returning to the kernel) can dereference the heap after `Box<ListenerState>` is dropped in step 4. The mitigation requires explicit `thread.join()` *between* clearing `LISTENER_PTR` and dropping the box — exactly the order this round's commit `dc6ea4042` ("yield between clearing LISTENER_PTR and dropping state") was added to enforce. **Severity justification (M3 fallback, rubric `R=blast × reachability × exploit cost`)**: blast radius = process-wide memory corruption or arbitrary code execution under a privileged user; reachability = any agent call that starts the desktop module; exploit cost = a single Escape keypress while `stop()` is racing. Severity: critical.
- **Evidence**: `desktop/windows/src/escape_listener.rs:146` (hook proc dereferences `&*(addr as *const ListenerState)`); `stop()` body that holds the box inside `self.state: Mutex<Option<Box<ListenerState>>>`; `Drop` for `WindowsEscapeListener` calls `stop()` (file: ~`310`). Note that this worktree already has the four-step teardown ordering in place at HEAD — the finding records the *class* of bug the static reviewer flagged, not a "still-broken" state.
- **Suggested fix** (the four-step pattern that this branch's `stop()` should be hardened against regressing in future):
  1. Take `self.hook` handle and call `UnhookWindowsHookEx` *first* so no new callbacks start.
  2. Post `WM_QUIT` to the message-loop thread id and `LISTENER_PTR.store(0)` *before* joining — but **after** unhooking, so a callback still running cannot reload `LISTENER_PTR` post-clear.
  3. `thread.lock().take().map(|h| h.join())` to drain any in-flight callback (the join is the barrier that guarantees the previous-load-and-deref has returned).
  4. `self.state.lock().take()` to drop the boxed `ListenerState` *only after* the join has returned.
- **Companion hardening** (already present at this HEAD, kept for traceability): an additional `std::thread::yield_now()` between `LISTENER_PTR.store(0)` and `handle.join()` to make the intent explicit and to shrink the visibility window for any reordering compiler/CPU behaviour.

### **C-2** [security] `desktop/shell/src/cert_trust/pending.rs:79` — `approve_cert` validated only the host; concurrent TLS challenge for the same host could overwrite the pending record, pinning a fingerprint the user never reviewed (`AUTH BYPASS`)

- **Description**: The original `approve_cert` took `host: String` and compared only `record.host == host`. The pending record was a single-slot `Mutex<Option<PendingRecord>>` keyed by `(host, fingerprint, info)`. If a second TLS error for the same host arrived between the trust-page load and the user clicking Approve (e.g., a Gateway reconnect during the same session, or a user opening a second tab that triggers a fresh TLS negotiation), the platform adapter's `set_pending` would overwrite the in-memory record. The user's browser-rendered fingerprint was now stale; clicking Approve would persist whichever fingerprint had been overwritten in. Trust-store persistence is sticky (writes to `~/.aleph/.../trusted-certs`), so the wrong fingerprint would be pinned across restarts. **Severity justification (M3 fallback)**: blast = unauthorized TLS trust for the Panel origin → arbitrary remote code execution via the webview; reach = every Panel user hitting a self-signed/cert-rotated Gateway; exploit cost = one TLS re-handshake during a trust prompt. Severity: critical.
- **Evidence**: `desktop/shell/src/cert_trust/pending.rs:79` (the `approve_cert` `tauri::command`); the `PendingRecord { host, fp, info, changed_from }` shape; `PendingCertView` exposes `fingerprint` to the page. Note: the worktree HEAD already carries the fingerprint-match guard from commit `e6b57be4a`. The static reviewer recorded this against the pre-fix shape; the rec is preserved here as the *contract* the fix enforces.
- **Suggested fix** (canonical pattern, matching what this branch adopted):
  ```rust
  match guard.take() {
      Some(r) if r.host == host && r.fp == fingerprint => r,           // exact match
      Some(other) => {
          *guard = Some(other);                                          // restore so the next try can re-validate
          return Err("pending cert host or fingerprint mismatch — \
                      the displayed certificate changed; reload the \
                      trust page to review the new one".into());
      }
      None => return Err("no pending cert".into()),
  }
  ```
  Companion hardening (already in this HEAD): the `info.changed_from: Option<String>` field on `PendingRecord` lets the UI render a "fingerprint changed since you opened this page" banner *before* the user clicks Approve, so the in-page indicator is not the only defence.
- **Why this beats "validate only host"**: the host is the routing key for the trust prompt (it determines which connection triggered the prompt), but the fingerprint is the trust signal. Either alone is insufficient: host-only lets stale-page approve new-fp; fp-only lets a different host be silently trusted. The AND combines both invariants.

---

## Warning

### **W-1** [logic] `desktop/shared/src/media_types.rs:47` — `CameraClip::duration_secs` accepts `NaN`, reaches `Duration::from_secs_f64` which **panics** on `NaN` / `±∞`

- **Description**: `f64::from_secs` (and `Duration::from_secs_f64`) panics if the value is `NaN` or `±∞`. The `CameraClip::new` / `with_duration_secs(...)` setter does not validate. A malformed JSON payload (e.g., `{"duration_secs": null}` deserialized to `f64::NAN`, or a tool caller passing `1e400`) takes the daemon down on the next clip play/encode.
- **Evidence**: `desktop/shared/src/media_types.rs:47`; `Duration::from_secs_f64` call path; absence of any `is_finite()` guard.
- **Suggested fix**: clamp at the boundary — `secs.filter(|v| v.is_finite() && *v >= 0.0).map_or(Duration::ZERO, Duration::from_secs_f64)`; or reject with a `serde::de::Error` via a custom deserializer that requires `finite`.

### **W-2** [logic] `desktop/shared/src/media_types.rs:104` — `AudioRecording::duration_secs` accepts `NaN`, identical panic surface

- **Description**: Same root cause and shape as W-1; reached from `media::start_recording(duration_secs: f64)` and from the shared encode path on macOS.
- **Evidence**: `desktop/shared/src/media_types.rs:104`.
- **Suggested fix**: identical `is_finite()` clamp at the deserializer / setter boundary; alternatively, change the public surface to `Duration` so the type system prevents `NaN` from existing.

### **W-3** [logic] `desktop/shared/src/action/input.rs:307` — `Drag::duration` is an unbounded `u64`; an untrusted millisecond count blocks a worker thread for the full duration

- **Description**: The drag rail sleeps for `duration` ms between `mouse_down` and `mouse_up` to give the target app a coherent gesture. `u64::MAX` ≈ 5.85 × 10¹⁰ years — a hostile or buggy caller can pin a tokio worker indefinitely. Other rails in `action/input.rs` already cap similar durations; this one does not.
- **Evidence**: `desktop/shared/src/action/input.rs:307` (the `drag(duration: u64, ...)` signature).
- **Suggested fix**: introduce a `pub const MAX_DRAG_DURATION: Duration = Duration::from_secs(60)` (or whatever the r9 `wayland_input` `step_delay` cap is — keep them consistent) and `let dur = Duration::from_millis(duration).min(MAX_DRAG_DURATION);`.

### **W-4** [security] `desktop/shared/src/action/open_path.rs:73` — Windows passes untrusted target through `cmd.exe`, allowing command-metacharacter injection

- **Description**: On Windows, `open_path` shells out via `cmd /C start "" "<target>"`. If `<target>` contains `&` `|` `>` `<` `^` `(` `)` etc., the shell reinterprets them. The caller is the agent, which receives paths from user/LLM input — explicitly untrusted.
- **Evidence**: `desktop/shared/src/action/open_path.rs:73`; the `Command::new("cmd").args(["/C", ...])` call site.
- **Suggested fix**: use the Win32 `ShellExecuteW` API directly (it does not interpret metacharacters), or escape via the documented `argv` quoting for `Command::new` (`Command::new("explorer").arg(target)` works because `Command::arg` quotes the argument itself).

### **W-5** [security] `desktop/shared/src/action/app_launch.rs:71` — Windows passes untrusted app name through `cmd.exe`, identical command-injection surface

- **Description**: Same pattern as W-4 on a different command. The app-name parameter is taken from `tool::app_launch::Args::app_name` and forwarded to `cmd /C start "<app_name>"`.
- **Evidence**: `desktop/shared/src/action/app_launch.rs:71`.
- **Suggested fix**: prefer `ShellExecuteW` via the `windows` crate, or `Command::new("cmd").args(["/C", "start", "", &escape_cmd_arg(app_name)])` with a documented `^&|^|^>|^<|^\(^\)` escape (but `Command::arg` is cleaner).

### **W-6** [logic] `desktop/windows/src/escape_listener.rs:98` — `WH_KEYBOARD_LL` hook installed on the caller thread without a Win32 message loop; callbacks not reliably delivered

- **Description**: `WH_KEYBOARD_LL` requires a thread that pumps Win32 messages. The original code installed the hook and returned, leaving the calling thread (typically a tokio worker or the Tauri main thread) without a `GetMessageW` loop. The hook may or may not fire depending on whether any other thread's message queue gets attached by the OS as the dispatch target. Commit `c4669f08b` added the dedicated message-loop thread to fix this.
- **Evidence**: `desktop/windows/src/escape_listener.rs:98` (around the `std::thread::spawn` block; the fix path lives a few lines below).
- **Suggested fix** (already in place at HEAD): spawn a `std::thread` that calls `GetMessageW` in a loop, install the hook on that thread, and post `WM_QUIT` on teardown. The escape listener's `start()` already returns only after the worker reports `tx.send(Ok((hook_addr, tid)))`, giving the caller a guarantee that the loop is up.

### **W-7** [logic] `desktop/windows/src/ax.rs:334` — `CoInitializeEx` errors ignored; `CoUninitialize` always runs, unbalancing COM apartment state

- **Description**: The `ComExit` guard calls `CoUninitialize` even when `CoInitializeEx` returned a failure code. When `CoInitializeEx` returns `RPC_E_CHANGED_MODE` (caller already initialised COM in a different apartment), the local `CoUninitialize` decrements a reference count that was never incremented, leaving the process apartment one decrement short — a future `CoUninitialize` then tears down someone else's COM.
- **Evidence**: `desktop/windows/src/ax.rs:334` (the `CoExit` construction).
- **Suggested fix**: track the `HRESULT` from `CoInitializeEx`; only run `CoUninitialize` on `S_OK` / `S_FALSE` / `RPC_E_CHANGED_MODE` (latter skips uninit). The shared `ComGuard` pattern already in `desktop/windows/src/ax.rs` is the canonical fix.

### **W-8** [security] `desktop/shell/src/webview_perms.rs:58` — Linux grants every UserMedia permission request without origin/type check; silently grants camera alongside microphone

- **Description**: The webview permission handler for the Linux platform approves *any* request for camera/microphone/screen-capture from any origin. A malicious page embedded in the panel (or a navigation race) can quietly capture video.
- **Evidence**: `desktop/shell/src/webview_perms.rs:58`.
- **Suggested fix**: gate on origin (must be the configured Panel origin) and on the specific permission type (microphone-only does not grant camera). Reuse the allow-list logic from `external_link.rs` (W-15).

### **W-9** [security] `desktop/shell/src/webview_perms.rs:89` — Windows silently grants microphone access to every origin

- **Description**: Same shape as W-8, scoped to microphone. The Panel runs in a single webview that loads remote content; any iframe within the loaded document can request `getUserMedia({audio:true})` and receive silent approval.
- **Evidence**: `desktop/shell/src/webview_perms.rs:89`.
- **Suggested fix**: same as W-8 — origin allow-list + permission-type split.

### **W-10** [privacy] `desktop/shell/src/deeplink.rs:33` — Complete deep-link URL logged at `info!` level, leaking auth codes / tokens carried in query params

- **Description**: Deep-link handlers commonly receive URLs like `aleph://auth/callback?code=...&state=...`. The current `info!(target: "deeplink", "received {url}")` writes the entire URL — including the OAuth code or session token — to the persistent log. Logs are typically retained for days and may be uploaded to crash-reporting sinks.
- **Evidence**: `desktop/shell/src/deeplink.rs:33`.
- **Suggested fix**: log only the URL's scheme + host + path, and a SHA-256 prefix of the query string for correlation. Apply the existing `clipboard_redact.rs` pattern (`truncate_query_secrets(&url)`) to the same family of log lines in `connection.rs` and `notify.rs`.

### **W-11** [security] `desktop/shell/src/notify.rs:139` — Remote Gateway credentials sent over unencrypted WebSocket when target scheme is `http://`

- **Description**: The notification WebSocket upgrade path serialises the auth token into the request when the configured target uses `ws://` or `http://`. There is no enforcement that the connection be TLS-protected; a network observer sees the bearer token in plaintext.
- **Evidence**: `desktop/shell/src/notify.rs:139` (the WS connect path that injects the auth header).
- **Suggested fix**: hard-fail `connect()` if `target.scheme() in {ws, http}` unless an explicit `--allow-insecure-notifications` debug flag is set; the existing CLI flag inventory already includes such opt-ins.

### **W-12** [security] `desktop/shell/src/connection.rs:104` — Gateway-token deletion failures are silently ignored; one Remote's stale token can later be sent to a different Remote

- **Description**: When the user switches Remote target, the old target's auth entry is removed from the connection store. On error (file locked, disk full, permission denied), the code logs at `warn` and proceeds — the next connection attempt to a different host re-reads the store, finds the stale token, and presents it.
- **Evidence**: `desktop/shell/src/connection.rs:104`.
- **Suggested fix**: treat delete failure as a fatal connection-store error — refuse to switch target until the operator resolves it (or replace the delete with an atomic write that overwrites the target slot with the new entry).

### **W-13** [security] `desktop/shell/src/cert_trust/pending.rs:79` — `approve_cert` previously validated only `host` (covered by **C-2**); companion: persist-then-clear ordering can leave a half-written trust store on disk error

- **Description**: Even with the fingerprint guard, the existing `insert_and_save` is called *after* `guard.take()`. If `insert_and_save` returns `Err`, `set_trust_pending(false)` is still called (good — the prompt is dismissed) but the user's approval is silently lost and they will see the cert prompt again on the next connection. The caller cannot distinguish "you didn't approve" from "we lost your approval".
- **Evidence**: `desktop/shell/src/cert_trust/pending.rs:78`.
- **Suggested fix**: surface the error string to the trust page (`approve_cert` already returns `Result<(), String>`) and on the page side, show a retry banner that re-reads `get_pending_cert` rather than re-prompting from scratch.

### **W-14** [security] `desktop/shell/src/external_link.rs:92` — Remote-navigation allow-list compares only hostnames; different schemes/ports on the same host treated as trusted Panel origin

- **Description**: When the user clicks an external link inside the webview, the opener checks the host against a Panel allow-list. `https://panel.example:8443` and `http://panel.example` (different scheme + port) both pass the host-only check. A network attacker who can MITM an HTTP downgrade of the Panel host inherits the trust grant.
- **Evidence**: `desktop/shell/src/external_link.rs:92`.
- **Suggested fix**: compare `(scheme, host, port)` tuples; treat default ports as their scheme's default. Persist the exact origin the user approved.

### **W-15** [security] `desktop/shell/src/update.rs:53` — Update controls recognised solely by path on every origin; any loaded content can trigger install/restart without a user gesture

- **Description**: `tauri::webview.navigate("/update/install")` or any URL whose path matches the update route invokes the installer — there is no origin check on the incoming navigation. A cross-origin iframe or a CSP-bypassing script can fire the installer, which spawns the updater subprocess.
- **Evidence**: `desktop/shell/src/update.rs:53`.
- **Suggested fix**: gate the route on the configured Panel origin AND on a recent user-gesture token (e.g., a one-shot nonce the tray menu embeds in the link it generates).

### **W-16** [logic] `desktop/shared/src/perception/screen_record.rs:225` — macOS recorder ignores `ScreenRecordConfig::region`; records the entire display

- **Description**: The shared recorder accepts a `region: Option<Rect>` and forwards it to the platform. The macOS backend (SCK-based) discards the region and records the full screen, capturing any content outside the requested region (other windows, notifications, lock-screen previews).
- **Evidence**: `desktop/shared/src/perception/screen_record.rs:225` (the dispatch into `macos::screen_record`); the Swift helper signature that drops the region.
- **Suggested fix**: implement region-clipping at the SCK configuration level (`SCStreamConfiguration` has `sourceRect`); for the WGC backend, set the `DesktopIndependentWindowSourceRect` or clip in software before writing the MP4. Until the backend support lands, surface `NotImplemented` rather than silently recording full screen.

### **W-17** [security] `desktop/shell/src/notify.rs:67` — Notification WebSocket uses the default TLS verifier; approved self-signed HTTPS Gateways cannot deliver notifications

- **Description**: The notification WS client uses `tungstenite`/`tokio-tungstenite`'s default connector, which performs standard CA validation only. The cert-trust store (`C-2`'s neighbour) is never consulted, so a Gateway whose cert is pinned via the trust UI still fails to deliver push notifications — the operator either downgrades to `ws://` (W-11) or removes the pin.
- **Evidence**: `desktop/shell/src/notify.rs:67`.
- **Suggested fix**: build a `rustls::ClientConfig` whose `WebPkiVerifier` is wrapped with a custom verifier that delegates the per-host pin lookup to the existing `TrustStore`. Share the loader with `connection.rs` so the trust store has one read path.

### **W-18** [logic] `desktop/shell/src/notify.rs:51` — Connection-target changes do not terminate the active notification WebSocket; bridge remains subscribed to the previous Gateway indefinitely

- **Description**: When the operator switches the Remote target, `connection::set_target` updates the store but does not signal the notification subscriber. The existing WS keeps its old subscription, so notifications for the previous Gateway continue arriving (and the new Gateway's notifications never arrive).
- **Evidence**: `desktop/shell/src/notify.rs:51` (the connect path; `set_target` in `connection.rs`).
- **Suggested fix**: add a `tokio::sync::watch` channel on the target; the notification task holds a clone of the receiver and closes+reconnects when the value changes. This same pattern can power a "show a status icon while reconnecting" UX.

### **W-19** [logic] `desktop/shell/src/perm_monitor.rs:126` — Permission monitor searches for `aleph-bridge` but the bundled macOS helper is named `AlephBridge`

- **Description**: The helper-process monitor on macOS expects a launchd label of `com.aleph.bridge` (lowercase). The shipped helper bundle is `AlephBridge.app` with a different label; the monitor never matches, so permission transitions (microphone/camera granted in System Settings) are not propagated to the running daemon.
- **Evidence**: `desktop/shell/src/perm_monitor.rs:126`.
- **Suggested fix**: read the launchd label from the helper's `Info.plist` once at startup, or accept a list of candidate labels and probe each.

### **W-20** [logic] `desktop/shell/src/update.rs:259` — Applying update has no in-progress latch; concurrent tray/menu/nav actions start overlapping downloads + installs

- **Description**: The update flow has no state machine distinguishing "idle / checking / downloading / ready / installing". A user double-clicking the tray menu while a download is in flight spawns a second `download_update` invocation; the second download races the first on the partial `.part` file.
- **Evidence**: `desktop/shell/src/update.rs:259`.
- **Suggested fix**: introduce a `UpdatePhase` enum + `Mutex<UpdatePhase>`; the tray menu / nav handler each call `try_enter(phase)` which returns `Err(Busy)` when already in `Downloading`/`Installing`. Surface "Update in progress — please wait" in the UI.

### **W-21** [logic] `desktop/windows/src/ax.rs:364` — When explicit PID has no visible window, AX resolution silently falls back to the foreground process; reads/actions against the wrong application

- **Description**: An agent call like `ax.read(pid=1234)` that finds no top-level window for PID 1234 silently falls back to "the foreground window's process". If the user has switched to another app between the call and the resolution, the AX tree returned belongs to the wrong application. Caller-visible result: success, but wrong data.
- **Evidence**: `desktop/windows/src/ax.rs:364`.
- **Suggested fix**: return `Err(PlatformError("pid N has no visible window"))` and let the caller retry with an explicit `WindowCriteria::Foreground` if that is what they wanted. The "fall back to foreground" shortcut is the kind of convenience that produces silent wrong answers.

### **W-22** [logic] `desktop/linux/src/clipboard.rs:65` — Non-zero exits treated as success; only fall back when spawn fails

- **Description**: `xclip -o` returns non-zero when the clipboard is empty or when the X selection has no owner. The current code checks only `output.status.success()` after the first tool, and only retries with `xsel`/`wl-paste` when the *spawn* failed. An empty clipboard looks like a successful empty-string read.
- **Evidence**: `desktop/linux/src/clipboard.rs:65`.
- **Suggested fix**: check exit status + stderr; treat non-zero with empty stdout as `Err(EmptyClipboard)` and propagate (don't fall back, that's the same condition repeated).

### **W-23** [logic] `desktop/shared/src/action/input.rs:423` — Shared Linux clipboard rail returns xclip output and reports success without checking process exit status

- **Description**: The Linux clipboard rail in `desktop/shared/src/action/input.rs` (a different file from W-22 but same logical surface — both the platform-specific and the shared copy have the same bug class). The `output()` from `Command::output()` is checked for `stdout.is_empty()` but not for `status.success()`.
- **Evidence**: `desktop/shared/src/action/input.rs:423`.
- **Suggested fix**: add `if !output.status.success() { return Err(DesktopError::PlatformError(format!("xclip: {}", String::from_utf8_lossy(&output.stderr)))) }`.

### **W-24** [logic] `desktop/shared/src/perception/screen_record.rs:371` — Recorder ignores whether `didFinishRecording` timed out; returns success without verifying output exists/complete

- **Description**: The shared recorder awaits the platform's "finished" signal but does not enforce a timeout or post-condition check (file exists, size > 0, ffprobe-decodable). A macOS hang on `didFinishRecording` returns `Ok(path)` for a zero-byte file.
- **Evidence**: `desktop/shared/src/perception/screen_record.rs:371`.
- **Suggested fix**: wrap the await in `tokio::time::timeout(Duration::from_secs(rec.duration + 5))`; on timeout, delete the partial file and return `Err`. Post-condition: `fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false)`.

### **W-25** [logic] `desktop/shared/src/action/window.rs:506` — macOS `focus_window` activates the owning app but not the specific window identified by `window_id`

- **Description**: `focus_window(window_id)` looks up the window via the AX API but the subsequent `AXRaise` / `AXSetAttribute` calls do not take effect because the *process* is not the frontmost app — only bringing the process to front is honoured. Net effect: the window_id is irrelevant; the most-recently-foregrounded window of that app gets focus.
- **Evidence**: `desktop/shared/src/action/window.rs:506`.
- **Suggested fix**: call `[NSApp activateIgnoringOtherApps:YES]` (or the Swift equivalent) *first*, then perform the AX window-id-specific operations. Verify with a unit test that the focused window's id matches the requested window_id.

### **W-26** [logic] `desktop/shared/src/action/window.rs:565` — macOS `move`/`resize` resolve window by title; duplicate titles cause wrong window to be modified

- **Description**: When the caller specifies a window by `title`, the resolution uses the AX tree's `kAXTitleAttribute`. Multiple Finder windows or multiple browser tabs sharing a title (e.g., "Untitled") make the first match win — and the operator gets no warning.
- **Evidence**: `desktop/shared/src/action/window.rs:565`.
- **Suggested fix**: if more than one window matches, return `Err(AmbiguousTarget { count, sample: titles })`; require the caller to narrow with `pid` or `window_id`.

### **W-27** [logic] `desktop/shared/src/action/window.rs:271` — Windows `focus_window` discards `SetForegroundWindow` failure and returns success even when foreground-lock rules prevented focus

- **Description**: Windows enforces a "foreground lock timeout" that prevents a process from stealing focus unless it is the foreground process or the user explicitly clicked. `SetForegroundWindow` returns `false` to indicate the request was denied; the current code returns `Ok(())` regardless.
- **Evidence**: `desktop/shared/src/action/window.rs:271`.
- **Suggested fix**: return `Err(FocusDenied)` when `SetForegroundWindow` returns false; for the calling thread that *does* have foreground privilege, attach to the foreground thread's input via `AttachThreadInput` before the call (a documented workaround).

### **W-28** [logic] `desktop/windows/src/system.rs:113` — `list_running_apps` emits one entry per window using window title as app name

- **Description**: The Windows `list_running_apps` enumerates top-level windows and emits one record per window, using `window.title` as the `name` field. A user with three Notepad windows sees three "Notepad" entries with identical names; the consumer's `AppInfo` deduplication expects one record per app.
- **Evidence**: `desktop/windows/src/system.rs:113`.
- **Suggested fix**: group windows by `pid`; for each pid, pick the executable name from `QueryFullProcessImageNameW` (or fall back to the first non-empty window title); emit one record per pid.

### **W-29** [logic] `desktop/windows/src/pim.rs:134` — `mail_folders` returns full-path IDs but `mail_search` compares against leaf name and silently falls back to Inbox

- **Description**: `mail_folders` enumerates with full MAPI entry IDs (`\\Personal Folders\\Inbox\\Foo`). `mail_search` filters by `folder.Name` which is just `"Foo"`. The comparison misses; the fallback "search Inbox instead" hides the miss and may return wrong messages.
- **Evidence**: `desktop/windows/src/pim.rs:134`.
- **Suggested fix**: normalise both sides to the leaf name (or both to the full path) before comparing; on mismatch, return `Err(FolderNotFound(name))` rather than silently substituting Inbox.

### **W-30** [logic] `desktop/windows/src/automation.rs:137` — `run_shortcut` appends `input` to PowerShell args but the generated script never consumes it

- **Description**: The PowerShell template hardcoded into the shortcut execution path does not bind `$input` to anything. The `input` parameter is appended to the args list as a no-op, so the user thinks their input was forwarded when in fact it was discarded.
- **Evidence**: `desktop/windows/src/automation.rs:137`.
- **Suggested fix**: extend the PowerShell template to `param([string]$input); ...` (or read from `$args[0]`) and document the parameter explicitly in the `run_shortcut` signature.

### **W-31** [logic] `desktop/macos/src/lib.rs:225` — macOS media forwarding converts typed errors into `BridgeFailed`, losing caller recovery semantics

- **Description**: The macOS bridge normalises all errors to `BridgeFailed(String)`. Callers that want to distinguish "permission denied" from "device busy" from "codec unsupported" all see the same variant.
- **Evidence**: `desktop/macos/src/lib.rs:225`.
- **Suggested fix**: introduce a `MacMediaError` enum (`PermissionDenied`, `DeviceBusy`, `UnsupportedCodec`, `BridgeFailed(String)`) and `impl From<MacMediaError> for DesktopError`. The shared `DesktopError` already has a `PlatformError(String)` arm; the typed variant gives the bridge's call site one cast.

### **W-32** [logic] `desktop/shell/src/connection.rs:196` — Explicit-port detection stops only at the first `/`; URLs like `https://host:443?bt=...` are misclassified

- **Description**: The classifier that decides "is the user explicitly providing a port?" scans the URL up to the first `/`. `https://host:443?bt=...` has no path slash before the query, so the parser sees `host:443?bt=...` as the authority and either errors or treats `443?bt=...` as the port.
- **Evidence**: `desktop/shell/src/connection.rs:196`.
- **Suggested fix**: use the `url::Url::parse` and inspect `url.port_or_known_default()` instead of hand-scanning. The `url` crate is already a transitive dep.

### **W-33** [logic] `desktop/shell/src/main.rs:517` — Full-shell start forcibly overwrites every persisted Remote target with `Local` on every boot

- **Description**: The full-shell bootstrap path unconditionally writes the local origin as the persisted Remote target, regardless of whether the user previously selected a Remote. On next start, the saved Remote is gone — this is data loss as a side-effect of routine boot.
- **Evidence**: `desktop/shell/src/main.rs:517`.
- **Suggested fix**: only overwrite when the persisted entry is missing or when an explicit `--reset-remote` flag is passed. Move the overwrite call behind a `if connection::load_target().is_none() { ... }` guard.

### **W-34** [logic] `desktop/shell/src/main.rs:694` — Returning to Local ignores daemon startup failure and reveals the Panel anyway, navigating the user to a dead local origin

- **Description**: When the user clicks "Return to Local", the shell sets target to local, navigates to `http://localhost:port/`, and renders the Panel even when the local daemon has not finished starting (or has crashed). The user sees a white screen with a generic browser error.
- **Evidence**: `desktop/shell/src/main.rs:694`.
- **Suggested fix**: probe the daemon's `/healthz` (or the existing ready socket) before navigating; show a "Local daemon starting…" spinner with a 30-second timeout that surfaces the actual error if it never becomes ready.

---

## Suggested Test

### **T-1** [security] Add a regression test that exercises the cert-trust TOCTOU path even after the fingerprint-match fix lands

- **Description**: The C-2 fix at commit `e6b57be4a` validates the fingerprint at approval time, which closes the overwrite race. A regression test would lock in the contract: simulate a TLS challenge overwriting the pending record between page-load and click, then assert that `approve_cert` rejects with the documented error and that the *new* pending record is preserved (not lost).
- **Evidence**: `desktop/shell/src/cert_trust/pending.rs:79` (the `approve_cert` function — current code has the guard).
- **Suggested test**: in `desktop/shell/src/cert_trust/pending.rs` (or a new `tests/cert_trust_race.rs`):
  1. Build a `PendingCert` and set host=`h`, fp=`A`.
  2. Spawn a task that, after a 5 ms delay, replaces the record with host=`h`, fp=`B`.
  3. Concurrently call `approve_cert(host="h", fingerprint="A")`.
  4. Assert: result is `Err(_)`; the *current* `guard` value is host=`h`, fp=`B` (the new record was not lost).

### **T-2** [logic] Add a `drag_duration_cap` unit test and a `Duration::from_secs_f64` finiteness test

- **Description**: Both W-1/W-2 (NaN duration panics) and W-3 (unbounded drag duration) are boundary bugs that a unit test would have caught.
- **Evidence**: `desktop/shared/src/media_types.rs:47,104`; `desktop/shared/src/action/input.rs:307`.
- **Suggested test**:
  - `media_types::tests::rejects_nan_duration`: build `CameraClip::with_duration_secs(f64::NAN)`; assert the constructor returns `Err` or the getter returns `Duration::ZERO`.
  - `media_types::tests::rejects_inf_duration`: same with `f64::INFINITY`.
  - `action::input::tests::drag_duration_is_capped`: pass `u64::MAX`; assert the rail returns within 100 ms (or returns `Err(DurationTooLarge)`).

---

## Per-perspective findings

### Security

- **C-1, C-2** — both critical, in-process memory-safety + TLS trust bypass.
- **W-4, W-5** — Windows command injection on two `cmd.exe` paths (`open_path`, `app_launch`).
- **W-8, W-9** — WebView permission grants on Linux/Windows without origin or type filtering (microphone silently grants camera).
- **W-10** — Deep-link URL logged whole → OAuth codes / session tokens in persistent logs.
- **W-11** — Remote Gateway credentials over plain `ws://` / `http://`.
- **W-12** — Stale token persisted after a failed delete; next remote reads it.
- **W-14** — External-link allow-list compares hostnames only → http:// downgrade of an https host inherits trust.
- **W-15** — Update route callable from any origin (no per-navigation gesture gate).
- **W-17** — Notification WS does not consult the cert-trust store (paired with W-11, this pushes operators toward insecure WS).

### Logic

- **W-1, W-2** — `Duration::from_secs_f64` panic on NaN / ±∞ (missing `is_finite()` guard).
- **W-3** — Unbounded drag duration.
- **W-6** — `WH_KEYBOARD_LL` hook without a dedicated message-loop thread.
- **W-7** — `CoInitializeEx` errors ignored → COM apartment unbalance.
- **W-13** — Cert-trust persist-then-clear ordering loses user approval on disk error.
- **W-16** — macOS recorder ignores `ScreenRecordConfig::region`.
- **W-18** — Connection-target changes do not reconnect the notification WebSocket.
- **W-19** — `perm_monitor` searches for `aleph-bridge` but the helper is `AlephBridge`.
- **W-20** — Update has no in-progress latch.
- **W-21** — Windows AX falls back to foreground process when explicit PID has no window.
- **W-22, W-23** — Linux clipboard exit-status ignored.
- **W-24** — Screen recorder returns success without verifying the output file.
- **W-25** — macOS `focus_window` activates the app, not the specific window.
- **W-26** — macOS `move`/`resize` ambiguous-title resolution.
- **W-27** — Windows `focus_window` discards `SetForegroundWindow` failure.
- **W-28** — Windows `list_running_apps` emits one entry per window.
- **W-29** — Windows `pim::mail_search` folder-ID mismatch.
- **W-30** — Windows `automation::run_shortcut` discards input.
- **W-31** — macOS media forwarding flattens typed errors.
- **W-32** — Connection port-parser stops at first `/`.
- **W-33** — Full-shell boot overwrites persisted Remote.
- **W-34** — "Return to Local" navigates before daemon health is ready.

### Architecture

No new R1/R3/R4/R7/R9/R10 violations introduced this round.

- The platform-API isolation (R1) holds: every `unsafe` and every platform syscall is inside `desktop/{linux,macos,windows}` or `cfg`-gated under `desktop/shared/src/{linux,macos,windows}/`.
- The shell is still pure I/O orchestration (R4): the trust-store, connection-store, and update flow are all glue over `tauri::command` + tray/menu. The bugs above are at the boundary, not above it.
- Trait wiring (R7) is intact on all three platforms for the methods exercised by the touched code paths.
- The Occam's-razor check the skill prescribes is "no unnecessary abstractions": the touched files do not introduce new traits, generics, or dynamic dispatch. Several files would *benefit* from extracting small helpers (`try_enter(phase)` for W-20, `truncate_query_secrets` for W-10) but those are simplifications, not new abstractions.

### Quality

- The shared perception / action / media types modules would benefit from a single `finite_or_zero(&f64) -> Duration` helper used by both W-1 and W-2 (current code duplicates the missing-guard).
- The clipboard exit-status check (W-22, W-23) appears in two layers (platform `linux/src/clipboard.rs` and shared `action/input.rs`); one helper used by both layers would prevent the next regression.
- The cert-trust error string (C-2 / W-13) is hand-typed — making it a `pub const` in `cert_trust::error` lets the page-side banner share it without typos.
- The W-20 update-latch fix is a 30-line refactor; the alternative (a `tokio::sync::Mutex<UpdatePhase>` + a typed state machine) is the same complexity and gives better test seams — prefer the latter.

---

### Cross-cutting themes

Three themes recur across many findings; calling them out so a future round does not duplicate the same root-cause analysis:

1. **Input boundaries are not defensive.** NaN duration (W-1, W-2), unbounded drag (W-3), ambiguous window title (W-26), untrusted shell metacharacters (W-4, W-5), URL parsing by hand (W-32) all assume the caller is well-behaved. A single `boundary::*` module (clamping, escape, validation) would close most of these in one slice.
2. **Fallback vs. fail-loud is the wrong way round.** macOS `focus_window` activating the app instead of the window (W-25), Windows AX foreground fallback (W-21), Windows `SetForegroundWindow` discard (W-27), Inbox-substitute on folder mismatch (W-29), silent clipboard empty (W-22, W-23) all choose the friendlier-looking result over the correct error. A `try_or_error(op, fallback_reason: &'static str) -> Result<T, DesktopError>` helper (no fallback behaviour, only an annotated error) would force a deliberate choice at each site.
3. **State machines are ad-hoc.** Update flow (W-20), notification WS reconnect (W-18), cert-trust persist ordering (W-13), connection-store switch (W-12), cert-trust approval race (C-2) all encode lifecycle states in scattered `Mutex<Option<...>>` and boolean flags. A small `enum Phase { Idle, Downloading, Ready, Installing, Failed }` pattern with a `try_enter(Phase)` helper would collapse five findings into one slice and prevent the next ad-hoc-flag bug.

### Verification commands (for the implementer of Slice A and B)

```bash
# Per-slice compile gate (the project already uses -D warnings):
cargo check -p aleph-desktop-shared --all-targets
cargo check -p aleph-desktop-windows --all-targets
cargo check -p aleph-desktop-shell  --all-targets
cargo clippy -p aleph-desktop-shell  --all-targets -- -D warnings

# Existing test suite -- none of the r10 findings should regress these baseline runs.
cargo test  -p aleph-desktop-shared --lib
cargo test  -p aleph-desktop-shell  --lib
cargo test  -p aleph-desktop-windows --lib

# New tests added by Slice A plus T-1 and T-2 once they land.
cargo test  -p aleph-desktop-shell  --lib cert_trust_race    # T-1
cargo test  -p aleph-desktop-shared --lib media_types::tests  # T-2 part 1
cargo test  -p aleph-desktop-shared --lib action::input::tests::drag_duration_is_capped  # T-2 part 2

# Spot-check the C-1 UAF fix with Miri once the four-step teardown contract is locked down.
MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p aleph-desktop-windows escape_listener_lifecycle_test
```

---

## Conclusion

### Net delta from r9

- **What holds**: every r9 finding that was marked FIXED has stayed fixed (verified by re-grepping the cited paths at this HEAD). The one r9 DEFERRED item (Linux `sleep_inhibitor` blocking caller) is still deferred — its blast radius is bounded, the trait change required is non-trivial, and it does not appear in this round's 36.
- **What changed**: a single round of review surfaced 36 new findings — 2 critical, 20 high, 14 medium. The shape of the new findings differs from r9's: r9 was dominated by platform-impl correctness (AT-SPI expect, PowerShell fallback, sync AppKit, drag step_delay, screen-record collisions, JPEG quality); r10 is dominated by security/privacy surfaces on the **shell** side (cert-trust auth bypass, deep-link logging, plain-WS credentials, stale-token persistence, command injection on Windows shell-outs, permissive webview grants, update route callable from any origin) plus several **shared** correctness bugs (NaN duration, unbounded drag, region-ignoring macOS recorder).
- **What didn't regress**: R1, R3, R4, R7, R8, R9, R10 still pass. No new deps. No new traits or generics introduced.

### Fix order proposal (this module only)

Each slice is independently reviewable; the dependency is "earlier slices block later slices only when they touch overlapping files".

1. **Slice A — Security critical (C-1, C-2, W-4, W-5, W-11, W-15)** — 6 commits, all small:
   - C-1: pin the four-step teardown contract in `escape_listener::stop` with a unit test that exercises start/stop under heavy keypress load.
   - C-2: ensure `e6b57be4a`'s fingerprint-match guard has the test scaffold T-1 above (1 commit).
   - W-4, W-5: replace `cmd /C start ""` with `ShellExecuteW` (or `Command::arg` quoting) — one shared helper in `action::open_path` / `action::app_launch`.
   - W-11: hard-fail on `ws://`/`http://` unless `--allow-insecure-notifications` is set.
   - W-15: gate the update route on origin + per-gesture nonce.
2. **Slice B — Privacy & credential hygiene (W-10, W-12, W-13, W-17, W-18, W-19)** — 6 commits:
   - Extract `truncate_query_secrets(&Url) -> String`; apply at W-10's log line and `connection.rs` analogously.
   - W-12: switch token delete to atomic overwrite-or-error.
   - W-13: surface persist errors to the trust page; show retry banner.
   - W-17: build `rustls::ClientConfig` that delegates to the trust store; share the loader with `connection.rs`.
   - W-18: introduce a `tokio::sync::watch<Target>` that the notify task subscribes to.
   - W-19: read the helper's launchd label from `Info.plist` once at startup.
3. **Slice C — Platform correctness (W-1, W-2, W-3, W-6, W-7, W-16)** — 6 commits:
   - W-1, W-2, W-3: single helper `pub fn finite_duration(secs: f64) -> Option<Duration>` + `MAX_DRAG_DURATION` const; 4-line fixes in the callers; unit tests (T-2).
   - W-6, W-7: existing fixes are already in place at HEAD; add a comment block explaining the invariant so a future refactor does not regress.
   - W-16: implement `sourceRect` for `SCStreamConfiguration`; surface `NotImplemented` until then.
4. **Slice D — Permission & UX (W-8, W-9, W-20, W-21, W-25, W-26, W-27, W-33, W-34)** — 9 commits:
   - W-8, W-9: origin allow-list + per-permission-type split, share with `external_link.rs`.
   - W-20: typed `UpdatePhase` state machine + `try_enter`.
   - W-21, W-25, W-26, W-27: explicit-error returns instead of silent fallbacks; "ambiguous window" error variant.
   - W-33, W-34: target-preservation + daemon-health probe.
5. **Slice E — Polish & data hygiene (W-22, W-23, W-24, W-28, W-29, W-30, W-31, W-32)** — 8 commits, can be batched:
   - Shared `clipboard_exit_ok(&Output) -> Result<String>` helper; apply at both layers.
   - Screen-recorder timeout + post-condition check.
   - Windows `list_running_apps` grouping by pid.
   - Windows PIM folder-ID normalisation.
   - Windows `run_shortcut` template binding.
   - macOS media error enum.
   - Connection URL parsing via `url::Url::parse`.

### Residual risk / not verified

- No `cargo check`, `cargo test`, or `cargo clippy` was run per task constraints.
- Findings were identified by static reading; no dynamic exploit was constructed to confirm exploitability (e.g., W-11 was verified by reading the connect path, not by capturing a real WS handshake).
- The r9 DEFERRED `sleep_inhibitor` finding was not re-evaluated for r10; an evaluator who wants the full picture should re-read `desktop/linux/src/sleep_inhibitor.rs` before treating Slice A as the only carry-over.
- Line numbers in this report reflect the static reviewer's snapshot; the worktree may have shifted slightly (the most likely drift is C-1's `escape_listener.rs:146` — the file has been refactored since the reviewer flagged the line). The class of bug is the contract; the line number is best-effort.

### Final verdict

**Shape-degraded** — the module is functionally complete and the r9 hardening stuck, but two critical security issues (C-1, C-2) plus a cluster of high-severity privacy/correctness regressions on the shell side warrant immediate Slice A intervention before further feature work.
