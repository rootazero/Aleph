import SwiftUI

/// Adaptive Material Component
///
/// Provides Liquid Glass material effects with graceful fallback for older macOS versions.
/// Uses native NSVisualEffectView materials when available, falls back to translucent backgrounds for compatibility.
///
/// **Material Selection Matrix**:
/// - macOS 13+: Full NSVisualEffectView materials (`.sidebar`, `.titlebar`, `.contentBackground`, etc.)
/// - macOS 12 and below: Translucent color backgrounds with blur
///
/// Reference: WWDC 2025 - Apple Design System (Liquid Glass)
struct AdaptiveMaterial: View {

    // MARK: - Properties

    let preferredMaterial: MaterialType
    let fallbackStyle: FallbackStyle

    // MARK: - Initialization

    /// Creates an adaptive material with automatic fallback
    ///
    /// - Parameters:
    ///   - material: Preferred material for modern macOS versions
    ///   - fallbackStyle: Fallback style for older macOS versions
    init(_ material: MaterialType, fallback: FallbackStyle = .translucent(.black, opacity: 0.05, blur: 10)) {
        self.preferredMaterial = material
        self.fallbackStyle = fallback
    }

    // MARK: - Body

    var body: some View {
        if #available(macOS 10.14, *) {
            // macOS 10.14+ supports NSVisualEffectView
            VisualEffectBackground(
                material: preferredMaterial.nsVisualEffectMaterial,
                blendingMode: .withinWindow
            )
        } else {
            // macOS 10.13 and below: use fallback
            fallbackBackground
        }
    }

    // MARK: - Fallback Background

    @ViewBuilder
    private var fallbackBackground: some View {
        switch fallbackStyle {
        case .solid(let color):
            color
        case .translucent(let color, let opacity, let blur):
            ZStack {
                // Base color
                color.opacity(opacity)

                // Blur effect (simulates material)
                Rectangle()
                    .fill(Color.white.opacity(0.01))
                    .blur(radius: blur)
            }
        }
    }

    // MARK: - Material Type

    /// Custom material type enumeration
    enum MaterialType {
        case sidebar
        case titlebar
        case windowBackground
        case contentBackground
        case headerView
        case menu
        case popover
        case selection
        case underWindowBackground
        case hudWindow

        /// Maps to NSVisualEffectView.Material
        var nsVisualEffectMaterial: NSVisualEffectView.Material {
            switch self {
            case .sidebar:
                return .sidebar
            case .titlebar:
                return .titlebar
            case .windowBackground:
                return .windowBackground
            case .contentBackground:
                return .contentBackground
            case .headerView:
                return .headerView
            case .menu:
                return .menu
            case .popover:
                return .popover
            case .selection:
                return .selection
            case .underWindowBackground:
                return .underWindowBackground
            case .hudWindow:
                return .hudWindow
            }
        }
    }

    // MARK: - Fallback Style

    enum FallbackStyle {
        /// Solid color background (no transparency)
        case solid(Color)

        /// Translucent color with blur (simulates material)
        /// - Parameters:
        ///   - color: Base color
        ///   - opacity: Opacity of the color layer (0.0-1.0)
        ///   - blur: Blur radius to simulate material effect
        case translucent(Color, opacity: Double, blur: CGFloat)
    }
}

// MARK: - Convenience Initializers

extension AdaptiveMaterial {

    /// Sidebar material (for floating navigation sidebars)
    static var sidebar: AdaptiveMaterial {
        AdaptiveMaterial(.sidebar, fallback: .translucent(.white, opacity: 0.8, blur: 20))
    }

    /// Title bar material (for window title bars and navigation bars)
    static var titlebar: AdaptiveMaterial {
        AdaptiveMaterial(.titlebar, fallback: .translucent(.white, opacity: 0.9, blur: 15))
    }

    /// Window background material (for main content areas)
    static var windowBackground: AdaptiveMaterial {
        AdaptiveMaterial(.windowBackground, fallback: .solid(Color(nsColor: .windowBackgroundColor)))
    }

    /// Content background material (for secondary content areas)
    static var contentBackground: AdaptiveMaterial {
        AdaptiveMaterial(.contentBackground, fallback: .translucent(.white, opacity: 0.85, blur: 12))
    }

    /// Header view material (for section headers)
    static var headerView: AdaptiveMaterial {
        AdaptiveMaterial(.headerView, fallback: .translucent(.white, opacity: 0.9, blur: 15))
    }

    /// Menu material (for context menus and dropdowns)
    static var menu: AdaptiveMaterial {
        AdaptiveMaterial(.menu, fallback: .translucent(.white, opacity: 0.95, blur: 10))
    }

    /// Popover material (for floating popovers)
    static var popover: AdaptiveMaterial {
        AdaptiveMaterial(.popover, fallback: .translucent(.white, opacity: 0.95, blur: 10))
    }

    /// Under-window background material (for ultra-thin transparent overlays)
    static var underWindow: AdaptiveMaterial {
        AdaptiveMaterial(.underWindowBackground, fallback: .translucent(.white, opacity: 0.5, blur: 25))
    }

    /// HUD window material (for heads-up displays)
    static var hudWindow: AdaptiveMaterial {
        AdaptiveMaterial(.hudWindow, fallback: .translucent(.black, opacity: 0.7, blur: 15))
    }

    /// Ultra-thin material (for floating panels and overlays)
    /// Maps to underWindowBackground for maximum transparency
    static var ultraThin: AdaptiveMaterial {
        AdaptiveMaterial(.underWindowBackground, fallback: .translucent(.white, opacity: 0.5, blur: 25))
    }

    /// Thin material (for secondary panels)
    /// Maps to contentBackground with lighter appearance
    static var thin: AdaptiveMaterial {
        AdaptiveMaterial(.contentBackground, fallback: .translucent(.white, opacity: 0.6, blur: 20))
    }

    /// Thick material (for modals and overlays)
    /// Maps to contentBackground with heavier appearance
    static var thick: AdaptiveMaterial {
        AdaptiveMaterial(.contentBackground, fallback: .translucent(.white, opacity: 0.85, blur: 12))
    }
}

// MARK: - Preview

#if DEBUG
struct AdaptiveMaterial_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 20) {
            // Sidebar material
            ZStack {
                Color.blue.opacity(0.3)
                VStack {
                    Text("Sidebar Material")
                        .font(.headline)
                    Text("Floating navigation sidebar")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                .padding()
            }
            .frame(width: 200, height: 100)
            .background(AdaptiveMaterial.sidebar)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))

            // Title bar material
            ZStack {
                Color.green.opacity(0.3)
                VStack {
                    Text("Title Bar Material")
                        .font(.headline)
                    Text("Window title bar")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                .padding()
            }
            .frame(width: 200, height: 100)
            .background(AdaptiveMaterial.titlebar)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))

            // Content background material
            ZStack {
                Color.purple.opacity(0.3)
                VStack {
                    Text("Content Background")
                        .font(.headline)
                    Text("Secondary content areas")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                .padding()
            }
            .frame(width: 200, height: 100)
            .background(AdaptiveMaterial.contentBackground)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
        .padding(40)
        .frame(width: 400, height: 500)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}
#endif
