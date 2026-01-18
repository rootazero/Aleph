//
//  ClipboardMonitorProtocol.swift
//  Aether
//
//  Protocol for clipboard change monitoring, enabling dependency injection and testability.
//

import Foundation

/// Protocol for clipboard change monitoring
///
/// Abstracts clipboard monitoring for dependency injection and testing.
/// The default implementation is ClipboardMonitor.
protocol ClipboardMonitorProtocol: AnyObject {

    /// Whether monitoring is currently active
    var isMonitoring: Bool { get }

    /// Time threshold for "recent" clipboard content (in seconds)
    var recentThresholdSeconds: TimeInterval { get }

    /// Whether clipboard has recent content
    var hasRecentContent: Bool { get }

    /// Time since last clipboard change
    var timeSinceLastChange: TimeInterval? { get }

    /// Start monitoring clipboard changes
    func startMonitoring()

    /// Stop monitoring clipboard changes
    func stopMonitoring()

    /// Get recent clipboard content (if within threshold)
    func getRecentClipboardContent() -> String?

    /// Check if clipboard was changed within the recent threshold
    /// Use this to determine if media attachments should be included
    func isClipboardRecent() -> Bool

    /// Clear recorded clipboard history
    func clearHistory()
}

// MARK: - Default Implementation Conformance

extension ClipboardMonitor: ClipboardMonitorProtocol {}
