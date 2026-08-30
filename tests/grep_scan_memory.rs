//! One `grep` call must not hold the tree it is searching.
//!
//! # The property, and why nothing else can see it
//!
//! `grep` scans every walked file, because `total_matches` is exact and a
//! window that cannot say how big the thing was that it was cut from is worse
//! than no window. Scanning everything is fine. *Holding* everything is not:
//! with `context: 10` each kept match is a twenty-one-line block, up to twenty
//! blocks are kept per file, and a pattern that matches in a few thousand
//! files therefore renders a few hundred megabytes of them. Whether those
//! blocks are alive at the same moment is a property of **how the scan stream
//! is consumed**, and it is invisible to every assertion about the result: the
//! returned page is byte-identical either way, so all the unit tests next to
//! the tool stay green while the process holds gigabytes.
//!
//! This file is its own test binary, which is the whole reason it exists as a
//! file: a `#[global_allocator]` here instruments only these measurements, and
//! there is exactly one test function so nothing else allocates alongside it.
//!
//! # What is asserted
//!
//! Not an absolute number — that would be a machine measurement dressed up as
//! an invariant. The invariant is **flatness**: quadruple the tree and the peak
//! must not follow. A regression to collecting the scan before folding it makes
//! the ratio track the file count (~4x here); bounded consumption leaves it at
//! roughly 1x. The loose absolute ceiling underneath catches the other
//! direction — a change that makes *both* sizes huge would keep the ratio at
//! 1.0 and say nothing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use alephcore::builtin_tools::{GrepArgs, GrepTool};
use alephcore::tools::AlephTool;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Pass-through allocator that remembers the high-water mark of live bytes.
///
/// `realloc` / `alloc_zeroed` are left to the trait's default bodies on
/// purpose: those route through `self.alloc` and `self.dealloc`, so they are
/// already counted. Overriding them to call `System` directly would drop every
/// `Vec` growth out of the measurement — which is most of what is being
/// measured.
struct Tracking;

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOC: Tracking = Tracking;

/// Files in the small tree; the large one is `FANOUT` times this.
const SMALL_FILES: usize = 120;
const FANOUT: usize = 4;
/// Lines per file, with a match every fifth line — comfortably more matches
/// than the per-file render cap, so every file contributes its full twenty
/// blocks.
const LINES_PER_FILE: usize = 200;

fn plant(root: &Path, files: usize) {
    std::fs::create_dir_all(root).unwrap();
    let filler = "x".repeat(70);
    for f in 0..files {
        let mut body = String::with_capacity(LINES_PER_FILE * 80);
        for line in 0..LINES_PER_FILE {
            if line % 5 == 0 {
                body.push_str("needle ");
            }
            body.push_str(&filler);
            body.push('\n');
        }
        std::fs::write(root.join(format!("f{f:04}.txt")), body).unwrap();
    }
}

fn args(root: &Path) -> GrepArgs {
    GrepArgs {
        pattern: "needle".to_string(),
        path: Some(root.to_string_lossy().to_string()),
        glob: None,
        ignore_case: None,
        literal: None,
        // The widest block the tool will render, which is the shape that makes
        // the difference between the two consumption strategies legible.
        context: Some(10),
        limit: None,
        offset: None,
        files_only: None,
        no_ignore: None,
    }
}

/// Run one `grep` and report the heap high-water mark it added.
fn peak_of(rt: &tokio::runtime::Runtime, root: &Path) -> (usize, usize) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let out = rt
        .block_on(GrepTool::new().call(args(root)))
        .expect("grep must succeed");
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    (peak, out.total_matches)
}

#[test]
fn one_grep_call_does_not_hold_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let small = dir.path().join("small");
    let large = dir.path().join("large");
    let warmup = dir.path().join("warmup");
    plant(&small, SMALL_FILES);
    plant(&large, SMALL_FILES * FANOUT);
    // Enough files to spawn every blocking thread the scan will use, so the
    // first measured run is not charged for their thread-local setup.
    plant(&warmup, 32);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    // Discarded: the first call pays for lazily-built statics (the config read
    // behind `get_denied_paths`, the regex engine's own tables) and for the
    // blocking pool's threads, and would otherwise charge all of it to
    // whichever tree happened to go first.
    let _ = peak_of(&rt, &warmup);

    // Large first, small second. If the two were measured in size order a
    // monotonically growing allocator arena would produce the flat ratio this
    // test is looking for, for the wrong reason.
    let (peak_large, matches_large) = peak_of(&rt, &large);
    let (peak_small, matches_small) = peak_of(&rt, &small);

    // The scan really did cover both trees — a denylist or a stray `.gitignore`
    // turning these into empty searches would otherwise pass everything below.
    let per_file = LINES_PER_FILE.div_ceil(5);
    assert_eq!(matches_small, SMALL_FILES * per_file, "small tree scanned");
    assert_eq!(
        matches_large,
        SMALL_FILES * FANOUT * per_file,
        "large tree scanned"
    );

    let ratio = peak_large as f64 / peak_small.max(1) as f64;
    assert!(
        ratio < 2.0,
        "peak heap tracks the tree: {FANOUT}x the files cost {ratio:.1}x the memory \
         ({} KiB for {} files vs {} KiB for {} files). The scan is being collected \
         before it is folded — consume the stream as it yields.",
        peak_large / 1024,
        SMALL_FILES * FANOUT,
        peak_small / 1024,
        SMALL_FILES,
    );

    // The other direction: a flat ratio proves nothing if both numbers are
    // enormous. The bound is deliberately loose — it is a sanity rail, not a
    // budget, and the ratio above is the assertion that means something.
    assert!(
        peak_large < 16 * 1024 * 1024,
        "peak heap for one grep call was {} KiB",
        peak_large / 1024
    );
}
