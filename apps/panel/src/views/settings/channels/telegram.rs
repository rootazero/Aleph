use leptos::prelude::*;

/// Telegram channel configuration view
#[component]
pub fn TelegramChannelView() -> impl IntoView {
    let token = RwSignal::new(String::new());
    let _status = RwSignal::new("disconnected".to_string());

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Header
                <div>
                    <div class="flex items-center gap-3 mb-1">
                        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-[#26A5E4]">
                            <path d="M21.2 4.4L2.9 11.3c-1.2.5-1.2 1.2-.2 1.5l4.7 1.5 1.8 5.6c.2.6.1.8.7.8.4 0 .6-.2.9-.4l2.1-2.1 4.4 3.3c.8.4 1.4.2 1.6-.8L22.4 5.6c.3-1.2-.5-1.7-1.2-1.2zM8.5 13.5l9.4-5.9c.4-.3.8-.1.5.2l-7.8 7-.3 3.2-1.8-4.5z"/>
                        </svg>
                        <h1 class="text-2xl font-semibold text-text-primary">"Telegram"</h1>
                    </div>
                    <p class="text-sm text-text-secondary">
                        "Connect Aleph to Telegram via Bot API for messaging and automation"
                    </p>
                </div>

                // Connection Status
                <div class="p-4 bg-surface-raised border border-border rounded-xl">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-3">
                            <div class="w-10 h-10 rounded-full bg-surface-sunken flex items-center justify-center">
                                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-text-tertiary">
                                    <path d="M21.2 4.4L2.9 11.3c-1.2.5-1.2 1.2-.2 1.5l4.7 1.5 1.8 5.6c.2.6.1.8.7.8.4 0 .6-.2.9-.4l2.1-2.1 4.4 3.3c.8.4 1.4.2 1.6-.8L22.4 5.6c.3-1.2-.5-1.7-1.2-1.2z"/>
                                </svg>
                            </div>
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Bot not configured"</div>
                                <div class="text-xs text-text-tertiary">"Enter your Bot Token to get started"</div>
                            </div>
                        </div>
                        <span class="px-2 py-1 text-xs rounded-full bg-surface-sunken text-text-tertiary">
                            "Disconnected"
                        </span>
                    </div>
                </div>

                // Token Configuration
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Bot Token"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-3">
                        <p class="text-xs text-text-secondary">
                            "Get your Bot Token from @BotFather on Telegram"
                        </p>
                        <div class="flex gap-2">
                            <input
                                type="password"
                                class="flex-1 px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm font-mono"
                                placeholder="123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
                                prop:value=move || token.get()
                                on:input=move |ev| token.set(event_target_value(&ev))
                            />
                            <button class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm">
                                "Validate"
                            </button>
                        </div>
                    </div>
                </div>

                // Allowlists
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Message Filters"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-4">
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"User Allowlist"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "Only respond to messages from these Telegram user IDs (empty = respond to all)"
                            </div>
                            <input
                                type="text"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                placeholder="12345678, 87654321"
                            />
                        </div>
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"Group Allowlist"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "Only respond in these group chat IDs (empty = respond in all groups)"
                            </div>
                            <input
                                type="text"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                placeholder="-1001234567890, -1009876543210"
                            />
                        </div>
                    </div>
                </div>

                // Info
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded-xl">
                    <div class="flex items-start gap-2">
                        <span class="text-sm text-info">"i"</span>
                        <span class="text-sm text-info">
                            "Telegram uses long-polling to receive messages. The bot supports text, attachments, images, audio, video, replies, and typing indicators. Maximum message length is 4096 characters."
                        </span>
                    </div>
                </div>
            </div>
        </div>
    }
}
