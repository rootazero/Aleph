//
//  LogViewerView.swift
//  Aether
//
//  Log viewer UI with search, export, and clear functionality.
//

import SwiftUI
import UniformTypeIdentifiers
import Compression

struct LogViewerView: View {
    let core: AetherCore
    @State private var logContent: String = ""
    @State private var searchText: String = ""
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?
    @State private var showingClearConfirmation: Bool = false
    @State private var logLevel: String = "info"
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            // Toolbar
            HStack {
                Text("Logs")
                    .font(.headline)

                Spacer()

                // Log level picker
                Picker("", selection: $logLevel) {
                    Text("Debug").tag("debug")
                    Text("Info").tag("info")
                    Text("Warn").tag("warn")
                    Text("Error").tag("error")
                }
                .pickerStyle(.segmented)
                .frame(width: 250)
                .onChange(of: logLevel) { _, newLevel in
                    do {
                        // Convert string to LogLevel enum
                        let level: LogLevel
                        switch newLevel {
                        case "error": level = .error
                        case "warn": level = .warn
                        case "info": level = .info
                        case "debug": level = .debug
                        default: level = .info
                        }
                        try core.setLogLevel(level: level)
                        loadLogs()
                    } catch {
                        errorMessage = "Failed to set log level: \(error.localizedDescription)"
                    }
                }

                Spacer()

                // Action buttons
                Button(action: exportLogs) {
                    Label("Export", systemImage: "square.and.arrow.up")
                }
                .help(L("logs.help.export"))

                Button(action: { showingClearConfirmation = true }) {
                    Label("Clear", systemImage: "trash")
                }
                .help(L("logs.help.clear"))

                Button(action: loadLogs) {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .help(L("logs.help.reload"))

                Button(action: { dismiss() }) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
                .help(L("logs.help.close"))
            }
            .padding()
            .background(Color(NSColor.controlBackgroundColor))

            Divider()

            // Search bar
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundColor(.secondary)
                TextField("Search logs...", text: $searchText)
                    .textFieldStyle(.plain)
                if !searchText.isEmpty {
                    Button(action: { searchText = "" }) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(8)
            .background(Color(NSColor.textBackgroundColor))
            .cornerRadius(6)
            .padding(.horizontal)
            .padding(.vertical, 8)

            // Log content area
            if isLoading {
                ProgressView("Loading logs...")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error = errorMessage {
                VStack(spacing: 12) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.system(size: 48))
                        .foregroundColor(.orange)
                    Text("Error Loading Logs")
                        .font(.headline)
                    Text(error)
                        .foregroundColor(.secondary)
                        .multilineTextAlignment(.center)
                    Button("Retry") {
                        loadLogs()
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding()
            } else {
                ScrollView {
                    Text(filteredLogContent)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding()
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color(NSColor.textBackgroundColor))
            }
        }
        .frame(width: 900, height: 600)
        .onAppear {
            loadLogs()
            loadCurrentLogLevel()
        }
        .alert(L("alert.logs.clear_title"), isPresented: $showingClearConfirmation) {
            Button(L("common.cancel"), role: .cancel) {}
            Button(L("alert.logs.clear_button"), role: .destructive) {
                clearLogs()
            }
        } message: {
            Text(L("settings.memory.clear_all_message"))
        }
    }

    // MARK: - Computed Properties

    private var filteredLogContent: AttributedString {
        let lines = logContent.components(separatedBy: .newlines)

        // Filter by log level first
        let levelFiltered = lines.filter { line in
            filterByLogLevel(line)
        }

        // Then filter by search text
        let filtered = searchText.isEmpty
            ? levelFiltered
            : levelFiltered.filter { $0.localizedCaseInsensitiveContains(searchText) }

        // Apply syntax highlighting
        var result = AttributedString()
        for line in filtered {
            var attributedLine = AttributedString(line + "\n")

            // Color by log level
            if line.contains("ERROR") {
                attributedLine.foregroundColor = .red
            } else if line.contains("WARN") {
                attributedLine.foregroundColor = .orange
            } else if line.contains("DEBUG") {
                attributedLine.foregroundColor = .gray
            } else {
                attributedLine.foregroundColor = .primary
            }

            // Highlight search matches
            if !searchText.isEmpty && line.localizedCaseInsensitiveContains(searchText) {
                attributedLine.backgroundColor = Color.yellow.opacity(0.3)
            }

            result.append(attributedLine)
        }

        return result
    }

    /// Filter log line based on current log level selection
    /// Log levels hierarchy: error > warn > info > debug
    /// When selecting a level, show that level and all levels above it
    private func filterByLogLevel(_ line: String) -> Bool {
        // Determine the log level of this line
        let lineLevel: Int
        if line.contains("ERROR") {
            lineLevel = 3  // error
        } else if line.contains("WARN") {
            lineLevel = 2  // warn
        } else if line.contains("DEBUG") || line.contains("TRACE") {
            lineLevel = 0  // debug
        } else if line.contains("INFO") {
            lineLevel = 1  // info
        } else {
            // Lines without level marker (e.g., continuation lines) - show based on context
            // Default to showing them
            return true
        }

        // Determine the minimum level to show based on current selection
        let minLevel: Int
        switch logLevel {
        case "error": minLevel = 3
        case "warn": minLevel = 2
        case "info": minLevel = 1
        case "debug": minLevel = 0
        default: minLevel = 1
        }

        return lineLevel >= minLevel
    }

    // MARK: - Actions

    private func loadLogs() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let logDir = try core.getLogDirectory()
                let content = try await readRecentLogs(from: logDir)

                await MainActor.run {
                    logContent = content
                    isLoading = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                    isLoading = false
                }
            }
        }
    }

    private func loadCurrentLogLevel() {
        let level = core.getLogLevel()
        // Convert LogLevel enum to string
        switch level {
        case .error: logLevel = "error"
        case .warn: logLevel = "warn"
        case .info: logLevel = "info"
        case .debug: logLevel = "debug"
        case .trace: logLevel = "debug"  // Map trace to debug for UI simplicity
        }
    }

    private func exportLogs() {
        Task {
            do {
                let logDir = try core.getLogDirectory()
                let zipURL = try await createLogsZip(from: logDir)

                await MainActor.run {
                    // Show save panel
                    let savePanel = NSSavePanel()
                    savePanel.allowedContentTypes = [UTType.zip]
                    savePanel.nameFieldStringValue = "aether-logs-\(formattedDate()).zip"
                    savePanel.message = "Export Aether logs"

                    savePanel.begin { response in
                        if response == .OK, let url = savePanel.url {
                            do {
                                // Move temp zip to selected location
                                try FileManager.default.copyItem(at: zipURL, to: url)

                                // Show success notification
                                let alert = NSAlert()
                                alert.messageText = L("alert.logs.exported_title")
                                alert.informativeText = L("alert.logs.exported_message", url.path)
                                alert.alertStyle = .informational
                                alert.addButton(withTitle: L("common.ok"))
                                alert.runModal()
                            } catch {
                                showError("Failed to save logs: \(error.localizedDescription)")
                            }
                        }

                        // Clean up temp file
                        try? FileManager.default.removeItem(at: zipURL)
                    }
                }
            } catch {
                await MainActor.run {
                    showError("Failed to export logs: \(error.localizedDescription)")
                }
            }
        }
    }

    private func clearLogs() {
        Task {
            do {
                let logDir = try core.getLogDirectory()
                let logDirURL = URL(fileURLWithPath: logDir)

                // Get all log files
                // Log files have format: aether.log.YYYY-MM-DD (from tracing-appender rolling)
                let fileManager = FileManager.default
                let logFiles = try fileManager.contentsOfDirectory(
                    at: logDirURL,
                    includingPropertiesForKeys: nil
                ).filter { $0.lastPathComponent.hasPrefix("aether.log") }

                // Delete each log file
                for file in logFiles {
                    try fileManager.removeItem(at: file)
                }

                await MainActor.run {
                    logContent = ""

                    // Show success alert
                    let alert = NSAlert()
                    alert.messageText = L("alert.logs.cleared_title")
                    alert.informativeText = L("alert.logs.cleared_message", logFiles.count)
                    alert.alertStyle = .informational
                    alert.addButton(withTitle: L("common.ok"))
                    alert.runModal()
                }
            } catch {
                await MainActor.run {
                    showError("Failed to clear logs: \(error.localizedDescription)")
                }
            }
        }
    }

    private func showError(_ message: String) {
        let alert = NSAlert()
        alert.messageText = L("error.title")
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: L("common.ok"))
        alert.runModal()
    }

    // MARK: - Helpers

    private func readRecentLogs(from directory: String) async throws -> String {
        let logDirURL = URL(fileURLWithPath: directory)
        let fileManager = FileManager.default

        // Check if log directory exists
        var isDirectory: ObjCBool = false
        let exists = fileManager.fileExists(atPath: logDirURL.path, isDirectory: &isDirectory)

        if !exists {
            // Create log directory if it doesn't exist
            try fileManager.createDirectory(at: logDirURL, withIntermediateDirectories: true)
            return L("logs.empty.first_run")
        }

        if !isDirectory.boolValue {
            throw NSError(domain: "LogViewerError", code: 2, userInfo: [
                NSLocalizedDescriptionKey: "Log path exists but is not a directory: \(logDirURL.path)"
            ])
        }

        // Get all log files sorted by modification date
        // Log files have format: aether.log.YYYY-MM-DD (from tracing-appender rolling)
        let logFiles = try fileManager.contentsOfDirectory(
            at: logDirURL,
            includingPropertiesForKeys: [.contentModificationDateKey]
        )
        .filter { $0.lastPathComponent.hasPrefix("aether.log") }
        .sorted { file1, file2 in
            let date1 = try? file1.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
            let date2 = try? file2.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
            return (date1 ?? Date.distantPast) > (date2 ?? Date.distantPast)
        }

        guard !logFiles.isEmpty else {
            return L("logs.empty.no_logs")
        }

        // Read the most recent log file (last 1000 lines)
        let latestLog = logFiles[0]
        let content = try String(contentsOf: latestLog, encoding: .utf8)
        let lines = content.components(separatedBy: .newlines)

        // Take last 1000 lines
        let recentLines = lines.suffix(1000)
        return recentLines.joined(separator: "\n")
    }

    private func createLogsZip(from directory: String) async throws -> URL {
        let logDirURL = URL(fileURLWithPath: directory)
        let fileManager = FileManager.default

        // Get all log files from last 3 days
        // Log files have format: aether.log.YYYY-MM-DD (from tracing-appender rolling)
        let cutoffDate = Date().addingTimeInterval(-3 * 24 * 60 * 60)
        let logFiles = try fileManager.contentsOfDirectory(
            at: logDirURL,
            includingPropertiesForKeys: [.contentModificationDateKey]
        )
        .filter { url in
            guard url.lastPathComponent.hasPrefix("aether.log") else { return false }
            let date = try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
            return (date ?? Date.distantPast) > cutoffDate
        }

        // Create temporary directory for ZIP
        let tempDir = fileManager.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try fileManager.createDirectory(at: tempDir, withIntermediateDirectories: true)

        // Copy log files to temp directory
        for logFile in logFiles {
            let destURL = tempDir.appendingPathComponent(logFile.lastPathComponent)
            try fileManager.copyItem(at: logFile, to: destURL)
        }

        // Create ZIP archive
        let zipURL = fileManager.temporaryDirectory.appendingPathComponent("aether-logs.zip")
        try? fileManager.removeItem(at: zipURL) // Remove if exists

        // Use system zip command
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/zip")
        process.arguments = ["-r", zipURL.path, "."]
        process.currentDirectoryURL = tempDir

        try process.run()
        process.waitUntilExit()

        // Clean up temp directory
        try fileManager.removeItem(at: tempDir)

        guard process.terminationStatus == 0 else {
            throw NSError(domain: "LogViewerError", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "Failed to create ZIP archive"
            ])
        }

        return zipURL
    }

    private func formattedDate() -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd-HHmmss"
        return formatter.string(from: Date())
    }
}

// MARK: - Preview

struct LogViewerView_Previews: PreviewProvider {
    static var previews: some View {
        if let core = try? initCore(configPath: "", handler: PreviewEventHandler()) {
            LogViewerView(core: core)
        } else {
            Text("Preview unavailable")
        }
    }
}

/// Event handler for SwiftUI Preview
class PreviewEventHandler: AetherEventHandler {
    func onThinking() {}
    func onToolStart(toolName: String) {}
    func onToolResult(toolName: String, result: String) {}
    func onStreamChunk(text: String) {}
    func onComplete(response: String) {}
    func onError(message: String) {}
    func onMemoryStored() {}
    func onAgentModeDetected(task: ExecutableTaskFfi) {}
    func onToolsChanged(toolCount: UInt32) {}
    func onMcpStartupComplete(report: McpStartupReportFfi) {}
    // Phase 5 callbacks
    func onSessionStarted(sessionId: String) {}
    func onToolCallStarted(callId: String, toolName: String) {}
    func onToolCallCompleted(callId: String, output: String) {}
    func onToolCallFailed(callId: String, error: String, isRetryable: Bool) {}
    func onLoopProgress(sessionId: String, iteration: UInt32, status: String) {}
    func onPlanCreated(sessionId: String, steps: [String]) {}
    func onSessionCompleted(sessionId: String, summary: String) {}
    func onSubagentStarted(parentSessionId: String, childSessionId: String, agentId: String) {}
    func onSubagentCompleted(childSessionId: String, success: Bool, summary: String) {}
    // Phase 7 callbacks
    func onRuntimeUpdatesAvailable(updates: [RuntimeUpdateInfo]) {}
    // DAG Plan Confirmation callback
    func onPlanConfirmationRequired(planId: String, plan: DagTaskPlan) {}
}
