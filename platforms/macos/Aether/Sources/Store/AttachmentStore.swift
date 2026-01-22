//
//  AttachmentStore.swift
//  Aether
//
//  SQLite persistence for attachments.
//  Linked to ConversationStore's database.
//

import Foundation
import GRDB

// MARK: - AttachmentStore

/// Manages SQLite persistence for attachments
///
/// Thread Safety:
/// - Uses ConversationStore's shared DatabaseQueue for consistency
/// - All operations are thread-safe via GRDB
final class AttachmentStore: @unchecked Sendable {

    // MARK: - Singleton

    static let shared = AttachmentStore()

    // MARK: - Initialization

    private init() {
        // Table migration is handled by ConversationStore
    }

    // MARK: - CRUD Operations

    /// Save a new attachment
    /// - Parameter attachment: The attachment to save
    /// - Returns: The saved attachment, or nil if failed
    @discardableResult
    func save(_ attachment: StoredAttachment) -> StoredAttachment? {
        do {
            try ConversationStore.shared.dbWrite { db in
                try attachment.insert(db)
            }
            print("[AttachmentStore] Saved attachment: \(attachment.id) for message: \(attachment.messageId)")
            return attachment
        } catch {
            print("[AttachmentStore] Failed to save attachment: \(error)")
            return nil
        }
    }

    /// Save multiple attachments
    /// - Parameter attachments: The attachments to save
    /// - Returns: Number of successfully saved attachments
    @discardableResult
    func saveAll(_ attachments: [StoredAttachment]) -> Int {
        do {
            var count = 0
            try ConversationStore.shared.dbWrite { db in
                for attachment in attachments {
                    try attachment.insert(db)
                    count += 1
                }
            }
            print("[AttachmentStore] Saved \(count) attachments")
            return count
        } catch {
            print("[AttachmentStore] Failed to save attachments: \(error)")
            return 0
        }
    }

    /// Get all attachments for a message
    /// - Parameter messageId: The message ID
    /// - Returns: Array of attachments, sorted by creation date
    func getAttachments(forMessage messageId: String) -> [StoredAttachment] {
        do {
            return try ConversationStore.shared.dbRead { db in
                try StoredAttachment
                    .filter(Column("messageId") == messageId)
                    .order(Column("createdAt").asc)
                    .fetchAll(db)
            } ?? []
        } catch {
            print("[AttachmentStore] Failed to fetch attachments: \(error)")
            return []
        }
    }

    /// Get a single attachment by ID
    /// - Parameter id: The attachment ID
    /// - Returns: The attachment, or nil if not found
    func getAttachment(id: String) -> StoredAttachment? {
        do {
            return try ConversationStore.shared.dbRead { db in
                try StoredAttachment.fetchOne(db, key: id)
            }
        } catch {
            print("[AttachmentStore] Failed to fetch attachment: \(error)")
            return nil
        }
    }

    /// Get all attachments for a topic (via messages)
    /// - Parameter topicId: The topic ID
    /// - Returns: Array of attachments
    func getAttachments(forTopic topicId: String) -> [StoredAttachment] {
        do {
            return try ConversationStore.shared.dbRead { db in
                try StoredAttachment
                    .joining(required: StoredAttachment.message.filter(Column("topicId") == topicId))
                    .order(Column("createdAt").asc)
                    .fetchAll(db)
            } ?? []
        } catch {
            print("[AttachmentStore] Failed to fetch topic attachments: \(error)")
            return []
        }
    }

    /// Update attachment's local path (e.g., after caching remote URL)
    /// - Parameters:
    ///   - id: The attachment ID
    ///   - localPath: The new local path
    func updateLocalPath(id: String, localPath: String) {
        do {
            try ConversationStore.shared.dbWrite { db in
                try db.execute(
                    sql: "UPDATE attachments SET localPath = ? WHERE id = ?",
                    arguments: [localPath, id]
                )
            }
            print("[AttachmentStore] Updated localPath for attachment: \(id)")
        } catch {
            print("[AttachmentStore] Failed to update localPath: \(error)")
        }
    }

    /// Delete a single attachment
    /// - Parameter id: The attachment ID
    /// - Returns: True if deleted successfully
    @discardableResult
    func delete(id: String) -> Bool {
        do {
            try ConversationStore.shared.dbWrite { db in
                try db.execute(
                    sql: "DELETE FROM attachments WHERE id = ?",
                    arguments: [id]
                )
            }
            print("[AttachmentStore] Deleted attachment: \(id)")
            return true
        } catch {
            print("[AttachmentStore] Failed to delete attachment: \(error)")
            return false
        }
    }

    /// Delete all attachments for a message
    /// - Parameter messageId: The message ID
    /// - Returns: Number of deleted attachments
    @discardableResult
    func deleteAttachments(forMessage messageId: String) -> Int {
        do {
            return try ConversationStore.shared.dbWrite { db in
                try db.execute(
                    sql: "DELETE FROM attachments WHERE messageId = ?",
                    arguments: [messageId]
                )
                return db.changesCount
            } ?? 0
        } catch {
            print("[AttachmentStore] Failed to delete message attachments: \(error)")
            return 0
        }
    }

    /// Delete all attachments for a topic (called when topic is deleted)
    /// - Parameter topicId: The topic ID
    /// - Returns: Number of deleted attachments
    @discardableResult
    func deleteAttachments(forTopic topicId: String) -> Int {
        do {
            return try ConversationStore.shared.dbWrite { db in
                // Delete attachments where message belongs to this topic
                try db.execute(
                    sql: """
                        DELETE FROM attachments WHERE messageId IN (
                            SELECT id FROM messages WHERE topicId = ?
                        )
                        """,
                    arguments: [topicId]
                )
                return db.changesCount
            } ?? 0
        } catch {
            print("[AttachmentStore] Failed to delete topic attachments: \(error)")
            return 0
        }
    }

    /// Get all attachment IDs for a topic (for file cleanup)
    /// - Parameter topicId: The topic ID
    /// - Returns: Array of attachment IDs and their local paths
    func getAttachmentPaths(forTopic topicId: String) -> [(id: String, localPath: String?)] {
        do {
            return try ConversationStore.shared.dbRead { db in
                let rows = try Row.fetchAll(db, sql: """
                    SELECT a.id, a.localPath FROM attachments a
                    JOIN messages m ON a.messageId = m.id
                    WHERE m.topicId = ?
                    """, arguments: [topicId])
                return rows.map { row in
                    (id: row["id"], localPath: row["localPath"])
                }
            } ?? []
        } catch {
            print("[AttachmentStore] Failed to get attachment paths: \(error)")
            return []
        }
    }

    // MARK: - Statistics

    /// Get attachment count for a message
    func getAttachmentCount(forMessage messageId: String) -> Int {
        do {
            return try ConversationStore.shared.dbRead { db in
                try StoredAttachment
                    .filter(Column("messageId") == messageId)
                    .fetchCount(db)
            } ?? 0
        } catch {
            print("[AttachmentStore] Failed to count attachments: \(error)")
            return 0
        }
    }

    /// Get total storage used by attachments (in bytes)
    func getTotalStorageUsed() -> Int64 {
        do {
            return try ConversationStore.shared.dbRead { db in
                let sum = try Int64.fetchOne(db, sql: "SELECT SUM(sizeBytes) FROM attachments")
                return sum ?? 0
            } ?? 0
        } catch {
            print("[AttachmentStore] Failed to get storage used: \(error)")
            return 0
        }
    }
}

// MARK: - GRDB Association

extension StoredAttachment {
    /// Association to the parent message
    /// nonisolated(unsafe) to satisfy Sendable requirements for GRDB association
    nonisolated(unsafe) static let message = belongsTo(ConversationMessage.self, using: ForeignKey(["messageId"]))
}
