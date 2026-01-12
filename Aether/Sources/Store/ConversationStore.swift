//
//  ConversationStore.swift
//  Aether
//
//  SQLite persistence for multi-turn conversations.
//

import Foundation
import GRDB

// MARK: - ConversationStore

/// Manages SQLite persistence for conversations
final class ConversationStore {

    // MARK: - Singleton

    static let shared = ConversationStore()

    // MARK: - Properties

    private var dbQueue: DatabaseQueue?

    // MARK: - Initialization

    private init() {
        setupDatabase()
    }

    // MARK: - Database Setup

    private func setupDatabase() {
        do {
            let dbPath = getDBPath()
            dbQueue = try DatabaseQueue(path: dbPath)
            try createTables()
            print("[ConversationStore] Database initialized at: \(dbPath)")
        } catch {
            print("[ConversationStore] Failed to setup database: \(error)")
        }
    }

    private func getDBPath() -> String {
        let configDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aether")

        // Create directory if needed
        try? FileManager.default.createDirectory(
            at: configDir,
            withIntermediateDirectories: true
        )

        return configDir.appendingPathComponent("conversations.db").path
    }

    private func createTables() throws {
        try dbQueue?.write { db in
            // Topics table
            try db.create(table: "topics", ifNotExists: true) { t in
                t.column("id", .text).primaryKey()
                t.column("title", .text).notNull()
                t.column("createdAt", .datetime).notNull()
                t.column("updatedAt", .datetime).notNull()
                t.column("isDeleted", .boolean).notNull().defaults(to: false)
            }

            // Messages table
            try db.create(table: "messages", ifNotExists: true) { t in
                t.column("id", .text).primaryKey()
                t.column("topicId", .text).notNull().references("topics", onDelete: .cascade)
                t.column("role", .text).notNull()
                t.column("content", .text).notNull()
                t.column("createdAt", .datetime).notNull()
            }

            // Indexes
            try db.create(index: "idx_messages_topic", on: "messages", columns: ["topicId"], ifNotExists: true)
            try db.create(index: "idx_topics_updated", on: "topics", columns: ["updatedAt"], ifNotExists: true)
        }
    }

    // MARK: - Topic Operations

    /// Create a new topic
    func createTopic(title: String = "New Conversation") -> Topic? {
        let topic = Topic(title: title)
        do {
            try dbQueue?.write { db in
                try topic.insert(db)
            }
            print("[ConversationStore] Created topic: \(topic.id)")
            return topic
        } catch {
            print("[ConversationStore] Failed to create topic: \(error)")
            return nil
        }
    }

    /// Get all non-deleted topics, sorted by updatedAt DESC
    func getAllTopics() -> [Topic] {
        do {
            return try dbQueue?.read { db in
                try Topic
                    .filter(Column("isDeleted") == false)
                    .order(Column("updatedAt").desc)
                    .fetchAll(db)
            } ?? []
        } catch {
            print("[ConversationStore] Failed to fetch topics: \(error)")
            return []
        }
    }

    /// Get a topic by ID
    func getTopic(id: String) -> Topic? {
        do {
            return try dbQueue?.read { db in
                try Topic.fetchOne(db, key: id)
            }
        } catch {
            print("[ConversationStore] Failed to fetch topic: \(error)")
            return nil
        }
    }

    /// Update topic title
    func updateTopicTitle(id: String, title: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "UPDATE topics SET title = ?, updatedAt = ? WHERE id = ?",
                    arguments: [title, Date(), id]
                )
            }
            print("[ConversationStore] Updated topic title: \(id) -> \(title)")
        } catch {
            print("[ConversationStore] Failed to update topic title: \(error)")
        }
    }

    /// Soft delete a topic
    func deleteTopic(id: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "UPDATE topics SET isDeleted = 1 WHERE id = ?",
                    arguments: [id]
                )
            }
            print("[ConversationStore] Deleted topic: \(id)")
        } catch {
            print("[ConversationStore] Failed to delete topic: \(error)")
        }
    }

    /// Update topic's updatedAt timestamp
    func touchTopic(id: String) {
        do {
            try dbQueue?.write { db in
                try db.execute(
                    sql: "UPDATE topics SET updatedAt = ? WHERE id = ?",
                    arguments: [Date(), id]
                )
            }
        } catch {
            print("[ConversationStore] Failed to touch topic: \(error)")
        }
    }
}
