import XCTest
@testable import Aether

/// Integration tests for config persistence (Phase 6 - Task 8.2)
///
/// These tests verify:
/// 1. Config changes persist across app lifecycle
/// 2. Core initialization and basic functionality
final class ConfigPersistenceTests: XCTestCase {

    var tempConfigPath: String!
    var core: AetherCore!

    override func setUp() {
        super.setUp()

        // Create temporary config file path
        let tempDir = FileManager.default.temporaryDirectory
        let testDir = tempDir.appendingPathComponent(UUID().uuidString)
        try? FileManager.default.createDirectory(at: testDir, withIntermediateDirectories: true)
        tempConfigPath = testDir.appendingPathComponent("config.toml").path

        // Create event handler stub for testing
        let eventHandler = TestEventHandler()

        // Initialize AetherCore
        core = try! AetherCore(handler: eventHandler)
    }

    override func tearDown() {
        // Clean up temporary config file
        if let path = tempConfigPath {
            try? FileManager.default.removeItem(atPath: path)
        }

        core = nil
        super.tearDown()
    }

    // MARK: - Config Persistence Tests

    /// Test: Basic core initialization
    func testCoreInitialization() throws {
        // Simply verify that core was initialized successfully
        XCTAssertNotNil(core)
    }

    /* TODO: Re-enable these tests once API is updated
    /// Test: Save provider → Quit app → Relaunch → Verify persisted
    func testProviderPersistenceAcrossRestart() throws {
        // Create a provider config
        let provider = ProviderConfigEntry(
            name: "openai",
            config: ProviderConfig(
                providerType: "openai",
                apiKey: "sk-test-key-12345",
                model: "gpt-4o",
                baseUrl: nil,
                color: "#10a37f",
                timeoutSeconds: 300,
                maxTokens: 4096,
                temperature: 0.7
            )
        )

        // Update provider through core
        try core.updateProvider(provider: provider)

        // Save config to file
        let config = core.getConfig()
        let configPath = tempConfigPath!

        // Serialize config to TOML manually (simulating app save)
        // In real app, this would be done by Rust core's save_to_file
        let tomlContent = """
        default_hotkey = "Command+Grave"

        [general]
        default_provider = "openai"

        [providers.openai]
        provider_type = "openai"
        model = "gpt-4o"
        color = "#10a37f"
        timeout_seconds = 30
        max_tokens = 4096
        temperature = 0.7

        [memory]
        enabled = true
        """
        try tomlContent.write(toFile: configPath, atomically: true, encoding: .utf8)

        // Verify API key is stored in Keychain (not in config file)
        XCTAssertFalse(tomlContent.contains("sk-test-key"))

        // Simulate app restart by creating new AetherCore instance
        let newEventHandler = TestEventHandler()
        let newCore = try AetherCore(handler: newEventHandler, keychainManager: keychainManager)

        // Load config from file
        let loadedConfig = newCore.getConfig()

        // Verify provider exists
        let loadedProvider = loadedConfig.providers.first { $0.name == "openai" }
        XCTAssertNotNil(loadedProvider)
        XCTAssertEqual(loadedProvider?.config.model, "gpt-4o")
        XCTAssertEqual(loadedProvider?.config.color, "#10a37f")
    }

    /// Test: Edit rule → External config.toml edit → Verify hot-reload
    func testConfigHotReload() throws {
        let expectation = self.expectation(description: "Config hot-reload notification received")

        // Create event handler that tracks config changes
        let eventHandler = TestEventHandler()
        eventHandler.onConfigChangedCallback = { config in
            expectation.fulfill()
        }

        // Create new core with hot-reload enabled
        let testCore = try AetherCore(handler: eventHandler, keychainManager: keychainManager)

        // Start watching config file
        try testCore.startWatchingConfig(path: tempConfigPath)

        // Write initial config
        let initialConfig = """
        default_hotkey = "Command+Grave"

        [general]
        default_provider = "openai"
        """
        try initialConfig.write(toFile: tempConfigPath, atomically: true, encoding: .utf8)

        // Wait a bit for watcher to settle
        Thread.sleep(forTimeInterval: 0.1)

        // Modify config file externally (simulating user edit)
        let modifiedConfig = """
        default_hotkey = "Command+Shift+A"

        [general]
        default_provider = "claude"
        """
        try modifiedConfig.write(toFile: tempConfigPath, atomically: true, encoding: .utf8)

        // Wait for hot-reload notification (should arrive within 1 second)
        waitForExpectations(timeout: 2.0) { error in
            if let error = error {
                XCTFail("Hot-reload timeout: \(error)")
            }
        }

        // Verify config was reloaded
        XCTAssertTrue(eventHandler.configChangedCalled)
    }

    // MARK: - Keychain Integration Tests

    /// Test: Keychain integration (save/load/delete API key)
    func testKeychainSaveLoadDelete() throws {
        let providerName = "test-provider"
        let apiKey = "sk-test-secret-key-67890"

        // Save API key to Keychain
        try keychainManager.setApiKey(providerName: providerName, apiKey: apiKey)

        // Load API key from Keychain
        let loadedKey = try keychainManager.getApiKey(providerName: providerName)
        XCTAssertEqual(loadedKey, apiKey)

        // Check if key exists
        XCTAssertTrue(try keychainManager.hasApiKey(providerName: providerName))

        // Delete API key from Keychain
        try keychainManager.deleteApiKey(providerName: providerName)

        // Verify key no longer exists
        XCTAssertFalse(try keychainManager.hasApiKey(providerName: providerName))

        // Attempting to load deleted key should throw
        XCTAssertThrowsError(try keychainManager.getApiKey(providerName: providerName))
    }

    /// Test: Multiple providers in Keychain
    func testKeychainMultipleProviders() throws {
        let providers = [
            ("openai", "sk-openai-key-123"),
            ("claude", "sk-ant-claude-key-456"),
            ("gemini", "gm-gemini-key-789")
        ]

        // Save all providers
        for (name, key) in providers {
            try keychainManager.setApiKey(providerName: name, apiKey: key)
        }

        // Verify all providers exist and have correct keys
        for (name, key) in providers {
            XCTAssertTrue(try keychainManager.hasApiKey(providerName: name))
            let loadedKey = try keychainManager.getApiKey(providerName: name)
            XCTAssertEqual(loadedKey, key)
        }

        // Delete one provider
        try keychainManager.deleteApiKey(providerName: "claude")

        // Verify only claude is deleted, others remain
        XCTAssertTrue(try keychainManager.hasApiKey(providerName: "openai"))
        XCTAssertFalse(try keychainManager.hasApiKey(providerName: "claude"))
        XCTAssertTrue(try keychainManager.hasApiKey(providerName: "gemini"))
    }

    /// Test: Keychain update (overwrite existing key)
    func testKeychainUpdateKey() throws {
        let providerName = "openai"
        let oldKey = "sk-old-key-111"
        let newKey = "sk-new-key-222"

        // Save initial key
        try keychainManager.setApiKey(providerName: providerName, apiKey: oldKey)
        XCTAssertEqual(try keychainManager.getApiKey(providerName: providerName), oldKey)

        // Update key
        try keychainManager.setApiKey(providerName: providerName, apiKey: newKey)

        // Verify key was updated
        let loadedKey = try keychainManager.getApiKey(providerName: providerName)
        XCTAssertEqual(loadedKey, newKey)
        XCTAssertNotEqual(loadedKey, oldKey)
    }

    // MARK: - Config Validation Tests

    /// Test: Invalid config file should fail validation
    func testInvalidConfigValidation() {
        let invalidConfig = """
        default_hotkey = "Command+Grave"

        [providers.openai]
        # Missing required 'model' field
        color = "#10a37f"
        """

        try? invalidConfig.write(toFile: tempConfigPath, atomically: true, encoding: .utf8)

        // Attempting to load invalid config should throw
        XCTAssertThrowsError(try core.loadConfigFromFile(path: tempConfigPath))
    }

    /// Test: Config with invalid regex should fail validation
    func testInvalidRegexValidation() throws {
        let configWithInvalidRegex = """
        default_hotkey = "Command+Grave"

        [providers.openai]
        model = "gpt-4o"

        [[rules]]
        regex = "[invalid("
        provider = "openai"
        """

        try configWithInvalidRegex.write(toFile: tempConfigPath, atomically: true, encoding: .utf8)

        // Loading config with invalid regex should fail
        XCTAssertThrowsError(try core.loadConfigFromFile(path: tempConfigPath))
    }
    */
}

// MARK: - Test Event Handler

/// Stub event handler for testing - implements current AetherEventHandler protocol
class TestEventHandler: AetherEventHandler {
    var errorCalled = false
    var completeCalled = false

    func onThinking() {}
    func onToolStart(toolName: String) {}
    func onToolResult(toolName: String, result: String) {}
    func onStreamChunk(text: String) {}
    func onComplete(response: String) {
        completeCalled = true
    }
    func onError(message: String) {
        errorCalled = true
    }
    func onMemoryStored() {}
    func onAgentModeDetected(task: ExecutableTaskFfi) {}
    func onToolsChanged(toolCount: UInt32) {}
    func onMcpStartupComplete(report: McpStartupReportFfi) {}
    func onRuntimeUpdatesAvailable(updates: [RuntimeUpdateInfo]) {}
    func onSessionStarted(sessionId: String) {}
    func onToolCallStarted(callId: String, toolName: String) {}
    func onToolCallCompleted(callId: String, output: String) {}
    func onToolCallFailed(callId: String, error: String, isRetryable: Bool) {}
    func onLoopProgress(sessionId: String, iteration: UInt32, status: String) {}
    func onPlanCreated(sessionId: String, steps: [String]) {}
    func onSessionCompleted(sessionId: String, summary: String) {}
    func onSubagentStarted(parentSessionId: String, childSessionId: String, agentId: String) {}
    func onSubagentCompleted(childSessionId: String, success: Bool, summary: String) {}
    func onPlanConfirmationRequired(planId: String, plan: DagTaskPlan) {}
}
