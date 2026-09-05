//! The shell Aleph spawns — one probe, one answer.
//!
//! Two facts made this module necessary. First, the agent's shell used to be a
//! `const fn` returning `"bash"` on every platform, so on Windows the prompt's
//! `- **Shell**: bash` line was a claim about a binary that need not exist —
//! the same defect [`crate::utils::host`] documents one field over, where the
//! host name was read from a variable no service manager exports. Second, the
//! "prefer `pwsh`, fall back to `powershell`" ladder was hand-copied into four
//! call sites, so a fix to one could never reach the others.
//!
//! [`ResolvedShell::program`] is the **absolute** path, not the bare name: the
//! sandbox `env_clear()`s its children (`sandbox::platforms::windows::driver`),
//! so a child resolved through `PATH` is a child resolved through whatever
//! `PATH` we happened to pass, and on Windows we pass a rebuilt one.
//!
//! Cached for the process lifetime: a shell does not appear or vanish while the
//! daemon runs, and this sits on the per-turn prompt-assembly path
//! (`RuntimeContext::collect_in`) as well as every `code_exec` spawn.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Threshold above which a `bash` script switches from `bash -c <script>` to
/// `bash -s` reading the script from stdin. Linux's `ARG_MAX` for a single
/// argv element (`MAX_ARG_STRLEN`) is typically 128 KiB; we keep a 4× margin to
/// leave room for the rest of the argv vector plus env.
///
/// PowerShell guards a different limit — see [`PWSH_STDIN_THRESHOLD`].
pub const STDIN_PIPE_THRESHOLD: usize = 32 * 1024;

/// Wrapped-script length above which a PowerShell call switches from
/// `-Command <script>` to `-Command -` reading the script from stdin.
///
/// This defends a **different limit from [`STDIN_PIPE_THRESHOLD`]**, and the
/// two must not be merged. `CreateProcess` caps the entire command line at
/// 32,767 chars — the program path, every flag and the whole wrapped script
/// together — where the bash constant caps one argv element. 32 KiB of script
/// sits under the bash threshold and already over this one, so sharing a
/// number would fail in the unsafe direction.
///
/// MEASURED (`qa/winshell/run.sh length`): a bare spawn carried 32,320 chars
/// and failed at 32,384 with `ENAMETOOLONG`; prologue + epilogue + argv cost
/// 379 of the budget. The value below is far under that on purpose — under the
/// sandbox the same command line also carries
/// `sandbox-init-windows --policy <json> --`, whose JSON is unbounded from
/// here, so the true headroom is not knowable at this layer. Deliberately not
/// the limit itself.
pub const PWSH_STDIN_THRESHOLD: usize = 8 * 1024;

/// Statements prepended to every PowerShell script we run.
///
/// * `$ErrorActionPreference='Continue'` keeps bash-like semantics: one failing
///   statement does not abort the rest of the script.
/// * The two encoding lines are not belt-and-braces. MEASURED on Windows 11 /
///   pwsh 7.6, both invocation forms from one parent: the child's
///   `[Console]::OutputEncoding.CodePage` came back as the **parent console's**
///   code page both times (936 on that host — the number is the console's, not
///   a constant), and nothing about how we invoke it changes that. So
///   UTF-8 has to be *stated*; a child left alone writes the host ANSI page and
///   every non-ASCII byte we capture is mojibake. (An earlier reading of 65001
///   was taken from inside an already-prologued script, i.e. it measured this
///   line's own effect.)
/// * `$global:LASTEXITCODE=0` because [`PS_EPILOGUE`] reads it unconditionally
///   and it is undefined until the first native command runs.
///
/// Deliberately NOT here: `$PSStyle.OutputRendering = 'PlainText'`. pwsh 7 does
/// colourise its errors even with stderr redirected (MEASURED: 32-88 escape
/// sequences for one error; 5.1 emits none), which looks like a reason to add
/// it — but `tool_output::sanitize::sanitize_command_output` already strips
/// ANSI from all four output paths before the model sees anything. Adding it
/// would be a second mechanism for one fact, paid for on every command line.
const PS_PROLOGUE: &str = concat!(
    "$ErrorActionPreference='Continue'\n",
    // `try`/`catch` around the two encoding lines, ported from codex's
    // `UTF8_OUTPUT_PREFIX` (`shell-command/src/powershell.rs:12`). Assigning
    // `[Console]::OutputEncoding` throws in a host with no console attached,
    // and a terminating error in the prologue would take the user's whole
    // script with it — the prologue would kill the command it exists to make
    // readable. Mojibake is the worse-but-survivable outcome, so the failure
    // here has to be swallowed rather than propagated.
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n",
    "try { $OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n",
    // Not wrapped: assigning a global cannot throw, and a `try` around it
    // would only suggest to the next reader that it can.
    "$global:LASTEXITCODE=0",
);

/// Statements appended to every PowerShell script we run, to give the caller a
/// POSIX-shaped exit code.
///
/// Without it a failing *native* child is invisible: MEASURED, `pwsh -Command
/// 'cmd /c exit 3'` exits **1**, not 3 — PowerShell reports "the pipeline
/// failed", and the child's own code survives only in `$LASTEXITCODE`.
///
/// Both values are captured into locals on the first two lines because reading
/// them later re-reads them: `$?` is the success of the *previous statement*,
/// so any statement of ours between the user's script and the test would answer
/// for itself instead of for the script.
const PS_EPILOGUE: &str = concat!(
    "$__aleph_ok=$?\n",
    "$__aleph_rc=$LASTEXITCODE\n",
    "if ($null -ne $__aleph_rc -and $__aleph_rc -ne 0) { exit $__aleph_rc }\n",
    "if ($__aleph_ok) { exit 0 }\n",
    "exit 1",
);

/// Prefix for the `cmd.exe` arm: without it the child writes in the host ANSI
/// code page and every non-ASCII byte we capture is mojibake.
const CMD_UTF8_PREFIX: &str = "chcp 65001>nul&";

/// A shell Aleph knows how to drive. The variant determines the argv shape —
/// see [`ShellKind::invocation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// POSIX `bash`. The only shell on non-Windows.
    Bash,
    /// PowerShell 7+ (`pwsh`), cross-platform, preferred on Windows.
    Pwsh,
    /// Windows PowerShell 5.1 (`powershell`), present on every Windows host.
    WindowsPowerShell,
    /// `cmd.exe` — the floor, reached only if neither PowerShell resolves.
    Cmd,
}

impl ShellKind {
    /// The bare program name. Doubles as the human/prompt label, so
    /// [`ResolvedShell::label`] is derived from this rather than spelled a
    /// second time.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Pwsh => "pwsh",
            Self::WindowsPowerShell => "powershell",
            Self::Cmd => "cmd",
        }
    }

    /// The `(args, stdin)` pair for running `script` under this shell. The
    /// program itself is [`ResolvedShell::program`].
    ///
    /// The PowerShell arms wrap the script in [`PS_PROLOGUE`] / [`PS_EPILOGUE`]
    /// joined with **newlines, never `;`** — a script whose last line is a `#`
    /// comment would swallow a `;`-joined epilogue and silently lose the exit
    /// code (pinned by `trailing_comment_cannot_swallow_the_epilogue`).
    ///
    /// Both families branch on size, for **different limits** — which is why
    /// they do not share a constant. `Bash` guards Linux's per-argv-element
    /// `MAX_ARG_STRLEN` ([`STDIN_PIPE_THRESHOLD`]); PowerShell guards
    /// `CreateProcess`'s cap on the *whole command line*
    /// ([`PWSH_STDIN_THRESHOLD`]), which also counts the program path, our
    /// flags, the prologue and the epilogue. A single constant would be wrong
    /// in the unsafe direction: 32 KiB of script is under the bash threshold
    /// and already over the Windows one.
    ///
    /// The stdin form is `-Command -`, and it is the same contract, not a
    /// second one. MEASURED: `cmd /c exit 3` + the epilogue exits **3** through
    /// stdin exactly as through `-Command <literal>`, and with [`PS_PROLOGUE`]
    /// the stdin child reports code page 65001 (both pinned by
    /// `qa/winshell/run.sh exit` and `… encoding`). Encoding is not a
    /// discriminator either way — measured, both forms inherit the parent
    /// console's code page, which is what the prologue exists to override.
    ///
    /// Why a threshold far below the measured ceiling rather than just under
    /// it: `qa/winshell/run.sh length` puts a bare spawn's largest carried
    /// script at 32,320 chars, but that is the ceiling for THIS argv. Under the
    /// sandbox the real command line also carries
    /// `sandbox-init-windows --policy <json> --`, and that JSON policy is
    /// unbounded from here — so the headroom a caller actually has is not
    /// knowable at this layer. The threshold is set low enough that the
    /// difference cannot matter instead of tracking a number it cannot see.
    #[must_use]
    pub fn invocation(&self, script: &str) -> (Vec<String>, Option<Vec<u8>>) {
        match self {
            Self::Bash => {
                if script.len() > STDIN_PIPE_THRESHOLD {
                    (vec!["-s".to_string()], Some(script.as_bytes().to_vec()))
                } else {
                    (vec!["-c".to_string(), script.to_string()], None)
                }
            }
            // No `-ExecutionPolicy Bypass`: it is irrelevant to `-Command`
            // (which never loads a script file) and would match our own
            // `win_execution_policy_bypass` rule in
            // `sandbox::command_policy::rules`. Likewise no `-EncodedCommand`,
            // which `win_encoded_command` warns on.
            Self::Pwsh | Self::WindowsPowerShell => {
                let wrapped = format!("{PS_PROLOGUE}\n{script}\n{PS_EPILOGUE}");
                let mut args = vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                ];
                // The wrapped length is what the command line has to carry, so
                // that — not the caller's `script` — is what the threshold is
                // measured against.
                if wrapped.len() > PWSH_STDIN_THRESHOLD {
                    args.push("-".to_string());
                    (args, Some(wrapped.into_bytes()))
                } else {
                    args.push(wrapped);
                    (args, None)
                }
            }
            // `/D` skips AutoRun registry commands (a per-machine hook we must
            // not inherit); `/S` fixes the quote-stripping rule so the script
            // is taken verbatim.
            Self::Cmd => (
                vec![
                    "/D".to_string(),
                    "/S".to_string(),
                    "/C".to_string(),
                    format!("{CMD_UTF8_PREFIX}{script}"),
                ],
                None,
            ),
        }
    }
}

/// A shell resolved on this host.
#[derive(Debug, Clone)]
pub struct ResolvedShell {
    /// Which shell it is — decides the argv shape.
    pub kind: ShellKind,
    /// Absolute path when resolution succeeded, the bare name as the P7 floor.
    pub program: PathBuf,
    /// Short name for humans and for the prompt's environment envelope.
    pub label: String,
}

impl ResolvedShell {
    fn new(kind: ShellKind, program: PathBuf) -> Self {
        Self {
            kind,
            program,
            label: kind.label().to_string(),
        }
    }

    /// P7 floor: nothing resolved, so hand back the bare name and let the
    /// spawn produce the OS's own "not found" instead of panicking here.
    fn bare(kind: ShellKind) -> Self {
        Self::new(kind, PathBuf::from(kind.label()))
    }

    /// Convenience for call sites that only need the string form of `program`.
    #[must_use]
    pub fn program_string(&self) -> String {
        self.program.to_string_lossy().into_owned()
    }
}

static AGENT_SHELL: OnceLock<ResolvedShell> = OnceLock::new();
static PWSH: OnceLock<Option<ResolvedShell>> = OnceLock::new();
static WINDOWS_POWERSHELL: OnceLock<Option<ResolvedShell>> = OnceLock::new();

/// Is this a Microsoft Store *alias* rather than a real installed program?
///
/// `%LOCALAPPDATA%\Microsoft\WindowsApps` holds zero-byte reparse points that
/// `which` resolves happily and that then behave nothing like the program they
/// name. Two ways they bite us, both measured on this host:
///
/// * `python3` there exits **49** with no output — so `code_exec{python}`
///   reports a failure the model cannot act on, for a Python that was never run.
/// * A Store PowerShell is not readable by a restricted-token child, which is
///   exactly what `sandbox::platforms::windows` spawns.
///
/// codex carries the same guard (`shell_detect.rs:137`,
/// `is_inaccessible_windows_apps_powershell_path`); it is here for both
/// PowerShell and Python because one predicate is cheaper to keep true than two.
#[cfg(windows)]
pub(crate) fn is_windows_apps_alias(path: &std::path::Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("WindowsApps"))
    })
}

/// Absolute paths tried when `PATH` does not carry a shell.
///
/// The sandbox hands its children a rebuilt `PATH`, and a minimal one is the
/// case this whole module exists for — so "not on `PATH`" must not be read as
/// "not installed". codex keeps the same two literals
/// (`shell_detect.rs:267`). Existence is checked before a literal is accepted;
/// they are candidates, not assumptions.
#[cfg(windows)]
fn well_known_paths(kind: ShellKind) -> &'static [&'static str] {
    match kind {
        ShellKind::Pwsh => &[r"C:\Program Files\PowerShell\7\pwsh.exe"],
        ShellKind::WindowsPowerShell => {
            &[r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"]
        }
        ShellKind::Cmd | ShellKind::Bash => &[],
    }
}

fn locate(kind: ShellKind) -> Option<ResolvedShell> {
    let found = which::which(kind.label()).ok();

    #[cfg(windows)]
    let found = found.filter(|p| !is_windows_apps_alias(p)).or_else(|| {
        well_known_paths(kind)
            .iter()
            .map(std::path::Path::new)
            .find(|p| p.is_file())
            .map(std::path::Path::to_path_buf)
    });

    found.map(|program| ResolvedShell::new(kind, program))
}

/// PowerShell 7 (`pwsh`) if it is installed.
pub fn pwsh() -> Option<&'static ResolvedShell> {
    PWSH.get_or_init(|| locate(ShellKind::Pwsh)).as_ref()
}

/// Windows PowerShell 5.1 (`powershell`) if it is installed.
pub fn windows_powershell() -> Option<&'static ResolvedShell> {
    WINDOWS_POWERSHELL
        .get_or_init(|| locate(ShellKind::WindowsPowerShell))
        .as_ref()
}

/// The best available PowerShell host, or `None` on a machine with neither.
///
/// Distinct from [`resolve`]: this never degrades to `cmd`, because its callers
/// pass PowerShell-only argv (`-NoProfile -File`, `-Command`) that `cmd` would
/// misread as file names.
pub fn powershell_host() -> Option<&'static ResolvedShell> {
    pwsh().or_else(windows_powershell)
}

/// `cmd.exe`, preferring the OS's own answer in `%COMSPEC%` over a `PATH` walk.
#[cfg(windows)]
fn cmd_shell() -> Option<ResolvedShell> {
    if let Some(path) = std::env::var_os("COMSPEC").map(PathBuf::from) {
        if path.is_file() {
            return Some(ResolvedShell::new(ShellKind::Cmd, path));
        }
    }
    locate(ShellKind::Cmd)
}

/// The shell the agent's `bash` / `code_exec` tool runs under, resolved once.
///
/// Windows order is `pwsh` → `powershell` → `cmd`: PowerShell 7 first because
/// it is the one shell whose behaviour matches its cross-platform
/// documentation (5.1 aliases `curl`/`wget` to `Invoke-WebRequest`, so scripts
/// written for a POSIX-ish shell misbehave in ways that read as our bug).
pub fn resolve() -> &'static ResolvedShell {
    AGENT_SHELL.get_or_init(|| {
        #[cfg(windows)]
        {
            powershell_host()
                .cloned()
                .or_else(cmd_shell)
                // Neither PowerShell nor cmd on a Windows box is not a state we
                // can repair; name the one we would have preferred so the error
                // the spawn raises says something useful.
                .unwrap_or_else(|| ResolvedShell::bare(ShellKind::Pwsh))
        }
        #[cfg(not(windows))]
        {
            locate(ShellKind::Bash).unwrap_or_else(|| ResolvedShell::bare(ShellKind::Bash))
        }
    })
}

/// A resolved interpreter: the program to spawn plus any args that must
/// precede the caller's own (`py` needs `-3` before `-c`).
#[derive(Debug, Clone)]
pub struct ResolvedInterpreter {
    /// Absolute path when resolution succeeded, the bare name as the P7 floor.
    pub program: PathBuf,
    /// Args inserted before the caller's — empty for every candidate but `py`.
    pub leading: Vec<String>,
}

static PYTHON3: OnceLock<ResolvedInterpreter> = OnceLock::new();

/// The Python 3 this host actually has.
///
/// `"python3"` was hardcoded for every platform, and on Windows that is the
/// wrong name: python.org ships `python.exe` and the `py` launcher, and the
/// only `python3` most Windows boxes have on `PATH` is the Store alias, which
/// exits 49 without running anything ([`is_windows_apps_alias`]). So
/// `code_exec{language:"python"}` reported a failure for a Python that was
/// never started.
///
/// Windows order is `py -3` (the official launcher, and the one name that
/// stays right across installs) → `python` → `python3`. Unix keeps bare
/// `python3` unchanged. When nothing resolves we fall back to `python3` rather
/// than inventing a new error: the caller then gets exactly the "not found"
/// it got before, instead of a message that only this round would explain.
pub fn python3() -> &'static ResolvedInterpreter {
    PYTHON3.get_or_init(|| {
        #[cfg(windows)]
        {
            for (name, leading) in [("py", &["-3"][..]), ("python", &[]), ("python3", &[])] {
                if let Ok(program) = which::which(name) {
                    if !is_windows_apps_alias(&program) {
                        return ResolvedInterpreter {
                            program,
                            leading: leading.iter().map(|s| (*s).to_string()).collect(),
                        };
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            if let Ok(program) = which::which("python3") {
                return ResolvedInterpreter {
                    program,
                    leading: Vec::new(),
                };
            }
        }
        ResolvedInterpreter {
            program: PathBuf::from("python3"),
            leading: Vec::new(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prompt shows `label`; the sandbox spawns `program`. If they can name
    /// different binaries the environment envelope is lying, so pin that the
    /// label is exactly the resolved file's stem (and the bare-name floor is
    /// trivially its own stem).
    #[test]
    fn label_is_the_stem_of_the_resolved_program() {
        let shell = resolve();
        let stem = shell
            .program
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert_eq!(stem, shell.label, "prompt label must name the spawned file");
    }

    #[cfg(windows)]
    #[test]
    fn resolve_prefers_pwsh_when_present() {
        if which::which("pwsh").is_ok() {
            assert_eq!(resolve().kind, ShellKind::Pwsh);
            assert!(
                resolve().program.is_absolute(),
                "program must survive a child with a different PATH"
            );
        } else {
            // Still an assertion, not a skip — and a falsifiable one: `Pwsh` is
            // reachable here only through the bare-name floor, which means the
            // ladder skipped a `powershell` / `cmd` that does exist. Listing all
            // three variants instead would be a predicate that cannot go red.
            assert_ne!(
                resolve().kind,
                ShellKind::Pwsh,
                "pwsh is not installed, so it must not have been selected"
            );
        }
    }

    // The argv-shape tests below build the kind from a literal so they run on
    // every platform — they are about the contract, not about what is installed.

    #[test]
    fn bash_argv_shape() {
        let (args, stdin) = ShellKind::Bash.invocation("echo hi");
        assert_eq!(args, vec!["-c".to_string(), "echo hi".to_string()]);
        assert!(stdin.is_none());

        let big = "x".repeat(STDIN_PIPE_THRESHOLD + 1);
        let (args, stdin) = ShellKind::Bash.invocation(&big);
        assert_eq!(args, vec!["-s".to_string()]);
        assert_eq!(stdin.as_deref(), Some(big.as_bytes()));
    }

    #[test]
    fn powershell_argv_shape() {
        for kind in [ShellKind::Pwsh, ShellKind::WindowsPowerShell] {
            let (args, stdin) = kind.invocation("Write-Output hi");
            assert_eq!(args[..3], ["-NoProfile", "-NonInteractive", "-Command"]);
            assert!(stdin.is_none(), "a small script stays on the command line");
            let wrapped = &args[3];
            assert!(wrapped.starts_with(PS_PROLOGUE));
            assert!(wrapped.ends_with(PS_EPILOGUE));
            assert!(wrapped.contains("Write-Output hi"));
            // Neither flag may appear: both match a rule of our own hard filter.
            assert!(!wrapped.contains("-EncodedCommand"));
            assert!(!args.iter().any(|a| a == "-ExecutionPolicy"));
        }
    }

    /// A script too big for `CreateProcess`'s command-line cap must move to
    /// stdin rather than fail at spawn with `ENAMETOOLONG` — an error that
    /// names a file the caller never mentioned.
    ///
    /// Goes red if the pwsh arm stops branching, if it branches on the raw
    /// `script` instead of the wrapped text (the wrapper is what the command
    /// line carries), or if the two thresholds are merged: `PWSH_STDIN` is a
    /// quarter of `STDIN_PIPE`, so a script between them proves this arm is
    /// not reading the bash constant.
    #[test]
    fn a_large_powershell_script_moves_to_stdin() {
        let big = "x".repeat(PWSH_STDIN_THRESHOLD + 1);
        assert!(
            big.len() < STDIN_PIPE_THRESHOLD,
            "the sample must sit BETWEEN the two thresholds or it proves nothing \
             about which one the pwsh arm reads",
        );

        for kind in [ShellKind::Pwsh, ShellKind::WindowsPowerShell] {
            let (args, stdin) = kind.invocation(&big);
            assert_eq!(args, ["-NoProfile", "-NonInteractive", "-Command", "-"]);

            let piped = String::from_utf8(stdin.expect("script must ride on stdin"))
                .expect("the wrapped script is UTF-8");
            // Same wrapper as the command-line form: the prologue and epilogue
            // are the contract, not an artefact of how the script is delivered.
            assert!(piped.starts_with(PS_PROLOGUE));
            assert!(piped.ends_with(PS_EPILOGUE));
            assert!(piped.contains(&big));
        }
    }

    /// A `;`-joined epilogue would land inside the user's trailing comment and
    /// never run, so the exit code would silently revert to PowerShell's own
    /// (MEASURED: 1 for any failing native child, whatever its real code).
    #[test]
    fn trailing_comment_cannot_swallow_the_epilogue() {
        let (args, _) = ShellKind::Pwsh.invocation("Write-Output ok # trailing comment");
        let wrapped = &args[3];
        let epilogue_at = wrapped
            .find(PS_EPILOGUE)
            .expect("epilogue must be present verbatim");
        assert!(
            wrapped[..epilogue_at].ends_with('\n'),
            "epilogue must start on its own line, not after a `;`"
        );
        assert!(!wrapped.contains("comment;"));
    }

    #[test]
    fn cmd_argv_shape() {
        let (args, stdin) = ShellKind::Cmd.invocation("echo hi");
        assert_eq!(args[..3], ["/D", "/S", "/C"]);
        assert_eq!(args[3], format!("{CMD_UTF8_PREFIX}echo hi"));
        assert!(stdin.is_none());
    }

    /// Every variant must produce a non-empty argv, or the shell would be
    /// spawned with the script dropped — a "reports success, ran nothing".
    #[test]
    fn every_kind_carries_the_script() {
        for kind in [
            ShellKind::Bash,
            ShellKind::Pwsh,
            ShellKind::WindowsPowerShell,
            ShellKind::Cmd,
        ] {
            let (args, stdin) = kind.invocation("marker-token");
            let seen = args.iter().any(|a| a.contains("marker-token"))
                || stdin.is_some_and(|s| String::from_utf8_lossy(&s).contains("marker-token"));
            assert!(seen, "{kind:?} dropped the script");
        }
    }
}
