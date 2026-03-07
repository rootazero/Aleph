use leptos::prelude::*;

#[component]
pub fn Card(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=format!("bg-surface-raised border border-border rounded-2xl {}", class)>
            {children()}
        </div>
    }
}

#[component]
pub fn CardHeader(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=format!("p-6 border-b border-border-subtle {}", class)>
            {children()}
        </div>
    }
}

#[component]
pub fn CardContent(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=format!("p-6 {}", class)>
            {children()}
        </div>
    }
}

#[component]
pub fn CardTitle(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <h3 class=format!("text-xl font-semibold tracking-tight text-text-primary {}", class)>
            {children()}
        </h3>
    }
}

#[component]
pub fn CardDescription(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <p class=format!("text-sm text-text-secondary mt-1 {}", class)>
            {children()}
        </p>
    }
}
