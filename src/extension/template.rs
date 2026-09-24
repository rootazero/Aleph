//! Skill template processor
//!
//! Renders a command or skill body with the Claude Code / pi prompt-template
//! grammar ([`SkillTemplate::render`] has the order):
//! - `$1 … $N`, `${N:-default}` - positional arguments ([`split_arguments`])
//! - `$ARGUMENTS`, `$@` - the whole argument string
//! - `` !`cmd` `` - inline shell, run by the caller's [`InlineShell`] with the
//!   arguments as data ([`inline_shell_command`])
//! - `@./path` - relative file reference (from skill directory)
//! - `@/path` - absolute file reference: rejected

use super::error::{ExtensionError, ExtensionResult};
use once_cell::sync::OnceCell;
use regex::Regex;
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

/// The environment variable an inline command reads the whole argument
/// string from (`$ARGUMENTS`).
const ARGUMENTS_VAR: &str = "ARGUMENTS";

/// Runs one `` !`cmd` `` body for [`SkillTemplate::render`].
///
/// The template knows nothing about consent, cwd or timeouts — the caller
/// (the gateway's slash-command seam) owns those and hands in an
/// implementation, which spawns [`inline_shell_command`]. `cmd` is the
/// plugin's text as written and never contains argument text; the arguments
/// arrive beside it in `args`. `Err(text)` is substituted verbatim: a withheld
/// or failed expansion must be visible to the model as such, never a silent
/// blank.
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

/// The process for one `` !`cmd` ``: `cmd` is the shell's source, the
/// arguments are its data.
///
/// On unix: `sh -c <cmd> sh <positional…>` with `ARGUMENTS=<raw>`, so `$1`,
/// `${2:-x}`, `$@`, `$#` and `$ARGUMENTS` are shell parameters — expanded as
/// values, never parsed: a `/cmd $(…)` is text to the command, not code.
/// `ARGUMENTS` is set even when empty, so the daemon's own value is never
/// inherited. Inside an inline command the shell's rules apply, not the
/// template's: `$10` is `${1}0` (write `${10}`), and a single-quoted `'$1'`
/// stays literal.
///
/// On Windows (`cmd /C`) there are no positional parameters, anything passed
/// after the command is appended to its line, and `%VAR%` is expanded before
/// the line is parsed — so no argument reaches an inline command there:
/// nothing is appended and `ARGUMENTS` is removed, not exported.
///
/// The caller sets the directory, stdio and timeout.
#[must_use]
pub fn inline_shell_command(cmd: &str, args: &InlineArgs<'_>) -> tokio::process::Command {
    use crate::utils::no_window::NoWindow;
    let mut command = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]).env_remove(ARGUMENTS_VAR);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd, "sh"])
            .args(args.positional)
            .env(ARGUMENTS_VAR, args.raw);
        c
    };
    command.no_window();
    command
}

/// `${N:-default}`, `$N` (1-based; out of range → the default, else empty),
/// `$ARGUMENTS` and `$@` over one stretch of template text.
fn substitute_arguments(text: &str, args: &InlineArgs<'_>) -> String {
    ARGUMENT_REGEX
        .replace_all(text, |cap: &regex::Captures<'_>| {
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
        })
        .into_owned()
}

/// One inline command's expansion: its stdout, the runner's placeholder, or —
/// with no runner — a placeholder saying so.
async fn run_inline(cmd: &str, args: &InlineArgs<'_>, ctx: &TemplateCtx<'_>) -> String {
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
    /// 3. `@./file` references are expanded over the result, so `@$1` names a
    ///    file by argument.
    ///
    /// Claude Code and pi substitute the arguments into the whole body first,
    /// so `` !`gh pr view $1` `` runs with the argument spliced into its text.
    /// Here the command reads `$1` as a shell parameter: the same result for
    /// a plain argument, text rather than code for one holding `$(…)`, `;` or
    /// a backtick.
    pub async fn render(&self, arguments: &str, ctx: &TemplateCtx<'_>) -> ExtensionResult<String> {
        let positional = split_arguments(arguments);
        let args = InlineArgs {
            raw: arguments,
            positional: &positional,
        };
        // Collected before the first `.await`: the match iterator is not held
        // across one.
        let commands: Vec<(std::ops::Range<usize>, &str)> = INLINE_SHELL_REGEX
            .captures_iter(&self.content)
            .filter_map(|cap| Some((cap.get(0)?.range(), cap.get(1)?.as_str().trim())))
            .collect();
        let text = |range: std::ops::Range<usize>| self.content.get(range).unwrap_or_default();
        let mut out = String::with_capacity(self.content.len());
        let mut last = 0;
        for (whole, cmd) in commands {
            out.push_str(&substitute_arguments(text(last..whole.start), &args));
            out.push_str(&run_inline(cmd, &args, ctx).await);
            last = whole.end;
        }
        out.push_str(&substitute_arguments(text(last..self.content.len()), &args));
        self.expand_file_refs(&out).await
    }

    /// Expand all file references in the content
    async fn expand_file_refs(&self, content: &str) -> ExtensionResult<String> {
        let mut result = content.to_string();
        let mut replacements = Vec::new();

        // Find all file references
        for cap in file_ref_regex()?.captures_iter(content) {
            if replacements.len() >= MAX_FILE_REFS {
                return Err(ExtensionError::template_error(format!(
                    "too many file references (cap {MAX_FILE_REFS})"
                )));
            }
            let full_match = cap
                .get(0)
                .ok_or_else(|| ExtensionError::template_error("regex capture group 0 missing"))?;
            let path_str = cap
                .get(1)
                .ok_or_else(|| {
                    ExtensionError::template_error("regex capture group 1 missing for file refs")
                })?
                .as_str();

            // Resolve the path
            let resolved_path = self.resolve_path(path_str)?;

            // Read file content
            let file_content = self.read_file(&resolved_path).await?;

            replacements.push((
                full_match.start(),
                full_match.end(),
                full_match.as_str().to_string(),
                file_content,
            ));
        }

        // Apply replacements in reverse order to preserve positions.
        // Use positional replacement (single occurrence) to avoid corrupting
        // file contents that may contain the same reference pattern.
        for (start, end, _, replacement) in replacements.into_iter().rev() {
            result.replace_range(start..end, &replacement);
        }

        Ok(result)
    }

    /// Resolve a file path from the template syntax
    fn resolve_path(&self, path_str: &str) -> ExtensionResult<PathBuf> {
        let path = if let Some(relative) = path_str.strip_prefix("./") {
            // Relative path from base_dir
            let resolved = self.base_dir.join(relative);

            // Security check: ensure the resolved path is within base_dir
            self.validate_path_security(&resolved)?;

            resolved
        } else if path_str.starts_with('/') {
            // Absolute paths are not allowed — they bypass base_dir containment
            return Err(ExtensionError::file_reference(
                path_str,
                "Absolute paths are not allowed in file references; use relative paths (./path) instead",
            ));
        } else {
            // Treat as relative
            let resolved = self.base_dir.join(path_str);
            self.validate_path_security(&resolved)?;
            resolved
        };

        Ok(path)
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

        // If the file exists, canonicalize and verify containment within base_dir
        if resolved.exists() {
            if let (Ok(canonical_path), Ok(canonical_base)) =
                (resolved.canonicalize(), self.base_dir.canonicalize())
            {
                if !canonical_path.starts_with(&canonical_base) {
                    return Err(ExtensionError::file_reference(
                        resolved,
                        "Resolved path escapes the base directory",
                    ));
                }
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

        let template = SkillTemplate::with_base_dir(
            &format!("Content: @{}", file_path.display()),
            PathBuf::from("/other"),
        );

        // Absolute paths must be rejected
        let result = template.render("", &no_shell()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExtensionError::FileReference { .. }));
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let template = SkillTemplate::with_base_dir(
            "Content: @./../../../etc/passwd",
            PathBuf::from("/test/skill"),
        );

        let result = template.render("", &no_shell()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExtensionError::FileReference { .. }));
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let template = SkillTemplate::with_base_dir(
            "Content: @./nonexistent.txt",
            PathBuf::from("/test/skill"),
        );

        let result = template.render("", &no_shell()).await;
        assert!(result.is_err());
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

    /// The production process builder behind a minimal runner: what P4.7c's
    /// consent-gated runner spawns, without the consent and the timeout.
    #[cfg(unix)]
    struct Sh(PathBuf);

    #[cfg(unix)]
    #[async_trait::async_trait]
    impl InlineShell for Sh {
        async fn run(&self, cmd: &str, args: &InlineArgs<'_>) -> Result<String, String> {
            let out = inline_shell_command(cmd, args)
                .current_dir(&self.0)
                .stdin(std::process::Stdio::null())
                .output()
                .await
                .map_err(|e| e.to_string())?;
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
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
