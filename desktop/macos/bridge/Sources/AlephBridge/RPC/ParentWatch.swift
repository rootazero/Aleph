import Foundation

/// Parent-process death watchdog.
///
/// When the Rust `SwiftBridge` client spawns us, it places us in our own
/// process group (setpgid) but does NOT enroll us in a controlling-process
/// death pipeline (kqueue NOTE_EXIT needs a pid + kqueue loop). As a
/// lightweight alternative, poll `getppid()` every 10 s; if the parent has
/// been reparented to init (ppid == 1), the aleph-server process has
/// exited and we should terminate so we do not linger as an orphan.
///
/// Activated from `main.swift` when the helper is launched in long-lived
/// RPC mode (Task 0.9). Under the legacy CLI mode (one-shot subcommand
/// invocation) this watch is not started — the process exits on its own
/// after the subcommand returns.
actor ParentWatch {
    func start() {
        Task.detached {
            while true {
                try? await Task.sleep(nanoseconds: 10_000_000_000)
                if getppid() == 1 {
                    FileHandle.standardError.write(
                        "aleph-bridge: parent died, exiting\n".data(using: .utf8)!
                    )
                    exit(0)
                }
            }
        }
    }
}
