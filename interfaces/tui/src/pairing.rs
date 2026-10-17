//! First-time device pairing for `aleph-tui --pair`.
//!
//! The one-time pairing ticket (minted by `aleph-server pair`) is read from a
//! hidden interactive TTY prompt and nowhere else: not argv, not an
//! environment variable, not the config file, not stdin redirection. The
//! resulting [`DeviceCredential`] is a bootstrap ticket under a freshly minted
//! `tui-<uuid-v4>` device id; `aleph_tui::run_with_device_credential` performs
//! the exchange and saves the long-lived token to the OS keyring.
//!
//! Every error produced here is a fixed string: it never contains what the
//! user typed, and never echoes an underlying I/O error that might.

use std::io::{self, IsTerminal};

use aleph_client::{CliError, CliResult, DeviceCredential};

/// Fixed prompt text (hidden input; nothing is echoed).
pub const PAIR_PROMPT: &str = "Pairing ticket (input hidden): ";

/// Generous upper bound on ticket length, in bytes. Server tickets are short
/// codes; this only stops a pasted blob from being forwarded to the Gateway.
const MAX_TICKET_LEN: usize = 512;

const ERR_NOT_TTY: &str =
    "--pair needs an interactive terminal to read the pairing ticket; refusing to read it from a pipe or redirect";
const ERR_PROMPT: &str = "could not read the pairing ticket (prompt failed or was cancelled)";
const ERR_EMPTY: &str = "no pairing ticket entered";
const ERR_TOO_LONG: &str = "the pairing ticket entered is too long";
const ERR_TASK: &str = "pairing prompt task failed";

/// A fresh, unique device id: `tui-<uuid v4>`. The device *name* is only a
/// label; this id is the identity the Gateway binds the device token to.
pub fn new_device_id() -> String {
    format!("tui-{}", uuid::Uuid::new_v4())
}

/// Read the ticket through the injected `prompt` and wrap it as a bootstrap
/// credential. `stdin_is_terminal` is checked FIRST: when false, `prompt` is
/// never called. Blocking; call from a blocking context.
pub fn collect_bootstrap_credential(
    stdin_is_terminal: bool,
    prompt: impl FnOnce() -> io::Result<String>,
    device_id: String,
) -> CliResult<DeviceCredential> {
    if !stdin_is_terminal {
        return Err(CliError::Other(ERR_NOT_TTY.into()));
    }
    let mut ticket = prompt().map_err(|_| CliError::Other(ERR_PROMPT.into()))?;
    // Trim in place: no second copy of the secret is left behind.
    ticket.truncate(ticket.trim_end().len());
    let leading = ticket.len() - ticket.trim_start().len();
    ticket.drain(..leading);
    if ticket.is_empty() {
        return Err(CliError::Other(ERR_EMPTY.into()));
    }
    if ticket.len() > MAX_TICKET_LEN {
        return Err(CliError::Other(ERR_TOO_LONG.into()));
    }
    // Moved straight into the redacting wrapper; the raw String is never Debug-printed.
    DeviceCredential::bootstrap_ticket(device_id, ticket)
        .map_err(|_| CliError::Other(ERR_EMPTY.into()))
}

/// Production prompt: real stdin TTY check + `rpassword` hidden prompt, run on
/// the blocking pool so no async worker is stalled on terminal input.
pub async fn prompt_bootstrap_credential() -> CliResult<DeviceCredential> {
    tokio::task::spawn_blocking(|| {
        collect_bootstrap_credential(
            io::stdin().is_terminal(),
            || rpassword::prompt_password(PAIR_PROMPT),
            new_device_id(),
        )
    })
    .await
    .unwrap_or_else(|_| Err(CliError::Other(ERR_TASK.into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn ok_prompt(s: &'static str) -> impl FnOnce() -> io::Result<String> {
        move || Ok(s.to_string())
    }

    #[test]
    fn non_tty_stdin_is_rejected_without_calling_the_prompt() {
        let called = Cell::new(false);
        let err = collect_bootstrap_credential(
            false,
            || {
                called.set(true);
                Ok("secret-ticket".into())
            },
            "tui-x".into(),
        )
        .unwrap_err();
        assert!(!called.get());
        assert!(!err.to_string().contains("secret-ticket"));
    }

    #[test]
    fn blank_and_eof_input_give_a_fixed_error() {
        for input in ["", "   ", "\t \n"] {
            let err =
                collect_bootstrap_credential(true, move || Ok(input.to_string()), "tui-x".into())
                    .unwrap_err();
            assert_eq!(err.to_string(), ERR_EMPTY);
        }
        let eof = collect_bootstrap_credential(
            true,
            || {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "leaky-detail-ticket-123",
                ))
            },
            "tui-x".into(),
        )
        .unwrap_err();
        assert_eq!(eof.to_string(), ERR_PROMPT);
        assert!(!format!("{eof:?}").contains("leaky-detail"));
    }

    #[test]
    fn oversize_input_is_rejected_without_echo() {
        let big = "a".repeat(MAX_TICKET_LEN + 1);
        let err =
            collect_bootstrap_credential(true, || Ok(big.clone()), "tui-x".into()).unwrap_err();
        assert_eq!(err.to_string(), ERR_TOO_LONG);
    }

    #[test]
    fn valid_ticket_is_trimmed_wrapped_and_redacted() {
        let c =
            collect_bootstrap_credential(true, ok_prompt("  s3cret-ticket \n"), "tui-abc".into())
                .unwrap();
        assert!(c.is_bootstrap_ticket());
        assert_eq!(c.device_id(), "tui-abc");
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("s3cret-ticket"));
        assert!(dbg.contains("[redacted]"));
    }

    #[test]
    fn device_ids_are_prefixed_and_unique() {
        let a = new_device_id();
        let b = new_device_id();
        assert!(a.starts_with("tui-") && b.starts_with("tui-"));
        assert_ne!(a, b);
        assert!(uuid::Uuid::parse_str(&a["tui-".len()..]).is_ok());
    }
}
