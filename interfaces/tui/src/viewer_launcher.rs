//! Local launch seam for the live browser viewer (deny-only scaffolding).
//!
//! # What this module is - and is not
//!
//! The browser viewer is opened with a **launch URL minted by the Gateway
//! issuer** (`IssueViewerGrantResponse::launch_url`), shaped
//! `<origin>/<path>#grant_id=<id>&nonce=<secret>`. The nonce lives only in the
//! URL fragment: browsers never put a fragment on the wire, so it reaches
//! neither request lines, `Referer`, nor server logs.
//!
//! This module only owns the *last local hop*: validate a URL that the issuer
//! already produced, then hand it - as a single argv element, never through a
//! shell - to the system opener or an explicit browser executable.
//!
//! It deliberately does **not**:
//!
//! * call the issuer RPC, consume / exchange a nonce, or talk to any route;
//! * mint, guess, or reconstruct a grant (no URL scanning of chat text);
//! * create a CDP / Bridge connection, or accept debugging flags.
//!
//! The TUI has no provable remote-User issuer authentication yet and the
//! production Gateway viewer backend is still deny-only, so
//! [`ViewerLauncherConfig::default`] is **disabled**: [`ViewerLauncher::launch`]
//! returns [`LaunchError::Disabled`] and the runner is never invoked (zero
//! browser starts). Wiring a real caller is a separate step that must first
//! obtain an issuer-generated URL over an authenticated channel.
//!
//! # Secret handling
//!
//! [`ViewerLaunchRequest`], [`LaunchCommand`] and [`LaunchError`] never print the
//! URL or nonce (`Debug`/`Display` are redacted or never carry it). Residual
//! risk: argv is visible to same-user process listings for the lifetime of the
//! opener; the nonce is single-use and short-lived server-side, which is what
//! bounds that exposure.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Hard cap on the whole launch URL.
const MAX_URL_LEN: usize = 2048;
/// Mirrors `shared/protocol` (`grant_id` <= 64 ASCII bytes).
const MAX_GRANT_ID_LEN: usize = 64;
/// Mirrors `shared/protocol` (viewer secret is exactly 43 base64url bytes).
const NONCE_LEN: usize = 43;
/// How long an explicit-executable browser is probed for an early failure.
const EARLY_EXIT_PROBE: Duration = Duration::from_millis(500);

/// The ONLY extra flags an explicit browser executable may receive. Exact,
/// case-sensitive, value-less matches: a denylist of Chromium switches can
/// never be complete (`--renderer-cmd-prefix`, `--proxy-server`,
/// `--host-resolver-rules`, `--js-flags`, ... all change what the browser
/// runs or where it connects), so anything not named here - including every
/// `--key=value` form and every unknown flag - is refused.
const ALLOWED_EXTRA_ARGS: &[&str] = &[
    "--new-window",
    "--no-first-run",
    "--no-default-browser-check",
    "--incognito",
];

/// Why a launch URL was rejected. Variants never carry URL content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlRejection {
    TooLong,
    NotAscii,
    ForbiddenCharacter,
    BadScheme,
    /// `http://` is only allowed for loopback hosts listed as trusted origins.
    InsecureHttp,
    BadAuthority,
    HasQuery,
    MissingFragment,
    BadPath,
    BadFragment,
    BadGrantId,
    BadNonce,
    UntrustedOrigin,
}

impl fmt::Display for UrlRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLong => "launch URL is too long",
            Self::NotAscii => "launch URL must be ASCII",
            Self::ForbiddenCharacter => "launch URL contains a forbidden character",
            Self::BadScheme => "launch URL must be http(s)",
            Self::InsecureHttp => "http launch URL is only allowed for a trusted loopback origin",
            Self::BadAuthority => "launch URL authority must be a plain host[:port]",
            Self::HasQuery => "launch URL must not have a query string",
            Self::MissingFragment => "launch URL must carry a fragment",
            Self::BadPath => "launch URL path is not allowed",
            Self::BadFragment => "launch URL fragment must be exactly grant_id and nonce",
            Self::BadGrantId => "launch URL grant_id is malformed",
            Self::BadNonce => "launch URL nonce is malformed",
            Self::UntrustedOrigin => "launch URL origin is not a trusted origin",
        })
    }
}

/// Typed launch failure. None of the variants can echo the nonce or URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// Issuer authentication / viewer backend not ready: nothing was started.
    Disabled,
    InvalidUrl(UrlRejection),
    /// An extra browser argument was rejected (e.g. a CDP flag).
    ForbiddenArgument,
    /// Explicit browser executable path is empty or not valid UTF-8.
    InvalidExecutable,
    /// The opener could not be started (only the `io::ErrorKind` is kept).
    Spawn(io::ErrorKind),
    /// The opener exited unsuccessfully (`None` = terminated by signal).
    NonZeroExit(Option<i32>),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str(
                "viewer launch disabled: no authenticated Gateway issuer launch URL is available",
            ),
            Self::InvalidUrl(reason) => write!(f, "invalid viewer launch URL: {reason}"),
            Self::ForbiddenArgument => f.write_str("forbidden browser argument"),
            Self::InvalidExecutable => f.write_str("invalid browser executable"),
            Self::Spawn(kind) => write!(f, "failed to start browser: {kind}"),
            Self::NonZeroExit(Some(code)) => write!(f, "browser opener exited with status {code}"),
            Self::NonZeroExit(None) => f.write_str("browser opener terminated abnormally"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Which program opens the URL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BrowserChoice {
    /// Platform default opener (`rundll32` / `xdg-open` / `open`).
    #[default]
    SystemDefault,
    /// Explicit Chrome / Chromium (or any browser) executable; receives the URL
    /// as its last argument, preceded by validated `extra_args`.
    Executable(PathBuf),
}

/// Launcher configuration. `Default` is **disabled**.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewerLauncherConfig {
    /// Master switch. Stays `false` until an authenticated issuer path exists.
    pub enabled: bool,
    pub browser: BrowserChoice,
    /// Extra flags for [`BrowserChoice::Executable`]; ignored for the default
    /// opener. Each must exactly equal an entry of the built-in allowlist
    /// (`ALLOWED_EXTRA_ARGS`); everything else is refused. Default: none.
    pub extra_args: Vec<String>,
    /// Exact origins (`scheme://host[:port]`) the URL must match. When empty,
    /// any `https://` origin is accepted but `http://` is always refused.
    pub trusted_origins: Vec<String>,
}

/// Target platform for opener selection (injectable for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// An issuer-produced launch URL. Validated in [`ViewerLauncher::launch`].
#[derive(Clone, PartialEq, Eq)]
pub struct ViewerLaunchRequest {
    launch_url: String,
}

impl ViewerLaunchRequest {
    /// Wrap the `launch_url` of an issuer response. Validation happens in
    /// [`ViewerLauncher::launch`] against the configured trust policy.
    pub fn from_issuer_launch_url(launch_url: impl Into<String>) -> Self {
        Self {
            launch_url: launch_url.into(),
        }
    }
}

impl fmt::Debug for ViewerLaunchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewerLaunchRequest")
            .field("launch_url", &"[REDACTED]")
            .finish()
    }
}

/// How the runner should treat the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPolicy {
    /// Wait for exit (opener helpers return immediately).
    UntilExit,
    /// A browser binary may stay alive; only an exit within the probe window is
    /// observed, and a still-running child counts as launched.
    EarlyExitProbe,
}

/// Fully resolved process invocation. No shell is ever involved.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: String,
    pub args: Vec<String>,
    pub wait: WaitPolicy,
}

impl fmt::Debug for LaunchCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args: Vec<&str> = self
            .args
            .iter()
            .map(|a| {
                if a.contains("://") {
                    "[REDACTED_URL]"
                } else {
                    a.as_str()
                }
            })
            .collect();
        f.debug_struct("LaunchCommand")
            .field("program", &self.program)
            .field("args", &args)
            .field("wait", &self.wait)
            .finish()
    }
}

/// Exit observation of a launched process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandExit {
    /// Process exited (`None` = no code, i.e. killed by signal).
    Exited(Option<i32>),
    /// Process was still running when the probe window ended.
    StillRunning,
}

/// Injectable process runner so tests never start a real browser.
pub trait CommandRunner {
    fn run(&self, command: &LaunchCommand) -> io::Result<CommandExit>;
}

/// Production runner over `std::process::Command` (argv, no shell, no stdio).
#[derive(Debug, Default, Clone, Copy)]
pub struct StdCommandRunner;

impl CommandRunner for StdCommandRunner {
    fn run(&self, command: &LaunchCommand) -> io::Result<CommandExit> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        match command.wait {
            WaitPolicy::UntilExit => Ok(CommandExit::Exited(child.wait()?.code())),
            WaitPolicy::EarlyExitProbe => {
                let deadline = Instant::now() + EARLY_EXIT_PROBE;
                loop {
                    if let Some(status) = child.try_wait()? {
                        return Ok(CommandExit::Exited(status.code()));
                    }
                    if Instant::now() >= deadline {
                        return Ok(CommandExit::StillRunning);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
}

/// Validates then launches an issuer-produced viewer URL.
pub struct ViewerLauncher<R: CommandRunner> {
    config: ViewerLauncherConfig,
    runner: R,
    platform: Platform,
}

impl ViewerLauncher<StdCommandRunner> {
    /// Real-process launcher for the current platform.
    pub fn system(config: ViewerLauncherConfig) -> Self {
        Self::new(config, StdCommandRunner, Platform::current())
    }
}

impl<R: CommandRunner> ViewerLauncher<R> {
    pub fn new(config: ViewerLauncherConfig, runner: R, platform: Platform) -> Self {
        Self {
            config,
            runner,
            platform,
        }
    }

    /// Open the viewer. Disabled config returns [`LaunchError::Disabled`]
    /// **before** any parsing or process creation. A non-zero exit is an error;
    /// success is reported only for exit 0 (or a browser still running).
    pub fn launch(&self, request: &ViewerLaunchRequest) -> Result<(), LaunchError> {
        if !self.config.enabled {
            return Err(LaunchError::Disabled);
        }
        let command = build_launch_command(&self.config, &request.launch_url, self.platform)?;
        match self.runner.run(&command) {
            Ok(CommandExit::Exited(Some(0))) | Ok(CommandExit::StillRunning) => Ok(()),
            Ok(CommandExit::Exited(code)) => Err(LaunchError::NonZeroExit(code)),
            Err(error) => Err(LaunchError::Spawn(error.kind())),
        }
    }
}

/// Pure: validate the URL and extra args and build the argv. Never starts a process.
pub fn build_launch_command(
    config: &ViewerLauncherConfig,
    launch_url: &str,
    platform: Platform,
) -> Result<LaunchCommand, LaunchError> {
    validate_launch_url(launch_url, &config.trusted_origins).map_err(LaunchError::InvalidUrl)?;
    match &config.browser {
        BrowserChoice::SystemDefault => Ok(match platform {
            Platform::Windows => LaunchCommand {
                program: "rundll32.exe".to_string(),
                args: vec![
                    "url.dll,FileProtocolHandler".to_string(),
                    launch_url.to_string(),
                ],
                wait: WaitPolicy::UntilExit,
            },
            Platform::Linux => LaunchCommand {
                program: "xdg-open".to_string(),
                args: vec![launch_url.to_string()],
                wait: WaitPolicy::UntilExit,
            },
            Platform::MacOs => LaunchCommand {
                program: "open".to_string(),
                args: vec![launch_url.to_string()],
                wait: WaitPolicy::UntilExit,
            },
        }),
        BrowserChoice::Executable(path) => {
            let program = path.to_str().ok_or(LaunchError::InvalidExecutable)?;
            if program.trim().is_empty() || program.contains('\0') {
                return Err(LaunchError::InvalidExecutable);
            }
            validate_extra_args(&config.extra_args)?;
            let mut args = config.extra_args.clone();
            args.push(launch_url.to_string());
            Ok(LaunchCommand {
                program: program.to_string(),
                args,
                wait: WaitPolicy::EarlyExitProbe,
            })
        }
    }
}

/// Pure: allowlist check. Every extra arg must exactly equal an entry of
/// `ALLOWED_EXTRA_ARGS`, so extra args can never smuggle a second URL, a
/// positional target, a CDP / automation flag, or any unlisted Chromium
/// switch (fail closed on unknown).
pub fn validate_extra_args(args: &[String]) -> Result<(), LaunchError> {
    if args
        .iter()
        .all(|arg| ALLOWED_EXTRA_ARGS.contains(&arg.as_str()))
    {
        Ok(())
    } else {
        Err(LaunchError::ForbiddenArgument)
    }
}

/// Pure: check an issuer-shaped launch URL
/// `http(s)://host[:port][/path]#grant_id=<id>&nonce=<43 base64url>`.
///
/// * no query (`?` is rejected anywhere), no userinfo, no non-ASCII,
///   whitespace, control characters, backslash or quotes;
/// * `http://` only for `127.0.0.1` / `localhost` **and** a matching entry in
///   `trusted_origins`; `https://` must match `trusted_origins` when non-empty;
/// * the fragment holds exactly the two keys `grant_id` and `nonce`, once each.
pub fn validate_launch_url(url: &str, trusted_origins: &[String]) -> Result<(), UrlRejection> {
    if url.len() > MAX_URL_LEN {
        return Err(UrlRejection::TooLong);
    }
    if !url.is_ascii() {
        return Err(UrlRejection::NotAscii);
    }
    if url.chars().any(|c| {
        c.is_control()
            || c.is_whitespace()
            || matches!(c, '\\' | '"' | '\'' | '`' | '^' | '<' | '>')
    }) {
        return Err(UrlRejection::ForbiddenCharacter);
    }
    let (is_https, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(UrlRejection::BadScheme);
    };
    if url.contains('?') {
        return Err(UrlRejection::HasQuery);
    }
    let (before_fragment, fragment) = rest.split_once('#').ok_or(UrlRejection::MissingFragment)?;
    let authority_end = before_fragment.find('/').unwrap_or(before_fragment.len());
    let (authority, path) = before_fragment.split_at(authority_end);
    let host = validate_authority(authority)?;
    let lower_path = path.to_ascii_lowercase();
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/-._~%".contains(&b))
        || path.split('/').any(|seg| seg == ".." || seg == ".")
        || lower_path.contains("%2e")
        || lower_path.contains("%2f")
    {
        return Err(UrlRejection::BadPath);
    }
    let origin = format!(
        "{}://{}",
        if is_https { "https" } else { "http" },
        authority
    );
    let trusted = trusted_origins
        .iter()
        .any(|o| o.eq_ignore_ascii_case(&origin));
    if !is_https && (!matches!(host, "127.0.0.1" | "localhost") || !trusted) {
        return Err(UrlRejection::InsecureHttp);
    }
    if !trusted_origins.is_empty() && !trusted {
        return Err(UrlRejection::UntrustedOrigin);
    }
    validate_fragment(fragment)
}

/// Returns the host part. Accepts `host` or `host:port` of DNS/IPv4 characters.
fn validate_authority(authority: &str) -> Result<&str, UrlRejection> {
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && !host.contains("..")
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
    });
    if host_ok && port_ok {
        Ok(host)
    } else {
        Err(UrlRejection::BadAuthority)
    }
}

fn is_b64url(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_fragment(fragment: &str) -> Result<(), UrlRejection> {
    let mut grant_id = None;
    let mut nonce = None;
    for pair in fragment.split('&') {
        let (key, value) = pair.split_once('=').ok_or(UrlRejection::BadFragment)?;
        let slot = match key {
            "grant_id" => &mut grant_id,
            "nonce" => &mut nonce,
            _ => return Err(UrlRejection::BadFragment),
        };
        if slot.replace(value).is_some() {
            return Err(UrlRejection::BadFragment);
        }
    }
    let grant_id = grant_id.ok_or(UrlRejection::BadFragment)?;
    let nonce = nonce.ok_or(UrlRejection::BadFragment)?;
    if grant_id.is_empty() || grant_id.len() > MAX_GRANT_ID_LEN || !is_b64url(grant_id) {
        return Err(UrlRejection::BadGrantId);
    }
    if nonce.len() != NONCE_LEN || !is_b64url(nonce) {
        return Err(UrlRejection::BadNonce);
    }
    Ok(())
}

/// Env var that explicitly enables the launcher (`1`; unset / empty / `0` = off).
pub const ENV_VIEWER_ENABLED: &str = "ALEPH_VIEWER_ENABLED";
/// Env var holding the comma-separated exact origins the launcher may open.
pub const ENV_VIEWER_TRUSTED_ORIGINS: &str = "ALEPH_VIEWER_TRUSTED_ORIGINS";

/// Why the launcher environment could not be turned into an enabled config.
/// Fixed messages only: the offending value is never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// `ALEPH_VIEWER_ENABLED` is neither empty, `0` nor `1` (or not UTF-8).
    InvalidEnabled,
    /// Enabled, but `ALEPH_VIEWER_TRUSTED_ORIGINS` is unset or empty.
    MissingTrustedOrigins,
    /// An origin is empty, not UTF-8, or not exactly `http(s)://host[:port]`.
    InvalidTrustedOrigin,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnabled => write!(f, "{ENV_VIEWER_ENABLED} must be 0 or 1"),
            Self::MissingTrustedOrigins => write!(
                f,
                "{ENV_VIEWER_TRUSTED_ORIGINS} must list at least one origin when enabled"
            ),
            Self::InvalidTrustedOrigin => write!(
                f,
                "{ENV_VIEWER_TRUSTED_ORIGINS} must be comma-separated scheme://host[:port] origins"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Pure: build a launcher config from the two env values.
///
/// Fail-closed: only `enabled == "1"` AND a non-empty list of well-formed
/// origins yields `enabled: true`; the browser is always the system default
/// opener with no extra args. Everything else is the disabled default, and a
/// malformed value is an `Err` (callers fall back to disabled).
pub fn config_from_values(
    enabled: Option<&str>,
    trusted_origins: Option<&str>,
) -> Result<ViewerLauncherConfig, ConfigError> {
    match enabled {
        None | Some("") | Some("0") => return Ok(ViewerLauncherConfig::default()),
        Some("1") => {}
        Some(_) => return Err(ConfigError::InvalidEnabled),
    }
    let raw = trusted_origins.unwrap_or("");
    if raw.trim().is_empty() {
        return Err(ConfigError::MissingTrustedOrigins);
    }
    let mut origins = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if !is_valid_origin(entry) {
            return Err(ConfigError::InvalidTrustedOrigin);
        }
        origins.push(entry.to_string());
    }
    Ok(ViewerLauncherConfig {
        enabled: true,
        trusted_origins: origins,
        ..Default::default()
    })
}

/// Read [`ENV_VIEWER_ENABLED`] / [`ENV_VIEWER_TRUSTED_ORIGINS`] from the process
/// environment. Not UTF-8 is treated as invalid (fail-closed).
pub fn config_from_env() -> Result<ViewerLauncherConfig, ConfigError> {
    fn read(name: &str, invalid: ConfigError) -> Result<Option<String>, ConfigError> {
        match std::env::var(name) {
            Ok(v) => Ok(Some(v)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(invalid),
        }
    }
    let enabled = read(ENV_VIEWER_ENABLED, ConfigError::InvalidEnabled)?;
    let origins = read(ENV_VIEWER_TRUSTED_ORIGINS, ConfigError::InvalidTrustedOrigin)?;
    config_from_values(enabled.as_deref(), origins.as_deref())
}

/// Exactly `http://` or `https://` + a valid authority; no path, query, fragment.
fn is_valid_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    rest.is_some_and(|authority| validate_authority(authority).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const NONCE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";
    const GRANT: &str = "grant-public_1";

    fn url(base: &str) -> String {
        format!("{base}#grant_id={GRANT}&nonce={NONCE}")
    }

    fn enabled() -> ViewerLauncherConfig {
        ViewerLauncherConfig {
            enabled: true,
            ..Default::default()
        }
    }

    struct FakeRunner {
        calls: RefCell<Vec<LaunchCommand>>,
        result: fn() -> io::Result<CommandExit>,
    }

    impl FakeRunner {
        fn ok() -> Self {
            Self::with(|| Ok(CommandExit::Exited(Some(0))))
        }
        fn with(result: fn() -> io::Result<CommandExit>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                result,
            }
        }
    }

    impl CommandRunner for &FakeRunner {
        fn run(&self, command: &LaunchCommand) -> io::Result<CommandExit> {
            self.calls.borrow_mut().push(command.clone());
            (self.result)()
        }
    }

    #[test]
    fn issuer_shaped_https_url_is_accepted() {
        for base in [
            "https://viewer.example.com/viewer/browser",
            "https://viewer.example.com",
            "https://viewer.example.com:8443/v",
        ] {
            assert_eq!(validate_launch_url(&url(base), &[]), Ok(()), "{base}");
        }
    }

    #[test]
    fn http_requires_loopback_and_explicit_trust() {
        let u = url("http://127.0.0.1:18790/viewer/browser");
        assert_eq!(
            validate_launch_url(&u, &[]),
            Err(UrlRejection::InsecureHttp)
        );
        let trusted = vec!["http://127.0.0.1:18790".to_string()];
        assert_eq!(validate_launch_url(&u, &trusted), Ok(()));
        let lh = vec!["http://localhost:1".to_string()];
        assert_eq!(
            validate_launch_url(&url("http://localhost:1/v"), &lh),
            Ok(())
        );
        // Trusted but non-loopback http is still refused.
        let remote = vec!["http://example.com".to_string()];
        assert_eq!(
            validate_launch_url(&url("http://example.com/v"), &remote),
            Err(UrlRejection::InsecureHttp)
        );
    }

    #[test]
    fn trusted_origin_list_filters_https() {
        let trusted = vec!["https://viewer.example.com".to_string()];
        assert_eq!(
            validate_launch_url(&url("https://viewer.example.com/v"), &trusted),
            Ok(())
        );
        assert_eq!(
            validate_launch_url(&url("https://evil.example.com/v"), &trusted),
            Err(UrlRejection::UntrustedOrigin)
        );
    }

    #[test]
    fn rejects_bad_scheme() {
        for base in [
            "ftp://a.example/v",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "a.example/v",
        ] {
            assert_eq!(
                validate_launch_url(&url(base), &[]),
                Err(UrlRejection::BadScheme),
                "{base}"
            );
        }
    }

    #[test]
    fn rejects_query_secrets_anywhere() {
        let q = format!(
            "https://a.example/v?grant_id={GRANT}&nonce={NONCE}#grant_id={GRANT}&nonce={NONCE}"
        );
        assert_eq!(validate_launch_url(&q, &[]), Err(UrlRejection::HasQuery));
        let secret_in_query_only = format!("https://a.example/v?nonce={NONCE}");
        assert!(validate_launch_url(&secret_in_query_only, &[]).is_err());
        let q_in_fragment = format!("https://a.example/v#grant_id={GRANT}&nonce={NONCE}?x=1");
        assert_eq!(
            validate_launch_url(&q_in_fragment, &[]),
            Err(UrlRejection::HasQuery)
        );
    }

    #[test]
    fn rejects_missing_or_malformed_fragment() {
        assert_eq!(
            validate_launch_url("https://a.example/v", &[]),
            Err(UrlRejection::MissingFragment)
        );
        let cases = [
            format!("https://a.example/v#nonce={NONCE}"),
            format!("https://a.example/v#grant_id={GRANT}"),
            format!("https://a.example/v#grant_id={GRANT}&nonce={NONCE}&secret=x"),
            format!("https://a.example/v#grant_id={GRANT}&grant_id={GRANT}&nonce={NONCE}"),
            format!("https://a.example/v#grant_id={GRANT}&nonce={NONCE}&nonce={NONCE}"),
            format!("https://a.example/v#grant_id={GRANT}&nonce"),
            "https://a.example/v#".to_string(),
        ];
        for c in cases {
            assert_eq!(
                validate_launch_url(&c, &[]),
                Err(UrlRejection::BadFragment),
                "{c}"
            );
        }
        assert_eq!(
            validate_launch_url(&format!("https://a.example/v#grant_id=&nonce={NONCE}"), &[]),
            Err(UrlRejection::BadGrantId)
        );
        assert_eq!(
            validate_launch_url(
                &format!(
                    "https://a.example/v#grant_id={}&nonce={NONCE}",
                    "g".repeat(65)
                ),
                &[]
            ),
            Err(UrlRejection::BadGrantId)
        );
        assert_eq!(
            validate_launch_url(
                &format!("https://a.example/v#grant_id={GRANT}&nonce=short"),
                &[]
            ),
            Err(UrlRejection::BadNonce)
        );
        assert_eq!(
            validate_launch_url(
                &format!(
                    "https://a.example/v#grant_id={GRANT}&nonce={}%",
                    &NONCE[..42]
                ),
                &[]
            ),
            Err(UrlRejection::BadNonce)
        );
    }

    #[test]
    fn rejects_bad_authority_and_path() {
        let bad_authority = [
            "https://user@a.example/v",
            "https://user:pw@a.example/v",
            "https://a.example@evil.example/v",
            "https://[::1]/v",
            "https://a.example:/v",
            "https://a.example:0/v",
            "https://a.example:99999/v",
            "https://a.example:80:80/v",
            "https:///v",
            "https://",
            "https://-a.example/v",
            "https://a..example/v",
        ];
        for base in bad_authority {
            // `https://` alone has no fragment; give it one so authority is what fails.
            assert_eq!(
                validate_launch_url(&url(base), &[]),
                Err(UrlRejection::BadAuthority),
                "{base}"
            );
        }
        for base in [
            "https://a.example/../v",
            "https://a.example/%2e%2e/v",
            "https://a.example/a%2fb",
        ] {
            assert_eq!(
                validate_launch_url(&url(base), &[]),
                Err(UrlRejection::BadPath),
                "{base}"
            );
        }
        for base in [
            "https://a.example\\@evil.example/v",
            "https://a.example/v w",
            "https://a.example/v\n",
            "https://a.example/\"v",
        ] {
            assert_eq!(
                validate_launch_url(&url(base), &[]),
                Err(UrlRejection::ForbiddenCharacter),
                "{base:?}"
            );
        }
        assert_eq!(
            validate_launch_url(&url("https://a.example/v\u{e9}"), &[]),
            Err(UrlRejection::NotAscii)
        );
    }

    #[test]
    fn default_browser_argv_per_platform_is_a_single_url_argument() {
        let u = url("https://a.example/v");
        let w = build_launch_command(&enabled(), &u, Platform::Windows).unwrap();
        assert_eq!(w.program, "rundll32.exe");
        assert_eq!(
            w.args,
            vec!["url.dll,FileProtocolHandler".to_string(), u.clone()]
        );
        let l = build_launch_command(&enabled(), &u, Platform::Linux).unwrap();
        assert_eq!(
            (l.program.as_str(), l.args.clone()),
            ("xdg-open", vec![u.clone()])
        );
        let m = build_launch_command(&enabled(), &u, Platform::MacOs).unwrap();
        assert_eq!((m.program.as_str(), m.args), ("open", vec![u]));
    }

    #[test]
    fn explicit_chrome_gets_explicit_executable_and_url_last() {
        let u = url("https://a.example/v");
        let cfg = ViewerLauncherConfig {
            browser: BrowserChoice::Executable(PathBuf::from(
                "C:/Program Files/Google/Chrome/chrome.exe",
            )),
            extra_args: vec!["--new-window".to_string()],
            ..enabled()
        };
        let cmd = build_launch_command(&cfg, &u, Platform::Windows).unwrap();
        assert_eq!(cmd.program, "C:/Program Files/Google/Chrome/chrome.exe");
        assert_eq!(cmd.args, vec!["--new-window".to_string(), u]);
        assert_eq!(cmd.wait, WaitPolicy::EarlyExitProbe);
        assert!(cmd.args.iter().all(|a| !a.contains("remote-debugging")));
    }

    #[test]
    fn forbidden_unknown_cdp_and_positional_args_are_rejected() {
        let u = url("https://a.example/v");
        for bad in [
            "--remote-debugging-port=9222",
            "--remote-debugging-pipe",
            "--REMOTE-DEBUGGING-PORT=0",
            "--remote-allow-origins=*",
            "--enable-automation",
            "--headless=new",
            "--user-data-dir=C:/x",
            "https://other.example/",
            "--app=https://other.example/",
            "positional",
            "--new window",
            "--renderer-cmd-prefix=/bin/sh",
            "--utility-cmd-prefix=calc.exe",
            "--proxy-server=http://attacker.example:8080",
            "--host-resolver-rules=MAP * attacker.example",
            "--js-flags=--allow-natives-syntax",
            "--some-future-unknown-switch",
            "--new-window=1",
            "--NEW-WINDOW",
            "",
        ] {
            let cfg = ViewerLauncherConfig {
                browser: BrowserChoice::Executable(PathBuf::from("chromium")),
                extra_args: vec![bad.to_string()],
                ..enabled()
            };
            assert_eq!(
                build_launch_command(&cfg, &u, Platform::Linux),
                Err(LaunchError::ForbiddenArgument),
                "{bad}"
            );
        }
        let empty_exe = ViewerLauncherConfig {
            browser: BrowserChoice::Executable(PathBuf::from("  ")),
            ..enabled()
        };
        assert_eq!(
            build_launch_command(&empty_exe, &u, Platform::Linux),
            Err(LaunchError::InvalidExecutable)
        );
    }

    #[test]
    fn allowlisted_args_pass_and_empty_is_default() {
        let u = url("https://a.example/v");
        assert_eq!(validate_extra_args(&[]), Ok(()));
        let all: Vec<String> = ALLOWED_EXTRA_ARGS.iter().map(|a| a.to_string()).collect();
        let cfg = ViewerLauncherConfig {
            browser: BrowserChoice::Executable(PathBuf::from("chromium")),
            extra_args: all.clone(),
            ..enabled()
        };
        let cmd = build_launch_command(&cfg, &u, Platform::Linux).unwrap();
        let (last, flags) = cmd.args.split_last().unwrap();
        assert_eq!(last, &u, "URL stays the single last argv element");
        assert_eq!(flags, all.as_slice());
        // One bad entry among allowed ones rejects the whole set.
        let mixed = vec!["--new-window".to_string(), "--js-flags=x".to_string()];
        assert_eq!(
            validate_extra_args(&mixed),
            Err(LaunchError::ForbiddenArgument)
        );
    }

    #[test]
    fn default_config_is_disabled_and_never_runs() {
        let runner = FakeRunner::ok();
        let launcher =
            ViewerLauncher::new(ViewerLauncherConfig::default(), &runner, Platform::Linux);
        let req = ViewerLaunchRequest::from_issuer_launch_url(url("https://a.example/v"));
        assert_eq!(launcher.launch(&req), Err(LaunchError::Disabled));
        // Disabled wins even over an invalid URL: no parsing, no process.
        let bad = ViewerLaunchRequest::from_issuer_launch_url("not a url");
        assert_eq!(launcher.launch(&bad), Err(LaunchError::Disabled));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn invalid_url_never_reaches_the_runner() {
        let runner = FakeRunner::ok();
        let launcher = ViewerLauncher::new(enabled(), &runner, Platform::Linux);
        let req = ViewerLaunchRequest::from_issuer_launch_url("https://a.example/v?nonce=x");
        assert_eq!(
            launcher.launch(&req),
            Err(LaunchError::InvalidUrl(UrlRejection::HasQuery))
        );
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn success_runs_exactly_one_command() {
        let runner = FakeRunner::ok();
        let launcher = ViewerLauncher::new(enabled(), &runner, Platform::Linux);
        let u = url("https://a.example/v");
        launcher
            .launch(&ViewerLaunchRequest::from_issuer_launch_url(u.clone()))
            .unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args, vec![u]);
    }

    #[test]
    fn nonzero_exit_and_spawn_failure_are_errors_without_nonce() {
        let req = ViewerLaunchRequest::from_issuer_launch_url(url("https://a.example/v"));
        let nonzero = FakeRunner::with(|| Ok(CommandExit::Exited(Some(3))));
        let launcher = ViewerLauncher::new(enabled(), &nonzero, Platform::Linux);
        let err = launcher.launch(&req).unwrap_err();
        assert_eq!(err, LaunchError::NonZeroExit(Some(3)));
        assert!(!err.to_string().contains(NONCE) && !format!("{err:?}").contains(NONCE));

        let signalled = FakeRunner::with(|| Ok(CommandExit::Exited(None)));
        let launcher = ViewerLauncher::new(enabled(), &signalled, Platform::Linux);
        assert_eq!(launcher.launch(&req), Err(LaunchError::NonZeroExit(None)));

        // An io error whose message echoes the argv must not leak it.
        let spawn = FakeRunner::with(|| {
            Err(io::Error::other(format!(
                "bad arg https://a.example/v#nonce={NONCE}"
            )))
        });
        let launcher = ViewerLauncher::new(enabled(), &spawn, Platform::Linux);
        let err = launcher.launch(&req).unwrap_err();
        assert_eq!(err, LaunchError::Spawn(io::ErrorKind::Other));
        assert!(!err.to_string().contains(NONCE) && !format!("{err:?}").contains(NONCE));
    }

    #[test]
    fn still_running_browser_counts_as_launched() {
        let runner = FakeRunner::with(|| Ok(CommandExit::StillRunning));
        let cfg = ViewerLauncherConfig {
            browser: BrowserChoice::Executable(PathBuf::from("chromium")),
            ..enabled()
        };
        let launcher = ViewerLauncher::new(cfg, &runner, Platform::Linux);
        let req = ViewerLaunchRequest::from_issuer_launch_url(url("https://a.example/v"));
        assert_eq!(launcher.launch(&req), Ok(()));
    }

    #[test]
    fn debug_and_display_never_contain_url_or_nonce() {
        let u = url("https://a.example/v");
        let req = ViewerLaunchRequest::from_issuer_launch_url(u.clone());
        let cmd = build_launch_command(&enabled(), &u, Platform::Windows).unwrap();
        for text in [format!("{req:?}"), format!("{cmd:?}")] {
            assert!(!text.contains(NONCE), "{text}");
            assert!(!text.contains("a.example"), "{text}");
        }
        // The only place the nonce appears is the single fragment argv element,
        // and never before the `#` (i.e. never in a query or path).
        let carriers: Vec<_> = cmd.args.iter().filter(|a| a.contains(NONCE)).collect();
        assert_eq!(carriers.len(), 1);
        assert!(!carriers[0].split('#').next().unwrap().contains(NONCE));
        assert!(!cmd.program.contains(NONCE));
    }

    #[test]
    fn env_config_is_disabled_unless_explicitly_enabled_with_origins() {
        for enabled in [None, Some(""), Some("0")] {
            let cfg = config_from_values(enabled, Some("https://v.example")).unwrap();
            assert_eq!(cfg, ViewerLauncherConfig::default());
            assert!(!cfg.enabled);
        }
        assert_eq!(
            config_from_values(Some("1"), None),
            Err(ConfigError::MissingTrustedOrigins)
        );
        assert_eq!(
            config_from_values(Some("1"), Some("  ")),
            Err(ConfigError::MissingTrustedOrigins)
        );
        let cfg = config_from_values(
            Some("1"),
            Some("https://v.example, http://127.0.0.1:18790"),
        )
        .unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.browser, BrowserChoice::SystemDefault);
        assert!(cfg.extra_args.is_empty());
        assert_eq!(
            cfg.trusted_origins,
            vec!["https://v.example", "http://127.0.0.1:18790"]
        );
    }

    #[test]
    fn env_config_rejects_malformed_values_without_echoing_them() {
        for enabled in ["true", "yes", "2", " 1"] {
            assert_eq!(
                config_from_values(Some(enabled), Some("https://v.example")),
                Err(ConfigError::InvalidEnabled),
                "{enabled}"
            );
        }
        for origins in [
            "https://v.example,",
            ",https://v.example",
            "v.example",
            "ftp://v.example",
            "https://v.example/viewer",
            "https://v.example#grant_id=x",
            "https://user@v.example",
            "https://",
            "https://v.example:0",
        ] {
            assert_eq!(
                config_from_values(Some("1"), Some(origins)),
                Err(ConfigError::InvalidTrustedOrigin),
                "{origins}"
            );
        }
        let text = ConfigError::InvalidTrustedOrigin.to_string();
        assert!(!text.contains("v.example"));
    }
}
