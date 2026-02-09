use leptos::prelude::*;
use tailwind_fuse::*;

#[derive(TwVariant, PartialEq)]
pub enum ButtonVariant {
    #[tw(default, class = "bg-indigo-600 text-white hover:bg-indigo-500 shadow-indigo-500/20 shadow-lg")]
    Primary,
    #[tw(class = "bg-slate-800 text-slate-200 hover:bg-slate-700 border border-slate-700")]
    Secondary,
    #[tw(class = "bg-transparent text-slate-400 hover:text-white hover:bg-slate-800")]
    Ghost,
    #[tw(class = "bg-red-950/30 text-red-500 hover:bg-red-900/40 border border-red-500/20")]
    Destructive,
}

#[derive(TwVariant, PartialEq)]
pub enum ButtonSize {
    #[tw(class = "h-8 px-3 text-xs")]
    Sm,
    #[tw(default, class = "h-10 px-4 text-sm")]
    Md,
    #[tw(class = "h-12 px-6 text-base")]
    Lg,
}

#[component]
pub fn Button(
    #[prop(into, optional)] variant: ButtonVariant,
    #[prop(into, optional)] size: ButtonSize,
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    let base_class = "inline-flex items-center justify-center rounded-xl font-medium transition-all active:scale-[0.98] disabled:opacity-50 disabled:pointer-events-none";
    
    view! {
        <button class=format!("{} {} {} {}", base_class, variant.as_class(), size.as_class(), class)>
            {children()}
        </button>
    }
}