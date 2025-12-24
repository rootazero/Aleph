/// macOS Keychain integration for secure API key storage
///
/// This module provides FFI trait definitions for accessing macOS Keychain
/// through Swift's Security framework. The actual implementation lives in Swift
/// because it requires direct access to Security.framework APIs.
///
/// Architecture:
/// 1. Rust defines the `KeychainManager` trait via UniFFI
/// 2. Swift implements the trait using Security.framework
/// 3. Swift implementation is injected into Rust at runtime
///
/// Security Guarantees:
/// - API keys stored in Keychain (not in config.toml)
/// - Keychain items marked as non-synchronizable (stay on device)
/// - Access control: always accessible when unlocked
use crate::error::AetherException;

/// Keychain manager trait for secure API key storage
///
/// This trait is implemented in Swift using the Security framework.
/// It provides secure storage for API keys without exposing them
/// in plaintext config files.
///
/// # Example Swift Implementation
///
/// ```swift
/// class KeychainManagerImpl: KeychainManager {
///     func setApiKey(provider: String, key: String) throws {
///         let query: [String: Any] = [
///             kSecClass as String: kSecClassGenericPassword,
///             kSecAttrService as String: "com.aether.api-keys",
///             kSecAttrAccount as String: provider,
///             kSecValueData as String: key.data(using: .utf8)!,
///             kSecAttrSynchronizable as String: false
///         ]
///         SecItemAdd(query as CFDictionary, nil)
///     }
///
///     func getApiKey(provider: String) throws -> String? {
///         let query: [String: Any] = [
///             kSecClass as String: kSecClassGenericPassword,
///             kSecAttrService as String: "com.aether.api-keys",
///             kSecAttrAccount as String: provider,
///             kSecReturnData as String: true
///         ]
///         var result: AnyObject?
///         let status = SecItemCopyMatching(query as CFDictionary, &result)
///         if status == errSecSuccess, let data = result as? Data {
///             return String(data: data, encoding: .utf8)
///         }
///         return nil
///     }
/// }
/// ```
pub trait KeychainManager: Send + Sync {
    /// Store an API key securely in macOS Keychain
    ///
    /// # Arguments
    /// * `provider` - Provider name (e.g., "openai", "claude")
    /// * `key` - API key to store
    ///
    /// # Errors
    /// * `AetherException` - Failed to store key
    ///
    /// # Example
    /// ```no_run
    /// keychain.set_api_key("openai".to_string(), "sk-...".to_string())?;
    /// ```
    fn set_api_key(&self, provider: String, key: String) -> Result<(), AetherException>;

    /// Retrieve an API key from macOS Keychain
    ///
    /// # Arguments
    /// * `provider` - Provider name (e.g., "openai", "claude")
    ///
    /// # Returns
    /// * `Ok(Some(key))` - Key found
    /// * `Ok(None)` - Key not found (not an error)
    /// * `Err(AetherException)` - Keychain access error
    ///
    /// # Example
    /// ```no_run
    /// if let Some(key) = keychain.get_api_key("openai".to_string())? {
    ///     println!("Found API key");
    /// }
    /// ```
    fn get_api_key(&self, provider: String) -> Result<Option<String>, AetherException>;

    /// Delete an API key from macOS Keychain
    ///
    /// # Arguments
    /// * `provider` - Provider name (e.g., "openai", "claude")
    ///
    /// # Errors
    /// * `AetherException` - Failed to delete key
    ///
    /// # Example
    /// ```no_run
    /// keychain.delete_api_key("openai".to_string())?;
    /// ```
    fn delete_api_key(&self, provider: String) -> Result<(), AetherException>;

    /// Check if an API key exists in Keychain
    ///
    /// # Arguments
    /// * `provider` - Provider name
    ///
    /// # Returns
    /// * `Ok(true)` - Key exists
    /// * `Ok(false)` - Key does not exist
    /// * `Err(AetherException)` - Keychain access error
    fn has_api_key(&self, provider: String) -> Result<bool, AetherException>;
}

/// Mock implementation for testing (bypasses Keychain)
///
/// This implementation stores keys in memory and is used for:
/// 1. Unit tests that don't require macOS Keychain
/// 2. Development environments without Keychain access
/// 3. CI/CD pipelines
///
/// **WARNING**: Do NOT use in production - keys are not persisted!
#[cfg(test)]
pub struct MockKeychainManager {
    keys: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
}

#[cfg(test)]
impl MockKeychainManager {
    pub fn new() -> Self {
        Self {
            keys: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

#[cfg(test)]
impl KeychainManager for MockKeychainManager {
    fn set_api_key(&self, provider: String, key: String) -> Result<(), AetherException> {
        let mut keys = self.keys.lock().unwrap();
        keys.insert(provider, key);
        Ok(())
    }

    fn get_api_key(&self, provider: String) -> Result<Option<String>, AetherException> {
        let keys = self.keys.lock().unwrap();
        Ok(keys.get(&provider).cloned())
    }

    fn delete_api_key(&self, provider: String) -> Result<(), AetherException> {
        let mut keys = self.keys.lock().unwrap();
        keys.remove(&provider);
        Ok(())
    }

    fn has_api_key(&self, provider: String) -> Result<bool, AetherException> {
        let keys = self.keys.lock().unwrap();
        Ok(keys.contains_key(&provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_keychain_set_get() {
        let keychain = MockKeychainManager::new();

        // Set API key
        keychain
            .set_api_key("openai".to_string(), "sk-test".to_string())
            .unwrap();

        // Get API key
        let key = keychain.get_api_key("openai".to_string()).unwrap();
        assert_eq!(key, Some("sk-test".to_string()));
    }

    #[test]
    fn test_mock_keychain_delete() {
        let keychain = MockKeychainManager::new();

        // Set and delete
        keychain
            .set_api_key("openai".to_string(), "sk-test".to_string())
            .unwrap();
        keychain.delete_api_key("openai".to_string()).unwrap();

        // Should be gone
        let key = keychain.get_api_key("openai".to_string()).unwrap();
        assert_eq!(key, None);
    }

    #[test]
    fn test_mock_keychain_has_key() {
        let keychain = MockKeychainManager::new();

        // Initially no key
        assert!(!keychain.has_api_key("openai".to_string()).unwrap());

        // After setting
        keychain
            .set_api_key("openai".to_string(), "sk-test".to_string())
            .unwrap();
        assert!(keychain.has_api_key("openai".to_string()).unwrap());
    }

    #[test]
    fn test_mock_keychain_get_nonexistent() {
        let keychain = MockKeychainManager::new();

        // Get nonexistent key should return None
        let key = keychain.get_api_key("nonexistent".to_string()).unwrap();
        assert_eq!(key, None);
    }
}
