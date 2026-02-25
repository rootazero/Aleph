//! Dynamic prompt template engine for cron jobs.

use std::sync::LazyLock;

use crate::cron::config::{CronJob, JobRun};

static ENV_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{env:(\w+)\}\}").unwrap());

/// Render a prompt template with variable substitution.
///
/// Built-in variables:
/// - `{{now}}` — current time ISO 8601
/// - `{{now_unix}}` — Unix timestamp (seconds)
/// - `{{job_name}}` — job name
/// - `{{last_output}}` — previous run's response
/// - `{{run_count}}` — total execution count
/// - `{{env:VAR}}` — environment variable
pub fn render_template(
    template: &str,
    job: &CronJob,
    last_run: Option<&JobRun>,
    run_count: u64,
) -> String {
    let now = chrono::Utc::now();
    let mut result = template.to_string();

    if result.contains("{{now}}") {
        result = result.replace("{{now}}", &now.to_rfc3339());
    }
    if result.contains("{{now_unix}}") {
        result = result.replace("{{now_unix}}", &now.timestamp().to_string());
    }
    if result.contains("{{job_name}}") {
        result = result.replace("{{job_name}}", &job.name);
    }
    if result.contains("{{run_count}}") {
        result = result.replace("{{run_count}}", &run_count.to_string());
    }
    if result.contains("{{last_output}}") {
        let last = last_run
            .and_then(|r| r.response.as_deref())
            .unwrap_or("(first run)");
        result = result.replace("{{last_output}}", last);
    }

    // Environment variables: {{env:VAR_NAME}}
    result = ENV_RE
        .replace_all(&result, |caps: &regex::Captures| {
            std::env::var(&caps[1]).unwrap_or_default()
        })
        .to_string();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_basic_variables() {
        let job = CronJob::new("Daily News", "0 0 9 * * *", "main", "unused");
        let result = render_template("Hello {{job_name}}, run #{{run_count}}", &job, None, 5);
        assert_eq!(result, "Hello Daily News, run #5");
    }

    #[test]
    fn test_render_now_variables() {
        let job = CronJob::new("Test", "0 0 * * * *", "main", "unused");
        let result = render_template("Time: {{now}}", &job, None, 0);
        assert!(result.starts_with("Time: 20"));
    }

    #[test]
    fn test_render_last_output_with_run() {
        let job = CronJob::new("Test", "0 0 * * * *", "main", "unused");
        let run = JobRun::new("job-1").success(Some("AI response here".to_string()));
        let result = render_template("Based on: {{last_output}}", &job, Some(&run), 1);
        assert_eq!(result, "Based on: AI response here");
    }

    #[test]
    fn test_render_last_output_first_run() {
        let job = CronJob::new("Test", "0 0 * * * *", "main", "unused");
        let result = render_template("Prev: {{last_output}}", &job, None, 0);
        assert_eq!(result, "Prev: (first run)");
    }

    #[test]
    fn test_render_env_variable() {
        std::env::set_var("ALEPH_CRON_TEST_VAR", "hello_world");
        let job = CronJob::new("Test", "0 0 * * * *", "main", "unused");
        let result = render_template("Val: {{env:ALEPH_CRON_TEST_VAR}}", &job, None, 0);
        assert_eq!(result, "Val: hello_world");
        std::env::remove_var("ALEPH_CRON_TEST_VAR");
    }

    #[test]
    fn test_render_no_templates() {
        let job = CronJob::new("Test", "0 0 * * * *", "main", "unused");
        let result = render_template("Plain text no variables", &job, None, 0);
        assert_eq!(result, "Plain text no variables");
    }

    #[test]
    fn test_render_unknown_variable_preserved() {
        let job = CronJob::new("Test", "0 0 * * * *", "main", "unused");
        let result = render_template("{{unknown_var}}", &job, None, 0);
        assert_eq!(result, "{{unknown_var}}");
    }
}
