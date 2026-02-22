use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::models::{ChannelInfo, PairingData};

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

#[component]
fn WhatsAppPanel(state: DashboardState) -> impl IntoView {
    let (qr_code, set_qr_code) = signal(Option::<String>::None);
    let (status, set_status) = signal("disconnected".to_string());

    let state_c = state.clone();
    let fetch_pairing = move || {
        let state = state_c.clone();
        spawn_local(async move {
            let result = state.rpc_call("channel.pairing_data", serde_json::json!({"channel_id": "whatsapp"})).await;
            if let Ok(val) = result {
                if let Ok(pairing) = serde_json::from_value::<PairingData>(val) {
                    match pairing {
                        PairingData::QrCode(data) => set_qr_code.set(Some(data)),
                        _ => {}
                    }
                }
            }
        });
    };

    let state_c2 = state.clone();
    let fetch_status = move || {
        let state = state_c2.clone();
        spawn_local(async move {
            let result = state.rpc_call("channels.status", serde_json::json!({"channel_id": "whatsapp"})).await;
            if let Ok(val) = result {
                if let Ok(info) = serde_json::from_value::<ChannelInfo>(val) {
                    set_status.set(info.status);
                }
            }
        });
    };

    Effect::new(move |_| {
        fetch_status();
    });

    view! {
        <div class="bg-slate-800 rounded-lg p-8 max-w-2xl border border-slate-700">
            <div class="flex flex-col items-center">
                <div class="mb-6">
                    <div class="w-16 h-16 bg-green-500 rounded-full flex items-center justify-center mb-4 mx-auto">
                        <svg class="w-10 h-10 text-white" fill="currentColor" viewBox="0 0 24 24">
                            <path d="M17.472 14.382c-.297-.149-1.758-.867-2.03-.967-.273-.099-.471-.148-.67.15-.197.297-.767.966-.94 1.164-.173.199-.347.223-.644.075-.297-.15-1.255-.463-2.39-1.475-.883-.788-1.48-1.761-1.653-2.059-.173-.297-.018-.458.13-.606.134-.133.298-.347.446-.52.149-.174.198-.298.298-.497.099-.198.05-.371-.025-.52-.075-.149-.669-1.612-.916-2.207-.242-.579-.487-.5-.669-.51-.173-.008-.371-.01-.57-.01-.198 0-.52.074-.792.372-.272.297-1.04 1.016-1.04 2.479 0 1.462 1.065 2.875 1.213 3.074.149.198 2.096 3.2 5.077 4.487.709.306 1.262.489 1.694.625.712.227 1.36.195 1.871.118.571-.085 1.758-.719 2.006-1.413.248-.694.248-1.289.173-1.413-.074-.124-.272-.198-.57-.347m-5.421 7.403h-.004a9.87 9.87 0 01-5.031-1.378l-.361-.214-3.741.982.998-3.648-.235-.374a9.86 9.86 0 01-1.51-5.26c.001-5.45 4.436-9.884 9.888-9.884 2.64 0 5.122 1.03 6.988 2.898a9.825 9.825 0 012.893 6.994c-.003 5.45-4.437 9.884-9.885 9.884m8.413-18.297A11.815 11.815 0 0012.05 0C5.495 0 .16 5.335.157 11.892c0 2.096.547 4.142 1.588 5.945L.057 24l6.305-1.654a11.882 11.882 0 005.683 1.448h.005c6.554 0 11.89-5.335 11.893-11.893a11.821 11.821 0 00-3.48-8.413Z"/>
                        </svg>
                    </div>
                    <h2 class="text-xl font-semibold text-center text-slate-100">"WhatsApp"</h2>
                    <p class="text-slate-400 text-center mt-2">"Link your WhatsApp account to Aleph"</p>
                </div>

                <div class="w-full flex justify-center mb-8">
                    {move || match qr_code.get() {
                        Some(data) => view! {
                            <div class="bg-white p-4 rounded-lg shadow-xl">
                                <img src=data class="w-64 h-64" alt="WhatsApp QR Code" />
                            </div>
                        }.into_any(),
                        None => view! {
                            <div class="w-64 h-64 bg-slate-700 rounded-lg flex items-center justify-center border-2 border-dashed border-slate-600">
                                <button
                                    on:click=move |_| fetch_pairing()
                                    class="bg-indigo-600 hover:bg-indigo-700 text-white px-6 py-2 rounded-md transition-colors"
                                >
                                    "Generate QR Code"
                                </button>
                            </div>
                        }.into_any(),
                    }}
                </div>

                <div class="w-full border-t border-slate-700 pt-6">
                    <div class="flex justify-between items-center">
                        <span class="text-slate-400">"Status"</span>
                        <div class="flex items-center">
                            <div class=move || format!("w-3 h-3 rounded-full mr-2 {}",
                                if status.get() == "connected" { "bg-green-500" }
                                else if status.get() == "connecting" { "bg-yellow-500 animate-pulse" }
                                else { "bg-red-500" }
                            )></div>
                            <span class="text-slate-200 capitalize">{move || status.get()}</span>
                        </div>
                    </div>
                </div>
            </div>
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
