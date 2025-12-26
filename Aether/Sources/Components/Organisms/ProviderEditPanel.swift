import SwiftUI
import AppKit

/// Unified panel for viewing and editing provider configurations
/// Replaces separate ProviderDetailPanel + ProviderConfigView modal
struct ProviderEditPanel: View {
    // MARK: - Dependencies

    let core: AetherCore
    let keychainManager: KeychainManagerImpl

    // MARK: - Bindings

    @Binding var providers: [ProviderConfigEntry]
    @Binding var selectedProvider: String?
    @Binding var isAddingNew: Bool  // NEW: External control for adding new provider

    // MARK: - State

    // Edit mode toggle
    @State private var isEditing: Bool = false

    // Form fields
    @State private var providerName: String = ""
    @State private var providerType: String = "openai"
    @State private var apiKey: String = ""
    @State private var model: String = ""
    @State private var baseURL: String = ""
    @State private var color: Color = .blue
    @State private var timeoutSeconds: String = "30"
    @State private var maxTokens: String = ""
    @State private var temperature: String = ""

    // Provider active state
    @State private var isProviderActive: Bool = false

    // UI state
    @State private var isSaving: Bool = false
    @State private var isTesting: Bool = false
    @State private var testResult: TestResult?
    @State private var errorMessage: String?
    @State private var showDeleteConfirmation: Bool = false

    // Section expansion states
    @State private var isConfigExpanded = true
    @State private var isAdvancedExpanded = false

    enum TestResult {
        case success(String)
        case failure(String)
    }

    // MARK: - Constants

    private let providerTypes = ["openai", "claude", "ollama", "custom"]

    private let defaultColors: [String: Color] = [
        "openai": Color(hex: "#10a37f") ?? .green,
        "claude": Color(hex: "#d97757") ?? .orange,
        "ollama": .black,
        "custom": .gray
    ]

    // MARK: - Computed Properties

    /// Current provider being viewed/edited
    private var currentProvider: ProviderConfigEntry? {
        guard let name = selectedProvider else { return nil }
        return providers.first { $0.name == name }
    }

    /// Check if provider has API key configured
    private var hasApiKey: Bool {
        guard let provider = currentProvider else { return false }
        if let apiKey = provider.config.apiKey, apiKey.starts(with: "keychain:") {
            do {
                return try keychainManager.hasApiKey(provider: provider.name)
            } catch {
                return false
            }
        }
        return provider.config.apiKey != nil || provider.config.providerType == "ollama"
    }

    // MARK: - Body

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                if isAddingNew || isEditing {
                    // Edit mode: Editable form
                    editModeContent
                } else if let provider = currentProvider {
                    // View mode: Read-only information
                    viewModeContent(for: provider)
                } else {
                    // No provider selected
                    emptyStateView
                }
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(DesignTokens.Colors.contentBackground)
        .onChange(of: isAddingNew) { newValue in
            if newValue {
                startNewProvider()
            }
        }
    }

    // MARK: - View Builders: View Mode

    @ViewBuilder
    private func viewModeContent(for provider: ProviderConfigEntry) -> some View {
        // Header
        headerSection(for: provider)

        Divider()

        // Description
        descriptionSection(for: provider)

        // Configuration (read-only)
        viewModeConfigSection(for: provider)

        Spacer()

        // Action buttons
        viewModeActionButtons(for: provider)
    }

    @ViewBuilder
    private func headerSection(for provider: ProviderConfigEntry) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            HStack(spacing: DesignTokens.Spacing.sm) {
                // Provider icon
                ZStack {
                    Circle()
                        .fill(Color(hex: provider.config.color) ?? DesignTokens.Colors.accentBlue)
                        .frame(width: 32, height: 32)

                    Image(systemName: getProviderIconName(provider.config.providerType))
                        .font(.system(size: 16))
                        .foregroundColor(.white)
                }

                Text(provider.name)
                    .font(DesignTokens.Typography.title)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                // Active/Inactive badge and toggle
                HStack(spacing: DesignTokens.Spacing.sm) {
                    // Active/Inactive badge
                    Text(hasApiKey ? "Active" : "Inactive")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(.white)
                        .padding(.horizontal, DesignTokens.Spacing.sm)
                        .padding(.vertical, 4)
                        .background(
                            Capsule()
                                .fill(hasApiKey ? Color.green : DesignTokens.Colors.textSecondary.opacity(0.5))
                        )

                    // Toggle switch (read-only in view mode, shows current state)
                    Toggle("", isOn: .constant(hasApiKey))
                        .labelsHidden()
                        .disabled(true)
                }
            }
        }
    }

    @ViewBuilder
    private func descriptionSection(for provider: ProviderConfigEntry) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text("About")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(getProviderDescription(provider.config.providerType))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    @ViewBuilder
    private func viewModeConfigSection(for provider: ProviderConfigEntry) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text("Configuration")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                configRow(label: "Provider Type", value: getProviderTypeName(provider.config.providerType))
                configRow(label: "Model", value: provider.config.model)

                if let baseUrl = provider.config.baseUrl {
                    configRow(label: "Base URL", value: baseUrl)
                }

                configRow(
                    label: "Max Tokens",
                    value: provider.config.maxTokens.map { "\($0)" } ?? "Default"
                )

                configRow(
                    label: "Temperature",
                    value: provider.config.temperature.map { String(format: "%.1f", $0) } ?? "Default"
                )

                configRow(
                    label: "API Key",
                    value: hasApiKey ? "••••••••" : "Not configured"
                )
            }
        }
    }

    @ViewBuilder
    private func viewModeActionButtons(for provider: ProviderConfigEntry) -> some View {
        VStack(spacing: DesignTokens.Spacing.sm) {
            ActionButton(
                "Edit Configuration",
                icon: "pencil",
                style: .primary,
                action: { enterEditMode() }
            )

            ActionButton(
                "Delete Provider",
                icon: "trash",
                style: .danger,
                action: { showDeleteConfirmation = true }
            )
        }
        .alert("Delete Provider", isPresented: $showDeleteConfirmation) {
            Button("Cancel", role: .cancel) {}
            Button("Delete", role: .destructive) {
                deleteCurrentProvider()
            }
        } message: {
            Text("Are you sure you want to delete \"\(provider.name)\"? This will also remove the API key from Keychain.")
        }
    }

    // MARK: - View Builders: Edit Mode

    @ViewBuilder
    private var editModeContent: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
            // Header
            HStack {
                Text(isAddingNew ? "Add Provider" : "Edit Provider")
                    .font(DesignTokens.Typography.title)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                if !isAddingNew {
                    Button(action: cancelEdit) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .imageScale(.large)
                    }
                    .buttonStyle(.plain)
                }
            }

            Divider()

            // Active state toggle (for edit mode)
            HStack {
                Text("Active")
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                Toggle("", isOn: $isProviderActive)
                    .labelsHidden()
            }
            .padding(.vertical, DesignTokens.Spacing.xs)

            Text(isProviderActive ? "Provider is enabled and will be used for routing" : "Provider is disabled and will be skipped by routing")
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Divider()

            // Form fields
            FormField(title: "Provider Name") {
                TextField("e.g., openai, claude, my-custom-provider", text: $providerName)
                    .textFieldStyle(.roundedBorder)
                    .disabled(!isAddingNew) // Can't rename existing provider
            }

            FormField(title: "Provider Type") {
                Picker("", selection: $providerType) {
                    ForEach(providerTypes, id: \.self) { type in
                        Text(type.capitalized).tag(type)
                    }
                }
                .pickerStyle(.segmented)
                .onChange(of: providerType) { newType in
                    updateDefaultsForProviderType(newType)
                    testResult = nil // Clear test result when provider type changes
                }
            }

            // API Key (not required for Ollama)
            if providerType != "ollama" {
                FormField(title: "API Key") {
                    SecureField("Enter your API key", text: $apiKey)
                        .textFieldStyle(.roundedBorder)
                        .onChange(of: apiKey) { _ in
                            testResult = nil // Clear test result when API key changes
                        }
                    Text("Stored securely in macOS Keychain")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            FormField(title: "Model") {
                TextField("e.g., gpt-4o, claude-3-5-sonnet-20241022", text: $model)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: model) { _ in
                        testResult = nil // Clear test result when model changes
                    }
            }

            FormField(title: "Base URL (Optional)") {
                TextField("Leave empty for official API", text: $baseURL)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: baseURL) { _ in
                        testResult = nil // Clear test result when base URL changes
                    }
                Text("For custom OpenAI-compatible endpoints")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            FormField(title: "Theme Color") {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    ColorPicker("", selection: $color, supportsOpacity: false)
                        .labelsHidden()

                    Circle()
                        .fill(color)
                        .frame(width: 32, height: 32)

                    Text("Used in Halo overlay")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            // Advanced settings (collapsible)
            DisclosureGroup("Advanced Settings", isExpanded: $isAdvancedExpanded) {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    FormField(title: "Timeout (seconds)") {
                        TextField("30", text: $timeoutSeconds)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 100)
                    }

                    FormField(title: "Max Tokens (Optional)") {
                        TextField("Leave empty for default", text: $maxTokens)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                    }

                    FormField(title: "Temperature (0.0-2.0)") {
                        TextField("Leave empty for default", text: $temperature)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                    }
                }
                .padding(.top, DesignTokens.Spacing.sm)
            }

            // Error message
            if let error = errorMessage {
                errorMessageView(error)
            }

            Spacer()

            // Action buttons
            editModeActionButtons
        }
    }

    @ViewBuilder
    private var editModeActionButtons: some View {
        VStack(spacing: DesignTokens.Spacing.sm) {
            // Test Connection button with inline result below
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    ActionButton(
                        isTesting ? "Testing..." : "Test Connection",
                        icon: "network",
                        style: .secondary,
                        action: testConnection
                    )
                    .disabled(isTesting || !isFormValid())
                }

                // Inline test result below button
                if let result = testResult {
                    testResultView(result)
                        .padding(.leading, DesignTokens.Spacing.xs)
                }
            }

            HStack(spacing: DesignTokens.Spacing.sm) {
                if !isAddingNew {
                    ActionButton(
                        "Cancel",
                        icon: "xmark",
                        style: .secondary,
                        action: cancelEdit
                    )
                }

                ActionButton(
                    isSaving ? "Saving..." : "Save",
                    icon: "checkmark",
                    style: .primary,
                    action: saveProvider
                )
                .disabled(isSaving || !isFormValid())
            }
        }
    }

    // MARK: - Helper Views

    @ViewBuilder
    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.lg) {
            Image(systemName: "cloud.fill")
                .font(.system(size: 60))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text("No Provider Selected")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text("Select a provider from the list or add a new one")
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private func configRow(label: String, value: String) -> some View {
        HStack(alignment: .top) {
            Text(label)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .frame(width: 100, alignment: .leading)

            Text(value)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)
        }
    }

    @ViewBuilder
    private func testResultView(_ result: TestResult) -> some View {
        switch result {
        case .success(let message):
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                    .font(.system(size: 12))

                Text(message)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.green)
                    .lineLimit(2)
                    .truncationMode(.tail)
            }

        case .failure(let message):
            HStack(spacing: 6) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.red)
                    .font(.system(size: 12))

                // Truncate long error messages
                let truncatedMessage = message.count > 80 ? String(message.prefix(80)) + "..." : message
                Text(truncatedMessage)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.red)
                    .lineLimit(2)
                    .truncationMode(.tail)
                    .help(message) // Full message in tooltip
            }
        }
    }

    @ViewBuilder
    private func errorMessageView(_ error: String) -> some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundColor(.orange)
            Text(error)
                .font(DesignTokens.Typography.caption)
        }
        .padding(DesignTokens.Spacing.sm)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.orange.opacity(0.1))
        .cornerRadius(DesignTokens.CornerRadius.small)
    }

    // MARK: - Actions

    func startNewProvider() {
        isEditing = true
        resetForm()
        providerName = ""
        providerType = "openai"
        isProviderActive = true  // New providers are active by default
        updateDefaultsForProviderType("openai")
    }

    private func enterEditMode() {
        guard let provider = currentProvider else { return }

        isEditing = true

        // Load provider data into form
        providerName = provider.name
        providerType = provider.config.providerType ?? "openai"
        model = provider.config.model
        baseURL = provider.config.baseUrl ?? ""
        color = Color(hex: provider.config.color) ?? .blue
        timeoutSeconds = String(provider.config.timeoutSeconds)
        maxTokens = provider.config.maxTokens.map { String($0) } ?? ""
        temperature = provider.config.temperature.map { String($0) } ?? ""

        // Load active state (based on API key presence)
        isProviderActive = hasApiKey

        // Load API key from Keychain
        Task {
            do {
                if let key = try keychainManager.getApiKey(provider: provider.name) {
                    await MainActor.run {
                        apiKey = key
                    }
                }
            } catch {
                print("Failed to load API key: \(error)")
            }
        }
    }

    private func cancelEdit() {
        if isAddingNew {
            isAddingNew = false
            isEditing = false
            selectedProvider = providers.first?.name
        } else {
            isEditing = false
        }
        resetForm()
    }

    private func resetForm() {
        apiKey = ""
        model = ""
        baseURL = ""
        timeoutSeconds = "30"
        maxTokens = ""
        temperature = ""
        isProviderActive = false
        testResult = nil
        errorMessage = nil
    }

    private func updateDefaultsForProviderType(_ type: String) {
        // Update default color
        if let defaultColor = defaultColors[type] {
            color = defaultColor
        }

        // Set default model
        switch type {
        case "openai":
            if model.isEmpty { model = "gpt-4o" }
        case "claude":
            if model.isEmpty { model = "claude-3-5-sonnet-20241022" }
        case "ollama":
            if model.isEmpty { model = "llama3.2" }
        default:
            break
        }
    }

    private func testConnection() {
        guard isFormValid() else { return }

        isTesting = true
        testResult = nil
        errorMessage = nil

        Task {
            do {
                // Temporarily save config (without persisting)
                try await saveProviderConfig(persist: false)

                // Test connection
                let result = try core.testProviderConnection(providerName: providerName)

                await MainActor.run {
                    testResult = .success(result)
                    isTesting = false
                }
            } catch {
                await MainActor.run {
                    testResult = .failure(error.localizedDescription)
                    isTesting = false
                }
            }
        }
    }

    private func saveProvider() {
        guard isFormValid() else { return }

        isSaving = true
        errorMessage = nil

        Task {
            do {
                try await saveProviderConfig(persist: true)

                await MainActor.run {
                    // Reload providers
                    let config = try! core.loadConfig()
                    providers = config.providers
                    selectedProvider = providerName

                    // Exit edit mode
                    isEditing = false
                    isAddingNew = false
                    resetForm()
                    isSaving = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to save: \(error.localizedDescription)"
                    isSaving = false
                }
            }
        }
    }

    private func saveProviderConfig(persist: Bool) async throws {
        // Save API key to Keychain
        if providerType != "ollama" && !apiKey.isEmpty {
            try keychainManager.setApiKey(provider: providerName, key: apiKey)
        }

        // Build config
        let config = ProviderConfig(
            providerType: providerType,
            apiKey: providerType == "ollama" ? nil : "keychain:\(providerName)",
            model: model,
            baseUrl: baseURL.isEmpty ? nil : baseURL,
            color: color.toHex(),
            timeoutSeconds: UInt64(timeoutSeconds) ?? 30,
            maxTokens: maxTokens.isEmpty ? nil : UInt32(maxTokens),
            temperature: temperature.isEmpty ? nil : Float(temperature)
        )

        if persist {
            try core.updateProvider(name: providerName, provider: config)
        }
    }

    private func deleteCurrentProvider() {
        guard let provider = currentProvider else { return }

        Task {
            do {
                try core.deleteProvider(name: provider.name)
                try? keychainManager.deleteApiKey(provider: provider.name)

                await MainActor.run {
                    let config = try! core.loadConfig()
                    providers = config.providers
                    selectedProvider = providers.first?.name
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to delete: \(error.localizedDescription)"
                }
            }
        }
    }

    // MARK: - Form Validation

    private func isFormValid() -> Bool {
        guard !providerName.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        guard !model.trimmingCharacters(in: .whitespaces).isEmpty else { return false }

        if providerType != "ollama" {
            guard !apiKey.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        }

        guard let timeout = UInt64(timeoutSeconds), timeout > 0 else { return false }

        if !maxTokens.isEmpty {
            guard UInt32(maxTokens) != nil else { return false }
        }

        if !temperature.isEmpty {
            guard let temp = Float(temperature), temp >= 0.0, temp <= 2.0 else { return false }
        }

        return true
    }

    // MARK: - Helpers

    private func getProviderIconName(_ type: String?) -> String {
        switch type?.lowercased() ?? "" {
        case "openai": return "brain.head.profile"
        case "claude", "anthropic": return "cpu"
        case "ollama": return "terminal"
        case "gemini", "google": return "sparkles"
        default: return "cloud.fill"
        }
    }

    private func getProviderTypeName(_ type: String?) -> String {
        switch type?.lowercased() ?? "" {
        case "openai": return "OpenAI"
        case "claude": return "Claude"
        case "anthropic": return "Anthropic"
        case "ollama": return "Ollama"
        case "gemini": return "Gemini"
        case "google": return "Google"
        default: return type?.capitalized ?? "Unknown"
        }
    }

    private func getProviderDescription(_ type: String?) -> String {
        switch type?.lowercased() ?? "" {
        case "openai":
            return "OpenAI provides access to GPT models including GPT-4o, GPT-4 Turbo, and GPT-3.5 Turbo. These models excel at natural language understanding, generation, and reasoning tasks."
        case "claude", "anthropic":
            return "Anthropic's Claude models are known for their helpful, harmless, and honest responses. Claude excels at analysis, coding, creative writing, and following complex instructions."
        case "ollama":
            return "Ollama allows you to run large language models locally on your machine. This provides privacy, offline access, and eliminates API costs for supported models."
        case "gemini", "google":
            return "Google's Gemini models offer multimodal capabilities with strong performance across text, code, and reasoning tasks."
        default:
            return "A configured AI language model provider for use with Aether."
        }
    }
}

// MARK: - Form Field Helper (reuse from ProviderConfigView)

struct FormField<Content: View>: View {
    let title: String
    let content: Content

    init(title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(title)
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)
            content
        }
    }
}

// MARK: - Color Extension

extension Color {
    /// Convert Color to hex string
    func toHex() -> String {
        #if os(macOS)
        guard let components = NSColor(self).cgColor.components else {
            return "#808080"
        }
        #else
        guard let components = UIColor(self).cgColor.components else {
            return "#808080"
        }
        #endif

        let r = Int(components[0] * 255.0)
        let g = Int(components[1] * 255.0)
        let b = Int(components[2] * 255.0)

        return String(format: "#%02X%02X%02X", r, g, b)
    }
}
