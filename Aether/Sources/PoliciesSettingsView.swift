//
//  PoliciesSettingsView.swift
//  Aether
//
//  Policies settings tab for configuring behavioral parameters.
//  Implements mechanism-policy separation allowing users to tune system behavior
//  through configuration without code changes.
//

import SwiftUI

/// Policies settings view with UnifiedSaveBar pattern
struct PoliciesSettingsView: View {
    // Dependencies
    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Current policy values
    @State private var policies: PoliciesConfig?

    // Saved policies (for comparison)
    @State private var savedPolicies: PoliciesConfig?

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?
    @State private var expandedSections: Set<PolicySection> = [.intent]

    enum PolicySection: String, CaseIterable {
        case intent = "Intent Detection"
        case memory = "Memory"
        case retry = "Network Retry"
        case webFetch = "Web Fetch"
        case text = "Text Format"
        case metrics = "Performance Metrics"
        case toolSafety = "Tool Safety"

        var icon: String {
            switch self {
            case .intent: return "brain.head.profile"
            case .memory: return "memorychip"
            case .retry: return "arrow.clockwise"
            case .webFetch: return "globe"
            case .text: return "text.alignleft"
            case .metrics: return "speedometer"
            case .toolSafety: return "shield"
            }
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                if let policies = policies {
                    // Intent Detection Section
                    policySectionCard(.intent) {
                        intentPolicyView(policies.intent)
                    }

                    // Memory Section
                    policySectionCard(.memory) {
                        memoryPolicyView(policies.memory)
                    }

                    // Retry Section
                    policySectionCard(.retry) {
                        retryPolicyView(policies.retry)
                    }

                    // Web Fetch Section
                    policySectionCard(.webFetch) {
                        webFetchPolicyView(policies.webFetch)
                    }

                    // Text Format Section
                    policySectionCard(.text) {
                        textPolicyView(policies.text)
                    }

                    // Metrics Section
                    policySectionCard(.metrics) {
                        metricsPolicyView(policies.metrics)
                    }

                    // Tool Safety Section
                    policySectionCard(.toolSafety) {
                        toolSafetyPolicyView(policies.toolSafety)
                    }
                } else {
                    loadingView
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadSettings()
            updateSaveBarState()
        }
        .onChange(of: policies) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
    }

    // MARK: - Section Card Builder

    @ViewBuilder
    private func policySectionCard<Content: View>(
        _ section: PolicySection,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header (always visible)
            Button(action: {
                withAnimation(.easeInOut(duration: 0.2)) {
                    if expandedSections.contains(section) {
                        expandedSections.remove(section)
                    } else {
                        expandedSections.insert(section)
                    }
                }
            }) {
                HStack {
                    Label(L("settings.policies.\(section.rawValue.lowercased().replacingOccurrences(of: " ", with: "_"))"), systemImage: section.icon)
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Image(systemName: expandedSections.contains(section) ? "chevron.up" : "chevron.down")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .padding(DesignTokens.Spacing.md)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            // Content (expandable)
            if expandedSections.contains(section) {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    Divider()
                    content()
                }
                .padding(.horizontal, DesignTokens.Spacing.md)
                .padding(.bottom, DesignTokens.Spacing.md)
            }
        }
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Policy Views

    @ViewBuilder
    private func intentPolicyView(_ intent: IntentDetectionPolicy) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            policyRow(
                title: L("settings.policies.confidence_threshold"),
                description: L("settings.policies.confidence_threshold_desc"),
                value: String(format: "%.2f", intent.confidenceThreshold)
            )

            policyRow(
                title: L("settings.policies.timeout_ms"),
                description: L("settings.policies.timeout_ms_desc"),
                value: "\(intent.timeoutMs) ms"
            )

            policyRow(
                title: L("settings.policies.min_input_length"),
                description: L("settings.policies.min_input_length_desc"),
                value: "\(intent.minInputLength)"
            )
        }
    }

    @ViewBuilder
    private func memoryPolicyView(_ memory: MemoryPolicies) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Text(L("settings.policies.compression"))
                .font(DesignTokens.Typography.caption)
                .fontWeight(.semibold)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            policyRow(
                title: L("settings.policies.idle_timeout"),
                description: L("settings.policies.idle_timeout_desc"),
                value: "\(memory.compression.idleTimeoutSeconds)s"
            )

            policyRow(
                title: L("settings.policies.turn_threshold"),
                description: L("settings.policies.turn_threshold_desc"),
                value: "\(memory.compression.turnThreshold)"
            )

            Divider()

            Text(L("settings.policies.ai_retrieval"))
                .font(DesignTokens.Typography.caption)
                .fontWeight(.semibold)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            policyRow(
                title: L("settings.policies.max_candidates"),
                description: L("settings.policies.max_candidates_desc"),
                value: "\(memory.aiRetrieval.maxCandidates)"
            )

            policyRow(
                title: L("settings.policies.fallback_count"),
                description: L("settings.policies.fallback_count_desc"),
                value: "\(memory.aiRetrieval.fallbackCount)"
            )
        }
    }

    @ViewBuilder
    private func retryPolicyView(_ retry: RetryPolicy) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            policyRow(
                title: L("settings.policies.max_retries"),
                description: L("settings.policies.max_retries_desc"),
                value: "\(retry.maxRetries)"
            )

            policyRow(
                title: L("settings.policies.initial_backoff"),
                description: L("settings.policies.initial_backoff_desc"),
                value: "\(retry.initialBackoffMs) ms"
            )

            policyRow(
                title: L("settings.policies.backoff_multiplier"),
                description: L("settings.policies.backoff_multiplier_desc"),
                value: String(format: "%.1fx", retry.backoffMultiplier)
            )

            HStack {
                Toggle(L("settings.policies.retry_on_timeout"), isOn: .constant(retry.retryOnTimeout))
                    .disabled(true)
                Spacer()
                Toggle(L("settings.policies.retry_on_network_error"), isOn: .constant(retry.retryOnNetworkError))
                    .disabled(true)
            }
            .font(DesignTokens.Typography.caption)
        }
    }

    @ViewBuilder
    private func webFetchPolicyView(_ webFetch: WebFetchPolicy) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            policyRow(
                title: L("settings.policies.max_content_length"),
                description: L("settings.policies.max_content_length_desc"),
                value: "\(webFetch.maxContentLength)"
            )

            policyRow(
                title: L("settings.policies.timeout"),
                description: L("settings.policies.web_timeout_desc"),
                value: "\(webFetch.timeoutSeconds)s"
            )

            policyRow(
                title: L("settings.policies.user_agent"),
                description: L("settings.policies.user_agent_desc"),
                value: webFetch.userAgent
            )

            Toggle(L("settings.policies.follow_redirects"), isOn: .constant(webFetch.followRedirects))
                .disabled(true)
                .font(DesignTokens.Typography.caption)
        }
    }

    @ViewBuilder
    private func textPolicyView(_ text: TextFormatPolicy) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            policyRow(
                title: L("settings.policies.default_truncate"),
                description: L("settings.policies.default_truncate_desc"),
                value: "\(text.defaultTruncateLength)"
            )

            policyRow(
                title: L("settings.policies.search_snippet"),
                description: L("settings.policies.search_snippet_desc"),
                value: "\(text.searchSnippetLength)"
            )

            policyRow(
                title: L("settings.policies.mcp_result"),
                description: L("settings.policies.mcp_result_desc"),
                value: "\(text.mcpResultLength)"
            )

            policyRow(
                title: L("settings.policies.truncation_suffix"),
                description: L("settings.policies.truncation_suffix_desc"),
                value: "\"\(text.truncationSuffix)\""
            )
        }
    }

    @ViewBuilder
    private func metricsPolicyView(_ metrics: MetricsPolicy) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Text(L("settings.policies.target_latencies"))
                .font(DesignTokens.Typography.caption)
                .fontWeight(.semibold)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            HStack(spacing: DesignTokens.Spacing.md) {
                metricBadge(L("settings.policies.hotkey_clipboard"), "\(metrics.targetHotkeyToClipboardMs)ms")
                metricBadge(L("settings.policies.clipboard_memory"), "\(metrics.targetClipboardToMemoryMs)ms")
                metricBadge(L("settings.policies.memory_ai"), "\(metrics.targetMemoryToAiMs)ms")
            }

            policyRow(
                title: L("settings.policies.warning_multiplier"),
                description: L("settings.policies.warning_multiplier_desc"),
                value: String(format: "%.1fx", metrics.warningMultiplier)
            )

            HStack {
                Toggle(L("settings.policies.enable_logging"), isOn: .constant(metrics.enableLogging))
                    .disabled(true)
                Spacer()
                Toggle(L("settings.policies.enable_warnings"), isOn: .constant(metrics.enableWarnings))
                    .disabled(true)
            }
            .font(DesignTokens.Typography.caption)
        }
    }

    @ViewBuilder
    private func toolSafetyPolicyView(_ toolSafety: ToolSafetyPolicy) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Text(L("settings.policies.safety_keywords"))
                .font(DesignTokens.Typography.caption)
                .fontWeight(.semibold)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            keywordRow(
                title: L("settings.policies.high_risk"),
                keywords: toolSafety.highRiskKeywords,
                color: .red
            )

            keywordRow(
                title: L("settings.policies.low_risk"),
                keywords: toolSafety.lowRiskKeywords,
                color: .orange
            )

            keywordRow(
                title: L("settings.policies.reversible"),
                keywords: toolSafety.reversibleKeywords,
                color: .blue
            )

            keywordRow(
                title: L("settings.policies.readonly"),
                keywords: toolSafety.readonlyKeywords,
                color: .green
            )

            Divider()

            Text(L("settings.policies.fallback_levels"))
                .font(DesignTokens.Typography.caption)
                .fontWeight(.semibold)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            HStack(spacing: DesignTokens.Spacing.sm) {
                fallbackBadge("Builtin", toolSafety.builtinFallback)
                fallbackBadge("Native", toolSafety.nativeFallback)
                fallbackBadge("MCP", toolSafety.mcpFallback)
            }
        }
    }

    // MARK: - Helper Views

    @ViewBuilder
    private func policyRow(title: String, description: String, value: String) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(DesignTokens.Typography.body)
                Text(description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
            Spacer()
            Text(value)
                .font(DesignTokens.Typography.code)
                .foregroundColor(DesignTokens.Colors.textPrimary)
        }
    }

    @ViewBuilder
    private func metricBadge(_ label: String, _ value: String) -> some View {
        VStack(spacing: 2) {
            Text(value)
                .font(DesignTokens.Typography.code)
                .foregroundColor(DesignTokens.Colors.textPrimary)
            Text(label)
                .font(.system(size: 9))
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .padding(.horizontal, DesignTokens.Spacing.sm)
        .padding(.vertical, DesignTokens.Spacing.xs)
        .background(DesignTokens.Colors.textPrimary.opacity(0.1))
        .cornerRadius(DesignTokens.CornerRadius.small)
    }

    @ViewBuilder
    private func keywordRow(title: String, keywords: [String], color: Color) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            HStack {
                Circle()
                    .fill(color)
                    .frame(width: 8, height: 8)
                Text(title)
                    .font(DesignTokens.Typography.caption)
                    .fontWeight(.medium)
                Text("(\(keywords.count))")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            FlowLayout(spacing: 4) {
                ForEach(keywords.prefix(10), id: \.self) { keyword in
                    Text(keyword)
                        .font(.system(size: 10, design: .monospaced))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(color.opacity(0.1))
                        .foregroundColor(color)
                        .cornerRadius(4)
                }
                if keywords.count > 10 {
                    Text("+\(keywords.count - 10)")
                        .font(.system(size: 10))
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
        }
    }

    @ViewBuilder
    private func fallbackBadge(_ category: String, _ level: String) -> some View {
        VStack(spacing: 2) {
            Text(category)
                .font(.system(size: 10))
                .foregroundColor(DesignTokens.Colors.textSecondary)
            Text(level)
                .font(.system(size: 9, design: .monospaced))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(safetyLevelColor(level).opacity(0.1))
                .foregroundColor(safetyLevelColor(level))
                .cornerRadius(4)
        }
    }

    private func safetyLevelColor(_ level: String) -> Color {
        switch level {
        case "readonly": return .green
        case "reversible": return .blue
        case "irreversible_low_risk": return .orange
        case "irreversible_high_risk": return .red
        default: return .gray
        }
    }

    private var loadingView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            ProgressView()
            Text(L("common.loading"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Computed Properties

    /// Check if current state differs from saved state (currently read-only)
    private var hasUnsavedChanges: Bool {
        // For now, policies view is read-only
        return false
    }

    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        return nil
    }

    // MARK: - Settings Management

    private func loadSettings() {
        Task {
            do {
                let config = try core.loadConfig()
                await MainActor.run {
                    policies = config.policies
                    savedPolicies = config.policies
                }
            } catch {
                print("Failed to load policies settings: \(error)")
                await MainActor.run {
                    errorMessage = "Failed to load: \(error.localizedDescription)"
                }
            }
        }
    }

    private func saveSettings() async {
        guard let policies = policies else { return }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            try core.updatePolicies(policies: policies)
            print("Policies settings saved successfully")

            await MainActor.run {
                savedPolicies = policies
                isSaving = false

                NotificationCenter.default.post(
                    name: .aetherConfigSavedInternally,
                    object: nil
                )
            }
        } catch {
            print("Failed to save policies: \(error)")
            await MainActor.run {
                errorMessage = "Failed to save: \(error.localizedDescription)"
                isSaving = false
            }
        }
    }

    private func cancelEditing() {
        policies = savedPolicies
        errorMessage = nil
    }

    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelEditing
        )
    }
}

// MARK: - Preview

struct PoliciesSettingsView_Previews: PreviewProvider {
    static var previews: some View {
        // Preview requires a mock core
        Text("PoliciesSettingsView Preview")
    }
}
