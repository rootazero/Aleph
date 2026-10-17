//! QA-only native credential execution trace (feature `qa-owned-root`).
//!
//! The QA harness needs evidence that the shell's own `panel_credential_*`
//! commands actually RAN and what shape their outcome had. That evidence must
//! not come from the page: a page-side callback (even one that merely copies
//! native fields) can be forged by any script on the page, and the Tauri JS
//! internals (`__TAURI_INTERNALS__.invoke`) are immutable, so they cannot be
//! wrapped either.
//!
//! So the evidence is emitted here, by Rust, AFTER the command's real work
//! (including the post-operation lease re-check) has finished, onto this
//! process' own stdout. The harness spawns the shell with stdout piped to
//! itself, so the line travels over a pipe bound to that exact child process;
//! the page has no way to write to it.
//!
//! One line per command execution, preceded by a newline so it always starts a
//! fresh line even if other output was unterminated:
//!
//! ```text
//! ALEPH_QA_TRACE {"v":1,"auth":"<root nonce>","pid":N,"seq":N,"command":"panel_credential_load","ok":true,"resultShape":"lease_object","errorCode":null,"credentialPresent":false}
//! ```
//!
//! * `seq` is a process-wide counter, starting at 1, assigned and written under
//!   one lock, so line order == `seq` order. A lost line shows as a gap.
//! * Only fixed enums, booleans and integers are emitted: never the credential,
//!   the lease id, the Gateway URL, the command arguments or a raw error.
//! * Without a proven owned root and monitor nonce nothing is emitted.
//!
//! This module is compiled ONLY into the QA build; production builds contain no
//! trace code and the command handlers carry no trace call (the calls are
//! `#[cfg(feature = "qa-owned-root")]`).

use std::io::Write;
use std::sync::Mutex;

/// Line prefix the harness looks for (must equal `TRACE_PREFIX` in
/// `qa/native_ipc/monitor_contract.mjs`).
pub(crate) const PREFIX: &str = "ALEPH_QA_TRACE ";

/// Wire version (== `TRACE_VERSION` in `monitor_contract.mjs`).
const VERSION: u32 = 1;

/// Process-wide record counter. The lock is held across the stdout write.
static SEQ: Mutex<u64> = Mutex::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Load,
    Save,
    Delete,
}

impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::Load => "panel_credential_load",
            Self::Save => "panel_credential_save",
            Self::Delete => "panel_credential_delete",
        }
    }
}

/// Outcome SHAPE of one command execution. No value can be carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// `shape` is a fixed label (`lease_object` / `null` / `bool`);
    /// `credential_present` is only meaningful for a successful load.
    Ok {
        shape: &'static str,
        credential_present: Option<bool>,
    },
    /// Fixed snake_case native error code (`BridgeError`).
    Err(&'static str),
}

/// Build one trace line body (no prefix, no newline). `auth` must already be
/// hex-only and the labels fixed enums, so no JSON escaping is needed.
fn format_record(auth: &str, pid: u32, seq: u64, command: Command, outcome: Outcome) -> String {
    let (ok, shape, code, present) = match outcome {
        Outcome::Ok {
            shape,
            credential_present,
        } => (
            true,
            format!("\"{shape}\""),
            "null".to_owned(),
            credential_present.map_or_else(|| "null".to_owned(), |b| b.to_string()),
        ),
        Outcome::Err(code) => (
            false,
            "null".to_owned(),
            format!("\"{code}\""),
            "null".to_owned(),
        ),
    };
    format!(
        "{{\"v\":{VERSION},\"auth\":\"{auth}\",\"pid\":{pid},\"seq\":{seq},\"command\":\"{}\",\"ok\":{ok},\"resultShape\":{shape},\"errorCode\":{code},\"credentialPresent\":{present}}}",
        command.name()
    )
}

/// Record one finished command execution on stdout. Best effort by design: a
/// failure to write never changes the command's result, and the harness
/// detects any missing record as a `seq` gap or a timeout (fail closed).
pub(crate) fn record(command: Command, outcome: Outcome) {
    let Some(auth) = crate::qa_root::monitor_auth_nonce() else {
        return;
    };
    if auth.is_empty() || !auth.bytes().all(|b| b.is_ascii_hexdigit()) {
        return;
    }
    let Ok(mut seq) = SEQ.lock() else {
        return;
    };
    *seq = seq.saturating_add(1);
    let line = format_record(auth, std::process::id(), *seq, command, outcome);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(format!("\n{PREFIX}{line}\n").as_bytes());
    let _ = out.flush();
}

/// Async-handler entry: run [`record`] on the blocking pool so the stdout lock
/// and write (which can block if the harness stops draining the pipe) never
/// stall an async runtime worker. Awaited by the caller, so the line is still
/// emitted AFTER the command's real result exists and BEFORE it is returned.
pub(crate) async fn record_blocking(command: Command, outcome: Outcome) {
    let _ = tauri::async_runtime::spawn_blocking(move || record(command, outcome)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn load_ok_line_has_fixed_shape() {
        let s = format_record(
            AUTH,
            42,
            1,
            Command::Load,
            Outcome::Ok {
                shape: "lease_object",
                credential_present: Some(false),
            },
        );
        assert_eq!(
            s,
            format!(
                "{{\"v\":1,\"auth\":\"{AUTH}\",\"pid\":42,\"seq\":1,\"command\":\"panel_credential_load\",\"ok\":true,\"resultShape\":\"lease_object\",\"errorCode\":null,\"credentialPresent\":false}}"
            )
        );
    }

    #[test]
    fn save_ok_and_error_lines() {
        let ok = format_record(
            AUTH,
            7,
            2,
            Command::Save,
            Outcome::Ok {
                shape: "null",
                credential_present: None,
            },
        );
        assert!(ok.contains("\"command\":\"panel_credential_save\""));
        assert!(ok.contains("\"resultShape\":\"null\""));
        assert!(ok.contains("\"credentialPresent\":null"));
        let err = format_record(AUTH, 7, 3, Command::Delete, Outcome::Err("stale_lease"));
        assert!(err.contains("\"ok\":false"));
        assert!(err.contains("\"resultShape\":null"));
        assert!(err.contains("\"errorCode\":\"stale_lease\""));
    }

    #[test]
    fn line_is_bounded_and_carries_no_free_text() {
        let long_auth = "a".repeat(128);
        let s = format_record(
            &long_auth,
            u32::MAX,
            u64::MAX,
            Command::Delete,
            Outcome::Err("lease_exhausted"),
        );
        assert!(s.len() + PREFIX.len() < 512, "len {}", s.len());
    }
}
