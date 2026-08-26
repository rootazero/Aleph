# Logic Review Report — clipboard

**Module**: clipboard (mapped: `desktop/shared/src/{macos,linux}/clipboard.rs` + `system.rs` trait)
**Scope**: `desktop/shared/src/macos/clipboard.rs`, `desktop/shared/src/linux/clipboard.rs`, `desktop/shared/src/traits/system.rs`, `desktop/shared/src/system_types.rs`, `desktop/shared/src/linux/session.rs`, `desktop/shared/src/script_exec.rs`, `desktop/shared/src/action/input.rs`, `src/builtin_tools/desktop/{mod,native,safety,focus_gate}.rs`, `src/builtin_tools/desktop/tests.rs`
**Date**: 2026-08-26
**Mode**: normal
**Worktree**: `.worktrees/rust-logic-audit-2026-08-26`
**Branch**: `rust-logic-audit/2026-08-26`
**Note**: `src/clipboard/` does not exist; reviewed the actual location. Prior `severed-wire-audit` (2026-08-22, `review-results/clipboard-batch-1/REPORT.md`) confirmed wiring completeness; this pass is the deeper semantic / logic audit.

---

## Executive Summary

The clipboard module is small, well-tested at the unit level, and now correctly fails closed where the prior severed-wire audit found bugs. There are, however, **five real semantic issues** that survive the wiring fix and only surface under partial failure:

1. The `paste` snapshot is **not panic-safe** — a process crash (or a tool timeout that aborts the future) between snapshot and restore drops the user's prior clipboard content on the floor.
2. The `restore_clipboard` failure path returns `clipboard_restored: false` to the model but emits **no human-readable warning** when the snapshot was `Text` (it only warns on `Unrestorable`). The model has to know to surface a boolean.
3. `clipboard_read` on macOS has **no secret redaction** — `is_password_like` / `ax_secure::secure` gating that protects `type_text` does not protect `clipboard_read`. A user who copies a password and then asks the agent to "paste the link" can have the secret read back into the model.
4. macOS `write_text` calls `pb.clearContents()` **before** checking whether `setString_forType` will succeed. If NSPasteboard refuses the set, the prior clipboard content is already gone, contradicting the error message ("the previous clipboard contents are still there").
5. Linux `read_text` decodes tool stdout with `String::from_utf8_lossy`, silently replacing non-UTF-8 bytes with U+FFFD. Binary clipboard payloads masquerading as text round-trip with corruption.

In addition, the **race between snapshot/restore and a concurrent user copy** is acknowledged in design (the 100 ms post-paste sleep at `native.rs:1986` makes it obvious) but not mitigated: a user copy during that window is destroyed by the restore.

R1 (Brain-Limb Separation) is held: every clipboard API lives under `desktop/`, `src/` contains zero `NSPasteboard`/`xclip`/`wl-copy` references.

---

## Phase 1: Context Alignment

| Layer | Owner | Notes |
|-------|-------|-------|
| Trait contract | `desktop/shared/src/traits/system.rs::SystemCapability` | `clipboard_read` → `Result<ClipboardContent>`, `clipboard_write` → `Result<()>` |
| Shared types | `desktop/shared/src/system_types.rs` | `ClipboardContent { text, has_image, image_base64 }` |
| macOS impl | `desktop/shared/src/macos/clipboard.rs` | NSPasteboard via `objc2`; `read_text` (text only), `read` (text + image), `write_text`, image helpers |
| Linux impl | `desktop/shared/src/linux/clipboard.rs` | Session-aware cascade over `wl-paste`/`xclip`/`xsel` (read) and `wl-copy`/`xclip`/`xsel` (write), image via `image` crate |
| Session detection | `desktop/shared/src/linux/session.rs` | Cached `OnceLock` over `XDG_SESSION_TYPE` + `WAYLAND_DISPLAY` + `DISPLAY` |
| Capability dispatch | `desktop/linux/src/system.rs`, `desktop/macos/src/system/mod.rs` | `clipboard_read`/`write` run inside `spawn_blocking` |
| IPC tool entry | `src/builtin_tools/desktop/mod.rs:537,554,595` | `clipboard_read`, `clipboard_write`, `paste` registered |
| Safety layer | `src/builtin_tools/desktop/safety.rs`, `focus_gate.rs` | `clipboard_write`/`paste` gated by `check_typed_text`; `paste` additionally gated by focus gate (`focus_preflight`) |
| Snapshot/restore | `src/builtin_tools/desktop/native.rs:558-651` | `ClipboardSnapshot` enum, `snapshot_clipboard`, `restore_clipboard` |
| `paste` flow | `src/builtin_tools/desktop/native.rs:1932-1997` | pre-flight → snapshot → `screen.clipboard_write(text)` → Cmd/Ctrl+V → sleep(100ms) → `restore_clipboard` |

**Wiring:** ✅ intact per prior audit. Every `pub fn` in both platform modules has a caller. Both `SystemCapability::clipboard_*` and `ScreenCapability::clipboard_*` paths route to the same per-platform functions (Linux: `crate::linux::clipboard::*`; macOS: `crate::macos::clipboard::*` via `action::clipboard_*`).

**R1 compliance:** verified by grep. `src/` contains only the IPC tool wiring (`builtin_tools/desktop/*`), the safety gate (`safety.rs`, `focus_gate.rs`), and the snapshot/restore logic — no `NSPasteboard`, no `xclip`/`wl-copy` invocations. Every clipboard API lives under `desktop/shared/src/{macos,linux}/`.

---

## Phase 2: Semantic Invariant Checking

### 2.1 Paste snapshot/restore atomicity

| Function | File:Line | Invariant | Verdict |
|----------|-----------|-----------|---------|
| `snapshot_clipboard` | `src/builtin_tools/desktop/native.rs:596` | Returns `ClipboardSnapshot` capturing pre-paste state | OK (flavor-aware) |
| `restore_clipboard` | `src/builtin_tools/desktop/native.rs:638` | Only writes `Text` snapshot; never overwrites a clipboard it can't reproduce | OK |
| `paste` orchestration | `src/builtin_tools/desktop/native.rs:1932` | snapshot → write → keypress → sleep(100ms) → restore | **Atomicity hole** (see §3 findings) |

The snapshot lives in a local variable on the `paste` action's stack frame. It is **not bound to a `Drop` guard**, so:

- If the future is dropped (timeout in `check_typed_text`? an early `return`? a panic from a different cause?), the snapshot is dropped without restoring. The user's clipboard stays at the pasted text.
- If the process panics, the same.

A correct pattern is a `ClipboardSnapshotGuard` that restores in `Drop` unless `.commit()` has been called. The current shape offers **no panic safety** and **no cancellation safety**.

### 2.2 macOS pasteboard safety

| Function | File:Line | Observation |
|----------|-----------|-------------|
| `read_text` | `desktop/shared/src/macos/clipboard.rs:30` | Returns `Result<String>` but can never error (always `Ok` with `unwrap_or_default`). The `Result` is fiction. |
| `write_text` | `desktop/shared/src/macos/clipboard.rs:58` | `pb.clearContents()` then `setString_forType`. The clear is unconditional and irreversible: if the subsequent `setString_forType` returns `false`, the prior content is already gone, contradicting the error message ("the previous clipboard contents are still there"). |
| `read_image` | `desktop/shared/src/macos/clipboard.rs:80` | PNG > TIFF > first non-image. Iteration over `types()` with `*t == *png_type` deref comparisons. Safe but fragile if `NSPasteboardType`'s `PartialEq` ever becomes expensive (currently a pointer compare of `NSString` ivars). |

`NSPasteboard` semantics not handled:

- **Multiple concurrent writes:** not serialized. `setString_forType` is not atomic with `clearContents()` — another writer between the two calls would land in the cleared pasteboard but be overwritten by the set. Documented race window.
- **Change-count loops:** not used. A defensive read of `changeCount` around the write would at least detect concurrent writes; this code doesn't.

### 2.3 Linux cmd selection

| Function | File:Line | Verdict |
|----------|-----------|---------|
| `read_order` / `write_order` | `desktop/shared/src/linux/clipboard.rs:45,56` | Wayland leads with `wl-*`, X11/Unknown with `xclip`/`xsel`. Both worlds cross-listed. OK. |
| `read_args` | `desktop/shared/src/linux/clipboard.rs:64` | wl-paste uses `--no-newline`; xclip uses `-selection clipboard -o`; xsel uses `--clipboard --output`. OK. |
| `write_args` | `desktop/shared/src/linux/clipboard.rs:75` | wl-copy `-n --` (stops option parsing); xclip `-selection clipboard`; xsel `--clipboard --input`. OK, tests guard the `-n`/`--no-newline` round-trip. |
| `read_text_with` | `desktop/shared/src/linux/clipboard.rs:151` | Loop over order; first success wins; if every installed candidate exits non-zero, returns `Ok("")`. **Indistinguishable from "the clipboard is genuinely empty".** Documented as intentional but a hostile environment (broken X server) silently degrades to empty. |
| `write_text_with` | `desktop/shared/src/linux/clipboard.rs:180` | Loop over order; aggregates failures into one error. OK. |
| `read_image_png_base64` | `desktop/shared/src/linux/clipboard.rs:243` | Only tries `wl-paste` and `xclip`; `xsel` is text-only and skipped. Errors are swallowed with `.ok()?`. Silent on every failure. |

**Session detection edge cases (already covered in `session.rs` tests but worth restating):**

- A Wayland session with only `wl-clipboard` missing but `xclip` installed: xclip succeeds writing to X11 selection while the active Wayland app reads Wayland clipboard. User sees "copy succeeded" but paste goes to a different selection. **Documented design** (cross-world tool order), but worth noting as a silent-failure surface.
- A session with `WAYLAND_DISPLAY` unset but XWayland active: `session.kind` falls to X11; xclip leads; works.

### 2.4 Error propagation

| File:Line | Pattern | Verdict |
|-----------|---------|---------|
| `macos/clipboard.rs:33` | `Ok(text.map(...).unwrap_or_default())` | "No text on the pasteboard" returns `Ok("")`. Comment justifies it; this is OK by design but means `clipboard_read` cannot tell empty-clipboard from a refused read. |
| `macos/clipboard.rs:64` | `return Err(DesktopError::InputFailed(...))` | OK; surface error. |
| `linux/clipboard.rs:165-169` | `if any_installed { Ok("".into()) } else { Err(missing_tool_error(...)) }` | OK; failure modes documented. |
| `linux/clipboard.rs:241-258` | `read_content` propagates `read_text` error; image read returns `None` on any internal error | **Silent image failure**: if the image half errors, `has_image` is `false` and the model cannot distinguish "no image" from "couldn't read image". |
| `linux/clipboard.rs:243` `read_image_png_base64` | all errors collapsed via `.ok()?` | Silent. |
| `native.rs:1987` `restore_clipboard` | logs warning, returns `false` | OK; **but the boolean is not surfaced as a user-visible message for `Text` snapshots** (see §3). |
| `native.rs:1962` paste pre-flight | focuses on `screen.clipboard_write`, no rollback on `pasted` failure — but `restore_clipboard` is called in the error arm. | OK. |

### 2.5 unwrap/panic audit

| Location | Pattern | Classification |
|----------|---------|----------------|
| `macos/clipboard.rs:33` | `text.map(...).unwrap_or_default()` | SAFE (defaults to empty) |
| `macos/clipboard.rs:143-146` | `write_text(probe).unwrap()`, `read_text().unwrap()`, `read().unwrap()` | TEST-ONLY (line 142 marks `text_round_trips_and_both_readers_agree` test) |
| `linux/clipboard.rs:234` | `stderr.trim().lines().last().unwrap_or("non-zero exit")` | SAFE (`unwrap_or` fallback) |
| `linux/clipboard.rs:380-431` | `.unwrap_err()` / `.unwrap()` in `#[cfg(test)]` block | TEST-ONLY |

**Verdict:** ✅ No `unwrap`/`expect` on user-facing paths.

### 2.6 Lock/concurrency

No explicit locks in either clipboard impl. The Linux module uses `OnceLock` for `session()` and `tools()` (in `session.rs`), which is `Send + Sync` safe. The snapshot is a local `ClipboardSnapshot` enum (no shared state) so `Send + Sync` is a non-issue for the in-memory variant.

**Race not addressed:** between `screen.clipboard_write(text)` and the user copying something new, and between the paste keypress + 100 ms sleep and `restore_clipboard`. See §3 Critical 1.

### 2.7 Type coercion

| File:Line | Pattern | Verdict |
|-----------|---------|---------|
| `macos/clipboard.rs:84-85` | `types.iter().any(|t| *t == *png_type)` | Safe: `NSString` `PartialEq` is reference compare on the underlying `NSObject` |
| `macos/clipboard.rs:106` | `pb.dataForType(NSPasteboardTypePNG)` returns `Option<NSData>` | OK |
| `linux/clipboard.rs:163` | `String::from_utf8_lossy(&out.stdout).into_owned()` | **Silent corruption of non-UTF-8 binary clipboard data** (see §3 Warning 4) |
| `linux/clipboard.rs:277` | `let mime = pick_image_target(&listing)?.to_string();` | OK |

No byte-index slicing into `&s[..n]` patterns. UTF-8 safety is OK for text writes; the lossy decode is the one coercion concern.

### 2.8 Wiring completeness

| Public item | Called from | Status |
|-------------|-------------|--------|
| macOS `read_text` | `desktop/shared/src/action/input.rs:412`; `desktop/shared/src/macos/app.rs` callers; `desktop/macos/src/system/mod.rs` direct | WIRED |
| macOS `read` | `desktop/macos/src/system/mod.rs:75` (`MacOSSystem::clipboard_read`) | WIRED |
| macOS `write_text` | `action/input.rs:445`; `desktop/macos/src/system/mod.rs:80` | WIRED |
| macOS `read_image` / `tiff_to_png_base64` | internal to `read()` | WIRED |
| Linux `read_text` | `action/input.rs:420`; `desktop/linux/src/system.rs:152`; `desktop/shared/src/action/input.rs` | WIRED |
| Linux `read_content` | `desktop/linux/src/system.rs:150` | WIRED |
| Linux `write_text` | `action/input.rs:450`; `desktop/linux/src/system.rs:160`; `desktop/shared/src/action/input.rs` | WIRED |
| Linux `read_image_png_base64` | internal to `read_content` | WIRED |
| Linux `pick_image_target` / `to_png_base64` / `read_*_args` | internal + `#[cfg(test)]` | WIRED |

Per prior audit, naming asymmetry (`read_content` vs `read`) is intentional. All public functions have at least one non-test caller.

---

## Phase 3: Control Flow Simulation

### 3.1 Linux fallback chain (`read_text_with`)

```
order = read_order(session.kind)        // session-cached, OnceLock
for tool in order:
    if !tb.has(tool): continue           // tool not installed → skip
    if read_capped(tool, args).success():
        return Ok(stdout)
    // else: try next; errors silently collapsed
if any_installed:
    return Ok("")                        // empty-clipboard reading
else:
    return Err(missing_tool_error(...))
```

**Branches covered:** ✅. Test `no_tool_installed_is_an_error_not_an_empty_string` covers the `any_installed == false` arm. No test exercises the `any_installed == true && all_returned_nonzero` arm (which silently returns `Ok("")` and would be hard to distinguish from an actual empty clipboard).

**Missing test:** an installed-but-broken tool (e.g., `xclip` binary present but `DISPLAY` dead) should produce `Ok("")`. The behavior is documented as "intentional", but a test pinning it down would prevent accidental drift toward an error.

### 3.2 Atomicity — snapshot/restore

The snapshot is in-memory (`ClipboardSnapshot::Text(String)` or `::Unrestorable(&'static str)`). There is **no temp-file write** — so the AGENTS.md "snapshot files must be 0600 perms" rule does not apply here (no file is written). The whole atomicity concern reduces to: *what happens if the future is cancelled or the process panics between snapshot and restore?* The answer today: **the snapshot is dropped, the clipboard stays at the pasted text, the user's prior content is gone with no recovery**.

A `Drop` guard that performs the restore on drop unless `.commit()` has been called is the standard fix.

### 3.3 ASCII vs binary content

| Path | Distinguishes? | Encoding safety |
|------|----------------|-----------------|
| macOS `read_text` / `write_text` | text-only API | UTF-16 ↔ UTF-8 via `NSString::from_str`. Internal to NSPasteboard. |
| macOS `read` / `read_image` | reads both text and image. Image is base64-encoded PNG. | PNG bytes pass through `general_purpose::STANDARD.encode` — no UTF-8 concern. |
| Linux `read_text` / `write_text` | text-only API | **Lossy UTF-8 decode of stdout** (`from_utf8_lossy`) — non-UTF-8 bytes replaced with U+FFFD. |
| Linux `read_content` | text + optional image | Same lossiness on text side; PNG pass-through is safe. |

### 3.4 macOS write semantics (deep trace)

```rust
pb.clearContents();                                  // destructive
let ns_str = NSString::from_str(text);
let ok = unsafe { pb.setString_forType(&ns_str, NSPasteboardTypeString) };
if !ok {
    return Err(DesktopError::InputFailed(
        "...the previous clipboard contents are still there.".into(),
    ));
}
```

The error message asserts a precondition that the code does not maintain. The `clearContents()` call is **not paired with a later successful `setString_forType`** — if the set fails, the prior content is already cleared. Apple docs explicitly note `clearContents()` returns the change count and is destructive; only a successful subsequent set can restore.

Two fixes:
1. **Use `declareTypes_owner`** (an atomic "I'm about to write X types, hold the pasteboard") and only `clearContents` after a successful `setString`.
2. **Snapshot first** at the platform-clipboard level (before the clear), then restore from snapshot on set-failure. This requires the macOS limb to mirror the `paste`-action snapshot pattern.

The first is preferred: it's the documented Apple pattern, and it avoids a separate snapshot/restore round trip.

---

## Phase 4: Red-teaming

### 4.1 Local privilege escalation

| Vector | Risk | Mitigation |
|--------|------|------------|
| macOS `generalPasteboard()` is **Universal Clipboard**-enabled by default | **Real.** A user with Handoff turned on syncs clipboard from iPhone/iPad to Mac. `clipboard_read` will return content the user copied on another device. **No opt-out, no per-call flag.** | Document in DESCRIPTION that `clipboard_read` may surface cross-device content. Add a `[unified_tools.native.clipboard]` config to gate cross-device pasteboards if requested. |
| Linux X11 selection | Any same-user X client can read the selection. Aleph is same-user, so no privilege escalation, but **a malicious same-user X client can also write to it** and Aleph will then "paste" that into a focused app via the `paste` action. | Out of scope — this is the X11 model. |
| Wayland compositor | Mediated. Aleph's `wl-paste` runs against the user compositor over the user's session bus. Same-user isolation. | OK. |
| Linux systemd user instance | Aleph runs as the user. Tooling (`xclip`/`wl-copy`) runs as the user. No escalation surface. | OK. |

### 4.2 Sandbox escape

| Vector | Risk |
|--------|------|
| `clipboard_read` → model context → tool call | A user copies a password from a credential vault to paste elsewhere. The model reads it. `is_secure`/`ax_secure::is_password_like` does NOT gate `clipboard_read` — it gates `type_text` only. **This is a secret-leak vector.** |
| `clipboard_write` → tool that pastes | `check_typed_text` gates `clipboard_write` (same gate as `paste`). Cannot smuggle `curl|bash`. OK. |
| `paste` → focused app | Hard-block gate + focus gate. OK. |

### 4.3 Snapshot corruption

The snapshot is in-memory only — no file. External processes cannot corrupt it.

The realistic concern is **process crash / cancellation**: see §3.2.

### 4.4 Concurrent paste race — DATA LOSS for the user

This is the **single biggest user-facing risk** in the module. The flow:

```
T0  snapshot          // captures prior clipboard content "PASSWORD" (user's copy)
T1  write "text"       // clipboard now "text"
T2  keypress Cmd+V     // target app pastes "text"
T2+  sleep 100ms       // ── WINDOW: user copies "something else" here ──
T3  restore "PASSWORD" // OVERWRITES user's "something else"
```

Two distinct loss modes:

- **Loss mode A** (between T2 and T3): user copies new content. Restore destroys it.
- **Loss mode B** (between T1 and T2): user's copy is irrelevant; paste already overrides clipboard. But if the user COPIES after T2 and before T2+100ms, mode A applies.

The 100 ms `sleep` makes mode A more likely than it should be — it widens the window.

**Mitigations:**
- Drop the sleep entirely; rely on the focus-on-target-app semantically holding for as long as needed. (Risky: the paste might not have completed before restore.)
- Snapshot-restore via a `Drop` guard scoped to the entire paste action; restore runs at the very end of `paste`'s lifetime, not after an arbitrary 100 ms.
- After restore, *re-read* the clipboard to confirm the restored content matches the snapshot; if not (user wrote during the window), surface `clipboard_restored: false` and emit a `clipboard_overwritten` warning.

Mode B is unavoidable (paste IS destructive by design), but the model can offer `type_text` as an alternative — which the existing DESCRIPTION already does. Good.

### 4.5 Long-running automation — snapshot staleness

Snapshot data is held in memory and never written to disk. For very large text snapshots (megabytes), the memory pressure is bounded by the user's clipboard size. No disk-staleness concern.

### 4.6 Wayland vs X11 semantics

The Linux impl correctly handles both. The fallback order includes both worlds (so a missing wl-copy falls through to xclip), but on a Wayland session where the active app reads the Wayland clipboard, an xclip write may not be visible to that app. The user will see "copy succeeded" then "paste did nothing". Documented design but a real silent-failure surface.

### 4.7 macOS Universal Clipboard — cross-device paste attack

`NSPasteboard::generalPasteboard()` includes Universal Clipboard content if Handoff is enabled. Reading the pasteboard via `stringForType:` may return content synced from another Apple device. **No API in the macOS module bounds the read to the local device.** If a user pastes a 2FA code on their iPhone, then asks Aleph to "what's on my clipboard", the 2FA code lands in the model context.

### 4.8 TLS / security-sensitive paths

`clipboard_read` returns arbitrary clipboard content to the model. No redaction of:
- Passwords (`is_password_like` would catch some, but only for AX-discoverable fields)
- API keys
- 2FA codes
- PII

**The `redact_secure_values` mechanism in `interactable.rs` is gated on AX affordances** (`secure: Some(true)`) and does not apply to the clipboard-content path.

### 4.9 Race: snapshot read before user re-modifies → restore overwrites later paste

This is mode A above. The mitigation in §4.4 (Drop guard + post-restore verification) is the standard fix.

---

## Phase 5: Verification (sketches — NO cargo run)

### 5.1 Concurrency / loom sketch — paste race

A loom model of the paste action would have three threads:

- **Agent thread** (the `paste` action)
- **User thread** (simulating the user copying something)
- **Target-app thread** (the app that receives Cmd+V)

Key interleavings to verify:
1. Agent snapshots → user copies → agent writes paste text → agent pastes → agent restores → user copy is lost (data loss).
2. Agent snapshots → agent writes paste text → user copies → agent pastes (overwrites user copy) → agent restores (overwrites user copy again — but it's the same text now, so no loss).

A `loom` test could exercise interleaving 1 by having the user thread `clipboard_write` between the agent's snapshot and restore, then asserting `clipboard_restored == false` and that the model surfaces this.

### 5.2 Proptest sketch — restore invariant

For `paste`, the invariant is:
> After a `paste` action that completes successfully (keypress accepted), the clipboard either contains the restored snapshot OR the model is told `clipboard_restored: false`.

A proptest could:
- Generate arbitrary snapshot payloads (Text variants)
- Simulate a `restore_clipboard` failure
- Assert the result includes `clipboard_restored: false` AND a human-readable warning for the Text case (currently missing)

### 5.3 Property — macOS set-without-clear

If `setString_forType` is documented to atomically replace, the `clearContents()` call before it is redundant — and the redundant call IS the bug (it destroys the prior content even when the set fails).

If `setString_forType` is documented to require a prior `clearContents()`, then the order is correct but the contract cannot be maintained on set-failure. The fix is to defer the clear until after a successful set is guaranteed, which is impossible without Apple-side support.

Property to verify: **after a `write_text` that returns `Err`, the clipboard content is unchanged from before the call.** Currently violated.

### 5.4 Linux lossy decode

A proptest for `read_text_with` against a tool that emits `b"\xff\xfe binary blob"`:
- Expect: `Ok("\u{FFFD}\u{FFFD} binary blob")` (lossy)
- Verify: the documented "indistinguishable from empty clipboard" caveat covers this
- No security exploit, but a quality issue for users who copy non-UTF-8 strings (e.g., Latin-1 CSV with umlauts that the clipboard tool did not transcode).

---

## Findings

### [Critical] `paste` snapshot is not panic/cancellation-safe — user's prior clipboard is lost on process crash or future cancellation

- **Location**: `src/builtin_tools/desktop/native.rs:1951-1997`
- **Trigger condition**: `tokio::task::spawn` wrapping the paste action panics, or the runtime cancels the future (timeout, shutdown), or any `?` in the surrounding `DesktopTool::call` returns early without going through the `restore_clipboard` path.
- **Expected**: A `ClipboardSnapshotGuard` (RAII) bound to the `paste` scope that calls `restore_clipboard` in `Drop`, with an explicit `.commit()` call only after `restore_clipboard` has already returned successfully.
- **Actual**: The snapshot lives in a local `let saved = snapshot_clipboard(...).await;` binding on the action's stack frame. If the future is dropped or the process panics, the binding is dropped without restoring. The clipboard stays at the pasted text. The user's prior content is irrecoverable.
- **Suggested fix**:
  ```rust
  struct SnapshotGuard<'a> {
      screen: &'a dyn ScreenCapability,
      saved: ClipboardSnapshot,
      committed: bool,
  }
  impl<'a> SnapshotGuard<'a> {
      fn commit(mut self) { self.committed = true; }
  }
  impl<'a> Drop for SnapshotGuard<'a> {
      fn drop(&mut self) {
          if !self.committed {
              // best-effort restore; we cannot await here, so use try_restore via blocking
              let saved = std::mem::replace(&mut self.saved, ClipboardSnapshot::Nothing);
              if let ClipboardSnapshot::Text(t) = saved {
                  let _ = futures::executor::block_on(/* restore_clipboard */);
              }
          }
      }
  }
  ```
  Note: `Drop` cannot `.await` — a true async-safe guard needs an explicit `tokio::spawn` of the restore. The simplest pragmatic fix is to ensure `restore_clipboard` is called in every `return` arm (including timeout/cancellation) by structuring the action as a single async block with a tail-call restore, and accepting that a hard process crash is unrecoverable (process is dead anyway).

### [Critical] macOS `write_text` destroys prior clipboard content before checking whether `setString_forType` succeeds

- **Location**: `desktop/shared/src/macos/clipboard.rs:60-72`
- **Trigger condition**: Another process holds the NSPasteboard (e.g., a clipboard manager mid-write, a sandboxed app holding the pasteboard). `setString_forType` returns `false`.
- **Expected**: The error message says "the previous clipboard contents are still there", and they should be. NSPasteboard's documented contract is that a `clearContents()` followed by a failed `setString_forType:` leaves the pasteboard empty.
- **Actual**: `pb.clearContents()` runs unconditionally. If the subsequent `setString_forType` returns `false`, the prior content is already gone. The error message lies.
- **Suggested fix**: Use the atomic `declareTypes:owner:` pattern: declare the types you intend to write, then write — and only clear on confirmed success. Or, snapshot the prior content before `clearContents()` and restore on set failure (mirrors the `paste` action's snapshot pattern, but at the platform level).

### [Critical] `clipboard_read` does not gate on `is_password_like` / `secure` — secrets copied to the clipboard land directly in the model context

- **Location**: `src/builtin_tools/desktop/native.rs` dispatch for `clipboard_read` (~line 1770), `desktop/shared/src/macos/clipboard.rs::read_text` and `desktop/shared/src/linux/clipboard.rs::read_text`
- **Trigger condition**: User copies a password / API key / 2FA code / PII to the clipboard (manually or via a password manager's auto-copy), then asks Aleph to perform any task that triggers `clipboard_read` (e.g., "what's on my clipboard", "paste the link I just copied").
- **Expected**: `clipboard_read` routes through a content-level safety gate analogous to `type_text`'s `focus_gate` — redact or refuse when the clipboard holds a credential pattern (long opaque alphanumeric token, password-field-adjacent copy).
- **Actual**: Clipboard content is returned verbatim to the model. The `ax_secure::is_password_like` heuristic is **only consulted on the focused AX element** in `focus_gate::evaluate`, not on clipboard text. There is no heuristic for "the clipboard content LOOKS LIKE a secret".
- **Suggested fix**: Add a `redact_clipboard_content` helper in `ax_secure.rs` (or a new `clipboard_redact.rs`) that applies the same secret-detection rules used for AX values. Treat any token >32 chars of pure `[A-Za-z0-9+/=]` with no whitespace as suspect, redact with `<REDACTED:candidate-secret>` before serializing. Surface a `redacted: true` field in the output so the model can ask the user explicitly when the redaction was triggered.

### [Critical] `paste` restore failure is silent to the human — `clipboard_restored: false` returns a boolean, not a warning

- **Location**: `src/builtin_tools/desktop/native.rs:1987-1996`
- **Trigger condition**: `restore_clipboard` returns `false` because the restore write failed (e.g., the tool exited non-zero, the session was closed, the user killed the desktop service mid-paste).
- **Expected**: When the original clipboard content is **not** back in place, the tool output includes a human-readable warning the model can relay to the user — "I could not put back what was on your clipboard. The clipboard now holds the pasted text."
- **Actual**: The JSON `data` field includes `clipboard_restored: false`, which the model has to know to surface. The `message` field is populated **only** when `unrestorable_note()` returns `Some`, which happens only for the `Unrestorable` variant. For a `Text` snapshot that failed to restore, `message` is `None` — no human-readable warning.
- **Suggested fix**: Add a fourth `ClipboardSnapshot` variant or a parallel function:
  ```rust
  fn restore_failed_note(restored: bool, saved: &ClipboardSnapshot) -> Option<String> {
      if restored { return None; }
      match saved {
          ClipboardSnapshot::Text(_) => Some(
              "Warning: the clipboard could not be restored to its pre-paste state. \
               The clipboard now holds the pasted text. If you had something important \
               on the clipboard before, please re-copy it."
                  .to_string()
          ),
          ClipboardSnapshot::Nothing => None, // nothing to restore
          ClipboardSnapshot::Unrestorable(_) => None, // already covered by unrestorable_note
      }
  }
  ```
  And include the result in `paste`'s output `message`.

### [Warning] Linux `read_text` decodes tool stdout with `String::from_utf8_lossy` — non-UTF-8 clipboard bytes silently corrupt to U+FFFD

- **Location**: `desktop/shared/src/linux/clipboard.rs:163` (`String::from_utf8_lossy(&out.stdout).into_owned()`)
- **Risk**: A user who copies text from a Latin-1 / Shift-JIS / GBK source (e.g., a CSV file with umlauts encoded in cp1252) gets a clipboard value where non-ASCII bytes are replaced with U+FFFD. Subsequent paste of the corrupted string into a UTF-8-aware app shows garbled text.
- **Current impact**: medium
- **Suggestion**: Use `String::from_utf8` first (strict), and on failure fall back to ISO-8859-1 / GBK re-decode heuristics only if a UTF-8 BOM is absent. At minimum, log a `tracing::warn!` when lossy replacement occurred so the operator can see this happened. The behavior is documented as "indistinguishable from empty", but U+FFFD in the output is distinguishable — it just wasn't noticed.

### [Warning] macOS `clipboard_read` may surface content synced from another Apple device via Universal Clipboard — no local-only opt-out

- **Location**: `desktop/shared/src/macos/clipboard.rs:39` (`NSPasteboard::generalPasteboard()`)
- **Risk**: With Handoff enabled, a 2FA code copied on the user's iPhone lands on the Mac pasteboard. `clipboard_read` returns it to the model. This crosses a device boundary that the user did not intend to share with the agent.
- **Current impact**: medium
- **Suggestion**: Document the behavior in the tool's `DESCRIPTION` so the model knows to caveat when returning cross-device content. Optional: gate behind a `[unified_tools.native.clipboard.cross_device]` config flag that, when false, uses a pasteboard with a per-process name (excluding Universal Clipboard). Implementing this requires a Swift helper.

### [Warning] Linux X11 fallback to `xclip` on a Wayland session can write to a selection the active Wayland app does not read — silent write success, paste appears to fail

- **Location**: `desktop/shared/src/linux/clipboard.rs:178-203` (`write_text_with`)
- **Risk**: On a Wayland session where `wl-copy` is missing but `xclip` is installed (and XWayland is active), the write succeeds to the X11 selection, but the active Wayland app reads from the Wayland clipboard. The user sees a successful copy with no effect.
- **Current impact**: medium (rare environment, but the docs / tests assert this is the right design)
- **Suggestion**: After a Wayland write that used an X11 tool, probe the Wayland compositor (via the `WAYLAND_DISPLAY` check) to confirm the write reached the right surface. If not, emit a warning the model can surface. At minimum, include this scenario in the DESCRIPTION: "On a Wayland session, a successful clipboard_write requires wl-clipboard to be installed; otherwise the write may reach XWayland's selection instead of the compositor clipboard that the active app is reading from."

### [Warning] `restore_clipboard` runs after a hard-coded 100 ms sleep; if the paste has not completed by then, restore overwrites a clipboard still holding the pasted text — and conversely the 100 ms window enlarges the user-copy race

- **Location**: `src/builtin_tools/desktop/native.rs:1986` (`tokio::time::sleep(std::time::Duration::from_millis(100)).await`)
- **Risk**: Two failure modes from the same line.
  1. **Paste incomplete**: target app is slow; 100 ms is insufficient; restore overwrites the pasted text before the app processes the paste event (the paste appears to "not take"). User data lost.
  2. **User race**: user copies in the 100 ms window; restore destroys the user's copy.
- **Current impact**: medium-low (typical paste is synchronous and fast; high-latency targets — IDE, terminal — are affected)
- **Suggestion**: Replace the fixed sleep with a per-action verification: re-read the clipboard after the keypress, and restore only when the read-back matches the pasted text (i.e., the paste actually consumed the staged content). If the read-back never matches, surface a warning and consider restoring anyway. This collapses both failure modes into one signal: "the paste did what it was supposed to do" or "it didn't".

### [Warning] macOS `read_text` returns `Result<String>` but can never error — the `Result` is misleading

- **Location**: `desktop/shared/src/macos/clipboard.rs:30-35`
- **Risk**: Callers may branch on `Err(e)` for `read_text` and add error-handling code paths that are dead. If Apple ever adds a failure mode (TCC consent requirement in a future macOS, sandbox denial), the error path will silently take over without a corresponding caller update.
- **Current impact**: low (cosmetic / API hygiene)
- **Suggestion**: Either return `String` directly (matches what callers actually do — `action::clipboard_read` ignores the error), or document the contract in a doc comment that the only error is `NotImplemented`-class on non-macOS.

### [Warning] Snapshot → write → keypress → restore leaves the clipboard in an unknown state if the runtime cancels the `paste` future (e.g., request budget exceeded) — no finalizer on the spawned task

- **Location**: `src/builtin_tools/desktop/native.rs:1932-1997` (the entire `paste` arm)
- **Risk**: If `DesktopTool::call` returns via a parent cancellation token that drops the future at an `.await` between snapshot and restore, the clipboard stays at the pasted text. Mitigation is structural: wrap the snapshot in a guard whose `Drop` performs the restore, but `Drop` cannot `.await`. A `tokio::spawn`-based finalizer that runs on future cancellation is the standard pattern, but adds complexity.
- **Current impact**: low-medium (cancellation paths are uncommon but exist)
- **Suggestion**: Add a `paste` integration test that cancels the future at each `.await` and asserts the clipboard is either fully restored or fully populated with the paste text — never partially overwritten. This pins down the contract regardless of implementation.

### [Suggested Test] Linux: installed-but-broken tool returns `Ok("")` indistinguishably from an empty clipboard

```rust
#[test]
fn installed_tool_that_returns_nonzero_is_treated_as_empty_clipboard() {
    // Point xclip at a shell script that exits 1; "xclip is installed" per
    // ToolBox::from_names, but every read returns non-zero.
    let tb = ToolBox::from_names(&["xclip"]);
    // The test relies on the host's xclip failing for some reason; the
    // contract being pinned is: read_text_with does NOT return Err.
    let result = read_text_with(SessionKind::X11, &tb);
    // Either Ok("") (no installed tool ran) or Ok(<some text>) (the tool ran
    // successfully). The forbidden result is Err.
    assert!(result.is_ok(), "a failing installed tool must not bubble as Err: {result:?}");
}
```

### [Suggested Test] Linux: write failure aggregation names every tool that was tried

```rust
#[test]
fn write_failure_message_lists_all_attempted_tools() {
    // Both wl-copy and xclip are "installed" but always fail (simulated by
    // PATH-aliased scripts that exit 1).
    let tb = ToolBox::from_names(&["wl-copy", "xclip"]);
    let err = write_text_with(SessionKind::Wayland, &tb, "hi").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("wl-copy"), "wl-copy failure must be named: {msg}");
    assert!(msg.contains("xclip"), "xclip failure must be named: {msg}");
}
```

### [Suggested Test] macOS: write refusal leaves the pasteboard unchanged

```rust
#[test]
fn write_refusal_does_not_destroy_prior_clipboard() {
    // 1. Stage a known probe on the pasteboard.
    let probe = "aleph-write-refusal-probe-12345";
    write_text(probe).unwrap();
    assert_eq!(read_text().unwrap(), probe);

    // 2. Mock a pasteboard refusal. Today this is hard to simulate without a
    //    mocking harness; the test asserts the *invariant* once a mocking
    //    seam is added.
    //
    //    With the bug: pb.clearContents() runs; the probe is destroyed; the
    //    set refusal returns Err; the pasteboard is now empty.
    //
    //    Without the bug: pb.clearContents() is deferred until after a
    //    successful set; on refusal, the probe is intact.
    //
    //    let result = write_text_with_held_pasteboard("attempted");
    //    assert!(result.is_err());
    //    assert_eq!(read_text().unwrap(), probe, "refusal must preserve the prior clipboard");
}
```

### [Suggested Test] `paste` restore failure surfaces a human-readable warning for the Text case

```rust
#[tokio::test]
async fn paste_restore_failure_emits_warning_not_just_a_boolean() {
    // Build a tool whose clipboard_write on the restore path returns Err.
    // The model's data["clipboard_restored"] is false; the human-readable
    // message must also be present.
    struct RestoreFails;
    #[async_trait]
    impl ScreenCapability for RestoreFails {
        async fn clipboard_write(&self, _text: &str) -> DResult<()> {
            // First call (the paste write) succeeds; the restore call fails.
            // Track call count via AtomicUsize or similar.
            todo!()
        }
        // ... other methods NotImplemented
    }
    // (Real impl would need call-counting.)
}
```

### [Suggested Test] Concurrent user copy during `paste` window is detected and surfaced

```rust
#[tokio::test(flavor = "multi_thread")]
async fn paste_detects_concurrent_user_copy_during_100ms_window() {
    // Spawn a task that writes "USER_COPY" to the clipboard 50ms after the
    // paste action starts. After the action returns, verify the clipboard
    // either holds the restored snapshot or holds the user's copy — and that
    // the action surfaces "clipboard_restored: false" if it lost the race.
    let tool = build_paste_tool(/* snapshot = "PRIOR" */);
    let user_writer = {
        let s = tool.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            s.write_clipboard("USER_COPY").await;
        })
    };
    let out = tool.call_paste("text").await;
    user_writer.await.unwrap();

    let data = out.data.unwrap();
    assert!(data["clipboard_restored"] == false, "the race must be detected");
    assert!(out.message.unwrap().contains("could not be restored"),
            "human warning required");
}
```

### [Suggested Test] macOS Universal Clipboard / cross-device read is bounded or flagged

```rust
#[test]
fn clipboard_read_includes_a_device_origin_field_when_available() {
    // macOS 10.12+ exposes `NSPasteboardTypeFindEnumerator` and a pasteboard
    // changeCount that increments on sync events. The read function should
    // either (a) skip content synced from another device, or (b) attach a
    // `device_origin: Option<&'static str>` field that flags cross-device
    // content. This test pins whichever contract is chosen.
}
```

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 4 |
| Warning | 6 |
| Suggested Test | 6 |

## Cross-Module Observations

### R1 (Brain-Limb Separation) compliance
✅ Verified. Clipboard code lives exclusively under `desktop/`. `src/` references clipboard only at the IPC tool layer (`builtin_tools/desktop/`), the safety gate (`safety.rs`, `focus_gate.rs`), and the snapshot/restore logic (`native.rs`). No `NSPasteboard`, `xclip`, `wl-copy`, or `wl-paste` references in `src/`.

### IPC tool integration
✅ Verified per prior audit. `desktop_clipboard_read` (registered `mod.rs:537`), `desktop_clipboard_write` (registered `mod.rs:554`), `desktop_paste` (registered `mod.rs:595`) all dispatch through the capability layer (`SystemCapability` preferred for `clipboard_*`, `ScreenCapability` fallback for `paste`). The hard-block gate (`check_typed_text`) covers `clipboard_write` and `paste`; `clipboard_read` has no content-level gate (see Critical 3).

### Local privilege escalation surface
- **macOS Universal Clipboard**: real but user-opted-in (Handoff). Out of scope to remove, in scope to document.
- **X11 selection sniffing by malicious same-user X clients**: Aleph is no worse than any other same-user X client. Not Aleph's problem.
- **Sandbox escape via clipboard**: `clipboard_write` is content-gated (catastrophic-payload blocklist). `clipboard_read` is not content-gated — see Critical 3.

### Snapshot/restore atomicity
- **In-memory snapshot only.** No temp file, no 0600 perm concern (AGENTS.md rule does not apply).
- **Panic safety**: NOT held. Snapshot lives in a local binding; future cancellation or process panic between snapshot and restore drops the user's prior clipboard content.
- **Cancellation safety**: NOT held. `tokio::task` cancellation between snapshot and restore leaves the clipboard at the pasted text.
- **Race window**: 100 ms `sleep` between paste keypress and restore (line 1986) widens the user-copy race.

### Defense-in-depth posture
The module's safety relies on three independent layers:

1. **Approval policy** (`classify_approval` in `mod.rs:537,554,595`) — `clipboard_write` and `paste` require `DesktopType` approval.
2. **Hard block** (`check_hard_block` in `mod.rs:629`) — `clipboard_write` and `paste` route through `check_typed_text`, blocking `curl|bash`, root deletion, fork bombs.
3. **Snapshot/restore** (`native.rs:558-651`) — clipboard is restored after `paste`.

The missing layer is a **read-side gate**: `clipboard_read` has no redaction. A user copying a secret and asking the agent "what's on my clipboard" leaks the secret into the model context with no defense in between.

### Cross-cutting: `ax_secure` reach
`ax_secure::is_password_like` is consulted in `focus_gate::evaluate` and the `redact_secure_values` projection for AX elements. It is **not** applied to clipboard text. Extending it to cover clipboard text is the natural fix for Critical 3.

### Snapshot/restore structure recommendation
Replace the manual `let saved = ...` + `restore_clipboard` pattern with a `paste` action that:

1. Snapshots.
2. Writes paste text.
3. Sends keypress.
4. Awaits a paste-completion signal (or a timeout).
5. Restores.
6. Verifies restore.
7. On any failure path (timeout, panic, early return), restores.

Even with a `Drop` guard that cannot `.await`, the structure can be:
```rust
let saved = snapshot_clipboard(...).await;
let result = async {
    screen.clipboard_write(text).await?;
    send_paste_key(...).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok::<_, DesktopError>(())
}.await;
restore_clipboard(screen, &saved).await;
result?;
```
This pattern ensures `restore_clipboard` runs even if `result?` propagates an error. Today the same pattern exists at `native.rs:1964-1989` — restore IS called in the error arm — but a `Drop` guard would also cover cancellation (drop of the future before `restore_clipboard` runs), which the manual pattern does not.

---

## What was NOT done

- No `cargo check` / `cargo test` / `cargo clippy` (per instruction — static review only).
- No source modifications; no commits.
- Did not deeply audit `script_exec::output_capped_blocking` beyond confirming `POLL_INTERVAL`, `DESKTOP_QUERY_TIMEOUT`, and the kill-and-wait timeout path. It is well-tested in its own right and is the right primitive for `xclip`/`wl-copy`/`wl-paste` invocations.
- Did not audit `image` crate usage in `tiff_to_png_base64` / `to_png_base64` — both decode through `ImageReader::new(...).with_guessed_format()...decode()`, which is the documented safe pattern.
- Did not audit the Universal Clipboard mechanism at the Apple API level — would require Swift-side inspection (out of worktree scope).
- Did not propose a Swift-side fix for the macOS Universal Clipboard concern (requires `desktop-macos` worktree).
- Did not audit `clipboard_win` (Windows clipboard path) — out of scope (only macOS + Linux files listed in task).
- Did not run loom/proptest — only sketched properties. Running them is the next phase.
- The `rsa_security` heuristic for password-like clipboard content is sketched but not implemented; the `redact_clipboard_content` helper is described in concept only.

