//
//  ProviderConfigView.swift
//  Aether
//
//  Modal dialog for adding/editing AI provider configurations
//

import SwiftUI

/// Modal dialog for configuring AI providers
struct ProviderConfigView: View {
    @Environment(\.dismiss) private var dismiss
    @Binding var providers: [ProviderConfigEntry]

    // Core reference for saving config
    let core: AetherCore

    // Edit mode: nil for new provider, provider name for editing
    let editingProvider: String?

    // Form state
    @State private var providerName: String = ""
    @State private var apiKey: String = ""
    @State private var model: String = ""
    @State private var baseURL: String = ""
    @State private var color: Color = .blue
    @State private var providerType: String = "openai"
    @State private var timeoutSeconds: String = "30"
    @State private var maxTokens: String = ""
    @State private var temperature: String = ""

    // UI state
    @State private var isTesting: Bool = false
    @State private var testResult: TestResult?
    @State private var isSaving: Bool = false
    @State private var errorMessage: String?

    enum TestResult {
        case success(String)
        case failure(String)
    }

    // Provider type options
    private let providerTypes = ["openai", "claude", "ollama", "custom"]

    // Default colors for providers
    private let defaultColors: [String: Color] = [
        "openai": Color(hex: "#10a37f") ?? .green,
        "claude": Color(hex: "#d97757") ?? .orange,
        "ollama": .black,
        "custom": .gray
    ]

    // Initialize for new provider
    init(providers: Binding<[ProviderConfigEntry]>, core: AetherCore) {
        self._providers = providers
        self.core = core
        self.editingProvider = nil
    }

    // Initialize for editing existing provider
    init(providers: Binding<[ProviderConfigEntry]>, core: AetherCore, editing providerName: String) {
        self._providers = providers
        self.core = core
        self.editingProvider = providerName

        // Load existing provider data
        if let provider = providers.wrappedValue.first(where: { $0.name == providerName }) {
            _providerName = State(initialValue: provider.name)
            _model = State(initialValue: provider.config.model)
            _baseURL = State(initialValue: provider.config.baseUrl ?? "")
            _color = State(initialValue: Color(hex: provider.config.color) ?? .blue)
            _providerType = State(initialValue: provider.config.providerType ?? "openai")
            _timeoutSeconds = State(initialValue: String(provider.config.timeoutSeconds))
            _maxTokens = State(initialValue: provider.config.maxTokens.map { String($0) } ?? "")
            _temperature = State(initialValue: provider.config.temperature.map { String($0) } ?? "")

            // Load API key from config
            _apiKey = State(initialValue: provider.config.apiKey ?? "")
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text(editingProvider == nil ? "Add Provider" : "Edit Provider")
                    .font(.title2)
                    .fontWeight(.semibold)
                Spacer()
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                        .imageScale(.large)
                }
                .buttonStyle(.plain)
            }
            .padding(20)

            Divider()

            // Form content
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    // Provider Name
                    FormField(title: "Provider Name") {
                        TextField("e.g., openai, claude, my-custom-provider", text: $providerName)
                            .textFieldStyle(.roundedBorder)
                            .disabled(editingProvider != nil) // Can't rename in edit mode
                    }

                    // Provider Type
                    FormField(title: "Provider Type") {
                        Picker("", selection: $providerType) {
                            ForEach(providerTypes, id: \.self) { type in
                                Text(type.capitalized).tag(type)
                            }
                        }
                        .pickerStyle(.segmented)
                        .onChange(of: providerType) { _, newType in
                            // Update default color when provider type changes
                            if let defaultColor = defaultColors[newType] {
                                color = defaultColor
                            }

                            // Set default model for known providers
                            switch newType {
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
                    }

                    // API Key (not required for Ollama)
                    if providerType != "ollama" {
                        FormField(title: "API Key") {
                            SecureField("Enter your API key", text: $apiKey)
                                .textFieldStyle(.roundedBorder)
                            Text("Stored in ~/.aether/config.toml")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                    // Model
                    FormField(title: "Model") {
                        TextField("e.g., gpt-4o, claude-3-5-sonnet-20241022", text: $model)
                            .textFieldStyle(.roundedBorder)
                    }

                    // Base URL (optional)
                    FormField(title: "Base URL (Optional)") {
                        TextField("Leave empty for official API", text: $baseURL)
                            .textFieldStyle(.roundedBorder)
                        Text("For custom OpenAI-compatible endpoints")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    // Color Picker
                    FormField(title: "Theme Color") {
                        HStack(spacing: 12) {
                            ColorPicker("", selection: $color, supportsOpacity: false)
                                .labelsHidden()

                            Circle()
                                .fill(color)
                                .frame(width: 32, height: 32)

                            Text("Used in Halo overlay")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                    // Advanced settings (collapsible)
                    DisclosureGroup("Advanced Settings") {
                        VStack(alignment: .leading, spacing: 16) {
                            // Timeout
                            FormField(title: "Timeout (seconds)") {
                                TextField("30", text: $timeoutSeconds)
                                    .textFieldStyle(.roundedBorder)
                                    .frame(width: 100)
                            }

                            // Max Tokens
                            FormField(title: "Max Tokens (Optional)") {
                                TextField("Leave empty for default", text: $maxTokens)
                                    .textFieldStyle(.roundedBorder)
                                    .frame(width: 150)
                            }

                            // Temperature
                            FormField(title: "Temperature (0.0-2.0)") {
                                TextField("Leave empty for default", text: $temperature)
                                    .textFieldStyle(.roundedBorder)
                                    .frame(width: 150)
                            }
                        }
                        .padding(.top, 8)
                    }
                    .padding(.vertical, 8)

                    // Test Connection Result
                    if let result = testResult {
                        switch result {
                        case .success(let message):
                            HStack(spacing: 8) {
                                Image(systemName: "checkmark.circle.fill")
                                    .foregroundColor(.green)
                                Text(message)
                                    .font(.callout)
                            }
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(Color.green.opacity(0.1))
                            .cornerRadius(8)

                        case .failure(let message):
                            HStack(spacing: 8) {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundColor(.red)
                                Text(message)
                                    .font(.callout)
                            }
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(Color.red.opacity(0.1))
                            .cornerRadius(8)
                        }
                    }

                    // Error message
                    if let error = errorMessage {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.orange)
                            Text(error)
                                .font(.callout)
                        }
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color.orange.opacity(0.1))
                        .cornerRadius(8)
                    }
                }
                .padding(20)
            }

            Divider()

            // Footer buttons
            HStack(spacing: 12) {
                // Test Connection button
                Button(action: testConnection) {
                    HStack {
                        if isTesting {
                            ProgressView()
                                .scaleEffect(0.7)
                                .frame(width: 14, height: 14)
                        } else {
                            Image(systemName: "network")
                        }
                        Text(isTesting ? "Testing..." : "Test Connection")
                    }
                }
                .disabled(isTesting || !isFormValid())

                Spacer()

                Button("Cancel") {
                    dismiss()
                }
                .keyboardShortcut(.escape)

                Button(action: saveProvider) {
                    HStack {
                        if isSaving {
                            ProgressView()
                                .scaleEffect(0.7)
                                .frame(width: 14, height: 14)
                        }
                        Text(isSaving ? "Saving..." : "Save")
                    }
                }
                .keyboardShortcut(.return)
                .buttonStyle(.borderedProminent)
                .disabled(isSaving || !isFormValid())
            }
            .padding(20)
        }
        .frame(width: 600, height: 700)
    }

    // MARK: - Form Validation

    private func isFormValid() -> Bool {
        // Provider name required
        guard !providerName.trimmingCharacters(in: .whitespaces).isEmpty else { return false }

        // Model required
        guard !model.trimmingCharacters(in: .whitespaces).isEmpty else { return false }

        // API key required for cloud providers (not Ollama)
        if providerType != "ollama" {
            guard !apiKey.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        }

        // Timeout must be valid number > 0
        guard let timeout = UInt64(timeoutSeconds), timeout > 0 else { return false }

        // Max tokens must be valid number if provided
        if !maxTokens.isEmpty {
            guard UInt32(maxTokens) != nil else { return false }
        }

        // Temperature must be valid float between 0-2 if provided
        if !temperature.isEmpty {
            guard let temp = Float(temperature), temp >= 0.0, temp <= 2.0 else { return false }
        }

        return true
    }

    // MARK: - Actions

    func testConnection() {
        guard isFormValid() else { return }

        isTesting = true
        testResult = nil
        errorMessage = nil

        Task {
            // Build temporary provider config with actual API key (not keychain reference)
            let testConfig = ProviderConfig(
                providerType: providerType,
                apiKey: providerType == "ollama" ? nil : apiKey,  // Use actual API key for testing
                model: model,
                baseUrl: baseURL.isEmpty ? nil : baseURL,
                color: color.toHex(),
                timeoutSeconds: UInt64(timeoutSeconds) ?? 30,
                enabled: false,
                maxTokens: maxTokens.isEmpty ? nil : UInt32(maxTokens),
                temperature: temperature.isEmpty ? nil : Float(temperature),
                topP: nil,
                topK: nil,
                frequencyPenalty: nil,
                presencePenalty: nil,
                stopSequences: nil,
                thinkingLevel: nil,
                mediaResolution: nil,
                repeatPenalty: nil,
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
        guard isFormValid() else { return }

        isSaving = true
        errorMessage = nil

        Task {
            do {
                // Build provider config with API key stored directly
                let config = ProviderConfig(
                    providerType: providerType,
                    apiKey: providerType == "ollama" ? nil : (apiKey.isEmpty ? nil : apiKey),
                    model: model,
                    baseUrl: baseURL.isEmpty ? nil : baseURL,
                    color: color.toHex(),
                    timeoutSeconds: UInt64(timeoutSeconds) ?? 30,
                    enabled: false,  // Providers are disabled by default, user must explicitly enable
                    maxTokens: maxTokens.isEmpty ? nil : UInt32(maxTokens),
                    temperature: temperature.isEmpty ? nil : Float(temperature),
                    topP: nil,
                    topK: nil,
                    frequencyPenalty: nil,
                    presencePenalty: nil,
                    stopSequences: nil,
                    thinkingLevel: nil,
                    mediaResolution: nil,
                    repeatPenalty: nil,
                    systemPromptMode: nil
                )

                // Save to Rust core and persist to config.toml
                try core.updateProvider(name: providerName, provider: config)

                // Reload config to update UI
                let fullConfig = try core.loadConfig()
                await MainActor.run {
                    providers = fullConfig.providers
                    dismiss()
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to save provider: \(error.localizedDescription)"
                    isSaving = false
                }
            }
        }
    }
}

// MARK: - Helper Views

struct FormField<Content: View>: View {
    let title: String
    let content: Content

    init(title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.headline)
                .foregroundColor(.primary)
            content
        }
    }
}
