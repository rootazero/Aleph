import SwiftUI

/// A reusable action button with multiple style variants and icon support
struct ActionButton: View {
    // MARK: - Button Style

    /// Available button style variants
    enum Style {
        case primary
        case secondary
        case danger
        case glass           // Liquid Glass style for glass windows
        case glassProminent  // Prominent glass button (like send button)

        /// Background color for each style
        var backgroundColor: Color {
            switch self {
            case .primary:
                return DesignTokens.Colors.accentBlue
            case .secondary:
                return Color.clear
            case .danger:
                return DesignTokens.Colors.error
            case .glass:
                return Color.white.opacity(0.10)
            case .glassProminent:
                return Color.white.opacity(0.15)
            }
        }

        /// Foreground (text/icon) color for each style
        var foregroundColor: Color {
            switch self {
            case .primary, .danger:
                return .white
            case .secondary:
                return DesignTokens.Colors.textPrimary
            case .glass, .glassProminent:
                return .white  // System applies vibrant treatment on macOS 26+
            }
        }

        /// Border color for each style
        var borderColor: Color {
            switch self {
            case .primary, .danger:
                return Color.clear
            case .secondary:
                return DesignTokens.Colors.border
            case .glass:
                return Color.white.opacity(0.15)
            case .glassProminent:
                return Color.white.opacity(0.20)
            }
        }

        /// Whether this is a glass style (requires special rendering)
        var isGlassStyle: Bool {
            switch self {
            case .glass, .glassProminent:
                return true
            default:
                return false
            }
        }
    }

    /// Available button size variants
    enum Size {
        case small
        case medium
        case large

        /// Horizontal padding for each size
        var horizontalPadding: CGFloat {
            switch self {
            case .small: return DesignTokens.Spacing.sm
            case .medium: return DesignTokens.Spacing.md
            case .large: return DesignTokens.Spacing.lg
            }
        }

        /// Vertical padding for each size
        var verticalPadding: CGFloat {
            switch self {
            case .small: return DesignTokens.Spacing.xs
            case .medium: return DesignTokens.Spacing.sm
            case .large: return DesignTokens.Spacing.md
            }
        }

        /// Font for each size
        var font: Font {
            switch self {
            case .small: return DesignTokens.Typography.caption
            case .medium: return DesignTokens.Typography.body
            case .large: return DesignTokens.Typography.heading
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

    /// Button size variant
    let size: Size

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
        size: Size = .medium,
        isDisabled: Bool = false,
        action: @escaping () -> Void
    ) {
        self.title = title
        self.icon = icon
        self.style = style
        self.size = size
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
                        .font(size.font)
                }

                // Button title
                Text(title)
                    .font(size.font)
            }
            .padding(.horizontal, size.horizontalPadding)
            .padding(.vertical, size.verticalPadding)
            .foregroundColor(isDisabled ? disabledTextColor : style.foregroundColor)
            .background(
                // Render background based on style
                buttonBackground
            )
            .overlay(
                RoundedRectangle(cornerRadius: buttonCornerRadius)
                    .stroke(style.borderColor, lineWidth: style.isGlassStyle ? 0.5 : 1)
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

    // MARK: - Helper Views

    /// Button background with glass effect support
    @ViewBuilder
    private var buttonBackground: some View {
        if style.isGlassStyle {
            if #available(macOS 26.0, *) {
                // macOS 26+: Use native glass effect
                RoundedRectangle(cornerRadius: buttonCornerRadius)
                    .fill(style.backgroundColor)
                    .glassEffect(
                        style == .glassProminent ? .regular.interactive() : .regular,
                        in: RoundedRectangle(cornerRadius: buttonCornerRadius)
                    )
            } else {
                // Fallback: Semi-transparent background
                RoundedRectangle(cornerRadius: buttonCornerRadius)
                    .fill(style.backgroundColor)
            }
        } else {
            // Non-glass styles: regular fill
            RoundedRectangle(cornerRadius: buttonCornerRadius)
                .fill(isDisabled ? DesignTokens.Colors.border : style.backgroundColor)
        }
    }

    /// Corner radius based on size
    private var buttonCornerRadius: CGFloat {
        switch size {
        case .small:
            return 6
        case .medium:
            return 8
        case .large:
            return 10
        }
    }

    /// Disabled text color based on style
    private var disabledTextColor: Color {
        style.isGlassStyle ? Color.white.opacity(0.4) : DesignTokens.Colors.textDisabled
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

        // Glass styles (shown on glass background)
        VStack(spacing: 12) {
            ActionButton("Glass Button", icon: "sparkles", style: .glass) {
                print("Glass tapped")
            }

            ActionButton("Glass Prominent", icon: "arrow.up", style: .glassProminent) {
                print("Glass Prominent tapped")
            }
        }
        .padding()
        .adaptiveGlass()
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

#Preview("Glass Buttons on Glass Surface") {
    ZStack {
        // Simulated glass background
        LinearGradient(
            colors: [.blue.opacity(0.6), .purple.opacity(0.6)],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )

        VStack(spacing: 16) {
            Text("Glass Button Styles")
                .font(.headline)
                .liquidGlassText()

            ActionButton("Confirm", icon: "checkmark", style: .glass) {}
            ActionButton("Cancel", icon: "xmark", style: .glass) {}
            ActionButton("Send", icon: "arrow.up", style: .glassProminent) {}

            HStack(spacing: 12) {
                ActionButton("Retry", icon: "arrow.clockwise", style: .glass, size: .small) {}
                ActionButton("Skip", icon: "arrow.forward", style: .glass, size: .small) {}
            }
        }
        .padding(20)
        .adaptiveGlass()
    }
    .frame(width: 320, height: 360)
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
