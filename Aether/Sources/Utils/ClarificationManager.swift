//
//  ClarificationManager.swift
//  Aether
//
//  Manages Phantom Flow clarification requests between Rust core and Halo UI.
//  This is the Swift-side coordinator for in-place user interaction.
//

import Foundation
import SwiftUI
import Combine

/// Manager for Phantom Flow clarification requests
///
/// This class bridges the synchronous Rust callback with the async SwiftUI interface.
/// It uses a semaphore to block the calling thread while waiting for user input.
///
/// Thread Safety:
/// - `handleRequest` can be called from ANY thread (including Rust background threads)
/// - UI state updates happen on main thread via DispatchQueue.main
/// - Uses lock for thread-safe access to shared state
///
/// Flow:
/// 1. Rust calls `onClarificationNeeded()` (blocks Rust thread)
/// 2. Manager posts notification to show clarification UI (on main thread)
/// 3. User interacts with Halo (select option or enter text)
/// 4. Manager signals completion, returns result to Rust
class ClarificationManager: ObservableObject {
    /// Shared instance for global access
    static let shared = ClarificationManager()

    // MARK: - Published Properties (Main Thread Only)

    /// Current clarification request being displayed
    @Published private(set) var currentRequest: ClarificationRequest?

    /// Whether a clarification is currently in progress
    @Published private(set) var isActive: Bool = false

    /// Selected option index (for select-type clarifications)
    @Published var selectedIndex: Int = 0

    /// Text input value (for text-type clarifications)
    @Published var textInput: String = ""

    // MARK: - Thread-Safe Properties

    /// Lock for thread-safe access
    private let lock = NSLock()

    /// Semaphore for synchronizing with calling thread
    private var completionSemaphore: DispatchSemaphore?

    /// Result to return to caller
    private var pendingResult: ClarificationResult?

    /// Timeout for clarification requests (seconds)
    private let timeoutSeconds: Double = 30.0

    private init() {}

    // MARK: - Public API (Thread-Safe)

    /// Handle a clarification request from Rust core
    ///
    /// This method can be called from ANY thread (including Rust/UniFFI background threads).
    /// It blocks until the user responds or times out.
    ///
    /// - Parameter request: The clarification request from Rust
    /// - Returns: The user's response
    func handleRequest(_ request: ClarificationRequest) -> ClarificationResult {
        print("[ClarificationManager] Handling request: \(request.id) on thread: \(Thread.current)")

        // Create semaphore for blocking
        let semaphore = DispatchSemaphore(value: 0)

        lock.lock()
        completionSemaphore = semaphore
        pendingResult = nil
        lock.unlock()

        // Update UI state on main thread
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            self.resetState()
            self.currentRequest = request
            self.isActive = true

            // Set default value if provided
            if let defaultValue = request.defaultValue {
                if request.clarificationType == .select,
                   let index = Int(defaultValue) {
                    self.selectedIndex = index
                } else if request.clarificationType == .text {
                    self.textInput = defaultValue
                }
            }

            // Post notification to show clarification UI in Halo
            NotificationCenter.default.post(
                name: .clarificationRequested,
                object: request
            )
        }

        // Wait for completion or timeout (blocks calling thread)
        let waitResult = semaphore.wait(timeout: .now() + timeoutSeconds)

        // Get the result thread-safely
        lock.lock()
        let response: ClarificationResult
        if waitResult == .timedOut {
            print("[ClarificationManager] Request timed out: \(request.id)")
            response = ClarificationResult(
                resultType: .timeout,
                selectedIndex: nil,
                value: nil
            )
        } else if let result = pendingResult {
            response = result
        } else {
            response = ClarificationResult(
                resultType: .cancelled,
                selectedIndex: nil,
                value: nil
            )
        }
        pendingResult = nil
        completionSemaphore = nil
        lock.unlock()

        // Cleanup UI on main thread
        DispatchQueue.main.async { [weak self] in
            self?.isActive = false
            self?.currentRequest = nil
        }

        print("[ClarificationManager] Returning result: \(response.resultType)")
        return response
    }

    /// Complete the current clarification with a selected option
    ///
    /// Must be called from main thread.
    ///
    /// - Parameters:
    ///   - index: The selected option index
    ///   - value: The value of the selected option
    @MainActor
    func completeWithSelection(index: Int, value: String) {
        print("[ClarificationManager] Completed with selection: index=\(index), value=\(value)")

        lock.lock()
        pendingResult = ClarificationResult(
            resultType: .selected,
            selectedIndex: UInt32(index),
            value: value
        )
        completionSemaphore?.signal()
        lock.unlock()
    }

    /// Complete the current clarification with text input
    ///
    /// Must be called from main thread.
    ///
    /// - Parameter text: The entered text
    @MainActor
    func completeWithText(_ text: String) {
        print("[ClarificationManager] Completed with text: \(text)")

        lock.lock()
        pendingResult = ClarificationResult(
            resultType: .textInput,
            selectedIndex: nil,
            value: text
        )
        completionSemaphore?.signal()
        lock.unlock()
    }

    /// Cancel the current clarification
    ///
    /// Must be called from main thread.
    @MainActor
    func cancel() {
        print("[ClarificationManager] Cancelled by user")

        lock.lock()
        pendingResult = ClarificationResult(
            resultType: .cancelled,
            selectedIndex: nil,
            value: nil
        )
        completionSemaphore?.signal()
        lock.unlock()
    }

    // MARK: - Private Helpers

    /// Reset UI state (must be called on main thread)
    private func resetState() {
        selectedIndex = 0
        textInput = ""
    }
}

// MARK: - Notification Names

extension Notification.Name {
    /// Posted when a clarification request needs to be displayed
    static let clarificationRequested = Notification.Name("AetherClarificationRequested")

    /// Posted when clarification is completed
    static let clarificationCompleted = Notification.Name("AetherClarificationCompleted")
}

// MARK: - Preview Helpers

#if DEBUG
extension ClarificationManager {
    /// Create a test request for previews
    static func testSelectRequest() -> ClarificationRequest {
        ClarificationRequest(
            id: "test-style",
            prompt: "What style would you like?",
            clarificationType: .select,
            options: [
                ClarificationOption(label: "Professional", value: "professional", description: "Formal business tone"),
                ClarificationOption(label: "Casual", value: "casual", description: "Friendly and relaxed"),
                ClarificationOption(label: "Humorous", value: "humorous", description: "Light and playful"),
            ],
            defaultValue: "0",
            placeholder: nil,
            source: "skill:refine-text"
        )
    }

    /// Create a test text request for previews
    static func testTextRequest() -> ClarificationRequest {
        ClarificationRequest(
            id: "test-language",
            prompt: "Enter target language:",
            clarificationType: .text,
            options: nil,
            defaultValue: nil,
            placeholder: "e.g., Spanish, French...",
            source: "skill:translate"
        )
    }
}
#endif
