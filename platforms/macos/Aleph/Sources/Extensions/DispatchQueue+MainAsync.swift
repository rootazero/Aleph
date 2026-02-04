//
//  DispatchQueue+MainAsync.swift
//  Aleph
//
//  Convenience extension to reduce boilerplate for main thread dispatch
//  with weak self capture pattern.
//
//  DEPRECATION NOTE: Prefer using Task { @MainActor in } pattern for new code.
//  This extension is kept for backward compatibility with existing code.
//

import Foundation

extension DispatchQueue {
    /// Execute a closure on the main thread with weak reference capture.
    ///
    /// - Note: **Deprecated for new code.** Prefer `Task { @MainActor [weak self] in }` pattern.
    ///
    /// Replaces the common pattern:
    /// ```swift
    /// DispatchQueue.main.async { [weak self] in
    ///     guard let self = self else { return }
    ///     self.doSomething()
    /// }
    /// ```
    ///
    /// With:
    /// ```swift
    /// DispatchQueue.mainAsync(weakRef: self) { slf in
    ///     slf.doSomething()
    /// }
    /// ```
    ///
    /// - Parameters:
    ///   - object: The object to capture weakly
    ///   - closure: The closure to execute with the unwrapped object
    static func mainAsync<T: AnyObject & Sendable>(weakRef object: T, _ closure: @escaping @Sendable (T) -> Void) {
        DispatchQueue.main.async { [weak object] in
            guard let obj = object else { return }
            closure(obj)
        }
    }

    /// Execute a closure on the main thread with weak reference capture, after a delay.
    ///
    /// - Note: **Deprecated for new code.** Prefer `Task { @MainActor [weak self] in try? await Task.sleep(seconds:) }` pattern.
    ///
    /// - Parameters:
    ///   - delay: The delay in seconds before executing
    ///   - object: The object to capture weakly
    ///   - closure: The closure to execute with the unwrapped object
    static func mainAsyncAfter<T: AnyObject & Sendable>(
        delay seconds: Double,
        weakRef object: T,
        _ closure: @escaping @Sendable (T) -> Void
    ) {
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak object] in
            guard let obj = object else { return }
            closure(obj)
        }
    }
}
