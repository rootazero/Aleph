import SwiftUI

/// Centralized design system constants for visual consistency
/// Provides semantic colors, spacing, typography, and other visual parameters
enum DesignTokens {

    // MARK: - Colors

    /// Semantic color definitions with automatic light/dark mode support
    enum Colors {
        // MARK: Background Colors

        /// Sidebar background color (system control background)
        static let sidebarBackground = Color(nsColor: .controlBackgroundColor)

        /// Card background with semi-transparency for depth
        static let cardBackground = Color(nsColor: .controlBackgroundColor).opacity(0.8)

        /// Main content background
        static let contentBackground = Color(nsColor: .windowBackgroundColor)

        // MARK: Accent Colors

        /// Primary accent color (blue) - used for primary actions and selected states
        static let accentBlue = Color(red: 0.0, green: 0.48, blue: 1.0)

        /// Secondary accent color for less prominent elements
        static let accentGray = Color.secondary

        // MARK: Status Colors

        /// Success/active state color (green)
        static let providerActive = Color.green

        /// Success state color (same as providerActive for consistency)
        static let success = Color.green

        /// Inactive/offline state color (gray)
        static let providerInactive = Color.gray

        /// Warning state color (yellow/orange)
        static let warning = Color.orange

        /// Error/danger state color (red)
        static let error = Color.red

        /// Information state color (blue)
        static let info = accentBlue

        // MARK: Text Colors

        /// Primary text color (high contrast)
        static let textPrimary = Color.primary

        /// Secondary text color (medium contrast)
        static let textSecondary = Color.secondary

        /// Disabled text color (low contrast)
        static let textDisabled = Color.gray.opacity(0.5)

        // MARK: UI Element Colors

        /// Border color for cards and inputs
        static let border = Color.gray.opacity(0.2)

        /// Border color for selected/focused elements
        static let borderSelected = accentBlue

        /// Hover overlay color
        static let hoverOverlay = Color.primary.opacity(0.05)
    }

    // MARK: - Spacing

    /// Consistent spacing scale for padding and margins
    enum Spacing {
        /// Extra small spacing (4pt) - tight spacing between related elements
        static let xs: CGFloat = 4

        /// Small spacing (8pt) - compact spacing for dense UIs
        static let sm: CGFloat = 8

        /// Medium spacing (16pt) - standard spacing between elements
        static let md: CGFloat = 16

        /// Large spacing (24pt) - comfortable spacing for sections
        static let lg: CGFloat = 24

        /// Extra large spacing (32pt) - loose spacing for major sections
        static let xl: CGFloat = 32
    }

    // MARK: - Corner Radius

    /// Corner radius values for consistent rounding
    enum CornerRadius {
        /// Small radius (6pt) - for buttons, chips, small controls
        static let small: CGFloat = 6

        /// Medium radius (10pt) - for cards, inputs, standard containers
        static let medium: CGFloat = 10

        /// Large radius (16pt) - for large containers, modals
        static let large: CGFloat = 16
    }

    // MARK: - Typography

    /// Font definitions for text hierarchy
    enum Typography {
        /// Page title font (22pt semibold)
        static let title = Font.system(size: 22, weight: .semibold)

        /// Section header font (17pt medium)
        static let heading = Font.system(size: 17, weight: .medium)

        /// Body text font (14pt regular)
        static let body = Font.system(size: 14, weight: .regular)

        /// Caption/supporting text font (12pt regular)
        static let caption = Font.system(size: 12, weight: .regular)

        /// Code/monospaced font (13pt monospaced)
        static let code = Font.system(size: 13, design: .monospaced)
    }

    // MARK: - Shadows

    /// Shadow parameters for depth and elevation
    enum Shadows {
        /// Card shadow - subtle elevation for card elements
        /// **DEPRECATED**: Use spacing and layout instead (Liquid Glass guideline)
        static let card = ShadowStyle(radius: 4, opacity: 0.1, offset: CGSize(width: 0, height: 2))

        /// Elevated shadow - stronger shadow for modals and popovers
        static let elevated = ShadowStyle(radius: 8, opacity: 0.15, offset: CGSize(width: 0, height: 4))

        /// Dropdown shadow - medium shadow for dropdown menus
        static let dropdown = ShadowStyle(radius: 6, opacity: 0.12, offset: CGSize(width: 0, height: 3))

        /// Floating layer shadow - subtle shadow for floating UI elements (e.g., sidebar)
        /// Used only for functional layers that need to appear above content
        static let floating = ShadowStyle(radius: 12, opacity: 0.08, offset: CGSize(width: 0, height: 4))
    }

    // MARK: - Materials (Liquid Glass)

    /// Adaptive materials with automatic fallback for older macOS versions
    /// Reference: WWDC 2025 - Apple Design System (Liquid Glass)
    enum Materials {
        /// Sidebar material - for floating navigation sidebars
        static var sidebar: AdaptiveMaterial { .sidebar }

        /// Title bar material - for window title bars and navigation bars
        static var titlebar: AdaptiveMaterial { .titlebar }

        /// Window background material - for main content areas
        static var windowBackground: AdaptiveMaterial { .windowBackground }

        /// Ultra-thin material - for floating panels and overlays
        static var ultraThin: AdaptiveMaterial { .ultraThin }

        /// Thin material - for secondary panels
        static var thin: AdaptiveMaterial { .thin }

        /// Thick material - for modals and overlays
        static var thick: AdaptiveMaterial { .thick }
    }

    // MARK: - Concentric Radius

    /// Concentric geometry corner radii
    /// Reference: ConcentricGeometry for calculation details
    enum ConcentricRadius {
        /// Window-level corner radius (12pt)
        static let window = ConcentricGeometry.windowRadius

        /// Sidebar corner radius (10pt)
        static let sidebar = ConcentricGeometry.sidebarRadius

        /// Content area corner radius (12pt)
        static let content = ConcentricGeometry.contentRadius

        /// Card corner radius (8pt)
        static let card = ConcentricGeometry.cardRadius

        /// Minimum corner radius (4pt)
        static let minimum = ConcentricGeometry.minimumRadius
    }

    // MARK: - Animation

    /// Standard animation durations and curves
    enum Animation {
        /// Quick animation (0.15s) - for micro-interactions
        static let quick = SwiftUI.Animation.easeInOut(duration: 0.15)

        /// Standard animation (0.25s) - for most transitions
        static let standard = SwiftUI.Animation.easeInOut(duration: 0.25)

        /// Slow animation (0.35s) - for complex transitions
        static let slow = SwiftUI.Animation.easeInOut(duration: 0.35)

        /// Spring animation - for playful, bouncy transitions
        static let spring = SwiftUI.Animation.spring(response: 0.3, dampingFraction: 0.7)
    }
}

// MARK: - Shadow Style Helper

/// Helper struct for consistent shadow definitions
struct ShadowStyle {
    let radius: CGFloat
    let opacity: Double
    let offset: CGSize
}

// MARK: - View Extension for Shadow Application

extension View {
    /// Apply a design token shadow style to the view
    func shadow(_ style: ShadowStyle, color: Color = .black) -> some View {
        self.shadow(
            color: color.opacity(style.opacity),
            radius: style.radius,
            x: style.offset.width,
            y: style.offset.height
        )
    }
}
