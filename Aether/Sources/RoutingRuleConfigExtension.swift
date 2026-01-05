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
            knowledgeBase: knowledgeBase
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
