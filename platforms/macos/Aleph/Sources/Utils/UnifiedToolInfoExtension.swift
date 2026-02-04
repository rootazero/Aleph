//
//  UnifiedToolInfoExtension.swift
//  Aether
//
//  Created by unify-tool-registry proposal
//  Extension to make UnifiedToolInfo work with Settings UI
//

import Foundation

// MARK: - UnifiedToolInfo UI Extensions

extension UnifiedToolInfo {
    /// Get the command string (e.g., "/search")
    var command: String {
        "/\(name)"
    }

    /// Get the localized description
    var localizedDescription: String {
        if let key = localizationKey {
            // Try to get localized string with key like "tool.search.description"
            let localizedKey = "\(key).description"
            let localized = L(localizedKey)
            // If not found, fall back to the raw description
            return localized == localizedKey ? description : localized
        }
        return description
    }

    /// Get the localized hint (short description)
    var localizedHint: String {
        if let key = localizationKey {
            let localizedKey = "\(key).hint"
            let localized = L(localizedKey)
            return localized == localizedKey ? displayName : localized
        }
        return displayName
    }

    /// Get the localized usage example
    var localizedUsage: String {
        if let key = localizationKey {
            let localizedKey = "\(key).usage"
            let localized = L(localizedKey)
            return localized == localizedKey ? (usage ?? command) : localized
        }
        return usage ?? command
    }

    /// Get SF Symbol icon name
    var iconName: String {
        icon ?? sourceType.defaultIcon
    }

    /// Whether this tool has subcommands
    var hasSubcommands: Bool {
        hasSubtools
    }
}

// MARK: - ToolSourceType Icon Extensions

extension ToolSourceType {
    /// Default icon for each source type
    var defaultIcon: String {
        switch self {
        case .native:
            return "star.fill"
        case .builtin:
            return "command.circle.fill"
        case .mcp:
            return "puzzlepiece.extension"
        case .skill:
            return "wand.and.stars"
        case .custom:
            return "command"
        }
    }

    /// Display label for each source type
    var displayLabel: String {
        switch self {
        case .native:
            return "Native"
        case .builtin:
            return L("tool.source.builtin")
        case .mcp:
            return "MCP"
        case .skill:
            return L("tool.source.skill")
        case .custom:
            return L("tool.source.custom")
        }
    }
}

// MARK: - Array Extension for Sorting

extension Array where Element == UnifiedToolInfo {
    /// Sort by sort_order, then by name
    func sortedByOrder() -> [UnifiedToolInfo] {
        self.sorted { a, b in
            if a.sortOrder != b.sortOrder {
                return a.sortOrder < b.sortOrder
            }
            return a.name < b.name
        }
    }
}
