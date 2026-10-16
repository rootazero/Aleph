//! The browser slot owns only its inline presentation; the root workspace owns facts.

use super::super::SlotRenderCtx;
use crate::api::ViewerCaller;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::platform::wide::views::browser::standalone::StandaloneNotice;
use crate::platform::wide::views::browser::viewer::{
    now_ms, InlineDestination, InlineLaunchNotice, LaunchSection,
};
use crate::platform::wide::views::browser::{
    model_launch_target, BrowserViewer, BrowserViewerModel, BrowserWorkspace, LaunchTarget,
    StandaloneBrowserViewer, ViewerSession,
};
use aleph_protocol::browser_live::ViewerBinding;
use leptos::prelude::*;
use leptos::tachys::reactive_graph::OwnedView;

pub(super) fn render(_ctx: SlotRenderCtx) -> AnyView {
    // Hidden descriptors can be inspected without a mounted browser workspace.
    if use_context::<BrowserWorkspace>().is_none() {
        return ().into_any();
    }
    view! { <BrowserSlot /> }.into_any()
}

#[derive(Clone, PartialEq, Eq)]
struct InlineIdentity {
    target: LaunchTarget,
    binding: ViewerBinding,
    caller: ViewerCaller,
}

fn inline_identity(
    model: &BrowserViewerModel,
    caller: Option<ViewerCaller>,
    now: i64,
) -> Option<InlineIdentity> {
    Some(InlineIdentity {
        target: model_launch_target(model, now).ok()?,
        binding: model.bound()?.viewer().clone(),
        caller: caller?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FallbackReason {
    InvalidFragment,
    MountError,
    UserRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingState {
    NoGrant,
    Pending,
    Running { was_live: bool },
    FallbackToTab { reason: FallbackReason },
}

impl EmbeddingState {
    fn observe(&mut self, notice: StandaloneNotice) {
        let Self::Running { was_live } = self else {
            return;
        };
        match notice {
            StandaloneNotice::Connecting => {}
            StandaloneNotice::Live => *was_live = true,
            StandaloneNotice::InvalidLink => {
                *self = Self::FallbackToTab {
                    reason: FallbackReason::InvalidFragment,
                }
            }
            StandaloneNotice::Unavailable | StandaloneNotice::Failed | StandaloneNotice::Closed => {
                *self = Self::FallbackToTab {
                    reason: FallbackReason::MountError,
                }
            }
        }
    }

    fn running(self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

#[component]
fn BrowserSlot() -> impl IntoView {
    let i18n = use_i18n();
    let workspace = use_context::<BrowserWorkspace>();
    let dashboard = use_context::<DashboardState>();
    // Equality filters model activity: only exact eligibility/binding/caller changes
    // replace this branch and dispose its grant/transport owner.
    let identity = Memo::new(move |_| {
        let model = workspace?.model.try_get()?;
        inline_identity(
            &model,
            dashboard.and_then(|d| ViewerCaller::tracked(&d)),
            now_ms(),
        )
    });
    view! {
        <div class="flex flex-col gap-3 p-3 min-h-0" data-testid="browser-slot">
            {move || {
                let model = workspace.and_then(|w| w.model.try_get()).unwrap_or_default();
                view! { <BrowserViewer model=model standalone=false suppress_launch=true /> }
            }}
            {move || identity.get().map(|held| view! {
                <InlineBrowser held=held identity=identity workspace=workspace />
            }.into_any()).unwrap_or_else(|| view! {
                <div class="flex flex-col gap-2">
                    <button data-testid="browser-slot-open-tab" type="button" disabled=true
                        class="self-start px-2 py-1 rounded border border-border text-xs opacity-50"
                        title=t_string!(i18n, browser_viewer.slot_unavailable).to_string()
                    >{t_string!(i18n, browser_viewer.slot_open_tab)}</button>
                    <p role="status" class="text-xs text-text-secondary">{t_string!(i18n, browser_viewer.slot_unavailable)}</p>
                </div>
            }.into_any()).into_any()}
        </div>
    }
}

fn stop_viewer(owner: StoredValue<Option<Owner>>) {
    if let Some(owner) = owner.try_update_value(Option::take).flatten() {
        owner.cleanup();
    }
}

#[component]
fn InlineBrowser(
    held: InlineIdentity,
    identity: Memo<Option<InlineIdentity>>,
    workspace: Option<BrowserWorkspace>,
) -> impl IntoView {
    let i18n = use_i18n();
    let target = held.target.clone();
    let held = StoredValue::new(held);
    let current = move || matches!((identity.try_get_untracked(), held.try_get_value()), (Some(Some(now)), Some(held)) if now == held);
    let state = RwSignal::new(EmbeddingState::NoGrant);
    // The session is moved exactly once to its owner; no launch URL is retained.
    let pending = StoredValue::new(None::<(ViewerSession, String)>);
    let viewer_owner = StoredValue::new(None::<Owner>);
    let destination = InlineDestination {
        is_current: Callback::new(move |()| current()),
        accept: Callback::new(move |session| {
            if !current() || state.try_get_untracked().is_none() {
                return false;
            }
            if pending.try_set_value(Some(session)).is_some() {
                return false;
            }
            state
                .try_set(EmbeddingState::Running { was_live: false })
                .is_none()
        }),
        notice: Callback::new(move |notice| {
            if !current() {
                return;
            }
            let next = match notice {
                InlineLaunchNotice::NoGrant => EmbeddingState::NoGrant,
                InlineLaunchNotice::Pending => EmbeddingState::Pending,
                InlineLaunchNotice::Failed => EmbeddingState::FallbackToTab {
                    reason: FallbackReason::MountError,
                },
                InlineLaunchNotice::InvalidFragment => EmbeddingState::FallbackToTab {
                    reason: FallbackReason::InvalidFragment,
                },
            };
            let _ = state.try_set(next);
        }),
        tab_requested: Callback::new(move |()| {
            // Explicit tab navigation ends transport synchronously before issuing.
            stop_viewer(viewer_owner);
            let _ = pending.try_set_value(None);
            let _ = state.try_set(EmbeddingState::FallbackToTab {
                reason: FallbackReason::UserRequested,
            });
        }),
    };
    let on_notice = Callback::new(move |notice| {
        if !current() {
            return;
        }
        let Some(mut next) = state.try_get_untracked() else {
            return;
        };
        next.observe(notice);
        if !next.running() {
            stop_viewer(viewer_owner);
        }
        let _ = state.try_set(next);
    });
    view! {
        <LaunchSection target=target workspace=workspace inline=destination />
        <p data-testid="browser-slot-status" role="status" class="text-xs text-text-secondary">
            {move || match state.get() {
                EmbeddingState::NoGrant => t_string!(i18n, browser_viewer.slot_no_grant).to_string(),
                EmbeddingState::Pending => t_string!(i18n, browser_viewer.slot_pending).to_string(),
                EmbeddingState::Running { .. } => String::new(),
                EmbeddingState::FallbackToTab { reason: FallbackReason::InvalidFragment } => t_string!(i18n, browser_viewer.standalone_invalid).to_string(),
                EmbeddingState::FallbackToTab { reason: FallbackReason::MountError } => t_string!(i18n, browser_viewer.slot_failed).to_string(),
                EmbeddingState::FallbackToTab { reason: FallbackReason::UserRequested } => t_string!(i18n, browser_viewer.slot_tab_hint).to_string(),
            }}
        </p>
        <Show when=move || state.get().running()>
            {move || {
                let Some((session, origin)) = pending.try_update_value(Option::take).flatten() else { return ().into_any(); };
                let owner = Owner::new();
                let on_transport_ready = Callback::new(move |transport_owner| {
                    viewer_owner.set_value(Some(transport_owner));
                });
                let view = owner.with(|| {
                    view! {
                        <StandaloneBrowserViewer
                            session=session
                            origin=origin
                            compact=true
                            on_notice=on_notice
                            on_transport_ready=on_transport_ready
                        />
                    }
                    .into_any()
                });
                OwnedView::new_with_owner(view, owner).into_any()
            }}
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::wide::views::browser::standalone::StandaloneNotice;
    use aleph_protocol::browser_live::{
        BrowserLiveSnapshot, BrowserTargetBinding, Controller, ConversationBinding, DisplayPolicy,
        IssuerIdentity, PreviewKind, PreviewProvenance, PreviewResource, PreviewSource,
        RedactionStatus, ResourceBinding, ViewportCssPx,
    };

    fn resource() -> PreviewResource {
        PreviewResource {
            resource_id: "resource-5".into(),
            revision: 1,
            binding: ResourceBinding::Browser {
                viewer: ViewerBinding {
                    principal: IssuerIdentity::User {
                        user_id: "alice".into(),
                    },
                    task: ConversationBinding {
                        session_key: "agent:operator:browser".into(),
                        run_id: "run-7".into(),
                    },
                    target: BrowserTargetBinding {
                        profile: "default".into(),
                        browser_session_id: "session-3".into(),
                        target_id: "target-9".into(),
                    },
                },
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms: 2_000,
            redaction: RedactionStatus::Safe,
            companion_candidates: Vec::new(),
        }
    }

    fn caller() -> Option<ViewerCaller> {
        ViewerCaller::derive_from(Some("alice"), true, 1)
    }

    fn identity_for(resource: PreviewResource) -> Option<InlineIdentity> {
        inline_identity(
            &BrowserViewerModel::from_resource(resource).unwrap(),
            caller(),
            1_000,
        )
    }

    #[test]
    fn identity_survives_ready_to_live_and_activity_but_tracks_every_binding_field() {
        let expected = identity_for(resource());
        assert!(expected.is_some());
        let model = BrowserViewerModel::from_snapshot(BrowserLiveSnapshot {
            resource: resource(),
            connection_epoch: 11,
            stream_epoch: 13,
            last_frame_seq: 17,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            controller: Controller::Agent,
            control_epoch: 19,
        })
        .unwrap();
        assert!(expected == inline_identity(&model, caller(), 1_000));
        for changed in 0..8 {
            let mut resource = resource();
            let ResourceBinding::Browser { viewer } = &mut resource.binding else {
                unreachable!()
            };
            match changed {
                0 => resource.revision += 1,
                1 => resource.resource_id.push_str("-changed"),
                2 => {
                    viewer.principal = IssuerIdentity::User {
                        user_id: "bob".into(),
                    }
                }
                3 => viewer.task.run_id.push_str("-changed"),
                4 => viewer.task.session_key.push_str("-changed"),
                5 => viewer.target.profile.push_str("-changed"),
                6 => viewer.target.browser_session_id.push_str("-changed"),
                _ => viewer.target.target_id.push_str("-changed"),
            }
            assert!(expected != identity_for(resource));
        }
        assert!(
            expected
                != inline_identity(
                    &model,
                    ViewerCaller::derive_from(Some("alice"), true, 2),
                    1_000
                )
        );
        assert!(
            expected
                != inline_identity(
                    &model,
                    ViewerCaller::derive_from(Some("bob"), true, 1),
                    1_000
                )
        );
        assert!(inline_identity(&model, None, 1_000).is_none());
        assert!(inline_identity(&model, caller(), 2_000).is_none());
        let mut pending = resource();
        pending.redaction = RedactionStatus::Pending;
        assert!(identity_for(pending).is_none());
    }

    #[test]
    fn explicit_tab_stop_cleans_the_viewer_owner_once_before_parent_disposal() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let parent = Owner::new();
        let stopped = Arc::new(AtomicUsize::new(0));
        parent.with(|| {
            let owner = Owner::new();
            let counter = stopped.clone();
            owner.with(|| {
                on_cleanup(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                })
            });
            let held = StoredValue::new(Some(owner));
            stop_viewer(held);
            stop_viewer(held);
            assert_eq!(stopped.load(Ordering::SeqCst), 1);
        });
        parent.cleanup();
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn explicit_tab_stops_transport_owner_before_render_owner_disposal() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let parent = Owner::new();
        let transport_stopped = Arc::new(AtomicUsize::new(0));
        let render_disposed = Arc::new(AtomicUsize::new(0));
        parent.with(|| {
            let render_owner = Owner::new();
            let render_counter = render_disposed.clone();
            render_owner.with(|| {
                on_cleanup(move || {
                    render_counter.fetch_add(1, Ordering::SeqCst);
                });
            });
            let transport_owner = render_owner.with(Owner::new);
            let transport_counter = transport_stopped.clone();
            transport_owner.with(|| {
                on_cleanup(move || {
                    transport_counter.fetch_add(1, Ordering::SeqCst);
                });
            });
            let held = StoredValue::new(Some(transport_owner));
            stop_viewer(held);
            stop_viewer(held);
            assert_eq!(transport_stopped.load(Ordering::SeqCst), 1);
            assert_eq!(render_disposed.load(Ordering::SeqCst), 0);

            render_owner.cleanup();
            assert_eq!(transport_stopped.load(Ordering::SeqCst), 1);
            assert_eq!(render_disposed.load(Ordering::SeqCst), 1);
        });
        parent.cleanup();
        assert_eq!(transport_stopped.load(Ordering::SeqCst), 1);
        assert_eq!(render_disposed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fallback_is_terminal_until_an_explicit_new_attempt() {
        let mut state = EmbeddingState::Running { was_live: false };
        state.observe(StandaloneNotice::Connecting);
        assert_eq!(state, EmbeddingState::Running { was_live: false });
        state.observe(StandaloneNotice::Live);
        assert_eq!(state, EmbeddingState::Running { was_live: true });
        state.observe(StandaloneNotice::Closed);
        assert_eq!(
            state,
            EmbeddingState::FallbackToTab {
                reason: FallbackReason::MountError
            }
        );
        state.observe(StandaloneNotice::Connecting);
        state.observe(StandaloneNotice::Live);
        assert_eq!(
            state,
            EmbeddingState::FallbackToTab {
                reason: FallbackReason::MountError
            }
        );
    }

    #[test]
    fn invalid_and_unavailable_notices_have_fixed_fallback_reasons() {
        for (notice, reason) in [
            (
                StandaloneNotice::InvalidLink,
                FallbackReason::InvalidFragment,
            ),
            (StandaloneNotice::Unavailable, FallbackReason::MountError),
            (StandaloneNotice::Failed, FallbackReason::MountError),
        ] {
            let mut state = EmbeddingState::Running { was_live: false };
            state.observe(notice);
            assert_eq!(state, EmbeddingState::FallbackToTab { reason });
        }
    }
}
