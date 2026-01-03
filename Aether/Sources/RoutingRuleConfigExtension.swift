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
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let regex = try container.decode(String.self, forKey: .regex)
        let provider = try container.decode(String.self, forKey: .provider)
        let systemPrompt = try container.decodeIfPresent(String.self, forKey: .systemPrompt)
        let stripPrefix = try container.decodeIfPresent(Bool.self, forKey: .stripPrefix)

        self.init(regex: regex, provider: provider, systemPrompt: systemPrompt, stripPrefix: stripPrefix)
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
    }
}
