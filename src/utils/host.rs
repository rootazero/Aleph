//! The machine's hostname — one source, one answer.
//!
//! Two call sites used to hand-roll this as `std::env::var("HOSTNAME")` with a
//! `COMPUTERNAME` fallback and `"unknown"` as the floor. Both were wrong in
//! production and wrong in the same silent way: `HOSTNAME` is a *shell*
//! variable, not an exported environment variable, so a daemon started by
//! launchd / systemd / a service manager — which is every production
//! `aleph-server` — inherits neither name. `RuntimeContext` therefore printed
//! `- **Host**: unknown` into the **cacheable** half of every system prompt,
//! and the A2A agent card advertised itself as `aleph-unknown`.
//!
//! This is the same defect class §2.3 already fixed one field over: `shell`
//! used to read `$SHELL` (the human's login shell) and was constantly
//! `unknown` on Windows while `code_exec` unconditionally spawned bash. The
//! fix there was to read the fact from whoever owns it. The owner here is the
//! OS, and the `hostname` crate — **already a direct dependency of
//! `alephcore`, with zero `use` sites before this module** — is the portable
//! way to ask it.
//!
//! Cached for the process lifetime: the answer cannot change while the daemon
//! runs (a rename requires a reboot on macOS/Windows and does not propagate to
//! a running process on Linux), and it is read on the per-turn prompt-assembly
//! path.

use std::sync::OnceLock;

/// Value returned when the OS refuses to name the machine *and* no environment
/// override is present. A literal rather than an empty string so the prompt's
/// `- **Host**:` bullet and the A2A card id stay well-formed.
pub const UNKNOWN_HOST: &str = "unknown";

static HOSTNAME: OnceLock<String> = OnceLock::new();

/// The machine's hostname, resolved once per process.
///
/// Resolution order:
/// 1. `hostname::get()` — the OS call (`gethostname` / `GetComputerNameExW`).
/// 2. `HOSTNAME` / `COMPUTERNAME` — kept as a **deliberate override**, not as
///    the primary: a container or a test harness that exports one is stating
///    an identity it wants used, and honouring it costs one `env::var`.
///    (Order is inverted from the code this replaces, which consulted only
///    these and therefore answered `unknown` on every real deployment.)
/// 3. [`UNKNOWN_HOST`].
#[must_use]
pub fn hostname() -> &'static str {
    HOSTNAME.get_or_init(resolve).as_str()
}

fn resolve() -> String {
    if let Ok(name) = hostname::get() {
        let name = name.to_string_lossy();
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| UNKNOWN_HOST.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this module exists to close: on any machine where `HOSTNAME` is
    /// not exported — macOS, and every service-manager-started Linux daemon —
    /// the old implementation returned the literal `"unknown"`. The OS always
    /// has an answer, so a green assertion here on a developer machine *and*
    /// in CI is the property, not a coincidence.
    #[test]
    fn resolves_to_a_real_name_even_without_the_hostname_env_var() {
        // Deliberately does NOT unset the env var (`std::env` is process-global
        // and libtest runs in parallel — see CLAUDE.md's "test switches must
        // not be environment variables"). Instead it asserts the OS path
        // directly, which is the path that used to be missing entirely.
        let os_name = hostname::get().expect("the OS can always name the host");
        assert!(
            !os_name.to_string_lossy().trim().is_empty(),
            "hostname::get() returned an empty name"
        );
        assert_eq!(
            hostname(),
            os_name.to_string_lossy().trim(),
            "the OS answer must win over any ambient HOSTNAME override"
        );
    }

    #[test]
    fn is_stable_across_calls() {
        assert_eq!(hostname(), hostname());
    }

    /// Guard against the two hand-rolled copies coming back. The rule is a
    /// source-level one because a runtime check cannot tell "read the env"
    /// from "read the env as a documented override" — this module is allowed
    /// to, nobody else is.
    #[test]
    fn no_other_module_hand_rolls_the_hostname_env_read() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders: Vec<String> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                if path.ends_with("utils/host.rs") {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Comments are documentation, not code — the same pre-filter
                // every source-level guard in this repo needs, because the
                // prose explaining why a name is gone is otherwise its own
                // search hit.
                for (i, line) in src.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") || code.starts_with("///") {
                        continue;
                    }
                    if code.contains("\"HOSTNAME\"") || code.contains("\"COMPUTERNAME\"") {
                        offenders.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "hostname is resolved by `utils::host::hostname()`; these read the env directly \
             and will answer \"unknown\" on every service-manager-started daemon: {offenders:?}"
        );
    }
}
