//
//  PendingPlanConfirmation.swift
//  Aether
//
//  Model for pending DAG plan confirmation shown inline in conversation.
//

import Foundation

/// A task in the pending plan confirmation
struct PendingPlanTask: Identifiable, Equatable, Sendable {
    /// Unique identifier
    let id: String
    /// Human-readable task name
    let name: String
    /// Risk level ("low" or "high")
    let riskLevel: String

    /// Whether this is a high-risk task
    var isHighRisk: Bool {
        riskLevel == "high"
    }
}

/// Pending plan confirmation to be shown inline in conversation
struct PendingPlanConfirmation: Identifiable, Equatable, Sendable {
    /// Plan identifier for confirmation callback
    let planId: String
    /// Human-readable plan title
    let title: String
    /// List of tasks in the plan
    let tasks: [PendingPlanTask]
    /// Whether the plan has any high-risk tasks
    var hasHighRiskTasks: Bool {
        tasks.contains { $0.isHighRisk }
    }

    /// Identifiable conformance
    var id: String { planId }

    /// Initialize from EventHandler.PlanConfirmationInfo
    init(planId: String, title: String, tasks: [(id: String, name: String, riskLevel: String)]) {
        self.planId = planId
        self.title = title
        self.tasks = tasks.map { PendingPlanTask(id: $0.id, name: $0.name, riskLevel: $0.riskLevel) }
    }
}
