//
//  HaloLegacyTypes.swift
//  Aleph
//
//  Legacy types for backwards compatibility with old Halo components.
//  These types are used by PlanProgressView, PlanConfirmationView, AgentPlanView,
//  and EventHandler's agentic plan handling.
//
//  TODO: Migrate to V2 streaming model and remove these types.
//

import SwiftUI

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
