//
//  FFICompatibilityLayer.swift
//  Aleph
//
//  Defines stub types and methods that were previously provided by UniFFI
//  generated bindings but are no longer present. These types allow the Swift
//  UI code to compile while actual functionality is served via Gateway
//  WebSocket RPC at runtime.
//
//  This file is intentionally self-contained. Do NOT merge into aleph.swift
//  or the generated bindings — those are auto-generated and will be
//  overwritten.
//

import Foundation

// MARK: - Task Classification Types

/// FFI stub for executable task classification result
struct ExecutableTaskFfi {
    let category: String
    let action: String
    let confidence: Float
    let target: String?

    init(category: String, action: String, confidence: Float, target: String? = nil) {
        self.category = category
        self.action = action
        self.confidence = confidence
        self.target = target
    }
}

/// FFI stub for task category enum
enum TaskCategoryFfi: String {
    case query
    case action
    case creation
    case analysis
}

// MARK: - MCP Types

/// FFI stub for MCP startup report
struct McpStartupReportFfi {
    let succeededServers: [String]
    let failedServers: [McpServerErrorFfi]
}

/// FFI stub for MCP server error
struct McpServerErrorFfi {
    let serverName: String
    let error: String
}

// MARK: - Runtime Types

/// FFI stub for runtime update information
struct RuntimeUpdateInfo {
    let runtimeId: String
    let currentVersion: String
    let latestVersion: String
}

// MARK: - DAG Task Plan Types

/// Display status for DAG tasks
enum DagTaskDisplayStatus: Equatable {
    case pending
    case running
    case completed
    case failed
    case cancelled
}

/// Information about a single DAG task
struct DagTaskInfo: Identifiable {
    let id: String
    let name: String
    let status: DagTaskDisplayStatus
    let riskLevel: String
    let dependencies: [String]

    init(id: String, name: String, status: DagTaskDisplayStatus, riskLevel: String, dependencies: [String] = []) {
        self.id = id
        self.name = name
        self.status = status
        self.riskLevel = riskLevel
        self.dependencies = dependencies
    }
}

/// DAG task execution plan
struct DagTaskPlan {
    let id: String
    let title: String
    let tasks: [DagTaskInfo]
    let requiresConfirmation: Bool

    init(id: String, title: String, tasks: [DagTaskInfo], requiresConfirmation: Bool = false) {
        self.id = id
        self.title = title
        self.tasks = tasks
        self.requiresConfirmation = requiresConfirmation
    }
}

// MARK: - Part Update Types

/// Event type for part updates
enum PartEventTypeFfi {
    case added
    case updated
    case removed
}

/// FFI stub for part update event
struct PartUpdateEventFfi {
    let sessionId: String
    let partId: String
    let partType: String
    let eventType: PartEventTypeFfi
    let partJson: String
    let delta: String?
    let timestamp: Int64

    /// Full initializer
    init(sessionId: String = "", partId: String, partType: String, eventType: PartEventTypeFfi, partJson: String, delta: String? = nil, timestamp: Int64) {
        self.sessionId = sessionId
        self.partId = partId
        self.partType = partType
        self.eventType = eventType
        self.partJson = partJson
        self.delta = delta
        self.timestamp = timestamp
    }

    /// Convenience initializer matching test usage order
    init(partType: String, partId: String, partJson: String, delta: String?, eventType: PartEventTypeFfi, timestamp: Int64) {
        self.sessionId = ""
        self.partType = partType
        self.partId = partId
        self.partJson = partJson
        self.delta = delta
        self.eventType = eventType
        self.timestamp = timestamp
    }
}

// MARK: - Clarification Types

/// Clarification type enum
enum ClarificationType: Equatable {
    case select
    case text
    case multiGroup
}

/// A single clarification option
struct ClarificationOption {
    let label: String
    let value: String
    let description: String?

    init(label: String, value: String, description: String? = nil) {
        self.label = label
        self.value = value
        self.description = description
    }
}

/// A group of questions for multi-group clarification
struct QuestionGroup {
    let id: String
    let prompt: String
    let options: [ClarificationOption]
    let defaultIndex: UInt32

    init(id: String, prompt: String, options: [ClarificationOption], defaultIndex: UInt32 = 0) {
        self.id = id
        self.prompt = prompt
        self.options = options
        self.defaultIndex = defaultIndex
    }
}

/// A clarification request from the agent
struct ClarificationRequest: Identifiable {
    let id: String
    let prompt: String
    let clarificationType: ClarificationType
    let options: [ClarificationOption]?
    let groups: [QuestionGroup]?
    let defaultValue: String?
    let placeholder: String?
    let source: String?

    init(
        id: String,
        prompt: String,
        clarificationType: ClarificationType,
        options: [ClarificationOption]? = nil,
        groups: [QuestionGroup]? = nil,
        defaultValue: String? = nil,
        placeholder: String? = nil,
        source: String? = nil
    ) {
        self.id = id
        self.prompt = prompt
        self.clarificationType = clarificationType
        self.options = options
        self.groups = groups
        self.defaultValue = defaultValue
        self.placeholder = placeholder
        self.source = source
    }
}

/// Result type for clarification responses
enum ClarificationResultType {
    case timeout
    case cancelled
    case selected
    case textInput
    case multiGroup
}

/// Result of a clarification request
struct ClarificationResult {
    let resultType: ClarificationResultType
    let selectedIndex: UInt32?
    let value: String?
    let groupAnswers: [String: String]?

    init(
        resultType: ClarificationResultType,
        selectedIndex: UInt32? = nil,
        value: String? = nil,
        groupAnswers: [String: String]? = nil
    ) {
        self.resultType = resultType
        self.selectedIndex = selectedIndex
        self.value = value
        self.groupAnswers = groupAnswers
    }
}

// MARK: - Process Options

/// Options for processing user input
struct ProcessOptions {
    let appContext: String?
    let windowTitle: String?
    let topicId: String?
    let stream: Bool
    let attachments: [MediaAttachment]?
    let preferredLanguage: String

    init(
        appContext: String? = nil,
        windowTitle: String? = nil,
        topicId: String? = nil,
        stream: Bool = true,
        attachments: [MediaAttachment]? = nil,
        preferredLanguage: String = "en"
    ) {
        self.appContext = appContext
        self.windowTitle = windowTitle
        self.topicId = topicId
        self.stream = stream
        self.attachments = attachments
        self.preferredLanguage = preferredLanguage
    }
}

// MARK: - Provider Configuration Types

/// Provider configuration
struct ProviderConfig {
    let providerType: String
    let apiKey: String?
    let model: String
    let baseUrl: String?
    let color: String?
    let timeoutSeconds: UInt64
    let enabled: Bool
    let maxTokens: UInt32?
    let temperature: Float?
    let topP: Float?
    let topK: UInt32?
    let frequencyPenalty: Float?
    let presencePenalty: Float?
    let stopSequences: String?
    let thinkingLevel: String?
    let mediaResolution: String?
    let repeatPenalty: Float?
    let systemPromptMode: String?

    init(
        providerType: String,
        apiKey: String? = nil,
        model: String = "",
        baseUrl: String? = nil,
        color: String? = nil,
        timeoutSeconds: UInt64 = 300,
        enabled: Bool = false,
        maxTokens: UInt32? = nil,
        temperature: Float? = nil,
        topP: Float? = nil,
        topK: UInt32? = nil,
        frequencyPenalty: Float? = nil,
        presencePenalty: Float? = nil,
        stopSequences: String? = nil,
        thinkingLevel: String? = nil,
        mediaResolution: String? = nil,
        repeatPenalty: Float? = nil,
        systemPromptMode: String? = nil
    ) {
        self.providerType = providerType
        self.apiKey = apiKey
        self.model = model
        self.baseUrl = baseUrl
        self.color = color
        self.timeoutSeconds = timeoutSeconds
        self.enabled = enabled
        self.maxTokens = maxTokens
        self.temperature = temperature
        self.topP = topP
        self.topK = topK
        self.frequencyPenalty = frequencyPenalty
        self.presencePenalty = presencePenalty
        self.stopSequences = stopSequences
        self.thinkingLevel = thinkingLevel
        self.mediaResolution = mediaResolution
        self.repeatPenalty = repeatPenalty
        self.systemPromptMode = systemPromptMode
    }
}

/// Named provider configuration entry
struct ProviderConfigEntry: Identifiable {
    let name: String
    let config: ProviderConfig

    var id: String { name }
}

/// Full application configuration
struct FullConfig {
    let providers: [ProviderConfigEntry]
    let shortcuts: ShortcutsConfig?

    init(providers: [ProviderConfigEntry] = [], shortcuts: ShortcutsConfig? = nil) {
        self.providers = providers
        self.shortcuts = shortcuts
    }
}

/// Keyboard shortcuts configuration
struct ShortcutsConfig {
    let commandPrompt: String

    init(commandPrompt: String = "Option+Space") {
        self.commandPrompt = commandPrompt
    }
}

// MARK: - Tool Types

/// FFI stub for tool information
struct ToolInfoFfi {
    let name: String
    let description: String
}

// MARK: - Memory Types

/// FFI stub for memory search result item
struct MemoryItem {
    let id: String
    let content: String
    let score: Float
}

// MARK: - Test Connection Types

/// Result of a provider connection test
struct TestConnectionResult {
    let success: Bool
    let message: String
}

// MARK: - Generation Types

/// Generation type enum (image, audio, tts)
enum GenerationTypeFfi {
    case image
    case audio
    case tts
}

// MARK: - Log Level

/// Log level enum for FFI
enum LogLevel {
    case error
    case warn
    case info
    case debug
    case trace
}

// MARK: - Model Profile Types

/// Model capability enum
enum ModelCapabilityFfi: Hashable {
    case codeGeneration
    case codeReview
    case textAnalysis
    case imageUnderstanding
    case videoUnderstanding
    case longContext
    case reasoning
    case localPrivacy
    case fastResponse
    case simpleTask
    case longDocument
}

/// Model cost tier
enum ModelCostTierFfi: Hashable {
    case free
    case low
    case medium
    case high
}

/// Model latency tier
enum ModelLatencyTierFfi: Hashable {
    case fast
    case medium
    case slow
}

/// Model profile configuration
struct ModelProfileFfi {
    let id: String
    let provider: String
    let model: String
    let capabilities: [ModelCapabilityFfi]
    let costTier: ModelCostTierFfi
    let latencyTier: ModelLatencyTierFfi
    let maxContext: UInt32?
    let local: Bool
}

// MARK: - Command Node Types

/// Source type for command nodes
enum CommandSourceType {
    case builtin
    case native
    case mcp
    case skill
    case custom
}

/// A node representing a command in the tool registry
struct CommandNode: Identifiable {
    let key: String
    let description: String
    let sourceId: String?
    let sourceType: CommandSourceType

    var id: String { key }

    init(key: String, description: String = "", sourceId: String? = nil, sourceType: CommandSourceType = .builtin) {
        self.key = key
        self.description = description
        self.sourceId = sourceId
        self.sourceType = sourceType
    }
}

// MARK: - Agent Task Graph Types (TaskGraphConfirmationView / TaskGraphProgressView)

/// Task type category for agent tasks
enum AgentTaskTypeCategory {
    case fileOperation
    case codeExecution
    case documentGeneration
    case appAutomation
    case aiInference
    case imageGeneration
    case videoGeneration
    case audioGeneration
}

/// Task status state for agent tasks
enum AgentTaskStatusState {
    case pending
    case running
    case completed
    case failed
    case cancelled
}

/// A single agent task in the task graph
struct AgentTaskFfi: Identifiable {
    let id: String
    let name: String
    let description: String?
    let taskType: AgentTaskTypeCategory
    let status: AgentTaskStatusState
    let progress: Float
    let errorMessage: String?

    init(id: String, name: String, description: String? = nil, taskType: AgentTaskTypeCategory, status: AgentTaskStatusState, progress: Float = 0.0, errorMessage: String? = nil) {
        self.id = id
        self.name = name
        self.description = description
        self.taskType = taskType
        self.status = status
        self.progress = progress
        self.errorMessage = errorMessage
    }
}

/// Dependency edge between two agent tasks
struct AgentTaskDependencyFfi {
    let fromTaskId: String
    let toTaskId: String
}

/// Agent task graph containing tasks and their dependency edges
struct AgentTaskGraphFfi {
    let id: String
    let title: String
    let originalRequest: String?
    let tasks: [AgentTaskFfi]
    let edges: [AgentTaskDependencyFfi]

    init(id: String, title: String, originalRequest: String? = nil, tasks: [AgentTaskFfi], edges: [AgentTaskDependencyFfi] = []) {
        self.id = id
        self.title = title
        self.originalRequest = originalRequest
        self.tasks = tasks
        self.edges = edges
    }
}

/// Agent execution state
enum AgentExecutionState: Equatable {
    case idle
    case planning
    case awaitingConfirmation
    case executing
    case paused
    case cancelled
    case completed
}

// MARK: - Settings Tab

/// Settings tab enum for sidebar navigation
enum SettingsTab: Hashable {
    case general
    case providers
    case generation
    case shortcuts
    case behavior
    case memory
    case search
    case mcp
    case skills
    case plugins
    case security
    case policies
    case guests
}

// MARK: - Provider Test Result

/// Result of a provider connection test (used by SearchProviderCard)
struct ProviderTestResult {
    let success: Bool
    let latencyMs: UInt32
    let errorMessage: String
    let errorType: String
}

// MARK: - Initialization Progress Handler

/// Protocol for initialization progress callbacks
protocol InitProgressHandlerFfi: AnyObject {
    func onPhaseStarted(phase: String, current: UInt32, total: UInt32)
    func onPhaseProgress(phase: String, progress: Double, message: String)
    func onPhaseCompleted(phase: String)
    func onDownloadProgress(item: String, downloaded: UInt64, total: UInt64)
    func onError(phase: String, message: String, isRetryable: Bool)
}

// MARK: - Unified Tool Info Types

/// Source type for tools (used by UnifiedToolInfoExtension)
enum ToolSourceType {
    case native
    case builtin
    case mcp
    case skill
    case custom
}

/// Unified tool information (used by UnifiedToolInfoExtension)
struct UnifiedToolInfo: Identifiable {
    let name: String
    let description: String
    let displayName: String
    let sourceType: ToolSourceType
    let localizationKey: String?
    let icon: String?
    let usage: String?
    let hasSubtools: Bool
    let sortOrder: Int

    var id: String { name }

    init(name: String, description: String = "", displayName: String = "", sourceType: ToolSourceType = .builtin, localizationKey: String? = nil, icon: String? = nil, usage: String? = nil, hasSubtools: Bool = false, sortOrder: Int = 0) {
        self.name = name
        self.description = description
        self.displayName = displayName
        self.sourceType = sourceType
        self.localizationKey = localizationKey
        self.icon = icon
        self.usage = usage
        self.hasSubtools = hasSubtools
        self.sortOrder = sortOrder
    }
}

// MARK: - Initialization Types

/// Result of running first-time initialization
struct FirstRunInitResult {
    let success: Bool
    let errorPhase: String?
    let errorMessage: String?

    init(success: Bool, errorPhase: String? = nil, errorMessage: String? = nil) {
        self.success = success
        self.errorPhase = errorPhase
        self.errorMessage = errorMessage
    }
}

/// Run first-time initialization with progress handler (stub — Gateway handles this)
func runInitialization(handler: InitProgressHandlerFfi) -> FirstRunInitResult {
    print("[FFICompat] runInitialization() called — use Gateway RPC")
    return FirstRunInitResult(success: true)
}

// MARK: - AlephCore Extension (Stub Methods)

extension AlephCore {

    /// Gateway client accessor (delegates to GatewayManager.shared)
    @MainActor var gatewayClient: GatewayClient {
        GatewayManager.shared.client
    }

    /// Get root commands from tool registry
    func getRootCommandsFromRegistry() -> [CommandNode] {
        print("[FFICompat] getRootCommandsFromRegistry() called — use Gateway RPC")
        return []
    }

    /// Cancel current processing (no-op stub — Gateway handles this)
    func cancel() {
        print("[FFICompat] cancel() called — use Gateway RPC")
    }

    /// Process user input (no-op stub — Gateway handles this)
    func process(input: String, options: ProcessOptions) throws {
        print("[FFICompat] process() called — use Gateway RPC")
    }

    /// List available tools
    func listTools() -> [ToolInfoFfi] {
        print("[FFICompat] listTools() called — use Gateway RPC")
        return []
    }

    /// Search memory
    func searchMemory(query: String, limit: UInt32) throws -> [MemoryItem] {
        print("[FFICompat] searchMemory() called — use Gateway RPC")
        return []
    }

    /// Clear all memory
    func clearMemory() throws {
        print("[FFICompat] clearMemory() called — use Gateway RPC")
    }

    /// Reload configuration from disk
    func reloadConfig() throws {
        print("[FFICompat] reloadConfig() called — use Gateway RPC")
    }

    /// Load full configuration
    func loadConfig() throws -> FullConfig {
        print("[FFICompat] loadConfig() called — use Gateway RPC")
        return FullConfig()
    }

    /// Get list of enabled provider names
    func getEnabledProviders() -> [String] {
        print("[FFICompat] getEnabledProviders() called — use Gateway RPC")
        return []
    }

    /// Get default provider name
    func getDefaultProvider() -> String {
        print("[FFICompat] getDefaultProvider() called — use Gateway RPC")
        return ""
    }

    /// Set default provider
    func setDefaultProvider(providerName: String) throws {
        print("[FFICompat] setDefaultProvider() called — use Gateway RPC")
    }

    /// Delete a provider
    func deleteProvider(name: String) throws {
        print("[FFICompat] deleteProvider() called — use Gateway RPC")
    }

    /// Update a provider configuration
    func updateProvider(name: String, provider: ProviderConfig) throws {
        print("[FFICompat] updateProvider(name:provider:) called — use Gateway RPC")
    }

    /// Update a provider configuration (single-arg variant)
    func updateProvider(provider: ProviderConfigEntry) throws {
        print("[FFICompat] updateProvider(provider:) called — use Gateway RPC")
    }

    /// Confirm or reject a DAG task plan
    func confirmTaskPlan(planId: String, confirmed: Bool) -> Bool {
        print("[FFICompat] confirmTaskPlan() called — use Gateway RPC")
        return false
    }

    /// Respond to a user input request
    func respondToUserInput(requestId: String, response: String) -> Bool {
        print("[FFICompat] respondToUserInput() called — use Gateway RPC")
        return false
    }

    /// Test provider connection with config
    func testProviderConnectionWithConfig(providerName: String, providerConfig: ProviderConfig) -> TestConnectionResult {
        print("[FFICompat] testProviderConnectionWithConfig() called — use Gateway RPC")
        return TestConnectionResult(success: false, message: "FFI stub — use Gateway RPC")
    }

    /// Set log level
    func setLogLevel(level: LogLevel) throws {
        print("[FFICompat] setLogLevel() called — use Gateway RPC")
    }

    /// Get log directory path
    func getLogDirectory() throws -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return "\(home)/.aleph/logs"
    }

    /// Get current log level
    func getLogLevel() -> LogLevel {
        return .info
    }

    /// Extract text from image data
    func extractText(imageData: Data) throws -> String {
        print("[FFICompat] extractText() called — use Gateway RPC")
        return ""
    }

    /// Update agent model profile
    func agentUpdateModelProfile(profile: ModelProfileFfi) throws {
        print("[FFICompat] agentUpdateModelProfile() called — use Gateway RPC")
    }
}

// MARK: - Free Functions

/// Initialize AlephCore with config path and event handler
/// Delegates to AlephCore(handler:) — configPath is ignored (Gateway handles config)
func initCore(configPath: String, handler: AlephEventHandler) throws -> AlephCore {
    return try AlephCore(handler: handler)
}

/// Check if first-time initialization is needed
func needsFirstTimeInit() -> Bool {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    let configPath = "\(home)/.aleph/config.toml"
    return !FileManager.default.fileExists(atPath: configPath)
}
