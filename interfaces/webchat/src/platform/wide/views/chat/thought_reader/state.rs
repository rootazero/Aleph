use aleph_protocol::thought::{
    ThoughtBlockId, ThoughtTranslationFailure, ThoughtTranslationUpdate, TranslationEngine,
    TranslationTarget,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scope {
    pub session: String,
    pub epoch: u64,
    pub target: TranslationTarget,
}

#[derive(Debug, Clone)]
pub(super) struct Pending {
    pub scope: Scope,
    pub request_id: String,
    pub block: ThoughtBlockId,
    pub engine: TranslationEngine,
}

#[derive(Debug, Clone)]
pub(super) enum ResultText {
    Streaming(String),
    Complete(String),
    Failed(ThoughtTranslationFailure),
}

#[derive(Default)]
pub(super) struct ReaderState {
    pub scope: Option<Scope>,
    pub pending: Option<Pending>,
    pub text: HashMap<(ThoughtBlockId, TranslationEngine), ResultText>,
}

impl ReaderState {
    pub fn accept(&mut self, scope: &Scope, request_id: &str, update: ThoughtTranslationUpdate) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if &pending.scope != scope || pending.request_id != request_id {
            return;
        }
        let body = self
            .text
            .entry((pending.block.clone(), pending.engine))
            .or_insert_with(|| ResultText::Streaming(String::new()));
        match update {
            ThoughtTranslationUpdate::Delta { text } => {
                if let ResultText::Streaming(accumulated) = body {
                    if accumulated.len().saturating_add(text.len()) <= 32 * 1024 {
                        accumulated.push_str(&text);
                    }
                }
            }
            ThoughtTranslationUpdate::Completed { text } => {
                *body = if text.len() > 32 * 1024 {
                    ResultText::Failed(ThoughtTranslationFailure::TooLarge)
                } else if text.trim().is_empty() {
                    ResultText::Failed(ThoughtTranslationFailure::Failed)
                } else {
                    ResultText::Complete(text)
                };
                self.pending = None;
            }
            ThoughtTranslationUpdate::Failed { code } => {
                *body = ResultText::Failed(code);
                self.pending = None;
            }
        }
    }
}

pub(super) type BlockKey = (ThoughtBlockId, TranslationEngine);

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (ReaderState, Scope, BlockKey) {
        let scope = Scope {
            session: "alpha".into(),
            epoch: 4,
            target: TranslationTarget::En,
        };
        let block = ThoughtBlockId("block".into());
        let engine = TranslationEngine::Ai;
        let state = ReaderState {
            pending: Some(Pending {
                scope: scope.clone(),
                request_id: "test".into(),
                block: block.clone(),
                engine,
            }),
            ..Default::default()
        };
        (state, scope, (block, engine))
    }

    #[test]
    fn thought_late_chunk_cannot_cross_locale_or_session() {
        let (mut state, scope, block) = fixture();
        for other in [
            Scope {
                target: TranslationTarget::Zh,
                ..scope.clone()
            },
            Scope {
                session: "beta".into(),
                ..scope.clone()
            },
            Scope {
                epoch: 5,
                ..scope.clone()
            },
        ] {
            state.accept(
                &other,
                "test",
                ThoughtTranslationUpdate::Delta {
                    text: "stale".into(),
                },
            );
        }
        state.accept(
            &scope,
            "other",
            ThoughtTranslationUpdate::Delta {
                text: "foreign".into(),
            },
        );
        assert!(state.text.is_empty());
        state.accept(
            &scope,
            "test",
            ThoughtTranslationUpdate::Delta {
                text: "current".into(),
            },
        );
        assert!(matches!(&state.text[&block], ResultText::Streaming(text) if text == "current"));
    }

    #[test]
    fn thought_reader_keeps_original_on_error() {
        let (mut state, scope, block) = fixture();
        state.accept(
            &scope,
            "test",
            ThoughtTranslationUpdate::Delta {
                text: "partial".into(),
            },
        );
        state.accept(
            &scope,
            "test",
            ThoughtTranslationUpdate::Failed {
                code: ThoughtTranslationFailure::Denied,
            },
        );
        state.accept(
            &scope,
            "test",
            ThoughtTranslationUpdate::Completed {
                text: "late".into(),
            },
        );
        assert!(state.pending.is_none());
        assert!(matches!(
            state.text[&block],
            ResultText::Failed(ThoughtTranslationFailure::Denied)
        ));
    }
}
