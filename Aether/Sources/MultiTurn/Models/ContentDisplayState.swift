//
//  ContentDisplayState.swift
//  Aether
//
//  Display state for unified conversation window content area.
//

import Foundation

// MARK: - ContentDisplayState

/// Mutually exclusive display states for the content area above input
enum ContentDisplayState: Equatable {
    /// No conversation, no commands - initial state
    case empty

    /// Showing conversation history
    case conversation

    /// Showing command or topic list
    case commandList(prefix: String)  // "/" for commands, "//" for topics

    /// Check if showing command list
    var isShowingCommandList: Bool {
        if case .commandList = self {
            return true
        }
        return false
    }

    /// Check if showing topic list (// prefix)
    var isShowingTopicList: Bool {
        if case .commandList(let prefix) = self {
            return prefix == "//"
        }
        return false
    }

    /// Check if showing command list (/ prefix, not //)
    var isShowingCommands: Bool {
        if case .commandList(let prefix) = self {
            return prefix == "/"
        }
        return false
    }
}
