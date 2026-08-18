//! Discord Channel Configuration View
//!
//! Provides a full management panel for Discord bot integration with 4 sections:
//! - Bot Identity: Shows bot name, avatar, ID, online/offline status
//! - Token Configuration: Password input for token, validate/reset buttons
//! - Guild Management: Dual-column guild + channel selection with checkboxes
//! - Permission Audit: Traffic-light permission checks with health badge

use crate::api::DiscordApi;
use crate::components::forms::ErrorMessageDynamic;
use crate::components::ui::AgentBindingSelector;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;

// ============================================================================
// Data types
// ============================================================================

/// Bot identity information returned from token validation
#[derive(Debug, Clone, Default)]
struct BotIdentity {
    name: String,
    discriminator: String,
    id: String,
    avatar_url: String,
    is_online: bool,
}

/// A single Discord guild
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct GuildInfo {
    id: u64,
    name: String,
    icon_url: Option<String>,
    member_count: Option<u64>,
    checked: bool,
}

/// A single Discord channel within a guild
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ChannelInfo {
    id: u64,
    name: String,
    kind: String,
    checked: bool,
}

/// Permission check result
#[derive(Debug, Clone)]
struct PermissionCheck {
    name: String,
    status: PermissionStatus,
    description: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PermissionStatus {
    Granted,
    Partial,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OverallHealth {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

// ============================================================================
// Main View
// ============================================================================

#[component]
#[must_use]
pub fn DiscordChannelView() -> impl IntoView {
    let i18n = use_i18n();
    let _state = expect_context::<DashboardState>();

    // Shared signals
    let token = RwSignal::new(String::new());
    let bot_identity = RwSignal::new(Option::<BotIdentity>::None);
    let guilds = RwSignal::new(Vec::<GuildInfo>::new());
    let selected_guild_id = RwSignal::new(Option::<u64>::None);
    let channels = RwSignal::new(Vec::<ChannelInfo>::new());
    let permissions = RwSignal::new(Vec::<PermissionCheck>::new());
    let overall_health = RwSignal::new(OverallHealth::Unknown);
    let error = RwSignal::new(Option::<String>::None);
    let validating = RwSignal::new(false);
    let loading_guilds = RwSignal::new(false);
    let loading_channels = RwSignal::new(false);
    let loading_permissions = RwSignal::new(false);

    // Channel ID for RPC calls (fixed for single-bot setup)
    let channel_id = "discord-default";

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">

                // Back link
                <A
                    href="/settings/channels"
                    attr:class="inline-flex items-center gap-1 text-sm text-text-tertiary hover:text-text-primary transition-colors"
                >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6"/>
                    </svg>
                    {t!(i18n, settings.channels.back_to_channels)}
                </A>

                // Header — matches config_template.rs structure
                <div>
                    <div class="flex items-center gap-3 mb-1">
                        <div
                            class="w-10 h-10 rounded-lg flex items-center justify-center"
                            style="background-color: #5865F215"
                        >
                            <svg
                                width="22"
                                height="22"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="#5865F2"
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                inner_html=super::definitions::ICON_DISCORD
                            />
                        </div>
                        <div>
                            <h1 class="text-2xl font-semibold text-text-primary">"Discord"</h1>
                        </div>
                    </div>
                    <p class="text-sm text-text-secondary mt-1">
                        {t!(i18n, settings.channels.discord_subtitle)}
                    </p>
                </div>

                // Error banner — uses shared ErrorMessageDynamic component
                <ErrorMessageDynamic error=error.into() />

                // Agent binding selector
                <AgentBindingSelector channel_id=channel_id.to_string() />

                // Section 1: Bot Identity
                <BotIdentitySection bot_identity=bot_identity />

                // Section 2: Token Configuration
                <TokenSection
                    token=token
                    bot_identity=bot_identity
                    guilds=guilds
                    error=error
                    validating=validating
                    loading_guilds=loading_guilds
                    channel_id=channel_id
                />

                // Section 3: Guild & Channel Management
                <GuildSection
                    guilds=guilds
                    selected_guild_id=selected_guild_id
                    channels=channels
                    permissions=permissions
                    overall_health=overall_health
                    loading_guilds=loading_guilds
                    loading_channels=loading_channels
                    loading_permissions=loading_permissions
                    error=error
                    channel_id=channel_id
                />

                // Section 4: Permission Audit
                <PermissionAuditSection
                    permissions=permissions
                    overall_health=overall_health
                    loading_permissions=loading_permissions
                    selected_guild_id=selected_guild_id
                />

                // Documentation link — matches config_template.rs
                <div class="flex items-center justify-end">
                    <a
                        href="https://discord.com/developers/docs"
                        target="_blank"
                        rel="noopener noreferrer"
                        class="text-sm text-text-tertiary hover:text-primary transition-colors inline-flex items-center gap-1"
                    >
                        {t!(i18n, settings.channels.documentation)}
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
                            <polyline points="15 3 21 3 21 9"/>
                            <line x1="10" y1="14" x2="21" y2="3"/>
                        </svg>
                    </a>
                </div>

            </div>
        </div>
    }
}

// ============================================================================
// Section 1: Bot Identity
// ============================================================================

#[component]
fn BotIdentitySection(bot_identity: RwSignal<Option<BotIdentity>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">{t!(i18n, settings.channels.bot_identity)}</h2>

            {move || {
                match bot_identity.get() {
                    Some(bot) => {
                        view! {
                            <div class="flex items-center gap-4">
                                // Avatar
                                <div class="relative">
                                    {if bot.avatar_url.is_empty() {
                                        view! {
                                            <div class="w-16 h-16 rounded-full bg-primary flex items-center justify-center text-white text-xl font-bold">
                                                {bot.name.chars().next().unwrap_or('?').to_string()}
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <img
                                                src=bot.avatar_url.clone()
                                                alt=t_string!(i18n, settings.channels.bot_avatar_alt)
                                                class="w-16 h-16 rounded-full object-cover"
                                            />
                                        }.into_any()
                                    }}
                                    // Online status dot
                                    <span class={
                                        let color = if bot.is_online { "bg-green-500" } else { "bg-gray-400" };
                                        format!("absolute bottom-0 right-0 w-4 h-4 rounded-full border-2 border-white {color}")
                                    }></span>
                                </div>

                                // Info
                                <div>
                                    <div class="flex items-center gap-2">
                                        <span class="text-lg font-semibold text-text-primary">{bot.name.clone()}</span>
                                        {if !bot.discriminator.is_empty() && bot.discriminator != "0" {
                                            Some(view! {
                                                <span class="text-text-tertiary text-sm">
                                                    {"#"}{bot.discriminator.clone()}
                                                </span>
                                            })
                                        } else {
                                            None
                                        }}
                                    </div>
                                    <div class="text-sm text-text-secondary font-mono mt-0.5">{bot.id.clone()}</div>
                                    <div class="mt-1">
                                        {if bot.is_online {
                                            view! {
                                                <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full bg-green-100 text-green-700">
                                                    <span class="w-1.5 h-1.5 rounded-full bg-green-500"></span>
                                                    {t!(i18n, settings.channels.bot_online)}
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full bg-gray-100 text-gray-600">
                                                    <span class="w-1.5 h-1.5 rounded-full bg-gray-400"></span>
                                                    {t!(i18n, settings.channels.bot_offline)}
                                                </span>
                                            }.into_any()
                                        }}
                                    </div>
                                </div>
                            </div>
                        }.into_any()
                    }
                    None => {
                        view! {
                            <div class="flex items-center gap-3 text-text-tertiary">
                                <div class="w-16 h-16 rounded-full bg-surface-sunken flex items-center justify-center">
                                    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/>
                                        <circle cx="12" cy="7" r="4"/>
                                    </svg>
                                </div>
                                <div>
                                    <p class="text-sm">{t!(i18n, settings.channels.no_bot_connected)}</p>
                                    <p class="text-xs text-text-tertiary">{t!(i18n, settings.channels.no_bot_hint)}</p>
                                </div>
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

// ============================================================================
// Section 2: Token Configuration
// ============================================================================

#[component]
fn TokenSection(
    token: RwSignal<String>,
    bot_identity: RwSignal<Option<BotIdentity>>,
    guilds: RwSignal<Vec<GuildInfo>>,
    error: RwSignal<Option<String>>,
    validating: RwSignal<bool>,
    loading_guilds: RwSignal<bool>,
    channel_id: &'static str,
) -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let state = expect_context::<DashboardState>();

    let on_validate = move |_| {
        let token_val = token.get();
        if token_val.is_empty() {
            error.set(Some("Please enter a bot token".to_string()));
            return;
        }

        validating.set(true);
        error.set(None);

        spawn_local(async move {
            match DiscordApi::validate_token(&state, token_val).await {
                Ok(result) => {
                    // Parse bot identity from response
                    let name = result
                        .get("username")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let discriminator = result
                        .get("discriminator")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0")
                        .to_string();
                    let id = result
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let avatar = result
                        .get("avatar")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let avatar_url = if !avatar.is_empty() && !id.is_empty() {
                        format!("https://cdn.discordapp.com/avatars/{id}/{avatar}.png?size=128")
                    } else {
                        String::new()
                    };

                    bot_identity.set(Some(BotIdentity {
                        name,
                        discriminator,
                        id,
                        avatar_url,
                        is_online: true,
                    }));

                    // Auto-fetch guilds after successful validation
                    loading_guilds.set(true);
                    match DiscordApi::list_guilds(&state, channel_id).await {
                        Ok(guild_list) => {
                            let parsed: Vec<GuildInfo> = guild_list
                                .iter()
                                .map(|g| GuildInfo {
                                    id: g
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .or_else(|| g.get("id").and_then(serde_json::Value::as_u64))
                                        .unwrap_or(0),
                                    name: g
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Unknown Guild")
                                        .to_string(),
                                    icon_url: g.get("icon").and_then(|v| v.as_str()).map(|icon| {
                                        let gid =
                                            g.get("id").and_then(|v| v.as_str()).unwrap_or("0");
                                        format!(
                                            "https://cdn.discordapp.com/icons/{gid}/{icon}.png?size=64"
                                        )
                                    }),
                                    member_count: g
                                        .get("approximate_member_count")
                                        .and_then(serde_json::Value::as_u64),
                                    checked: false,
                                })
                                .collect();
                            guilds.set(parsed);
                        }
                        Err(e) => {
                            error.set(Some(crate::components::admin_refusal::settings_load_error(
                                i18n,
                                &e,
                                |e| format!("Token valid, but failed to fetch guilds: {e}"),
                            )));
                        }
                    }
                    loading_guilds.set(false);
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Token validation failed: {e}")
                        }),
                    ));
                    bot_identity.set(None);
                }
            }
            validating.set(false);
        });
    };

    let on_reset = move |_| {
        token.set(String::new());
        bot_identity.set(None);
        guilds.set(Vec::new());
        error.set(None);
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">{t!(i18n, settings.channels.token_config)}</h2>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        {t!(i18n, settings.channels.bot_token_label)}
                    </label>
                    <div class="flex gap-3">
                        <input
                            type="password"
                            prop:value=move || token.get()
                            on:input=move |ev| {
                                token.set(event_target_value(&ev));
                            }
                            placeholder=t_string!(i18n, settings.channels.bot_token_placeholder)
                            class="flex-1 px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono text-sm"
                        />
                        <button
                            on:click=on_validate
                            disabled=move || validating.get() || token.get().is_empty()
                            class="px-4 py-2 bg-primary text-text-inverse rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm font-medium"
                        >
                            {move || if validating.get() { t_string!(i18n, settings.channels.validating) } else { t_string!(i18n, settings.channels.validate) }}
                        </button>
                        <button
                            on:click=on_reset
                            class="px-4 py-2 bg-surface-sunken border border-border text-text-secondary rounded-lg hover:bg-surface hover:text-text-primary transition-colors text-sm font-medium"
                        >
                            {t!(i18n, settings.channels.reset)}
                        </button>
                    </div>
                    <p class="mt-2 text-xs text-text-tertiary">
                        {t!(i18n, settings.channels.token_privacy_note)}
                    </p>
                </div>
            </div>
        </div>
    }
}

// ============================================================================
// Section 3: Guild & Channel Management
// ============================================================================

#[component]
fn GuildSection(
    guilds: RwSignal<Vec<GuildInfo>>,
    selected_guild_id: RwSignal<Option<u64>>,
    channels: RwSignal<Vec<ChannelInfo>>,
    permissions: RwSignal<Vec<PermissionCheck>>,
    overall_health: RwSignal<OverallHealth>,
    loading_guilds: RwSignal<bool>,
    loading_channels: RwSignal<bool>,
    loading_permissions: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    channel_id: &'static str,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Refresh guilds
    let on_refresh = move |_| {
        loading_guilds.set(true);
        error.set(None);
        spawn_local(async move {
            match DiscordApi::list_guilds(&state, channel_id).await {
                Ok(guild_list) => {
                    let parsed: Vec<GuildInfo> = guild_list
                        .iter()
                        .map(|g| GuildInfo {
                            id: g
                                .get("id")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<u64>().ok())
                                .or_else(|| g.get("id").and_then(serde_json::Value::as_u64))
                                .unwrap_or(0),
                            name: g
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown Guild")
                                .to_string(),
                            icon_url: g.get("icon").and_then(|v| v.as_str()).map(|icon| {
                                let gid = g.get("id").and_then(|v| v.as_str()).unwrap_or("0");
                                format!("https://cdn.discordapp.com/icons/{gid}/{icon}.png?size=64")
                            }),
                            member_count: g
                                .get("approximate_member_count")
                                .and_then(serde_json::Value::as_u64),
                            checked: false,
                        })
                        .collect();
                    guilds.set(parsed);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to refresh guilds: {e}"),
                    )));
                }
            }
            loading_guilds.set(false);
        });
    };

    // Select a guild -> load channels & audit permissions
    let on_select_guild = move |guild_id: u64| {
        selected_guild_id.set(Some(guild_id));

        // Load channels
        loading_channels.set(true);
        spawn_local(async move {
            match DiscordApi::list_channels(&state, channel_id, guild_id).await {
                Ok(channel_list) => {
                    let parsed: Vec<ChannelInfo> = channel_list
                        .iter()
                        .map(|c| ChannelInfo {
                            id: c
                                .get("id")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<u64>().ok())
                                .or_else(|| c.get("id").and_then(serde_json::Value::as_u64))
                                .unwrap_or(0),
                            name: c
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            kind: {
                                let kind_val = c
                                    .get("type")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or(0);
                                match kind_val {
                                    0 => "Text".to_string(),
                                    2 => "Voice".to_string(),
                                    4 => "Category".to_string(),
                                    5 => "Announcement".to_string(),
                                    13 => "Stage".to_string(),
                                    15 => "Forum".to_string(),
                                    _ => format!("Type {kind_val}"),
                                }
                            },
                            checked: false,
                        })
                        .collect();
                    channels.set(parsed);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to load channels: {e}"),
                    )));
                }
            }
            loading_channels.set(false);
        });

        // Audit permissions
        loading_permissions.set(true);
        spawn_local(async move {
            match DiscordApi::audit_permissions(&state, channel_id, guild_id).await {
                Ok(result) => {
                    // Parse permission checks from audit result
                    let checks_val = result
                        .get("checks")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();

                    let parsed_checks: Vec<PermissionCheck> = checks_val
                        .iter()
                        .map(|c| {
                            let status_str = c
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("missing");
                            PermissionCheck {
                                name: c
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown")
                                    .to_string(),
                                status: match status_str {
                                    "granted" => PermissionStatus::Granted,
                                    "partial" => PermissionStatus::Partial,
                                    _ => PermissionStatus::Missing,
                                },
                                description: c
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            }
                        })
                        .collect();

                    // Compute overall health
                    let health = if parsed_checks.is_empty() {
                        OverallHealth::Unknown
                    } else if parsed_checks
                        .iter()
                        .all(|c| c.status == PermissionStatus::Granted)
                    {
                        OverallHealth::Healthy
                    } else if parsed_checks
                        .iter()
                        .any(|c| c.status == PermissionStatus::Missing)
                    {
                        OverallHealth::Unhealthy
                    } else {
                        OverallHealth::Degraded
                    };

                    permissions.set(parsed_checks);
                    overall_health.set(health);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to audit permissions: {e}"),
                    )));
                }
            }
            loading_permissions.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <div class="flex items-center justify-between mb-4">
                <h2 class="text-xl font-semibold text-text-primary">{t!(i18n, settings.channels.guild_channel_management)}</h2>
                <button
                    on:click=on_refresh
                    disabled=move || loading_guilds.get()
                    class="px-3 py-1.5 bg-surface-sunken border border-border text-text-secondary rounded-lg hover:bg-surface hover:text-text-primary disabled:opacity-50 transition-colors text-sm flex items-center gap-1.5"
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                        class=move || if loading_guilds.get() { "animate-spin" } else { "" }
                    >
                        <polyline points="23 4 23 10 17 10"/>
                        <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
                    </svg>
                    {move || if loading_guilds.get() { t_string!(i18n, settings.channels.refreshing) } else { t_string!(i18n, settings.channels.refresh) }}
                </button>
            </div>

            {move || {
                let guild_list = guilds.get();
                if guild_list.is_empty() && !loading_guilds.get() {
                    view! {
                        <div class="text-center py-8 text-text-tertiary text-sm">
                            <p>{t!(i18n, settings.channels.no_guilds)}</p>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="flex gap-6 min-h-[300px]">
                            // Left column: Guild list
                            <div class="w-1/2 border border-border rounded-lg overflow-hidden">
                                <div class="bg-surface-sunken px-3 py-2 border-b border-border">
                                    <span class="text-xs font-medium text-text-secondary uppercase tracking-wider">
                                        "Guilds ("{guild_list.len()}")"
                                    </span>
                                </div>
                                <div class="overflow-y-auto max-h-[320px]">
                                    {guild_list.iter().map(|guild| {
                                        let gid = guild.id;
                                        let gname = guild.name.clone();
                                        let icon = guild.icon_url.clone();
                                        let members = guild.member_count;
                                        let is_selected = move || selected_guild_id.get() == Some(gid);
                                        let on_click = move |_| {
                                            on_select_guild(gid);
                                        };
                                        view! {
                                            <div
                                                on:click=on_click
                                                class=move || {
                                                    if is_selected() {
                                                        "flex items-center gap-3 px-3 py-2.5 cursor-pointer bg-primary-subtle border-l-2 border-primary"
                                                    } else {
                                                        "flex items-center gap-3 px-3 py-2.5 cursor-pointer hover:bg-surface-sunken border-l-2 border-transparent"
                                                    }
                                                }
                                            >
                                                // Guild icon
                                                {match icon {
                                                    Some(ref url) => {
                                                        let url = url.clone();
                                                        view! {
                                                            <img
                                                                src=url
                                                                alt=t_string!(i18n, settings.channels.guild_icon_alt)
                                                                class="w-8 h-8 rounded-full"
                                                            />
                                                        }.into_any()
                                                    }
                                                    None => {
                                                        let initial = gname.chars().next().unwrap_or('?').to_string();
                                                        view! {
                                                            <div class="w-8 h-8 rounded-full bg-primary-subtle flex items-center justify-center text-primary text-xs font-bold">
                                                                {initial}
                                                            </div>
                                                        }.into_any()
                                                    }
                                                }}
                                                <div class="min-w-0 flex-1">
                                                    <div class="text-sm font-medium text-text-primary truncate">
                                                        {gname}
                                                    </div>
                                                    {members.map(|m| {
                                                        view! {
                                                            <div class="text-xs text-text-tertiary">
                                                                {t!(i18n, settings.channels.guild_members, count = move || m)}
                                                            </div>
                                                        }
                                                    })}
                                                </div>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            </div>

                            // Right column: Channels for selected guild
                            <div class="w-1/2 border border-border rounded-lg overflow-hidden">
                                <div class="bg-surface-sunken px-3 py-2 border-b border-border">
                                    <span class="text-xs font-medium text-text-secondary uppercase tracking-wider">
                                        {t!(i18n, settings.channels.channel_list)}
                                    </span>
                                </div>
                                <div class="overflow-y-auto max-h-[320px]">
                                    {move || {
                                        if loading_channels.get() {
                                            view! {
                                                <div class="p-4 text-center text-text-tertiary text-sm">
                                                    {t!(i18n, settings.channels.loading_channels)}
                                                </div>
                                            }.into_any()
                                        } else if selected_guild_id.get().is_none() {
                                            view! {
                                                <div class="p-4 text-center text-text-tertiary text-sm">
                                                    {t!(i18n, settings.channels.select_guild_hint)}
                                                </div>
                                            }.into_any()
                                        } else {
                                            let ch_list = channels.get();
                                            if ch_list.is_empty() {
                                                view! {
                                                    <div class="p-4 text-center text-text-tertiary text-sm">
                                                        {t!(i18n, settings.channels.no_channels_found)}
                                                    </div>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <div>
                                                        {ch_list.iter().enumerate().map(|(idx, ch)| {
                                                            let ch_name = ch.name.clone();
                                                            let ch_kind = ch.kind.clone();
                                                            let is_checked = ch.checked;
                                                            let kind_icon = match ch_kind.as_str() {
                                                                "Text" => "#",
                                                                "Voice" => "V",
                                                                "Category" => ">",
                                                                "Announcement" => "A",
                                                                "Stage" => "S",
                                                                "Forum" => "F",
                                                                _ => "?",
                                                            };
                                                            view! {
                                                                <div class="flex items-center gap-3 px-3 py-2 hover:bg-surface-sunken">
                                                                    <input
                                                                        type="checkbox"
                                                                        checked=is_checked
                                                                        on:change=move |_| {
                                                                            channels.update(|chs| {
                                                                                if let Some(ch) = chs.get_mut(idx) {
                                                                                    ch.checked = !ch.checked;
                                                                                }
                                                                            });
                                                                        }
                                                                        class="rounded border-border text-primary focus:ring-primary"
                                                                    />
                                                                    <span class="text-xs text-text-tertiary font-mono w-4">
                                                                        {kind_icon}
                                                                    </span>
                                                                    <span class="text-sm text-text-primary">{ch_name}</span>
                                                                    <span class="ml-auto text-xs text-text-tertiary">{ch_kind}</span>
                                                                </div>
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                }.into_any()
                                            }
                                        }
                                    }}
                                </div>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ============================================================================
// Section 4: Permission Audit
// ============================================================================

#[component]
fn PermissionAuditSection(
    permissions: RwSignal<Vec<PermissionCheck>>,
    overall_health: RwSignal<OverallHealth>,
    loading_permissions: RwSignal<bool>,
    selected_guild_id: RwSignal<Option<u64>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <div class="flex items-center justify-between mb-4">
                <h2 class="text-xl font-semibold text-text-primary">{t!(i18n, settings.channels.permission_audit)}</h2>

                // Overall health badge
                {move || {
                    let health = overall_health.get();
                    match health {
                        OverallHealth::Healthy => {
                            view! {
                                <span class="inline-flex items-center gap-1.5 text-xs font-medium px-2.5 py-1 rounded-full bg-green-100 text-green-700">
                                    <span class="w-2 h-2 rounded-full bg-green-500"></span>
                                    {t!(i18n, settings.channels.perm_all_granted)}
                                </span>
                            }.into_any()
                        }
                        OverallHealth::Degraded => {
                            view! {
                                <span class="inline-flex items-center gap-1.5 text-xs font-medium px-2.5 py-1 rounded-full bg-yellow-100 text-yellow-700">
                                    <span class="w-2 h-2 rounded-full bg-yellow-500"></span>
                                    {t!(i18n, settings.channels.perm_partial)}
                                </span>
                            }.into_any()
                        }
                        OverallHealth::Unhealthy => {
                            view! {
                                <span class="inline-flex items-center gap-1.5 text-xs font-medium px-2.5 py-1 rounded-full bg-red-100 text-red-700">
                                    <span class="w-2 h-2 rounded-full bg-red-500"></span>
                                    {t!(i18n, settings.channels.perm_missing)}
                                </span>
                            }.into_any()
                        }
                        OverallHealth::Unknown => {
                            view! {
                                <span class="inline-flex items-center gap-1.5 text-xs font-medium px-2.5 py-1 rounded-full bg-gray-100 text-gray-600">
                                    <span class="w-2 h-2 rounded-full bg-gray-400"></span>
                                    {t!(i18n, settings.channels.perm_not_audited)}
                                </span>
                            }.into_any()
                        }
                    }
                }}
            </div>

            {move || {
                if loading_permissions.get() {
                    view! {
                        <div class="py-6 text-center text-text-tertiary text-sm">
                            {t!(i18n, settings.channels.auditing_permissions)}
                        </div>
                    }.into_any()
                } else if selected_guild_id.get().is_none() {
                    view! {
                        <div class="py-6 text-center text-text-tertiary text-sm">
                            {t!(i18n, settings.channels.perm_select_guild_hint)}
                        </div>
                    }.into_any()
                } else {
                    let perms = permissions.get();
                    if perms.is_empty() {
                        view! {
                            <div class="py-6 text-center text-text-tertiary text-sm">
                                {t!(i18n, settings.channels.perm_no_data)}
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-2">
                                {perms.iter().map(|perm| {
                                    let (dot_color, text_color) = match perm.status {
                                        PermissionStatus::Granted => ("bg-green-500", "text-green-700"),
                                        PermissionStatus::Partial => ("bg-yellow-500", "text-yellow-700"),
                                        PermissionStatus::Missing => ("bg-red-500", "text-red-700"),
                                    };
                                    let perm_name = perm.name.clone();
                                    let perm_desc = perm.description.clone();
                                    let status_label = match perm.status {
                                        PermissionStatus::Granted => "Granted",
                                        PermissionStatus::Partial => "Partial",
                                        PermissionStatus::Missing => "Missing",
                                    };

                                    view! {
                                        <div class="flex items-center gap-3 px-4 py-3 bg-surface-sunken rounded-lg">
                                            // Traffic light dot
                                            <span class={format!("w-2.5 h-2.5 rounded-full flex-shrink-0 {dot_color}")}></span>
                                            // Permission name
                                            <div class="flex-1 min-w-0">
                                                <span class="text-sm font-medium text-text-primary">{perm_name}</span>
                                                {if !perm_desc.is_empty() {
                                                    Some(view! {
                                                        <p class="text-xs text-text-tertiary mt-0.5">{perm_desc}</p>
                                                    })
                                                } else {
                                                    None
                                                }}
                                            </div>
                                            // Status label
                                            <span class={format!("text-xs font-medium {text_color}")}>
                                                {status_label}
                                            </span>
                                        </div>
                                    }
                                }).collect_view()}

                                // Fix suggestions for missing or partial permissions
                                {move || {
                                    let perms = permissions.get();
                                    let missing: Vec<&PermissionCheck> = perms.iter()
                                        .filter(|p| p.status != PermissionStatus::Granted)
                                        .collect();

                                    if missing.is_empty() {
                                        None
                                    } else {
                                        Some(view! {
                                            <div class="mt-4 p-4 bg-warning-subtle border border-warning/30 rounded-lg">
                                                <h3 class="text-sm font-semibold text-warning mb-2">
                                                    {t!(i18n, settings.channels.perm_fix_suggestions)}
                                                </h3>
                                                <ul class="space-y-1 text-xs text-warning">
                                                    {missing.iter().map(|p| {
                                                        let suggestion = format!(
                                                            "Grant '{}' permission in Server Settings > Roles",
                                                            p.name
                                                        );
                                                        view! {
                                                            <li class="flex items-start gap-2">
                                                                <span class="mt-0.5 text-warning">">"</span>
                                                                <span>{suggestion}</span>
                                                            </li>
                                                        }
                                                    }).collect_view()}
                                                    <li class="flex items-start gap-2 mt-2 pt-2 border-t border-warning/30">
                                                        <span class="mt-0.5 text-warning">">"</span>
                                                        <span>{t!(i18n, settings.channels.invite_link_hint)}</span>
                                                    </li>
                                                </ul>
                                            </div>
                                        })
                                    }
                                }}
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}
