use leptos::prelude::*;
use tailwind_fuse::*;
use crate::components::sidebar::AlertLevel;

#[derive(TwVariant, PartialEq)]
pub enum BadgeVariant {
    #[tw(default, class = "bg-indigo-500/10 text-indigo-400 border-indigo-500/20")]
    Indigo,
    #[tw(class = "bg-emerald-500/10 text-emerald-400 border-emerald-500/20")]
    Emerald,
    #[tw(class = "bg-amber-500/10 text-amber-400 border-amber-500/20")]
    Amber,
    #[tw(class = "bg-red-500/10 text-red-400 border-red-500/20")]
    Red,
    #[tw(class = "bg-slate-800 text-slate-400 border-slate-700")]
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
        AlertLevel::Info => ("bg-blue-500", ""),
        AlertLevel::Warning => ("bg-yellow-500", ""),
        AlertLevel::Critical => ("bg-red-500", "animate-pulse"),
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