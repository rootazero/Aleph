import Foundation

/// Concentric Geometry Utility
///
/// Provides mathematical calculations for concentric shape radii based on Apple's Liquid Glass design system.
/// All shapes are aligned around a common center point with radii calculated using the formula:
///
/// **Child Radius = max(Parent Radius - Padding, Minimum Radius)**
///
/// This ensures visual harmony and optical balance across nested UI elements.
///
/// Reference: WWDC 2025 - Apple Design System (Liquid Glass)
struct ConcentricGeometry {

    // MARK: - Constants

    /// Window-level corner radius (outermost shape)
    /// Matches macOS standard window corner radius
    static let windowRadius: CGFloat = 12

    /// Sidebar corner radius
    /// Calculated: windowRadius - sidebarPadding (12 - 12 = 0, but we use a fixed minimum)
    static let sidebarRadius: CGFloat = 10

    /// Content area corner radius
    /// Calculated: windowRadius - contentPadding (12 - 0 = 12)
    static let contentRadius: CGFloat = 12

    /// Card corner radius
    /// Calculated: contentRadius - cardPadding (typically 12 - 8 = 4)
    static let cardRadius: CGFloat = 8

    /// Minimum corner radius to prevent visual artifacts
    /// Ensures shapes never become completely square unintentionally
    static let minimumRadius: CGFloat = 4

    // MARK: - Calculation Methods

    /// Calculates child shape radius based on parent radius and padding
    ///
    /// - Parameters:
    ///   - parent: The corner radius of the parent shape
    ///   - padding: The padding/inset between parent and child
    ///   - minimum: Minimum allowed radius (default: 4pt)
    /// - Returns: Calculated child radius, guaranteed to be >= minimum
    ///
    /// - Example:
    /// ```swift
    /// // Window with 12pt radius, content with 16pt padding
    /// let contentRadius = ConcentricGeometry.childRadius(parent: 12, padding: 16)
    /// // Returns: 4 (because 12 - 16 = -4, clamped to minimum 4)
    /// ```
    static func childRadius(parent: CGFloat, padding: CGFloat, minimum: CGFloat = minimumRadius) -> CGFloat {
        return max(parent - padding, minimum)
    }

    /// Calculates content area radius with given padding
    ///
    /// - Parameter padding: The padding from window edge to content area
    /// - Returns: Content area corner radius
    static func contentRadius(padding: CGFloat) -> CGFloat {
        return childRadius(parent: windowRadius, padding: padding)
    }

    /// Calculates card radius based on content padding and card padding
    ///
    /// - Parameters:
    ///   - contentPadding: The padding from window to content area
    ///   - cardPadding: The padding from content area to card
    /// - Returns: Card corner radius
    static func cardRadius(contentPadding: CGFloat, cardPadding: CGFloat) -> CGFloat {
        let contentRad = contentRadius(padding: contentPadding)
        return childRadius(parent: contentRad, padding: cardPadding)
    }
}

// MARK: - View Extensions

import SwiftUI

extension View {
    /// Applies concentric shape radius based on parent radius and padding
    ///
    /// - Parameters:
    ///   - parent: Parent shape corner radius
    ///   - padding: Padding between parent and this shape
    ///   - minimum: Minimum corner radius (default: 4pt)
    /// - Returns: View with rounded corners using concentric geometry
    func concentricShape(parent: CGFloat, padding: CGFloat, minimum: CGFloat = ConcentricGeometry.minimumRadius) -> some View {
        let radius = ConcentricGeometry.childRadius(parent: parent, padding: padding, minimum: minimum)
        return self.clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
    }

    /// Applies window-level corner radius (outermost shape)
    func windowShape() -> some View {
        self.clipShape(RoundedRectangle(cornerRadius: ConcentricGeometry.windowRadius, style: .continuous))
    }

    /// Applies content area corner radius
    func contentShape(padding: CGFloat = 0) -> some View {
        let radius = ConcentricGeometry.contentRadius(padding: padding)
        return self.clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
    }

    /// Applies card corner radius
    func cardShape() -> some View {
        self.clipShape(RoundedRectangle(cornerRadius: ConcentricGeometry.cardRadius, style: .continuous))
    }
}
