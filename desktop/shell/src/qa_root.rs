//! QA-only owned-root resolver (`qa-owned-root` feature).
//!
//! The Native IPC QA run launches a real debug build of the lite shell. That
//! shell would normally write under the user's real `~/.aleph` (connection
//! markers, trusted certs) and the real `%LOCALAPPDATA%` (WebView2 profile). This
//! module is the single choke point that re-roots every one of those writes onto
//! a directory the QA *harness* created and owns, and it fails closed when that
//! ownership cannot be proven:
//!
//! * the root is passed explicitly: `--qa-root <abs-dir> --qa-nonce <hex>`.
//!   There is no environment-variable, `HOME`, `dirs::*` or CWD fallback; a
//!   missing/invalid argument aborts startup (see `main`).
//! * the root must be an existing, non-reparse directory that is a *direct
//!   child* of the canonical `std::env::temp_dir()`, named `aleph-shell-qa-*`,
//!   free of any `.aleph` component, not inside the canonical home /
//!   `%APPDATA%` / `%LOCALAPPDATA%` (other than where the OS temp dir itself
//!   legitimately lives), and must contain a `.qa-owned` file whose content is
//!   exactly the nonce the harness handed us.
//! * an optional `--qa-monitor-port <1024..=65535>` (default absent) selects the
//!   loopback port of the harness's redacted IPC monitor (see `main`'s
//!   `qa_monitor_script`). It is parsed from argv only, published only after the
//!   root above is proven, and `monitor_port()` yields `None` without a proven root.
//!   The proven root nonce is also the monitor's first-message authentication
//!   value (`monitor_auth_nonce()`, `None` unless the root AND a monitor port were
//!   proven/given). It is the QA OWNERSHIP nonce, not a secret credential and not
//!   proof of IPC or identity.
//! * the shell NEVER deletes anything here; creation and cleanup belong to the
//!   harness (`qa/native_ipc`). The only write this module performs is a
//!   non-recursive `create_dir` of `<root>/webview2`.
//!
//! The env vars `APPDATA` / `LOCALAPPDATA` and `dirs::home_dir()` are read only
//! to *forbid* locations, never to choose one.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Literal embedded in every QA binary (printed on refusal in `main`). The QA
/// harness greps the executable for it before launch: a non-QA build would
/// ignore `--qa-root` and write to the real home / keyring, so "is this really a
/// `qa-owned-root` build" must be provable without running it.
pub(crate) const QA_BUILD_MARKER: &str = "aleph-qa-owned-root-build-v1";
/// Required leaf-name prefix of the owned root.
pub(crate) const ROOT_PREFIX: &str = "aleph-shell-qa-";
/// Ownership marker the harness writes inside the root; content == nonce.
pub(crate) const NONCE_FILE: &str = ".qa-owned";
/// Subdirectory (inside the root) that holds the WebView2 user-data folder.
const WEBVIEW_SUBDIR: &str = "webview2";
/// The ownership file is a nonce, not data: refuse anything large.
const NONCE_FILE_MAX_BYTES: u64 = 256;
const NONCE_MIN_LEN: usize = 16;
const NONCE_MAX_LEN: usize = 128;
/// `--qa-credential-backend os` derives its keyring service from the nonce, so
/// it needs more entropy than the memory mode: >=32 lowercase hex.
const OS_NONCE_MIN_LEN: usize = 32;
/// Keyring service prefix for the QA-only OS credential backend. Must stay in
/// lockstep with `aleph-client`'s `credential_store::QA_SERVICE_PREFIX`, which
/// re-validates the full service and refuses anything else.
const QA_SERVICE_PREFIX: &str = "aleph-qa.device-credential.";
const CREDENTIAL_BACKEND_FLAG: &str = "--qa-credential-backend";
const MONITOR_PORT_FLAG: &str = "--qa-monitor-port";
/// Unprivileged loopback port range accepted for the QA monitor.
const MONITOR_PORT_MIN: u32 = 1024;
const MONITOR_PORT_MAX: u32 = 65535;

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static CREDENTIAL_BACKEND: OnceLock<CredentialBackendChoice> = OnceLock::new();
static MONITOR_PORT: OnceLock<u16> = OnceLock::new();
static MONITOR_AUTH: OnceLock<String> = OnceLock::new();

/// Where the QA shell keeps the paired-device credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialBackendChoice {
    /// Process-local in-memory map (default; nothing reaches the OS keyring).
    Memory,
    /// Real OS keyring, filed under this QA-only service namespace
    /// (`aleph-qa.device-credential.<nonce>`), derived from the proven nonce.
    Os { service: String },
}

/// Why the owned root was refused. Every variant aborts startup.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QaRootError {
    MissingArg(&'static str),
    DuplicateArg(&'static str),
    MalformedNonce,
    UnknownCredentialBackend,
    OsBackendNeedsStrongNonce,
    MalformedMonitorPort,
    NotAbsolute,
    NonNormalizedPath,
    AlephComponent,
    NotFound,
    NotDirectory,
    ReparsePoint,
    AliasedPath,
    TempDirUnavailable,
    NotDirectChildOfTemp,
    WrongPrefix,
    ForbiddenLocation,
    NonceFileMissing,
    NonceFileInvalid,
    NonceMismatch,
    AlreadyInitialized,
}

impl fmt::Display for QaRootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::MissingArg(a) => return write!(f, "missing or malformed argument {a} <value>"),
            Self::DuplicateArg(a) => return write!(f, "argument {a} given more than once"),
            Self::MalformedNonce => "--qa-nonce must be 16..=128 hex characters",
            Self::UnknownCredentialBackend => {
                "--qa-credential-backend must be exactly 'memory' or 'os'"
            }
            Self::OsBackendNeedsStrongNonce => {
                "--qa-credential-backend os requires --qa-nonce of 32..=128 lowercase hex"
            }
            Self::MalformedMonitorPort => {
                "--qa-monitor-port must be a plain decimal integer in 1024..=65535"
            }
            Self::NotAbsolute => "--qa-root must be an absolute path",
            Self::NonNormalizedPath => "--qa-root must not contain '.' or '..' components",
            Self::AlephComponent => "--qa-root must not contain a '.aleph' component",
            Self::NotFound => "--qa-root does not exist",
            Self::NotDirectory => "--qa-root is not a directory",
            Self::ReparsePoint => "--qa-root is a symlink or reparse point",
            Self::AliasedPath => {
                "--qa-root differs from its canonical form (symlink/junction/8.3 alias in path)"
            }
            Self::TempDirUnavailable => "the OS temp directory cannot be canonicalized",
            Self::NotDirectChildOfTemp => {
                "--qa-root must be a direct child of the canonical OS temp directory"
            }
            Self::WrongPrefix => "--qa-root directory name must start with 'aleph-shell-qa-'",
            Self::ForbiddenLocation => {
                "--qa-root is inside (or contains) the home, APPDATA or LOCALAPPDATA tree"
            }
            Self::NonceFileMissing => "--qa-root has no regular '.qa-owned' ownership file",
            Self::NonceFileInvalid => "'.qa-owned' is not a small regular file",
            Self::NonceMismatch => "'.qa-owned' content does not match --qa-nonce",
            Self::AlreadyInitialized => "qa root already initialized",
        };
        f.write_str(s)
    }
}

impl std::error::Error for QaRootError {}

/// Parse `--qa-root <path>` / `--qa-nonce <hex>` out of a full argv (argv[0]
/// included). Only the space-separated form is accepted; unknown arguments are
/// ignored; a value that itself looks like a flag counts as missing.
fn parse_args<I, S>(args: I) -> Result<(PathBuf, String), QaRootError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut root: Option<PathBuf> = None;
    let mut nonce: Option<String> = None;
    let mut it = args.into_iter().map(Into::<OsString>::into).skip(1);
    while let Some(arg) = it.next() {
        if arg == "--qa-root" {
            let value = it.next().ok_or(QaRootError::MissingArg("--qa-root"))?;
            if value.is_empty() || value.to_string_lossy().starts_with("--") {
                return Err(QaRootError::MissingArg("--qa-root"));
            }
            if root.replace(PathBuf::from(value)).is_some() {
                return Err(QaRootError::DuplicateArg("--qa-root"));
            }
        } else if arg == "--qa-nonce" {
            let value = it.next().ok_or(QaRootError::MissingArg("--qa-nonce"))?;
            let value = value.into_string().map_err(|_| QaRootError::MalformedNonce)?;
            if nonce.replace(value).is_some() {
                return Err(QaRootError::DuplicateArg("--qa-nonce"));
            }
        }
    }
    let root = root.ok_or(QaRootError::MissingArg("--qa-root"))?;
    let nonce = nonce.ok_or(QaRootError::MissingArg("--qa-nonce"))?;
    if !(NONCE_MIN_LEN..=NONCE_MAX_LEN).contains(&nonce.len())
        || !nonce.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(QaRootError::MalformedNonce);
    }
    Ok((root, nonce))
}

/// Parse the optional `--qa-credential-backend memory|os` (default `memory`)
/// and, for `os`, derive the QA keyring service from `nonce`. Pure; reads
/// nothing but its arguments (no env / HOME fallback). Duplicate flag, missing
/// or unknown value, and an `os` nonce that is not 32..=128 lowercase hex are
/// all refused.
fn parse_credential_backend<I, S>(
    args: I,
    nonce: &str,
) -> Result<CredentialBackendChoice, QaRootError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut seen: Option<String> = None;
    let mut it = args.into_iter().map(Into::<OsString>::into).skip(1);
    while let Some(arg) = it.next() {
        // `--flag=value` is not accepted; refuse it rather than silently fall
        // back to memory when the harness meant `os`.
        if arg.to_string_lossy().starts_with("--qa-credential-backend=") {
            return Err(QaRootError::MissingArg(CREDENTIAL_BACKEND_FLAG));
        }
        if arg == CREDENTIAL_BACKEND_FLAG {
            let value = it
                .next()
                .ok_or(QaRootError::MissingArg(CREDENTIAL_BACKEND_FLAG))?;
            if value.is_empty() || value.to_string_lossy().starts_with("--") {
                return Err(QaRootError::MissingArg(CREDENTIAL_BACKEND_FLAG));
            }
            let value = value
                .into_string()
                .map_err(|_| QaRootError::UnknownCredentialBackend)?;
            if seen.replace(value).is_some() {
                return Err(QaRootError::DuplicateArg(CREDENTIAL_BACKEND_FLAG));
            }
        }
    }
    match seen.as_deref() {
        None | Some("memory") => Ok(CredentialBackendChoice::Memory),
        Some("os") => {
            let strong = (OS_NONCE_MIN_LEN..=NONCE_MAX_LEN).contains(&nonce.len())
                && nonce
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !strong {
                return Err(QaRootError::OsBackendNeedsStrongNonce);
            }
            Ok(CredentialBackendChoice::Os {
                service: format!("{QA_SERVICE_PREFIX}{nonce}"),
            })
        }
        Some(_) => Err(QaRootError::UnknownCredentialBackend),
    }
}

/// Parse the optional `--qa-monitor-port <1024..=65535>` (default: absent). Pure;
/// reads nothing but its arguments (no env / HOME fallback). The value must be a
/// plain decimal integer (no sign, no leading zero, no whitespace). A duplicate
/// flag, a missing/flag-looking value, the `--flag=value` form and an
/// out-of-range or non-numeric value are all refused.
fn parse_monitor_port<I, S>(args: I) -> Result<Option<u16>, QaRootError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut seen: Option<u16> = None;
    let mut it = args.into_iter().map(Into::<OsString>::into).skip(1);
    while let Some(arg) = it.next() {
        if arg.to_string_lossy().starts_with("--qa-monitor-port=") {
            return Err(QaRootError::MissingArg(MONITOR_PORT_FLAG));
        }
        if arg == MONITOR_PORT_FLAG {
            let value = it
                .next()
                .ok_or(QaRootError::MissingArg(MONITOR_PORT_FLAG))?;
            if value.is_empty() || value.to_string_lossy().starts_with("--") {
                return Err(QaRootError::MissingArg(MONITOR_PORT_FLAG));
            }
            let value = value
                .into_string()
                .map_err(|_| QaRootError::MalformedMonitorPort)?;
            let port = parse_port_value(&value)?;
            if seen.replace(port).is_some() {
                return Err(QaRootError::DuplicateArg(MONITOR_PORT_FLAG));
            }
        }
    }
    Ok(seen)
}

fn parse_port_value(value: &str) -> Result<u16, QaRootError> {
    if value.is_empty()
        || value.len() > 5
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(QaRootError::MalformedMonitorPort);
    }
    let n: u32 = value
        .parse()
        .map_err(|_| QaRootError::MalformedMonitorPort)?;
    if !(MONITOR_PORT_MIN..=MONITOR_PORT_MAX).contains(&n) {
        return Err(QaRootError::MalformedMonitorPort);
    }
    u16::try_from(n).map_err(|_| QaRootError::MalformedMonitorPort)
}

/// Resolve + prove the owned root from the process arguments and publish it.
/// Must run first in `main`; callable once.
pub(crate) fn init_from_args<I, S>(args: I) -> Result<(), QaRootError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let (root, nonce) = parse_args(args.iter().cloned())?;
    let backend = parse_credential_backend(args.iter().cloned(), &nonce)?;
    let monitor = parse_monitor_port(args.iter().cloned())?;
    let temp = fs::canonicalize(std::env::temp_dir()).map_err(|_| QaRootError::TempDirUnavailable)?;
    let temp = strip_verbatim(&temp);
    let proven = validate_with(&root, &nonce, &temp, &forbidden_roots())?;
    ROOT.set(proven).map_err(|_| QaRootError::AlreadyInitialized)?;
    // Published only after the root (and thus the nonce) is proven.
    if let Some(port) = monitor {
        MONITOR_PORT
            .set(port)
            .map_err(|_| QaRootError::AlreadyInitialized)?;
        if let Some(auth) = monitor_auth_for(Some(port), &nonce) {
            MONITOR_AUTH
                .set(auth)
                .map_err(|_| QaRootError::AlreadyInitialized)?;
        }
    }
    CREDENTIAL_BACKEND
        .set(backend)
        .map_err(|_| QaRootError::AlreadyInitialized)
}

/// The credential backend chosen at startup, or `None` if `init_from_args` has
/// not succeeded (callers then keep the in-memory default; they never reach the
/// production keyring in a QA build).
pub(crate) fn credential_backend() -> Option<&'static CredentialBackendChoice> {
    CREDENTIAL_BACKEND.get()
}

/// The QA monitor loopback port, or `None` when `--qa-monitor-port` was not
/// given or the owned root has not been proven (the port is only ever published
/// after `init_from_args` proved the root).
pub(crate) fn monitor_port() -> Option<u16> {
    ROOT.get()?;
    MONITOR_PORT.get().copied()
}

/// The monitor auth value for a given port option: the nonce, only when a monitor
/// port was requested. Pure; `init_from_args` calls it AFTER the root proof.
fn monitor_auth_for(port: Option<u16>, nonce: &str) -> Option<String> {
    port.map(|_| nonce.to_owned())
}

/// The root nonce for the QA monitor's first-message authentication, or `None`
/// unless the owned root was proven and `--qa-monitor-port` was given. Validated
/// by `validate_with` (`.qa-owned` content == this nonce) before it is published.
pub(crate) fn monitor_auth_nonce() -> Option<&'static str> {
    ROOT.get()?;
    MONITOR_PORT.get()?;
    MONITOR_AUTH.get().map(String::as_str)
}

/// The proven owned root, or `None` if `init_from_args` has not succeeded.
/// Callers treat `None` as "no storage": nothing falls back to the real home.
pub(crate) fn try_root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

/// The proven owned root. Panics if `init_from_args` never succeeded — in a
/// QA-only build that is the correct fail-closed behaviour.
pub(crate) fn root() -> &'static Path {
    try_root().expect("qa_root::init_from_args must succeed before qa_root::root()")
}

/// `<root>/webview2`, created (non-recursively; its parent is the proven root)
/// and re-proven to be a real directory contained in the root. Handed to
/// `WebviewWindowBuilder::data_directory`.
pub(crate) fn webview_data_dir() -> std::io::Result<PathBuf> {
    let root = root();
    let dir = root.join(WEBVIEW_SUBDIR);
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let bad = |msg: &str| std::io::Error::new(std::io::ErrorKind::PermissionDenied, msg.to_owned());
    let md = fs::symlink_metadata(&dir)?;
    if !md.is_dir() || is_link_or_reparse(&md) {
        return Err(bad("webview2 data dir is not a plain directory"));
    }
    let canon = strip_verbatim(&fs::canonicalize(&dir)?);
    if canon.parent() != Some(root) {
        return Err(bad("webview2 data dir escapes the QA root"));
    }
    Ok(dir)
}

/// Locations the owned root must not live in. Read only to forbid.
fn forbidden_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: Option<PathBuf>| {
        if let Some(p) = p {
            if let Ok(c) = fs::canonicalize(&p) {
                out.push(strip_verbatim(&c));
            }
        }
    };
    push(dirs::home_dir());
    push(std::env::var_os("APPDATA").map(PathBuf::from));
    push(std::env::var_os("LOCALAPPDATA").map(PathBuf::from));
    out
}

/// All checks, with the canonical temp dir and the forbidden set injected so
/// the rules are testable without touching the real environment.
fn validate_with(
    root: &Path,
    nonce: &str,
    canonical_temp: &Path,
    forbidden: &[PathBuf],
) -> Result<PathBuf, QaRootError> {
    let root = strip_verbatim(root);
    if !root.is_absolute() {
        return Err(QaRootError::NotAbsolute);
    }
    if root
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(QaRootError::NonNormalizedPath);
    }
    if has_aleph_component(&root) {
        return Err(QaRootError::AlephComponent);
    }

    // Any failure to stat (missing, access denied, ...) is refused as NotFound.
    let md = fs::symlink_metadata(&root).map_err(|_| QaRootError::NotFound)?;
    if is_link_or_reparse(&md) {
        return Err(QaRootError::ReparsePoint);
    }
    if !md.is_dir() {
        return Err(QaRootError::NotDirectory);
    }

    // Any symlink/junction/8.3 alias in the chain makes canonical != lexical.
    let canon = strip_verbatim(&fs::canonicalize(&root).map_err(|_| QaRootError::NotFound)?);
    if normalized(&canon) != normalized(&root) {
        return Err(QaRootError::AliasedPath);
    }
    if has_aleph_component(&canon) {
        return Err(QaRootError::AlephComponent);
    }

    if canon.parent() != Some(canonical_temp) {
        return Err(QaRootError::NotDirectChildOfTemp);
    }
    let leaf = canon
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !(leaf.starts_with(ROOT_PREFIX) && leaf.len() > ROOT_PREFIX.len()) {
        return Err(QaRootError::WrongPrefix);
    }

    for f in forbidden {
        // The OS temp dir normally sits under %LOCALAPPDATA%; a forbidden tree
        // only matters when the root is inside it but the temp dir is not
        // (i.e. the root would be in a *deeper* forbidden subtree), or when the
        // root contains the forbidden tree.
        let inside = canon.starts_with(f) && !canonical_temp.starts_with(f);
        let contains = f.starts_with(&canon);
        if inside || contains {
            return Err(QaRootError::ForbiddenLocation);
        }
    }

    check_nonce_file(&canon, nonce)?;
    Ok(canon)
}

fn check_nonce_file(root: &Path, nonce: &str) -> Result<(), QaRootError> {
    let path = root.join(NONCE_FILE);
    let md = fs::symlink_metadata(&path).map_err(|_| QaRootError::NonceFileMissing)?;
    if is_link_or_reparse(&md) || !md.is_file() || md.len() > NONCE_FILE_MAX_BYTES {
        return Err(QaRootError::NonceFileInvalid);
    }
    let mut buf = String::new();
    fs::File::open(&path)
        .and_then(|f| f.take(NONCE_FILE_MAX_BYTES).read_to_string(&mut buf))
        .map_err(|_| QaRootError::NonceFileInvalid)?;
    if buf.trim_end_matches(['\r', '\n']) == nonce {
        Ok(())
    } else {
        Err(QaRootError::NonceMismatch)
    }
}

fn has_aleph_component(p: &Path) -> bool {
    p.components().any(|c| match c {
        Component::Normal(s) => s.to_string_lossy().eq_ignore_ascii_case(".aleph"),
        _ => false,
    })
}

fn is_link_or_reparse(md: &fs::Metadata) -> bool {
    if md.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// `\\?\C:\x` -> `C:\x` (drive-letter verbatim only; UNC verbatim is left
/// alone and will then fail the temp-dir comparison, i.e. fail closed).
fn strip_verbatim(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                return PathBuf::from(rest);
            }
        }
    }
    p.to_path_buf()
}

/// Component list for equality; case-insensitive on Windows.
fn normalized(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| {
            let s = c.as_os_str().to_string_lossy().into_owned();
            if cfg!(windows) {
                s.to_ascii_lowercase()
            } else {
                s
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    /// Test fixture dir under the OS temp dir, removed on drop. Mirrors the
    /// `connection::test_home` pattern: only ever deletes a directory it created
    /// itself, and refuses to delete anything outside the temp dir.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let base = strip_verbatim(&fs::canonicalize(std::env::temp_dir()).unwrap());
            let dir = base.join(format!(
                "aleph-shell-qa-unittest-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&dir).unwrap();
            Fixture(strip_verbatim(&fs::canonicalize(dir).unwrap()))
        }

        /// Make `<fixture>/<name>` a plausible owned root with a nonce file.
        fn owned(&self, name: &str) -> PathBuf {
            let p = self.0.join(name);
            fs::create_dir(&p).unwrap();
            fs::write(p.join(NONCE_FILE), NONCE).unwrap();
            p
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let temp = fs::canonicalize(std::env::temp_dir()).map(|t| strip_verbatim(&t));
            let ok = temp.map(|t| self.0.parent() == Some(t.as_path())).unwrap_or(false)
                && self
                    .0
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with("aleph-shell-qa-unittest-"))
                    .unwrap_or(false);
            if ok {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    fn argv(parts: &[&str]) -> Vec<OsString> {
        std::iter::once("aleph-shell")
            .chain(parts.iter().copied())
            .map(OsString::from)
            .collect()
    }

    // ----- argument parsing -------------------------------------------------

    #[test]
    fn parse_accepts_explicit_pair() {
        let (root, nonce) = parse_args(argv(&["--qa-root", "C:\\x", "--qa-nonce", NONCE])).unwrap();
        assert_eq!(root, PathBuf::from("C:\\x"));
        assert_eq!(nonce, NONCE);
    }

    #[test]
    fn parse_rejects_missing_and_malformed() {
        assert_eq!(
            parse_args(argv(&["--qa-nonce", NONCE])),
            Err(QaRootError::MissingArg("--qa-root"))
        );
        assert_eq!(
            parse_args(argv(&["--qa-root", "/x"])),
            Err(QaRootError::MissingArg("--qa-nonce"))
        );
        assert_eq!(
            parse_args(argv(&["--qa-root", "--qa-nonce", NONCE])),
            Err(QaRootError::MissingArg("--qa-root"))
        );
        assert_eq!(
            parse_args(argv(&["--qa-root"])),
            Err(QaRootError::MissingArg("--qa-root"))
        );
        // `=` form is not accepted.
        assert!(parse_args(argv(&["--qa-root=/x", "--qa-nonce", NONCE])).is_err());
        for bad in ["", "abc", "zz23456789abcdef0123456789abcdef", &"a".repeat(129)] {
            assert_eq!(
                parse_args(argv(&["--qa-root", "/x", "--qa-nonce", bad])),
                Err(QaRootError::MalformedNonce),
                "nonce {bad:?}"
            );
        }
        assert_eq!(
            parse_args(argv(&[
                "--qa-root", "/x", "--qa-root", "/y", "--qa-nonce", NONCE
            ])),
            Err(QaRootError::DuplicateArg("--qa-root"))
        );
    }

    #[test]
    fn parse_ignores_argv0_and_unknown_args() {
        // argv[0] that looks like a flag must not be parsed as one.
        let a = ["--qa-root", "--qa-root", "/x", "--qa-nonce", NONCE, "--other"]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        let (root, _) = parse_args(a).unwrap();
        assert_eq!(root, PathBuf::from("/x"));
    }

    // ----- credential backend selection -------------------------------------

    const STRONG: &str = "0123456789abcdef0123456789abcdef"; // 32 lowercase hex

    fn backend(parts: &[&str], nonce: &str) -> Result<CredentialBackendChoice, QaRootError> {
        parse_credential_backend(argv(parts), nonce)
    }

    #[test]
    fn backend_defaults_to_memory_and_accepts_explicit_memory() {
        assert_eq!(backend(&[], NONCE), Ok(CredentialBackendChoice::Memory));
        // Memory mode keeps the legacy 16..=128 (even uppercase) nonce rule.
        assert_eq!(
            backend(&["--qa-credential-backend", "memory"], "ABCDEF0123456789"),
            Ok(CredentialBackendChoice::Memory)
        );
        // argv[0] that looks like the flag is not parsed.
        let a = ["--qa-credential-backend", "os"].map(OsString::from);
        assert_eq!(
            parse_credential_backend(a, NONCE),
            Ok(CredentialBackendChoice::Memory)
        );
    }

    #[test]
    fn backend_os_derives_service_from_proven_nonce() {
        assert_eq!(
            backend(&["--qa-credential-backend", "os"], STRONG),
            Ok(CredentialBackendChoice::Os {
                service: format!("aleph-qa.device-credential.{STRONG}")
            })
        );
        let long = "f".repeat(128);
        assert!(backend(&["--qa-credential-backend", "os"], &long).is_ok());
    }

    #[test]
    fn backend_os_rejects_weak_or_non_lowercase_nonce() {
        for bad in [
            "0123456789abcdef",                 // 16 hex: valid for memory only
            "0123456789ABCDEF0123456789ABCDEF", // uppercase
            "",
        ] {
            assert_eq!(
                backend(&["--qa-credential-backend", "os"], bad),
                Err(QaRootError::OsBackendNeedsStrongNonce),
                "nonce {bad:?}"
            );
        }
        assert_eq!(
            backend(&["--qa-credential-backend", "os"], &"a".repeat(129)),
            Err(QaRootError::OsBackendNeedsStrongNonce)
        );
    }

    #[test]
    fn backend_rejects_unknown_duplicate_and_missing_values() {
        for bad in ["OS", "Memory", "keyring", "os ", ""] {
            let r = backend(&["--qa-credential-backend", bad], STRONG);
            assert!(
                matches!(
                    r,
                    Err(QaRootError::UnknownCredentialBackend | QaRootError::MissingArg(_))
                ),
                "value {bad:?} -> {r:?}"
            );
        }
        assert_eq!(
            backend(
                &["--qa-credential-backend", "memory", "--qa-credential-backend", "memory"],
                STRONG
            ),
            Err(QaRootError::DuplicateArg("--qa-credential-backend"))
        );
        assert_eq!(
            backend(&["--qa-credential-backend"], STRONG),
            Err(QaRootError::MissingArg("--qa-credential-backend"))
        );
        assert_eq!(
            backend(&["--qa-credential-backend", "--qa-root"], STRONG),
            Err(QaRootError::MissingArg("--qa-credential-backend"))
        );
        // `=` form is refused (never silently downgraded to memory).
        assert_eq!(
            backend(&["--qa-credential-backend=os"], STRONG),
            Err(QaRootError::MissingArg("--qa-credential-backend"))
        );
    }

    // ----- monitor port -----------------------------------------------------

    fn monitor(parts: &[&str]) -> Result<Option<u16>, QaRootError> {
        parse_monitor_port(argv(parts))
    }

    #[test]
    fn monitor_port_absent_by_default_and_parses_range_edges() {
        assert_eq!(monitor(&[]), Ok(None));
        assert_eq!(monitor(&["--qa-root", "/x"]), Ok(None));
        assert_eq!(monitor(&["--qa-monitor-port", "1024"]), Ok(Some(1024)));
        assert_eq!(monitor(&["--qa-monitor-port", "8080"]), Ok(Some(8080)));
        assert_eq!(monitor(&["--qa-monitor-port", "65535"]), Ok(Some(65535)));
        // argv[0] that looks like the flag is not parsed.
        let a = ["--qa-monitor-port", "9000"].map(OsString::from);
        assert_eq!(parse_monitor_port(a), Ok(None));
    }

    #[test]
    fn monitor_port_rejects_out_of_range_and_non_plain_decimal() {
        for bad in [
            "0", "1", "1023", "65536", "99999", "100000", "-1", "+8080", "08080", " 8080",
            "8080 ", "80.80", "0x1f90", "abcd", "\u{0668}\u{0660}\u{0668}\u{0660}", "1e4",
        ] {
            assert_eq!(
                monitor(&["--qa-monitor-port", bad]),
                Err(QaRootError::MalformedMonitorPort),
                "port {bad:?}"
            );
        }
    }

    #[test]
    fn monitor_port_rejects_missing_duplicate_and_equals_form() {
        assert_eq!(
            monitor(&["--qa-monitor-port"]),
            Err(QaRootError::MissingArg("--qa-monitor-port"))
        );
        assert_eq!(
            monitor(&["--qa-monitor-port", ""]),
            Err(QaRootError::MissingArg("--qa-monitor-port"))
        );
        assert_eq!(
            monitor(&["--qa-monitor-port", "--qa-root"]),
            Err(QaRootError::MissingArg("--qa-monitor-port"))
        );
        assert_eq!(
            monitor(&["--qa-monitor-port=8080"]),
            Err(QaRootError::MissingArg("--qa-monitor-port"))
        );
        // Duplicate is refused even when both values are valid and equal.
        assert_eq!(
            monitor(&["--qa-monitor-port", "8080", "--qa-monitor-port", "8080"]),
            Err(QaRootError::DuplicateArg("--qa-monitor-port"))
        );
    }

    #[test]
    fn monitor_port_does_not_disturb_root_or_backend_parsing() {
        let parts = [
            "--qa-root",
            "/x",
            "--qa-nonce",
            NONCE,
            "--qa-monitor-port",
            "8080",
            "--qa-credential-backend",
            "memory",
        ];
        let (root, nonce) = parse_args(argv(&parts)).unwrap();
        assert_eq!((root, nonce.as_str()), (PathBuf::from("/x"), NONCE));
        assert_eq!(monitor(&parts), Ok(Some(8080)));
        assert_eq!(backend(&parts, NONCE), Ok(CredentialBackendChoice::Memory));
    }

    #[test]
    fn monitor_auth_only_with_port_and_unpublished_before_proof() {
        assert_eq!(monitor_auth_for(None, NONCE), None);
        assert_eq!(monitor_auth_for(Some(8080), NONCE).as_deref(), Some(NONCE));
        // Nothing is published until `init_from_args` proves a root (these tests never call it).
        assert_eq!(monitor_auth_nonce(), None);
    }

    // ----- root validation --------------------------------------------------

    #[test]
    fn accepts_owned_direct_child_of_temp() {
        let fx = Fixture::new();
        let root = fx.owned("aleph-shell-qa-ok");
        let got = validate_with(&root, NONCE, &fx.0, &[]).unwrap();
        assert_eq!(got, root);
        // A trailing newline in the ownership file (editors, PowerShell) is fine.
        fs::write(root.join(NONCE_FILE), format!("{NONCE}\r\n")).unwrap();
        assert!(validate_with(&root, NONCE, &fx.0, &[]).is_ok());
    }

    #[test]
    fn rejects_relative_and_non_normalized() {
        let fx = Fixture::new();
        assert_eq!(
            validate_with(Path::new("aleph-shell-qa-x"), NONCE, &fx.0, &[]),
            Err(QaRootError::NotAbsolute)
        );
        let root = fx.owned("aleph-shell-qa-ok");
        let dotted = root.join("..").join("aleph-shell-qa-ok");
        assert_eq!(
            validate_with(&dotted, NONCE, &fx.0, &[]),
            Err(QaRootError::NonNormalizedPath)
        );
    }

    #[test]
    fn rejects_missing_file_and_wrong_prefix() {
        let fx = Fixture::new();
        assert_eq!(
            validate_with(&fx.0.join("aleph-shell-qa-nope"), NONCE, &fx.0, &[]),
            Err(QaRootError::NotFound)
        );
        let file = fx.0.join("aleph-shell-qa-file");
        fs::write(&file, NONCE).unwrap();
        assert_eq!(
            validate_with(&file, NONCE, &fx.0, &[]),
            Err(QaRootError::NotDirectory)
        );
        let other = fx.owned("other-prefix");
        assert_eq!(
            validate_with(&other, NONCE, &fx.0, &[]),
            Err(QaRootError::WrongPrefix)
        );
        // Prefix alone (empty suffix) is not enough.
        let bare = fx.owned("aleph-shell-qa-");
        assert_eq!(
            validate_with(&bare, NONCE, &fx.0, &[]),
            Err(QaRootError::WrongPrefix)
        );
    }

    #[test]
    fn rejects_bad_ownership_file() {
        let fx = Fixture::new();
        let none = fx.0.join("aleph-shell-qa-none");
        fs::create_dir(&none).unwrap();
        assert_eq!(
            validate_with(&none, NONCE, &fx.0, &[]),
            Err(QaRootError::NonceFileMissing)
        );
        let wrong = fx.owned("aleph-shell-qa-wrong");
        fs::write(wrong.join(NONCE_FILE), "ffffffffffffffffffffffffffffffff").unwrap();
        assert_eq!(
            validate_with(&wrong, NONCE, &fx.0, &[]),
            Err(QaRootError::NonceMismatch)
        );
        let dir_marker = fx.0.join("aleph-shell-qa-dirmarker");
        fs::create_dir_all(dir_marker.join(NONCE_FILE)).unwrap();
        assert_eq!(
            validate_with(&dir_marker, NONCE, &fx.0, &[]),
            Err(QaRootError::NonceFileInvalid)
        );
        let big = fx.owned("aleph-shell-qa-big");
        fs::write(big.join(NONCE_FILE), "a".repeat(1024)).unwrap();
        assert_eq!(
            validate_with(&big, NONCE, &fx.0, &[]),
            Err(QaRootError::NonceFileInvalid)
        );
    }

    #[test]
    fn rejects_outside_temp_and_temp_itself() {
        let fx = Fixture::new();
        let temp = fx.0.join("t");
        fs::create_dir(&temp).unwrap();
        // A sibling of the (injected) temp dir is outside it.
        let outside = fx.owned("aleph-shell-qa-outside");
        assert_eq!(
            validate_with(&outside, NONCE, &temp, &[]),
            Err(QaRootError::NotDirectChildOfTemp)
        );
        // Deeper than a direct child is refused too.
        let nested_parent = temp.join("sub");
        fs::create_dir(&nested_parent).unwrap();
        let nested = nested_parent.join("aleph-shell-qa-nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join(NONCE_FILE), NONCE).unwrap();
        assert_eq!(
            validate_with(&nested, NONCE, &temp, &[]),
            Err(QaRootError::NotDirectChildOfTemp)
        );
        // The temp dir itself is not a strict descendant of itself.
        assert!(validate_with(&temp, NONCE, &temp, &[]).is_err());
    }

    #[test]
    fn rejects_dot_aleph_component() {
        let fx = Fixture::new();
        let aleph = fx.0.join(".aleph");
        fs::create_dir(&aleph).unwrap();
        let root = aleph.join("aleph-shell-qa-x");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(NONCE_FILE), NONCE).unwrap();
        assert_eq!(
            validate_with(&root, NONCE, &aleph, &[]),
            Err(QaRootError::AlephComponent)
        );
        assert!(has_aleph_component(Path::new("C:\\Users\\x\\.ALEPH\\y")));
    }

    #[test]
    fn rejects_forbidden_trees() {
        let fx = Fixture::new();
        let root = fx.owned("aleph-shell-qa-forbid");
        // Root equal to a forbidden dir that does not also contain temp.
        assert_eq!(
            validate_with(&root, NONCE, &fx.0, std::slice::from_ref(&root)),
            Err(QaRootError::ForbiddenLocation)
        );
        // Root containing a forbidden dir (e.g. someone passes the whole home).
        let inner = root.join("home");
        assert_eq!(
            validate_with(&root, NONCE, &fx.0, &[inner]),
            Err(QaRootError::ForbiddenLocation)
        );
        // A forbidden tree that merely contains the temp dir (the normal
        // `%LOCALAPPDATA%\Temp` layout) must NOT veto a root under temp.
        let ancestor = fx.0.parent().unwrap().to_path_buf();
        assert!(validate_with(&root, NONCE, &fx.0, &[ancestor]).is_ok());
    }

    #[test]
    fn webview_dir_is_a_direct_child_name() {
        // Pure string rule: the subdir is a single normal component, so a
        // `join` can never escape the root.
        assert_eq!(Path::new(WEBVIEW_SUBDIR).components().count(), 1);
        assert!(matches!(
            Path::new(WEBVIEW_SUBDIR).components().next(),
            Some(Component::Normal(_))
        ));
    }
}
