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

use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Canonical raw-block-device class shared by the `dd` / redirect / device-wipe
/// hardline rules. Written **once** here so a newly-covered device class can
/// never drift out of sync across the three device rules — the alternation was
/// previously pasted four separate times, and adding a class to one rule but
/// not another was a silent bypass (exactly the failure that let
/// `dd of=/dev/mapper/vg-root` escape the catastrophic floor). Covers:
/// SCSI/SATA (`sd`), NVMe, macOS (`disk`, which also matches Linux
/// `/dev/disk/by-*` symlinks), legacy IDE (`hd`), Xen/AWS-EC2 root volumes
/// (`xvd`), virtio (`vd`), SD/eMMC (`mmcblk`), loop, optical (`sr`), persistent
/// memory (`pmem`), device-mapper / LVM (`dm-`, `mapper`), software-RAID (`md`),
/// and the kernel-memory nodes (`mem`/`kmem`/`port`) a raw write to which
/// compromises or crashes the host. The alternation is anchored right after
/// `/dev/`, and every rule only tests match *presence*, so alternation order
/// does not affect matching.
const RAW_BLOCK_DEVICE_CLASS: &str =
    "sd|nvme|disk|hd|xvd|vd|mmcblk|loop|sr|pmem|dm-|mapper|md|mem|kmem|port";

/// Shared `rm` + recursive-flag prefix for the two recursive-remove rules
/// (hardline [`rm_rf_root`](hardline_rules) and tunable
/// [`rm_rf_system_path`](default_rules)), single-sourced so the notion of "a
/// recursive rm" cannot drift between the floor and the warn. Matches `rm`
/// then, in any flag order, a recursive flag — a combined cluster
/// (`-rf`/`-fr`/`-R`), a bare short `-r`, or long `--recursive` — with any other
/// flags allowed before/after. The trailing `\s+` leaves the scan positioned at
/// the target argument, which each rule then constrains.
const RM_RECURSIVE_PREFIX: &str =
    r"\brm\b(?:\s+-{1,2}\S+)*\s+(?:-[a-z]*r[a-z]*|--recursive)\b(?:\s+-{1,2}\S+)*\s+";

static DD_TO_BLOCK_DEVICE_RE: LazyLock<String> =
    LazyLock::new(|| format!(r#"\bdd\b[^\n]*\bof\s*=\s*["']?/dev/({RAW_BLOCK_DEVICE_CLASS})"#));

static REDIRECT_TO_BLOCK_DEVICE_RE: LazyLock<String> =
    LazyLock::new(|| format!(r#">\s*["']?/dev/({RAW_BLOCK_DEVICE_CLASS})"#));

static DEVICE_WIPE_TOOLS_RE: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"\b(?:wipefs|blkdiscard)\b[^\n]*\s["']?/dev/(?:{c})|\bshred\b[^\n]*\s["']?/dev/(?:{c})"#,
        c = RAW_BLOCK_DEVICE_CLASS
    )
});

static RM_RF_ROOT_RE: LazyLock<String> = LazyLock::new(|| {
    // Root target = one-or-more `/` (so `//`, `///` — all POSIX root — are
    // covered, closing the `rm -rf //` bypass), an optional trailing `.`
    // (`/.`), then a terminator or the root glob `*`. A redundant-slash *subdir*
    // (`//tmp`) does not match: the char after the slash run is a path segment,
    // not a terminator.
    format!(r#"{RM_RECURSIVE_PREFIX}["']?/+\.?(?:\s|\*|$|["';&|])"#)
});

static RM_RF_SYSTEM_PATH_RE: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"{RM_RECURSIVE_PREFIX}["']?(?:/|~|\$HOME|/etc|/usr|/var|/bin|/boot|/lib|/sys|/root|/sbin)(?:\s|/|\*|$|[\x22\x27;&|])"#
    )
});

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
            description: "recursive rm of the bare filesystem root (/, //, /. or /*) — irreversible whole-disk wipe",
            action: Block,
            // The catastrophic sibling of the tunable `rm_rf_system_path`: a
            // recursive remove whose target is the *bare* root `/` (or `//`,
            // `/.`, the root glob `/*`), which `rm_no_preserve_root` misses
            // entirely on the platforms where it bites hardest — busybox/Alpine
            // `rm -rf /` has no `--preserve-root` guard, and GNU `rm -rf /*`
            // expands the glob so the `--preserve-root` refusal never triggers.
            // Force is optional: in the agent's non-interactive shell `rm -r /`
            // deletes without a prompt. Recursive flag + bare-root target is the
            // precise, never-legitimate shape — a subdir target (`/etc`,
            // `/tmp/x`, `//tmp`) does not match because the char after the slash
            // run must be a terminator, leaving those to the tunable
            // `rm_rf_system_path` warn. Prefix + root fragment single-sourced in
            // `RM_RECURSIVE_PREFIX` / `RM_RF_ROOT_RE`.
            pattern: RM_RF_ROOT_RE.as_str(),
        },
        // Raw-block-device rules. The device class itself is single-sourced in
        // `RAW_BLOCK_DEVICE_CLASS` and composed into each pattern via a
        // `LazyLock` (`DD_TO_BLOCK_DEVICE_RE` / `REDIRECT_TO_BLOCK_DEVICE_RE` /
        // `DEVICE_WIPE_TOOLS_RE`), so a device class added there is covered by
        // all three at once — no more manual "keep in sync" across four pasted
        // copies.
        PolicyRule {
            name: "dd_to_block_device",
            description: "dd writing directly to a raw block device (disk-wipe / overwrite)",
            action: Block,
            pattern: DD_TO_BLOCK_DEVICE_RE.as_str(),
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
            pattern: REDIRECT_TO_BLOCK_DEVICE_RE.as_str(),
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
            pattern: DEVICE_WIPE_TOOLS_RE.as_str(),
        },
        // --- Windows catastrophic shapes (cmd.exe / PowerShell) -------------
        // The Unix floor above does not cover the native Windows command
        // surface an agent reaches through `cmd.exe /c …` or `powershell -c …`.
        // These are the Windows analogues — disk format, drive-root recursive
        // delete, shadow-copy destruction (ransomware precursor), boot-config
        // deletion, registry-hive wipe — each essentially never legitimate in a
        // per-session workspace. A leading `(?:^|[\s;&|(])` boundary keeps the
        // common words (`format`, `del`) from matching flag fragments such as
        // `git log --format=` or a `--del` long-option.
        PolicyRule {
            name: "win_format_volume",
            description: "Windows `format <drive:>` / `Format-Volume` — wipes an entire volume",
            action: Block,
            pattern: r"(?:^|[\s;&|(])(?:format\s+(?:/\S+\s+)*[a-z]:|format-volume\b)",
        },
        PolicyRule {
            name: "win_recursive_root_delete",
            description: "cmd `del`/`rd`/`rmdir /s` targeting a bare drive root (recursive wipe)",
            action: Block,
            // Requires the recursive `/s` switch AND a bare drive root
            // (`C:\`, `C:`, `C:\*`) — a recursive delete of a *subdir*
            // (`C:\Users\me\build`) does not match because the char after the
            // drive root must be a terminator, not another path segment.
            pattern: r"(?:^|[\s;&|(])(?:del|erase|rd|rmdir)\b[^\n]*\s/s\b[^\n]*[\s\x22\x27][a-z]:\\?(?:\*|\s|\x22|$)",
        },
        PolicyRule {
            name: "win_powershell_recursive_root_delete",
            description: "PowerShell `Remove-Item -Recurse` of a drive root or registry hive root",
            action: Block,
            // No `\b` before `-r`: a `\b` between a space and `-` never matches
            // (both non-word). The `-r(?:ecurse)?\b` flag plus a bare drive /
            // hive root (the normaliser has already stripped path backslashes,
            // so `\\?` is optional) is what makes this catastrophic-precise.
            pattern: r"\bremove-item\b[^\n]*-r(?:ecurse)?\b[^\n]*\s(?:[a-z]:\\?|hk(?:lm|cu|cr|u):\\?)(?:\*|\s|\x22|\x27|$)",
        },
        PolicyRule {
            name: "win_delete_shadow_copies",
            description: "volume shadow-copy destruction (vssadmin / wmic / Win32_ShadowCopy) — ransomware precursor",
            action: Block,
            pattern: r"\bvssadmin\b[^\n]*\bdelete\b[^\n]*\bshadows?\b|\bwmic\b[^\n]*\bshadowcopy\b[^\n]*\bdelete\b|\bwin32_shadowcopy\b[^\n]*\bdelete\b",
        },
        PolicyRule {
            name: "win_bcdedit_delete",
            description: "`bcdedit /delete` — destroys Windows boot configuration entries",
            action: Block,
            pattern: r"\bbcdedit\b[^\n]*/delete",
        },
        PolicyRule {
            name: "win_registry_hive_delete",
            description: "`reg delete <HIVE> /f` of a whole root hive (HKLM / HKCU / …)",
            action: Block,
            // Whitespace right after the hive name = deleting the entire hive;
            // a subkey delete (`reg delete HKLM\Software\App /f`) has a `\`
            // there and is intentionally excluded.
            pattern: r"\breg\b\s+delete\s+(?:hklm|hkcu|hkcr|hku|hkey_local_machine|hkey_current_user|hkey_classes_root)\s+[^\n]*/f\b",
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
            // target on the same line. The recursive flag (shared with the
            // hardline `rm_rf_root` via `RM_RECURSIVE_PREFIX`) is matched as a
            // combined cluster (`-rf`/`-fr`/`-R`), a bare short `-r`, OR the long
            // `--recursive`, with any other flags before/after it — so the split
            // form `rm -r -f /etc` and the recursive-only `rm -r /etc` (which
            // deletes without a prompt in the agent's non-interactive shell) are
            // both caught. Force is not required: `-r` alone is destructive here.
            // Relative targets (`build/`, `./target`) are excluded by the
            // absolute-path requirement.
            pattern: RM_RF_SYSTEM_PATH_RE.as_str(),
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
            name: "proc_sysrq_trigger",
            description:
                "writing the magic SysRq trigger (/proc/sysrq-trigger) — instant host crash/reboot (`echo c` panics the kernel, `echo b` reboots) bypassing a clean sync/shutdown",
            action: Warn,
            // Same reversible "host availability" tier as `system_shutdown` (the
            // machine comes back), so it audits rather than joins the
            // irreversible hardline floor — but it is a sneakier takedown than a
            // plain `reboot`, so it earns its own paper trail. Matches only a
            // *write* into the trigger (redirect `>`/`>>` or `tee`); reading
            // ordinary procfs stays clean.
            pattern: r"(?:>>?|\btee\b[^\n]*)\s*/proc/sysrq-trigger\b",
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
        // a CI bootstrap) but is the Windows analogue of the Unix `curl|sh`
        // download-cradle and the "disable my own defences" shapes worth a
        // paper trail.
        PolicyRule {
            name: "win_download_cradle",
            description:
                "PowerShell download-and-execute cradle (IEX of DownloadString, iwr|iex, certutil -urlcache, bitsadmin /transfer)",
            action: Warn,
            pattern: r"\b(?:iex|invoke-expression)\b[^\n]*\b(?:downloadstring|downloaddata|invoke-webrequest|invoke-restmethod|iwr|irm|webclient)\b|\b(?:iwr|irm|invoke-webrequest|invoke-restmethod)\b[^\n]*\|[^\n]*\b(?:iex|invoke-expression)\b|\bcertutil\b[^\n]*-urlcache\b[^\n]*\bhttp|\bbitsadmin\b[^\n]*/transfer\b",
        },
        PolicyRule {
            name: "win_disable_defender",
            description: "disabling Microsoft Defender (Set-MpPreference -DisableRealtimeMonitoring / Add-MpPreference -ExclusionPath)",
            action: Warn,
            pattern: r"\b(?:set-mppreference|add-mppreference)\b[^\n]*-(?:disablerealtimemonitoring|exclusionpath|exclusionprocess|exclusionextension|disableioavprotection)\b",
        },
        PolicyRule {
            name: "win_disable_firewall",
            description: "disabling the Windows firewall (netsh advfirewall set … state off / firewall set opmode disable)",
            action: Warn,
            pattern: r"\bnetsh\b[^\n]*\badvfirewall\b[^\n]*\bset\b[^\n]*\bstate\b[^\n]*\boff\b|\bnetsh\b[^\n]*\bfirewall\b[^\n]*\bset\b[^\n]*\bopmode\b[^\n]*\bdisable\b",
        },
        PolicyRule {
            name: "win_registry_run_persistence",
            description: "registry autorun persistence (`reg add … \\CurrentVersion\\Run`)",
            action: Warn,
            // Backslashes are optional: the normaliser strips path separators
            // (`…\CurrentVersion\Run` → `…CurrentVersionRun`) before matching,
            // so the keyword run is anchored without relying on the `\`.
            pattern: r"\breg\b\s+add\b[^\n]*currentversion\\?run(?:once)?\b",
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
}
