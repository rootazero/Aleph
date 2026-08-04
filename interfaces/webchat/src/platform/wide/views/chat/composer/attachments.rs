//! Attachment helpers — size formatter, file-list reader, preview bar.
//!
//! The preview chips above the textarea live here so the composer
//! orchestrator only has to wire a signal and a callback. The reader
//! helper centralises the `FileReader` → data-URL → base64 dance so the
//! same logic is reachable from both the paperclip click and the
//! chat-surface drop zone (the latter not yet wired through here, but
//! the API is shaped for it).

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::views::chat::state::PendingAttachment;

/// Format a byte count as a human-readable label (`B` / `KB` / `MB`).
///
/// Pure — handy for unit tests; the preview bar reads this for each chip.
pub(super) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Push every entry of `file_list` onto `attachments` as a base64
/// `PendingAttachment`. Browser `FileReader` is async, so each entry is
/// appended independently when its `load` event fires.
///
/// Errors are silent (the chip simply never appears) — the previous
/// implementation behaved the same way and the alternative is a noisy
/// banner for permissions that the browser already surfaces.
pub(crate) fn read_file_list_into(
    file_list: &web_sys::FileList,
    attachments: RwSignal<Vec<PendingAttachment>>,
) {
    for i in 0..file_list.length() {
        let Some(file) = file_list.get(i) else {
            continue;
        };
        let name = file.name();
        let mime_type = file.type_();
        let size = file.size() as u64;

        let reader = match web_sys::FileReader::new() {
            Ok(r) => r,
            Err(_) => continue,
        };

        let reader_clone = reader.clone();
        let file_mime = if mime_type.is_empty() {
            "application/octet-stream".to_string()
        } else {
            mime_type
        };
        let file_name = name.clone();

        let onload = Closure::wrap(Box::new(move || {
            if let Ok(result) = reader_clone.result() {
                if let Some(data_url) = result.as_string() {
                    // data URL: "data:<mime>;base64,<data>"
                    let base64_data = data_url.split(',').nth(1).unwrap_or("").to_string();
                    let attachment = PendingAttachment {
                        name: file_name.clone(),
                        mime_type: file_mime.clone(),
                        data_base64: base64_data,
                        size,
                    };
                    attachments.update(|list| list.push(attachment));
                }
            }
        }) as Box<dyn Fn()>);

        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
        onload.forget();

        let _ = reader.read_as_data_url(&file);
    }
}

/// Horizontal chip strip rendered above the textarea, one chip per
/// pending attachment. Each chip carries its own ✕ — clearing the
/// draft text does NOT drop chips (composer-level decision).
#[component]
pub(crate) fn AttachmentPreviewBar(attachments: RwSignal<Vec<PendingAttachment>>) -> impl IntoView {
    let i18n = use_i18n();
    let on_remove = move |idx: usize| {
        attachments.update(|list| {
            if idx < list.len() {
                list.remove(idx);
            }
        });
    };

    view! {
        <Show when=move || !attachments.get().is_empty()>
            <div class="flex flex-wrap gap-2 mb-2">
                <For
                    each=move || {
                        attachments.get().into_iter().enumerate().collect::<Vec<_>>()
                    }
                    key=|(idx, f)| format!("{}:{}", idx, f.name)
                    children=move |(idx, file)| {
                        let file_name = file.name.clone();
                        let file_size = format_size(file.size);
                        view! {
                            <div class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg
                                        bg-surface-raised border border-border text-xs text-text-secondary">
                                <svg xmlns="http://www.w3.org/2000/svg"
                                     class="w-3.5 h-3.5 text-text-tertiary shrink-0"
                                     viewBox="0 0 20 20" fill="currentColor">
                                    <path fill-rule="evenodd"
                                          d="M4 4a2 2 0 0 1 2-2h4.586A2 2 0 0 1 12 2.586L15.414 6A2 2 0 0 1 16 7.414V16a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4Z"
                                          clip-rule="evenodd" />
                                </svg>
                                <span class="max-w-[120px] truncate">{file_name}</span>
                                <span class="text-text-tertiary">{file_size}</span>
                                <button
                                    class="ml-0.5 p-0.5 rounded hover:bg-danger/10 hover:text-danger transition-colors"
                                    title=move || t_string!(i18n, chat.remove).to_string()
                                    on:click=move |_| on_remove(idx)
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
                                    </svg>
                                </button>
                            </div>
                        }
                    }
                />
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_picks_smallest_unit() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn format_size_uses_kb_in_range() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn format_size_uses_mb_at_threshold() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 + 1024 * 512), "3.5 MB");
    }
}
