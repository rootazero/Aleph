use leptos::prelude::*;
use leptos::task::spawn_local;
use gloo_timers::future::TimeoutFuture;
use crate::context::DashboardState;
use crate::models::{ChannelInfo, PairingData, PairingState};

#[component]
pub fn SocialConnections() -> impl IntoView {
    let (active_tab, set_active_tab) = signal("whatsapp".to_string());
    let state = use_context::<DashboardState>().expect("DashboardState not found");

    view! {
        <div class="p-6">
            <h1 class="text-2xl font-bold mb-6 text-slate-100">"Social Connections"</h1>

            // Tabs
            <div class="flex space-x-4 mb-8 border-b border-slate-700">
                <button
                    class=move || {
                        let active = active_tab.get() == "whatsapp";
                        let base = "pb-2 px-4 transition-colors duration-200 ";
                        if active { format!("{} border-b-2 border-indigo-500 text-indigo-400 font-medium", base) }
                        else { format!("{} text-slate-400 hover:text-slate-200", base) }
                    }
                    on:click=move |_| set_active_tab.set("whatsapp".to_string())
                >
                    "WhatsApp"
                </button>
                <button
                    class=move || {
                        let active = active_tab.get() == "telegram";
                        let base = "pb-2 px-4 transition-colors duration-200 ";
                        if active { format!("{} border-b-2 border-indigo-500 text-indigo-400 font-medium", base) }
                        else { format!("{} text-slate-400 hover:text-slate-200", base) }
                    }
                    on:click=move |_| set_active_tab.set("telegram".to_string())
                >
                    "Telegram"
                </button>
                <button
                    class=move || {
                        let active = active_tab.get() == "discord";
                        let base = "pb-2 px-4 transition-colors duration-200 ";
                        if active { format!("{} border-b-2 border-indigo-500 text-indigo-400 font-medium", base) }
                        else { format!("{} text-slate-400 hover:text-slate-200", base) }
                    }
                    on:click=move |_| set_active_tab.set("discord".to_string())
                >
                    "Discord"
                </button>
            </div>

            // Tab Content
            <div class="mt-4">
                {move || match active_tab.get().as_str() {
                    "whatsapp" => view! { <WhatsAppPanel state=state.clone() /> }.into_any(),
                    "telegram" => view! { <TelegramPanel state=state.clone() /> }.into_any(),
                    "discord" => view! { <DiscordPanel state=state.clone() /> }.into_any(),
                    _ => view! { <div>"Select a tab"</div> }.into_any(),
                }}
            </div>
        </div>
    }
}

/// WhatsApp SVG icon (reused across states)
fn whatsapp_icon() -> impl IntoView {
    view! {
        <svg class="w-10 h-10 text-white" fill="currentColor" viewBox="0 0 24 24">
            <path d="M17.472 14.382c-.297-.149-1.758-.867-2.03-.967-.273-.099-.471-.148-.67.15-.197.297-.767.966-.94 1.164-.173.199-.347.223-.644.075-.297-.15-1.255-.463-2.39-1.475-.883-.788-1.48-1.761-1.653-2.059-.173-.297-.018-.458.13-.606.134-.133.298-.347.446-.52.149-.174.198-.298.298-.497.099-.198.05-.371-.025-.52-.075-.149-.669-1.612-.916-2.207-.242-.579-.487-.5-.669-.51-.173-.008-.371-.01-.57-.01-.198 0-.52.074-.792.372-.272.297-1.04 1.016-1.04 2.479 0 1.462 1.065 2.875 1.213 3.074.149.198 2.096 3.2 5.077 4.487.709.306 1.262.489 1.694.625.712.227 1.36.195 1.871.118.571-.085 1.758-.719 2.006-1.413.248-.694.248-1.289.173-1.413-.074-.124-.272-.198-.57-.347m-5.421 7.403h-.004a9.87 9.87 0 01-5.031-1.378l-.361-.214-3.741.982.998-3.648-.235-.374a9.86 9.86 0 01-1.51-5.26c.001-5.45 4.436-9.884 9.888-9.884 2.64 0 5.122 1.03 6.988 2.898a9.825 9.825 0 012.893 6.994c-.003 5.45-4.437 9.884-9.885 9.884m8.413-18.297A11.815 11.815 0 0012.05 0C5.495 0 .16 5.335.157 11.892c0 2.096.547 4.142 1.588 5.945L.057 24l6.305-1.654a11.882 11.882 0 005.683 1.448h.005c6.554 0 11.89-5.335 11.893-11.893a11.821 11.821 0 00-3.48-8.413Z"/>
        </svg>
    }
}

/// Mask middle digits of a phone number for privacy display
fn mask_phone(phone: &str) -> String {
    let digits: Vec<char> = phone.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
    let len = digits.len();
    if len <= 6 {
        return phone.to_string();
    }
    let prefix_len = 3.min(len);
    let suffix_len = 3.min(len - prefix_len);
    let mask_len = len - prefix_len - suffix_len;
    let prefix: String = digits[..prefix_len].iter().collect();
    let suffix: String = digits[len - suffix_len..].iter().collect();
    let mask = "*".repeat(mask_len);
    format!("{}{}{}", prefix, mask, suffix)
}

#[component]
fn WhatsAppPanel(state: DashboardState) -> impl IntoView {
    let (pairing_state, set_pairing_state) = signal(PairingState::Idle);
    let (polling_active, set_polling_active) = signal(false);

    // Fetch initial status on mount
    let state_init = state;
    Effect::new(move |_| {
        let state = state_init;
        spawn_local(async move {
            let result = state.rpc_call(
                "channels.status",
                serde_json::json!({"channel_id": "whatsapp"}),
            ).await;
            if let Ok(val) = result {
                // Try to parse as PairingState first, then fall back to ChannelInfo
                if let Ok(ps) = serde_json::from_value::<PairingState>(val.clone()) {
                    set_pairing_state.set(ps);
                } else if let Ok(info) = serde_json::from_value::<ChannelInfo>(val) {
                    match info.status.as_str() {
                        "connected" => set_pairing_state.set(PairingState::Connected {
                            device_name: "Unknown".to_string(),
                            phone_number: "Unknown".to_string(),
                        }),
                        "disconnected" => set_pairing_state.set(PairingState::Idle),
                        _ => {}
                    }
                }
            }
        });
    });

    // Start WhatsApp connection: call channel.start, then poll pairing_data
    let state_start = state;
    let start_connection = move || {
        set_pairing_state.set(PairingState::Initializing);
        let state = state_start;
        spawn_local(async move {
            let result = state.rpc_call(
                "channel.start",
                serde_json::json!({"channel_id": "whatsapp"}),
            ).await;
            match result {
                Ok(_) => {
                    set_polling_active.set(true);
                }
                Err(e) => {
                    set_pairing_state.set(PairingState::Failed { error: e });
                }
            }
        });
    };

    // Polling effect: when polling_active is true, periodically fetch pairing data
    let state_poll = state;
    Effect::new(move |_| {
        if !polling_active.get() {
            return;
        }
        let state = state_poll;
        spawn_local(async move {
            // Poll up to 120 times (roughly 2 minutes at 1s intervals)
            for _ in 0..120 {
                if !polling_active.get_untracked() {
                    break;
                }
                let result = state.rpc_call(
                    "channel.pairing_data",
                    serde_json::json!({"channel_id": "whatsapp"}),
                ).await;
                if let Ok(val) = result {
                    if let Ok(ps) = serde_json::from_value::<PairingState>(val.clone()) {
                        let should_stop = matches!(
                            &ps,
                            PairingState::Connected { .. }
                            | PairingState::Failed { .. }
                            | PairingState::Disconnected { .. }
                        );
                        set_pairing_state.set(ps);
                        if should_stop {
                            set_polling_active.set(false);
                            break;
                        }
                    } else if let Ok(pairing) = serde_json::from_value::<PairingData>(val) {
                        match pairing {
                            PairingData::QrCode(data) => {
                                set_pairing_state.set(PairingState::WaitingQr {
                                    qr_data: data,
                                    expires_at: String::new(),
                                });
                            }
                            PairingData::Code(_) | PairingData::None => {}
                        }
                    }
                }
                TimeoutFuture::new(1000).await;
            }
        });
    });

    // Disconnect action
    let state_disconnect = state;
    let disconnect = move || {
        let state = state_disconnect;
        spawn_local(async move {
            let _ = state.rpc_call(
                "channel.stop",
                serde_json::json!({"channel_id": "whatsapp"}),
            ).await;
            set_pairing_state.set(PairingState::Idle);
            set_polling_active.set(false);
        });
    };

    // Re-pair action (disconnect then start fresh)
    let start_connection_replair = start_connection.clone();
    let replair = move || {
        let state = state_disconnect;
        spawn_local(async move {
            let _ = state.rpc_call(
                "channel.stop",
                serde_json::json!({"channel_id": "whatsapp"}),
            ).await;
        });
        set_polling_active.set(false);
        start_connection_replair();
    };

    view! {
        <div class="bg-slate-800 rounded-lg p-8 max-w-2xl border border-slate-700">
            {move || {
                let ps = pairing_state.get();
                match ps {
                    // ── Idle: Show "Connect WhatsApp" button ──
                    PairingState::Idle => {
                        let start = start_connection.clone();
                        view! {
                            <div class="flex flex-col items-center">
                                <div class="w-20 h-20 bg-green-500 rounded-full flex items-center justify-center mb-6">
                                    {whatsapp_icon()}
                                </div>
                                <h2 class="text-xl font-semibold text-slate-100 mb-2">"WhatsApp"</h2>
                                <p class="text-slate-400 text-center mb-8">"Link your WhatsApp account to Aleph"</p>
                                <button
                                    on:click=move |_| start()
                                    class="bg-green-600 hover:bg-green-700 text-white font-medium px-8 py-3 rounded-lg transition-colors flex items-center gap-3"
                                >
                                    <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24">
                                        <path d="M17.472 14.382c-.297-.149-1.758-.867-2.03-.967-.273-.099-.471-.148-.67.15-.197.297-.767.966-.94 1.164-.173.199-.347.223-.644.075-.297-.15-1.255-.463-2.39-1.475-.883-.788-1.48-1.761-1.653-2.059-.173-.297-.018-.458.13-.606.134-.133.298-.347.446-.52.149-.174.198-.298.298-.497.099-.198.05-.371-.025-.52-.075-.149-.669-1.612-.916-2.207-.242-.579-.487-.5-.669-.51-.173-.008-.371-.01-.57-.01-.198 0-.52.074-.792.372-.272.297-1.04 1.016-1.04 2.479 0 1.462 1.065 2.875 1.213 3.074.149.198 2.096 3.2 5.077 4.487.709.306 1.262.489 1.694.625.712.227 1.36.195 1.871.118.571-.085 1.758-.719 2.006-1.413.248-.694.248-1.289.173-1.413-.074-.124-.272-.198-.57-.347m-5.421 7.403h-.004a9.87 9.87 0 01-5.031-1.378l-.361-.214-3.741.982.998-3.648-.235-.374a9.86 9.86 0 01-1.51-5.26c.001-5.45 4.436-9.884 9.888-9.884 2.64 0 5.122 1.03 6.988 2.898a9.825 9.825 0 012.893 6.994c-.003 5.45-4.437 9.884-9.885 9.884m8.413-18.297A11.815 11.815 0 0012.05 0C5.495 0 .16 5.335.157 11.892c0 2.096.547 4.142 1.588 5.945L.057 24l6.305-1.654a11.882 11.882 0 005.683 1.448h.005c6.554 0 11.89-5.335 11.893-11.893a11.821 11.821 0 00-3.48-8.413Z"/>
                                    </svg>
                                    "Connect WhatsApp"
                                </button>
                            </div>
                        }.into_any()
                    }

                    // ── Initializing: Show spinner ──
                    PairingState::Initializing => {
                        view! {
                            <div class="flex flex-col items-center py-12">
                                <div class="w-12 h-12 border-4 border-slate-600 border-t-green-500 rounded-full animate-spin mb-6"></div>
                                <p class="text-slate-300 text-lg font-medium">"Starting bridge..."</p>
                                <p class="text-slate-500 text-sm mt-2">"Initializing WhatsApp connection"</p>
                            </div>
                        }.into_any()
                    }

                    // ── WaitingQr: Show QR code + countdown + pairing code alternative ──
                    PairingState::WaitingQr { qr_data, expires_at } => {
                        let expires_display = if expires_at.is_empty() {
                            "Waiting for scan...".to_string()
                        } else {
                            format!("Expires at: {}", expires_at)
                        };
                        view! {
                            <div class="flex flex-col items-center">
                                <h2 class="text-xl font-semibold text-slate-100 mb-2">"Scan QR Code"</h2>
                                <p class="text-slate-400 text-sm mb-6">"Open WhatsApp on your phone > Settings > Linked Devices > Link a Device"</p>

                                // QR Code display
                                <div class="bg-white p-4 rounded-xl shadow-xl mb-4">
                                    <img src=qr_data class="w-64 h-64" alt="WhatsApp QR Code" />
                                </div>

                                // Expiry countdown
                                <p class="text-slate-500 text-sm mb-6">{expires_display}</p>

                                // Alternative: pairing code
                                <div class="w-full border-t border-slate-700 pt-6 mt-2">
                                    <p class="text-slate-500 text-sm text-center">
                                        "Alternatively, use a "
                                        <span class="text-indigo-400 font-medium">"pairing code"</span>
                                        " from your phone's Linked Devices menu."
                                    </p>
                                </div>
                            </div>
                        }.into_any()
                    }

                    // ── QrExpired: Grayed-out QR placeholder ──
                    PairingState::QrExpired => {
                        view! {
                            <div class="flex flex-col items-center py-8">
                                <div class="w-64 h-64 bg-slate-700/50 rounded-xl flex items-center justify-center mb-6 border-2 border-dashed border-slate-600">
                                    <div class="text-center">
                                        <div class="w-10 h-10 border-4 border-slate-600 border-t-indigo-500 rounded-full animate-spin mx-auto mb-3"></div>
                                        <p class="text-slate-400 font-medium">"Refreshing..."</p>
                                    </div>
                                </div>
                                <p class="text-slate-500 text-sm">"QR code expired. Generating a new one..."</p>
                            </div>
                        }.into_any()
                    }

                    // ── Scanned: Success checkmark ──
                    PairingState::Scanned => {
                        view! {
                            <div class="flex flex-col items-center py-12">
                                <div class="w-16 h-16 bg-green-500/20 rounded-full flex items-center justify-center mb-6 ring-4 ring-green-500/30">
                                    <svg class="w-10 h-10 text-green-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5">
                                        <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
                                    </svg>
                                </div>
                                <p class="text-slate-100 text-lg font-medium">"Scanned!"</p>
                                <p class="text-slate-400 text-sm mt-2">"Waiting for confirmation..."</p>
                                <div class="mt-4 flex items-center gap-2">
                                    <div class="w-2 h-2 bg-green-500 rounded-full animate-pulse"></div>
                                    <span class="text-slate-500 text-xs">"Processing"</span>
                                </div>
                            </div>
                        }.into_any()
                    }

                    // ── Syncing: Progress bar ──
                    PairingState::Syncing { progress } => {
                        let pct = (progress * 100.0).round() as u32;
                        let width_style = format!("width: {}%", pct);
                        view! {
                            <div class="flex flex-col items-center py-12 w-full">
                                <div class="w-12 h-12 bg-indigo-500/20 rounded-full flex items-center justify-center mb-6">
                                    <svg class="w-7 h-7 text-indigo-400 animate-pulse" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
                                        <path stroke-linecap="round" stroke-linejoin="round" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                                    </svg>
                                </div>
                                <p class="text-slate-100 text-lg font-medium mb-1">"Syncing encryption keys..."</p>
                                <p class="text-indigo-400 text-sm font-mono mb-4">{format!("{}%", pct)}</p>
                                <div class="w-full max-w-xs bg-slate-700 rounded-full h-2 overflow-hidden">
                                    <div
                                        class="bg-indigo-500 h-full rounded-full transition-all duration-300"
                                        style=width_style
                                    ></div>
                                </div>
                            </div>
                        }.into_any()
                    }

                    // ── Connected: Green status, device info, action buttons ──
                    PairingState::Connected { device_name, phone_number } => {
                        let masked_phone = mask_phone(&phone_number);
                        let dc = disconnect.clone();
                        let rp = replair.clone();
                        view! {
                            <div class="flex flex-col items-center w-full">
                                // Status header
                                <div class="flex items-center gap-3 mb-6">
                                    <div class="w-3 h-3 bg-green-500 rounded-full shadow-lg shadow-green-500/30"></div>
                                    <span class="text-green-400 font-semibold text-lg">"Connected"</span>
                                </div>

                                // Device info card
                                <div class="w-full bg-slate-900/50 rounded-lg p-6 mb-6 space-y-4">
                                    <div class="flex justify-between items-center">
                                        <span class="text-slate-400 text-sm">"Device"</span>
                                        <span class="text-slate-100 font-medium">{device_name}</span>
                                    </div>
                                    <div class="flex justify-between items-center">
                                        <span class="text-slate-400 text-sm">"Phone Number"</span>
                                        <span class="text-slate-100 font-mono">{masked_phone}</span>
                                    </div>
                                </div>

                                // Action buttons
                                <div class="flex gap-3 w-full">
                                    <button
                                        on:click=move |_| dc()
                                        class="flex-1 bg-slate-700 hover:bg-slate-600 text-slate-300 font-medium py-2.5 rounded-lg transition-colors"
                                    >
                                        "Disconnect"
                                    </button>
                                    <button
                                        on:click=move |_| rp()
                                        class="flex-1 bg-indigo-600 hover:bg-indigo-700 text-white font-medium py-2.5 rounded-lg transition-colors"
                                    >
                                        "Re-pair"
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    }

                    // ── Disconnected: Yellow status + reason + reconnect ──
                    PairingState::Disconnected { reason } => {
                        let start = start_connection.clone();
                        view! {
                            <div class="flex flex-col items-center w-full">
                                <div class="flex items-center gap-3 mb-4">
                                    <div class="w-3 h-3 bg-yellow-500 rounded-full shadow-lg shadow-yellow-500/30"></div>
                                    <span class="text-yellow-400 font-semibold text-lg">"Disconnected"</span>
                                </div>
                                <p class="text-slate-400 text-sm mb-6 text-center">{reason}</p>
                                <button
                                    on:click=move |_| start()
                                    class="bg-yellow-600 hover:bg-yellow-700 text-white font-medium px-8 py-3 rounded-lg transition-colors"
                                >
                                    "Reconnect"
                                </button>
                            </div>
                        }.into_any()
                    }

                    // ── Failed: Red status + error + retry ──
                    PairingState::Failed { error } => {
                        let start = start_connection.clone();
                        view! {
                            <div class="flex flex-col items-center w-full">
                                <div class="flex items-center gap-3 mb-4">
                                    <div class="w-3 h-3 bg-red-500 rounded-full shadow-lg shadow-red-500/30"></div>
                                    <span class="text-red-400 font-semibold text-lg">"Connection Failed"</span>
                                </div>
                                <div class="w-full bg-red-500/10 border border-red-500/20 rounded-lg p-4 mb-6">
                                    <p class="text-red-400 text-sm font-mono">{error}</p>
                                </div>
                                <button
                                    on:click=move |_| start()
                                    class="bg-red-600 hover:bg-red-700 text-white font-medium px-8 py-3 rounded-lg transition-colors"
                                >
                                    "Retry"
                                </button>
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

#[component]
fn TelegramPanel(state: DashboardState) -> impl IntoView {
    let _ = state;
    view! {
        <div class="bg-slate-800 rounded-lg p-8 max-w-2xl border border-slate-700 text-slate-100">
            <div class="flex flex-col items-center">
                <div class="mb-6">
                    <div class="w-16 h-16 bg-blue-500 rounded-full flex items-center justify-center mb-4 mx-auto">
                        <svg class="w-10 h-10 text-white" fill="currentColor" viewBox="0 0 24 24">
                            <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm4.64 6.8c-.15 1.58-.8 5.42-1.13 7.19-.14.75-.42 1-.68 1.03-.58.05-1.02-.38-1.58-.75-.88-.58-1.38-.94-2.23-1.5-.99-.65-.35-1.01.22-1.59.15-.15 2.71-2.48 2.76-2.69.01-.03.01-.14-.07-.2-.08-.06-.19-.04-.27-.02-.11.02-1.93 1.23-5.46 3.62-.51.35-.98.53-1.39.52-.46-.01-1.33-.26-1.98-.48-.8-.27-1.43-.42-1.37-.89.03-.25.38-.51 1.03-.78 4.04-1.76 6.74-2.92 8.09-3.48 3.85-1.6 4.64-1.88 5.17-1.89.11 0 .37.03.54.17.14.12.18.28.2.45-.02.07-.02.13-.03.2z"/>
                        </svg>
                    </div>
                    <h2 class="text-xl font-semibold text-center">"Telegram"</h2>
                    <p class="text-slate-400 text-center mt-2">"Connect your Telegram Bot"</p>
                </div>

                <div class="w-full space-y-4 mb-8">
                    <div>
                        <label class="block text-sm font-medium text-slate-400 mb-1">"Bot Token"</label>
                        <input
                            type="password"
                            placeholder="123456789:ABCDefgh..."
                            class="w-full bg-slate-900 border border-slate-700 rounded-md px-4 py-2 text-slate-100 focus:outline-none focus:ring-2 focus:ring-indigo-500"
                        />
                    </div>
                    <button class="w-full bg-indigo-600 hover:bg-indigo-700 text-white font-medium py-2 rounded-md transition-colors">
                        "Connect Bot"
                    </button>
                </div>

                <div class="w-full border-t border-slate-700 pt-6">
                    <p class="text-sm text-slate-400">
                        "To create a bot, talk to "
                        <a href="https://t.me/BotFather" target="_blank" class="text-indigo-400 hover:underline">"@BotFather"</a>
                        " on Telegram and get your API token."
                    </p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn DiscordPanel(state: DashboardState) -> impl IntoView {
    let _ = state;
    view! {
        <div class="bg-slate-800 rounded-lg p-8 max-w-2xl border border-slate-700 text-slate-100">
            <div class="flex flex-col items-center">
                <div class="mb-6 text-center">
                    <div class="w-16 h-16 bg-indigo-500 rounded-full flex items-center justify-center mb-4 mx-auto">
                        <svg class="w-10 h-10 text-white" fill="currentColor" viewBox="0 0 24 24">
                            <path d="M19.27 4.58c-1.53-.71-3.15-1.23-4.85-1.53-.2.36-.43.79-.58 1.15-1.83-.27-3.64-.27-5.45 0-.16-.36-.39-.79-.59-1.15-1.7.3-3.32.82-4.86 1.53-3.1 4.63-3.94 9.14-3.52 13.57 2.06 1.52 4.06 2.45 6.01 3.05.49-.67.92-1.39 1.28-2.15-1.12-.42-2.18-.95-3.17-1.59.27-.2.53-.41.78-.63 3.91 1.8 8.16 1.8 12.06 0 .25.22.51.43.78.63-.99.64-2.05 1.17-3.17 1.59.36.76.79 1.48 1.28 2.15 1.95-.6 3.95-1.53 6.01-3.05.48-5.11-.8-9.56-3.52-13.57zM8.02 15.33c-1.18 0-2.16-1.08-2.16-2.42 0-1.33.95-2.42 2.16-2.42 1.21 0 2.18 1.09 2.16 2.42 0 1.34-.95 2.42-2.16 2.42zm7.97 0c-1.18 0-2.16-1.08-2.16-2.42 0-1.33.95-2.42 2.16-2.42 1.21 0 2.18 1.09 2.16 2.42 0 1.34-.95 2.42-2.16 2.42z"/>
                        </svg>
                    </div>
                    <h2 class="text-xl font-semibold">"Discord"</h2>
                    <p class="text-slate-400 mt-2">"Integrate with your Discord Server"</p>
                </div>

                <div class="w-full space-y-4 mb-8">
                    <div>
                        <label class="block text-sm font-medium text-slate-400 mb-1">"Application Token"</label>
                        <input
                            type="password"
                            placeholder="MTAyND..."
                            class="w-full bg-slate-900 border border-slate-700 rounded-md px-4 py-2 text-slate-100 focus:outline-none focus:ring-2 focus:ring-indigo-500"
                        />
                    </div>
                    <button class="w-full bg-indigo-600 hover:bg-indigo-700 text-white font-medium py-2 rounded-md transition-colors">
                        "Connect Discord Bot"
                    </button>
                </div>

                <div class="w-full border-t border-slate-700 pt-6">
                    <p class="text-sm text-slate-400">
                        "Create an application in the "
                        <a href="https://discord.com/developers/applications" target="_blank" class="text-indigo-400 hover:underline">"Discord Developer Portal"</a>
                        " to get your bot token."
                    </p>
                </div>
            </div>
        </div>
    }
}
