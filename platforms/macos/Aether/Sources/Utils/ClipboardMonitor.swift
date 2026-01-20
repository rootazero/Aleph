// ClipboardMonitor.swift
// Monitors clipboard changes and tracks timestamps
//
// This manager tracks when the clipboard was last modified,
// allowing us to determine if clipboard content is "recent" (within 10 seconds)
// and should be used as context for AI prompts.

import Cocoa
import Foundation

/// Clipboard change event data
struct ClipboardChange {
    /// Text content (nil for non-text clipboard content like images)
    let content: String?
    let timestamp: Date
    let changeCount: Int
}

/// Monitors clipboard changes and tracks timestamps
///
/// Thread Safety:
/// - Marked as @MainActor since Timer and NSPasteboard operations require main thread
@MainActor
class ClipboardMonitor {

    // MARK: - Singleton

    static let shared = ClipboardMonitor()

    // MARK: - Properties

    /// Last clipboard change event
    private var lastChange: ClipboardChange?

    /// Timer for periodic clipboard checking
    private var monitorTimer: Timer?

    /// Last known changeCount
    private var lastChangeCount: Int = 0

    /// Whether monitoring is currently active
    private(set) var isMonitoring: Bool = false

    /// Time threshold for "recent" clipboard content (in seconds)
    let recentThresholdSeconds: TimeInterval = 10.0

    /// Clipboard manager for clipboard operations (lazy to avoid circular dependency)
    private var clipboardManager: any ClipboardManagerProtocol {
        DependencyContainer.shared.clipboardManager
    }

    // MARK: - Initialization

    private init() {
        // Initialize with current clipboard state
        // Use NSPasteboard directly here to avoid accessing DependencyContainer during init
        lastChangeCount = NSPasteboard.general.changeCount
    }

    // MARK: - Monitoring Control

    /// Start monitoring clipboard changes
    func startMonitoring() {
        guard !isMonitoring else {
            print("[ClipboardMonitor] Already monitoring")
            return
        }

        print("[ClipboardMonitor] Starting clipboard monitoring (checking every 1 second)")

        // Check immediately
        checkClipboardChange()

        // Set up timer to check every second
        // Note: Timer fires on main thread but closure isn't MainActor-isolated by default
        // Use assumeIsolated since Timer.scheduledTimer on main RunLoop guarantees main thread execution
        monitorTimer = Timer.scheduledTimer(
            withTimeInterval: 1.0,
            repeats: true
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.checkClipboardChange()
            }
        }

        isMonitoring = true
    }

    /// Stop monitoring clipboard changes
    func stopMonitoring() {
        guard isMonitoring else { return }

        print("[ClipboardMonitor] Stopping clipboard monitoring")
        monitorTimer?.invalidate()
        monitorTimer = nil
        isMonitoring = false
    }

    // MARK: - Change Detection

    /// Check if clipboard has changed since last check
    private func checkClipboardChange() {
        let currentChangeCount = clipboardManager.changeCount()

        // Check if clipboard changed
        guard currentChangeCount != lastChangeCount else {
            return // No change
        }

        // Clipboard changed - record the event
        lastChangeCount = currentChangeCount

        // Get current clipboard content (may be nil for non-text content like images)
        let content = clipboardManager.getText()

        // Record the change (even for non-text content, we need the timestamp)
        let change = ClipboardChange(
            content: content,
            timestamp: Date(),
            changeCount: currentChangeCount
        )

        lastChange = change

        if let content = content {
            print("[ClipboardMonitor] Clipboard changed (count: \(currentChangeCount), text: \(content.prefix(30))...)")
        } else {
            print("[ClipboardMonitor] Clipboard changed (count: \(currentChangeCount), non-text content)")
        }
    }

    // MARK: - Query Methods

    /// Get recent clipboard content if it was changed within the threshold
    ///
    /// - Returns: Clipboard content if it was changed within `recentThresholdSeconds`, nil otherwise
    func getRecentClipboardContent() -> String? {
        guard let change = lastChange else {
            return nil // No recorded change
        }

        // CRITICAL: Check if clipboard has changed since we recorded this change.
        // If user copied new content after this was recorded,
        // changeCount will be different, meaning our recorded content is stale.
        let currentChangeCount = clipboardManager.changeCount()
        if change.changeCount != currentChangeCount {
            print("[ClipboardMonitor] Clipboard changeCount mismatch (\(change.changeCount) vs \(currentChangeCount)) - content is stale")
            return nil
        }

        // Check if this change had text content
        guard let content = change.content else {
            return nil // Non-text content (e.g., image)
        }

        let elapsed = Date().timeIntervalSince(change.timestamp)

        guard elapsed <= recentThresholdSeconds else {
            print("[ClipboardMonitor] Clipboard content too old (\(Int(elapsed))s > \(Int(recentThresholdSeconds))s)")
            return nil
        }

        print("[ClipboardMonitor] Found recent clipboard content (\(Int(elapsed))s ago)")
        return content
    }

    /// Check if clipboard was changed within the recent threshold
    ///
    /// This method checks if ANY clipboard change (text or media) occurred within
    /// the threshold. Use this to determine if media attachments should be included.
    ///
    /// - Returns: true if clipboard was changed within `recentThresholdSeconds`
    func isClipboardRecent() -> Bool {
        guard let change = lastChange else {
            // No recorded change - cannot determine recency without timestamp
            return false
        }

        // Check if clipboard has changed since we recorded this change.
        let currentChangeCount = clipboardManager.changeCount()
        if change.changeCount != currentChangeCount {
            // Clipboard changed since last recording - we don't have
            // timestamp for the new content, so we can't determine recency.
            // To be safe, return false to avoid including stale content.
            print("[ClipboardMonitor] Clipboard changeCount mismatch - cannot determine recency")
            return false
        }

        let elapsed = Date().timeIntervalSince(change.timestamp)
        let isRecent = elapsed <= recentThresholdSeconds

        if !isRecent {
            print("[ClipboardMonitor] Clipboard too old for attachments (\(Int(elapsed))s > \(Int(recentThresholdSeconds))s)")
        } else {
            let contentType = change.content != nil ? "text" : "non-text"
            print("[ClipboardMonitor] Clipboard is recent (\(Int(elapsed))s, \(contentType) content)")
        }

        return isRecent
    }

    /// Check if clipboard has recent content
    var hasRecentContent: Bool {
        return getRecentClipboardContent() != nil
    }

    /// Get time since last clipboard change
    var timeSinceLastChange: TimeInterval? {
        guard let change = lastChange else { return nil }
        return Date().timeIntervalSince(change.timestamp)
    }

    /// Clear recorded clipboard history
    func clearHistory() {
        lastChange = nil
        print("[ClipboardMonitor] Clipboard history cleared")
    }
}
