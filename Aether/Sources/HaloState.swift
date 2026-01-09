//
//  HaloState.swift
//  Aether
//
//  State machine for Halo overlay animations.
//

import SwiftUI

// MARK: - HaloState Callbacks

/// Stores closures separately from HaloState to enable automatic Equatable synthesis.
/// Closures cannot be compared, so they must be stored outside the enum.
struct HaloStateCallbacks {
    /// Callback when toast is dismissed
    var toastOnDismiss: (() -> Void)?

    /// Callback when tool confirmation is executed
    var toolConfirmationOnExecute: (() -> Void)?

    /// Callback when tool confirmation is cancelled
    var toolConfirmationOnCancel: (() -> Void)?

    /// Callback when unified input is submitted
    var unifiedInputOnSubmit: ((String) -> Void)?

    /// Callback when unified input is cancelled
    var unifiedInputOnCancel: (() -> Void)?

    /// Callback when command is selected in unified input
    var unifiedInputOnCommandSelected: ((CommandNode) -> Void)?

    /// Reset all callbacks
    mutating func reset() {
        toastOnDismiss = nil
        toolConfirmationOnExecute = nil
        toolConfirmationOnCancel = nil
        unifiedInputOnSubmit = nil
        unifiedInputOnCancel = nil
        unifiedInputOnCommandSelected = nil
    }
}

// MARK: - Toast Types

/// Toast notification types for Halo overlay
enum ToastType: Equatable {
    case info      // Blue accent, info.circle icon - for success/confirmation
    case warning   // Orange accent, exclamationmark.triangle icon - for warnings
    case error     // Red accent, xmark.circle icon - for errors

    /// SF Symbol icon name for each toast type
    var iconName: String {
        switch self {
        case .info:
            return "info.circle.fill"
        case .warning:
            return "exclamationmark.triangle.fill"
        case .error:
            return "xmark.circle.fill"
        }
    }

    /// Accent color for each toast type
    var accentColor: Color {
        switch self {
        case .info:
            return Color(red: 0, green: 0.478, blue: 1.0)  // #007AFF
        case .warning:
            return Color(red: 1.0, green: 0.584, blue: 0)  // #FF9500
        case .error:
            return Color(red: 1.0, green: 0.231, blue: 0.188)  // #FF3B30
        }
    }

    /// Display name for accessibility
    var displayName: String {
        switch self {
        case .info:
            return "Information"
        case .warning:
            return "Warning"
        case .error:
            return "Error"
        }
    }
}

enum HaloState: Equatable {
    case idle
    case listening
    // DEPRECATED: commandMode is replaced by unifiedInput as part of refactor-unified-halo-window
    // Will be removed in Phase 8. Use unifiedInput instead for command completion.
    case commandMode  // Command completion mode (add-command-completion-system) - DEPRECATED
    case retrievingMemory  // Retrieving memories from database
    case processingWithAI(providerColor: Color, providerName: String?)  // AI provider is processing
    case processing(providerColor: Color, streamingText: String? = nil)  // Generic processing
    case typewriting(progress: Float)  // Typewriter animation in progress
    case success(finalText: String? = nil)
    case error(type: ErrorType, message: String, suggestion: String? = nil)  // Error with optional suggestion
    case permissionRequired(type: PermissionType)  // Permission prompt
    case toast(type: ToastType, title: String, message: String, autoDismiss: Bool)  // Toast notification (callbacks in HaloStateCallbacks)
    case clarification(request: ClarificationRequest)  // Phantom Flow clarification (add-phantom-flow-interaction)
    case conversationInput(sessionId: String, turnCount: UInt32)  // Multi-turn conversation input (add-multi-turn-conversation)
    case toolConfirmation(  // Async tool confirmation (Phase 6) - callbacks in HaloStateCallbacks
        confirmationId: String,
        toolName: String,
        toolDescription: String,
        reason: String,
        confidence: Float
    )
    case unifiedInput(  // Unified Halo input (refactor-unified-halo-window)
        sessionId: String,
        turnCount: UInt32,
        subPanelMode: SubPanelMode
    )
    // Note: Equatable is now auto-derived since closures are stored in HaloStateCallbacks
}

// MARK: - Convenience Extensions

extension HaloState {
    /// Check if this is a toast state
    var isToast: Bool {
        if case .toast = self { return true }
        return false
    }

    /// Check if this is a tool confirmation state
    var isToolConfirmation: Bool {
        if case .toolConfirmation = self { return true }
        return false
    }

    /// Check if this is a unified input state
    var isUnifiedInput: Bool {
        if case .unifiedInput = self { return true }
        return false
    }

    /// Get the SubPanelMode if in unifiedInput state
    var subPanelMode: SubPanelMode? {
        if case .unifiedInput(_, _, let mode) = self { return mode }
        return nil
    }
}
