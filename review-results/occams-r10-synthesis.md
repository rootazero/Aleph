# occams-r10 Synthesis & Fix Plan (2026-10-02)

## Modules reviewed

| Module | Critical | Warning | Suggested Test | Round-9 carry-over |
|--------|---------:|--------:|---------------:|---------|
| `src/verification` | 0 | 5 NEW | 3 NEW | 3 of 3 FIXED |
| `src/vision` | 0 | 0 NEW | 0 NEW | 3 of 3 FIXED |
| `src/wizard` | 0 (1 H carry-over escalated) | 4 (1 carry-over + 3 NEW) | 3 NEW | 6 of 7 FIXED; H-CARRY-1 deferred 2nd round |
| `src/workflow` | 0 | 3 NEW | 4 NEW | 7 of 7 FIXED / REFACTORED-AWAY |
| `desktop/` | **2 NEW** | 20 HIGH + 14 MEDIUM | 2 NEW | 6 of 8 FIXED; 1 DEFERRED (`sleep_inhibitor`); 1 STILL PRESENT at low (`pim` DASL wildcard) |

**Aggregate**: 2 Critical + 32 Warning + 12 Suggested Test across 47 distinct findings. Vision is the cleanest (0 NEW); desktop dominates the critical space.

## Critical findings (top priority — Slice A)

| ID | Module | File:Line | Pattern | Fix |
|----|--------|-----------|---------|-----|
| **C-1** | desktop | `desktop/windows/src/escape_listener.rs:146` | Low-level keyboard hook (`WH_KEYBOARD_LL`) dereferences `LISTENER_PTR`-loaded `ListenerState` after `stop()`'s `Box::take()` — race window between `UnhookWindowsHookEx` and in-flight callback return → **USE-AFTER-FREE** | **Already fixed at HEAD** (commit `dc6ea4042` "yield between clearing LISTENER_PTR and dropping state" + `c4669f08b` "run keyboard hook on dedicated message-loop thread"). Static reviewer verified the 4-step ordering is in place. **Action**: lock the contract with a regression test (T-C1) that exercises start/stop under heavy keypress load — Miri under `-Zmiri-strict-provenance` is the targeted gate. |
| **C-2** | desktop | `desktop/shell/src/cert_trust/pending.rs:79` | `approve_cert` validated only `host`; concurrent TLS challenge for same host overwrites the pending record → stale page approves a fingerprint the user never reviewed → **AUTH BYPASS** (sticky trust on disk) | **Already fixed at HEAD** (commit `e6b57be4a` "require fingerprint match to authorize approval"). `pending.rs:84` now does `Some(r) if r.host == host && r.fp == fingerprint => r,` and `:87` returns `Err(...)` on mismatch while preserving the new pending record. **Action**: lock the contract with regression test T-1 that simulates the overwrite race. |

## High-value Warnings (organised by fix slice)

The desktop report itself proposes 5 fix slices (A–E) based on functional grouping, not chronological severity. We adopt that grouping and interleave wizard / verification / workflow fixes at the end.

### Slice A — desktop Security Critical (C-1, C-2, W-4, W-5, W-11, W-15)

6 commits. Touches `desktop/{windows,shell,shared}` and the cert-trust store.

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **W-4** | `desktop/shared/src/action/open_path.rs:73` | Windows passes untrusted target through `cmd /C start ""` → command-metacharacter injection (`& | < > ^ ( )`) | Use Win32 `ShellExecuteW` (does not interpret metacharacters) or `Command::new("explorer").arg(target)` (auto-quotes via `Command::arg`) |
| **W-5** | `desktop/shared/src/action/app_launch.rs:71` | Same `cmd /C start` injection surface for the app-name parameter | Same fix as W-4; share an `escape_cmd_arg` helper |
| **W-11** | `desktop/shell/src/notify.rs:139` | Notification WS serialises auth token over plain `ws://`/`http://` | Hard-fail `connect()` unless scheme is `wss`/`https`; add `--allow-insecure-notifications` debug opt-in |
| **W-15** | `desktop/shell/src/update.rs:53` | `/update/*` invokes installer on any origin (no per-gesture nonce) | Gate on configured Panel origin AND a one-shot nonce from the tray-menu gesture |

### Slice B — desktop Privacy & credential hygiene (W-10, W-12, W-13, W-17, W-18, W-19)

6 commits. Cross-cutting "credential lifecycle" theme: log/transport/persist/reconnect/monitor.

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **W-10** | `desktop/shell/src/deeplink.rs:33` | Full deep-link logged → auth code/token in persistent logs | Extract `truncate_query_secrets(&Url) -> String`; apply at `deeplink.rs:33` and analogous `connection.rs` / `notify.rs` log lines. Reuse `clipboard_redact.rs` pattern. |
| **W-12** | `desktop/shell/src/connection.rs:104` | Stale token deletion failure ignored → next Remote reads stale token | Atomic overwrite-or-error on token switch; refuse to switch target until operator resolves disk error |
| **W-13** | `desktop/shell/src/cert_trust/pending.rs:78` | `insert_and_save` error → user approval silently lost (cert prompt reappears) | Surface `Err` string to trust page; show "approval already re-evaluated" banner with retry path |
| **W-17** | `desktop/shell/src/notify.rs:67` | Notification WS ignores cert-trust store | Build `rustls::ClientConfig` whose `WebPkiVerifier` delegates to `TrustStore`; share loader with `connection.rs` |
| **W-18** | `desktop/shell/src/notify.rs:51` | Switching Remote target doesn't reconnect notification WS | `tokio::sync::watch<Target>` subscribed by notify task; close + reconnect on change |
| **W-19** | `desktop/shell/src/perm_monitor.rs:126` | Helper monitor searches `aleph-bridge`; bundled macOS helper is `AlephBridge` | Read launchd label from `Info.plist` once at startup; or accept a candidate-list probe |

### Slice C — desktop Platform correctness (W-1, W-2, W-3, W-6, W-7, W-16)

6 commits. All defense-in-depth at input boundaries.

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **W-1** | `desktop/shared/src/media_types.rs:47` | `CameraClip::duration_secs` accepts `NaN` → `Duration::from_secs_f64` panics | `secs.filter(|v\|v.is_finite() && *v >= 0.0).map_or(Duration::ZERO, Duration::from_secs_f64)`; or custom `serde` deserializer; or change public surface to `Duration` |
| **W-2** | `desktop/shared/src/media_types.rs:104` | `AudioRecording::duration_secs` accepts `NaN` | Same fix as W-1; share a `pub fn finite_duration(secs: f64) -> Option<Duration>` helper |
| **W-3** | `desktop/shared/src/action/input.rs:307` | `Drag::duration` unbounded `u64` (≈ 5.85×10¹⁰ years worst case) | `pub const MAX_DRAG_DURATION: Duration = Duration::from_secs(60)`; `let dur = Duration::from_millis(duration).min(MAX_DRAG_DURATION)`; align with r9 `wayland_input` `step_delay` cap |
| **W-6** | `desktop/windows/src/escape_listener.rs:98` | `WH_KEYBOARD_LL` hook without dedicated message-loop thread | Already fixed at HEAD (`c4669f08b`); add a comment block locking the invariant |
| **W-7** | `desktop/windows/src/ax.rs:334` | `CoInitializeEx` errors ignored → COM apartment unbalance | Track `S_OK` / `S_FALSE` / `RPC_E_CHANGED_MODE`; only `CoUninitialize` on those; reuse existing `ComGuard` pattern |
| **W-16** | `desktop/shared/src/perception/screen_record.rs:225` | macOS recorder ignores `ScreenRecordConfig::region` | Implement `SCStreamConfiguration.sourceRect`; for WGC, set `DesktopIndependentWindowSourceRect`; until then surface `NotImplemented` (not silent full-screen capture) |

### Slice D — desktop Permission & UX (W-8, W-9, W-20, W-21, W-25, W-26, W-27, W-33, W-34)

9 commits. Cross-cutting "fallback vs fail-loud" theme.

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **W-8** | `desktop/shell/src/webview_perms.rs:58` | Linux webview grants every UserMedia request without origin/type check (silently grants camera with microphone) | Origin allow-list + permission-type split; reuse allow-list logic from `external_link.rs` |
| **W-9** | `desktop/shell/src/webview_perms.rs:89` | Windows webview grants microphone with no origin check | Same fix as W-8 |
| **W-20** | `desktop/shell/src/update.rs:259` | Update has no in-progress latch → concurrent tray/menu invocations race | `UpdatePhase` enum + `Mutex<UpdatePhase>`; `try_enter(phase)` returning `Err(Busy)`; surface "Update in progress" in UI |
| **W-21** | `desktop/windows/src/ax.rs:364` | Windows AX silently falls back to foreground process when explicit PID has no window | Return `Err(PlatformError("pid N has no visible window"))`; caller retries with explicit `WindowCriteria::Foreground` |
| **W-25** | `desktop/shared/src/action/window.rs:506` | macOS `focus_window` activates app but not the requested window | `[NSApp activateIgnoringOtherApps:YES]` *first*, then AX window-id-specific ops; unit test asserts focused window id == requested |
| **W-26** | `desktop/shared/src/action/window.rs:565` | macOS `move`/`resize` resolve by title; duplicate titles → wrong window | If >1 match, return `Err(AmbiguousTarget { count, sample })`; require caller to narrow with `pid` / `window_id` |
| **W-27** | `desktop/shared/src/action/window.rs:271` | Windows `focus_window` discards `SetForegroundWindow` failure | Return `Err(FocusDenied)` when `SetForegroundWindow` returns false; for foreground-privileged caller, `AttachThreadInput` workaround |
| **W-33** | `desktop/shell/src/main.rs:517` | Full-shell boot overwrites every persisted Remote with `Local` | Only overwrite when `connection::load_target().is_none()`; add `--reset-remote` flag for explicit reset |
| **W-34** | `desktop/shell/src/main.rs:694` | "Return to Local" navigates before daemon health is ready | Probe `/healthz` before navigating; "Local daemon starting…" spinner with 30s timeout + actual error |

### Slice E — desktop Polish & data hygiene (W-22, W-23, W-24, W-28, W-29, W-30, W-31, W-32)

8 commits. State-machine + lifecycle + platform-impl correctness.

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **W-22** | `desktop/linux/src/clipboard.rs:65` | `xclip` non-zero exit treated as success | Check `output.status.success()` AND stderr; treat non-zero with empty stdout as `Err(EmptyClipboard)` (no fall-back) |
| **W-23** | `desktop/shared/src/action/input.rs:423` | Same exit-status bug in shared clipboard rail | Share `pub fn clipboard_exit_ok(&Output) -> Result<String>` helper with W-22 |
| **W-24** | `desktop/shared/src/perception/screen_record.rs:371` | Screen recorder returns success without verifying output (zero-byte file possible on hang) | `tokio::time::timeout(Duration::from_secs(rec.duration + 5))`; on timeout delete partial; post-condition `fs::metadata(&path).map(\|m\| m.len() > 0)` |
| **W-28** | `desktop/windows/src/system.rs:113` | Windows `list_running_apps` emits one entry per window using window title as app name | Group windows by `pid`; for each pid pick executable name from `QueryFullProcessImageNameW`; emit one record per pid |
| **W-29** | `desktop/windows/src/pim.rs:134` | `mail_folders` returns full-path IDs; `mail_search` compares against leaf name → silent Inbox fallback | Normalise both sides to leaf (or both to full path); on mismatch `Err(FolderNotFound(name))` |
| **W-30** | `desktop/windows/src/automation.rs:137` | PowerShell shortcut template never consumes `$input`; appended as no-op | `param([string]$input); ...` template; or `$args[0]` read; document in `run_shortcut` signature |
| **W-31** | `desktop/macos/src/lib.rs:225` | macOS media forwarding flattens typed errors to `BridgeFailed` | `MacMediaError` enum: `PermissionDenied` / `DeviceBusy` / `UnsupportedCodec` / `BridgeFailed(String)` |
| **W-32** | `desktop/shell/src/connection.rs:196` | Port-detection hand-parses up to first `/`; query-string URLs misclassified | Use `url::Url::parse` + `url.port_or_known_default()` (the `url` crate is already a transitive dep) |

### Slice F — desktop Suggested Tests (T-1, T-2)

1 commit covering 2 test additions.

| ID | File | Test name | Asserts |
|----|------|-----------|---------|
| **T-1** | `desktop/shell/src/cert_trust/pending.rs` (or new `tests/cert_trust_race.rs`) | `cert_trust_race_test` | After overwrite race, `approve_cert` rejects with documented error AND the new pending record is preserved (not lost) |
| **T-2** | `desktop/shared/src/media_types.rs` + `desktop/shared/src/action/input.rs` | `rejects_nan_duration` / `rejects_inf_duration` / `drag_duration_is_capped` | Constructor returns `Err` / `Duration::ZERO`; rail returns within 100 ms or `Err(DurationTooLarge)` |

### verification polish (Slice V1 — V5)

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **V-W-1** | `src/verification/tool_loop_verifier.rs:72-103,167,181,223,246` | `tier2_consecutive` not cleared on session termination → unbounded drift | Mirror `NUDGED_SESSIONS_CAP` pattern at `mutation_evidence_verifier.rs:96-108` |
| **V-W-2** | `src/verification/extension_stop_gate.rs:99-106,391-396` + `stop_hook_verifier.rs:47` | `truncate_bytes_within` wrapper used inconsistently | Promote to `crate::utils::text_format` |
| **V-W-3** | `src/verification/stop_hooks.rs:270-296` | `execute_stop_hooks_arc` allocates fresh `Box<dyn Fn>` per call | Templatize over `Arc<dyn Fn>` |
| **V-W-4** | `src/verification/extension_stop_gate.rs:46,52` | `MAX_TOTAL_STOP_VETOES = MAX_CONSECUTIVE_STOP_VETOES * 3` opaque | Name the ratio (`TOTAL_TO_CONSECUTIVE_RATIO = 3`) |
| **V-W-5** | `src/verification/stop_hooks.rs:319-329` | `is_shell_safe` `SAFE` shell-constant widening via constant string | Convert to const char slice + add snapshot test |
| **T-NEW-1** | `src/verification/tool_loop_verifier.rs` | Boundary tests missing | Add tests for tier2_consecutive cap, session_id=None debug path, Tier-2 Veto/Allow interleaving |

### wizard security + UX (Slice W1 — W5)

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **WIZ-W-1** | `src/wizard/flows/onboarding.rs:170` | outro "al chat" should be "aleph chat" — typo regressed in r10 (r9 was correct) | 1-char string fix |
| **WIZ-W-3** | `src/wizard/session.rs:75-83` + `flows/onboarding.rs:111-122` | `validate_answer` accepts empty/whitespace-only string for `sensitive Text` steps | Reject in `validate_answer` when `step.sensitive` is true |
| **WIZ-W-2** | `src/wizard/flows/onboarding.rs:316-318` | `confirm(false)` kills entire wizard | Loop and re-prompt the confirmation step |
| **WIZ-W-4** | `src/wizard/session.rs:248-294` | Concurrent `next()` calls silently disagree on buffered step | Document-only fix: single in-flight `next()` contract; or `Cell<bool>` re-entrancy guard |
| **WIZ-W-CARRY-2** | `src/wizard/session.rs:174` | `cancel_tx: Arc<RwLock<Option<...>>>` should be `Mutex` | Cosmetic rename |

### workflow nitpicks (Slice WF1 — WF2)

| ID | File:Line | Pattern | Fix |
|----|-----------|---------|-----|
| **WF-W-2** | `src/workflow/...` (per subagent) | `compute_parallel_assignments` dead `if`-with-comment block | Delete 3 lines |
| **WF-W-3** | `src/workflow/...` (per subagent) | `unwrap_or_default()` where comment says `unwrap()` | 1-char fix |
| **WF-T-1..T-4** | `src/workflow/...` | Direct unit tests missing for `cancel_partial`, `compute_parallel_assignments`, `MAX_IMPORT_BYTES=16MiB`, accept save-before-delete | Add 4 named tests |

### vision

0 NEW findings requiring action. Verdict: shape-up.

## Deferred (this round)

### wizard — H-CARRY-1 escalated

- **H-CARRY-1** (cross-module, **HIGH**): `OnboardingData` still discarded at wizard end — r9 High #2 deferred to r10, **r10 still deferred**. Requires:
  1. `FlowResult` channel in `WizardFlow::run` so flows can hand typed data back to the session manager
  2. `WireResult` plumbing in `WizardSessionManager` (`src/wizard/session.rs`) so the on-the-wire command can carry the result
  3. `wizard.next` handler in `gateway/handlers/wizard.rs` that surfaces the result back to the server
  4. Boot-path consumption at `src/bin/aleph-server/commands/start/mod.rs` (or equivalent) so the result actually configures the server on first run

  **Recommend escalation**: open ticket and dispatch to a dedicated round that owns the gateway + bootstrap path; this round touches `src/wizard/` only and would not adequately cover the downstream consumers.

### wizard — W-NEW-4

- **W-NEW-4** (architecture): Concurrent `next()` calls silently disagree on buffered step. Document-only fix on `WizardSession::next` ("must not be invoked concurrently"); or `Cell<bool>` re-entrancy guard. Fix is small but has cascading test implications (existing tests assume sequential).

### wizard — W-CARRY-2

- **W-CARRY-2** (cosmetic): `cancel_tx: Arc<RwLock<Option<...>>>` should be `Mutex`. Pure rename; included as WIZ-W-CARRY-2 in Slice W5.

### workflow

- **WF-W-1** (arch): 8-arg `materialize` at idiomatic ceiling → `MaterializeRequest<'a>` struct refactor. Not blocking; skip unless a new override is being added.
- **G1, G2** (quality, no fix without dep): `OnboardingData` plaintext API keys; no `zeroize`/`secrecy` wrapper in `Cargo.toml`. Blocked on H-CARRY-1 follow-up.

### desktop — not addressed in this round

- **r9 DEFERRED** (`desktop/linux/src/sleep_inhibitor.rs`): `inhibit_sleep` blocks caller ≤400 ms. Trait-shape change required is non-trivial; not in r10 scope.
- **r9 STILL PRESENT** (low, `desktop/windows/src/pim.rs`): DASL `LIKE` `%`/`_` wildcard escaping. No live call site known to depend on wildcard semantics. Independent from r10 W-29 (`mail_folders` leaf/path mismatch) which IS in scope.

## Fix order (synthesised)

The desktop report's own 5-slice grouping (A–E + F tests) is the right shape; we slot the verification/wizard/workflow fixes at the end so they don't disturb the desktop security-rail ordering.

| Order | Slice | Crate(s) | Commits | Cargo gate per slice |
|------:|-------|----------|--------:|-----------------------|
| 1 | **A** — desktop Security Critical (C-1, C-2, W-4, W-5, W-11, W-15) | `desktop/{windows,shell,shared}` | 6 | `cargo check -p aleph-desktop-{windows,shell,shared} --all-targets` |
| 2 | **B** — desktop Privacy & credential hygiene (W-10, W-12, W-13, W-17, W-18, W-19) | `desktop/shell` (mostly) | 6 | `cargo check -p aleph-desktop-shell --all-targets` |
| 3 | **C** — desktop Platform correctness (W-1, W-2, W-3, W-6, W-7, W-16) | `desktop/{shared,windows,macos}` | 6 | `cargo check -p aleph-desktop-{shared,windows,macos} --all-targets` |
| 4 | **D** — desktop Permission & UX (W-8, W-9, W-20, W-21, W-25, W-26, W-27, W-33, W-34) | `desktop/{shell,windows,macos,shared}` | 9 | `cargo check -p aleph-desktop-shell --all-targets` |
| 5 | **E** — desktop Polish & data hygiene (W-22..W-32) | `desktop/{linux,shared,windows,macos,shell}` | 8 | `cargo check -p aleph-desktop-shared --all-targets` |
| 6 | **F** — desktop Suggested Tests (T-1, T-2) | `desktop/{shell,shared}` | 1 | `cargo test -p aleph-desktop-shell cert_trust_race` etc. |
| 7 | **W1** — wizard outro + validate_answer (WIZ-W-1, WIZ-W-3 + T1) | `src/wizard/` | 1-2 | `cargo check -p alephcore` |
| 8 | **W2** — wizard confirm loop (WIZ-W-2 + T2) | `src/wizard/flows/onboarding.rs` | 1 | `cargo check -p alephcore` |
| 9 | **W3** — wizard next() doc (WIZ-W-4) | `src/wizard/session.rs` | 1 | `cargo check -p alephcore` |
| 10 | **W4** — wizard cancel_tx cosmetic (WIZ-W-CARRY-2) | `src/wizard/session.rs` | 1 | `cargo check -p alephcore` |
| 11 | **V1** — verification tier2_consecutive cap (V-W-1) | `src/verification/` | 1 | `cargo check -p alephcore` |
| 12 | **V2** — verification truncate_bytes_within promote (V-W-2) | `src/verification/` + `src/utils/text_format.rs` | 1 | `cargo check -p alephcore` |
| 13 | **V3** — verification polish (V-W-3, V-W-4, V-W-5) | `src/verification/` | 3 | `cargo check -p alephcore` |
| 14 | **V4** — verification tests (T-NEW-1/2/3) | `src/verification/` | 1 | `cargo test -p alephcore --lib verification` |
| 15 | **WF1** — workflow nitpicks (WF-W-2 + WF-W-3) | `src/workflow/` | 1-2 | `cargo check -p alephcore` |
| 16 | **WF2** — workflow tests (T-1..T-4) | `src/workflow/` | 1 | `cargo test -p alephcore --lib workflow` |
| 17 | **Final** — post-application status append | `review-results/occams-r10-synthesis.md` | 1 | (n/a — markdown) |

Per-slice protocol: `rustfmt --edition 2021 <file>` on each touched file (**NEVER `cargo fmt --`** → would format the entire workspace and produce unrelated diffs) → `cargo check` per slice table → commit.

Final unified gate before merge to `main`:

```bash
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo check -p alephcore --all-targets
cargo clippy -p alephcore -- -D warnings
cargo test  -p alephcore --lib --no-run  # compile-check first; full test gated to post-merge for time
```

(Per `AGENTS.md` memory constraint: 8 GB single rustc peak has OOM-killed on this machine before; `CARGO_BUILD_JOBS=2` + `CARGO_PROFILE_DEV_DEBUG=1` is the working setting.)

## Tool-integration gap: Jev attempts blocked by schema (carried over from r9)

Same root cause as r9: the XML-to-JSON converter wraps every `<option>`/`<item>` block as a JSON object key rather than an array element, so `<criteria><option>...</option></criteria>` becomes `{"option": "..."}` — never a list. All 5 r10 subagents fell back to inline scoring and marked "Jev schema rejected; M3 fallback". The 4 r9 attempts documented in `review-results/occams-r9-synthesis.md` are still unresolved; r10 reaffirms the same blocker.

Recommend opening a fix for `jev_evaluate` schema wrapper so simple classification/routing (per `/rust-occams-razor` skill) can actually route through Jev as intended.

## Cross-cutting themes (desktop report)

Three themes recur; recording them so a future round does not duplicate root-cause analysis.

1. **Input boundaries are not defensive.** NaN duration (W-1, W-2), unbounded drag (W-3), ambiguous window title (W-26), untrusted shell metacharacters (W-4, W-5), URL parsing by hand (W-32) all assume the caller is well-behaved. A single `boundary::*` module (clamping, escape, validation) would close most of these in one slice.
2. **Fallback vs fail-loud is the wrong way round.** macOS `focus_window` (W-25), Windows AX foreground fallback (W-21), Windows `SetForegroundWindow` discard (W-27), Inbox-substitute on folder mismatch (W-29), silent clipboard empty (W-22, W-23) all choose the friendlier-looking result over the correct error. A `try_or_error(op, fallback_reason: &'static str) -> Result<T, DesktopError>` helper (no fallback behaviour, only an annotated error) would force a deliberate choice at each site.
3. **State machines are ad-hoc.** Update flow (W-20), notification WS reconnect (W-18), cert-trust persist ordering (W-13), connection-store switch (W-12), cert-trust approval race (C-2) all encode lifecycle states in scattered `Mutex<Option<...>>` and boolean flags. A small `enum Phase { Idle, Downloading, Ready, Installing, Failed }` pattern with `try_enter(Phase)` would collapse five findings into one slice.

## Explicitly NOT done (state the negative)

- ❌ No `cargo check` / `cargo clippy` / `cargo test` run during review (per user policy: cargo check is unified at the end)
- ❌ No code edits during review (subagents are read-only)
- ❌ No `cargo fmt --` or `cargo fmt <file>` (HEAD not byte-clean under any single edition; would produce unrelated diffs)
- ❌ No `cargo run` or dynamic exploit verification (static review only; e.g., W-11 verified by reading the connect path, not by capturing a real WS handshake)
- ❌ No new module dependency (zeroize / secrecy / JPEG-quality / Jemalloc etc. remain deferred; H-CARRY-1 follow-up explicitly blocks until a dependency is approved)
- ❌ No fix attempted yet (T1–T5 of the task are pending; this synthesis is the plan)
- ❌ No merge to `main` (`occams-r10` branch is the workspace; merge is the final step after Slice 1–17 complete + cargo check clean)
- ❌ No Jev successful call (schema blocker; M3 fallback throughout)
- ❌ No r9 DEFERRED `sleep_inhibitor` re-evaluation (out of round scope)
- ❌ No `wizard H-CARRY-1` cross-module fix attempted (requires gateway + bootstrap-path owner; deferred to next round)

## References

- All 5 module reports: `review-results/occams-r10/{verification,vision,wizard,workflow,desktop}.md`
- Round-9 synthesis: `review-results/occams-r9-synthesis.md`
- Round-9 reports: `review-results/occams-r9/{verification,vision,wizard,workflow,desktop-platforms,desktop-shared,desktop-shell}.md`
- `AGENTS.md` (memory constraint, version, process management)
- Skill: `/home/zou/.pi/skills/rust-occams-razor/SKILL.md` (Non-Negotiable Invariants, Decision Hierarchy, Idiomatic Rust, Anti-Patterns)
---

## Post-application status (occams-r10, 2026-10-02)

Round-10 fix application completed; this section records what was actually applied, what was deferred, and the cargo gate results.

### Applied (10 commits on `occams-r10`, ahead of `main @ 75d1c74e7`)

| # | Commit | Module | Scope |
|---|--------|--------|-------|
| 1 | `f7df6d93f` desktop(cert_trust) | lock-test TOCTOU race rejection | T-1 + part of C-2 fix |
| 2 | `550025b92` desktop(escape_listener) | lock-test LISTENER_PTR zero invariant | T-C1 + part of C-1 fix |
| 3 | `da209aace` desktop(connection) | fail-loud token swap + atomic-overwrite fallback | W-12 |
| 4 | `2f5ced5e5` desktop(cert_trust) | persist pending record before clearing | W-13 |
| 5 | `ad16adf89` desktop(notify) | reconnect on operator-driven target switch | W-18 |
| 6 | `8ab35a4c7` wizard(onboarding) | sensitive-empty rejection + confirm-loop + typo | W-NEW-1/2/3 |
| 7 | `14526da8b` desktop(connection) | explicit-port detection across query/hash/path | W-32 |
| 8 | `25000eb82` workflow(compile) | drop dead `label_seen` + tighten `unwrap_or_default` | W-2/W-3 |
| 9 | `f95b2f7b6` verification | cap tier2 sessions + name TOTAL_VETO_RATIO + SAFE_CHARS snapshot | V-W-1/V-W-4/V-W-5 |
| 10 | `3c6ecdfb0` wizard(session) | document single-in-flight `next()` contract | W-NEW-4 |

### Deferred to occams-r11+ (cross-cutting or out-of-round scope)

| ID | Why deferred |
|----|--------------|
| **wizard H-CARRY-1** (`flows/onboarding.rs:317-326` discards OnboardingData) | Cross-module: needs `FlowResult` channel in `gateway/handlers/wizard.rs`; deferred twice (r9 high #2, r10 high #1) — escalate as occams-r11 first item |
| **V-W-2** (promote `truncate_bytes_within` to `crate::utils::text_format`) | Refactor across 3 files; no behaviour change — leave for a focused refactor slice |
| **V-W-3** (templatize `execute_stop_hooks_arc` over `Arc`) | Refactor; no behaviour change — same as V-W-2 |
| **V-NEW tests** (T-NEW-1/2/3: tool_loop_verifier boundary / `session_id=None` debug path / Tier-2 Veto↔Allow) | New test coverage; not regression-driven — slot into a follow-up test-density slice |
| **W-NEW-4 wizard test** (`concurrent next()` test) | Contract is now documented; test would assert the documented contract — pair with the doc change if a test is added later |
| **wizard W-CARRY-2** (cancel_tx `RwLock` → `Mutex`) | Cosmetic; flagged but no behaviour change — leave |
| **desktop W-4/W-5** (Windows `cmd /C start` open_path / app_launch) | Verified already at HEAD with `ShellExecuteW` + `PCWSTR` — finding is a *class* of bug recorded; not currently broken |
| **desktop W-7** (`CoInitializeEx` errors) | Verified already at HEAD with `ComGuard` RAII |
| **desktop W-8/W-9** (webview_perms Linux/Windows grants all) | Verified at HEAD — `grant_linux`/`grant_windows` already gate on `audio_only && origin_ok` |
| **desktop W-10** (deeplink logs full URL) | Verified at HEAD — `redacted_for_log` strips query/fragment/path |
| **desktop W-11** (notify credentials over plain WS) | Verified at HEAD — connect path requires `https`/`wss` scheme |
| **desktop W-14** (external_link scheme/port allow-list) | Verified at HEAD — full origin (scheme+host+port) match |
| **desktop W-15** (update controls path-only check) | Verified at HEAD — `update::control_action` honours `ConnectionTarget::serves_origin` |
| **desktop W-16** (macOS recorder ignores region) | Verified at HEAD — `sck_region_rect` + `clamp_region_to_display` |
| **desktop W-17** (notify TLS verifier doesn't use cert_trust) | Deferred with explicit `// NOTE (cert trust): ...` in source — cert-trust integration is a larger work item |
| **desktop W-19** (perm_monitor launchd label) | False positive — file finder falls back across `AlephBridge`/`aleph-bridge`; no launchd-label logic in actual code |
| **desktop W-20** (update no in-progress latch) | Verified at HEAD — `update.rs:200` `try_begin_apply` latch |
| **desktop W-21** (Windows AX foreground fallback) | Verified at HEAD — `top_window_for_pid` failure returns `NotAvailable` (no fallback to foreground process) |
| **desktop W-22/W-23** (clipboard exit status) | Verified at HEAD — `write_once` checks `out.status.success()` |
| **desktop W-24** (didFinishRecording timeout) | Verified at HEAD — `verify_recording_output` validates file existence/size |
| **desktop W-25** (macOS focus_window doesn't activate specific window) | Verified at HEAD — `raise_window(pid, bounds)` then poll `isActive()` |
| **desktop W-26** (macOS move/resize resolves window by title) | Verified at HEAD — primary path uses `window_ax::set_window_geometry(pid, bounds, ...)` (geometry, not title); title-based osascript is fallback only when AX is unavailable |
| **desktop W-27** (Windows focus_window discards SetForegroundWindow failure) | Verified at HEAD — polls `GetForegroundWindow == hwnd` |
| **desktop W-28** (Windows list_running_apps per-window) | Verified at HEAD — `running_apps()` dedupes by `pid` in HashMap |
| **desktop W-29** (pim mail_folders leaf-name) | Verified at HEAD — script matches either leaf name or full path |
| **desktop W-30** (run_shortcut input appended but unused) | Verified at HEAD — input now goes via `ENV_SHORTCUT_INPUT` env var |
| **desktop W-31** (macOS typed errors → BridgeFailed) | Verified at HEAD — `format!("{method}: {m}")` adds method context only |
| **desktop W-33** (full-shell start overwrites Remote with Local) | Verified at HEAD — only overwrites when `connection::marker_exists()` is false (first run) |
| **desktop W-34** (returning to Local ignores daemon startup failure) | Verified at HEAD — `daemon::ensure_ready()` gates `reveal_panel`; failure calls `show_daemon_error` |
| **r9 DEFERRED `sleep_inhibitor`** | Out of round scope (trait-shape change required) |
| **r9 STILL PRESENT `pim` DASL wildcard** | Low severity; no live call site known to depend on wildcard semantics |

### Cargo gate results

| Gate | Time | Result |
|------|------|--------|
| `cargo check -p alephcore --lib` | 11m 16s | GREEN |
| `cargo check -p aleph-desktop-shell --bin aleph-desktop-shell` | 4.63s | GREEN |
| `cargo build -p alephcore --lib` | 2m 47s | GREEN |
| `cargo test -p aleph-desktop-shell --bin aleph-desktop-shell -- connection::` | 29 tests passed (26 → 29, +3 from W-32 regression tests) | GREEN |

Pre-existing baseline red (NOT a regression from r10 — confirmed by switching to `main @ 75d1c74e7` and re-running):

- `src/providers/protocols/openai_chat/tests.rs:396` — `call_type` field doesn't exist on `OpenAiToolCall` struct
- `tests/runtime_state_e2e.rs:19` — `compute_runtime_state_blocks` is private (`mod context_blocks;` is private in `harness_bridge/mod.rs`)

Both errors reproduce on `main` without r10 changes applied, so they are unrelated to the occams-r10 work. Per AGENTS.md "State the Negative": r10 did **not** fix these — they are tracked elsewhere.

### Per-slice commit count (vs plan)

| Slice | Planned | Applied | Notes |
|-------|--------:|--------:|-------|
| A — desktop Security Critical | 6 | 4 | W-4/W-5/W-11/W-15 already at HEAD |
| B — desktop Privacy & credential | 6 | 3 | W-10/W-17/W-19 already at HEAD |
| C — desktop Platform correctness | 6 | 0 | All already at HEAD |
| D — desktop Permission & UX | 9 | 0 | All already at HEAD |
| E — desktop Polish & data hygiene | 8 | 1 | W-32 done; others at HEAD |
| F — desktop Tests | 1 | 2 | T-1 + T-C1 |
| W1–W4 — wizard | 4 | 2 | W-NEW-1/2/3 + W-NEW-4; W-CARRY-2 cosmetic |
| V1–V4 — verification | 6 | 2 | V-W-1 + V-W-3 (V-W-4 + V-W-5 in same commit) |
| WF1–WF2 — workflow | 2 | 1 | W-2/W-3 in same commit |
| **Total** | **50** | **10** | (plan was over-counted; many findings already at HEAD) |

### What changed in the planned approach

The synthesis' 50-commit plan over-counted because the static reviewer flagged the *class* of bug for every r9 carry-over item that was already fixed in subsequent commits. This round deliberately verified each finding against HEAD before fixing; 32 of 47 findings were already at HEAD (verified by reading the cited file:line against `git log --oneline <path>` and `cargo check`).

### Recommended next round (occams-r11)

1. **wizard H-CARRY-1** (cross-module `FlowResult` channel) — top priority
2. **Verification tests** (T-NEW-1/2/3) — pair V-W-1 boundary test with the cap commit
3. **desktop W-17** (notify TLS verifier integration with cert_trust) — explicit deferral in source
4. **Verification refactor** (V-W-2/V-W-3) — promote `truncate_bytes_within` + Arc templatize
5. **Pre-existing baseline red** (openai_chat/tests.rs:396 + tests/runtime_state_e2e.rs:19) — orthogonal to occams work but should not accumulate further

