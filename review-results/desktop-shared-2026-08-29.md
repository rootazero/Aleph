# Module: desktop-shared (round 1)

- **Path**: `desktop/shared/` (capability contracts + cross-platform traits + Linux/macOS/Windows perception/action code paths)
- **Worktree**: `.worktrees/review-2026-08-29`
- **Branch**: `review/desktop-interfaces-shared-2026-08-29`
- **Files scanned**: 60 `.rs` files (under `desktop/shared/src/`)
- **Total LOC**: ~24 600

## Summary

| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 1 |
| medium   | 2 |
| low      | 0 |
| **Total**| **3** |

## R1 / R7 / R9 Verification

- **R1 (Brain-Limb Separation)**: `desktop/shared` defines TRAITS only at the top level (`traits/{automation,ax,media,permission,pim,power,screen,system}.rs`). The platform-specific code paths under `desktop/shared/src/linux/` and `desktop/shared/src/macos/` are SHARED capability code gated by `#[cfg(target_os = "...")]` — they exist inside the shared crate because they are required to build the Linux/macOS shared lib, but they never call platform APIs without the cfg gate. **PASS.**
- **R7 (One core, many shells)**: all three platform impls (`desktop/linux`, `desktop/macos`, `desktop/windows`) provide the trait methods required by the shared contract. Verified by cross-referencing trait method list against each platform's `impl DesktopPlatform for XxxPlatform`. **PASS.**
- **R9 (All configurability exposed as tools)**: permission/approval flows are reachable from `shared/client` (gateway-callable), not hidden behind shell-only UI. **PASS.**

## High-Confidence Issues

### [High] `wayland_input::drag` ignores the `step_delay` returned by `drag_path`, so drags fire as fast as ydotool can dispatch
- **Location**: `desktop/shared/src/action/wayland_input.rs:421-426`
- **Description**: `drag_path(...)` returns `(path, step_delay)` — a tuple of `(Vec<(i32, i32)>, Duration)`. The ydotool rail extracted only `.0` (the path) and called `run_ydotool(&mousemove_args(...))` in a tight loop, never `sleep`-ing between successive mousemove events. The intent of `step_delay` is to throttle drag motion so receiving applications see a coherent drag gesture rather than a teleportation (which most X11/Wayland compositors interpret as "no drag happened").
- **Trigger**: any drag on a Linux desktop via the wayland-input rail (e.g. `desktop.shared.action.input.drag` invoked by an agent); ydotool may or may not enforce its own throttling internally, but the *guarantee* `drag_path` documents is now honoured.
- **Expected**: each step along the path is delayed by `step_delay`.
- **Actual**: zero inter-step delay; the second issue is that this only affected the Linux ydotool rail — the other rails in `super::input` consume `step_delay` correctly.
- **Fix applied**: destructure `(path, step_delay)` and `std::thread::sleep(step_delay)` between mousemove calls. See commit `desktop-shared: review round 1 findings (1 high, 2 medium)`.

### [Medium] Two screen recordings started in the same millisecond silently overwrite each other
- **Location**: `desktop/shared/src/perception/screen_record.rs:289-291` (before fix)
- **Description**: `screen_record_output_path` joins `format!("screen_record_{ts}.mp4")` where `ts` is millisecond-resolution since the UNIX epoch. Two concurrent calls (e.g. an automated test running two parallel captures, or a user clicking the panel record button twice within the same millisecond) race on the same filename; whichever finishes second truncates the first.
- **Trigger**: any code path that calls `screen_record(...)` concurrently from two threads/tasks. Realistic on test rigs (`tests/screen_record_e2e.rs` historically runs two captures back-to-back) and on the macOS recorder where the SCK pipeline accepts overlapping captures.
- **Expected**: each call writes to a distinct path.
- **Actual**: second call's `rename` overwrites the first's recording silently.
- **Fix applied**: append a process-local `AtomicU64` counter (`screen_record_{ts}_{counter}.mp4`). Counter resets per process, so each call within the same process still gets a unique path; the millisecond component is preserved for human-readable timestamps.

### [Medium] `process_screenshot_with_scale` passes caller-controlled JPEG quality directly to `image::JpegEncoder`, which panics on out-of-range values
- **Location**: `desktop/shared/src/perception/screenshot.rs:540-548` (before fix)
- **Description**: `image::JpegEncoder::new(...).encode(...)` validates `quality ∈ [1, 100]`. The helper currently forwards whatever the caller passed (LLM-controlled or tool-controlled) without clamping. A value of 0 or 101+ panics inside the encoder.
- **Trigger**: an LLM that proposes `quality=0` to "save the most space"; a future feature flag that lets the user pick quality; a tool definition that passes an out-of-range scalar.
- **Expected**: invalid quality is clamped or rejected.
- **Actual**: panic on out-of-range input.
- **Fix applied**: `let quality = quality.clamp(1, 100);` before the encode.

## Per-perspective findings (lower confidence)

### Security
- The Linux Polkit-driven permission rail in `permissions_types.rs` and the macOS TCC bridge in `desktop/shared/src/macos/mod.rs` correctly forward-only the platform permission result; no observed leakage of internal preflight state. The `clipboard_redact.rs` module already redacts well-known credential prefixes before exposing clipboard contents.
- `script_exec.rs` runs shell scripts via `tokio::process::Command` with `kill_on_drop(true)`. It does NOT sanitise script content, but it is gated behind the existing `exec` approval flow — flagged as a defense-in-depth concern only; no fix in this round because the existing approval gate is the documented contract.

### Logic
- The cross-platform `coord.rs` and `native_screen.rs` correctly handle the multi-monitor geometry with the OS-native coordinate origin convention. No observed off-by-one in the bounds calculations.
- The Linux `wayland_input` fix above is the only drag-path correctness gap found in this round; other rails (`x11rb` direct, `xdotool` shell-out) consume `step_delay` correctly.

### Architecture (R1-R10)
- **R1**: every platform call site under `desktop/shared/src/{linux,macos,windows}/` is `#[cfg(target_os = "...")]`-gated. Cross-platform crates (e.g. `xcap`, `enigo`, `image`) are the only non-cfg-gated paths, which is the documented contract.
- **R3**: `desktop/shared` pulls in `xcap 0.8`, `enigo 0.3`, `x11rb 0.13`, `clipboard-win 5`, plus a long `windows = "0.58"` feature list and the macOS `objc2`/`core-graphics` toolchain. Each is load-bearing for the platform it targets; no observed dead deps.
- **R7**: every `pub fn` in `desktop/shared/src/{traits,action,perception}/` has either a `desktop/{linux,macos,windows}/src/` caller OR is itself called from `desktop/shell/`. Cross-checked by `grep -r 'use aleph_desktop::' desktop/{linux,macos,windows,shell}` — no orphan signatures.
- **R8 / R10**: no regex in this module beyond `clipboard_redact.rs` (which is on a machine format: well-known credential prefixes).

### Quality
- The cross-platform module docs at `traits/mod.rs` and `perception/mod.rs` correctly enumerate the platform-availability matrix for each capability, so a future maintainer does not have to grep the codebase to know whether `screen_record_wgc` is available on Linux.
- Test coverage is heavy on the perception module (`screenshot.rs` 14 unit tests, `screen_record.rs` 4 integration tests, `ocr_*` per-platform); light on the action module (`action/open_path.rs`, `app_launch.rs` are not unit-tested but are platform-impl-exercised through `desktop/{linux,macos,windows}/tests/`).
- The fix to `wayland_input::drag` is a 2-line change with no behaviour change for any caller that does not depend on a specific drag-step cadence (the documented default in `drag_path` is 16 ms, which matches what users expect from a real mouse drag).

## Conclusion

`desktop/shared/` is in good shape. The 1 high and 2 medium findings are all narrow, with concrete root causes and minimal fixes applied. R1/R7/R9 all pass. No new deps required. The remaining risk surface — exec approval flow, command injection in `open_path` / `app_launch` on Windows — is owned by the platform-impl crates (reviewed in `desktop-platforms-2026-08-29.md` and prior rounds) and is not duplicated here.

## Commit

```text
desktop-shared: review round 1 findings (1 high, 2 medium)
audit: review report for desktop-shared (round 1)
```