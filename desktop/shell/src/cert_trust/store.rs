//! Pinned TOFU trust store: `host:port -> SHA-256 fingerprint`. JSON file,
//! best-effort load (corrupt/missing = empty; never brick, never auto-allow).

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pinned: BTreeMap<String, String>,
}

impl TrustStore {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            pinned: BTreeMap::new(),
        }
    }

    /// Best-effort load: any error (missing, unreadable, malformed) yields an
    /// empty store so every host re-prompts rather than the app bricking.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<TrustStore>(&s).ok())
            .unwrap_or_else(Self::empty)
    }

    #[must_use]
    pub fn lookup(&self, host: &str) -> Option<&str> {
        self.pinned.get(host).map(String::as_str)
    }

    /// In-memory insert (tests / pre-save staging).
    pub fn insert_mem(&mut self, host: &str, fp: &str) {
        self.pinned.insert(host.to_string(), fp.to_string());
    }

    /// Insert and persist atomically (write temp + rename).
    pub fn insert_and_save(&mut self, host: &str, fp: &str, path: &Path) -> std::io::Result<()> {
        self.insert_mem(host, fp);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_insert_lookup_save_load() {
        let dir = std::env::temp_dir().join(format!("aleph-ct-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trusted-certs");
        let mut s = TrustStore::empty();
        s.insert_and_save("172.245.43.211:18790", "49:3D", &path)
            .unwrap();
        let reloaded = TrustStore::load(&path);
        assert_eq!(reloaded.lookup("172.245.43.211:18790"), Some("49:3D"));
        assert_eq!(reloaded.lookup("other:1"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_loads_empty_not_panic() {
        let dir = std::env::temp_dir().join(format!("aleph-ct-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trusted-certs");
        std::fs::write(&path, b"\x00not json{{").unwrap();
        let s = TrustStore::load(&path); // must not panic
        assert_eq!(s.lookup("anything"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrite_replaces_fingerprint() {
        let mut s = TrustStore::empty();
        s.insert_mem("h:1", "OLD");
        s.insert_mem("h:1", "NEW");
        assert_eq!(s.lookup("h:1"), Some("NEW"));
    }
}
