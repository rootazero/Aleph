// RefStore.swift
// Stores ref-to-element mappings from UI snapshots for the Desktop Bridge.

import Foundation

/// A resolved UI element from a snapshot.
struct ResolvedElement {
    let refId: String
    let role: String
    let label: String?
    let frame: CGRect
}

/// Error types for ref resolution.
enum RefError: LocalizedError {
    case notFound(String)
    case noSnapshot

    var errorDescription: String? {
        switch self {
        case .notFound(let refId):
            return "ref '\(refId)' not found in current snapshot. Run snapshot to refresh refs."
        case .noSnapshot:
            return "No snapshot available. Run snapshot first."
        }
    }
}

/// Stores the current ref map from the most recent snapshot.
/// Thread-safe: all access is serialized through a lock.
final class RefStore: @unchecked Sendable {
    static let shared = RefStore()

    private let lock = NSLock()
    private var refs: [String: ResolvedElement] = [:]
    private var snapshotTimestamp: Date?

    /// Replace refs with a new set from a fresh snapshot.
    func update(newRefs: [String: ResolvedElement]) {
        lock.lock()
        defer { lock.unlock() }
        refs = newRefs
        snapshotTimestamp = Date()
    }

    /// Resolve a ref ID to a center point for action targeting.
    func resolve(_ refId: String) -> Result<CGPoint, RefError> {
        lock.lock()
        defer { lock.unlock() }

        guard snapshotTimestamp != nil else {
            return .failure(.noSnapshot)
        }

        guard let element = refs[refId] else {
            return .failure(.notFound(refId))
        }

        let center = CGPoint(
            x: element.frame.origin.x + element.frame.size.width / 2,
            y: element.frame.origin.y + element.frame.size.height / 2
        )
        return .success(center)
    }

    /// Clear stored refs.
    func clear() {
        lock.lock()
        defer { lock.unlock() }
        refs.removeAll()
        snapshotTimestamp = nil
    }
}
