//
//  ModelProfilesSettingsView.swift
//  Aether
//
//  Model profiles configuration UI for multi-model routing.
//  Displays and manages AI model profiles with their capabilities.
//

import SwiftUI

/// Model profiles settings view for configuring AI models
struct ModelProfilesSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @Binding var isPresented: Bool

    // State
    @State private var profiles: [ModelProfileFfi] = []
    @State private var selectedProfile: ModelProfileFfi?
    @State private var isShowingEditSheet = false
    @State private var isShowingAddSheet = false
    @State private var isLoading = true
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    headerSection

                    if isLoading {
                        loadingView
                    } else if profiles.isEmpty {
                        emptyStateView
                    } else {
                        profilesList
                    }

                    if let error = errorMessage {
                        errorView(error)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
            .frame(minWidth: 500, minHeight: 400)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("common.close")) {
                        isPresented = false
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        selectedProfile = nil
                        isShowingAddSheet = true
                    } label: {
                        Label(L("settings.model_routing.profiles.add"), systemImage: "plus")
                    }
                }
            }
            .navigationTitle(L("settings.model_routing.profiles.title"))
        }
        .onAppear {
            loadProfiles()
        }
        .sheet(isPresented: $isShowingEditSheet) {
            if let profile = selectedProfile {
                ModelProfileEditSheet(
                    core: core,
                    profile: profile,
                    isPresented: $isShowingEditSheet,
                    onSave: { loadProfiles() }
                )
            }
        }
        .sheet(isPresented: $isShowingAddSheet) {
            ModelProfileEditSheet(
                core: core,
                profile: nil,
                isPresented: $isShowingAddSheet,
                onSave: { loadProfiles() }
            )
        }
    }

    // MARK: - View Components

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text(L("settings.model_routing.profiles.description"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var loadingView: some View {
        HStack {
            Spacer()
            ProgressView()
                .progressViewStyle(.circular)
            Spacer()
        }
        .padding(DesignTokens.Spacing.xl)
    }

    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "cpu")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text(L("settings.model_routing.profiles.empty"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Button {
                selectedProfile = nil
                isShowingAddSheet = true
            } label: {
                Label(L("settings.model_routing.profiles.add_first"), systemImage: "plus.circle")
            }
            .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.xl)
    }

    private var profilesList: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            ForEach(profiles, id: \.id) { profile in
                profileCard(for: profile)
            }
        }
    }

    private func profileCard(for profile: ModelProfileFfi) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            // Header row with name and actions
            HStack {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text(profile.id)
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    HStack(spacing: DesignTokens.Spacing.sm) {
                        Label(profile.provider, systemImage: "server.rack")
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Text("•")
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Text(profile.model)
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }

                Spacer()

                // Cost/Latency badges
                HStack(spacing: DesignTokens.Spacing.xs) {
                    costBadge(for: profile.costTier)
                    latencyBadge(for: profile.latencyTier)
                    if profile.local {
                        localBadge
                    }
                }

                // Actions
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Button {
                        selectedProfile = profile
                        isShowingEditSheet = true
                    } label: {
                        Image(systemName: "pencil")
                    }
                    .buttonStyle(.borderless)

                    Button {
                        deleteProfile(profile)
                    } label: {
                        Image(systemName: "trash")
                            .foregroundColor(.red)
                    }
                    .buttonStyle(.borderless)
                }
            }

            // Capabilities
            if !profile.capabilities.isEmpty {
                capabilitiesView(for: profile.capabilities)
            }

            // Max context
            if let maxContext = profile.maxContext {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "text.alignleft")
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Text(L("settings.model_routing.profiles.max_context_value", formatTokenCount(maxContext)))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private func capabilitiesView(for capabilities: [ModelCapabilityFfi]) -> some View {
        FlowLayout(spacing: DesignTokens.Spacing.xs) {
            ForEach(capabilities, id: \.self) { capability in
                capabilityBadge(for: capability)
            }
        }
    }

    private func capabilityBadge(for capability: ModelCapabilityFfi) -> some View {
        Text(capabilityDisplayName(capability))
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(DesignTokens.Colors.textSecondary)
            .padding(.horizontal, DesignTokens.Spacing.sm)
            .padding(.vertical, 4)
            .background(DesignTokens.Colors.surfaceSecondary)
            .clipShape(Capsule())
    }

    private func costBadge(for tier: ModelCostTierFfi) -> some View {
        let (color, text) = costTierInfo(tier)
        return Text(text)
            .font(.system(size: 10, weight: .semibold))
            .foregroundColor(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color)
            .clipShape(Capsule())
    }

    private func latencyBadge(for tier: ModelLatencyTierFfi) -> some View {
        let (color, text) = latencyTierInfo(tier)
        return Text(text)
            .font(.system(size: 10, weight: .semibold))
            .foregroundColor(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color)
            .clipShape(Capsule())
    }

    private var localBadge: some View {
        Text(L("settings.model_routing.profiles.local"))
            .font(.system(size: 10, weight: .semibold))
            .foregroundColor(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.purple)
            .clipShape(Capsule())
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

    private func costTierInfo(_ tier: ModelCostTierFfi) -> (Color, String) {
        switch tier {
        case .free: return (.green, L("settings.model_routing.cost.free"))
        case .low: return (.blue, L("settings.model_routing.cost.low"))
        case .medium: return (.orange, L("settings.model_routing.cost.medium"))
        case .high: return (.red, L("settings.model_routing.cost.high"))
        }
    }

    private func latencyTierInfo(_ tier: ModelLatencyTierFfi) -> (Color, String) {
        switch tier {
        case .fast: return (.green, L("settings.model_routing.latency.fast"))
        case .medium: return (.yellow, L("settings.model_routing.latency.medium"))
        case .slow: return (.orange, L("settings.model_routing.latency.slow"))
        }
    }

    private func formatTokenCount(_ count: UInt32) -> String {
        if count >= 1_000_000 {
            return String(format: "%.1fM", Double(count) / 1_000_000)
        } else if count >= 1_000 {
            return String(format: "%.0fK", Double(count) / 1_000)
        }
        return "\(count)"
    }

    // MARK: - Data Operations

    private func loadProfiles() {
        guard let core = core else {
            errorMessage = L("error.core_not_initialized")
            isLoading = false
            return
        }

        isLoading = true
        errorMessage = nil
        profiles = core.agentGetModelProfiles()
        isLoading = false
    }

    private func deleteProfile(_ profile: ModelProfileFfi) {
        guard let core = core else { return }

        do {
            try core.agentDeleteModelProfile(profileId: profile.id)
            loadProfiles()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

// MARK: - FlowLayout for Capabilities

struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let result = FlowResult(in: proposal.width ?? 0, subviews: subviews, spacing: spacing)
        return result.size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = FlowResult(in: bounds.width, subviews: subviews, spacing: spacing)
        for (index, subview) in subviews.enumerated() {
            subview.place(at: CGPoint(x: bounds.minX + result.positions[index].x,
                                       y: bounds.minY + result.positions[index].y),
                         proposal: .unspecified)
        }
    }

    struct FlowResult {
        var size: CGSize = .zero
        var positions: [CGPoint] = []

        init(in maxWidth: CGFloat, subviews: Subviews, spacing: CGFloat) {
            var currentX: CGFloat = 0
            var currentY: CGFloat = 0
            var lineHeight: CGFloat = 0
            var maxWidth: CGFloat = 0

            for subview in subviews {
                let size = subview.sizeThatFits(.unspecified)

                if currentX + size.width > maxWidth && currentX > 0 {
                    currentX = 0
                    currentY += lineHeight + spacing
                    lineHeight = 0
                }

                positions.append(CGPoint(x: currentX, y: currentY))
                lineHeight = max(lineHeight, size.height)
                currentX += size.width + spacing
                maxWidth = max(maxWidth, currentX)
            }

            size = CGSize(width: maxWidth, height: currentY + lineHeight)
        }
    }
}

// MARK: - Preview

#Preview {
    ModelProfilesSettingsView(
        core: nil,
        isPresented: .constant(true)
    )
}
