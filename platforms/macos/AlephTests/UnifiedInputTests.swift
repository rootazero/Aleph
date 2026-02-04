//
//  UnifiedInputTests.swift
//  AetherTests
//
//  Unit tests for command input parsing.
//  Part of: refactor-unified-halo-window Phase 7
//

import XCTest
import AppKit
@testable import Aether

/// Unit tests for command input parsing
///
/// Tests verify:
/// 1. Command prefix detection
/// 2. Command key extraction
/// 3. Command content parsing
/// 4. Hotkey configuration parsing
final class CommandParsingTests: XCTestCase {

    // MARK: - Command Prefix Detection Tests

    /// Test: Input starting with "/" is detected as command
    func testCommandPrefixDetection() {
        XCTAssertTrue(isCommandInput("/en hello"))
        XCTAssertTrue(isCommandInput("/translate"))
        XCTAssertTrue(isCommandInput("/"))
        XCTAssertFalse(isCommandInput("hello"))
        XCTAssertFalse(isCommandInput(""))
        XCTAssertFalse(isCommandInput(" /en"))  // Space before slash
    }

    /// Test: Extract command key from input
    func testCommandKeyExtraction() {
        XCTAssertEqual(extractCommandKey("/en hello world"), "en")
        XCTAssertEqual(extractCommandKey("/translate"), "translate")
        XCTAssertEqual(extractCommandKey("/search what is AI"), "search")
        XCTAssertEqual(extractCommandKey("/"), "")
        XCTAssertEqual(extractCommandKey("/ "), "")
    }

    /// Test: Extract command content from input
    func testCommandContentExtraction() {
        XCTAssertEqual(extractCommandContent("/en hello world"), "hello world")
        XCTAssertEqual(extractCommandContent("/translate"), "")
        XCTAssertEqual(extractCommandContent("/search what is AI"), "what is AI")
        XCTAssertEqual(extractCommandContent("/en  multiple  spaces"), " multiple  spaces")
    }

    /// Test: Parse complex command inputs
    func testComplexCommandParsing() {
        // Command with newlines
        let multiline = "/summarize First line\nSecond line"
        XCTAssertEqual(extractCommandKey(multiline), "summarize")
        XCTAssertTrue(extractCommandContent(multiline).contains("\n"))

        // Command with special characters
        let special = "/code print('hello')"
        XCTAssertEqual(extractCommandKey(special), "code")
        XCTAssertEqual(extractCommandContent(special), "print('hello')")

        // Command with unicode
        let unicode = "/zh 你好世界"
        XCTAssertEqual(extractCommandKey(unicode), "zh")
        XCTAssertEqual(extractCommandContent(unicode), "你好世界")
    }

    // MARK: - Hotkey Configuration Parsing Tests

    /// Test: Parse standard hotkey config strings
    func testHotkeyConfigParsing() {
        let (modifiers1, keyCode1) = parseHotkeyConfig("Command+Option+/")
        XCTAssertTrue(modifiers1.contains(.command))
        XCTAssertTrue(modifiers1.contains(.option))
        XCTAssertEqual(keyCode1, 44)  // / key

        let (modifiers2, keyCode2) = parseHotkeyConfig("Command+Shift+Space")
        XCTAssertTrue(modifiers2.contains(.command))
        XCTAssertTrue(modifiers2.contains(.shift))
        XCTAssertEqual(keyCode2, 49)  // Space key

        let (modifiers3, keyCode3) = parseHotkeyConfig("Control+Option+`")
        XCTAssertTrue(modifiers3.contains(.control))
        XCTAssertTrue(modifiers3.contains(.option))
        XCTAssertEqual(keyCode3, 50)  // ` key
    }

    /// Test: Parse hotkey config with various key codes
    func testHotkeyKeyCodes() {
        XCTAssertEqual(parseHotkeyConfig("Command+Option+/").keyCode, 44)
        XCTAssertEqual(parseHotkeyConfig("Command+Option+`").keyCode, 50)
        XCTAssertEqual(parseHotkeyConfig("Command+Option+\\").keyCode, 42)
        XCTAssertEqual(parseHotkeyConfig("Command+Option+;").keyCode, 41)
        XCTAssertEqual(parseHotkeyConfig("Command+Option+,").keyCode, 43)
        XCTAssertEqual(parseHotkeyConfig("Command+Option+.").keyCode, 47)
        XCTAssertEqual(parseHotkeyConfig("Command+Option+Space").keyCode, 49)
    }

    /// Test: Invalid hotkey config falls back to default
    func testInvalidHotkeyConfig() {
        // Too few parts
        let (_, keyCode1) = parseHotkeyConfig("Command")
        XCTAssertEqual(keyCode1, 44)  // Default to /

        // Unknown key
        let (_, keyCode2) = parseHotkeyConfig("Command+Option+UnknownKey")
        XCTAssertEqual(keyCode2, 44)  // Default to /
    }

    // MARK: - Helper Methods

    private func isCommandInput(_ text: String) -> Bool {
        text.hasPrefix("/")
    }

    private func extractCommandKey(_ text: String) -> String {
        guard text.hasPrefix("/") else { return "" }
        let parts = text.dropFirst().split(separator: " ", maxSplits: 1)
        return String(parts.first ?? "")
    }

    private func extractCommandContent(_ text: String) -> String {
        guard text.hasPrefix("/") else { return "" }
        let parts = text.dropFirst().split(separator: " ", maxSplits: 1)
        return parts.count > 1 ? String(parts[1]) : ""
    }

    private func parseHotkeyConfig(_ configString: String) -> (modifiers: NSEvent.ModifierFlags, keyCode: UInt16) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count >= 2 else {
            return ([], 44)  // Default
        }

        var modifiers: NSEvent.ModifierFlags = []

        // Parse modifiers (all parts except the last)
        for i in 0..<(parts.count - 1) {
            switch parts[i] {
            case "Command": modifiers.insert(.command)
            case "Option": modifiers.insert(.option)
            case "Control": modifiers.insert(.control)
            case "Shift": modifiers.insert(.shift)
            default: break
            }
        }

        // Parse key code
        let keyCode: UInt16
        switch parts[parts.count - 1] {
        case "/": keyCode = 44
        case "`": keyCode = 50
        case "\\": keyCode = 42
        case ";": keyCode = 41
        case ",": keyCode = 43
        case ".": keyCode = 47
        case "Space": keyCode = 49
        default: keyCode = 44
        }

        return (modifiers, keyCode)
    }
}

// MARK: - Selector Option Tests

/// Unit tests for SelectorOption
final class SelectorOptionTests: XCTestCase {

    /// Test: SelectorOption initialization
    func testSelectorOptionInit() {
        let option = SelectorOption(
            id: "test-id",
            label: "Test Label",
            description: "Test Description",
            isSelected: false,
            iconName: "star"
        )

        XCTAssertEqual(option.id, "test-id")
        XCTAssertEqual(option.label, "Test Label")
        XCTAssertEqual(option.description, "Test Description")
        XCTAssertFalse(option.isSelected)
        XCTAssertEqual(option.iconName, "star")
    }

    /// Test: SelectorOption default values
    func testSelectorOptionDefaults() {
        let option = SelectorOption(label: "Simple")

        XCTAssertNotNil(option.id)  // UUID generated
        XCTAssertNil(option.description)
        XCTAssertFalse(option.isSelected)
        XCTAssertNil(option.iconName)
    }

    /// Test: SelectorOption equality
    func testSelectorOptionEquality() {
        let option1 = SelectorOption(id: "same-id", label: "A")
        let option2 = SelectorOption(id: "same-id", label: "A")
        let option3 = SelectorOption(id: "diff-id", label: "A")

        XCTAssertEqual(option1, option2)
        XCTAssertNotEqual(option1, option3)
    }
}

// MARK: - CLI Output Line Tests

/// Unit tests for CLIOutputLine
final class CLIOutputLineTests: XCTestCase {

    /// Test: CLIOutputLine initialization
    func testCLIOutputLineInit() {
        let line = CLIOutputLine(type: .success, content: "Operation completed")

        XCTAssertNotNil(line.id)
        XCTAssertNotNil(line.timestamp)
        XCTAssertEqual(line.type, .success)
        XCTAssertEqual(line.content, "Operation completed")
    }

    /// Test: CLIOutputLine equality
    func testCLIOutputLineEquality() {
        let id = UUID()
        let timestamp = Date()

        let line1 = CLIOutputLine(id: id, timestamp: timestamp, type: .info, content: "Test")
        let line2 = CLIOutputLine(id: id, timestamp: timestamp, type: .info, content: "Test")

        XCTAssertEqual(line1, line2)
    }

    /// Test: Different lines are not equal
    func testCLIOutputLineInequality() {
        let line1 = CLIOutputLine(type: .info, content: "Test")
        let line2 = CLIOutputLine(type: .info, content: "Test")

        // Different UUIDs
        XCTAssertNotEqual(line1.id, line2.id)
    }
}
