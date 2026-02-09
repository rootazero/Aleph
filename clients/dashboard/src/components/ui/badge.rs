use leptos::prelude::*;
use tailwind_fuse::*;

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