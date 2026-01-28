//
//  PlanStep.swift
//  Aether
//
//  Model for tracking execution plan steps in multi-turn conversations.
//

import Foundation

/// A step in an execution plan
struct PlanStep: Identifiable, Equatable, Sendable {
    /// Unique identifier
    let id: String
    /// Human-readable description of the step
    let description: String
    /// Current status of the step
    var status: StepStatus
    /// Dependencies (step IDs that must complete before this step)
    let dependencies: [String]

    init(id: String, description: String, status: StepStatus = .pending, dependencies: [String] = []) {
        self.id = id
        self.description = description
        self.status = status
        self.dependencies = dependencies
    }

    /// Parse from JSON dictionary
    static func fromJSON(_ json: [String: Any]) -> PlanStep? {
        guard let id = json["step_id"] as? String,
              let description = json["description"] as? String else {
            return nil
        }

        let statusStr = (json["status"] as? String) ?? "pending"
        let status = StepStatus(rawValue: statusStr) ?? .pending
        let dependencies = json["dependencies"] as? [String] ?? []

        return PlanStep(
            id: id,
            description: description,
            status: status,
            dependencies: dependencies
        )
    }
}

/// Status of a plan step
enum StepStatus: String, Equatable, Sendable {
    /// Step is waiting to be executed
    case pending
    /// Step is currently executing
    case running
    /// Step completed successfully
    case completed
    /// Step failed
    case failed

    /// Icon for display
    var icon: String {
        switch self {
        case .pending: return "circle"
        case .running: return "arrow.trianglehead.2.clockwise"
        case .completed: return "checkmark.circle.fill"
        case .failed: return "xmark.circle.fill"
        }
    }

    /// Color for display
    var color: String {
        switch self {
        case .pending: return "gray"
        case .running: return "green"
        case .completed: return "green"
        case .failed: return "red"
        }
    }
}
