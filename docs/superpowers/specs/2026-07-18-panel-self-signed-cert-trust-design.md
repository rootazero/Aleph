# Panel Self-Signed Cert Trust (TOFU) — Design

**Date:** 2026-07-18
**Status:** Approved (brainstorming) → ready for implementation plan
**Scope:** Desktop lite shell (macOS / Linux / Windows) + iOS Panel app

## Problem

When a Panel app connects to a remote Aleph core over a **self-signed** TLS
certificate (the zero-config default for a bare public-IP deployment — e.g.
`https://172.245.43.211:18790`), the native webview validates the cert against
the OS system trust store, the self-signed cert is rejected, and the remote
Panel never loads. Unlike a browser (which offers a "proceed anyway"
interstitial), the native webviews (WKWebView / WebKitGTK / WebView2) give the
user no in-app affordance to accept the cert, and the desktop shell's
reachability supervisor (a TCP-only probe) cannot even detect the failure, so
the user is stuck with a blank/error window.

Requiring the user to manually import + trust the cert in each client's OS
keychain is unacceptably unfriendly. iOS additionally **cannot** add certs to
the system trust store at all.

## Requirements

1. When a Panel app hits a **self-signed / untrusted** cert, it must
   automatically obtain the cert, show the user its fingerprint, and let the
   user authorize it in-app (a Trust-On-First-Use prompt). On approval the
   remote Panel loads normally.
2. **Trigger only on TLS validation failure.** A CA-valid cert (the normal case
   for a **domain** deployment with a real Let's Encrypt/CA cert) passes system
   validation and loads silently — **no prompt, no involvement**. The bare-IP +
   self-signed case is the only one that engages this flow.
3. **TOFU semantics:** first sight → prompt + pin; thereafter → silent; only a
   **changed** fingerprint for a previously-trusted host re-prompts, with a
   prominent possible-MITM warning.
4. **No OS trust-store modification.** Trust is pinned inside the app only
   (revocable, cross-platform-uniform, and the only option iOS supports).
5. Cover all four platforms: macOS, Linux, Windows (desktop lite shell) and iOS.

## Trigger Logic (the "IP vs domain" relationship)

The flow is gated on **TLS validation failure**, not on a string check of
"is this an IP":

- **Bare public IP + self-signed cert** → system trust validation fails →
  engine cert-error hook fires → trust flow engages.
- **Domain + real CA cert** → system trust validation passes → engine
  cert-error hook never fires → webview loads silently.

This falls out naturally from Approach A's **reactive** per-engine hooks: they
only fire on validation failure. Valid certs are never touched.

> Ties back to the SAN auto-discovery fix (2026-07-18, `gateway/tls.rs`): the
> self-signed cert for a bare IP now includes that IP in its SAN, so the sole
> validation failure is "self-signed / untrusted issuer" — never also a
> hostname mismatch. The prompt's failure reason is therefore clean.

## Architecture (Approach A — per-webview in-app cert pinning)

Rejected alternative (Approach B — loopback TLS-terminating proxy): terminates
TLS for *all* connections (would route valid-domain certs through the proxy
too, contradicting Requirement 2), rewrites the webview origin to loopback
(localStorage/device_token keyed to loopback; extra WS-proxy code), and iOS
still needs the native handler regardless. Approach A keeps the real remote
origin, reuses the existing `with_webview` pattern, and has no proxy.

### Shared Core (platform-agnostic, pure logic — R1: no platform APIs)

Lives at `desktop/shell/src/cert_trust/` (Rust) for desktop; iOS gets a small
Swift mirror of the same logic (no Rust-FFI — the iOS Panel is a thin native
Swift app by design).

Three pure units:

1. **Pinned trust store (TOFU persistence)**
   - Maps `host:port → SHA-256 fingerprint (hex)`.
   - Desktop persistence file, namespaced like the existing shell markers:
     `~/.aleph/.desktop-shell-panel-trusted-certs` (lite) /
     `~/.aleph/.desktop-shell-trusted-certs` (full app). Reuses
     `connection::marker_path(name)`.
   - iOS: Keychain (consistent with the existing `ConnectionStore`).
   - Format: JSON (serde on Rust / Codable on Swift) so both ends read the same
     shape.
   - Operations: `lookup(host) -> Option<Fp>`, `insert(host, fp)`,
     `overwrite(host, fp)`. Best-effort load — a corrupt/unreadable store is
     treated as **empty** (fail toward "prompt", never brick, never auto-allow).

2. **Trust decision (pure function)**
   `decide(host, presented_fp, store) -> Decision`
   - host absent from store → `PromptUnknown { fp, san, reason }`
   - host present, fp matches → `Allow`
   - host present, fp differs → `WarnChanged { old_fp, new_fp, san, reason }`

3. **Fingerprint / cert parsing (pure function)**
   - DER leaf cert → SHA-256 hex (grouped colon display form).
   - Parse SAN list, subject, and a human failure reason (e.g. "self-signed /
     untrusted issuer", "expired", "hostname mismatch") for display only. The
     decision pins the fingerprint regardless of the specific failure reason;
     the reason is surfaced to the user for awareness.

The core never calls a platform API and never fetches anything — the cert is
handed to it by the engine hook.

### Per-Platform Adapters

Each adapter: obtain the cert from the engine's cert-error hook → call
`core.decide()` → drive the approval UI and/or allow the connection. All reach
the native webview via the existing `window.with_webview(|pv| ...)` pattern
(`webview_perms.rs` precedent). All allow mechanisms are **per-challenge
temporary grants** — none writes the OS trust store.

| Platform | Hook | Allow mechanism (after approval) | Feasibility |
|---|---|---|---|
| **Linux** WebKitGTK | `connect_load_failed_with_tls_errors(uri, cert, errors) -> bool` | `web_context.allow_tls_certificate_for_host(&cert, host)` then `load_uri(uri)` (reload); return `true` | ✅ `webkit2gtk 2.0.2` exposes it |
| **Windows** WebView2 | `add_ServerCertificateErrorDetected` (defer via `GetDeferral`) | `args.Action = AllowAlways` | ✅ `webview2-com 0.38.2` |
| **iOS** WKWebView | `webView(_:didReceive:completionHandler:)` (ServerTrust; own delegate) | `completion(.useCredential, URLCredential(trust: serverTrust))` | ✅ own Swift delegate, simplest |
| **macOS** WKWebView | same challenge, but **wry owns the WKNavigationDelegate** | same as iOS | ⚠️ **spike**: hook/swizzle wry's delegate; if infeasible, fall back to a macOS-only loopback proxy |

Async prompts: WKWebView and WebView2 natively support suspending the decision
(completion handler / deferral). WebKitGTK's callback is synchronous, so it uses
**fail → prompt → on approve, whitelist + reload** (the reload succeeds because
the cert is now allowed for the host).

**Security guard (all adapters):** there is never a blanket "accept any invalid
cert" mode. Only an exact pinned-fingerprint match is allowed. The adapter's
default path (no decision / user cancel) performs the engine's default
validation → the load fails. **Fail-closed.**

### Approval UX Surface

- **Desktop:** the failed load leaves the webview blank, so navigate it to a
  bundled splash page **`cert-trust.html`** (reuses the `connect.html` native
  splash surface — the explicit R2 exception for connection config). The shell
  stashes the pending cert in state; the page reads it via a Tauri command
  `get_pending_cert()` (fingerprint / SAN / reason — **not** via URL params) and
  decides via `approve_cert(host, fp)` / `reject_cert()`, which update the store
  and trigger the reload.
- **iOS:** a SwiftUI sheet over `PanelWebView` showing the same content
  (fingerprint / SAN / reason + Trust / Cancel).
- **Changed fingerprint (`WarnChanged`):** the same surface with prominent
  warning styling ("证书已变化，可能是服务器轮换或中间人攻击") and an explicit
  re-approval requirement.

Prompt content: server address, SHA-256 fingerprint (grouped colon form), SAN
list, failure reason, Trust / Cancel.

## Data Flow (first connect to a bare-IP self-signed core)

1. Shell navigates webview → `https://IP:port` → TLS validation fails (self-signed).
2. Engine cert-error hook fires → adapter extracts the leaf cert → SHA-256 +
   parse SAN/reason.
3. `core.decide(host, fp, store)` → `PromptUnknown`.
4. Shell stashes the pending cert → navigates webview to `cert-trust.html`
   (desktop) / presents the SwiftUI sheet (iOS).
5. UI renders fingerprint / SAN / reason via `get_pending_cert()`.
6. User clicks Trust → `approve_cert(host, fp)` → `store.insert(host, fp)` →
   reload the remote URL.
7. Reload: the hook fires again → `core.decide` → now `Allow` (fp matches) →
   adapter grants (useCredential / allow_tls_certificate_for_host / AllowAlways)
   → the Panel loads → the (already-fixed) TokenWall appears → the user enters
   the Gateway token.

## Error Handling / Edge Cases

- **User cancels:** do not store; return to the connect screen; no load.
- **Fingerprint changed:** prominent warning; explicit re-approval; on approve,
  overwrite the store entry; on decline, block.
- **Store corrupt/unreadable:** best-effort → treat as empty → every host
  prompts (fail-closed to "prompt", never "auto-allow", never brick).
- **Cert unreadable / parse failure:** show an explicit error + retry/cancel; do
  not auto-allow.
- **Supervisor coordination:** while a trust prompt is pending, a "trust-pending"
  latch suppresses `supervise_remote_lite`'s 40s relocate-to-connect-page (and
  freezes its tick counter) so the user is not yanked off the prompt. Under
  Approach A, once trusted the load succeeds, so the TCP probe no longer
  false-reports "unreachable" — this latch is the only coordination needed.

## Testing

- **Shared core (pure, host-testable, high coverage):** trust-store round-trip
  (insert / lookup / overwrite), `decide()` truth table
  (unknown→prompt, match→allow, mismatch→warn), fingerprint computation
  (known DER → known SHA-256), SAN/reason parsing.
- **Per-platform integration:** requires a webview + a self-signed server →
  **manual / e2e** (against ColoCrossing or a local self-signed test server),
  not unit. The macOS spike carries its own verification.
- **iOS:** Swift unit tests over the decision/store logic (mirrors the Rust
  core; reuses the existing `AlephPaneliOSTests` in-memory pattern).

## Implementation Phasing (one plan, five phases)

1. **Shared Rust core** — trust store + `decide` + fingerprint; pure logic,
   fully unit-tested. The reference for the Swift mirror.
2. **macOS adapter + `cert-trust.html` + wry-delegate spike** — the reference
   desktop platform (primary test environment); prove the whole flow end-to-end
   here before fanning out. If the spike fails, fall back to a macOS-only
   loopback proxy (documented as a contingency, not the default).
3. **Linux adapter** (WebKitGTK).
4. **Windows adapter** (WebView2).
5. **iOS** — Swift mirror of the core + WKWebView delegate adapter + SwiftUI sheet.

Each phase is independently testable; the macOS phase validates the design
before the other platforms replicate it.

## Constraints (carried from the constitution)

- **R1 Brain/Limb separation:** the shared decision core is pure logic; all
  platform-API contact is isolated in the per-platform adapters.
- **R2 Single source of UI truth / connection-config exception:** the trust
  prompt reuses the `connect.html` splash surface, the established native
  connection-config exception — it does not introduce business UI in the native
  shell.
- **Fail-closed security:** never a blanket accept-invalid mode; only exact
  pinned-fingerprint matches; default/cancel = engine default validation = fail.
- **No new async runtime, no new heavy deps:** reuse existing webview engine
  crates (`webkit2gtk`, `webview2-com`, wry via Tauri) and `serde`.
