import SwiftUI

/// Adaptive Material Component
///
/// Provides Liquid Glass material effects using native NSVisualEffectView materials.
/// Requires macOS 15.0 or later.
///
/// Reference: WWDC 2025 - Apple Design System (Liquid Glass)
struct AdaptiveMaterial: View {

    // MARK: - Properties

    let material: MaterialType

    // MARK: - Initialization

    /// Creates an adaptive material
    ///
    /// - Parameters:
    ///   - material: Material type for visual effect
    init(_ material: MaterialType) {
        self.material = material
    }

    // MARK: - Body

    var body: some View {
        VisualEffectBackground(
            material: material.nsVisualEffectMaterial,
            blendingMode: .withinWindow
        )
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
}

// MARK: - Convenience Initializers

extension AdaptiveMaterial {

    /// Sidebar material (for floating navigation sidebars)
    static var sidebar: AdaptiveMaterial {
        AdaptiveMaterial(.sidebar)
    }

    /// Title bar material (for window title bars and navigation bars)
    static var titlebar: AdaptiveMaterial {
        AdaptiveMaterial(.titlebar)
    }

    /// Window background material (for main content areas)
    static var windowBackground: AdaptiveMaterial {
        AdaptiveMaterial(.windowBackground)
    }

    /// Content background material (for secondary content areas)
    static var contentBackground: AdaptiveMaterial {
        AdaptiveMaterial(.contentBackground)
    }

    /// Header view material (for section headers)
    static var headerView: AdaptiveMaterial {
        AdaptiveMaterial(.headerView)
    }

    /// Menu material (for context menus and dropdowns)
    static var menu: AdaptiveMaterial {
        AdaptiveMaterial(.menu)
    }

    /// Popover material (for floating popovers)
    static var popover: AdaptiveMaterial {
        AdaptiveMaterial(.popover)
    }

    /// Under-window background material (for ultra-thin transparent overlays)
    static var underWindow: AdaptiveMaterial {
        AdaptiveMaterial(.underWindowBackground)
    }

    /// HUD window material (for heads-up displays)
    static var hudWindow: AdaptiveMaterial {
        AdaptiveMaterial(.hudWindow)
    }

    /// Ultra-thin material (for floating panels and overlays)
    /// Maps to underWindowBackground for maximum transparency
    static var ultraThin: AdaptiveMaterial {
        AdaptiveMaterial(.underWindowBackground)
    }

    /// Thin material (for secondary panels)
    /// Maps to contentBackground with lighter appearance
    static var thin: AdaptiveMaterial {
        AdaptiveMaterial(.contentBackground)
    }

    /// Thick material (for modals and overlays)
    /// Maps to contentBackground with heavier appearance
    static var thick: AdaptiveMaterial {
        AdaptiveMaterial(.contentBackground)
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
