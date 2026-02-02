//! Cron job management command handlers

use crate::cli::CronAction;

/// Handle cron subcommands
#[cfg(feature = "gateway")]
pub async fn handle_cron_command(action: CronAction) -> Result<(), Box<dyn std::error::Error>> {
    use aethecore::cli::{cron, GatewayClient, OutputFormat};

    match action {
        CronAction::List { json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            cron::handle_list(&client, format).await?;
        }
        CronAction::Status { json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            cron::handle_status(&client, format).await?;
        }
        CronAction::Run { job_id, url } => {
            let client = GatewayClient::new().with_url(&url);
            cron::handle_run(&client, job_id).await?;
        }
    }

    Ok(())
}
