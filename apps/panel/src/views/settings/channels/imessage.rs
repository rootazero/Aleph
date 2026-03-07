use leptos::prelude::*;

/// iMessage channel configuration view
#[component]
pub fn IMessageChannelView() -> impl IntoView {
    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Header
                <div>
                    <div class="flex items-center gap-3 mb-1">
                        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-[#34C759]">
                            <path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z"/>
                        </svg>
                        <h1 class="text-2xl font-semibold text-text-primary">"iMessage"</h1>
                    </div>
                    <p class="text-sm text-text-secondary">
                        "Connect Aleph to macOS iMessage for direct messaging via the native Messages app"
                    </p>
                </div>

                // Connection Status
                <div class="p-4 bg-surface-raised border border-border rounded-xl">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-3">
                            <div class="w-10 h-10 rounded-full bg-surface-sunken flex items-center justify-center">
                                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-text-tertiary">
                                    <path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z"/>
                                </svg>
                            </div>
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Not running"</div>
                                <div class="text-xs text-text-tertiary">"macOS only - requires Full Disk Access"</div>
                            </div>
                        </div>
                        <span class="px-2 py-1 text-xs rounded-full bg-surface-sunken text-text-tertiary">
                            "Disconnected"
                        </span>
                    </div>
                </div>

                // Requirements
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"System Requirements"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-3">
                        <div class="flex items-center gap-3">
                            <div class="w-6 h-6 rounded-full bg-surface-sunken flex items-center justify-center flex-shrink-0">
                                <span class="text-xs text-text-tertiary">"?"</span>
                            </div>
                            <div>
                                <div class="text-sm font-medium text-text-primary">"macOS Platform"</div>
                                <div class="text-xs text-text-secondary">"iMessage integration is only available on macOS"</div>
                            </div>
                        </div>
                        <div class="flex items-center gap-3">
                            <div class="w-6 h-6 rounded-full bg-surface-sunken flex items-center justify-center flex-shrink-0">
                                <span class="text-xs text-text-tertiary">"?"</span>
                            </div>
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Full Disk Access"</div>
                                <div class="text-xs text-text-secondary">"Required to read the Messages SQLite database at ~/Library/Messages/chat.db"</div>
                            </div>
                        </div>
                        <div class="flex items-center gap-3">
                            <div class="w-6 h-6 rounded-full bg-surface-sunken flex items-center justify-center flex-shrink-0">
                                <span class="text-xs text-text-tertiary">"?"</span>
                            </div>
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Automation Permission"</div>
                                <div class="text-xs text-text-secondary">"Required for AppleScript-based message sending via Messages.app"</div>
                            </div>
                        </div>
                    </div>
                </div>

                // Settings
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Settings"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-4">
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"Database Path"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "Path to the Messages SQLite database"
                            </div>
                            <input
                                type="text"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm font-mono"
                                placeholder="~/Library/Messages/chat.db"
                                value="~/Library/Messages/chat.db"
                                readonly=true
                            />
                        </div>
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"Polling Interval"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "How often to check for new messages (in seconds)"
                            </div>
                            <input
                                type="number"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                placeholder="2"
                                value="2"
                                min="1"
                                max="30"
                            />
                        </div>
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"Contact Allowlist"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "Only respond to messages from these contacts (empty = respond to all)"
                            </div>
                            <input
                                type="text"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                placeholder="+1234567890, user@icloud.com"
                            />
                        </div>
                    </div>
                </div>

                // Info
                <div class="p-4 bg-primary-subtle border border-primary/20 rounded-xl">
                    <div class="flex items-start gap-2">
                        <span class="text-sm text-info">"i"</span>
                        <span class="text-sm text-info">
                            "iMessage integration reads messages by polling the local Messages database and sends messages via AppleScript. Supports text, attachments (images, audio, video), and reactions. Maximum attachment size is 100 MB."
                        </span>
                    </div>
                </div>
            </div>
        </div>
    }
}
