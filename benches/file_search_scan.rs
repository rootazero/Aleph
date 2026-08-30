//! What one `grep` call costs on a real tree.
//!
//! # The number this exists to produce
//!
//! `grep` reports `total_matches` for the whole search, not just the page it
//! renders: a window that does not say how big the thing was that it was cut
//! from leaves the caller unable to tell "these are all of them" from "this is
//! the first sixty". That exactness is bought by scanning *every* walked file,
//! and the round that shipped the tool never measured the price. This is that
//! measurement, on whatever tree it is pointed at.
//!
//! ```bash
//! cargo bench --bench file_search_scan                        # this repository
//! ALEPH_BENCH_ROOT=/path/to/monorepo cargo bench --bench file_search_scan
//! ```
//!
//! # Two numbers, because time was only half of it
//!
//! Wall time is the obvious half. The other half is **peak heap**, tracked by
//! the allocator below, and it is the half that found something: the scan does
//! not only take time proportional to the tree, it can *hold* memory
//! proportional to the tree. `context: 10` turns every match into a
//! twenty-one-line block, and a pattern that matches in five thousand files
//! produces five thousand files' worth of those. Whether they are all alive at
//! once is a property of how the scan stream is consumed, which no assertion
//! about the returned page can see — the page is correct either way.
//!
//! # What these numbers are not
//!
//! Nothing in `cargo test` reads them, and nothing here asserts. Both halves
//! are instruments: the tracking allocator adds two relaxed atomics per
//! allocation, so the timings run a few percent slow, and the machine, the page
//! cache and the tree all move them further than that. They are an
//! order-of-magnitude reading — enough to answer "does one call fit in a turn",
//! not a regression gate.
//!
//! Every row is measured against a tree, so a row without its tree is a number
//! without its predicate: the header prints the root, the file count and the
//! commit, and any quoted result should carry all three.

use std::alloc::{GlobalAlloc, Layout, System};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use alephcore::builtin_tools::{FindArgs, FindTool, GrepArgs, GrepTool};
use alephcore::tools::AlephTool;

/// Bytes currently handed out by the allocator, and the high-water mark of
/// that figure. `PEAK` is reset to the live figure before each case, so a row
/// reports what *that* call added rather than what the process has ever held.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// A pass-through allocator that remembers the high-water mark.
///
/// `realloc` and `alloc_zeroed` are deliberately left to the trait's default
/// bodies, which route through `self.alloc` / `self.dealloc` and are therefore
/// already counted; overriding them to call `System` directly would silently
/// drop every `Vec` growth out of the measurement.
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

/// One measured call.
struct Row {
    case: &'static str,
    ms: u128,
    peak_bytes: usize,
    /// What the tool itself reported — files it read, matches it counted.
    detail: String,
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Run `fut` to completion, reporting elapsed time and the heap high-water
/// mark *above the level already live when the call started*.
fn measure<T>(rt: &tokio::runtime::Runtime, fut: impl Future<Output = T>) -> (T, u128, usize) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let started = Instant::now();
    let out = rt.block_on(fut);
    let ms = started.elapsed().as_millis();
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    (out, ms, peak)
}

fn grep_args(root: &str, pattern: &str) -> GrepArgs {
    GrepArgs {
        pattern: pattern.to_string(),
        path: Some(root.to_string()),
        glob: None,
        ignore_case: None,
        literal: None,
        context: None,
        limit: None,
        offset: None,
        files_only: None,
        no_ignore: None,
    }
}

fn main() {
    // `CARGO_MANIFEST_DIR` is baked in at compile time, so it names the tree
    // this binary was *built* from — not necessarily the one you are standing
    // in. `ALEPH_BENCH_ROOT` is how you point it at a bigger tree.
    let root = std::env::var("ALEPH_BENCH_ROOT")
        .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    let mut rows: Vec<Row> = Vec::new();

    // The walk on its own: `find` does everything `grep` does except read a
    // single byte of file content, so the gap between this row and the next is
    // exactly what scanning costs.
    let (found, ms, peak) = measure(
        &rt,
        FindTool::new().call(FindArgs {
            pattern: "**/*".to_string(),
            path: Some(root.clone()),
            limit: Some(2000),
            offset: None,
            no_ignore: None,
        }),
    );
    let found = found.expect("find");
    let walked = found.total;
    rows.push(Row {
        case: "find **/*                    (walk only, no bytes read)",
        ms,
        peak_bytes: peak,
        detail: format!("{} file(s) walked", found.total),
    });

    // A pattern that matches almost nowhere still pays for the whole scan —
    // that is the point of the exact `total_matches`, and this row is its bill.
    let (out, ms, peak) = measure(
        &rt,
        GrepTool::new().call(grep_args(&root, "MAX_WALK_FILES")),
    );
    let out = out.expect("grep rare");
    rows.push(Row {
        case: "grep rare literal            (few matches, full scan)",
        ms,
        peak_bytes: peak,
        detail: format!(
            "{} scanned, {} match(es) in {} file(s)",
            out.files_scanned, out.total_matches, out.files_with_matches
        ),
    });

    let (out, ms, peak) = measure(&rt, GrepTool::new().call(grep_args(&root, r"\bfn\b")));
    let out = out.expect("grep common");
    rows.push(Row {
        case: r"grep \bfn\b                  (many matches, context 0)",
        ms,
        peak_bytes: peak,
        detail: format!(
            "{} scanned, {} match(es) in {} file(s)",
            out.files_scanned, out.total_matches, out.files_with_matches
        ),
    });

    // The worst case for rendering: `context: 10` makes every kept match a
    // twenty-one-line block, and the per-file cap keeps twenty of them.
    let mut args = grep_args(&root, r"\bfn\b");
    args.context = Some(10);
    let (out, ms, peak) = measure(&rt, GrepTool::new().call(args));
    let out = out.expect("grep context");
    rows.push(Row {
        case: r"grep \bfn\b context=10       (widest blocks)",
        ms,
        peak_bytes: peak,
        detail: format!(
            "{} scanned, {} match(es) in {} file(s)",
            out.files_scanned, out.total_matches, out.files_with_matches
        ),
    });

    // "Which files even mention this" — the cheap first question, and the one
    // whose page is measured in paths rather than blocks.
    let mut args = grep_args(&root, r"\bfn\b");
    args.files_only = Some(true);
    let (out, ms, peak) = measure(&rt, GrepTool::new().call(args));
    let out = out.expect("grep files_only");
    rows.push(Row {
        case: r"grep \bfn\b files_only       (paths, not blocks)",
        ms,
        peak_bytes: peak,
        detail: format!(
            "{} scanned, {} file(s) with matches",
            out.files_scanned, out.files_with_matches
        ),
    });

    // What the `.gitignore`-aware walk is worth: the same question asked of
    // every generated tree as well. This is the row that says how close a real
    // repository gets to `MAX_WALK_FILES`.
    let mut args = grep_args(&root, r"\bfn\b");
    args.no_ignore = Some(true);
    let (out, ms, peak) = measure(&rt, GrepTool::new().call(args));
    let out = out.expect("grep no_ignore");
    rows.push(Row {
        case: r"grep \bfn\b no_ignore=true   (generated trees too)",
        ms,
        peak_bytes: peak,
        detail: format!(
            "{} scanned, {} match(es); truncated={}",
            out.files_scanned, out.total_matches, out.truncated
        ),
    });

    let commit = std::process::Command::new("git")
        .args(["-C", &root, "rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unknown".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        );

    println!("\nfile_search: what one call costs");
    println!("  root    {root}");
    println!("  commit  {commit}");
    println!("  walked  {walked} file(s) with ignore rules in force\n");
    println!(
        "  {:<44} {:>8} {:>11}   {}",
        "case", "ms", "peak heap", "reported"
    );
    for row in &rows {
        println!(
            "  {:<44} {:>8} {:>8.1} MiB   {}",
            row.case,
            row.ms,
            mib(row.peak_bytes),
            row.detail
        );
    }
    println!();
}
