//
//  HaloToastView.swift
//  Aether
//
//  Toast notification view for Halo overlay.
//  Replaces system NSAlert with a native, non-intrusive toast.
//

import SwiftUI

/// Toast notification view with light background and dynamic sizing
struct HaloToastView: View {
    let type: ToastType
    let title: String
    let message: String
    let onDismiss: (() -> Void)?

    @State private var isAppearing = false
    @State private var isHoveringClose = false

    // Design constants
    private let minWidth: CGFloat = 200
    private let maxWidth: CGFloat = 400
    private let padding: CGFloat = 16
    private let cornerRadius: CGFloat = 12
    private let iconSize: CGFloat = 24

    var body: some View {
        toastContent
            .padding(padding)
            .frame(minWidth: minWidth, maxWidth: maxWidth)
            .background(backgroundView)
            .overlay(borderOverlay)
            .scaleEffect(isAppearing ? 1.0 : 0.9)
            .opacity(isAppearing ? 1.0 : 0.0)
            .onAppear(perform: animateAppearance)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(type.displayName): \(title)")
            .accessibilityValue(message)
            .accessibilityHint("Double tap close button to dismiss")
            .accessibilityAddTraits(.isStaticText)
    }

    // MARK: - Subviews

    private var toastContent: some View {
        HStack(alignment: .top, spacing: 12) {
            iconView
            textContent
            Spacer(minLength: 8)
            closeButton
        }
    }

    private var iconView: some View {
        Image(systemName: type.iconName)
            .font(.system(size: iconSize, weight: .medium))
            .foregroundColor(type.accentColor)
            .frame(width: iconSize, height: iconSize)
    }

    private var textContent: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(Color.primary)
                .lineLimit(1)

            if !message.isEmpty {
                Text(message)
                    .font(.system(size: 12, weight: .regular))
                    .foregroundColor(Color.secondary)
                    .lineLimit(5)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: 300, alignment: .leading)
    }

    private var closeButton: some View {
        Button(action: dismissWithAnimation) {
            Image(systemName: "xmark")
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(isHoveringClose ? Color.primary : Color.secondary)
                .frame(width: 16, height: 16)
                .background(closeButtonBackground)
        }
        .buttonStyle(.plain)
        .scaleEffect(isHoveringClose ? 1.1 : 1.0)
        .animation(.easeInOut(duration: 0.15), value: isHoveringClose)
        .onHover { isHoveringClose = $0 }
        .accessibilityLabel("Close")
        .accessibilityHint("Dismiss this notification")
    }

    private var closeButtonBackground: some View {
        Circle()
            .fill(isHoveringClose ? Color.gray.opacity(0.2) : Color.clear)
    }

    private var backgroundView: some View {
        RoundedRectangle(cornerRadius: cornerRadius)
            .fill(.regularMaterial)
            .shadow(color: Color.black.opacity(0.15), radius: 10, x: 0, y: 4)
    }

    private var borderOverlay: some View {
        RoundedRectangle(cornerRadius: cornerRadius)
            .stroke(type.accentColor.opacity(0.3), lineWidth: 1)
    }

    // MARK: - Actions

    private func animateAppearance() {
        withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
            isAppearing = true
        }
    }

    private func dismissWithAnimation() {
        withAnimation(.easeOut(duration: 0.2)) {
            isAppearing = false
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            onDismiss?()
        }
    }
}

// MARK: - Preview

#Preview("Toast Types") {
    VStack(spacing: 20) {
        HaloToastView(
            type: .info,
            title: "Export Successful",
            message: "Your routing rules have been exported to routing-rules.json",
            onDismiss: { print("Dismissed") }
        )

        HaloToastView(
            type: .warning,
            title: "Provider Not Set",
            message: "Could not set 'openai' as default provider.",
            onDismiss: { print("Dismissed") }
        )

        HaloToastView(
            type: .error,
            title: "Initialization Failed",
            message: "Failed to initialize Aether core.",
            onDismiss: { print("Dismissed") }
        )

        HaloToastView(
            type: .info,
            title: "Saved",
            message: "",
            onDismiss: { print("Dismissed") }
        )
    }
    .padding(40)
    .background(Color.gray.opacity(0.2))
}
