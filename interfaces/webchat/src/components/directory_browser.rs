//! Cross-platform directory picker — Modal that browses the *server's*
//! filesystem via `fs.*` RPCs. Same UX whether the panel is loaded by
//! Tauri shell, localhost web tab, or remote Tailnet web.
//!
//! Why we don't use a native OS dialog: see [R6 in CLAUDE.md] — a Tauri
//! `pickDirectory` only sees the *client's* laptop, which is wrong the
//! moment server and client are on different machines (Tailnet). Routing
//! through the server is the only correct semantics.
//!
//! Usage pattern in a parent component:
//! ```ignore
//! let open = RwSignal::new(false);
//! let on_pick = Callback::new(move |path: String| { /* … use the path */ });
//!
//! view! {
//!     <DirectoryBrowser
//!         open
//!         on_pick
//!         title="Use existing folder"
//!         confirm_label="Choose this directory"
//!     />
//! }
//! ```
//!
//! The component owns its own breadcrumb / hidden-files / loading / error
//! state internally; the parent only sees the final picked path.

use crate::api::fs::{AllowedRoot, DirEntry, FsApi, ListDirResult};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// `<DirectoryBrowser />` — a modal directory picker rooted at the server.
///
/// - `open`        — bidirectional open/close signal; component sets it to
///                   false on cancel + after a successful pick.
/// - `on_pick`     — invoked with the absolute server path the user chose.
/// - `title`       — header text (e.g. "Use existing folder" / "New blank project — choose parent directory").
/// - `confirm_label` — bottom-right button label (e.g. "Choose this directory").
#[component]
#[must_use]
pub fn DirectoryBrowser(
    open: RwSignal<bool>,
    on_pick: Callback<String>,
    /// Header text; reactive so a single instance can serve multiple
    /// purposes (e.g. "pick existing" vs "new blank — choose parent").
    #[prop(into)]
    title: Signal<String>,
    /// Bottom-right confirm button label — also reactive for the same
    /// reason as `title`.
    #[prop(into)]
    confirm_label: Signal<String>,
    /// When true, the first time the modal settles into a directory after
    /// opening it auto-pops the inline "New subdirectory" input — the "new blank
    /// project" entry point, so the user names a fresh folder immediately.
    #[prop(optional, into)]
    auto_create: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = expect_context::<DashboardState>();

    // `current_path = None` means "show the list of configured roots."
    // Switching to `Some(path)` triggers a `fs.list_dir` against it.
    let current_path: RwSignal<Option<String>> = RwSignal::new(None);
    let roots: RwSignal<Vec<AllowedRoot>> = RwSignal::new(Vec::new());
    let listing: RwSignal<Option<ListDirResult>> = RwSignal::new(None);
    let show_hidden = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    // Inline "New subdirectory" prompt: when Some(""), prompt is open and the
    // text field is mounted. Empty string == initial blank value.
    let create_name: RwSignal<Option<String>> = RwSignal::new(None);
    // Armed by `auto_create` on each open; fires once the modal lands in a
    // directory (see the effect below), then disarms so deeper navigation
    // doesn't keep re-opening the create prompt.
    let auto_create_armed = RwSignal::new(false);

    // When the modal first opens, fetch the root list. Reset every time
    // it re-opens so the user always starts at the top.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        // Reset on every open.
        current_path.set(None);
        listing.set(None);
        error.set(None);
        create_name.set(None);
        auto_create_armed.set(auto_create.get_untracked());
        let dash = dashboard;
        spawn_local(async move {
            busy.set(true);
            match FsApi::allowed_roots(&dash).await {
                Ok(r) => {
                    if r.len() == 1 {
                        // Single root → skip the roots screen and dive
                        // straight in. Cuts one click for the common case.
                        let only = r[0].path.clone();
                        roots.set(r);
                        current_path.set(Some(only));
                    } else {
                        roots.set(r);
                    }
                }
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("fs.allowed_roots: {e}")
                    }),
                )),
            }
            busy.set(false);
        });
    });

    // Re-list whenever the path (or hidden toggle) changes. Skipped while
    // `current_path == None` — that branch renders the root list directly.
    Effect::new(move |_| {
        let Some(path) = current_path.get() else {
            return;
        };
        let hidden = show_hidden.get();
        let dash = dashboard;
        spawn_local(async move {
            busy.set(true);
            error.set(None);
            match FsApi::list_dir(&dash, &path, hidden).await {
                Ok(res) => listing.set(Some(res)),
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("fs.list_dir: {e}"),
                    )));
                    listing.set(None);
                }
            }
            busy.set(false);
        });
    });

    // "New blank project": once we settle into a directory after opening,
    // pop the inline create-folder input so the user can name a fresh
    // folder right away — no flaky `window.prompt`. Disarms after firing.
    Effect::new(move |_| {
        if current_path.get().is_some()
            && auto_create_armed.get()
            && create_name.get_untracked().is_none()
        {
            auto_create_armed.set(false);
            create_name.set(Some(String::new()));
        }
    });

    // ── Action handlers ────────────────────────────────────────────────

    let navigate_into = move |path: String| {
        current_path.set(Some(path));
    };

    let go_up = move |_| {
        let Some(parent) = listing.get_untracked().and_then(|l| l.parent) else {
            // No parent in scope → back to root list (only useful when
            // there's more than one root; single-root case the button is
            // disabled).
            if roots.get_untracked().len() > 1 {
                current_path.set(None);
                listing.set(None);
            }
            return;
        };
        current_path.set(Some(parent));
    };

    let confirm_pick = move |_| {
        let Some(path) = current_path.get_untracked() else {
            return;
        };
        on_pick.run(path);
        open.set(false);
    };

    let cancel = move |_| {
        open.set(false);
    };

    let toggle_hidden = move |_| {
        show_hidden.update(|v| *v = !*v);
    };

    // These three are no-arg actions so they can be invoked from both
    // mouse clicks (`move |_| begin_create()`) and key events (`Enter` /
    // `Escape` inside the inline create-folder input) without arg-type
    // friction.
    let begin_create = move || {
        if current_path.get_untracked().is_some() {
            create_name.set(Some(String::new()));
        }
    };

    let cancel_create = move || {
        create_name.set(None);
    };

    let submit_create = move || {
        let Some(name) = create_name.get_untracked() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let Some(parent) = current_path.get_untracked() else {
            return;
        };
        let dash = dashboard;
        spawn_local(async move {
            busy.set(true);
            error.set(None);
            match FsApi::create_dir(&dash, &parent, &name).await {
                Ok(new_path) => {
                    create_name.set(None);
                    // Re-list current dir to show the new entry, then auto-
                    // navigate into it (matches Finder/Files.app behaviour
                    // after "New Folder").
                    // `show_hidden` is component-owned and the browser is a
                    // modal: dismissing it while `create_dir` is in flight
                    // disposes this signal, and a plain read would panic the
                    // panel. Nothing left to re-list for, so bail.
                    let Some(hidden) = show_hidden.try_get_untracked() else {
                        return;
                    };
                    if let Ok(res) = FsApi::list_dir(&dash, &parent, hidden).await {
                        listing.set(Some(res));
                    }
                    current_path.set(Some(new_path));
                }
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("fs.create_dir: {e}"),
                ))),
            }
            busy.set(false);
        });
    };

    // ── Sub-views ──────────────────────────────────────────────────────

    let roots_view = move || {
        let list = roots.get();
        if list.is_empty() {
            return view! {
                <div class="px-4 py-8 text-center text-sm text-text-tertiary">
                    {t!(i18n, common.browser_no_roots)}
                </div>
            }
            .into_any();
        }
        view! {
            <ul class="divide-y divide-border-subtle">
                {list.into_iter().map(|root| {
                    let path_for_click = root.path.clone();
                    view! {
                        <li>
                            <button
                                type="button"
                                class="w-full text-left px-4 py-3 hover:bg-surface-sunken flex items-center gap-3 text-sm"
                                on:click=move |_| {
                                    navigate_into(path_for_click.clone());
                                }
                            >
                                <svg viewBox="0 0 16 16" class="w-4 h-4 text-primary shrink-0" fill="none" stroke="currentColor" stroke-width="1.5">
                                    <path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.379a1.5 1.5 0 0 1 1.06.44L8.5 5h4A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-6.5z" />
                                </svg>
                                <span class="font-medium">{root.label}</span>
                                <span class="text-text-tertiary text-xs truncate">{root.path}</span>
                            </button>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ul>
        }
        .into_any()
    };

    let entries_view = move || {
        let Some(listing) = listing.get() else {
            return view! { <div></div> }.into_any();
        };
        let entries = listing.entries;
        if entries.is_empty() {
            return view! {
                <div class="px-4 py-8 text-center text-sm text-text-tertiary">
                    {t!(i18n, common.browser_empty_dir)}
                </div>
            }
            .into_any();
        }
        view! {
            <ul class="divide-y divide-border-subtle">
                {entries.into_iter().map(|e: DirEntry| {
                    let is_dir = e.is_dir;
                    let path_for_click = e.path.clone();
                    let name = e.name;
                    view! {
                        <li>
                            <button
                                type="button"
                                class=if is_dir {
                                    "w-full text-left px-4 py-2 hover:bg-surface-sunken flex items-center gap-3 text-sm"
                                } else {
                                    "w-full text-left px-4 py-2 flex items-center gap-3 text-sm text-text-tertiary cursor-default"
                                }
                                disabled=!is_dir
                                on:click=move |_| {
                                    if is_dir {
                                        navigate_into(path_for_click.clone());
                                    }
                                }
                            >
                                {if is_dir {
                                    view! {
                                        <svg viewBox="0 0 16 16" class="w-4 h-4 text-primary shrink-0" fill="none" stroke="currentColor" stroke-width="1.5">
                                            <path d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.379a1.5 1.5 0 0 1 1.06.44L8.5 5h4A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5v-6.5z" />
                                        </svg>
                                    }.into_any()
                                } else {
                                    view! {
                                        <svg viewBox="0 0 16 16" class="w-4 h-4 text-text-tertiary shrink-0" fill="none" stroke="currentColor" stroke-width="1.5">
                                            <path d="M4 2.5a1.5 1.5 0 0 1 1.5-1.5h4.379a1.5 1.5 0 0 1 1.06.44l2.621 2.62a1.5 1.5 0 0 1 .44 1.06v8.38a1.5 1.5 0 0 1-1.5 1.5h-7a1.5 1.5 0 0 1-1.5-1.5v-11z" />
                                        </svg>
                                    }.into_any()
                                }}
                                <span class="truncate">{name}</span>
                            </button>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ul>
        }
        .into_any()
    };

    let header_path_label = move || {
        current_path
            .get()
            .unwrap_or_else(|| t_string!(i18n, common.browser_pick_root).to_string())
    };

    view! {
        <Show when=move || open.get() fallback=|| ()>
            // Backdrop — click to cancel.
            <div
                class="fixed inset-0 z-[60] bg-black/40 aleph-scrim"
                data-tauri-drag-region="false"
                on:click=cancel
            />
            // Modal card.
            <div
                class="fixed left-1/2 top-[12vh] -translate-x-1/2 z-[61] w-[min(640px,calc(100vw-32px))] \
                       glass bg-surface-overlay/85 border border-border \
                       rounded-2xl shadow-2xl overflow-hidden flex flex-col"
                data-tauri-drag-region="false"
                role="dialog"
                aria-modal="true"
                aria-label=move || title.get()
            >
                // ── Header ─────────────────────────────────────────────
                <div class="flex items-center gap-2 px-4 py-3 border-b border-border">
                    <h2 class="text-sm font-medium text-text-primary truncate flex-1">
                        {move || title.get()}
                    </h2>
                    <button
                        type="button"
                        class="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-surface-sunken"
                        title=move || t_string!(i18n, common.browser_close).to_string()
                        on:click=cancel
                    >
                        "×"
                    </button>
                </div>

                // ── Path bar ───────────────────────────────────────────
                <div class="flex items-center gap-2 px-4 py-2 border-b border-border-subtle text-xs">
                    <button
                        type="button"
                        class="px-2 py-1 rounded hover:bg-surface-sunken disabled:opacity-30 disabled:cursor-not-allowed"
                        disabled=move || current_path.get().is_none() && roots.get().len() <= 1
                        title=move || t_string!(i18n, common.browser_go_up).to_string()
                        on:click=go_up
                    >
                        {t!(i18n, common.browser_go_up_label)}
                    </button>
                    <span class="flex-1 truncate text-text-secondary font-mono">
                        {header_path_label}
                    </span>
                    <button
                        type="button"
                        class=move || {
                            let active = show_hidden.get();
                            format!(
                                "px-2 py-1 rounded text-xs {}",
                                if active { "bg-primary/15 text-primary" } else { "hover:bg-surface-sunken text-text-secondary" }
                            )
                        }
                        title=move || t_string!(i18n, common.browser_show_hidden).to_string()
                        on:click=toggle_hidden
                    >
                        {t!(i18n, common.browser_hidden_files)}
                    </button>
                    <button
                        type="button"
                        class="px-2 py-1 rounded hover:bg-surface-sunken disabled:opacity-30 disabled:cursor-not-allowed text-text-secondary"
                        disabled=move || current_path.get().is_none()
                        title=move || t_string!(i18n, common.browser_new_subdir).to_string()
                        on:click=move |_| begin_create()
                    >
                        {t!(i18n, common.browser_new_subdir_label)}
                    </button>
                </div>

                // ── Inline "create folder" prompt ──────────────────────
                // The two-button cluster is wrapped in its own div so the
                // outer flex row stays at <=3 children — works around a
                // tachys/leptos type-explosion when a single element has
                // too many heterogeneous siblings (InertElement + Input +
                // Button + Button blew the trait-bound recursion).
                <Show when=move || create_name.get().is_some()>
                    <div class="flex items-center gap-2 px-4 py-2 border-b border-border-subtle bg-surface-sunken/50">
                        <span class="text-xs text-text-tertiary shrink-0">{t!(i18n, common.browser_new_name)}</span>
                        <input
                            type="text"
                            autofocus
                            placeholder=move || t_string!(i18n, common.browser_new_name_placeholder).to_string()
                            class="flex-1 px-2 py-1 text-sm bg-surface-base border border-border rounded outline-none focus:border-primary"
                            prop:value=move || create_name.get().unwrap_or_default()
                            on:input=move |ev| {
                                create_name.set(Some(event_target_value(&ev)));
                            }
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" { submit_create(); }
                                else if ev.key() == "Escape" { cancel_create(); }
                            }
                        />
                        <div class="flex items-center gap-1 shrink-0">
                            <button
                                type="button"
                                class="px-3 py-1 text-xs rounded bg-primary text-white hover:bg-primary-hover"
                                on:click=move |_| submit_create()
                            >
                                {t!(i18n, common.browser_create)}
                            </button>
                            <button
                                type="button"
                                class="px-2 py-1 text-xs rounded hover:bg-surface-sunken text-text-secondary"
                                on:click=move |_| cancel_create()
                            >
                                {t!(i18n, common.browser_cancel)}
                            </button>
                        </div>
                    </div>
                </Show>

                // ── Listing ────────────────────────────────────────────
                <div class="flex-1 min-h-[260px] max-h-[55vh] overflow-y-auto">
                    <Show when=move || error.get().is_some()>
                        <div class="px-4 py-3 text-sm text-danger bg-danger/10 border-b border-danger/30">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>
                    <Show when=move || busy.get()>
                        <div class="px-4 py-2 text-xs text-text-tertiary">{t!(i18n, common.browser_loading)}</div>
                    </Show>
                    {move || {
                        if current_path.get().is_none() {
                            roots_view().into_any()
                        } else {
                            entries_view().into_any()
                        }
                    }}
                </div>

                // ── Footer ─────────────────────────────────────────────
                <div class="px-4 py-3 border-t border-border flex items-center justify-end gap-2">
                    <button
                        type="button"
                        class="px-3 py-1.5 text-sm rounded-lg hover:bg-surface-sunken text-text-secondary"
                        on:click=cancel
                    >
                        {t!(i18n, common.browser_cancel)}
                    </button>
                    <button
                        type="button"
                        class="px-4 py-1.5 text-sm rounded-lg bg-primary text-white hover:bg-primary-hover \
                               disabled:opacity-40 disabled:cursor-not-allowed"
                        disabled=move || current_path.get().is_none() || busy.get()
                        on:click=confirm_pick
                    >
                        {move || confirm_label.get()}
                    </button>
                </div>
            </div>
        </Show>
    }
}
