import Foundation

// MARK: - Logs RPC Types

/// Result for logs.getLevel
struct GWLogLevelResult: Codable, Sendable {
    let level: String
}

/// Parameters for logs.setLevel
struct GWSetLogLevelParams: Codable, Sendable {
    let level: String
}

/// Result for logs.getDirectory
struct GWLogDirectoryResult: Codable, Sendable {
    let directory: String
}

// MARK: - Commands RPC Types

/// Command info for commands.list
struct GWCommandInfo: Codable, Sendable, Identifiable {
    let name: String
    let description: String
    let category: String?

    var id: String { name }
}

/// Result for commands.list
struct GWCommandListResult: Codable, Sendable {
    let commands: [GWCommandInfo]
}

// MARK: - OCR RPC Types

/// Parameters for ocr.recognize
struct GWOcrRecognizeParams: Codable, Sendable {
    /// Base64 encoded image data
    let image: String
    /// Optional languages to recognize (e.g., ["en", "zh"])
    let languages: [String]?
}

/// Result for ocr.recognize
struct GWOcrRecognizeResult: Codable, Sendable {
    let text: String
}

// MARK: - Memory RPC Types

/// Parameters for memory.search
struct GWMemorySearchParams: Codable, Sendable {
    let query: String
    let limit: Int?
}

/// Memory search result item
struct GWMemorySearchItem: Codable, Sendable, Identifiable {
    let id: String
    let content: String
    let similarity: Double
    let metadata: AnyCodable?
}

/// Result for memory.search
struct GWMemorySearchResult: Codable, Sendable {
    let results: [GWMemorySearchItem]
}

/// Parameters for memory.delete
struct GWMemoryDeleteParams: Codable, Sendable {
    let id: String
}

/// Result for memory.clearFacts
struct GWMemoryClearFactsResult: Codable, Sendable {
    let deleted: Int
}

/// Result for memory.stats
struct GWMemoryStatsResult: Codable, Sendable {
    let count: Int
    let sizeBytes: Int

    enum CodingKeys: String, CodingKey {
        case count
        case sizeBytes = "size_bytes"
    }
}

/// Result for memory.compress
struct GWMemoryCompressResult: Codable, Sendable {
    let ok: Bool
}

/// Result for memory.appList
struct GWMemoryAppListResult: Codable, Sendable {
    let apps: [String]
}

// MARK: - Plugins RPC Types

/// Plugin info for plugins.list
struct GWPluginInfo: Codable, Sendable, Identifiable {
    let id: String
    let name: String
    let version: String
    let enabled: Bool
    let description: String?
}

/// Result for plugins.list
struct GWPluginListResult: Codable, Sendable {
    let plugins: [GWPluginInfo]
}

/// Parameters for plugins.install
struct GWPluginInstallParams: Codable, Sendable {
    let url: String
}

/// Parameters for plugins.installFromZip
struct GWPluginInstallFromZipParams: Codable, Sendable {
    /// Base64 encoded zip data
    let data: String
}

/// Parameters for plugins.uninstall/enable/disable
struct GWPluginIdParams: Codable, Sendable {
    let id: String
}

// MARK: - Skills RPC Types

/// Skill info for skills.list (Gateway RPC version)
struct GWSkillInfo: Codable, Sendable, Identifiable {
    let id: String
    let name: String
    let description: String?
    let source: String?
}

/// Result for skills.list
struct GWSkillListResult: Codable, Sendable {
    let skills: [GWSkillInfo]
}

/// Parameters for skills.install
struct GWSkillInstallParams: Codable, Sendable {
    let url: String
}

/// Parameters for skills.installFromZip
struct GWSkillInstallFromZipParams: Codable, Sendable {
    /// Base64 encoded zip data
    let data: String
}

/// Parameters for skills.delete
struct GWSkillDeleteParams: Codable, Sendable {
    let id: String
}

// MARK: - MCP RPC Types

/// MCP server info
struct GWMcpServerInfo: Codable, Sendable, Identifiable {
    let name: String
    let enabled: Bool
    let url: String?
    let transport: String?

    var id: String { name }
}

/// Result for mcp.listServers
struct GWMcpServerListResult: Codable, Sendable {
    let servers: [GWMcpServerInfo]
}

/// Parameters for mcp.getServer
struct GWMcpGetServerParams: Codable, Sendable {
    let name: String
}

/// Result for mcp.getServer
struct GWMcpGetServerResult: Codable, Sendable {
    let server: GWMcpServerInfo
}

/// MCP server configuration for add/update (Gateway RPC version)
struct GWMcpServerConfig: Codable, Sendable {
    let name: String
    let enabled: Bool
    let transport: String
    let url: String?
    let command: String?
    let args: [String]?
    let env: [String: String]?
}

/// Parameters for mcp.addServer
struct GWMcpAddServerParams: Codable, Sendable {
    let config: GWMcpServerConfig
}

/// Parameters for mcp.removeServer/enableServer/disableServer
struct GWMcpServerNameParams: Codable, Sendable {
    let name: String
}

// MARK: - Providers RPC Types

/// AI Provider info
struct GWProviderInfo: Codable, Sendable, Identifiable {
    let name: String
    let enabled: Bool
    let model: String
    let providerType: String?
    let isDefault: Bool

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case enabled
        case model
        case providerType = "provider_type"
        case isDefault = "is_default"
    }
}

/// Result for providers.list
struct GWProviderListResult: Codable, Sendable {
    let providers: [GWProviderInfo]
}

/// Parameters for providers.get
struct GWProviderGetParams: Codable, Sendable {
    let name: String
}

/// Result for providers.get
struct GWProviderGetResult: Codable, Sendable {
    let provider: GWProviderInfo
}

/// Provider configuration for update (Gateway RPC version)
struct GWProviderConfig: Codable, Sendable {
    let enabled: Bool
    let model: String
    let apiKey: String?
    let baseUrl: String?

    enum CodingKeys: String, CodingKey {
        case enabled
        case model
        case apiKey = "api_key"
        case baseUrl = "base_url"
    }
}

/// Parameters for providers.update
struct GWProviderUpdateParams: Codable, Sendable {
    let name: String
    let config: GWProviderConfig
}

/// Parameters for providers.delete/setDefault
struct GWProviderNameParams: Codable, Sendable {
    let name: String
}

/// Parameters for providers.test
struct GWProviderTestParams: Codable, Sendable {
    let config: GWProviderConfig
}

/// Result for providers.test (Gateway RPC version)
struct GWProviderTestResult: Codable, Sendable {
    let success: Bool
    let error: String?
    let latencyMs: Int?

    enum CodingKeys: String, CodingKey {
        case success
        case error
        case latencyMs = "latency_ms"
    }
}

// MARK: - Generation Providers RPC Types

/// Generation provider info (image, audio, tts)
struct GWGenerationProviderInfo: Codable, Sendable, Identifiable {
    let name: String
    let enabled: Bool
    let providerType: String
    let model: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case enabled
        case providerType = "provider_type"
        case model
    }
}

/// Result for generation.listProviders
struct GWGenerationProviderListResult: Codable, Sendable {
    let providers: [GWGenerationProviderInfo]
}

/// Parameters for generation.getProvider
struct GWGenerationGetProviderParams: Codable, Sendable {
    let name: String
}

/// Generation provider config
struct GWGenerationProviderConfig: Codable, Sendable {
    let enabled: Bool
    let providerType: String
    let model: String?
    let apiKey: String?
    let baseUrl: String?

    enum CodingKeys: String, CodingKey {
        case enabled
        case providerType = "provider_type"
        case model
        case apiKey = "api_key"
        case baseUrl = "base_url"
    }
}

/// Parameters for generation.updateProvider
struct GWGenerationUpdateProviderParams: Codable, Sendable {
    let name: String
    let config: GWGenerationProviderConfig
}

/// Parameters for generation.testProvider
struct GWGenerationTestProviderParams: Codable, Sendable {
    let name: String
    let config: GWGenerationProviderConfig
}

/// Result for generation.testProvider
struct GWGenerationTestResult: Codable, Sendable {
    let success: Bool
    let error: String?
}

// MARK: - Agent Extension RPC Types

/// Parameters for agent.confirmPlan
struct GWConfirmPlanParams: Codable, Sendable {
    let planId: String
    let confirmed: Bool

    enum CodingKeys: String, CodingKey {
        case planId = "plan_id"
        case confirmed
    }
}

/// Parameters for agent.respondToInput
struct GWRespondToInputParams: Codable, Sendable {
    let requestId: String
    let response: String

    enum CodingKeys: String, CodingKey {
        case requestId = "request_id"
        case response
    }
}

/// Parameters for agent.generateTitle
struct GWGenerateTitleParams: Codable, Sendable {
    let userInput: String
    let aiResponse: String

    enum CodingKeys: String, CodingKey {
        case userInput = "user_input"
        case aiResponse = "ai_response"
    }
}

/// Result for agent.generateTitle
struct GWGenerateTitleResult: Codable, Sendable {
    let title: String
}

// MARK: - Config Sub-domain RPC Types

/// Behavior config (Gateway RPC version)
struct GWBehaviorConfig: Codable, Sendable {
    let autoApply: Bool
    let confirmBeforeApply: Bool
    let maxContextTokens: Int?

    enum CodingKeys: String, CodingKey {
        case autoApply = "auto_apply"
        case confirmBeforeApply = "confirm_before_apply"
        case maxContextTokens = "max_context_tokens"
    }
}

/// Result for config.behavior.get
struct GWBehaviorConfigResult: Codable, Sendable {
    let behavior: GWBehaviorConfig
}

/// Search config (Gateway RPC version)
struct GWSearchConfig: Codable, Sendable {
    let enabled: Bool
    let provider: String?
    let apiKey: String?

    enum CodingKeys: String, CodingKey {
        case enabled
        case provider
        case apiKey = "api_key"
    }
}

/// Result for config.search.get
struct GWSearchConfigResult: Codable, Sendable {
    let search: GWSearchConfigView
}

/// Search config view (without api_key for display)
struct GWSearchConfigView: Codable, Sendable {
    let enabled: Bool
    let provider: String?
}

/// Result for config.search.test
struct GWSearchTestResult: Codable, Sendable {
    let success: Bool
    let error: String?
}

/// Policies config (Gateway RPC version)
struct GWPoliciesConfig: Codable, Sendable {
    let allowWebBrowsing: Bool
    let allowFileAccess: Bool
    let allowCodeExecution: Bool

    enum CodingKeys: String, CodingKey {
        case allowWebBrowsing = "allow_web_browsing"
        case allowFileAccess = "allow_file_access"
        case allowCodeExecution = "allow_code_execution"
    }
}

/// Result for config.policies.get
struct GWPoliciesConfigResult: Codable, Sendable {
    let policies: GWPoliciesConfig
}

/// Shortcuts config (Gateway RPC version)
struct GWShortcutsConfig: Codable, Sendable {
    let triggerHotkey: String?
    let visionHotkey: String?

    enum CodingKeys: String, CodingKey {
        case triggerHotkey = "trigger_hotkey"
        case visionHotkey = "vision_hotkey"
    }
}

/// Result for config.shortcuts.get
struct GWShortcutsConfigResult: Codable, Sendable {
    let shortcuts: GWShortcutsConfig
}

/// Triggers config (Gateway RPC version)
struct GWTriggersConfig: Codable, Sendable {
    let doubleTapEnabled: Bool
    let doubleTapIntervalMs: Int?

    enum CodingKeys: String, CodingKey {
        case doubleTapEnabled = "double_tap_enabled"
        case doubleTapIntervalMs = "double_tap_interval_ms"
    }
}

/// Result for config.triggers.get
struct GWTriggersConfigResult: Codable, Sendable {
    let triggers: GWTriggersConfig
}

/// Code execution config (Gateway RPC version)
struct GWCodeExecConfig: Codable, Sendable {
    let enabled: Bool
    let sandbox: Bool
    let timeoutMs: Int?

    enum CodingKeys: String, CodingKey {
        case enabled
        case sandbox
        case timeoutMs = "timeout_ms"
    }
}

/// Result for config.security.getCodeExec
struct GWCodeExecConfigResult: Codable, Sendable {
    let codeExec: GWCodeExecConfig
}

/// File operations config (Gateway RPC version)
struct GWFileOpsConfig: Codable, Sendable {
    let enabled: Bool
    let allowedPaths: [String]
    let deniedPaths: [String]

    enum CodingKeys: String, CodingKey {
        case enabled
        case allowedPaths = "allowed_paths"
        case deniedPaths = "denied_paths"
    }
}

/// Result for config.security.getFileOps
struct GWFileOpsConfigResult: Codable, Sendable {
    let fileOps: GWFileOpsConfig
}

/// Model profile config (Gateway RPC version)
struct GWModelProfileConfig: Codable, Sendable {
    let name: String
    let model: String
    let thinking: String?
    let maxTokens: Int?

    enum CodingKeys: String, CodingKey {
        case name
        case model
        case thinking
        case maxTokens = "max_tokens"
    }
}

// MARK: - Generic Success Result

/// Generic success result for operations that just return { ok: true }
struct GWOkResult: Codable, Sendable {
    let ok: Bool
}
