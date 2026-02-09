//
//  RootContentView.swift
//  Aleph
//
//  Simplified root layout - all configuration now managed via ControlPlane
//

import SwiftUI
import AppKit

/// Simplified root content view for Settings window
struct RootContentView: View {
    // MARK: - Dependencies

    /// core (rig-core based) - used for connection status
    var core: AlephCore? {
        appDelegate.core
    }

    // Observe AppDelegate for core updates
    @EnvironmentObject private var appDelegate: AppDelegate

    // MARK: - Body

    var body: some View {
        SettingsView(core: core)
            .frame(minWidth: 400, maxWidth: 400, minHeight: 300, maxHeight: 300)
            .background(.windowBackground)
    }
}

// MARK: - Preview

#if DEBUG
struct RootContentView_Previews: PreviewProvider {
    static var previews: some View {
        RootContentView()
            .environmentObject(AppDelegate())
    }
}
#endif
