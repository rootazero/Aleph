mod state;

use crate::api::{thought::ThoughtApi, trace::TraceApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, Locale};
use aleph_protocol::thought::{
    ThoughtBlockRef, ThoughtTranslationEngines, ThoughtTranslationEvent, ThoughtTranslationFailure,
    ThoughtTranslationRequest, ThoughtTranslationUpdate, TranslationEngine, TranslationTarget,
    THOUGHT_TRANSLATION_TOPIC,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use state::{Pending, ReaderState, ResultText, Scope};
use std::sync::atomic::{AtomicU64, Ordering};

use super::state::ChatState;

const MODE_KEY: &str = "aleph-thought-mode";
const ENGINE_KEY: &str = "aleph-thought-engine";
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadingMode {
    Original,
    Translated,
    Bilingual,
}

impl ReadingMode {
    fn storage(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Translated => "translated",
            Self::Bilingual => "bilingual",
        }
    }

    fn from_storage(value: &str) -> Self {
        match value {
            "translated" => Self::Translated,
            "bilingual" => Self::Bilingual,
            _ => Self::Original,
        }
    }
}

#[derive(Clone, Copy)]
struct Reader {
    dash: DashboardState,
    scope: Signal<Option<Scope>>,
    state: RwSignal<ReaderState>,
    mode: RwSignal<ReadingMode>,
    engine: RwSignal<TranslationEngine>,
}

pub(super) fn provide_thought_reader(chat: ChatState, dash: DashboardState) {
    let i18n = use_i18n();
    let scope = Signal::derive(move || {
        let session = chat.session_key.try_get().flatten()?;
        let epoch = dash.connection_epoch.try_get()?;
        let target = match i18n.get_locale() {
            Locale::en => TranslationTarget::En,
            Locale::zh => TranslationTarget::Zh,
        };
        Some(Scope {
            session,
            epoch,
            target,
        })
    });
    let reader = Reader {
        dash,
        scope,
        state: RwSignal::new(ReaderState::default()),
        mode: RwSignal::new(ReadingMode::from_storage(
            crate::appearance::read_key(MODE_KEY)
                .as_deref()
                .unwrap_or_default(),
        )),
        engine: RwSignal::new(
            if crate::appearance::read_key(ENGINE_KEY).as_deref() == Some("ai") {
                TranslationEngine::Ai
            } else {
                TranslationEngine::Machine
            },
        ),
    };
    provide_context(reader);
    let subscriber = dash.subscribe_events(move |event| {
        if event.topic != THOUGHT_TRANSLATION_TOPIC {
            return;
        }
        let Ok(event) = serde_json::from_value::<ThoughtTranslationEvent>(event.data) else {
            return;
        };
        let Some(scope) = reader.scope.try_get_untracked().flatten() else {
            return;
        };
        let _ = reader
            .state
            .try_update(|state| state.accept(&scope, &event.request_id, event.update));
    });
    Effect::new(move |_| {
        let scope = reader.scope.get();
        let pending = reader
            .state
            .try_update(|state| {
                if state.scope == scope {
                    return None;
                }
                let pending = state.pending.take();
                *state = ReaderState {
                    scope,
                    ..Default::default()
                };
                pending
            })
            .flatten();
        if let Some(pending) = pending {
            reader.cancel(pending);
        }
    });
    on_cleanup(move || {
        dash.unsubscribe_events(subscriber);
        if let Some(pending) = reader
            .state
            .try_update(|state| state.pending.take())
            .flatten()
        {
            reader.cancel(pending);
        }
    });
}

impl Reader {
    fn begin(
        self,
        block: ThoughtBlockRef,
        engine: TranslationEngine,
        expected_destination: String,
    ) {
        let Some(scope) = self.scope.try_get_untracked().flatten() else {
            return;
        };
        if block.session_key != scope.session {
            return;
        }
        let request_id = format!(
            "panel-thought-{}-{}",
            scope.epoch,
            REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let request = ThoughtTranslationRequest {
            block,
            engine,
            target: scope.target,
            expected_destination,
            request_id: request_id.clone(),
        };
        let Some(true) = self.state.try_update(|state| {
            if state.scope.as_ref() != Some(&scope) || state.pending.is_some() {
                return false;
            }
            state.text.remove(&(request.block.block_id.clone(), engine));
            state.pending = Some(Pending {
                scope: scope.clone(),
                request_id: request_id.clone(),
                block: request.block.block_id.clone(),
                engine,
            });
            true
        }) else {
            return;
        };
        spawn_local(async move {
            if self.scope.try_get_untracked().flatten().as_ref() != Some(&scope)
                || !self
                    .state
                    .try_with_untracked(|state| {
                        state
                            .pending
                            .as_ref()
                            .is_some_and(|pending| pending.request_id == request_id)
                    })
                    .unwrap_or(false)
            {
                return;
            }
            if ThoughtApi::start(self.dash, scope.epoch, request)
                .await
                .is_err()
            {
                let _ = self.state.try_update(|state| {
                    state.accept(
                        &scope,
                        &request_id,
                        ThoughtTranslationUpdate::Failed {
                            code: ThoughtTranslationFailure::Failed,
                        },
                    )
                });
            }
        });
    }

    fn cancel(self, pending: Pending) {
        spawn_local(ThoughtApi::cancel(
            self.dash,
            pending.scope.epoch,
            pending.request_id,
        ));
    }

    fn cancel_block(self, key: &state::BlockKey) {
        let pending = self
            .state
            .try_update(|state| {
                if state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.block == key.0 && pending.engine == key.1)
                {
                    state.text.remove(key);
                    state.pending.take()
                } else {
                    None
                }
            })
            .flatten();
        if let Some(pending) = pending {
            self.cancel(pending);
        }
    }
}

#[derive(Clone, Deserialize)]
struct ThoughtBlock {
    thought_block_ref: ThoughtBlockRef,
    text: String,
}

#[derive(Clone, Default)]
enum Blocks {
    #[default]
    Closed,
    Loading,
    Ready(Vec<ThoughtBlock>, ThoughtTranslationEngines),
}

#[component]
pub(super) fn ThoughtReader(run_id: String) -> impl IntoView {
    let Some(reader) = use_context::<Reader>() else {
        return ().into_any();
    };
    let i18n = use_i18n();
    let display = RwSignal::new(Blocks::Closed);
    let generation = RwSignal::new(0_u64);
    let run_id = StoredValue::new(run_id);
    let invalidate = move || {
        generation.update(|generation| *generation = generation.wrapping_add(1));
        display.set(Blocks::Closed);
    };
    // The original remains readable across locale changes.
    let trace_scope =
        Memo::new(move |_| reader.scope.get().map(|scope| (scope.session, scope.epoch)));
    Effect::new(move |_| {
        let _ = trace_scope.get();
        invalidate();
    });
    let load = move || {
        let Some((session, epoch)) = trace_scope.get_untracked() else {
            return;
        };
        generation.update(|generation| *generation = generation.wrapping_add(1));
        let request_generation = generation.get_untracked();
        let run_id = run_id.get_value();
        display.set(Blocks::Loading);
        spawn_local(async move {
            let runs = TraceApi::by_runs(&reader.dash, &session, vec![run_id.clone()]).await;
            let blocks = runs
                .ok()
                .and_then(|mut runs| runs.remove(&run_id))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|row| serde_json::from_value::<ThoughtBlock>(row).ok())
                .filter(|block| {
                    block.thought_block_ref.session_key == session
                        && block.thought_block_ref.run_id == run_id
                        && !block.text.trim().is_empty()
                })
                .collect::<Vec<_>>();
            let engines = if blocks.is_empty() {
                Default::default()
            } else {
                ThoughtApi::engines(reader.dash).await
            };
            if generation.try_get_untracked() == Some(request_generation)
                && trace_scope.try_get_untracked().flatten() == Some((session, epoch))
            {
                display.set(Blocks::Ready(blocks, engines));
            }
        });
    };
    view! {
        <section class="panel-thought-reader">
            <button type="button" class="panel-thought-button"
                aria-expanded=move || (!matches!(display.get(), Blocks::Closed)).to_string()
                on:click=move |_| if matches!(display.get_untracked(), Blocks::Closed) { load(); } else { invalidate(); }>
                {t!(i18n, chat.thought_blocks)}
            </button>
            {move || match display.get() {
                Blocks::Closed => ().into_any(),
                Blocks::Loading => view! { <p role="status">{t!(i18n, chat.thought_loading)}</p> }.into_any(),
                Blocks::Ready(blocks, _) if blocks.is_empty() => view! {
                    <p role="status">{t!(i18n, chat.thought_empty)}</p>
                    <button type="button" class="panel-thought-button" on:click=move |_| load()>{t!(i18n, chat.thought_refresh)}</button>
                }.into_any(),
                Blocks::Ready(blocks, engines) => view! {
                    <div class="panel-thought-tools">
                        <label>{t!(i18n, chat.thought_reading)}
                            <select prop:value=move || reader.mode.get().storage() on:change=move |event| {
                                let mode = ReadingMode::from_storage(&event_target_value(&event));
                                reader.mode.set(mode);
                                crate::appearance::persist(MODE_KEY, Some(mode.storage()));
                            }>
                                <option value="original">{t!(i18n, chat.thought_original)}</option>
                                <option value="translated">{t!(i18n, chat.thought_translated)}</option>
                                <option value="bilingual">{t!(i18n, chat.thought_bilingual)}</option>
                            </select>
                        </label>
                        <label>{t!(i18n, chat.thought_engine)}
                            <select prop:value=move || engine_id(reader.engine.get()) on:change=move |event| {
                                let engine = if event_target_value(&event) == "ai" { TranslationEngine::Ai } else { TranslationEngine::Machine };
                                reader.engine.set(engine);
                                crate::appearance::persist(ENGINE_KEY, Some(engine_id(engine)));
                            }>
                                <option value="machine">{t!(i18n, chat.thought_machine)}</option>
                                <option value="ai">{t!(i18n, chat.thought_ai)}</option>
                            </select>
                        </label>
                    </div>
                    <For each=move || blocks.clone() key=|block| block.thought_block_ref.block_id.clone()
                        children=move |block| view! { <BlockView block=block engines=engines.clone() /> } />
                }.into_any(),
            }}
        </section>
    }.into_any()
}

fn engine_id(engine: TranslationEngine) -> &'static str {
    match engine {
        TranslationEngine::Machine => "machine",
        TranslationEngine::Ai => "ai",
    }
}

#[component]
fn BlockView(block: ThoughtBlock, engines: ThoughtTranslationEngines) -> impl IntoView {
    let reader = expect_context::<Reader>();
    let i18n = use_i18n();
    let block = StoredValue::new(block);
    let engines = StoredValue::new(engines);
    let key = move || {
        (
            block.with_value(|block| block.thought_block_ref.block_id.clone()),
            reader.engine.get(),
        )
    };
    let text = move || {
        let key = key();
        let scope = reader.scope.get();
        reader.state.with(|state| {
            if state.scope == scope {
                state.text.get(&key).cloned()
            } else {
                None
            }
        })
    };
    let destination = move || {
        engines.with_value(|engines| match reader.engine.get() {
            TranslationEngine::Machine => engines.machine.clone(),
            TranslationEngine::Ai => engines.ai.clone(),
        })
    };
    let block_pending = move || {
        reader
            .state
            .with(|state| state.pending.clone())
            .filter(|pending| {
                pending.block == block.with_value(|block| block.thought_block_ref.block_id.clone())
            })
    };
    view! {
        <article class="panel-thought-block">
            <div class="panel-thought-columns" class:panel-thought-bilingual=move || reader.mode.get() == ReadingMode::Bilingual>
                <Show when=move || reader.mode.get() != ReadingMode::Translated || !matches!(text(), Some(ResultText::Complete(_)))>
                    <div><h4>{t!(i18n, chat.thought_original)}</h4><p>{block.with_value(|block| block.text.clone())}</p></div>
                </Show>
                {move || match text() {
                    Some(ResultText::Streaming(text) | ResultText::Complete(text)) if reader.mode.get() != ReadingMode::Original => view! {
                        <div><h4>{t!(i18n, chat.thought_translated)}</h4><p>{text}</p></div>
                    }.into_any(),
                    _ => ().into_any(),
                }}
            </div>
            <div class="panel-thought-tools">
                {move || if let Some(pending) = block_pending() {
                    let key = (pending.block, pending.engine);
                    view! {
                        <span role="status">{t!(i18n, chat.thought_working)}</span>
                        <button type="button" class="panel-thought-button" on:click=move |_| reader.cancel_block(&key)>{t!(i18n, chat.thought_cancel)}</button>
                    }.into_any()
                } else if let Some(destination) = destination() {
                    let label = destination.clone();
                    let engine = reader.engine.get();
                    view! {
                        <span>{t!(i18n, chat.thought_destination, destination = label.clone())}</span>
                        <button type="button" class="panel-thought-button" disabled=move || reader.state.with(|s| s.pending.is_some())
                            on:click=move |_| {
                                if reader.mode.get_untracked() == ReadingMode::Original { reader.mode.set(ReadingMode::Bilingual); }
                                reader.begin(block.with_value(|block| block.thought_block_ref.clone()), engine, destination.clone());
                            }>{t!(i18n, chat.thought_translate)}</button>
                    }.into_any()
                } else { view! { <span>{t!(i18n, chat.thought_unavailable)}</span> }.into_any() }}
                {move || match text() {
                    Some(ResultText::Failed(code)) => view! {
                        <span role="status">{match code {
                            ThoughtTranslationFailure::Unavailable => t_string!(i18n, chat.thought_unavailable).to_string(),
                            ThoughtTranslationFailure::Denied => t_string!(i18n, chat.thought_denied).to_string(),
                            ThoughtTranslationFailure::Cancelled => t_string!(i18n, chat.thought_cancelled).to_string(),
                            ThoughtTranslationFailure::Timeout => t_string!(i18n, chat.thought_timeout).to_string(),
                            ThoughtTranslationFailure::TooLarge => t_string!(i18n, chat.thought_too_large).to_string(),
                            ThoughtTranslationFailure::Failed => t_string!(i18n, chat.thought_failed).to_string(),
                        }}</span>
                    }.into_any(),
                    _ => ().into_any(),
                }}
            </div>
        </article>
    }
}
