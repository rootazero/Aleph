//
//  DispatchQueue+MainAsync.swift
//  Aether
//
//  Convenience extension to reduce boilerplate for main thread dispatch
//  with weak self capture pattern.
//

import Foundation

extension DispatchQueue {
    /// Execute a closure on the main thread with weak reference capture.
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
    static func mainAsync<T: AnyObject>(weakRef object: T, _ closure: @escaping (T) -> Void) {
        DispatchQueue.main.async { [weak object] in
            guard let obj = object else { return }
            closure(obj)
        }
    }

    /// Execute a closure on the main thread with weak reference capture, after a delay.
    ///
    /// - Parameters:
    ///   - delay: The delay in seconds before executing
    ///   - object: The object to capture weakly
    ///   - closure: The closure to execute with the unwrapped object
    static func mainAsyncAfter<T: AnyObject>(
        delay seconds: Double,
        weakRef object: T,
        _ closure: @escaping (T) -> Void
    ) {
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak object] in
            guard let obj = object else { return }
            closure(obj)
        }
    }
}
