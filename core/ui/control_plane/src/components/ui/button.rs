use leptos::prelude::*;
use tailwind_fuse::*;

#[derive(TwVariant, PartialEq)]
pub enum ButtonVariant {
    #[tw(default, class = "bg-primary text-text-inverse hover:bg-primary-hover")]
    Primary,
    #[tw(class = "bg-surface-sunken text-text-primary border border-border hover:bg-surface-raised")]
    Secondary,
    #[tw(class = "bg-transparent text-text-secondary hover:text-text-primary hover:bg-surface-sunken")]
    Ghost,
    #[tw(class = "bg-danger text-text-inverse hover:brightness-95")]
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
    #[prop(into, optional)] disabled: MaybeSignal<bool>,
    children: Children,
) -> impl IntoView {
    let base_class = "inline-flex items-center justify-center rounded-xl font-medium transition-all active:scale-[0.98] disabled:opacity-50 disabled:pointer-events-none";

    view! {
        <button
            class=format!("{} {} {} {}", base_class, variant.as_class(), size.as_class(), class)
            disabled=move || disabled.get()
        >
            {children()}
        </button>
    }
}