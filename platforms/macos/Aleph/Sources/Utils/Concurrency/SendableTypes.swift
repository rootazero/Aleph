//
//  SendableTypes.swift
//  Aether
//
//  Provides Sendable-compatible wrappers and utilities for Swift 6 concurrency.
//  These types help bridge legacy code with structured concurrency.
//

import Foundation

// MARK: - Unchecked Sendable Wrappers

/// A wrapper that marks a value as Sendable without compiler verification.
/// Use with caution - only when you can guarantee thread-safety manually.
///
/// Example usage:
/// ```swift
/// let unchecked = UncheckedSendable(someNonSendableValue)
/// Task { @MainActor in
///     let value = unchecked.value
///     // use value on main actor
/// }
/// ```
@frozen
public struct UncheckedSendable<Value>: @unchecked Sendable {
    public var value: Value

    public init(_ value: Value) {
        self.value = value
    }
}

/// A box type for reference types that need to be passed across concurrency boundaries.
/// Use when you need to pass a class instance that you know is thread-safe.
public final class UncheckedSendableBox<Value>: @unchecked Sendable {
    public var value: Value

    public init(_ value: Value) {
        self.value = value
    }
}

// MARK: - Main Actor Utilities

/// Execute a closure on the main actor.
/// This is useful for bridging from synchronous contexts where you need to update UI.
@MainActor
public func onMainActor<T>(_ operation: @MainActor () -> T) -> T {
    operation()
}

/// Execute an async closure on the main actor.
@MainActor
public func onMainActorAsync<T>(_ operation: @MainActor () async -> T) async -> T {
    await operation()
}

// MARK: - Sendable Notification UserInfo

/// A Sendable wrapper for notification userInfo dictionaries.
/// Useful when posting notifications across concurrency boundaries.
public struct SendableUserInfo: Sendable {
    private let storage: [String: AnySendable]

    public init(_ dictionary: [String: Any]) {
        var sendableStorage: [String: AnySendable] = [:]
        for (key, value) in dictionary {
            // Wrap all values - caller is responsible for ensuring thread safety
            sendableStorage[key] = AnySendable(value)
        }
        self.storage = sendableStorage
    }

    public subscript<T>(_ key: String) -> T? {
        storage[key]?.value as? T
    }

    public var asDictionary: [String: Any] {
        var result: [String: Any] = [:]
        for (key, value) in storage {
            result[key] = value.value
        }
        return result
    }
}

/// Type-erased Sendable wrapper.
/// Note: The caller is responsible for ensuring the wrapped value is actually thread-safe.
public struct AnySendable: @unchecked Sendable {
    public let value: Any

    public init(_ value: Any) {
        self.value = value
    }
}

// MARK: - Task Sleep Extension

public extension Task where Success == Never, Failure == Never {
    /// Sleep for a specified number of seconds.
    /// Convenience wrapper for Task.sleep(nanoseconds:).
    ///
    /// Example:
    /// ```swift
    /// try await Task.sleep(seconds: 0.5)
    /// ```
    static func sleep(seconds: Double) async throws {
        try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }
}

// MARK: - UniFFI Generated Types Sendable Conformance

// These extensions add Sendable conformance to UniFFI-generated types.
// These types are value types with only Sendable members, so they are safe
// to pass across concurrency boundaries.

// Clarification types
extension ClarificationRequest: @unchecked Sendable {}
extension ClarificationOption: @unchecked Sendable {}
extension ClarificationType: @unchecked Sendable {}
extension ClarificationResult: @unchecked Sendable {}

// Media types
extension MediaAttachment: @unchecked Sendable {}

// Task types
extension ExecutableTaskFfi: @unchecked Sendable {}
extension TaskCategoryFfi: @unchecked Sendable {}

// MCP types
extension McpStartupReportFfi: @unchecked Sendable {}
extension McpServerErrorFfi: @unchecked Sendable {}

// Runtime types
extension RuntimeUpdateInfo: @unchecked Sendable {}

// Note: AetherCore already has @unchecked Sendable in UniFFI generated code (aether.swift:882)
// No need to add it again here

// Generation types
extension GenerationTypeFfi: @unchecked Sendable {}

// MARK: - Migration Notes

/*
 Swift 6 Async/Await Migration Patterns:

 1. DispatchQueue.main.async { } → Task { @MainActor in }
    Before:
        DispatchQueue.main.async { self.updateUI() }
    After:
        Task { @MainActor in self.updateUI() }

 2. DispatchQueue.mainAsync(weakRef: self) { slf in } → Task { @MainActor [weak self] in }
    Before:
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.updateUI()
        }
    After:
        Task { @MainActor [weak self] in
            self?.updateUI()
        }

 3. DispatchQueue.global(qos:).async { } → Task.detached(priority:) { }
    Before:
        DispatchQueue.global(qos: .userInitiated).async {
            let result = process()
            DispatchQueue.main.async { updateUI(result) }
        }
    After:
        Task.detached(priority: .userInitiated) {
            let result = process()
            await MainActor.run { updateUI(result) }
        }

 4. DispatchQueue.main.asyncAfter(deadline:) → Task.sleep
    Before:
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            self.doSomething()
        }
    After:
        Task { @MainActor in
            try? await Task.sleep(seconds: 0.5)
            self.doSomething()
        }

 5. completion: @escaping (T) -> Void → async -> T
    Before:
        func fetch(completion: @escaping (Result) -> Void)
    After:
        func fetch() async -> Result

 6. DispatchGroup → withTaskGroup
    Before:
        let group = DispatchGroup()
        for item in items {
            group.enter()
            process(item) { group.leave() }
        }
        group.notify(queue: .main) { ... }
    After:
        await withTaskGroup(of: Void.self) { group in
            for item in items {
                group.addTask { await process(item) }
            }
        }
        // continuation here runs after all tasks complete
*/
