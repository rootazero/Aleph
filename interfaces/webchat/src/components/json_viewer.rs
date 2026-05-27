//! Hierarchical collapsible JSON tree view.
//!
//! Inspired by UI-TARS-desktop's `JSONViewer.tsx` but rewritten as a
//! Leptos component so we can render `serde_json::Value` directly
//! without a DOM-string round-trip. Replaces the prior
//! `<pre>{pretty_json(...)}</pre>` flat dump used by
//! `components/workspace_panel.rs::PayloadBlock`.
//!
//! Single landing consumer: PayloadBlock. Kept narrow on purpose —
//! per R10/YAGNI, this is not a general-purpose tree widget; it
//! handles `serde_json::Value` and nothing else.
//!
//! Behaviour notes:
//! - Top two depth levels (root + first children) are expanded by default;
//!   anything deeper starts collapsed so large payloads don't blow up
//!   the viewport.
//! - Long strings (> `STRING_PREVIEW_LIMIT` chars) are clipped with an
//!   inline "expand" toggle. UTF-8 safe — uses `char_indices` rather
//!   than byte slicing so multi-byte sequences are never split.
//! - Primitives are type-coloured (string=emerald, number=sky,
//!   bool=purple, null=tertiary) matching the JSON token convention.
//! - Each composite node carries a `Copy` button that writes the
//!   pretty-printed subtree to `navigator.clipboard`.

use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Max characters of a string preview before the "expand" toggle appears.
/// 160 keeps a single line readable on a 340 px panel without wrap.
const STRING_PREVIEW_LIMIT: usize = 160;

/// Levels expanded by default. 1 means root + first child level are open;
/// grandchildren start collapsed. Matches UI-TARS's `level <= 1` default
/// and prevents 10 MB tool results from janking the panel on mount.
const DEFAULT_EXPANDED_DEPTH: usize = 1;

/// Recursive entry point.
///
/// `value` is taken by value so the caller can move-out — the component
/// keeps a clone for the lifetime of its overlay tree, which is correct
/// since `serde_json::Value` cloning is structural-sharing-free but
/// payloads here are bounded (per-tool-call args / results).
#[component]
pub fn JsonViewer(#[prop(into)] value: Value) -> impl IntoView {
    view! {
        <div class="font-mono text-xs leading-relaxed text-text-secondary overflow-x-auto">
            <JsonNode value=value depth=0 />
        </div>
    }
}

#[component]
fn JsonNode(value: Value, depth: usize) -> impl IntoView {
    match value {
        Value::Object(map) => {
            // Materialise to a Vec first so the `pairs=` attribute value
            // is a single identifier; Leptos's `view!` macro treats
            // `pairs=map.foo()` as a `pairs:map` namespaced attribute.
            let pairs: Vec<(Option<String>, Value)> =
                map.into_iter().map(|(k, v)| (Some(k), v)).collect();
            view! { <Composite kind=BracketKind::Object pairs=pairs depth=depth /> }.into_any()
        }
        Value::Array(arr) => {
            let pairs: Vec<(Option<String>, Value)> =
                arr.into_iter().map(|v| (None, v)).collect();
            view! { <Composite kind=BracketKind::Array pairs=pairs depth=depth /> }.into_any()
        }
        primitive => render_primitive(primitive),
    }
}

#[derive(Clone, Copy)]
enum BracketKind {
    Object,
    Array,
}

impl BracketKind {
    fn open(self) -> &'static str {
        match self {
            Self::Object => "{",
            Self::Array => "[",
        }
    }
    fn close(self) -> &'static str {
        match self {
            Self::Object => "}",
            Self::Array => "]",
        }
    }
    fn label(self, len: usize) -> String {
        match self {
            Self::Object => format!("{{ {len} keys }}"),
            Self::Array => format!("[ {len} items ]"),
        }
    }
}

#[component]
fn Composite(kind: BracketKind, pairs: Vec<(Option<String>, Value)>, depth: usize) -> impl IntoView {
    let open = RwSignal::new(depth <= DEFAULT_EXPANDED_DEPTH);
    let toggle = move |_| open.update(|o| *o = !*o);

    let len = pairs.len();
    let is_empty = len == 0;

    // Pretty-printed full subtree for the copy button. Built once per
    // node — Value is small relative to the wasm heap, and we'd pay
    // the cost anyway on first click.
    let copy_text = serialize_pretty(&match kind {
        BracketKind::Object => Value::Object(
            pairs
                .iter()
                .filter_map(|(k, v)| k.as_ref().map(|k| (k.clone(), v.clone())))
                .collect(),
        ),
        BracketKind::Array => Value::Array(pairs.iter().map(|(_, v)| v.clone()).collect()),
    });

    // Empty composites render inline ([] / {}) — no toggle needed.
    if is_empty {
        return view! {
            <span class="text-text-tertiary">{kind.open()}{kind.close()}</span>
        }
        .into_any();
    }

    view! {
        <div class="flex flex-col">
            <div class="flex items-center gap-1 group/json">
                <button
                    on:click=toggle
                    class="inline-flex items-center justify-center w-3.5 h-3.5 text-text-tertiary hover:text-text-primary transition-colors"
                    aria-label=move || if open.get() { "Collapse" } else { "Expand" }
                >
                    <span
                        class="inline-block transition-transform"
                        style=move || if open.get() { "transform: rotate(90deg);" } else { "" }
                    >"▸"</span>
                </button>
                <span class="text-text-tertiary">{kind.open()}</span>
                <Show when=move || !open.get() fallback=|| view! { <span /> }>
                    <span class="text-text-tertiary italic">
                        {kind.label(len)}
                    </span>
                    <span class="text-text-tertiary">{kind.close()}</span>
                </Show>
                <CopyButton text=copy_text />
            </div>
            <Show when=move || open.get() fallback=|| view! { <span /> }>
                <div class="ml-4 border-l border-border/40 pl-2 flex flex-col gap-0.5">
                    {pairs.clone().into_iter().enumerate().map(|(idx, (key, val))| {
                        view! {
                            <Row key=key index=idx value=val depth=depth + 1 />
                        }
                    }).collect_view()}
                </div>
                <span class="text-text-tertiary">{kind.close()}</span>
            </Show>
        </div>
    }
    .into_any()
}

#[component]
fn Row(key: Option<String>, index: usize, value: Value, depth: usize) -> impl IntoView {
    let label = match key {
        Some(k) => view! {
            <span class="text-text-secondary">"\""{k}"\""</span>
            <span class="text-text-tertiary mr-1">":"</span>
        }
        .into_any(),
        None => view! {
            <span class="text-text-tertiary mr-1">{format!("{index}:")}</span>
        }
        .into_any(),
    };
    view! {
        <div class="flex items-start gap-1 min-w-0">
            {label}
            <div class="flex-1 min-w-0">
                <JsonNode value=value depth=depth />
            </div>
        </div>
    }
}

fn render_primitive(v: Value) -> AnyView {
    match v {
        Value::Null => view! {
            <span class="text-text-tertiary italic">"null"</span>
        }
        .into_any(),
        Value::Bool(b) => view! {
            <span class="text-purple-400">{b.to_string()}</span>
        }
        .into_any(),
        Value::Number(n) => view! {
            <span class="text-sky-400">{n.to_string()}</span>
        }
        .into_any(),
        Value::String(s) => view! { <PrimitiveString text=s /> }.into_any(),
        _ => unreachable!("composites handled in JsonNode"),
    }
}

/// String primitive with UTF-8-safe truncation + per-string expand.
///
/// We do not use `&s[..N]` because `s` may contain multi-byte UTF-8
/// (Chinese, emoji) and slicing on a non-boundary panics. `char_indices`
/// gives us a safe boundary at exactly `STRING_PREVIEW_LIMIT` graphemes.
#[component]
fn PrimitiveString(text: String) -> impl IntoView {
    let expanded = RwSignal::new(false);
    let is_long = text.chars().count() > STRING_PREVIEW_LIMIT;
    let preview = if is_long {
        let cut = text
            .char_indices()
            .nth(STRING_PREVIEW_LIMIT)
            .map(|(idx, _)| idx)
            .unwrap_or(text.len());
        format!("{}…", &text[..cut])
    } else {
        text.clone()
    };
    let full_text = text;
    let toggle = move |_| expanded.update(|e| *e = !*e);

    view! {
        <span class="text-emerald-400 break-words">
            "\""
            {move || if !is_long || !expanded.get() {
                preview.clone()
            } else {
                full_text.clone()
            }}
            "\""
            <Show when=move || is_long fallback=|| view! { <span /> }>
                <button
                    on:click=toggle.clone()
                    class="ml-1 text-text-tertiary hover:text-text-primary text-[10px] underline"
                >
                    {move || if expanded.get() { "less" } else { "more" }}
                </button>
            </Show>
        </span>
    }
}

/// Copy-to-clipboard button. Uses the modern async Clipboard API
/// (`navigator.clipboard.writeText`) which is available in all
/// Tauri-embedded WebKit / WebView2 / WebKitGTK runtimes we ship to.
/// Best-effort — clipboard write failure is silent (e.g. browser
/// security context revoked permission). No `expect_throw`.
#[component]
fn CopyButton(text: String) -> impl IntoView {
    let copied = RwSignal::new(false);
    let to_copy = text;
    let on_click = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        let Some(win) = web_sys::window() else { return };
        let promise = win.navigator().clipboard().write_text(&to_copy);
        let copied_sig = copied;
        wasm_bindgen_futures::spawn_local(async move {
            if JsFuture::from(promise).await.is_ok() {
                copied_sig.set(true);
                // Reset the "✓" badge after ~1.5 s. Best-effort timeout
                // through setTimeout — no leak even if dropped early.
                if let Some(win) = web_sys::window() {
                    let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                        copied_sig.set(false);
                    });
                    let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                        cb.as_ref().unchecked_ref(),
                        1500,
                    );
                }
            }
        });
    };
    view! {
        <button
            on:click=on_click
            class="opacity-0 group-hover/json:opacity-60 hover:opacity-100 text-text-tertiary hover:text-text-primary text-[10px] transition-opacity ml-1"
            title="Copy"
        >
            {move || if copied.get() { "✓" } else { "⎘" }}
        </button>
    }
}

fn serialize_pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_label_object() {
        assert_eq!(BracketKind::Object.label(3), "{ 3 keys }");
    }

    #[test]
    fn bracket_label_array() {
        assert_eq!(BracketKind::Array.label(0), "[ 0 items ]");
    }

    #[test]
    fn serialize_pretty_handles_null() {
        assert_eq!(serialize_pretty(&Value::Null), "null");
    }

    #[test]
    fn serialize_pretty_indents_objects() {
        let v = serde_json::json!({"a": 1});
        let out = serialize_pretty(&v);
        assert!(out.contains('\n'));
        assert!(out.contains("\"a\""));
    }

    /// Regression: a UTF-8 string whose byte length crosses
    /// STRING_PREVIEW_LIMIT in the middle of a multi-byte code point
    /// must NOT panic during preview computation. We mirror the prod
    /// logic here without rendering to keep the test wasm-free.
    #[test]
    fn truncate_utf8_safe_across_multibyte() {
        let text: String = "中".repeat(STRING_PREVIEW_LIMIT + 10);
        let cut = text
            .char_indices()
            .nth(STRING_PREVIEW_LIMIT)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        // Must land on a UTF-8 boundary.
        assert!(text.is_char_boundary(cut));
        let preview = format!("{}…", &text[..cut]);
        assert!(preview.ends_with('…'));
    }
}
