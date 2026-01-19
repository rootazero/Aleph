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
            // Left: Status message - always show status (saved or unsaved)
            HStack(spacing: 6) {
                if hasUnsavedChanges {
                    // Unsaved changes - warning icon
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(DesignTokens.Colors.warning)

                    Text(L("settings.save_bar.changes_unsaved"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                } else {
                    // All saved - checkmark icon
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundColor(DesignTokens.Colors.success)

                    Text(L("settings.save_bar.all_changes_saved"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                // Error message (if provided via statusMessage parameter)
                if let message = statusMessage, message.contains("Failed") || message.contains("Error") || message.contains("失败") || message.contains("错误") {
                    Text("• \(message)")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.error)
                }
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(hasUnsavedChanges ? "Warning: Changes unsaved" : "All changes saved")

            Spacer()

            // Right: Action buttons
            HStack(spacing: DesignTokens.Spacing.sm) {
                // Cancel button
                Button(action: onCancel) {
                    Text(L("common.cancel"))
                        .font(DesignTokens.Typography.body)
                        .foregroundColor((hasUnsavedChanges && !isSaving) ? DesignTokens.Colors.textPrimary : DesignTokens.Colors.textSecondary)
                }
                .buttonStyle(.plain)
                .disabled(!hasUnsavedChanges || isSaving)
                .help("Revert all changes (Esc)")
                .accessibilityLabel("Cancel changes")
                .accessibilityHint("Reverts all fields to last saved state")

                // Save button
                Button(action: {
                    print("[UnifiedSaveBar] Save button clicked, hasUnsavedChanges: \(hasUnsavedChanges), isSaving: \(isSaving)")
                    Task {
                        print("[UnifiedSaveBar] Calling onSave...")
                        await onSave()
                        print("[UnifiedSaveBar] onSave completed")
                    }
                }) {
                    HStack(spacing: 6) {
                        if isSaving {
                            ProgressView()
                                .scaleEffect(0.7)
                                .frame(width: 12, height: 12)
                                .tint(.white)
                        }
                        Text(isSaving ? L("common.saving") : L("common.save"))
                            .font(DesignTokens.Typography.body)
                    }
                    .frame(minWidth: 60)
                    .padding(.horizontal, DesignTokens.Spacing.md)
                    .padding(.vertical, DesignTokens.Spacing.sm)
                    .background(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                            .fill(hasUnsavedChanges && !isSaving ? DesignTokens.Colors.accentBlue : DesignTokens.Colors.textSecondary.opacity(0.15))
                    )
                    .foregroundColor(hasUnsavedChanges && !isSaving ? .white : DesignTokens.Colors.textSecondary)
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
        statusMessage: nil,
        onSave: {},
        onCancel: {}
    )
    .frame(width: 600)
}

#Preview("Saving") {
    UnifiedSaveBar(
        hasUnsavedChanges: true,
        isSaving: true,
        statusMessage: nil,
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
