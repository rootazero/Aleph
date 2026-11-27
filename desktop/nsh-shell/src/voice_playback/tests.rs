use super::*;
use admission::{DISCONNECTED, HIDDEN};
use std::{cell::RefCell, rc::Rc};

impl<P: Player> Playback<P> {
    fn begin_fixture(&mut self, surface: u64, id: String, play: Reply) -> Option<Epoch> {
        self.begin(surface, id, play, (), Instant::now() + PLAYBACK_LEASE, None)
            .map(|(epoch, ())| epoch)
    }
}

#[derive(Clone, Default)]
struct Trace(Rc<RefCell<Vec<&'static str>>>);

impl Trace {
    fn add(&self, step: &'static str) {
        self.0.borrow_mut().push(step);
    }

    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    fn reply(&self, ok: &'static str, err: &'static str) -> Reply {
        let trace = self.clone();
        Box::new(move |result| trace.add(if result.is_ok() { ok } else { err }))
    }

    fn open(&self) -> FakePlayer {
        self.add("open");
        FakePlayer(self.clone())
    }
}

struct FakeDecodedAudio(Trace);

impl Drop for FakeDecodedAudio {
    fn drop(&mut self) {
        self.0.add("drop decoded audio");
    }
}

struct FakePlayer(Trace);

impl Player for FakePlayer {
    fn start(&self) -> bool {
        self.0.add("start");
        true
    }

    fn stop(&self) {
        self.0.add("stop player");
    }
}

impl Drop for FakePlayer {
    fn drop(&mut self) {
        self.0.add("drop player");
    }
}

#[test]
fn playback_epoch_and_teardown_stop_the_actual_player() {
    let trace = Trace::default();
    let mut playback = Playback::default();
    playback.mount(1);
    let first = playback
        .begin_fixture(
            1,
            "attempt".into(),
            trace.reply("play ok", "play cancelled"),
        )
        .unwrap();
    playback.stop(
        1,
        "Attempt",
        trace.reply("wrong stop ok", "wrong stop rejected"),
    );
    assert_eq!(trace.take(), ["wrong stop rejected"]);
    playback.stop(1, "attempt", trace.reply("stop released", "stop rejected"));
    assert!(playback
        .begin_fixture(
            1,
            "next".into(),
            trace.reply("busy play ok", "busy play rejected")
        )
        .is_none());
    assert_eq!(trace.take(), ["busy play rejected"]);
    let decoded = FakeDecodedAudio(trace.clone());
    assert!(!playback.opened(first, move || {
        drop(decoded);
        panic!("cancelled decode must not open a player")
    }));
    assert_eq!(
        trace.take(),
        ["drop decoded audio", "play cancelled", "stop released"]
    );

    // Reuse the same caller ID to prove the stale completion is checked against
    // the actual operation epoch, not merely its caller ID.
    let second = playback
        .begin_fixture(
            1,
            "attempt".into(),
            trace.reply("play ok", "play cancelled"),
        )
        .unwrap();
    assert!(playback.opened(second, || Ok(trace.open())));
    playback.terminal(first, Ok(()));
    playback.terminal(second, Ok(())); // no successful terminal before start
    assert_eq!(trace.take(), ["open"]);
    playback.start(second);
    assert_eq!(trace.take(), ["start"]);
    playback.stop(1, "attempt", trace.reply("stop released", "stop rejected"));
    assert_eq!(
        trace.take(),
        [
            "stop player",
            "drop player",
            "play cancelled",
            "stop released"
        ]
    );

    let hidden = playback
        .begin_fixture(1, "hidden".into(), trace.reply("play ok", "play cancelled"))
        .unwrap();
    assert!(playback.opened(hidden, || Ok(trace.open())));
    playback.set_visible(false);
    playback.start(hidden); // queued start after close-to-hide must be harmless
    assert_eq!(
        trace.take(),
        ["open", "stop player", "drop player", "play cancelled"]
    );
    assert!(playback
        .begin_fixture(
            1,
            "hidden".into(),
            trace.reply("hidden play ok", "hidden play rejected")
        )
        .is_none());
    assert_eq!(trace.take(), ["hidden play rejected"]);
    playback.set_visible(true);

    let playing_at_hide = playback
        .begin_fixture(
            1,
            "playing".into(),
            trace.reply("play ok", "play cancelled"),
        )
        .unwrap();
    assert!(playback.opened(playing_at_hide, || Ok(trace.open())));
    playback.start(playing_at_hide);
    playback.set_visible(false);
    assert_eq!(
        trace.take(),
        [
            "open",
            "start",
            "stop player",
            "drop player",
            "play cancelled"
        ]
    );
    playback.set_visible(true);

    let closed = playback
        .begin_fixture(
            1,
            "attempt".into(),
            trace.reply("play ok", "play cancelled"),
        )
        .unwrap();
    assert!(playback.opened(closed, || Ok(trace.open())));
    playback.start(closed);
    playback.unmount(1);
    assert_eq!(
        trace.take(),
        [
            "open",
            "start",
            "stop player",
            "drop player",
            "play cancelled"
        ]
    );
    playback.mount(2);
    let final_attempt = playback
        .begin_fixture(
            2,
            "attempt".into(),
            trace.reply("play ok", "play cancelled"),
        )
        .unwrap();
    assert!(playback.opened(final_attempt, || Ok(trace.open())));
    playback.start(final_attempt);
    playback.terminal(closed, Ok(()));
    playback.connection_lost(1); // lost connection on a previous surface
    assert_eq!(trace.take(), ["open", "start"]);
    playback.terminal(final_attempt, Ok(()));
    assert_eq!(trace.take(), ["stop player", "drop player", "play ok"]);
}

#[test]
fn playback_unmount_waits_for_pending_decode_across_surface_replacement() {
    let trace = Trace::default();
    let mut playback = Playback::<FakePlayer>::default();
    playback.mount(1);
    let old = playback
        .begin_fixture(1, "old".into(), trace.reply("play ok", "play cancelled"))
        .unwrap();
    playback.unmount(1);
    playback.mount(2);
    playback.stop(
        2,
        "old",
        trace.reply("wrong stop ok", "wrong stop rejected"),
    );
    assert!(playback
        .begin_fixture(2, "new".into(), trace.reply("play ok", "play rejected"))
        .is_none());
    assert_eq!(trace.take(), ["wrong stop rejected", "play rejected"]);
    assert!(!playback.opened(old, || panic!("old surface must not open a player")));
    assert_eq!(trace.take(), ["play cancelled"]);
    assert!(playback
        .begin_fixture(1, "old".into(), trace.reply("play ok", "play rejected"))
        .is_none());
    assert_eq!(trace.take(), ["play rejected"]);
    let new = playback
        .begin_fixture(2, "new".into(), trace.reply("play ok", "play rejected"))
        .unwrap();
    assert!(playback.opened(new, || Ok(trace.open())));
    playback.start(new);
    playback.connection_lost(2);
    assert_eq!(
        trace.take(),
        [
            "open",
            "start",
            "stop player",
            "drop player",
            "play rejected"
        ]
    );
}

#[test]
fn playback_ingress_rejects_a_queued_old_generation_after_reconnect() {
    use super::admission::{Admission, Slot};
    use std::sync::Arc;

    static SLOT: Slot = Slot::new();
    let admission = Arc::new(Admission::default());
    let trace = Trace::default();
    let mut playback = Playback::<FakePlayer>::default();
    playback.mount(1);
    assert!(matches!(admission.reserve(&SLOT), Err(DISCONNECTED)));

    admission.connected_fake(1, || true);
    let (old, reservation) = admission.reserve(&SLOT).unwrap();
    let queued_audio = FakeDecodedAudio(trace.clone());
    assert!(matches!(admission.reserve(&SLOT), Err(BUSY)));
    admission.disconnected();
    assert!(matches!(admission.reserve(&SLOT), Err(DISCONNECTED)));
    admission.connected_fake(2, || true);
    assert!(matches!(admission.reserve(&SLOT), Err(BUSY)));
    assert!(playback
        .admit(
            1,
            "old".into(),
            trace.reply("unexpected play", "stale play rejected"),
            queued_audio,
            &old,
            reservation
        )
        .is_none());
    assert_eq!(trace.take(), ["drop decoded audio", "stale play rejected"]);
    assert!(admission.reserve(&SLOT).is_ok());
}

#[test]
fn playback_ingress_holds_one_reservation_through_decoder_and_player_disposal() {
    use super::admission::{Admission, Slot};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    static SLOT: Slot = Slot::new();
    let admission = Arc::new(Admission::default());
    let trace = Trace::default();
    let mut playback = Playback::default();
    playback.mount(1);
    admission.connected_fake(1, || true);
    let (old, reservation) = admission.reserve(&SLOT).unwrap();
    let (epoch, _, decoded) = playback
        .admit(
            1,
            "old".into(),
            trace.reply("unexpected play", "play cancelled"),
            FakeDecodedAudio(trace.clone()),
            &old,
            reservation,
        )
        .unwrap();
    playback.stop(
        1,
        "old",
        trace.reply("stop released", "unexpected stop rejection"),
    );
    assert!(trace.take().is_empty());
    admission.disconnected();
    admission.connected_fake(2, || true);
    assert!(matches!(admission.reserve(&SLOT), Err(BUSY)));
    assert!(!playback.open_current(epoch, &old, move || {
        drop(decoded);
        panic!("a cancelled decoder must not open a player");
    }));
    assert_eq!(
        trace.take(),
        ["drop decoded audio", "play cancelled", "stop released"]
    );

    let (next, reservation) = admission.reserve(&SLOT).unwrap();
    let (epoch, _, ()) = playback
        .admit(
            1,
            "next".into(),
            trace.reply("unexpected play", "stale decode rejected"),
            (),
            &next,
            reservation,
        )
        .unwrap();
    admission.disconnected();
    // A decoder completion can race ahead of the queued teardown callback.
    assert!(!playback.open_current(epoch, &next, || panic!(
        "a disconnected decoder must not open a player"
    )));
    assert_eq!(trace.take(), ["stale decode rejected"]);

    let socket_live = Arc::new(AtomicBool::new(true));
    let live = socket_live.clone();
    admission.connected_fake(3, move || live.load(Ordering::Acquire));
    let (current, reservation) = admission.reserve(&SLOT).unwrap();
    let (epoch, _, ()) = playback
        .admit(
            1,
            "current".into(),
            trace.reply("unexpected play", "stale start rejected"),
            (),
            &current,
            reservation,
        )
        .unwrap();
    assert!(playback.open_current(epoch, &current, || Ok(trace.open())));
    assert!(matches!(admission.reserve(&SLOT), Err(BUSY)));
    socket_live.store(false, Ordering::Release);
    assert!(matches!(admission.reserve(&SLOT), Err(DISCONNECTED)));
    playback.start_current(epoch, &current);
    assert_eq!(
        trace.take(),
        ["open", "stop player", "drop player", "stale start rejected"]
    );
    admission.disconnected();
    admission.connected_fake(4, || true);
    assert!(admission.reserve(&SLOT).is_ok());
}

#[test]
fn playback_terminal_lease_stops_player_without_delegate_and_ignores_stale_timer() {
    let trace = Trace::default();
    let mut playback = Playback::default();
    playback.mount(1);
    let timed_out = trace.clone();
    let epoch = playback
        .begin_fixture(
            1,
            "same-id".into(),
            Box::new(move |result| {
                assert_eq!(result, Err(EXPIRED));
                timed_out.add("deadline failure");
            }),
        )
        .unwrap();
    assert!(playback.opened(epoch, || Ok(trace.open())));
    playback.start(epoch);
    let now = Instant::now();
    let deadline = now + Duration::from_millis(5);
    playback.active.as_mut().unwrap().deadline = deadline;
    assert!(!playback.expire(epoch, now));
    assert_eq!(trace.take(), ["open", "start"]);
    // No delegate callback at all: the timer must release the real player path.
    assert!(playback.expire(epoch, deadline));
    assert_eq!(
        trace.take(),
        ["stop player", "drop player", "deadline failure"]
    );

    let next = playback
        .begin_fixture(1, "same-id".into(), trace.reply("play ok", "play failed"))
        .unwrap();
    assert!(playback.opened(next, || Ok(trace.open())));
    playback.start(next);
    assert!(!playback.expire(epoch, deadline + PLAYBACK_LEASE));
    playback.terminal(epoch, Ok(()));
    assert_eq!(trace.take(), ["open", "start"]);
    playback.terminal(next, Ok(()));
    assert_eq!(trace.take(), ["stop player", "drop player", "play ok"]);
}

#[test]
fn playback_lease_counts_queue_time_and_retains_timed_out_decoder_reservation() {
    use super::admission::{Admission, Slot};
    use std::sync::Arc;

    static SLOT: Slot = Slot::new();
    let trace = Trace::default();
    let admission = Arc::new(Admission::default());
    admission.connected_fake(1, || true);
    let mut playback = Playback::<FakePlayer>::default();
    playback.mount(1);

    let (generation, mut reservation) = admission.reserve(&SLOT).unwrap();
    reservation.acquired -= PLAYBACK_LEASE;
    assert!(playback
        .admit(
            1,
            "queued".into(),
            Box::new(|result| assert_eq!(result, Err(EXPIRED))),
            FakeDecodedAudio(trace.clone()),
            &generation,
            reservation
        )
        .is_none());
    assert_eq!(trace.take(), ["drop decoded audio"]);

    let (generation, mut reservation) = admission.reserve(&SLOT).unwrap();
    reservation.acquired -= PLAYBACK_LEASE - Duration::from_secs(1);
    let expected_deadline = reservation.acquired + PLAYBACK_LEASE;
    let timed_out = trace.clone();
    let (epoch, deadline, decoded) = playback
        .admit(
            1,
            "decode".into(),
            Box::new(move |result| {
                assert_eq!(result, Err(EXPIRED));
                timed_out.add("deadline failure");
            }),
            FakeDecodedAudio(trace.clone()),
            &generation,
            reservation,
        )
        .unwrap();
    assert_eq!(deadline, expected_deadline);
    assert!(playback.expire(epoch, deadline));
    assert_eq!(trace.take(), ["deadline failure"]);
    assert!(matches!(admission.reserve(&SLOT), Err(BUSY)));
    assert!(!playback.opened(epoch, move || {
        drop(decoded);
        panic!("timed-out decode must not open a player")
    }));
    assert_eq!(trace.take(), ["drop decoded audio"]);
    assert!(admission.reserve(&SLOT).is_ok());
}

#[test]
fn playback_stop_confirms_only_latest_released_id_on_own_surface() {
    let trace = Trace::default();
    let mut playback = Playback::default();
    playback.mount(1);
    let epoch = playback
        .begin_fixture(1, "first".into(), trace.reply("play ok", "play failed"))
        .unwrap();
    assert!(playback.opened(epoch, || Ok(trace.open())));
    playback.start(epoch);
    let deadline = playback.active.as_ref().unwrap().deadline;
    assert!(playback.expire(epoch, deadline));
    playback.stop(1, "first", trace.reply("released", "not released"));
    playback.stop(
        2,
        "first",
        trace.reply("wrong surface released", "wrong surface rejected"),
    );
    assert_eq!(
        trace.take(),
        [
            "open",
            "start",
            "stop player",
            "drop player",
            "play failed",
            "released",
            "wrong surface rejected"
        ]
    );

    // Reusing the last ID must not mask a new decoder that still owns bytes.
    let epoch = playback
        .begin_fixture(1, "first".into(), trace.reply("play ok", "play failed"))
        .unwrap();
    playback.stop(
        1,
        "first",
        trace.reply("decode released", "decode not released"),
    );
    playback.stop(
        1,
        "first",
        trace.reply("duplicate released", "duplicate pending rejected"),
    );
    assert_eq!(trace.take(), ["duplicate pending rejected"]);
    let decoded = FakeDecodedAudio(trace.clone());
    assert!(!playback.opened(epoch, move || {
        drop(decoded);
        panic!("cancelled decode must not open")
    }));
    assert_eq!(
        trace.take(),
        ["drop decoded audio", "play failed", "decode released"]
    );

    let epoch = playback
        .begin_fixture(1, "latest".into(), trace.reply("play ok", "play failed"))
        .unwrap();
    assert!(playback.opened(epoch, || Ok(trace.open())));
    playback.start(epoch);
    playback.terminal(epoch, Ok(()));
    playback.stop(1, "first", trace.reply("older released", "older rejected"));
    playback.stop(
        1,
        "latest",
        trace.reply("latest released", "latest rejected"),
    );
    playback.unmount(1);
    playback.mount(2);
    playback.stop(
        2,
        "latest",
        trace.reply("remount released", "remount rejected"),
    );
    assert_eq!(
        trace.take(),
        [
            "open",
            "start",
            "stop player",
            "drop player",
            "play ok",
            "older rejected",
            "latest released",
            "remount rejected"
        ]
    );
}

#[test]
fn playback_queued_start_cannot_cross_the_panel_host_hide_show_edge() {
    use crate::panel_host::{Handle, Target};
    use admission::{Admission, Slot};
    use std::sync::Arc;

    static SLOT: Slot = Slot::new();
    let gate = Arc::new(Admission::default());
    let host = Handle::start(
        nsh_engine::HostChannel::default(),
        Target::Unavailable("the playback fixture installs a fake connection"),
        true,
        gate.clone(),
        || {},
    )
    .unwrap();
    gate.connected_fake(1, || true);
    let trace = Trace::default();
    let mut playback = Playback::<FakePlayer>::default();
    playback.mount(1);
    let (generation, reservation) = gate.reserve(&SLOT).unwrap();
    let (epoch, _, ()) = playback
        .admit(
            1,
            "queued".into(),
            trace.reply("play ok", "play failed"),
            (),
            &generation,
            reservation,
        )
        .unwrap();
    assert!(playback.open_current(epoch, &generation, || Ok(trace.open())));
    let queued_before_hide =
        move |playback: &mut Playback<FakePlayer>| playback.start_current(epoch, &generation);

    // Window wiring synchronously invokes this Handle edge before it queues
    // main-thread cleanup. The previously queued start runs first in this fixture.
    host.set_visible(false);
    assert!(matches!(gate.reserve(&SLOT), Err(HIDDEN)));
    host.set_visible(true);
    queued_before_hide(&mut playback);
    assert_eq!(
        trace.take(),
        ["open", "stop player", "drop player", "play failed"]
    );

    // Reopening admits a fresh token, not a permanently closed curtain.
    let (generation, reservation) = gate.reserve(&SLOT).unwrap();
    let (epoch, _, ()) = playback
        .admit(
            1,
            "fresh".into(),
            trace.reply("play ok", "play failed"),
            (),
            &generation,
            reservation,
        )
        .unwrap();
    assert!(playback.open_current(epoch, &generation, || Ok(trace.open())));
    playback.start_current(epoch, &generation);
    playback.terminal(epoch, Ok(()));
    assert_eq!(
        trace.take(),
        ["open", "start", "stop player", "drop player", "play ok"]
    );
}
