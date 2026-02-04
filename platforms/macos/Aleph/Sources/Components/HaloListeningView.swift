//
//  HaloListeningView.swift
//  Aleph
//
//  Listening state view for Halo overlay.
//  Shows a pulsing indicator when waiting for input.
//

import SwiftUI

/// View for displaying the listening state in Halo overlay
///
/// Features:
/// - Pulsing arc spinner animation
/// - Compact design for minimal footprint
///
/// Usage:
/// ```swift
/// HaloListeningView()
/// ```
struct HaloListeningView: View {
    @State private var isAnimating = false

    var body: some View {
        VStack(spacing: 6) {
            ArcSpinner(size: 24, color: .purple)
                .opacity(isAnimating ? 1.0 : 0.6)

            Text(L("halo.listening"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)
        }
        .padding(12)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
        .onAppear {
            withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                isAnimating = true
            }
        }
    }
}

// MARK: - Previews

#if DEBUG
#Preview("Listening State") {
    ZStack {
        Color.black.opacity(0.8)
        HaloListeningView()
    }
    .frame(width: 120, height: 100)
}
#endif
