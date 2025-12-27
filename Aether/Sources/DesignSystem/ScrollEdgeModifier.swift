import SwiftUI

/// Scroll Edge Modifier
///
/// Adds subtle fade-in/fade-out effects at the edges of scrollable content,
/// creating a soft boundary between UI and content (Scroll Edge Effect).
///
/// **Styles**:
/// - **Soft**: Subtle transition, suitable for iOS/iPadOS and interactive elements
/// - **Hard**: Higher opacity, suitable for macOS with fixed text and controls
///
/// **Usage**:
/// ```swift
/// ScrollView {
///     content
/// }
/// .scrollEdge(edges: [.top, .bottom], style: .hard)
/// ```
///
/// Reference: WWDC 2025 - Apple Design System (Liquid Glass)
struct ScrollEdgeModifier: ViewModifier {

    // MARK: - Properties

    let edges: Edge.Set
    let style: Style

    // MARK: - Initialization

    init(edges: Edge.Set = [.top, .bottom], style: Style = .hard()) {
        self.edges = edges
        self.style = style
    }

    // MARK: - Body

    func body(content: Content) -> some View {
        content
            .mask(
                LinearGradient(
                    gradient: Gradient(stops: gradientStops),
                    startPoint: .top,
                    endPoint: .bottom
                )
            )
    }

    // MARK: - Gradient Stops

    private var gradientStops: [Gradient.Stop] {
        let hasTop = edges.contains(.top)
        let hasBottom = edges.contains(.bottom)

        switch style {
        case .soft(let opacity, _):
            return buildStops(hasTop: hasTop, hasBottom: hasBottom, opacity: opacity, softTransition: true)
        case .hard(let opacity, _):
            return buildStops(hasTop: hasTop, hasBottom: hasBottom, opacity: opacity, softTransition: false)
        }
    }

    private func buildStops(hasTop: Bool, hasBottom: Bool, opacity: Double, softTransition: Bool) -> [Gradient.Stop] {
        var stops: [Gradient.Stop] = []

        if hasTop {
            // Top edge fade-in (reduced range for sidebar navigation)
            stops.append(.init(color: .clear, location: 0))
            stops.append(.init(color: .white.opacity(opacity), location: softTransition ? 0.02 : 0.02))  // Reduced from 0.05
            stops.append(.init(color: .white, location: softTransition ? 0.04 : 0.05))  // Reduced from 0.1
        } else {
            // No top fade, start fully visible
            stops.append(.init(color: .white, location: 0))
        }

        if hasBottom {
            // Bottom edge fade-out (reduced range)
            stops.append(.init(color: .white, location: softTransition ? 0.96 : 0.95))  // Increased from 0.9
            stops.append(.init(color: .white.opacity(opacity), location: softTransition ? 0.98 : 0.98))  // Increased from 0.95
            stops.append(.init(color: .clear, location: 1))
        } else {
            // No bottom fade, end fully visible
            stops.append(.init(color: .white, location: 1))
        }

        return stops
    }

    // MARK: - Style

    enum Style {
        /// Soft transition (subtle, suitable for interactive elements)
        /// - Parameters:
        ///   - opacity: Opacity of the transition zone (default: 0.3)
        ///   - blur: Blur radius (default: 8pt)
        case soft(opacity: Double = 0.3, blur: CGFloat = 8)

        /// Hard transition (higher opacity, suitable for macOS fixed UI)
        /// - Parameters:
        ///   - opacity: Opacity of the transition zone (default: 0.6)
        ///   - blur: Blur radius (default: 12pt)
        case hard(opacity: Double = 0.6, blur: CGFloat = 12)
    }
}

// MARK: - View Extension

extension View {
    /// Applies scroll edge fade-in/fade-out effect
    ///
    /// - Parameters:
    ///   - edges: Which edges to apply the effect (default: top and bottom)
    ///   - style: Fade style (default: hard)
    /// - Returns: View with scroll edge effect
    ///
    /// - Example:
    /// ```swift
    /// ScrollView {
    ///     ForEach(items) { item in
    ///         ItemView(item)
    ///     }
    /// }
    /// .scrollEdge(edges: [.top, .bottom], style: .hard())
    /// ```
    func scrollEdge(edges: Edge.Set = [.top, .bottom], style: ScrollEdgeModifier.Style = .hard()) -> some View {
        self.modifier(ScrollEdgeModifier(edges: edges, style: style))
    }
}

// MARK: - Preview

#if DEBUG
struct ScrollEdgeModifier_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 20) {
            // Hard style (macOS)
            VStack(alignment: .leading, spacing: 8) {
                Text("Hard Style (macOS)")
                    .font(.headline)

                ScrollView {
                    VStack(spacing: 12) {
                        ForEach(0..<20) { index in
                            HStack {
                                Text("Item \(index + 1)")
                                    .font(.body)
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .foregroundColor(.secondary)
                            }
                            .padding(.horizontal)
                            .padding(.vertical, 8)
                            .background(Color.gray.opacity(0.1))
                            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding()
                }
                .frame(height: 200)
                .background(Color(nsColor: .windowBackgroundColor))
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .scrollEdge(edges: [.top, .bottom], style: .hard())
            }

            // Soft style (iOS-like)
            VStack(alignment: .leading, spacing: 8) {
                Text("Soft Style (iOS-like)")
                    .font(.headline)

                ScrollView {
                    VStack(spacing: 12) {
                        ForEach(0..<20) { index in
                            HStack {
                                Text("Item \(index + 1)")
                                    .font(.body)
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .foregroundColor(.secondary)
                            }
                            .padding(.horizontal)
                            .padding(.vertical, 8)
                            .background(Color.blue.opacity(0.1))
                            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding()
                }
                .frame(height: 200)
                .background(Color(nsColor: .windowBackgroundColor))
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .scrollEdge(edges: [.top, .bottom], style: .soft())
            }
        }
        .padding(40)
        .frame(width: 500, height: 600)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}
#endif
