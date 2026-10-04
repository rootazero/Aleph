//! Path validation and resolution utilities

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use tracing::{info, warn};

use crate::builtin_tools::error::ToolError;

/// Denied paths for security.
///
/// Adding entries here is backwards-compatible (strictly tighter) — these are
/// well-known credential stores an agent should never read or overwrite.
/// Matched by [`check_and_resolve_path`] via symlink-canonicalizing prefix
/// comparison, so a directory entry (e.g. `~/.ssh`) covers everything beneath
/// it and a leaf file (e.g. `~/.netrc`) covers exactly that file.
///
/// The credential breadth here mirrors `OpenSquilla`'s `sensitive_paths.py`
/// (SSH/cloud/registry/secret stores) layered onto Aleph's stronger checker,
/// and — like hermes-agent's `get_read_block_error` — extends the deny set to
/// Aleph's *own* credential surface (the encrypted `secrets.vault` and the
/// `data/` auth/device-pairing databases), which an agent must never read or
/// clobber through its file tools.
///
/// The returned list carries TWO kinds of entry, each [`DeniedPath`] carrying
/// its own and compiled by [`denied_entry_normalized`]:
/// 1. the fixed credential locations above, matched by canonicalizing prefix —
///    every entry this function builds is one, whatever characters its path
///    contains;
/// 2. the operator's `[sandbox] deny_read_globs`
///    ([`configured_deny_read_globs`]), matched by the same anchored regex the
///    OS sandbox floor uses — the only entries that can be patterns.
///
/// (2) exists because a file has **two faces that can read it** and the
/// setting used to bind only one: `deny_read_globs` reached the OS drivers
/// (macOS seatbelt `(deny file-read* …)` / Windows deny-read ACEs) and stopped
/// there, so `deny_read_globs = ["**/.env"]` kernel-blocked `bash` while
/// `file_read`, `file_ops search` and `file_ops stats` read the same file in
/// plain text — with nothing anywhere telling the operator the floor was
/// half-applied.
pub fn get_denied_paths() -> Vec<DeniedPath> {
    let mut denied_paths: Vec<DeniedPath> = [
        // SSH / PGP / AWS — the original Unix credential directories.
        "~/.ssh",
        "~/.gnupg",
        "~/.aws",
        // Cloud-provider credential stores.
        "~/.config/gcloud",
        "~/.kube",
        "~/.azure",
        // Container-registry + package-registry credentials.
        "~/.docker/config.json",
        "~/.npmrc",
        "~/.pypirc",
        // Generic secret stores and credential leaf files.
        "~/.password-store",
        "~/.netrc",
        "~/.git-credentials",
    ]
    .into_iter()
    .map(DeniedPath::template)
    .collect();

    // Add specific Aleph config files (not the entire directory)
    // We allow the output directory but deny sensitive config files
    if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
        info!(config_dir = %config_dir.display(), "FileOpsTool: config_dir for denied_paths");
        let under_config =
            |leaf: &str| DeniedPath::literal(format!("{}/{leaf}", config_dir.display()));
        // Deny config files but NOT the output directory
        denied_paths.push(under_config("config.toml"));
        denied_paths.push(under_config("memory.db"));
        denied_paths.push(under_config("conversations.db"));
        denied_paths.push(under_config("skills"));
        denied_paths.push(under_config("plugins"));
        denied_paths.push(under_config("mcp"));
        // Aleph's own credential / auth state — the crown jewels. `secrets.vault`
        // is the encrypted credential store (`VaultStore::default_path()` =
        // `<config_dir>/secrets.vault`); `data/` holds the device-pairing,
        // session, security and devices databases plus the singleton
        // `aleph.lock`. Denying the directory covers every current and future
        // leaf beneath it via the canonicalizing prefix match. Without this the
        // agent's own `file_read`/`file_write` could exfiltrate or corrupt the
        // vault — a hole the OS `deny_globs` does not close because it only
        // applies to commands run inside the sandbox, not to the file tools.
        // The reverse leg of that asymmetry is closed at the bottom of this
        // function: the operator's `deny_read_globs` are appended here so they
        // bind the file tools too.
        denied_paths.push(under_config("secrets.vault"));
        denied_paths.push(under_config("secrets.vault.lock"));
        denied_paths.push(under_config("data"));
        // The state of the gates only a HUMAN opens. A model that could write
        // one could open its own gate — on an operator turn it can already
        // write a skill (`skill_manage`, a project's `.aleph/skills`), so one
        // more `file_write` of an `approved` consent entry would be a shell
        // outside the sandbox with nobody in the loop. Each path comes from
        // the store that owns it, never re-spelled here:
        // - the shell-command consent registry (hooks, and every inline
        //   `` !`cmd` `` — a plugin command's and a skill's);
        // - the exec-approval grants a human gave "always";
        // - the config-tier approval policy (its allow/block lists);
        // - plugin enable + owner-trust state (`plugins.toml` — only a human
        //   enables a Claude Code install). It sits under `data/` (above);
        //   named on its own so moving it out of `data/` cannot un-deny it.
        for gate in [
            crate::extension::hooks::ShellHookConsent::default_path(),
            crate::sandbox::exec_approval::grants::GrantStore::default_path(),
            crate::approval::ConfigApprovalPolicy::config_path(),
        ]
        .into_iter()
        .chain(
            aleph_protocol::paths::data_dir()
                .map(|data| data.join(crate::extension::plugin_state::PLUGINS_CONFIG_FILE)),
        ) {
            denied_paths.push(DeniedPath::literal(gate.display().to_string()));
        }
        // Note: output directory is intentionally NOT denied
    }

    // Every directory a typed `/<skill>` may pre-grant its `allowed-tools:`
    // from — `<config>/skills` and `<config>/plugins` above, plus
    // `~/.claude/skills` and `~/.claude/plugins`. A model that could write
    // one could add a name to a skill's list that registration validates at
    // the next boot. One list (`utils::paths::pregrant_roots`), shared with
    // the pre-grant's own origin check.
    for root in crate::utils::paths::pregrant_roots() {
        if !denied_paths.iter().any(|d| Path::new(d.as_str()) == root) {
            denied_paths.push(DeniedPath::literal(root.display().to_string()));
        }
    }

    // Add Unix-specific paths. Beyond the classic credential files, deny the
    // privilege-escalation / persistence surfaces an agent's file tools must
    // never read or clobber — writing any of these is a host-takeover vector
    // (sudoers, cron, PAM, the dynamic-linker preload hook), and reading the
    // SSH host-key dir or root's home leaks credentials. Mirrors hermes-agent's
    // `_SENSITIVE_PATH_PREFIXES`; each is a directory or leaf covered by the
    // canonicalizing prefix match below.
    #[cfg(unix)]
    {
        denied_paths.extend(
            [
                "/etc/passwd",
                "/etc/shadow",
                "/etc/sudoers",
                "/etc/sudoers.d",
                "/etc/ssh",
                "/etc/pam.d",
                "/etc/crontab",
                "/etc/cron.d",
                "/etc/ld.so.preload",
                "/root/.ssh",
            ]
            .map(DeniedPath::literal),
        );
    }

    // Add Windows-specific sensitive paths. The `%APPDATA%` / `%LOCALAPPDATA%`
    // entries are templates: each is expanded once, when it is compiled
    // (`denied_entry_key` → `expand_denied_entry`) — without that they never
    // fire (a canonical path never literally contains `%APPDATA%`).
    #[cfg(target_os = "windows")]
    {
        denied_paths.extend(
            [
                "%APPDATA%\\Microsoft\\Credentials",
                "%LOCALAPPDATA%\\Microsoft\\Credentials",
            ]
            .map(DeniedPath::template),
        );
        denied_paths.push(DeniedPath::literal("C:\\Windows\\System32\\config"));
    }

    // The operator's `[sandbox] deny_read_globs` floor. Appended last so a
    // reader of this function sees the fixed credential set first and the
    // configured patterns as an explicit extension of it.
    denied_paths.extend_from_slice(configured_deny_read_globs());

    denied_paths
}

/// One denylist entry, carrying its kind: what it is, and whether it is
/// expanded.
///
/// The kind is fixed where the entry is MADE, by who spelled it, and never
/// re-derived from how it reads — neither whether it is a pattern nor whether
/// it is expanded:
/// - **A location Aleph's code built** ([`DeniedPath::literal`]) is matched
///   exactly as built. `format!("{}/secrets.vault", config_dir.display())` is
///   already an expansion of `$ALEPH_HOME`, not a spelling anyone chose. Read
///   by shape, a home under `h[1]` turned the vault, `data/`, the human-gate
///   files and the pre-grant roots into patterns that matched nothing; read
///   as a template, `ALEPH_HOME=~/x` left unexpanded (systemd, launchd,
///   `docker -e`) had its `~` expanded to `$HOME/x`, so the entries protected
///   a file that is not Aleph's while Aleph's own state under `<cwd>/~/x`
///   went unprotected (P4.19).
/// - **A template this module spells** ([`DeniedPath::template`], private) —
///   `~/.ssh`, `%APPDATA%\…` — is expanded once, when it is compiled
///   ([`expand_denied_entry`]).
/// - **An operator's `[sandbox] deny_read_globs` entry**
///   ([`DeniedPath::operator_spelled`], private) is the only one that may be
///   a pattern; a metacharacter-free one is a template, as it always was.
///
/// Every face that consults the denylist gets its list from
/// [`get_denied_paths`] and an entry's meaning from [`denied_entry_normalized`],
/// so they all read the kind from here; the OS sandbox reads
/// `deny_read_globs` itself and never sees a code-built entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeniedPath {
    spelling: String,
    kind: DeniedKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeniedKind {
    /// Matched exactly as built; never expanded.
    Location,
    /// A `~/…` / `%VAR%` template, expanded when it is compiled.
    Template,
    /// A glob, compiled by the OS floor's translator; never expanded.
    Pattern,
}

impl DeniedPath {
    /// A location Aleph's code built from a path it resolved (the config dir,
    /// a store's `default_path()`, a pre-grant root, a system file). Matched
    /// exactly as built: never a pattern and never expanded, whatever
    /// characters it contains — a leading `~` or a `%APPDATA%` in it is part
    /// of a directory's name, because that is how the store that owns the
    /// path opens it.
    ///
    /// A relative path (a relative `ALEPH_HOME`) is made absolute against the
    /// working directory here, as the store opening it would, so the memo
    /// does not depend on what the working directory is when it compiles.
    pub fn literal(spelling: impl Into<String>) -> Self {
        let spelling = spelling.into();
        let spelling = if Path::new(&spelling).is_relative() {
            std::path::absolute(&spelling)
                .map(|absolute| absolute.to_string_lossy().into_owned())
                .unwrap_or(spelling)
        } else {
            spelling
        };
        Self {
            spelling,
            kind: DeniedKind::Location,
        }
    }

    /// A `~/…` / `%APPDATA%\…` template spelled in this module's source.
    /// `&'static str`, so a path resolved at runtime cannot be handed to the
    /// expanding kind.
    fn template(spelling: &'static str) -> Self {
        Self {
            spelling: spelling.to_string(),
            kind: DeniedKind::Template,
        }
    }

    /// An entry as an operator wrote it in `[sandbox] deny_read_globs`: a
    /// pattern if its spelling has a glob metacharacter ([`looks_like_glob`]),
    /// a template otherwise. The one place the shape of a string decides a
    /// kind. Private so no other producer can mint a pattern.
    ///
    /// Says so once, here, when the entry names `~` or a `%VAR%` token: the
    /// OS sandbox expands neither, so the two faces disagree about it.
    fn operator_spelled(spelling: impl Into<String>) -> Self {
        let spelling = spelling.into();
        let kind = if looks_like_glob(&spelling) {
            DeniedKind::Pattern
        } else {
            DeniedKind::Template
        };
        if names_an_unexpanded_token(&spelling) {
            let consequence = if kind == DeniedKind::Pattern {
                "the file tools do not expand a pattern either, so it matches nothing"
            } else {
                "the file tools do, so a command run inside the sandbox can still read it"
            };
            warn!(
                entry = %spelling,
                "file_ops: deny_read_globs entry starts with `~` or holds a `%VAR%` token; \
                 the OS sandbox does not expand this entry; {consequence}"
            );
        }
        Self { spelling, kind }
    }

    /// The entry as spelled (a pattern's text, a template's unexpanded form).
    pub fn as_str(&self) -> &str {
        &self.spelling
    }

    /// Whether this entry is a pattern (only an operator's glob can be).
    pub fn is_pattern(&self) -> bool {
        self.kind == DeniedKind::Pattern
    }
}

/// The operator's `[sandbox] deny_read_globs`, read once per process.
///
/// **Why a snapshot and not a live read.** `[sandbox]` is a restart-scoped
/// section (`ReloadImpact::classify("sandbox") == Restart`), and the OS drivers
/// that consume the same setting latch it when the sandbox is constructed. A
/// process-lifetime snapshot is therefore the honest reading, and it matches
/// how the rest of this module already behaves — [`get_denied_paths`] is called
/// at tool construction and [`denied_entry_normalized`] memoises each entry for
/// the process lifetime.
///
/// **Why the raw file and not `Config::load()`.** `Config::load()` *writes* a
/// default config file when none exists; a deny check running inside a tool
/// call must not create what it measures. This reads the effective config path
/// (a pure lookup) and parses nothing but the one array it needs, so an
/// unrelated malformed section cannot take the credential denylist down with
/// it.
///
/// Each entry is classified here, once, as operator-spelled
/// ([`DeniedPath::operator_spelled`]).
fn configured_deny_read_globs() -> &'static [DeniedPath] {
    static GLOBS: OnceLock<Vec<DeniedPath>> = OnceLock::new();
    GLOBS.get_or_init(|| {
        let path = crate::config::Config::effective_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            // No config file (first boot, or a test/CI home): no floor to apply.
            return Vec::new();
        };
        let globs: Vec<DeniedPath> = parse_deny_read_globs(&text)
            .into_iter()
            .map(DeniedPath::operator_spelled)
            .collect();
        if !globs.is_empty() {
            info!(
                count = globs.len(),
                config = %path.display(),
                "file_ops: [sandbox] deny_read_globs floor applied to the file tools"
            );
        }
        globs
    })
}

/// Extract `[sandbox] deny_read_globs` from a config TOML document.
///
/// Fail-soft by necessity — this runs on the file-tool path, where hard-failing
/// on an unrelated config problem would take every file operation down — but
/// never silently: an unparseable document or a non-string entry is logged as
/// a warning that names what is *not* being enforced.
fn parse_deny_read_globs(toml_text: &str) -> Vec<String> {
    let doc = match toml_text.parse::<toml::Value>() {
        Ok(doc) => doc,
        Err(e) => {
            warn!(
                error = %e,
                "file_ops: config file is not valid TOML; [sandbox] deny_read_globs NOT applied to the file tools"
            );
            return Vec::new();
        }
    };
    let Some(entries) = doc
        .get("sandbox")
        .and_then(|s| s.get("deny_read_globs"))
        .and_then(toml::Value::as_array)
    else {
        return Vec::new();
    };
    let mut globs = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry.as_str() {
            Some(pattern) if !pattern.is_empty() => globs.push(pattern.to_string()),
            _ => warn!(
                entry = ?entry,
                "file_ops: ignoring empty/non-string deny_read_globs entry; it denies NOTHING to the file tools"
            ),
        }
    }
    globs
}

/// Expand a **template** denylist entry's leading `~` (home) and Windows
/// environment tokens (`%APPDATA%` / `%LOCALAPPDATA%` / `%USERPROFILE%`) to
/// concrete paths so the prefix comparison below sees the same shape a
/// canonical path has. On other targets the `%…%` expansion is a no-op.
///
/// Pattern entries deliberately do NOT come through here — see
/// [`compile_denied_pattern`] — and neither do code-built locations
/// ([`DeniedPath::literal`]): they are already expanded.
fn expand_denied_entry(denied: &str) -> String {
    // `mut` is only exercised on Windows (the env-token expansion below); on
    // other targets the binding is written once.
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut out = if denied.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            home.join(denied.strip_prefix("~/").unwrap_or(denied))
                .to_string_lossy()
                .to_string()
        } else {
            denied.to_string()
        }
    } else {
        denied.to_string()
    };
    #[cfg(target_os = "windows")]
    {
        for (token, var) in [
            ("%APPDATA%", "APPDATA"),
            ("%LOCALAPPDATA%", "LOCALAPPDATA"),
            ("%USERPROFILE%", "USERPROFILE"),
        ] {
            if out.contains(token) {
                if let Ok(val) = std::env::var(var) {
                    out = out.replace(token, &val);
                }
            }
        }
    }
    out
}

/// One denylist entry in the form the matchers consume.
enum DeniedEntry {
    /// A concrete location (a template expanded by [`expand_denied_entry`], a
    /// code-built location as built) normalized
    /// ([`safe_normalize`]) the same way an input path is, then matched by
    /// path-component prefix so the entry covers its whole subtree.
    Literal(PathBuf),
    /// A git-style pattern from `[sandbox] deny_read_globs`, translated by the
    /// SAME function the OS floor uses —
    /// [`crate::sandbox::deny_globs::glob_to_anchored_regex`], which feeds the
    /// macOS seatbelt `(deny file-read* (regex …))` rules and the Windows
    /// deny-read ACE resolver. A second translator here would be the exact
    /// mistake `src/sandbox/platforms/common.rs` documents deleting: a
    /// semantically weaker twin that passes its own tests while producing a
    /// quieter deny floor than the one the operator configured.
    Glob(regex::Regex),
    /// A `deny_read_globs` entry that did not translate or did not compile.
    /// Matches nothing — the same outcome the OS floor reaches (it drops
    /// uncompilable patterns with a warning). Kept as an explicit third state,
    /// rather than dropped at parse time, so the memo stays a total function of
    /// the entry string and the warning fires exactly once per process.
    InertGlob,
}

/// Whether an OPERATOR-spelled `deny_read_globs` entry is a glob pattern
/// rather than a concrete path. Called only by
/// [`DeniedPath::operator_spelled`]; a code-built entry's kind is its
/// provenance ([`DeniedPath::literal`]) and is never read off its text.
///
/// It is still a shape test for the operator's own entries, deliberately:
/// `deny_read_globs = ["/srv/app/secrets"]` has always been a location that
/// covers its subtree (on both faces — the OS translator adds the subtree
/// suffix to a metacharacter-free entry), and an operator entry that contains
/// a literal `[` — `/data/[archive]` — is a pattern, a character class, as it
/// always was. The operator wrote a glob list; a `[` in it is theirs to mean.
fn looks_like_glob(entry: &str) -> bool {
    glob_shape_subject(entry).contains(['*', '?', '['])
}

/// The part of a denylist entry the glob shape test is allowed to read.
///
/// Windows verbatim paths open with `\\?\` — a literal `?` that is not a
/// wildcard, and that `std::fs::canonicalize` puts in front of *every* path it
/// returns on that platform. [`looks_like_glob`] reading it classified every
/// canonicalized entry as a pattern and sent it to the regex translator, which
/// then produced an `InertGlob` that denies nothing: a deny that silently
/// evaporated, on Windows only, for any caller that handed the list an
/// already-canonical path. `fs_scope_rebase_cannot_bypass_deny` is the test
/// that caught it — a rebased worktree target went from refused to `Ok`.
///
/// The strip is for the *classification* question alone. The entry stored for
/// the component-wise `starts_with` in [`path_is_denied`] keeps its full
/// spelling on purpose: a canonical input carries the prefix too, so removing
/// it from one side of that comparison is the shape that has flipped
/// `starts_with` from allow to deny elsewhere in this repo (see
/// `utils::paths::display_string`, whose own conversion is deliberately
/// *partial* and therefore not reusable here — it keeps the prefix for UNC and
/// past-MAX_PATH paths, i.e. exactly the entries that would stay misclassified).
///
/// Unconditional rather than `#[cfg(windows)]`: `\\?\` prefixes no legitimate
/// Unix path either, and keeping it cross-platform is what makes the test below
/// run on the machine you are reading this on.
fn glob_shape_subject(entry: &str) -> &str {
    entry.strip_prefix(r"\\?\").unwrap_or(entry)
}

/// A pattern entry, as written. Whether an entry is one is its provenance
/// ([`DeniedPath`]), never its expansion — see [`compile_denied_literal`].
fn compile_denied_pattern(denied: &str) -> DeniedEntry {
    // Deliberately NO `~` / `%APPDATA%` expansion for patterns: the OS
    // floor does not expand either, and a pattern that meant two different
    // things to the two faces is the very asymmetry this wiring closes. A
    // pattern that names `~` or `%VAR%` is warned about where it is made
    // ([`DeniedPath::operator_spelled`]).
    let Some(pattern) = crate::sandbox::deny_globs::glob_to_anchored_regex(denied) else {
        warn!(entry = %denied, "file_ops: empty deny_read_globs entry ignored");
        return DeniedEntry::InertGlob;
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => DeniedEntry::Glob(re),
        Err(e) => {
            warn!(
                entry = %denied,
                regex = %pattern,
                error = %e,
                "file_ops: deny_read_globs pattern failed to compile; it denies NOTHING to the file tools"
            );
            DeniedEntry::InertGlob
        }
    }
}

/// Whether an operator entry names a home or environment token: a leading
/// `~`, or a `%NAME%` pair. The OS sandbox expands neither (its regex is
/// anchored at `^` and matched against canonical absolute paths). On the
/// file tools a template is expanded ([`expand_denied_entry`]) and a pattern
/// is not — so such a pattern is dead on both faces, and such a template is
/// enforced by the file tools alone.
fn names_an_unexpanded_token(entry: &str) -> bool {
    let env_token = entry
        .split_once('%')
        .and_then(|(_, rest)| rest.split_once('%'))
        .is_some_and(|(name, _)| !name.is_empty());
    entry.starts_with('~') || env_token
}

/// A location or template entry, from its key ([`denied_entry_key`]): a
/// location as built, a template already expanded ([`expand_denied_entry`]).
///
/// Never re-classified: an expansion may contain `*`, `?` or `[` — a home
/// directory named `a[1]` — and is still the one location the operator's
/// spelling (`~/.ssh`) names. Classified on the expansion, `[1]` became a
/// character class: the real `~/.ssh` stopped being refused, silently, and a
/// sibling the class matched was refused instead.
fn compile_denied_literal(expanded: &str) -> DeniedEntry {
    DeniedEntry::Literal(
        safe_normalize(Path::new(expanded)).unwrap_or_else(|_| PathBuf::from(expanded)),
    )
}

/// Memo of the compiled form of each denylist entry, keyed by
/// [`denied_entry_key`] — what the entry names, not how it is spelled.
static DENIED_NORM_CACHE: OnceLock<RwLock<DeniedMemo>> = OnceLock::new();

/// Patterns and literals in separate keyspaces: a literal's expansion can be
/// the very text of some pattern entry (`/h[1]/.ssh` from `~/.ssh` under
/// `HOME=/h[1]`), and one map would hand either the other's compile.
#[derive(Default)]
struct DeniedMemo {
    patterns: HashMap<String, Arc<DeniedEntry>>,
    literals: HashMap<String, Arc<DeniedEntry>>,
}

impl DeniedMemo {
    fn side(&mut self, pattern: bool) -> &mut HashMap<String, Arc<DeniedEntry>> {
        if pattern {
            &mut self.patterns
        } else {
            &mut self.literals
        }
    }
}

/// The key an entry is memoised under: a template's expansion under the
/// current `$HOME` / environment; a location's or a pattern's own text. The
/// kind is the entry's ([`DeniedPath`]), fixed where it was made — never
/// read off the spelling or the expansion.
///
/// Keyed by the raw spelling, `~/.ssh` was compiled once under whatever home
/// was current at the first lookup and then served under every home after it —
/// a key coarser than its derivation. That only ever showed in a test binary,
/// where a fixture that moves `$HOME` got to compile the entry first and every
/// later test was judged against that fixture's dead temp directory.
/// **With a constant `$HOME` — every running server — behaviour is the same as
/// when the key was the raw spelling:** each entry maps to exactly one key,
/// compiles exactly once, and keeps the kind it was made with (the
/// expansion is never re-classified; see [`compile_denied_literal`]).
/// Expanding on every lookup is an env read and a join; the
/// `canonicalize()` / regex work the memo exists to avoid stays memoised.
///
/// Patterns are keyed as written because they are never expanded (see
/// [`compile_denied_pattern`]); locations because they are already expanded
/// ([`DeniedPath::literal`]) — re-expanding one read `ALEPH_HOME=~/x` as
/// `$HOME/x`. Templates without a `~` or `%…%` token expand to themselves
/// and borrow.
fn denied_entry_key(denied: &str, kind: DeniedKind) -> std::borrow::Cow<'_, str> {
    let expands = kind == DeniedKind::Template && (denied.starts_with('~') || denied.contains('%'));
    if expands {
        std::borrow::Cow::Owned(expand_denied_entry(denied))
    } else {
        std::borrow::Cow::Borrowed(denied)
    }
}

/// The compiled ([`compile_denied_pattern`] / [`compile_denied_literal`])
/// form of one denylist entry —
/// computed once per process for each thing it names.
///
/// [`path_is_denied`] runs once per glob match inside the `search` / `stats`
/// walks, and normalizing every entry on every call meant a `canonicalize()`
/// syscall per entry per match: `stats` over a few thousand files issued tens of
/// thousands of blocking syscalls on a tokio worker before returning four
/// numbers — minutes of round-trips on a network mount. Each key names one
/// location for as long as it exists, which is what makes compiling it exactly
/// once sound — including when `$HOME` moves, because a moved home is a
/// different key ([`denied_entry_key`]).
/// The same argument covers pattern entries: regex compilation is far more
/// expensive than a `canonicalize()`, and `[sandbox]` is restart-scoped.
///
/// This is also the reason the two directions cannot disagree: both
/// [`path_is_denied`] and [`contains_denied_descendant`] read an entry's
/// meaning from here and nowhere else.
fn denied_entry_normalized(entry: &DeniedPath) -> Arc<DeniedEntry> {
    let (denied, pattern) = (entry.as_str(), entry.is_pattern());
    let key = denied_entry_key(denied, entry.kind);
    let cache = DENIED_NORM_CACHE.get_or_init(Default::default);
    let hit = {
        let memo = cache.read().unwrap_or_else(|e| e.into_inner());
        let side = if pattern {
            &memo.patterns
        } else {
            &memo.literals
        };
        side.get(key.as_ref()).cloned()
    };
    if let Some(hit) = hit {
        return hit;
    }
    // A literal is compiled from the key, not the raw entry, so the stored
    // meaning is the one the key names even if `$HOME` moves between the two
    // expansions — and it is compiled AS a literal, whatever its expansion
    // spells.
    let compiled = Arc::new(if pattern {
        compile_denied_pattern(denied)
    } else {
        compile_denied_literal(&key)
    });
    cache
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .side(pattern)
        .insert(key.into_owned(), Arc::clone(&compiled));
    compiled
}

/// Whether an already-canonical path falls under any denylist entry.
///
/// The single source of truth for the deny check, shared by
/// [`check_and_resolve_path`] and by the per-entry re-checks that enumeration /
/// relocation operations (`stats`, `organize`, recursive `copy`) run on paths
/// they discover *after* the initial gate — a symlink or glob match can point
/// at a denied target the top-level path never named.
///
/// Literal entries (templates expanded by [`expand_denied_entry`], locations
/// as built) are normalized the
/// SAME way as the input (resolving symlinks in existing ancestors) before the
/// component-wise prefix compare, so a symlinked ancestor (`/etc` →
/// `/private/etc` on macOS) cannot defeat it. Pattern entries are matched
/// against the `/`-normalised string form of the same canonical path — the
/// identical normalisation
/// [`crate::sandbox::deny_globs::resolve_deny_read_paths_under`] applies before
/// handing paths to the Windows ACE stamper, so a Windows `\` path and a Unix
/// `/` path are judged by one rule.
///
/// The prefix compare is the file system's, not the string's
/// ([`names_within`]): where names are compared case-insensitively, a path
/// that spells a denied entry in another case is that entry.
pub fn path_is_denied(canonical: &Path, denied_paths: &[DeniedPath]) -> bool {
    // Computed at most once per call, and only if a pattern entry is present.
    let mut slash_form: Option<String> = None;
    for denied in denied_paths {
        match &*denied_entry_normalized(denied) {
            DeniedEntry::Literal(location) => {
                if names_within(canonical, location) {
                    return true;
                }
            }
            DeniedEntry::Glob(re) => {
                let subject = slash_form
                    .get_or_insert_with(|| canonical.to_string_lossy().replace('\\', "/"));
                if re.is_match(subject) {
                    return true;
                }
            }
            DeniedEntry::InertGlob => {}
        }
    }
    false
}

/// The denylist entry living *beneath* `candidate`, if any.
///
/// [`path_is_denied`] only answers the downward question — "is this path under a
/// protected entry" — so an operation on a PARENT sailed past it: nothing on the
/// denylist names `<config_dir>` itself, yet `remove_dir_all` on it wipes the
/// `secrets.vault` and `data/` auth databases that deleting either directly is
/// correctly refused, and `rename` relocates that whole protected tree out to an
/// undenied location in a single syscall. Shares
/// [`denied_entry_normalized`] with the downward check so the two directions can
/// never disagree about what an entry means.
///
/// Returns the protected location so the refusal can name it. Equality is not a
/// hit: a candidate that *is* a denied entry is already refused by the downward
/// check.
///
/// # Pattern entries answer only the downward question
///
/// `deny_read_globs` entries ([`DeniedEntry::Glob`]) are skipped here, and that
/// is a deliberate, disclosed gap rather than an oversight:
///
/// * A glob is a *predicate over paths*, not a location. There is no "the
///   protected entry beneath `candidate`" to return without walking the
///   candidate's subtree, and a walk has to be bounded (the OS-side walk
///   [`crate::sandbox::deny_globs::resolve_deny_read_paths_under`] caps at
///   50 000 entries). A capped walk answers "I found nothing" when it means "I
///   stopped looking" — a fail-soft skip read as evidence of absence, on a
///   security gate. That is worse than a documented gap.
/// * The OS floor draws the same line. Seatbelt emits per-access
///   `(deny file-read* …)` / `(deny file-write-unlink …)` rules; it refuses to
///   *read or unlink a matching path*, and equally does not refuse renaming an
///   ancestor directory that happens to contain one.
///
/// Consequence, stated plainly: with `deny_read_globs = ["**/.env"]`, a
/// `file_ops delete` or `move` aimed at a *parent directory* still takes the
/// matching file with it, whereas naming the file directly is refused (the
/// downward check in [`path_is_denied`] covers that) and a recursive `copy`
/// skips it and says so. The fixed credential entries keep full two-direction
/// coverage, which is why the match below is on the entry kind and not a bare
/// `if` — the two directions still read one compiled entry from
/// [`denied_entry_normalized`] and cannot disagree about what an entry *means*.
pub fn contains_denied_descendant(
    candidate: &Path,
    denied_paths: &[DeniedPath],
) -> Option<PathBuf> {
    denied_paths
        .iter()
        .find_map(|denied| match &*denied_entry_normalized(denied) {
            DeniedEntry::Literal(location) => (names_within(location, candidate)
                && !names_within(candidate, location))
            .then(|| location.clone()),
            DeniedEntry::Glob(_) | DeniedEntry::InertGlob => None,
        })
}

/// Whether `path` names `prefix` or something beneath it, compared the way
/// this platform's default file system compares names — the one comparison
/// both deny directions ([`path_is_denied`], [`contains_denied_descendant`])
/// make against a literal entry.
///
/// A path reaches here from [`safe_normalize`]: its deepest EXISTING ancestor
/// is canonical — on macOS and Windows in the case stored on disk — and
/// whatever does not exist yet is appended AS WRITTEN. So a missing leaf keeps
/// the model's spelling: `<config>/SHELL-HOOKS-ALLOWLIST.json`, written before
/// the registry exists, is the file `ShellHookConsent` then reads as
/// `shell-hooks-allowlist.json` on a case-insensitive volume, and a
/// case-sensitive `starts_with` let the model create its own approval that
/// way (review P4.17 R2-I-1). The same held for every absent credential leaf
/// (`~/.netrc`, `~/.git-credentials`, `~/.npmrc`, `~/.pypirc`,
/// `~/.docker/config.json`) and for the gate files `approval-grants.json` and
/// `approval-policy.json`, which a quiet install never creates.
///
/// - **macOS and Windows:** names are compared case-folded
///   ([`fold_name`]), always. APFS and HFS+ volumes are case-insensitive by
///   default and NTFS is on Windows. On a volume formatted case-SENSITIVE this
///   over-denies — `~/.NETRC` is then a different file and is refused anyway —
///   which fails closed: a refused write the operator can do by hand, never a
///   write that lands on a protected file.
/// - **Elsewhere:** names compare exactly, as the file system does.
///
/// Not covered (recorded, unverified): Unicode normalisation (APFS treats NFC
/// and NFD spellings as one name; every fixed entry is ASCII, so an alias
/// would need a decomposable letter the entry does not have), and Windows 8.3
/// short names (`SHELL-~1.JSO`), which only exist for files that exist — and
/// an existing file is canonicalized to its long name before it gets here.
fn names_within(path: &Path, prefix: &Path) -> bool {
    if !cfg!(any(target_os = "macos", windows)) {
        return path.starts_with(prefix);
    }
    let mut names = path.components();
    prefix
        .components()
        .all(|want| names.next().is_some_and(|got| same_name(got, want)))
}

/// Whether two path components name the same thing on a case-insensitive
/// file system ([`names_within`]).
fn same_name(a: std::path::Component<'_>, b: std::path::Component<'_>) -> bool {
    use std::path::Component;
    match (a, b) {
        (Component::Normal(a), Component::Normal(b)) => fold_name(a) == fold_name(b),
        (Component::Prefix(a), Component::Prefix(b)) => {
            fold_name(a.as_os_str()) == fold_name(b.as_os_str())
        }
        _ => a == b,
    }
}

/// A name as a case-insensitive file system compares it: upper- then
/// lower-cased, so letters whose simple lower case is not their fold still
/// meet (`ſ` → `S` → `s`, the Kelvin sign `K` → `k`). On Windows, also
/// without the trailing dots and spaces Win32 strips from a name
/// (`shell-hooks-allowlist.json.` opens `shell-hooks-allowlist.json`).
fn fold_name(name: &std::ffi::OsStr) -> String {
    let folded = name.to_string_lossy().to_uppercase().to_lowercase();
    if cfg!(windows) {
        folded.trim_end_matches(['.', ' ']).to_string()
    } else {
        folded
    }
}

/// Whether `canonical` is a Linux `/proc/<pid>/…` pseudo-file that leaks another
/// process's secrets (environment, memory, mappings). These are not covered by
/// the credential denylist and are not regular files an agent has any business
/// reading — `/proc/<pid>/environ` alone exposes every exported secret of a
/// running process. Defense-in-depth mirroring hermes-agent's
/// `_is_blocked_device_path`; a no-op on non-Linux where `/proc` is absent.
pub fn is_blocked_proc_path(canonical: &Path) -> bool {
    use std::path::Component;
    let mut comps = canonical.components();
    // Must be rooted at `/proc/<something>/…`.
    if comps.next() != Some(Component::RootDir) {
        return false;
    }
    if comps.next() != Some(Component::Normal(std::ffi::OsStr::new("proc"))) {
        return false;
    }
    // `<pid>` (or `self` / `thread-self`) — any single component.
    if comps.next().is_none() {
        return false;
    }
    // Block the secret-bearing leaves anywhere below the pid dir.
    const BLOCKED_LEAVES: &[&str] = &[
        "environ",
        "cmdline",
        "mem",
        "maps",
        "smaps",
        "smaps_rollup",
        "numa_maps",
        "auxv",
        "pagemap",
        "stack",
        "syscall",
    ];
    canonical
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|leaf| BLOCKED_LEAVES.contains(&leaf))
}

/// Reject glob patterns that would escape the (already deny-checked) base
/// directory: absolute patterns replace the base via `Path::join`, and any
/// `..` component climbs out of it. Relative, non-climbing patterns are safe
/// because every match still lands under `canonical`.
///
/// Uses `has_root()` instead of `is_absolute()` so that root-anchored-but-
/// drive-relative patterns (e.g. `/etc/*` on Windows, which has a root but no
/// drive prefix) are also rejected — they still escape the base via `join`.
///
/// Additionally rejects any pattern containing a drive or UNC prefix
/// (`Component::Prefix`) — e.g. `C:foo` on Windows. Such patterns are not
/// root-anchored (`has_root()` returns false) yet `Path::join(base, "C:foo")`
/// discards the base entirely and resolves relative to drive C's current
/// directory, bypassing the deny-checked base. On Unix `Component::Prefix`
/// never occurs, so this check is a safe no-op there.
pub(crate) fn reject_unsafe_glob_pattern(pattern: &str) -> Result<(), ToolError> {
    let p = std::path::Path::new(pattern);
    if p.has_root() {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must be relative to the search directory: {pattern}"
        )));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::Prefix(_)))
    {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain a drive/UNC prefix: {pattern}"
        )));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain `..`: {pattern}"
        )));
    }
    Ok(())
}

/// Expand `$HOME`/`$USER`, a leading `~`, and a relative base into a concrete
/// path **without canonicalizing** — so a final-component symlink is preserved
/// (canonicalization would resolve it to its target). Shared by
/// [`check_and_resolve_path`] and [`resolve_for_removal`].
fn expand_input_path(
    path: &Path,
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    // First, expand environment variables in the path string
    let path_str = path.to_string_lossy();
    let expanded_str = if path_str.contains('$') {
        // BT-A-R4-04: anchor the $HOME / $USER substitution so substrings
        // like `$HOMEBREW`, `$USERDATA`, `$(HOME)`, or a `path=$HOME/foo`
        // embedded inside a longer identifier are NOT mangled. The previous
        // `String::replace` swapped every literal occurrence, turning
        // `/opt/$HOMEBREW/bin/foo` into `/opt//home/aliceBREW/bin/foo`,
        // which then failed canonicalize() and surfaced as a generic
        // "file not found" with no hint at the real cause.
        //
        // Walk the string token by token: only `$NAME` (followed by a
        // non-identifier byte) or `${NAME}` (followed by `}`) is treated
        // as a substitution candidate. The allowlist remains `HOME` and
        // `USER` — arbitrary env-var expansion stays off so a hostile
        // shell cannot inject a path via $IFS / $PATH / $LD_PRELOAD.
        let mut out = String::with_capacity(path_str.len());
        let bytes = path_str.as_bytes();
        let mut i = 0;
        let home_str = dirs::home_dir().map(|h| h.to_string_lossy().into_owned());
        let user_str = std::env::var("USER").ok();
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let after = &bytes[i + 1..];
                if let Some(rest) = after.strip_prefix(b"{") {
                    if let Some(close) = rest.iter().position(|&b| b == b'}') {
                        let name = std::str::from_utf8(&rest[..close]).unwrap_or("");
                        match name {
                            "HOME" => {
                                if let Some(ref h) = home_str {
                                    out.push_str(h);
                                } else {
                                    out.push_str("${HOME}");
                                }
                                i += 1 + 1 + close + 1;
                                continue;
                            }
                            "USER" => {
                                if let Some(ref u) = user_str {
                                    out.push_str(u);
                                } else {
                                    out.push_str("${USER}");
                                }
                                i += 1 + 1 + close + 1;
                                continue;
                            }
                            _ => {
                                // Unknown braced var: pass through verbatim
                                // so the operator sees the literal in any
                                // later error.
                                let end = i + 1 + 1 + close + 1;
                                out.push_str(&path_str[i..end]);
                                i = end;
                                continue;
                            }
                        }
                    }
                }
                // Unbraced: read identifier characters [A-Za-z0-9_].
                let id_end = after
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
                    .count();
                if id_end > 0 {
                    let name = std::str::from_utf8(&after[..id_end]).unwrap_or("");
                    match name {
                        "HOME" => {
                            if let Some(ref h) = home_str {
                                out.push_str(h);
                            } else {
                                out.push('$');
                                out.push_str(name);
                            }
                            i += 1 + id_end;
                            continue;
                        }
                        "USER" => {
                            if let Some(ref u) = user_str {
                                out.push_str(u);
                            } else {
                                out.push('$');
                                out.push_str(name);
                            }
                            i += 1 + id_end;
                            continue;
                        }
                        _ => {
                            // Pass through — do NOT replace.
                            out.push('$');
                            out.push_str(name);
                            i += 1 + id_end;
                            continue;
                        }
                    }
                }
            }
            // Push the current UTF-8 character (bytes[i..] may be multi-byte).
            let ch_end = i + path_str[i..].chars().next().map_or(1, |c| c.len_utf8());
            out.push_str(&path_str[i..ch_end]);
            i = ch_end;
        }
        PathBuf::from(out)
    } else {
        path.to_path_buf()
    };

    // Expand ~ to home directory
    if expanded_str.starts_with("~/") || expanded_str.as_os_str() == "~" {
        let home = dirs::home_dir()
            .ok_or_else(|| ToolError::InvalidArgs("Cannot determine home directory".to_string()))?;
        Ok(home.join(
            expanded_str
                .strip_prefix("~")
                .unwrap_or_else(|_| std::path::Path::new("")),
        ))
    } else if expanded_str.is_relative() {
        // Relative paths are resolved to:
        // 1. Per-run FsScope base (task-local — worktree root for isolated
        //    agents, workspace artifact dir for normal runs)
        // 2. ToolContext output_dir override (workspace-scoped, set by ExecutionEngine)
        // 3. Error if neither is available — callers must provide a base directory
        let base_dir = if let Some(scope) = crate::tools::fs_scope::current() {
            info!(fs_scope = %scope.base.display(), "check_path: using per-run FsScope base");
            scope.base
        } else if let Some(override_dir) = output_dir_override {
            info!(output_dir = %override_dir.display(), "check_path: using ToolContext output_dir override");
            override_dir.to_path_buf()
        } else {
            return Err(ToolError::InvalidArgs(
                "Relative path requires an active run scope or an output directory override; \
                 provide an absolute path instead"
                    .to_string(),
            ));
        };
        Ok(base_dir.join(expanded_str))
    } else {
        Ok(expanded_str)
    }
}

/// Resolve a path for a **removal or rename** whose final component must NOT be
/// followed when it is a symlink.
///
/// `check_and_resolve_path` canonicalizes a final-component symlink to its
/// *target*; a `delete`/`move` acting on that target would destroy the tree the
/// link points at and leave the link dangling (or move the target out from
/// under it). Filesystem `remove_file` / `rename` never follow a final symlink,
/// so operating on the link path is both correct and what the user meant.
///
/// The full deny check still runs against the resolved target (via
/// [`check_and_resolve_path`]), and the link's own location is deny-checked too,
/// so neither the link nor its target can name a protected location. Returns the
/// path to operate on: the un-followed link when the final component is a
/// symlink, otherwise the canonical target (identical to
/// `check_and_resolve_path`).
pub fn resolve_for_removal(
    path: &Path,
    denied_paths: &[DeniedPath],
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    // Deny-check the resolved target first (conservative: a link whose target is
    // protected cannot be used as a handle to it).
    let canonical_target = check_and_resolve_path(path, denied_paths, output_dir_override)?;

    let expanded = expand_input_path(path, output_dir_override)?;
    let is_symlink = std::fs::symlink_metadata(&expanded)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_symlink {
        return Ok(canonical_target);
    }

    // The final component is a symlink: operate on the LINK, not its target.
    // Canonicalize only the PARENT (resolving any intermediate symlinks + the
    // FsScope rebase) and re-attach the un-followed final component.
    let Some(file_name) = expanded.file_name() else {
        return Ok(canonical_target);
    };
    let parent = expanded.parent().unwrap_or_else(|| Path::new("/"));
    let canon_parent = safe_normalize(parent)
        .map_err(|e| ToolError::Execution(format!("Failed to resolve parent: {e}")))?;
    let canon_parent =
        match crate::tools::fs_scope::current().and_then(|s| s.rebase_path(&canon_parent)) {
            Some(rebased) => safe_normalize(&rebased).map_err(|e| {
                ToolError::Execution(format!("Failed to normalize rebased parent: {e}"))
            })?,
            None => canon_parent,
        };
    let link_path = canon_parent.join(file_name);
    if path_is_denied(&link_path, denied_paths) {
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} is in a protected location",
            path.display()
        )));
    }
    Ok(link_path)
}

/// Check if path is allowed and resolve it — the **file layer's** sole path
/// resolver.
///
/// # There are two path resolvers in this repo, on purpose
///
/// The other one is `sandbox::workspace::path::normalize_path` in
/// `src/sandbox/workspace/path.rs`, and the two answer *different questions*.
/// Unifying them would silently delete one of the two answers, so
/// `path_utils::tests::the_two_path_resolvers_stay_split` fails by name if
/// either stops being the sole resolver for its own layer, or if a third
/// appears.
///
/// | | this function (file layer) | `sandbox::workspace::path::normalize_path` (exec layer) |
/// |---|---|---|
/// | question | "may the model's file tools touch this path, and where does it really land?" | "does this path stay inside the session's workspace jail?" |
/// | `~` / `$HOME` / `$USER` | expanded | not expanded |
/// | relative base | task-local [`FsScope`](crate::tools::fs_scope::FsScope), else the `ToolContext` output dir | the workspace root, always |
/// | symlinks | canonicalized (existing ancestors resolved) | never resolved — `..` is popped *lexically*, before any syscall |
/// | denylist | yes: credential entries + `[sandbox] deny_read_globs` + `/proc` secrets | none |
/// | root containment | none — an absolute path is used as-is (see the tool `DESCRIPTION`) | hard jail enforced by the caller |
///
/// Net: the exec layer is an **allowlist jail with no denylist**; the file layer
/// is a **denylist with no jail**. Each is unsound as the other's gate.
///
/// Path resolution rules:
/// 1. Environment variables ($HOME, $USER, etc.) - expanded first
/// 2. Absolute paths (starting with `/`) - used as-is, then rebased through
///    the active [`FsScope`](crate::tools::fs_scope::FsScope) remap when the
///    run is worktree-isolated (parent-repo paths land inside the worktree,
///    mirroring what `WorktreeSandbox` already does for command execution)
/// 3. Home paths (starting with `~`) - expanded to home directory
/// 4. Relative paths - resolved relative to:
///    a. the per-run `FsScope` task-local base — per-run truth, immune to a
///    concurrent run rewriting the shared `ToolContextHandle` mid-run
///    b. `output_dir_override` if provided (workspace-scoped output dir from `ToolContext`)
///    c. Error if neither is available — no global fallback
///
/// The deny check always runs on the FINAL path (post-rebase), so a remap can
/// never smuggle a denied location past the gate.
pub fn check_and_resolve_path(
    path: &Path,
    denied_paths: &[DeniedPath],
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    info!(path = %path.display(), "check_path: input path");

    // Env-var / `~` / relative-base expansion (NO canonicalization — a final
    // symlink is preserved). Shared with `resolve_for_removal` so the two
    // resolvers cannot drift on how a spelled path becomes a filesystem path.
    let expanded = expand_input_path(path, output_dir_override)?;

    info!(expanded = %expanded.display(), exists = expanded.exists(), "check_path: expanded path");

    // Canonicalize if exists; for non-existent files, manually normalize to resolve ".."
    // components. This prevents path traversal bypasses (e.g., "/allowed/../secret/file").
    let canonical = if expanded.exists() {
        expanded
            .canonicalize()
            .map_err(|e| ToolError::Execution(format!("Failed to resolve path: {e}")))?
    } else {
        // For non-existent paths, canonicalize the longest existing ancestor
        // then append remaining components. This prevents symlink-based traversal
        // that pure component normalization would miss.
        safe_normalize(&expanded).map_err(|e| {
            ToolError::Execution(format!("Failed to normalize non-existent path: {e}"))
        })?
    };

    info!(canonical = %canonical.display(), "check_path: canonical path");

    // Worktree-isolation remap: when the active FsScope declares a rebase,
    // canonical paths under the parent repo are redirected into the isolated
    // worktree BEFORE the deny check below — the gate therefore evaluates the
    // path that will actually be touched.
    let canonical = match crate::tools::fs_scope::current().and_then(|s| s.rebase_path(&canonical))
    {
        Some(rebased) => {
            info!(
                from = %canonical.display(),
                to = %rebased.display(),
                "check_path: FsScope rebase into isolated worktree"
            );
            // Re-normalize so the result stays canonical (the worktree side
            // may sit behind a symlinked tmpdir) — keeps `path_locks` keys
            // consistent across spellings of the same file.
            safe_normalize(&rebased).map_err(|e| {
                ToolError::Execution(format!("Failed to normalize rebased path: {e}"))
            })?
        }
        None => canonical,
    };

    // Check against denied paths. Uses Path-component prefix matching (not
    // string starts_with, which would falsely match "/foo-bar" against "/foo")
    // via the shared `path_is_denied` helper, which canonicalizes each denied
    // entry the same way as the input so a symlinked ancestor (macOS
    // `/etc` -> `/private/etc`) cannot defeat it.
    if path_is_denied(&canonical, denied_paths) {
        info!(
            canonical = %canonical.display(),
            "check_path: ACCESS DENIED - path matches denied pattern"
        );
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} is in a protected location",
            path.display()
        )));
    }

    // Defense-in-depth: block `/proc/<pid>/{environ,maps,mem,…}` secret-bearing
    // pseudo-files regardless of the credential denylist.
    if is_blocked_proc_path(&canonical) {
        info!(
            canonical = %canonical.display(),
            "check_path: ACCESS DENIED - /proc secret pseudo-file"
        );
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} exposes another process's secrets",
            path.display()
        )));
    }

    info!(canonical = %canonical.display(), "check_path: path allowed");
    Ok(canonical)
}

/// Normalize a non-existent path by canonicalizing the longest existing ancestor,
/// then appending the remaining components. This prevents symlink-based path traversal
/// that pure component-level normalization would miss.
///
/// Returns an error if the longest existing ancestor cannot be canonicalized
/// (e.g., due to permission issues), ensuring we never return an uncanonicalized
/// path that could bypass security checks.
fn safe_normalize(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut remaining = Vec::new();
    while !existing.exists() {
        if let Some(file_name) = existing.file_name() {
            remaining.push(file_name.to_owned());
            existing.pop();
        } else {
            break;
        }
    }
    let mut result = existing.canonicalize().map_err(|e| {
        format!(
            "Failed to canonicalize ancestor '{}': {}",
            existing.display(),
            e
        )
    })?;
    for component in remaining.into_iter().rev() {
        if component == ".." {
            result.pop();
        } else if component != "." {
            result.push(component);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // --- reject_unsafe_glob_pattern ---

    #[test]
    fn glob_guard_allows_relative_patterns() {
        assert!(
            reject_unsafe_glob_pattern("*.txt").is_ok(),
            "bare wildcard must be accepted"
        );
        assert!(
            reject_unsafe_glob_pattern("images/photo.jpg").is_ok(),
            "relative sub-path must be accepted"
        );
        assert!(
            reject_unsafe_glob_pattern("**/foo").is_ok(),
            "recursive glob must be accepted"
        );
    }

    #[test]
    fn glob_guard_rejects_root_anchored() {
        assert!(
            matches!(
                reject_unsafe_glob_pattern("/etc/*"),
                Err(ToolError::InvalidArgs(_))
            ),
            "/etc/* is root-anchored and must be rejected"
        );
    }

    #[test]
    fn glob_guard_rejects_parent_dir() {
        assert!(
            matches!(
                reject_unsafe_glob_pattern("../secrets"),
                Err(ToolError::InvalidArgs(_))
            ),
            "../secrets contains `..` and must be rejected"
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_guard_rejects_drive_relative_prefix() {
        // On Windows, `C:foo` has a Prefix component but no root — Path::join
        // with any base replaces the base entirely, so it must be rejected.
        assert!(
            matches!(
                reject_unsafe_glob_pattern("C:foo"),
                Err(ToolError::InvalidArgs(_))
            ),
            "C:foo is a drive-relative pattern and must be rejected on Windows"
        );
    }

    /// The denylist must include Aleph's own encrypted vault and the `data/`
    /// auth directory. Asserted by path *suffix* so the test stays hermetic and
    /// independent of where `get_config_dir()` resolves in the test environment
    /// (no `ALEPH_HOME`/`$HOME` mutation, hence no cross-test env leak).
    #[test]
    fn denied_paths_cover_aleph_credential_stores() {
        let denied = get_denied_paths();
        assert!(
            denied
                .iter()
                .any(|p| p.as_str().ends_with("/secrets.vault")),
            "secrets.vault missing from denylist: {denied:?}"
        );
        assert!(
            denied.iter().any(|p| p.as_str().ends_with("/data")),
            "data/ auth dir missing from denylist: {denied:?}"
        );
    }

    /// End-to-end enforcement: the vault leaf file is rejected, a file *inside*
    /// the denied `data/` directory is rejected via the canonicalizing prefix
    /// match, and an unrelated sibling under the same root is still allowed.
    #[test]
    fn check_path_blocks_vault_and_data_allows_sibling() {
        let root = tempdir().unwrap();
        let vault = root.path().join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let data = root.path().join("data");
        fs::create_dir(&data).unwrap();
        let pairing = data.join("pairing.db");
        fs::write(&pairing, b"db").unwrap();
        let allowed = root.path().join("output.txt");
        fs::write(&allowed, b"ok").unwrap();

        let denied = vec![
            DeniedPath::literal(vault.to_string_lossy()),
            DeniedPath::literal(data.to_string_lossy()),
        ];

        // Vault leaf file is denied.
        assert!(
            check_and_resolve_path(&vault, &denied, None).is_err(),
            "vault read should be denied"
        );
        // A file inside the denied data/ dir is denied (directory-prefix match).
        assert!(
            check_and_resolve_path(&pairing, &denied, None).is_err(),
            "data/pairing.db read should be denied"
        );
        // An unrelated sibling under the same root is allowed.
        assert!(
            check_and_resolve_path(&allowed, &denied, None).is_ok(),
            "unrelated sibling should be allowed"
        );
    }

    /// Relative paths anchor at the per-run `FsScope` base when one is
    /// published — and the scope wins over the (potentially stale, shared)
    /// `output_dir_override`.
    #[tokio::test]
    async fn fs_scope_base_anchors_relative_paths() {
        let scope_root = tempdir().unwrap();
        let other_root = tempdir().unwrap();
        let scope = crate::tools::fs_scope::FsScope::workspace(scope_root.path().to_path_buf());
        let resolved = crate::tools::fs_scope::with_fs_scope(Some(scope), async {
            check_and_resolve_path(Path::new("sub/file.txt"), &[], Some(other_root.path()))
        })
        .await
        .expect("relative path must resolve inside the scope base");
        let canonical_scope = scope_root.path().canonicalize().unwrap();
        assert_eq!(resolved, canonical_scope.join("sub/file.txt"));
    }

    /// Worktree isolation: an absolute path under the parent repo is rebased
    /// into the worktree checkout before any filesystem access.
    #[tokio::test]
    async fn fs_scope_rebase_redirects_parent_repo_paths() {
        let repo = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::create_dir_all(repo.path().join("src")).unwrap();
        fs::write(repo.path().join("src/a.rs"), b"fn main() {}").unwrap();
        let repo_c = repo.path().canonicalize().unwrap();
        let wt_c = wt.path().canonicalize().unwrap();

        let scope = crate::tools::fs_scope::FsScope::worktree(wt_c.clone(), repo_c.clone());
        let input = repo_c.join("src/a.rs");
        let resolved = crate::tools::fs_scope::with_fs_scope(Some(scope), async move {
            check_and_resolve_path(&input, &[], None)
        })
        .await
        .expect("rebase must succeed");
        assert_eq!(resolved, wt_c.join("src/a.rs"));
    }

    #[test]
    fn path_is_denied_matches_directory_prefix_not_string_prefix() {
        let root = tempdir().unwrap();
        let secret_dir = root.path().join("secret");
        fs::create_dir(&secret_dir).unwrap();
        let sibling = root.path().join("secret-sibling");
        fs::create_dir(&sibling).unwrap();
        let denied = vec![DeniedPath::literal(secret_dir.to_string_lossy())];
        // `path_is_denied` expects an already-canonical input (its contract);
        // canonicalize the dirs so a symlinked tempdir root (macOS
        // `/var` → `/private/var`) does not defeat the prefix compare.
        let secret_c = secret_dir.canonicalize().unwrap();
        let sibling_c = sibling.canonicalize().unwrap();

        assert!(path_is_denied(&secret_c.join("k.pem"), &denied));
        // A string-prefix sibling ("secret-sibling") must NOT match.
        assert!(!path_is_denied(&sibling_c.join("ok.txt"), &denied));
    }

    /// The upward check sees a protected entry the downward check cannot: a
    /// parent is not "under" the denylist, but destroying or relocating it takes
    /// the protected entry with it. Directions must stay disjoint — a candidate
    /// that IS the entry is the downward check's case.
    #[test]
    fn contains_denied_descendant_finds_protected_child_only() {
        let root = tempdir().unwrap();
        let config = root.path().join("aleph");
        fs::create_dir(&config).unwrap();
        let vault = config.join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let sibling = root.path().join("other");
        fs::create_dir(&sibling).unwrap();
        let denied = vec![DeniedPath::literal(vault.to_string_lossy())];

        let config_c = config.canonicalize().unwrap();
        let vault_c = vault.canonicalize().unwrap();
        assert_eq!(
            contains_denied_descendant(&config_c, &denied),
            Some(vault_c.clone()),
            "the parent must report the protected entry it holds"
        );
        assert_eq!(
            contains_denied_descendant(&vault_c, &denied),
            None,
            "the entry itself is the downward check's case, not a descendant"
        );
        assert_eq!(
            contains_denied_descendant(&sibling.canonicalize().unwrap(), &denied),
            None,
            "an unrelated directory holds nothing protected"
        );
    }

    /// Each raw denylist entry is expanded + normalized ONCE per process: the
    /// walk operations call `path_is_denied` per glob match, and re-running
    /// `canonicalize()` for every entry on every call is what turned a `stats`
    /// over a few thousand files into tens of thousands of blocking syscalls.
    ///
    /// Observable via a swap the memo must not notice: the entry first resolves
    /// through a symlink, then the symlink is replaced by a real directory.
    #[cfg(unix)]
    #[test]
    fn denied_entry_is_normalized_once_per_process() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = root.path().join("link");
        symlink(&real, &link).unwrap();
        let real_c = real.canonicalize().unwrap();

        let denied = vec![DeniedPath::literal(link.to_string_lossy())];
        assert!(
            path_is_denied(&real_c.join("k.pem"), &denied),
            "the entry resolves through the symlink to `real`"
        );

        // Replace the symlink with a real directory: a re-normalization would
        // now resolve the entry to `link` itself and stop covering `real`.
        fs::remove_file(&link).unwrap();
        fs::create_dir(&link).unwrap();
        assert!(
            path_is_denied(&real_c.join("k.pem"), &denied),
            "the entry must not be re-normalized on the second call"
        );
    }

    #[cfg(unix)]
    #[test]
    fn is_blocked_proc_path_flags_secret_leaves_only() {
        use std::path::Path;
        assert!(is_blocked_proc_path(Path::new("/proc/1234/environ")));
        assert!(is_blocked_proc_path(Path::new("/proc/self/maps")));
        assert!(is_blocked_proc_path(Path::new("/proc/1/mem")));
        // A benign /proc leaf and non-/proc paths are allowed.
        assert!(!is_blocked_proc_path(Path::new("/proc/1234/status")));
        assert!(!is_blocked_proc_path(Path::new("/proc/cpuinfo")));
        assert!(!is_blocked_proc_path(Path::new("/home/u/environ")));
    }

    #[cfg(unix)]
    #[test]
    fn denied_paths_cover_privilege_escalation_surfaces() {
        let denied = get_denied_paths();
        for p in ["/etc/sudoers", "/etc/cron.d", "/etc/pam.d", "/root/.ssh"] {
            assert!(
                denied.iter().any(|d| d.as_str() == p),
                "{p} missing from denylist: {denied:?}"
            );
        }
    }

    // --- `[sandbox] deny_read_globs` (the OS floor's second face) ---

    /// RED before the fix: `deny_read_globs = ["**/.env"]` kernel-blocked
    /// `bash` while `file_read` / `file_ops` read the same file in plain text,
    /// because the file layer only ever understood literal path entries.
    #[test]
    fn deny_read_glob_entry_refuses_the_matching_path() {
        let root = tempdir().unwrap();
        let secret = root.path().join("app/.env");
        fs::create_dir_all(secret.parent().unwrap()).unwrap();
        fs::write(&secret, b"TOKEN=1").unwrap();
        let benign = root.path().join("app/config.toml");
        fs::write(&benign, b"k=1").unwrap();

        // Exactly the string an operator puts in `[sandbox] deny_read_globs`.
        let denied = vec![DeniedPath::operator_spelled("**/.env")];

        let err = check_and_resolve_path(&secret, &denied, None)
            .expect_err("a deny_read_globs match must be refused by the file tools too");
        assert!(
            err.to_string().contains("protected location"),
            "refusal must name the reason, got: {err}"
        );
        assert!(
            check_and_resolve_path(&benign, &denied, None).is_ok(),
            "a non-matching sibling must still be readable"
        );
    }

    /// The pattern is translated by the OS floor's own translator, so the two
    /// faces cannot drift: component-scoped `*`, `**/` spanning directories,
    /// and a metacharacter-free entry covering its whole subtree.
    #[test]
    fn deny_read_glob_semantics_match_the_os_floor() {
        let root = tempdir().unwrap();
        let deep = root.path().join("a/b");
        fs::create_dir_all(&deep).unwrap();
        let nested_pem = deep.join("key.pem");
        fs::write(&nested_pem, b"-----BEGIN-----").unwrap();
        let sub = root.path().join("a/keys");
        fs::create_dir_all(&sub).unwrap();
        let under_dir = sub.join("id_rsa");
        fs::write(&under_dir, b"priv").unwrap();
        let root_c = root.path().canonicalize().unwrap();

        // `**/*.pem` crosses directories; the same regex the seatbelt driver
        // would emit.
        assert!(path_is_denied(
            &nested_pem.canonicalize().unwrap(),
            &[DeniedPath::operator_spelled("**/*.pem")]
        ));
        // `*` stays inside one component, so it must NOT reach a nested file.
        assert!(!path_is_denied(
            &nested_pem.canonicalize().unwrap(),
            &[DeniedPath::operator_spelled(format!(
                "{}/*.pem",
                root_c.display()
            ))]
        ));
        // A metacharacter-free entry is a literal location covering its subtree
        // (both the glob translator and the literal prefix match agree here).
        assert!(path_is_denied(
            &under_dir.canonicalize().unwrap(),
            &[DeniedPath::operator_spelled(sub.to_string_lossy())]
        ));
    }

    /// A Windows verbatim path is a LOCATION, not a pattern.
    ///
    /// `std::fs::canonicalize` returns `\\?\C:\...` on Windows, so a caller that
    /// hands the denylist an already-canonical path — which
    /// `fs_scope_rebase_cannot_bypass_deny` does, and which is a perfectly
    /// reasonable thing to do — used to have that entry read as a glob (the
    /// prefix's literal `?`) and compiled to an `InertGlob` that denies
    /// nothing. The deny evaporated in silence, on Windows only.
    ///
    /// A code-built entry is a literal by construction now ([`DeniedPath`]),
    /// so the shape test only ever reads an operator's `deny_read_globs`
    /// entry — and an operator may paste a verbatim path there too.
    ///
    /// Runs everywhere: the classification is a pure string test, so this pins
    /// the behaviour on the machine you are reading it on rather than waiting
    /// for a Windows runner to disagree.
    #[test]
    fn a_windows_verbatim_entry_is_a_literal_not_a_pattern() {
        const CANONICAL: &str = r"\\?\C:\Users\me\creds\id_rsa";

        assert!(
            !looks_like_glob(CANONICAL),
            "the `?` in the verbatim prefix is not a wildcard"
        );
        assert!(
            matches!(
                &*denied_entry_normalized(&DeniedPath::operator_spelled(CANONICAL)),
                DeniedEntry::Literal(_)
            ),
            "a canonical Windows entry must compile to a literal location, \
             or the deny it encodes matches nothing"
        );

        // Stripping the prefix must not disarm the shape test for an entry
        // that carries a real wildcard behind it.
        assert!(
            looks_like_glob(r"\\?\C:\Users\me\**\.env"),
            "a genuine wildcard after the prefix is still a pattern"
        );

        // Unprefixed entries are judged exactly as before.
        assert!(looks_like_glob("**/.env"));
        assert!(!looks_like_glob("/home/me/.ssh"));
    }

    /// A pattern entry answers only the DOWNWARD question. The upward twin
    /// returns `None` for it by design (see `contains_denied_descendant`), and
    /// this pins that so the gap stays a decision rather than a regression —
    /// while a literal entry keeps full two-direction coverage.
    #[test]
    fn glob_entries_are_downward_only_literals_are_two_directional() {
        let root = tempdir().unwrap();
        let proj = root.path().join("proj");
        fs::create_dir(&proj).unwrap();
        let env = proj.join(".env");
        fs::write(&env, b"TOKEN=1").unwrap();
        let proj_c = proj.canonicalize().unwrap();
        let env_c = env.canonicalize().unwrap();

        // Downward: the pattern denies the file itself.
        assert!(path_is_denied(
            &env_c,
            &[DeniedPath::operator_spelled("**/.env")]
        ));
        // Upward: the pattern cannot name a protected location under `proj`.
        assert_eq!(
            contains_denied_descendant(&proj_c, &[DeniedPath::operator_spelled("**/.env")]),
            None,
            "a glob is a predicate, not a location — see the doc comment"
        );
        // A literal entry naming the same file still answers upward.
        assert_eq!(
            contains_denied_descendant(&proj_c, &[DeniedPath::literal(env.to_string_lossy())]),
            Some(env_c),
            "literal entries must keep two-direction coverage"
        );
    }

    /// An uncompilable pattern denies nothing (matching the OS floor, which
    /// drops patterns whose regex will not compile) and must not poison the
    /// literal entries sitting beside it in the same list.
    #[test]
    fn inert_glob_entry_denies_nothing_and_does_not_break_the_list() {
        let root = tempdir().unwrap();
        let vault = root.path().join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let plain = root.path().join("notes.txt");
        fs::write(&plain, b"hi").unwrap();

        // `[z-a]` translates to a syntactically valid glob class but an
        // invalid regex range.
        let denied = vec![
            DeniedPath::operator_spelled("[z-a]"),
            DeniedPath::operator_spelled("**/.env"),
            DeniedPath::literal(vault.to_string_lossy()),
        ];
        assert!(
            matches!(
                &*denied_entry_normalized(&DeniedPath::operator_spelled("[z-a]")),
                DeniedEntry::InertGlob
            ),
            "an uncompilable pattern must land in the inert state, not silently \
             become a literal"
        );
        assert!(check_and_resolve_path(&vault, &denied, None).is_err());
        assert!(check_and_resolve_path(&plain, &denied, None).is_ok());
    }

    /// The config reader is a narrow, hermetic parse of one array — no
    /// `Config::load()` (which writes a default file) and no dependency on the
    /// developer's real `~/.aleph/config.toml`.
    #[test]
    fn parse_deny_read_globs_reads_the_sandbox_array_only() {
        let toml = r#"
[gateway]
host = "127.0.0.1"

[sandbox]
enabled = true
deny_read_globs = ["**/.env", "**/*.pem"]
"#;
        assert_eq!(
            parse_deny_read_globs(toml),
            vec!["**/.env".to_string(), "**/*.pem".to_string()]
        );
        // Absent section / absent key / wrong shape → empty, never a panic.
        assert!(parse_deny_read_globs("[gateway]\nhost = \"x\"\n").is_empty());
        assert!(parse_deny_read_globs("[sandbox]\nenabled = true\n").is_empty());
        assert!(parse_deny_read_globs("this is not toml {{{").is_empty());
        // Non-string / empty entries are dropped, the rest survive.
        assert_eq!(
            parse_deny_read_globs("[sandbox]\ndeny_read_globs = [\"**/.env\", 7, \"\"]\n"),
            vec!["**/.env".to_string()]
        );
    }

    /// A `~` entry compiled under one home must not be served under another.
    /// Keyed by the raw spelling, step 1 compiled `~/.ssh` as A's and step 2
    /// judged B's `.ssh` against it: not refused. That was the P4 phase-end
    /// red of `gateway::handlers::fs`'s credential parity test, where a
    /// fixture that moves `$HOME` compiled the entry first.
    #[test]
    fn a_home_entry_compiled_under_one_home_is_not_served_under_another() {
        use crate::runtimes::post_install::HomeEnvGuard;
        let entries = [DeniedPath::template("~/.ssh")];
        let (dir_a, dir_b) = (tempdir().unwrap(), tempdir().unwrap());
        let a = dir_a.path().canonicalize().unwrap();
        let b = dir_b.path().canonicalize().unwrap();
        let under = |home: &Path| home.join(".ssh").join("id_ed25519");

        {
            let _home = HomeEnvGuard::acquire_and_set(&a);
            assert!(
                path_is_denied(&under(&a), &entries),
                "home A's ~/.ssh was judged against an earlier home's compile"
            );
        }
        let _home = HomeEnvGuard::acquire_and_set(&b);
        assert!(
            path_is_denied(&under(&b), &entries),
            "home B's ~/.ssh was judged against home A's compile"
        );
        assert!(
            !path_is_denied(&under(&a), &entries),
            "home A's ~/.ssh is still refused under home B"
        );
    }

    /// A `$HOME` whose path carries glob metacharacters is still a literal
    /// location. Whether an entry is a pattern is a property of how the
    /// operator spelled it (`~/.ssh`), never of what it expands to: classified
    /// on the expansion, `[1]` in the home's name became a character class, so
    /// the home's own `~/.ssh` stopped being refused and a sibling directory
    /// the class happens to match was refused in its place.
    #[test]
    fn a_home_with_glob_metacharacters_is_still_a_literal_location() {
        use crate::runtimes::post_install::HomeEnvGuard;
        let entries = [DeniedPath::template("~/.ssh")];
        let parent = tempdir().unwrap();
        let parent_path = parent.path().canonicalize().unwrap();
        let under = |home: &Path| home.join(".ssh").join("id_ed25519");
        // (home directory name, a sibling only its metacharacters match)
        let mut cases = vec![("a[1]b", "a1b")];
        if cfg!(unix) {
            // Not legal in a Windows file name.
            cases.push(("a*b", "a-anything-b"));
            cases.push(("a?b", "aqb"));
        }

        for (name, sibling) in cases {
            let home = parent_path.join(name);
            std::fs::create_dir(&home).unwrap();
            let _home = HomeEnvGuard::acquire_and_set(&home);
            assert!(
                path_is_denied(&under(&home), &entries),
                "HOME {name:?}: its own ~/.ssh is not refused"
            );
            assert!(
                !path_is_denied(&under(&parent_path.join(sibling)), &entries),
                "HOME {name:?}: {sibling:?}'s .ssh is refused because HOME's name was read as a pattern"
            );
        }
    }

    /// Code-built entries are literal locations, whatever their path contains.
    ///
    /// `format!("{}/secrets.vault", config_dir.display())` is not a spelling
    /// anyone chose; it is an expansion of `$ALEPH_HOME`. Read by shape, a home
    /// under `h[1]` made every such entry a pattern whose `[1]` is a character
    /// class: the vault, `data/`, the human-gate files and the pre-grant roots
    /// stopped being refused, and an `h1` sibling was refused in their place.
    /// Every target is judged by the production pair, `get_denied_paths` →
    /// `path_is_denied`, and every miss is collected so the list is the answer.
    #[test]
    fn code_built_entries_are_literal_locations_under_any_home() {
        use crate::runtimes::post_install::HomeEnvGuards;
        let parent = tempdir().unwrap();
        let parent_path = parent.path().canonicalize().unwrap();
        // (home directory name, a sibling only its metacharacters match)
        let mut cases = vec![("h[1]", "h1")];
        if cfg!(unix) {
            // Not legal in a Windows file name.
            cases.push(("h*", "h-anything"));
            cases.push(("h?", "hq"));
        }

        let mut wrong = Vec::new();
        for (name, sibling) in cases {
            let home = parent_path.join(name);
            // Any directory will do for `ALEPH_HOME`; the real name would trip
            // `utils::paths::tests::no_hand_rolled_aleph_home_outside_the_allowlist`.
            let aleph = home.join("aleph-home");
            fs::create_dir_all(aleph.join("data")).unwrap();
            let _env = HomeEnvGuards::acquire_and_set(&aleph, &home);
            // Each gate file from the store that owns it, as production names it.
            let targets = [
                ("secrets.vault", aleph.join("secrets.vault")),
                ("data/x.db", aleph.join("data").join("x.db")),
                (
                    "shell-hooks-allowlist.json",
                    crate::extension::hooks::ShellHookConsent::default_path(),
                ),
                (
                    "approval-grants.json",
                    crate::sandbox::exec_approval::grants::GrantStore::default_path(),
                ),
                (
                    "approval-policy.json",
                    crate::approval::ConfigApprovalPolicy::config_path(),
                ),
                (
                    "data/plugins.toml",
                    aleph
                        .join("data")
                        .join(crate::extension::plugin_state::PLUGINS_CONFIG_FILE),
                ),
                (
                    "~/.claude/skills/s/SKILL.md",
                    home.join(".claude")
                        .join("skills")
                        .join("s")
                        .join("SKILL.md"),
                ),
                (
                    "~/.claude/plugins/p/plugin.json",
                    home.join(".claude")
                        .join("plugins")
                        .join("p")
                        .join("plugin.json"),
                ),
                // Control: a `~` template, already literal before this test.
                ("~/.ssh/id_ed25519", home.join(".ssh").join("id_ed25519")),
            ];
            let denied = get_denied_paths();
            for (label, target) in targets {
                if !path_is_denied(&target, &denied) {
                    wrong.push(format!("HOME {name:?}: {label} is not refused"));
                }
                let twin = parent_path.join(sibling).join(
                    target
                        .strip_prefix(&home)
                        .expect("each target lives under HOME"),
                );
                if path_is_denied(&twin, &denied) {
                    wrong.push(format!(
                        "HOME {name:?}: sibling {sibling:?}'s {label} is refused"
                    ));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "a code-built denylist entry was read as a pattern:\n{}",
            wrong.join("\n")
        );
    }

    /// The kind of every entry `get_denied_paths` builds is Literal, under a
    /// home whose path is a glob. Only an operator's `deny_read_globs` entry
    /// may be a pattern — and this test's `ALEPH_HOME` has no config file, so
    /// the process-wide operator snapshot is either empty or another test's;
    /// it is excluded by value, not by shape.
    #[test]
    fn every_code_built_entry_is_a_literal() {
        use crate::runtimes::post_install::HomeEnvGuards;
        let parent = tempdir().unwrap();
        let home = parent.path().canonicalize().unwrap().join("h[1]");
        let aleph = home.join("aleph-home");
        fs::create_dir_all(&aleph).unwrap();
        let _env = HomeEnvGuards::acquire_and_set(&aleph, &home);
        let operator = configured_deny_read_globs();
        let denied = get_denied_paths();
        let patterns: Vec<&str> = denied
            .iter()
            .filter(|e| e.is_pattern() && !operator.contains(*e))
            .map(DeniedPath::as_str)
            .collect();
        assert!(
            patterns.is_empty(),
            "code-built entries classified as patterns: {patterns:?}"
        );
    }

    /// Pin (P5.11 / P4.18 concern 2): the operator `deny_read_globs` snapshot
    /// is ONE per process, latched by whichever test first reaches
    /// `get_denied_paths` under whatever `$ALEPH_HOME` was current. A second
    /// home with its own `[sandbox] deny_read_globs` is NOT read. Production
    /// is unaffected (`$ALEPH_HOME` is constant and `[sandbox]` is
    /// restart-scoped); the pin exists so a future test that writes a config
    /// and expects the file tools to honour its globs fails here, with the
    /// reason, instead of passing or failing by test order. Test it through
    /// `parse_deny_read_globs`, which is per-document.
    #[test]
    fn the_operator_glob_snapshot_is_process_wide_not_per_home() {
        use crate::runtimes::post_install::HomeEnvGuards;
        let parent = tempdir().unwrap();
        let aleph = parent.path().join("aleph");
        fs::create_dir_all(&aleph).unwrap();
        let _env = HomeEnvGuards::acquire_and_set(&aleph, parent.path().join("home"));
        let before = configured_deny_read_globs();
        let config = crate::config::Config::effective_path();
        assert!(
            config.starts_with(&aleph),
            "refusing to write a config outside the test's tempdir: {}",
            config.display()
        );
        fs::write(
            &config,
            "[sandbox]\ndeny_read_globs = [\"**/p511-only-here\"]\n",
        )
        .unwrap();
        let after = configured_deny_read_globs();
        assert!(
            std::ptr::eq(before, after),
            "the snapshot was re-read: it is documented as once per process"
        );
        assert!(
            !after.iter().any(|e| e.as_str() == "**/p511-only-here"),
            "a config written under a later $ALEPH_HOME reached the snapshot"
        );
    }

    /// A code-built entry is matched as built, never re-expanded.
    ///
    /// `ALEPH_HOME=~/<x>` reaches the process unexpanded when systemd,
    /// launchd or `docker -e` sets it, and Aleph then keeps its state under
    /// `<cwd>/~/<x>`: the stores open the relative path they are given. The
    /// denylist re-read the entry's leading `~` as a template and protected
    /// `$HOME/<x>` instead — a file that is not Aleph's — while the real vault,
    /// the auth DBs and the human-gate files were not refused.
    ///
    /// No file is created under the working directory. Where the stores
    /// open is derived independently of the entry under test —
    /// `std::path::absolute(get_config_dir())`, the same relative path the
    /// stores are handed, resolved against the working directory — and the
    /// vault entry must name exactly that; every real location is then judged
    /// by the production pair.
    #[test]
    fn a_code_built_entry_is_never_re_expanded() {
        use crate::runtimes::post_install::HomeEnvGuards;
        let home = tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let tag = format!(
            "p419f-{}",
            home_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .trim_start_matches('.')
        );
        // (the unexpanded `ALEPH_HOME`, what a re-expansion would read its
        // token as — resolved while the guard holds the environment)
        let mut cases: Vec<(String, fn() -> Option<PathBuf>)> =
            vec![(format!("~/{tag}"), dirs::home_dir)];
        if cfg!(windows) {
            cases.push((format!("%USERPROFILE%\\{tag}"), || {
                std::env::var_os("USERPROFILE").map(PathBuf::from)
            }));
        }

        let mut wrong = Vec::new();
        for (spelled, token) in cases {
            let _env = HomeEnvGuards::acquire_and_set(&spelled, &home_path);
            let token_target = token().map(|t| t.join(&tag));
            let denied = get_denied_paths();
            // Where the stores open: the relative config dir they are handed,
            // resolved against the working directory. Read right beside
            // `get_denied_paths()`, and never from the entry under test.
            let real_home =
                std::path::absolute(crate::utils::paths::get_config_dir().unwrap()).unwrap();
            let vault_entry = denied
                .iter()
                .map(DeniedPath::as_str)
                .find(|d| Path::new(d).ends_with(Path::new(&spelled).join("secrets.vault")))
                .expect("the vault entry names the configured home")
                .to_string();
            if Path::new(&vault_entry) != real_home.join("secrets.vault") {
                wrong.push(format!(
                    "{spelled}: the vault entry is {vault_entry}, but the store opens {}",
                    real_home.join("secrets.vault").display()
                ));
            }
            for leaf in [
                "secrets.vault",
                "data/x.db",
                "shell-hooks-allowlist.json",
                "approval-grants.json",
                "approval-policy.json",
                "data/plugins.toml",
                "skills/s/SKILL.md",
                "plugins/p/plugin.json",
            ] {
                let real = safe_normalize(&real_home.join(leaf)).unwrap();
                if !path_is_denied(&real, &denied) {
                    wrong.push(format!("{spelled}: the real {leaf} is not refused"));
                }
                let Some(decoy) = token_target.as_ref().map(|t| t.join(leaf)) else {
                    continue;
                };
                if path_is_denied(&decoy, &denied) {
                    wrong.push(format!(
                        "{spelled}: {} is refused: the entry was re-expanded to a file that is \
                         not Aleph's",
                        decoy.display()
                    ));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Under an ordinary absolute home, no code-built location names a `~`
    /// or `%VAR%` component.
    ///
    /// `DeniedPath::literal` is `pub` and never expands, and a relative
    /// `ALEPH_HOME` legitimately reaches it with a leading `~`, so it cannot
    /// refuse one at runtime. A `~/…` credential constant routed through it
    /// instead of `template` would protect `<cwd>/~/…` and leave the real
    /// `~/…` readable. Today's twelve are also named, one by one, in
    /// `file_ops::tests::test_check_path_denies_protected`; a NEW constant
    /// is named by no lookup test, and this is where that mistake shows.
    #[test]
    fn no_code_built_location_names_a_home_or_env_token() {
        use crate::runtimes::post_install::HomeEnvGuards;
        let home = tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let aleph = home_path.join("aleph-home");
        let _env = HomeEnvGuards::acquire_and_set(&aleph, &home_path);
        let denied = get_denied_paths();
        let tokenised: Vec<&str> = denied
            .iter()
            .filter(|e| e.kind == DeniedKind::Location)
            .filter(|e| {
                let path = Path::new(e.as_str());
                !path.is_absolute()
                    || path.components().any(|c| {
                        let name = c.as_os_str().to_string_lossy();
                        name == "~" || names_an_unexpanded_token(&name)
                    })
            })
            .map(DeniedPath::as_str)
            .collect();
        assert!(
            tokenised.is_empty(),
            "a location carries a `~` / `%VAR%` component under an absolute home — a \
             template constant was built with `DeniedPath::literal`: {tokenised:?}"
        );
    }

    /// A literal and a pattern whose texts coincide keep separate compiles.
    ///
    /// `~/.ssh` under `HOME=<p>/home[1]` expands to `<p>/home[1]/.ssh`, which
    /// is also the exact text of an operator glob naming `<p>/home1/.ssh` by
    /// a character class. One shared memo would serve whichever compiled
    /// first to both: the literal would stop refusing its own directory, or
    /// the pattern would stop refusing `home1`. Both entry orders are
    /// checked, since the first compile wins.
    #[cfg(unix)]
    #[test]
    fn a_literal_and_a_pattern_with_one_text_keep_separate_compiles() {
        use crate::runtimes::post_install::HomeEnvGuard;
        let parent = tempdir().unwrap();
        let parent_path = parent.path().canonicalize().unwrap();
        for (i, literal_first) in [true, false].into_iter().enumerate() {
            // A fresh home per order, so neither order inherits the other's memo.
            let home = parent_path.join(format!("home{i}[1]"));
            let class_match = parent_path.join(format!("home{i}1"));
            fs::create_dir_all(home.join(".ssh")).unwrap();
            fs::create_dir_all(class_match.join(".ssh")).unwrap();
            let _home = HomeEnvGuard::acquire_and_set(&home);
            let literal = DeniedPath::template("~/.ssh");
            let pattern = DeniedPath::operator_spelled(expand_denied_entry("~/.ssh"));
            assert!(pattern.is_pattern(), "the operator's `[1]` is a class");
            assert_eq!(pattern.as_str(), home.join(".ssh").to_string_lossy());
            let entries = if literal_first {
                [literal, pattern]
            } else {
                [pattern, literal]
            };
            assert!(
                path_is_denied(&home.join(".ssh").join("id_ed25519"), &entries),
                "literal_first={literal_first}: the literal ~/.ssh lost its own directory"
            );
            assert!(
                path_is_denied(&class_match.join(".ssh"), &entries),
                "literal_first={literal_first}: the pattern stopped matching its class"
            );
        }
    }

    /// An operator's `deny_read_globs` entry keeps the shape rule: a
    /// wildcard is a pattern, and so is a literal `[` (a character class —
    /// the operator wrote a glob list). A metacharacter-free entry stays a
    /// location. `**/*.pem` still refuses a nested key.
    #[test]
    fn operator_entries_keep_the_shape_rule() {
        assert!(DeniedPath::operator_spelled("**/*.pem").is_pattern());
        assert!(DeniedPath::operator_spelled("/data/[archive]").is_pattern());
        assert!(!DeniedPath::operator_spelled("/srv/app/secrets").is_pattern());
        assert!(!DeniedPath::literal("/data/[archive]").is_pattern());
        // A metacharacter-free operator entry is expanded as it always was;
        // a code-built location never is, whatever it starts with.
        assert_eq!(
            DeniedPath::operator_spelled("~/private").kind,
            DeniedKind::Template
        );
        let built = DeniedPath::literal("~/aleph-home/secrets.vault");
        assert_eq!(built.kind, DeniedKind::Location);
        assert!(
            Path::new(built.as_str()).is_absolute(),
            "a relative location is made absolute where it is built: {}",
            built.as_str()
        );

        let root = tempdir().unwrap();
        let key = root.path().join("a").join("b").join("k.pem");
        fs::create_dir_all(key.parent().unwrap()).unwrap();
        fs::write(&key, b"-----BEGIN-----").unwrap();
        assert!(path_is_denied(
            &key.canonicalize().unwrap(),
            &[DeniedPath::operator_spelled("**/*.pem")]
        ));
    }

    /// Only a template's key is an expansion; a location's is its spelling.
    ///
    /// On unix a built location cannot start with `~` once it is absolute,
    /// and `%…%` expansion is Windows-only, so no lookup there can tell a
    /// re-expanded location from one matched as built — only this pin and the
    /// Windows case of `a_code_built_entry_is_never_re_expanded` can.
    #[test]
    fn only_a_template_key_is_an_expansion() {
        let _env = crate::runtimes::post_install::HomeEnvGuards::acquire();
        for spelling in ["~/x/secrets.vault", "%USERPROFILE%\\x\\secrets.vault"] {
            assert_eq!(
                denied_entry_key(spelling, DeniedKind::Location),
                spelling,
                "a location is keyed as built"
            );
            assert_eq!(
                denied_entry_key(spelling, DeniedKind::Pattern),
                spelling,
                "a pattern is keyed as written"
            );
        }
        assert_eq!(
            denied_entry_key("~/x", DeniedKind::Template),
            expand_denied_entry("~/x"),
            "a template is keyed by its expansion"
        );
        assert_ne!(
            denied_entry_key("~/x", DeniedKind::Template),
            "~/x",
            "`~` expands wherever a home resolves"
        );
    }

    /// An operator entry that names `~` or `%VAR%` says so once, where it is
    /// made, whichever kind it is: the OS sandbox expands neither, so a
    /// pattern is dead on both faces and a template binds the file tools
    /// alone.
    #[test]
    fn an_unexpandable_operator_entry_warns_once() {
        // The template cases expand `~` when they compile.
        let _env = crate::runtimes::post_install::HomeEnvGuards::acquire();
        #[derive(Clone, Default)]
        struct Sink(crate::sync_primitives::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let warned = |spelling: &str| {
            let sink = Sink::default();
            let writer = sink.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || writer.clone())
                .with_max_level(tracing::Level::WARN)
                .with_ansi(false)
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                // Through the production memo, twice: the second is a hit.
                let entry = DeniedPath::operator_spelled(spelling);
                denied_entry_normalized(&entry);
                denied_entry_normalized(&entry);
            });
            let text = String::from_utf8_lossy(&sink.0.lock().unwrap_or_else(|e| e.into_inner()))
                .into_owned();
            text.matches("the OS sandbox does not expand this entry")
                .count()
        };
        // Unique texts, so the memo compiles each here and not in another test.
        let tag = tempdir().unwrap();
        let tag = tag
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            warned(&format!("~/.{tag}/**")),
            1,
            "a `~` glob must warn once"
        );
        assert_eq!(
            warned(&format!("%APPDATA%/{tag}/*")),
            1,
            "a `%VAR%` glob must warn once"
        );
        assert_eq!(
            warned(&format!("~/{tag}-private")),
            1,
            "a `~` template must warn once too: the file tools expand it, the OS sandbox does not"
        );
        assert_eq!(
            warned(&format!("%APPDATA%/{tag}-private")),
            1,
            "a `%VAR%` template must warn once too"
        );
        assert_eq!(
            warned(&format!("**/{tag}/*.pem")),
            0,
            "an ordinary glob must not"
        );
        assert_eq!(
            warned(&format!("/srv/{tag}/secrets")),
            0,
            "an ordinary location must not"
        );

        assert!(names_an_unexpanded_token("~/.config/**/token"));
        assert!(names_an_unexpanded_token("C:/%LOCALAPPDATA%/**"));
        assert!(!names_an_unexpanded_token("**/100%/*.txt"));
        assert!(!names_an_unexpanded_token("**/%%/*"));
        assert!(!names_an_unexpanded_token("/home/x/~backup/*"));
    }

    /// The shape test that tells a pattern entry from a concrete location.
    #[test]
    fn looks_like_glob_separates_patterns_from_paths() {
        assert!(looks_like_glob("**/.env"));
        assert!(looks_like_glob("/tmp/file?.txt"));
        assert!(looks_like_glob("/tmp/[abc].txt"));
        for literal in [
            "~/.ssh",
            "/etc/passwd",
            // Spelled without the real config-dir name on purpose:
            // `utils::paths::tests::no_hand_rolled_aleph_home_outside_the_allowlist`
            // is a FILE-level guard, and this module legitimately calls
            // `dirs::home_dir()` — naming that directory here would make the
            // pair look like a hand-rolled home resolution.
            "/Users/x/config-dir/secrets.vault",
            "%APPDATA%\\Microsoft\\Credentials",
        ] {
            assert!(
                !looks_like_glob(literal),
                "{literal} must stay a literal entry"
            );
        }
    }

    /// V4 guard: this repo has TWO path resolvers and the split is deliberate.
    ///
    /// `file_ops::path_utils::check_and_resolve_path` is a denylist with no
    /// jail (tilde/HOME expansion, `FsScope` anchoring, canonicalization,
    /// credential + glob denylist, `/proc` block, and — per the tool
    /// DESCRIPTION — absolute paths used as-is).
    /// `sandbox::workspace::path::normalize_path` is a jail with no denylist
    /// (lexical `..` popping *before* any syscall, no expansion, no denylist,
    /// hard containment enforced by its caller).
    ///
    /// Unifying them silently deletes one of the two answers, so this fails by
    /// name if either stops being the sole resolver for its own layer, or if a
    /// third resolver appears in either file.
    #[test]
    fn the_two_path_resolvers_stay_split() {
        // CRLF-safe: strip carriage returns FIRST, then split on an unanchored
        // needle (the bare attribute, no surrounding newlines) so a CRLF
        // checkout does not turn the "production prefix" into the whole file.
        fn production_prefix(src: &str) -> String {
            crate::utils::source_scan::production_prefix(src)
        }
        /// Every `fn` name in `src` whose name mentions resolving or
        /// normalizing — i.e. every candidate path resolver. Matches on a line
        /// that *starts* a definition (after visibility / `const` / `async` /
        /// `unsafe`), so prose mentioning a function name is not counted.
        fn resolver_fn_names(src: &str) -> Vec<String> {
            src.lines()
                .filter_map(|line| {
                    let mut rest = line.trim_start();
                    for prefix in [
                        "pub(crate) ",
                        "pub(super) ",
                        "pub ",
                        "const ",
                        "async ",
                        "unsafe ",
                    ] {
                        if let Some(stripped) = rest.strip_prefix(prefix) {
                            rest = stripped;
                        }
                    }
                    let name = rest.strip_prefix("fn ")?;
                    let name = name.split(['(', '<', ' ']).next().unwrap_or_default();
                    (name.contains("resolve") || name.contains("normaliz"))
                        .then(|| name.to_string())
                })
                .collect()
        }

        let file_layer_src = include_str!("path_utils.rs");
        let exec_layer_src = include_str!("../../sandbox/workspace/path.rs");
        let file_layer = production_prefix(file_layer_src);
        let exec_layer = production_prefix(exec_layer_src);

        // Non-vacuity: the split really removed this file's test module, and
        // both halves really are the files we think they are.
        assert!(
            file_layer.len() < file_layer_src.replace('\r', "").len(),
            "the cfg(test) split cut nothing off path_utils.rs — the needle drifted"
        );
        assert!(
            !file_layer.contains("fn the_two_path_resolvers_stay_split"),
            "this very test leaked into the production prefix"
        );
        assert!(
            exec_layer.contains("workspace sandbox"),
            "exec-layer source not found where expected"
        );

        // 1. Each layer has exactly the resolvers it is supposed to have. A new
        //    one — or a moved one — fails here, by name.
        let mut file_resolvers = resolver_fn_names(&file_layer);
        file_resolvers.sort();
        assert_eq!(
            file_resolvers,
            vec![
                "check_and_resolve_path",
                "denied_entry_normalized",
                "resolve_for_removal",
                "safe_normalize",
            ],
            "file-layer resolvers changed; if this is a new resolver, say which \
             of the two questions it answers before adding it"
        );
        let mut exec_resolvers = resolver_fn_names(&exec_layer);
        exec_resolvers.sort();
        assert_eq!(
            exec_resolvers,
            vec!["normalize_path"],
            "exec-layer resolvers changed; `normalize_path` must stay the only one"
        );

        // 2. The properties that make them different must survive. The exec
        //    layer must never grow a denylist (its caller's jail is the gate),
        //    and it must not start canonicalizing (its `..` popping is
        //    deliberately lexical and pre-syscall).
        for banned in ["path_is_denied", "denied_paths", "canonicalize("] {
            assert!(
                !exec_layer.contains(banned),
                "`{banned}` appeared in the exec-layer resolver: the jail does not \
                 get a denylist — that is the file layer's question"
            );
        }
        // And the file layer must not start jailing through the exec resolver.
        assert!(
            !file_layer.contains("normalize_path("),
            "the file layer must not call the exec-layer resolver; it has no \
             workspace root to jail against"
        );
        assert!(
            file_layer.contains("fn path_is_denied") && file_layer.contains("fs_scope"),
            "the file layer lost its denylist or its FsScope anchoring"
        );
    }

    /// The deny gate evaluates the FINAL (post-rebase) path — a rebase can
    /// never launder a denied target.
    #[tokio::test]
    async fn fs_scope_rebase_cannot_bypass_deny() {
        let repo = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::write(repo.path().join("secret.txt"), b"s").unwrap();
        let repo_c = repo.path().canonicalize().unwrap();
        let wt_c = wt.path().canonicalize().unwrap();

        // Deny the REBASED location only.
        let denied = vec![DeniedPath::literal(
            wt_c.join("secret.txt").to_string_lossy(),
        )];
        let scope = crate::tools::fs_scope::FsScope::worktree(wt_c, repo_c.clone());
        let input = repo_c.join("secret.txt");
        let result = crate::tools::fs_scope::with_fs_scope(Some(scope), async move {
            check_and_resolve_path(&input, &denied, None)
        })
        .await;
        assert!(
            result.is_err(),
            "deny must apply to the post-rebase target, got {result:?}"
        );
    }
}
