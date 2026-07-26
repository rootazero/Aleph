//! `artifact_publish` — hand the user the finished work product.
//!
//! # Why this is a tool and not a heuristic
//!
//! Aleph could have tried to *detect* the final result: take the last assistant
//! message of a run, or the longest one, or the one after the last tool call,
//! and render that. Every version of that is the harness deciding what the work
//! was — a judgement the model is far better placed to make and the only entity
//! that actually knows (R7). So the model declares it, with a tool (R8), and
//! the code does nothing but render and store what it was handed.
//!
//! # Why the output is a document and not a chat message
//!
//! A long report inside a chat bubble is unreadable and unkeepable: it scrolls
//! away, it cannot be printed, and it dies with the conversation. Published
//! here it becomes an artifact — durable, capability-addressed, listed in the
//! pane, opened in a real browser tab, still readable offline a year later with
//! no server running.
//!
//! # What it is not
//!
//! It is not `session.export_html`. That renders the *conversation*; this
//! renders the *result*. Confusing the two is what this tool exists to end.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::artifacts::{ArtifactOrigin, ArtifactRecord, ArtifactStore};
use crate::error::{AlephError, Result};
use crate::export::{collect_artifacts, render_document_html, ExportDocument};
use crate::gateway::event_emitter::artifact_ping::publish_artifact_ping;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;

/// Longest slug taken from the title, in characters. Leaves room under the
/// store's own filename ceiling for the extension.
const MAX_SLUG_CHARS: usize = 60;

/// Filename stem used when a title slugifies to nothing (punctuation only).
const FALLBACK_STEM: &str = "deliverable";

/// Most figures one document may name. A deliverable that wants a hundred
/// images is not a deliverable; the byte budget in [`crate::export`] would
/// refuse them anyway, this just refuses them earlier and says so.
const MAX_FIGURES: usize = 12;

/// Arguments for `artifact_publish`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ArtifactPublishArgs {
    /// Title of the document, as the user should see it.
    pub title: String,

    /// The deliverable itself, in Markdown. Headings, lists, tables, code and
    /// links all render; raw HTML is escaped, not executed.
    pub content: String,

    /// Artifact ids from this session to embed as figures, in the order they
    /// should appear. Get them from a previous tool result or `artifacts.list`.
    #[serde(default)]
    pub figures: Vec<String>,
}

/// Result of a successful publish.
///
/// Deliberately no `_display` field. `media_send` has one and the `_` prefix
/// suggests the infrastructure treats it specially — it does not: nothing in
/// the repository reads `_display`, only `_media`. Copying the convention would
/// propagate that fiction.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactPublishOutput {
    /// What just happened, in the model's own working language.
    ///
    /// Carries one fact the model cannot infer from anywhere else: the document
    /// is already in front of the user. Without it the very next move is to
    /// paste the whole report into the reply as well, which is precisely the
    /// chat-bubble burial this tool exists to avoid.
    pub published: String,
    /// Store id of the published document.
    pub artifact_id: String,
    /// Filename it was stored under.
    pub filename: String,
    /// Size of the rendered document in bytes.
    pub size: u64,
    /// Figure ids that could not be embedded, with why. Reported rather than
    /// swallowed so the model can correct an id instead of believing a figure
    /// made it in.
    pub skipped_figures: Vec<String>,
}

/// The `artifact_publish` tool.
#[derive(Clone, Default)]
pub struct ArtifactPublishTool;

impl ArtifactPublishTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AlephTool for ArtifactPublishTool {
    const NAME: &'static str = "artifact_publish";
    const DESCRIPTION: &'static str = "Publish the finished work product as a standalone document \
         the user can read, keep and share. Opens in their browser and appears in the artifacts \
         pane. \
         Use this when the task produced something worth having on its own — a report, an \
         analysis, a plan, a summary, a spec, a comparison — instead of burying it in a chat \
         message that scrolls away. Do NOT use it for a short conversational reply, an \
         acknowledgement, or a progress update; just answer normally for those. \
         `content` is Markdown: headings, lists, tables, code blocks and links all render. Raw \
         HTML is escaped, and the document loads no remote resources, so images must come from \
         `figures` (artifact ids already stored in this session, e.g. a chart a previous tool \
         produced) rather than from Markdown image links. Publishing again creates a new \
         document; it never overwrites an earlier one.";
    type Args = ArtifactPublishArgs;
    type Output = ArtifactPublishOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let title = args.title.trim();
        if title.is_empty() {
            return Err(AlephError::tool("artifact_publish requires a title"));
        }
        if args.content.trim().is_empty() {
            return Err(AlephError::tool(
                "artifact_publish requires content: the document body cannot be empty",
            ));
        }

        // Without a turn there is no session to attribute the document to, and
        // a deliverable nobody can find is not a deliverable. Unlike the media
        // harvest — which is a side effect of a call that already succeeded and
        // so degrades silently — publishing IS the call, so this fails loudly.
        let turn = current_turn_context().ok_or_else(|| {
            AlephError::tool(
                "artifact_publish has no session context; it can only run inside an agent turn",
            )
        })?;
        let session_key = turn.session_key.to_key_string();
        let run_id = (!turn.run_id.is_empty()).then_some(turn.run_id.as_str());

        let store = ArtifactStore::shared().ok_or_else(|| {
            AlephError::tool("artifact_publish is unavailable: no artifact directory")
        })?;

        let (figures, skipped_figures) = resolve_figures(store, &session_key, &args.figures).await;

        let doc = ExportDocument {
            title: title.to_string(),
            provenance: provenance_line(),
            markdown: args.content,
        };
        let html = render_document_html(&doc, &figures);

        let record = store
            .put(
                &session_key,
                run_id,
                ArtifactOrigin::Deliverable,
                &slug_filename(title),
                "text/html",
                html.as_bytes(),
            )
            .await
            .map_err(|e| AlephError::tool(format!("failed to publish the deliverable: {e}")))?;

        // Content-free invalidation ping: the pane re-reads the authoritative
        // list. See `gateway::event_emitter::artifact_ping` for why the frame
        // deliberately carries nothing else.
        publish_artifact_ping(&session_key);

        Ok(ArtifactPublishOutput {
            published: format!(
                "Published \"{title}\" ({}). It is now open in the user's browser and listed in \
                 their artifacts pane — refer to it in your reply, do not repeat its contents.",
                human_size(record.size)
            ),
            artifact_id: record.id,
            filename: record.filename,
            size: record.size,
            skipped_figures,
        })
    }
}

/// Read the named figures, reporting the ones that could not be used.
///
/// A bad id is not fatal: the report is still worth publishing without one of
/// its charts, and the model is told exactly which id failed so it can fix the
/// reference rather than assume the figure is there.
async fn resolve_figures(
    store: &ArtifactStore,
    session_key: &str,
    ids: &[String],
) -> (Vec<crate::export::ExportArtifact>, Vec<String>) {
    let mut records: Vec<ArtifactRecord> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for id in ids.iter().take(MAX_FIGURES) {
        match store.read(session_key, id).await {
            Ok((record, _bytes)) => records.push(record),
            Err(e) => {
                warn!(artifact_id = %id, error = %e, "figure skipped while publishing a deliverable");
                skipped.push(format!("{id}: {e}"));
            }
        }
    }
    for id in ids.iter().skip(MAX_FIGURES) {
        skipped.push(format!("{id}: over the {MAX_FIGURES}-figure limit"));
    }

    // Second pass through the shared budget-aware reader rather than keeping
    // the bytes from the probe above: that reader is the one place that decides
    // what a document may inline, and duplicating its rule here is how the two
    // would drift.
    let figures = collect_artifacts(store, session_key, &records).await;
    (figures, skipped)
}

/// One line of provenance under the title.
fn provenance_line() -> String {
    format!("Aleph · {}", chrono::Local::now().format("%Y-%m-%d %H:%M"))
}

/// Derive a filename from the title.
///
/// Alphanumerics survive — including CJK, which `is_alphanumeric` covers and an
/// ASCII-only filter would erase entirely, leaving every Chinese-titled report
/// named `deliverable.html`. Everything else collapses to a single `-`.
fn slug_filename(title: &str) -> String {
    let mut slug = String::with_capacity(title.len().min(MAX_SLUG_CHARS));
    let mut pending_sep = false;

    for c in title.chars() {
        if c.is_alphanumeric() {
            if pending_sep && !slug.is_empty() {
                slug.push('-');
            }
            pending_sep = false;
            slug.extend(c.to_lowercase());
            if slug.chars().count() >= MAX_SLUG_CHARS {
                break;
            }
        } else {
            pending_sep = true;
        }
    }

    if slug.is_empty() {
        slug.push_str(FALLBACK_STEM);
    }
    format!("{slug}.html")
}

/// Byte count for the model-facing line. Kept local (rather than reaching into
/// `export`'s private helper) because this one string is all it is for.
fn human_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_title_becomes_a_readable_filename() {
        assert_eq!(slug_filename("Q3 Review"), "q3-review.html");
        assert_eq!(
            slug_filename("Cost/benefit: 2026 plan!"),
            "cost-benefit-2026-plan.html"
        );
    }

    #[test]
    fn a_cjk_title_keeps_its_characters() {
        // An ASCII-only filter would leave every Chinese report as
        // `deliverable.html`, which is indistinguishable from every other one.
        assert_eq!(slug_filename("季度复盘 报告"), "季度复盘-报告.html");
    }

    #[test]
    fn a_title_of_pure_punctuation_falls_back() {
        assert_eq!(slug_filename("***"), "deliverable.html");
        assert_eq!(slug_filename("   "), "deliverable.html");
    }

    #[test]
    fn a_long_title_is_truncated_without_splitting_a_char() {
        let slug = slug_filename(&"é".repeat(MAX_SLUG_CHARS * 2));
        let stem = slug.trim_end_matches(".html");
        assert_eq!(stem.chars().count(), MAX_SLUG_CHARS);
    }

    #[test]
    fn the_slug_never_carries_a_path_separator() {
        for title in ["a/b", "a\\b", "../etc/passwd", "C:\\x"] {
            let slug = slug_filename(title);
            assert!(!slug.contains('/'), "{slug}");
            assert!(!slug.contains('\\'), "{slug}");
            assert!(!slug.contains(".."), "{slug}");
        }
    }

    #[tokio::test]
    async fn a_missing_figure_is_reported_not_silently_dropped() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());
        let session = "agent:main:main";

        let real = store
            .put(
                session,
                None,
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"png",
            )
            .await
            .expect("put");
        let ghost = uuid::Uuid::new_v4().to_string();

        let (figures, skipped) =
            resolve_figures(&store, session, &[real.id.clone(), ghost.clone()]).await;

        assert_eq!(figures.len(), 1, "the good figure still made it in");
        assert_eq!(figures[0].filename, "chart.png");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].starts_with(&ghost), "{:?}", skipped);
    }

    #[tokio::test]
    async fn figures_past_the_limit_are_reported_rather_than_ignored() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());
        let session = "agent:main:main";

        let mut ids = Vec::new();
        for i in 0..=MAX_FIGURES {
            let r = store
                .put(
                    session,
                    None,
                    ArtifactOrigin::Outbound,
                    &format!("f{i}.png"),
                    "image/png",
                    b"x",
                )
                .await
                .expect("put");
            ids.push(r.id);
        }

        let (figures, skipped) = resolve_figures(&store, session, &ids).await;
        assert_eq!(figures.len(), MAX_FIGURES);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("limit"), "{:?}", skipped);
    }

    #[tokio::test]
    async fn publishing_without_a_turn_context_fails_loudly() {
        // A deliverable nobody can find is not a deliverable — unlike the media
        // harvest, this refuses rather than degrading.
        let err = ArtifactPublishTool::new()
            .call(ArtifactPublishArgs {
                title: "t".to_string(),
                content: "body".to_string(),
                figures: Vec::new(),
            })
            .await
            .expect_err("expected a failure without a turn");
        assert!(err.to_string().contains("session context"), "{err}");
    }

    #[tokio::test]
    async fn an_empty_body_is_refused_before_anything_is_stored() {
        let err = ArtifactPublishTool::new()
            .call(ArtifactPublishArgs {
                title: "t".to_string(),
                content: "   \n".to_string(),
                figures: Vec::new(),
            })
            .await
            .expect_err("expected a failure on an empty body");
        assert!(err.to_string().contains("content"), "{err}");
    }
}
