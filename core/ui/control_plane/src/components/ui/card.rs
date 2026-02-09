use leptos::prelude::*;

#[component]
pub fn Card(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=format!("bg-slate-900/40 border border-slate-800 rounded-3xl backdrop-blur-sm shadow-glass {}", class)>
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
        <div class=format!("p-6 border-b border-slate-800/50 {}", class)>
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
        <h3 class=format!("text-xl font-semibold tracking-tight text-slate-100 {}", class)>
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
        <p class=format!("text-sm text-slate-400 mt-1 {}", class)>
            {children()}
        </p>
    }
}
