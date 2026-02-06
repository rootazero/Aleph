//
//  ConversationModels.swift
//  Aleph
//
//  Data models for conversation persistence.
//

import Foundation
import GRDB

// MARK: - Topic

/// A conversation topic (session)
struct Topic: Identifiable, Codable, FetchableRecord, PersistableRecord {
    var id: String
    var title: String
    var createdAt: Date
    var updatedAt: Date
    var isDeleted: Bool

    static let databaseTableName = "topics"

    init(id: String = UUID().uuidString, title: String = "New Conversation") {
        self.id = id
        self.title = title
        self.createdAt = Date()
        self.updatedAt = Date()
        self.isDeleted = false
    }
}

// MARK: - Message

/// A single message in a conversation
struct ConversationMessage: Identifiable, Codable, FetchableRecord, PersistableRecord {
    var id: String
    var topicId: String
    var role: MessageRole
    var content: String
    var createdAt: Date

    static let databaseTableName = "messages"

    init(id: String = UUID().uuidString, topicId: String, role: MessageRole, content: String) {
        self.id = id
        self.topicId = topicId
        self.role = role
        self.content = content
        self.createdAt = Date()
    }
}

// MARK: - MessageRole

enum MessageRole: String, Codable, DatabaseValueConvertible {
    case user
    case assistant
}
