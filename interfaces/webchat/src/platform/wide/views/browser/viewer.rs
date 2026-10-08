//! Accessible status shell for the browser viewer.
//!
//! Renders whatever [`BrowserViewerModel`] already holds. It owns no target
//! and performs no I/O: no socket, no ticket exchange, no `location.hash`.
//! The only local state is which companion candidate is highlighted, and that
//! is restricted to ids the bound resource itself published.

use super::model::{
    BoundBrowser, BrowserViewerModel, BrowserWorkspace, UnboundReason, ViewerPhase,
};
use crate::i18n::{t_string, use_i18n, I18nCtx};
use aleph_protocol::browser_live::{
    ClosedReason, CompanionArrangement, CompanionCandidate, Controller, RedactionStatus,
};
use leptos::prelude::*;

fn closed_reason_label(i18n: I18nCtx, reason: ClosedReason) -> String {
    match reason {
        ClosedReason::TargetClosed => t_string!(i18n, browser_viewer.closed_target_closed),
        ClosedReason::EngineExited => t_string!(i18n, browser_viewer.closed_engine_exited),
        ClosedReason::ServiceExited => t_string!(i18n, browser_viewer.closed_service_exited),
        ClosedReason::RunEnded => t_string!(i18n, browser_viewer.closed_run_ended),
        ClosedReason::Revoked => t_string!(i18n, browser_viewer.closed_revoked),
        ClosedReason::Expired => t_string!(i18n, browser_viewer.closed_expired),
    }
    .to_string()
}

fn arrangement_label(i18n: I18nCtx, arrangement: &CompanionArrangement) -> String {
    match arrangement {
        CompanionArrangement::Single => t_string!(i18n, browser_viewer.arrangement_single),
        CompanionArrangement::SideBySide => {
            t_string!(i18n, browser_viewer.arrangement_side_by_side)
        }
        CompanionArrangement::HorizontalScreens => {
            t_string!(i18n, browser_viewer.arrangement_horizontal)
        }
    }
    .to_string()
}

fn closed_text(i18n: I18nCtx, reason: ClosedReason) -> String {
    t_string!(
        i18n,
        browser_viewer.status_closed,
        reason = closed_reason_label(i18n, reason)
    )
    .to_string()
}

fn unbound_text(i18n: I18nCtx, reason: &UnboundReason) -> String {
    match reason {
        UnboundReason::NoResource => t_string!(i18n, browser_viewer.status_none).to_string(),
        UnboundReason::Refused(_) => t_string!(i18n, browser_viewer.status_refused).to_string(),
        UnboundReason::Closed(tombstone) => closed_text(i18n, tombstone.reason),
    }
}

/// Why a bound resource must not be described yet. While redaction is
/// pending (or, defensively, refused) the view shows one localized status
/// line and nothing the resource carries: no ids, labels or animations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Withheld {
    Pending,
    Refused,
}

fn withheld(bound: &BoundBrowser) -> Option<Withheld> {
    match bound.resource().redaction {
        RedactionStatus::Safe | RedactionStatus::Redacted => None,
        RedactionStatus::Pending => Some(Withheld::Pending),
        RedactionStatus::Refused => Some(Withheld::Refused),
    }
}

/// Layout / animation metadata for one candidate, as plain text lines.
fn candidate_lines(i18n: I18nCtx, candidate: &CompanionCandidate) -> Vec<String> {
    let mut lines = vec![
        arrangement_label(i18n, &candidate.layout.arrangement),
        t_string!(
            i18n,
            browser_viewer.screens,
            n = candidate.layout.screens.len() as u64
        )
        .to_string(),
    ];
    if let Some(animation) = &candidate.state.animation {
        let mut line = t_string!(
            i18n,
            browser_viewer.animation,
            name = animation.name.clone(),
            ms = animation.duration_ms
        )
        .to_string();
        if animation.looped {
            line.push_str(" · ");
            line.push_str(t_string!(i18n, browser_viewer.animation_looped));
        }
        lines.push(line);
    }
    if candidate.state.prefers_reduced_motion {
        lines.push(t_string!(i18n, browser_viewer.reduced_motion).to_string());
    }
    lines
}

/// Plain-text summary lines for a bound viewer. Pure, so the view below is a
/// straight render of strings that were derived in one place.
struct BoundText {
    phase: String,
    hint: Option<String>,
    resource: String,
    binding: Vec<String>,
}

fn bound_text(i18n: I18nCtx, bound: &BoundBrowser) -> BoundText {
    let resource = bound.resource();
    let viewer = bound.viewer();
    let phase = match bound.phase() {
        ViewerPhase::AwaitingAuthorization => {
            t_string!(i18n, browser_viewer.phase_awaiting).to_string()
        }
        ViewerPhase::Live => t_string!(i18n, browser_viewer.phase_live).to_string(),
        ViewerPhase::Closed(reason) => closed_text(i18n, reason),
    };
    let hint = match bound.phase() {
        ViewerPhase::AwaitingAuthorization => {
            Some(t_string!(i18n, browser_viewer.awaiting_hint).to_string())
        }
        ViewerPhase::Live => Some(t_string!(i18n, browser_viewer.live_hint).to_string()),
        ViewerPhase::Closed(_) => None,
    };
    let mut binding = vec![
        t_string!(
            i18n,
            browser_viewer.binding_run,
            v = viewer.task.run_id.clone()
        )
        .to_string(),
        t_string!(
            i18n,
            browser_viewer.binding_profile,
            v = viewer.target.profile.clone()
        )
        .to_string(),
        t_string!(
            i18n,
            browser_viewer.binding_session,
            v = viewer.target.browser_session_id.clone()
        )
        .to_string(),
        t_string!(
            i18n,
            browser_viewer.binding_target,
            v = viewer.target.target_id.clone()
        )
        .to_string(),
    ];
    if let Some(live) = bound.live() {
        binding.push(
            t_string!(
                i18n,
                browser_viewer.viewport,
                w = u64::from(live.viewport_css_px.width),
                h = u64::from(live.viewport_css_px.height)
            )
            .to_string(),
        );
        binding.push(
            match live.controller {
                Controller::Agent => t_string!(i18n, browser_viewer.controller_agent),
                Controller::Human { .. } => t_string!(i18n, browser_viewer.controller_human),
            }
            .to_string(),
        );
        binding.push(
            t_string!(
                i18n,
                browser_viewer.epochs,
                c = live.connection_epoch,
                s = live.stream_epoch,
                k = live.control_epoch
            )
            .to_string(),
        );
    }
    BoundText {
        phase,
        hint,
        resource: t_string!(
            i18n,
            browser_viewer.resource_line,
            id = resource.resource_id.clone(),
            n = resource.revision
        )
        .to_string(),
        binding,
    }
}

#[component]
fn BoundSummary(bound: BoundBrowser) -> impl IntoView {
    let i18n = use_i18n();
    let text = bound_text(i18n, &bound);

    // Highlight only ids the resource itself published; never a free-form id.
    // Selection lives in the workspace model when this very resource is the
    // one it holds, so it survives re-renders and newer revisions. A viewer
    // with no matching context keeps a local highlight instead.
    let candidates: Vec<CompanionCandidate> = bound.candidates().to_vec();
    let resource_id = bound.resource().resource_id.clone();
    let workspace = use_context::<BrowserWorkspace>().filter(|workspace| {
        workspace.model.with_untracked(|model| {
            model
                .bound()
                .is_some_and(|held| held.resource().resource_id == resource_id)
        })
    });
    let local = RwSignal::new(bound.selected_candidate().map(|c| c.id.clone()));
    let selected = move || match workspace {
        Some(workspace) => workspace.model.with(|model| {
            model
                .bound()
                .and_then(|held| held.selected_candidate().map(|c| c.id.clone()))
        }),
        None => local.get(),
    };

    let candidate_list = if candidates.is_empty() {
        view! {
            <p class="text-xs text-text-secondary">
                {t_string!(i18n, browser_viewer.candidates_empty)}
            </p>
        }
        .into_any()
    } else {
        view! {
            <ul class="flex flex-col gap-1">
                {candidates
                    .into_iter()
                    .map(|candidate| {
                        let id = candidate.id.clone();
                        let pressed_id = id.clone();
                        let lines = candidate_lines(i18n, &candidate);
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="w-full text-left px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover"
                                    aria-pressed=move || {
                                        (selected().as_deref() == Some(pressed_id.as_str()))
                                            .to_string()
                                    }
                                    on:click=move |_| match workspace {
                                        Some(workspace) => workspace.model.update(|model| {
                                            // Unknown ids are refused by the model.
                                            let _ = model.select_candidate(&id);
                                        }),
                                        None => local.set(Some(id.clone())),
                                    }
                                >
                                    <span class="font-medium text-text-primary">
                                        {candidate.label.clone()}
                                    </span>
                                    {lines
                                        .into_iter()
                                        .map(|line| view! {
                                            <span class="block text-text-secondary">{line}</span>
                                        })
                                        .collect_view()}
                                </button>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    };

    view! {
        <div class="flex flex-col gap-3">
            <p class="text-sm font-medium text-text-primary" role="status">{text.phase}</p>
            {text.hint.map(|hint| view! { <p class="text-xs text-text-secondary">{hint}</p> })}

            <section class="flex flex-col gap-1" aria-labelledby="browser-viewer-resource">
                <h3 id="browser-viewer-resource" class="text-xs font-semibold text-text-secondary">
                    {t_string!(i18n, browser_viewer.resource_title)}
                </h3>
                <p class="text-xs text-text-primary break-all">{text.resource}</p>
            </section>

            <section class="flex flex-col gap-1" aria-labelledby="browser-viewer-binding">
                <h3 id="browser-viewer-binding" class="text-xs font-semibold text-text-secondary">
                    {t_string!(i18n, browser_viewer.binding_title)}
                </h3>
                <ul class="text-xs text-text-primary break-all">
                    {text
                        .binding
                        .into_iter()
                        .map(|line| view! { <li>{line}</li> })
                        .collect_view()}
                </ul>
            </section>

            <section class="flex flex-col gap-1" aria-labelledby="browser-viewer-candidates">
                <h3 id="browser-viewer-candidates" class="text-xs font-semibold text-text-secondary">
                    {t_string!(i18n, browser_viewer.candidates_title)}
                </h3>
                {candidate_list}
            </section>
        </div>
    }
}

/// Shared browser viewer body.
///
/// `standalone` marks the dedicated-page presentation (own heading, full
/// height). The in-workspace tab omits the heading because the tab label
/// already carries it.
#[component]
pub fn BrowserViewer(model: BrowserViewerModel, standalone: bool) -> impl IntoView {
    let i18n = use_i18n();
    let container = if standalone {
        "flex flex-col gap-3 p-4 min-h-screen"
    } else {
        "flex flex-col gap-3 p-3"
    };
    let body = match model {
        BrowserViewerModel::Unbound(reason) => view! {
            <p class="text-sm text-text-secondary" role="status">{unbound_text(i18n, &reason)}</p>
        }
        .into_any(),
        BrowserViewerModel::Bound(bound) => match withheld(&bound) {
            Some(why) => view! {
                <p class="text-sm text-text-secondary" role="status">
                    {match why {
                        Withheld::Pending => t_string!(i18n, browser_viewer.status_pending),
                        Withheld::Refused => t_string!(i18n, browser_viewer.status_refused),
                    }}
                </p>
            }
            .into_any(),
            None => view! { <BoundSummary bound=*bound /> }.into_any(),
        },
    };
    view! {
        <section
            class=container
            aria-label=t_string!(i18n, browser_viewer.region_label).to_string()
        >
            {standalone.then(|| view! {
                <h2 class="text-base font-semibold text-text-primary">
                    {t_string!(i18n, browser_viewer.title)}
                </h2>
            })}
            {body}
        </section>
    }
}

/// Dedicated-page entry point.
///
/// It displays whatever the surrounding app already put in a
/// [`BrowserWorkspace`] context and otherwise the unbound state. It never
/// derives a target from the URL, the hash, or any tab, so opening this page
/// by itself grants and shows nothing.
#[component]
pub fn BrowserViewerPage() -> impl IntoView {
    let workspace = use_context::<BrowserWorkspace>();
    move || {
        let model = workspace
            .map(|workspace| workspace.model.get())
            .unwrap_or_default();
        view! { <BrowserViewer model=model standalone=true /> }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        BrowserTargetBinding, ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind,
        PreviewProvenance, PreviewResource, PreviewSource, ResourceBinding, ViewerBinding,
    };

    fn resource(redaction: RedactionStatus) -> PreviewResource {
        PreviewResource {
            resource_id: "resource-5".to_string(),
            revision: 1,
            binding: ResourceBinding::Browser {
                viewer: ViewerBinding {
                    principal: IssuerIdentity::User {
                        user_id: "user-42".to_string(),
                    },
                    task: ConversationBinding {
                        session_key: "agent:operator:browser".to_string(),
                        run_id: "run-7".to_string(),
                    },
                    target: BrowserTargetBinding {
                        profile: "default".to_string(),
                        browser_session_id: "browser-session-3".to_string(),
                        target_id: "target-9".to_string(),
                    },
                },
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms: 2_000,
            redaction,
            companion_candidates: Vec::new(),
        }
    }

    fn withheld_for(redaction: RedactionStatus) -> Option<Withheld> {
        let model = BrowserViewerModel::from_resource(resource(redaction)).unwrap();
        model.bound().and_then(withheld)
    }

    #[test]
    fn only_cleared_redaction_states_are_described() {
        assert_eq!(withheld_for(RedactionStatus::Safe), None);
        assert_eq!(withheld_for(RedactionStatus::Redacted), None);
        assert_eq!(
            withheld_for(RedactionStatus::Pending),
            Some(Withheld::Pending)
        );
        // Refused never binds at all, so there is no bound model to describe.
        assert!(
            BrowserViewerModel::from_resource(resource(RedactionStatus::Refused))
                .unwrap()
                .bound()
                .is_none()
        );
    }
}
