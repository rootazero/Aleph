//! Where a global npm package lands — one answer, shared by the installer and
//! the probe.
//!
//! Keeping this in one place is the point: the installer picks the directory
//! and the probe has to find the binary there afterwards. Two copies of that
//! fact drift in the direction where the install "succeeds" and the probe then
//! reports the capability missing.

use std::ffi::OsString;
use std::path::PathBuf;

/// npm `--prefix` for user-level global installs, or `None` when no home
/// directory can be determined (npm's own default then applies).
///
/// **Deliberately not the node installation's own tree.** Under a version
/// manager — fnm, which Aleph itself installs, but equally nvm or asdf —
/// `npm install -g` writes into `<manager>/node-versions/<v>/…`. Every fnm
/// alias (`default`, `lts`, `lts-latest`) is a symlink into one such version
/// directory, so the next time the user moves to a newer node **every global
/// CLI silently disappears** while the runtime ledger still reports it Ready.
/// A user-level prefix outlives the node it was installed with.
///
/// - Unix/macOS: `$HOME/.local` — the XDG-style user prefix. npm writes
///   `lib/node_modules/<pkg>` and links executables into `bin`.
/// - Windows: `%APPDATA%\npm` — npm's own documented per-user global location.
///   The package goes to `node_modules\<pkg>` and the `.cmd` shim sits directly
///   in the prefix.
///
/// An explicit `npm_config_prefix` / `NPM_CONFIG_PREFIX` wins over both: the
/// operator configured npm on purpose, so install where they said rather than
/// where we would have guessed.
#[must_use]
pub fn prefix() -> Option<PathBuf> {
    prefix_from(
        std::env::var_os("npm_config_prefix").or_else(|| std::env::var_os("NPM_CONFIG_PREFIX")),
    )
}

/// The decision itself, with the environment lifted out so it can be tested
/// without mutating process-global state — two tests racing on `set_var` is a
/// flake this repo has already paid for once.
fn prefix_from(configured: Option<OsString>) -> Option<PathBuf> {
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    default_prefix()
}

#[cfg(windows)]
fn default_prefix() -> Option<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(appdata).join("npm"));
    }
    // A service-launched daemon can be missing APPDATA while still knowing the
    // profile root; reconstruct npm's own default from it rather than giving up
    // and letting the install land in the node version tree.
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("AppData").join("Roaming").join("npm"))
}

#[cfg(not(windows))]
fn default_prefix() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".local"))
}

/// Directory the executables of a package installed under [`prefix`] land in.
///
/// npm puts them in `<prefix>/bin` on Unix and directly in `<prefix>` on
/// Windows — that split is npm's, not ours.
#[must_use]
pub fn bin_dir() -> Option<PathBuf> {
    let prefix = prefix()?;
    #[cfg(windows)]
    {
        Some(prefix)
    }
    #[cfg(not(windows))]
    {
        Some(prefix.join("bin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the module: the chosen prefix must not sit inside a
    /// node version manager's tree, because that is what makes global CLIs
    /// evaporate on the next node upgrade.
    #[test]
    fn default_prefix_is_outside_any_node_version_tree() {
        let Some(prefix) = default_prefix() else {
            return; // no HOME in this environment; nothing to assert
        };
        let text = prefix.to_string_lossy().replace('\\', "/");
        for marker in [
            "node-versions",
            "/fnm/",
            "/.nvm/",
            "/versions/node",
            "/.asdf/",
        ] {
            assert!(
                !text.contains(marker),
                "prefix {text} sits inside a version-manager tree (matched {marker})"
            );
        }
    }

    /// `bin_dir` must be reachable from `prefix` — they are two views of one
    /// decision, and a probe that searches the wrong one finds nothing.
    #[test]
    fn bin_dir_is_derived_from_prefix() {
        match (prefix(), bin_dir()) {
            (Some(p), Some(b)) => assert!(
                b.starts_with(&p),
                "bin dir {} is not under prefix {}",
                b.display(),
                p.display()
            ),
            (None, None) => {}
            (p, b) => panic!("prefix and bin_dir disagree on availability: {p:?} vs {b:?}"),
        }
    }

    #[test]
    fn explicit_npm_config_prefix_wins() {
        assert_eq!(
            prefix_from(Some(OsString::from("/opt/team/npm"))),
            Some(PathBuf::from("/opt/team/npm")),
            "an operator who configured npm gets the directory they configured"
        );
    }

    /// An empty value is npm's "unset", not a request to install at the
    /// filesystem root.
    #[test]
    fn empty_npm_config_prefix_falls_back_to_the_default() {
        assert_eq!(prefix_from(Some(OsString::new())), default_prefix());
        assert_eq!(prefix_from(None), default_prefix());
    }
}
