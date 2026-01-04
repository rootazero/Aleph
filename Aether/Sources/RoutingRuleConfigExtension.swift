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
        let regex = try container.decode(String.self, forKey: .regex)
        let provider = try container.decode(String.self, forKey: .provider)
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
        try container.encode(regex, forKey: .regex)
        try container.encode(provider, forKey: .provider)
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
