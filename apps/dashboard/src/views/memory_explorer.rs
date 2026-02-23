//! Memory Explorer view
//!
//! Displays memory statistics, search functionality, and fact browsing.
//! Simulates memory search with mock data.

use leptos::*;
use crate::models::MemorySearchItem;
use crate::mock_data::{generate_mock_memory_stats, generate_mock_memory_search};

#[component]
pub fn MemoryExplorer() -> impl IntoView {
    // Memory statistics
    let (stats, set_stats) = create_signal(generate_mock_memory_stats());

    // Search query
    let (search_query, set_search_query) = create_signal(String::new());

    // Search results
    let (search_results, set_search_results) = create_signal(Vec::<MemorySearchItem>::new());

    // Loading state
    let (is_searching, set_is_searching) = create_signal(false);

    // Handle search
    let do_search = move || {
        let query = search_query.get();
        if query.is_empty() {
            set_search_results.set(Vec::new());
            return;
        }

        log::info!("Searching for: {}", query);
        set_is_searching.set(true);

        // Simulate search delay
        set_timeout(
            move || {
                let results = generate_mock_memory_search();
                set_search_results.set(results);
                set_is_searching.set(false);
                log::info!("Search completed");
            },
            std::time::Duration::from_millis(500),
        );
    };

    let on_search_click = move |_| {
        do_search();
    };

    let on_search_keypress = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" {
            do_search();
        }
    };

    // Format timestamp
    let format_timestamp = |timestamp: f64| -> String {
        let now = js_sys::Date::now();
        let diff_ms = now - timestamp;
        let diff_seconds = (diff_ms / 1000.0) as i64;

        if diff_seconds < 60 {
            format!("{} seconds ago", diff_seconds)
        } else if diff_seconds < 3600 {
            format!("{} minutes ago", diff_seconds / 60)
        } else if diff_seconds < 86400 {
            format!("{} hours ago", diff_seconds / 3600)
        } else {
            format!("{} days ago", diff_seconds / 86400)
        }
    };

    // Format bytes
    let format_bytes = |bytes: u64| -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.2} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    };

    view! {
        <div class="space-y-6">
            // Memory Statistics
            <div class="card">
                <h2 class="card-header">"Memory Statistics"</h2>
                <div class="grid grid-cols-3 gap-4">
                    <div class="bg-gray-700 rounded p-4">
                        <div class="text-sm text-gray-400">"Total Facts"</div>
                        <div class="text-2xl font-bold text-white">
                            {move || stats.get().count.to_string()}
                        </div>
                    </div>
                    <div class="bg-gray-700 rounded p-4">
                        <div class="text-sm text-gray-400">"Storage Size"</div>
                        <div class="text-2xl font-bold text-blue-400">
                            {move || format_bytes(stats.get().size_bytes)}
                        </div>
                    </div>
                    <div class="bg-gray-700 rounded p-4">
                        <div class="text-sm text-gray-400">"Apps Tracked"</div>
                        <div class="text-2xl font-bold text-green-400">
                            {move || stats.get().apps_count.to_string()}
                        </div>
                    </div>
                </div>
            </div>

            // Search Interface
            <div class="card">
                <h2 class="card-header">"Search Memory"</h2>
                <div class="space-y-4">
                    <div class="flex space-x-4">
                        <input
                            type="text"
                            class="flex-1 bg-gray-700 border border-gray-600 rounded-md px-4 py-2 text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
                            prop:value=move || search_query.get()
                            on:input=move |ev| {
                                set_search_query.set(event_target_value(&ev));
                            }
                            on:keypress=on_search_keypress
                            placeholder="Search for facts..."
                        />
                        <button
                            class="px-6 py-2 bg-blue-600 hover:bg-blue-700 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                            on:click=on_search_click
                            disabled=move || is_searching.get()
                        >
                            {move || if is_searching.get() { "Searching..." } else { "Search" }}
                        </button>
                    </div>

                    // Search Results
                    {move || {
                        let results = search_results.get();
                        if results.is_empty() && !search_query.get().is_empty() && !is_searching.get() {
                            view! {
                                <div class="text-center py-8 text-gray-400">
                                    "No results found"
                                </div>
                            }.into_view()
                        } else if !results.is_empty() {
                            view! {
                                <div class="space-y-3">
                                    <div class="text-sm text-gray-400">
                                        {format!("{} results found", results.len())}
                                    </div>
                                    {results.into_iter().map(|item| {
                                        let timestamp_str = format_timestamp(item.timestamp);
                                        let score_percent = (item.score * 100.0) as u32;
                                        view! {
                                            <div class="bg-gray-700 rounded-lg p-4 hover:bg-gray-600 transition-colors">
                                                <div class="flex items-start justify-between">
                                                    <div class="flex-1">
                                                        <div class="text-white font-medium mb-2">
                                                            {item.content}
                                                        </div>
                                                        <div class="flex items-center space-x-4 text-sm text-gray-400">
                                                            <span>{timestamp_str}</span>
                                                            <span class="text-gray-500">"•"</span>
                                                            <span>{format!("ID: {}", item.id)}</span>
                                                        </div>
                                                    </div>
                                                    <div class="ml-4">
                                                        <div class="text-xs text-gray-400 mb-1">"Relevance"</div>
                                                        <div class="text-lg font-bold text-green-400">
                                                            {format!("{}%", score_percent)}
                                                        </div>
                                                    </div>
                                                </div>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="text-center py-8 text-gray-400">
                                    "Enter a search query to find facts"
                                </div>
                            }.into_view()
                        }
                    }}

                    <div class="bg-blue-900/20 border border-blue-500 rounded p-3 text-sm text-blue-300">
                        <strong>"Simulation: "</strong> "Search returns mock data with relevance scores. Real implementation will use vector search + BM25 hybrid retrieval."
                    </div>
                </div>
            </div>

            // Recent Facts
            <div class="card">
                <h2 class="card-header">"Recent Facts"</h2>
                <div class="space-y-3">
                    {generate_mock_memory_search().into_iter().map(|item| {
                        let timestamp_str = format_timestamp(item.timestamp);
                        view! {
                            <div class="bg-gray-700 rounded-lg p-4 hover:bg-gray-600 transition-colors cursor-pointer">
                                <div class="text-white font-medium mb-2">
                                    {item.content}
                                </div>
                                <div class="flex items-center space-x-4 text-sm text-gray-400">
                                    <span>{timestamp_str}</span>
                                    <span class="text-gray-500">"•"</span>
                                    <span>{format!("ID: {}", item.id)}</span>
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}
