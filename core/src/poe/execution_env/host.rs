//! Direct host execution environment.

use super::{CommandOutput, ExecutionEnvironment};
use async_trait::async_trait;
use std::path::Path;
use std::time::Instant;

/// Direct host execution (current behavior, no sandboxing).
pub struct HostEnvironment;

impl HostEnvironment {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HostEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecutionEnvironment for HostEnvironment {
    async fn execute_command(
        &self,
        cmd: &str,
        args: &[String],
        timeout_ms: u64,
        working_dir: Option<&Path>,
    ) -> crate::error::Result<CommandOutput> {
        let start = Instant::now();

        let mut command = tokio::process::Command::new(cmd);
        command.args(args);

        if let Some(dir) = working_dir {
            command.current_dir(dir);
        }

        let future = command.output();
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);

        let result = tokio::time::timeout(timeout_duration, future)
            .await
            .map_err(|_| {
                crate::error::AlephError::other(format!(
                    "Command '{}' timed out after {}ms",
                    cmd, timeout_ms
                ))
            })?
            .map_err(|e| {
                crate::error::AlephError::other(format!("Failed to execute command '{}': {}", cmd, e))
            })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(CommandOutput {
            exit_code: result.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            duration_ms,
        })
    }

    fn name(&self) -> &str {
        "host"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_host_executes_echo() {
        let env = HostEnvironment::new();
        let result = env
            .execute_command("echo", &["hello".to_string()], 5000, None)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_host_timeout() {
        let env = HostEnvironment::new();
        let result = env
            .execute_command("sleep", &["10".to_string()], 100, None)
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("timed out"));
    }

    #[tokio::test]
    async fn test_host_captures_exit_code() {
        let env = HostEnvironment::new();
        let result = env
            .execute_command("false", &[], 5000, None)
            .await
            .unwrap();

        assert_ne!(result.exit_code, 0);
    }
}
