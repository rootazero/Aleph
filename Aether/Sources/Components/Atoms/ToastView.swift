import SwiftUI

/// Toast notification component for temporary messages
struct ToastView: View {
    // MARK: - Toast Style

    enum Style {
        case success
        case error
        case info
        case warning

        var icon: String {
            switch self {
            case .success: return "checkmark.circle.fill"
            case .error: return "xmark.circle.fill"
            case .info: return "info.circle.fill"
            case .warning: return "exclamationmark.triangle.fill"
            }
        }

        var color: Color {
            switch self {
            case .success: return DesignTokens.Colors.providerActive
            case .error: return DesignTokens.Colors.error
            case .info: return DesignTokens.Colors.info
            case .warning: return DesignTokens.Colors.warning
            }
        }
    }

    // MARK: - Properties

    /// Toast message text
    let message: String

    /// Toast style
    let style: Style

    /// Whether the toast is visible
    @Binding var isShowing: Bool

    /// Visibility state for animation
    @State private var opacity: Double = 0
    @State private var offset: CGFloat = -20

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            Image(systemName: style.icon)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(style.color)

            Text(message)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)
        }
        .padding(.horizontal, DesignTokens.Spacing.md)
        .padding(.vertical, DesignTokens.Spacing.sm)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(DesignTokens.Colors.cardBackground)
                .shadow(DesignTokens.Shadows.elevated)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .stroke(style.color.opacity(0.3), lineWidth: 1)
        )
        .opacity(opacity)
        .offset(y: offset)
        .onChange(of: isShowing) { _, newValue in
            if newValue {
                showToast()
            }
        }
        .onAppear {
            if isShowing {
                showToast()
            }
        }
    }

    // MARK: - Helpers

    /// Show toast with animation
    private func showToast() {
        // Slide in from top
        withAnimation(DesignTokens.Animation.spring) {
            opacity = 1
            offset = 0
        }

        // Auto-dismiss after 3 seconds
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
            withAnimation(DesignTokens.Animation.standard) {
                opacity = 0
                offset = -20
            }

            // Reset binding after animation completes
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                isShowing = false
            }
        }
    }
}

/// Toast modifier for easy integration
struct ToastModifier: ViewModifier {
    @Binding var toast: ToastData?

    func body(content: Content) -> some View {
        ZStack {
            content

            if let toast = toast {
                VStack {
                    ToastView(
                        message: toast.message,
                        style: toast.style,
                        isShowing: Binding(
                            get: { self.toast != nil },
                            set: { if !$0 { self.toast = nil } }
                        )
                    )
                    .padding(.top, DesignTokens.Spacing.lg)

                    Spacer()
                }
                .transition(.move(edge: .top).combined(with: .opacity))
                .zIndex(999)
            }
        }
    }
}

/// Toast data structure
struct ToastData: Equatable {
    let message: String
    let style: ToastView.Style
}

// MARK: - View Extension

extension View {
    /// Show toast notification on this view
    func toast(_ toast: Binding<ToastData?>) -> some View {
        modifier(ToastModifier(toast: toast))
    }
}

// MARK: - Preview Provider

#Preview("Success Toast") {
    struct SuccessPreview: View {
        @State private var showToast = true

        var body: some View {
            VStack {
                Spacer()
            }
            .frame(width: 400, height: 300)
            .toast(.constant(ToastData(message: "Provider saved successfully!", style: .success)))
        }
    }

    return SuccessPreview()
}

#Preview("Error Toast") {
    struct ErrorPreview: View {
        @State private var showToast = true

        var body: some View {
            VStack {
                Spacer()
            }
            .frame(width: 400, height: 300)
            .toast(.constant(ToastData(message: "Failed to connect to provider", style: .error)))
        }
    }

    return ErrorPreview()
}

#Preview("Info Toast") {
    struct InfoPreview: View {
        @State private var showToast = true

        var body: some View {
            VStack {
                Spacer()
            }
            .frame(width: 400, height: 300)
            .toast(.constant(ToastData(message: "Configuration updated", style: .info)))
        }
    }

    return InfoPreview()
}

#Preview("Warning Toast") {
    struct WarningPreview: View {
        @State private var showToast = true

        var body: some View {
            VStack {
                Spacer()
            }
            .frame(width: 400, height: 300)
            .toast(.constant(ToastData(message: "API key not found in Keychain", style: .warning)))
        }
    }

    return WarningPreview()
}
