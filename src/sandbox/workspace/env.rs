//! Environment-variable helpers for the workspace sandbox.

/// Codex-inspired env-friendly mechanism tag derived from a driver's
/// `os/mechanism` platform string. We strip the OS prefix so child
/// processes can branch on `ALEPH_SANDBOX=seatbelt|landlock|bwrap|token`
/// without parsing slashes. Falls back to the full string when no slash
/// is present.
pub(crate) fn sandbox_env_tag(platform: &'static str) -> &'static str {
    platform.rsplit('/').next().unwrap_or(platform)
}
