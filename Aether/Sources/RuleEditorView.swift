//
//  RuleEditorView.swift
//  Aether
//
//  Modal dialog for adding/editing routing rules
//  Simplified UI: users input command name or keyword, regex is auto-generated
//

import SwiftUI

/// Rule type enum for UI selection
enum RuleType: String, CaseIterable {
    case command = "command"
    case keyword = "keyword"

    var displayName: String {
        switch self {
        case .command: return L("settings.routing.type.command")
        case .keyword: return L("settings.routing.type.keyword")
        }
    }

    var icon: String {
        switch self {
        case .command: return "command"
        case .keyword: return "text.magnifyingglass"
        }
    }

    var color: Color {
        switch self {
        case .command: return .blue
        case .keyword: return .green
        }
    }
}

/// Modal dialog for configuring routing rules
struct RuleEditorView: View {
    @Environment(\.dismiss) private var dismiss
    @Binding var rules: [RoutingRuleConfig]

    // Core reference for validation and saving
    let core: AetherCore
    let providers: [ProviderConfigEntry]

    // Edit mode: nil for new rule, index for editing
    let editingIndex: Int?

    // Form state - simplified
    @State private var ruleType: RuleType = .command
    @State private var commandName: String = ""      // For command rules: just the name (e.g., "draw")
    @State private var keyword: String = ""          // For keyword rules: just the keyword (e.g., "urgent")
    @State private var selectedProvider: String = ""
    @State private var systemPrompt: String = ""

    // UI state
    @State private var isSaving: Bool = false
    @State private var errorMessage: String?

    // Generated regex pattern (computed)
    private var generatedPattern: String {
        switch ruleType {
        case .command:
            let name = commandName.trimmingCharacters(in: .whitespaces)
            guard !name.isEmpty else { return "" }
            // Generate: ^/commandName\s+ (matches /commandName followed by space and content)
            return "^/\(name)\\s+"
        case .keyword:
            let kw = keyword.trimmingCharacters(in: .whitespaces)
            guard !kw.isEmpty else { return "" }
            // Generate: .*keyword.* (matches anything containing the keyword)
            // Escape special regex characters in keyword
            let escaped = NSRegularExpression.escapedPattern(for: kw)
            return ".*\(escaped).*"
        }
    }

    // Initial rule type for new rules
    private let initialRuleType: RuleType?

    // The rule being edited (captured at init time for stability)
    private let editingRule: RoutingRuleConfig?

    // Initialize for new rule with optional initial type
    init(rules: Binding<[RoutingRuleConfig]>, core: AetherCore, providers: [ProviderConfigEntry], initialType: RuleType? = nil) {
        self._rules = rules
        self.core = core
        self.providers = providers
        self.editingIndex = nil
        self.editingRule = nil
        self.initialRuleType = initialType
    }

    // Initialize for editing existing rule (with rule object for stability)
    init(rules: Binding<[RoutingRuleConfig]>, core: AetherCore, providers: [ProviderConfigEntry], editingRule: RoutingRuleConfig, editingIndex: Int) {
        self._rules = rules
        self.core = core
        self.providers = providers
        self.editingIndex = editingIndex
        self.editingRule = editingRule
        self.initialRuleType = nil
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            headerSection

            Divider()

            // Form content
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    // Rule Type Selector
                    ruleTypeSelector

                    // Input section (different for command vs keyword)
                    if ruleType == .command {
                        commandInputSection
                    } else {
                        keywordInputSection
                    }

                    // Provider Selection (only for command rules)
                    if ruleType == .command {
                        providerSection
                    }

                    // System Prompt
                    systemPromptSection

                    // Preview section
                    previewSection

                    // Error message
                    if let error = errorMessage {
                        errorSection(error)
                    }
                }
                .padding(20)
            }

            Divider()

            // Footer buttons
            footerSection
        }
        .frame(width: 550, height: 580)
        .onAppear {
            loadFormData()
        }
    }

    // MARK: - Header Section

    private var headerSection: some View {
        HStack {
            Text(editingIndex == nil
                 ? L("settings.routing.add_rule")
                 : L("settings.routing.edit_rule"))
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
    }

    // MARK: - Rule Type Selector

    private var ruleTypeSelector: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L("settings.routing.editor.rule_type"))
                .font(.headline)

            // Custom segmented button style
            HStack(spacing: 0) {
                ForEach(RuleType.allCases, id: \.self) { type in
                    HStack(spacing: 6) {
                        Image(systemName: type.icon)
                            .font(.system(size: 12, weight: .medium))
                        Text(type.displayName)
                            .font(.system(size: 13, weight: .medium))
                    }
                    .foregroundColor(ruleType == type ? .white : .primary)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity)
                    .background(
                        RoundedRectangle(cornerRadius: 6)
                            .fill(ruleType == type ? type.color : Color.clear)
                    )
                    .contentShape(Rectangle())
                    .onTapGesture {
                        if ruleType != type {
                            ruleType = type
                            // Clear input when switching types
                            commandName = ""
                            keyword = ""
                            errorMessage = nil
                        }
                    }
                }
            }
            .padding(3)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(Color(NSColor.controlBackgroundColor))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color.gray.opacity(0.2), lineWidth: 1)
            )

            // Description based on type
            HStack(spacing: 6) {
                Image(systemName: "info.circle")
                    .foregroundColor(ruleType.color)
                Text(ruleType == .command
                    ? L("settings.routing.editor.command_description")
                    : L("settings.routing.editor.keyword_description"))
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
            .padding(.top, 4)
        }
    }

    // MARK: - Command Input Section

    private var commandInputSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L("settings.routing.editor.command_name"))
                .font(.headline)

            HStack(spacing: 0) {
                // Fixed "/" prefix
                Text("/")
                    .font(.system(.body, design: .monospaced))
                    .fontWeight(.semibold)
                    .foregroundColor(.blue)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(Color.blue.opacity(0.1))
                    .cornerRadius(6, corners: [.topLeft, .bottomLeft])

                // Command name input
                TextField(L("settings.routing.editor.command_placeholder"), text: $commandName)
                    .textFieldStyle(.plain)
                    .font(.system(.body, design: .monospaced))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(Color(NSColor.controlBackgroundColor))
                    .cornerRadius(6, corners: [.topRight, .bottomRight])
            }
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(Color.gray.opacity(0.3), lineWidth: 1)
            )

            Text(L("settings.routing.editor.command_help"))
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Keyword Input Section

    private var keywordInputSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L("settings.routing.editor.keyword_name"))
                .font(.headline)

            TextField(L("settings.routing.editor.keyword_placeholder"), text: $keyword)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))

            Text(L("settings.routing.editor.keyword_help"))
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Provider Section

    private var providerSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L("settings.routing.provider").replacingOccurrences(of: ":", with: ""))
                .font(.headline)

            Picker("", selection: $selectedProvider) {
                ForEach(providers, id: \.name) { provider in
                    HStack(spacing: 6) {
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
                HStack(spacing: 6) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(.orange)
                    Text(L("settings.routing.editor.no_providers"))
                        .font(.caption)
                        .foregroundColor(.orange)
                }
            }
        }
    }

    // MARK: - System Prompt Section

    private var systemPromptSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(L("settings.routing.prompt").replacingOccurrences(of: ":", with: ""))
                    .font(.headline)

                Text(ruleType == .command ? "(\(L("common.optional")))" : "(\(L("common.required")))")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            TextEditor(text: $systemPrompt)
                .font(.system(.body, design: .monospaced))
                .frame(minHeight: 80, maxHeight: 150)
                .padding(4)
                .background(Color(NSColor.controlBackgroundColor))
                .cornerRadius(6)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(Color.gray.opacity(0.3), lineWidth: 1)
                )

            Text(ruleType == .command
                ? L("settings.routing.editor.prompt_help_command")
                : L("settings.routing.editor.prompt_help_keyword"))
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Preview Section

    private var previewSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L("settings.routing.editor.preview"))
                .font(.headline)

            VStack(alignment: .leading, spacing: 6) {
                // Generated pattern
                HStack(spacing: 8) {
                    Text(L("settings.routing.editor.generated_pattern"))
                        .font(.caption)
                        .foregroundColor(.secondary)

                    if !generatedPattern.isEmpty {
                        Text(generatedPattern)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundColor(ruleType.color)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(ruleType.color.opacity(0.1))
                            .cornerRadius(4)
                    } else {
                        Text("—")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }

                // Example match
                if !generatedPattern.isEmpty {
                    HStack(spacing: 8) {
                        Text(L("settings.routing.editor.example_match"))
                            .font(.caption)
                            .foregroundColor(.secondary)

                        Text(exampleMatchText)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundColor(.primary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Color.gray.opacity(0.1))
                            .cornerRadius(4)
                    }
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(NSColor.controlBackgroundColor).opacity(0.5))
            .cornerRadius(8)
        }
    }

    private var exampleMatchText: String {
        switch ruleType {
        case .command:
            let name = commandName.trimmingCharacters(in: .whitespaces)
            return name.isEmpty ? "—" : "/\(name) Hello world"
        case .keyword:
            let kw = keyword.trimmingCharacters(in: .whitespaces)
            return kw.isEmpty ? "—" : "Text with \(kw) in it"
        }
    }

    // MARK: - Error Section

    private func errorSection(_ error: String) -> some View {
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

    // MARK: - Footer Section

    private var footerSection: some View {
        HStack(spacing: 12) {
            Spacer()

            Button(L("common.cancel")) {
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
                    Text(isSaving ? L("common.saving") : L("common.save"))
                }
            }
            .keyboardShortcut(.return)
            .buttonStyle(.borderedProminent)
            .disabled(isSaving || !isFormValid())
        }
        .padding(20)
    }

    // MARK: - Data Loading

    private func loadFormData() {
        if let rule = editingRule {
            // Load existing rule data (use captured rule for stability)
            // Detect rule type and extract name/keyword from pattern
            if rule.isCommandRule {
                ruleType = .command
                commandName = extractCommandName(from: rule.regex)
            } else {
                ruleType = .keyword
                keyword = extractKeyword(from: rule.regex)
            }

            selectedProvider = rule.provider ?? ""
            systemPrompt = rule.systemPrompt ?? ""
        } else {
            // New rule: set defaults (use initialRuleType if provided)
            ruleType = initialRuleType ?? .command
            commandName = ""
            keyword = ""
            systemPrompt = ""
            if let firstProvider = providers.first {
                selectedProvider = firstProvider.name
            } else {
                selectedProvider = ""
            }
        }

        errorMessage = nil
        isSaving = false
    }

    /// Extract command name from regex pattern (e.g., "^/draw\\s+" → "draw")
    private func extractCommandName(from regex: String) -> String {
        // Pattern: ^/name\s+ or ^/name
        if regex.hasPrefix("^/") {
            var name = String(regex.dropFirst(2))
            // Remove trailing \s+ if present
            if name.hasSuffix("\\s+") {
                name = String(name.dropLast(4))
            }
            return name
        }
        return regex
    }

    /// Extract keyword from regex pattern (e.g., ".*urgent.*" → "urgent")
    private func extractKeyword(from regex: String) -> String {
        // Pattern: .*keyword.*
        if regex.hasPrefix(".*") && regex.hasSuffix(".*") {
            let inner = regex.dropFirst(2).dropLast(2)
            // Unescape if needed
            return String(inner).replacingOccurrences(of: "\\", with: "")
        }
        return regex
    }

    // MARK: - Validation

    private func isFormValid() -> Bool {
        switch ruleType {
        case .command:
            // Command name required
            guard !commandName.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
            // Provider required
            guard !selectedProvider.isEmpty else { return false }
            return true

        case .keyword:
            // Keyword required
            guard !keyword.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
            // System prompt required for keyword rules
            guard !systemPrompt.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
            return true
        }
    }

    // MARK: - Actions

    private func saveRule() {
        guard isFormValid() else { return }

        isSaving = true
        errorMessage = nil

        Task {
            do {
                // Create new rule config with generated pattern
                let newRule = RoutingRuleConfig(
                    ruleType: ruleType.rawValue,
                    isBuiltin: false,
                    regex: generatedPattern,
                    provider: ruleType == .command ? selectedProvider : nil,
                    systemPrompt: systemPrompt.isEmpty ? nil : systemPrompt,
                    stripPrefix: nil,
                    capabilities: nil,
                    intentType: nil,
                    contextFormat: nil,
                    skillId: nil,
                    skillVersion: nil,
                    workflow: nil,
                    tools: nil,
                    knowledgeBase: nil
                )

                // Update rules array
                var updatedRules = rules
                if let index = editingIndex {
                    updatedRules[index] = newRule
                } else {
                    // Insert new rule at the correct position based on type
                    // Command rules go to the front of command section (index 0)
                    // Keyword rules go to the front of keyword section (after all command rules)
                    if ruleType == .command {
                        // Insert at the very beginning
                        updatedRules.insert(newRule, at: 0)
                    } else {
                        // Find the first keyword rule position (after all command rules)
                        let firstKeywordIndex = updatedRules.firstIndex(where: { $0.isKeywordRule }) ?? updatedRules.count
                        updatedRules.insert(newRule, at: firstKeywordIndex)
                    }
                }

                // Save via Rust core
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

// MARK: - Corner Radius Extension

extension View {
    func cornerRadius(_ radius: CGFloat, corners: UIRectCorner) -> some View {
        clipShape(RoundedCorner(radius: radius, corners: corners))
    }
}

struct RoundedCorner: Shape {
    var radius: CGFloat = .infinity
    var corners: UIRectCorner = .allCorners

    func path(in rect: CGRect) -> Path {
        let path = NSBezierPath(
            roundedRect: rect,
            byRoundingCorners: corners,
            cornerRadii: CGSize(width: radius, height: radius)
        )
        return Path(path.cgPath)
    }
}

// UIRectCorner equivalent for macOS
struct UIRectCorner: OptionSet {
    let rawValue: Int

    static let topLeft = UIRectCorner(rawValue: 1 << 0)
    static let topRight = UIRectCorner(rawValue: 1 << 1)
    static let bottomLeft = UIRectCorner(rawValue: 1 << 2)
    static let bottomRight = UIRectCorner(rawValue: 1 << 3)
    static let allCorners: UIRectCorner = [.topLeft, .topRight, .bottomLeft, .bottomRight]
}

extension NSBezierPath {
    convenience init(roundedRect rect: CGRect, byRoundingCorners corners: UIRectCorner, cornerRadii: CGSize) {
        self.init()

        let topLeft = corners.contains(.topLeft) ? cornerRadii.width : 0
        let topRight = corners.contains(.topRight) ? cornerRadii.width : 0
        let bottomLeft = corners.contains(.bottomLeft) ? cornerRadii.width : 0
        let bottomRight = corners.contains(.bottomRight) ? cornerRadii.width : 0

        move(to: CGPoint(x: rect.minX + topLeft, y: rect.minY))

        // Top edge
        line(to: CGPoint(x: rect.maxX - topRight, y: rect.minY))
        if topRight > 0 {
            appendArc(withCenter: CGPoint(x: rect.maxX - topRight, y: rect.minY + topRight),
                     radius: topRight, startAngle: -90, endAngle: 0, clockwise: false)
        }

        // Right edge
        line(to: CGPoint(x: rect.maxX, y: rect.maxY - bottomRight))
        if bottomRight > 0 {
            appendArc(withCenter: CGPoint(x: rect.maxX - bottomRight, y: rect.maxY - bottomRight),
                     radius: bottomRight, startAngle: 0, endAngle: 90, clockwise: false)
        }

        // Bottom edge
        line(to: CGPoint(x: rect.minX + bottomLeft, y: rect.maxY))
        if bottomLeft > 0 {
            appendArc(withCenter: CGPoint(x: rect.minX + bottomLeft, y: rect.maxY - bottomLeft),
                     radius: bottomLeft, startAngle: 90, endAngle: 180, clockwise: false)
        }

        // Left edge
        line(to: CGPoint(x: rect.minX, y: rect.minY + topLeft))
        if topLeft > 0 {
            appendArc(withCenter: CGPoint(x: rect.minX + topLeft, y: rect.minY + topLeft),
                     radius: topLeft, startAngle: 180, endAngle: 270, clockwise: false)
        }

        close()
    }

    var cgPath: CGPath {
        let path = CGMutablePath()
        var points = [CGPoint](repeating: .zero, count: 3)

        for i in 0..<elementCount {
            let type = element(at: i, associatedPoints: &points)
            switch type {
            case .moveTo:
                path.move(to: points[0])
            case .lineTo:
                path.addLine(to: points[0])
            case .curveTo:
                path.addCurve(to: points[2], control1: points[0], control2: points[1])
            case .closePath:
                path.closeSubpath()
            @unknown default:
                break
            }
        }

        return path
    }
}
