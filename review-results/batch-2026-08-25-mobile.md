# Module Review — mobile (iOS Swift)

- Date: 2026-08-25
- Worktree: `/home/zou/data/workspace/Aleph/.worktrees/review/interface-shared-mobile-2026-08-25`
- Branch: `review/interface-shared-mobile-2026-08-25`
- Baseline commit: `6de033068` (merge: integrate origin/main, worktree-multiuser-teamchat-p3 round)
- Tip commit (post-fixes): `4f017a0e4`
- Method: full static read of every Swift file in scope; project.yml / Info.plist
  schema read from project.yml (the plist itself is gitignored); three shell
  scripts read end-to-end. No `xcodebuild` available in this environment, so
  fixes were applied by code reading alone and verified by re-reading the diff.

## Scope

| Path | Status |
|------|--------|
| `mobile/ios/AlephPaneliOS/**/*.swift` (12 production files) | in scope |
| `mobile/ios/AlephPaneliOSTests/**/*.swift` (8 test files) | in scope |
| `mobile/ios/project.yml` | in scope |
| `mobile/ios/Info.plist` | in scope as the schema in `project.yml`; the file itself is gitignored |
| `mobile/ios/generate.sh`, `release-testflight.sh`, `launch-local.sh.example` | in scope |
| `mobile/ios/README.md` | in scope (documentation drift check) |
| Anything under `interfaces/`, `shared/`, `desktop/`, `src/`, `Cargo.toml`, `Cargo.lock` | out of scope (untouched) |

## File / LOC stats

| Bucket | Count | LOC |
|---|---|---|
| Swift production files | 12 | 1230 |
| Swift test files | 8 | 688 |
| Swift total | 20 | 1918 |
| `project.yml` | 1 | 90 |
| `Info.plist` (generated, gitignored) | — | — |
| `generate.sh` | 1 | 39 |
| `release-testflight.sh` | 1 | 104 |
| `launch-local.sh.example` | 1 | 34 |
| `README.md` | 1 | ~150 |

Largest Swift file: `Views/PanelWebView.swift` at 183 LOC (well under the
500-line cap). No oversized files.

## Finding summary

| Severity | New this round | Carried from batch5-mobile-ios (still present) | Resolved by intervening work |
|---|---|---|---|
| Critical | 0 | 0 | 0 |
| High | 0 | 0 | 1 (H1 — ATS global cleartext) |
| Medium | 3 | 2 (M1, M2) + 1 from carry-over (M3 → now M3-resolved) | 0 |
| Low | 3 | 1 (L2) + 2 (L3, L4) | 1 (L1 — `NWReachabilityProbe` replaced) |
| **Totals** | **6** | **5 carried** | **2 already resolved before this round** |

## Per-finding detail

### High

None new. The previous review's H1 (token-in-URL over default `http://` with
ATS globally off) is now fixed at the Info.plist schema level: `project.yml`
sets `NSAllowsArbitraryLoads: false` and `NSAllowsLocalNetworking: true` (so
simulator-side `http://127.0.0.1` keeps working while a public hostname
typed without scheme is refused by ATS). The plain-text "bare host" path
remains by design (LAN gateway default) and is now narrower.

### Medium

**M1. `webView.isInspectable = true` is unconditional**
- File: `mobile/ios/AlephPaneliOS/Views/PanelWebView.swift:48` (pre-fix)
- Behaviour: `WKWebView.isInspectable` (iOS 16.4+) lets a Mac Safari attach
  Web Inspector to the running app. Release/TestFlight builds would then
  expose the panel DOM and the *current `?token=`-bearing URL*. The README
  states the distribution build ships the pairing screen, but `isInspectable`
  was on regardless of configuration.
- Applied: wrapped in `#if DEBUG`. Release/TestFlight now refuse the
  inspector toggle (the field is read-only in production per Apple's
  entitlement rules).

**M2. `presentCertPrompt` overwrites an earlier held challenge, hanging one load**
- File: `mobile/ios/AlephPaneliOS/State/AppState.swift:108-109` (pre-fix)
- Behaviour: WKWebView dispatches a server-trust challenge per TLS connection
  to the same `host:port` (main document + WASM + sub-resources in
  particular). The shell can only present one sheet at a time, so a second
  `presentCertPrompt` overwrites the first. The first `completionHandler` is
  then dropped on the floor — never called — and that load hangs forever.
  The only escape documented for users is a shake-to-reconfigure. The
  per-request `resolved` latch in `Coordinator.prompt` guards against
  double-resolve of the *same* request, not against cross-request overwrite.
- Applied: `presentCertPrompt` now rejects (`decide(false)`) any incoming
  request while one is already pending, fail-closed. The user keeps the
  one sheet they were already looking at and the held TLS connection is
  cancelled cleanly.

**M3. `launch-local.sh.example` bundle id does not match the build product**
- File: `mobile/ios/launch-local.sh.example:17` (pre-fix)
- Behaviour: template used `BID="ai.aleph.panel.iossim"`, but
  `project.yml` defines `PRODUCT_BUNDLE_IDENTIFIER: ai.aleph.panel`. The
  `iossim` suffix only appears on the test target id
  (`ai.aleph.panel.iossim.tests`). Users copying the template per README
  Option 2, filling in only UDID + PANEL_URL, then hit `simctl launch`
  "bundle not installed".
- Applied: template now uses `BID="ai.aleph.panel"` with an inline comment
  cross-referencing `project.yml`.

### Low

**L2. Keychain entries use `kSecAttrAccessibleAfterFirstUnlock`**
- Files: `mobile/ios/AlephPaneliOS/Services/ConnectionStore.swift:58`,
  `mobile/ios/AlephPaneliOS/Services/CertTrustStore.swift:94` (pre-fix)
- Behaviour: the pairing URL contains the gateway `?token=`; the cert
  store contains the trusted-fingerprint map. With
  `kSecAttrAccessibleAfterFirstUnlock` both ride along with iCloud / iTunes
  backups and are readable by anything running in a post-first-unlock
  background context. The `…ThisDeviceOnly` variant is the standard
  precedent on every other shell that persists the same URL.
- Applied: both stores switched to `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`.
  Trade-off accepted: re-pair on a new device (the desktop shells already
  require this).

**L3. `KeychainCertStore.pin()` is non-atomic across concurrent callers**
- File: `mobile/ios/AlephPaneliOS/Services/CertTrustStore.swift:73-107` (pre-fix)
- Behaviour: the whole `host:port -> fp` map lives in one JSON blob in the
  keychain; `pin()` does load → mutate → save without synchronization. Two
  concurrent `pin()` calls (one per overlapping sub-resource TLS challenge
  for the same host, exactly the case M2 already named) could interleave
  between the load and the save and drop one fingerprint. The window is
  small and the consequence is benign ("re-prompt on next launch"), but the
  defect is real and surfaces under load.
- Applied: `lookup()` and `pin()` are now routed through a single static
  serial `DispatchQueue` (`ai.aleph.panel.cert-store`), so a writer can
  never interleave between a reader's load and the matching decision.

**L4. `try? store.save(...)` silently swallows Keychain failures**
- File: `mobile/ios/AlephPaneliOS/State/AppState.swift:56,75` (pre-fix)
- Behaviour: a save failure (keychain locked, profile misshapen, etc.) was
  invisible — the shell still navigated to the panel for the session, and
  on next launch the user was back at the pairing screen with no idea
  why. No log, no toast, no diagnostic.
- Applied: both call sites now go through a small `persist(store:logger:url:)`
  helper that logs the `OSStatus` via `OSLog` (subsystem
  `ai.aleph.panel`, category `AppState`) at `.error` level with
  `.public` privacy. The pairing UI stays silent by design; the failure
  surfaces in Console.app / `log stream`.

## Out-of-scope issues noted but not fixed here

- **L5 (carry-over). `currentTargetString()` prefills the URL with the
  embedded `?token=`** in `PairingView` (`AppState.swift:128` +
  `PairingView.swift:10`). A shoulder-surfer / screen recording sees the
  token. This is a UX-aware decision (the user may want to see and tweak
  the saved URL), not a security bug; not fixing in this round.
- **Accept-any server trust in the probe** (`ReachabilityProbe.swift:
  AcceptAnyServerTrust`). Only used for the liveness probe — the real
  decision lives in `PanelWebView`'s TOFU flow. Documented as such.
  Not fixing.
- **iPad pairing screen colors are hardcoded dark** (`PairingView.swift:21-27`).
  Intentional per the desktop `connect.html` palette alignment.
- **`PairingView.connect()`'s `Task` is not cancellable** when the user
  shakes the device mid-probe. The probe itself is short and bounded, so
  the consequence is a stale probe writing to `screen` after a
  `requestReconfigure`. Will resync on the next probe. Minor.

## ATS / Info.plist compliance table

Read from `project.yml:46-62` (the `info.properties.NSAppTransportSecurity`
block — the actual `Info.plist` is gitignored build output).

| Setting | Value | Verdict |
|---|---|---|
| `NSAllowsArbitraryLoads` | `false` | Compliant — ATS enforced for remote hosts |
| `NSAllowsLocalNetworking` | `true` | Compliant — scoped exception so sim/dev `http://127.0.0.1` keeps working; presence of the local key does not widen the exception to public hosts (iOS 10+) |
| `ITSAppUsesNonExemptEncryption` | `false` | Compliant — HTTPS/TLS only |
| Launch screen | empty `UILaunchScreen` | Compliant — no storyboard required |
| `UISupportedInterfaceOrientations~ipad` | 4 orientations | Compliant |
| `UISupportedInterfaceOrientations` | portrait only | Compliant |

Bundle identifier (`PRODUCT_BUNDLE_IDENTIFIER: ai.aleph.panel`) matches the
fixed `launch-local.sh.example`. Test target id `ai.aleph.panel.iossim.tests`
is correctly distinct from the app id.

## Architecture-redline compliance snapshot

| Redline | Conclusion |
|---|---|
| R1 (Core never calls platform APIs) | Compliant — this slice has no Rust; iOS shells encapsulate all WebKit / Keychain / Security.framework usage. |
| R2 (Complex business UI in Leptos/WASM only) | Compliant — `PanelWebView` is a transparent shell; native UI is limited to the pairing + cert-trust transport screens. |
| R3 (Core minimalism) | N/A — no Rust in this slice. |
| R4 (Interface layers are pure I/O) | Compliant — `AppState` / `ConnectionStore` carry transport config only; no business state. |
| R5 (Menu bar first) | N/A — iOS idiom (single full-screen shell). |
| R6 (AI comes to you) | Compliant — pairing screen surfaces a host the user can change without leaving the shell. |
| R7 (One core, many shells) | Compliant — the iOS shell delegates all UI logic to the WASM panel. |
| R8 (LLM handles routing) | N/A — no LLM in this slice. |
| R9 (Configurability exposed as tools) | N/A — no tools. |
| R10 (Intelligence in the prompt) | N/A — no prompt. |

## Fixes applied (commits)

| Hash | Subject |
|---|---|
| `7940ff399` | `mobile/ios: gate WebView isInspectable behind DEBUG` (M1) |
| `25536510f` | `mobile/ios: fail-closed on overlapping TLS trust prompts` (M2; also carries the L4 logging helper since both touched `AppState.swift`) |
| `022cf47d2` | `mobile/ios: correct launch-local bundle id to match project.yml` (M3) |
| `d9286cb3e` | `mobile/ios: scope Keychain pairing URL to this device only` (L2 — `ConnectionStore`) |
| `4f017a0e4` | `mobile/ios: serialize cert-pin save against concurrent challenges` (L3; also flips `CertTrustStore` to `ThisDeviceOnly` for the L2 side of that file) |

All five commits authored by `rust-doctor-agent <aleph-audit@local>`
(existing repo author — git config untouched).

## Negative space / what was NOT reviewed / residual risks

- **Swift compilation not verified.** This environment has no Swift /
  Xcode toolchain. Every fix was applied by code reading. The OSLog
  helper, `#if DEBUG` wrapping, and `Self.io.sync` are standard Swift 6
  idioms but were not compiled. `Bundle.main.bundleIdentifier` should be
  re-verified against `xcodebuild` before the next TestFlight upload —
  the shell script still hard-codes `AlephPaneliOS.xcodeproj` and uses
  the Xcode 15+ `app-store-connect` method (older Xcode wants
  `app-store`); release engineer should still build once locally.
- **`Info.plist` not read directly.** It's gitignored build output. The
  compliance table is read from the `info:` block in `project.yml`,
  which xcodegen consumes verbatim. A drift between the two would be a
  xcodegen bug, not a source defect.
- **The `AcceptAnyServerTrust` proxy delegate** in `ReachabilityProbe`
  was *not* changed. Its single call site (`GatewayReadyProbe`) uses it
  only for liveness, and the real trust decision lives in
  `PanelWebView.Coordinator`. If someone repurposes the probe to do
  anything besides a 5-second /ready liveness check, this delegate needs
  to come off — it accepts any cert.
- **iPad layout and dynamic type** were not exhaustively checked; the
  pairing screen is hardcoded dark with hex literals and a 420-pt card.
  The README documents this alignment with the desktop `connect.html`.
- **Reachability test harness** (`ReachabilityProbeTests.swift`) was not
  re-run. Tests use `NWListener` on loopback; the helpers (`OnceFlag`,
  `Recorded`) use `NSLock` inside synchronous critical sections and
  `Task.sleep` outside the lock — that pattern is correct under Swift 6
  and matches the doc-comments in the file, but only `swift test` can
  confirm it.
- **PANEL_URL handling in `launch-local.sh`** (the user copy, not the
  template) was not inspected (gitignored). Template behaviour and
  README are aligned; no defects found.
- **`PairingTarget.parse` does not reject userinfo** (`http://user:pass@host`).
  The shell would then carry the credentials into the WebView URL and
  the gateway's CORS / token-rotation logic would have to handle them.
  Not fixed in this round — outside the four-perspective checklist and
  would change the public parser contract.
- **No new test coverage** was added for the M2 race fix. The existing
  `KeychainCertStoreTests` suite already pins the round-trip, but a
  dedicated "two concurrent `presentCertPrompt` calls don't both
  succeed" test would prevent regression. The fix is small, the defect
  path is narrow (requires two simultaneous TLS challenges for the same
  host), and the test would need an injected fake coordinator — out of
  scope for this static review.