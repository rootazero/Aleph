#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    async fn test_daemon_cli_command_parsing() {
        use clap::Parser;

        let args = vec!["daemon", "install"];
        let cli = DaemonCli::try_parse_from(args);
        assert!(cli.is_ok());

        let cli = cli.unwrap();
        assert!(matches!(cli.command, DaemonCommand::Install));
    }
}
