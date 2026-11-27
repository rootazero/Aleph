//! The bundled local server, started in the background so the window can
//! show progress, and a failure the user can retry instead of an exit.

use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ServerPhase {
    Starting,
    Ready,
    Failed(String),
}

/// Brings the local server up; blocks until it is ready or refused.
pub(crate) type Launcher = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

/// Stops the owned child and reports whether there was one.
pub(crate) type Stopper = Box<dyn Fn() -> Result<bool, String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct LocalService {
    launcher: Arc<Launcher>,
    stopper: Arc<Stopper>,
    phase: Arc<watch::Sender<ServerPhase>>,
    /// Whether an attempt is in flight. Held while the phase changes, so an
    /// attempt that settles cannot overwrite the start of a newer one.
    running: Arc<(Mutex<bool>, Condvar)>,
}

impl LocalService {
    pub(crate) fn new(launcher: Launcher, stopper: Stopper) -> Self {
        Self {
            launcher: Arc::new(launcher),
            stopper: Arc::new(stopper),
            phase: Arc::new(watch::Sender::new(ServerPhase::Starting)),
            running: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// The `aleph-server` next to this executable, as the packaged app
    /// ships it; a compatible server already listening is used as it is.
    pub(crate) fn bundled() -> Self {
        use aleph_desktop_host::local_server::{ensure_compatible_ready, BundledServer};

        let owned = Arc::new(Mutex::new(None::<BundledServer>));
        let child = owned.clone();
        Self::new(
            Box::new(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "could not initialize the local service runtime".to_owned())?
                    .block_on(ensure_compatible_ready(&mut child.lock().unwrap()))
            }),
            Box::new(move || {
                let mut owned = owned.lock().unwrap();
                let Some(server) = owned.as_mut() else {
                    return Ok(false);
                };
                server.stop()?;
                *owned = None;
                Ok(true)
            }),
        )
    }

    /// Waits for any in-flight startup before stopping the child. Call off the
    /// UI thread. A reused listener has no child handle and is left running.
    pub(crate) fn stop(&self) -> Result<bool, String> {
        self.stop_or_restart(false)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn restart_owned(&self) -> Result<bool, String> {
        self.stop_or_restart(true)
    }

    fn stop_or_restart(&self, restart: bool) -> Result<bool, String> {
        let (attempt, settled) = &*self.running;
        let active = attempt.lock().unwrap();
        let _active = settled.wait_while(active, |active| *active).unwrap();
        let stopped = (self.stopper)()?;
        if stopped && restart {
            self.phase.send_replace(ServerPhase::Starting);
            let result = (self.launcher)();
            self.publish_outcome(&result);
            result?;
        }
        Ok(stopped)
    }

    /// Starts an attempt in the background, unless one is in flight.
    pub(crate) fn start(&self) {
        let Ok(mut running) = self.running.0.try_lock() else {
            return;
        };
        if *running {
            return;
        }
        *running = true;
        self.phase.send_replace(ServerPhase::Starting);
        let service = self.clone();
        let attempt = std::thread::Builder::new()
            .name("aleph-local-service".into())
            .spawn(move || {
                let outcome = (service.launcher)();
                let (attempt, settled) = &*service.running;
                let mut active = attempt.lock().unwrap();
                service.publish_outcome(&outcome);
                *active = false;
                settled.notify_all();
            });
        if attempt.is_err() {
            *running = false;
            self.phase.send_replace(ServerPhase::Failed(
                "could not start the local service thread".into(),
            ));
        }
    }

    fn publish_outcome(&self, outcome: &Result<(), String>) {
        self.phase.send_replace(match outcome {
            Ok(()) => ServerPhase::Ready,
            Err(error) => ServerPhase::Failed(error.clone()),
        });
    }

    pub(crate) fn phase(&self) -> ServerPhase {
        self.phase.borrow().clone()
    }

    /// Every phase change from now on, starting with the current phase.
    pub(crate) fn subscribe(&self) -> watch::Receiver<ServerPhase> {
        self.phase.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::{LocalService, ServerPhase};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Waits until the attempt in flight settles.
    fn settled(service: &LocalService) -> ServerPhase {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let phase = service.phase();
            if phase != ServerPhase::Starting || Instant::now() > deadline {
                return phase;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A launcher that blocks until the test releases it, counting calls.
    fn gated(result: Result<(), String>) -> (LocalService, mpsc::Sender<()>, Arc<AtomicUsize>) {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let service = LocalService::new(
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = gate.lock().unwrap().recv();
                result.clone()
            }),
            Box::new(|| Ok(false)),
        );
        (service, release, calls)
    }

    #[test]
    fn phase_moves_starting_to_ready() {
        let (service, release, _) = gated(Ok(()));
        service.start();
        assert_eq!(service.phase(), ServerPhase::Starting);
        release.send(()).unwrap();
        assert_eq!(settled(&service), ServerPhase::Ready);
    }

    #[test]
    fn failure_is_reported_not_exited() {
        let service = LocalService::new(
            Box::new(|| Err("port 18790 busy".into())),
            Box::new(|| Ok(false)),
        );
        service.start();
        assert_eq!(
            settled(&service),
            ServerPhase::Failed("port 18790 busy".into())
        );
    }

    #[test]
    fn retry_calls_launcher_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let service = LocalService::new(
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Err("port 18790 busy".into())
            }),
            Box::new(|| Ok(false)),
        );
        service.start();
        assert!(matches!(settled(&service), ServerPhase::Failed(_)));
        service.start();
        assert!(matches!(settled(&service), ServerPhase::Failed(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn concurrent_start_runs_launcher_once() {
        let (service, release, calls) = gated(Ok(()));
        service.start();
        service.start();
        service.clone().start();
        release.send(()).unwrap();
        assert_eq!(settled(&service), ServerPhase::Ready);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reused_ready_listener_does_not_grant_stop() {
        let (service, release, _) = gated(Ok(()));
        service.start();
        release.send(()).unwrap();
        assert_eq!(settled(&service), ServerPhase::Ready);
        assert!(!service.stop().unwrap());
    }

    #[test]
    fn stop_waits_for_startup_and_propagates_stop_failure() {
        let (release, gate) = mpsc::channel();
        let gate = Mutex::new(gate);
        let service = LocalService::new(
            Box::new(move || {
                gate.lock()
                    .unwrap()
                    .recv()
                    .map_err(|error| error.to_string())
            }),
            Box::new(|| Err("shutdown failed".into())),
        );
        service.start();
        let (done, outcome) = mpsc::channel();
        std::thread::spawn(move || done.send(service.stop()).unwrap());
        assert!(outcome.recv_timeout(Duration::from_millis(30)).is_err());
        release.send(()).unwrap();
        assert_eq!(
            outcome.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err("shutdown failed".into())
        );
    }

    #[test]
    fn stop_runs_the_stopper_and_start_brings_it_back() {
        let stops = Arc::new(AtomicUsize::new(0));
        let counted = stops.clone();
        let service = LocalService::new(
            Box::new(|| Ok(())),
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            }),
        );
        service.start();
        assert_eq!(settled(&service), ServerPhase::Ready);
        assert!(service.stop().unwrap());
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        service.start();
        assert_eq!(settled(&service), ServerPhase::Ready);
    }
}
