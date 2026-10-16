use super::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

// Leptos 0.8 generates a public hidden implementation even for crate-local components.
#[allow(unreachable_pub)]
#[component]
pub(crate) fn TraySurface() -> impl IntoView {
    let tray = expect_context::<TrayState>();
    let chat = expect_context::<crate::platform::wide::views::chat::state::ChatState>();
    let i18n = crate::i18n::use_i18n();
    let reveal = RwSignal::new(false);
    let now = || now_millis();

    let upload = move |event: web_sys::Event| {
        let Some(input) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        if let Some(files) = input.files() {
            let files: Vec<_> = (0..files.length())
                .filter_map(|index| files.get(index))
                .collect();
            // Read a multi-selection sequentially to bound transient base64 copies.
            spawn_local(async move {
                for file in files {
                    if let Err(error) = tray.pin_file(file).await {
                        let _ = tray.fail(error);
                    }
                }
            });
        }
        input.set_value("");
    };
    view! {
        <div class="flex-1 min-h-0 flex flex-col px-3 py-2">
            <div class="flex items-center gap-2 pb-2 border-b border-border shrink-0">
                <span class="text-xs font-semibold">{crate::i18n::t!(i18n, tray.title)}</span>
                <span class="text-[11px] text-text-tertiary">{move || tray.items.get().len().to_string()}</span>
                <label class="text-[11px] text-primary cursor-pointer ml-auto">
                    {crate::i18n::t!(i18n, tray.upload)}
                    <input type="file" multiple class="sr-only" aria-label=move || crate::i18n::t_string!(i18n, tray.upload).to_string() on:change=upload />
                </label>
            </div>
            {move || tray.error.get().map(|_| view! {
                <div role="status" class="mt-2 px-2 py-1 rounded bg-error/10 text-error text-[11px]">{crate::i18n::t!(i18n, tray.failed)}</div>
            })}
            <div class="flex-1 min-h-0 overflow-y-auto py-2">
                {move || {
                    let all = tray.items.get();
                    let folded = folded_items(&all, now());
                    let rows = if reveal.get() { all } else { folded.visible };
                    if rows.is_empty() {
                        view! { <div class="text-xs text-text-tertiary">{crate::i18n::t!(i18n, tray.empty)}</div> }.into_any()
                    } else {
                        rows.into_iter().map(|item| view! {
                            <TrayRow item=item tray=tray chat=chat />
                        }).collect::<Vec<_>>().into_any()
                    }
                }}
            </div>
            {move || {
                let hidden = folded_items(&tray.items.get(), now()).hidden_count;
                (hidden > 0 && !reveal.get()).then(|| view! {
                    <button type="button" class="text-[11px] text-primary py-1" on:click=move |_| reveal.set(true)>
                        {move || crate::i18n::t_string!(i18n, tray.reveal, count=hidden).to_string()}
                    </button>
                })
            }}
        </div>
    }
}

#[component]
fn TrayRow(
    item: TrayItem,
    tray: TrayState,
    chat: crate::platform::wide::views::chat::state::ChatState,
) -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let name = item.name.clone();
    let source = move || match item.source {
        TraySource::DraftPin => crate::i18n::t_string!(i18n, tray.source_draft).to_string(),
        TraySource::ArtifactPin => crate::i18n::t_string!(i18n, tray.source_artifact).to_string(),
        TraySource::UserUpload => crate::i18n::t_string!(i18n, tray.source_upload).to_string(),
    };
    let size = crate::api::artifacts::human_size(item.size);
    let download_item = item.clone();
    let reuse_item = item.clone();
    let remove_item = item.clone();
    view! {
        <div class="flex items-center gap-2 py-1.5 border-b border-border/40">
            <span class="text-primary text-sm" aria-hidden="true">"◆"</span>
            <span class="min-w-0 flex-1">
                <span class="block text-xs truncate">{name}</span>
                <span class="block text-[11px] text-text-tertiary">{move || format!("{size} · {}", source())}</span>
            </span>
            <button type="button" class="text-[11px] text-text-secondary hover:text-primary" on:click=move |_| {
                let item = download_item.clone(); let tray = tray;
                spawn_local(async move { if let Err(error) = tray.download(item).await { let _ = tray.fail(error); } });
            }>{crate::i18n::t!(i18n, tray.download)}</button>
            <button type="button" class="text-[11px] text-text-secondary hover:text-primary"
                disabled=item.size > crate::platform::wide::views::chat::composer::attachments::MAX_ATTACHMENT_SIZE_BYTES
                title=move || crate::i18n::t_string!(i18n, tray.reuse_hint).to_string()
                on:click=move |_| {
                let item = reuse_item.clone(); let tray = tray; let chat = chat;
                spawn_local(async move {
                    if let Ok(bytes) = tray.read_bytes(item.clone()).await {
                        use crate::platform::wide::views::chat::composer::attachments::append_attachment;
                        if let Ok(data_base64) = storage::bytes_to_base64(&bytes) {
                            let _ = chat.pending_attachments.try_update(|rows| append_attachment(rows, PendingAttachment {
                                name: item.name, mime_type: item.mime_type, data_base64, size: bytes.len() as u64,
                            }));
                        }
                    }
                });
            }>{crate::i18n::t!(i18n, tray.reuse)}</button>
            <button type="button" class="text-[11px] text-danger hover:underline" on:click=move |_| {
                let item = remove_item.clone(); let tray = tray;
                spawn_local(async move { let _ = tray.remove(item).await; });
            }>{crate::i18n::t!(i18n, chat.remove)}</button>
        </div>
    }
}

#[allow(unreachable_pub)]
#[component]
pub(crate) fn TrayPinDraftButton(attachment: PendingAttachment) -> impl IntoView {
    let tray = expect_context::<TrayState>();
    let i18n = crate::i18n::use_i18n();
    let id = draft_id(&attachment);
    let pinned = Memo::new(move |_| {
        tray.items
            .with(|items| items.iter().any(|item| item.id == id))
    });
    let busy = RwSignal::new(false);
    view! {
        <button type="button" class="text-[11px] text-primary hover:underline disabled:opacity-50"
            disabled=move || busy.get() aria-pressed=move || pinned.get()
            title=move || crate::i18n::t_string!(i18n, tray.pin).to_string()
            on:click=move |_| {
                if busy.get_untracked() { return; }
                busy.set(true);
                let attachment = attachment.clone();
                spawn_local(async move {
                    if let Err(error) = tray.pin_draft(attachment).await { let _ = tray.fail(error); }
                    let _ = busy.try_set(false);
                });
            }>
            {move || if pinned.get() { crate::i18n::t_string!(i18n, tray.pinned).to_string() } else { crate::i18n::t_string!(i18n, tray.pin).to_string() }}
        </button>
    }
}

#[allow(unreachable_pub)]
#[component]
pub(crate) fn TrayPinArtifactButton(item: ArtifactItem) -> impl IntoView {
    let tray = expect_context::<TrayState>();
    let i18n = crate::i18n::use_i18n();
    let id = artifact_id(&item);
    let pinned = Memo::new(move |_| {
        tray.items
            .with(|items| items.iter().any(|item| item.id == id))
    });
    let busy = RwSignal::new(false);
    view! {
        <button type="button" class="text-[11px] text-primary hover:underline disabled:opacity-50"
            disabled=move || busy.get() aria-pressed=move || pinned.get()
            title=move || crate::i18n::t_string!(i18n, tray.pin).to_string()
            on:click=move |event: web_sys::MouseEvent| {
                event.stop_propagation();
                if busy.get_untracked() { return; }
                busy.set(true);
                let item = item.clone();
                spawn_local(async move {
                    if let Err(error) = tray.pin_artifact(item).await { let _ = tray.fail(error); }
                    let _ = busy.try_set(false);
                });
            }>
            {move || if pinned.get() { crate::i18n::t_string!(i18n, tray.pinned).to_string() } else { crate::i18n::t_string!(i18n, tray.pin).to_string() }}
        </button>
    }
}
