//! Pure decision logic for team-chat self-echo dedup.
//!
//! `teams.chat.send`'s response and the live `team.<id>.message` event it
//! triggers both carry the same server-issued `message_id` — the composer
//! must remember its own recent send ids so the live echo of a message this
//! viewer just sent does not double up as a second bubble. Keyed by
//! `message_id`, not "am I the author": a second browser tab for the same
//! human still needs to receive the echo and render it, so dedup cannot key
//! on identity.
//!
//! The remembered set is bounded (FIFO eviction) so a long-lived session
//! cannot grow it without bound.

use std::collections::VecDeque;

/// Record `id` as one of this viewer's own recently-sent team-chat message
/// ids, evicting the oldest entry once the bound is exceeded.
///
/// A no-op if `id` is already remembered (no duplicate entries, no needless
/// re-ordering — a message is sent once).
pub fn remember_own_message_id(ids: &mut VecDeque<String>, id: String, cap: usize) {
    if cap == 0 || ids.contains(&id) {
        return;
    }
    ids.push_back(id);
    while ids.len() > cap {
        ids.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_an_id() {
        let mut ids = VecDeque::new();
        remember_own_message_id(&mut ids, "m1".to_string(), 32);
        assert!(ids.contains(&"m1".to_string()));
    }

    #[test]
    fn evicts_the_oldest_once_over_the_bound() {
        let mut ids = VecDeque::new();
        for n in 0..40 {
            remember_own_message_id(&mut ids, format!("m{n}"), 32);
        }
        assert_eq!(ids.len(), 32, "bounded to the cap");
        assert!(!ids.contains(&"m0".to_string()), "oldest evicted first");
        assert!(ids.contains(&"m39".to_string()), "newest kept");
    }

    #[test]
    fn a_repeated_id_is_not_duplicated_or_reordered() {
        let mut ids = VecDeque::new();
        remember_own_message_id(&mut ids, "m1".to_string(), 32);
        remember_own_message_id(&mut ids, "m2".to_string(), 32);
        remember_own_message_id(&mut ids, "m1".to_string(), 32);
        assert_eq!(ids.len(), 2);
        // m1 stayed at the front — a repeat must not push m2 out on a bound
        // this small (it would if the repeat re-pushed m1 to the back).
        assert_eq!(ids.front(), Some(&"m1".to_string()));
    }

    #[test]
    fn cap_zero_never_remembers_anything() {
        let mut ids = VecDeque::new();
        remember_own_message_id(&mut ids, "m1".to_string(), 0);
        assert!(ids.is_empty());
    }
}
