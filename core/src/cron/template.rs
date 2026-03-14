//! Dynamic prompt template engine for cron jobs.

use std::sync::LazyLock;

use crate::cron::clock::Clock;
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
    clock: &dyn Clock,
) -> String {
    let now = clock.now_utc();
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
    use crate::cron::clock::SystemClock;
    use crate::cron::config::ScheduleKind;

    fn make_job(name: &str) -> CronJob {
        CronJob::new(
            name,
            "main",
            "unused",
            ScheduleKind::Cron {
                expr: "0 0 * * * *".to_string(),
                tz: None,
                stagger_ms: None,
            },
        )
    }

    #[test]
    fn test_render_basic_variables() {
        let clock = SystemClock;
        let job = make_job("Daily News");
        let result =
            render_template("Hello {{job_name}}, run #{{run_count}}", &job, None, 5, &clock);
        assert_eq!(result, "Hello Daily News, run #5");
    }

    #[test]
    fn test_render_now_variables() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("Time: {{now}}", &job, None, 0, &clock);
        assert!(result.starts_with("Time: 20"));
    }

    #[test]
    fn test_render_last_output_with_run() {
        let clock = SystemClock;
        let job = make_job("Test");
        let run = JobRun::new("job-1").success(Some("AI response here".to_string()));
        let result =
            render_template("Based on: {{last_output}}", &job, Some(&run), 1, &clock);
        assert_eq!(result, "Based on: AI response here");
    }

    #[test]
    fn test_render_last_output_first_run() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("Prev: {{last_output}}", &job, None, 0, &clock);
        assert_eq!(result, "Prev: (first run)");
    }

    #[test]
    fn test_render_env_variable() {
        let clock = SystemClock;
        std::env::set_var("ALEPH_CRON_TEST_VAR", "hello_world");
        let job = make_job("Test");
        let result =
            render_template("Val: {{env:ALEPH_CRON_TEST_VAR}}", &job, None, 0, &clock);
        assert_eq!(result, "Val: hello_world");
        std::env::remove_var("ALEPH_CRON_TEST_VAR");
    }

    #[test]
    fn test_render_no_templates() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("Plain text no variables", &job, None, 0, &clock);
        assert_eq!(result, "Plain text no variables");
    }

    #[test]
    fn test_render_unknown_variable_preserved() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("{{unknown_var}}", &job, None, 0, &clock);
        assert_eq!(result, "{{unknown_var}}");
    }

    #[test]
    fn test_render_with_fake_clock() {
        use crate::cron::clock::testing::FakeClock;
        // 2025-06-15T12:00:00Z
        let clock = FakeClock::new(1_750_003_200_000);
        let job = make_job("Test");
        let result = render_template("Time: {{now}}", &job, None, 0, &clock);
        assert!(result.contains("2025-06-15"));
    }
}
