//
//  ModelProfileEditSheet.swift
//  Aether
//
//  Edit sheet for adding/editing AI model profiles.
//

import SwiftUI

/// Edit sheet for model profile configuration
struct ModelProfileEditSheet: View {
    // Dependencies
    let core: AetherCore?
    let profile: ModelProfileFfi?  // nil for new profile
    @Binding var isPresented: Bool
    let onSave: () -> Void

    // Form state
    @State private var profileId: String = ""
    @State private var provider: String = "anthropic"
    @State private var modelName: String = ""
    @State private var selectedCapabilities: Set<ModelCapabilityFfi> = []
    @State private var costTier: ModelCostTierFfi = .medium
    @State private var latencyTier: ModelLatencyTierFfi = .medium
    @State private var maxContext: String = ""
    @State private var isLocal: Bool = false

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    private var isEditing: Bool { profile != nil }

    private let providers = [
        ("anthropic", "Anthropic"),
        ("openai", "OpenAI"),
        ("google", "Google"),
        ("ollama", "Ollama"),
        ("deepseek", "DeepSeek"),
        ("azure", "Azure OpenAI"),
    ]

    private let allCapabilities: [ModelCapabilityFfi] = [
        .codeGeneration,
        .codeReview,
        .textAnalysis,
        .imageUnderstanding,
        .videoUnderstanding,
        .longContext,
        .reasoning,
        .localPrivacy,
        .fastResponse,
        .simpleTask,
        .longDocument,
    ]

    var body: some View {
        NavigationStack {
            Form {
                // Basic Information
                Section(L("settings.model_routing.edit.basic")) {
                    if !isEditing {
                        TextField(L("settings.model_routing.edit.profile_id"), text: $profileId)
                            .textFieldStyle(.roundedBorder)
                    } else {
                        LabeledContent(L("settings.model_routing.edit.profile_id"), value: profileId)
                    }

                    Picker(L("settings.model_routing.edit.provider"), selection: $provider) {
                        ForEach(providers, id: \.0) { provider in
                            Text(provider.1).tag(provider.0)
                        }
                    }

                    TextField(L("settings.model_routing.edit.model_name"), text: $modelName)
                        .textFieldStyle(.roundedBorder)
                }

                // Capabilities
                Section(L("settings.model_routing.edit.capabilities")) {
                    ForEach(allCapabilities, id: \.self) { capability in
                        Toggle(capabilityDisplayName(capability), isOn: Binding(
                            get: { selectedCapabilities.contains(capability) },
                            set: { isSelected in
                                if isSelected {
                                    selectedCapabilities.insert(capability)
                                } else {
                                    selectedCapabilities.remove(capability)
                                }
                            }
                        ))
                    }
                }

                // Cost & Performance
                Section(L("settings.model_routing.edit.performance")) {
                    Picker(L("settings.model_routing.edit.cost_tier"), selection: $costTier) {
                        Text(L("settings.model_routing.cost.free")).tag(ModelCostTierFfi.free)
                        Text(L("settings.model_routing.cost.low")).tag(ModelCostTierFfi.low)
                        Text(L("settings.model_routing.cost.medium")).tag(ModelCostTierFfi.medium)
                        Text(L("settings.model_routing.cost.high")).tag(ModelCostTierFfi.high)
                    }

                    Picker(L("settings.model_routing.edit.latency_tier"), selection: $latencyTier) {
                        Text(L("settings.model_routing.latency.fast")).tag(ModelLatencyTierFfi.fast)
                        Text(L("settings.model_routing.latency.medium")).tag(ModelLatencyTierFfi.medium)
                        Text(L("settings.model_routing.latency.slow")).tag(ModelLatencyTierFfi.slow)
                    }
                }

                // Advanced
                Section(L("settings.model_routing.edit.advanced")) {
                    TextField(L("settings.model_routing.edit.max_context"), text: $maxContext)
                        .textFieldStyle(.roundedBorder)

                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Image(systemName: "info.circle")
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                        Text(L("settings.model_routing.edit.max_context_hint"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    Toggle(L("settings.model_routing.edit.local"), isOn: $isLocal)

                    if isLocal {
                        HStack(spacing: DesignTokens.Spacing.xs) {
                            Image(systemName: "info.circle")
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                            Text(L("settings.model_routing.edit.local_hint"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                    }
                }

                // Error message
                if let error = errorMessage {
                    Section {
                        HStack(spacing: DesignTokens.Spacing.sm) {
                            Image(systemName: "exclamationmark.triangle")
                                .foregroundColor(.red)
                            Text(error)
                                .foregroundColor(.red)
                        }
                    }
                }
            }
            .formStyle(.grouped)
            .frame(minWidth: 450, minHeight: 500)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("common.cancel")) {
                        isPresented = false
                    }
                    .disabled(isSaving)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isEditing ? L("common.save") : L("common.add")) {
                        saveProfile()
                    }
                    .disabled(!isValid || isSaving)
                }
            }
            .navigationTitle(isEditing ?
                L("settings.model_routing.edit.title_edit") :
                L("settings.model_routing.edit.title_add"))
        }
        .onAppear {
            loadProfile()
        }
    }

    // MARK: - Validation

    private var isValid: Bool {
        !profileId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
        !modelName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    // MARK: - Helper Functions

    private func capabilityDisplayName(_ capability: ModelCapabilityFfi) -> String {
        switch capability {
        case .codeGeneration: return L("settings.model_routing.capability.code_generation")
        case .codeReview: return L("settings.model_routing.capability.code_review")
        case .textAnalysis: return L("settings.model_routing.capability.text_analysis")
        case .imageUnderstanding: return L("settings.model_routing.capability.image_understanding")
        case .videoUnderstanding: return L("settings.model_routing.capability.video_understanding")
        case .longContext: return L("settings.model_routing.capability.long_context")
        case .reasoning: return L("settings.model_routing.capability.reasoning")
        case .localPrivacy: return L("settings.model_routing.capability.local_privacy")
        case .fastResponse: return L("settings.model_routing.capability.fast_response")
        case .simpleTask: return L("settings.model_routing.capability.simple_task")
        case .longDocument: return L("settings.model_routing.capability.long_document")
        }
    }

    // MARK: - Data Operations

    private func loadProfile() {
        guard let profile = profile else { return }

        profileId = profile.id
        provider = profile.provider
        modelName = profile.model
        selectedCapabilities = Set(profile.capabilities)
        costTier = profile.costTier
        latencyTier = profile.latencyTier
        if let ctx = profile.maxContext {
            maxContext = String(ctx)
        }
        isLocal = profile.local
    }

    private func saveProfile() {
        guard let core = core else {
            errorMessage = L("error.core_not_initialized")
            return
        }

        isSaving = true
        errorMessage = nil

        // Parse max context
        let parsedMaxContext: UInt32? = {
            let trimmed = maxContext.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { return nil }
            return UInt32(trimmed)
        }()

        // Build profile
        let newProfile = ModelProfileFfi(
            id: profileId.trimmingCharacters(in: .whitespacesAndNewlines),
            provider: provider,
            model: modelName.trimmingCharacters(in: .whitespacesAndNewlines),
            capabilities: Array(selectedCapabilities),
            costTier: costTier,
            latencyTier: latencyTier,
            maxContext: parsedMaxContext,
            local: isLocal
        )

        do {
            try core.coworkUpdateModelProfile(profile: newProfile)
            onSave()
            isPresented = false
        } catch {
            errorMessage = error.localizedDescription
        }

        isSaving = false
    }
}

// MARK: - Preview

#Preview {
    ModelProfileEditSheet(
        core: nil,
        profile: nil,
        isPresented: .constant(true),
        onSave: {}
    )
}
