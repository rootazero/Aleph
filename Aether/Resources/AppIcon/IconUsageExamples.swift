// Aether Icon Usage Examples

import SwiftUI

// MARK: - Menu Bar Icon (Template Mode)
// Use in menu bar, always renders as monochrome
struct MenuBarExample: View {
    var body: some View {
        Image("MenuBarIcon")
            .renderingMode(.template)
            .foregroundColor(.primary)
    }
}

// MARK: - App Logo (Full Color)
// Use in settings, about screen, etc.
struct AppLogoExample: View {
    var body: some View {
        Image("AppLogo")
            .resizable()
            .aspectRatio(contentMode: .fit)
            .frame(width: 64, height: 64)
    }
}

// MARK: - Different Sizes
struct IconSizesExample: View {
    var body: some View {
        VStack(spacing: 20) {
            // Small (Menu bar)
            Image("MenuBarIcon")
                .renderingMode(.template)
                .frame(width: 16, height: 16)

            // Medium (Settings)
            Image("AppLogo")
                .frame(width: 32, height: 32)

            // Large (About screen)
            Image("AppLogo")
                .frame(width: 128, height: 128)
        }
    }
}
