//
//  HaloState.swift
//  Aether
//
//  Simplified Halo state enum without theme support.
//  Success state has been removed - AI response completion is implicit.
//

import SwiftUI

/// Halo overlay states (simplified, no themes)
enum HaloState {
    /// Hidden - Halo is not visible
    case idle

    /// Listening for clipboard/input
    case listening

    /// Retrieving context from memory
    case retrievingMemory

    /// AI is processing the request (unified - no provider color)
    case processingWithAI(providerName: String?)

    /// General processing with optional streaming text preview
    case processing(streamingText: String?)

    /// Typewriter output in progress
    case typewriting(progress: Float)

    // NOTE: success state REMOVED - AI response completion is implicit

    /// Error state with retry/dismiss actions
    case error(type: ErrorType, message: String, suggestion: String?)

    /// Permission required (deprecated - use PermissionGateView)
    case permissionRequired(type: HaloPermissionType)

    /// Toast notification
    case toast(type: ToastType, title: String, message: String, autoDismiss: Bool)

    /// Clarification needed (phantom flow)
    case clarification(request: ClarificationRequest)

    /// Multi-turn conversation input
    case conversationInput(sessionId: String, turnCount: UInt32)

    /// Tool confirmation dialog
    case toolConfirmation(
        confirmationId: String,
        toolName: String,
        toolDescription: String,
        reason: String,
        confidence: Float
    )

    // MARK: - State Query Helpers

    /// Check if state is toast
    var isToast: Bool {
        if case .toast = self { return true }
        return false
    }

    /// Check if state is tool confirmation
    var isToolConfirmation: Bool {
        if case .toolConfirmation = self { return true }
        return false
    }

    /// Check if state is processing (any kind)
    var isProcessing: Bool {
        switch self {
        case .processing, .processingWithAI, .retrievingMemory:
            return true
        default:
            return false
        }
    }

    /// Check if state is conversation input
    var isConversationInput: Bool {
        if case .conversationInput = self { return true }
        return false
    }
}

// MARK: - Supporting Types

// MARK: - Equatable Conformance

extension HaloState: Equatable {
    static func == (lhs: HaloState, rhs: HaloState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle):
            return true
        case (.listening, .listening):
            return true
        case (.retrievingMemory, .retrievingMemory):
            return true
        case (.processingWithAI(let lName), .processingWithAI(let rName)):
            return lName == rName
        case (.processing(let lText), .processing(let rText)):
            return lText == rText
        case (.typewriting(let lProgress), .typewriting(let rProgress)):
            return lProgress == rProgress
        case (.error(let lType, let lMsg, let lSug), .error(let rType, let rMsg, let rSug)):
            return lType == rType && lMsg == rMsg && lSug == rSug
        case (.permissionRequired(let lType), .permissionRequired(let rType)):
            return lType == rType  // HaloPermissionType is Equatable
        case (.toast(let lType, let lTitle, let lMsg, let lAuto), .toast(let rType, let rTitle, let rMsg, let rAuto)):
            return lType == rType && lTitle == rTitle && lMsg == rMsg && lAuto == rAuto
        case (.clarification(let lReq), .clarification(let rReq)):
            return lReq == rReq
        case (.conversationInput(let lSid, let lCount), .conversationInput(let rSid, let rCount)):
            return lSid == rSid && lCount == rCount
        case (.toolConfirmation(let lId, let lName, let lDesc, let lReason, let lConf),
              .toolConfirmation(let rId, let rName, let rDesc, let rReason, let rConf)):
            return lId == rId && lName == rName && lDesc == rDesc && lReason == rReason && lConf == rConf
        default:
            return false
        }
    }
}

/// Toast notification types
enum ToastType: Equatable {
    case info
    case warning
    case error

    /// Display name for accessibility
    var displayName: String {
        switch self {
        case .info: return L("toast.type.info")
        case .warning: return L("toast.type.warning")
        case .error: return L("toast.type.error")
        }
    }

    /// SF Symbol icon name
    var iconName: String {
        switch self {
        case .info: return "info.circle.fill"
        case .warning: return "exclamationmark.triangle.fill"
        case .error: return "xmark.circle.fill"
        }
    }

    /// Accent color for the toast type
    var accentColor: Color {
        switch self {
        case .info: return .blue
        case .warning: return .orange
        case .error: return .red
        }
    }
}

/// Permission types for Halo UI (deprecated, use PermissionGateView instead)
enum HaloPermissionType: Equatable {
    case accessibility
    case inputMonitoring
}

/// Halo state callbacks (stored separately for Equatable synthesis)
class HaloStateCallbacks {
    var toastOnDismiss: (() -> Void)?
    var toolConfirmationOnExecute: (() -> Void)?
    var toolConfirmationOnCancel: (() -> Void)?

    func reset() {
        toastOnDismiss = nil
        toolConfirmationOnExecute = nil
        toolConfirmationOnCancel = nil
    }
}
