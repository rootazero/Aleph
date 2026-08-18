//! `bootstrap-token` subcommand — prints the auto-provisioned shared Gateway
//! token so the desktop shell (or an operator generating a QR / LAN URL) can
//! authorize a remote Panel.
//!
//! Same threat model as `secret list`: reads `~/.aleph/data/security.db`
//! directly (file mode 0600 enforced by `SQLite` + OS), no daemon required.

use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
use std::error::Error;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// Read the shared token from `db_path` if one has been provisioned.
/// `data_dir` is used to locate the secrets vault (its existence is not
/// required for token retrieval, but `SharedTokenManager::new` opens it).
///
/// Returns `None` when the DB has no plaintext token (first-run state).
fn read_token_from_db(db_path: &Path, data_dir: &Path) -> Option<String> {
    let store = Arc::new(SecurityStore::open(db_path).ok()?);
    let vault_path = data_dir.join("secrets.vault");
    let mgr = SharedTokenManager::new(store, vault_path);
    mgr.try_load_token_from_db()
}

/// Handle the `aleph-server bootstrap-token` subcommand.
///
/// Resolves the standard `~/.aleph/data/` paths via `alephcore::utils::paths`,
/// then prints the token to stdout followed by a single newline (no banner,
/// no decoration — the shell parses it). Exits with `EX_USAGE` (64) and a
/// stderr message when no token exists.
pub fn handle_bootstrap_token() -> Result<(), Box<dyn Error>> {
    use alephcore::utils::paths;

    let db_path =
        paths::get_security_db_path().map_err(|e| format!("resolve security DB path: {e}"))?;
    let data_dir = db_path
        .parent()
        .ok_or("security DB has no parent directory")?
        .to_path_buf();

    if let Some(token) = read_token_from_db(&db_path, &data_dir) {
        // Warn on stderr (NOT stdout — stdout is reserved for the shell to
        // parse the token line, with no banner by design). The token never
        // expires, doubles as the vault master key, and any device that
        // receives it keeps it permanently — operators have occasionally
        // piped this straight into a QR / URL, which the module-level
        // docstring explicitly forbids. One reminder is cheap; the
        // mistake is permanent.
        eprintln!(
            "warning: this is the shared Gateway token — it never expires and is \
             also the secret vault's master key. Do not put it in a URL or QR; use \
             `aleph-server pair` to authorize a device instead."
        );
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        writeln!(handle, "{token}")?;
        Ok(())
    } else {
        eprintln!(
            "aleph-server: no shared token provisioned yet — start the \
             server once (`aleph-server start`) to generate one."
        );
        std::process::exit(64); // EX_USAGE per sysexits.h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn returns_existing_token_when_db_has_one() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("security.db");
        let vault_path = dir.path().join("secrets.vault");

        let store = Arc::new(SecurityStore::open(&db_path).expect("open store"));
        let mgr = SharedTokenManager::new(store, vault_path);
        let expected = mgr.generate_token().expect("generate");

        let out = read_token_from_db(&db_path, dir.path()).expect("read");
        assert_eq!(out, expected);
        assert!(out.starts_with("aleph-"), "expected aleph-<uuid> format");
    }

    #[test]
    fn returns_none_when_db_empty() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("security.db");
        let data_dir = dir.path();
        // No token generated yet — first-run state.
        assert!(read_token_from_db(&db_path, data_dir).is_none());
    }
}
