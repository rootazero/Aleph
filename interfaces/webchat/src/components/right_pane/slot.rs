//! Pure visibility inputs and the object-safe slot rendering boundary.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use leptos::prelude::AnyView;

use crate::i18n::Locale;
use crate::state::layout::LayoutMode;

/// The fixed stage-one slot boundaries; this value cannot be widened at runtime.
pub const STAGE_ONE_TOPICS: [&str; 5] = [
    "subagent.tree",
    "artifacts.changed",
    "capabilities.changed",
    "browser.changed",
    "stream.approval.requested",
];

/// The closed set of topic subscription failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicError {
    UnknownTopic(String),
}

impl fmt::Display for TopicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTopic(topic) => write!(f, "unknown right-pane topic: {topic}"),
        }
    }
}

impl std::error::Error for TopicError {}

fn validate_topic(topic: &str) -> Result<&'static str, TopicError> {
    STAGE_ONE_TOPICS
        .into_iter()
        .find(|allowed| *allowed == topic)
        .ok_or_else(|| TopicError::UnknownTopic(topic.to_owned()))
}

/// An erased subscription contract token, with no receiver or live transport.
///
/// Later stages replace this token with the typed receiver adapter, owned and
/// cleaned up by the mounted Leptos owner. Success here only validates a topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicSubscription {
    topic: &'static str,
}

impl TopicSubscription {
    pub fn topic(&self) -> &'static str {
        self.topic
    }
}

/// Read-only, object-safe subscription boundary; no publish or mutation API.
///
/// Implementations must immediately reject topics outside `STAGE_ONE_TOPICS`.
/// Slot consumers use `SlotCtx::subscribe`, which also enforces that boundary
/// before dispatching to an injected adapter.
pub trait TopicBus: Send + Sync {
    fn subscribe(&self, topic: &str) -> Result<TopicSubscription, TopicError>;
}

/// Stage-one validation only; no DashboardState, stream.chat or event receiver.
#[derive(Default)]
pub struct StageOneTopicBus;

impl TopicBus for StageOneTopicBus {
    fn subscribe(&self, topic: &str) -> Result<TopicSubscription, TopicError> {
        validate_topic(topic).map(|topic| TopicSubscription { topic })
    }
}

/// Read-only service boundary reserved for later slot service adapters.
/// No backend operations are exposed by this stage-one marker.
pub trait ServiceHub: Send + Sync {}

/// Empty service implementation used by default snapshots and placeholders.
#[derive(Default)]
pub struct EmptyServiceHub;

impl ServiceHub for EmptyServiceHub {}

/// An owned, read-only snapshot supplied by the host for one visibility pass.
///
/// The host must advance `visible_version` whenever an input changes. These
/// values are projections of existing state, never another state store. Run ids
/// use the existing Panel wire representation (`String`).
#[derive(Clone)]
pub struct SlotCtx {
    pub locale: Locale,
    pub mode: LayoutMode,
    pub is_connected: bool,
    pub active_run_id: Option<String>,
    pub visible_version: u64,
    pub user_opened: BTreeSet<&'static str>,
    pub user_closed: BTreeSet<&'static str>,
    pub capability_revision_loaded: Option<u64>,
    pub capability_fetch_attempted: bool,
    pub has_browser_binding: bool,
    pub has_pending_approvals: bool,
    /// Shared read-only adapter. Use `subscribe` to enforce the context boundary.
    pub topic_bus: Arc<dyn TopicBus>,
    pub services: Arc<dyn ServiceHub>,
}

impl Default for SlotCtx {
    fn default() -> Self {
        Self {
            locale: Locale::default(),
            mode: LayoutMode::default(),
            is_connected: false,
            active_run_id: None,
            visible_version: 0,
            user_opened: BTreeSet::new(),
            user_closed: BTreeSet::new(),
            capability_revision_loaded: None,
            capability_fetch_attempted: false,
            has_browser_binding: false,
            has_pending_approvals: false,
            topic_bus: Arc::new(StageOneTopicBus),
            services: Arc::new(EmptyServiceHub),
        }
    }
}

impl SlotCtx {
    /// Reject unknown topics synchronously, before invoking the read-only adapter.
    pub fn subscribe(&self, topic: &str) -> Result<TopicSubscription, TopicError> {
        validate_topic(topic)?;
        self.topic_bus.subscribe(topic)
    }

    /// User-open wins over user-close, which wins over the slot's default.
    pub fn effective_open(&self, slot: &dyn RightPaneSlot) -> bool {
        if self.user_opened.contains(slot.id()) {
            return true;
        }
        if self.user_closed.contains(slot.id()) {
            return false;
        }
        slot.default_open()
    }
}

/// Rendering receives its own snapshot, separate from the pure visibility API.
///
/// Its snapshot shares the same read-only services and topic adapter, rather
/// than keeping a second set of handles. No live services are wired in Task 1.
#[derive(Clone, Default)]
pub struct SlotRenderCtx {
    pub snapshot: SlotCtx,
}

impl SlotRenderCtx {
    pub fn subscribe(&self, topic: &str) -> Result<TopicSubscription, TopicError> {
        self.snapshot.subscribe(topic)
    }

    pub fn services(&self) -> &dyn ServiceHub {
        self.snapshot.services.as_ref()
    }
}

/// A stable slot descriptor with no workspace, persistence or subscription cache.
///
/// `id`, `label`, `icon`, `order`, and `default_open` must be immutable. Labels
/// and optional icons are resource keys, not translated text. `visible` must be
/// pure. Rendering must work even for a hidden slot; effects and subscriptions
/// belong to the mounted Leptos owner and must end when that owner unmounts.
pub trait RightPaneSlot: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn icon(&self) -> Option<&'static str>;
    fn order(&self) -> u16;
    fn visible(&self, ctx: &SlotCtx) -> bool;
    fn default_open(&self) -> bool;
    fn render(&self, ctx: SlotRenderCtx) -> AnyView;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::right_pane::stage_one_registry;

    #[test]
    fn subscribe_unknown_topic_fails_closed() {
        let ctx = SlotCtx::default();
        let render = SlotRenderCtx::default();
        for topic in [
            "never.allowed",
            "stream.chat.*",
            "stream.chat.delta",
            "subagent.tree.child",
            "Subagent.tree",
            "browser.changed ",
            "",
        ] {
            let expected = Err(TopicError::UnknownTopic(topic.to_owned()));
            assert_eq!(ctx.topic_bus.subscribe(topic), expected);
            assert_eq!(ctx.subscribe(topic), expected);
            assert_eq!(render.subscribe(topic), expected);
        }
    }

    #[test]
    fn topic_bus_subscribe_whitelist_contains_all_five_slots() {
        let ctx = SlotCtx::default();
        let render = SlotRenderCtx {
            snapshot: ctx.clone(),
        };
        for topic in [
            "subagent.tree",
            "artifacts.changed",
            "capabilities.changed",
            "browser.changed",
            "stream.approval.requested",
        ] {
            for result in [
                ctx.topic_bus.subscribe(topic),
                ctx.subscribe(topic),
                render.subscribe(topic),
            ] {
                assert_eq!(result.unwrap().topic(), topic);
            }
        }
    }

    #[test]
    fn caller_mutation_cannot_expand_or_remove_allowed_topics() {
        let ctx = SlotCtx::default();
        let mut topics = STAGE_ONE_TOPICS;
        topics.fill("stream.chat.delta");
        assert_eq!(
            ctx.subscribe(topics[0]),
            Err(TopicError::UnknownTopic("stream.chat.delta".to_owned()))
        );
        assert_eq!(
            ctx.subscribe("browser.changed").unwrap().topic(),
            "browser.changed"
        );
        let mut requested = String::from("subagent.tree");
        let subscription = ctx.subscribe(&requested).unwrap();
        requested.clear();
        requested.push_str("stream.chat.delta");
        assert_eq!(subscription.topic(), "subagent.tree");
        assert_eq!(
            ctx.subscribe(&requested),
            Err(TopicError::UnknownTopic(requested.clone()))
        );
        assert!(ctx.subscribe("subagent.tree").is_ok());
    }

    #[test]
    fn context_rejects_unknown_topic_before_dispatching_to_injected_bus() {
        struct UndispatchedBus;
        impl TopicBus for UndispatchedBus {
            fn subscribe(&self, _topic: &str) -> Result<TopicSubscription, TopicError> {
                panic!("unknown topic reached the adapter");
            }
        }
        let render = SlotRenderCtx {
            snapshot: SlotCtx {
                topic_bus: Arc::new(UndispatchedBus),
                ..SlotCtx::default()
            },
        };
        assert_eq!(
            render.subscribe("stream.chat.delta"),
            Err(TopicError::UnknownTopic("stream.chat.delta".to_owned()))
        );
    }

    #[test]
    fn render_context_preserves_shared_read_only_handles() {
        struct TestServices;
        impl ServiceHub for TestServices {}
        let ctx = SlotCtx {
            services: Arc::new(TestServices),
            ..SlotCtx::default()
        };
        let render = SlotRenderCtx {
            snapshot: ctx.clone(),
        };
        assert!(Arc::ptr_eq(&ctx.services, &render.snapshot.services));
        assert!(std::ptr::eq(ctx.services.as_ref(), render.services()));
        assert!(Arc::ptr_eq(&ctx.topic_bus, &render.snapshot.topic_bus));
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SlotCtx>();
        assert_send_sync::<SlotRenderCtx>();
        assert_send_sync::<super::super::RightPaneRegistry>();
    }

    #[test]
    fn effective_open_three_way_decision() {
        let registry = stage_one_registry();
        let open = registry.get("rps.subagent_tree").unwrap();
        let closed = registry.get("rps.browser").unwrap();
        let mut ctx = SlotCtx::default();
        assert!(ctx.effective_open(open.as_ref()));
        assert!(!ctx.effective_open(closed.as_ref()));
        ctx.user_closed.insert(open.id());
        assert!(!ctx.effective_open(open.as_ref()));
        ctx.user_opened.insert(open.id());
        assert!(ctx.effective_open(open.as_ref()));
        ctx.user_opened.insert(closed.id());
        assert!(ctx.effective_open(closed.as_ref()));
    }

    #[test]
    fn placeholder_render_accepts_empty_context_even_when_hidden() {
        let registry = stage_one_registry();
        for id in [
            "rps.subagent_tree",
            "rps.artifacts",
            "rps.capabilities",
            "rps.browser",
            "rps.approvals",
        ] {
            let slot = registry.get(id).unwrap();
            let _view = slot.render(SlotRenderCtx::default());
        }
    }
}
