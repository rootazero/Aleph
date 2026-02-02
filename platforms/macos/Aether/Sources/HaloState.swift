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

    /// Success state with checkmark (for OCR and other quick operations)
    case success(message: String?)

    /// Error state with retry/dismiss actions
    case error(type: ErrorType, message: String, suggestion: String?)

    /// Toast notification
    case toast(type: ToastType, title: String, message: String, autoDismiss: Bool, actionTitle: String?)

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

    /// Task graph confirmation (multi-task orchestration)
    case taskGraphConfirmation(taskGraph: AgentTaskGraphFfi)

    /// Agent task graph execution progress
    case taskGraphProgress(taskGraph: AgentTaskGraphFfi, state: AgentExecutionState)

    /// Agent plan confirmation (Cursor-style)
    case agentPlan(
        planId: String,
        title: String,
        operations: [AgentOperation],
        summary: AgentPlanSummary
    )

    /// Agent execution progress
    case agentProgress(
        planId: String,
        progress: Float,
        currentOperation: String,
        completedCount: Int,
        totalCount: Int
    )

    /// Agent conflict resolution
    case agentConflict(
        planId: String,
        fileName: String,
        targetPath: String,
        applyToAll: Bool
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

    /// Check if state is task graph confirmation
    var isTaskGraphConfirmation: Bool {
        if case .taskGraphConfirmation = self { return true }
        return false
    }

    /// Check if state is task graph progress
    var isTaskGraphProgress: Bool {
        if case .taskGraphProgress = self { return true }
        return false
    }

    /// Check if state is agent plan
    var isAgentPlan: Bool {
        if case .agentPlan = self { return true }
        return false
    }

    /// Check if state is agent progress
    var isAgentProgress: Bool {
        if case .agentProgress = self { return true }
        return false
    }

    /// Check if state is agent conflict
    var isAgentConflict: Bool {
        if case .agentConflict = self { return true }
        return false
    }
}

// MARK: - Supporting Types

// MARK: - Agent Operation Types

/// Single operation in agent plan
struct AgentOperation: Equatable {
    /// Operation action type (e.g., "create_folder", "move_file")
    let action: String
    /// Source path (for move/copy operations)
    let source: String?
    /// Target path or folder
    let target: String

    /// Icon for the action type
    var iconName: String {
        switch action {
        case "create_folder": return "folder.badge.plus"
        case "move_file": return "arrow.right.doc"
        case "copy_file": return "doc.on.doc"
        case "delete_file": return "trash"
        case "rename_file": return "pencil"
        default: return "gearshape"
        }
    }

    /// Localized action description
    var actionDescription: String {
        switch action {
        case "create_folder": return L("agent.action.create_folder")
        case "move_file": return L("agent.action.move_file")
        case "copy_file": return L("agent.action.copy_file")
        case "delete_file": return L("agent.action.delete_file")
        case "rename_file": return L("agent.action.rename_file")
        default: return action
        }
    }
}

/// Summary of agent plan
struct AgentPlanSummary: Equatable {
    /// Number of files affected
    let filesAffected: Int
    /// Number of folders to create
    let foldersToCreate: Int
}

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
        case (.success(let lMsg), .success(let rMsg)):
            return lMsg == rMsg
        case (.error(let lType, let lMsg, let lSug), .error(let rType, let rMsg, let rSug)):
            return lType == rType && lMsg == rMsg && lSug == rSug
        case (.toast(let lType, let lTitle, let lMsg, let lAuto, let lAction),
              .toast(let rType, let rTitle, let rMsg, let rAuto, let rAction)):
            return lType == rType && lTitle == rTitle && lMsg == rMsg && lAuto == rAuto && lAction == rAction
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
        case (.taskGraphConfirmation(let lGraph), .taskGraphConfirmation(let rGraph)):
            return lGraph == rGraph
        case (.taskGraphProgress(let lGraph, let lState), .taskGraphProgress(let rGraph, let rState)):
            return lGraph == rGraph && lState == rState
        case (.agentPlan(let lId, let lTitle, let lOps, let lSummary),
              .agentPlan(let rId, let rTitle, let rOps, let rSummary)):
            return lId == rId && lTitle == rTitle && lOps == rOps && lSummary == rSummary
        case (.agentProgress(let lId, let lProgress, let lOp, let lCompleted, let lTotal),
              .agentProgress(let rId, let rProgress, let rOp, let rCompleted, let rTotal)):
            return lId == rId && lProgress == rProgress && lOp == rOp && lCompleted == rCompleted && lTotal == rTotal
        case (.agentConflict(let lId, let lFile, let lPath, let lApply),
              .agentConflict(let rId, let rFile, let rPath, let rApply)):
            return lId == rId && lFile == rFile && lPath == rPath && lApply == rApply
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

/// Halo state callbacks (stored separately for Equatable synthesis)
class HaloStateCallbacks {
    var toastOnDismiss: (() -> Void)?
    var toastOnAction: (() -> Void)?
    var toolConfirmationOnExecute: (() -> Void)?
    var toolConfirmationOnCancel: (() -> Void)?
    var planConfirmationOnExecute: (() -> Void)?
    var planConfirmationOnCancel: (() -> Void)?
    var taskGraphConfirmationOnExecute: (() -> Void)?
    var taskGraphConfirmationOnCancel: (() -> Void)?
    var taskGraphOnPause: (() -> Void)?
    var taskGraphOnResume: (() -> Void)?
    var taskGraphOnCancel: (() -> Void)?
    // Agent mode callbacks
    var agentPlanOnExecute: (() -> Void)?
    var agentPlanOnCancel: (() -> Void)?
    var agentConflictOnSkip: (() -> Void)?
    var agentConflictOnRename: (() -> Void)?
    var agentConflictOnOverwrite: (() -> Void)?
    var agentConflictOnApplyToAll: ((Bool) -> Void)?
    var agentOnCancel: (() -> Void)?

    func reset() {
        toastOnDismiss = nil
        toastOnAction = nil
        toolConfirmationOnExecute = nil
        toolConfirmationOnCancel = nil
        planConfirmationOnExecute = nil
        planConfirmationOnCancel = nil
        taskGraphConfirmationOnExecute = nil
        taskGraphConfirmationOnCancel = nil
        taskGraphOnPause = nil
        taskGraphOnResume = nil
        taskGraphOnCancel = nil
        // Reset agent callbacks
        agentPlanOnExecute = nil
        agentPlanOnCancel = nil
        agentConflictOnSkip = nil
        agentConflictOnRename = nil
        agentConflictOnOverwrite = nil
        agentConflictOnApplyToAll = nil
        agentOnCancel = nil
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

// MARK: - ========================================
// MARK: - HaloStateV2 (New Simplified State Model)
// MARK: - ========================================

/// Simplified Halo overlay states (V2)
/// Reduces 16+ states to 7 unified states for cleaner state management.
enum HaloStateV2: Equatable {
    /// Hidden - Halo is not visible
    case idle

    /// Listening for clipboard/input
    case listening

    /// AI streaming response (covers: processingWithAI, processing, retrievingMemory, planProgress)
    case streaming(StreamingContext)

    /// User confirmation required (covers: toolConfirmation, planConfirmation, taskGraphConfirmation, agentPlan, agentConflict)
    case confirmation(ConfirmationContext)

    /// Operation completed (covers: success, typewriting)
    case result(ResultContext)

    /// Error state (covers: error, toast[error type])
    case error(ErrorContext)

    /// History list view (// command)
    case historyList(HistoryListContext)
}

// MARK: - HaloStateV2 State Query Helpers

extension HaloStateV2 {
    /// Check if state is idle
    var isIdle: Bool {
        if case .idle = self { return true }
        return false
    }

    /// Check if state is listening
    var isListening: Bool {
        if case .listening = self { return true }
        return false
    }

    /// Check if state is streaming
    var isStreaming: Bool {
        if case .streaming = self { return true }
        return false
    }

    /// Check if state is confirmation
    var isConfirmation: Bool {
        if case .confirmation = self { return true }
        return false
    }

    /// Check if state is result
    var isResult: Bool {
        if case .result = self { return true }
        return false
    }

    /// Check if state is error
    var isError: Bool {
        if case .error = self { return true }
        return false
    }

    /// Check if state is history list
    var isHistoryList: Bool {
        if case .historyList = self { return true }
        return false
    }

    /// Check if state requires user interaction
    var isInteractive: Bool {
        switch self {
        case .confirmation, .error, .historyList:
            return true
        default:
            return false
        }
    }
}

// MARK: - HaloStateV2 Window Size

extension HaloStateV2 {
    /// Computed window size based on current state
    var windowSize: NSSize {
        switch self {
        case .idle:
            return NSSize(width: 0, height: 0)
        case .listening:
            return NSSize(width: 80, height: 60)
        case .streaming(let ctx):
            switch ctx.phase {
            case .thinking:
                return NSSize(width: 80, height: 60)
            case .responding:
                return NSSize(width: 320, height: 100)
            case .toolExecuting:
                return NSSize(width: 280, height: 120)
            }
        case .confirmation:
            return NSSize(width: 340, height: 280)
        case .result:
            return NSSize(width: 280, height: 80)
        case .error:
            return NSSize(width: 320, height: 200)
        case .historyList:
            return NSSize(width: 380, height: 420)
        }
    }
}

// MARK: - HaloState Migration Helper

extension HaloState {
    /// Convert old HaloState to new HaloStateV2
    /// This helper enables gradual migration from the old state model to the new one.
    func toV2() -> HaloStateV2 {
        switch self {
        case .idle:
            return .idle

        case .listening:
            return .listening

        case .retrievingMemory:
            return .streaming(StreamingContext(
                runId: UUID().uuidString,
                text: "",
                phase: .thinking
            ))

        case .processingWithAI(let providerName):
            return .streaming(StreamingContext(
                runId: UUID().uuidString,
                text: "",
                reasoning: providerName != nil ? "Using \(providerName!)" : nil,
                phase: .thinking
            ))

        case .processing(let streamingText):
            return .streaming(StreamingContext(
                runId: UUID().uuidString,
                text: streamingText ?? "",
                phase: .responding
            ))

        case .typewriting(let progress):
            let summary = ResultSummary.success(
                message: String(format: "%.0f%% complete", progress * 100),
                durationMs: 0,
                finalResponse: ""
            )
            return .result(ResultContext(
                runId: UUID().uuidString,
                summary: summary
            ))

        case .success(let message):
            let summary = ResultSummary.success(
                message: message,
                durationMs: 0,
                finalResponse: message ?? ""
            )
            return .result(ResultContext(
                runId: UUID().uuidString,
                summary: summary
            ))

        case .error(let type, let message, let suggestion):
            return .error(ErrorContext(
                type: type.toHaloErrorType(),
                message: message,
                suggestion: suggestion
            ))

        case .toast(let type, _, let message, _, _):
            // Map error toasts to error state, others to result
            if type == .error {
                return .error(ErrorContext(
                    type: .unknown,
                    message: message
                ))
            } else {
                let status: ResultStatus = type == .warning ? .partial : .success
                let summary = ResultSummary(
                    status: status,
                    message: message,
                    toolsExecuted: 0,
                    tokensUsed: nil,
                    durationMs: 0,
                    finalResponse: message
                )
                return .result(ResultContext(
                    runId: UUID().uuidString,
                    summary: summary
                ))
            }

        case .clarification(let request):
            return .confirmation(ConfirmationContext(
                runId: UUID().uuidString,
                type: .userQuestion,
                title: L("clarification.title"),
                description: request.question,
                options: ConfirmationContext.defaultOptions(for: .userQuestion)
            ))

        case .conversationInput(let sessionId, _):
            // Map to listening since it's waiting for input
            // Note: Conversation context is tracked elsewhere
            return .streaming(StreamingContext(
                runId: sessionId,
                text: "",
                phase: .thinking
            ))

        case .toolConfirmation(let confirmationId, let toolName, let toolDescription, let reason, _):
            return .confirmation(ConfirmationContext(
                runId: confirmationId,
                type: .toolExecution,
                title: toolName,
                description: "\(toolDescription)\n\n\(reason)",
                options: ConfirmationContext.defaultOptions(for: .toolExecution)
            ))

        case .planConfirmation(let planInfo):
            let stepsDescription = planInfo.steps.map { step in
                "\(step.index). \(step.toolName): \(step.description)"
            }.joined(separator: "\n")
            return .confirmation(ConfirmationContext(
                runId: planInfo.planId,
                type: .planApproval,
                title: L("plan.confirmation.title"),
                description: "\(planInfo.description)\n\n\(stepsDescription)",
                options: ConfirmationContext.defaultOptions(for: .planApproval)
            ))

        case .planProgress(let progressInfo):
            var toolCalls = progressInfo.stepProgress.map { step in
                let status: ToolStatus
                switch step.status {
                case .pending: status = .pending
                case .running: status = .running
                case .completed: status = .completed
                case .failed: status = .failed
                case .skipped: status = .completed
                }
                return ToolCallInfo(
                    id: "\(step.index)",
                    name: step.toolName,
                    status: status,
                    progressText: step.resultPreview ?? step.errorMessage
                )
            }
            // Limit tool calls displayed
            if toolCalls.count > StreamingContext.maxToolCalls {
                toolCalls = Array(toolCalls.suffix(StreamingContext.maxToolCalls))
            }
            return .streaming(StreamingContext(
                runId: progressInfo.planId,
                text: progressInfo.description,
                toolCalls: toolCalls,
                phase: .toolExecuting
            ))

        case .taskGraphConfirmation(let taskGraph):
            let tasksDescription = taskGraph.tasks.map { task in
                "- \(task.description)"
            }.joined(separator: "\n")
            return .confirmation(ConfirmationContext(
                runId: taskGraph.id,
                type: .planApproval,
                title: L("taskGraph.confirmation.title"),
                description: tasksDescription,
                options: ConfirmationContext.defaultOptions(for: .planApproval)
            ))

        case .taskGraphProgress(let taskGraph, _):
            let toolCalls = taskGraph.tasks.prefix(StreamingContext.maxToolCalls).map { task in
                ToolCallInfo(
                    id: task.id,
                    name: task.toolName ?? "Task",
                    status: .running,
                    progressText: task.description
                )
            }
            return .streaming(StreamingContext(
                runId: taskGraph.id,
                text: "",
                toolCalls: Array(toolCalls),
                phase: .toolExecuting
            ))

        case .agentPlan(let planId, let title, let operations, _):
            let opsDescription = operations.map { op in
                "- \(op.actionDescription): \(op.target)"
            }.joined(separator: "\n")
            return .confirmation(ConfirmationContext(
                runId: planId,
                type: .planApproval,
                title: title,
                description: opsDescription,
                options: ConfirmationContext.defaultOptions(for: .planApproval)
            ))

        case .agentProgress(let planId, _, let currentOperation, let completedCount, let totalCount):
            return .streaming(StreamingContext(
                runId: planId,
                text: "\(completedCount)/\(totalCount)",
                toolCalls: [ToolCallInfo(
                    id: "current",
                    name: currentOperation,
                    status: .running
                )],
                phase: .toolExecuting
            ))

        case .agentConflict(let planId, let fileName, _, _):
            return .confirmation(ConfirmationContext(
                runId: planId,
                type: .fileConflict,
                title: L("conflict.title"),
                description: fileName,
                options: ConfirmationContext.defaultOptions(for: .fileConflict)
            ))
        }
    }
}

// MARK: - ErrorType to HaloErrorType Conversion

extension ErrorType {
    /// Convert FFI ErrorType to HaloErrorType
    func toHaloErrorType() -> HaloErrorType {
        switch self {
        case .network:
            return .network
        case .permission:
            return .provider  // Map permission to provider (closest match)
        case .quota:
            return .provider  // Map quota to provider (closest match)
        case .timeout:
            return .timeout
        case .unknown:
            return .unknown
        }
    }
}
