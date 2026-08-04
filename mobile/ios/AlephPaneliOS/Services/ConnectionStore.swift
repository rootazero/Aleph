import Foundation
import Security

/// Persists the chosen connection target (full Gateway URL, including its
/// `?token=` secret). The target is transport config — R2/R4 keep all business
/// state in the WASM panel.
protocol ConnectionStoring {
    func load() -> URL?
    func save(_ url: URL) throws
}

enum KeychainError: Error {
    case unexpectedStatus(OSStatus)
}

/// Keychain-backed store. The URL carries the token, so it lives in the
/// Keychain (never UserDefaults) per the Swift security rules.
struct KeychainConnectionStore: ConnectionStoring {
    private let service = "ai.aleph.panel"
    private let account = "pairing-url"

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    func load() -> URL? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess,
              let data = item as? Data,
              let string = String(data: data, encoding: .utf8),
              let url = URL(string: string) else {
            return nil
        }
        return url
    }

    func save(_ url: URL) throws {
        let data = Data(url.absoluteString.utf8)
        // Try update first; if nothing to update, add.
        let updateStatus = SecItemUpdate(
            baseQuery as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        if updateStatus == errSecItemNotFound {
            var addQuery = baseQuery
            addQuery[kSecValueData as String] = data
            addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
            guard addStatus == errSecSuccess else {
                throw KeychainError.unexpectedStatus(addStatus)
            }
            return
        }
        throw KeychainError.unexpectedStatus(updateStatus)
    }
}
