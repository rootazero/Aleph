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
    @State private var memoryConfig: MemoryConfig
    @State private var memoryStats: MemoryStats?
    @State private var memories: [MemoryEntry] = []
    @State private var selectedAppFilter: String = "All Apps"
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var showDeleteConfirmation = false
    @State private var memoryToDelete: MemoryEntry?
    @State private var showClearAllConfirmation = false

    let core: AetherCore

    init(core: AetherCore) {
        self.core = core
        // Load initial config
        _memoryConfig = State(initialValue: core.getMemoryConfig())
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                // Header
                headerSection

                // Configuration Section
                configurationSection

                // Statistics Section
                if memoryConfig.enabled {
                    statisticsSection

                    // Memory Browser Section
                    memoryBrowserSection
                }
            }
            .padding(20)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear {
            refreshData()
        }
        .alert("Error", isPresented: .constant(errorMessage != nil)) {
            Button("OK") {
                errorMessage = nil
            }
        } message: {
            if let error = errorMessage {
                Text(error)
            }
        }
        .alert("Delete Memory", isPresented: $showDeleteConfirmation) {
            Button("Cancel", role: .cancel) {
                memoryToDelete = nil
            }
            Button("Delete", role: .destructive) {
                if let memory = memoryToDelete {
                    deleteMemory(memory)
                }
            }
        } message: {
            Text("Are you sure you want to delete this memory? This action cannot be undone.")
        }
        .alert("Clear All Memories", isPresented: $showClearAllConfirmation) {
            Button("Cancel", role: .cancel) {}
            Button("Clear All", role: .destructive) {
                clearAllMemories()
            }
        } message: {
            Text("Are you sure you want to delete ALL memories? This action cannot be undone.")
        }
    }

    // MARK: - Header Section

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Memory Management")
                .font(.title)
                .fontWeight(.bold)

            Text("Aether remembers past interactions to provide context-aware responses. All data is stored locally and never leaves your device.")
                .font(.subheadline)
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Configuration Section

    private var configurationSection: some View {
        GroupBox(label: Label("Configuration", systemImage: "gearshape.fill")) {
            VStack(alignment: .leading, spacing: 16) {
                // Enable/Disable Toggle
                Toggle("Enable Memory", isOn: Binding(
                    get: { memoryConfig.enabled },
                    set: { newValue in
                        memoryConfig.enabled = newValue
                        updateConfig()
                    }
                ))
                .toggleStyle(.switch)

                if memoryConfig.enabled {
                    Divider()

                    // Retention Policy
                    HStack {
                        Text("Retention Policy:")
                            .frame(width: 150, alignment: .leading)

                        Picker("", selection: Binding(
                            get: { memoryConfig.retentionDays },
                            set: { newValue in
                                memoryConfig.retentionDays = newValue
                                updateConfig()
                            }
                        )) {
                            Text("7 days").tag(UInt32(7))
                            Text("30 days").tag(UInt32(30))
                            Text("90 days").tag(UInt32(90))
                            Text("1 year").tag(UInt32(365))
                            Text("Never").tag(UInt32(0))
                        }
                        .pickerStyle(.menu)
                        .frame(width: 150)

                        Spacer()

                        Text("Auto-delete memories older than selected period")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    // Max Context Items
                    HStack {
                        Text("Max Context Items:")
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
                            .foregroundColor(.secondary)

                        Spacer()

                        Text("Number of past interactions to retrieve")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    // Similarity Threshold
                    HStack {
                        Text("Similarity Threshold:")
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
                            .foregroundColor(.secondary)

                        Spacer()

                        Text("Minimum similarity score to include memory")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
            }
            .padding(.vertical, 8)
        }
    }

    // MARK: - Statistics Section

    private var statisticsSection: some View {
        GroupBox(label: Label("Statistics", systemImage: "chart.bar.fill")) {
            if let stats = memoryStats {
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        Text("Total Memories:")
                            .frame(width: 150, alignment: .leading)
                        Text("\(stats.totalMemories)")
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    HStack {
                        Text("Total Apps:")
                            .frame(width: 150, alignment: .leading)
                        Text("\(stats.totalApps)")
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    HStack {
                        Text("Database Size:")
                            .frame(width: 150, alignment: .leading)
                        Text(String(format: "%.2f MB", stats.databaseSizeMb))
                            .fontWeight(.semibold)
                        Spacer()
                    }

                    if stats.totalMemories > 0 {
                        HStack {
                            Text("Date Range:")
                                .frame(width: 150, alignment: .leading)
                            Text("\(formatTimestamp(stats.oldestMemoryTimestamp)) - \(formatTimestamp(stats.newestMemoryTimestamp))")
                                .fontWeight(.semibold)
                            Spacer()
                        }
                    }
                }
                .padding(.vertical, 8)
            } else {
                Text("Loading statistics...")
                    .foregroundColor(.secondary)
                    .padding(.vertical, 8)
            }
        }
    }

    // MARK: - Memory Browser Section

    private var memoryBrowserSection: some View {
        GroupBox(label: Label("Memory Browser", systemImage: "tray.fill")) {
            VStack(alignment: .leading, spacing: 16) {
                // Controls
                HStack {
                    // Filter by app
                    Picker("Filter:", selection: $selectedAppFilter) {
                        Text("All Apps").tag("All Apps")
                        // TODO: Add dynamic app list from database
                    }
                    .pickerStyle(.menu)
                    .frame(width: 200)
                    .onChange(of: selectedAppFilter) { _ in
                        loadMemories()
                    }

                    Spacer()

                    // Refresh button
                    Button(action: refreshData) {
                        Label("Refresh", systemImage: "arrow.clockwise")
                    }

                    // Clear all button
                    Button(action: {
                        showClearAllConfirmation = true
                    }) {
                        Label("Clear All", systemImage: "trash.fill")
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(.red)
                }

                Divider()

                // Memory list
                if isLoading {
                    HStack {
                        Spacer()
                        ProgressView("Loading memories...")
                        Spacer()
                    }
                    .padding(.vertical, 20)
                } else if memories.isEmpty {
                    VStack(spacing: 8) {
                        Image(systemName: "tray")
                            .font(.system(size: 48))
                            .foregroundColor(.secondary)
                        Text("No memories stored yet")
                            .font(.headline)
                            .foregroundColor(.secondary)
                        Text("Memories will appear here after you use Aether with memory enabled.")
                            .font(.caption)
                            .foregroundColor(.secondary)
                            .multilineTextAlignment(.center)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 40)
                } else {
                    ScrollView {
                        VStack(spacing: 12) {
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
            .padding(.vertical, 8)
        }
    }

    // MARK: - Helper Methods

    private func refreshData() {
        loadStats()
        loadMemories()
    }

    private func loadStats() {
        do {
            memoryStats = try core.getMemoryStats()
        } catch {
            errorMessage = "Failed to load memory statistics: \(error.localizedDescription)"
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
            // Refresh data
            refreshData()
        } catch {
            errorMessage = "Failed to clear memories: \(error.localizedDescription)"
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
        VStack(alignment: .leading, spacing: 8) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text(memory.appBundleId)
                        .font(.headline)
                        .lineLimit(1)

                    if !memory.windowTitle.isEmpty {
                        Text(memory.windowTitle)
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }

                    Text(formatTimestamp(memory.timestamp))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                Spacer()

                // Similarity score badge
                if let score = memory.similarityScore {
                    Text(String(format: "%.0f%%", score * 100))
                        .font(.caption)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color.blue.opacity(0.2))
                        .cornerRadius(4)
                }

                // Delete button
                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
                .help("Delete this memory")
            }

            // Content preview
            VStack(alignment: .leading, spacing: 4) {
                Text("User: \(memory.userInput)")
                    .font(.caption)
                    .lineLimit(isExpanded ? nil : 2)
                    .foregroundColor(.primary)

                Text("AI: \(memory.aiOutput)")
                    .font(.caption)
                    .lineLimit(isExpanded ? nil : 2)
                    .foregroundColor(.secondary)
            }
            .padding(.top, 4)

            // Expand/Collapse button
            Button(action: { isExpanded.toggle() }) {
                Text(isExpanded ? "Show Less" : "Show More")
                    .font(.caption)
                    .foregroundColor(.accentColor)
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(Color.gray.opacity(0.1))
        .cornerRadius(8)
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
