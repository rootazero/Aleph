//
//  RoutingView.swift
//  Aether
//
//  Routing rules configuration tab
//

import SwiftUI
import UniformTypeIdentifiers

struct RoutingView: View {
    let core: AetherCore
    let providers: [ProviderConfigEntry]

    // Rules state
    @State private var rules: [RoutingRuleConfig] = []
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?

    // UI state
    @State private var showingRuleEditor: Bool = false
    @State private var editingRuleIndex: Int?
    @State private var showingDeleteConfirmation: Bool = false
    @State private var deletingRuleIndex: Int?
    @State private var showingImportExport: Bool = false

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text(LocalizedStringKey("settings.routing.title"))
                        .font(DesignTokens.Typography.title)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Text(LocalizedStringKey("settings.routing.description"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                Spacer()

                // Import/Export menu
                Menu {
                    Button(action: exportRules) {
                        Label(LocalizedStringKey("settings.routing.export_rules"), systemImage: "square.and.arrow.up")
                    }
                    .disabled(rules.isEmpty)

                    Button(action: importRules) {
                        Label(LocalizedStringKey("settings.routing.import_rules"), systemImage: "square.and.arrow.down")
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .imageScale(.large)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                }
                .buttonStyle(.plain)
                .help(LocalizedStringKey("settings.routing.import_export_help"))

                // Add Rule button
                ActionButton(LocalizedStringKey("settings.routing.add_rule"), icon: "plus.circle.fill", style: .primary) {
                    addNewRule()
                }
            }

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

            // Rules List
            if isLoading {
                ProgressView(LocalizedStringKey("settings.routing.loading"))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if rules.isEmpty {
                VStack(spacing: DesignTokens.Spacing.md) {
                    Image(systemName: "square.stack.3d.up.slash")
                        .font(.system(size: 48))
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    Text(LocalizedStringKey("settings.routing.no_rules"))
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    Text(LocalizedStringKey("settings.routing.no_rules_message"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    ActionButton(LocalizedStringKey("settings.routing.add_rule"), icon: "plus.circle.fill", style: .secondary) {
                        addNewRule()
                    }
                    .padding(.top, DesignTokens.Spacing.sm)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(rules.enumerated()), id: \.offset) { index, rule in
                        RuleCard(
                            rule: rule,
                            index: index,
                            provider: providers.first(where: { $0.name == rule.provider }),
                            onEdit: { editRule(at: index) },
                            onDelete: { confirmDelete(at: index) }
                        )
                        .padding(.vertical, DesignTokens.Spacing.xs)
                    }
                }
            }

            // Footer info
            if !rules.isEmpty {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Image(systemName: "info.circle")
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Text(LocalizedStringKey("settings.routing.footer_info"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(DesignTokens.Spacing.lg)
        .onAppear {
            loadRules()
        }
        .sheet(isPresented: $showingRuleEditor) {
            if let index = editingRuleIndex {
                RuleEditorView(rules: $rules, core: core, providers: providers, editing: index)
            } else {
                RuleEditorView(rules: $rules, core: core, providers: providers)
            }
        }
        .alert(NSLocalizedString("settings.routing.delete_rule", comment: ""), isPresented: $showingDeleteConfirmation) {
            Button(NSLocalizedString("common.cancel", comment: ""), role: .cancel) {}
            Button(NSLocalizedString("common.delete", comment: ""), role: .destructive) {
                if let index = deletingRuleIndex {
                    deleteRule(at: index)
                }
            }
        } message: {
            if let index = deletingRuleIndex {
                Text(String(format: NSLocalizedString("settings.routing.delete_rule_message", comment: ""), rules[index].regex))
            }
        }
    }

    // MARK: - Data Loading

    private func loadRules() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let config = try core.loadConfig()

                await MainActor.run {
                    rules = config.rules
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
        editingRuleIndex = nil
        showingRuleEditor = true
    }

    private func editRule(at index: Int) {
        editingRuleIndex = index
        showingRuleEditor = true
    }

    private func confirmDelete(at index: Int) {
        deletingRuleIndex = index
        showingDeleteConfirmation = true
    }

    private func deleteRule(at index: Int) {
        Task {
            do {
                var updatedRules = rules
                updatedRules.remove(at: index)

                try core.updateRoutingRules(rules: updatedRules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    rules = config.rules
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to delete rule: \(error.localizedDescription)"
                }
            }
        }
    }

    private func moveRule(from source: IndexSet, to destination: Int) {
        var updatedRules = rules
        updatedRules.move(fromOffsets: source, toOffset: destination)

        Task {
            do {
                try core.updateRoutingRules(rules: updatedRules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    rules = config.rules
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to reorder rules: \(error.localizedDescription)"
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
                // Convert rules to JSON
                let encoder = JSONEncoder()
                encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
                let jsonData = try encoder.encode(rules)

                // Write to file
                try jsonData.write(to: url)

                // Show success notification
                DispatchQueue.main.async {
                    let alert = NSAlert()
                    alert.messageText = "Export Successful"
                    alert.informativeText = "Routing rules exported to \(url.lastPathComponent)"
                    alert.alertStyle = .informational
                    alert.addButton(withTitle: "OK")
                    alert.runModal()
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
        alert.messageText = "Import Routing Rules"
        alert.informativeText = "Found \(importedRules.count) rule(s). How would you like to import them?"
        alert.alertStyle = .informational

        alert.addButton(withTitle: "Append")
        alert.addButton(withTitle: "Replace All")
        alert.addButton(withTitle: "Cancel")

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
                var updatedRules = rules
                updatedRules.append(contentsOf: importedRules)

                try core.updateRoutingRules(rules: updatedRules)

                // Reload config
                let config = try core.loadConfig()

                await MainActor.run {
                    rules = config.rules

                    // Show success message
                    let alert = NSAlert()
                    alert.messageText = "Import Successful"
                    alert.informativeText = "Added \(importedRules.count) rule(s) to existing rules"
                    alert.alertStyle = .informational
                    alert.addButton(withTitle: "OK")
                    alert.runModal()
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
                    rules = config.rules

                    // Show success message
                    let alert = NSAlert()
                    alert.messageText = "Import Successful"
                    alert.informativeText = "Replaced all rules with \(importedRules.count) imported rule(s)"
                    alert.alertStyle = .informational
                    alert.addButton(withTitle: "OK")
                    alert.runModal()
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to import rules: \(error.localizedDescription)"
                }
            }
        }
    }
}

// MARK: - Rule Card Component

/// Card component for displaying a routing rule with modern styling
struct RuleCard: View {
    let rule: RoutingRuleConfig
    let index: Int
    let provider: ProviderConfigEntry?
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var isHovering = false

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Priority badge
            ZStack {
                Circle()
                    .fill(DesignTokens.Colors.accentBlue.opacity(0.2))
                    .frame(width: 32, height: 32)

                Text("#\(index + 1)")
                    .font(DesignTokens.Typography.caption)
                    .fontWeight(.semibold)
                    .foregroundColor(DesignTokens.Colors.accentBlue)
            }

            // Rule details
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                // Pattern
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Text(LocalizedStringKey("settings.routing.pattern"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                    Text(rule.regex)
                        .font(DesignTokens.Typography.code)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                }

                // Provider
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Text(LocalizedStringKey("settings.routing.provider"))
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
                        Text(rule.provider)
                            .font(DesignTokens.Typography.body)
                            .foregroundColor(DesignTokens.Colors.warning)
                        Text(LocalizedStringKey("settings.routing.not_configured"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.warning)
                    }
                }

                // System prompt preview (if exists)
                if let prompt = rule.systemPrompt, !prompt.isEmpty {
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Text(LocalizedStringKey("settings.routing.prompt"))
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
                .help(LocalizedStringKey("settings.routing.edit_rule_help"))

                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .foregroundColor(DesignTokens.Colors.error)
                        .font(DesignTokens.Typography.body)
                }
                .buttonStyle(.plain)
                .help(LocalizedStringKey("settings.routing.delete_rule_help"))
            }
        }
        .padding(DesignTokens.Spacing.md)
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


