//! Skill template processor
//!
//! Renders a command or skill body with the Claude Code / pi prompt-template
//! grammar ([`SkillTemplate::render`] has the order):
//! - `$1 … $N`, `${N:-default}` - positional arguments ([`split_arguments`])
//! - `$ARGUMENTS`, `$@` - the whole argument string
//! - `` !`cmd` `` - inline shell, run by the caller's [`InlineShell`] with the
//!   arguments as data ([`inline_shell_command`])
//! - `@./path` - relative file reference (from skill directory), expanded only
//!   where the `@` is the template's own text; one that cannot be read stays
//!   as written. An argument may complete the path (`@./$1`), never make it
//!   absolute or leave the directory
//! - `@/path` - absolute file reference: rejected (left as written)

use super::error::{ExtensionError, ExtensionResult};
use super::hooks::{
    bounded_env_value, plugin_shell_line, PLUGIN_DATA_VARIABLES, PLUGIN_ROOT_VARIABLES,
};
use super::plugin_vars::PluginVars;
use once_cell::sync::OnceCell;
use regex::Regex;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

/// Regex for matching file references: @./path or @/path
/// Matches @./relative/path or @/absolute/path, stopping at whitespace or common delimiters
static FILE_REF_REGEX: OnceCell<Regex> = OnceCell::new();

/// Maximum bytes a single `@./file` reference expands to. Keeps a skill that
/// references a large file from inflating memory and the model context
/// window (mirrors the hook executor's `MAX_HOOK_OUTPUT_BYTES` cap).
const MAX_FILE_REF_BYTES: usize = 64 * 1024;

/// Maximum number of `@./file` references expanded per render.
const MAX_FILE_REFS: usize = 32;

/// Returns the compiled file-reference regex, initializing it on first use.
fn file_ref_regex() -> ExtensionResult<&'static Regex> {
    FILE_REF_REGEX.get_or_try_init(|| {
        // Pattern: @./path or @/path, stopping at whitespace or delimiters.
        // The regex is a compile-time constant; a parse failure is a programmer error.
        Regex::new(r#"@(\.?/[^\s\]\)>`"']+)"#).map_err(|e| {
            ExtensionError::template_error(format!("Invalid file reference regex: {e}"))
        })
    })
}

/// `${N:-default}`, `$N`, `$ARGUMENTS` and `$@` — one alternation, so a single
/// pass substitutes all four and never rescans a value it inserted.
static ARGUMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{(\d+):-([^}]*)\}|\$(\d+)|\$ARGUMENTS|\$@")
        .expect("hardcoded argument regex must compile")
});

/// `` !`cmd` `` — a backtick-fenced shell body after a bang.
static INLINE_SHELL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!`([^`]+)`").expect("hardcoded inline-shell regex must compile"));

/// Every `` !`cmd` `` in `content`, in document order: the byte range of the
/// whole span and the command it runs (trimmed).
///
/// The one place that decides which text of a body is shell source, on every
/// face that runs one: [`SkillTemplate::render`] (a command's body) and
/// `skill_read`'s preprocessor (`skill::preprocess`, a skill's body) run
/// exactly these, and the manifest adapter leaves exactly these unexpanded
/// (`AdapterRegistry::parse_dir`), so a plugin's install path never becomes
/// part of such a command's source. Both runners take the spans from the
/// text as written and never rescan what they insert around them.
#[must_use]
pub(crate) fn inline_commands(content: &str) -> Vec<(Range<usize>, &str)> {
    INLINE_SHELL_REGEX
        .captures_iter(content)
        .filter_map(|cap| Some((cap.get(0)?.range(), cap.get(1)?.as_str().trim())))
        .collect()
}

/// The environment variable an inline command reads the whole argument
/// string from (`$ARGUMENTS`).
const ARGUMENTS_VAR: &str = "ARGUMENTS";

/// The variable naming a skill's own directory: exported to a skill's inline
/// commands ([`InlineSite::skill_dir`]), and — spelled [`SKILL_DIR_TOKEN`] —
/// expanded as text in a skill's prose (`skill::preprocess`).
pub const SKILL_DIR_VARIABLE: &str = "ALEPH_SKILL_DIR";

/// [`SKILL_DIR_VARIABLE`] as a skill's prose spells it.
// rust-doctor-disable-next-line hardcoded-secrets
// Not a secret: this is the literal placeholder name used in skill templates.
pub(crate) const SKILL_DIR_TOKEN: &str = "${ALEPH_SKILL_DIR}";

/// The daemon's environment an inline command inherits: what a shell and the
/// programs it starts need to run — where programs are, whose home and
/// account, the shell, the locale (`LANG` and the POSIX `LC_*` categories —
/// not every `LC_` name: `LC_*` is how ssh forwards arbitrary variables), the
/// terminal, the temp directory, the time zone, the XDG base directories —
/// and how they reach the network (proxies, private CA bundles). Everything
/// else is cleared: provider API keys, channel bot tokens and Aleph's own
/// settings live in the daemon's environment, and whoever sends the
/// `/command` picks the arguments of a command whose output the model reads
/// (`` !`printenv $1` ``). An ssh agent socket is a credential channel too and
/// is not inherited; nor is `XDG_RUNTIME_DIR`, the directory of the user's
/// control sockets — its `bus` is where dbus falls back to without
/// `DBUS_SESSION_BUS_ADDRESS`, and it reaches the keyring (Secret Service)
/// and `systemctl --user`. A proxy URL can carry a credential; it is
/// inherited because an inline command runs only for an operator, whose
/// network it is.
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "TMPDIR",
    "TZ",
    // Locale.
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_COLLATE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    // XDG base directories — not `XDG_RUNTIME_DIR` (see above).
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    // Network: proxies (both spellings programs read) and private CAs.
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
    "ALL_PROXY",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];

/// On Windows, additionally what `cmd.exe` and most programs fail without:
/// the system root and drive, the command interpreter, the executable
/// extensions, and the temp and profile directories.
const INHERITED_ENV_WINDOWS: [&str; 9] = [
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "USERNAME",
];

/// Whether an inline command inherits the daemon's variable `name`
/// ([`INHERITED_ENV`]). Windows names are case-insensitive (`Path`).
fn inherited(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let name = if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_string()
    };
    INHERITED_ENV.contains(&name.as_str())
        || (cfg!(windows) && INHERITED_ENV_WINDOWS.contains(&name.as_str()))
}

/// Runs one `` !`cmd` `` body for [`SkillTemplate::render`].
///
/// The template knows nothing about consent, cwd or timeouts — the caller
/// (the gateway's slash-command seam) owns those and hands in an
/// implementation, which spawns [`inline_shell_command`] with the command's
/// [`InlineSite`]. `cmd` is the plugin's text as written and never contains
/// argument text; the arguments arrive beside it in `args`. `Err(text)` is
/// substituted verbatim: a withheld or failed expansion must be visible to the
/// model as such, never a silent blank.
#[async_trait::async_trait]
pub trait InlineShell: Send + Sync {
    async fn run(&self, cmd: &str, args: &InlineArgs<'_>) -> Result<String, String>;
}

/// The invocation's arguments as an inline command receives them: beside its
/// source, never inside it.
#[derive(Debug, Clone, Copy)]
pub struct InlineArgs<'a> {
    /// `$ARGUMENTS`: the argument string as typed.
    pub raw: &'a str,
    /// `$1 … $N`: [`split_arguments`] of `raw`, the same split the text
    /// around the inline commands is substituted from.
    pub positional: &'a [String],
}

/// What a render may reach beyond its own file: the shell for `` !`cmd` ``,
/// or `None` (every inline command is then withheld with a placeholder).
pub struct TemplateCtx<'a> {
    pub shell: Option<&'a dyn InlineShell>,
}

/// Positional arguments for `$1 … $N`: whitespace-split, quoting NOT
/// interpreted (Claude Code and pi do the same; a shell-quoted argument
/// reaches `$1` with its quotes).
#[must_use]
pub fn split_arguments(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

/// Where an inline command runs and which plugin ships it: what its twin, a
/// plugin's command hook, is told through the same variables
/// ([`command_hook_invocation`](crate::extension::hooks::command_hook_invocation)).
#[derive(Debug, Clone, Copy)]
pub struct InlineSite<'a> {
    /// The run's directory: `CLAUDE_PROJECT_DIR`, and the child's working
    /// directory unless the command is a skill's ([`Self::skill_dir`]). One
    /// value for a command, so the two cannot disagree there. There is no
    /// default: a caller that does not know it must not spawn.
    pub cwd: &'a Path,
    /// The plugin that ships the command: its id and its install root (not
    /// its `commands/` directory). Sets `CLAUDE_PLUGIN_ROOT` and every
    /// spelling of it and, for a valid plugin id, the `_DATA` pair. `None`:
    /// all of them are removed.
    pub plugin: Option<(&'a str, &'a Path)>,
    /// The skill whose body holds the command: the child runs in this
    /// directory and `ALEPH_SKILL_DIR` ([`SKILL_DIR_VARIABLE`]) names it.
    /// `None` (a command's body): the variable is removed, never inherited.
    pub skill_dir: Option<&'a Path>,
}

/// The process for one `` !`cmd` ``: `cmd` is the shell's source, the
/// arguments are its data.
///
/// On unix: `sh -c <cmd> sh <positional…>` with `ARGUMENTS=<raw>`, so `$1`,
/// `${2:-x}`, `$@`, `$#` and `$ARGUMENTS` are shell parameters — expanded as
/// values, never parsed: a `/cmd $(…)` is text to the command, not code.
/// `ARGUMENTS` is set even when empty, so the daemon's own value is never
/// inherited; past the hook executor's env cap it holds only the marker
/// `[N bytes — read from $1 … $N]` (an oversized value would make the spawn
/// fail for every inline command in the body) while the positional
/// parameters keep every word. Inside an inline command the shell's rules
/// apply, not the template's: `$10` is `${1}0` (write `${10}`), and a
/// single-quoted `'$1'` stays literal.
///
/// On Windows (`cmd /C`) there are no positional parameters, anything passed
/// after the command is appended to its line, and `%VAR%` is expanded before
/// the line is parsed — so no argument reaches an inline command there:
/// nothing is appended and `ARGUMENTS` is removed, not exported.
///
/// `cmd` holds the path variables as the plugin wrote them
/// (`${CLAUDE_PLUGIN_ROOT}/x`, `${ALEPH_SKILL_DIR}/x`): the manifest adapter
/// and the skill preprocessor leave every inline command unexpanded. On unix
/// they reach the shell through the environment only; on Windows `cmd`
/// cannot expand `${…}`, so they — and nothing else — are substituted into
/// the line: the plugin's by the one derivation a command hook uses
/// ([`plugin_shell_line`]), then the skill's directory.
///
/// The daemon's environment is cleared first, except what a shell and its
/// programs need ([`INHERITED_ENV`]) — unlike a plugin command hook, which
/// inherits all of it: an inline command's arguments are picked by whoever
/// sends the
/// `/command`, and its output is read by the model. On top of that, the
/// variables a hook is told: every spelling of the plugin root and data
/// directory, and `CLAUDE_PROJECT_DIR`, set from `site` when known and
/// removed when not — never the daemon's own value (a daemon launched from
/// inside a Claude Code session has them). The data directory is created
/// when the command names it, as for a hook.
///
/// The child runs in `site.cwd` — a skill's command in its skill directory —
/// with stdin closed (`/dev/null`) and is killed when the handle is dropped,
/// so a timeout does not orphan it. The caller sets stdout / stderr and the
/// timeout.
#[must_use]
pub fn inline_shell_command(
    cmd: &str,
    args: &InlineArgs<'_>,
    site: &InlineSite<'_>,
) -> tokio::process::Command {
    use crate::utils::no_window::NoWindow;
    // What the shell parses: the command hook's derivation — verbatim on
    // unix, the path variables substituted on Windows — and it creates the
    // data directory when the command names it.
    let line = plugin_shell_line(
        cmd,
        site.plugin.map(|(_, root)| root),
        site.plugin.map_or("", |(id, _)| id),
    );
    let line = match site.skill_dir {
        Some(dir) if cfg!(windows) => line.replace(SKILL_DIR_TOKEN, &dir.to_string_lossy()),
        _ => line,
    };
    let mut command = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| inherited(name)));
    if cfg!(windows) {
        command.args(["/C", line.as_str()]);
    } else {
        command
            .args(["-c", line.as_str(), "sh"])
            .args(args.positional)
            .env(
                ARGUMENTS_VAR,
                bounded_env_value(ARGUMENTS_VAR, args.raw, "$1 … $N"),
            );
    }
    // A data directory for plugin-owned commands only, as for a hook: a
    // label that is not a plugin id has none.
    let vars = site
        .plugin
        .filter(|(id, _)| crate::extension::manifest::validate_plugin_id(id).is_ok())
        .map(|(id, root)| PluginVars::new(id, root));
    set_or_remove(
        &mut command,
        &PLUGIN_ROOT_VARIABLES,
        site.plugin.map(|(_, root)| root),
    );
    set_or_remove(
        &mut command,
        &PLUGIN_DATA_VARIABLES,
        vars.as_ref().map(PluginVars::data_dir),
    );
    set_or_remove(&mut command, &[SKILL_DIR_VARIABLE], site.skill_dir);
    command
        .env("CLAUDE_PROJECT_DIR", site.cwd)
        .current_dir(site.skill_dir.unwrap_or(site.cwd))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .no_window();
    command
}

/// Every name in `names` set to `value`, or removed when there is none.
fn set_or_remove(command: &mut tokio::process::Command, names: &[&str], value: Option<&Path>) {
    for name in names {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// `${N:-default}`, `$N` (1-based; out of range → the default, else empty),
/// `$ARGUMENTS` and `$@` over one stretch of template text, in one pass.
/// Also returns the byte ranges of the result that are inserted argument
/// text: data, never read as template syntax again (a `@` there is not a
/// file reference).
fn substitute_arguments(text: &str, args: &InlineArgs<'_>) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut inserted = Vec::new();
    let mut last = 0;
    for cap in ARGUMENT_REGEX.captures_iter(text) {
        let Some(whole) = cap.get(0) else {
            continue;
        };
        out.push_str(text.get(last..whole.start()).unwrap_or_default());
        let start = out.len();
        out.push_str(&argument_value(&cap, args));
        inserted.push(start..out.len());
        last = whole.end();
    }
    out.push_str(text.get(last..).unwrap_or_default());
    (out, inserted)
}

/// The value one [`ARGUMENT_REGEX`] match stands for.
fn argument_value(cap: &regex::Captures<'_>, args: &InlineArgs<'_>) -> String {
    let (index, default) = match (cap.get(1), cap.get(3)) {
        (Some(n), _) => (n.as_str(), cap.get(2).map_or("", |d| d.as_str())),
        (None, Some(n)) => (n.as_str(), ""),
        // `$ARGUMENTS` / `$@`
        (None, None) => return args.raw.to_string(),
    };
    index
        .parse::<usize>()
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| args.positional.get(i))
        .map_or_else(|| default.to_string(), Clone::clone)
}

/// One inline command's expansion: its stdout, the runner's placeholder, or —
/// with no runner — a placeholder saying so. Every face's: a command body's
/// ([`SkillTemplate::render`]) and a skill body's (`skill::preprocess`).
pub(crate) async fn run_inline(cmd: &str, args: &InlineArgs<'_>, ctx: &TemplateCtx<'_>) -> String {
    match ctx.shell {
        Some(shell) => match shell.run(cmd, args).await {
            Ok(stdout) => stdout.trim_end().to_string(),
            Err(placeholder) => placeholder,
        },
        None => format!("[!`{cmd}` not run: no shell available for inline commands]"),
    }
}

/// Skill template processor
#[derive(Debug, Clone)]
pub struct SkillTemplate {
    /// Raw template content
    content: String,
    /// Base directory for relative paths
    base_dir: PathBuf,
}

impl SkillTemplate {
    /// Create a new template processor
    ///
    /// # Arguments
    /// * `content` - Raw skill content with template syntax
    /// * `source_path` - Path to the skill file (used to derive `base_dir`)
    #[must_use]
    pub fn new(content: &str, source_path: &Path) -> Self {
        let base_dir = source_path
            .parent()
            .map_or_else(|| PathBuf::from("."), |p| p.to_path_buf());

        Self {
            content: content.to_string(),
            base_dir,
        }
    }

    /// Create from content and explicit base directory
    #[must_use]
    pub fn with_base_dir(content: &str, base_dir: PathBuf) -> Self {
        Self {
            content: content.to_string(),
            base_dir,
        }
    }

    /// Get the base directory
    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Render with the Claude Code / pi prompt-template grammar:
    ///
    /// 1. Every `` !`cmd` `` is found in the TEMPLATE, before any argument is
    ///    inserted, and runs through `ctx.shell` — sequentially, in document
    ///    order — with the arguments as data ([`inline_shell_command`]).
    ///    Argument text therefore can neither become part of a command's
    ///    source nor spell an inline command of its own.
    /// 2. The text around them gets `${N:-default}`, `$N` (1-based over
    ///    [`split_arguments`]; out of range → the default, else empty),
    ///    `$ARGUMENTS` and `$@` in one pass.
    /// 3. `@./file` references are expanded in that text where the `@` is the
    ///    template's own: an argument may complete the path (`@./$1`), but an
    ///    `@` inside argument text or an inline command's output is data and
    ///    stays as it is. A reference that cannot be read — absolute,
    ///    escaping the base directory, missing — stays as written and is
    ///    logged; it does not fail the render.
    ///
    /// Claude Code and pi substitute the arguments into the whole body first,
    /// so `` !`gh pr view $1` `` runs with the argument spliced into its text.
    /// Here the command reads `$1` as a shell parameter: the same result for
    /// a plain argument (one with no shell syntax — no quotes, `~`, `$`,
    /// backtick or `;`, which reach the command as literal characters), text
    /// rather than code for one holding `$(…)`, `;` or a backtick.
    pub async fn render(&self, arguments: &str, ctx: &TemplateCtx<'_>) -> ExtensionResult<String> {
        let positional = split_arguments(arguments);
        let args = InlineArgs {
            raw: arguments,
            positional: &positional,
        };
        // Collected before the first `.await`: the match iterator is not held
        // across one.
        let commands = inline_commands(&self.content);
        let text = |range: Range<usize>| self.content.get(range).unwrap_or_default();
        // File references attempted so far, across every stretch of the render.
        let mut refs = 0;
        let mut out = String::with_capacity(self.content.len());
        let mut last = 0;
        for (whole, cmd) in commands {
            out.push_str(
                &self
                    .render_text(text(last..whole.start), &args, &mut refs)
                    .await?,
            );
            out.push_str(&run_inline(cmd, &args, ctx).await);
            last = whole.end;
        }
        out.push_str(
            &self
                .render_text(text(last..self.content.len()), &args, &mut refs)
                .await?,
        );
        Ok(out)
    }

    /// Steps 2 and 3 of [`Self::render`] over one stretch of template text.
    async fn render_text(
        &self,
        text: &str,
        args: &InlineArgs<'_>,
        refs: &mut usize,
    ) -> ExtensionResult<String> {
        let (substituted, inserted) = substitute_arguments(text, args);
        self.expand_file_refs(&substituted, &inserted, refs).await
    }

    /// Expand the file references in `content` whose `@` is template text —
    /// one starting inside an `inserted` range (argument text) is data and is
    /// left alone. A reference that cannot be resolved or read stays as
    /// written and is logged. `refs` counts the references attempted across
    /// the whole render; past [`MAX_FILE_REFS`] the render fails (the
    /// template's own references are the only ones counted).
    async fn expand_file_refs(
        &self,
        content: &str,
        inserted: &[Range<usize>],
        refs: &mut usize,
    ) -> ExtensionResult<String> {
        // Collected before the first `.await`, as in `render`.
        let mut found = Vec::new();
        for cap in file_ref_regex()?.captures_iter(content) {
            let (Some(full_match), Some(path)) = (cap.get(0), cap.get(1)) else {
                continue;
            };
            if inserted.iter().any(|r| r.contains(&full_match.start())) {
                continue;
            }
            if *refs >= MAX_FILE_REFS {
                return Err(ExtensionError::template_error(format!(
                    "too many file references (cap {MAX_FILE_REFS})"
                )));
            }
            *refs += 1;
            found.push((full_match.range(), path.as_str()));
        }

        let mut replacements = Vec::new();
        for (range, path_str) in found {
            let read = match self.resolve_path(path_str) {
                Ok(path) => self.read_file(&path).await,
                Err(e) => Err(e),
            };
            match read {
                Ok(file_content) => replacements.push((range, file_content)),
                Err(e) => tracing::warn!(
                    reference = content.get(range).unwrap_or_default(),
                    base_dir = %self.base_dir.display(),
                    error = %e,
                    "file reference not read; left as written"
                ),
            }
        }

        // Apply replacements in reverse order to preserve positions.
        // Use positional replacement (single occurrence) to avoid corrupting
        // file contents that may contain the same reference pattern.
        let mut result = content.to_string();
        for (range, replacement) in replacements.into_iter().rev() {
            result.replace_range(range, &replacement);
        }

        Ok(result)
    }

    /// Resolve a file path from the template syntax
    fn resolve_path(&self, path_str: &str) -> ExtensionResult<PathBuf> {
        let absolute = || {
            ExtensionError::file_reference(
                path_str,
                "Absolute paths are not allowed in file references; use relative paths (./path) instead",
            )
        };
        // Absolute paths are not allowed — they bypass base_dir containment
        if path_str.starts_with('/') {
            return Err(absolute());
        }
        // `./` only spells "relative"; what follows it must be relative too.
        // An argument completes `@./$1` into `@.//etc/passwd`: that remainder
        // is rooted, and `join` would REPLACE `base_dir` with it (a Windows
        // `C:\` is a `Prefix`).
        let relative = path_str.strip_prefix("./").unwrap_or(path_str);
        if Path::new(relative)
            .components()
            .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)))
        {
            return Err(absolute());
        }
        let resolved = self.base_dir.join(relative);
        // Security check: ensure the resolved path is within base_dir
        self.validate_path_security(&resolved)?;
        Ok(resolved)
    }

    /// Validate that a path doesn't escape the base directory (for relative paths)
    fn validate_path_security(&self, resolved: &Path) -> ExtensionResult<()> {
        // review(extension): use component check, not substring — a legitimate
        // filename like `notes..txt` would otherwise be falsely rejected as
        // traversal. The previous substring `..` check rejected every filename
        // containing two adjacent dots regardless of position.
        if resolved
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(ExtensionError::file_reference(
                resolved,
                "Path traversal (..) not allowed in relative file references",
            ));
        }

        // If the file exists, canonicalize and verify containment within
        // base_dir. A canonicalization that fails refuses rather than skips
        // the check: containment that cannot be shown was not shown — and
        // the base can be gone while its command is still registered (a
        // `plugin update` swap).
        if resolved.exists() {
            let (Ok(canonical_path), Ok(canonical_base)) =
                (resolved.canonicalize(), self.base_dir.canonicalize())
            else {
                return Err(ExtensionError::file_reference(
                    resolved,
                    "Cannot verify that the path stays inside the base directory",
                ));
            };
            if !canonical_path.starts_with(&canonical_base) {
                return Err(ExtensionError::file_reference(
                    resolved,
                    "Resolved path escapes the base directory",
                ));
            }
        }

        Ok(())
    }

    /// Read a file's content, capped at [`MAX_FILE_REF_BYTES`] with a
    /// truncation marker appended when the file exceeds the cap.
    async fn read_file(&self, path: &Path) -> ExtensionResult<String> {
        use tokio::io::AsyncReadExt;

        // Read at most cap+1 bytes so an oversized file is detected without
        // being loaded whole into memory.
        let file = tokio::fs::File::open(path).await.map_err(|e| {
            ExtensionError::file_reference(path, format!("Failed to open file: {e}"))
        })?;
        let mut buf = Vec::new();
        file.take(MAX_FILE_REF_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| {
                ExtensionError::file_reference(path, format!("Failed to read file: {e}"))
            })?;
        let truncated = buf.len() > MAX_FILE_REF_BYTES;
        if truncated {
            buf.truncate(MAX_FILE_REF_BYTES);
        }
        let mut content = String::from_utf8_lossy(&buf).into_owned();
        if truncated {
            content.push_str("\n...[truncated: file exceeds 64 KiB cap]");
        }
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn no_shell() -> TemplateCtx<'static> {
        TemplateCtx { shell: None }
    }

    #[test]
    fn test_arguments_substitution() {
        let template = SkillTemplate::new("Hello $ARGUMENTS!", Path::new("/test/skill"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(template.render("World", &no_shell())).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_multiple_arguments() {
        let template = SkillTemplate::new("$ARGUMENTS says $ARGUMENTS", Path::new("/test/skill"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(template.render("Hello", &no_shell())).unwrap();
        assert_eq!(result, "Hello says Hello");
    }

    #[tokio::test]
    async fn test_file_reference_relative() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        tokio::fs::write(&config_path, r#"{"key": "value"}"#)
            .await
            .unwrap();

        let template =
            SkillTemplate::with_base_dir("Config: @./config.json", temp.path().to_path_buf());

        let result = template.render("", &no_shell()).await.unwrap();
        assert_eq!(result, r#"Config: {"key": "value"}"#);
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: @/absolute file-ref syntax (a Windows C:\ path isn't matched)
    async fn test_file_reference_absolute_blocked() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        tokio::fs::write(&file_path, "Test content").await.unwrap();

        let body = format!("Content: @{}", file_path.display());
        let template = SkillTemplate::with_base_dir(&body, temp.path().to_path_buf());

        // Absolute paths are never read — even one inside the base directory.
        // Not an error either: the reference stays as written.
        let result = template.render("", &no_shell()).await.unwrap();
        assert_eq!(result, body, "an absolute reference was read");
    }

    /// Base dir `<tmp>/skill`, a real file at `<tmp>/outside.txt`: `..` does
    /// not reach it, and the render goes on with the reference as written.
    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("skill");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(temp.path().join("outside.txt"), "OUTSIDE").unwrap();
        let template = SkillTemplate::with_base_dir("Content: @./../outside.txt", base);

        let result = template.render("", &no_shell()).await.unwrap();
        assert_eq!(
            result, "Content: @./../outside.txt",
            "`..` escaped the base directory"
        );
    }

    /// A symlink inside the base directory that points out of it is not
    /// followed.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_symlink_out_of_the_base_directory_is_not_read() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("skill");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(temp.path().join("outside.txt"), "OUTSIDE").unwrap();
        std::os::unix::fs::symlink(temp.path().join("outside.txt"), base.join("link.txt")).unwrap();
        let template = SkillTemplate::with_base_dir("Content: @./link.txt", base);

        let result = template.render("", &no_shell()).await.unwrap();
        assert_eq!(
            result, "Content: @./link.txt",
            "a symlink escaped the base directory"
        );
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let template = SkillTemplate::with_base_dir(
            "Content: @./nonexistent.txt",
            PathBuf::from("/test/skill"),
        );

        let result = template.render("", &no_shell()).await.unwrap();
        assert_eq!(result, "Content: @./nonexistent.txt");
    }

    /// One reference that cannot be read does not cost the others, or the
    /// render: the `@/` import alias of a Next.js / Vite codebase, a missing
    /// file and a readable one, side by side.
    #[tokio::test]
    async fn an_unreadable_reference_stays_as_written_and_the_render_goes_on() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("ok.txt"), "OK").unwrap();
        let template = SkillTemplate::with_base_dir(
            "a @/lib/utils b @./missing.txt c @./ok.txt d",
            temp.path().to_path_buf(),
        );
        assert_eq!(
            template.render("", &no_shell()).await.unwrap(),
            "a @/lib/utils b @./missing.txt c OK d"
        );
    }

    /// The path of a template reference may come from an argument
    /// (`@./$1`, `@$1`): the `@` is the template's.
    #[tokio::test]
    async fn an_argument_may_complete_a_template_reference() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("notes.txt"), "NOTES").unwrap();
        let template = SkillTemplate::with_base_dir("[@./$1] [@$2]", temp.path().to_path_buf());
        assert_eq!(
            template
                .render("notes.txt ./notes.txt", &no_shell())
                .await
                .unwrap(),
            "[NOTES] [NOTES]"
        );
    }

    /// `<tmp>/skill` as the base with its own `inside.txt`, and a real
    /// `<tmp>/outside.txt` beside it: the tempdir, the base, the outside path.
    /// Both helpers serve only the `#[cfg(unix)]` path-escape tests below;
    /// gated to match so non-unix test builds stay warning-clean.
    #[cfg(unix)]
    fn escape_fixture() -> (TempDir, PathBuf, String) {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("skill");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(base.join("inside.txt"), "INSIDE").unwrap();
        std::fs::write(temp.path().join("outside.txt"), "OUTSIDE").unwrap();
        let outside = temp.path().join("outside.txt").display().to_string();
        (temp, base, outside)
    }

    /// Each `(template, arguments)` rendered against `base`, no shell.
    #[cfg(unix)]
    async fn render_each(base: &Path, cases: &[(&str, String)]) -> Vec<String> {
        let mut out = Vec::new();
        for (template, arguments) in cases {
            let t = SkillTemplate::with_base_dir(template, base.to_path_buf());
            out.push(t.render(arguments, &no_shell()).await.unwrap());
        }
        out
    }

    /// An argument that completes a template reference (`@./$1`, `@$1`)
    /// cannot take it out of the base directory — by `..`, or by an
    /// absolute path the `./` of the template runs into.
    #[tokio::test]
    #[cfg(unix)]
    async fn an_argument_completing_a_reference_stays_inside_the_base_directory() {
        let (_temp, base, outside) = escape_fixture();
        let cases = [
            ("@./$1", format!("./{outside}")),
            ("@$1", "./../outside.txt".to_string()),
            ("@./$1", outside.clone()),
            ("@$1", format!("./{outside}")),
        ];
        let rendered = render_each(&base, &cases).await;
        for (text, (template, arguments)) in rendered.iter().zip(&cases) {
            assert!(
                !text.contains("OUTSIDE"),
                "`{template}` + `{arguments}` read a file outside the base: {text}"
            );
            assert!(text.starts_with('@'), "not left as written: {text}");
        }
    }

    /// The command directory can be gone while its command is still
    /// registered (a `plugin update` swap): containment that cannot be
    /// checked is a refusal, and an argument-completed absolute path is
    /// refused before any check.
    #[tokio::test]
    #[cfg(unix)]
    async fn an_argument_completed_absolute_path_is_not_read_when_the_base_is_gone() {
        let (temp, _base, outside) = escape_fixture();
        let gone = temp.path().join("gone/commands");
        let cases = [("@./$1", outside.clone()), ("@$1", format!("./{outside}"))];
        for text in render_each(&gone, &cases).await {
            assert!(
                !text.contains("OUTSIDE"),
                "read through a missing base: {text}"
            );
        }
    }

    /// Absolute is refused as such, not only when it lands outside: the
    /// template's `@/…` is never read even inside the base
    /// (`test_file_reference_absolute_blocked`), and neither is one an
    /// argument spells after the template's `./`.
    #[tokio::test]
    #[cfg(unix)]
    async fn an_argument_completed_absolute_path_is_not_read_even_inside_the_base() {
        let (_temp, base, _outside) = escape_fixture();
        let inside = base.join("inside.txt").display().to_string();
        let rendered = render_each(&base, &[("@./$1", inside.clone())]).await;
        assert_eq!(rendered, [format!("@./{inside}")]);
    }

    /// An `@` the arguments bring is data: not read, even when the file is
    /// there.
    #[tokio::test]
    async fn a_file_reference_in_argument_text_is_not_read() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("notes.txt"), "NOTES").unwrap();
        let template =
            SkillTemplate::with_base_dir("Q: $ARGUMENTS [$1]", temp.path().to_path_buf());
        assert_eq!(
            template.render("@./notes.txt", &no_shell()).await.unwrap(),
            "Q: @./notes.txt [@./notes.txt]"
        );
    }

    #[test]
    fn test_file_ref_regex() {
        let content = "See @./config.json and @/etc/hosts for details.";
        let matches: Vec<_> = file_ref_regex().unwrap().find_iter(content).collect();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].as_str(), "@./config.json");
        assert_eq!(matches[1].as_str(), "@/etc/hosts");
    }

    #[tokio::test]
    async fn test_combined_template() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("settings.json");
        tokio::fs::write(&config_path, r#"{"name": "test"}"#)
            .await
            .unwrap();

        let template = SkillTemplate::with_base_dir(
            "User: $ARGUMENTS\nSettings: @./settings.json",
            temp.path().to_path_buf(),
        );

        let result = template.render("Alice", &no_shell()).await.unwrap();
        assert_eq!(result, "User: Alice\nSettings: {\"name\": \"test\"}");
    }

    #[tokio::test]
    async fn positional_and_default_arguments() {
        let t = SkillTemplate::new(
            "pr=$1 who=${2:-nobody} all=[$ARGUMENTS] again=[$@]",
            Path::new("/x/cmd.md"),
        );
        assert_eq!(
            t.render("123", &no_shell()).await.unwrap(),
            "pr=123 who=nobody all=[123] again=[123]"
        );
        assert_eq!(
            t.render("123 alice", &no_shell()).await.unwrap(),
            "pr=123 who=alice all=[123 alice] again=[123 alice]"
        );
        // `$10` is the tenth argument, not `$1` followed by `0`.
        let t = SkillTemplate::new("[$10][$1]", Path::new("/x/cmd.md"));
        assert_eq!(
            t.render("a b c d e f g h i j", &no_shell()).await.unwrap(),
            "[j][a]"
        );
        // Out of range with no default → empty, not the literal.
        let t = SkillTemplate::new("[$3]", Path::new("/x/cmd.md"));
        assert_eq!(t.render("a", &no_shell()).await.unwrap(), "[]");
    }

    /// An inserted value is data: one pass over the template, so an argument
    /// that spells `$ARGUMENTS` / `$@` / `$2` is not substituted again.
    #[tokio::test]
    async fn an_inserted_argument_is_never_read_as_template_syntax() {
        let t = SkillTemplate::new("[$1] [$ARGUMENTS] [$@]", Path::new("/x/cmd.md"));
        assert_eq!(
            t.render("$ARGUMENTS $2", &no_shell()).await.unwrap(),
            "[$ARGUMENTS] [$ARGUMENTS $2] [$ARGUMENTS $2]"
        );
    }

    /// One `run` call as the template made it.
    #[derive(Debug, PartialEq)]
    struct Call {
        cmd: String,
        raw: String,
        positional: Vec<String>,
    }

    struct RecordingShell(std::sync::Mutex<Vec<Call>>, Result<String, String>);

    impl RecordingShell {
        fn calls(&self) -> Vec<Call> {
            std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    #[async_trait::async_trait]
    impl InlineShell for RecordingShell {
        async fn run(&self, cmd: &str, args: &InlineArgs<'_>) -> Result<String, String> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push(Call {
                cmd: cmd.to_string(),
                raw: args.raw.to_string(),
                positional: args.positional.to_vec(),
            });
            self.1.clone()
        }
    }

    /// The command reaches the shell as the plugin wrote it; the arguments
    /// travel beside it. Claude Code splices them into the text first — a
    /// `/cmd $(…)` would then be shell source (see [`SkillTemplate::render`]).
    #[tokio::test]
    async fn inline_shell_receives_the_arguments_as_data_not_as_source() {
        let shell = RecordingShell(Default::default(), Ok("main\n".into()));
        let t = SkillTemplate::new(
            "Branch: !`git branch --show-current $1`.",
            Path::new("/x/cmd.md"),
        );
        let out = t
            .render(
                "--verbose",
                &TemplateCtx {
                    shell: Some(&shell),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "Branch: main.");
        assert_eq!(
            shell.calls(),
            [Call {
                cmd: "git branch --show-current $1".into(),
                raw: "--verbose".into(),
                positional: vec!["--verbose".into()],
            }]
        );
    }

    /// Argument text that spells an inline command stays text: only the
    /// plugin's own template can name a command to run.
    #[tokio::test]
    async fn argument_text_never_becomes_an_inline_command() {
        let shell = RecordingShell(Default::default(), Ok("RAN".into()));
        let t = SkillTemplate::new("Args: $ARGUMENTS / $1", Path::new("/x/cmd.md"));
        let out = t
            .render(
                "!`touch N`",
                &TemplateCtx {
                    shell: Some(&shell),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "Args: !`touch N` / !`touch");
        assert_eq!(
            shell.calls(),
            [],
            "an argument was run as an inline command"
        );
    }

    #[tokio::test]
    async fn withheld_shell_leaves_the_placeholder_in_place() {
        // Consent not given (or the command failed): the placeholder is what
        // the model reads, so a withheld expansion is visible, never blank.
        let shell = RecordingShell(
            Default::default(),
            Err("[withheld: pending approval]".into()),
        );
        let t = SkillTemplate::new("Files: !`git diff --name-only`", Path::new("/x/cmd.md"));
        let out = t
            .render(
                "",
                &TemplateCtx {
                    shell: Some(&shell),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "Files: [withheld: pending approval]");
    }

    #[tokio::test]
    async fn no_shell_means_every_inline_command_is_withheld() {
        let t = SkillTemplate::new("!`whoami` done", Path::new("/x/cmd.md"));
        let out = t.render("", &no_shell()).await.unwrap();
        assert!(out.starts_with("[!`whoami` not run"), "{out}");
        assert!(out.ends_with(" done"));
    }

    #[test]
    fn split_arguments_is_whitespace_only() {
        assert_eq!(split_arguments("  a   b\tc "), ["a", "b", "c"]);
        assert_eq!(
            split_arguments("\"a b\""),
            ["\"a", "b\""],
            "quoting is not interpreted (documented)"
        );
        assert!(split_arguments("").is_empty());
    }

    /// `ARGUMENTS` is set on every inline command — empty when there are
    /// none — so a daemon started with its own `ARGUMENTS` exported cannot
    /// hand that value to a command as if the user had typed it.
    #[test]
    #[cfg(unix)]
    fn the_arguments_variable_is_always_set_never_inherited() {
        let none: [String; 0] = [];
        let cmd = inline_shell_command(
            "true",
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd: Path::new("/"),
                plugin: None,
                skill_dir: None,
            },
        );
        let env: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(
            env.contains(&(
                std::ffi::OsStr::new("ARGUMENTS"),
                Some(std::ffi::OsStr::new(""))
            )),
            "{env:?}"
        );
    }

    /// The builder, not its caller, closes stdin and kills the child when the
    /// handle is dropped (a timeout must not orphan it), and runs it in the
    /// site's directory. `{:#?}` is the only reader std offers for a
    /// command's stdin; it prints the field only when it was set.
    #[test]
    fn the_builder_closes_stdin_and_kills_on_drop() {
        let none: [String; 0] = [];
        let cwd = std::env::temp_dir();
        let cmd = inline_shell_command(
            "true",
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd: &cwd,
                plugin: None,
                skill_dir: None,
            },
        );
        assert!(cmd.get_kill_on_drop(), "kill_on_drop is not set");
        // std offers a command's stdin only through its Debug output, and the
        // Windows Debug impl prints just the program line — there is no
        // cross-platform reader, so the stdin introspection is unix-only.
        #[cfg(unix)]
        {
            let shown = format!("{:#?}", cmd.as_std());
            assert!(
                shown.contains("stdin: Some(") && shown.contains("Null"),
                "stdin is not closed: {shown}"
            );
        }
        assert_eq!(cmd.as_std().get_current_dir(), Some(cwd.as_path()));
    }

    /// The production process builder behind a minimal runner: what P4.7c's
    /// consent-gated runner spawns, without the consent and the timeout.
    #[cfg(unix)]
    struct Sh(PathBuf);

    #[cfg(unix)]
    #[async_trait::async_trait]
    impl InlineShell for Sh {
        async fn run(&self, cmd: &str, args: &InlineArgs<'_>) -> Result<String, String> {
            let site = InlineSite {
                cwd: &self.0,
                plugin: None,
                skill_dir: None,
            };
            let out = inline_shell_command(cmd, args, &site)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
    }

    /// Output is data. A PR body read through `` !`gh pr view $1` `` is
    /// written by whoever opened the PR: an inline command, `$` syntax or a
    /// file reference in it is neither run, substituted nor read.
    #[tokio::test]
    async fn inline_output_is_never_rescanned() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("notes.txt"), "NOTES").unwrap();
        let shell = RecordingShell(
            Default::default(),
            Ok("!`touch X` $1 ${1:-d} $ARGUMENTS @./notes.txt @/lib/utils".into()),
        );
        let t = SkillTemplate::with_base_dir("[!`gh pr view $1`]", temp.path().to_path_buf());
        let out = t
            .render(
                "ARG",
                &TemplateCtx {
                    shell: Some(&shell),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            out,
            "[!`touch X` $1 ${1:-d} $ARGUMENTS @./notes.txt @/lib/utils]"
        );
        assert_eq!(
            shell.calls(),
            [Call {
                cmd: "gh pr view $1".into(),
                raw: "ARG".into(),
                positional: vec!["ARG".into()],
            }],
            "the output was run as an inline command"
        );
    }

    /// Past the env cap `ARGUMENTS` holds a marker that says where the words
    /// still are, and the positional parameters keep them all.
    #[tokio::test]
    #[cfg(unix)]
    async fn an_oversized_argument_string_is_a_marker_in_arguments() {
        let dir = TempDir::new().unwrap();
        let shell = Sh(dir.path().to_path_buf());
        let long = "a".repeat(40 * 1024);
        let t = SkillTemplate::new(
            r#"[!`printf '%s|%s|%s' "$ARGUMENTS" "${#1}" "$2"`]"#,
            Path::new("/x/cmd.md"),
        );
        let out = t
            .render(
                &format!("{long} b"),
                &TemplateCtx {
                    shell: Some(&shell),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "[[40962 bytes — read from $1 … $N]|40960|b]");
    }

    /// The daemon-side values an inline command must never see as its own.
    #[cfg(unix)]
    const DAEMON_ENV: [&str; 6] = [
        "PLUGIN_ROOT",
        "CLAUDE_PLUGIN_ROOT",
        "ALEPH_PLUGIN_ROOT",
        "CLAUDE_PLUGIN_DATA",
        "ALEPH_PLUGIN_DATA",
        "CLAUDE_PROJECT_DIR",
    ];

    /// Runs `printf` over [`DAEMON_ENV`] (`unset` when a name is not set) in
    /// `cwd` for `plugin`.
    #[cfg(unix)]
    async fn shown_env(cwd: &Path, plugin: Option<(&str, &Path)>) -> Vec<String> {
        let none: [String; 0] = [];
        let shown: Vec<String> = DAEMON_ENV
            .iter()
            .map(|key| format!("\"${{{key}-unset}}\""))
            .collect();
        let out = inline_shell_command(
            &format!("printf '%s\\n' {}", shown.join(" ")),
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd,
                plugin,
                skill_dir: None,
            },
        )
        .output()
        .await
        .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The site's plugin root, data directory and run directory reach the
    /// command under every spelling a plugin command hook gets.
    #[tokio::test]
    #[cfg(unix)]
    async fn the_path_variables_come_from_the_site() {
        let cwd = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let shown = shown_env(cwd.path(), Some(("plug", root.path()))).await;
        let root = root.path().to_string_lossy().to_string();
        assert_eq!(shown[..3], [root.clone(), root.clone(), root], "{shown:?}");
        for data in &shown[3..5] {
            assert!(
                Path::new(data).ends_with("data/plug"),
                "not the plugin's data directory: {shown:?}"
            );
        }
        assert_eq!(shown[5], cwd.path().to_string_lossy(), "{shown:?}");
    }

    /// The line the shell receives for a plugin's inline command, on this
    /// platform: its args after the program.
    fn shell_args(cmd: &str, root: &Path) -> Vec<String> {
        let none: [String; 0] = [];
        let cwd = std::env::temp_dir();
        inline_shell_command(
            cmd,
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd: &cwd,
                plugin: Some(("plug", root)),
                skill_dir: None,
            },
        )
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    /// On unix the plugin's text is the source, unchanged: the root reaches
    /// it through the environment (`the_path_variables_come_from_the_site`),
    /// never spliced into what `sh` parses.
    #[test]
    #[cfg(unix)]
    fn on_unix_an_inline_command_is_the_text_as_written() {
        let cmd = r#"cat "${CLAUDE_PLUGIN_ROOT}/x""#;
        assert_eq!(
            shell_args(cmd, Path::new("/r $(touch M)")),
            ["-c", cmd, "sh"]
        );
    }

    /// On Windows `cmd` cannot expand `${…}`: the path variables — and
    /// nothing else — are substituted into the line, as for a command hook
    /// (`plugin_shell_line`).
    #[test]
    #[cfg(windows)]
    fn on_windows_an_inline_command_gets_the_path_variables_substituted() {
        let root = Path::new(r"C:\plugins\plug");
        assert_eq!(
            shell_args(r"type ${CLAUDE_PLUGIN_ROOT}\x %ARGUMENTS%", root),
            ["/C", r"type C:\plugins\plug\x %ARGUMENTS%"]
        );
    }

    /// A skill's inline command: the text as written is the source (unix),
    /// the child runs in the skill's directory, `ALEPH_SKILL_DIR` names it —
    /// a directory named with a command substitution stays one word of data
    /// — and `CLAUDE_PROJECT_DIR` is the run's directory, not the skill's.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_skills_command_runs_in_its_dir_and_names_it_only_through_the_environment() {
        let tmp = TempDir::new().unwrap();
        let skill = tmp.path().join("s $(touch M)");
        std::fs::create_dir_all(&skill).unwrap();
        let run = tmp.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let none: [String; 0] = [];
        let cmd = r#"printf '%s\n' "${ALEPH_SKILL_DIR}" "$CLAUDE_PROJECT_DIR"; pwd -P"#;
        let mut command = inline_shell_command(
            cmd,
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd: &run,
                plugin: None,
                skill_dir: Some(&skill),
            },
        );
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-c", cmd, "sh"]);
        let out = command.output().await.unwrap();
        let shown = String::from_utf8_lossy(&out.stdout).to_string();
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(
            lines,
            [
                skill.to_string_lossy().as_ref(),
                run.to_string_lossy().as_ref(),
                std::fs::canonicalize(&skill)
                    .unwrap()
                    .to_string_lossy()
                    .as_ref(),
            ],
            "{shown}"
        );
        assert!(!skill.join("M").exists() && !run.join("M").exists());
    }

    /// A command body's inline command gets no `ALEPH_SKILL_DIR`, even when
    /// the daemon has one.
    #[tokio::test]
    #[cfg(unix)]
    #[serial_test::serial] // writes process env a spawned child reads
    async fn a_commands_inline_command_does_not_inherit_a_skill_dir() {
        struct DaemonEnv;
        impl Drop for DaemonEnv {
            fn drop(&mut self) {
                std::env::remove_var(SKILL_DIR_VARIABLE);
            }
        }
        let _daemon = DaemonEnv;
        std::env::set_var(SKILL_DIR_VARIABLE, "from-the-daemon");
        let cwd = TempDir::new().unwrap();
        let none: [String; 0] = [];
        let out = inline_shell_command(
            r#"printf '%s' "${ALEPH_SKILL_DIR-unset}""#,
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd: cwd.path(),
                plugin: None,
                skill_dir: None,
            },
        )
        .output()
        .await
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "unset");
    }

    /// On Windows a skill's `${ALEPH_SKILL_DIR}` is substituted into the
    /// line beside the plugin's path variables.
    #[test]
    #[cfg(windows)]
    fn on_windows_a_skills_command_gets_its_dir_substituted() {
        let none: [String; 0] = [];
        let cwd = std::env::temp_dir();
        let skill = Path::new(r"C:\skills\demo");
        let args: Vec<String> = inline_shell_command(
            r"type ${ALEPH_SKILL_DIR}\x",
            &InlineArgs {
                raw: "",
                positional: &none,
            },
            &InlineSite {
                cwd: &cwd,
                plugin: None,
                skill_dir: Some(skill),
            },
        )
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert_eq!(args, ["/C", r"type C:\skills\demo\x"]);
    }

    /// A name the site does not know is removed, not inherited from the
    /// daemon — whose `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PROJECT_DIR` are real
    /// values when it was launched from inside a Claude Code session. A
    /// label that is not a plugin id has a root but no data directory.
    #[tokio::test]
    #[cfg(unix)]
    #[serial_test::serial] // writes process env a spawned child reads
    async fn a_path_variable_the_site_lacks_is_not_inherited_from_the_daemon() {
        /// Sets the daemon-side values for the test and removes them after.
        struct DaemonEnv;
        impl Drop for DaemonEnv {
            fn drop(&mut self) {
                for key in DAEMON_ENV {
                    std::env::remove_var(key);
                }
            }
        }
        let _daemon = DaemonEnv;
        for key in DAEMON_ENV {
            std::env::set_var(key, "from-the-daemon");
        }
        let cwd = TempDir::new().unwrap();
        let here = cwd.path().to_string_lossy().to_string();

        assert_eq!(
            shown_env(cwd.path(), None).await,
            ["unset", "unset", "unset", "unset", "unset", here.as_str()]
        );
        let root = TempDir::new().unwrap();
        let root_text = root.path().to_string_lossy().to_string();
        assert_eq!(
            shown_env(cwd.path(), Some(("user:project", root.path()))).await,
            [
                root_text.as_str(),
                root_text.as_str(),
                root_text.as_str(),
                "unset",
                "unset",
                here.as_str()
            ]
        );
    }

    /// S4. The daemon's environment does not reach an inline command: a
    /// variable only the daemon has (where its provider keys and bot tokens
    /// live) is absent — and so is an `LC_` name that is no locale category
    /// (N7), and the control-socket directory `XDG_RUNTIME_DIR` (N8-a) —
    /// while what a shell needs — `PATH` — is inherited as the
    /// daemon's own value (a shell started without one may set a default of
    /// its own, so "some PATH" would prove nothing), and so is a proxy (N8).
    #[tokio::test]
    #[cfg(unix)]
    #[serial_test::serial] // writes process env a spawned child reads
    async fn the_daemons_environment_is_cleared_except_what_a_shell_needs() {
        /// Restores the daemon-side values after the test.
        struct Sentinel(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for Sentinel {
            fn drop(&mut self) {
                for (name, value) in &self.0 {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
        let set = [
            ("ALEPH_TEST_SECRET", "from-the-daemon"),
            // `LC_*` is how ssh forwards arbitrary variables: only the POSIX
            // locale categories are inherited.
            ("LC_SMUGGLED_TOKEN", "from-the-daemon"),
            ("HTTPS_PROXY", "http://proxy.test:3128"),
            // Where the user's dbus / keyring / `systemctl --user` sockets live.
            ("XDG_RUNTIME_DIR", "/run/user/test"),
        ];
        let _sentinel = Sentinel(
            set.iter()
                .map(|(name, _)| (*name, std::env::var_os(name)))
                .collect(),
        );
        for (name, value) in set {
            std::env::set_var(name, value);
        }
        let cwd = TempDir::new().unwrap();
        let out = inline_shell_command(
            r#"printf '%s|%s|%s|%s|%s' "${ALEPH_TEST_SECRET-unset}" "${LC_SMUGGLED_TOKEN-unset}" "${XDG_RUNTIME_DIR-unset}" "${HTTPS_PROXY-unset}" "$PATH""#,
            &InlineArgs {
                raw: "",
                positional: &[],
            },
            &InlineSite {
                cwd: cwd.path(),
                plugin: None,
                skill_dir: None,
            },
        )
        .output()
        .await
        .unwrap();
        let path = std::env::var("PATH").expect("the test process has a PATH");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("unset|unset|unset|http://proxy.test:3128|{path}")
        );
    }

    /// The gateway awaits a render inside a spawned task.
    #[test]
    fn a_render_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let t = SkillTemplate::new("x", Path::new("/x/cmd.md"));
        let ctx = no_shell();
        let render = t.render("", &ctx);
        assert_send(&render);
    }

    /// `/cmd $(touch M)` on a body that echoes its arguments: the shell reads
    /// the argument as a value and never parses it (P4.4b's invariant for
    /// command hooks, applied to inline commands).
    #[tokio::test]
    #[cfg(unix)]
    async fn a_command_substitution_in_the_arguments_does_not_run_in_an_inline_command() {
        let dir = TempDir::new().unwrap();
        let shell = Sh(dir.path().to_path_buf());
        let ctx = TemplateCtx {
            shell: Some(&shell),
        };

        let t = SkillTemplate::new("[!`echo $ARGUMENTS`]", Path::new("/x/cmd.md"));
        let out = t.render("$(touch M)", &ctx).await.unwrap();
        assert!(!dir.path().join("M").exists(), "the argument ran as code");
        assert_eq!(out, "[$(touch M)]");

        let t = SkillTemplate::new("[!`echo $1`]", Path::new("/x/cmd.md"));
        let out = t.render(";touch${IFS}N", &ctx).await.unwrap();
        assert!(!dir.path().join("N").exists(), "the argument ran as code");
        assert_eq!(out, "[;touch${IFS}N]");
    }

    /// The other half: the arguments DO reach the command — positional,
    /// defaulted and whole — so "nothing ran" above is not "nothing arrived".
    #[tokio::test]
    #[cfg(unix)]
    async fn the_arguments_reach_an_inline_command_as_shell_parameters() {
        let dir = TempDir::new().unwrap();
        let shell = Sh(dir.path().to_path_buf());
        let t = SkillTemplate::new(
            r#"[!`printf '%s' "$1|$2|${3:-none}|$ARGUMENTS|$#"`]"#,
            Path::new("/x/cmd.md"),
        );
        let out = t
            .render(
                "a  b",
                &TemplateCtx {
                    shell: Some(&shell),
                },
            )
            .await
            .unwrap();
        assert_eq!(out, "[a|b|none|a  b|2]");
    }
}
