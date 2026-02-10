use leptos::prelude::*;

#[component]
pub fn PoliciesView() -> impl IntoView {
    let content_filter = RwSignal::new(false);
    let filter_level = RwSignal::new("moderate".to_string());
    let log_conversations = RwSignal::new(true);
    let data_retention_days = RwSignal::new(90);
    let allow_analytics = RwSignal::new(false);

    // TODO: Load policies from Gateway

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-slate-950">
            <div class="max-w-2xl space-y-6">
                // Page Header
                <div>
                    <h1 class="text-2xl font-semibold text-slate-100 mb-1">
                        "Policies"
                    </h1>
                    <p class="text-sm text-slate-400">
                        "Configure content moderation and data policies"
                    </p>
                </div>

                // Content Safety Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-slate-200">"Content Safety"</h2>

                    <div class="p-4 bg-slate-900 border border-slate-800 rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-slate-200">"Content Filter"</div>
                                <div class="text-xs text-slate-400 mt-1">
                                    "Filter potentially harmful content"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || content_filter.get()
                                    on:change=move |ev| {
                                        content_filter.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-slate-700 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-indigo-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-indigo-600"></div>
                            </label>
                        </div>
                    </div>

                    {move || {
                        if content_filter.get() {
                            view! {
                                <div class="p-4 bg-slate-900 border border-slate-800 rounded">
                                    <div class="flex items-center justify-between">
                                        <div>
                                            <div class="text-sm font-medium text-slate-200">"Filter Level"</div>
                                            <div class="text-xs text-slate-400 mt-1">
                                                "Strictness of content filtering"
                                            </div>
                                        </div>
                                        <select
                                            class="px-3 py-1.5 bg-slate-800 border border-slate-700 rounded text-slate-200 text-sm"
                                            on:change=move |ev| filter_level.set(event_target_value(&ev))
                                        >
                                            <option value="strict" selected=move || filter_level.get() == "strict">"Strict"</option>
                                            <option value="moderate" selected=move || filter_level.get() == "moderate">"Moderate"</option>
                                            <option value="off" selected=move || filter_level.get() == "off">"Off"</option>
                                        </select>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="hidden"></div> }.into_any()
                        }
                    }}
                </div>

                // Data & Privacy Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-slate-200">"Data & Privacy"</h2>

                    <div class="p-4 bg-slate-900 border border-slate-800 rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-slate-200">"Log Conversations"</div>
                                <div class="text-xs text-slate-400 mt-1">
                                    "Save conversation history locally"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || log_conversations.get()
                                    on:change=move |ev| {
                                        log_conversations.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-slate-700 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-indigo-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-indigo-600"></div>
                            </label>
                        </div>
                    </div>

                    {move || {
                        if log_conversations.get() {
                            view! {
                                <div class="p-4 bg-slate-900 border border-slate-800 rounded">
                                    <div class="flex items-center justify-between">
                                        <div>
                                            <div class="text-sm font-medium text-slate-200">"Data Retention"</div>
                                            <div class="text-xs text-slate-400 mt-1">
                                                "Days to keep conversation logs"
                                            </div>
                                        </div>
                                        <div class="flex items-center gap-3 w-48">
                                            <input
                                                type="range"
                                                min="7"
                                                max="365"
                                                step="7"
                                                class="flex-1"
                                                value=move || data_retention_days.get()
                                                on:input=move |ev| {
                                                    if let Ok(val) = event_target_value(&ev).parse::<i32>() {
                                                        data_retention_days.set(val);
                                                    }
                                                }
                                            />
                                            <span class="text-xs text-slate-400 w-12 text-right font-mono">
                                                {move || format!("{}d", data_retention_days.get())}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="hidden"></div> }.into_any()
                        }
                    }}
                </div>

                // Analytics Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-slate-200">"Analytics"</h2>

                    <div class="p-4 bg-slate-900 border border-slate-800 rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-slate-200">"Allow Analytics"</div>
                                <div class="text-xs text-slate-400 mt-1">
                                    "Send anonymous usage data to improve Aleph"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || allow_analytics.get()
                                    on:change=move |ev| {
                                        allow_analytics.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-slate-700 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-indigo-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-indigo-600"></div>
                            </label>
                        </div>
                    </div>

                    {move || {
                        if allow_analytics.get() {
                            view! {
                                <div class="p-4 bg-blue-900/20 border border-blue-800 rounded">
                                    <span class="text-sm text-blue-300">
                                        "Analytics include: feature usage, performance metrics, and crash reports. No personal data, conversation content, or API keys are collected."
                                    </span>
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="hidden"></div> }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
