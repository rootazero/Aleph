#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogState {
    Loading,
    Ready,
    Empty,
    Refused,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    Idle,
    Saving,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerRequest {
    pub session_key: Option<String>,
    pub generation: u64,
}

pub fn next_generation(current: u64) -> u64 {
    current.checked_add(1).expect("picker generation exhausted")
}

pub fn accepts_result(current: &PickerRequest, request: &PickerRequest) -> bool {
    current == request
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Ignored,
    Confirm,
    Commit(Option<String>),
    Persist {
        request: PickerRequest,
        tier: Option<String>,
    },
}

#[derive(Clone, Debug)]
struct PendingSelection {
    request: PickerRequest,
    tier: Option<String>,
    requires_confirmation: bool,
}

#[derive(Clone, Debug)]
pub struct PickerState {
    session_key: Option<String>,
    generation: u64,
    load_request: Option<PickerRequest>,
    pending: Option<PendingSelection>,
    pub catalog: CatalogState,
    pub save: SaveState,
    pub confirming: Option<String>,
}

impl PickerState {
    pub fn new(session_key: Option<String>) -> Self {
        Self {
            session_key,
            generation: 0,
            load_request: None,
            pending: None,
            catalog: CatalogState::Loading,
            save: SaveState::Idle,
            confirming: None,
        }
    }

    pub fn switch_session(&mut self, session_key: Option<String>) {
        if self.session_key == session_key {
            return;
        }
        self.session_key = session_key;
        self.generation = next_generation(self.generation);
        self.load_request = None;
        self.pending = None;
        self.catalog = CatalogState::Loading;
        self.save = SaveState::Idle;
        self.confirming = None;
    }

    fn request(&mut self) -> PickerRequest {
        self.generation = next_generation(self.generation);
        PickerRequest {
            session_key: self.session_key.clone(),
            generation: self.generation,
        }
    }

    pub fn begin_load(&mut self) -> PickerRequest {
        let request = self.request();
        self.load_request = Some(request.clone());
        self.catalog = CatalogState::Loading;
        self.confirming = None;
        request
    }

    pub fn finish_load(&mut self, request: &PickerRequest, catalog: CatalogState) -> bool {
        if !self
            .load_request
            .as_ref()
            .is_some_and(|current| accepts_result(current, request))
        {
            return false;
        }
        self.load_request = None;
        self.catalog = catalog;
        true
    }

    pub fn choose(&mut self, tier: Option<String>, requires_confirmation: bool) -> Selection {
        if self.save == SaveState::Saving
            || !(self.catalog == CatalogState::Ready
                || (self.catalog == CatalogState::Empty && tier.is_none()))
        {
            return Selection::Ignored;
        }
        if requires_confirmation && self.confirming.as_ref() != tier.as_ref() {
            self.confirming = tier;
            return Selection::Confirm;
        }
        self.confirming = None;
        self.pending = None;
        self.save = SaveState::Idle;
        if self.session_key.is_none() {
            return Selection::Commit(tier);
        }
        let request = self.request();
        self.pending = Some(PendingSelection {
            request: request.clone(),
            tier: tier.clone(),
            requires_confirmation,
        });
        self.save = SaveState::Saving;
        Selection::Persist { request, tier }
    }

    pub fn accepts_save(&self, request: &PickerRequest) -> bool {
        self.save == SaveState::Saving
            && self
                .pending
                .as_ref()
                .is_some_and(|pending| accepts_result(&pending.request, request))
    }

    /// Only this successful, current acknowledgement releases a value for the UI to commit.
    pub fn complete_save(
        &mut self,
        request: &PickerRequest,
        success: bool,
    ) -> Option<Option<String>> {
        if !self.accepts_save(request) {
            return None;
        }
        let pending = self.pending.as_ref()?;
        if !success {
            self.save = SaveState::Failed;
            self.confirming = None;
            return None;
        }
        let tier = pending.tier.clone();
        self.pending = None;
        self.save = SaveState::Idle;
        Some(tier)
    }

    pub fn retry(&mut self) -> Selection {
        if self.save != SaveState::Failed {
            return Selection::Ignored;
        }
        let Some(pending) = self.pending.take() else {
            return Selection::Ignored;
        };
        self.save = SaveState::Idle;
        self.confirming = None;
        // Retrying Full starts a fresh two-click choice, never a previously armed write.
        if pending.requires_confirmation {
            return Selection::Ignored;
        }
        self.choose(pending.tier, false)
    }

    pub fn close(&mut self) {
        self.confirming = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_failure_retains_failed_state_and_can_retry() {
        let mut state = PickerState::new(None);
        let first = state.begin_load();
        assert!(state.finish_load(&first, CatalogState::Failed));
        assert_eq!(state.catalog, CatalogState::Failed);
        let second = state.begin_load();
        assert!(!state.finish_load(&first, CatalogState::Ready));
        assert!(state.finish_load(&second, CatalogState::Ready));
        assert_eq!(state.catalog, CatalogState::Ready);
    }

    #[test]
    fn empty_catalogue_is_not_ready() {
        let mut state = ready(Some("a"));
        let request = state.begin_load();
        state.finish_load(&request, CatalogState::Empty);
        assert_eq!(state.choose(Some("full".into()), true), Selection::Ignored);
        assert!(matches!(
            state.choose(None, false),
            Selection::Persist { tier: None, .. }
        ));
    }

    #[test]
    fn save_failure_keeps_previous_override() {
        let mut state = ready(Some("a"));
        let Selection::Persist { request, .. } = state.choose(Some("ask".into()), false) else {
            panic!("must persist before committing");
        };
        assert_eq!(state.complete_save(&request, false), None);
        assert_eq!(state.save, SaveState::Failed);
        assert!(
            matches!(state.retry(), Selection::Persist { tier: Some(tier), .. } if tier == "ask")
        );
    }

    #[test]
    fn save_success_commits_new_override() {
        let mut state = ready(Some("a"));
        let Selection::Persist { request, .. } = state.choose(Some("ask".into()), false) else {
            panic!("must persist before committing");
        };
        assert_eq!(state.choose(None, false), Selection::Ignored);
        assert_eq!(
            state.complete_save(&request, true),
            Some(Some("ask".into()))
        );
        assert_eq!(state.complete_save(&request, true), None);
    }

    #[test]
    fn retrying_full_requires_fresh_confirmation() {
        let mut state = ready(Some("a"));
        assert_eq!(state.choose(Some("full".into()), true), Selection::Confirm);
        let Selection::Persist { request, .. } = state.choose(Some("full".into()), true) else {
            panic!("confirmed selection must persist");
        };
        state.complete_save(&request, false);
        assert_eq!(state.retry(), Selection::Ignored);
        assert_eq!(state.choose(Some("full".into()), true), Selection::Confirm);
        assert!(matches!(
            state.choose(Some("full".into()), true),
            Selection::Persist { .. }
        ));
    }

    #[test]
    fn session_switch_resets_pending_save_and_confirmation() {
        let mut state = ready(Some("a"));
        let Selection::Persist { request, .. } = state.choose(Some("ask".into()), false) else {
            panic!("must persist");
        };
        let load = state.begin_load();
        state.switch_session(Some("b".into()));
        assert_eq!(state.save, SaveState::Idle);
        assert_eq!(state.confirming, None);
        assert_eq!(state.complete_save(&request, true), None);
        assert!(!state.finish_load(&load, CatalogState::Ready));
        state.switch_session(Some("a".into()));
        assert_eq!(state.complete_save(&request, true), None);
    }

    #[test]
    fn closing_popover_clears_full_confirmation() {
        let mut state = ready(None);
        assert_eq!(state.choose(Some("full".into()), true), Selection::Confirm);
        state.close();
        assert_eq!(state.choose(Some("full".into()), true), Selection::Confirm);
        assert_eq!(
            state.choose(Some("full".into()), true),
            Selection::Commit(Some("full".into()))
        );
    }

    #[test]
    fn catalogue_refresh_does_not_cancel_an_inflight_save() {
        let mut state = ready(Some("a"));
        let Selection::Persist { request, .. } = state.choose(None, false) else {
            panic!("must persist")
        };
        state.begin_load();
        assert_eq!(state.complete_save(&request, true), Some(None));
    }

    fn ready(session: Option<&str>) -> PickerState {
        let mut state = PickerState::new(session.map(str::to_string));
        let request = state.begin_load();
        state.finish_load(&request, CatalogState::Ready);
        state
    }

    #[test]
    fn request_result_is_rejected_after_session_switch() {
        let current = PickerRequest {
            session_key: Some("session-b".to_string()),
            generation: 2,
        };
        let request = PickerRequest {
            session_key: Some("session-a".to_string()),
            generation: 1,
        };

        assert!(!accepts_result(&current, &request));
    }

    #[test]
    fn request_result_is_rejected_after_newer_generation() {
        let current = PickerRequest {
            session_key: Some("session-a".to_string()),
            generation: 2,
        };
        let request = PickerRequest {
            session_key: Some("session-a".to_string()),
            generation: 1,
        };

        assert!(!accepts_result(&current, &request));
    }

    #[test]
    fn request_result_is_accepted_for_current_session_and_generation() {
        let current = PickerRequest {
            session_key: Some("session-a".to_string()),
            generation: 3,
        };
        let request = current.clone();

        assert!(accepts_result(&current, &request));
        assert_eq!(next_generation(current.generation), 4);
    }

    #[test]
    fn catalog_state_distinguishes_empty_refused_and_failed() {
        assert_ne!(CatalogState::Empty, CatalogState::Refused);
        assert_ne!(CatalogState::Refused, CatalogState::Failed);
        assert_ne!(CatalogState::Empty, CatalogState::Failed);
    }

    #[test]
    fn existing_session_commit_requires_successful_save() {
        let mut state = ready(Some("a"));
        let Selection::Persist { request, .. } = state.choose(None, false) else {
            panic!("must persist")
        };
        assert!(state.accepts_save(&request));
        assert_eq!(state.complete_save(&request, false), None);
        assert!(!state.accepts_save(&request));
        assert_eq!(state.complete_save(&request, true), None);
    }

    #[test]
    fn new_session_selection_does_not_require_save() {
        let mut state = ready(None);
        assert_eq!(
            state.choose(Some("ask".into()), false),
            Selection::Commit(Some("ask".into()))
        );
        assert_eq!(state.choose(None, false), Selection::Commit(None));
    }

    #[test]
    fn session_switch_requires_full_confirmation_again() {
        let mut state = ready(Some("a"));
        assert_eq!(state.choose(Some("full".into()), true), Selection::Confirm);
        state.switch_session(Some("b".into()));
        let load = state.begin_load();
        state.finish_load(&load, CatalogState::Ready);
        assert_eq!(state.choose(Some("full".into()), true), Selection::Confirm);
    }

    #[test]
    fn failed_catalogue_never_releases_a_selection() {
        for catalog in [
            CatalogState::Loading,
            CatalogState::Refused,
            CatalogState::Failed,
        ] {
            let mut state = ready(None);
            let request = state.begin_load();
            state.finish_load(&request, catalog);
            assert_eq!(state.choose(Some("ask".into()), false), Selection::Ignored);
            assert_eq!(state.choose(None, false), Selection::Ignored);
        }
    }
}
