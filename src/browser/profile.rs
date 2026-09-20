// Browser profile configuration and system-level config.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::engine::Engine;
use super::network_policy::SsrfConfig;

/// Supported browser engines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BrowserType {
    #[default]
    Chromium,
    Chrome,
    Brave,
    Edge,
}

/// How Aleph talks to the browser. **Orthogonal to `Engine`**, which is what
/// the browser IS (spec §5.4).
///
/// The two legacy spellings are frozen: `"managed"` and `"existing_session"`
/// are in every config file on every install, and a config value that quietly
/// stops parsing is a silent driver change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDriver {
    /// Aleph launches a dedicated Chromium and drives it through
    /// `playwright-cli attach --cdp`.
    Managed,
    /// Attach to the user's running Chrome via the Chrome `DevTools` MCP
    /// server.
    ExistingSession,
    /// Aleph's own CDP client drives the engine directly — the only driver
    /// obscura has, and the one Chromium gains as the escape hatch.
    ///
    /// **The default since the dual-engine round.** It is the only driver that
    /// can speak to obscura, and obscura is `default_engine`; leaving the
    /// default on `Managed` would have made the engine default unreachable
    /// without a config edit, because `resolved_engine` pins a legacy driver to
    /// Chromium.
    ///
    /// The flip lives HERE, on the type, and not at either of
    /// `ProfileManager::new`'s two auto-injection sites — a driver named
    /// explicitly at one site and defaulted at the other is how those two
    /// could disagree about it, which is 判据 §1 with the two authors one
    /// screen apart.
    #[default]
    Cdp,
}

impl BrowserDriver {
    /// Every driver, so an enumerator reaches a new one by adding a variant
    /// here rather than by being remembered elsewhere (判据 §5).
    pub const ALL: [Self; 3] = [Self::Managed, Self::ExistingSession, Self::Cdp];

    /// The wire spelling — the same string serde reads and writes.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::ExistingSession => "existing_session",
            Self::Cdp => "cdp",
        }
    }

    /// The inverse of [`Self::as_wire`], exact match only.
    ///
    /// Exists because the Panel's config surface is a `String` in both
    /// directions (`gateway::handlers::browser_config`), and a two-way `if`
    /// there silently rewrote every driver it did not recognise into
    /// `Managed` — a config change the operator never made, produced by
    /// saving an unrelated field.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_wire() == s)
    }
}

/// Per-profile browser configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileConfig {
    /// Which browser binary to use, within the Chromium family.
    ///
    /// **Selects nothing on `engine = obscura`, which is the default**: obscura
    /// is one binary out of the runtime ledger, not a family. The value is read
    /// on `engine = chromium` (`engine::chromium::launch` →
    /// `chromium_resolve::resolve_binary`) and on the managed driver, where it
    /// steers `discovery::find_chromium_preferred`. The managed driver no
    /// longer passes the engine to `playwright-cli` (it launches the browser
    /// itself). When the requested one is not installed the search degrades to
    /// whatever is, and `PlaywrightCliDriver::ensure_chromium` warns with the
    /// engine it actually resolved — the substitution is reported by the code
    /// that performs it, which is why the old boot-time warning is gone.
    #[serde(default)]
    pub browser: BrowserType,

    /// Profile-level override for headless (None = follow global `playwright_cli.headless`).
    ///
    /// **Ignored on `engine = obscura`, which is the default**: obscura has no
    /// window and no `--headless`, so `headless = false` is warned about and the
    /// launch proceeds headless anyway (`engine::obscura::launch`). Honoured by
    /// the managed and existing-session drivers and on `engine = chromium`.
    /// Which setting wins for a given profile is computed rather than guessed —
    /// `gateway::handlers::browser_config` reports it as `headless_shadowed_by`.
    #[serde(default)]
    pub headless: Option<bool>,

    /// Proxy server URL (e.g. "<socks5://127.0.0.1:1080>").
    ///
    /// Honored by every driver and both engines: Chrome's `--proxy-server` on
    /// the existing-session and cdp-chromium launch argv, obscura's own
    /// `--proxy=`, and `browser.launchOptions.proxy.server` in the generated
    /// `open --config` file on the managed side. The CLI has no proxy *flag*,
    /// which is why this was once documented as existing-session only — the
    /// surface is the config file, not the flag list.
    #[serde(default)]
    pub proxy: Option<String>,

    /// Custom user data directory for browser state isolation.
    ///
    /// Honored by every driver: Chrome's `--user-data-dir` on the
    /// existing-session and cdp-chromium launch argv, obscura's own storage-dir
    /// flag, and `browser.userDataDir` in the generated `open --config` file on
    /// the managed side. Left unset, a managed session keeps its profile in
    /// memory.
    ///
    /// **On a cdp profile the value is per-engine.** A directory written by one
    /// engine is in that engine's format and the other cannot read it, so
    /// `ProfileManager::request_from` hands the target engine the managed path
    /// under its own `data_subdir` instead of this one, and `switch_engine`
    /// warns when it does.
    #[serde(default)]
    pub user_data_dir: Option<String>,

    /// Extra command-line arguments passed to the browser process.
    ///
    /// Honored by every driver, but **the POSITION differs between them and the
    /// difference is a security property**. On the cdp engines they go
    /// **first** — `engine::chromium::ChromiumLaunchSpec::argv` and
    /// `engine::obscura::obscura_argv`, each with a test pinning the order — so
    /// that an operator's duplicate of a flag the launch depends on
    /// (`--use-mock-keychain`, `--password-store=basic`,
    /// `--remote-debugging-port`) cannot displace it: Chrome takes the FIRST
    /// occurrence. obscura additionally **refuses** a launch whose `extra_args`
    /// names a flag it decides itself (`reserved_flag_in`). On the
    /// existing-session side they are appended last, and on the managed side
    /// they are passed as `browser.launchOptions.args` in the generated
    /// `open --config` file.
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Seconds of inactivity before the browser session is automatically torn
    /// down.
    ///
    /// Honored by the managed and existing-session drivers:
    /// `ProfileManager::reap_idle` destroys the Chrome MCP session for an
    /// existing-session profile and runs `playwright-cli close` for a managed
    /// one. (The CLI does have a stop-session command — `close`, plus
    /// `close-all` / `kill-all`; this was once documented as having none, so the
    /// setting was accepted and never enforced on the managed side.)
    ///
    /// ⚠️ **Not enforced on `driver = cdp`, which is the default.**
    /// `idle_managed_profiles` filters `Managed` because it judges liveness
    /// through `chromium_alive`, which cannot answer for an engine — so on a
    /// cdp profile this setting has a writer and no reader until the CDP reaper
    /// lands. Said here, and not only in `manager.rs`, because this is the line
    /// an operator reads when they set it. Tabs are reclaimed sooner and
    /// separately via [`Self::tab_idle_timeout_secs`], which **is** enforced on
    /// cdp.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,

    /// Driver mode: managed (launch dedicated browser) or existing-session (attach to user's Chrome).
    #[serde(default)]
    pub driver: BrowserDriver,

    /// Which engine backs this profile. `None` follows
    /// `[general.browser] default_engine` — resolve it through
    /// [`Self::resolved_engine`], never by reading this field directly, so
    /// "which engine is this profile on" has exactly one answer.
    #[serde(default)]
    pub engine: Option<Engine>,

    /// Max concurrently-open tabs for this profile before the least-recently-used
    /// are reclaimed on the next sweep. Enforced for `Managed` **and `Cdp`**
    /// profiles (Aleph-owned browsers); `ExistingSession` tabs belong to the
    /// user and are never reaped. (openclaw per-session cap parity.)
    ///
    /// The driver pair is derived in the code from `BrowserDriver::ALL` and
    /// pinned by `the_tab_sweeper_selects_exactly_the_drivers_touch_tab_records`,
    /// so `reap_idle_tabs` and `touch_tab` cannot drift apart. **This sentence
    /// is not derived from that guard** — it said `Managed` only for a month
    /// after the code learned otherwise, and nothing could notice. Anchoring
    /// the prose to the same set is a named follow-up.
    #[serde(default = "default_max_tabs")]
    pub max_tabs_per_profile: usize,

    /// Seconds a tab may sit idle before it is reclaimed. Shorter than
    /// `idle_timeout_secs` — an unused tab is cheap to reopen. `Managed` and
    /// `Cdp`, as above.
    #[serde(default = "default_tab_idle_timeout")]
    pub tab_idle_timeout_secs: u64,
}

const fn default_idle_timeout() -> u64 {
    1800
}

const fn default_max_tabs() -> usize {
    super::tab_registry::DEFAULT_MAX_TABS_PER_PROFILE
}

const fn default_tab_idle_timeout() -> u64 {
    super::tab_registry::DEFAULT_TAB_IDLE_TIMEOUT_SECS
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            browser: BrowserType::default(),
            headless: None,
            proxy: None,
            user_data_dir: None,
            extra_args: Vec::new(),
            idle_timeout_secs: default_idle_timeout(),
            driver: BrowserDriver::default(),
            engine: None,
            max_tabs_per_profile: default_max_tabs(),
            tab_idle_timeout_secs: default_tab_idle_timeout(),
        }
    }
}

impl ProfileConfig {
    /// The engine this profile actually runs on.
    ///
    /// The ONE place the precedence lives, and the rule is **not** simply
    /// "per-profile beats global": a **legacy driver pins the engine**
    /// (skeleton Global Constraints; spec §5.4 「旧配置的
    /// `driver=managed_cli|chrome_mcp` 隐含 `engine=chromium`」).
    ///
    /// `managed` launches a Chromium-family binary and `existing_session`
    /// attaches to the user's Chrome. A profile that names either has already
    /// named its engine, and dragging it onto a new global default it cannot
    /// run would refuse the config at load — on every install that has ever
    /// saved the Panel's browser settings, because
    /// `gateway::handlers::browser_config::handle_update` writes `driver` into
    /// both the `default` and `user` profiles on every save.
    ///
    /// So `default_engine` is consulted **only** under `Cdp`, the driver that
    /// can actually run either engine. An explicit `engine` on a legacy driver
    /// does not silently win here — it is a contradiction, and
    /// [`validate_engine_driver`] refuses it by name at load rather than
    /// letting this function pick a side.
    #[must_use]
    pub fn resolved_engine(&self, default_engine: Engine) -> Engine {
        match self.driver {
            BrowserDriver::Managed | BrowserDriver::ExistingSession => Engine::Chromium,
            BrowserDriver::Cdp => self.engine.unwrap_or(default_engine),
        }
    }
}

// `default_true` lives in `super::network_policy` (single source); the
// serde `default = "..."` attribute on this module's bool fields uses
// a small local wrapper because serde macros require a path resolvable
// in the current module's scope.
const fn default_true() -> bool {
    super::network_policy::default_true()
}

/// Configuration for the Playwright CLI integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaywrightCliConfig {
    /// Optional override: absolute path to `playwright-cli` binary.
    /// When `None`, resolved via `fnm exec --using lts which playwright-cli`.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Global default: run headless (profile-level `headless: Option<bool>` overrides).
    #[serde(default = "default_true")]
    pub headless: bool,

    /// Timeout (seconds) for navigate / `wait_for_text`.
    #[serde(default = "default_nav_timeout")]
    pub nav_timeout_secs: u64,

    /// Timeout (seconds) for other actions (click/fill/type/etc).
    #[serde(default = "default_action_timeout")]
    pub action_timeout_secs: u64,
}

const fn default_nav_timeout() -> u64 {
    30
}
const fn default_action_timeout() -> u64 {
    10
}

impl Default for PlaywrightCliConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            headless: true,
            nav_timeout_secs: 30,
            action_timeout_secs: 10,
        }
    }
}

/// External-runtime settings for the managed driver's browser.
///
/// Chromium is deliberately NOT in any Aleph installer (D4): all three
/// artifacts stay Chromium-free and the browser is supplied at runtime, the
/// same way `playwright-cli` already is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserRuntimeConfig {
    /// Absolute path to a Chromium-family binary, pinned by the operator.
    /// Highest precedence — a pin that does not exist is a hard failure, not a
    /// fallback, because silently launching a different browser than the one
    /// named is worse than refusing.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Use a system-installed Chromium-family browser (via
    /// `discovery::find_chromium_preferred`) before Playwright's own.
    ///
    /// Default `true`: Windows almost always has Edge and macOS usually has
    /// Chrome, so the ~150 MB download is only for a clean Linux host. The
    /// Chrome spike ran system Chrome 152 against playwright-core 1.60 with no
    /// trouble, so the cross-version mixing this permits is measured, not hoped.
    #[serde(default = "default_true")]
    pub prefer_system_browser: bool,

    /// `PLAYWRIGHT_DOWNLOAD_HOST` for the install. Playwright's CDN is blocked
    /// on some networks exactly as GitHub release assets are; npmmirror carries
    /// a mirror. A config key rather than "go export a variable", because the
    /// installer runs inside the daemon.
    #[serde(default)]
    pub download_host: Option<String>,
}

impl BrowserRuntimeConfig {
    /// The pinned binary, or `None` when unset **or blank**.
    ///
    /// A cleared form field posts `""`, and `Some("")` would be spent as a path
    /// — resolving to the current directory and failing with a message that
    /// names nothing.
    #[must_use]
    pub fn pinned_binary(&self) -> Option<&str> {
        self.binary_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// The download mirror, or `None` when unset or blank. See
    /// [`Self::pinned_binary`] for why blank is not a value.
    #[must_use]
    pub fn download_host(&self) -> Option<&str> {
        self.download_host
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

impl Default for BrowserRuntimeConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            prefer_system_browser: true,
            download_host: None,
        }
    }
}

/// Which obscura archive a profile runs.
///
/// `default` carries the renderer; `stealth` is the anti-detection build and
/// is opt-in per profile (spec §6.1). Never `no-render` — Aleph needs layout
/// geometry for the page-state tree, which is exactly what that variant drops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ObscuraVariant {
    #[default]
    Default,
    Stealth,
}

/// External-runtime settings for obscura — the twin of
/// [`BrowserRuntimeConfig`], for the other engine.
///
/// A separate table rather than three more keys on `[runtime]`: that section's
/// keys are Chromium's (`prefer_system_browser` has no obscura meaning — there
/// is no "system obscura"), and folding them would make each key's meaning
/// depend on a value elsewhere.
///
/// `pinned_binary`/`download_host` mirror [`BrowserRuntimeConfig`]'s exactly
/// (same field type, same trim/empty-filter semantics) — two runtime configs
/// answering one question two ways is the shape this branch spends most of
/// its review budget on (判据 §1).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ObscuraRuntimeConfig {
    /// Absolute path to an obscura binary, pinned by the operator. Highest
    /// precedence, and a pin that does not exist is a hard failure rather
    /// than a fallback — same rule as [`BrowserRuntimeConfig::binary_path`],
    /// for the same reason: launching a different binary than the one named
    /// is worse than refusing.
    #[serde(default)]
    pub binary_path: Option<String>,

    /// Which release archive to install and run.
    #[serde(default)]
    pub variant: ObscuraVariant,

    /// Mirror host for the release-asset download.
    ///
    /// Same key name and same meaning as
    /// [`BrowserRuntimeConfig::download_host`]: GitHub's release-assets host is
    /// DNS-blocked on some networks exactly as Playwright's CDN is, and the
    /// installer runs inside the daemon, so "go export a variable" is not a
    /// remedy an operator can reach.
    ///
    /// It is also what keeps `[general.browser.obscura]` safe to name in an
    /// operator-facing message: `config::dead_keys`'s census (the
    /// `every_operator_facing_browser_config_path_is_actually_read` test)
    /// takes every bracketed browser path it finds in four files and
    /// deserialises `binary_path` **and** `download_host` under it,
    /// asserting neither comes back dead. A section with only `binary_path`
    /// fails that test the moment `error.rs` names it.
    #[serde(default)]
    pub download_host: Option<String>,
}

impl ObscuraRuntimeConfig {
    /// The pinned binary, or `None` when unset **or blank**.
    ///
    /// A cleared Panel field posts `""`, and `Some("")` spent as a path
    /// resolves to the current directory and then fails naming nothing — the
    /// exact trap [`BrowserRuntimeConfig::pinned_binary`] documents.
    #[must_use]
    pub fn pinned_binary(&self) -> Option<&str> {
        self.binary_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// The download mirror, or `None` when unset or blank. See
    /// [`Self::pinned_binary`] for why blank is not a value.
    #[must_use]
    pub fn download_host(&self) -> Option<&str> {
        self.download_host
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// Configuration for the Chrome `DevTools` MCP integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChromeMcpConfig {
    /// Command to launch Chrome `DevTools` MCP server.
    #[serde(default = "default_chrome_mcp_command")]
    pub command: String,

    /// Arguments for the MCP command.
    #[serde(default = "default_chrome_mcp_args")]
    pub args: Vec<String>,
}

fn default_chrome_mcp_command() -> String {
    "npx".to_string()
}

/// The default `npx` invocation for the Chrome DevTools MCP server.
///
/// `--allow-unrestricted-paths` turns OFF a guard the server added in v1.6.0,
/// and that is deliberate. Absent negotiated MCP `roots`, the server confines
/// every `filePath` argument to the OS temp directory — a boundary nobody here
/// chose: it permits anything under `/tmp` while refusing the user's own
/// Downloads folder, so `browser_upload` failed for the files people actually
/// want to upload and succeeded for scratch files nobody does.
///
/// The gate that stays is Aleph's own: `file_ops`' protected-location denylist
/// and allowed roots run over the upload path before the tool is ever called,
/// and that one is informed, configurable and tested. This is the same call
/// made for the managed driver when `outputDir` was found to have narrowed
/// playwright-cli's write roots: switch off the weaker second answer rather
/// than route around it.
///
/// Deliberately NOT solved by declaring the `roots` capability. That is the
/// protocol-correct answer, and it belongs in `src/mcp/` where it would apply
/// to every server Aleph connects to — a much larger blast radius than a
/// browser-profile default, and one that needs its own round.
///
/// Servers older than 1.6.0 ignore the unknown switch (yargs is not strict
/// here — verified against 1.5.0), so the default stays safe for a pinned
/// older version.
fn default_chrome_mcp_args() -> Vec<String> {
    vec![
        "-y".to_string(),
        "chrome-devtools-mcp@latest".to_string(),
        "--autoConnect".to_string(),
        "--experimentalStructuredContent".to_string(),
        "--allow-unrestricted-paths".to_string(),
    ]
}

impl Default for ChromeMcpConfig {
    fn default() -> Self {
        Self {
            command: default_chrome_mcp_command(),
            args: default_chrome_mcp_args(),
        }
    }
}

/// Ceiling on `cdp_command_timeout_secs`, exclusive.
///
/// obscura cuts any CDP command off at its own
/// `OBSCURA_CDP_COMMAND_TIMEOUT_MS` (60 s). An Aleph budget at or above that
/// can never fire: obscura has already answered, so our timer measures nothing
/// and the variant that would have named the stall
/// (`BrowserError::EngineBusy`) becomes unreachable — a 恒假 arm (判据 §2).
pub const CDP_COMMAND_TIMEOUT_CEILING_SECS: u64 = 60;

const fn default_cdp_command_timeout_secs() -> u64 {
    30
}

/// Top-level browser system configuration.
///
/// **`Default` is deliberately NOT derived.** A derived `Default` would give
/// `cdp_command_timeout_secs == 0` — every CDP call timing out before it was
/// sent — for the 20+ tests that build a manager from
/// `BrowserSystemConfig::default()`, while a real config file missing the key
/// gets 30 from serde. See the hand-written impl below.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSystemConfig {
    /// Named browser profiles.
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    /// SSRF protection policy.
    #[serde(default)]
    pub policy: SsrfConfig,

    /// Reads both `[playwright_cli]` and legacy `[playwright_mcp]` (unknown fields dropped).
    #[serde(default, alias = "playwright_mcp")]
    pub playwright_cli: PlaywrightCliConfig,

    /// Chrome `DevTools` MCP integration settings.
    #[serde(default)]
    pub chrome_mcp: ChromeMcpConfig,

    /// External-runtime supply for the managed driver's Chromium.
    #[serde(default)]
    pub runtime: BrowserRuntimeConfig,

    /// External-runtime supply for obscura.
    #[serde(default)]
    pub obscura: ObscuraRuntimeConfig,

    /// The engine a profile runs when it names none.
    #[serde(default)]
    pub default_engine: Engine,

    /// Per-command budget for Aleph's CDP client, both engines.
    ///
    /// Must be below [`CDP_COMMAND_TIMEOUT_CEILING_SECS`]; enforced by
    /// [`Self::validate`], not by clamping — a value silently clamped is a
    /// setting that "sometimes works".
    ///
    /// The field-level serde default is kept ALONGSIDE the hand-written
    /// `Default` below: they are two different mechanisms serving two
    /// different callers (a config file missing the key; `Self::default()`),
    /// and dropping either one puts a 0 back on that path.
    #[serde(default = "default_cdp_command_timeout_secs")]
    pub cdp_command_timeout_secs: u64,
}

impl Default for BrowserSystemConfig {
    /// Hand-written, matching every field's serde default exactly.
    ///
    /// `#[derive(Default)]` would give `cdp_command_timeout_secs == 0` while a
    /// config file with no such key gives 30 — one fact with two answers,
    /// where the wrong one is what every unit test sees and no operator ever
    /// runs. `browser_system_config_default_timeout_is_30_not_0` pins both
    /// paths; `the_struct_default_and_the_empty_config_agree_on_every_new_key`
    /// pins the other two keys.
    fn default() -> Self {
        Self {
            profiles: HashMap::new(),
            policy: SsrfConfig::default(),
            playwright_cli: PlaywrightCliConfig::default(),
            chrome_mcp: ChromeMcpConfig::default(),
            runtime: BrowserRuntimeConfig::default(),
            obscura: ObscuraRuntimeConfig::default(),
            default_engine: Engine::default(),
            cdp_command_timeout_secs: default_cdp_command_timeout_secs(),
        }
    }
}

impl BrowserSystemConfig {
    /// The per-command CDP budget as a `Duration`.
    #[must_use]
    pub fn cdp_command_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.cdp_command_timeout_secs)
    }

    /// Every profile setting its engine cannot honour, as
    /// `(profile, key, explanation)` — for the doctor to REPORT, never to
    /// refuse on.
    ///
    /// Spec §6.3: under obscura, `browser = <Chromium family>` 「被忽略并在
    /// doctor 记一条」. `browser` steers
    /// `discovery::find_chromium_preferred`, which obscura never reaches, so
    /// the field is inert there. Inert is not harmless: the operator set it
    /// and believes it did something, which is the silent-no-op shape
    /// (判据 §11). Refusing instead would break a profile that merely carries
    /// a leftover key, so this reports and [`Self::validate`] stays quiet
    /// about it.
    ///
    /// A NON-default value is the evidence, not the value itself: `browser`
    /// has a `Default`, so every profile has one whether or not anybody chose
    /// it, and reporting the default would name a setting nobody made.
    ///
    /// Returned rather than logged so the doctor check that renders it
    /// (Task 16) and this function are not two authors of one list.
    #[must_use]
    pub fn ignored_fields(&self) -> Vec<(String, &'static str, String)> {
        let mut names: Vec<&String> = self.profiles.keys().collect();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            let cfg = &self.profiles[name];
            if cfg.resolved_engine(self.default_engine) == Engine::Obscura
                && cfg.browser != BrowserType::default()
            {
                out.push((
                    (*name).clone(),
                    "browser",
                    format!(
                        "browser = {:?} selects a Chromium-family binary and is ignored while \
                         this profile runs obscura; set engine = \"chromium\" if you meant to \
                         pin that browser",
                        cfg.browser
                    ),
                ));
            }
        }
        out
    }

    /// Load-time validation for the browser section.
    ///
    /// Refuses only combinations that CANNOT start, and every message names
    /// the edit that fixes it — a fail-closed answer that does not say how to
    /// open the gate is fail-dead (判据 §14).
    ///
    /// Deliberately says nothing about a `browser` key that obscura ignores:
    /// that is a report, not a refusal — see [`Self::ignored_fields`].
    pub fn validate(&self) -> Result<(), String> {
        if self.cdp_command_timeout_secs == 0
            || self.cdp_command_timeout_secs >= CDP_COMMAND_TIMEOUT_CEILING_SECS
        {
            return Err(format!(
                "[general.browser] cdp_command_timeout_secs = {} is out of range: it must be \
                 between 1 and {} seconds. obscura cuts every CDP command off at its own \
                 OBSCURA_CDP_COMMAND_TIMEOUT_MS ({} s), so a larger Aleph budget can never \
                 fire — the engine has already answered, and the stall arrives as a protocol \
                 error instead of a timeout. Set it to 30 (the default) unless you have \
                 measured a reason not to.",
                self.cdp_command_timeout_secs,
                CDP_COMMAND_TIMEOUT_CEILING_SECS - 1,
                CDP_COMMAND_TIMEOUT_CEILING_SECS
            ));
        }
        let mut names: Vec<&String> = self.profiles.keys().collect();
        // Sorted: a `HashMap` iteration order would make the same bad config
        // report a different profile on each load.
        names.sort();
        for name in names {
            validate_engine_driver(name, &self.profiles[name])?;
        }
        Ok(())
    }
}

/// Refuse a profile whose engine and driver contradict each other.
///
/// obscura speaks CDP and nothing else: it is not a Chromium-family binary
/// `playwright-cli` can launch, and it is not the user's own Chrome for the
/// `DevTools` MCP server to attach to. Caught at load rather than at launch,
/// because the launch-time failure is a missing-binary message that sends the
/// operator looking for the wrong thing.
///
/// **Reads the EXPLICIT field, not the resolved engine, and that is the whole
/// design.** [`ProfileConfig::resolved_engine`] never answers obscura for a
/// legacy driver — it pins those to Chromium — so a check written against the
/// resolved engine would be 恒假 for exactly the drivers it was written to
/// police (判据 §2). The two functions divide the work: the resolver keeps
/// every legacy config loading, and this one refuses the one input the
/// resolver would otherwise have to silently discard — a profile that says
/// `engine = "obscura"` *and* a driver that cannot run it. Telling the
/// operator beats quietly running Chromium under a line that says obscura.
///
/// Inheriting obscura from `default_engine` is therefore **not** a
/// contradiction and is not refused: such a profile resolves to Chromium and
/// runs exactly as it did before the key existed.
pub fn validate_engine_driver(profile_name: &str, cfg: &ProfileConfig) -> Result<(), String> {
    if cfg.engine == Some(Engine::Obscura) && cfg.driver != BrowserDriver::Cdp {
        return Err(format!(
            "[general.browser.profiles.{profile_name}] sets engine = \"obscura\" with \
             driver = \"{}\". obscura is driven only over CDP — it is not a Chromium-family \
             binary playwright-cli can launch, and not a Chrome the DevTools MCP server can \
             attach to. Fix it either way: set driver = \"cdp\" to run obscura, or drop the \
             engine key to keep the current driver (a legacy driver already implies \
             chromium).",
            cfg.driver.as_wire()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_config_defaults() {
        let config = ProfileConfig::default();
        assert_eq!(config.browser, BrowserType::Chromium);
        assert_eq!(config.headless, None);
        assert!(config.proxy.is_none());
        assert!(config.user_data_dir.is_none());
        assert!(config.extra_args.is_empty());
        assert_eq!(config.idle_timeout_secs, 1800);
    }

    #[test]
    fn test_browser_system_config_toml_deserialization() {
        let toml_str = r##"
[profiles.work]
browser = "chrome"
headless = true
color = "#ff0000"
proxy = "socks5://127.0.0.1:1080"
extra_args = ["--disable-gpu"]
idle_timeout_secs = 3600

[profiles.personal]
browser = "brave"

[policy]
block_private = true
blocked_domains = ["*.malware.com"]

[playwright_mcp]
enabled = false
command = "node"
args = ["./mcp-server.js"]
"##;

        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();

        // Work profile
        let work = config.profiles.get("work").unwrap();
        assert_eq!(work.browser, BrowserType::Chrome);
        assert_eq!(work.headless, Some(true));
        assert_eq!(work.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(work.extra_args, vec!["--disable-gpu"]);
        assert_eq!(work.idle_timeout_secs, 3600);

        // Personal profile
        let personal = config.profiles.get("personal").unwrap();
        assert_eq!(personal.browser, BrowserType::Brave);
        assert_eq!(personal.headless, None); // default
        assert_eq!(personal.idle_timeout_secs, 1800); // default

        // Policy
        assert!(config.policy.block_private);
        assert_eq!(config.policy.blocked_domains, vec!["*.malware.com"]);

        // Playwright CLI (legacy [playwright_mcp] section still maps to
        // playwright_cli via the serde alias; unknown legacy keys are ignored,
        // surviving fields fall back to defaults).
        assert!(config.playwright_cli.headless);
        assert_eq!(config.playwright_cli.nav_timeout_secs, 30);
    }

    /// The three `[browser.runtime]` keys, and the one property that matters
    /// about all of them: an EMPTY string is not a value.
    ///
    /// `download_host = ""` is what the spec's own config sample shows, and
    /// what a Panel form posts when the operator clears the field. Handing that
    /// to the installer as `PLAYWRIGHT_DOWNLOAD_HOST=` is not "no mirror", it is
    /// "the mirror is the empty host" — every download then fails with a URL
    /// error that names nothing. Same for a `binary_path` cleared to "".
    #[test]
    fn browser_runtime_reads_its_three_keys_and_treats_empty_as_unset() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
[runtime]
binary_path = "/opt/chromium/chrome"
prefer_system_browser = false
download_host = "https://npmmirror.com/mirrors/playwright"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.runtime.pinned_binary(), Some("/opt/chromium/chrome"));
        assert!(!cfg.runtime.prefer_system_browser);
        assert_eq!(
            cfg.runtime.download_host(),
            Some("https://npmmirror.com/mirrors/playwright")
        );

        let cleared: BrowserSystemConfig = toml::from_str(
            r#"
[runtime]
binary_path = ""
download_host = "   "
"#,
        )
        .expect("parse");
        assert_eq!(cleared.runtime.pinned_binary(), None, "empty pin is unset");
        assert_eq!(cleared.runtime.download_host(), None, "blank host is unset");
        // The `[runtime]` table is PRESENT here and the key is absent, so this
        // exercises serde's field-level `default = "default_true"` — which is a
        // different mechanism from `Default::default()` and the one that would
        // silently flip to `false` if the attribute were dropped.
        assert!(
            cleared.runtime.prefer_system_browser,
            "a system browser is preferred unless the operator says otherwise: \
             Windows almost always has Edge and macOS usually has Chrome, so the \
             download is for clean Linux servers"
        );
    }

    /// A config with no `[runtime]` table at all must still produce the
    /// defaults — this section is new, and every config file on every existing
    /// install predates it.
    #[test]
    fn a_config_without_the_runtime_table_still_gets_the_defaults() {
        let cfg: BrowserSystemConfig =
            toml::from_str("[policy]\nblock_private = true\n").expect("parse");
        assert!(cfg.runtime.prefer_system_browser);
        assert_eq!(cfg.runtime.pinned_binary(), None);
        assert_eq!(cfg.runtime.download_host(), None);
    }

    #[test]
    fn test_browser_type_serde_roundtrip() {
        let types = vec![
            BrowserType::Chromium,
            BrowserType::Chrome,
            BrowserType::Brave,
            BrowserType::Edge,
        ];

        for bt in types {
            let json = serde_json::to_string(&bt).unwrap();
            let deserialized: BrowserType = serde_json::from_str(&json).unwrap();
            assert_eq!(bt, deserialized);
        }

        // Verify lowercase serialization
        assert_eq!(
            serde_json::to_string(&BrowserType::Chromium).unwrap(),
            "\"chromium\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserType::Chrome).unwrap(),
            "\"chrome\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserType::Brave).unwrap(),
            "\"brave\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserType::Edge).unwrap(),
            "\"edge\""
        );
    }

    #[test]
    fn test_browser_system_config_defaults() {
        let config = BrowserSystemConfig::default();
        assert!(config.profiles.is_empty());
        assert!(config.policy.block_private);
        assert!(config.playwright_cli.headless);
    }

    /// Renamed rather than re-pointed: a test called `…_is_managed` that
    /// asserts `Cdp` is a name contradicting its own body, and the next reader
    /// believes the name.
    #[test]
    fn test_browser_driver_default_is_cdp() {
        let driver = BrowserDriver::default();
        assert_eq!(driver, BrowserDriver::Cdp);
    }

    /// The wire spellings are frozen config compatibility. Driven from
    /// `BrowserDriver::ALL` rather than a hand-written vector: the previous
    /// version listed two of three drivers, so `Cdp`'s serde name shipped
    /// unexercised — and a list beside an enum is the list that rots (判据 §5).
    #[test]
    fn test_browser_driver_serde_roundtrip() {
        for d in BrowserDriver::ALL {
            let json = serde_json::to_string(&d).unwrap();
            let deserialized: BrowserDriver = serde_json::from_str(&json).unwrap();
            assert_eq!(d, deserialized);
        }
        assert_eq!(
            serde_json::to_string(&BrowserDriver::Managed).unwrap(),
            "\"managed\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserDriver::ExistingSession).unwrap(),
            "\"existing_session\""
        );
        assert_eq!(
            serde_json::to_string(&BrowserDriver::Cdp).unwrap(),
            "\"cdp\""
        );
        // serde's spelling and `as_wire`'s must be the same string: the Panel
        // reads and writes through `as_wire`/`from_wire` while the config file
        // goes through serde, so two spellings would be one profile that two
        // front doors disagree about.
        for d in BrowserDriver::ALL {
            assert_eq!(
                serde_json::to_string(&d).unwrap(),
                format!("\"{}\"", d.as_wire()),
                "{d:?}"
            );
        }
    }

    /// **The flip, from the config side.** A legacy config that names a driver
    /// keeps it: the default changes what happens when nothing was said, never
    /// what an operator wrote down.
    #[test]
    fn an_explicit_legacy_driver_survives_the_default_flip() {
        let cfg: BrowserSystemConfig = toml::from_str("[profiles.legacy]\ndriver = \"managed\"\n")
            .expect("legacy config must parse");
        let p = &cfg.profiles["legacy"];
        assert_eq!(p.driver, BrowserDriver::Managed);
        // A legacy driver pins the engine, so the global default_engine flip
        // must not drag this profile onto obscura.
        assert_eq!(p.resolved_engine(Engine::Obscura), Engine::Chromium);

        // …and a profile that says nothing follows the new default.
        let silent: BrowserSystemConfig =
            toml::from_str("[profiles.fresh]\n").expect("empty profile must parse");
        let f = &silent.profiles["fresh"];
        assert_eq!(f.driver, BrowserDriver::Cdp);
        assert_eq!(f.resolved_engine(Engine::Obscura), Engine::Obscura);
    }

    #[test]
    fn the_global_default_engine_is_obscura() {
        assert_eq!(
            BrowserSystemConfig::default().default_engine,
            Engine::Obscura
        );
    }

    #[test]
    fn test_profile_config_driver_defaults_to_cdp() {
        let config = ProfileConfig::default();
        assert_eq!(config.driver, BrowserDriver::Cdp);
        // And `engine: None` means "follow `default_engine`", which is what
        // makes obscura the engine a fresh install actually runs.
        assert_eq!(config.engine, None);
        assert_eq!(
            config.resolved_engine(BrowserSystemConfig::default().default_engine),
            Engine::Obscura
        );
    }

    #[test]
    fn test_old_playwright_mcp_toml_deserializes_to_playwright_cli() {
        let toml_str = r##"
[playwright_mcp]
enabled = true
command = "npx"
args = ["@playwright/mcp@latest", "--headless"]
"##;
        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
        assert!(config.playwright_cli.headless);
        assert_eq!(config.playwright_cli.nav_timeout_secs, 30);
    }

    #[test]
    fn test_playwright_cli_defaults() {
        let config = PlaywrightCliConfig::default();
        assert!(config.binary_path.is_none());
        assert!(config.headless);
        assert_eq!(config.nav_timeout_secs, 30);
        assert_eq!(config.action_timeout_secs, 10);
    }

    #[test]
    fn test_profile_config_headless_option_compat() {
        let toml_str = r##"
[profiles.default]
browser = "chromium"
headless = true
"##;
        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
        let p = config.profiles.get("default").unwrap();
        assert_eq!(p.headless, Some(true));
    }

    #[test]
    fn test_chrome_mcp_config_defaults() {
        let config = ChromeMcpConfig::default();
        assert_eq!(config.command, "npx");
        assert!(config
            .args
            .contains(&"chrome-devtools-mcp@latest".to_string()));
        assert!(config.args.contains(&"--autoConnect".to_string()));
    }

    #[test]
    fn test_browser_system_config_with_chrome_mcp() {
        let toml_str = r##"
[profiles.user]
browser = "chrome"
driver = "existing_session"
color = "#00AA00"

[chrome_mcp]
command = "npx"
args = ["-y", "chrome-devtools-mcp@latest", "--autoConnect"]
"##;

        let config: BrowserSystemConfig = toml::from_str(toml_str).unwrap();
        let user = config.profiles.get("user").unwrap();
        assert_eq!(user.browser, BrowserType::Chrome);
        assert_eq!(user.driver, BrowserDriver::ExistingSession);
        assert_eq!(config.chrome_mcp.command, "npx");
    }

    use crate::browser::engine::Engine;

    /// `driver` and `engine` are two axes, not one (spec §5.4). The legacy
    /// values keep their exact wire spelling — a config file that says
    /// `driver = "managed"` must land on the same variant it always has,
    /// because those files exist on every install.
    #[test]
    fn the_driver_axis_gains_cdp_without_moving_the_two_legacy_spellings() {
        for (wire, expected) in [
            ("managed", BrowserDriver::Managed),
            ("existing_session", BrowserDriver::ExistingSession),
            ("cdp", BrowserDriver::Cdp),
        ] {
            let cfg: BrowserSystemConfig =
                toml::from_str(&format!("[profiles.p]\ndriver = \"{wire}\"\n")).expect("parse");
            assert_eq!(cfg.profiles["p"].driver, expected, "{wire}");
            // Serialisation is the same string — the two directions of one
            // fact, checked against each other rather than against a second
            // literal list.
            assert_eq!(
                serde_json::to_string(&expected).expect("serialize"),
                format!("\"{wire}\"")
            );
            assert_eq!(expected.as_wire(), wire);
            assert_eq!(BrowserDriver::from_wire(wire), Some(expected));
        }
        assert_eq!(BrowserDriver::from_wire("managed_cli"), None);
        assert_eq!(BrowserDriver::from_wire(""), None);
    }

    /// Under the `cdp` driver — the only one that can run either engine — a
    /// profile with no `engine` follows the global default and one that names
    /// an engine overrides it. The legacy drivers do not reach the default at
    /// all; that half of the rule is
    /// `legacy_managed_profile_resolves_to_chromium_under_obscura_default`.
    /// Both halves are resolved in ONE function so no caller invents a third
    /// answer.
    #[test]
    fn a_profile_without_an_engine_follows_the_global_default() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
default_engine = "chromium"

[profiles.follows]
driver = "cdp"

[profiles.overrides]
driver = "cdp"
engine = "obscura"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.default_engine, Engine::Chromium);
        assert_eq!(cfg.profiles["follows"].engine, None);
        assert_eq!(
            cfg.profiles["follows"].resolved_engine(cfg.default_engine),
            Engine::Chromium
        );
        assert_eq!(
            cfg.profiles["overrides"].resolved_engine(cfg.default_engine),
            Engine::Obscura
        );
    }

    /// Missing keys are product defaults; a corrupt value is an error. Never
    /// the other way round — a config that silently substituted a default for
    /// an engine name the operator typed would run a browser they did not ask
    /// for and report nothing.
    #[test]
    fn a_missing_engine_key_defaults_and_a_misspelled_one_refuses() {
        let empty: BrowserSystemConfig = toml::from_str("").expect("parse");
        assert_eq!(empty.default_engine, Engine::Obscura);
        assert_eq!(empty.cdp_command_timeout_secs, 30);
        assert_eq!(empty.obscura.variant, ObscuraVariant::Default);
        assert_eq!(empty.obscura.pinned_binary(), None);

        for bad in ["chrome", "Obscura", "webkit", ""] {
            let parsed =
                toml::from_str::<BrowserSystemConfig>(&format!("default_engine = \"{bad}\"\n"));
            assert!(parsed.is_err(), "accepted default_engine = {bad:?}");
        }
        assert!(
            toml::from_str::<BrowserSystemConfig>("[profiles.p]\nengine = \"chrome\"\n").is_err(),
            "a misspelled per-profile engine must refuse, not fall back"
        );
    }

    /// **Both** paths to a default timeout must land on 30, and neither may
    /// land on 0.
    ///
    /// There are exactly two, and they are different mechanisms: `Default`
    /// (which `BrowserSystemConfig::default()` uses, and which 20+ tests in
    /// `manager.rs` build their manager from) and serde's field-level
    /// `default = "default_cdp_command_timeout_secs"` (which a real config
    /// file missing the key uses). `#[derive(Default)]` satisfies the second
    /// and answers **0** for the first — a budget under which every CDP call
    /// times out before it is sent, in exactly the builds a developer runs
    /// and never in the one an operator does.
    ///
    /// The `assert_ne!` against 0 is not redundant with the `assert_eq!`: if
    /// the constant is ever retuned, the equality assertions follow it and
    /// this one still refuses the value that means "no budget at all".
    ///
    /// **What actually falsifies path 2, and what does not (K2, re-measured
    /// against this exact tree with K1/K3 already fixed — a first pass taken
    /// mid-round conflated this mutation's effect with an unrelated,
    /// then-still-open bug in `gateway::handlers::browser_config::handle_update`,
    /// which inflated the count by one; the numbers below are the corrected
    /// ones, stated as absolutes rather than as deltas across three separate
    /// commit messages that never agreed — those said 3, then 4, then 5
    /// "other files"; 5 is correct, verified below by full path):**
    ///
    /// - Deleting `#[serde(default = "default_cdp_command_timeout_secs")]`
    ///   from the field turns it from optional to mandatory for TOML
    ///   deserialization. It compiles cleanly — `cargo test --lib --no-run`
    ///   after that deletion produces zero errors — so it is **not** a
    ///   compile failure. It IS a genuine runtime red for path 2 here
    ///   (`toml::from_str` panics with `missing field
    ///   cdp_command_timeout_secs` inside the `.expect("parse")` a few lines
    ///   below — path 2 never gets as far as comparing against 30). Running
    ///   the full `-p alephcore --lib` suite with the attribute deleted: **20
    ///   failures
    ///   observed**, of which one —
    ///   `utils::host::tests::no_other_module_hand_rolls_the_hostname_env_read`
    ///   — is a pre-existing, always-failing test with zero relation to this
    ///   change (confirmed separately). **19 failures are attributable to
    ///   this mutation**: this test itself, plus 18 others split across
    ///   `browser::profile::tests` (13 of the 18) and five other files, one
    ///   failure each — `config::dead_keys`, `config::load`
    ///   (`dead_key_tests`), `config::tests::serialization`,
    ///   `config::types::general`, and `diagnostics::checks::config_parse`.
    ///   That is a real falsifier, but a blunt, crate-wide one: it does not
    ///   tell you THIS test caught it, only that something did.
    /// - The mutation that falsifies path 2 **on its own**, leaving path 1
    ///   untouched, is repointing the attribute at a second function
    ///   returning a different value (verified: a `mutation_k2_wrong_cdp_timeout()
    ///   -> u64 { 99 }` swapped into the `#[serde(default = "...")]` string).
    ///   `impl Default` still calls the original, unedited
    ///   `default_cdp_command_timeout_secs()`, so `built` stays 30 and only
    ///   `parsed` becomes 99. **The assertion that actually reds is
    ///   `assert_eq!(parsed.cdp_command_timeout_secs, 30)`** (measured:
    ///   `left: 99, right: 30`) — the SAME line path 1's own value never
    ///   touches, four lines before the final cross-check. The final
    ///   `assert_eq!(built.cdp_command_timeout_secs,
    ///   parsed.cdp_command_timeout_secs)` is not reachable under this
    ///   mutation (the panic above ends the test first), and it could not
    ///   have caught it regardless: by the time execution would reach it,
    ///   both `built == 30` and `parsed == 30` are already independently
    ///   proven, so `built == parsed` is entailed and cannot fail — a 判据
    ///   §2 arm. It stands as a statement of intent ("these two must agree"),
    ///   not as a load-bearing assertion; the line above it is the one doing
    ///   the falsifying. This is the mutation to reach for when the question
    ///   is "does path 2 specifically still work", not "does something in
    ///   this file still work".
    #[test]
    fn browser_system_config_default_timeout_is_30_not_0() {
        // Path 1: the Rust default.
        let built = BrowserSystemConfig::default();
        assert_ne!(
            built.cdp_command_timeout_secs, 0,
            "a derived Default puts a 0s CDP budget under every test that \
             builds a manager from BrowserSystemConfig::default()"
        );
        assert_eq!(built.cdp_command_timeout_secs, 30);
        assert_eq!(
            built.cdp_command_timeout(),
            std::time::Duration::from_secs(30)
        );

        // Path 2: a config file with no such key. The `[policy]` table is
        // present so this is a real parse of a real (if minimal) file, not an
        // empty-string special case.
        let parsed: BrowserSystemConfig =
            toml::from_str("[policy]\nblock_private = true\n").expect("parse");
        assert_ne!(parsed.cdp_command_timeout_secs, 0);
        assert_eq!(parsed.cdp_command_timeout_secs, 30);

        // And the two paths agree with each other, which is the property that
        // breaks the moment one of the two mechanisms is dropped.
        assert_eq!(
            built.cdp_command_timeout_secs,
            parsed.cdp_command_timeout_secs
        );
    }

    /// The same "two derivations of the product defaults must agree" property
    /// for the other two new keys. Kept apart from the timeout test above
    /// because that one is about a specific value that must never be 0; this
    /// one is about the mechanism, and it will need a line for every key added
    /// later.
    #[test]
    fn the_struct_default_and_the_empty_config_agree_on_every_new_key() {
        let parsed: BrowserSystemConfig = toml::from_str("").expect("parse");
        let built = BrowserSystemConfig::default();
        assert_eq!(built.default_engine, parsed.default_engine);
        assert_eq!(built.default_engine, Engine::Obscura);
        assert_eq!(built.obscura.variant, parsed.obscura.variant);
        assert_eq!(built.obscura.variant, ObscuraVariant::Default);
        assert_eq!(built.obscura.binary_path, parsed.obscura.binary_path);
        assert_eq!(built.obscura.pinned_binary(), None);
        assert_eq!(built.obscura.download_host, parsed.obscura.download_host);
        assert_eq!(built.obscura.download_host(), None);
    }

    /// obscura only answers CDP. A profile that asks for it through the
    /// playwright-cli or the Chrome-MCP driver is not a degraded setup, it is
    /// one that cannot start — so it is refused at load with the edit that
    /// fixes it, rather than failing later inside a launch with a message
    /// about a missing binary.
    #[test]
    fn obscura_with_a_non_cdp_driver_is_refused_and_the_text_names_the_fix() {
        for driver in [BrowserDriver::Managed, BrowserDriver::ExistingSession] {
            let cfg = ProfileConfig {
                engine: Some(Engine::Obscura),
                driver,
                ..Default::default()
            };
            let Err(err) = validate_engine_driver("work", &cfg) else {
                panic!("obscura x {driver:?} must refuse");
            };
            assert!(err.contains("work"), "must name the profile: {err}");
            assert!(err.contains("obscura"), "must name the engine: {err}");
            assert!(
                err.contains(driver.as_wire()),
                "must quote the driver as the config file spells it: {err}"
            );
            assert!(
                err.contains("driver = \"cdp\""),
                "must name the edit that fixes it, not just the problem: {err}"
            );
        }

        // Every other pairing is legal — including a legacy driver with no
        // engine key, which is what every config file on every install looks
        // like and which must keep loading.
        for (engine, driver) in [
            (Some(Engine::Obscura), BrowserDriver::Cdp),
            (Some(Engine::Chromium), BrowserDriver::Cdp),
            (Some(Engine::Chromium), BrowserDriver::Managed),
            (Some(Engine::Chromium), BrowserDriver::ExistingSession),
            (None, BrowserDriver::Cdp),
            (None, BrowserDriver::Managed),
            (None, BrowserDriver::ExistingSession),
        ] {
            let ok = ProfileConfig {
                engine,
                driver,
                ..Default::default()
            };
            assert!(
                validate_engine_driver("p", &ok).is_ok(),
                "{engine:?} x {driver:?} was refused"
            );
        }
    }

    /// The rule that keeps every existing install loading: a legacy driver
    /// pins the engine, and a new global default does not reach it.
    ///
    /// This is the assertion whose absence would have shipped a config layer
    /// that refuses any file with `driver = "managed"` the moment
    /// `default_engine` defaults to obscura — i.e. every install whose
    /// operator has ever pressed save on the Panel's browser settings page,
    /// because `handle_update` writes `driver` into both the `default` and
    /// `user` profiles on every save. `Config::validate()` is called with `?`
    /// on the load path, so those daemons would not start.
    #[test]
    fn legacy_managed_profile_resolves_to_chromium_under_obscura_default() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
default_engine = "obscura"

[profiles.default]
driver = "managed"

[profiles.user]
driver = "existing_session"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.default_engine, Engine::Obscura);
        for name in ["default", "user"] {
            assert_eq!(
                cfg.profiles[name].resolved_engine(cfg.default_engine),
                Engine::Chromium,
                "{name} was dragged onto the new global default"
            );
        }
        assert!(
            cfg.validate().is_ok(),
            "a config that predates the engine key must still load"
        );

        // And the contrast that shows the default is not simply ignored: the
        // same global default DOES reach a cdp profile with no engine key.
        let cdp: BrowserSystemConfig =
            toml::from_str("default_engine = \"obscura\"\n[profiles.p]\ndriver = \"cdp\"\n")
                .expect("parse");
        assert_eq!(
            cdp.profiles["p"].resolved_engine(cdp.default_engine),
            Engine::Obscura
        );
    }

    /// The timeout must sit strictly below obscura's own guillotine, and the
    /// error has to name it — otherwise an operator reads "must be under 60"
    /// as an arbitrary Aleph rule and raises it in the next release.
    #[test]
    fn a_cdp_timeout_at_or_above_the_guillotine_is_refused_by_name() {
        for bad in [60, 61, 600] {
            let cfg = BrowserSystemConfig {
                cdp_command_timeout_secs: bad,
                ..Default::default()
            };
            let Err(err) = cfg.validate() else {
                panic!("{bad} must refuse");
            };
            assert!(
                err.contains("cdp_command_timeout_secs"),
                "must name the key: {err}"
            );
            assert!(
                err.contains("OBSCURA_CDP_COMMAND_TIMEOUT_MS"),
                "must name the guillotine it is bounded by, or the number \
                 reads as an arbitrary Aleph rule: {err}"
            );
            assert!(err.contains("60"), "must name the ceiling: {err}");
        }
        // Zero is the other end: every command would time out before it was
        // sent, and the model would read that as the page being unreachable.
        let zero = BrowserSystemConfig {
            cdp_command_timeout_secs: 0,
            ..Default::default()
        };
        assert!(zero.validate().is_err(), "0 must refuse");

        for good in [1, 30, 59] {
            let cfg = BrowserSystemConfig {
                cdp_command_timeout_secs: good,
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "{good} must be accepted");
        }
    }

    /// Spec §6.3: a `browser` key under obscura is ignored and REPORTED, not
    /// refused. Both halves matter — refusing would break a profile carrying a
    /// leftover key, and staying silent would leave the operator believing a
    /// setting took effect (判据 §11).
    #[test]
    fn a_browser_key_under_obscura_is_reported_and_never_refused() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
default_engine = "obscura"

[profiles.pinned]
driver = "cdp"
browser = "brave"

[profiles.silent]
driver = "cdp"

[profiles.legacy]
driver = "managed"
browser = "brave"
"#,
        )
        .expect("parse");

        // Reported, and the config still loads.
        assert!(cfg.validate().is_ok(), "an ignored key must not refuse");
        let ignored = cfg.ignored_fields();
        assert_eq!(ignored.len(), 1, "{ignored:?}");
        assert_eq!(ignored[0].0, "pinned");
        assert_eq!(ignored[0].1, "browser");
        assert!(
            ignored[0].2.contains("engine = \"chromium\""),
            "must name the edit that makes the key take effect: {}",
            ignored[0].2
        );

        // `silent` carries the DEFAULT browser value, which nobody chose —
        // reporting it would name a setting the operator never made.
        // `legacy` runs Chromium (a legacy driver pins the engine), so its
        // `browser` key is honoured and there is nothing to report.
        assert!(!ignored.iter().any(|(p, _, _)| p == "silent"));
        assert!(!ignored.iter().any(|(p, _, _)| p == "legacy"));
    }

    /// The obscura pin is the twin of `[general.browser.runtime] binary_path`
    /// and has the same trap: a Panel form that clears the field posts `""`,
    /// and `Some("")` spent as a path resolves to the current directory and
    /// fails naming nothing.
    #[test]
    fn an_empty_obscura_pin_is_unset_not_the_current_directory() {
        let cfg: BrowserSystemConfig = toml::from_str(
            r#"
[obscura]
binary_path = "   "
download_host = ""
variant = "stealth"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.obscura.pinned_binary(), None, "blank pin is unset");
        assert_eq!(cfg.obscura.download_host(), None, "blank host is unset");
        assert_eq!(cfg.obscura.variant, ObscuraVariant::Stealth);

        let pinned: BrowserSystemConfig =
            toml::from_str("[obscura]\nbinary_path = \"/opt/obscura/obscura\"\n").expect("parse");
        assert_eq!(
            pinned.obscura.pinned_binary(),
            // R49 supersedes the brief's `Option<&Path>` shape: `ObscuraRuntimeConfig`
            // mirrors `BrowserRuntimeConfig` exactly, including the `Option<&str>`
            // return type — a `&Path` cannot carry the trim/empty-filter semantics
            // its sibling already has.
            Some("/opt/obscura/obscura")
        );
        // The table is PRESENT and `variant` is absent, so this exercises the
        // field-level serde default rather than `Default::default()`.
        assert_eq!(pinned.obscura.variant, ObscuraVariant::Default);
    }
}
