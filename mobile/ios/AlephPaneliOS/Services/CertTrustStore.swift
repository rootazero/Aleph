import Foundation
import Security
import CryptoKit

/// Cert facts shown to the user. Display-only — the decision pins the
/// fingerprint regardless of the specific failure reason. Mirrors the Rust
/// `CertInfo` (`desktop/shell/src/cert_trust/mod.rs`).
struct CertInfo: Equatable {
    let sans: [String]
    let subject: String
    let reason: String
}

/// TOFU verdict for a presented leaf cert. Mirrors the Rust `Decision` enum.
enum CertDecision: Equatable {
    case allow
    case promptUnknown(fp: String, info: CertInfo)
    case warnChanged(old: String, new: String, info: CertInfo)
}

/// Pinned TOFU trust store: `host:port -> SHA-256 fingerprint`. Mirrors the
/// Rust `TrustStore` (`desktop/shell/src/cert_trust/store.rs`).
protocol CertStore {
    func lookup(_ host: String) -> String?
    func pin(_ host: String, _ fp: String)
}

/// Pure decision: compare the presented fingerprint against the pinned
/// store. Mirrors the Rust `decide()`. `info` is placeholder-only here — the
/// caller (Task 11's WKWebView challenge handler) does not yet have a
/// SAN/subject parser on this platform, so it threads real `CertInfo` in
/// separately when it builds the approval sheet.
func certDecision(host: String, presentedFP: String, store: CertStore) -> CertDecision {
    let info = CertInfo(sans: [], subject: "", reason: "")
    guard let pinned = store.lookup(host) else {
        return .promptUnknown(fp: presentedFP, info: info)
    }
    if pinned == presentedFP {
        return .allow
    }
    return .warnChanged(old: pinned, new: presentedFP, info: info)
}

/// Colon-grouped uppercase hex SHA-256 of the leaf DER — matches
/// `openssl x509 -fingerprint -sha256` and the Rust `fingerprint_sha256`.
func sha256Fingerprint(_ der: Data) -> String {
    SHA256.hash(data: der)
        .map { String(format: "%02X", $0) }
        .joined(separator: ":")
}

/// Keychain-backed store. The whole `host:port -> fp` map is persisted as
/// JSON in ONE generic-password item (mirrors `KeychainConnectionStore`'s
/// `SecItem*` usage). Best-effort: a missing/corrupt item decodes to an
/// empty map — fail toward "prompt", never crash, never auto-allow, same
/// contract as the Rust `TrustStore::load`.
struct KeychainCertStore: CertStore {
    private let service = "ai.aleph.panel"
    private let account = "pinned-certs"

    /// Serializes load/modify/save so two concurrent `pin()` calls (one per
    /// sub-resource TLS challenge for the same host, for instance) cannot
    /// race the read-then-write and drop a fingerprint. `lookup()` rides the
    /// same queue so a writer cannot interleave between a reader's load and
    /// the matching decision.
    private static let io = DispatchQueue(label: "ai.aleph.panel.cert-store")

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    private func loadMap() -> [String: String] {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess,
              let data = item as? Data,
              let map = try? JSONDecoder().decode([String: String].self, from: data) else {
            return [:]
        }
        return map
    }

    private func saveMap(_ map: [String: String]) {
        guard let data = try? JSONEncoder().encode(map) else { return }
        let updateStatus = SecItemUpdate(
            baseQuery as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        if updateStatus == errSecItemNotFound {
            var addQuery = baseQuery
            addQuery[kSecValueData as String] = data
            addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            SecItemAdd(addQuery as CFDictionary, nil)
        }
    }

    func lookup(_ host: String) -> String? {
        Self.io.sync { loadMap()[host] }
    }

    func pin(_ host: String, _ fp: String) {
        Self.io.sync {
            var map = loadMap()
            map[host] = fp
            saveMap(map)
        }
    }
}
