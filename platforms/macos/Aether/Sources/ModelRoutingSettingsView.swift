//
//  ModelRoutingSettingsView.swift
//  Aether
//
//  Model routing rules configuration UI.
//  Configures how tasks are routed to different AI models.
//

import SwiftUI

/// Model routing settings view for task-to-model mapping configuration
struct ModelRoutingSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @Binding var isPresented: Bool

    // State
    @State private var routingRules: ModelRoutingRulesFfi?
    @State private var profiles: [ModelProfileFfi] = []
    @State private var isLoading = true
    @State private var errorMessage: String?
    @State private var isSaving = false

    // Editable state
    @State private var costStrategy: ModelCostStrategyFfi = .balanced
    @State private var defaultModel: String = ""
    @State private var enablePipelines: Bool = true

    // Task type mappings
    @State private var codeGenerationModel: String = ""
    @State private var codeReviewModel: String = ""
    @State private var imageAnalysisModel: String = ""
    @State private var videoUnderstandingModel: String = ""
    @State private var longDocumentModel: String = ""
    @State private var quickTasksModel: String = ""
    @State private var privacySensitiveModel: String = ""
    @State private var reasoningModel: String = ""

    private let taskTypes = [
        ("code_generation", "settings.model_routing.task.code_generation"),
        ("code_review", "settings.model_routing.task.code_review"),
        ("image_analysis", "settings.model_routing.task.image_analysis"),
        ("video_understanding", "settings.model_routing.task.video_understanding"),
        ("long_document", "settings.model_routing.task.long_document"),
        ("quick_tasks", "settings.model_routing.task.quick_tasks"),
        ("privacy_sensitive", "settings.model_routing.task.privacy_sensitive"),
        ("reasoning", "settings.model_routing.task.reasoning"),
    ]

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    if isLoading {
                        loadingView
                    } else {
                        strategySection
                        defaultModelSection
                        pipelinesSection
                        taskMappingsSection
                    }

                    if let error = errorMessage {
                        errorView(error)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
            .frame(minWidth: 500, minHeight: 500)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("common.cancel")) {
                        isPresented = false
                    }
                    .disabled(isSaving)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(L("common.save")) {
                        saveSettings()
                    }
                    .disabled(isSaving)
                }
            }
            .navigationTitle(L("settings.model_routing.routing.title"))
        }
        .onAppear {
            loadSettings()
        }
    }

    // MARK: - View Components

    private var loadingView: some View {
        HStack {
            Spacer()
            ProgressView()
                .progressViewStyle(.circular)
            Spacer()
        }
        .padding(DesignTokens.Spacing.xl)
    }

    private var strategySection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.model_routing.routing.cost_strategy"), systemImage: "dollarsign.circle")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.model_routing.routing.cost_strategy_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Picker(L("settings.model_routing.routing.cost_strategy_picker"), selection: $costStrategy) {
                Text(L("settings.model_routing.strategy.cheapest")).tag(ModelCostStrategyFfi.cheapest)
                Text(L("settings.model_routing.strategy.balanced")).tag(ModelCostStrategyFfi.balanced)
                Text(L("settings.model_routing.strategy.best_quality")).tag(ModelCostStrategyFfi.bestQuality)
            }
            .pickerStyle(.segmented)
            .padding(.top, DesignTokens.Spacing.xs)

            // Strategy description
            strategyHint
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    @ViewBuilder
    private var strategyHint: some View {
        let (icon, text): (String, String) = {
            switch costStrategy {
            case .cheapest:
                return ("leaf", L("settings.model_routing.strategy.cheapest_hint"))
            case .balanced:
                return ("scale.3d", L("settings.model_routing.strategy.balanced_hint"))
            case .bestQuality:
                return ("star", L("settings.model_routing.strategy.best_quality_hint"))
            }
        }()

        HStack(spacing: DesignTokens.Spacing.xs) {
            Image(systemName: icon)
                .foregroundColor(DesignTokens.Colors.accentBlue)
            Text(text)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .padding(.top, DesignTokens.Spacing.xs)
    }

    private var defaultModelSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.model_routing.routing.default_model"), systemImage: "star.fill")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.model_routing.routing.default_model_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Picker(L("settings.model_routing.routing.default_model_picker"), selection: $defaultModel) {
                Text(L("settings.model_routing.routing.none")).tag("")
                ForEach(profiles, id: \.id) { profile in
                    Text("\(profile.id) (\(profile.provider))").tag(profile.id)
                }
            }
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var pipelinesSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.model_routing.routing.pipelines"), systemImage: "arrow.triangle.branch")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.model_routing.routing.pipelines_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.model_routing.routing.pipelines_toggle"), isOn: $enablePipelines)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var taskMappingsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Divider()
                .padding(.vertical, DesignTokens.Spacing.md)

            Label(L("settings.model_routing.routing.task_mappings"), systemImage: "arrow.right.arrow.left")
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.model_routing.routing.task_mappings_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Task type mappings
            taskMappingRow(
                taskType: "code_generation",
                label: L("settings.model_routing.task.code_generation"),
                icon: "chevron.left.forwardslash.chevron.right",
                binding: $codeGenerationModel
            )

            taskMappingRow(
                taskType: "code_review",
                label: L("settings.model_routing.task.code_review"),
                icon: "eye",
                binding: $codeReviewModel
            )

            taskMappingRow(
                taskType: "image_analysis",
                label: L("settings.model_routing.task.image_analysis"),
                icon: "photo",
                binding: $imageAnalysisModel
            )

            taskMappingRow(
                taskType: "video_understanding",
                label: L("settings.model_routing.task.video_understanding"),
                icon: "video",
                binding: $videoUnderstandingModel
            )

            taskMappingRow(
                taskType: "long_document",
                label: L("settings.model_routing.task.long_document"),
                icon: "doc.text",
                binding: $longDocumentModel
            )

            taskMappingRow(
                taskType: "quick_tasks",
                label: L("settings.model_routing.task.quick_tasks"),
                icon: "bolt",
                binding: $quickTasksModel
            )

            taskMappingRow(
                taskType: "privacy_sensitive",
                label: L("settings.model_routing.task.privacy_sensitive"),
                icon: "lock.shield",
                binding: $privacySensitiveModel
            )

            taskMappingRow(
                taskType: "reasoning",
                label: L("settings.model_routing.task.reasoning"),
                icon: "brain",
                binding: $reasoningModel
            )
        }
    }

    private func taskMappingRow(taskType: String, label: String, icon: String, binding: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            HStack {
                Label(label, systemImage: icon)
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                Picker("", selection: binding) {
                    Text(L("settings.model_routing.routing.auto")).tag("")
                    ForEach(profiles, id: \.id) { profile in
                        Text(profile.id).tag(profile.id)
                    }
                }
                .frame(width: 200)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private func errorView(_ error: String) -> some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundColor(.red)
            Text(error)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(.red)
        }
        .padding(DesignTokens.Spacing.md)
        .background(Color.red.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
    }

    // MARK: - Data Operations

    private func loadSettings() {
        guard let core = core else {
            errorMessage = L("error.core_not_initialized")
            isLoading = false
            return
        }

        isLoading = true
        errorMessage = nil

        // Load profiles for picker options
        profiles = core.agentGetModelProfiles()

        // Load routing rules
        let rules = core.agentGetRoutingRules()
        routingRules = rules

        // Set state from rules
        costStrategy = rules.costStrategy
        defaultModel = rules.defaultModel ?? ""
        enablePipelines = rules.enablePipelines

        // Load task type mappings
        for mapping in rules.taskTypeMappings {
            switch mapping.taskType {
            case "code_generation": codeGenerationModel = mapping.modelId
            case "code_review": codeReviewModel = mapping.modelId
            case "image_analysis": imageAnalysisModel = mapping.modelId
            case "video_understanding": videoUnderstandingModel = mapping.modelId
            case "long_document": longDocumentModel = mapping.modelId
            case "quick_tasks": quickTasksModel = mapping.modelId
            case "privacy_sensitive": privacySensitiveModel = mapping.modelId
            case "reasoning": reasoningModel = mapping.modelId
            default: break
            }
        }

        isLoading = false
    }

    private func saveSettings() {
        guard let core = core else {
            errorMessage = L("error.core_not_initialized")
            return
        }

        isSaving = true
        errorMessage = nil

        do {
            // Update cost strategy
            try core.agentUpdateCostStrategy(strategy: costStrategy)

            // Update default model
            if !defaultModel.isEmpty {
                try core.agentUpdateDefaultModel(modelId: defaultModel)
            }

            // Update task type mappings
            let mappings: [(String, String)] = [
                ("code_generation", codeGenerationModel),
                ("code_review", codeReviewModel),
                ("image_analysis", imageAnalysisModel),
                ("video_understanding", videoUnderstandingModel),
                ("long_document", longDocumentModel),
                ("quick_tasks", quickTasksModel),
                ("privacy_sensitive", privacySensitiveModel),
                ("reasoning", reasoningModel),
            ]

            for (taskType, modelId) in mappings {
                if !modelId.isEmpty {
                    try core.agentUpdateRoutingRule(taskType: taskType, modelId: modelId)
                } else {
                    // Try to delete the rule (ignore if not found)
                    try? core.agentDeleteRoutingRule(taskType: taskType)
                }
            }

            isPresented = false
        } catch {
            errorMessage = error.localizedDescription
        }

        isSaving = false
    }
}

// MARK: - Preview

#Preview {
    ModelRoutingSettingsView(
        core: nil,
        isPresented: .constant(true)
    )
}
