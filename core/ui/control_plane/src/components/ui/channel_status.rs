use leptos::prelude::*;

/// Five-state model for channel connection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error,
    Disabled,
}

impl ChannelStatus {
    /// Parse a status string (case-insensitive).
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "connected" => Self::Connected,
            "connecting" => Self::Connecting,
            "error" => Self::Error,
            "disabled" => Self::Disabled,
            _ => Self::Disconnected,
        }
    }

    /// Human-readable label for the status.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Error => "Error",
            Self::Disabled => "Disabled",
        }
    }

    /// Tailwind classes for the colored dot indicator.
    pub fn dot_class(&self) -> &'static str {
        match self {
            Self::Disconnected => "bg-text-tertiary",
            Self::Connecting => "bg-warning animate-pulse",
            Self::Connected => "bg-success",
            Self::Error => "bg-danger",
            Self::Disabled => "bg-border",
        }
    }

    /// Tailwind classes for the status label text color.
    pub fn text_class(&self) -> &'static str {
        match self {
            Self::Disconnected => "text-text-tertiary",
            Self::Connecting => "text-warning",
            Self::Connected => "text-success",
            Self::Error => "text-danger",
            Self::Disabled => "text-text-tertiary",
        }
    }

    /// Tailwind classes for the pill background + text color.
    pub fn pill_class(&self) -> &'static str {
        match self {
            Self::Connected => "bg-success-subtle text-success",
            Self::Connecting => "bg-warning-subtle text-warning",
            Self::Error => "bg-danger-subtle text-danger",
            _ => "bg-surface-sunken text-text-tertiary",
        }
    }
}

/// Inline dot + label badge for channel connection status.
///
/// Renders a small colored dot followed by the status label text.
#[component]
pub fn ChannelStatusBadge(
    status: Signal<ChannelStatus>,
) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1.5">
            <span class=move || {
                format!("w-2 h-2 rounded-full {}", status.get().dot_class())
            }></span>
            <span class=move || {
                format!("text-xs {}", status.get().text_class())
            }>
                {move || status.get().label()}
            </span>
        </span>
    }
}

/// Pill-shaped badge for channel status, suitable for cards and lists.
///
/// Renders a rounded pill with status-colored background and label.
#[component]
pub fn ChannelStatusPill(
    status: Signal<ChannelStatus>,
) -> impl IntoView {
    view! {
        <span class=move || {
            format!(
                "px-2 py-0.5 rounded-full text-xs font-medium {}",
                status.get().pill_class()
            )
        }>
            {move || status.get().label()}
        </span>
    }
}
