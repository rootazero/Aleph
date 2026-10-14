import json

def load(p):
    return open(p, encoding='utf8', newline='').read().replace('\r\n', '\n')

def save(p, s):
    open(p, 'w', encoding='utf8', newline='').write(s.replace('\n', '\r\n'))

def rep(s, old, new):
    assert s.count(old) == 1, old[:60]
    return s.replace(old, new, 1)

p = 'src/platform/wide/views/browser/viewer.rs'
s = load(p)
s = rep(s, '''//! Accessible status shell for the browser viewer.
//!
//! Renders whatever [`BrowserViewerModel`] already holds. It owns no target
//! and performs no I/O: no socket, no ticket exchange, no `location.hash`.
//! The only local state is which companion candidate is highlighted (restricted
//! to ids the bound resource itself published) and the in-flight flag of the
//! Safe / Refused decision on a `Pending` revision. That decision is the one
//! outbound call, and it is addressed by the exact `resource_id` + `revision`
//! the shown resource carried, never by URL, profile or current tab.
''', '''//! Accessible status shell for the browser viewer.
//!
//! Renders whatever [`BrowserViewerModel`] already holds. It owns no target
//! and opens no socket, performs no ticket exchange and reads no
//! `location.hash`. The only local state is which companion candidate is
//! highlighted (restricted to ids the bound resource itself published), the
//! in-flight flag of the Safe / Refused decision on a `Pending` revision, and
//! the Frames-only "open viewer" launch state of an exact Safe resource
//! ([`super::launch`]). Both outbound calls are addressed by the exact
//! `resource_id` (+ `revision` for the decision) the shown resource carried,
//! never by URL, profile or current tab. The launch URL the gateway returns is
//! handed, unchanged, to a user-initiated new-tab open and is neither rendered
//! nor logged.
''')
s = rep(s, '''use super::model::{
    BoundBrowser, BrowserViewerModel, BrowserWorkspace, UnboundReason, ViewerPhase,
};
''', '''use super::launch::{
    begin_issue, finish_issue, launch_target, model_launch_target, open_ready, take_unopened,
    LaunchBlock, LaunchPhase, LaunchTarget,
};
use super::model::{
    BoundBrowser, BrowserViewerModel, BrowserWorkspace, UnboundReason, ViewerPhase,
};
''')
s = rep(s, '''use leptos::task::spawn_local;
''', '''use leptos::task::spawn_local;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
''')

launch_comp = '''/// Wall-clock now in ms. Native builds (unit tests only) report 0, i.e. never
/// expired; the UI itself only runs under wasm.
#[cfg(target_arch = "wasm32")]
fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(not(target_arch = "wasm32"))]
const fn now_ms() -> i64 {
    0
}

/// Open the issuer's `launch_url` in a new tab, exactly as returned. `true`
/// only when a window came back (a blocked pop-up is `false`, so the caller
/// keeps the link for a retry). The child's `opener` is cleared: the viewer
/// needs no handle to the Panel.
#[cfg(target_arch = "wasm32")]
fn open_viewer_tab(url: &str) -> bool {
    use wasm_bindgen::JsValue;
    let Some(window) = web_sys::window() else {
        return false;
    };
    match window.open_with_url_and_target(url, "_blank") {
        Ok(Some(child)) => {
            let _ = child.set_opener(&JsValue::NULL);
            true
        }
        _ => false,
    }
}

/// Host unit tests have no window: the same answer a blocked pop-up gives.
#[cfg(not(target_arch = "wasm32"))]
const fn open_viewer_tab(_url: &str) -> bool {
    false
}

fn launch_block_text(i18n: I18nCtx, block: LaunchBlock) -> String {
    match block {
        LaunchBlock::NoResource => t_string!(i18n, browser_viewer.launch_block_unavailable),
        LaunchBlock::Closed => t_string!(i18n, browser_viewer.launch_block_closed),
        LaunchBlock::Expired => t_string!(i18n, browser_viewer.launch_block_expired),
        LaunchBlock::NotSafe => t_string!(i18n, browser_viewer.launch_block_not_safe),
    }
    .to_string()
}

fn launch_status_text(i18n: I18nCtx, phase: &LaunchPhase) -> String {
    match phase {
        LaunchPhase::Idle => String::new(),
        LaunchPhase::Issuing => t_string!(i18n, browser_viewer.launch_busy).to_string(),
        LaunchPhase::Ready(_) => t_string!(i18n, browser_viewer.launch_ready).to_string(),
        LaunchPhase::Blocked(_) => t_string!(i18n, browser_viewer.launch_blocked).to_string(),
        LaunchPhase::Opened => t_string!(i18n, browser_viewer.launch_opened).to_string(),
        LaunchPhase::Failed => t_string!(i18n, browser_viewer.launch_failed).to_string(),
    }
}

/// Best-effort revoke of a grant that will not be used.
fn revoke_in_background(dashboard: Option<DashboardState>, grant_id: String) {
    if let Some(dashboard) = dashboard {
        spawn_local(async move {
            let _ = BrowserViewerApi::revoke_grant(&dashboard, &grant_id).await;
        });
    }
}

/// "Open viewer" for one exact Safe resource: a Frames-only (view-only) grant
/// is requested from the gateway, then the user opens the issuer's link in a
/// new tab with a second click (a pop-up opened from an async callback would
/// be blocked). Built per render, so any model change (new revision, binding,
/// close) rebuilds it: the unopened link is revoked on teardown and a late
/// gateway answer for a stale view is revoked instead of adopted.
#[component]
fn LaunchSection(target: Result<LaunchTarget, LaunchBlock>) -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = use_context::<DashboardState>();
    let workspace = use_context::<BrowserWorkspace>();
    let phase = ArcRwSignal::new(LaunchPhase::Idle);
    let alive = Arc::new(AtomicBool::new(true));

    {
        let phase = phase.clone();
        let alive = alive.clone();
        on_cleanup(move || {
            alive.store(false, Ordering::SeqCst);
            let mut unopened = None;
            phase.update_untracked(|p| unopened = take_unopened(p));
            if let Some(grant_id) = unopened {
                revoke_in_background(dashboard, grant_id);
            }
        });
    }

    // Issuing needs both the authenticated connection and the workspace that
    // holds the model this section was derived from.
    let can_request = dashboard.is_some() && workspace.is_some() && target.is_ok();
    let block = target.as_ref().err().copied();

    let request = {
        let phase = phase.clone();
        let alive = alive.clone();
        let shown = target.clone();
        move |_| {
            let (Some(dashboard), Some(workspace), Ok(shown)) =
                (dashboard, workspace, shown.clone())
            else {
                return;
            };
            // The workspace must still hold exactly the target this button
            // was rendered for, and it must still be launchable now.
            let current = workspace
                .model
                .with_untracked(|m| model_launch_target(m, now_ms()));
            if current.as_ref() != Ok(&shown) {
                return;
            }
            let mut started = false;
            phase.update(|p| started = begin_issue(p));
            if !started {
                return;
            }
            let phase = phase.clone();
            let alive = alive.clone();
            spawn_local(async move {
                let result =
                    BrowserViewerApi::issue_frames_grant(&dashboard, &shown.resource_id).await;
                let still_current = alive.load(Ordering::SeqCst)
                    && workspace
                        .model
                        .try_with_untracked(|m| {
                            model_launch_target(m, now_ms()).as_ref() == Ok(&shown)
                        })
                        .unwrap_or(false);
                let mut stale = None;
                phase.update(|p| stale = finish_issue(p, still_current, result, now_ms()));
                if let Some(grant_id) = stale {
                    revoke_in_background(Some(dashboard), grant_id);
                }
            });
        }
    };

    let open = {
        let phase = phase.clone();
        move |_| {
            phase.update(|p| {
                let _ = open_ready(p, now_ms(), open_viewer_tab);
            });
        }
    };
    let dismiss = {
        let phase = phase.clone();
        move |_| {
            let mut unopened = None;
            phase.update(|p| unopened = take_unopened(p));
            if let Some(grant_id) = unopened {
                revoke_in_background(dashboard, grant_id);
            }
        }
    };

    let holding = {
        let phase = phase.clone();
        move || matches!(phase.get(), LaunchPhase::Ready(_) | LaunchPhase::Blocked(_))
    };
    let request_disabled = {
        let phase = phase.clone();
        move || !can_request || matches!(phase.get(), LaunchPhase::Issuing)
    };
    let status = {
        let phase = phase.clone();
        move || launch_status_text(i18n, &phase.get())
    };
    let button =
        "px-2 py-1 rounded border border-border text-xs hover:bg-surface-hover disabled:opacity-50";

    view! {
        <section class="flex flex-col gap-2" aria-labelledby="browser-viewer-launch">
            <h3 id="browser-viewer-launch" class="text-xs font-semibold text-text-secondary">
                {t_string!(i18n, browser_viewer.launch_title)}
            </h3>
            <p class="text-xs text-text-secondary">
                {t_string!(i18n, browser_viewer.launch_hint)}
            </p>
            {block.map(|block| view! {
                <p class="text-xs text-text-secondary" role="status">
                    {launch_block_text(i18n, block)}
                </p>
            })}
            <div class="flex gap-2">
                <Show
                    when=holding
                    fallback=move || view! {
                        <button
                            type="button"
                            class=button
                            prop:disabled=request_disabled.clone()
                            on:click=request.clone()
                        >
                            {t_string!(i18n, browser_viewer.launch_open)}
                        </button>
                    }
                >
                    <button type="button" class=button on:click=open.clone()>
                        {t_string!(i18n, browser_viewer.launch_open_tab)}
                    </button>
                    <button type="button" class=button on:click=dismiss.clone()>
                        {t_string!(i18n, browser_viewer.launch_dismiss)}
                    </button>
                </Show>
            </div>
            <p class="text-xs text-text-secondary" role="status">{status}</p>
        </section>
    }
}

'''
s = rep(s, '''/// Layout / animation metadata for one candidate, as plain text lines.''',
        launch_comp + '''/// Layout / animation metadata for one candidate, as plain text lines.''')
s = rep(s, '''#[component]
fn BoundSummary(bound: BoundBrowser) -> impl IntoView {
    let i18n = use_i18n();
    let text = bound_text(i18n, &bound);
''', '''#[component]
fn BoundSummary(bound: BoundBrowser, launch: bool) -> impl IntoView {
    let i18n = use_i18n();
    let text = bound_text(i18n, &bound);
    let launch_target = launch.then(|| launch_target(&bound, now_ms()));
''')
s = rep(s, '''                {candidate_list}
            </section>
        </div>''', '''                {candidate_list}
            </section>

            {launch_target.map(|target| view! { <LaunchSection target=target /> })}
        </div>''')
s = rep(s, '''None => view! { <BoundSummary bound=*bound /> }.into_any(),''',
        '''None => view! { <BoundSummary bound=*bound launch=!standalone /> }.into_any(),''')
save(p, s)

p = 'src/platform/wide/views/browser/mod.rs'
s = load(p)
s = rep(s, '''//! The Panel shell still does not issue grants or provide an authorized
//! in-panel viewer workflow. Until that grant/auth wiring exists, nothing in
//! the shell can name a browser target by itself, and no resource target is
//! chosen from a URL, profile or current tab.
''', '''//! The in-panel shell can ask the Gateway for a Frames-only (view-only) grant
//! on the exact Safe resource it shows and open the issuer's `launch_url`,
//! unchanged, in a new tab ([`launch`] holds the pure rules; `viewer.rs`
//! wires the authenticated RPC and the user-initiated open). There is no
//! in-panel frame rendering, no input and no navigation, and no resource
//! target is chosen from a URL, profile or current tab.
''')
s = rep(s, 'pub mod bridge;\n', 'pub mod bridge;\npub mod launch;\n')
save(p, s)

LOC = {
    'en': {
        "launch_title": "Open viewer",
        "launch_hint": "Opens a view-only browser viewer in a new tab. It shows frames only and cannot type, click or navigate.",
        "launch_open": "Open viewer",
        "launch_busy": "Requesting a viewer link from the gateway…",
        "launch_ready": "The viewer link is ready. It can be used once.",
        "launch_open_tab": "Open in new tab",
        "launch_dismiss": "Dismiss",
        "launch_opened": "The viewer was opened in a new tab.",
        "launch_blocked": "The browser blocked the new tab. Allow pop-ups and try again.",
        "launch_failed": "A viewer link could not be issued. Please try again.",
        "launch_block_closed": "This viewer has closed.",
        "launch_block_expired": "This preview has expired.",
        "launch_block_not_safe": "Only a preview marked safe can be opened in the viewer.",
        "launch_block_unavailable": "No browser preview is available to open.",
    },
    'zh': {
        "launch_title": "打开查看器",
        "launch_hint": "在新标签页中打开仅供查看的浏览器查看器：只显示画面，不能输入、点击或导航。",
        "launch_open": "打开查看器",
        "launch_busy": "正在向网关请求查看器链接…",
        "launch_ready": "查看器链接已就绪，仅可使用一次。",
        "launch_open_tab": "在新标签页中打开",
        "launch_dismiss": "放弃",
        "launch_opened": "已在新标签页中打开查看器。",
        "launch_blocked": "浏览器拦截了新标签页，请允许弹出窗口后重试。",
        "launch_failed": "未能签发查看器链接，请重试。",
        "launch_block_closed": "此查看器已关闭。",
        "launch_block_expired": "此预览已过期。",
        "launch_block_not_safe": "只有被标记为安全的预览才能在查看器中打开。",
        "launch_block_unavailable": "没有可打开的浏览器预览。",
    },
}
for lang, kv in LOC.items():
    p = f'locales/{lang}.json'
    s = load(p)
    anchor = '    "standalone_invalid"'
    block = ''.join(f'    "{k}": {json.dumps(v, ensure_ascii=False)},\n' for k, v in kv.items())
    s = rep(s, anchor, block + anchor)
    save(p, s)
    json.loads(s)
print('ok')
