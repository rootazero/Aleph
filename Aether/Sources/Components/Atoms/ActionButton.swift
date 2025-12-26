import SwiftUI

/// A reusable action button with multiple style variants and icon support
struct ActionButton: View {
    // MARK: - Button Style

    /// Available button style variants
    enum Style {
        case primary
        case secondary
        case danger

        /// Background color for each style
        var backgroundColor: Color {
            switch self {
            case .primary:
                return DesignTokens.Colors.accentBlue
            case .secondary:
                return Color.clear
            case .danger:
                return DesignTokens.Colors.error
            }
        }

        /// Foreground (text/icon) color for each style
        var foregroundColor: Color {
            switch self {
            case .primary, .danger:
                return .white
            case .secondary:
                return DesignTokens.Colors.textPrimary
            }
        }

        /// Border color for each style
        var borderColor: Color {
            switch self {
            case .primary, .danger:
                return Color.clear
            case .secondary:
                return DesignTokens.Colors.border
            }
        }
    }

    // MARK: - Properties

    /// Button title text
    let title: String

    /// Optional SF Symbol icon name
    let icon: String?

    /// Button style variant
    let style: Style

    /// Whether the button is disabled
    let isDisabled: Bool

    /// Action to perform when button is tapped
    let action: () -> Void

    /// Pressed state for animation
    @State private var isPressed = false

    // MARK: - Initialization

    init(
        _ title: String,
        icon: String? = nil,
        style: Style = .primary,
        isDisabled: Bool = false,
        action: @escaping () -> Void
    ) {
        self.title = title
        self.icon = icon
        self.style = style
        self.isDisabled = isDisabled
        self.action = action
    }

    // MARK: - Body

    var body: some View {
        Button(action: handleTap) {
            HStack(spacing: DesignTokens.Spacing.xs) {
                // Optional leading icon
                if let icon = icon {
                    Image(systemName: icon)
                        .font(DesignTokens.Typography.body)
                }

                // Button title
                Text(title)
                    .font(DesignTokens.Typography.body)
            }
            .padding(.horizontal, DesignTokens.Spacing.md)
            .padding(.vertical, DesignTokens.Spacing.sm)
            .foregroundColor(isDisabled ? DesignTokens.Colors.textDisabled : style.foregroundColor)
            .background(
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                    .fill(isDisabled ? DesignTokens.Colors.border : style.backgroundColor)
            )
            .overlay(
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                    .stroke(style.borderColor, lineWidth: 1)
            )
            .scaleEffect(isPressed ? 0.95 : 1.0)
            .animation(DesignTokens.Animation.quick, value: isPressed)
        }
        .buttonStyle(.plain)
        .disabled(isDisabled)
        .simultaneousGesture(
            DragGesture(minimumDistance: 0)
                .onChanged { _ in
                    if !isDisabled {
                        isPressed = true
                    }
                }
                .onEnded { _ in
                    isPressed = false
                }
        )
    }

    // MARK: - Actions

    /// Handle button tap with haptic feedback
    private func handleTap() {
        guard !isDisabled else { return }
        action()
    }
}

// MARK: - Preview Provider

#Preview("Button Styles") {
    VStack(spacing: DesignTokens.Spacing.md) {
        ActionButton("Primary Button", style: .primary) {
            print("Primary tapped")
        }

        ActionButton("Secondary Button", style: .secondary) {
            print("Secondary tapped")
        }

        ActionButton("Danger Button", style: .danger) {
            print("Danger tapped")
        }
    }
    .padding()
}

#Preview("With Icons") {
    VStack(spacing: DesignTokens.Spacing.md) {
        ActionButton("Add Provider", icon: "plus.circle", style: .primary) {
            print("Add tapped")
        }

        ActionButton("Test Connection", icon: "network", style: .secondary) {
            print("Test tapped")
        }

        ActionButton("Delete", icon: "trash", style: .danger) {
            print("Delete tapped")
        }
    }
    .padding()
}

#Preview("Disabled States") {
    VStack(spacing: DesignTokens.Spacing.md) {
        ActionButton("Disabled Primary", style: .primary, isDisabled: true) {
            print("Should not print")
        }

        ActionButton("Disabled Secondary", icon: "gear", style: .secondary, isDisabled: true) {
            print("Should not print")
        }

        ActionButton("Disabled Danger", icon: "trash", style: .danger, isDisabled: true) {
            print("Should not print")
        }
    }
    .padding()
}

#Preview("Various Combinations") {
    VStack(spacing: DesignTokens.Spacing.md) {
        ActionButton("Save Changes", icon: "checkmark.circle") {}
        ActionButton("Cancel", style: .secondary) {}
        ActionButton("Export Settings", icon: "square.and.arrow.up", style: .secondary) {}
        ActionButton("Reset to Defaults", icon: "arrow.counterclockwise", style: .danger) {}
    }
    .padding()
}
