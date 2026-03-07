use leptos::prelude::*;
use tailwind_fuse::*;
use crate::components::sidebar::AlertLevel;

#[derive(TwVariant, PartialEq)]
pub enum BadgeVariant {
    #[tw(default, class = "bg-primary-subtle text-primary border-primary/20")]
    Indigo,
    #[tw(class = "bg-success-subtle text-success border-success/20")]
    Emerald,
    #[tw(class = "bg-warning-subtle text-warning border-warning/20")]
    Amber,
    #[tw(class = "bg-danger-subtle text-danger border-danger/20")]
    Red,
    #[tw(class = "bg-surface-sunken text-text-secondary border-border")]
    Slate,
}

#[component]
pub fn Badge(
    #[prop(into, optional)] variant: BadgeVariant,
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    let base_class = "inline-flex items-center px-2 py-0.5 rounded text-[10px] font-bold uppercase tracking-widest border transition-all";
    
    view! {
        <span class=format!("{} {} {}", base_class, variant.as_class(), class)>
            {children()}
        </span>
    }
}

#[component]
pub fn StatusBadge(
    level: AlertLevel,
    #[prop(optional)] count: Option<u32>,
) -> impl IntoView {
    let (bg_class, animation_class) = match level {
        AlertLevel::None => return view! {}.into_any(),
        AlertLevel::Info => ("bg-info", ""),
        AlertLevel::Warning => ("bg-warning", ""),
        AlertLevel::Critical => ("bg-danger", "animate-pulse"),
    };

    view! {
        <div class=format!(
            "absolute -top-1 -right-1 {} {} rounded-full min-w-[16px] h-4 flex items-center justify-center text-[10px] font-bold text-white px-1",
            bg_class, animation_class
        )>
            {count.map(|c| c.to_string()).unwrap_or_default()}
        </div>
    }.into_any()
}