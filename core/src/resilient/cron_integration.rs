//! Integration with the Cron scheduling system.
//!
//! Provides resilient execution for scheduled jobs.

use tracing::info;

use crate::error::Result;

use super::executor::ResilientExecutor;
use super::task::ResilientTask;
use super::types::{DegradationStrategy, ResilienceConfig, TaskContext, TaskOutcome};

/// A cron job wrapped with resilience
pub struct ResilientCronJob<T: ResilientTask> {
    /// The underlying task
    pub task: T,
    /// Job schedule (cron expression)
    pub schedule: String,
    /// Job name
    pub name: String,
    /// Whether job is enabled
    pub enabled: bool,
}

impl<T: ResilientTask> ResilientCronJob<T> {
    /// Create a new resilient cron job
    pub fn new(task: T, schedule: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            task,
            schedule: schedule.into(),
            name: name.into(),
            enabled: true,
        }
    }

    /// Enable or disable the job
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Execute the job with resilience
    pub async fn run(&self, executor: &ResilientExecutor) -> TaskOutcome<T::Output> {
        if !self.enabled {
            return TaskOutcome::Failed {
                error: "Job is disabled".to_string(),
                attempts: 0,
                last_attempt_duration: std::time::Duration::ZERO,
            };
        }

        info!(job_name = %self.name, schedule = %self.schedule, "Running resilient cron job");
        executor.execute(&self.task).await
    }
}

/// Example: Podcast generation task with TTS fallback to markdown
pub struct PodcastTask {
    /// Podcast title
    pub title: String,
    /// Content to convert
    pub content: String,
    /// Resilience configuration
    config: ResilienceConfig,
}

impl PodcastTask {
    /// Create a new podcast task
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            config: ResilienceConfig {
                max_attempts: 3,
                timeout_ms: 120000, // 2 minutes for TTS
                degradation_strategy: DegradationStrategy::Fallback {
                    fallback_id: "markdown-summary".to_string(),
                },
                ..Default::default()
            },
        }
    }

    /// Create with custom config
    pub fn with_config(mut self, config: ResilienceConfig) -> Self {
        self.config = config;
        self
    }
}

impl ResilientTask for PodcastTask {
    type Output = PodcastResult;

    #[allow(clippy::needless_return)]
    fn execute<'a>(
        &'a self,
        _ctx: &'a TaskContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Output>> + Send + 'a>>
    {
        let title = self.title.clone();
        let _content = self.content.clone();

        Box::pin(async move {
            // Simulate TTS generation (in real impl, call TTS API)
            info!(title = %title, "Generating podcast audio via TTS");

            // For demonstration, simulate occasional failure
            #[cfg(test)]
            {
                return Err(crate::error::AlephError::NetworkError {
                    message: "TTS service unavailable".to_string(),
                    suggestion: None,
                });
            }

            #[cfg(not(test))]
            {
                // Real TTS implementation would go here
                Ok(PodcastResult::Audio {
                    title,
                    audio_url: "https://example.com/podcast.mp3".to_string(),
                    duration_secs: 300,
                })
            }
        })
    }

    fn fallback<'a>(
        &'a self,
        _ctx: &'a TaskContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Output>> + Send + 'a>>
    {
        let title = self.title.clone();
        let content = self.content.clone();

        Box::pin(async move {
            // Generate markdown summary instead
            info!(title = %title, "Falling back to markdown summary");

            let summary = generate_markdown_summary(&content);
            Ok(PodcastResult::Markdown {
                title,
                content: summary,
            })
        })
    }

    fn has_fallback(&self) -> bool {
        true
    }

    fn task_id(&self) -> &str {
        &self.title
    }

    fn config(&self) -> ResilienceConfig {
        self.config.clone()
    }
}

/// Result of podcast generation
#[derive(Debug, Clone)]
pub enum PodcastResult {
    /// Full audio podcast
    Audio {
        title: String,
        audio_url: String,
        duration_secs: u64,
    },
    /// Markdown summary fallback
    Markdown { title: String, content: String },
}

impl PodcastResult {
    /// Check if this is the primary (audio) result
    pub fn is_audio(&self) -> bool {
        matches!(self, PodcastResult::Audio { .. })
    }

    /// Check if this is a fallback (markdown) result
    pub fn is_markdown(&self) -> bool {
        matches!(self, PodcastResult::Markdown { .. })
    }

    /// Get the title
    pub fn title(&self) -> &str {
        match self {
            PodcastResult::Audio { title, .. } => title,
            PodcastResult::Markdown { title, .. } => title,
        }
    }
}

/// Generate a markdown summary from content
fn generate_markdown_summary(content: &str) -> String {
    // Simple summarization - in real impl, use LLM
    let sentences: Vec<&str> = content.split('.').take(5).collect();
    format!(
        "## Summary\n\n{}\n\n*This is a text summary because audio generation was unavailable.*",
        sentences.join(". ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_podcast_task_fallback() {
        let task = PodcastTask::new("Test Podcast", "This is test content for the podcast.");

        let executor = ResilientExecutor::new();
        let outcome = executor.execute(&task).await;

        // Should degrade to markdown
        match outcome {
            TaskOutcome::Degraded { result, .. } => {
                assert!(result.is_markdown());
            }
            _ => panic!("Expected Degraded outcome with markdown fallback"),
        }
    }

    #[tokio::test]
    async fn test_resilient_cron_job() {
        let task = PodcastTask::new("Daily News", "Today's news content...");
        let job = ResilientCronJob::new(task, "0 8 * * *", "daily-news-podcast");

        let executor = ResilientExecutor::new();
        let outcome = job.run(&executor).await;

        // Should produce a result (either audio or markdown)
        assert!(outcome.is_ok());
    }

    #[tokio::test]
    async fn test_disabled_job() {
        let task = PodcastTask::new("Test", "Content");
        let job = ResilientCronJob::new(task, "* * * * *", "test").with_enabled(false);

        let executor = ResilientExecutor::new();
        let outcome = job.run(&executor).await;

        assert!(outcome.is_failed());
    }

    #[tokio::test]
    async fn test_podcast_result_accessors() {
        let audio = PodcastResult::Audio {
            title: "Test".to_string(),
            audio_url: "https://example.com/audio.mp3".to_string(),
            duration_secs: 120,
        };
        assert!(audio.is_audio());
        assert!(!audio.is_markdown());
        assert_eq!(audio.title(), "Test");

        let markdown = PodcastResult::Markdown {
            title: "Summary".to_string(),
            content: "Content".to_string(),
        };
        assert!(!markdown.is_audio());
        assert!(markdown.is_markdown());
        assert_eq!(markdown.title(), "Summary");
    }
}
