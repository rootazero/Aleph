//
//  ClarificationManagerProtocol.swift
//  Aleph
//
//  Protocol for clarification flow management, enabling dependency injection and testability.
//

import Foundation
import Combine

/// Protocol for Phantom Flow clarification management
///
/// Abstracts clarification flow for dependency injection and testing.
/// The default implementation is ClarificationManager.
protocol ClarificationManagerProtocol: ObservableObject {

    /// Current clarification request being displayed
    var currentRequest: ClarificationRequest? { get }

    /// Whether a clarification is currently in progress
    var isActive: Bool { get }

    /// Selected option index (for select-type clarifications)
    var selectedIndex: Int { get set }

    /// Text input value (for text-type clarifications)
    var textInput: String { get set }

    /// Handle a clarification request from Rust core
    ///
    /// This method blocks until the user responds or times out.
    ///
    /// - Parameter request: The clarification request from Rust
    /// - Returns: The user's response
    func handleRequest(_ request: ClarificationRequest) -> ClarificationResult

    /// Complete the current clarification with a selected option
    @MainActor
    func completeWithSelection(index: Int, value: String)

    /// Complete the current clarification with text input
    @MainActor
    func completeWithText(_ text: String)

    /// Cancel the current clarification
    @MainActor
    func cancel()
}

// MARK: - Default Implementation Conformance

extension ClarificationManager: ClarificationManagerProtocol {}
