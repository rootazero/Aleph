//
//  RoutingRuleConfigExtension.swift
//  Aether
//
//  Extension to add Codable support for RoutingRuleConfig
//  This allows import/export of routing rules as JSON
//

import Foundation

// MARK: - Codable Extension for RoutingRuleConfig

extension RoutingRuleConfig: Codable {
    enum CodingKeys: String, CodingKey {
        case ruleType = "rule_type"
        case isBuiltin = "is_builtin"
        case regex
        case provider
        case systemPrompt = "system_prompt"
        case stripPrefix = "strip_prefix"
        case capabilities
        case intentType = "intent_type"
        case contextFormat = "context_format"
        case skillId = "skill_id"
        case skillVersion = "skill_version"
        case workflow
        case tools
        case knowledgeBase = "knowledge_base"
        case icon
        case hint
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let ruleType = try container.decodeIfPresent(String.self, forKey: .ruleType)
        let isBuiltin = try container.decodeIfPresent(Bool.self, forKey: .isBuiltin) ?? false
        let regex = try container.decode(String.self, forKey: .regex)
        let provider = try container.decodeIfPresent(String.self, forKey: .provider)
        let systemPrompt = try container.decodeIfPresent(String.self, forKey: .systemPrompt)
        let stripPrefix = try container.decodeIfPresent(Bool.self, forKey: .stripPrefix)
        let capabilities = try container.decodeIfPresent([String].self, forKey: .capabilities)
        let intentType = try container.decodeIfPresent(String.self, forKey: .intentType)
        let contextFormat = try container.decodeIfPresent(String.self, forKey: .contextFormat)
        let skillId = try container.decodeIfPresent(String.self, forKey: .skillId)
        let skillVersion = try container.decodeIfPresent(String.self, forKey: .skillVersion)
        let workflow = try container.decodeIfPresent(String.self, forKey: .workflow)
        let tools = try container.decodeIfPresent(String.self, forKey: .tools)
        let knowledgeBase = try container.decodeIfPresent(String.self, forKey: .knowledgeBase)
        let icon = try container.decodeIfPresent(String.self, forKey: .icon)
        let hint = try container.decodeIfPresent(String.self, forKey: .hint)

        self.init(
            ruleType: ruleType,
            isBuiltin: isBuiltin,
            regex: regex,
            provider: provider,
            systemPrompt: systemPrompt,
            stripPrefix: stripPrefix,
            capabilities: capabilities,
            intentType: intentType,
            contextFormat: contextFormat,
            skillId: skillId,
            skillVersion: skillVersion,
            workflow: workflow,
            tools: tools,
            knowledgeBase: knowledgeBase,
            icon: icon,
            hint: hint
        )
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        if let ruleType = ruleType {
            try container.encode(ruleType, forKey: .ruleType)
        }
        try container.encode(isBuiltin, forKey: .isBuiltin)
        try container.encode(regex, forKey: .regex)
        if let provider = provider {
            try container.encode(provider, forKey: .provider)
        }
        if let systemPrompt = systemPrompt {
            try container.encode(systemPrompt, forKey: .systemPrompt)
        }
        if let stripPrefix = stripPrefix {
            try container.encode(stripPrefix, forKey: .stripPrefix)
        }
        if let capabilities = capabilities {
            try container.encode(capabilities, forKey: .capabilities)
        }
        if let intentType = intentType {
            try container.encode(intentType, forKey: .intentType)
        }
        if let contextFormat = contextFormat {
            try container.encode(contextFormat, forKey: .contextFormat)
        }
        if let skillId = skillId {
            try container.encode(skillId, forKey: .skillId)
        }
        if let skillVersion = skillVersion {
            try container.encode(skillVersion, forKey: .skillVersion)
        }
        if let workflow = workflow {
            try container.encode(workflow, forKey: .workflow)
        }
        if let tools = tools {
            try container.encode(tools, forKey: .tools)
        }
        if let knowledgeBase = knowledgeBase {
            try container.encode(knowledgeBase, forKey: .knowledgeBase)
        }
        if let icon = icon {
            try container.encode(icon, forKey: .icon)
        }
        if let hint = hint {
            try container.encode(hint, forKey: .hint)
        }
    }
}

// MARK: - Rule Type Detection

extension RoutingRuleConfig {
    /// Get the effective rule type
    ///
    /// Priority:
    /// 1. Explicit ruleType if set
    /// 2. Auto-detect from regex pattern: ^/ = command, else = keyword
    var effectiveRuleType: String {
        if let type = ruleType, !type.isEmpty {
            return type
        }
        return regex.hasPrefix("^/") ? "command" : "keyword"
    }

    /// Check if this is a command rule
    ///
    /// Command rules:
    /// - Start with ^/ (e.g., ^/draw, ^/translate)
    /// - First-match-stops semantics
    /// - Require provider selection
    /// - Command prefix is stripped before sending to AI
    var isCommandRule: Bool {
        effectiveRuleType == "command"
    }

    /// Check if this is a keyword rule
    ///
    /// Keyword rules:
    /// - Don't start with ^/ (e.g., .*code.*, urgent)
    /// - All-match semantics (multiple can match)
    /// - Only provide system prompt (no provider)
    /// - Prompts are combined with \n\n separator
    var isKeywordRule: Bool {
        effectiveRuleType == "keyword"
    }

    /// Get localized rule type display name
    var ruleTypeDisplayName: String {
        isCommandRule ? L("settings.routing.type.command") : L("settings.routing.type.keyword")
    }

    /// Get rule type icon
    var ruleTypeIcon: String {
        isCommandRule ? "command" : "text.magnifyingglass"
    }

    /// Get rule type color
    var ruleTypeColor: String {
        isCommandRule ? "#007AFF" : "#34C759"  // Blue for command, Green for keyword
    }

    /// Get user-friendly display name from regex pattern
    ///
    /// For command rules: `^/en\s+` → `/en`
    /// For keyword rules: `.*keyword.*` → `keyword`
    var displayName: String {
        if isCommandRule {
            // Extract command name from pattern like "^/en\s+" or "^/translate\s+"
            if regex.hasPrefix("^/") {
                var name = String(regex.dropFirst(2))  // Remove "^/"
                // Remove trailing \s+ if present (3 chars: \, s, +)
                if name.hasSuffix("\\s+") {
                    name = String(name.dropLast(3))
                }
                // Remove trailing \s* if present (3 chars: \, s, *)
                if name.hasSuffix("\\s*") {
                    name = String(name.dropLast(3))
                }
                return "/\(name)"
            }
        } else {
            // Extract keyword from pattern like ".*keyword.*"
            if regex.hasPrefix(".*") && regex.hasSuffix(".*") {
                let inner = String(regex.dropFirst(2).dropLast(2))
                // Unescape common regex escapes
                return inner
                    .replacingOccurrences(of: "\\.", with: ".")
                    .replacingOccurrences(of: "\\(", with: "(")
                    .replacingOccurrences(of: "\\)", with: ")")
                    .replacingOccurrences(of: "\\[", with: "[")
                    .replacingOccurrences(of: "\\]", with: "]")
                    .replacingOccurrences(of: "\\\\", with: "\\")
            }
        }
        // Fallback to raw regex if pattern doesn't match expected format
        return regex
    }
}

// MARK: - Preset Rule Detection

extension RoutingRuleConfig {
    /// Check if this is a preset system rule
    ///
    /// Preset rules are:
    /// - `/search` - Web search capability (implemented)
    /// - `/mcp` - MCP integration (reserved)
    /// - `/skill` - Skills workflow (reserved)
    ///
    /// Detection: isBuiltin flag or intent_type starts with "builtin_" or equals "skills"
    var isPreset: Bool {
        if isBuiltin { return true }
        guard let intent = intentType else { return false }
        return intent.hasPrefix("builtin_") || intent == "skills"
    }

    /// Get user-friendly name for preset rule
    var presetCommandName: String? {
        guard isPreset else { return nil }

        // Extract command from regex pattern
        // Pattern format: ^/command\s+
        let pattern = regex
        if pattern.hasPrefix("^/") {
            let parts = pattern.dropFirst(2).split(separator: "\\")
            if let command = parts.first {
                return "/\(command)"
            }
        }

        return nil
    }

    /// Get description for preset rule
    var presetDescription: String? {
        guard let intent = intentType, isPreset else { return nil }

        switch intent {
        case "builtin_search":
            return L("settings.routing.preset.search.description")
        case "builtin_mcp":
            return L("settings.routing.preset.mcp.description")
        case "skills":
            return L("settings.routing.preset.skills.description")
        default:
            return nil
        }
    }

    /// Check if preset feature is implemented
    var isPresetImplemented: Bool {
        guard let intent = intentType else { return false }
        return intent == "builtin_search"  // Only /search is implemented
    }
}
