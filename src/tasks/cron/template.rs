//! Dynamic prompt template engine for cron jobs.
//!
//! A job may carry a `prompt_template` with `{{var}}` placeholders instead of
//! a static `prompt`. The timer loop renders it at fire time via
//! [`resolve_job_prompt`], so a job can reference the current time, its own
//! previous output, a run counter, custom variables, or environment values.

use std::sync::LazyLock;

use crate::tasks::cron::config::CronJob;
use crate::tasks::shared::clock::Clock;

static ENV_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{env:(\w+)\}\}")
        .expect("ENV_RE pattern compiles: hardcoded literal")
});

/// Render a prompt template with variable substitution.
///
/// Built-in variables:
/// - `{{now}}` — current time ISO 8601
/// - `{{now_unix}}` — Unix timestamp (seconds)
/// - `{{job_name}}` — job name
/// - `{{last_output}}` — previous run's output (`(first run)` if none)
/// - `{{run_count}}` — number of completed runs so far
/// - `{{env:VAR}}` — environment variable
/// - `{{key}}` — any key from the job's JSON `context_vars`
pub fn render_template(
    template: &str,
    job: &CronJob,
    last_output: Option<&str>,
    run_count: u64,
    context_vars: Option<&str>,
    clock: &dyn Clock,
) -> String {
    let now = clock.now_utc();
    let mut result = template.to_string();

    // Substitute {{env:VAR}} FIRST on the original template text. Any
    // `{{env:...}}` token that arrives via `last_output` or `context_vars`
    // would otherwise be re-expanded against the live process environment
    // when those values are spliced in below — letting a previous run's
    // output, or a poisoned context_vars payload, exfiltrate secrets like
    // `{{env:AWS_SECRET_ACCESS_KEY}}` into the prompt.
    result = ENV_RE
        .replace_all(&result, |caps: &regex::Captures| {
            std::env::var(&caps[1]).unwrap_or_default()
        })
        .to_string();

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
        result = result.replace("{{last_output}}", last_output.unwrap_or("(first run)"));
    }

    // User-defined variables from the job's JSON `context_vars` object.
    if let Some(vars_json) = context_vars {
        match serde_json::from_str::<serde_json::Value>(vars_json) {
            Ok(serde_json::Value::Object(map)) => {
                for (key, value) in map {
                    let placeholder = format!("{{{{{key}}}}}");
                    if result.contains(&placeholder) {
                        let replacement = match value {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        };
                        result = result.replace(&placeholder, &replacement);
                    }
                }
            }
            Ok(_) => {
                tracing::warn!("job context_vars is not a JSON object, ignoring");
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse job context_vars JSON, ignoring");
            }
        }
    }

    result
}

/// Resolve a job's effective prompt: render `prompt_template` if the job has
/// one, otherwise return the static `prompt` unchanged.
pub fn resolve_job_prompt<C: Clock>(job: &CronJob, clock: &C) -> String {
    match &job.prompt_template {
        Some(template) => render_template(
            template,
            job,
            job.state.last_output.as_deref(),
            job.state.run_count,
            job.context_vars.as_deref(),
            clock,
        ),
        None => job.prompt.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::clock::SystemClock;
    use crate::tasks::cron::config::ScheduleKind;

    fn make_job(name: &str) -> CronJob {
        CronJob::new(
            name,
            "main",
            "static prompt",
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
        let result = render_template(
            "Hello {{job_name}}, run #{{run_count}}",
            &job,
            None,
            5,
            None,
            &clock,
        );
        assert_eq!(result, "Hello Daily News, run #5");
    }

    #[test]
    fn test_render_now_variables() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("Time: {{now}}", &job, None, 0, None, &clock);
        assert!(result.starts_with("Time: 20"));
    }

    #[test]
    fn test_render_last_output_with_value() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template(
            "Based on: {{last_output}}",
            &job,
            Some("AI response here"),
            1,
            None,
            &clock,
        );
        assert_eq!(result, "Based on: AI response here");
    }

    #[test]
    fn test_render_last_output_first_run() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("Prev: {{last_output}}", &job, None, 0, None, &clock);
        assert_eq!(result, "Prev: (first run)");
    }

    #[test]
    fn test_render_env_variable() {
        let clock = SystemClock;
        std::env::set_var("ALEPH_CRON_TEST_VAR", "hello_world");
        let job = make_job("Test");
        let result = render_template(
            "Val: {{env:ALEPH_CRON_TEST_VAR}}",
            &job,
            None,
            0,
            None,
            &clock,
        );
        assert_eq!(result, "Val: hello_world");
        std::env::remove_var("ALEPH_CRON_TEST_VAR");
    }

    #[test]
    fn test_render_context_vars() {
        let clock = SystemClock;
        let job = make_job("Test");
        let vars = r#"{"city":"Shanghai","limit":10}"#;
        let result = render_template(
            "Weather in {{city}}, top {{limit}}",
            &job,
            None,
            0,
            Some(vars),
            &clock,
        );
        assert_eq!(result, "Weather in Shanghai, top 10");
    }

    #[test]
    fn test_render_no_templates() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("Plain text no variables", &job, None, 0, None, &clock);
        assert_eq!(result, "Plain text no variables");
    }

    #[test]
    fn test_render_unknown_variable_preserved() {
        let clock = SystemClock;
        let job = make_job("Test");
        let result = render_template("{{unknown_var}}", &job, None, 0, None, &clock);
        assert_eq!(result, "{{unknown_var}}");
    }

    #[test]
    fn test_render_with_fake_clock() {
        use crate::tasks::cron::clock::testing::FakeClock;
        // 2025-06-15T12:00:00Z
        let clock = FakeClock::new(1_750_003_200_000);
        let job = make_job("Test");
        let result = render_template("Time: {{now}}", &job, None, 0, None, &clock);
        assert!(result.contains("2025-06-15"));
    }

    #[test]
    fn resolve_uses_static_prompt_without_template() {
        let clock = SystemClock;
        let job = make_job("Test");
        assert_eq!(resolve_job_prompt(&job, &clock), "static prompt");
    }

    #[test]
    fn resolve_renders_prompt_template_when_set() {
        let clock = SystemClock;
        let mut job = make_job("Reporter");
        job.prompt_template = Some("Run #{{run_count}} of {{job_name}}".to_string());
        job.state.run_count = 7;
        assert_eq!(resolve_job_prompt(&job, &clock), "Run #7 of Reporter");
    }
}
