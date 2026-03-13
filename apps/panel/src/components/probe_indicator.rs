//! Probe status indicator
//!
//! Displays connection test result: loading spinner, success checkmark, or error.

use leptos::prelude::*;

/// Probe status for display
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeStatus {
    /// No probe performed yet
    Idle,
    /// Probe in progress
    Loading,
    /// Probe succeeded
    Success { latency_ms: u64 },
    /// Probe failed
    Error { message: String },
}

/// Probe status indicator
#[component]
pub fn ProbeIndicator(
    /// Current probe status
    status: Signal<ProbeStatus>,
) -> impl IntoView {
    view! {
        <span class="probe-indicator">
            {move || match status.get() {
                ProbeStatus::Idle => view! { <span></span> }.into_any(),
                ProbeStatus::Loading => view! {
                    <span class="probe-loading" title="Testing connection...">
                        "..."
                    </span>
                }.into_any(),
                ProbeStatus::Success { latency_ms } => view! {
                    <span class="probe-success" title=format!("Connected ({}ms)", latency_ms)>
                        {format!("OK {}ms", latency_ms)}
                    </span>
                }.into_any(),
                ProbeStatus::Error { message } => view! {
                    <span class="probe-error" title=message.clone()>
                        "Failed"
                    </span>
                }.into_any(),
            }}
        </span>
    }
}
