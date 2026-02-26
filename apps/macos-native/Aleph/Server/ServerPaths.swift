import Foundation

/// Filesystem path constants for the Aleph server layout.
///
/// These paths mirror the conventions used by `aleph-server` and the Tauri bridge:
/// - `~/.aleph/`                          data directory
/// - `~/.aleph/bridge.sock`               UDS socket (server <-> bridge)
/// - `~/Library/Application Support/aleph/` config (macOS)
/// - `~/.config/aleph/`                   config (fallback)
enum ServerPaths {
    /// ~/.aleph/
    static var alephHome: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".aleph")
    }

    /// ~/.aleph/bridge.sock
    static var bridgeSocket: URL {
        alephHome.appendingPathComponent("bridge.sock")
    }

    /// Path to aleph-server binary in app bundle
    static var serverBinary: URL? {
        Bundle.main.url(forResource: "aleph-server", withExtension: nil)
    }

    /// ~/Library/Application Support/aleph/ or ~/.config/aleph/
    static var configDir: URL {
        if let appSupport = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first
        {
            return appSupport.appendingPathComponent("aleph")
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aleph")
    }

    /// settings.json path
    static var settingsFile: URL {
        configDir.appendingPathComponent("settings.json")
    }

    /// Ensure required directories exist.
    static func ensureDirectories() throws {
        let fm = FileManager.default
        try fm.createDirectory(at: alephHome, withIntermediateDirectories: true)
        try fm.createDirectory(at: configDir, withIntermediateDirectories: true)
    }
}
