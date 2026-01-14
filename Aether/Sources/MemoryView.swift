//
//  MemoryView.swift
//  Aether
//
//  Memory management UI for viewing, configuring, and managing stored memories.
//  Phase 4E - Task 21: Settings UI (Memory Tab)
//

import SwiftUI

// MARK: - Memory View

struct MemoryView: View {
    let core: AetherV2Core
    @ObservedObject var saveBarState: SettingsSaveBarState

    @State private var memoryConfig: MemoryConfig
    @State private var memoryStats: MemoryStats?
    @State private var memories: [MemoryEntry] = []
    @State private var selectedAppFilter: String = "All Apps"
    @State private var availableApps: [AppMemoryInfo] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var showDeleteConfirmation = false
    @State private var memoryToDelete: MemoryEntry?
    @State private var showClearAllConfirmation = false
    @State private var showClearFactsConfirmation = false
    @State private var showModelDownloadWindow = false
    @State private var isCheckingModel = false

    // Compression state
    @State private var compressionStats: CompressionStats?
    @State private var isCompressing = false
    @State private var lastCompressionResult: CompressionResult?

    init(core: AetherV2Core, saveBarState: SettingsSaveBarState) {
        self.core = core
        self._saveBarState = ObservedObject(wrappedValue: saveBarState)
        // Load initial config
        _memoryConfig = State(initialValue: core.getMemoryConfig())
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Configuration Card
                configurationCard

                // Statistics Card
                if memoryConfig.enabled {
                    statisticsCard

                    // Compression Card (Dual-Layer Memory Architecture)
                    if memoryConfig.compressionEnabled {
                        compressionCard
                    }

                    // Memory Browser Card
                    memoryBrowserCard
                }
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear {
            refreshData()
            // Set save bar to disabled state for instant-save view
            saveBarState.update(
                hasUnsavedChanges: false,
                isSaving: false,
                statusMessage: nil,
                onSave: nil,
                onCancel: nil
            )
        }
        .alert(L("common.error"), isPresented: .constant(errorMessage != nil)) {
            Button(L("common.ok")) {
                errorMessage = nil
            }
        } message: {
            if let error = errorMessage {
                Text(error)
            }
        }
        .alert(L("settings.memory.delete_memory"), isPresented: $showDeleteConfirmation) {
            Button(L("common.cancel"), role: .cancel) {
                memoryToDelete = nil
            }
            Button(L("common.delete"), role: .destructive) {
                if let memory = memoryToDelete {
                    deleteMemory(memory)
                }
            }
        } message: {
            Text(L("settings.memory.delete_memory_message"))
        }
        .alert(L("settings.memory.clear_all_title"), isPresented: $showClearAllConfirmation) {
            Button(L("common.cancel"), role: .cancel) {}
            Button(L("settings.memory.clear_all_button"), role: .destructive) {
                clearAllMemories()
            }
        } message: {
            Text(L("settings.memory.clear_all_message"))
        }
        .alert(L("settings.memory.clear_facts_title"), isPresented: $showClearFactsConfirmation) {
            Button(L("common.cancel"), role: .cancel) {}
            Button(L("settings.memory.clear_all_button"), role: .destructive) {
                clearAllFacts()
            }
        } message: {
            Text(L("settings.memory.clear_facts_message"))
        }
        .sheet(isPresented: $showModelDownloadWindow) {
            ModelDownloadView(
                onSuccess: { [self] in
                    print("[MemoryView] Model download succeeded - enabling memory")
                    memoryConfig.enabled = true
                    updateConfig()
                    showModelDownloadWindow = false
                },
                onFailure: { [self] error in
                    print("[MemoryView] Model download failed: \(error)")
                    errorMessage = "Failed to download model: \(error)"
                    showModelDownloadWindow = false
                }
            )
        }
    }

    // MARK: - Configuration Card

    private var configurationCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.memory.configuration"), systemImage: "gearshape.fill")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                // Enable/Disable Toggle
                HStack {
                    Toggle(L("settings.memory.enabled"), isOn: Binding(
                        get: { memoryConfig.enabled },
                        set: { newValue in
                            handleMemoryToggle(newValue)
                        }
                    ))
                    .toggleStyle(.switch)
                    .font(DesignTokens.Typography.body)
                    .disabled(isCheckingModel)

                    if isCheckingModel {
                        ProgressView()
                            .scaleEffect(0.7)
                            .padding(.leading, DesignTokens.Spacing.xs)
                    }
                }

                if memoryConfig.enabled {
                    // Retention Policy
                    HStack {
                        Text(L("settings.memory.retention_policy"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)

                        Picker("", selection: Binding(
                            get: { memoryConfig.retentionDays },
                            set: { newValue in
                                memoryConfig.retentionDays = newValue
                                updateConfig()
                            }
                        )) {
                            Text(L("settings.memory.retention_7days")).tag(UInt32(7))
                            Text(L("settings.memory.retention_30days")).tag(UInt32(30))
                            Text(L("settings.memory.retention_90days")).tag(UInt32(90))
                            Text(L("settings.memory.retention_1year")).tag(UInt32(365))
                            Text(L("settings.memory.retention_never")).tag(UInt32(0))
                        }
                        .pickerStyle(.menu)
                        .frame(width: 150)

                        Spacer()

                        Text(L("settings.memory.retention_help"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Max Context Items
                    HStack {
                        Text(L("settings.memory.max_context_items"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)

                        Slider(
                            value: Binding(
                                get: { Double(memoryConfig.maxContextItems) },
                                set: { newValue in
                                    memoryConfig.maxContextItems = UInt32(newValue)
                                    updateConfig()
                                }
                            ),
                            in: 1...10,
                            step: 1
                        )
                        .frame(width: 200)

                        Text("\(memoryConfig.maxContextItems)")
                            .frame(width: 30)
                            .font(DesignTokens.Typography.body)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Spacer()

                        Text(L("settings.memory.max_context_help"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }

                    // Similarity Threshold
                    HStack {
                        Text(L("settings.memory.similarity_threshold"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)

                        Slider(
                            value: Binding(
                                get: { Double(memoryConfig.similarityThreshold) },
                                set: { newValue in
                                    memoryConfig.similarityThreshold = Float(newValue)
                                    updateConfig()
                                }
                            ),
                            in: 0.0...1.0,
                            step: 0.05
                        )
                        .frame(width: 200)

                        Text(String(format: "%.2f", memoryConfig.similarityThreshold))
                            .frame(width: 40)
                            .font(DesignTokens.Typography.code)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        Spacer()

                        Text(L("settings.memory.similarity_help"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Statistics Card

    private var statisticsCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.memory.statistics"), systemImage: "chart.bar.fill")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            if let stats = memoryStats {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                    HStack {
                        Text(L("settings.memory.total_memories"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)
                        Text("\(stats.totalMemories)")
                            .font(DesignTokens.Typography.body)
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    HStack {
                        Text(L("settings.memory.total_apps"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)
                        Text("\(stats.totalApps)")
                            .font(DesignTokens.Typography.body)
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    HStack {
                        Text(L("settings.memory.database_size"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)
                        Text(String(format: "%.2f MB", stats.databaseSizeMb))
                            .font(DesignTokens.Typography.body)
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    if stats.totalMemories > 0 {
                        HStack {
                            Text(L("settings.memory.date_range"))
                                .font(DesignTokens.Typography.body)
                                .frame(width: 150, alignment: .leading)
                            Text("\(formatTimestamp(stats.oldestMemoryTimestamp)) - \(formatTimestamp(stats.newestMemoryTimestamp))")
                                .font(DesignTokens.Typography.caption)
                                .fontWeight(.semibold)
                            Spacer()
                        }
                    }
                }
            } else {
                Text(L("settings.memory.loading_stats"))
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Compression Card

    private var compressionCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            HStack {
                Label(L("settings.memory.compression"), systemImage: "archivebox.fill")
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                // Compress Now button
                ActionButton(
                    isCompressing ? L("settings.memory.compressing") : L("settings.memory.compress_now"),
                    icon: isCompressing ? "hourglass" : "arrow.triangle.2.circlepath",
                    style: .primary
                ) {
                    triggerCompression()
                }
                .disabled(isCompressing)

                // Clear all compressed facts button
                ActionButton(L("settings.memory.clear_all_button"), icon: "trash.fill", style: .danger) {
                    showClearFactsConfirmation = true
                }
            }

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                if let stats = compressionStats {
                    // Raw Memories (Layer 1)
                    HStack {
                        Text(L("settings.memory.raw_memories"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)
                        Text("\(stats.totalRawMemories)")
                            .font(DesignTokens.Typography.body)
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    // Compressed Facts (Layer 2)
                    HStack {
                        Text(L("settings.memory.compressed_facts"))
                            .font(DesignTokens.Typography.body)
                            .frame(width: 150, alignment: .leading)
                        Text("\(stats.validFacts) / \(stats.totalFacts)")
                            .font(DesignTokens.Typography.body)
                            .fontWeight(.semibold)

                        Text(L("settings.memory.valid_total"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                        Spacer()
                    }

                    // Facts by type breakdown
                    if !stats.factsByType.isEmpty {
                        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                            Text(L("settings.memory.facts_by_type"))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)

                            HStack(spacing: DesignTokens.Spacing.md) {
                                ForEach(Array(stats.factsByType.keys.sorted()), id: \.self) { factType in
                                    if let count = stats.factsByType[factType], count > 0 {
                                        HStack(spacing: DesignTokens.Spacing.xs) {
                                            Image(systemName: iconForFactType(factType))
                                                .font(.caption)
                                                .foregroundColor(colorForFactType(factType))
                                            Text("\(factType): \(count)")
                                                .font(DesignTokens.Typography.caption)
                                        }
                                        .padding(.horizontal, DesignTokens.Spacing.sm)
                                        .padding(.vertical, DesignTokens.Spacing.xs)
                                        .background(colorForFactType(factType).opacity(0.15))
                                        .cornerRadius(DesignTokens.CornerRadius.small)
                                    }
                                }
                            }
                        }
                    }

                    // Last compression result
                    if let result = lastCompressionResult {
                        HStack {
                            Image(systemName: "checkmark.circle.fill")
                                .foregroundColor(DesignTokens.Colors.success)
                            Text(String(format: L("settings.memory.compression_result"),
                                        result.memoriesProcessed,
                                        result.factsExtracted,
                                        result.durationMs))
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                        }
                        .padding(.top, DesignTokens.Spacing.xs)
                    }
                } else {
                    Text(L("settings.memory.loading_compression_stats"))
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            // Compression description
            Text(L("settings.memory.compression_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Memory Browser Card

    private var memoryBrowserCard: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.memory.browser"), systemImage: "tray.fill")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                // Controls
                HStack(spacing: DesignTokens.Spacing.md) {
                    // Filter by app
                    Picker(L("settings.memory.filter"), selection: $selectedAppFilter) {
                        Text(L("settings.memory.all_apps")).tag("All Apps")

                        // Dynamic app list from database
                        ForEach(availableApps, id: \.appBundleId) { appInfo in
                            Text("\(appInfo.appBundleId) (\(appInfo.memoryCount))")
                                .tag(appInfo.appBundleId)
                        }
                    }
                    .pickerStyle(.menu)
                    .frame(width: 300)
                    .onChange(of: selectedAppFilter) {
                        loadMemories()
                    }

                    Spacer()

                    // Refresh button
                    ActionButton(L("settings.memory.refresh"), icon: "arrow.clockwise", style: .secondary) {
                        refreshData()
                    }

                    // Clear all button
                    ActionButton(L("settings.memory.clear_all_button"), icon: "trash.fill", style: .danger) {
                        showClearAllConfirmation = true
                    }
                }

                // Memory list
                if isLoading {
                    HStack {
                        Spacer()
                        ProgressView(L("settings.memory.loading_memories"))
                        Spacer()
                    }
                    .padding(.vertical, DesignTokens.Spacing.lg)
                } else if memories.isEmpty {
                    VStack(spacing: DesignTokens.Spacing.sm) {
                        Image(systemName: "tray")
                            .font(.system(size: 48))
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                        Text(L("settings.memory.no_memories"))
                            .font(DesignTokens.Typography.heading)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                        Text(L("settings.memory.no_memories_message"))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .multilineTextAlignment(.center)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, DesignTokens.Spacing.xl)
                } else {
                    ScrollView {
                        VStack(spacing: DesignTokens.Spacing.md) {
                            ForEach(memories, id: \.id) { memory in
                                MemoryEntryCard(memory: memory) {
                                    memoryToDelete = memory
                                    showDeleteConfirmation = true
                                }
                            }
                        }
                    }
                    .frame(maxHeight: 400)
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    // MARK: - Helper Methods

    /// Handle memory toggle - check model and download if needed
    private func handleMemoryToggle(_ newValue: Bool) {
        if newValue {
            // User wants to enable memory - check if model exists
            isCheckingModel = true

            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    let modelExists = try checkEmbeddingModelExists()

                    DispatchQueue.main.async {
                        isCheckingModel = false

                        if modelExists {
                            // Model exists - enable memory
                            print("[MemoryView] Model exists, enabling memory")
                            memoryConfig.enabled = true
                            updateConfig()
                        } else {
                            // Model doesn't exist - show download window
                            print("[MemoryView] Model doesn't exist, showing download window")
                            showModelDownloadWindow = true
                        }
                    }
                } catch {
                    DispatchQueue.main.async {
                        isCheckingModel = false
                        errorMessage = "Failed to check model: \(error.localizedDescription)"
                    }
                }
            }
        } else {
            // User wants to disable memory - just update config
            print("[MemoryView] Disabling memory")
            memoryConfig.enabled = false
            updateConfig()
        }
    }

    private func refreshData() {
        loadStats()
        loadCompressionStats()
        loadAppList()
        loadMemories()
    }

    private func loadCompressionStats() {
        do {
            compressionStats = try core.getCompressionStats()
        } catch {
            print("[MemoryView] Failed to load compression stats: \(error.localizedDescription)")
            // Not critical - just don't show the stats
        }
    }

    private func triggerCompression() {
        isCompressing = true
        lastCompressionResult = nil

        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let result = try core.triggerCompression()

                DispatchQueue.main.async {
                    isCompressing = false
                    lastCompressionResult = result
                    // Refresh stats after compression
                    loadCompressionStats()
                    loadStats()
                }
            } catch {
                DispatchQueue.main.async {
                    isCompressing = false
                    errorMessage = "Compression failed: \(error.localizedDescription)"
                }
            }
        }
    }

    private func iconForFactType(_ factType: String) -> String {
        switch factType.lowercased() {
        case "preference": return "heart.fill"
        case "plan": return "calendar"
        case "learning": return "book.fill"
        case "project": return "folder.fill"
        case "personal": return "person.fill"
        default: return "doc.text.fill"
        }
    }

    private func colorForFactType(_ factType: String) -> Color {
        switch factType.lowercased() {
        case "preference": return .pink
        case "plan": return .blue
        case "learning": return .green
        case "project": return .orange
        case "personal": return .purple
        default: return .gray
        }
    }

    private func loadStats() {
        do {
            memoryStats = try core.getMemoryStats()
        } catch {
            errorMessage = "Failed to load memory statistics: \(error.localizedDescription)"
        }
    }

    private func loadAppList() {
        do {
            availableApps = try core.getMemoryAppList()
        } catch {
            errorMessage = "Failed to load app list: \(error.localizedDescription)"
            availableApps = []
        }
    }

    private func loadMemories() {
        isLoading = true

        // Load memories based on filter
        do {
            if selectedAppFilter == "All Apps" {
                // Load all recent memories (limit to 50 for performance)
                memories = try core.searchMemories(
                    appBundleId: "",
                    windowTitle: nil,
                    limit: 50
                )
            } else {
                memories = try core.searchMemories(
                    appBundleId: selectedAppFilter,
                    windowTitle: nil,
                    limit: 50
                )
            }
        } catch {
            errorMessage = "Failed to load memories: \(error.localizedDescription)"
            memories = []
        }

        isLoading = false
    }

    private func updateConfig() {
        do {
            try core.updateMemoryConfig(config: memoryConfig)
            // Refresh data after config change
            refreshData()
        } catch {
            errorMessage = "Failed to update configuration: \(error.localizedDescription)"
        }
    }

    private func deleteMemory(_ memory: MemoryEntry) {
        do {
            try core.deleteMemory(id: memory.id)
            // Remove from local list
            memories.removeAll { $0.id == memory.id }
            // Refresh stats
            loadStats()
            memoryToDelete = nil
        } catch {
            errorMessage = "Failed to delete memory: \(error.localizedDescription)"
        }
    }

    private func clearAllMemories() {
        do {
            let deletedCount = try core.clearMemories(appBundleId: nil, windowTitle: nil)
            print("[MemoryView] Cleared \(deletedCount) memories")

            // Also clear all conversation topics when clearing all memories
            let topicsDeleted = ConversationStore.shared.clearAllTopics()
            print("[MemoryView] Cleared \(topicsDeleted) conversation topics")

            // Refresh data
            refreshData()
        } catch {
            errorMessage = "Failed to clear memories: \(error.localizedDescription)"
        }
    }

    private func clearAllFacts() {
        do {
            let deletedCount = try core.clearFacts()
            print("[MemoryView] Cleared \(deletedCount) compressed facts")
            // Refresh compression stats
            loadCompressionStats()
        } catch {
            errorMessage = "Failed to clear facts: \(error.localizedDescription)"
        }
    }

    private func formatTimestamp(_ timestamp: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(timestamp))
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .none
        return formatter.string(from: date)
    }
}

// MARK: - Memory Entry Card

struct MemoryEntryCard: View {
    let memory: MemoryEntry
    let onDelete: () -> Void
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text(memory.appBundleId)
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                        .lineLimit(1)

                    if !memory.windowTitle.isEmpty {
                        Text(memory.windowTitle)
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .lineLimit(1)
                    }

                    Text(formatTimestamp(memory.timestamp))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                Spacer()

                // Similarity score badge
                if let score = memory.similarityScore {
                    Text(String(format: "%.0f%%", score * 100))
                        .font(DesignTokens.Typography.caption)
                        .padding(.horizontal, DesignTokens.Spacing.sm)
                        .padding(.vertical, DesignTokens.Spacing.xs)
                        .background(DesignTokens.Colors.accentBlue.opacity(0.2))
                        .cornerRadius(DesignTokens.CornerRadius.small)
                }

                // Delete button
                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .foregroundColor(DesignTokens.Colors.error)
                        .font(DesignTokens.Typography.body)
                }
                .buttonStyle(.plain)
                .help(L("settings.memory.delete_memory_help"))
            }

            // Content preview
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text(String(format: L("settings.memory.user_prefix"), memory.userInput))
                    .font(DesignTokens.Typography.caption)
                    .lineLimit(isExpanded ? nil : 2)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Text(String(format: L("settings.memory.ai_prefix"), memory.aiOutput))
                    .font(DesignTokens.Typography.caption)
                    .lineLimit(isExpanded ? nil : 2)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
            .padding(.top, DesignTokens.Spacing.xs)

            // Expand/Collapse button
            Button(action: { isExpanded.toggle() }) {
                Text(isExpanded ? L("settings.memory.show_less") : L("settings.memory.show_more"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.accentBlue)
            }
            .buttonStyle(.plain)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground.opacity(0.5))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.ConcentricRadius.card, style: .continuous))
    }

    private func formatTimestamp(_ timestamp: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(timestamp))
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }
}

// MARK: - Preview

struct MemoryView_Previews: PreviewProvider {
    static var previews: some View {
        // Note: Preview requires AetherCore instance, which needs proper initialization
        // For now, this is a placeholder
        Text("MemoryView Preview")
    }
}
