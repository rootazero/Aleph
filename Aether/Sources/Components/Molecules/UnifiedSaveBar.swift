//
//  UnifiedSaveBar.swift
//  Aether
//
//  Unified save/cancel bar component for all settings tabs.
//  Displays at bottom of content area with status message and action buttons.
//

import SwiftUI

/// Unified save bar component providing consistent save/cancel UX across all settings tabs
///
/// **States**:
/// - Idle: No unsaved changes, buttons disabled
/// - Dirty: Unsaved changes exist, Save button highlighted, Cancel enabled
/// - Saving: Save operation in progress, buttons disabled with spinner
/// - Error: Save failed, error message displayed
///
/// **Layout**: [Status Message] [Spacer] [Cancel] [Save]
struct UnifiedSaveBar: View {
    // MARK: - Properties

    /// Whether form has unsaved changes (enables/disables buttons)
    let hasUnsavedChanges: Bool

    /// Whether save operation is in progress
    let isSaving: Bool

    /// Status message to display (e.g., "Unsaved changes", error message)
    let statusMessage: String?

    /// Callback when user clicks Save button
    let onSave: () async -> Void

    /// Callback when user clicks Cancel button
    let onCancel: () -> Void

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Left: Status message
            if let message = statusMessage {
                HStack(spacing: 6) {
                    // Warning icon for unsaved changes
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(DesignTokens.Colors.warning)

                    Text(message)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Warning: \(message)")
            }

            Spacer()

            // Right: Action buttons
            HStack(spacing: DesignTokens.Spacing.sm) {
                // Cancel button
                Button(action: onCancel) {
                    Text(LocalizedStringKey("common.cancel"))
                        .font(DesignTokens.Typography.body)
                }
                .buttonStyle(.plain)
                .foregroundColor(DesignTokens.Colors.textPrimary)
                .disabled(!hasUnsavedChanges || isSaving)
                .opacity((hasUnsavedChanges && !isSaving) ? 1.0 : 0.5)
                .help("Revert all changes (Esc)")
                .accessibilityLabel("Cancel changes")
                .accessibilityHint("Reverts all fields to last saved state")

                // Save button
                Button(action: {
                    Task {
                        await onSave()
                    }
                }) {
                    HStack(spacing: 6) {
                        if isSaving {
                            ProgressView()
                                .scaleEffect(0.7)
                                .frame(width: 12, height: 12)
                                .tint(.white)
                        }
                        Text(isSaving ? LocalizedStringKey("common.saving") : LocalizedStringKey("common.save"))
                            .font(DesignTokens.Typography.body)
                    }
                    .frame(minWidth: 60)
                    .padding(.horizontal, DesignTokens.Spacing.md)
                    .padding(.vertical, DesignTokens.Spacing.sm)
                    .background(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                            .fill(hasUnsavedChanges && !isSaving ? DesignTokens.Colors.accentBlue : DesignTokens.Colors.textSecondary.opacity(0.2))
                    )
                    .foregroundColor(.white)
                }
                .buttonStyle(.plain)
                .disabled(!hasUnsavedChanges || isSaving)
                .help("Save settings (⌘S)")
                .accessibilityLabel(isSaving ? "Saving settings" : "Save settings")
                .accessibilityHint("Commits all changes to configuration")
                .accessibilityValue(hasUnsavedChanges ? "Enabled" : "Disabled")
                .animation(.easeInOut(duration: 0.2), value: hasUnsavedChanges)
            }
        }
        .padding(.horizontal, DesignTokens.Spacing.lg)
        .padding(.vertical, DesignTokens.Spacing.md)
        .frame(height: 52)
        // No background - transparent to show content area background
        .overlay(
            Rectangle()
                .fill(DesignTokens.Colors.border)
                .frame(height: 1),
            alignment: .top  // Top border line only
        )
    }
}

// MARK: - Preview Provider

#Preview("Idle State") {
    UnifiedSaveBar(
        hasUnsavedChanges: false,
        isSaving: false,
        statusMessage: nil,
        onSave: {},
        onCancel: {}
    )
    .frame(width: 600)
}

#Preview("Unsaved Changes") {
    UnifiedSaveBar(
        hasUnsavedChanges: true,
        isSaving: false,
        statusMessage: "Unsaved changes",
        onSave: {},
        onCancel: {}
    )
    .frame(width: 600)
}

#Preview("Saving") {
    UnifiedSaveBar(
        hasUnsavedChanges: true,
        isSaving: true,
        statusMessage: "Unsaved changes",
        onSave: {},
        onCancel: {}
    )
    .frame(width: 600)
}

#Preview("Error State") {
    UnifiedSaveBar(
        hasUnsavedChanges: true,
        isSaving: false,
        statusMessage: "Failed to save: Permission denied",
        onSave: {},
        onCancel: {}
    )
    .frame(width: 600)
}
