//! Draft limits, shared ingestion, type icons and lazy thumbnail chips.
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::components::right_pane::tray::{read_attachment, TrayPinDraftButton};
use crate::i18n::{t_string, use_i18n};
use crate::views::chat::state::PendingAttachment;

pub(super) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
pub(crate) const MAX_ATTACHMENT_SIZE_BYTES: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_ATTACHMENT_COUNT: usize = 10;
pub(crate) const THUMBNAIL_MAX_BYTES: u64 = 2 * 1024 * 1024;

fn attachment_key(att: &PendingAttachment) -> String {
    crate::components::right_pane::tray::stable_attachment_key(att)
}

/// Used by input, clipboard, drop and tray reuse, including after every await.
pub(crate) fn append_attachment(
    list: &mut Vec<PendingAttachment>,
    file: PendingAttachment,
) -> bool {
    if file.size > MAX_ATTACHMENT_SIZE_BYTES || list.len() >= MAX_ATTACHMENT_COUNT {
        return false;
    }
    list.push(file);
    true
}
fn remove_attachment(list: &mut Vec<PendingAttachment>, key: &str) {
    if let Some(index) = list.iter().position(|file| attachment_key(file) == key) {
        list.remove(index);
    }
}

/// Identical files may be attached twice. Their occurrence number disambiguates
/// render keys without making another file's key depend on its current index.
fn keyed_attachments(files: Vec<PendingAttachment>) -> Vec<(String, PendingAttachment)> {
    let mut occurrences = std::collections::HashMap::<String, usize>::new();
    files
        .into_iter()
        .map(|file| {
            let identity = attachment_key(&file);
            let occurrence = occurrences.entry(identity.clone()).or_default();
            let key = format!("{identity}:{occurrence}");
            *occurrence += 1;
            (key, file)
        })
        .collect()
}

pub(crate) fn read_file_list_into(
    files: &web_sys::FileList,
    attachments: RwSignal<Vec<PendingAttachment>>,
) {
    let available = attachments
        .try_with_untracked(|list| MAX_ATTACHMENT_COUNT.saturating_sub(list.len()))
        .unwrap_or_default();
    for index in 0..files.length().min(available as u32) {
        if let Some(file) = files.get(index) {
            read_file_into(file, attachments);
        }
    }
}

/// No retained FileReader closures: the bundled promise clears handlers on settle.
pub(crate) fn read_file_into(file: web_sys::File, attachments: RwSignal<Vec<PendingAttachment>>) {
    let size = file.size() as u64;
    if size > MAX_ATTACHMENT_SIZE_BYTES
        || attachments
            .try_with_untracked(|list| list.len() >= MAX_ATTACHMENT_COUNT)
            .unwrap_or(true)
    {
        return;
    }
    let name = file.name();
    let mime_type = if file.type_().is_empty() {
        "application/octet-stream".into()
    } else {
        file.type_()
    };
    spawn_local(async move {
        if let Ok(data_base64) = read_attachment(&file).await {
            // try_update is also the lifetime gate if the composer was disposed.
            let _ = attachments.try_update(|list| {
                append_attachment(
                    list,
                    PendingAttachment {
                        name,
                        mime_type,
                        data_base64,
                        size,
                    },
                )
            });
        }
    });
}

/// MIME first, extension fallback second; all eight spec families are distinct.
#[must_use]
pub fn file_icon(mime: &str, name: &str) -> &'static str {
    fn family(mime: &str) -> Option<&'static str> {
        if mime.starts_with("image/") {
            Some("▧")
        } else if [
            "javascript",
            "typescript",
            "python",
            "rust",
            "x-c",
            "x-java",
            "x-go",
        ]
        .iter()
        .any(|code| mime.contains(code))
        {
            Some("</>")
        } else if mime == "application/pdf" {
            Some("▤")
        } else if mime == "application/json" || mime.ends_with("+json") {
            Some("{ }")
        } else if mime.starts_with("text/") || mime.ends_with("/xml") || mime.ends_with("+xml") {
            Some("≡")
        } else if mime.contains("zip") || mime.contains("tar") || mime.contains("gzip") {
            Some("▥")
        } else {
            None
        }
    }
    if let Some(icon) = family(mime) {
        return icon;
    }
    let lower = name.to_ascii_lowercase();
    match lower.rsplit('.').next().unwrap_or_default() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" => "▧",
        "rs" | "js" | "jsx" | "ts" | "tsx" | "py" | "go" | "java" | "c" | "cpp" | "h" => "</>",
        "pdf" => "▤",
        "json" => "{ }",
        "txt" | "md" | "csv" | "xml" | "html" => "≡",
        "zip" | "tar" | "gz" | "tgz" => "▥",
        "bin" | "dat" if mime == "application/octet-stream" => "⊘",
        _ => "◇",
    }
}
#[must_use]
pub fn thumbnail_eligible(file: &PendingAttachment) -> bool {
    file.mime_type.starts_with("image/") && file.size < THUMBNAIL_MAX_BYTES
}

fn image_thumbnail(image: &web_sys::HtmlImageElement) -> Option<String> {
    let canvas = web_sys::window()?
        .document()?
        .create_element("canvas")
        .ok()?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()?;
    canvas.set_width(36);
    canvas.set_height(36);
    let context = canvas
        .get_context("2d")
        .ok()??
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .ok()?;
    context
        .draw_image_with_html_image_element_and_dw_and_dh(image, 0., 0., 36., 36.)
        .ok()?;
    canvas.to_data_url_with_type("image/png").ok()
}

#[component]
fn AttachmentChip(
    attachments: RwSignal<Vec<PendingAttachment>>,
    file: PendingAttachment,
) -> impl IntoView {
    let i18n = use_i18n();
    let thumbnail = RwSignal::new(None::<String>);
    let failed = RwSignal::new(false);
    let eligible = thumbnail_eligible(&file);
    let source = if eligible {
        format!("data:{};base64,{}", file.mime_type, file.data_base64)
    } else {
        String::new()
    };
    let name = file.name.clone();
    let icon = file_icon(&file.mime_type, &name);
    let icon_class = match icon {
        "▧" => "text-info",
        "≡" => "text-text-secondary",
        "▤" | "⊘" => "text-danger",
        "{ }" => "text-warning",
        "</>" => "text-primary",
        _ => "text-text-tertiary",
    };
    let size = format_size(file.size);
    let key = attachment_key(&file);
    view! {
        <div class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-surface-raised border border-border text-xs text-text-secondary">
            <span class=format!("relative w-9 h-9 shrink-0 flex items-center justify-center {icon_class}") aria-hidden="true">
                {icon}
                // The browser's lazy image load is the decode trigger. No effect decodes hidden chips.
                <Show when=move || eligible && !failed.get()>
                    <img src={let source = source.clone(); move || thumbnail.get().unwrap_or_else(|| source.clone())}
                        alt="" loading="lazy" width="36" height="36"
                        class="absolute inset-0 w-9 h-9 rounded object-cover bg-surface-muted"
                        on:load=move |event| {
                            if thumbnail.get_untracked().is_some() { return; }
                            let value = event.target().and_then(|target| target.dyn_into::<web_sys::HtmlImageElement>().ok()).and_then(|image| image_thumbnail(&image));
                            match value { Some(url) => { let _ = thumbnail.try_set(Some(url)); }, None => { let _ = failed.try_set(true); } }
                        }
                        on:error=move |_| { let _ = failed.try_set(true); }
                    />
                </Show>
            </span>
            <span class="max-w-[120px] truncate">{name}</span>
            <span class="text-text-tertiary">{size}</span>
            <TrayPinDraftButton attachment=file />
            <button type="button" class="ml-0.5 p-0.5 rounded hover:bg-danger/10 hover:text-danger transition-colors"
                title=move || t_string!(i18n, chat.remove).to_string()
                on:click=move |_| { let _ = attachments.try_update(|list| remove_attachment(list, &key)); }>
                <span aria-hidden="true">"×"</span>
            </button>
        </div>
    }
}

#[component]
pub(crate) fn AttachmentPreviewBar(attachments: RwSignal<Vec<PendingAttachment>>) -> impl IntoView {
    view! {
        <Show when=move || !attachments.get().is_empty()>
            <div class="flex flex-wrap gap-2 mb-2">
                <For each=move || keyed_attachments(attachments.get()) key=|(key, _)| key.clone() children=move |(_, file)| {
                    view! { <AttachmentChip attachments=attachments file=file /> }
                } />
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn att(name: &str, size: u64) -> PendingAttachment {
        PendingAttachment {
            name: name.into(),
            size,
            mime_type: "text/plain".into(),
            data_base64: String::new(),
        }
    }
    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }
    #[test]
    fn surviving_chip_deletes_itself_after_middle_removal() {
        let a = att("same", 1);
        let mut b = a.clone();
        b.data_base64 = "Yg==".into();
        let mut c = a.clone();
        c.data_base64 = "Yw==".into();
        let b_key = attachment_key(&b);
        let c_key = attachment_key(&c);
        assert_ne!(b_key, c_key);
        let mut rows = vec![a.clone(), b, c];
        remove_attachment(&mut rows, &b_key);
        remove_attachment(&mut rows, &c_key);
        assert_eq!(rows, vec![a]);
    }
    #[test]
    fn identical_files_have_distinct_keys_and_other_files_keep_their_keys() {
        let a = att("same", 1);
        let b = att("other", 1);
        let original = keyed_attachments(vec![a.clone(), a.clone(), b.clone()]);
        assert_ne!(original[0].0, original[1].0);
        let mut rows = vec![a.clone(), a, b];
        let key = attachment_key(&rows[0]);
        remove_attachment(&mut rows, &key);
        let surviving = keyed_attachments(rows);
        assert_eq!(original[2].0, surviving[1].0);
    }
    #[test]
    fn completion_order_cannot_exceed_limits() {
        let mut rows = vec![att("old", 1); MAX_ATTACHMENT_COUNT - 1];
        assert!(append_attachment(
            &mut rows,
            att("first completion", MAX_ATTACHMENT_SIZE_BYTES)
        ));
        assert!(!append_attachment(&mut rows, att("late completion", 1)));
        rows.clear();
        assert!(!append_attachment(
            &mut rows,
            att("oversize", MAX_ATTACHMENT_SIZE_BYTES + 1)
        ));
    }
    #[test]
    fn thumbnail_boundary_and_nonimage() {
        let mut a = att("x.png", THUMBNAIL_MAX_BYTES - 1);
        assert!(!thumbnail_eligible(&a));
        a.mime_type = "image/png".into();
        assert!(thumbnail_eligible(&a));
        a.size += 1;
        assert!(!thumbnail_eligible(&a));
    }
    #[test]
    fn all_eight_icon_families_and_extensions() {
        let cases = [
            ("image/png", "x", "▧"),
            ("text/plain", "x", "≡"),
            ("application/pdf", "x", "▤"),
            ("application/json", "x", "{ }"),
            ("text/javascript", "x", "</>"),
            ("application/zip", "x", "▥"),
            ("application/octet-stream", "x.bin", "⊘"),
            ("unknown/type", "x", "◇"),
        ];
        for (mime, name, expected) in cases {
            assert_eq!(file_icon(mime, name), expected);
        }
        for (name, expected) in [
            ("x.JPG", "▧"),
            ("x.rs", "</>"),
            ("x.pdf", "▤"),
            ("x.json", "{ }"),
            ("x.md", "≡"),
            ("x.tar", "▥"),
        ] {
            assert_eq!(file_icon("application/octet-stream", name), expected);
        }
        assert_eq!(
            file_icon("text/plain", "x.rs"),
            "≡",
            "a known MIME takes precedence"
        );
    }
}
