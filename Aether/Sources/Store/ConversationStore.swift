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
}
