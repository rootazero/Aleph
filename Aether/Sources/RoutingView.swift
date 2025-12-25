//
//  RoutingView.swift
//  Aether
//
//  Routing rules configuration tab
//

import SwiftUI

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
        VStack(alignment: .leading, spacing: 16) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Routing Rules")
                        .font(.title2)

                    Text("Define how clipboard content is routed to AI providers based on patterns.")
                        .foregroundColor(.secondary)
                        .font(.callout)
                }

                Spacer()

                // Import/Export menu
                Menu {
                    Button(action: exportRules) {
                        Label("Export Rules", systemImage: "square.and.arrow.up")
                    }
                    .disabled(rules.isEmpty)

                    Button(action: importRules) {
                        Label("Import Rules", systemImage: "square.and.arrow.down")
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .imageScale(.large)
                }
                .buttonStyle(.plain)
                .help("Import/Export Rules")

                // Add Rule button
                Button(action: addNewRule) {
                    HStack(spacing: 4) {
                        Image(systemName: "plus.circle.fill")
                        Text("Add Rule")
                    }
                }
                .buttonStyle(.borderedProminent)
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

            // Rules List
            if isLoading {
                ProgressView("Loading rules...")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if rules.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "square.stack.3d.up.slash")
                        .font(.system(size: 48))
                        .foregroundColor(.secondary)

                    Text("No routing rules configured")
                        .font(.headline)
                        .foregroundColor(.secondary)

                    Text("Add your first rule to start routing clipboard content")
                        .font(.callout)
                        .foregroundColor(.secondary)

                    Button(action: addNewRule) {
                        HStack(spacing: 4) {
                            Image(systemName: "plus.circle.fill")
                            Text("Add Rule")
                        }
                    }
                    .buttonStyle(.bordered)
                    .padding(.top, 8)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(Array(rules.enumerated()), id: \.offset) { index, rule in
                        RuleRow(
                            rule: rule,
                            index: index,
                            provider: providers.first(where: { $0.name == rule.provider }),
                            onEdit: { editRule(at: index) },
                            onDelete: { confirmDelete(at: index) }
                        )
                    }
                    .onMove { source, destination in
                        moveRule(from: source, to: destination)
                    }
                }
                .listStyle(.inset)
            }

            // Footer info
            if !rules.isEmpty {
                HStack {
                    Image(systemName: "info.circle")
                        .foregroundColor(.secondary)
                    Text("Rules are evaluated top-to-bottom. Drag to reorder priority.")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(20)
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
        .alert("Delete Rule", isPresented: $showingDeleteConfirmation) {
            Button("Cancel", role: .cancel) {}
            Button("Delete", role: .destructive) {
                if let index = deletingRuleIndex {
                    deleteRule(at: index)
                }
            }
        } message: {
            if let index = deletingRuleIndex {
                Text("Are you sure you want to delete the rule with pattern '\(rules[index].regex)'?")
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

// MARK: - Rule Row Component

struct RuleRow: View {
    let rule: RoutingRuleConfig
    let index: Int
    let provider: ProviderConfigEntry?
    let onEdit: () -> Void
    let onDelete: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            // Priority number
            Text("#\(index + 1)")
                .font(.caption)
                .foregroundColor(.secondary)
                .frame(width: 30, alignment: .leading)

            // Rule details
            VStack(alignment: .leading, spacing: 6) {
                // Pattern
                HStack(spacing: 8) {
                    Text("Pattern:")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text(rule.regex)
                        .font(.system(.body, design: .monospaced))
                }

                // Provider
                HStack(spacing: 8) {
                    Text("Provider:")
                        .font(.caption)
                        .foregroundColor(.secondary)

                    if let provider = provider {
                        HStack(spacing: 4) {
                            Circle()
                                .fill(Color(hex: provider.config.color) ?? .gray)
                                .frame(width: 8, height: 8)
                            Text(provider.name)
                                .font(.body)
                        }
                    } else {
                        Text(rule.provider)
                            .font(.body)
                            .foregroundColor(.orange)
                        Text("(not configured)")
                            .font(.caption)
                            .foregroundColor(.orange)
                    }
                }

                // System prompt preview (if exists)
                if let prompt = rule.systemPrompt, !prompt.isEmpty {
                    HStack(spacing: 8) {
                        Text("Prompt:")
                            .font(.caption)
                            .foregroundColor(.secondary)
                        Text(prompt.prefix(50) + (prompt.count > 50 ? "..." : ""))
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }
                }
            }

            Spacer()

            // Action buttons
            HStack(spacing: 8) {
                Button(action: onEdit) {
                    Image(systemName: "pencil")
                        .foregroundColor(.blue)
                }
                .buttonStyle(.plain)
                .help("Edit rule")

                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
                .help("Delete rule")
            }
        }
        .padding(.vertical, 8)
    }
}

// MARK: - Color Extension

extension Color {
    /// Initialize Color from hex string
    init?(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)

        let r, g, b: UInt64
        switch hex.count {
        case 6: // RGB (24-bit)
            (r, g, b) = ((int >> 16) & 0xFF, (int >> 8) & 0xFF, int & 0xFF)
        default:
            return nil
        }

        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue: Double(b) / 255,
            opacity: 1
        )
    }
}
