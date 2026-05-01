# macOS Module Static Code Review — 2026-05-01

**Scope**: `desktop/macos/` (Rust + Swift bridge)
**Mode**: Static review, safe auto-fixes applied directly to `main`
**Reviewers**: correctness, testing, maintainability, project-standards, agent-native, security (6 parallel agents)

---

## Fixes Applied

| # | File | Issue | Fix |
|---|------|-------|-----|
| 1 | `CalendarCommands.swift` L143-148 | Guard failures fell through with nil date, causing crash on `startDate/endDate` nil-coalescing | Added `return` after each `printError` in guard blocks |
| 2 | `NotesCommands.swift` L15 | `NSAppleScript(source: source)!` force-unwrap could crash if init fails | Replaced with `guard let script = NSAppleScript(source: source)` + error return |
| 3 | `SystemCommands.swift` L20,25 | `username` (via `NSUserName()`) exposed in system info output — privacy concern | Removed `username` field from `Info` output |
| 4 | *(prior session)* | `CalendarCommands.swift` magic number `7*24*3600` | Already replaced with named constant `SecondsPerWeek` |

---

## Issues Flagged for Discussion (Not Auto-Fixed)

These require architectural decisions or intentional trade-offs:

### Security

1. **AppleScript Injection** (`NotesCommands.swift`, `CalendarCommands.swift`)
   - `escapeAppleScript()` only escapes `\`, `"` — semicolons, newlines, `--` comments unescaped
   - Risk: Malicious note title/body could break out of string context or inject comments
   - **Fix needed**: Proper escaping of all special AppleScript characters

2. **Shell Injection in `automation.rs`**
   - `run_shell()` passes raw user input to `bash -c`
   - `escape_shell()` only escapes `$`, `` ` ``, `"`, `\`, `;`
   - PowerAutomation arm uses string formatting with unsanitized input
   - **Fix needed**: Use `Command::new()` with separate args instead of shell interpolation

3. **TCC Human-in-the-Loop** (`permission.rs`, `PermGuide.swift`)
   - `requestScreenRecordingAccess()`, `requestAutomationAccess()` return immediately if not granted
   - User must manually approve in System Preferences
   - No retry loop or user guidance built into the flow
   - **Trade-off**: Architectural — whether to block or degrade gracefully

### Architecture

4. **AX Unbounded Depth** (`ax.rs`, `AxSession.swift`)
   - `max_depth = 50` and `node_count_limit = 500` caps exist in AxSession but raw `query_recursive` in `ax.rs` has no depth limiting
   - Malformed accessibility trees could cause stack overflow
   - **Fix needed**: Propagate depth limiting to `ax.rs` query_recursive

5. **Bridge Shutdown Exposure** (`Handlers.swift`, `lib.rs`)
   - `bridge.shutdown` handler present and calls `println!("bridge.shutdown")` then `std::process::exit(0)`
   - No auth check — any process with bridge IPC access can kill it
   - **Fix needed**: Remove `shutdown` handler or gate behind capability check

6. **Bridge Enumeration** (`Handlers.swift`)
   - `bridge.enumerate` exposes all registered handlers
   - Reveals capability surface to attackers
   - **Fix needed**: Remove or gate behind auth

### Privacy

7. **`username` Removal** ✅ Fixed — was in `SystemCommands.Info` output

---

## Prior Fixes (Already Applied)

- `AxSession.swift`: Added `max_depth = 50` cap and `node_count_limit = 500` to prevent stack overflow
- `AxHandlers.swift`: Removed `shutdown` and `enumerate` from handler list
- `SpeechHandlers.swift`: Added path validation before file operations
- `automation.rs`: Added `#[allow(unreachable_patterns)]` on PowerShell arm (already correct, suppress warning)

---

## Test Acceptability

The following `unwrap()` calls in test-only modules are acceptable and not flagged:
- `permission.rs` test: `unwrap()` on TCC result (test context)
- `sysinfo.rs` test: `unwrap()` on system info (test context)
- `workspace.rs` test: `unwrap()` on workspace info (test context)

---

## Status

- [x] Safe auto-fixes applied to `CalendarCommands.swift`, `NotesCommands.swift`, `SystemCommands.swift`
- [x] Review artifact written
- [ ] Committed to `main`
