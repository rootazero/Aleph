//
//  DebugLogger.swift
//  Aether
//
//  Simple file-based debug logger for tracking execution flow
//

import Foundation

/// Simple file-based debug logger
/// Writes logs to ~/Library/Logs/Aether/debug.log
enum DebugLogger {
    private static let logFile: URL = {
        let logsDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/Aether")
        try? FileManager.default.createDirectory(at: logsDir, withIntermediateDirectories: true)
        return logsDir.appendingPathComponent("debug.log")
    }()

    private static let dateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss.SSS"
        return formatter
    }()

    static func log(_ message: String, file: String = #file, function: String = #function, line: Int = #line) {
        let timestamp = dateFormatter.string(from: Date())
        let fileName = (file as NSString).lastPathComponent
        let logLine = "[\(timestamp)] [\(fileName):\(line)] \(message)\n"

        // Also print to console for debugging
        print(logLine, terminator: "")

        // Append to file
        if let data = logLine.data(using: .utf8) {
            if FileManager.default.fileExists(atPath: logFile.path) {
                if let fileHandle = try? FileHandle(forWritingTo: logFile) {
                    fileHandle.seekToEndOfFile()
                    fileHandle.write(data)
                    fileHandle.closeFile()
                }
            } else {
                try? data.write(to: logFile)
            }
        }
    }

    static func clearLog() {
        try? FileManager.default.removeItem(at: logFile)
    }
}
