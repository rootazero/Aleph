import SwiftUI

/// Provider status for search providers
enum SearchProviderStatus: Equatable {
    case notConfigured
    case configured  // Configured but not tested
    case testing
    case available(latency: UInt32)
    case error(String)

    var icon: String {
        switch self {
        case .notConfigured: return "exclamationmark.triangle.fill"
        case .configured: return "checkmark.circle"
        case .testing: return "arrow.triangle.2.circlepath"
        case .available: return "checkmark.circle.fill"
        case .error: return "xmark.circle.fill"
        }
    }

    var color: Color {
        switch self {
        case .notConfigured: return .orange
        case .configured: return .blue
        case .testing: return .blue
        case .available: return .green
        case .error: return .red
        }
    }

    var text: String {
        switch self {
        case .notConfigured: return L("settings.search.status.not_configured")
        case .configured: return L("settings.search.status.configured")
        case .testing: return L("settings.search.status.testing")
        case .available(let latency): return L("settings.search.status.available") + " (\(latency)ms)"
        case .error(let message): return message
        }
    }
}

/// A card component for search provider configuration
struct SearchProviderCard: View {
    // MARK: - Properties

    /// Search provider preset template
    let preset: SearchProviderPreset

    /// Current field values
    @Binding var fieldValues: [String: String]

    /// Current provider status
    @State private var status: SearchProviderStatus = .notConfigured

    /// Callback for testing connection
    let onTestConnection: (String, [String: String]) async -> ProviderTestResult

    /// Whether this card is expanded
    @State private var isExpanded: Bool = false

    /// Hover state for visual feedback
    @State private var isHovered = false

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Header
            header

            // Body (fields) - shown when expanded
            if isExpanded {
                Divider()
                    .padding(.horizontal, DesignTokens.Spacing.md)

                fieldsSection
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(DesignTokens.Colors.cardBackground)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .strokeBorder(
                    isHovered ? DesignTokens.Colors.borderHover : DesignTokens.Colors.border,
                    lineWidth: 1
                )
        )
        .onHover { hovering in
            isHovered = hovering
        }
        .onAppear {
            // Check initial configuration status
            updateStatusFromFields()
        }
        .onChange(of: fieldValues) { _, _ in
            updateStatusFromFields()
        }
        .animation(.easeInOut(duration: 0.2), value: isExpanded)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Icon
            Image(systemName: preset.iconName)
                .font(.system(size: 24))
                .foregroundColor(Color(hex: preset.color) ?? DesignTokens.Colors.accentBlue)
                .frame(width: 40, height: 40)
                .background(
                    Circle()
                        .fill((Color(hex: preset.color) ?? DesignTokens.Colors.accentBlue).opacity(0.1))
                )

            // Name and description
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text(preset.displayName)
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Text(preset.description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .lineLimit(2)
            }

            Spacer()

            // Status badge
            statusBadge

            // Expand/collapse button
            Button(action: {
                withAnimation {
                    isExpanded.toggle()
                }
            }) {
                Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
            .buttonStyle(.plain)
        }
        .padding(DesignTokens.Spacing.md)
    }

    // MARK: - Status Badge

    private var statusBadge: some View {
        HStack(spacing: DesignTokens.Spacing.xs) {
            Image(systemName: status.icon)
                .font(.system(size: 12))

            Text(status.text)
                .font(DesignTokens.Typography.caption)
        }
        .foregroundColor(.white)
        .padding(.horizontal, DesignTokens.Spacing.sm)
        .padding(.vertical, 4)
        .background(
            Capsule()
                .fill(status.color)
        )
    }

    // MARK: - Fields Section

    private var fieldsSection: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            // Dynamic fields from preset
            ForEach(preset.fields, id: \.key) { field in
                fieldRow(for: field)
            }

            // Footer: Test button and docs link
            footer
        }
        .padding(DesignTokens.Spacing.md)
    }

    // MARK: - Field Row

    @ViewBuilder
    private func fieldRow(for field: SearchPresetField) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            // Field label
            HStack(spacing: DesignTokens.Spacing.xs) {
                Text(field.displayName)
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                if field.required {
                    Text("*")
                        .foregroundColor(.red)
                }
            }

            // Field input
            switch field.type {
            case .secureText:
                SecureField(field.placeholder ?? "", text: binding(for: field.key))
                    .textFieldStyle(.roundedBorder)

            case .text:
                TextField(field.placeholder ?? "", text: binding(for: field.key))
                    .textFieldStyle(.roundedBorder)

            case .picker:
                if let options = field.options {
                    Picker("", selection: binding(for: field.key)) {
                        ForEach(options, id: \.self) { option in
                            Text(option).tag(option)
                        }
                    }
                    .pickerStyle(.segmented)
                }
            }
        }
    }

    // MARK: - Footer

    private var footer: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Test Connection button
            Button(action: {
                Task {
                    await testConnection()
                }
            }) {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    if case .testing = status {
                        ProgressView()
                            .scaleEffect(0.7)
                    } else {
                        Image(systemName: "bolt.fill")
                    }
                    Text(L("settings.search.test_connection"))
                }
                .font(DesignTokens.Typography.body)
                .foregroundColor(.white)
                .padding(.horizontal, DesignTokens.Spacing.md)
                .padding(.vertical, DesignTokens.Spacing.sm)
                .background(
                    Capsule()
                        .fill(Color(hex: preset.color) ?? DesignTokens.Colors.accentBlue)
                )
            }
            .buttonStyle(.plain)
            .disabled(status == .testing || !isConfigured)

            Spacer()

            // Get Free API link (if available)
            if let getApiKeyURL = preset.getApiKeyURL {
                Link(destination: getApiKeyURL) {
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Image(systemName: "key.fill")
                        Text(L("settings.search.get_api_key"))
                    }
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(Color(hex: preset.color) ?? DesignTokens.Colors.accentBlue)
                }
            }

            // Documentation link
            Link(destination: preset.docsURL) {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "book.fill")
                    Text(L("settings.search.documentation"))
                }
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
    }

    // MARK: - Helper Methods

    /// Get binding for field value
    private func binding(for key: String) -> Binding<String> {
        Binding(
            get: {
                // Return existing value or default value
                if let value = fieldValues[key] {
                    return value
                }
                if let field = preset.fields.first(where: { $0.key == key }),
                   let defaultValue = field.defaultValue {
                    return defaultValue
                }
                return ""
            },
            set: { newValue in
                fieldValues[key] = newValue
                // Update status when fields change
                updateStatusFromFields()
            }
        )
    }

    /// Check if provider is configured (all required fields have values)
    private var isConfigured: Bool {
        preset.fields.filter { $0.required }.allSatisfy { field in
            if let value = fieldValues[field.key], !value.isEmpty {
                return true
            }
            return false
        }
    }

    /// Update status based on field values
    private func updateStatusFromFields() {
        if isConfigured {
            // Only update to .configured if currently not configured
            // Don't change if already testing or has test result
            switch status {
            case .notConfigured:
                status = .configured
            case .configured, .testing, .available, .error:
                // Keep existing status
                break
            }
        } else {
            status = .notConfigured
        }
    }

    /// Test connection to provider
    func testConnection() async {
        guard isConfigured else { return }

        status = .testing

        let result = await onTestConnection(preset.id, fieldValues)

        if result.success {
            status = .available(latency: result.latencyMs)
        } else {
            status = .error(result.errorMessage)
        }
    }
}

// MARK: - Preview

#Preview {
    @Previewable @State var fieldValues: [String: String] = [
        "api_key": "",
        "search_depth": "basic"
    ]

    SearchProviderCard(
        preset: SearchProviderPresets.all[0],  // Tavily
        fieldValues: $fieldValues,
        onTestConnection: { _, _ in
            // Simulate test delay
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            return ProviderTestResult(
                success: true,
                latencyMs: 120,
                errorMessage: "",
                errorType: ""
            )
        }
    )
    .padding()
    .frame(width: 600)
}
