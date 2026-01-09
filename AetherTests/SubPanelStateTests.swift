//
//  SubPanelStateTests.swift
//  AetherTests
//
//  Unit tests for SubPanelState component.
//  Part of: refactor-unified-halo-window Phase 7
//

import XCTest
@testable import Aether

/// Unit tests for SubPanelState
///
/// Tests verify:
/// 1. Mode transitions work correctly
/// 2. Height calculations are accurate
/// 3. Navigation in command completion works
/// 4. CLI output operations work correctly
final class SubPanelStateTests: XCTestCase {

    var state: SubPanelState!

    override func setUp() {
        super.setUp()
        state = SubPanelState()
    }

    override func tearDown() {
        state = nil
        super.tearDown()
    }

    // MARK: - Initial State Tests

    /// Test: Initial state should be hidden
    func testInitialStateIsHidden() {
        XCTAssertEqual(state.mode, .hidden)
        XCTAssertFalse(state.mode.isVisible)
        XCTAssertEqual(state.calculatedHeight, 0)
    }

    // MARK: - Mode Transition Tests

    /// Test: Transition to command completion mode
    func testShowCommandCompletion() {
        let commands = createMockCommands(count: 3)
        state.showCommandCompletion(commands: commands, inputPrefix: "te")

        if case .commandCompletion(let cmds, let idx, let prefix) = state.mode {
            XCTAssertEqual(cmds.count, 3)
            XCTAssertEqual(idx, 0)
            XCTAssertEqual(prefix, "te")
            XCTAssertTrue(state.mode.isVisible)
        } else {
            XCTFail("Expected commandCompletion mode")
        }
    }

    /// Test: Transition to CLI output mode
    func testShowCLIOutput() {
        state.showCLIOutput()

        if case .cliOutput(let lines, let streaming) = state.mode {
            XCTAssertEqual(lines.count, 0)
            XCTAssertTrue(streaming)
            XCTAssertTrue(state.mode.isVisible)
        } else {
            XCTFail("Expected cliOutput mode")
        }
    }

    /// Test: Transition to selector mode
    func testShowSelector() {
        let options = [
            SelectorOption(label: "Option 1", description: "First option"),
            SelectorOption(label: "Option 2", description: "Second option")
        ]
        state.showSelector(options: options, prompt: "Choose one:", multiSelect: false)

        if case .selector(let opts, let prompt, let multi) = state.mode {
            XCTAssertEqual(opts.count, 2)
            XCTAssertEqual(prompt, "Choose one:")
            XCTAssertFalse(multi)
            XCTAssertTrue(state.mode.isVisible)
        } else {
            XCTFail("Expected selector mode")
        }
    }

    /// Test: Transition to confirmation mode
    func testShowConfirmation() {
        state.showConfirmation(
            title: "Confirm Action",
            message: "Are you sure?",
            confirmLabel: "Yes",
            cancelLabel: "No"
        )

        if case .confirmation(let title, let message, let confirm, let cancel) = state.mode {
            XCTAssertEqual(title, "Confirm Action")
            XCTAssertEqual(message, "Are you sure?")
            XCTAssertEqual(confirm, "Yes")
            XCTAssertEqual(cancel, "No")
            XCTAssertTrue(state.mode.isVisible)
        } else {
            XCTFail("Expected confirmation mode")
        }
    }

    /// Test: Hide transitions to hidden mode
    func testHide() {
        state.showCLIOutput()
        XCTAssertTrue(state.mode.isVisible)

        state.hide()
        XCTAssertFalse(state.mode.isVisible)
        XCTAssertEqual(state.mode, .hidden)
    }

    // MARK: - Command Completion Navigation Tests

    /// Test: Move selection down in command list
    func testMoveSelectionDown() {
        let commands = createMockCommands(count: 3)
        state.showCommandCompletion(commands: commands, inputPrefix: "")

        // Initial selection is 0
        XCTAssertEqual(getSelectedIndex(), 0)

        // Move down
        state.moveSelectionDown()
        XCTAssertEqual(getSelectedIndex(), 1)

        // Move down again
        state.moveSelectionDown()
        XCTAssertEqual(getSelectedIndex(), 2)

        // Wrap around
        state.moveSelectionDown()
        XCTAssertEqual(getSelectedIndex(), 0)
    }

    /// Test: Move selection up in command list
    func testMoveSelectionUp() {
        let commands = createMockCommands(count: 3)
        state.showCommandCompletion(commands: commands, inputPrefix: "")

        // Initial selection is 0
        XCTAssertEqual(getSelectedIndex(), 0)

        // Move up wraps to end
        state.moveSelectionUp()
        XCTAssertEqual(getSelectedIndex(), 2)

        // Move up
        state.moveSelectionUp()
        XCTAssertEqual(getSelectedIndex(), 1)
    }

    /// Test: Get selected command
    func testGetSelectedCommand() {
        let commands = createMockCommands(count: 3)
        state.showCommandCompletion(commands: commands, inputPrefix: "")

        let selected = state.getSelectedCommand()
        XCTAssertNotNil(selected)
        XCTAssertEqual(selected?.key, "cmd0")

        state.moveSelectionDown()
        let selected2 = state.getSelectedCommand()
        XCTAssertEqual(selected2?.key, "cmd1")
    }

    /// Test: Update commands preserves selection when possible
    func testUpdateCommandsPreservesSelection() {
        let commands = createMockCommands(count: 5)
        state.showCommandCompletion(commands: commands, inputPrefix: "")

        // Move to index 2
        state.moveSelectionDown()
        state.moveSelectionDown()
        XCTAssertEqual(getSelectedIndex(), 2)

        // Update with new commands
        let newCommands = createMockCommands(count: 3)
        state.updateCommands(newCommands, inputPrefix: "new")

        // Selection should be preserved at index 2
        XCTAssertEqual(getSelectedIndex(), 2)

        // Update with fewer commands than current selection
        let fewerCommands = createMockCommands(count: 2)
        state.updateCommands(fewerCommands, inputPrefix: "few")

        // Selection should be clamped to max index
        XCTAssertEqual(getSelectedIndex(), 1)
    }

    // MARK: - CLI Output Tests

    /// Test: Append CLI line
    func testAppendCLILine() {
        state.showCLIOutput()

        let line = CLIOutputLine(type: .info, content: "Test message")
        state.appendCLILine(line)

        if case .cliOutput(let lines, _) = state.mode {
            XCTAssertEqual(lines.count, 1)
            XCTAssertEqual(lines[0].content, "Test message")
            XCTAssertEqual(lines[0].type, .info)
        } else {
            XCTFail("Expected cliOutput mode")
        }
    }

    /// Test: Append CLI text convenience method
    func testAppendCLIText() {
        state.showCLIOutput()

        state.appendCLIText("Info message", type: .info)
        state.appendCLIText("Error message", type: .error)
        state.appendCLIText("Success message", type: .success)

        if case .cliOutput(let lines, _) = state.mode {
            XCTAssertEqual(lines.count, 3)
            XCTAssertEqual(lines[0].type, .info)
            XCTAssertEqual(lines[1].type, .error)
            XCTAssertEqual(lines[2].type, .success)
        } else {
            XCTFail("Expected cliOutput mode")
        }
    }

    /// Test: Complete CLI output stops streaming indicator
    func testCompleteCLIOutput() {
        state.showCLIOutput()

        // Initially streaming
        if case .cliOutput(_, let streaming) = state.mode {
            XCTAssertTrue(streaming)
        }

        state.completeCLIOutput()

        // After completion, not streaming
        if case .cliOutput(_, let streaming) = state.mode {
            XCTAssertFalse(streaming)
        } else {
            XCTFail("Expected cliOutput mode")
        }
    }

    /// Test: Clear CLI output
    func testClearCLIOutput() {
        state.showCLIOutput()
        state.appendCLIText("Line 1")
        state.appendCLIText("Line 2")

        if case .cliOutput(let lines, _) = state.mode {
            XCTAssertEqual(lines.count, 2)
        }

        state.clearCLIOutput()

        if case .cliOutput(let lines, _) = state.mode {
            XCTAssertEqual(lines.count, 0)
        } else {
            XCTFail("Expected cliOutput mode")
        }
    }

    /// Test: CLI output line limit (should keep last 100)
    func testCLIOutputLineLimit() {
        state.showCLIOutput()

        // Add 110 lines
        for i in 0..<110 {
            state.appendCLIText("Line \(i)")
        }

        if case .cliOutput(let lines, _) = state.mode {
            XCTAssertEqual(lines.count, 100)
            // First line should be "Line 10" (first 10 removed)
            XCTAssertEqual(lines[0].content, "Line 10")
            // Last line should be "Line 109"
            XCTAssertEqual(lines[99].content, "Line 109")
        } else {
            XCTFail("Expected cliOutput mode")
        }
    }

    // MARK: - Selector Tests

    /// Test: Toggle selector option (single select)
    func testToggleSelectorOptionSingleSelect() {
        let options = [
            SelectorOption(label: "A"),
            SelectorOption(label: "B"),
            SelectorOption(label: "C")
        ]
        state.showSelector(options: options, prompt: "Pick one", multiSelect: false)

        // Select first option
        state.toggleSelectorOption(at: 0)

        if case .selector(let opts, _, _) = state.mode {
            XCTAssertTrue(opts[0].isSelected)
            XCTAssertFalse(opts[1].isSelected)
            XCTAssertFalse(opts[2].isSelected)
        }

        // Select second option (deselects first)
        state.toggleSelectorOption(at: 1)

        if case .selector(let opts, _, _) = state.mode {
            XCTAssertFalse(opts[0].isSelected)
            XCTAssertTrue(opts[1].isSelected)
            XCTAssertFalse(opts[2].isSelected)
        } else {
            XCTFail("Expected selector mode")
        }
    }

    /// Test: Toggle selector option (multi select)
    func testToggleSelectorOptionMultiSelect() {
        let options = [
            SelectorOption(label: "A"),
            SelectorOption(label: "B"),
            SelectorOption(label: "C")
        ]
        state.showSelector(options: options, prompt: "Pick many", multiSelect: true)

        // Select multiple
        state.toggleSelectorOption(at: 0)
        state.toggleSelectorOption(at: 2)

        if case .selector(let opts, _, _) = state.mode {
            XCTAssertTrue(opts[0].isSelected)
            XCTAssertFalse(opts[1].isSelected)
            XCTAssertTrue(opts[2].isSelected)
        }

        // Toggle off
        state.toggleSelectorOption(at: 0)

        if case .selector(let opts, _, _) = state.mode {
            XCTAssertFalse(opts[0].isSelected)
            XCTAssertTrue(opts[2].isSelected)
        } else {
            XCTFail("Expected selector mode")
        }
    }

    /// Test: Get selected options
    func testGetSelectedOptions() {
        let options = [
            SelectorOption(label: "A"),
            SelectorOption(label: "B"),
            SelectorOption(label: "C")
        ]
        state.showSelector(options: options, prompt: "Pick", multiSelect: true)

        state.toggleSelectorOption(at: 0)
        state.toggleSelectorOption(at: 2)

        let selected = state.getSelectedOptions()
        XCTAssertEqual(selected.count, 2)
        XCTAssertEqual(selected[0].label, "A")
        XCTAssertEqual(selected[1].label, "C")
    }

    // MARK: - Height Calculation Tests

    /// Test: Hidden mode has zero height
    func testHiddenModeHeight() {
        XCTAssertEqual(state.calculatedHeight, 0)
    }

    /// Test: Command completion height calculation
    func testCommandCompletionHeight() {
        let commands = createMockCommands(count: 3)
        state.showCommandCompletion(commands: commands, inputPrefix: "")

        // 3 commands * 36 + 40 header = 148
        let expectedHeight = CGFloat(3) * SubPanelState.commandRowHeight + SubPanelState.commandHeaderHeight
        XCTAssertEqual(state.calculatedHeight, expectedHeight)
    }

    /// Test: Command completion height respects max
    func testCommandCompletionMaxHeight() {
        let commands = createMockCommands(count: 20)
        state.showCommandCompletion(commands: commands, inputPrefix: "")

        XCTAssertLessThanOrEqual(state.calculatedHeight, SubPanelState.maxHeight)
    }

    /// Test: Confirmation mode has fixed height
    func testConfirmationHeight() {
        state.showConfirmation(title: "Test", message: "Test")
        XCTAssertEqual(state.calculatedHeight, SubPanelState.confirmationHeight)
    }

    // MARK: - Debug Description Tests

    /// Test: Debug descriptions are meaningful
    func testDebugDescriptions() {
        XCTAssertEqual(SubPanelMode.hidden.debugDescription, "hidden")

        let commands = createMockCommands(count: 2)
        let cmdMode = SubPanelMode.commandCompletion(commands: commands, selectedIndex: 1, inputPrefix: "te")
        XCTAssertTrue(cmdMode.debugDescription.contains("commandCompletion"))
        XCTAssertTrue(cmdMode.debugDescription.contains("2 commands"))

        let cliMode = SubPanelMode.cliOutput(lines: [], isStreaming: true)
        XCTAssertTrue(cliMode.debugDescription.contains("cliOutput"))
        XCTAssertTrue(cliMode.debugDescription.contains("streaming=true"))
    }

    // MARK: - Helper Methods

    private func createMockCommands(count: Int) -> [CommandNode] {
        return (0..<count).map { i in
            CommandNode(
                key: "cmd\(i)",
                description: "Command \(i) description",
                icon: "text.quote",
                hint: "Hint for command \(i)",
                nodeType: .prompt,
                hasChildren: false,
                sourceId: nil
            )
        }
    }

    private func getSelectedIndex() -> Int {
        if case .commandCompletion(_, let idx, _) = state.mode {
            return idx
        }
        return -1
    }
}
