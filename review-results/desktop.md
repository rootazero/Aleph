# Module: desktop

- Path: `desktop/` (linux, macos, windows, shared, shell)
- Files scanned: 99
- Total LOC: 24623

## Summary
| Severity | Count |
|----------|------:|
| critical | 2 |
| high     | 20 |
| medium   | 14 |
| low      | 0 |
| **Total**| **36** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness (CRITICAL + HIGH)
```
ISSUE|desktop/shared/src/media_types.rs:47|high|Camera clip duration accepts NaN; on macOS it reaches Duration::from_secs_f64 and panics
ISSUE|desktop/shared/src/media_types.rs:104|high|Audio recording duration accepts NaN; on macOS it reaches Duration::from_secs_f64 and panics
ISSUE|desktop/shared/src/action/input.rs:307|high|Drag duration is not capped; an untrusted u64 duration can block a worker thread indefinitely
ISSUE|desktop/shared/src/action/open_path.rs:73|high|Windows passes untrusted target through cmd.exe; command metacharacters can inject additional commands
ISSUE|desktop/shared/src/action/app_launch.rs:71|high|Windows passes untrusted app name through cmd.exe; command injection via shell metacharacters
ISSUE|desktop/windows/src/escape_listener.rs:146|critical|Stopping the listener can free ListenerState after a concurrent hook callback has loaded its raw address — USE-AFTER-FREE
ISSUE|desktop/windows/src/escape_listener.rs:98|high|The low-level keyboard hook is installed on the caller thread without running a Windows message loop — Escape callbacks not reliably delivered
ISSUE|desktop/windows/src/ax.rs:334|high|COM initialization errors ignored; CoUninitialize always runs — unbalances COM on incompatible apartment
ISSUE|desktop/shell/src/webview_perms.rs:58|high|Linux grants every UserMedia permission request without origin/type check — silently grants camera access alongside microphone
ISSUE|desktop/shell/src/webview_perms.rs:89|high|Windows silently grants microphone access to every origin in webview instead of restricting to configured Panel origin
ISSUE|desktop/shell/src/deeplink.rs:33|high|Complete deep-link URL logged at info level — leaks auth codes/tokens commonly carried in query params
ISSUE|desktop/shell/src/notify.rs:139|high|Remote Gateway credentials sent over unencrypted WebSocket whenever target uses HTTP — operator tokens exposed to network observers
ISSUE|desktop/shell/src/connection.rs:104|high|Gateway-token deletion failures ignored — token belonging to one remote can later be sent to a different remote
ISSUE|desktop/shell/src/cert_trust/pending.rs:79|critical|Certificate approval validates only the host; if another TLS challenge for the same host overwrites the pending record, a stale page approves a fingerprint the user never reviewed — AUTH BYPASS
ISSUE|desktop/shell/src/external_link.rs:92|high|Remote navigation allow-list compares only hostnames; different schemes/ports on the same host treated as trusted Panel origin
ISSUE|desktop/shell/src/update.rs:53|high|Update controls recognized solely by path on every origin — any loaded content can programmatically trigger install/restart without user gesture
```

### Perspective 2 — Logic & Correctness
```
ISSUE|desktop/linux/src/clipboard.rs:65|medium|Clipboard reads/writes accept non-zero exits as success and only try fallback tools when spawning fails — false empty reads, false successful writes
ISSUE|desktop/shared/src/action/input.rs:423|medium|Shared Linux clipboard rail returns xclip output and reports success without checking process exit status
ISSUE|desktop/shared/src/perception/screen_record.rs:225|high|macOS recorder ignores ScreenRecordConfig.region and records entire display — may capture content outside requested region (PII RISK)
ISSUE|desktop/shared/src/perception/screen_record.rs:371|medium|Recorder ignores whether waiting for didFinishRecording timed out — returns success without verifying output exists/complete
ISSUE|desktop/shared/src/action/window.rs:506|medium|macOS focus_window activates only the owning application, not the window identified by window_id — wrong window focused
ISSUE|desktop/shared/src/action/window.rs:565|medium|macOS move/resize resolve window by title — duplicate titles cause wrong window to be modified
ISSUE|desktop/shared/src/action/window.rs:271|medium|Windows focus_window discards SetForegroundWindow failure and returns success even when foreground-lock rules prevented focus
ISSUE|desktop/windows/src/ax.rs:364|high|When explicit PID has no visible window, AX resolution silently falls back to foreground process — reads/actions against wrong application
ISSUE|desktop/windows/src/system.rs:113|medium|list_running_apps emits one entry per window using window title as app name — duplicates, violates AppInfo contract
ISSUE|desktop/windows/src/pim.rs:134|medium|mail_folders returns full-path IDs but mail_search compares against folder leaf Name and silently falls back to Inbox on mismatch
ISSUE|desktop/windows/src/automation.rs:137|medium|run_shortcut appends optional input to PowerShell args but generated script never consumes/forwards it to shortcut target
ISSUE|desktop/macos/src/lib.rs:225|medium|macOS media forwarding converts typed errors into generic BridgeFailed — loses caller recovery semantics
ISSUE|desktop/shell/src/notify.rs:67|high|Notification WebSocket uses default TLS verifier and never consults in-app cert pin store — approved self-signed HTTPS Gateway cannot deliver notifications
ISSUE|desktop/shell/src/connection.rs:196|medium|Explicit-port detection stops only at slash — URLs like https://host:443?bt=... are misclassified
ISSUE|desktop/shell/src/notify.rs:51|high|Connection-target changes do not terminate active notification WebSocket — bridge remains subscribed to previous Gateway indefinitely
ISSUE|desktop/shell/src/perm_monitor.rs:126|high|Permission monitor searches for aleph-bridge but bundled macOS helper is named AlephBridge — permission transitions unmonitored
ISSUE|desktop/shell/src/main.rs:517|medium|Full shell exposes/persists Remote targets but forcibly overwrites every persisted Remote target with Local on next startup — DATA LOSS
ISSUE|desktop/shell/src/cert_trust/pending.rs:78|medium|Approval removes pending record before path resolution/persistence; save error leaves trust latch set with no record for retry
ISSUE|desktop/shell/src/update.rs:259|high|Applying update has no in-progress latch — repeated tray/menu/nav actions start concurrent download+install
ISSUE|desktop/shell/src/main.rs:694|medium|Returning to Local ignores daemon startup failure and reveals Panel anyway — navigates user to dead local origin
```

### Perspective 3 — Architecture Compliance
No specific R1/R3/R4/R8/R9/R10 redlines flagged in this module (desktop is the platform-specific layer per R1's exemption).

### Perspective 4 — Code Quality
No findings meet the severity threshold.
