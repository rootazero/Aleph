//! Cross-platform argv parser for the `sandbox-init-windows`
//! subcommand. Not `cfg`-gated so it unit-tests on macOS / Linux dev
//! boxes; only the Windows [`super::run_init`] entry point consumes the
//! parsed result.

use super::policy::WindowsInitPolicy;

/// Output of `parse_init_args`. `target_args` is the slice after `--`.
/// `dead_code` allow: only the Windows `run_init` consumes it; unit tests
/// reference it cross-platform.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct ParsedInitArgs {
    pub(crate) policy: WindowsInitPolicy,
    pub(crate) target: String,
    pub(crate) target_args: Vec<String>,
}

/// argv layout: `[--policy <json> -- <target> <target-args...>]`.
/// The leading `sandbox-init-windows` subcommand name is stripped by
/// the CLI dispatcher before calling `run_init`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn parse_init_args(args: &[String]) -> Result<ParsedInitArgs, String> {
    let mut policy_json: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--policy" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--policy requires a value".to_string())?;
                policy_json = Some(v.as_str());
                i += 2;
            }
            "--" => {
                i += 1;
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let policy_str = policy_json.ok_or_else(|| "missing --policy".to_string())?;
    let policy: WindowsInitPolicy =
        serde_json::from_str(policy_str).map_err(|e| format!("--policy JSON parse error: {e}"))?;

    let target = args
        .get(i)
        .ok_or_else(|| "missing target program after `--`".to_string())?
        .clone();
    let target_args = args[i + 1..].to_vec();

    Ok(ParsedInitArgs {
        policy,
        target,
        target_args,
    })
}
