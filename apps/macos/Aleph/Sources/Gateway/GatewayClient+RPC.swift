import Foundation

// MARK: - GatewayClient RPC Extensions

extension GatewayClient {

    // MARK: - Logs

    /// Get current log level
    func logsGetLevel() async throws -> String {
        let result: GWLogLevelResult = try await call(method: "logs.getLevel")
        return result.level
    }

    /// Set log level
    func logsSetLevel(_ level: String) async throws {
        let params = GWSetLogLevelParams(level: level)
        let _: GWOkResult = try await call(method: "logs.setLevel", params: params)
    }

    /// Get log directory path
    func logsGetDirectory() async throws -> String {
        let result: GWLogDirectoryResult = try await call(method: "logs.getDirectory")
        return result.directory
    }

    // MARK: - Commands

    /// List available commands
    func commandsList() async throws -> [GWCommandInfo] {
        let result: GWCommandListResult = try await call(method: "commands.list")
        return result.commands
    }

    // MARK: - OCR

    /// Recognize text from image
    /// - Parameters:
    ///   - imageData: Image data to process
    ///   - languages: Optional list of language codes (e.g., ["en", "zh"])
    /// - Returns: Recognized text
    func ocrRecognize(imageData: Data, languages: [String]? = nil) async throws -> String {
        let base64 = imageData.base64EncodedString()
        let params = GWOcrRecognizeParams(image: base64, languages: languages)
        let result: GWOcrRecognizeResult = try await call(method: "ocr.recognize", params: params)
        return result.text
    }

    // MARK: - Memory

    /// Search memory
    func memorySearch(query: String, limit: Int? = nil) async throws -> [GWMemorySearchItem] {
        let params = GWMemorySearchParams(query: query, limit: limit)
        let result: GWMemorySearchResult = try await call(method: "memory.search", params: params)
        return result.results
    }

    /// Delete a memory entry
    func memoryDelete(id: String) async throws {
        let params = GWMemoryDeleteParams(id: id)
        let _: GWOkResult = try await call(method: "memory.delete", params: params)
    }

    /// Clear all memory entries
    func memoryClear() async throws {
        let _: GWOkResult = try await call(method: "memory.clear")
    }

    /// Clear all facts from memory
    func memoryClearFacts() async throws -> Int {
        let result: GWMemoryClearFactsResult = try await call(method: "memory.clearFacts")
        return result.deleted
    }

    /// Get memory statistics
    func memoryStats() async throws -> GWMemoryStatsResult {
        try await call(method: "memory.stats")
    }

    /// Compress memory database
    func memoryCompress() async throws -> Bool {
        let result: GWMemoryCompressResult = try await call(method: "memory.compress")
        return result.ok
    }

    /// List all apps with memory entries
    func memoryAppList() async throws -> [String] {
        let result: GWMemoryAppListResult = try await call(method: "memory.appList")
        return result.apps
    }

    // MARK: - Plugins

    /// List all installed plugins
    func pluginsList() async throws -> [GWPluginInfo] {
        let result: GWPluginListResult = try await call(method: "plugins.list")
        return result.plugins
    }

    /// Install a plugin from URL
    func pluginsInstall(url: String) async throws -> Bool {
        let params = GWPluginInstallParams(url: url)
        let result: GWOkResult = try await call(method: "plugins.install", params: params)
        return result.ok
    }

    /// Install a plugin from zip data
    func pluginsInstallFromZip(data: Data) async throws -> Bool {
        let base64 = data.base64EncodedString()
        let params = GWPluginInstallFromZipParams(data: base64)
        let result: GWOkResult = try await call(method: "plugins.installFromZip", params: params)
        return result.ok
    }

    /// Uninstall a plugin
    func pluginsUninstall(id: String) async throws -> Bool {
        let params = GWPluginIdParams(id: id)
        let result: GWOkResult = try await call(method: "plugins.uninstall", params: params)
        return result.ok
    }

    /// Enable a plugin
    func pluginsEnable(id: String) async throws -> Bool {
        let params = GWPluginIdParams(id: id)
        let result: GWOkResult = try await call(method: "plugins.enable", params: params)
        return result.ok
    }

    /// Disable a plugin
    func pluginsDisable(id: String) async throws -> Bool {
        let params = GWPluginIdParams(id: id)
        let result: GWOkResult = try await call(method: "plugins.disable", params: params)
        return result.ok
    }

    // MARK: - Skills

    /// List all installed skills
    func skillsList() async throws -> [GWSkillInfo] {
        let result: GWSkillListResult = try await call(method: "skills.list")
        return result.skills
    }

    /// Install a skill from URL
    func skillsInstall(url: String) async throws -> Bool {
        let params = GWSkillInstallParams(url: url)
        let result: GWOkResult = try await call(method: "skills.install", params: params)
        return result.ok
    }

    /// Install a skill from zip data
    func skillsInstallFromZip(data: Data) async throws -> Bool {
        let base64 = data.base64EncodedString()
        let params = GWSkillInstallFromZipParams(data: base64)
        let result: GWOkResult = try await call(method: "skills.installFromZip", params: params)
        return result.ok
    }

    /// Delete a skill
    func skillsDelete(id: String) async throws -> Bool {
        let params = GWSkillDeleteParams(id: id)
        let result: GWOkResult = try await call(method: "skills.delete", params: params)
        return result.ok
    }

    // MARK: - MCP

    /// List all MCP servers
    func mcpListServers() async throws -> [GWMcpServerInfo] {
        let result: GWMcpServerListResult = try await call(method: "mcp.listServers")
        return result.servers
    }

    /// Get an MCP server by name
    func mcpGetServer(name: String) async throws -> GWMcpServerInfo {
        let params = GWMcpGetServerParams(name: name)
        let result: GWMcpGetServerResult = try await call(method: "mcp.getServer", params: params)
        return result.server
    }

    /// Add an MCP server
    func mcpAddServer(config: GWMcpServerConfig) async throws -> Bool {
        let params = GWMcpAddServerParams(config: config)
        let result: GWOkResult = try await call(method: "mcp.addServer", params: params)
        return result.ok
    }

    /// Remove an MCP server
    func mcpRemoveServer(name: String) async throws -> Bool {
        let params = GWMcpServerNameParams(name: name)
        let result: GWOkResult = try await call(method: "mcp.removeServer", params: params)
        return result.ok
    }

    /// Enable an MCP server
    func mcpEnableServer(name: String) async throws -> Bool {
        let params = GWMcpServerNameParams(name: name)
        let result: GWOkResult = try await call(method: "mcp.enableServer", params: params)
        return result.ok
    }

    /// Disable an MCP server
    func mcpDisableServer(name: String) async throws -> Bool {
        let params = GWMcpServerNameParams(name: name)
        let result: GWOkResult = try await call(method: "mcp.disableServer", params: params)
        return result.ok
    }

    // MARK: - Providers

    /// List all AI providers
    func providersList() async throws -> [GWProviderInfo] {
        let result: GWProviderListResult = try await call(method: "providers.list")
        return result.providers
    }

    /// Get an AI provider by name
    func providersGet(name: String) async throws -> GWProviderInfo {
        let params = GWProviderGetParams(name: name)
        let result: GWProviderGetResult = try await call(method: "providers.get", params: params)
        return result.provider
    }

    /// Update an AI provider
    func providersUpdate(name: String, config: GWProviderConfig) async throws -> Bool {
        let params = GWProviderUpdateParams(name: name, config: config)
        let result: GWOkResult = try await call(method: "providers.update", params: params)
        return result.ok
    }

    /// Delete an AI provider
    func providersDelete(name: String) async throws -> Bool {
        let params = GWProviderNameParams(name: name)
        let result: GWOkResult = try await call(method: "providers.delete", params: params)
        return result.ok
    }

    /// Test an AI provider connection
    func providersTest(config: GWProviderConfig) async throws -> GWProviderTestResult {
        let params = GWProviderTestParams(config: config)
        return try await call(method: "providers.test", params: params)
    }

    /// Set default AI provider
    func providersSetDefault(name: String) async throws -> Bool {
        let params = GWProviderNameParams(name: name)
        let result: GWOkResult = try await call(method: "providers.setDefault", params: params)
        return result.ok
    }

    // MARK: - Generation Providers

    /// List all generation providers (image, audio, tts)
    func generationListProviders() async throws -> [GWGenerationProviderInfo] {
        let result: GWGenerationProviderListResult = try await call(method: "generation.listProviders")
        return result.providers
    }

    /// Get a generation provider by name
    func generationGetProvider(name: String) async throws -> GWGenerationProviderInfo {
        let params = GWGenerationGetProviderParams(name: name)
        return try await call(method: "generation.getProvider", params: params)
    }

    /// Update a generation provider
    func generationUpdateProvider(name: String, config: GWGenerationProviderConfig) async throws -> Bool {
        let params = GWGenerationUpdateProviderParams(name: name, config: config)
        let result: GWOkResult = try await call(method: "generation.updateProvider", params: params)
        return result.ok
    }

    /// Test a generation provider
    func generationTestProvider(name: String, config: GWGenerationProviderConfig) async throws -> GWGenerationTestResult {
        let params = GWGenerationTestProviderParams(name: name, config: config)
        return try await call(method: "generation.testProvider", params: params)
    }

    // MARK: - Agent Extensions

    /// Confirm or reject a task plan
    func agentConfirmPlan(planId: String, confirmed: Bool) async throws -> Bool {
        let params = GWConfirmPlanParams(planId: planId, confirmed: confirmed)
        let result: GWOkResult = try await call(method: "agent.confirmPlan", params: params)
        return result.ok
    }

    /// Respond to an agent input request
    func agentRespondToInput(requestId: String, response: String) async throws -> Bool {
        let params = GWRespondToInputParams(requestId: requestId, response: response)
        let result: GWOkResult = try await call(method: "agent.respondToInput", params: params)
        return result.ok
    }

    /// Generate a conversation title
    func agentGenerateTitle(userInput: String, aiResponse: String) async throws -> String {
        let params = GWGenerateTitleParams(userInput: userInput, aiResponse: aiResponse)
        let result: GWGenerateTitleResult = try await call(method: "agent.generateTitle", params: params)
        return result.title
    }

    // MARK: - Config: Behavior

    /// Get behavior config
    func configBehaviorGet() async throws -> GWBehaviorConfig {
        let result: GWBehaviorConfigResult = try await call(method: "config.behavior.get")
        return result.behavior
    }

    /// Update behavior config
    func configBehaviorUpdate(_ config: GWBehaviorConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.behavior.update", params: config)
        return result.ok
    }

    // MARK: - Config: Search

    /// Get search config
    func configSearchGet() async throws -> GWSearchConfigView {
        let result: GWSearchConfigResult = try await call(method: "config.search.get")
        return result.search
    }

    /// Update search config
    func configSearchUpdate(_ config: GWSearchConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.search.update", params: config)
        return result.ok
    }

    /// Test search provider
    func configSearchTest(_ config: GWSearchConfig) async throws -> GWSearchTestResult {
        try await call(method: "config.search.test", params: config)
    }

    // MARK: - Config: Policies

    /// Get policies config
    func configPoliciesGet() async throws -> GWPoliciesConfig {
        let result: GWPoliciesConfigResult = try await call(method: "config.policies.get")
        return result.policies
    }

    /// Update policies config
    func configPoliciesUpdate(_ config: GWPoliciesConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.policies.update", params: config)
        return result.ok
    }

    // MARK: - Config: Shortcuts

    /// Get shortcuts config
    func configShortcutsGet() async throws -> GWShortcutsConfig {
        let result: GWShortcutsConfigResult = try await call(method: "config.shortcuts.get")
        return result.shortcuts
    }

    /// Update shortcuts config
    func configShortcutsUpdate(_ config: GWShortcutsConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.shortcuts.update", params: config)
        return result.ok
    }

    // MARK: - Config: Triggers

    /// Get triggers config
    func configTriggersGet() async throws -> GWTriggersConfig {
        let result: GWTriggersConfigResult = try await call(method: "config.triggers.get")
        return result.triggers
    }

    /// Update triggers config
    func configTriggersUpdate(_ config: GWTriggersConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.triggers.update", params: config)
        return result.ok
    }

    // MARK: - Config: Security

    /// Get code execution config
    func configSecurityGetCodeExec() async throws -> GWCodeExecConfig {
        let result: GWCodeExecConfigResult = try await call(method: "config.security.getCodeExec")
        return result.codeExec
    }

    /// Update code execution config
    func configSecurityUpdateCodeExec(_ config: GWCodeExecConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.security.updateCodeExec", params: config)
        return result.ok
    }

    /// Get file operations config
    func configSecurityGetFileOps() async throws -> GWFileOpsConfig {
        let result: GWFileOpsConfigResult = try await call(method: "config.security.getFileOps")
        return result.fileOps
    }

    /// Update file operations config
    func configSecurityUpdateFileOps(_ config: GWFileOpsConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.security.updateFileOps", params: config)
        return result.ok
    }

    // MARK: - Config: Model Profiles

    /// Update a model profile
    func configModelProfilesUpdate(_ profile: GWModelProfileConfig) async throws -> Bool {
        let result: GWOkResult = try await call(method: "config.modelProfiles.update", params: profile)
        return result.ok
    }
}
