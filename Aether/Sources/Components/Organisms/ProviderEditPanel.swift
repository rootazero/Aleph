import SwiftUI
import AppKit

/// Unified panel for viewing and editing provider configurations
/// Replaces separate ProviderDetailPanel + ProviderConfigView modal
struct ProviderEditPanel: View {
    // MARK: - Dependencies

    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

    // MARK: - Bindings

    @Binding var providers: [ProviderConfigEntry]
    @Binding var selectedProvider: String?
    @Binding var isAddingNew: Bool  // NEW: External control for adding new provider
    @Binding var selectedPreset: PresetProvider?  // NEW: Selected preset provider

    // Default provider state (NEW for default provider management)
    var defaultProviderId: Binding<String?>? = nil

    // MARK: - State

    // Form fields - Basic
    @State private var providerName: String = ""
    @State private var providerType: String = "openai"
    @State private var apiKey: String = ""
    @State private var model: String = ""
    @State private var baseURL: String = ""
    @State private var color: Color = .blue
    @State private var timeoutSeconds: String = "30"

    // Form fields - Common generation parameters
    @State private var maxTokens: String = ""
    @State private var temperature: String = ""
    @State private var topP: String = ""
    @State private var topK: String = ""

    // Form fields - OpenAI-specific
    @State private var frequencyPenalty: String = ""
    @State private var presencePenalty: String = ""

    // Form fields - Claude/Gemini/Ollama-specific
    @State private var stopSequences: String = ""

    // Form fields - Gemini-specific
    @State private var thinkingLevel: String = "HIGH"
    @State private var mediaResolution: String = "MEDIUM"

    // Form fields - Ollama-specific
    @State private var repeatPenalty: String = ""

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

    private let defaultColors: [String: Color] = [
        "openai": Color(hex: "#10a37f") ?? .green,
        "claude": Color(hex: "#d97757") ?? .orange,
        "gemini": Color(hex: "#4285F4") ?? .blue,
        "ollama": .black,
        "custom": .gray
    ]

    // MARK: - Computed Properties

    /// Current provider being viewed/edited
    private var currentProvider: ProviderConfigEntry? {
        guard let name = selectedProvider else { return nil }
        return providers.first { $0.name == name }
    }

    /// Check if current provider is a custom provider
    private var isCustomProvider: Bool {
        return selectedPreset?.id == "custom" || providerType == "custom"
    }

    /// Check if provider has API key configured
    private var hasApiKey: Bool {
        guard let provider = currentProvider else { return false }
        return provider.config.apiKey != nil || provider.config.providerType == "ollama"
    }

    /// Check if can test connection
    private var canTestConnection: Bool {
        // Ollama doesn't need API key
        if providerType == "ollama" {
            return !model.isEmpty
        }
        // Other providers need API key and model
        return !apiKey.isEmpty && !model.isEmpty
    }

    /// Binding for default provider toggle with mutual exclusion logic
    private var isDefaultBinding: Binding<Bool> {
        Binding(
            get: {
                defaultProviderId?.wrappedValue == providerName
            },
            set: { newValue in
                if newValue {
                    // Set this provider as default (will auto-clear others)
                    setAsDefaultProvider()
                } else {
                    // Cannot unset default by toggling off - must set another provider as default
                    print("[ProviderEditPanel] Cannot unset default provider directly")
                }
            }
        )
    }

    // MARK: - Body

    var body: some View {
        // Scrollable content only (no internal footer)
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                if selectedProvider != nil || selectedPreset != nil {
                    // Always show edit form when a provider is selected
                    editModeFormContent
                } else {
                    // No provider selected
                    emptyStateView
                }
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(DesignTokens.Colors.contentBackground)
        .onChange(of: isAddingNew) { _, newValue in
            if newValue {
                startNewProviderFromPreset()
            }
            updateSaveBarState()
        }
        .onChange(of: selectedPreset) { _, newPreset in
            // When preset changes, load provider data
            // Skip if we're just updating the preset after save
            if newPreset != nil && !isSaving {
                loadProviderData()
            }
            updateSaveBarState()
        }
        .onChange(of: selectedProvider) { _, newProvider in
            // When selected provider changes, load provider data
            // Skip if we're in the middle of saving to prevent reload
            if newProvider != nil && !isSaving {
                loadProviderData()
            }
            updateSaveBarState()
        }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
        .onChange(of: errorMessage) { _, _ in updateSaveBarState() }
        .onAppear {
            updateSaveBarState()
        }
    }

    // MARK: - View Builders: Edit Mode

    @ViewBuilder
    private var editModeFormContent: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
            // Delete button for existing providers (top-right corner)
            if !isAddingNew, let provider = currentProvider {
                HStack {
                    Spacer()

                    Button(action: { showDeleteConfirmation = true }) {
                        Image(systemName: "trash")
                            .foregroundColor(.red)
                            .imageScale(.large)
                    }
                    .buttonStyle(.plain)
                    .alert(NSLocalizedString("provider.delete.title", comment: ""), isPresented: $showDeleteConfirmation) {
                        Button(NSLocalizedString("common.cancel", comment: ""), role: .cancel) {}
                        Button(NSLocalizedString("common.delete", comment: ""), role: .destructive) {
                            deleteCurrentProvider()
                        }
                    } message: {
                        Text(String(format: NSLocalizedString("provider.delete.message", comment: ""), provider.name))
                    }
                }
            }

            // Provider Information Display Card (unified for both preset and custom)
            if let preset = selectedPreset {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    HStack(spacing: DesignTokens.Spacing.md) {
                        // Provider icon
                        ZStack {
                            Circle()
                                .fill(isCustomProvider ? color : (Color(hex: preset.color) ?? DesignTokens.Colors.accentBlue))
                                .frame(width: 48, height: 48)

                            Image(systemName: preset.iconName)
                                .font(.system(size: 24))
                                .foregroundColor(.white)
                        }

                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                            // Show provider name - editable for custom, fixed for preset
                            if isCustomProvider && !providerName.isEmpty {
                                Text(providerName)
                                    .font(DesignTokens.Typography.title)
                                    .foregroundColor(DesignTokens.Colors.textPrimary)
                            } else if !isCustomProvider {
                                Text(preset.name)
                                    .font(DesignTokens.Typography.title)
                                    .foregroundColor(DesignTokens.Colors.textPrimary)
                            } else {
                                Text(LocalizedStringKey("provider.custom_provider"))
                                    .font(DesignTokens.Typography.title)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }

                            Text(getProviderTypeName(preset.providerType))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }

                        Spacer()

                        // Action buttons area: Two-row layout
                        // Row 1: Test Connection (left) + Active Toggle (right)
                        // Row 2: Set as Default Toggle (right-aligned)
                        VStack(alignment: .trailing, spacing: DesignTokens.Spacing.sm) {
                            // First row: Test Connection + Active toggle
                            HStack(spacing: DesignTokens.Spacing.md) {
                                // Test Connection button
                                Button(action: testConnection) {
                                    HStack(spacing: 4) {
                                        if isTesting {
                                            ProgressView()
                                                .scaleEffect(0.7)
                                                .frame(width: 14, height: 14)
                                        } else {
                                            Image(systemName: "network")
                                                .font(.system(size: 12))
                                        }
                                        Text(isTesting ? NSLocalizedString("provider.button.testing", comment: "") : NSLocalizedString("common.test_connection", comment: ""))
                                            .font(.system(size: 12, weight: .medium))
                                    }
                                    .foregroundColor(.white)
                                    .padding(.horizontal, 12)
                                    .padding(.vertical, 6)
                                    .background(canTestConnection ? Color(hex: "#007AFF") ?? .blue : DesignTokens.Colors.textSecondary.opacity(0.3))
                                    .cornerRadius(6)
                                }
                                .buttonStyle(.plain)
                                .disabled(!canTestConnection || isTesting)
                                .help(canTestConnection ? NSLocalizedString("common.test_connection", comment: "") : "Configure API key and model first")

                                Spacer()

                                // Active toggle (right-aligned)
                                Toggle(NSLocalizedString("provider.field.active", comment: ""), isOn: $isProviderActive)
                                    .toggleStyle(.switch)
                            }

                            // Second row: Set as Default toggle (right-aligned with Active toggle above)
                            if !isAddingNew {
                                Toggle(isOn: isDefaultBinding) {
                                    Text(NSLocalizedString("provider.action.set_default", comment: "Set as Default"))
                                        .font(.system(size: 12, weight: .medium))
                                }
                                .toggleStyle(.switch)
                                .disabled(!isProviderActive)
                                .help(isProviderActive ? NSLocalizedString("provider.help.set_default", comment: "") : NSLocalizedString("provider.help.set_default_disabled", comment: ""))
                            }
                        }
                    }

                    // Test result display (moved from card to edit panel)
                    if let result = testResult {
                        testResultView(result)
                            .padding(.top, DesignTokens.Spacing.xs)
                    }

                    // Description - custom or preset
                    if isCustomProvider {
                        if !baseURL.isEmpty {
                            Text(String(format: NSLocalizedString("provider.custom_api_endpoint", comment: ""), baseURL))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            Text(LocalizedStringKey("provider.custom_compatible"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    } else {
                        Text(preset.description)
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.vertical, DesignTokens.Spacing.sm)

                Divider()
            }

            // Form fields
            // Provider Name (only for custom providers)
            if isCustomProvider {
                FormField(title: NSLocalizedString("provider.field.provider_name", comment: "")) {
                    TextField(LocalizedStringKey("provider.placeholder.provider_name"), text: $providerName)
                        .textFieldStyle(.roundedBorder)
                    Text(LocalizedStringKey("provider.help.provider_name"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            // Theme Color (only for custom providers)
            if isCustomProvider {
                FormField(title: NSLocalizedString("provider.field.theme_color", comment: "")) {
                    HStack(spacing: DesignTokens.Spacing.sm) {
                        ColorPicker("", selection: $color, supportsOpacity: false)
                            .labelsHidden()

                        Circle()
                            .fill(color)
                            .frame(width: 32, height: 32)

                        Text(LocalizedStringKey("provider.help.theme_color"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }
            }

            // Provider Type is hidden and auto-determined from preset
            // No user selection needed

            // API Key (not required for Ollama)
            if providerType != "ollama" {
                FormField(title: NSLocalizedString("provider.field.api_key", comment: "")) {
                    SecureField(LocalizedStringKey("provider.placeholder.api_key"), text: $apiKey)
                        .textFieldStyle(.roundedBorder)
                        .onChange(of: apiKey) {
                            testResult = nil // Clear test result when API key changes
                        }
                }
            }

            FormField(title: NSLocalizedString("provider.field.model", comment: "")) {
                TextField(LocalizedStringKey("provider.placeholder.model"), text: $model)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: model) {
                        testResult = nil // Clear test result when model changes
                    }
            }

            FormField(title: isCustomProvider ? NSLocalizedString("provider.field.base_url", comment: "") : NSLocalizedString("provider.field.base_url_optional", comment: "")) {
                TextField(isCustomProvider ? LocalizedStringKey("provider.placeholder.base_url_custom") : LocalizedStringKey("provider.placeholder.base_url_official"), text: $baseURL)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: baseURL) {
                        testResult = nil // Clear test result when base URL changes
                    }
                Text(isCustomProvider ? LocalizedStringKey("provider.help.base_url_custom") : LocalizedStringKey("provider.help.base_url_official"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            // Generation Parameters (collapsible)
            DisclosureGroup(LocalizedStringKey("provider.section.generation_params"), isExpanded: $isAdvancedExpanded) {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    // Common parameters (all providers)
                    FormField(title: NSLocalizedString("provider.field.max_tokens_optional", comment: "")) {
                        TextField(getMaxTokensPlaceholder(), text: $maxTokens)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                        Text(LocalizedStringKey("provider.help.max_tokens"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    FormField(title: NSLocalizedString("provider.field.temperature_optional", comment: "")) {
                        TextField(getTemperaturePlaceholder(), text: $temperature)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                        Text(getTemperatureHelp())
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Top-P (all providers except Ollama uses it optionally)
                    FormField(title: NSLocalizedString("provider.field.top_p_optional", comment: "")) {
                        TextField(LocalizedStringKey("provider.placeholder.top_p"), text: $topP)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                        Text(LocalizedStringKey("provider.help.top_p"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Top-K (Claude, Gemini, Ollama)
                    if providerType == "claude" || providerType == "gemini" || providerType == "ollama" {
                        FormField(title: NSLocalizedString("provider.field.top_k_optional", comment: "")) {
                            TextField(providerType == "ollama" ? LocalizedStringKey("provider.placeholder.top_k_ollama") : LocalizedStringKey("provider.placeholder.top_k_default"), text: $topK)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 150)
                            Text(LocalizedStringKey("provider.help.top_k"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                    }

                    // OpenAI-specific parameters
                    if providerType == "openai" {
                        Divider()
                        Text("OpenAI-Specific Parameters")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        FormField(title: "Frequency Penalty (Optional)") {
                            TextField("-2.0 to 2.0, leave empty for default", text: $frequencyPenalty)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 150)
                            Text("Reduce repetition based on token frequency")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }

                        FormField(title: "Presence Penalty (Optional)") {
                            TextField("-2.0 to 2.0, leave empty for default", text: $presencePenalty)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 150)
                            Text("Encourage exploring new topics")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                    }

                    // Claude/Gemini/Ollama stop sequences
                    if providerType == "claude" || providerType == "gemini" || providerType == "ollama" {
                        Divider()
                        FormField(title: "Stop Sequences (Optional)") {
                            TextField("Comma-separated, e.g., END,STOP", text: $stopSequences)
                                .textFieldStyle(.roundedBorder)
                            Text("Sequences that will stop generation")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                    }

                    // Gemini-specific parameters
                    if providerType == "gemini" {
                        Divider()
                        Text("Gemini-Specific Parameters")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        FormField(title: "Thinking Level") {
                            Picker("", selection: $thinkingLevel) {
                                Text("Low").tag("LOW")
                                Text("High (Recommended)").tag("HIGH")
                            }
                            .pickerStyle(.segmented)
                            Text("Depth of reasoning for Gemini 3 models")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }

                        FormField(title: "Media Resolution") {
                            Picker("", selection: $mediaResolution) {
                                Text("Low").tag("LOW")
                                Text("Medium").tag("MEDIUM")
                                Text("High").tag("HIGH")
                            }
                            .pickerStyle(.segmented)
                            Text("Resolution for image/video processing")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                    }

                    // Ollama-specific parameters
                    if providerType == "ollama" {
                        Divider()
                        Text("Ollama-Specific Parameters")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        FormField(title: "Repeat Penalty (Optional)") {
                            TextField("e.g., 1.1 (default)", text: $repeatPenalty)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 150)
                            Text("Penalty for repeating tokens (>= 1.0)")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                    }

                    Divider()
                    FormField(title: "Timeout (seconds)") {
                        TextField("30", text: $timeoutSeconds)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 100)
                        Text("Request timeout in seconds")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }
                .padding(.top, DesignTokens.Spacing.sm)
            }

            // Error message
            if let error = errorMessage {
                errorMessageView(error)
            }
        }
    }

    // MARK: - Computed Properties for Save Bar

    /// Check if form has unsaved changes (simplified version)
    private var hasUnsavedFormChanges: Bool {
        // For now, consider form "dirty" if any field is non-empty
        // TODO: Implement proper working copy vs saved state comparison
        if isAddingNew {
            return !providerName.isEmpty || !model.isEmpty || !apiKey.isEmpty
        } else {
            // For existing providers, always consider as potentially changed
            return true
        }
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedFormChanges {
            return "Unsaved changes"  // Simplified message without localization for now
        }
        return nil
    }

    /// Update saveBarState to reflect current provider editing state
    private func updateSaveBarState() {
        // Only show save bar when a provider is selected
        let hasChanges = (selectedProvider != nil || selectedPreset != nil) && hasUnsavedFormChanges

        saveBarState.update(
            hasUnsavedChanges: hasChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: { await self.saveProviderAsync() },
            onCancel: cancelEditing
        )
    }

    /// Async wrapper for saveProvider
    private func saveProviderAsync() async {
        await MainActor.run {
            saveProvider()
        }
    }


    // MARK: - Helper Views

    @ViewBuilder
    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.lg) {
            Image(systemName: "cloud.fill")
                .font(.system(size: 60))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text(LocalizedStringKey("provider.empty_state.title"))
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(LocalizedStringKey("provider.empty_state.message"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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

    /// Set current provider as default (NEW for default provider management)
    private func setAsDefaultProvider() {
        guard !providerName.isEmpty else { return }
        guard isProviderActive else {
            print("[ProviderEditPanel] Cannot set disabled provider as default")
            return
        }

        do {
            try core.setDefaultProvider(providerName: providerName)
            // Update binding to trigger UI refresh
            defaultProviderId?.wrappedValue = providerName
            print("[ProviderEditPanel] Set default provider to: \(providerName)")
        } catch {
            print("[ProviderEditPanel] Error setting default provider: \(error)")
            errorMessage = "Failed to set default: \(error.localizedDescription)"
        }
    }

    /// Load provider data into form (for both new and existing providers)
    private func loadProviderData() {
        // If adding new provider from preset
        if isAddingNew {
            startNewProviderFromPreset()
            return
        }

        // If editing existing provider
        if let provider = currentProvider {
            loadExistingProvider(provider)
        } else if let preset = selectedPreset {
            // Preset selected but not configured yet
            loadPresetDefaults(preset)
        }
    }

    /// Load existing configured provider data
    private func loadExistingProvider(_ provider: ProviderConfigEntry) {
        providerName = provider.name
        providerType = provider.config.providerType ?? "openai"
        model = provider.config.model
        baseURL = provider.config.baseUrl ?? ""
        color = Color(hex: provider.config.color) ?? .blue
        timeoutSeconds = String(provider.config.timeoutSeconds)

        // Common generation parameters
        maxTokens = provider.config.maxTokens.map { String($0) } ?? ""
        temperature = provider.config.temperature.map { String($0) } ?? ""
        topP = provider.config.topP.map { String($0) } ?? ""
        topK = provider.config.topK.map { String($0) } ?? ""

        // OpenAI-specific
        frequencyPenalty = provider.config.frequencyPenalty.map { String($0) } ?? ""
        presencePenalty = provider.config.presencePenalty.map { String($0) } ?? ""

        // Claude/Gemini/Ollama
        stopSequences = provider.config.stopSequences ?? ""

        // Gemini-specific
        thinkingLevel = provider.config.thinkingLevel ?? "HIGH"
        mediaResolution = provider.config.mediaResolution ?? "MEDIUM"

        // Ollama-specific
        repeatPenalty = provider.config.repeatPenalty.map { String($0) } ?? ""

        // Load active state from config
        isProviderActive = provider.config.enabled

        // Load API key from config
        apiKey = provider.config.apiKey ?? ""
    }

    /// Load preset defaults for unconfigured provider
    private func loadPresetDefaults(_ preset: PresetProvider) {
        if preset.id == "custom" {
            // Custom provider - user will define everything
            providerName = ""
            providerType = preset.providerType
            model = ""
            baseURL = ""
            color = Color(hex: preset.color) ?? .gray
        } else {
            // Preset provider - use predefined values
            providerName = preset.id
            providerType = preset.providerType
            model = preset.defaultModel
            baseURL = preset.baseUrl ?? ""
            color = Color(hex: preset.color) ?? .blue
        }
        timeoutSeconds = "30"
        maxTokens = ""
        temperature = ""
        isProviderActive = false  // New providers are disabled by default
        apiKey = ""
    }

    func startNewProvider() {
        resetForm()
        providerName = ""
        providerType = "openai"
        isProviderActive = false  // New providers are disabled by default
        updateDefaultsForProviderType("openai")
    }

    func startNewProviderFromPreset() {
        guard let preset = selectedPreset else {
            startNewProvider()
            return
        }

        resetForm()
        loadPresetDefaults(preset)
    }

    private func resetForm() {
        apiKey = ""
        model = ""
        baseURL = ""
        timeoutSeconds = "30"

        // Common generation parameters
        maxTokens = ""
        temperature = ""
        topP = ""
        topK = ""

        // OpenAI-specific
        frequencyPenalty = ""
        presencePenalty = ""

        // Claude/Gemini/Ollama
        stopSequences = ""

        // Gemini-specific
        thinkingLevel = "HIGH"
        mediaResolution = "MEDIUM"

        // Ollama-specific
        repeatPenalty = ""

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
        case "gemini":
            if model.isEmpty { model = "gemini-3-flash" }
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
            // Build temporary provider config with actual API key (not keychain reference)
            // This allows testing without persisting the configuration to disk
            let testConfig = ProviderConfig(
                providerType: providerType,
                apiKey: providerType == "ollama" ? nil : apiKey,  // Use actual API key for testing
                model: model,
                baseUrl: baseURL.isEmpty ? nil : baseURL,
                color: color.toHex(),
                timeoutSeconds: UInt64(timeoutSeconds) ?? 30,
                enabled: isProviderActive,
                maxTokens: maxTokens.isEmpty ? nil : UInt32(maxTokens),
                temperature: temperature.isEmpty ? nil : Float(temperature),
                topP: topP.isEmpty ? nil : Float(topP),
                topK: topK.isEmpty ? nil : UInt32(topK),
                frequencyPenalty: frequencyPenalty.isEmpty ? nil : Float(frequencyPenalty),
                presencePenalty: presencePenalty.isEmpty ? nil : Float(presencePenalty),
                stopSequences: stopSequences.isEmpty ? nil : stopSequences,
                thinkingLevel: (providerType == "gemini" && !thinkingLevel.isEmpty) ? thinkingLevel : nil,
                mediaResolution: (providerType == "gemini" && !mediaResolution.isEmpty) ? mediaResolution : nil,
                repeatPenalty: repeatPenalty.isEmpty ? nil : Float(repeatPenalty)
            )

            // Test connection with temporary config (does not persist to disk)
            let result = core.testProviderConnectionWithConfig(
                providerName: providerName,
                providerConfig: testConfig
            )

            await MainActor.run {
                if result.success {
                    testResult = .success(result.message)
                } else {
                    testResult = .failure(result.message)
                }
                isTesting = false
            }
        }
    }

    private func saveProvider() {
        guard isFormValid() else { return }

        isSaving = true
        errorMessage = nil

        // Save the current provider name to restore selection after save
        let savedProviderName = providerName

        Task {
            do {
                try await saveProviderConfig(persist: true)

                await MainActor.run {
                    // Reload providers list
                    let config = try! core.loadConfig()
                    providers = config.providers

                    // CRITICAL: Keep the current provider selected
                    // This prevents jumping to the first provider
                    selectedProvider = savedProviderName

                    // Update selectedPreset for both custom and preset providers
                    // This ensures the UI stays on the current provider after save
                    if isCustomProvider {
                        // For custom providers, create a temporary PresetProvider
                        // This will be replaced when ProvidersView updates
                        let customPreset = PresetProvider(
                            id: savedProviderName,
                            name: savedProviderName,
                            iconName: "puzzlepiece.extension",
                            color: color.toHex(),
                            providerType: providerType,
                            defaultModel: model,
                            description: baseURL.isEmpty ? "Custom OpenAI-compatible provider" : "OpenAI-compatible API: \(baseURL)",
                            baseUrl: baseURL.isEmpty ? nil : baseURL
                        )
                        selectedPreset = customPreset
                    } else {
                        // For preset providers, find and update the preset
                        // This ensures the displayed information matches the saved provider
                        if let preset = PresetProviders.find(byId: savedProviderName) {
                            selectedPreset = preset
                        }
                    }

                    // Exit add mode if we were adding a new provider
                    isAddingNew = false

                    isSaving = false

                    // Notify that configuration was saved internally
                    // This prevents ConfigWatcher from triggering a full view rebuild
                    NotificationCenter.default.post(
                        name: NSNotification.Name("AetherConfigSavedInternally"),
                        object: savedProviderName  // Pass the saved provider name
                    )
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to save: \(error.localizedDescription)"
                    isSaving = false
                }
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        // Clear any error messages
        errorMessage = nil
        testResult = nil

        // Reload provider data to revert changes
        loadProviderData()
    }

    private func saveProviderConfig(persist: Bool) async throws {
        // Build config with all parameters (API key stored directly in config)
        let config = ProviderConfig(
            providerType: providerType,
            apiKey: providerType == "ollama" ? nil : (apiKey.isEmpty ? nil : apiKey),
            model: model,
            baseUrl: baseURL.isEmpty ? nil : baseURL,
            color: color.toHex(),
            timeoutSeconds: UInt64(timeoutSeconds) ?? 30,
            enabled: isProviderActive,
            maxTokens: maxTokens.isEmpty ? nil : UInt32(maxTokens),
            temperature: temperature.isEmpty ? nil : Float(temperature),
            topP: topP.isEmpty ? nil : Float(topP),
            topK: topK.isEmpty ? nil : UInt32(topK),
            frequencyPenalty: frequencyPenalty.isEmpty ? nil : Float(frequencyPenalty),
            presencePenalty: presencePenalty.isEmpty ? nil : Float(presencePenalty),
            stopSequences: stopSequences.isEmpty ? nil : stopSequences,
            thinkingLevel: (providerType == "gemini" && !thinkingLevel.isEmpty) ? thinkingLevel : nil,
            mediaResolution: (providerType == "gemini" && !mediaResolution.isEmpty) ? mediaResolution : nil,
            repeatPenalty: repeatPenalty.isEmpty ? nil : Float(repeatPenalty)
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
        // Basic required fields
        guard !providerName.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        guard !model.trimmingCharacters(in: .whitespaces).isEmpty else { return false }

        // API key required for non-Ollama providers
        if providerType != "ollama" {
            guard !apiKey.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        }

        // Base URL required for custom providers
        if isCustomProvider {
            guard !baseURL.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        }

        // Timeout validation
        guard let timeout = UInt64(timeoutSeconds), timeout > 0 else { return false }

        // Common parameter validation
        if !maxTokens.isEmpty {
            guard let tokens = UInt32(maxTokens), tokens > 0 else { return false }
        }

        if !temperature.isEmpty {
            guard let temp = Float(temperature) else { return false }
            // Provider-specific temperature range
            switch providerType {
            case "claude":
                guard temp >= 0.0 && temp <= 1.0 else { return false }
            case "openai", "gemini":
                guard temp >= 0.0 && temp <= 2.0 else { return false }
            case "ollama":
                guard temp >= 0.0 else { return false }
            default:
                guard temp >= 0.0 && temp <= 2.0 else { return false }
            }
        }

        if !topP.isEmpty {
            guard let p = Float(topP), p >= 0.0, p <= 1.0 else { return false }
        }

        if !topK.isEmpty {
            guard let k = UInt32(topK), k > 0 else { return false }
        }

        // OpenAI-specific validation
        if providerType == "openai" {
            if !frequencyPenalty.isEmpty {
                guard let penalty = Float(frequencyPenalty), penalty >= -2.0, penalty <= 2.0 else { return false }
            }

            if !presencePenalty.isEmpty {
                guard let penalty = Float(presencePenalty), penalty >= -2.0, penalty <= 2.0 else { return false }
            }
        }

        // Ollama-specific validation
        if providerType == "ollama" {
            if !repeatPenalty.isEmpty {
                guard let penalty = Float(repeatPenalty), penalty >= 1.0 else { return false }
            }
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

    // MARK: - Parameter Helpers

    private func getMaxTokensPlaceholder() -> String {
        switch providerType {
        case "openai": return "e.g., 1024 (default)"
        case "claude": return "e.g., 1024 (default)"
        case "gemini": return "e.g., 2048 (default)"
        case "ollama": return "e.g., 512 (default)"
        default: return "Leave empty for default"
        }
    }

    private func getTemperaturePlaceholder() -> String {
        switch providerType {
        case "openai": return "0.0-2.0, default 1.0"
        case "claude": return "0.0-1.0, default 1.0"
        case "gemini": return "0.0-2.0, default 1.0"
        case "ollama": return "0.0+, default 0.8"
        default: return "Leave empty for default"
        }
    }

    private func getTemperatureHelp() -> String {
        switch providerType {
        case "claude": return "Controls randomness (0.0-1.0, 0=deterministic)"
        case "ollama": return "Controls randomness (higher = more creative)"
        default: return "Controls randomness (0.0-2.0, 0=deterministic)"
        }
    }
}
