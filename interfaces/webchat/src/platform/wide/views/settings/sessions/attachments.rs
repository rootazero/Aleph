//! Attachment Gallery
//!
//! Grid layout for session attachments with image previews and download buttons.

use crate::api::session_management::{AttachmentInfo, SessionsApi};
use crate::context::DashboardState;
use crate::i18n::use_i18n;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Get a file type icon based on MIME type
fn file_type_icon(mime_type: &str) -> &'static str {
    if mime_type.starts_with("image/") {
        "🖼️"
    } else if mime_type.starts_with("video/") {
        "🎬"
    } else if mime_type.starts_with("audio/") {
        "🔊"
    } else if mime_type == "application/pdf" {
        "📄"
    } else if mime_type.starts_with("text/") {
        "📝"
    } else if mime_type.contains("zip") || mime_type.contains("archive") {
        "📦"
    } else {
        "📎"
    }
}

/// Format file size in human-readable format
fn format_file_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Extract filename from path or use a default
fn extract_filename(path: &str) -> String {
    path.split('/').last().unwrap_or("attachment").to_string()
}

#[component]
#[must_use]
pub fn AttachmentGallery(session_id: String) -> impl IntoView {
    let _i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    let attachments = RwSignal::new(Vec::<AttachmentInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    let session_id_clone = session_id.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }

        let session_id = session_id_clone.clone();
        spawn_local(async move {
            loading.set(true);
            match SessionsApi::get(&state, &session_id).await {
                Ok(data) => {
                    attachments.set(data.attachments);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load attachments: {}", e)));
                }
            }
            loading.set(false);
        });
    });

    view! {
        <div class="space-y-4">
            {move || error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                    {e}
                </div>
            })}

            {move || {
                if loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center text-sm">
                            "Loading attachments..."
                        </div>
                    }.into_any();
                }

                let items = attachments.get();
                if items.is_empty() {
                    return view! {
                        <div class="text-text-tertiary py-8 text-center text-sm">
                            "No attachments in this session"
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4">
                        {items.into_iter().map(|attachment| {
                            let is_image = attachment.mime_type.starts_with("image/");
                            let download_url = SessionsApi::attachment_url(
                                &attachment.session_id,
                                &attachment.attachment_id
                            );
                            let filename = extract_filename(&attachment.file_path);
                            let file_size = format_file_size(attachment.size_bytes);
                            let icon = file_type_icon(&attachment.mime_type);

                            view! {
                                <div class="bg-surface-raised border border-border rounded-xl overflow-hidden hover:border-primary/50 transition-colors">
                                    // Preview area
                                    <div class="aspect-square bg-surface-sunken relative">
                                        {if is_image {
                                            view! {
                                                <img
                                                    src=download_url.clone()
                                                    alt=filename.clone()
                                                    class="w-full h-full object-cover"
                                                    loading="lazy"
                                                />
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="w-full h-full flex items-center justify-center">
                                                    <span class="text-6xl opacity-40">{icon}</span>
                                                </div>
                                            }.into_any()
                                        }}
                                    </div>

                                    // Info and download
                                    <div class="p-3 space-y-2">
                                        <div class="text-xs font-medium text-text-primary truncate" title=filename.clone()>
                                            {filename.clone()}
                                        </div>
                                        <div class="flex items-center justify-between text-[10px] text-text-tertiary">
                                            <span>{file_size}</span>
                                            <span class="truncate max-w-[80px]" title=attachment.mime_type.clone()>
                                                {attachment.mime_type.clone()}
                                            </span>
                                        </div>
                                        <a
                                            href=download_url
                                            download=filename
                                            class="block w-full px-2 py-1.5 rounded border border-border text-xs text-text-secondary hover:bg-surface-sunken transition-colors text-center"
                                        >
                                            "Download"
                                        </a>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
