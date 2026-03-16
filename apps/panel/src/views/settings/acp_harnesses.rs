use leptos::prelude::*;

/// ACP Harnesses settings page — manages external CLI tools
#[component]
pub fn AcpHarnessesView() -> impl IntoView {
    view! {
        <div class="p-6">
            <h2 class="text-lg font-semibold text-text-primary">"ACP Agent CLI"</h2>
            <p class="text-sm text-text-secondary mt-1">"Manage external CLI tools that Aleph can delegate tasks to."</p>
        </div>
    }
}
