//! Accessible status shell for the browser viewer.
//!
//! Renders whatever [`BrowserViewerModel`] already holds. It owns no target
//! and performs no I/O: no socket, no ticket exchange, no `location.hash`.
//! The only local state is which companion candidate is highlighted (restricted
//! to ids the bound resource itself published) and the in-flight flag of the
//! Safe / Refused decision on a `Pending` revision. That decision is the one
//! outbound call, and it is addressed by the exact `resource_id` + `revision`
//! the shown resource carried, never by URL, profile or current tab.

use super::launch::{
    begin_issue, finish_issue, model_launch_target, open_ready, take_unopened, LaunchPhase,
    LaunchTarget,
};
use super::model::{
    BoundBrowser, BrowserViewerModel, BrowserWorkspace, UnboundReason, ViewerPhase,
};
use crate::api::{BrowserViewerApi, ViewerCaller};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n, I18nCtx};
use aleph_protocol::browser_live::{
    ClosedReason, CompanionArrangement, CompanionCandidate, Controller, RedactionStatus,
    ResourceDecision,
};

#[cfg(target_arch = "wasm32")]
fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(not(target_arch = "wasm32"))]
const fn now_ms() -> i64 {
    0
}

#[cfg(target_arch = "wasm32")]
fn open_in_new_tab(url: &str) -> bool {
    web_sys::window()
        .is_some_and(|window| matches!(window.open_with_url_and_target(url, "_blank"), Ok(Some(_))))
}

#[cfg(not(target_arch = "wasm32"))]
const fn open_in_new_tab(_url: &str) -> bool {
    false
}

use leptos::prelude::*;
use leptos::task::spawn_local;

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

/// Current paired caller as a tracked read (`None` = not paired / no context).
fn caller_now(dashboard: Option<DashboardState>) -> Option<ViewerCaller> {
    dashboard.and_then(|dashboard| ViewerCaller::tracked(&dashboard))
}

/// A held or in-flight grant stays usable only while the resource target is
/// unchanged and the exact caller that requested it is still the paired one.
/// One derivation shared by the Effect, `finish_issue` and the open click.
fn launch_authorized(
    target_matches: bool,
    held: Option<&ViewerCaller>,
    now: Option<&ViewerCaller>,
) -> bool {
    target_matches && ViewerCaller::still_valid(held, now)
}

/// Drop an unopened link when it is no longer authorized; returns the grant id
/// to revoke. `Opened`, `Issuing`, `Idle` and `Failed` are left alone.
#[must_use]
fn invalidate_unopened(phase: &mut LaunchPhase, authorized: bool) -> Option<String> {
    if authorized {
        None
    } else {
        take_unopened(phase)
    }
}

/// Single decision state: one value, so "busy" and "failed" can never both
/// hold and one check guards re-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionUi {
    Idle,
    /// Sent; waiting for the gateway's resource / closed event. Never cleared
    /// locally on success: the view is replaced by that event, not by us.
    Busy,
    /// The call failed; the buttons are live again.
    Failed,
}

/// Enter `Busy` unless a decision is already in flight.
fn begin_decision(state: &mut DecisionUi) -> bool {
    if *state == DecisionUi::Busy {
        return false;
    }
    *state = DecisionUi::Busy;
    true
}

/// Safe / Refused controls for one exact `Pending` revision. The target is
/// the snapshot taken when this component was created; a newer revision
/// re-renders the viewer and builds a fresh component for it.
#[component]
fn PendingDecision(resource_id: String, revision: u64) -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = use_context::<DashboardState>();
    let state = RwSignal::new(DecisionUi::Idle);
    let target = StoredValue::new((resource_id, revision));

    let decide = move |decision: ResourceDecision| {
        let Some(dashboard) = dashboard else {
            state.set(DecisionUi::Failed);
            return;
        };
        let Some(caller) = ViewerCaller::current(&dashboard) else {
            return;
        };
        let mut next = state.get_untracked();
        if !begin_decision(&mut next) {
            return;
        }
        state.set(next);
        let (resource_id, revision) = target.get_value();
        spawn_local(async move {
            // Success changes nothing here: the new state arrives as the
            // Gateway's resource / closed event through the root bridge.
            if BrowserViewerApi::decide(&dashboard, &caller, &resource_id, revision, decision)
                .await
                .is_err()
            {
                let _ = state.try_set(DecisionUi::Failed);
            }
        });
    };
    let paired = move || caller_now(dashboard).is_some();
    let busy = move || state.get() == DecisionUi::Busy || !paired();

    view! {
        <section
            data-testid="browser-live-decision"
            class="flex flex-col gap-2" aria-labelledby="browser-viewer-decision">
            <h3 id="browser-viewer-decision" class="text-xs font-semibold text-text-secondary">
                {t_string!(i18n, browser_viewer.decision_title)}
            </h3>
            <p class="text-xs text-text-secondary">
                {t_string!(i18n, browser_viewer.decision_hint)}
            </p>
            {move || (!paired()).then(|| view! { <PairingNotice dashboard=dashboard /> })}
            <div class="flex gap-2">
                <button
                    data-testid="browser-live-decision-safe"
                    type="button"
                    class="px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover disabled:opacity-50"
                    prop:disabled=busy
                    on:click=move |_| decide(ResourceDecision::Safe)
                >
                    {t_string!(i18n, browser_viewer.decision_safe)}
                </button>
                <button
                    data-testid="browser-live-decision-refuse"
                    type="button"
                    class="px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover disabled:opacity-50"
                    prop:disabled=busy
                    on:click=move |_| decide(ResourceDecision::Refused)
                >
                    {t_string!(i18n, browser_viewer.decision_refuse)}
                </button>
            </div>
            <p class="text-xs text-text-secondary" role="status">
                {move || match state.get() {
                    DecisionUi::Idle => String::new(),
                    DecisionUi::Busy => t_string!(i18n, browser_viewer.decision_busy).to_string(),
                    DecisionUi::Failed => {
                        t_string!(i18n, browser_viewer.decision_failed).to_string()
                    }
                }}
            </p>
        </section>
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
fn PairingNotice(dashboard: Option<DashboardState>) -> impl IntoView {
    let i18n = use_i18n();
    let native = crate::api::native_credentials::native_expected();
    view! {
        <div data-testid="pairing-notice" class="flex flex-col gap-2" role="status">
            <p class="text-xs text-text-secondary">{t_string!(i18n, browser_viewer.pair_required_hint)}</p>
            <Show when=move || native && dashboard.is_some()>
                <button data-testid="pairing-notice-pair-desktop" type="button" class="px-2 py-1 rounded border border-border text-xs"
                    on:click=move |_| { if let Some(state) = dashboard { state.request_native_pairing(); } }>
                    {t_string!(i18n, browser_viewer.pair_desktop)}
                </button>
            </Show>
        </div>
    }
}

fn unopened_grant_for_cleanup(phase: RwSignal<LaunchPhase>) -> Option<String> {
    let mut next = phase.try_get_untracked()?;
    take_unopened(&mut next)
}

fn commit_launch_phase(
    phase: RwSignal<LaunchPhase>,
    next: LaunchPhase,
    revoke: Option<String>,
    response_grant: Option<String>,
) -> Option<String> {
    // `try_set` returns None on success and the rejected value on disposal.
    if phase.try_set(next).is_none() {
        revoke
    } else {
        revoke.or(response_grant)
    }
}

#[component]
fn LaunchSection(target: LaunchTarget, workspace: Option<BrowserWorkspace>) -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = use_context::<DashboardState>();
    let phase = RwSignal::new(LaunchPhase::default());
    let target = StoredValue::new(target);
    // The paired caller the held / in-flight grant was requested under.
    let held_caller = StoredValue::new(None::<ViewerCaller>);

    // A resource revision, binding or caller change invalidates every unopened
    // link. An in-flight response is handled by `finish_issue`, which sees the
    // authorization lapse and returns the newly issued grant for revocation.
    if let Some(workspace) = workspace {
        Effect::new(move |_| {
            let target_matches = workspace.model.with(|model| {
                model_launch_target(model, now_ms())
                    .ok()
                    .is_some_and(|current| current == target.get_value())
            });
            let now = caller_now(dashboard);
            let authorized = launch_authorized(
                target_matches,
                held_caller.get_value().as_ref(),
                now.as_ref(),
            );
            if authorized {
                return;
            }
            let mut next = phase.get_untracked();
            let Some(grant_id) = invalidate_unopened(&mut next, authorized) else {
                return;
            };
            phase.set(next);
            if let Some(dashboard) = dashboard {
                spawn_local(async move {
                    let _ = BrowserViewerApi::revoke_grant(&dashboard, &grant_id).await;
                });
            }
        });
    }

    on_cleanup(move || {
        let Some(grant_id) = unopened_grant_for_cleanup(phase) else {
            return;
        };
        let Some(dashboard) = dashboard else {
            return;
        };
        spawn_local(async move {
            let _ = BrowserViewerApi::revoke_grant(&dashboard, &grant_id).await;
        });
    });

    let issue = move |_| {
        let Some(dashboard) = dashboard else {
            phase.set(LaunchPhase::Failed);
            return;
        };
        if !begin_issue(&mut phase.get_untracked()) {
            return;
        }
        let target_value = target.get_value();
        let current = workspace.is_some_and(|workspace| {
            workspace.model.with_untracked(|model| {
                model_launch_target(model, now_ms())
                    .ok()
                    .is_some_and(|current| current == target_value)
            })
        });
        // No paired caller (or a stale target): nothing is sent.
        let caller = ViewerCaller::current(&dashboard);
        let Some(caller) = caller.filter(|_| current) else {
            phase.set(LaunchPhase::Failed);
            return;
        };
        held_caller.set_value(Some(caller.clone()));
        phase.set(LaunchPhase::Issuing);
        let phase = phase;
        spawn_local(async move {
            let result = BrowserViewerApi::issue_frames_grant(
                &dashboard,
                &caller,
                &target_value.resource_id,
            )
            .await;
            let target_matches = workspace.is_some_and(|workspace| {
                workspace.model.try_get_untracked().is_some_and(|model| {
                    model_launch_target(&model, now_ms())
                        .ok()
                        .is_some_and(|current| current == target_value)
                })
            });
            let authorized = launch_authorized(
                target_matches,
                Some(&caller),
                ViewerCaller::current(&dashboard).as_ref(),
            );
            let Some(mut next) = phase.try_get_untracked() else {
                if let Ok(response) = &result {
                    let _ = BrowserViewerApi::revoke_grant(&dashboard, &response.grant_id).await;
                }
                return;
            };
            let response_grant_id = match &result {
                Ok(response) => Some(response.grant_id.clone()),
                Err(_) => None,
            };
            let grant_to_revoke = finish_issue(&mut next, authorized, result, now_ms());
            let grant_to_revoke =
                commit_launch_phase(phase, next, grant_to_revoke, response_grant_id);
            if let Some(grant_id) = grant_to_revoke {
                let _ = BrowserViewerApi::revoke_grant(&dashboard, &grant_id).await;
            }
        });
    };

    let open = move |_: web_sys::MouseEvent| {
        let target_matches = workspace.is_some_and(|workspace| {
            workspace.model.with_untracked(|model| {
                model_launch_target(model, now_ms())
                    .ok()
                    .is_some_and(|current| current == target.get_value())
            })
        });
        let now = dashboard.and_then(|dashboard| ViewerCaller::current(&dashboard));
        let authorized = launch_authorized(
            target_matches,
            held_caller.get_value().as_ref(),
            now.as_ref(),
        );
        let mut next = phase.get_untracked();
        if let Some(grant_id) = invalidate_unopened(&mut next, authorized) {
            // A late grant is never handed to a different caller / target.
            phase.set(next);
            if let Some(dashboard) = dashboard {
                spawn_local(async move {
                    let _ = BrowserViewerApi::revoke_grant(&dashboard, &grant_id).await;
                });
            }
            return;
        }
        if !authorized {
            return;
        }
        let _ = open_ready(&mut next, now_ms(), open_in_new_tab);
        phase.set(next);
    };
    let dismiss = move |_: web_sys::MouseEvent| {
        let mut next = phase.get_untracked();
        let grant_id = take_unopened(&mut next);
        phase.set(next);
        if let (Some(dashboard), Some(grant_id)) = (dashboard, grant_id) {
            spawn_local(async move {
                let _ = BrowserViewerApi::revoke_grant(&dashboard, &grant_id).await;
            });
        }
    };
    let unpaired = move || caller_now(dashboard).is_none();

    view! {
        <section
            data-testid="browser-live-launch"
            class="flex flex-col gap-2" aria-labelledby="browser-viewer-launch">
            <h3 id="browser-viewer-launch" class="text-xs font-semibold text-text-secondary">
                {t_string!(i18n, browser_viewer.launch_title)}
            </h3>
            <p class="text-xs text-text-secondary">
                {t_string!(i18n, browser_viewer.launch_hint)}
            </p>
            {move || unpaired().then(|| view! { <PairingNotice dashboard=dashboard /> })}
            {move || match phase.get() {
                LaunchPhase::Ready(_) | LaunchPhase::Blocked(_) => view! {
                    <div class="flex gap-2">
                        <button
                            data-testid="browser-live-open"
                            type="button"
                            class="px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover"
                            on:click=open
                        >
                            {t_string!(i18n, browser_viewer.launch_open)}
                        </button>
                        <button
                            data-testid="browser-live-dismiss"
                            type="button"
                            class="px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover"
                            on:click=dismiss
                        >
                            {t_string!(i18n, browser_viewer.launch_dismiss)}
                        </button>
                    </div>
                }.into_any(),
                LaunchPhase::Issuing => view! {
                    <button type="button" class="px-2 py-1 rounded border border-border text-xs" disabled=true>
                        {t_string!(i18n, browser_viewer.launch_busy)}
                    </button>
                }.into_any(),
                LaunchPhase::Opened => view! {
                    <p class="text-xs text-text-secondary" role="status">
                        {t_string!(i18n, browser_viewer.launch_opened)}
                    </p>
                }.into_any(),
                LaunchPhase::Failed => view! {
                    <div class="flex flex-col gap-1">
                        <p class="text-xs text-text-secondary" role="status">
                            {t_string!(i18n, browser_viewer.launch_failed)}
                        </p>
                        <button
                            type="button"
                            class="self-start px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover disabled:opacity-50"
                            prop:disabled=unpaired
                            on:click=issue
                        >
                            {t_string!(i18n, browser_viewer.launch_open)}
                        </button>
                    </div>
                }.into_any(),
                LaunchPhase::Idle => view! {
                    <button
                        type="button"
                        class="self-start px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover"
                        prop:disabled=move || workspace.is_none() || unpaired()
                        on:click=issue
                    >
                        {t_string!(i18n, browser_viewer.launch_open)}
                    </button>
                }.into_any(),
            }}
        </section>
    }
}

/// A browser-viewer grant can only be requested for the exact safe resource
/// represented by this summary. The issuer's URL is handed to the browser
/// unchanged after the launch seam validates its route and grant id.
fn launch_target_for_bound(bound: &BoundBrowser) -> Option<LaunchTarget> {
    super::launch::launch_target(bound, now_ms()).ok()
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
            {launch_target_for_bound(&bound).map(|target| {
                view! { <LaunchSection target=target workspace=workspace /> }
            })}
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
            Some(Withheld::Pending) => {
                // Snapshot the exact revision being withheld; the decision is
                // addressed to it and nothing else.
                let resource_id = bound.resource().resource_id.clone();
                let revision = bound.resource().revision;
                view! {
                    <p class="text-sm text-text-secondary" role="status">
                        {t_string!(i18n, browser_viewer.status_pending)}
                    </p>
                    <PendingDecision resource_id=resource_id revision=revision />
                }
                .into_any()
            }
            Some(Withheld::Refused) => view! {
                <p class="text-sm text-text-secondary" role="status">
                    {t_string!(i18n, browser_viewer.status_refused)}
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

    fn ready_phase() -> LaunchPhase {
        let mut phase = LaunchPhase::Issuing;
        let revoke = finish_issue(
            &mut phase,
            true,
            Ok(aleph_protocol::browser_live::IssueViewerGrantResponse {
                launch_url: "https://gw.example/viewer/browser#grant_id=g-1&nonce=n_2".into(),
                grant_id: "g-1".into(),
                expires_at_ms: 2_000,
            }),
            1_000,
        );
        assert_eq!(revoke, None);
        assert!(matches!(phase, LaunchPhase::Ready(_)));
        phase
    }

    #[test]
    fn owner_cleanup_can_collect_the_unopened_grant_before_signal_disposal() {
        let owner = Owner::new();
        let collected = std::sync::Arc::new(std::sync::Mutex::new(None));
        owner.with(|| {
            let phase = RwSignal::new(ready_phase());
            let collected = collected.clone();
            on_cleanup(move || {
                *collected.lock().unwrap() = unopened_grant_for_cleanup(phase);
            });
        });
        owner.cleanup();
        assert_eq!(collected.lock().unwrap().as_deref(), Some("g-1"));
    }

    #[test]
    fn a_live_view_retains_the_ready_grant_without_revocation() {
        let owner = Owner::new();
        owner.with(|| {
            let phase = RwSignal::new(LaunchPhase::Issuing);
            assert_eq!(
                commit_launch_phase(phase, ready_phase(), None, Some("g-1".into())),
                None
            );
            assert!(matches!(phase.get_untracked(), LaunchPhase::Ready(_)));
        });
        owner.cleanup();
    }

    #[test]
    fn a_disposed_view_returns_the_ready_grant_for_revocation() {
        let owner = Owner::new();
        let phase = owner.with(|| RwSignal::new(LaunchPhase::Issuing));
        owner.cleanup();
        assert_eq!(
            commit_launch_phase(phase, ready_phase(), None, Some("g-1".into())),
            Some("g-1".into())
        );
    }
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
    fn a_decision_in_flight_refuses_reentry_but_a_failure_allows_retry() {
        let mut state = DecisionUi::Idle;
        assert!(begin_decision(&mut state));
        assert_eq!(state, DecisionUi::Busy);
        assert!(!begin_decision(&mut state));
        state = DecisionUi::Failed;
        assert!(begin_decision(&mut state));
        assert_eq!(state, DecisionUi::Busy);
    }

    #[test]
    fn launch_needs_the_same_target_and_the_same_paired_caller() {
        let alice = ViewerCaller::derive_from(Some("alice"), true, 1);
        let bob = ViewerCaller::derive_from(Some("bob"), true, 1);
        assert!(launch_authorized(true, alice.as_ref(), alice.as_ref()));
        assert!(!launch_authorized(false, alice.as_ref(), alice.as_ref()));
        assert!(!launch_authorized(true, alice.as_ref(), bob.as_ref()));
        assert!(!launch_authorized(true, alice.as_ref(), None));
        assert!(!launch_authorized(true, None, None));
    }

    #[test]
    fn invalidation_never_touches_non_unopened_phases() {
        for start in [LaunchPhase::Idle, LaunchPhase::Issuing, LaunchPhase::Opened] {
            let mut phase = start.clone();
            assert_eq!(invalidate_unopened(&mut phase, false), None);
            assert_eq!(phase, start);
        }
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
