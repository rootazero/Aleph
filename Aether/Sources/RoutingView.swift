//
//  RoutingView.swift
//  Aether
//
//  Routing rules configuration tab
//

import SwiftUI
import UniformTypeIdentifiers

/// State for the rule editor sheet
struct RuleEditorState: Identifiable {
    let id = UUID()
    let editingRule: RoutingRuleConfig?  // nil for new rule, non-nil for editing
    let editingIndex: Int?               // Index in customRules array
}

struct RoutingView: View {
    let core: AetherCore
    let providers: [ProviderConfigEntry]
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Rules state (only custom rules from config)
    @State private var customRules: [RoutingRuleConfig] = []
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?

    // Separated rules by type
    private var commandRules: [RoutingRuleConfig] {
        customRules.filter { $0.isCommandRule }
    }
    private var keywordRules: [RoutingRuleConfig] {
        customRules.filter { $0.isKeywordRule }
    }

    // UI state - use sheet(item:) for reliable data passing
    @State private var ruleEditorState: RuleEditorState?
    @State private var showingDeleteConfirmation: Bool = false
    @State private var deletingRuleIndex: Int?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xl) {
                // Error message
                if let error = errorMessage {
                    HStack(spacing: DesignTokens.Spacing.sm) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(DesignTokens.Colors.warning)
                        Text(error)
                            .font(DesignTokens.Typography.body)
                    }
                    .padding(DesignTokens.Spacing.md)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(DesignTokens.Colors.warning.opacity(0.1))
                    .cornerRadius(DesignTokens.CornerRadius.medium)
                }

                // =============================================
                // SECTION 1: Preset Commands (Hardcoded, Read-only)
                // =============================================
                presetCommandsSection

                // =============================================
                // SECTION 2: Custom Rules (User-defined)
                // =============================================
                customRulesSection

                // Footer info
                footerInfoSection
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadRules()
            // Set save bar to disabled state for instant-save view
            saveBarState.update(
                hasUnsavedChanges: false,
                isSaving: false,
                statusMessage: nil,
                onSave: nil,
                onCancel: nil
            )
        }
        .sheet(item: $ruleEditorState) { state in
            if let rule = state.editingRule, let index = state.editingIndex {
                RuleEditorView(rules: $customRules, core: core, providers: providers, editingRule: rule, editingIndex: index)
            } else {
                RuleEditorView(rules: $customRules, core: core, providers: providers)
            }
        }
        .alert(L("settings.routing.delete_rule"), isPresented: $showingDeleteConfirmation) {
            Button(L("common.cancel"), role: .cancel) {}
            Button(L("common.delete"), role: .destructive) {
                if let index = deletingRuleIndex {
                    deleteRule(at: index)
                }
            }
        } message: {
            if let index = deletingRuleIndex {
                Text(String(format: L("settings.routing.delete_rule_message"), customRules[index].regex))
            }
        }
    }

    // MARK: - Preset Commands Section (Hardcoded)

    private var presetCommandsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            // Section header
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text(L("settings.routing.preset_commands"))
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                    .fontWeight(.semibold)

                Text(L("settings.routing.preset_commands_subtitle"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            // Hardcoded preset commands
            VStack(spacing: DesignTokens.Spacing.sm) {
                ForEach(PresetCommands.all, id: \.command) { preset in
                    PresetCommandCard(preset: preset)
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.accentPurple.opacity(0.03))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.large, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.large, style: .continuous)
                .stroke(DesignTokens.Colors.accentPurple.opacity(0.15), lineWidth: 1)
        )
    }

    // MARK: - Custom Rules Section

    private var customRulesSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            // Section header with action buttons
            HStack {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text(L("settings.routing.custom_rules"))
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                        .fontWeight(.semibold)

                    Text(L("settings.routing.custom_rules_subtitle"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                Spacer()

                // Import/Export menu
                Menu {
                    Button(action: exportRules) {
                        Label(L("settings.routing.export_rules"), systemImage: "square.and.arrow.up")
                    }
                    .disabled(customRules.isEmpty)

                    Button(action: importRules) {
                        Label(L("settings.routing.import_rules"), systemImage: "square.and.arrow.down")
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .imageScale(.large)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                }
                .buttonStyle(.plain)
                .help(L("settings.routing.import_export_help"))

                // Add Rule button
                ActionButton(L("settings.routing.add_rule"), icon: "plus.circle.fill", style: .primary) {
                    addNewRule()
                }
            }

            // Custom rules list or empty state
            if isLoading {
                ProgressView(L("settings.routing.loading"))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, DesignTokens.Spacing.xl)
            } else if customRules.isEmpty {
                // Empty state
                VStack(spacing: DesignTokens.Spacing.md) {
                    Image(systemName: "square.stack.3d.up.slash")
                        .font(.system(size: 40))
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    Text(L("settings.routing.no_custom_rules"))
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    Text(L("settings.routing.no_custom_rules_message"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .multilineTextAlignment(.center)

                    ActionButton(L("settings.routing.add_rule"), icon: "plus.circle.fill", style: .primary) {
                        addNewRule()
                    }
                    .padding(.top, DesignTokens.Spacing.sm)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, DesignTokens.Spacing.xl)
                .background(DesignTokens.Colors.cardBackground.opacity(0.5))
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
            } else {
                // Command rules section
                commandRulesSubsection

                // Divider between sections
                if !commandRules.isEmpty && !keywordRules.isEmpty {
                    Rectangle()
                        .fill(DesignTokens.Colors.textSecondary.opacity(0.2))
                        .frame(height: 1)
                        .padding(.vertical, DesignTokens.Spacing.sm)
                }

                // Keyword rules section
                keywordRulesSubsection
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground.opacity(0.3))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.large, style: .continuous))
        .disableWindowDrag()  // Prevent window drag when reordering rules
    }

    // MARK: - Command Rules Subsection

    private var commandRulesSubsection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Subsection header
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "command")
                    .font(.system(size: 12))
                    .foregroundColor(DesignTokens.Colors.accentBlue)
                Text(L("settings.routing.command_rules_title"))
                    .font(DesignTokens.Typography.body)
                    .fontWeight(.medium)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                Text("(\(commandRules.count))")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            if commandRules.isEmpty {
                Text(L("settings.routing.no_command_rules"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, DesignTokens.Spacing.md)
            } else {
                // Command rules list with drag reordering
                List {
                    ForEach(Array(commandRules.enumerated()), id: \.element.regex) { index, rule in
                        let globalIndex = findGlobalIndex(for: rule)
                        RuleCard(
                            rule: rule,
                            index: index,
                            provider: providers.first(where: { $0.name == rule.provider }),
                            onEdit: { editRule(at: globalIndex) },
                            onDelete: { confirmDelete(at: globalIndex) }
                        )
                        .listRowInsets(EdgeInsets(top: 2, leading: 0, bottom: 2, trailing: 0))
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                    }
                    .onMove(perform: moveCommandRules)
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .frame(height: min(CGFloat(commandRules.count) * 74, 296))  // 74pt per rule (70 + 4 gap), max 4 visible
            }
        }
    }

    // MARK: - Keyword Rules Subsection

    private var keywordRulesSubsection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Subsection header
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "text.magnifyingglass")
                    .font(.system(size: 12))
                    .foregroundColor(DesignTokens.Colors.success)
                Text(L("settings.routing.keyword_rules_title"))
                    .font(DesignTokens.Typography.body)
                    .fontWeight(.medium)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                Text("(\(keywordRules.count))")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            if keywordRules.isEmpty {
                Text(L("settings.routing.no_keyword_rules"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, DesignTokens.Spacing.md)
            } else {
                // Keyword rules list with drag reordering
                List {
                    ForEach(Array(keywordRules.enumerated()), id: \.element.regex) { index, rule in
                        let globalIndex = findGlobalIndex(for: rule)
                        RuleCard(
                            rule: rule,
                            index: index,
                            provider: providers.first(where: { $0.name == rule.provider }),
                            onEdit: { editRule(at: globalIndex) },
                            onDelete: { confirmDelete(at: globalIndex) }
                        )
                        .listRowInsets(EdgeInsets(top: 2, leading: 0, bottom: 2, trailing: 0))
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                    }
                    .onMove(perform: moveKeywordRules)
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .frame(height: min(CGFloat(keywordRules.count) * 74, 296))  // 74pt per rule (70 + 4 gap), max 4 visible
            }
        }
    }

    // MARK: - Drag Reorder Helpers

    /// Find the global index of a rule in customRules array
    private func findGlobalIndex(for rule: RoutingRuleConfig) -> Int {
        customRules.firstIndex(where: { $0.regex == rule.regex }) ?? 0
    }

    /// Move command rules within command section only
    private func moveCommandRules(from source: IndexSet, to destination: Int) {
        // Get current command rules
        var commands = commandRules

        // Perform move within command rules
        commands.move(fromOffsets: source, toOffset: destination)

        // Rebuild customRules: new command order + existing keyword order
        let updatedRules = commands + keywordRules

        // Save to config
        saveReorderedRules(updatedRules)
    }

    /// Move keyword rules within keyword section only
    private func moveKeywordRules(from source: IndexSet, to destination: Int) {
        // Get current keyword rules
        var keywords = keywordRules

        // Perform move within keyword rules
        keywords.move(fromOffsets: source, toOffset: destination)

        // Rebuild customRules: existing command order + new keyword order
        let updatedRules = commandRules + keywords

        // Save to config
        saveReorderedRules(updatedRules)
    }

    /// Save reordered rules to config
    private func saveReorderedRules(_ rules: [RoutingRuleConfig]) {
        Task {
            do {
                try core.updateRoutingRules(rules: rules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    customRules = config.rules.filter { !$0.isPreset }
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to reorder rules: \(error.localizedDescription)"
                }
            }
        }
    }

    // MARK: - Footer Info Section

    private var footerInfoSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Rule evaluation order hint
            HStack(spacing: DesignTokens.Spacing.sm) {
                Image(systemName: "info.circle")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                Text(L("settings.routing.footer_info"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            // Default provider hint
            HStack(spacing: DesignTokens.Spacing.sm) {
                Image(systemName: "info.circle")
                    .foregroundColor(DesignTokens.Colors.accentBlue)

                if let defaultName = defaultProviderName {
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Text(L("settings.routing.default_provider_hint"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        // Default provider badge
                        HStack(spacing: DesignTokens.Spacing.xs) {
                            if let color = defaultProviderColor {
                                Circle()
                                    .fill(color)
                                    .frame(width: 6, height: 6)
                            }
                            Text(defaultName)
                                .font(DesignTokens.Typography.caption)
                                .fontWeight(.semibold)
                                .foregroundColor(DesignTokens.Colors.textPrimary)
                        }
                        .padding(.horizontal, DesignTokens.Spacing.xs)
                        .padding(.vertical, 2)
                        .background(DesignTokens.Colors.accentBlue.opacity(0.1))
                        .cornerRadius(DesignTokens.CornerRadius.small)
                    }
                } else {
                    Text(L("settings.routing.no_default_provider_hint"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.warning)
                }
            }
        }
    }

    // MARK: - Computed Properties

    /// Get the current default provider name
    private var defaultProviderName: String? {
        return core.getDefaultProvider()
    }

    /// Get default provider color
    private var defaultProviderColor: Color? {
        guard let defaultName = defaultProviderName,
              let provider = providers.first(where: { $0.name == defaultName }) else {
            return nil
        }
        return Color(hex: provider.config.color)
    }

    // MARK: - Data Loading

    private func loadRules() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let config = try core.loadConfig()

                await MainActor.run {
                    // Filter out preset rules - only load custom rules
                    customRules = config.rules.filter { !$0.isPreset }
                    isLoading = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to load rules: \(error.localizedDescription)"
                    isLoading = false
                }
            }
        }
    }

    // MARK: - Actions

    private func addNewRule() {
        ruleEditorState = RuleEditorState(editingRule: nil, editingIndex: nil)
    }

    private func editRule(at index: Int) {
        guard index >= 0 && index < customRules.count else { return }
        let rule = customRules[index]
        ruleEditorState = RuleEditorState(editingRule: rule, editingIndex: index)
    }

    private func confirmDelete(at index: Int) {
        deletingRuleIndex = index
        showingDeleteConfirmation = true
    }

    private func deleteRule(at index: Int) {
        Task {
            do {
                var updatedRules = customRules
                updatedRules.remove(at: index)

                try core.updateRoutingRules(rules: updatedRules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    customRules = config.rules.filter { !$0.isPreset }
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to delete rule: \(error.localizedDescription)"
                }
            }
        }
    }

    // MARK: - Import/Export

    private func exportRules() {
        let savePanel = NSSavePanel()
        savePanel.title = "Export Routing Rules"
        savePanel.nameFieldStringValue = "aether-routing-rules.json"
        savePanel.allowedContentTypes = [.json]
        savePanel.canCreateDirectories = true

        savePanel.begin { response in
            guard response == .OK, let url = savePanel.url else { return }

            do {
                // Convert custom rules to JSON (exclude presets)
                let encoder = JSONEncoder()
                encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
                let jsonData = try encoder.encode(customRules)

                // Write to file
                try jsonData.write(to: url)

                // Show success notification
                DispatchQueue.main.async {
                    showInfoToast(
                        title: L("alert.routing.export_title"),
                        message: String(format: L("alert.routing.export_message"), url.lastPathComponent)
                    )
                }
            } catch {
                DispatchQueue.main.async {
                    errorMessage = "Failed to export rules: \(error.localizedDescription)"
                }
            }
        }
    }

    private func importRules() {
        let openPanel = NSOpenPanel()
        openPanel.title = "Import Routing Rules"
        openPanel.allowedContentTypes = [.json]
        openPanel.allowsMultipleSelection = false
        openPanel.canChooseDirectories = false

        openPanel.begin { response in
            guard response == .OK, let url = openPanel.url else { return }

            do {
                // Read JSON file
                let jsonData = try Data(contentsOf: url)

                // Decode rules
                let decoder = JSONDecoder()
                let importedRules = try decoder.decode([RoutingRuleConfig].self, from: jsonData)

                // Show import options
                DispatchQueue.main.async {
                    showImportOptions(importedRules: importedRules)
                }
            } catch {
                DispatchQueue.main.async {
                    errorMessage = "Failed to import rules: \(error.localizedDescription)"
                }
            }
        }
    }

    private func showImportOptions(importedRules: [RoutingRuleConfig]) {
        let alert = NSAlert()
        alert.messageText = L("alert.routing.import_title")
        alert.informativeText = String(format: L("alert.routing.import_message"), importedRules.count)
        alert.alertStyle = .informational

        alert.addButton(withTitle: L("common.append"))
        alert.addButton(withTitle: L("common.replace"))
        alert.addButton(withTitle: L("common.cancel"))

        let response = alert.runModal()

        switch response {
        case .alertFirstButtonReturn: // Append
            appendImportedRules(importedRules)
        case .alertSecondButtonReturn: // Replace
            replaceAllRules(importedRules)
        default: // Cancel
            break
        }
    }

    private func appendImportedRules(_ importedRules: [RoutingRuleConfig]) {
        Task {
            do {
                var updatedRules = customRules
                updatedRules.append(contentsOf: importedRules)

                try core.updateRoutingRules(rules: updatedRules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    customRules = config.rules.filter { !$0.isPreset }

                    // Show success message
                    showInfoToast(
                        title: L("alert.routing.import_success_append"),
                        message: String(format: L("alert.routing.import_success_append_message"), importedRules.count)
                    )
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to import rules: \(error.localizedDescription)"
                }
            }
        }
    }

    private func replaceAllRules(_ importedRules: [RoutingRuleConfig]) {
        Task {
            do {
                try core.updateRoutingRules(rules: importedRules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    customRules = config.rules.filter { !$0.isPreset }

                    // Show success message
                    showInfoToast(
                        title: L("alert.routing.import_success_replace"),
                        message: String(format: L("alert.routing.import_success_replace_message"), importedRules.count)
                    )
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to import rules: \(error.localizedDescription)"
                }
            }
        }
    }
}

// MARK: - Preset Rules Data Model

/// Hardcoded preset rules that are displayed as read-only guides
struct PresetRule {
    let command: String
    let description: String
    let descriptionKey: String
    let isImplemented: Bool
    let icon: String
    let usage: String           // Usage example
    let usageKey: String        // Localization key for usage
    let subcommands: [PresetSubcommand]  // Optional subcommands
}

/// Subcommand definition for preset rules
struct PresetSubcommand {
    let name: String            // e.g., "google", "tavily"
    let description: String
    let descriptionKey: String
    let isImplemented: Bool
}

/// All preset rules
enum PresetRules {
    static let all: [PresetRule] = [
        PresetRule(
            command: "/search",
            description: "Search the web and get AI-summarized results",
            descriptionKey: "settings.routing.preset.search.description",
            isImplemented: true,
            icon: "magnifyingglass",
            usage: "/search <query>",
            usageKey: "settings.routing.preset.search.usage",
            subcommands: []  // Search provider is configured in Settings > Search
        ),
        PresetRule(
            command: "/mcp",
            description: "Invoke MCP tools for extended capabilities",
            descriptionKey: "settings.routing.preset.mcp.description",
            isImplemented: true,
            icon: "puzzlepiece.extension",
            usage: "/mcp <tool> [params]",
            usageKey: "settings.routing.preset.mcp.usage",
            subcommands: [
                PresetSubcommand(
                    name: "list",
                    description: "List available MCP tools",
                    descriptionKey: "settings.routing.preset.mcp.sub.list",
                    isImplemented: true
                ),
                PresetSubcommand(
                    name: "<tool_name>",
                    description: "Execute specific MCP tool",
                    descriptionKey: "settings.routing.preset.mcp.sub.tool",
                    isImplemented: true
                )
            ]
        ),
        PresetRule(
            command: "/skill",
            description: "Execute predefined skill workflows",
            descriptionKey: "settings.routing.preset.skills.description",
            isImplemented: true,
            icon: "wand.and.stars",
            usage: "/skill <name>",
            usageKey: "settings.routing.preset.skills.usage",
            subcommands: [
                PresetSubcommand(
                    name: "list",
                    description: "List available skills",
                    descriptionKey: "settings.routing.preset.skills.sub.list",
                    isImplemented: true
                ),
                PresetSubcommand(
                    name: "<skill_name>",
                    description: "Execute specific skill",
                    descriptionKey: "settings.routing.preset.skills.sub.skill",
                    isImplemented: true
                )
            ]
        ),
        PresetRule(
            command: "/video",
            description: "Analyze YouTube video content via transcript extraction",
            descriptionKey: "settings.routing.preset.video.description",
            isImplemented: true,
            icon: "play.rectangle",
            usage: "/video <YouTube URL>",
            usageKey: "settings.routing.preset.video.usage",
            subcommands: []  // Auto-detects YouTube URLs in input
        ),
        PresetRule(
            command: "/chat",
            description: "Start a multi-turn conversation session",
            descriptionKey: "settings.routing.preset.chat.description",
            isImplemented: true,
            icon: "bubble.left.and.bubble.right",
            usage: "/chat <message>",
            usageKey: "settings.routing.preset.chat.usage",
            subcommands: []  // Multi-turn mode can also be enabled by default in Behavior settings
        )
    ]
}

// MARK: - Backward Compatibility Alias
typealias PresetCommand = PresetRule
enum PresetCommands {
    static var all: [PresetRule] { PresetRules.all }
}

// MARK: - Preset Rule Card Component

/// Card component for displaying a preset rule (read-only) - compact view with detail popup
struct PresetCommandCard: View {
    let preset: PresetRule
    @State private var showingDetail: Bool = false

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Command name (title)
            Text(preset.command)
                .font(DesignTokens.Typography.code)
                .fontWeight(.semibold)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            // Description
            Text(L(preset.descriptionKey))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .lineLimit(1)

            Spacer()

            // View button
            Button(action: { showingDetail = true }) {
                Text(L("common.view"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.accentPurple)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, DesignTokens.Spacing.md)
        .padding(.vertical, DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.cardBackground.opacity(0.5))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .sheet(isPresented: $showingDetail) {
            PresetRuleDetailView(preset: preset)
        }
    }
}

// MARK: - Preset Rule Detail View (Popup)

/// Detail view for preset rule shown in a popup sheet
struct PresetRuleDetailView: View {
    let preset: PresetRule
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
            // Header with close button
            HStack {
                // Icon + Title
                HStack(spacing: DesignTokens.Spacing.md) {
                    ZStack {
                        Circle()
                            .fill(DesignTokens.Colors.accentPurple.opacity(0.2))
                            .frame(width: 40, height: 40)

                        Image(systemName: preset.icon)
                            .font(.system(size: 18))
                            .foregroundColor(DesignTokens.Colors.accentPurple)
                    }

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                        HStack(spacing: DesignTokens.Spacing.sm) {
                            Text(preset.command)
                                .font(DesignTokens.Typography.heading)
                                .fontWeight(.semibold)
                                .foregroundColor(DesignTokens.Colors.textPrimary)

                            // Status badge
                            if preset.isImplemented {
                                HStack(spacing: 2) {
                                    Image(systemName: "checkmark.circle.fill")
                                        .font(.system(size: 8))
                                    Text(L("settings.routing.implemented"))
                                        .font(.system(size: 10))
                                }
                                .foregroundColor(.white)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(DesignTokens.Colors.success)
                                .cornerRadius(DesignTokens.CornerRadius.small)
                            } else {
                                HStack(spacing: 2) {
                                    Image(systemName: "clock.fill")
                                        .font(.system(size: 8))
                                    Text(L("settings.routing.coming_soon"))
                                        .font(.system(size: 10))
                                }
                                .foregroundColor(.white)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(DesignTokens.Colors.textSecondary)
                                .cornerRadius(DesignTokens.CornerRadius.small)
                            }
                        }

                        // Lock indicator
                        HStack(spacing: DesignTokens.Spacing.xs) {
                            Image(systemName: "lock.fill")
                                .font(.system(size: 10))
                            Text(L("settings.routing.preset_badge"))
                                .font(DesignTokens.Typography.caption)
                        }
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }

                Spacer()

                // Close button
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 20))
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .buttonStyle(.plain)
            }

            Divider()

            // Description
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text(L(preset.descriptionKey))
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
            }

            // Usage
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text(L("settings.routing.usage"))
                    .font(DesignTokens.Typography.caption)
                    .fontWeight(.medium)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                Text(preset.usage)
                    .font(DesignTokens.Typography.code)
                    .foregroundColor(DesignTokens.Colors.accentBlue)
                    .padding(DesignTokens.Spacing.sm)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(DesignTokens.Colors.cardBackground)
                    .cornerRadius(DesignTokens.CornerRadius.small)
            }

            // Subcommands (if any)
            if !preset.subcommands.isEmpty {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    Text(L("settings.routing.subcommands"))
                        .font(DesignTokens.Typography.caption)
                        .fontWeight(.medium)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    ForEach(preset.subcommands, id: \.name) { subcommand in
                        HStack(spacing: DesignTokens.Spacing.sm) {
                            // Subcommand name
                            Text(subcommand.name)
                                .font(DesignTokens.Typography.code)
                                .foregroundColor(DesignTokens.Colors.textPrimary)

                            // Status indicator
                            if subcommand.isImplemented {
                                Circle()
                                    .fill(DesignTokens.Colors.success)
                                    .frame(width: 6, height: 6)
                            } else {
                                Circle()
                                    .fill(DesignTokens.Colors.textSecondary.opacity(0.5))
                                    .frame(width: 6, height: 6)
                            }

                            // Description
                            Text(L(subcommand.descriptionKey))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)

                            Spacer()
                        }
                        .padding(DesignTokens.Spacing.sm)
                        .background(DesignTokens.Colors.cardBackground.opacity(0.5))
                        .cornerRadius(DesignTokens.CornerRadius.small)
                    }
                }
            }

            Spacer()
        }
        .padding(DesignTokens.Spacing.lg)
        .frame(width: 400, height: 350)
        .background(DesignTokens.Colors.contentBackground)
    }
}

// MARK: - Rule Card Component

/// Card component for displaying a routing rule with modern styling
/// Supports both command rules (with provider) and keyword rules (prompt only)
struct RuleCard: View {
    let rule: RoutingRuleConfig
    let index: Int
    let provider: ProviderConfigEntry?
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var isHovering = false

    /// Rule type badge color
    private var ruleTypeColor: Color {
        rule.isCommandRule ? DesignTokens.Colors.accentBlue : DesignTokens.Colors.success
    }

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Priority badge with rule type indicator
            ZStack {
                Circle()
                    .fill(ruleTypeColor.opacity(0.2))
                    .frame(width: 32, height: 32)

                Text("#\(index + 1)")
                    .font(DesignTokens.Typography.caption)
                    .fontWeight(.semibold)
                    .foregroundColor(ruleTypeColor)
            }

            // Rule details
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                // Rule type badge + Pattern
                HStack(spacing: DesignTokens.Spacing.sm) {
                    // Rule type badge
                    HStack(spacing: 3) {
                        Image(systemName: rule.ruleTypeIcon)
                            .font(.system(size: 9))
                        Text(rule.ruleTypeDisplayName)
                            .font(.system(size: 10, weight: .medium))
                    }
                    .foregroundColor(.white)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(ruleTypeColor)
                    .cornerRadius(DesignTokens.CornerRadius.small)

                    // Display name (user-friendly)
                    Text(rule.displayName)
                        .font(DesignTokens.Typography.code)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                }

                // Provider (only for command rules)
                if rule.isCommandRule {
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Text(L("settings.routing.provider"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        if let provider = provider {
                            HStack(spacing: DesignTokens.Spacing.xs) {
                                Circle()
                                    .fill(Color(hex: provider.config.color) ?? .gray)
                                    .frame(width: 8, height: 8)
                                Text(provider.name)
                                    .font(DesignTokens.Typography.body)
                                    .foregroundColor(DesignTokens.Colors.textPrimary)
                            }
                        } else {
                            Text(rule.provider ?? L("settings.routing.no_provider"))
                                .font(DesignTokens.Typography.body)
                                .foregroundColor(DesignTokens.Colors.warning)
                            Text(L("settings.routing.not_configured"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.warning)
                        }
                    }
                } else {
                    // Keyword rule: show all-match hint
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Image(systemName: "arrow.triangle.merge")
                            .font(.system(size: 10))
                            .foregroundColor(DesignTokens.Colors.success)
                        Text(L("settings.routing.keyword_hint"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.success)
                    }
                }

                // System prompt preview (if exists)
                if let prompt = rule.systemPrompt, !prompt.isEmpty {
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Text(L("settings.routing.prompt"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                        Text(prompt.prefix(50) + (prompt.count > 50 ? "..." : ""))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .lineLimit(1)
                    }
                }
            }

            Spacer()

            // Action buttons
            HStack(spacing: DesignTokens.Spacing.sm) {
                Button(action: onEdit) {
                    Image(systemName: "pencil")
                        .foregroundColor(DesignTokens.Colors.accentBlue)
                        .font(DesignTokens.Typography.body)
                }
                .buttonStyle(.plain)
                .help(L("settings.routing.edit_rule_help"))

                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .foregroundColor(DesignTokens.Colors.error)
                        .font(DesignTokens.Typography.body)
                }
                .buttonStyle(.plain)
                .help(L("settings.routing.delete_rule_help"))
            }
        }
        .padding(.horizontal, DesignTokens.Spacing.md)
        .padding(.vertical, DesignTokens.Spacing.sm)
        .frame(height: 70)  // Fixed height for consistent layout
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
        .scaleEffect(isHovering ? 1.02 : 1.0)
        .animation(DesignTokens.Animation.quick, value: isHovering)
        .onHover { hovering in
            isHovering = hovering
        }
    }
}

// MARK: - Color Extension


