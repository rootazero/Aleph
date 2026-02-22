//! Reusable form components for settings pages
//!
//! This module provides a set of composable form components that follow
//! a consistent design system across all settings pages.

use leptos::prelude::*;

// ============================================================================
// Section Container
// ============================================================================

/// A styled section container for grouping related settings
///
/// # Example
/// ```rust
/// view! {
///     <SettingsSection title="Language">
///         <FormField label="Interface Language">
///             <select>...</select>
///         </FormField>
///     </SettingsSection>
/// }
/// ```
#[component]
pub fn SettingsSection(
    /// Section title
    title: &'static str,
    /// Optional description text
    #[prop(optional)]
    description: Option<&'static str>,
    /// Section content
    children: Children,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <div class="mb-4">
                <h2 class="text-xl font-semibold text-text-primary">{title}</h2>
                {description.map(|desc| view! {
                    <p class="text-sm text-text-secondary mt-1">{desc}</p>
                })}
            </div>
            <div class="space-y-4">
                {children()}
            </div>
        </div>
    }
}

// ============================================================================
// Form Field Container
// ============================================================================

/// A form field with label and optional help text
///
/// # Example
/// ```rust
/// view! {
///     <FormField label="Email" help_text="Your email address">
///         <input type="email" />
///     </FormField>
/// }
/// ```
#[component]
pub fn FormField(
    /// Field label
    label: &'static str,
    /// Optional help text shown below the field
    #[prop(optional)]
    help_text: Option<&'static str>,
    /// Form control element
    children: Children,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            <label class="block text-sm font-medium text-text-secondary">
                {label}
            </label>
            {children()}
            {help_text.map(|text| view! {
                <p class="text-xs text-text-tertiary">{text}</p>
            })}
        </div>
    }
}

// ============================================================================
// Text Input
// ============================================================================

/// A styled text input field
#[component]
pub fn TextInput(
    /// Current value
    value: Signal<String>,
    /// Change handler
    on_change: impl Fn(String) + 'static,
    /// Optional placeholder
    #[prop(optional)]
    placeholder: Option<&'static str>,
    /// Input type (default: "text")
    #[prop(optional)]
    input_type: Option<&'static str>,
    /// Use monospace font
    #[prop(optional)]
    monospace: bool,
) -> impl IntoView {
    let input_type = input_type.unwrap_or("text");
    let font_class = if monospace { "font-mono" } else { "" };

    view! {
        <input
            type=input_type
            value=move || value.get()
            on:input=move |ev| on_change(event_target_value(&ev))
            placeholder=placeholder.unwrap_or("")
            class=format!(
                "w-full px-3 py-2 bg-surface-raised border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary {}",
                font_class
            )
        />
    }
}

// ============================================================================
// Select Dropdown
// ============================================================================

/// A styled select dropdown
#[component]
pub fn SelectInput(
    /// Current value
    value: Signal<String>,
    /// Change handler
    on_change: impl Fn(String) + 'static,
    /// Select options as (value, label) pairs
    options: Vec<(&'static str, &'static str)>,
) -> impl IntoView {
    view! {
        <select
            prop:value=move || value.get()
            on:change=move |ev| on_change(event_target_value(&ev))
            class="w-full px-3 py-2 bg-surface-raised border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
        >
            {options.into_iter().map(|(val, label)| view! {
                <option value=val>{label}</option>
            }).collect_view()}
        </select>
    }
}

// ============================================================================
// Number Input with Slider
// ============================================================================

/// A number input with an optional slider
#[component]
pub fn NumberInput(
    /// Current value
    value: Signal<i32>,
    /// Change handler
    on_change: impl Fn(i32) + 'static + Copy,
    /// Minimum value
    min: i32,
    /// Maximum value
    max: i32,
    /// Step size
    #[prop(optional)]
    step: Option<i32>,
    /// Show slider
    #[prop(optional)]
    show_slider: bool,
    /// Value suffix (e.g., "ms", "px")
    #[prop(optional)]
    suffix: Option<&'static str>,
) -> impl IntoView {
    let step = step.unwrap_or(1);

    view! {
        <div class="flex items-center gap-3">
            {if show_slider {
                view! {
                    <input
                        type="range"
                        min=min
                        max=max
                        step=step
                        value=move || value.get()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<i32>() {
                                on_change(v);
                            }
                        }
                        class="flex-1 h-2 bg-border rounded-lg appearance-none cursor-pointer accent-primary"
                    />
                }.into_any()
            } else {
                view! { <div class="flex-1"></div> }.into_any()
            }}
            <div class="flex items-center gap-1">
                <input
                    type="number"
                    min=min
                    max=max
                    step=step
                    value=move || value.get()
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<i32>() {
                            on_change(v);
                        }
                    }
                    class="w-20 px-2 py-1 bg-surface-raised border border-border rounded text-text-primary text-sm text-right focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                />
                {suffix.map(|s| view! {
                    <span class="text-sm text-text-secondary">{s}</span>
                })}
            </div>
        </div>
    }
}

// ============================================================================
// Switch Toggle
// ============================================================================

/// A toggle switch component
#[component]
pub fn SwitchInput(
    /// Current checked state
    checked: Signal<bool>,
    /// Change handler
    on_change: impl Fn(bool) + 'static,
    /// Optional label
    #[prop(optional)]
    label: Option<&'static str>,
) -> impl IntoView {
    view! {
        <label class="flex items-center gap-3 cursor-pointer">
            <div class="relative">
                <input
                    type="checkbox"
                    checked=move || checked.get()
                    on:change=move |ev| on_change(event_target_checked(&ev))
                    class="sr-only peer"
                />
                <div class="w-11 h-6 bg-border peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
            </div>
            {label.map(|l| view! {
                <span class="text-sm text-text-secondary">{l}</span>
            })}
        </label>
    }
}

// ============================================================================
// Error Message
// ============================================================================

/// An error message box
#[component]
pub fn ErrorMessage(
    /// Error message text
    message: &'static str,
) -> impl IntoView {
    view! {
        <div class="p-4 bg-danger-subtle border border-danger/30 rounded-lg text-danger text-sm">
            {message}
        </div>
    }
}

/// A dynamic error message box
#[component]
pub fn ErrorMessageDynamic(
    /// Error message signal
    error: Signal<Option<String>>,
) -> impl IntoView {
    view! {
        {move || error.get().map(|msg| view! {
            <div class="p-4 bg-danger-subtle border border-danger/30 rounded-lg text-danger text-sm">
                {msg}
            </div>
        })}
    }
}

// ============================================================================
// Success Message
// ============================================================================

/// A success message box
#[component]
pub fn SuccessMessage(
    /// Success message text
    message: &'static str,
) -> impl IntoView {
    view! {
        <div class="p-4 bg-success-subtle border border-success/30 rounded-lg text-success text-sm">
            {message}
        </div>
    }
}

// ============================================================================
// Save Button
// ============================================================================

/// A save button with loading state
#[component]
pub fn SaveButton(
    /// Click handler
    on_click: impl Fn() + 'static,
    /// Loading state
    #[prop(optional)]
    loading: Signal<bool>,
    /// Button text (default: "Save")
    #[prop(optional)]
    text: Option<&'static str>,
) -> impl IntoView {
    let text = text.unwrap_or("Save");

    view! {
        <button
            on:click=move |_| on_click()
            disabled=move || loading.get()
            class="px-4 py-2 bg-primary text-text-inverse rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
        >
            {move || if loading.get() {
                "Saving..."
            } else {
                text
            }}
        </button>
    }
}
