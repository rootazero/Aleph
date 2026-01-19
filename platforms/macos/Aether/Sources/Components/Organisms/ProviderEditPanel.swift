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

    // Form fields - OpenAI-compatible specific
    // Default to "prepend" for better compatibility with third-party APIs
    @State private var systemPromptMode: String = "prepend"

    // Provider active state
    @State private var isProviderActive: Bool = false

    // Saved state for change detection (NEW)
    @State private var savedProviderName: String = ""
    @State private var savedProviderType: String = "openai"
    @State private var savedApiKey: String = ""
    @State private var savedModel: String = ""
    @State private var savedBaseURL: String = ""
    @State private var savedTimeoutSeconds: String = "30"
    @State private var savedMaxTokens: String = ""
    @State private var savedTemperature: String = ""
    @State private var savedTopP: String = ""
    @State private var savedTopK: String = ""
    @State private var savedFrequencyPenalty: String = ""
    @State private var savedPresencePenalty: String = ""
    @State private var savedStopSequences: String = ""
    @State private var savedThinkingLevel: String = "HIGH"
    @State private var savedMediaResolution: String = "MEDIUM"
    @State private var savedRepeatPenalty: String = ""
    @State private var savedSystemPromptMode: String = "prepend"
    @State private var savedIsProviderActive: Bool = false

    // UI state
    @State private var isSaving: Bool = false
    @State private var isTesting: Bool = false
    @State private var testResult: TestResult?
    @State private var errorMessage: String?
    @State private var showDeleteConfirmation: Bool = false
    @State private var justSavedProviderName: String? = nil  // Track just-saved provider to skip unnecessary reload

    // Section expansion states
    @State private var isConfigExpanded = true
    @State private var isAdvancedExpanded = false

    enum TestResult {
        case success(String)
        case failure(String)
    }

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
                // Skip reload if we just saved this provider (data is already correct)
                if let justSaved = justSavedProviderName, justSaved == newPreset?.id {
                    justSavedProviderName = nil  // Clear flag after skipping
                } else {
                    loadProviderData()
                }
            }
            updateSaveBarState()
        }
        .onChange(of: selectedProvider) { _, newProvider in
            // When selected provider changes, load provider data
            // Skip if we're in the middle of saving to prevent reload
            if newProvider != nil && !isSaving {
                // Skip reload if we just saved this provider (data is already correct)
                if let justSaved = justSavedProviderName, justSaved == newProvider {
                    // Don't clear flag here - let selectedPreset onChange clear it
                } else {
                    loadProviderData()
                }
            }
            updateSaveBarState()
        }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
        .onChange(of: errorMessage) { _, _ in updateSaveBarState() }
        // Monitor form field changes to update save bar state
        .onChange(of: providerName) { _, _ in updateSaveBarState() }
        .onChange(of: apiKey) { _, _ in updateSaveBarState() }
        .onChange(of: model) { _, _ in updateSaveBarState() }
        .onChange(of: baseURL) { _, _ in updateSaveBarState() }
        .onChange(of: isProviderActive) { _, _ in updateSaveBarState() }
        .onChange(of: temperature) { _, _ in updateSaveBarState() }
        .onChange(of: maxTokens) { _, _ in updateSaveBarState() }
        .onChange(of: systemPromptMode) { _, _ in updateSaveBarState() }
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
                    .alert(L("provider.delete.title"), isPresented: $showDeleteConfirmation) {
                        Button(L("common.cancel"), role: .cancel) {}
                        Button(L("common.delete"), role: .destructive) {
                            deleteCurrentProvider()
                        }
                    } message: {
                        Text(String(format: L("provider.delete.message"), provider.name))
                    }
                }
            }

            // Provider Information Display Card (unified for both preset and custom)
            if let preset = selectedPreset {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    HStack(spacing: DesignTokens.Spacing.md) {
                        // Provider icon with brand logo
                        if isCustomProvider {
                            // Custom provider - use consistent gradient style with ProviderIcon
                            ProviderIcon(providerType: "custom", size: 48)
                        } else {
                            // Preset provider - use brand SVG icon (use preset.id for correct icon)
                            ProviderIcon(
                                providerType: preset.id,
                                size: 48
                            )
                        }

                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                            // Provider name
                            if isCustomProvider && !providerName.isEmpty {
                                Text(providerName)
                                    .font(DesignTokens.Typography.title)
                                    .foregroundColor(DesignTokens.Colors.textPrimary)
                            } else if !isCustomProvider {
                                Text(preset.name)
                                    .font(DesignTokens.Typography.title)
                                    .foregroundColor(DesignTokens.Colors.textPrimary)
                            } else {
                                Text(L("provider.custom_provider"))
                                    .font(DesignTokens.Typography.title)
                                    .foregroundColor(DesignTokens.Colors.textSecondary)
                            }

                            // Test connection button below provider name
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
                                    Text(isTesting ? L("provider.button.testing") : L("common.test_connection"))
                                        .font(.system(size: 12, weight: .medium))
                                }
                                .foregroundColor(canTestConnection ? .white : DesignTokens.Colors.textSecondary)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(canTestConnection ? Color(hex: "#007AFF") ?? .blue : DesignTokens.Colors.textSecondary.opacity(0.15))
                                .cornerRadius(6)
                            }
                            .buttonStyle(.plain)
                            .disabled(!canTestConnection || isTesting)
                            .help(canTestConnection ? L("common.test_connection") : "Configure API key and model first")
                        }

                        Spacer()

                        // Action buttons area: Single-row layout
                        // Active Toggle + Set as Default Toggle
                        VStack(alignment: .trailing, spacing: DesignTokens.Spacing.sm) {
                            // Active toggle
                            Toggle(L("provider.field.active"), isOn: $isProviderActive)
                                .toggleStyle(.switch)

                            // Set as Default toggle (always show, disable when inactive or adding new)
                            Toggle(isOn: isDefaultBinding) {
                                Text(L("provider.action.set_default"))
                                    .font(.system(size: 12, weight: .medium))
                            }
                            .toggleStyle(.switch)
                            .disabled(!isProviderActive || isAddingNew)
                            .help(isAddingNew ? L("provider.help.set_default_save_first") :
                                  (isProviderActive ? L("provider.help.set_default") : L("provider.help.set_default_disabled")))
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
                            Text(String(format: L("provider.custom_api_endpoint"), baseURL))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            Text(L("provider.custom_compatible"))
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
                FormField(title: L("provider.field.provider_name")) {
                    TextField(L("provider.placeholder.provider_name"), text: $providerName)
                        .textFieldStyle(.roundedBorder)
                    Text(L("provider.help.provider_name"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            // Provider Type is hidden and auto-determined from preset
            // No user selection needed

            // API Key (not required for Ollama)
            if providerType != "ollama" {
                FormField(title: L("provider.field.api_key")) {
                    SecureField(L("provider.placeholder.api_key"), text: $apiKey)
                        .textFieldStyle(.roundedBorder)
                        .onChange(of: apiKey) {
                            testResult = nil // Clear test result when API key changes
                        }
                }
            }

            FormField(title: L("provider.field.model")) {
                TextField(L("provider.placeholder.model"), text: $model)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: model) {
                        testResult = nil // Clear test result when model changes
                    }
            }

            FormField(title: isCustomProvider ? L("provider.field.base_url") : L("provider.field.base_url_optional")) {
                TextField(getBaseUrlPlaceholder(), text: $baseURL)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: baseURL) {
                        testResult = nil // Clear test result when base URL changes
                    }
                Text(getBaseUrlHelp())
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            // System Prompt Mode (OpenAI only) - outside generation params
            if providerType == "openai" {
                FormField(title: L("provider.field.system_prompt_mode")) {
                    Picker("", selection: $systemPromptMode) {
                        Text(L("provider.system_prompt_mode.standard")).tag("standard")
                        Text(L("provider.system_prompt_mode.prepend")).tag("prepend")
                    }
                    .pickerStyle(.segmented)
                    .frame(width: 280)
                    Text(L("provider.help.system_prompt_mode"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            // Generation Parameters (collapsible)
            DisclosureGroup(L("provider.section.generation_params"), isExpanded: $isAdvancedExpanded) {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    // Common parameters (all providers)
                    FormField(title: L("provider.field.max_tokens_optional")) {
                        TextField(getMaxTokensPlaceholder(), text: $maxTokens)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                        Text(L("provider.help.max_tokens"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    FormField(title: L("provider.field.temperature_optional")) {
                        TextField(getTemperaturePlaceholder(), text: $temperature)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                        Text(getTemperatureHelp())
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Top-P (all providers except Ollama uses it optionally)
                    FormField(title: L("provider.field.top_p_optional")) {
                        TextField(L("provider.placeholder.top_p"), text: $topP)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 150)
                        Text(L("provider.help.top_p"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Top-K (Claude, Gemini, Ollama)
                    if providerType == "claude" || providerType == "gemini" || providerType == "ollama" {
                        FormField(title: L("provider.field.top_k_optional")) {
                            TextField(providerType == "ollama" ? L("provider.placeholder.top_k_ollama") : L("provider.placeholder.top_k_default"), text: $topK)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 150)
                            Text(L("provider.help.top_k"))
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
        // Unconfigured preset - only "dirty" if API key entered
        guard let provider = currentProvider else {
            if isAddingNew {
                // Custom provider: check if user entered name, base URL, or API key
                if isCustomProvider {
                    return !providerName.isEmpty || !baseURL.isEmpty || !apiKey.isEmpty
                }
                // Preset provider: only check API key (name/model are preset defaults)
                return !apiKey.isEmpty
            }
            // Unconfigured preset - only show unsaved if user entered API key
            return !apiKey.isEmpty
        }

        // For existing providers, compare directly with saved config (not savedXxx state)
        let config = provider.config
        return providerName != provider.name ||
               apiKey != (config.apiKey ?? "") ||
               model != config.model ||
               baseURL != (config.baseUrl ?? "") ||
               timeoutSeconds != String(config.timeoutSeconds) ||
               maxTokens != (config.maxTokens.map { String($0) } ?? "") ||
               temperature != (config.temperature.map { String($0) } ?? "") ||
               topP != (config.topP.map { String($0) } ?? "") ||
               topK != (config.topK.map { String($0) } ?? "") ||
               frequencyPenalty != (config.frequencyPenalty.map { String($0) } ?? "") ||
               presencePenalty != (config.presencePenalty.map { String($0) } ?? "") ||
               stopSequences != (config.stopSequences ?? "") ||
               thinkingLevel != (config.thinkingLevel ?? "HIGH") ||
               mediaResolution != (config.mediaResolution ?? "MEDIUM") ||
               repeatPenalty != (config.repeatPenalty.map { String($0) } ?? "") ||
               systemPromptMode != (config.systemPromptMode == "standard" ? "standard" : "prepend") ||
               isProviderActive != config.enabled
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

        NSLog("[ProviderEditPanel] updateSaveBarState() - hasChanges: %d, selectedProvider: %@, selectedPreset: %@, hasUnsavedFormChanges: %d", hasChanges ? 1 : 0, selectedProvider ?? "nil", selectedPreset?.id ?? "nil", hasUnsavedFormChanges ? 1 : 0)

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
        NSLog("[ProviderEditPanel] saveProviderAsync() called")
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

            Text(L("provider.empty_state.title"))
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("provider.empty_state.message"))
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
    ///
    /// IMPORTANT: This method uses the saved provider name (currentProvider.name),
    /// not the form field (providerName), to avoid setting an unsaved provider as default.
    private func setAsDefaultProvider() {
        // Use the saved provider name, not the form field value
        // This prevents setting an unsaved/renamed provider as default
        guard let savedName = currentProvider?.name else {
            print("[ProviderEditPanel] Cannot set default: no saved provider")
            errorMessage = L("provider.error.save_first")
            return
        }

        guard isProviderActive else {
            print("[ProviderEditPanel] Cannot set disabled provider as default")
            return
        }

        // Check if there are unsaved changes - warn user if so
        if hasUnsavedFormChanges {
            print("[ProviderEditPanel] Warning: setting default with unsaved changes")
            // Still proceed using the saved name, but log the warning
        }

        do {
            try core.setDefaultProvider(providerName: savedName)
            // Update binding to trigger UI refresh
            defaultProviderId?.wrappedValue = savedName
            print("[ProviderEditPanel] Set default provider to: \(savedName)")
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

        // OpenAI-compatible specific - default to prepend for better compatibility
        systemPromptMode = provider.config.systemPromptMode == "standard" ? "standard" : "prepend"

        // Load active state from config
        isProviderActive = provider.config.enabled

        // Load API key from config
        apiKey = provider.config.apiKey ?? ""

        // Save current state as baseline for change detection
        saveSavedState()
    }

    /// Save current form state as the baseline for change detection
    private func saveSavedState() {
        savedProviderName = providerName
        savedProviderType = providerType
        savedApiKey = apiKey
        savedModel = model
        savedBaseURL = baseURL
        savedTimeoutSeconds = timeoutSeconds
        savedMaxTokens = maxTokens
        savedTemperature = temperature
        savedTopP = topP
        savedTopK = topK
        savedFrequencyPenalty = frequencyPenalty
        savedPresencePenalty = presencePenalty
        savedStopSequences = stopSequences
        savedThinkingLevel = thinkingLevel
        savedMediaResolution = mediaResolution
        savedRepeatPenalty = repeatPenalty
        savedSystemPromptMode = systemPromptMode
        savedIsProviderActive = isProviderActive
    }

    /// Load preset defaults for unconfigured provider
    private func loadPresetDefaults(_ preset: PresetProvider) {
        if preset.id == "custom" {
            // Custom provider - user will define everything
            providerName = ""
            providerType = preset.providerType
            model = ""
            baseURL = ""
        } else {
            // Preset provider - use predefined values
            providerName = preset.id
            providerType = preset.providerType
            model = preset.defaultModel
            baseURL = preset.baseUrl ?? ""
        }
        timeoutSeconds = "30"
        maxTokens = ""
        temperature = ""
        isProviderActive = false  // New providers are disabled by default
        apiKey = ""

        // Save initial state as baseline for change detection
        // This prevents showing "unsaved" when user just views an unconfigured provider
        saveSavedState()
    }

    func startNewProvider() {
        resetForm()
        providerName = ""
        providerType = "openai"
        isProviderActive = false  // New providers are disabled by default
        updateDefaultsForProviderType("openai")
        // Save initial state as baseline for change detection
        saveSavedState()
    }

    func startNewProviderFromPreset() {
        guard let preset = selectedPreset else {
            startNewProvider()
            return
        }

        resetForm()
        loadPresetDefaults(preset)
        // Note: loadPresetDefaults already calls saveSavedState()
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
                color: "#5E5CE6",  // Fixed default color (not used in UI)
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
                repeatPenalty: repeatPenalty.isEmpty ? nil : Float(repeatPenalty),
                systemPromptMode: nil
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
        // Validate form and show specific error messages
        if let validationError = getValidationError() {
            errorMessage = validationError
            return
        }
        isSaving = true
        errorMessage = nil

        // Save the current provider name to restore selection after save
        let savedProviderName = providerName

        Task {
            do {
                try await saveProviderConfig(persist: true)

                // Reload config outside MainActor to avoid try! crash
                let config = try core.loadConfig()

                await MainActor.run {
                    // Set flag to skip unnecessary loadProviderData() calls
                    // This MUST be set BEFORE any state changes that could trigger onChange
                    justSavedProviderName = savedProviderName

                    // Update providers list with reloaded config
                    providers = config.providers

                    // CRITICAL: Update selectedProvider BEFORE setting isAddingNew = false
                    // This ensures currentProvider can find the saved provider when
                    // loadProviderData() is triggered by state changes
                    selectedProvider = savedProviderName

                    // Update selectedPreset for both custom and preset providers
                    // This ensures the UI stays on the current provider after save
                    // Check custom provider by examining the current providerType and preset
                    let isCustom = selectedPreset?.id == "custom" || providerType == "custom"
                    if isCustom {
                        // For custom providers, create a temporary PresetProvider
                        let customPreset = PresetProvider(
                            id: savedProviderName,
                            name: savedProviderName,
                            iconName: "puzzlepiece.extension",
                            color: "#5E5CE6",  // Fixed default color
                            providerType: providerType,
                            defaultModel: model,
                            description: baseURL.isEmpty ? "Custom OpenAI-compatible provider" : "OpenAI-compatible API: \(baseURL)",
                            baseUrl: baseURL.isEmpty ? nil : baseURL
                        )
                        selectedPreset = customPreset
                    } else {
                        // For preset providers, find and update the preset
                        if let preset = PresetProviders.find(byId: savedProviderName) {
                            selectedPreset = preset
                        }
                    }

                    // Exit add mode AFTER updating selectedProvider and selectedPreset
                    // This ensures currentProvider is valid when loadProviderData() checks
                    isAddingNew = false

                    // Update saved state after successful save
                    saveSavedState()

                    // Set isSaving = false to allow onChange handlers to process
                    isSaving = false

                    // Notify that configuration was saved internally
                    // This prevents ConfigWatcher from triggering a full view rebuild
                    NotificationCenter.default.post(
                        name: .aetherConfigSavedInternally,
                        object: savedProviderName  // Pass the saved provider name
                    )
                }
            } catch {
                await MainActor.run {
                    // DEBUG: Show error alert
                    let errorAlert = NSAlert()
                    errorAlert.messageText = "保存失败！"
                    errorAlert.informativeText = error.localizedDescription
                    errorAlert.addButton(withTitle: "确定")
                    errorAlert.runModal()

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
            color: "#5E5CE6",  // Fixed default color (not used in UI)
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
            repeatPenalty: repeatPenalty.isEmpty ? nil : Float(repeatPenalty),
            // Save systemPromptMode: "prepend" is default, only save when explicitly set
            systemPromptMode: providerType == "openai" ? systemPromptMode : nil
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

                // Reload config outside MainActor to avoid try! crash
                let config = try core.loadConfig()

                await MainActor.run {
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

    /// Get specific validation error message for user feedback
    /// Returns nil if form is valid
    private func getValidationError() -> String? {
        // Basic required fields
        if providerName.trimmingCharacters(in: .whitespaces).isEmpty {
            return L("provider.error.name_required")
        }
        if model.trimmingCharacters(in: .whitespaces).isEmpty {
            return L("provider.error.model_required")
        }

        // API key required for non-Ollama providers
        if providerType != "ollama" && apiKey.trimmingCharacters(in: .whitespaces).isEmpty {
            return L("provider.error.api_key_required")
        }

        // Base URL required for custom providers
        if isCustomProvider && baseURL.trimmingCharacters(in: .whitespaces).isEmpty {
            return L("provider.error.base_url_required")
        }

        // Timeout validation
        if UInt64(timeoutSeconds) == nil || UInt64(timeoutSeconds) == 0 {
            return L("provider.error.invalid_timeout")
        }

        // Common parameter validation
        if !maxTokens.isEmpty {
            if UInt32(maxTokens) == nil || UInt32(maxTokens) == 0 {
                return L("provider.error.invalid_max_tokens")
            }
        }

        if !temperature.isEmpty {
            guard let temp = Float(temperature) else {
                return L("provider.error.invalid_temperature")
            }
            // Provider-specific temperature range
            switch providerType {
            case "claude":
                if temp < 0.0 || temp > 1.0 {
                    return String(format: L("provider.error.temperature_range"), "0.0-1.0")
                }
            case "openai", "gemini":
                if temp < 0.0 || temp > 2.0 {
                    return String(format: L("provider.error.temperature_range"), "0.0-2.0")
                }
            case "ollama":
                if temp < 0.0 {
                    return String(format: L("provider.error.temperature_range"), "≥ 0.0")
                }
            default:
                if temp < 0.0 || temp > 2.0 {
                    return String(format: L("provider.error.temperature_range"), "0.0-2.0")
                }
            }
        }

        if !topP.isEmpty {
            if let p = Float(topP), p < 0.0 || p > 1.0 {
                return L("provider.error.invalid_top_p")
            } else if Float(topP) == nil {
                return L("provider.error.invalid_top_p")
            }
        }

        if !topK.isEmpty {
            if UInt32(topK) == nil || UInt32(topK) == 0 {
                return L("provider.error.invalid_top_k")
            }
        }

        // OpenAI-specific validation
        if providerType == "openai" {
            if !frequencyPenalty.isEmpty {
                if let penalty = Float(frequencyPenalty), penalty < -2.0 || penalty > 2.0 {
                    return L("provider.error.invalid_frequency_penalty")
                } else if Float(frequencyPenalty) == nil {
                    return L("provider.error.invalid_frequency_penalty")
                }
            }

            if !presencePenalty.isEmpty {
                if let penalty = Float(presencePenalty), penalty < -2.0 || penalty > 2.0 {
                    return L("provider.error.invalid_presence_penalty")
                } else if Float(presencePenalty) == nil {
                    return L("provider.error.invalid_presence_penalty")
                }
            }
        }

        // Ollama-specific validation
        if providerType == "ollama" {
            if !repeatPenalty.isEmpty {
                if let penalty = Float(repeatPenalty), penalty < 1.0 {
                    return L("provider.error.invalid_repeat_penalty")
                } else if Float(repeatPenalty) == nil {
                    return L("provider.error.invalid_repeat_penalty")
                }
            }
        }

        return nil
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

    /// Get the default base URL for the current provider
    /// Returns nil if no default URL is available (e.g., Azure OpenAI, GitHub Copilot)
    private func getDefaultBaseUrl() -> String? {
        // Use provider name to determine default URL
        let name = providerName.lowercased()
        switch name {
        case "openai":
            return "https://api.openai.com/v1"
        case "anthropic":
            return "https://api.anthropic.com"
        case "google-gemini":
            return "https://generativelanguage.googleapis.com"
        case "ollama":
            return "http://localhost:11434"
        case "deepseek":
            return "https://api.deepseek.com"
        case "moonshot":
            return "https://api.moonshot.cn/v1"
        case "openrouter":
            return "https://openrouter.ai/api/v1"
        // Azure OpenAI and GitHub Copilot require user configuration
        case "azure-openai", "github-copilot":
            return nil
        default:
            // For other providers, check by provider type
            switch providerType {
            case "claude":
                return "https://api.anthropic.com"
            case "gemini":
                return "https://generativelanguage.googleapis.com"
            case "ollama":
                return "http://localhost:11434"
            default:
                return nil
            }
        }
    }

    /// Get placeholder for base URL field
    /// Shows default URL for preset providers, or example URL for custom providers
    private func getBaseUrlPlaceholder() -> String {
        if isCustomProvider {
            return L("provider.placeholder.base_url_custom")
        }
        // For preset providers, show default URL as placeholder if available
        if let defaultUrl = getDefaultBaseUrl() {
            return defaultUrl
        }
        return L("provider.placeholder.base_url_custom")
    }

    /// Get help text for base URL field
    private func getBaseUrlHelp() -> String {
        if isCustomProvider {
            return L("provider.help.base_url_custom")
        }
        // For preset providers with default URL, show combined help text
        if getDefaultBaseUrl() != nil {
            return L("provider.help.base_url_with_default")
        }
        return L("provider.help.base_url_custom")
    }
}
