import Foundation

/// Process-lifetime retention for the parent-exit event source. A resumed
/// `DispatchSource` is cancelled the instant its last strong reference is
/// dropped, so the source must be held for as long as we want it to fire.
/// Written exactly once during `ParentWatch.start()`, before anything reads it.
private final class ParentExitSourceHolder: @unchecked Sendable {
    var source: DispatchSourceProcess?
    static let shared = ParentExitSourceHolder()
}

/// Parent-process death watchdog.
///
/// When the Rust `SwiftBridge` client spawns us, it places us in our own
/// process group (setpgid) but does NOT enroll us in a controlling-process
/// death pipeline. Crucially, the aleph-server SIGTERM handler calls
/// `std::process::exit`, which bypasses Rust `Drop` — so our stdin is *not*
/// closed on a clean `aleph-server stop`, and the usual stdin-EOF shutdown
/// path never triggers. We must detect the parent's death ourselves.
///
/// Fast path: a `DispatchSource` process-exit event on the parent pid (kqueue
/// `NOTE_EXIT` under the hood) fires the instant the parent dies, so the helper
/// terminates in milliseconds instead of lingering as an orphan for up to one
/// poll interval. Backstop path: a slow `getppid()` poll covers the rare cases
/// where the event source could not be armed (pid already reaped) or never
/// fires — degrading gracefully to the original poll-only behavior.
///
/// Activated from `main.swift` when the helper is launched in long-lived
/// RPC mode. Under the legacy CLI mode (one-shot subcommand invocation) this
/// watch is not started — the process exits on its own after the subcommand
/// returns.
actor ParentWatch {
    func start() {
        let ppid = getppid()
        // Parent already reparented to launchd (pid 1) before we could arm.
        if ppid <= 1 {
            ParentWatch.terminate()
        }

        // Fast path: event-driven parent-exit notification.
        let src = DispatchSource.makeProcessSource(
            identifier: ppid,
            eventMask: .exit,
            queue: DispatchQueue.global()
        )
        src.setEventHandler {
            ParentWatch.terminate()
        }
        src.resume()
        ParentExitSourceHolder.shared.source = src

        // TOCTOU guard: the parent may have died between reading getppid() and
        // arming the source above; re-check now that the source is live.
        if getppid() <= 1 {
            ParentWatch.terminate()
        }

        // Backstop path: slow poll in case the event source never fires. This
        // preserves the original behavior as a floor — at worst we linger for
        // one interval, exactly as before the fast path existed.
        Task.detached {
            while true {
                try? await Task.sleep(nanoseconds: 10_000_000_000)
                if getppid() <= 1 {
                    ParentWatch.terminate()
                }
            }
        }
    }

    private static func terminate() -> Never {
        FileHandle.standardError.write(
            "aleph-bridge: parent died, exiting\n".data(using: .utf8)!
        )
        exit(0)
    }
}
