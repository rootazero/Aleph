//
//  RuleEditorView.swift
//  Aether
//
//  Modal dialog for adding/editing routing rules
//

import SwiftUI

/// Modal dialog for configuring routing rules
struct RuleEditorView: View {
    @Environment(\.dismiss) private var dismiss
    @Binding var rules: [RoutingRuleConfig]

    // Core reference for validation and saving
    let core: AetherCore
    let providers: [ProviderConfigEntry]

    // Edit mode: nil for new rule, index for editing
    let editingIndex: Int?

    // Form state
    @State private var pattern: String = ""
    @State private var selectedProvider: String = ""
    @State private var systemPrompt: String = ""

    // Pattern testing
    @State private var testInput: String = ""
    @State private var testResult: TestResult?

    // UI state
    @State private var isSaving: Bool = false
    @State private var errorMessage: String?
    @State private var patternError: String?

    enum TestResult {
        case match
        case noMatch
    }

    // Initialize for new rule
    init(rules: Binding<[RoutingRuleConfig]>, core: AetherCore, providers: [ProviderConfigEntry]) {
        self._rules = rules
        self.core = core
        self.providers = providers
        self.editingIndex = nil
    }

    // Initialize for editing existing rule
    init(rules: Binding<[RoutingRuleConfig]>, core: AetherCore, providers: [ProviderConfigEntry], editing index: Int) {
        self._rules = rules
        self.core = core
        self.providers = providers
        self.editingIndex = index
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text(editingIndex == nil ? "Add Routing Rule" : "Edit Routing Rule")
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
                    // Pattern Input
                    FormField(title: "Regex Pattern") {
                        TextField("e.g., ^/draw or .*code.*", text: $pattern)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(.body, design: .monospaced))
                            .onChange(of: pattern) {
                                validatePattern()
                            }

                        if let error = patternError {
                            HStack(spacing: 6) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .foregroundColor(.red)
                                    .imageScale(.small)
                                Text(error)
                                    .font(.caption)
                                    .foregroundColor(.red)
                            }
                        } else if !pattern.isEmpty {
                            HStack(spacing: 6) {
                                Image(systemName: "checkmark.circle.fill")
                                    .foregroundColor(.green)
                                    .imageScale(.small)
                                Text("Valid regex pattern")
                                    .font(.caption)
                                    .foregroundColor(.green)
                            }
                        }

                        Text("Use regex to match clipboard content. Examples:\n• ^/draw - Starts with /draw\n• .*code.* - Contains 'code'\n• .* - Matches everything")
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    // Provider Selection
                    FormField(title: "Provider") {
                        Picker("Select provider", selection: $selectedProvider) {
                            ForEach(providers, id: \.name) { provider in
                                HStack {
                                    Circle()
                                        .fill(Color(hex: provider.config.color) ?? .gray)
                                        .frame(width: 10, height: 10)
                                    Text(provider.name)
                                }
                                .tag(provider.name)
                            }
                        }
                        .pickerStyle(.menu)
                        .frame(maxWidth: .infinity, alignment: .leading)

                        if providers.isEmpty {
                            Text("No providers configured. Add a provider in the Providers tab first.")
                                .font(.caption)
                                .foregroundColor(.orange)
                        }
                    }

                    // System Prompt (Optional)
                    FormField(title: "System Prompt (Optional)") {
                        VStack(alignment: .leading, spacing: 6) {
                            TextEditor(text: $systemPrompt)
                                .font(.system(.body, design: .monospaced))
                                .frame(minHeight: 100, maxHeight: 200)
                                .border(Color.gray.opacity(0.3))

                            Text("Custom instructions for this rule. Leave empty to use default.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                    Divider()

                    // Pattern Tester
                    FormField(title: "Test Pattern") {
                        VStack(alignment: .leading, spacing: 12) {
                            HStack(spacing: 8) {
                                TextField("Enter test input", text: $testInput)
                                    .textFieldStyle(.roundedBorder)

                                Button(action: testPattern) {
                                    Text("Test")
                                }
                                .disabled(pattern.isEmpty || patternError != nil)
                            }

                            if let result = testResult {
                                HStack(spacing: 8) {
                                    switch result {
                                    case .match:
                                        Image(systemName: "checkmark.circle.fill")
                                            .foregroundColor(.green)
                                        Text("Pattern matches!")
                                            .foregroundColor(.green)
                                    case .noMatch:
                                        Image(systemName: "xmark.circle.fill")
                                            .foregroundColor(.orange)
                                        Text("Pattern does not match")
                                            .foregroundColor(.orange)
                                    }
                                }
                                .font(.callout)
                                .padding(8)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(result == .match ? Color.green.opacity(0.1) : Color.orange.opacity(0.1))
                                .cornerRadius(6)
                            }
                        }
                    }

                    // Error message
                    if let error = errorMessage {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.red)
                            Text(error)
                                .font(.callout)
                        }
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color.red.opacity(0.1))
                        .cornerRadius(8)
                    }
                }
                .padding(20)
            }

            Divider()

            // Footer buttons
            HStack(spacing: 12) {
                Spacer()

                Button("Cancel") {
                    dismiss()
                }
                .keyboardShortcut(.escape)

                Button(action: saveRule) {
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
        .frame(width: 600, height: 650)
        .onAppear {
            loadFormData()
        }
    }

    // MARK: - Data Loading

    /// Load form data on appear to ensure correct state
    private func loadFormData() {
        if let index = editingIndex, index < rules.count {
            // Load existing rule data
            let rule = rules[index]
            pattern = rule.regex
            selectedProvider = rule.provider
            systemPrompt = rule.systemPrompt ?? ""
        } else {
            // New rule: set default provider
            pattern = ""
            systemPrompt = ""
            if let firstProvider = providers.first {
                selectedProvider = firstProvider.name
            } else {
                selectedProvider = ""
            }
        }
        // Reset test state
        testInput = ""
        testResult = nil
        errorMessage = nil
        isSaving = false
        // Validate pattern after loading
        validatePattern()
    }

    // MARK: - Validation

    private func isFormValid() -> Bool {
        // Pattern required and must be valid
        guard !pattern.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        guard patternError == nil else { return false }

        // Provider required
        guard !selectedProvider.isEmpty else { return false }

        return true
    }

    private func validatePattern() {
        guard !pattern.isEmpty else {
            patternError = nil
            return
        }

        do {
            let isValid = try core.validateRegex(pattern: pattern)
            patternError = isValid ? nil : "Invalid regex pattern"
        } catch {
            patternError = "Invalid regex: \(error.localizedDescription)"
        }
    }

    private func testPattern() {
        guard patternError == nil, !pattern.isEmpty, !testInput.isEmpty else { return }

        // Test pattern match using NSRegularExpression
        do {
            let regex = try NSRegularExpression(pattern: pattern, options: [])
            let range = NSRange(testInput.startIndex..., in: testInput)
            let match = regex.firstMatch(in: testInput, options: [], range: range)

            testResult = match != nil ? .match : .noMatch
        } catch {
            testResult = .noMatch
        }
    }

    // MARK: - Actions

    private func saveRule() {
        guard isFormValid() else { return }

        isSaving = true
        errorMessage = nil

        Task {
            do {
                // Create new rule config
                let newRule = RoutingRuleConfig(
                    regex: pattern,
                    provider: selectedProvider,
                    systemPrompt: systemPrompt.isEmpty ? nil : systemPrompt,
                    stripPrefix: nil  // Auto-detect: true for ^/ patterns
                )

                // Update rules array
                var updatedRules = rules
                if let index = editingIndex {
                    // Replace existing rule
                    updatedRules[index] = newRule
                } else {
                    // Add new rule at the beginning (highest priority)
                    updatedRules.insert(newRule, at: 0)
                }

                // Save via Rust core (will validate and persist)
                try core.updateRoutingRules(rules: updatedRules)

                // Reload config to update UI
                let fullConfig = try core.loadConfig()

                await MainActor.run {
                    rules = fullConfig.rules
                    dismiss()
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to save rule: \(error.localizedDescription)"
                    isSaving = false
                }
            }
        }
    }
}

// MARK: - Color Extension


