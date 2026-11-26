use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use dispatch2::{DispatchQueue, DispatchTime};
use nsh_engine::{HostCall, HostChannel};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, AnyThread, DefinedClass};
use objc2_avf_audio::{AVAudioPlayer, AVAudioPlayerDelegate};
use objc2_foundation::{NSData, NSError, NSObject, NSObjectProtocol, NSString};
use serde_json::Value;

use super::admission::{Admission, Generation, Slot};
use super::inline::{Mime, Play, Stop};
use super::{Epoch, Playback, Player, Reply};

const PLAY: &str = "aleph.voice.play";
const STOP: &str = "aleph.voice.stop";
// A surface can be replaced while its decoder is still finishing.
static PLAYBACK_SLOT: Slot = Slot::new();

thread_local! {
    // Retain an in-flight decode lease across surface destruction until its
    // result returns. Only the main queue can touch the AVAudioPlayer.
    static PLAYBACK: RefCell<Playback<NativePlayer>> = RefCell::default();
}

pub(crate) fn mount(surface: u64, channel: &HostChannel, admission: Arc<Admission>) {
    PLAYBACK.with_borrow_mut(|playback| playback.mount(surface));
    channel.handle(PLAY, move |call| play(surface, &admission, call));
    channel.handle(STOP, move |call| stop(surface, call));
}

pub(crate) fn unmount(surface: u64, channel: &HostChannel) {
    PLAYBACK.with_borrow_mut(|playback| playback.unmount(surface));
    channel.remove_handler(PLAY);
    channel.remove_handler(STOP);
}

pub(crate) fn set_visible(visible: bool) {
    on_main(move |playback| playback.set_visible(visible));
}

pub(crate) fn connection_lost(surface: u64) {
    on_main(move |playback| playback.connection_lost(surface));
}

fn on_main(f: impl FnOnce(&mut Playback<NativePlayer>) + Send + 'static) {
    DispatchQueue::main().exec_async(move || PLAYBACK.with_borrow_mut(f));
}

fn reply(call: HostCall) -> Reply {
    Box::new(move |result| match result {
        Ok(()) => call.resolve(Value::Null),
        Err(error) => call.reject(error),
    })
}

fn play(surface: u64, admission: &Arc<Admission>, mut call: HostCall) {
    if call.is_post() {
        return;
    }
    let (generation, reservation) = match admission.reserve(&PLAYBACK_SLOT) {
        Ok(reservation) => reservation,
        Err(error) => return call.reject(error),
    };
    let mut request = match Play::parse(call.take_data()) {
        Ok(request) => request,
        Err(error) => {
            drop(reservation);
            return call.reject(error);
        }
    };
    let id = std::mem::take(&mut request.playback_id);
    on_main(move |playback| {
        let Some((epoch, deadline, request)) =
            playback.admit(surface, id, reply(call), request, &generation, reservation)
        else {
            return;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timer = DispatchTime::try_from(remaining).ok().and_then(|when| {
            DispatchQueue::main()
                .after(when, move || {
                    PLAYBACK.with_borrow_mut(|playback| playback.expire(epoch, Instant::now()));
                })
                .ok()
        });
        if timer.is_none() {
            playback.opened(epoch, || {
                Err("voice playback deadline could not be scheduled")
            });
            return;
        }
        let worker = std::thread::Builder::new()
            .name("aleph-voice-decode".into())
            .spawn(move || {
                let decoded = request.decode();
                on_main(move |playback| {
                    if playback.open_current(epoch, &generation, || {
                        let (mime, bytes) = decoded?;
                        NativePlayer::open(epoch, generation.clone(), mime, bytes)
                    }) {
                        // Let queued stops run before start. The start guard
                        // also checks the synchronously invalidated visibility epoch.
                        on_main(move |playback| playback.start_current(epoch, &generation));
                    }
                });
            });
        if worker.is_err() {
            playback.opened(epoch, || Err("voice playback decoder could not start"));
        }
    });
}

fn stop(surface: u64, mut call: HostCall) {
    let request = match Stop::parse(call.take_data()) {
        Ok(request) => request,
        Err(error) => return call.reject(error),
    };
    on_main(move |playback| playback.stop(surface, &request.playback_id, reply(call)));
}

struct NativePlayer {
    player: Retained<AVAudioPlayer>,
    // AVAudioPlayer only keeps a weak delegate.
    _delegate: Retained<AlephVoicePlaybackDelegate>,
}

impl NativePlayer {
    fn open(
        epoch: Epoch,
        generation: Generation,
        mime: Mime,
        bytes: Vec<u8>,
    ) -> Result<Self, &'static str> {
        let data = NSData::with_bytes(&bytes);
        drop(bytes);
        let file_type = NSString::from_str(match mime {
            Mime::Mp3 => "public.mp3",
            Mime::Wav => "com.microsoft.waveform-audio",
        });
        // SAFETY: This player and delegate are created and used on the main
        // queue. The initializer consumes an allocation and retains its data.
        let player = unsafe {
            AVAudioPlayer::initWithData_fileTypeHint_error(
                AVAudioPlayer::alloc(),
                &data,
                Some(&file_type),
            )
        }
        .map_err(|_| "native voice playback could not decode the audio")?;
        // SAFETY: The initialized player is still exclusively on the main queue.
        let duration = unsafe { player.duration() };
        if !duration.is_finite() || duration <= 0.0 {
            return Err("native voice playback has no playable duration");
        }
        // SAFETY: The player is still exclusively on the main queue. Prepare
        // here so the IPC retirement guard only encloses the output start.
        if !unsafe { player.prepareToPlay() } {
            return Err("native voice playback could not prepare the audio");
        }
        let delegate = AlephVoicePlaybackDelegate::alloc().set_ivars((epoch, generation));
        // SAFETY: Initialize the NSObject superclass before registering the
        // delegate. NativePlayer keeps the weak delegate target alive.
        let delegate: Retained<AlephVoicePlaybackDelegate> =
            unsafe { objc2::msg_send![super(delegate), init] };
        unsafe { player.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        Ok(Self {
            player,
            _delegate: delegate,
        })
    }
}

impl Player for NativePlayer {
    fn start(&self) -> bool {
        // SAFETY: All use is serialized on the main queue. `play` starts
        // output; only the subsequent delegate completion reports success.
        unsafe { self.player.play() }
    }

    fn stop(&self) {
        // SAFETY: The main queue owns the player and outlives this call.
        // Detach before stopping; any already queued delegate is epoch-bound.
        unsafe {
            self.player.setDelegate(None);
            self.player.stop();
        }
    }
}

define_class!(
    // SAFETY: NSObject subclass with immutable Rust ivars. Delegate methods
    // enqueue value-only callbacks; they never touch the player or host on
    // the callback thread.
    #[unsafe(super = NSObject)]
    #[ivars = (Epoch, Generation)]
    struct AlephVoicePlaybackDelegate;

    unsafe impl NSObjectProtocol for AlephVoicePlaybackDelegate {}

    unsafe impl AVAudioPlayerDelegate for AlephVoicePlaybackDelegate {
        #[unsafe(method(audioPlayerDidFinishPlaying:successfully:))]
        fn did_finish_playing(&self, _player: &AVAudioPlayer, success: bool) {
            let (epoch, generation) = self.ivars().clone();
            let result = success
                .then_some(())
                .ok_or("native voice playback did not finish");
            on_main(move |playback| {
                playback.terminal(epoch, generation.check().and(result));
            });
        }

        #[unsafe(method(audioPlayerDecodeErrorDidOccur:error:))]
        fn decode_error_did_occur(&self, _player: &AVAudioPlayer, _error: Option<&NSError>) {
            let epoch = self.ivars().0;
            on_main(move |playback| {
                playback.terminal(epoch, Err("native voice playback decode error"))
            });
        }
    }
);
