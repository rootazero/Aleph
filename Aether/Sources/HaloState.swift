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

    /// Plan confirmation dialog (multi-step execution)
    case planConfirmation(planInfo: PlanDisplayInfo)

    /// Plan execution progress (multi-step execution)
    case planProgress(progressInfo: PlanProgressInfo)

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

    /// Check if state is plan confirmation
    var isPlanConfirmation: Bool {
        if case .planConfirmation = self { return true }
        return false
    }

    /// Check if state is plan progress
    var isPlanProgress: Bool {
        if case .planProgress = self { return true }
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
        case (.planConfirmation(let lInfo), .planConfirmation(let rInfo)):
            return lInfo == rInfo
        case (.planProgress(let lInfo), .planProgress(let rInfo)):
            return lInfo == rInfo
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
    var planConfirmationOnExecute: (() -> Void)?
    var planConfirmationOnCancel: (() -> Void)?

    func reset() {
        toastOnDismiss = nil
        toolConfirmationOnExecute = nil
        toolConfirmationOnCancel = nil
        planConfirmationOnExecute = nil
        planConfirmationOnCancel = nil
    }
}

// MARK: - Plan Display Info

/// Information needed to display plan confirmation UI
struct PlanDisplayInfo: Equatable {
    /// Plan ID for tracking
    let planId: String

    /// Human-readable plan description
    let description: String

    /// Steps in the plan
    let steps: [PlanStepDisplayInfo]

    /// Whether plan contains irreversible operations
    let hasIrreversibleSteps: Bool

    /// Overall confidence score (0.0-1.0)
    let confidence: Float
}

/// Step information for display
struct PlanStepDisplayInfo: Equatable {
    /// Step index (1-based for display)
    let index: UInt32

    /// Tool name
    let toolName: String

    /// Step description
    let description: String

    /// Safety level label (e.g., "Read Only", "High Risk")
    let safetyLevel: String

    /// Whether this step is irreversible
    var isIrreversible: Bool {
        safetyLevel == "Low Risk" || safetyLevel == "High Risk"
    }

    /// Icon name for safety level
    var safetyIcon: String {
        switch safetyLevel {
        case "Read Only": return "eye"
        case "Reversible": return "arrow.uturn.backward"
        case "Low Risk": return "exclamationmark.circle"
        case "High Risk": return "exclamationmark.triangle.fill"
        default: return "questionmark.circle"
        }
    }

    /// Color for safety level
    var safetyColor: Color {
        switch safetyLevel {
        case "Read Only": return .green
        case "Reversible": return .blue
        case "Low Risk": return .orange
        case "High Risk": return .red
        default: return .gray
        }
    }
}

// MARK: - Plan Progress Info

/// Information needed to display plan execution progress
struct PlanProgressInfo: Equatable {
    /// Plan ID for tracking
    let planId: String

    /// Human-readable plan description
    let description: String

    /// Total number of steps
    let totalSteps: UInt32

    /// Current step index (0-based)
    let currentStep: UInt32

    /// Current step name
    let currentStepName: String

    /// Progress of all steps
    let stepProgress: [PlanStepProgressInfo]

    /// Overall status
    let status: PlanExecutionStatus

    /// Error message (if status is .failed)
    let errorMessage: String?
}

/// Progress information for a single plan step
struct PlanStepProgressInfo: Equatable {
    /// Step index (1-based for display)
    let index: UInt32

    /// Tool name
    let toolName: String

    /// Step description
    let description: String

    /// Step status
    let status: PlanStepStatus

    /// Result preview (if completed)
    let resultPreview: String?

    /// Error message (if failed)
    let errorMessage: String?
}

/// Status of plan execution
enum PlanExecutionStatus: Equatable {
    case running
    case completed
    case failed
    case cancelled
}

/// Status of a single step
enum PlanStepStatus: Equatable {
    case pending
    case running
    case completed
    case failed
    case skipped
}
