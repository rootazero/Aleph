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
                Picker("Level:", selection: $logLevel) {
                    Text("Debug").tag("debug")
                    Text("Info").tag("info")
                    Text("Warn").tag("warn")
                    Text("Error").tag("error")
                }
                .pickerStyle(.segmented)
                .frame(width: 250)
                .onChange(of: logLevel) { newLevel in
                    do {
                        try core.setLogLevel(level: newLevel)
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
                .help("Export logs as ZIP file")

                Button(action: { showingClearConfirmation = true }) {
                    Label("Clear", systemImage: "trash")
                }
                .help("Delete all log files")

                Button(action: loadLogs) {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .help("Reload logs")

                Button(action: { dismiss() }) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
                .help("Close log viewer")
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
        .alert("Clear All Logs?", isPresented: $showingClearConfirmation) {
            Button("Cancel", role: .cancel) {}
            Button("Clear", role: .destructive) {
                clearLogs()
            }
        } message: {
            Text("This will permanently delete all log files. This action cannot be undone.")
        }
    }

    // MARK: - Computed Properties

    private var filteredLogContent: AttributedString {
        let lines = logContent.components(separatedBy: .newlines)
        let filtered = searchText.isEmpty
            ? lines
            : lines.filter { $0.localizedCaseInsensitiveContains(searchText) }

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
        do {
            logLevel = try core.getLogLevel()
        } catch {
            print("Failed to get log level: \(error)")
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
                                alert.messageText = "Logs Exported"
                                alert.informativeText = "Logs saved to: \(url.path)"
                                alert.alertStyle = .informational
                                alert.addButton(withTitle: "OK")
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
                let fileManager = FileManager.default
                let logFiles = try fileManager.contentsOfDirectory(
                    at: logDirURL,
                    includingPropertiesForKeys: nil
                ).filter { $0.pathExtension == "log" }

                // Delete each log file
                for file in logFiles {
                    try fileManager.removeItem(at: file)
                }

                await MainActor.run {
                    logContent = ""

                    // Show success alert
                    let alert = NSAlert()
                    alert.messageText = "Logs Cleared"
                    alert.informativeText = "Deleted \(logFiles.count) log file(s)."
                    alert.alertStyle = .informational
                    alert.addButton(withTitle: "OK")
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
        alert.messageText = "Error"
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    // MARK: - Helpers

    private func readRecentLogs(from directory: String) async throws -> String {
        let logDirURL = URL(fileURLWithPath: directory)
        let fileManager = FileManager.default

        // Get all log files sorted by modification date
        let logFiles = try fileManager.contentsOfDirectory(
            at: logDirURL,
            includingPropertiesForKeys: [.contentModificationDateKey]
        )
        .filter { $0.pathExtension == "log" }
        .sorted { file1, file2 in
            let date1 = try? file1.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
            let date2 = try? file2.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
            return (date1 ?? Date.distantPast) > (date2 ?? Date.distantPast)
        }

        guard !logFiles.isEmpty else {
            return "No log files found."
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
        let cutoffDate = Date().addingTimeInterval(-3 * 24 * 60 * 60)
        let logFiles = try fileManager.contentsOfDirectory(
            at: logDirURL,
            includingPropertiesForKeys: [.contentModificationDateKey]
        )
        .filter { url in
            guard url.pathExtension == "log" else { return false }
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
        if let core = try? AetherCore(handler: PreviewEventHandler()) {
            LogViewerView(core: core)
        } else {
            Text("Preview unavailable")
        }
    }
}

class PreviewEventHandler: AetherEventHandler {
    func onStateChanged(state: ProcessingState) {}
    func onHaloShow(position: HaloPosition, providerColor: String?) {}
    func onHaloHide() {}
    func onError(message: String) {}
}
