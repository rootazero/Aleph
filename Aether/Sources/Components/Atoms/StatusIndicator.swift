import SwiftUI

/// Status indicator component showing a colored circle with optional label
struct StatusIndicator: View {
    // MARK: - Status Type

    /// Available status types with associated colors
    enum Status {
        case success
        case warning
        case error
        case inactive
        case inProgress

        /// Color associated with each status
        var color: Color {
            switch self {
            case .success:
                return DesignTokens.Colors.providerActive
            case .warning:
                return DesignTokens.Colors.warning
            case .error:
                return DesignTokens.Colors.error
            case .inactive:
                return DesignTokens.Colors.providerInactive
            case .inProgress:
                return DesignTokens.Colors.info
            }
        }

        /// Label text for each status
        var label: String {
            switch self {
            case .success:
                return "Active"
            case .warning:
                return "Warning"
            case .error:
                return "Error"
            case .inactive:
                return "Inactive"
            case .inProgress:
                return "In Progress"
            }
        }
    }

    // MARK: - Properties

    /// Current status to display
    let status: Status

    /// Optional custom label text (overrides default status label)
    let customLabel: String?

    /// Whether to show a text label next to the indicator
    let showLabel: Bool

    /// Whether to animate with a pulsing effect
    let shouldPulse: Bool

    /// Diameter of the status indicator circle
    let size: CGFloat

    /// Animation state for pulsing effect
    @State private var isPulsing = false

    // MARK: - Initialization

    init(
        status: Status,
        label: String? = nil,
        showLabel: Bool = true,
        shouldPulse: Bool = false,
        size: CGFloat = 8
    ) {
        self.status = status
        self.customLabel = label
        self.showLabel = showLabel
        self.shouldPulse = shouldPulse
        self.size = size
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.xs) {
            // Status indicator circle
            Circle()
                .fill(status.color)
                .frame(width: size, height: size)
                .opacity(isPulsing ? 0.5 : 1.0)
                .scaleEffect(isPulsing ? 1.2 : 1.0)
                .animation(
                    shouldPulse
                        ? Animation.easeInOut(duration: 1.0).repeatForever(autoreverses: true)
                        : nil,
                    value: isPulsing
                )

            // Optional text label
            if showLabel {
                Text(customLabel ?? status.label)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
        .onAppear {
            if shouldPulse {
                isPulsing = true
            }
        }
    }
}

// MARK: - Preview Provider

#Preview("All Status Types") {
    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
        StatusIndicator(status: .success)
        StatusIndicator(status: .warning)
        StatusIndicator(status: .error)
        StatusIndicator(status: .inactive)
        StatusIndicator(status: .inProgress, shouldPulse: true)
    }
    .padding()
}

#Preview("Without Labels") {
    HStack(spacing: DesignTokens.Spacing.md) {
        StatusIndicator(status: .success, showLabel: false)
        StatusIndicator(status: .warning, showLabel: false)
        StatusIndicator(status: .error, showLabel: false)
        StatusIndicator(status: .inactive, showLabel: false)
        StatusIndicator(status: .inProgress, showLabel: false, shouldPulse: true)
    }
    .padding()
}

#Preview("Custom Labels") {
    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
        StatusIndicator(status: .success, label: "Online")
        StatusIndicator(status: .inactive, label: "Offline")
        StatusIndicator(status: .inProgress, label: "Testing...", shouldPulse: true)
        StatusIndicator(status: .error, label: "Connection Failed")
    }
    .padding()
}

#Preview("Different Sizes") {
    HStack(spacing: DesignTokens.Spacing.md) {
        StatusIndicator(status: .success, size: 6)
        StatusIndicator(status: .success, size: 8)
        StatusIndicator(status: .success, size: 10)
        StatusIndicator(status: .success, size: 12)
    }
    .padding()
}
