//! Session detail view with transcript, tool output, and attachment tabs.

use crate::api::session_management::{SessionData, SessionsApi};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::attachments::AttachmentGallery;
use super::tools::ToolOutputsTab;
use super::transcript::{MessageRole, TranscriptMessage, TranscriptViewer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Transcript,
    ToolOutputs,
    Attachments,
}

#[component]
#[must_use]
pub fn SessionDetail(session_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let active_tab = RwSignal::new(Tab::Transcript);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let session_data = RwSignal::new(Option::<SessionData>::None);
    let transcript_messages = RwSignal::new(Vec::<TranscriptMessage>::new());

    let id_for_effect = session_id.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let state = state.clone();
        let id = id_for_effect.clone();
        spawn_local(async move {
            loading.set(true);
            match SessionsApi::get(&state, &id).await {
                Ok(data) => {
                    transcript_messages.set(parse_transcript(&data.transcript));
                    session_data.set(Some(data));
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
        });
    });

    view! {
        <div class="flex flex-col h-full">
            <div class="px-6 py-4 border-b border-border bg-surface-raised">
                <h1 class="text-2xl font-bold text-text-primary">
                    {move || session_data.get().map(|data| data.metadata.agent_id).unwrap_or_else(|| "Session".to_string())}
                </h1>
                <p class="text-xs text-text-tertiary mt-1 font-mono">{format!("Session ID: {session_id}")}</p>
            </div>
            <div class="flex border-b border-border bg-surface-base">
                <TabButton tab=Tab::Transcript active_tab label="Transcript" count=Signal::derive(move || transcript_messages.get().len()) />
                <TabButton tab=Tab::ToolOutputs active_tab label="Tool Outputs" count=Signal::derive(|| 0usize) />
                <TabButton tab=Tab::Attachments active_tab label="Attachments" count=Signal::derive(|| 0usize) />
            </div>
            <div class="flex-1 overflow-auto">
                {move || {
                    if loading.get() {
                        view! { <div class="flex items-center justify-center h-full p-6 text-text-tertiary">"Loading session..."</div> }.into_any()
                    } else if let Some(message) = error.get() {
                        view! { <div class="p-6 text-danger">{message}</div> }.into_any()
                    } else {
                        match active_tab.get() {
                            Tab::Transcript => view! { <TranscriptViewer messages=Signal::derive(move || transcript_messages.get()) /> }.into_any(),
                            Tab::ToolOutputs => view! { <div class="p-6"><ToolOutputsTab session_id=session_id.clone() /></div> }.into_any(),
                            Tab::Attachments => view! { <div class="p-6"><AttachmentGallery session_id=session_id.clone() /></div> }.into_any(),
                        }
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn TabButton(
    tab: Tab,
    active_tab: RwSignal<Tab>,
    label: &'static str,
    count: Signal<usize>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            on:click=move |_| active_tab.set(tab)
            class=move || if active_tab.get() == tab { "px-6 py-3 font-medium text-sm text-primary border-b-2 border-primary" } else { "px-6 py-3 font-medium text-sm text-text-secondary hover:text-text-primary" }
        >
            {label}
            <span class="ml-2 px-2 py-0.5 text-xs rounded-full bg-surface-sunken text-text-tertiary font-mono">{move || count.get()}</span>
        </button>
    }
}

fn parse_transcript(raw: &str) -> Vec<TranscriptMessage> {
    let mut messages = Vec::new();
    let mut current: Option<TranscriptMessage> = None;

    for line in raw.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            if let Some(message) = current.take() {
                messages.push(message);
            }
            let (role, timestamp) = header.split_once(" - ").unwrap_or((header, ""));
            let role = match role.trim().to_ascii_lowercase().as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                _ => MessageRole::System,
            };
            current = Some(TranscriptMessage {
                role,
                content: String::new(),
                timestamp: (!timestamp.is_empty()).then(|| timestamp.to_string()),
            });
        } else if let Some(message) = current.as_mut() {
            if !message.content.is_empty() {
                message.content.push('\n');
            }
            message.content.push_str(line);
        }
    }
    if let Some(message) = current {
        messages.push(message);
    }
    messages
}
