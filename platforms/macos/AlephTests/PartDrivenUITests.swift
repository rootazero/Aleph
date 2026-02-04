import XCTest
@testable import Aleph

/// Unit tests for Part-driven UI system (Phase 7 - Task 3)
///
/// Tests verify:
/// 1. Part serialization/deserialization from JSON
/// 2. ViewModel Part update logic (added/updated/removed events)
/// 3. Configuration filtering logic
/// 4. Complex task flow (reasoning → plan → tool → complete)
/// 5. Edge cases (empty data, malformed JSON, concurrent updates)
final class PartDrivenUITests: XCTestCase {

    // MARK: - Part Model Tests

    /// Test: ReasoningPart JSON parsing
    func testReasoningPartFromJSON() throws {
        let json: [String: Any] = [
            "content": "Analyzing the problem...",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        ]

        let part = ReasoningPart.fromJSON(json)

        XCTAssertNotNil(part)
        XCTAssertEqual(part?.content, "Analyzing the problem...")
        XCTAssertEqual(part?.step, 1)
        XCTAssertEqual(part?.isComplete, false)
        XCTAssertEqual(part?.timestamp, 1234567890123)
        XCTAssertNotNil(part?.id)
    }

    /// Test: ReasoningPart with missing fields should fail
    func testReasoningPartInvalidJSON() {
        let invalidJSON: [String: Any] = [
            "content": "Test",
            // Missing: step, is_complete, timestamp
        ]

        let part = ReasoningPart.fromJSON(invalidJSON)
        XCTAssertNil(part, "ReasoningPart should fail to parse with missing fields")
    }

    /// Test: PlanPart JSON parsing with multiple steps
    func testPlanPartFromJSON() throws {
        let json: [String: Any] = [
            "plan_id": "plan-123",
            "requires_confirmation": true,
            "created_at": 1234567890123,
            "steps": [
                [
                    "step_id": "step-1",
                    "description": "Read file",
                    "status": "pending",
                    "dependencies": []
                ],
                [
                    "step_id": "step-2",
                    "description": "Process data",
                    "status": "running",
                    "dependencies": ["step-1"]
                ],
                [
                    "step_id": "step-3",
                    "description": "Write output",
                    "status": "completed",
                    "dependencies": ["step-2"]
                ]
            ]
        ]

        let part = PlanPart.fromJSON(json)

        XCTAssertNotNil(part)
        XCTAssertEqual(part?.id, "plan-123")
        XCTAssertEqual(part?.requiresConfirmation, true)
        XCTAssertEqual(part?.steps.count, 3)

        let step1 = part?.steps[0]
        XCTAssertEqual(step1?.id, "step-1")
        XCTAssertEqual(step1?.description, "Read file")
        XCTAssertEqual(step1?.status, .pending)
        XCTAssertEqual(step1?.dependencies.count, 0)

        let step2 = part?.steps[1]
        XCTAssertEqual(step2?.status, .running)
        XCTAssertEqual(step2?.dependencies, ["step-1"])

        let step3 = part?.steps[2]
        XCTAssertEqual(step3?.status, .completed)
    }

    /// Test: PlanPart with empty steps
    func testPlanPartEmptySteps() {
        let json: [String: Any] = [
            "plan_id": "plan-empty",
            "requires_confirmation": false,
            "created_at": 1234567890123,
            "steps": []
        ]

        let part = PlanPart.fromJSON(json)

        XCTAssertNotNil(part)
        XCTAssertEqual(part?.steps.count, 0)
    }

    /// Test: PlanStep status enum parsing
    func testPlanStepStatusParsing() {
        let statusValues: [(String, PlanStep.StepStatus)] = [
            ("pending", .pending),
            ("running", .running),
            ("completed", .completed),
            ("failed", .failed),
            ("unknown", .pending)  // Unknown status should default to pending
        ]

        for (statusStr, expected) in statusValues {
            let json: [String: Any] = [
                "step_id": "test",
                "description": "Test step",
                "status": statusStr,
                "dependencies": []
            ]

            if let step = PlanStep.fromJSON(json) {
                XCTAssertEqual(step.status, expected, "Status '\(statusStr)' should parse to \(expected)")
            }
        }
    }

    // MARK: - ViewModel Part Update Tests

    /// Test: ReasoningPart added event
    @MainActor
    func testReasoningPartAddedEvent() async throws {
        let viewModel = UnifiedConversationViewModel()

        // Enable reasoning display
        viewModel.updatePartDisplayConfig(showReasoning: true)

        // Simulate Part added event
        let partJSON = """
        {
            "content": "Thinking about solution...",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        }
        """

        let event = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "reasoning_1",
            partJson: partJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )

        viewModel.handlePartUpdate(event: event)

        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)
        XCTAssertEqual(viewModel.activeReasoningParts[0].step, 1)
        XCTAssertEqual(viewModel.activeReasoningParts[0].content, "Thinking about solution...")
    }

    /// Test: ReasoningPart updated event
    @MainActor
    func testReasoningPartUpdatedEvent() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showReasoning: true)

        // Add initial part
        let addJSON = """
        {
            "content": "Initial thought",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        }
        """
        let addEvent = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "reasoning_1",
            partJson: addJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )
        viewModel.handlePartUpdate(event: addEvent)

        // Update part
        let updateJSON = """
        {
            "content": "Updated thought with more details",
            "step": 1,
            "is_complete": true,
            "timestamp": 1234567890124
        }
        """
        let updateEvent = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "reasoning_1",
            partJson: updateJSON,
            delta: nil,
            eventType: .updated,
            timestamp: 1234567890124
        )
        viewModel.handlePartUpdate(event: updateEvent)

        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)
        XCTAssertEqual(viewModel.activeReasoningParts[0].content, "Updated thought with more details")
        XCTAssertEqual(viewModel.activeReasoningParts[0].isComplete, true)
    }

    /// Test: ReasoningPart removed event
    @MainActor
    func testReasoningPartRemovedEvent() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showReasoning: true)

        // Add part
        let addJSON = """
        {
            "content": "Test",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        }
        """
        let addEvent = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "reasoning_1",
            partJson: addJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )
        viewModel.handlePartUpdate(event: addEvent)
        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)

        // Remove part
        let removeEvent = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "reasoning_1",
            partJson: addJSON,
            delta: nil,
            eventType: .removed,
            timestamp: 1234567890124
        )
        viewModel.handlePartUpdate(event: removeEvent)

        XCTAssertEqual(viewModel.activeReasoningParts.count, 0)
    }

    /// Test: PlanPart added event
    @MainActor
    func testPlanPartAddedEvent() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showPlan: true)

        let partJSON = """
        {
            "plan_id": "plan-test",
            "requires_confirmation": false,
            "created_at": 1234567890123,
            "steps": [
                {
                    "step_id": "step-1",
                    "description": "First step",
                    "status": "pending",
                    "dependencies": []
                }
            ]
        }
        """

        let event = PartUpdateEventFfi(
            partType: "plan",
            partId: "plan-test",
            partJson: partJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )

        viewModel.handlePartUpdate(event: event)

        XCTAssertEqual(viewModel.activePlanParts.count, 1)
        XCTAssertEqual(viewModel.activePlanParts[0].id, "plan-test")
        XCTAssertEqual(viewModel.activePlanParts[0].steps.count, 1)
        XCTAssertEqual(viewModel.activePlanParts[0].steps[0].description, "First step")
    }

    // MARK: - Configuration Filtering Tests

    /// Test: ReasoningPart filtered when disabled
    @MainActor
    func testReasoningPartFilteredWhenDisabled() async throws {
        let viewModel = UnifiedConversationViewModel()

        // Disable reasoning display (default)
        viewModel.updatePartDisplayConfig(showReasoning: false)

        let partJSON = """
        {
            "content": "Should not appear",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        }
        """

        let event = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "reasoning_1",
            partJson: partJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )

        viewModel.handlePartUpdate(event: event)

        // Part should NOT be added due to config filter
        XCTAssertEqual(viewModel.activeReasoningParts.count, 0)
    }

    /// Test: PlanPart filtered when disabled
    @MainActor
    func testPlanPartFilteredWhenDisabled() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showPlan: false)

        let partJSON = """
        {
            "plan_id": "plan-hidden",
            "requires_confirmation": false,
            "created_at": 1234567890123,
            "steps": []
        }
        """

        let event = PartUpdateEventFfi(
            partType: "plan",
            partId: "plan-hidden",
            partJson: partJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )

        viewModel.handlePartUpdate(event: event)

        XCTAssertEqual(viewModel.activePlanParts.count, 0)
    }

    /// Test: ToolCall pruning respects maxRecentToolCalls config
    @MainActor
    func testToolCallPruningWithConfig() async throws {
        let viewModel = UnifiedConversationViewModel()

        // Set max to 2
        viewModel.updatePartDisplayConfig(maxRecentToolCalls: 2)

        // Simulate 5 completed tool calls
        for i in 1...5 {
            let toolJSON = """
            {
                "id": "tool-\(i)",
                "tool_name": "read_file",
                "status": "completed",
                "started_at": \(1234567890000 + i * 1000),
                "completed_at": \(1234567890000 + i * 1000 + 500),
                "duration_ms": 500,
                "inputs": {"path": "/test.txt"},
                "output": "File content",
                "error": null
            }
            """

            let addEvent = PartUpdateEventFfi(
                partType: "tool_call",
                partId: "tool-\(i)",
                partJson: toolJSON,
                delta: nil,
                eventType: .added,
                timestamp: Int64(1234567890000 + i * 1000)
            )
            viewModel.handlePartUpdate(event: addEvent)

            let updateEvent = PartUpdateEventFfi(
                partType: "tool_call",
                partId: "tool-\(i)",
                partJson: toolJSON,
                delta: nil,
                eventType: .updated,
                timestamp: Int64(1234567890000 + i * 1000 + 500)
            )
            viewModel.handlePartUpdate(event: updateEvent)
        }

        // Should only keep most recent 2 terminal tool calls
        XCTAssertEqual(viewModel.activeToolCalls.count, 2)

        // Verify they are the most recent (tool-4 and tool-5)
        let toolIds = Set(viewModel.activeToolCalls.map { $0.id })
        XCTAssertTrue(toolIds.contains("tool-4"))
        XCTAssertTrue(toolIds.contains("tool-5"))
    }

    /// Test: Config reload clears filtered parts immediately
    @MainActor
    func testConfigReloadClearsFilteredParts() async throws {
        let viewModel = UnifiedConversationViewModel()

        // Enable all
        viewModel.updatePartDisplayConfig(showReasoning: true, showPlan: true, showToolCalls: true)

        // Add parts
        let reasoningJSON = """
        {
            "content": "Test reasoning",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        }
        """
        let reasoningEvent = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "r1",
            partJson: reasoningJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )
        viewModel.handlePartUpdate(event: reasoningEvent)

        let planJSON = """
        {
            "plan_id": "p1",
            "requires_confirmation": false,
            "created_at": 1234567890123,
            "steps": []
        }
        """
        let planEvent = PartUpdateEventFfi(
            partType: "plan",
            partId: "p1",
            partJson: planJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )
        viewModel.handlePartUpdate(event: planEvent)

        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)
        XCTAssertEqual(viewModel.activePlanParts.count, 1)

        // Disable reasoning and plan
        viewModel.updatePartDisplayConfig(showReasoning: false, showPlan: false)

        // Parts should be cleared immediately
        XCTAssertEqual(viewModel.activeReasoningParts.count, 0)
        XCTAssertEqual(viewModel.activePlanParts.count, 0)
    }

    // MARK: - Complex Task Flow Tests

    /// Test: Complete task flow (reasoning → plan → tool → complete)
    @MainActor
    func testCompleteTaskFlow() async throws {
        let viewModel = UnifiedConversationViewModel()

        // Enable all parts
        viewModel.updatePartDisplayConfig(showReasoning: true, showPlan: true, showToolCalls: true)

        // 1. Add reasoning part
        let reasoningJSON = """
        {
            "content": "Analyzing the task...",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890100
        }
        """
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "reasoning",
            partId: "r1",
            partJson: reasoningJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890100
        ))
        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)

        // 2. Add plan part
        let planJSON = """
        {
            "plan_id": "plan-1",
            "requires_confirmation": false,
            "created_at": 1234567890200,
            "steps": [
                {
                    "step_id": "s1",
                    "description": "Read config",
                    "status": "pending",
                    "dependencies": []
                },
                {
                    "step_id": "s2",
                    "description": "Process data",
                    "status": "pending",
                    "dependencies": ["s1"]
                }
            ]
        }
        """
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "plan",
            partId: "plan-1",
            partJson: planJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890200
        ))
        XCTAssertEqual(viewModel.activePlanParts.count, 1)
        XCTAssertEqual(viewModel.activePlanParts[0].steps.count, 2)

        // 3. Add tool call
        let toolJSON = """
        {
            "id": "tool-1",
            "tool_name": "read_file",
            "status": "running",
            "started_at": 1234567890300,
            "completed_at": null,
            "duration_ms": null,
            "inputs": {"path": "/config.toml"},
            "output": null,
            "error": null
        }
        """
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "tool_call",
            partId: "tool-1",
            partJson: toolJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890300
        ))
        XCTAssertEqual(viewModel.activeToolCalls.count, 1)

        // 4. Complete tool call
        let toolCompletedJSON = """
        {
            "id": "tool-1",
            "tool_name": "read_file",
            "status": "completed",
            "started_at": 1234567890300,
            "completed_at": 1234567890400,
            "duration_ms": 100,
            "inputs": {"path": "/config.toml"},
            "output": "Config content...",
            "error": null
        }
        """
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "tool_call",
            partId: "tool-1",
            partJson: toolCompletedJSON,
            delta: nil,
            eventType: .updated,
            timestamp: 1234567890400
        ))

        // Verify flow state
        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)
        XCTAssertEqual(viewModel.activePlanParts.count, 1)
        XCTAssertEqual(viewModel.activeToolCalls.count, 1)
        XCTAssertEqual(viewModel.activeToolCalls[0].status, .completed)
    }

    // MARK: - Edge Cases

    /// Test: Malformed JSON should not crash
    @MainActor
    func testMalformedJSONHandling() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showReasoning: true)

        let malformedJSON = "{ invalid json }"

        let event = PartUpdateEventFfi(
            partType: "reasoning",
            partId: "bad",
            partJson: malformedJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )

        // Should not crash, just log and ignore
        viewModel.handlePartUpdate(event: event)

        XCTAssertEqual(viewModel.activeReasoningParts.count, 0)
    }

    /// Test: Empty Part list after clear
    @MainActor
    func testClearActiveParts() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showReasoning: true, showPlan: true, showToolCalls: true)

        // Add multiple parts
        let reasoningJSON = """
        {
            "content": "Test",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890123
        }
        """
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "reasoning",
            partId: "r1",
            partJson: reasoningJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        ))

        XCTAssertGreaterThan(viewModel.activeReasoningParts.count, 0)

        // Clear all
        viewModel.clearActiveParts()

        XCTAssertEqual(viewModel.activeReasoningParts.count, 0)
        XCTAssertEqual(viewModel.activePlanParts.count, 0)
        XCTAssertEqual(viewModel.activeToolCalls.count, 0)
    }

    /// Test: Unknown part type should be ignored gracefully
    @MainActor
    func testUnknownPartType() async throws {
        let viewModel = UnifiedConversationViewModel()

        let unknownJSON = """
        {
            "some_field": "value"
        }
        """

        let event = PartUpdateEventFfi(
            partType: "unknown_part_type",
            partId: "unknown-1",
            partJson: unknownJSON,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890123
        )

        // Should not crash
        viewModel.handlePartUpdate(event: event)

        // No parts should be added
        XCTAssertEqual(viewModel.activeReasoningParts.count, 0)
        XCTAssertEqual(viewModel.activePlanParts.count, 0)
    }

    /// Test: Concurrent updates to same part
    @MainActor
    func testConcurrentPartUpdates() async throws {
        let viewModel = UnifiedConversationViewModel()
        viewModel.updatePartDisplayConfig(showReasoning: true)

        let json1 = """
        {
            "content": "Update 1",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890100
        }
        """

        let json2 = """
        {
            "content": "Update 2",
            "step": 1,
            "is_complete": false,
            "timestamp": 1234567890200
        }
        """

        // Add part
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "reasoning",
            partId: "r1",
            partJson: json1,
            delta: nil,
            eventType: .added,
            timestamp: 1234567890100
        ))

        // Update same part
        viewModel.handlePartUpdate(event: PartUpdateEventFfi(
            partType: "reasoning",
            partId: "r1",
            partJson: json2,
            delta: nil,
            eventType: .updated,
            timestamp: 1234567890200
        ))

        // Should only have one part with latest content
        XCTAssertEqual(viewModel.activeReasoningParts.count, 1)
        XCTAssertEqual(viewModel.activeReasoningParts[0].content, "Update 2")
    }
}
