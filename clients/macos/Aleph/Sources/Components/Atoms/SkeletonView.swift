import SwiftUI

/// Skeleton loading view for content placeholders
struct SkeletonView: View {
    // MARK: - Properties

    /// Width of the skeleton view
    let width: CGFloat?

    /// Height of the skeleton view
    let height: CGFloat

    /// Corner radius of the skeleton view
    let cornerRadius: CGFloat

    /// Animation state for shimmer effect
    @State private var isAnimating = false

    // MARK: - Initialization

    init(
        width: CGFloat? = nil,
        height: CGFloat = 20,
        cornerRadius: CGFloat = DesignTokens.CornerRadius.small
    ) {
        self.width = width
        self.height = height
        self.cornerRadius = cornerRadius
    }

    // MARK: - Body

    var body: some View {
        GeometryReader { geometry in
            RoundedRectangle(cornerRadius: cornerRadius)
                .fill(
                    LinearGradient(
                        gradient: Gradient(colors: [
                            DesignTokens.Colors.border.opacity(0.3),
                            DesignTokens.Colors.border.opacity(0.1),
                            DesignTokens.Colors.border.opacity(0.3)
                        ]),
                        startPoint: .leading,
                        endPoint: .trailing
                    )
                )
                .frame(width: width, height: height)
                .mask(
                    RoundedRectangle(cornerRadius: cornerRadius)
                        .fill(
                            LinearGradient(
                                gradient: Gradient(stops: [
                                    .init(color: .clear, location: 0),
                                    .init(color: .white, location: 0.3),
                                    .init(color: .white, location: 0.7),
                                    .init(color: .clear, location: 1)
                                ]),
                                startPoint: .leading,
                                endPoint: .trailing
                            )
                        )
                        .frame(width: width ?? geometry.size.width)
                        .offset(x: isAnimating ? (width ?? geometry.size.width) : -(width ?? geometry.size.width))
                )
        }
        .frame(width: width, height: height)
        .onAppear {
            withAnimation(
                Animation.linear(duration: 1.5).repeatForever(autoreverses: false)
            ) {
                isAnimating = true
            }
        }
    }
}

/// Skeleton card for provider list loading state
struct SkeletonProviderCard: View {
    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Icon placeholder
            Circle()
                .fill(DesignTokens.Colors.border.opacity(0.2))
                .frame(width: 44, height: 44)

            // Content placeholders
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                SkeletonView(width: 120, height: 16)
                SkeletonView(width: 80, height: 14)
                SkeletonView(width: 200, height: 12)
            }

            Spacer()

            // Status placeholder
            VStack(alignment: .trailing, spacing: DesignTokens.Spacing.sm) {
                SkeletonView(width: 100, height: 14)
                SkeletonView(width: 80, height: 12)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(DesignTokens.Colors.cardBackground)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .stroke(DesignTokens.Colors.border, lineWidth: 1)
        )
    }
}

// MARK: - Preview Provider

#Preview("Skeleton View") {
    VStack(spacing: DesignTokens.Spacing.md) {
        SkeletonView(width: 200, height: 20)
        SkeletonView(width: 150, height: 16)
        SkeletonView(width: 100, height: 14)
    }
    .padding()
}

#Preview("Skeleton Provider Card") {
    VStack(spacing: DesignTokens.Spacing.md) {
        SkeletonProviderCard()
        SkeletonProviderCard()
        SkeletonProviderCard()
    }
    .padding()
    .frame(width: 500)
}
