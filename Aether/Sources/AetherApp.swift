//
//  AetherApp.swift
//  Aether
//
//  Legacy SwiftUI App structure - NO LONGER USED as entry point.
//
//  IMPORTANT: This file is kept for reference but is not the entry point.
//  The actual entry point is main.swift which uses traditional AppKit launch.
//
//  We switched from SwiftUI App lifecycle to AppKit because macOS 26 (Tahoe)
//  has a bug where ANY SwiftUI Scene (Settings, WindowGroup, etc.) triggers
//  NSHostingView.invalidateSafeAreaCornerInsets() crash during early
//  initialization before the view hierarchy is fully set up.
//
//  Using main.swift + NSApplicationMain() gives us full control over when
//  windows are created, avoiding the crash entirely.
//

import SwiftUI

// NOTE: @main attribute REMOVED - see main.swift for entry point
struct AetherApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // This is never actually used since we launch via main.swift
        // Kept for SwiftUI protocol conformance if needed in future
        WindowGroup {
            EmptyView()
        }
    }
}
