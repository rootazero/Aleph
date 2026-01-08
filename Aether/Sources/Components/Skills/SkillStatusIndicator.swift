//
//  SkillStatusIndicator.swift
//  Aether
//
//  Status indicator for unified skills (MCP servers and prompt templates).
//  Maps UnifiedSkillStatus to visual representation.
//

import SwiftUI

/// Status indicator showing skill state with color and optional label
struct SkillStatusIndicator: View {
    // MARK: - Properties

    /// Current skill status from Rust core
    let status: UnifiedSkillStatus

    /// Whether to show text label
    let showLabel: Bool

    /// Custom label override
    let customLabel: String?

    /// Size of the indicator dot
    let size: CGFloat

    /// Whether to animate (for Starting state)
    @State private var isAnimating = false

    // MARK: - Initialization

    init(
        status: UnifiedSkillStatus,
        showLabel: Bool = true,
        customLabel: String? = nil,
        size: CGFloat = 8
    ) {
        self.status = status
        self.showLabel = showLabel
        self.customLabel = customLabel
        self.size = size
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.xs) {
            // Status dot
            Circle()
                .fill(statusColor)
                .frame(width: size, height: size)
                .opacity(isAnimating ? 0.5 : 1.0)
                .scaleEffect(isAnimating ? 1.2 : 1.0)
                .animation(
                    status == .starting
                        ? Animation.easeInOut(duration: 0.8).repeatForever(autoreverses: true)
                        : nil,
                    value: isAnimating
                )

            // Label
            if showLabel {
                Text(customLabel ?? statusLabel)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
        .onAppear {
            if status == .starting {
                isAnimating = true
            }
        }
        .onChange(of: status) { _, newValue in
            isAnimating = newValue == .starting
        }
    }

    // MARK: - Computed Properties

    /// Color for each status
    private var statusColor: Color {
        switch status {
        case .running:
            return DesignTokens.Colors.providerActive
        case .stopped:
            return DesignTokens.Colors.providerInactive
        case .starting:
            return DesignTokens.Colors.warning
        case .error:
            return DesignTokens.Colors.error
        }
    }

    /// Localized label for each status
    private var statusLabel: String {
        switch status {
        case .running:
            return L("skills.status.running")
        case .stopped:
            return L("skills.status.stopped")
        case .starting:
            return L("skills.status.starting")
        case .error:
            return L("skills.status.error")
        }
    }
}

// MARK: - Capsule Style Variant

/// Status indicator with capsule background (for headers)
struct SkillStatusBadge: View {
    let status: UnifiedSkillStatus

    var body: some View {
        HStack(spacing: 4) {
            SkillStatusIndicator(status: status, showLabel: false, size: 6)
            Text(statusText)
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Color.secondary.opacity(0.1))
        .clipShape(Capsule())
    }

    private var statusText: String {
        switch status {
        case .running: return L("skills.status.running")
        case .stopped: return L("skills.status.stopped")
        case .starting: return L("skills.status.starting")
        case .error: return L("skills.status.error")
        }
    }
}

// MARK: - Preview Provider

#Preview("All Status Types") {
    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
        SkillStatusIndicator(status: .running)
        SkillStatusIndicator(status: .stopped)
        SkillStatusIndicator(status: .starting)
        SkillStatusIndicator(status: .error)
    }
    .padding()
}

#Preview("Without Labels") {
    HStack(spacing: DesignTokens.Spacing.md) {
        SkillStatusIndicator(status: .running, showLabel: false)
        SkillStatusIndicator(status: .stopped, showLabel: false)
        SkillStatusIndicator(status: .starting, showLabel: false)
        SkillStatusIndicator(status: .error, showLabel: false)
    }
    .padding()
}

#Preview("Badge Style") {
    VStack(spacing: DesignTokens.Spacing.md) {
        SkillStatusBadge(status: .running)
        SkillStatusBadge(status: .stopped)
        SkillStatusBadge(status: .starting)
        SkillStatusBadge(status: .error)
    }
    .padding()
}

#Preview("Custom Labels") {
    VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
        SkillStatusIndicator(status: .running, customLabel: "Active")
        SkillStatusIndicator(status: .stopped, customLabel: "Disabled")
        SkillStatusIndicator(status: .error, customLabel: "Connection Failed")
    }
    .padding()
}
