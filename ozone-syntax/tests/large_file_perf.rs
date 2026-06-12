//! Baseline timing for opening, editing, and highlighting a large Rust
//! buffer through the existing full-reparse path (no incremental sylven
//! session reuse yet). Establishes numbers to compare against before setting
//! incremental-parsing performance targets.

use std::time::{Duration, Instant};

use ozone_syntax::{fold_line_ranges, parse_features, scan_buffer};
use taste::Language;

const LINE_BLOCKS: usize = 2_000;

/// A repeated pattern of doc-commented functions, large enough to approximate
/// a big real-world source file.
fn large_rust_source(blocks: usize) -> String {
    let mut out = String::with_capacity(blocks * 64);
    for i in 0..blocks {
        out.push_str(&format!(
            "/// Doc comment for item {i}.\nfn item_{i}(x: i32) -> i32 {{\n    let y = x + {i};\n    y * 2\n}}\n\n"
        ));
    }
    out
}

/// Sanity ceiling for a full reparse of the buffer; generous enough to allow
/// for slow CI machines while still catching pathological regressions.
const PERF_CEILING: Duration = Duration::from_secs(5);

#[test]
fn large_file_open_edit_highlight_timings() {
    let lang = Some(Language::RUST);
    let text = large_rust_source(LINE_BLOCKS);
    let line_count = text.lines().count();

    let t0 = Instant::now();
    let highlights = scan_buffer(lang, &text);
    let open_scan = t0.elapsed();

    let t1 = Instant::now();
    let features = parse_features(lang, &text).expect("rust plugin registered");
    let open_parse = t1.elapsed();

    let t2 = Instant::now();
    let folds = fold_line_ranges(lang, &text);
    let open_folds = t2.elapsed();

    // Simulate a single edit: append one more item at the end of the file.
    // This produces a new content hash, so parse_features cannot reuse the
    // memoized result above and must reparse from scratch.
    let mut edited = text.clone();
    edited.push_str("fn appended() -> i32 {\n    42\n}\n");

    let t3 = Instant::now();
    let edited_highlights = scan_buffer(lang, &edited);
    let edit_scan = t3.elapsed();

    let t4 = Instant::now();
    let edited_features = parse_features(lang, &edited).expect("rust plugin registered");
    let edit_parse = t4.elapsed();

    eprintln!(
        "large_file_perf: {line_count} lines, {} bytes\n\
         open  scan_buffer:     {open_scan:?}\n\
         open  parse_features:  {open_parse:?} ({} highlights, {} symbols)\n\
         open  fold_line_ranges: {open_folds:?} ({} folds)\n\
         edit  scan_buffer:     {edit_scan:?}\n\
         edit  parse_features:  {edit_parse:?}",
        text.len(),
        features.highlights.len(),
        features.symbols.len(),
        folds.len(),
    );

    assert_eq!(highlights.len(), line_count + 1);
    assert_eq!(edited_highlights.len(), highlights.len() + 3);
    assert!(!features.highlights.is_empty());
    assert!(!features.symbols.is_empty());
    assert!(!folds.is_empty());
    assert!(!edited_features.highlights.is_empty());

    for (label, d) in [
        ("open scan_buffer", open_scan),
        ("open parse_features", open_parse),
        ("open fold_line_ranges", open_folds),
        ("edit scan_buffer", edit_scan),
        ("edit parse_features", edit_parse),
    ] {
        assert!(
            d < PERF_CEILING,
            "{label} took {d:?}, expected < {PERF_CEILING:?}"
        );
    }
}
