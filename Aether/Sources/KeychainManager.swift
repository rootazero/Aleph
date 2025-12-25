//
//  KeychainManager.swift
//  Aether
//
//  Secure API key storage using macOS Keychain via Security.framework
//  Implements the KeychainManager protocol defined in Rust/UniFFI
//

import Foundation
import Security

/// Swift implementation of KeychainManager trait for secure API key storage
///
/// This class uses macOS Security.framework to store API keys securely in the
/// system Keychain. Keys are:
/// - Encrypted by the system
/// - Never stored in plaintext config files
/// - Not synchronized to iCloud (stays on device)
/// - Only accessible when device is unlocked
class KeychainManagerImpl: KeychainManager {
    private let service = "com.aether.api-keys"

    /// Store an API key securely in Keychain
    ///
    /// - Parameters:
    ///   - provider: Provider name (e.g., "openai", "claude")
    ///   - key: API key to store
    /// - Throws: AetherException if storage fails
    func setApiKey(provider: String, key: String) throws {
        // First, try to delete existing key to avoid duplicate error
        _ = try? deleteApiKey(provider: provider)

        // Convert key to data
        guard let keyData = key.data(using: .utf8) else {
            throw AetherException.Error(message: "Failed to encode API key as UTF-8")
        }

        // Build query dictionary
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: provider,
            kSecValueData as String: keyData,
            kSecAttrSynchronizable as String: false // Don't sync to iCloud
        ]

        // Add to Keychain
        let status = SecItemAdd(query as CFDictionary, nil)

        guard status == errSecSuccess else {
            throw AetherException.Error(
                message: "Failed to store API key in Keychain: \(keychainErrorMessage(status))"
            )
        }
    }

    /// Retrieve an API key from Keychain
    ///
    /// - Parameter provider: Provider name (e.g., "openai", "claude")
    /// - Returns: API key if found, nil otherwise
    /// - Throws: AetherException if Keychain access fails
    func getApiKey(provider: String) throws -> String? {
        // Build query dictionary
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: provider,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]

        // Search Keychain
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        // Handle different status codes
        switch status {
        case errSecSuccess:
            guard let data = result as? Data,
                  let key = String(data: data, encoding: .utf8) else {
                throw AetherException.Error(message: "Failed to decode API key from Keychain")
            }
            return key

        case errSecItemNotFound:
            return nil // Key not found (not an error)

        default:
            throw AetherException.Error(
                message: "Failed to retrieve API key from Keychain: \(keychainErrorMessage(status))"
            )
        }
    }

    /// Delete an API key from Keychain
    ///
    /// - Parameter provider: Provider name (e.g., "openai", "claude")
    /// - Throws: AetherException if deletion fails
    func deleteApiKey(provider: String) throws {
        // Build query dictionary
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: provider
        ]

        // Delete from Keychain
        let status = SecItemDelete(query as CFDictionary)

        // Success or item not found are both OK
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw AetherException.Error(
                message: "Failed to delete API key from Keychain: \(keychainErrorMessage(status))"
            )
        }
    }

    /// Check if an API key exists in Keychain
    ///
    /// - Parameter provider: Provider name
    /// - Returns: true if key exists, false otherwise
    /// - Throws: AetherException if Keychain access fails
    func hasApiKey(provider: String) throws -> Bool {
        // Build query dictionary (without returning data, just check existence)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: provider,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]

        // Check existence
        let status = SecItemCopyMatching(query as CFDictionary, nil)

        switch status {
        case errSecSuccess:
            return true
        case errSecItemNotFound:
            return false
        default:
            throw AetherException.Error(
                message: "Failed to check API key existence in Keychain: \(keychainErrorMessage(status))"
            )
        }
    }

    // MARK: - Helper Methods

    /// Convert OSStatus error code to human-readable message
    private func keychainErrorMessage(_ status: OSStatus) -> String {
        if let errorMessage = SecCopyErrorMessageString(status, nil) {
            return errorMessage as String
        }
        return "OSStatus \(status)"
    }
}
