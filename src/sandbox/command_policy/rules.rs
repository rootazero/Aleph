//! Catastrophic-command rulesets + action / enforcement types.
//!
//! Ported in spirit from clawshell's DLP `[[patterns]]` engine (regex +
//! `action = block|redact`), but specialised for *shell command* content
//! rather than HTTP payloads, and matched in a single pass via
//! `regex::RegexSet` instead of clawshell's sequential `Vec<Regex>` scan.
//!
//! Philosophy (CLAUDE.md R7 "safe hard-filter" — a sanctioned hard-filter, NOT an
//! LLM-replacing rule engine): this layer is defence-in-depth *in front of*
//! the OS sandbox. It does not decide intent; it refuses a small set of
//! patterns that are essentially never legitimate inside an agent workspace
//! and audits a slightly larger set of high-signal suspicious shapes. The
//! OS seatbelt/bwrap/job-object remains the real enforcer.
//!
//! Two tiers (see [`super::CommandPolicy`]):
//!
//! * [`hardline_rules`] — catastrophic, irreversible shapes (disk wipe,
//!   filesystem-root delete, fork bomb). These are an **undisableable floor**:
//!   enforced regardless of [`EnforcementMode`] and present even when the
//!   tunable policy is switched off. Mirrors hermes-agent's `HARDLINE_PATTERNS`
//!   that never bypass, even under `--yolo`.
//! * [`default_rules`] — high-signal but occasionally-legitimate shapes that an
//!   operator can downgrade (`warn`) or disable (`off`) for staged rollout.
//!
//! # Shared pattern fragments
//!
//! Several rules must agree on what "a raw block device" or "a bare Windows
//! volume root" looks like. Those vocabularies live in the `macro_rules!`
//! fragments below and are spliced in with [`concat!`], so the alternations
//! cannot drift apart the way hand-copied ones do — an earlier revision carried
//! the device class three times under a "kept in sync" comment, which is the
//! documentation of a hazard rather than a defence against it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Canonical raw-block-device class shared by the `dd` / redirect / wipe rules:
/// SCSI/SATA (`sd`), NVMe, macOS (`disk`), legacy IDE (`hd`), virtio (`vd`),
/// Xen/AWS-EC2 root volumes (`xvd` — previously uncovered, so
/// `dd of=/dev/xvda` wiped an EC2 root disk undetected), SD/eMMC (`mmcblk`),
/// loop, optical (`sr`), persistent memory (`pmem`), device-mapper/LVM (`dm-`)
/// and software-RAID (`md`). The alternation is always anchored right after
/// `/dev/`, so member order does not affect matching.
macro_rules! unix_block_device {
    () => {
        r"(?:sd|nvme|disk|hd|xvd|vd|mmcblk|loop|sr|pmem|dm-|md)"
    };
}

/// Gap between two tokens that must belong to the **same command segment**.
///
/// `[^\n]*` spans the whole line, which lets an unrelated later statement
/// supply the second half of a rule: `del /s build\* & echo C:\` matched the
/// drive-root floor because `echo C:\` sat on the same line. Excluding the
/// shell statement separators (`&`, `|`, `;` — cmd, PowerShell and POSIX all
/// use these) keeps a verb bound to its own arguments. Rules that deliberately
/// straddle a pipe (the download cradles) keep `[^\n]*` instead.
macro_rules! seg {
    () => {
        r"[^\n&|;]*"
    };
}

/// Every spelling of "delete this recursively" an agent can reach on Windows:
/// the cmd built-ins plus the PowerShell cmdlet **and its aliases**, which all
/// resolve to `Remove-Item` (`ri`, `rm`, `rd`, `rmdir`, `del`, `erase`). A rule
/// that only knew the literal `remove-item` was bypassed by every alias.
macro_rules! win_delete_verb {
    () => {
        r#"(?:^|[\s;&|({"'])(?:remove-item|erase|rmdir|del|rd|ri|rm)\b"#
    };
}

/// Recursive-delete flag in either dialect: PowerShell's `-Recurse` (and every
/// prefix abbreviation it accepts, plus POSIX clusters like `-rf`) or cmd's
/// `/s`.
macro_rules! win_recursive_flag {
    () => {
        r"(?:-r[a-z]*|/s)\b"
    };
}

/// A **bare** Windows volume or registry-hive root, as a standalone argument.
///
/// Includes the leading token boundary and the trailing terminator, because
/// "bare" is the whole point: `C:\Users\me\build` shares the `C:` prefix with
/// `C:\` and only the terminator tells them apart. Covers the literal drive
/// letter, the environment-variable spellings cmd and PowerShell expand to the
/// same thing (`%SystemDrive%`, `$env:SystemDrive`), and the hive roots that
/// PowerShell's registry provider exposes as drives.
///
/// The optional `\\?` is what makes this work in both normalisation views: the
/// POSIX view has folded the separator away, the native view still carries it
/// (see [`super::normalize`]).
macro_rules! win_bare_root {
    () => {
        r#"["'\s](?:[a-z]:|%systemdrive%|\$\{?env:systemdrive\}?|hk(?:lm|cu|cr|u|ey_local_machine|ey_current_user|ey_classes_root|ey_users):)\\?(?:\*|\s|["';&|]|$)"#
    };
}

/// A Windows system location whose recursive removal breaks the host: anything
/// under `\Windows` / `\ProgramData` / `\Program Files`, the *bare* `\Users`
/// root, and the environment-variable spellings of the same places.
///
/// `\Users\<someone>\...` is deliberately **not** here — an agent workspace
/// normally lives there, so matching subpaths would warn on every ordinary
/// build-directory cleanup.
macro_rules! win_system_path {
    () => {
        r#"["'\s](?:(?:[a-z]:)?[\\/](?:windows|winnt|programdata|program files(?: \(x86\))?)(?:[\\/]|["'\s;&|*]|$)|(?:[a-z]:)?[\\/]users(?:["'\s;&|*]|$)|%(?:systemroot|windir|programfiles|programdata)%|\$\{?env:(?:systemroot|windir|programfiles|programdata)\}?)"#
    };
}

/// What a matched rule asks the policy to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RuleAction {
    /// Refuse execution outright (subject to the global [`EnforcementMode`]).
    #[default]
    Block,
    /// Allow execution but emit an audit log line.
    Warn,
}

/// Global override applied on top of per-rule [`RuleAction`]s.
///
/// Applies to the **tunable** ruleset only. The [`hardline_rules`] floor is
/// always enforced and ignores this mode entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnforcementMode {
    /// Honour per-rule actions: `Block` rules deny, `Warn` rules audit.
    #[default]
    Block,
    /// Observation mode: downgrade every tunable `Block` to a `Warn`. No
    /// tunable rule is ever denied — useful for staged rollout / measuring
    /// false positives. The hardline floor still blocks.
    Warn,
    /// Disable the tunable ruleset (those matches short-circuit to Allow). The
    /// hardline floor still blocks.
    Off,
}

/// A single command-policy rule: a name, a human-readable description, the
/// action to take on match, and the regex source. `description` is surfaced
/// in the deny/warn message and audit log so an operator can tell *why* a
/// command was refused.
#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub name: &'static str,
    pub description: &'static str,
    pub action: RuleAction,
    pub pattern: &'static str,
}

/// The undisableable catastrophic floor.
///
/// Each entry targets an irreversible, never-legitimate shape inside a
/// per-session agent workspace (disk wipe, filesystem-root delete, fork bomb).
/// [`super::CommandPolicy::evaluate`] applies these regardless of the
/// configured [`EnforcementMode`], and the factory keeps them active even when
/// the tunable policy is disabled, so no config change can remove this floor.
/// All patterns are matched case-insensitively (see [`super::CommandPolicy`]).
#[must_use]
pub fn hardline_rules() -> Vec<PolicyRule> {
    use RuleAction::Block;
    vec![
        PolicyRule {
            name: "fork_bomb",
            description: "fork bomb — a self-piping backgrounded function that exhausts PIDs",
            action: Block,
            // Matches the structural shape `…(){ … | … & };` (e.g. the classic
            // `:(){ :|:& };:`). No backrefs in the regex crate, so this keys on
            // the function-body shape rather than name equality.
            pattern: r"\(\s*\)\s*\{[^}]*\|[^}]*&[^}]*\}\s*;",
        },
        PolicyRule {
            name: "rm_no_preserve_root",
            description: "rm --no-preserve-root — explicit request to delete the filesystem root",
            action: Block,
            pattern: r"\brm\b[^\n]*--no-preserve-root",
        },
        PolicyRule {
            name: "rm_rf_root",
            description: "recursive rm of the bare filesystem root (/ or /*) — irreversible whole-disk wipe",
            action: Block,
            // The catastrophic sibling of the tunable `rm_rf_system_path`: a
            // recursive remove whose target is the *bare* root `/` (or the root
            // glob `/*`), which `rm_no_preserve_root` misses entirely on the
            // platforms where it bites hardest — busybox/Alpine `rm -rf /` has
            // no `--preserve-root` guard, and GNU `rm -rf /*` expands the glob so
            // the `--preserve-root` refusal never triggers. Force is optional: in
            // the agent's non-interactive shell `rm -r /` deletes without a
            // prompt. A recursive flag (combined `-rf`/`-fr`, short `-r`/`-R`, or
            // long `--recursive`) plus a bare-root target is the precise,
            // never-legitimate shape — a subdir target (`/etc`, `/tmp/x`) does
            // not match because the char after `/` must be a terminator, leaving
            // those to the tunable `rm_rf_system_path` warn.
            pattern: r#"\brm\b(?:\s+-{1,2}\S+)*\s+(?:-[a-z]*r[a-z]*|--recursive)\b(?:\s+-{1,2}\S+)*\s+["']?/(?:\s|\*|$|["';&|])"#,
        },
        PolicyRule {
            name: "dd_to_block_device",
            description: "dd writing directly to a raw block device (disk-wipe / overwrite)",
            action: Block,
            pattern: concat!(
                r#"\bdd\b[^\n]*\bof\s*=\s*["']?/dev/"#,
                unix_block_device!()
            ),
        },
        PolicyRule {
            name: "mkfs_device",
            description: "mkfs formatting a device node (destroys an existing filesystem)",
            action: Block,
            pattern: r#"\bmkfs(\.\w+)?\b[^\n]*\s["']?/dev/"#,
        },
        PolicyRule {
            name: "redirect_to_block_device",
            description: "shell redirect overwriting a raw block device",
            action: Block,
            pattern: concat!(r#">\s*["']?/dev/"#, unix_block_device!()),
        },
        PolicyRule {
            name: "device_wipe_tools",
            description: "wipefs/blkdiscard/shred targeting a raw block device (destroys partition table / filesystem signatures)",
            action: Block,
            // `wipefs`/`blkdiscard` operate *only* on block devices, so a
            // `/dev/<class>` argument is enough. `shred` is also a legitimate
            // file-shredder (`shred -u secret.txt`), so it is catastrophic only
            // when its target is a raw device — hence the explicit `/dev/<class>`
            // requirement keeps file-level `shred` off the floor.
            pattern: concat!(
                r#"\b(?:wipefs|blkdiscard)\b[^\n]*\s["']?/dev/"#,
                unix_block_device!(),
                r#"|\bshred\b[^\n]*\s["']?/dev/"#,
                unix_block_device!()
            ),
        },
        // --- Windows catastrophic shapes (cmd.exe / PowerShell) -------------
        // The Unix floor above does not cover the native Windows command
        // surface an agent reaches through `cmd.exe /c …` or `powershell -c …`.
        // These are the Windows analogues — disk format, drive-root recursive
        // delete, shadow-copy and backup-catalog destruction (the ransomware
        // precursors), boot-config deletion, registry-hive wipe — each
        // essentially never legitimate in a per-session workspace. A leading
        // boundary keeps the common words (`format`, `del`) from matching flag
        // fragments such as `git log --format=` or a `--del` long-option.
        PolicyRule {
            name: "win_format_volume",
            description: "Windows `format <drive:>` / `Format-Volume` — wipes an entire volume",
            action: Block,
            pattern: r#"(?:^|[\s;&|("'])(?:format\s+(?:/\S+\s+)*["']?[a-z]:|format-volume\b)"#,
        },
        PolicyRule {
            name: "win_recursive_root_delete",
            description: "recursive delete of a bare drive root or registry hive root (cmd `del`/`rd /s`, PowerShell `Remove-Item -Recurse` and its aliases)",
            action: Block,
            // One rule for both dialects: cmd and PowerShell share the verb
            // namespace (`rm`/`rd`/`del` are all `Remove-Item` aliases), so
            // splitting them only produced two half-blind rules — the cmd one
            // did not know `-Recurse`, the PowerShell one did not know its own
            // aliases.
            //
            // Both argument orders are spelled out because the regex crate has
            // no lookaround, and both orders are valid shell: `del /s /q C:\`
            // and `del C:\ /s /q` do the same thing, but only the first matched
            // a flag-then-target pattern. codex reaches the same place with an
            // order-free token-set test (`has_delete && has_force`).
            pattern: concat!(
                win_delete_verb!(),
                "(?:",
                seg!(),
                r"\s",
                win_recursive_flag!(),
                seg!(),
                win_bare_root!(),
                "|",
                seg!(),
                win_bare_root!(),
                seg!(),
                win_recursive_flag!(),
                ")"
            ),
        },
        PolicyRule {
            name: "win_delete_shadow_copies",
            description: "volume shadow-copy destruction (vssadmin / wmic / Win32_ShadowCopy) — ransomware precursor",
            action: Block,
            pattern: concat!(
                r"\bvssadmin\b",
                seg!(),
                r"\bdelete\b",
                seg!(),
                r"\bshadows?\b|\bwmic\b",
                seg!(),
                r"\bshadowcopy\b",
                seg!(),
                // The WMI object form genuinely straddles a pipe
                // (`gwmi win32_shadowcopy | remove-wmiobject`), so that
                // alternative keeps the line-wide gap.
                r"\bdelete\b|\bwin32_shadowcopy\b[^\n]*\b(?:delete|remove-wmiobject|remove-ciminstance)\b"
            ),
        },
        PolicyRule {
            name: "win_backup_catalog_destruction",
            description: "`wbadmin delete catalog|backup|systemstatebackup` — destroys the Windows backup catalog, the other half of the shadow-copy ransomware precursor",
            action: Block,
            // The twin of `win_delete_shadow_copies`: real-world destruction
            // playbooks pair `vssadmin delete shadows` with
            // `wbadmin delete catalog`, and the floor previously knew only the
            // first of the two.
            pattern: concat!(
                r"\bwbadmin\b",
                seg!(),
                r"\bdelete\b",
                seg!(),
                r"\b(?:catalog|backup|systemstatebackup)\b|\bremove-wbbackupset\b"
            ),
        },
        PolicyRule {
            name: "win_disk_wipe_tools",
            description: "raw-disk destruction on Windows (`Clear-Disk -RemoveData`, `diskpart … clean`, a write to `\\\\.\\PhysicalDriveN`) — the Windows analogue of dd-to-/dev/sda",
            action: Block,
            // `\\.\PhysicalDriveN` reaches the normaliser as a plain
            // `PhysicalDriveN` (the device-namespace prefix is canonicalised
            // away), so no backslashes appear here.
            //
            // Coverage note: `diskpart /s script.txt` carries its `clean` in a
            // file this filter never sees. The OS sandbox is the backstop for
            // that form; the inline and piped spellings are what match here.
            pattern: concat!(
                r"\bclear-disk\b",
                seg!(),
                r"-remove(?:data|oem)\b|-remove(?:data|oem)\b",
                seg!(),
                r"\bclear-disk\b|\bdiskpart\b[^\n]*\bclean\b|\bclean\b[^\n]*\|[^\n]*\bdiskpart\b",
                r#"|(?:>|\bof\s*=)\s*["']?\\?\.?\\?physicaldrive\d"#
            ),
        },
        PolicyRule {
            name: "win_bcdedit_delete",
            description: "`bcdedit /delete` — destroys Windows boot configuration entries",
            action: Block,
            pattern: concat!(r"\bbcdedit\b", seg!(), r"/delete"),
        },
        PolicyRule {
            name: "win_boot_recovery_disable",
            description: "`bcdedit /set … recoveryenabled No` / `bootstatuspolicy ignoreallfailures` — disables Windows recovery so a damaged system cannot self-repair",
            action: Block,
            pattern: concat!(
                r"\bbcdedit\b",
                seg!(),
                r"/set\b",
                seg!(),
                r"\brecoveryenabled\b",
                seg!(),
                r"\bno\b|\bbcdedit\b",
                seg!(),
                r"/set\b",
                seg!(),
                r"\bbootstatuspolicy\b",
                seg!(),
                r"\bignoreallfailures\b"
            ),
        },
        PolicyRule {
            name: "win_registry_hive_delete",
            description: "`reg delete <HIVE> /f` of a whole root hive (HKLM / HKCU / …)",
            action: Block,
            // Whitespace right after the hive name = deleting the entire hive;
            // a subkey delete (`reg delete HKLM\Software\App /f`) has a `\`
            // there and is intentionally excluded.
            pattern: concat!(
                r"\breg\b\s+delete\s+(?:hklm|hkcu|hkcr|hku|hkey_local_machine|hkey_current_user|hkey_classes_root)\s+",
                seg!(),
                r"/f\b"
            ),
        },
    ]
}

/// The curated tunable ruleset.
///
/// High-signal shapes that can occasionally be legitimate, so they default to
/// `Warn` (audited, not refused) and respect the operator's [`EnforcementMode`].
/// Operators may append `[[sandbox.command_policy.custom_rules]]` of their own,
/// including `block`-action rules (which then respect the enforcement mode —
/// unlike the [`hardline_rules`] floor). All patterns are matched
/// case-insensitively (see [`super::CommandPolicy`]).
#[must_use]
pub fn default_rules() -> Vec<PolicyRule> {
    use RuleAction::Warn;
    vec![
        PolicyRule {
            name: "rm_rf_system_path",
            description: "recursive remove targeting an absolute root / system / home path",
            action: Warn,
            // Requires rm + a recursive flag + an absolute root / system / home
            // target on the same line. The recursive flag is matched as a
            // combined cluster (`-rf`/`-fr`/`-R`), a bare short `-r`, OR the long
            // `--recursive`, with any other flags before/after it — so the split
            // form `rm -r -f /etc` and the recursive-only `rm -r /etc` (which
            // deletes without a prompt in the agent's non-interactive shell) are
            // both caught. The previous pattern required the recursive *and*
            // force letters in a single token, so `rm -r -f /etc` and
            // `rm -r /etc` evaded it. Force is no longer required: `-r` alone is
            // destructive here. Relative targets (`build/`, `./target`) are
            // excluded by the absolute-path requirement.
            pattern: r#"\brm\b(?:\s+-{1,2}\S+)*\s+(?:-[a-z]*r[a-z]*|--recursive)\b(?:\s+-{1,2}\S+)*\s+["']?(?:/|~|\$HOME|/etc|/usr|/var|/bin|/boot|/lib|/sys|/root|/sbin)(?:\s|/|\*|$|[\x22\x27;&|])"#,
        },
        PolicyRule {
            name: "pipe_to_shell",
            description:
                "download piped straight into an interpreter (curl|wget … | sh/bash/python)",
            action: Warn,
            pattern: r"\b(?:curl|wget|fetch)\b[^\n|]*\|[^\n]*\b(?:sh|bash|zsh|ksh|python3?|perl|ruby|node)\b",
        },
        PolicyRule {
            name: "shell_eval_download",
            description:
                "interpreter executing downloaded content via process substitution or eval (bash <(curl …) / eval \"$(curl …)\")",
            action: Warn,
            // The `| sh` cradle has two common pipe-free siblings that evade
            // `pipe_to_shell`: process substitution fed to an interpreter
            // (`bash <(curl …)`, `source <(wget …)`) and command substitution
            // inside `eval` (`eval "$(curl …)"`). Both download-and-execute
            // without a literal `|`-into-shell, so they warrant the same paper
            // trail.
            pattern: r"\b(?:sh|bash|zsh|ksh|source|eval|python3?|perl|ruby|node)\b[^\n]*<\(\s*(?:curl|wget|fetch)\b|\beval\b[^\n]*\$\(\s*(?:curl|wget|fetch)\b",
        },
        PolicyRule {
            name: "chmod_777_system",
            description: "world-writable chmod 777 on an absolute root / system path",
            action: Warn,
            pattern: r"\bchmod\s+(?:-{1,2}\S+\s+)*[0-7]*777[0-7]*\s+(?:/|/\*|/etc|/usr|/bin|/var)(?:\s|/|$)",
        },
        PolicyRule {
            name: "write_sensitive_etc",
            description:
                "writing to a sensitive system credential file (/etc/passwd, shadow, sudoers)",
            action: Warn,
            pattern: r"(?:>|>>|\btee\b[^\n]*)\s*/etc/(?:passwd|shadow|sudoers|gshadow)\b",
        },
        PolicyRule {
            name: "reverse_shell_devtcp",
            description: "bash /dev/tcp reverse-shell or raw TCP socket exfiltration",
            action: Warn,
            pattern: r"/dev/tcp/\d",
        },
        PolicyRule {
            name: "system_shutdown",
            description:
                "host shutdown / reboot / poweroff — takes down the machine Aleph runs on (Unix shutdown/reboot/init 0|6/systemctl, Windows shutdown/Stop-Computer/Restart-Computer)",
            action: Warn,
            // High-signal but reversible (unlike the irreversible hardline floor),
            // so it audits rather than blocks and respects the enforcement mode.
            // `shutdown` is anchored to a following shutdown flag/`now`/`+<min>`
            // so an app subcommand like `nginx -s shutdown` (no such flag) does
            // not trip it, while `bash -c "shutdown -h now"` still does. `reboot`
            // / `poweroff` are command-position anchored. Windows `Stop-Computer`
            // / `Restart-Computer` and `shutdown /s|/r` are covered too.
            pattern: r"\bshutdown\b[^\n]*(?:\s/[sr]\b|\s-{1,2}(?:h|r|p|halt|reboot|poweroff)\b|\bnow\b|\s\+\d)|(?:^|[\s;&|(])(?:reboot|poweroff)\b|\bsystemctl\b[^\n]*\b(?:poweroff|reboot|halt)\b|\b(?:init|telinit)\s+[06]\b|\b(?:stop-computer|restart-computer)\b",
        },
        PolicyRule {
            name: "sudo_privilege_stdin",
            description:
                "sudo reading a password from stdin (-S/--stdin/--askpass) or spawning a root shell (-s) — password-guessing / privilege-escalation vector",
            action: Warn,
            // Targets sudo's *own* leading options (only flag tokens may precede
            // the match), so `sudo apt-get install -s` — where `-s` is the
            // wrapped command's flag, not sudo's — does not trip it. Matches
            // `-S` (case-insensitive, also `-s`), `--stdin`, `--askpass`.
            pattern: r"\bsudo\b(?:\s+-{1,2}\S+)*\s+-{1,2}(?:s|stdin|askpass)\b",
        },
        PolicyRule {
            name: "write_ssh_authorized_keys",
            description:
                "writing/copying into ~/.ssh/authorized_keys (SSH backdoor key persistence)",
            action: Warn,
            // Appending a public key to authorized_keys is the classic SSH
            // backdoor — essentially never a legitimate workspace task. Covers
            // redirect (`>>`/`>`), `tee`, in-place `sed -i`, and `cp`/`mv`/
            // `install` whose target path ends in `.ssh/authorized_keys`.
            pattern: r"(?:>>?|\btee\b|\bsed\b[^\n]*\s-\S*i|\bcp\b|\bmv\b|\binstall\b)[^\n]*\.ssh/authorized_keys\b",
        },
        // --- Windows high-signal shapes (cmd.exe / PowerShell) -------------
        // Audited, not blocked: each is occasionally legitimate (an installer,
        // a CI bootstrap, an ops runbook) but is the Windows analogue of a Unix
        // shape this ruleset already audits — the `curl|sh` download cradle, the
        // `authorized_keys` backdoor, the "disable my own defences" family.
        PolicyRule {
            name: "win_rm_system_path",
            description: "recursive delete targeting a Windows system location (\\Windows, \\Program Files, \\ProgramData, the bare \\Users root, %SystemRoot%)",
            action: Warn,
            // The Windows twin of `rm_rf_system_path`, and the reason the
            // normaliser keeps a path-preserving view at all: the POSIX reading
            // folds `C:\Windows` into `C:Windows`, which no rule can name.
            pattern: concat!(
                win_delete_verb!(),
                "(?:",
                seg!(),
                r"\s",
                win_recursive_flag!(),
                seg!(),
                win_system_path!(),
                "|",
                seg!(),
                win_system_path!(),
                seg!(),
                win_recursive_flag!(),
                ")"
            ),
        },
        PolicyRule {
            name: "win_download_cradle",
            description:
                "PowerShell download-and-execute cradle (IEX of DownloadString, iwr|iex, certutil -urlcache, bitsadmin /transfer)",
            action: Warn,
            pattern: r"\b(?:iex|invoke-expression)\b[^\n]*\b(?:downloadstring|downloaddata|invoke-webrequest|invoke-restmethod|iwr|irm|webclient)\b|\b(?:iwr|irm|invoke-webrequest|invoke-restmethod)\b[^\n]*\|[^\n]*\b(?:iex|invoke-expression)\b|\bcertutil\b[^\n]*-urlcache\b[^\n]*\bhttp|\bbitsadmin\b[^\n]*/transfer\b",
        },
        PolicyRule {
            name: "win_encoded_command",
            description:
                "PowerShell `-EncodedCommand <base64>` — the script is hidden from plain reading (its decoded text is scanned separately by every other rule)",
            action: Warn,
            // The payload itself is unwrapped by `normalize::normalize_for_matching`
            // and judged on its merits, so this rule is purely the paper trail
            // for *having encoded it*: a clean payload still runs.
            pattern: concat!(
                r"\b(?:powershell|pwsh)(?:\.exe)?\b",
                seg!(),
                r#"\s[-/]e(?:c|n[a-z]*)?\s+["']?[a-z0-9+/]{20,}"#
            ),
        },
        PolicyRule {
            name: "win_disable_defender",
            description: "disabling Microsoft Defender (Set-MpPreference -DisableRealtimeMonitoring / -ExclusionPath, or stopping/deleting the WinDefend service)",
            action: Warn,
            pattern: concat!(
                r"\b(?:set-mppreference|add-mppreference)\b[^\n]*-(?:disablerealtimemonitoring|exclusionpath|exclusionprocess|exclusionextension|disableioavprotection)\b",
                r"|\b(?:sc|net)\b",
                seg!(),
                r"\b(?:stop|delete|config)\b",
                seg!(),
                r"\bwindefend\b|\b(?:stop-service|set-service)\b",
                seg!(),
                r"\bwindefend\b|\buninstall-windowsfeature\b",
                seg!(),
                r"\bwindows-defender\b"
            ),
        },
        PolicyRule {
            name: "win_disable_firewall",
            description: "disabling the Windows firewall (netsh advfirewall set … state off / firewall set opmode disable)",
            action: Warn,
            pattern: r"\bnetsh\b[^\n]*\badvfirewall\b[^\n]*\bset\b[^\n]*\bstate\b[^\n]*\boff\b|\bnetsh\b[^\n]*\bfirewall\b[^\n]*\bset\b[^\n]*\bopmode\b[^\n]*\bdisable\b",
        },
        PolicyRule {
            name: "win_execution_policy_bypass",
            description: "weakening the PowerShell execution policy (`Set-ExecutionPolicy Bypass|Unrestricted`, `-ExecutionPolicy Bypass`)",
            action: Warn,
            pattern: concat!(
                r"\bexecutionpolicy\b",
                seg!(),
                r"\b(?:bypass|unrestricted)\b"
            ),
        },
        PolicyRule {
            name: "win_amsi_bypass",
            description: "AMSI tampering (AmsiUtils / amsiInitFailed / AmsiScanBuffer) — disables in-process script scanning",
            action: Warn,
            pattern: r"\bamsiutils\b|\bamsiinitfailed\b|\bamsiscanbuffer\b|\bamsicontext\b",
        },
        PolicyRule {
            name: "win_event_log_clear",
            description: "clearing Windows event logs (`wevtutil cl`, `Clear-EventLog`) — anti-forensics",
            action: Warn,
            pattern: concat!(
                r"\bwevtutil\b",
                seg!(),
                r"\b(?:cl|clear-log)\b|\b(?:clear-eventlog|remove-eventlog)\b"
            ),
        },
        PolicyRule {
            name: "win_local_admin_backdoor",
            description: "creating a local account or adding one to Administrators (`net user … /add`, `net localgroup administrators … /add`) — the Windows twin of an authorized_keys backdoor",
            action: Warn,
            pattern: concat!(
                r"\bnet\b",
                seg!(),
                r"\b(?:user|localgroup)\b",
                seg!(),
                r"/add\b|\b(?:new-localuser|add-localgroupmember)\b"
            ),
        },
        PolicyRule {
            name: "win_registry_run_persistence",
            description: "registry autorun persistence (`reg add … \\CurrentVersion\\Run`)",
            action: Warn,
            // The trailing backslash is optional because the POSIX normalisation
            // view has folded path separators away
            // (`…\CurrentVersion\Run` → `…CurrentVersionRun`); the native view
            // still carries them.
            pattern: r"\breg\b\s+add\b[^\n]*currentversion\\?run(?:once)?\b",
        },
        PolicyRule {
            name: "win_persistence_task_service",
            description: "scheduled-task or service persistence (`schtasks /create`, `Register-ScheduledTask`, `sc create … binPath=`, `New-Service`)",
            action: Warn,
            // The sibling of `win_registry_run_persistence` — same goal, the two
            // other autostart surfaces Windows offers.
            pattern: concat!(
                r"\bschtasks\b",
                seg!(),
                r"/create\b|\bregister-scheduledtask\b|\bnew-service\b|\bsc\b",
                seg!(),
                r"\bcreate\b",
                seg!(),
                r"\bbinpath\b"
            ),
        },
        PolicyRule {
            name: "win_acl_takeover_root",
            description: "rewriting ownership / ACLs on a drive or hive root (`takeown /f C:\\ /r`, `icacls C:\\ /grant …`) — makes the system unbootable as surely as deleting it",
            action: Warn,
            pattern: concat!(
                r"\btakeown\b",
                seg!(),
                r"/f\b",
                seg!(),
                win_bare_root!(),
                seg!(),
                r"/r\b|\bicacls\b",
                seg!(),
                win_bare_root!(),
                seg!(),
                r"/(?:grant|deny|reset|setowner|inheritance)\b"
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulesets_are_nonempty_and_globally_unique() {
        let hardline = hardline_rules();
        let tunable = default_rules();
        assert!(hardline.len() >= 5, "expected the full catastrophic floor");
        assert!(tunable.len() >= 4, "expected a meaningful tunable ruleset");

        // Names must be unique *across both tiers* — they all land in the same
        // `blocked`/`warned` vectors, so a collision would be ambiguous.
        let mut names: Vec<&str> = hardline
            .iter()
            .chain(tunable.iter())
            .map(|r| r.name)
            .collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "rule names must be globally unique");
    }

    #[test]
    fn hardline_rules_are_all_block_action() {
        assert!(
            hardline_rules()
                .iter()
                .all(|r| r.action == RuleAction::Block),
            "the catastrophic floor must be block-action"
        );
    }

    #[test]
    fn tunable_rules_are_all_warn_action() {
        // The curated tunable set is an audit layer: anything that should
        // *refuse* belongs on the undisableable floor instead, where an
        // operator's `enforcement` setting cannot silence it.
        assert!(
            default_rules().iter().all(|r| r.action == RuleAction::Warn),
            "curated tunable rules audit rather than refuse"
        );
    }

    #[test]
    fn enforcement_and_action_defaults() {
        assert_eq!(EnforcementMode::default(), EnforcementMode::Block);
        assert_eq!(RuleAction::default(), RuleAction::Block);
    }

    #[test]
    fn action_serde_roundtrip_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&RuleAction::Block).unwrap(),
            "\"block\""
        );
        assert_eq!(
            serde_json::from_str::<RuleAction>("\"warn\"").unwrap(),
            RuleAction::Warn
        );
    }

    #[test]
    fn shared_fragments_are_spliced_not_copied() {
        // The device class exists once; every rule that needs it must carry the
        // identical text. A hand-edited copy would show up here as a rule that
        // no longer contains the canonical fragment.
        let hardline = hardline_rules();
        for name in [
            "dd_to_block_device",
            "redirect_to_block_device",
            "device_wipe_tools",
        ] {
            let rule = hardline
                .iter()
                .find(|r| r.name == name)
                .expect("rule present");
            assert!(
                rule.pattern.contains(unix_block_device!()),
                "{name} must splice the canonical device class"
            );
        }
    }
}
