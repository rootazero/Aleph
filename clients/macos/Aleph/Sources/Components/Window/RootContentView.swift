//
//  RootContentView.swift
//  Aleph
//
//  Simplified root layout - all configuration now managed via ControlPlane
//  Uses WebSocket connection instead of FFI
//

import SwiftUI
import AppKit

/// Simplified root content view for Settings window
struct RootContentView: View {
    // MARK: - Body

    var body: some View {
        SettingsView()
            .frame(minWidth: 400, maxWidth: 400, minHeight: 350, maxHeight: 350)
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
