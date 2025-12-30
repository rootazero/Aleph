//
//  FormState.swift
//  Aether
//
//  Protocol for settings tabs implementing staged commit pattern.
//  Provides working copy / saved state separation with save/cancel functionality.
//

import Foundation

/// Protocol for settings tabs that implement staged commit pattern with working copy / saved state separation
///
/// **Usage**:
/// ```swift
/// struct MySettingsView: View, FormStateful {
///     @State private var workingCopy: MyConfig = .default
///     @State private var savedState: MyConfig = .default
///
///     var hasUnsavedChanges: Bool {
///         workingCopy != savedState
///     }
///
///     func save() async throws {
///         try await core.updateConfig(workingCopy)
///         savedState = workingCopy
///     }
///
///     func cancel() {
///         workingCopy = savedState
///     }
/// }
/// ```
protocol FormStateful {
    /// Type representing the editable working copy of form data
    associatedtype WorkingCopy: Equatable

    /// Type representing the last saved/persisted state
    associatedtype SavedState: Equatable

    /// Current working copy of form data (editable)
    var workingCopy: WorkingCopy { get set }

    /// Last saved state (persisted to disk/config)
    var savedState: SavedState { get }

    /// Whether form has unsaved changes (working copy differs from saved state)
    var hasUnsavedChanges: Bool { get }

    /// Save working copy to persistent storage and update saved state
    /// - Throws: Error if save operation fails (disk write, validation, etc.)
    func save() async throws

    /// Cancel unsaved changes by reverting working copy to saved state
    func cancel()

    /// Load saved state from persistent storage (called on view appear)
    func loadSavedState() async
}

// MARK: - Default Implementations

extension FormStateful where WorkingCopy == SavedState {
    /// Default implementation: Compare working copy to saved state
    var hasUnsavedChanges: Bool {
        workingCopy != savedState
    }

    /// Default implementation: Revert working copy to saved state
    func cancel() {
        // Note: This requires `workingCopy` to be mutable
        // Implementations must handle this in their struct
    }
}

// MARK: - Validation Helper

extension FormStateful {
    /// Check if form is valid for saving (override in conforming types)
    /// - Returns: `true` if all required fields are valid, `false` otherwise
    func isFormValid() -> Bool {
        // Default: Always valid. Override in conforming types for validation logic.
        return true
    }

    /// Check if form is dirty (has unsaved changes)
    /// - Returns: `true` if working copy differs from saved state
    func isDirty() -> Bool {
        return hasUnsavedChanges
    }
}

// MARK: - Error Types

/// Errors that can occur during form state operations
enum FormStateError: LocalizedError {
    /// Save operation failed due to disk write error
    case saveFailedDiskWrite(String)

    /// Save operation failed due to invalid configuration
    case saveFailedInvalidConfig(String)

    /// Load operation failed
    case loadFailed(String)

    /// Validation failed
    case validationFailed(String)

    var errorDescription: String? {
        switch self {
        case .saveFailedDiskWrite(let message):
            return "Failed to save: \(message)"
        case .saveFailedInvalidConfig(let message):
            return "Invalid configuration: \(message)"
        case .loadFailed(let message):
            return "Failed to load: \(message)"
        case .validationFailed(let message):
            return "Validation error: \(message)"
        }
    }
}
