//! One owned playback operation for the NSH Panel surface. The macOS adapter
//! serializes these transitions on the main queue; tests use a fake player.

pub(crate) mod admission;
mod inline;
#[cfg(all(feature = "tsx-panel", target_os = "macos"))]
pub(crate) mod macos;
#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use admission::{Generation, Reservation, BUSY};

const PLAYBACK_LEASE: Duration = Duration::from_secs(120);
const CANCELLED: &str = "voice playback cancelled";
const EXPIRED: &str = "voice playback deadline exceeded";
type Reply = Box<dyn FnOnce(Result<(), &'static str>)>;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Epoch(u64);

trait Player {
    fn start(&self) -> bool;
    fn stop(&self);
}

struct Playback<P> {
    visible: bool,
    surface: Option<u64>,
    last_epoch: u64,
    active: Option<Attempt<P>>,
    last_released: Option<(u64, String)>,
}

struct Attempt<P> {
    epoch: Epoch,
    surface: u64,
    id: String,
    play: Option<Reply>,
    reservation: Option<Reservation>,
    deadline: Instant,
    phase: Phase<P>,
}

enum Phase<P> {
    Decoding,
    // Keep the lease until the worker returns, even across a surface remount.
    CancellingDecode(Option<Reply>),
    Open { player: P, started: bool },
}

impl<P: Player> Default for Playback<P> {
    fn default() -> Self {
        Self {
            visible: true,
            surface: None,
            last_epoch: 0,
            active: None,
            last_released: None,
        }
    }
}

impl<P: Player> Playback<P> {
    fn mount(&mut self, surface: u64) {
        if self.surface != Some(surface) {
            self.last_released = None;
        }
        self.surface = Some(surface);
    }

    fn unmount(&mut self, surface: u64) {
        if self.surface == Some(surface) {
            self.surface = None;
            self.last_released = None;
            self.cancel(None);
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            self.cancel(None);
        }
    }

    fn connection_lost(&mut self, surface: u64) {
        if self.surface == Some(surface) {
            self.cancel(None);
        }
    }

    fn admit<T>(
        &mut self,
        surface: u64,
        id: String,
        play: Reply,
        input: T,
        generation: &Generation,
        reservation: Reservation,
    ) -> Option<(Epoch, Instant, T)> {
        if let Err(error) = generation.check() {
            drop(input);
            drop(reservation);
            play(Err(error));
            return None;
        }
        let deadline = reservation.acquired + PLAYBACK_LEASE;
        self.begin(surface, id, play, input, deadline, Some(reservation))
            .map(|(epoch, input)| (epoch, deadline, input))
    }

    fn begin<T>(
        &mut self,
        surface: u64,
        id: String,
        play: Reply,
        input: T,
        deadline: Instant,
        reservation: Option<Reservation>,
    ) -> Option<(Epoch, T)> {
        let error = if Instant::now() >= deadline {
            Some(EXPIRED)
        } else if self.surface != Some(surface) || !self.visible {
            Some("voice playback surface is not visible")
        } else if self.active.is_some() {
            Some(BUSY)
        } else {
            None
        };
        if let Some(error) = error {
            drop(input);
            drop(reservation);
            play(Err(error));
            return None;
        }
        self.last_epoch += 1;
        let epoch = Epoch(self.last_epoch);
        self.active = Some(Attempt {
            epoch,
            surface,
            id,
            play: Some(play),
            reservation,
            deadline,
            phase: Phase::Decoding,
        });
        Some((epoch, input))
    }

    fn stop(&mut self, surface: u64, id: &str, stop: Reply) {
        if self.surface == Some(surface) {
            if self
                .active
                .as_ref()
                .is_some_and(|op| op.surface == surface && op.id == id)
            {
                self.cancel(Some(stop));
                return;
            }
            if self
                .last_released
                .as_ref()
                .is_some_and(|(owner, released)| *owner == surface && released == id)
            {
                stop(Ok(()));
                return;
            }
        }
        stop(Err("voice playback ID is not active on this surface"));
    }

    fn open_current(
        &mut self,
        epoch: Epoch,
        generation: &Generation,
        open: impl FnOnce() -> Result<P, &'static str>,
    ) -> bool {
        self.opened(epoch, || generation.check().and_then(|()| open()))
    }

    fn opened(&mut self, epoch: Epoch, open: impl FnOnce() -> Result<P, &'static str>) -> bool {
        self.expire(epoch, Instant::now());
        let Some(op) = self.active.as_mut().filter(|op| op.epoch == epoch) else {
            return false;
        };
        match op.phase {
            Phase::Decoding => match open() {
                Ok(player) => {
                    op.phase = Phase::Open {
                        player,
                        started: false,
                    };
                    true
                }
                Err(error) => {
                    self.release(Err(error));
                    false
                }
            },
            Phase::CancellingDecode(_) => {
                // Drop the decode result (owned by `open`) before replying.
                drop(open);
                self.release(Err(CANCELLED));
                false
            }
            Phase::Open { .. } => false,
        }
    }

    fn start_current(&mut self, epoch: Epoch, generation: &Generation) {
        self.start_with(epoch, |player| generation.with_ready(|| player.start()));
    }

    #[cfg(test)]
    fn start(&mut self, epoch: Epoch) {
        self.start_with(epoch, |player| Ok(player.start()));
    }

    fn start_with(&mut self, epoch: Epoch, start: impl FnOnce(&P) -> Result<bool, &'static str>) {
        if self.expire(epoch, Instant::now()) {
            return;
        }
        if let Some(Attempt {
            phase: Phase::Open { player, started },
            ..
        }) = self.active.as_mut().filter(|op| op.epoch == epoch)
        {
            if !*started {
                // The start guard has been released before teardown or reply.
                match start(player) {
                    Ok(true) => *started = true,
                    Ok(false) => self.release(Err("native voice playback could not start")),
                    Err(error) => self.release(Err(error)),
                }
            }
        }
    }

    fn terminal(&mut self, epoch: Epoch, result: Result<(), &'static str>) {
        if self.expire(epoch, Instant::now()) {
            return;
        }
        if self.active.as_ref().is_some_and(|op| {
            op.epoch == epoch
                && matches!(op.phase, Phase::Open { started, .. } if started || result.is_err())
        }) {
            self.release(result);
        }
    }

    fn expire(&mut self, epoch: Epoch, now: Instant) -> bool {
        if !self
            .active
            .as_ref()
            .is_some_and(|op| op.epoch == epoch && now >= op.deadline)
        {
            return false;
        }
        self.cancel_with(None, EXPIRED);
        true
    }

    fn cancel(&mut self, stop: Option<Reply>) {
        self.cancel_with(stop, CANCELLED);
    }

    fn cancel_with(&mut self, stop: Option<Reply>, reason: &'static str) {
        let Some(op) = self.active.as_mut() else {
            return;
        };
        match &mut op.phase {
            Phase::Decoding => op.phase = Phase::CancellingDecode(stop),
            Phase::CancellingDecode(pending) => {
                if let Some(stop) = stop {
                    if pending.is_none() {
                        *pending = Some(stop);
                    } else {
                        stop(Err("voice playback is already releasing"));
                    }
                }
            }
            Phase::Open { .. } => {
                self.release(Err(reason));
                if let Some(stop) = stop {
                    stop(Ok(()));
                }
                return;
            }
        }
        // A timed-out caller is terminal, but the decoder still owns its
        // bytes until it returns. Keep the reservation until they are dropped.
        if reason == EXPIRED {
            if let Some(play) = op.play.take() {
                play(Err(reason));
            }
        }
    }

    fn release(&mut self, result: Result<(), &'static str>) {
        let Some(op) = self.active.take() else { return };
        let stop = match op.phase {
            Phase::Open { player, .. } => {
                player.stop();
                drop(player);
                None
            }
            Phase::CancellingDecode(stop) => stop,
            Phase::Decoding => None,
        };
        drop(op.reservation);
        if self.surface == Some(op.surface) {
            self.last_released = Some((op.surface, op.id));
        }
        if let Some(play) = op.play {
            play(result);
        }
        if let Some(stop) = stop {
            stop(Ok(()));
        }
    }
}
