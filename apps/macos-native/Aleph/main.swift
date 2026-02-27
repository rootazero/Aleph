import AppKit

// Pure AppKit entry point — no SwiftUI App lifecycle.
// SwiftUI's @main + @NSApplicationDelegateAdaptor conflicts with
// NSStatusItem creation for menu bar apps.
//
// main.swift runs on the main thread, so MainActor.assumeIsolated is safe.

let app = NSApplication.shared
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
