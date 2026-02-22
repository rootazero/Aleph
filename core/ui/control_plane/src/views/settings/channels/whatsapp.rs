use leptos::prelude::*;

/// WhatsApp channel configuration view
#[component]
pub fn WhatsAppChannelView() -> impl IntoView {
    let _pairing_state = RwSignal::new("idle".to_string());

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Header
                <div>
                    <div class="flex items-center gap-3 mb-1">
                        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-[#25D366]">
                            <path d="M17.47 14.38c-.29-.14-1.7-.84-1.96-.94-.27-.1-.46-.14-.65.14-.2.29-.75.94-.92 1.13-.17.2-.34.22-.63.07-.29-.14-1.22-.45-2.32-1.43-.86-.77-1.44-1.71-1.61-2-.17-.29-.02-.45.13-.59.13-.13.29-.34.44-.51.14-.17.2-.29.29-.48.1-.2.05-.37-.02-.51-.07-.15-.65-1.56-.89-2.14-.24-.56-.48-.49-.65-.49-.17 0-.37-.02-.56-.02-.2 0-.51.07-.78.37-.27.29-1.02 1-1.02 2.43 0 1.43 1.04 2.82 1.19 3.01.14.2 2.05 3.13 4.97 4.39.7.3 1.24.48 1.66.61.7.22 1.33.19 1.83.12.56-.08 1.7-.7 1.94-1.37.24-.68.24-1.26.17-1.38-.07-.12-.27-.2-.56-.34zM12 2C6.48 2 2 6.48 2 12c0 1.77.46 3.43 1.27 4.88L2 22l5.23-1.37A9.93 9.93 0 0 0 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>
                        </svg>
                        <h1 class="text-2xl font-semibold text-text-primary">"WhatsApp"</h1>
                    </div>
                    <p class="text-sm text-text-secondary">
                        "Connect Aleph to WhatsApp via multi-device bridge for personal and group messaging"
                    </p>
                </div>

                // Connection Status
                <div class="p-4 bg-surface-raised border border-border rounded-xl">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-3">
                            <div class="w-10 h-10 rounded-full bg-surface-sunken flex items-center justify-center">
                                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-text-tertiary">
                                    <path d="M12 2C6.48 2 2 6.48 2 12c0 1.77.46 3.43 1.27 4.88L2 22l5.23-1.37A9.93 9.93 0 0 0 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>
                                </svg>
                            </div>
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Not paired"</div>
                                <div class="text-xs text-text-tertiary">"Scan QR code with WhatsApp to pair"</div>
                            </div>
                        </div>
                        <span class="px-2 py-1 text-xs rounded-full bg-surface-sunken text-text-tertiary">
                            "Disconnected"
                        </span>
                    </div>
                </div>

                // QR Pairing
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Device Pairing"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl">
                        <div class="text-center py-8">
                            <div class="w-48 h-48 mx-auto bg-surface-sunken border border-border rounded-lg flex items-center justify-center mb-4">
                                <span class="text-text-tertiary text-sm">"QR Code will appear here"</span>
                            </div>
                            <button class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover text-sm">
                                "Generate QR Code"
                            </button>
                            <p class="text-xs text-text-tertiary mt-3">
                                "Open WhatsApp > Settings > Linked Devices > Link a Device"
                            </p>
                        </div>
                    </div>
                </div>

                // Chat Filters
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Chat Filters"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-4">
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"Chat Allowlist"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "Only respond in these chat JIDs (empty = respond to all)"
                            </div>
                            <input
                                type="text"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                placeholder="1234567890@s.whatsapp.net"
                            />
                        </div>
                        <div>
                            <div class="text-sm font-medium text-text-primary mb-1">"Group Allowlist"</div>
                            <div class="text-xs text-text-secondary mb-2">
                                "Only respond in these group JIDs (empty = respond in all groups)"
                            </div>
                            <input
                                type="text"
                                class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                placeholder="120363012345678901@g.us"
                            />
                        </div>
                    </div>
                </div>

                // Bridge Settings
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Bridge Settings"</h2>
                    <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-3">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Send Read Receipts"</div>
                                <div class="text-xs text-text-secondary mt-1">
                                    "Mark messages as read when processed by Aleph"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input type="checkbox" class="sr-only peer" />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>
                    </div>
                </div>

                // Info
                <div class="p-4 bg-warning-subtle border border-warning/20 rounded-xl">
                    <div class="flex items-start gap-2">
                        <span class="text-sm text-warning">"!"</span>
                        <span class="text-sm text-warning">
                            "WhatsApp integration uses a Go bridge process for multi-device pairing. The bridge must be running for WhatsApp connectivity. QR code pairing is required for first-time setup."
                        </span>
                    </div>
                </div>
            </div>
        </div>
    }
}
