//
//  SkillEnvVarEditor.swift
//  Aether
//
//  Editor component for managing environment variables.
//  Supports secure field display with visibility toggle.
//

import SwiftUI

/// Editor for managing skill environment variables
struct SkillEnvVarEditor: View {
    // MARK: - Properties

    /// Environment variables binding
    @Binding var envVars: [UnifiedEnvVar]

    /// Callback when env vars change
    var onChange: (() -> Void)?

    // MARK: - Body

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            Label(L("skills.env_vars"), systemImage: "key")
                .font(DesignTokens.Typography.heading)

            // Description
            Text(L("skills.env_vars_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Env vars list
            if envVars.isEmpty {
                emptyState
            } else {
                VStack(spacing: 4) {
                    ForEach(Array(envVars.enumerated()), id: \.offset) { index, envVar in
                        EnvVarRowView(
                            key: envVar.key,
                            value: envVar.value,
                            onUpdate: { key, value in
                                envVars[index] = UnifiedEnvVar(key: key, value: value)
                                onChange?()
                            },
                            onDelete: {
                                envVars.remove(at: index)
                                onChange?()
                            }
                        )
                    }
                }
            }

            // Add button
            Button(action: addEnvVar) {
                Label(L("skills.add_variable"), systemImage: "plus")
            }
            .buttonStyle(.borderless)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    // MARK: - Subviews

    private var emptyState: some View {
        HStack {
            Spacer()
            VStack(spacing: 8) {
                Image(systemName: "key.slash")
                    .font(.system(size: 24))
                    .foregroundColor(.secondary)
                Text(L("skills.no_env_vars"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
            .padding(.vertical, DesignTokens.Spacing.md)
            Spacer()
        }
    }

    // MARK: - Actions

    private func addEnvVar() {
        envVars.append(UnifiedEnvVar(key: "", value: ""))
        onChange?()
    }
}

// MARK: - Environment Variable Row

/// Single environment variable row with secure field
private struct EnvVarRowView: View {
    let key: String
    let value: String
    let onUpdate: (String, String) -> Void
    let onDelete: () -> Void

    @State private var editKey: String
    @State private var editValue: String
    @State private var isValueVisible = false
    @FocusState private var isKeyFocused: Bool
    @FocusState private var isValueFocused: Bool

    init(
        key: String,
        value: String,
        onUpdate: @escaping (String, String) -> Void,
        onDelete: @escaping () -> Void
    ) {
        self.key = key
        self.value = value
        self.onUpdate = onUpdate
        self.onDelete = onDelete
        _editKey = State(initialValue: key)
        _editValue = State(initialValue: value)
    }

    var body: some View {
        HStack(spacing: 8) {
            // Key field
            TextField("KEY", text: $editKey)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .frame(width: 150)
                .focused($isKeyFocused)
                .onChange(of: editKey) { _, newValue in
                    onUpdate(newValue, editValue)
                }

            // Value field (secure or visible)
            Group {
                if isValueVisible {
                    TextField(L("skills.env_var_value"), text: $editValue)
                } else {
                    SecureField(L("skills.env_var_value"), text: $editValue)
                }
            }
            .textFieldStyle(.roundedBorder)
            .font(.system(.body, design: .monospaced))
            .focused($isValueFocused)
            .onChange(of: editValue) { _, newValue in
                onUpdate(editKey, newValue)
            }

            // Visibility toggle
            Button(action: { isValueVisible.toggle() }) {
                Image(systemName: isValueVisible ? "eye.slash" : "eye")
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.borderless)
            .help(isValueVisible ? L("skills.hide_value") : L("skills.show_value"))

            // Delete button
            Button(action: onDelete) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.borderless)
            .help(L("skills.remove_variable"))
        }
        .padding(.vertical, 4)
    }
}

// MARK: - Preview Provider

#Preview("Empty State") {
    SkillEnvVarEditor(envVars: .constant([]))
        .padding()
        .frame(width: 500)
}

#Preview("With Variables") {
    SkillEnvVarEditor(
        envVars: .constant([
            UnifiedEnvVar(key: "LINEAR_API_KEY", value: "lin_api_xxxxx"),
            UnifiedEnvVar(key: "GITHUB_TOKEN", value: "ghp_yyyyyyy"),
            UnifiedEnvVar(key: "NODE_ENV", value: "production")
        ])
    )
    .padding()
    .frame(width: 500)
}

#Preview("Single Variable") {
    SkillEnvVarEditor(
        envVars: .constant([
            UnifiedEnvVar(key: "API_KEY", value: "secret-value-here")
        ])
    )
    .padding()
    .frame(width: 500)
}
