# Logic Review Report
**Module**: daemon (src/bin/aleph-server/daemon.rs + src/gateway/handlers/daemon_control.rs)
**Scope**: Full static review of daemon process management and gateway control handlers
**Date**: 2026-05-31
**Mode**: strict

## Findings

### [Warning] Missing SAFETY comments on unsafe blocks
- **Location**: `src/bin/aleph-server/daemon.rs` (multiple locations)
- **Risk**: Unsafe blocks lack required safety documentation per AGENTS.md security guidelines
- **Current impact**: medium
- **Suggestion**: Add `// SAFETY:` comments explaining invariants for each unsafe block

**Fixed**: Added SAFETY comments to all 8 unsafe blocks:
- `libc::getuid()` - async-signal-safe, always succeeds
- `libc::kill(pid, 0)` - standard Unix process existence check
- `libc::kill(pid, SIGTERM)` - validated PID, standard graceful termination
- `libc::kill(pid, SIGKILL)` - validated PID, standard forceful termination
- `libc::fork()` - single-threaded context (before tokio runtime)
- `libc::setsid()` - standard session creation for daemonization
- `libc::umask()` - deterministic file permissions
- `libc::chdir()` - valid null-terminated C string
- `libc::dup2()` - valid file descriptors from open files

### [Warning] stdin not redirected during daemonization
- **Location**: `src/bin/aleph-server/daemon.rs:254-276`
- **Risk**: Daemon process retains connection to controlling terminal via stdin, potentially causing issues with terminal reattachment or unexpected input
- **Current impact**: medium
- **Suggestion**: Redirect stdin to /dev/null (or log file) alongside stdout/stderr

**Fixed**: Added `libc::dup2(fd, libc::STDIN_FILENO)` to both log-file and /dev/null branches.

### [Warning] Log level filtering may produce false matches
- **Location**: `src/gateway/handlers/daemon_control.rs:86-90`
- **Risk**: Pattern `" ERROR "` (with spaces) may not match all log formats; partial matches could include unrelated lines
- **Current impact**: low
- **Suggestion**: Support multiple log format patterns

**Fixed**: Enhanced filtering to match three patterns:
- `" LEVEL "` (space-delimited)
- `"[LEVEL]"` (bracketed)
- `" LEVEL"` (end-of-line)

### [Suggested Test] Daemonization integration test
```rust
#[test]
#[cfg(unix)]
fn test_daemonize_redirects_stdin() {
    // Verify that after daemonize(), stdin is not connected to terminal
    use std::os::unix::io::isatty;
    unsafe {
        assert!(!isatty(0), "stdin should not be a tty after daemonization");
    }
}
```

### [Suggested Test] Stale PID file handling
```rust
#[test]
fn test_handle_stop_removes_stale_pid_file() {
    let temp_dir = std::env::temp_dir();
    let pid_file = temp_dir.join("test_stale.pid");
    std::fs::write(&pid_file, "99999").unwrap(); // Non-existent PID
    
    let result = handle_stop(pid_file.to_str().unwrap());
    assert!(result.is_ok());
    assert!(!pid_file.exists(), "stale PID file should be removed");
}
```

### [Suggested Test] Log level filtering accuracy
```rust
#[test]
fn test_log_level_filtering() {
    let lines = vec![
        "2026-01-01 [ERROR] something failed",
        "2026-01-01 [WARN]  not an error",
        "2026-01-01 ERROR happened here",
        "2026-01-01 WARN  not matching",
    ];
    // Verify only ERROR lines are retained
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 3 |
| Suggested Test | 3 |

## Fixes Applied
1. Added SAFETY comments to all 8 unsafe blocks in `daemon.rs`
2. Added stdin redirect (`dup2(fd, STDIN_FILENO)`) in daemonization path
3. Improved log level filtering robustness in `daemon_control.rs`

## Verification
- `cargo check -p alephcore --lib` — **通过**（仅预存 warning）
- `cargo check --bin aleph-server` — **通过**（仅预存 warning）
