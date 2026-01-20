//
//  PlanStep.swift
//  Aether
//
//  Model for tracking execution plan steps in multi-turn conversations.
//

import Foundation

/// Status of a plan step
enum StepStatus: Equatable {
    /// Step is waiting to be executed
    case pending
    /// Step is currently executing
    case running
    /// Step completed successfully
    case completed
    /// Step failed
    case failed
}

/// A step in an execution plan
struct PlanStep: Identifiable, Equatable {
    /// Unique identifier
    let id: String
    /// Human-readable description of the step
    let description: String
    /// Current status of the step
    var status: StepStatus

    init(id: String, description: String, status: StepStatus = .pending) {
        self.id = id
        self.description = description
        self.status = status
    }
}
