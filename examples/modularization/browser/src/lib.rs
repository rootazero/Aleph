//! Standalone API-only browser consumer with profile-scoped in-memory tabs.
//! The caller supplies an executor. Claims describe this mock's three methods.

use aleph_slot_api::browser::{
    validate_browser_claim, validate_browser_reply, validate_browser_request, BrowserCapabilities,
    BrowserContractError, BrowserError, BrowserFeature, BrowserManagerContract, BrowserOperation,
    BrowserReply, BrowserRequest, TabLine,
};
use aleph_slot_api::{async_trait, AlephError};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    next_id: u64,
    profiles: BTreeMap<String, Vec<TabLine>>,
}

/// A small browser-port implementation for consumers that need tab state.
/// All state belongs to this instance; profile names partition its tab rows.
#[derive(Default)]
pub struct InMemoryBrowser {
    state: Mutex<State>,
}

fn validation(message: &str) -> BrowserContractError {
    AlephError::Validation(message.into()).into()
}

#[async_trait]
impl BrowserManagerContract for InMemoryBrowser {
    fn capabilities(&self) -> BrowserCapabilities {
        BrowserCapabilities {
            supported: vec![
                BrowserFeature::OpenTab,
                BrowserFeature::Navigate,
                BrowserFeature::ListTabs,
            ],
        }
    }

    async fn execute(
        &self,
        request: &BrowserRequest,
    ) -> Result<BrowserReply, BrowserContractError> {
        validate_browser_claim(&[request.operation.feature()], &self.capabilities())?;
        validate_browser_request(request)?;

        // Keep the caller's request borrowed across an await, then read it.
        // No lock is held across the await; no runtime dependency is needed.
        std::future::ready(()).await;
        if request.profile.trim().is_empty() {
            return Err(validation("an in-memory browser profile must be nonempty"));
        }

        let reply = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| validation("in-memory browser state lock poisoned"))?;
            match &request.operation {
                BrowserOperation::OpenTab { url } => {
                    if url.trim().is_empty() {
                        return Err(validation("open_tab requires a nonempty URL"));
                    }
                    let next_id = state
                        .next_id
                        .checked_add(1)
                        .ok_or_else(|| validation("in-memory tab identifiers exhausted"))?;
                    let id = format!("memory-tab-{next_id}");
                    let tabs = state.profiles.entry(request.profile.clone()).or_default();
                    for tab in tabs.iter_mut() {
                        tab.selected = false;
                    }
                    tabs.push(TabLine {
                        id: id.clone(),
                        url: url.clone(),
                        selected: true,
                    });
                    state.next_id = next_id;
                    BrowserReply::OpenTab { value: id }
                }
                BrowserOperation::Navigate { tab_id, url } => {
                    if url.trim().is_empty() {
                        return Err(validation("navigate requires a nonempty URL"));
                    }
                    let tab = state
                        .profiles
                        .get_mut(&request.profile)
                        .and_then(|tabs| tabs.iter_mut().find(|tab| tab.id == *tab_id))
                        .ok_or_else(|| BrowserError::TabNotFound(tab_id.clone()))?;
                    tab.url = url.clone();
                    BrowserReply::Navigate { value: () }
                }
                BrowserOperation::ListTabs {} => BrowserReply::ListTabs {
                    value: state
                        .profiles
                        .get(&request.profile)
                        .cloned()
                        .unwrap_or_default(),
                },
                operation => return Err(BrowserContractError::Unsupported(operation.feature())),
            }
        };
        validate_browser_reply(&request.operation, &reply)?;
        Ok(reply)
    }
}

/// Demonstrate the port through a dyn consumer with stack-owned requests.
/// Returns the actual listing after changing a tab's URL and rejecting CloseTab.
/// This example does not supply an executor or mount any native browser adapter.
pub async fn borrowed_round_trip() -> Result<Vec<TabLine>, BrowserContractError> {
    let browser = InMemoryBrowser::default();
    let port: &dyn BrowserManagerContract = &browser;
    let profile = String::from("example");
    let open = BrowserRequest {
        profile: profile.clone(),
        operation: BrowserOperation::OpenTab {
            url: "https://example.test/old".into(),
        },
    };
    let pending_open = port.execute(&open);
    let opened = pending_open.await?;
    validate_browser_reply(&open.operation, &opened)?;
    let tab_id = match opened {
        BrowserReply::OpenTab { value } if !value.is_empty() => value,
        _ => {
            return Err(validation(
                "open_tab did not return a nonempty tab identifier",
            ))
        }
    };

    let changed_url = String::from("https://example.test/changed");
    let navigate = BrowserRequest {
        profile: profile.clone(),
        operation: BrowserOperation::Navigate {
            tab_id: tab_id.clone(),
            url: changed_url.clone(),
        },
    };
    let pending_navigation = port.execute(&navigate);
    let navigated = pending_navigation.await?;
    validate_browser_reply(&navigate.operation, &navigated)?;

    let close = BrowserRequest {
        profile: profile.clone(),
        operation: BrowserOperation::CloseTab {
            tab_id: tab_id.clone(),
        },
    };
    match port.execute(&close).await {
        Err(BrowserContractError::Unsupported(BrowserFeature::CloseTab)) => {}
        _ => {
            return Err(validation(
                "unclaimed close_tab must return typed Unsupported",
            ))
        }
    }

    let list = BrowserRequest {
        profile,
        operation: BrowserOperation::ListTabs {},
    };
    let pending_list = port.execute(&list);
    let listed = pending_list.await?;
    validate_browser_reply(&list.operation, &listed)?;
    match listed {
        BrowserReply::ListTabs { value }
            if value.len() == 1
                && value[0].id == tab_id
                && value[0].url == changed_url
                && value[0].selected =>
        {
            Ok(value)
        }
        _ => Err(validation(
            "list_tabs did not retain the navigated tab and changed URL",
        )),
    }
}
