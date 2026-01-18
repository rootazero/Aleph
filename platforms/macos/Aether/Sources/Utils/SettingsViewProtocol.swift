//
//  SettingsViewProtocol.swift
//  Aether
//
//  Shared state for settings views to communicate with RootContentView's UnifiedSaveBar
//

import Foundation
import Combine

/// Shared observable state for settings save bar
/// Each settings view updates this to control the unified save bar in RootContentView
class SettingsSaveBarState: ObservableObject {
    // Published properties that RootContentView subscribes to
    @Published var hasUnsavedChanges: Bool = false
    @Published var isSaving: Bool = false
    @Published var statusMessage: String? = nil

    // Closures that settings views populate
    var onSave: (() async -> Void)?
    var onCancel: (() -> Void)?

    /// Reset state (called when switching tabs)
    func reset() {
        hasUnsavedChanges = false
        isSaving = false
        statusMessage = nil
        onSave = nil
        onCancel = nil
    }

    /// Update state from settings view
    func update(
        hasUnsavedChanges: Bool,
        isSaving: Bool = false,
        statusMessage: String? = nil,
        onSave: (() async -> Void)? = nil,
        onCancel: (() -> Void)? = nil
    ) {
        self.hasUnsavedChanges = hasUnsavedChanges
        self.isSaving = isSaving
        self.statusMessage = statusMessage
        if let onSave = onSave {
            self.onSave = onSave
        }
        if let onCancel = onCancel {
            self.onCancel = onCancel
        }
    }
}
