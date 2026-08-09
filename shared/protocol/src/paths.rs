//! Where Aleph state lives — the single derivation, shared by the core and by
//! clients that are forbidden to depend on it.
//!
//! `aleph-cli` and `aleph-client` MUST NOT depend on `alephcore` (both crates
//! say so in their `Cargo.toml`), and the core's own rule for locating
//! `~/.aleph` has a source-level guard behind it precisely because a second,
//! hand-rolled copy of it is invisible until someone sets `ALEPH_HOME`: with
//! the variable unset both spellings agree byte for byte, so the machine that
//! wrote the bug can never reproduce it.
//!
//! So the derivation lives here, in the one crate both sides already depend on.
//! `alephcore::utils::paths` delegates to it rather than restating it.
//!
//! **Everything here is a pure lookup.** Nothing creates a directory — a
//! diagnostic, a client probing for a certificate, or an audit must be able to
//! ask where something *would* be without bringing it into existence.

use std::path::PathBuf;

/// The user's home directory.
///
/// Tried in order: `HOME` (Unix standard, and set by Git Bash / MSYS2 on
/// Windows), `USERPROFILE` (Windows standard), then `HOMEDRIVE` + `HOMEPATH`
/// (older Windows). Returns `None` when none of them is set.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        return Some(PathBuf::from(format!("{drive}{path}")));
    }
    None
}

/// The Aleph home directory: `$ALEPH_HOME` when set, else `~/.aleph`.
///
/// `ALEPH_HOME` points *directly at* the `.aleph` directory — it is not a
/// parent to join `.aleph` onto. It is the single knob for relocating all
/// Aleph state, which is what makes an isolated test or QA instance possible.
#[must_use]
pub fn aleph_home() -> Option<PathBuf> {
    aleph_home_from(
        std::env::var_os("ALEPH_HOME").map(PathBuf::from),
        home_dir(),
    )
}

/// The rule itself, with the environment lifted out so it can be tested
/// without mutating a process-global every other test in the binary reads.
fn aleph_home_from(explicit: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or_else(|| Some(home?.join(".aleph")))
}

/// `<aleph_home>/data` — where the databases, the vault, the lock and the TLS
/// material live. Pure lookup: unlike the core's `get_data_dir`, this does not
/// create the directory.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    Some(aleph_home()?.join("data"))
}

/// `<data_dir>/tls/cert.pem` — the certificate `gateway::tls::load_or_generate`
/// writes for the self-signed tier.
///
/// This is the whole reason the derivation is shared: a CLI on the same machine
/// as the server it is talking to can pin the server's own certificate instead
/// of being told to disable verification. Returns the path whether or not the
/// file exists — the caller decides what an absent certificate means.
#[must_use]
pub fn self_signed_cert_path() -> Option<PathBuf> {
    Some(data_dir()?.join("tls").join("cert.pem"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the paths, asserted without touching the environment —
    /// these tests must not race the process-global env with anything else in
    /// the suite, so they build from an explicit root rather than setting
    /// `ALEPH_HOME`.
    #[test]
    fn the_tls_certificate_hangs_off_the_data_dir() {
        let home = PathBuf::from("/tmp/fake-aleph");
        let expected = home.join("data").join("tls").join("cert.pem");
        assert_eq!(
            expected.strip_prefix(&home).unwrap(),
            PathBuf::from("data").join("tls").join("cert.pem"),
            "if this layout changes, `self_signed_cert_path` and \
             `gateway::tls::load_or_generate` have drifted apart"
        );
    }

    #[test]
    fn aleph_home_is_the_override_itself_not_a_parent_to_join_onto() {
        assert_eq!(
            aleph_home_from(Some(PathBuf::from("/srv/aleph")), Some(PathBuf::from("/u"))),
            Some(PathBuf::from("/srv/aleph")),
            "ALEPH_HOME points AT the directory — joining `.aleph` onto it \
             would send an isolated instance to a sibling of its own state"
        );
        assert_eq!(
            aleph_home_from(None, Some(PathBuf::from("/u"))),
            Some(PathBuf::from("/u").join(".aleph")),
        );
        assert_eq!(aleph_home_from(None, None), None);
    }
}
