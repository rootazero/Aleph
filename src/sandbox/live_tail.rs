//! Live tail of a still-running sandboxed child's output.
//!
//! [`run_child_with_drain`](crate::sandbox::platforms::common::run_child_with_drain)
//! only materialises stdout/stderr when the child exits, so a backgrounded
//! 20-minute `cargo build` is a black box until it finishes. `LiveTail` is the
//! side channel that makes it observable: the drain loops tee every chunk they
//! read into a fixed-size ring, and `bash`'s `process_action: "poll"` / `"wait"`
//! render a snapshot of it.
//!
//! **Ring, not head.** A head-shaped live view would FREEZE at the first few
//! MiB — on exactly the long builds this exists for. The ring keeps the LAST
//! [`LIVE_TAIL_BYTES`] instead, so the view always tracks the frontier, and a
//! monotonic counter reports how much has flowed through in total.
//!
//! `drain_bounded` also retains a rolling tail window of its own, and that is
//! **not** a reason to delete this type: those fragments are assembled only
//! when the child exits, which is the moment this side channel stops being
//! needed. Mid-run, the ring is the only readable view of the frontier.
//!
//! **The bytes here are PRE-scrub.** They are the raw pipe bytes, before
//! [`crate::sandbox::scrub::scrub_and_gate_output`] has seen them. Every reader
//! must run the same scrub-and-gate floor the finished path runs before showing
//! a snapshot to a model or a human — see `bash_exec::partial_view`.

use crate::sync_primitives::Mutex;

/// Bytes retained per stream. Matches
/// `agents::background_persistence::PARTIAL_RESULT_TAIL_BYTES` — the byte gate
/// the codebase already uses for "a useful tail of a long-running thing".
pub const LIVE_TAIL_BYTES: usize = 8 * 1024;

/// Which stream a tee'd chunk belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStream {
    Stdout,
    Stderr,
}

/// A point-in-time read of a [`LiveTail`].
///
/// `stdout` / `stderr` are the retained tail, trimmed to a UTF-8 codepoint
/// boundary at both ends by `trim_to_utf8_boundaries`. `*_total` counts **every**
/// byte the drain loop ever read for that stream, including bytes the ring has
/// since overwritten — so `total > tail.len()` is the honest signal that the
/// view is a window, not the whole output.
#[derive(Debug, Clone, Default)]
pub struct LiveSnapshot {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_total: u64,
    pub stderr_total: u64,
}

impl LiveSnapshot {
    /// True when the child has not produced a single byte on either stream.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stdout_total == 0 && self.stderr_total == 0
    }
}

/// Fixed-capacity byte ring plus a monotonic total.
struct Ring {
    /// Backing store. Grows to `cap` and then never reallocates; once full,
    /// `head` is the index of the OLDEST byte and also the next write slot.
    buf: Vec<u8>,
    head: usize,
    cap: usize,
    total: u64,
}

impl Ring {
    fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            head: 0,
            cap,
            total: 0,
        }
    }

    /// Append `bytes`, evicting the oldest bytes once the ring is full.
    /// `total` always counts the full input, evicted or not.
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        if self.cap == 0 {
            return;
        }
        // Only the last `cap` bytes of a big chunk can possibly survive, so
        // skip straight to them instead of writing each byte over the last.
        let bytes = if bytes.len() > self.cap {
            &bytes[bytes.len() - self.cap..]
        } else {
            bytes
        };
        for &b in bytes {
            if self.buf.len() < self.cap {
                self.buf.push(b);
            } else {
                self.buf[self.head] = b;
            }
            self.head = (self.head + 1) % self.cap;
        }
    }

    /// Copy the retained bytes out, oldest first, trimmed so the result never
    /// starts or ends mid-codepoint.
    ///
    /// The ring cuts at an arbitrary byte offset by construction, so the FRONT
    /// can begin on a UTF-8 continuation byte (`0b10xx_xxxx`) — we advance past
    /// those, the same forward walk `common::truncate_output` does at its tail
    /// cut. The BACK can hold a truncated lead byte for a codepoint whose
    /// remaining bytes simply have not been read yet — we drop that partial
    /// sequence rather than let a lossy conversion stamp a U+FFFD onto a
    /// character that is about to arrive intact.
    fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.buf.len());
        if self.buf.len() < self.cap {
            out.extend_from_slice(&self.buf);
        } else {
            out.extend_from_slice(&self.buf[self.head..]);
            out.extend_from_slice(&self.buf[..self.head]);
        }
        trim_to_utf8_boundaries(out)
    }
}

/// Drop a leading UTF-8 continuation run and a trailing incomplete sequence.
/// Genuinely invalid bytes in the middle are left alone — binary output stays
/// as it was, and the caller's lossy conversion decides how to render it.
fn trim_to_utf8_boundaries(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut start = 0usize;
    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
        start += 1;
    }
    if start > 0 {
        bytes.drain(..start);
    }
    // `Utf8Error::error_len() == None` means "unexpected end of input", i.e. a
    // multi-byte sequence cut short — exactly the frontier case. Any other
    // error is real corruption and is preserved.
    if let Err(e) = std::str::from_utf8(&bytes) {
        if e.error_len().is_none() {
            bytes.truncate(e.valid_up_to());
        }
    }
    bytes
}

/// Per-stream live tails for one running child. Cheap to clone as an
/// `Arc<LiveTail>`: the drain loops hold one clone each, the registry holds one
/// for as long as the job is running, and the exec-context task-local
/// [`crate::sandbox::context::LIVE_TAIL`] carries it from the spawning tool down
/// to the platform driver.
pub struct LiveTail {
    stdout: Mutex<Ring>,
    stderr: Mutex<Ring>,
}

impl LiveTail {
    /// A tail retaining [`LIVE_TAIL_BYTES`] per stream.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(LIVE_TAIL_BYTES)
    }

    /// A tail retaining `cap` bytes per stream. Exposed for tests that want to
    /// exercise ring wrap without pushing 8 KiB.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            stdout: Mutex::new(Ring::new(cap)),
            stderr: Mutex::new(Ring::new(cap)),
        }
    }

    /// Tee one freshly-read chunk into the ring for `stream`.
    pub fn push(&self, stream: LiveStream, bytes: &[u8]) {
        // P7: a panic while another thread held this leaf lock must not wedge
        // the drain loop — the tail is observability, never correctness.
        let mut ring = match stream {
            LiveStream::Stdout => self.stdout.lock().unwrap_or_else(|e| e.into_inner()),
            LiveStream::Stderr => self.stderr.lock().unwrap_or_else(|e| e.into_inner()),
        };
        ring.push(bytes);
    }

    /// Read both rings. The two locks are taken one at a time, so a snapshot is
    /// not a consistent cut across streams — stdout may be a few bytes fresher
    /// than stderr. That is fine for a progress view and keeps the tee lock-free
    /// with respect to the other stream's drain loop.
    #[must_use]
    pub fn snapshot(&self) -> LiveSnapshot {
        let (stdout, stdout_total) = {
            let ring = self.stdout.lock().unwrap_or_else(|e| e.into_inner());
            (ring.snapshot(), ring.total)
        };
        let (stderr, stderr_total) = {
            let ring = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
            (ring.snapshot(), ring.total)
        };
        LiveSnapshot {
            stdout,
            stderr,
            stdout_total,
            stderr_total,
        }
    }
}

impl Default for LiveTail {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_writes_are_returned_whole() {
        let t = LiveTail::with_capacity(64);
        t.push(LiveStream::Stdout, b"hello ");
        t.push(LiveStream::Stdout, b"world");
        let s = t.snapshot();
        assert_eq!(s.stdout, b"hello world");
        assert_eq!(s.stdout_total, 11);
        assert!(s.stderr.is_empty());
        assert_eq!(s.stderr_total, 0);
    }

    #[test]
    fn streams_are_independent() {
        let t = LiveTail::with_capacity(64);
        t.push(LiveStream::Stdout, b"out");
        t.push(LiveStream::Stderr, b"errr");
        let s = t.snapshot();
        assert_eq!(s.stdout, b"out");
        assert_eq!(s.stderr, b"errr");
        assert_eq!(s.stdout_total, 3);
        assert_eq!(s.stderr_total, 4);
    }

    /// The whole point of a ring: past capacity we keep the NEWEST bytes.
    /// A head-shaped buffer would still be showing "aaaa" here — the freeze
    /// this type exists to avoid.
    #[test]
    fn ring_wraps_and_keeps_the_tail_not_the_head() {
        let t = LiveTail::with_capacity(8);
        t.push(LiveStream::Stdout, b"aaaaaaaa"); // fills exactly
        assert_eq!(t.snapshot().stdout, b"aaaaaaaa");
        t.push(LiveStream::Stdout, b"bcd");
        let s = t.snapshot();
        assert_eq!(s.stdout, b"aaaaabcd", "oldest three bytes evicted");
        assert_eq!(s.stdout_total, 11);
    }

    #[test]
    fn a_single_write_larger_than_capacity_keeps_its_own_tail() {
        let t = LiveTail::with_capacity(4);
        t.push(LiveStream::Stdout, b"0123456789");
        let s = t.snapshot();
        assert_eq!(s.stdout, b"6789");
        assert_eq!(s.stdout_total, 10);
    }

    /// The total must survive many wraps exactly — it is what tells the model
    /// "you are looking at 4 bytes of a 400-byte stream".
    #[test]
    fn total_is_exact_across_many_wraps() {
        let t = LiveTail::with_capacity(4);
        for _ in 0..100 {
            t.push(LiveStream::Stdout, b"xyzw");
        }
        let s = t.snapshot();
        assert_eq!(s.stdout_total, 400);
        assert_eq!(s.stdout.len(), 4, "ring stays bounded");
    }

    #[test]
    fn empty_tail_reports_empty() {
        let t = LiveTail::with_capacity(16);
        assert!(t.snapshot().is_empty());
        t.push(LiveStream::Stderr, b"x");
        assert!(!t.snapshot().is_empty());
    }

    /// The ring cuts at a byte offset, so the front of a wrapped view lands
    /// mid-codepoint. The read side must advance past the continuation bytes
    /// rather than hand back a fragment that renders as U+FFFD.
    #[test]
    fn wrapped_front_never_starts_mid_codepoint() {
        // 4 euro signs = 12 bytes; a 10-byte ring drops the first 2 bytes of
        // the leading euro sign, leaving one continuation byte at the front.
        let t = LiveTail::with_capacity(10);
        t.push(LiveStream::Stdout, "€€€€".as_bytes());
        let s = t.snapshot();
        assert_eq!(
            std::str::from_utf8(&s.stdout).expect("front trimmed to a boundary"),
            "€€€"
        );
        assert_eq!(s.stdout_total, 12);
    }

    /// The frontier case: the drain loop read the lead byte of a codepoint but
    /// not its continuation bytes yet. Drop the partial rather than corrupt it.
    #[test]
    fn trailing_partial_codepoint_is_dropped_until_it_arrives() {
        let t = LiveTail::with_capacity(64);
        let euro = "€".as_bytes();
        t.push(LiveStream::Stdout, b"ok");
        t.push(LiveStream::Stdout, &euro[..2]); // half a euro sign
        let s = t.snapshot();
        assert_eq!(std::str::from_utf8(&s.stdout).unwrap(), "ok");
        assert_eq!(s.stdout_total, 4, "total still counts the bytes read");
        // The rest arrives on the next read → the character appears whole.
        t.push(LiveStream::Stdout, &euro[2..]);
        assert_eq!(
            std::str::from_utf8(&t.snapshot().stdout).unwrap(),
            "ok€",
            "the codepoint materialises once complete"
        );
    }

    /// Genuinely invalid bytes in the middle are NOT silently eaten — only the
    /// two frontier cuts are trimmed. Binary output stays byte-visible.
    #[test]
    fn interior_invalid_bytes_are_preserved() {
        let t = LiveTail::with_capacity(64);
        t.push(LiveStream::Stdout, b"a\xffb");
        assert_eq!(t.snapshot().stdout, b"a\xffb");
    }

    #[test]
    fn zero_capacity_still_counts_bytes() {
        let t = LiveTail::with_capacity(0);
        t.push(LiveStream::Stdout, b"anything");
        let s = t.snapshot();
        assert!(s.stdout.is_empty());
        assert_eq!(s.stdout_total, 8, "the counter is independent of the ring");
    }
}
