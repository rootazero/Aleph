use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aleph_client::AlephClient;

pub(crate) const DISCONNECTED: &str = "the Gateway is not connected for voice playback";
pub(super) const BUSY: &str = "voice playback is already active or releasing";

pub(crate) const HIDDEN: &str = "voice playback surface is no longer visible in this epoch";

#[derive(Default)]
pub(crate) struct Admission(Mutex<State>);

struct State {
    live: Option<Live>,
    visible: bool,
    visibility_epoch: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            live: None,
            visible: true,
            visibility_epoch: 0,
        }
    }
}

struct Live {
    generation: u64,
    connection: Connection,
}

enum Connection {
    Local(Arc<AlephClient>),
    #[cfg(test)]
    Fake(Box<dyn Fn() -> bool + Send + Sync>),
}

impl Connection {
    fn with_ready<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        match self {
            Self::Local(client) => client.with_ready_local_connection(action),
            #[cfg(test)]
            Self::Fake(ready) => ready().then(action),
        }
    }
}

impl Admission {
    pub(crate) fn connected(&self, generation: u64, client: Arc<AlephClient>) {
        self.install(generation, Connection::Local(client));
    }

    #[cfg(test)]
    pub(super) fn connected_fake(
        &self,
        generation: u64,
        ready: impl Fn() -> bool + Send + Sync + 'static,
    ) {
        self.install(generation, Connection::Fake(Box::new(ready)));
    }

    fn install(&self, generation: u64, connection: Connection) {
        let replaced = self.0.lock().unwrap().live.replace(Live {
            generation,
            connection,
        });
        drop(replaced);
    }

    pub(crate) fn disconnected(&self) {
        let retired = self.0.lock().unwrap().live.take();
        drop(retired);
    }

    pub(crate) fn set_visible(&self, visible: bool) -> bool {
        let mut state = self.0.lock().unwrap();
        if state.visible == visible {
            return false;
        }
        state.visible = visible;
        state.visibility_epoch += 1;
        true
    }

    pub(crate) fn visibility(&self) -> (bool, u64) {
        let state = self.0.lock().unwrap();
        (state.visible, state.visibility_epoch)
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        slot: &'static Slot,
    ) -> Result<(Generation, Reservation), &'static str> {
        let state = self.0.lock().unwrap();
        let live = state.live.as_ref().ok_or(DISCONNECTED)?;
        if !state.visible {
            return Err(HIDDEN);
        }
        live.connection
            .with_ready(|| {
                slot.0
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| BUSY)?;
                Ok((
                    Generation {
                        admission: Arc::clone(self),
                        number: live.generation,
                        visibility_epoch: state.visibility_epoch,
                    },
                    Reservation {
                        slot,
                        acquired: Instant::now(),
                    },
                ))
            })
            .ok_or(DISCONNECTED)?
    }
}

#[derive(Clone)]
pub(super) struct Generation {
    admission: Arc<Admission>,
    number: u64,
    visibility_epoch: u64,
}

impl Generation {
    pub(super) fn check(&self) -> Result<(), &'static str> {
        self.with_ready(|| ())
    }

    // Order: host Link (if held) -> Admission -> client request state.
    // Only the short start call belongs here; no client, Link or engine calls.
    pub(super) fn with_ready<T>(&self, action: impl FnOnce() -> T) -> Result<T, &'static str> {
        let state = self.admission.0.lock().unwrap();
        let live = state
            .live
            .as_ref()
            .filter(|live| live.generation == self.number)
            .ok_or(DISCONNECTED)?;
        if !state.visible || state.visibility_epoch != self.visibility_epoch {
            return Err(HIDDEN);
        }
        live.connection.with_ready(action).ok_or(DISCONNECTED)
    }
}

pub(super) struct Slot(AtomicBool);

impl Slot {
    pub(super) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
}

pub(super) struct Reservation {
    slot: &'static Slot,
    pub(super) acquired: Instant,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let Slot(busy) = self.slot;
        busy.store(false, Ordering::Release);
    }
}
