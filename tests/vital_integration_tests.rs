mod common;
use common::is_vital_installed;
use plugin_hostkit::{
    PluginKind,
    scanner::{PluginScanner, ScanProgress},
};
use std::sync::{Arc, Mutex};
#[test]
fn scan_progress_callback_fires_for_each_plugin() {
    if !is_vital_installed() {
        eprintln!("SKIPPED: Vital not installed");
        return;
    }

    let scanner = PluginScanner::new();

    let events: Arc<Mutex<Vec<ScanProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);

    let bundles = scanner.discover_with_progress(Arc::new(move |p| {
        events_clone.lock().unwrap().push(p);
    }));

    let log = events.lock().unwrap();

    // One callback per bundle
    assert_eq!(
        log.len(),
        bundles.len(),
        "Expected one progress event per bundle"
    );

    // scanned counts up sequentially from 1 to total
    for (i, event) in log.iter().enumerate() {
        assert_eq!(event.scanned, i + 1, "scanned should count up sequentially");
        assert_eq!(event.total, bundles.len(), "total should be consistent");
    }

    // Vital should appear in the progress events
    let vital_reported = log.iter().any(|e| {
        e.current_plugin
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case("Vital"))
    });
    assert!(
        vital_reported,
        "Expected Vital to appear in progress events."
    );
}

/// Test that Vital metadata is extracted from the bundle (moduleinfo.json / Info.plist).
#[test]
fn vital_metadata_extracted_from_bundle() {
    if !is_vital_installed() {
        eprintln!("SKIPPED: Vital not installed");
        return;
    }

    let scanner = PluginScanner::new();
    let bundles = scanner.discover_bundles();

    let vital_bundle = match bundles
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case("Vital"))
    {
        Some(b) => b,
        None => {
            eprintln!("SKIPPED: Vital.vst3 bundle not found");
            return;
        }
    };

    let info = PluginScanner::bundle_to_info(vital_bundle);

    assert_eq!(info.id, "vital");
    assert_eq!(info.name, "Vital");

    // Vendor may be extracted from Info.plist, moduleinfo.json, or factory FFI.
    // Source builds may not have packaging metadata, so only warn (don't fail).
    if info.vendor.is_empty() {
        eprintln!(
            "WARNING: vendor is empty (expected for source builds without packaging metadata)"
        );
    }

    // Version may be empty for source builds without packaging metadata.
    if info.version.is_empty() {
        eprintln!("WARNING: version is empty (expected for source builds)");
    }

    // If subcategories were extracted (via moduleinfo.json or out-of-process scanner),
    // Vital should be categorised as Instrument. Otherwise category may be Unknown
    // depending on which metadata sources are available.
    if !info.subcategories.is_empty() {
        assert_eq!(
            PluginKind::from_subcategories(&info.subcategories),
            PluginKind::Instrument,
            "Vital should be categorised as Instrument when subcategories are available, got {:?}",
            PluginKind::from_subcategories(&info.subcategories)
        );
    }

    println!(
        "Vital metadata: vendor={}, category={:?}, subcategories={}, version={}",
        info.vendor,
        PluginKind::from_subcategories(&info.subcategories),
        info.subcategories,
        info.version
    );
}

/// Test that with_paths + discover_with_progress limits scope to specified directories.
#[test]
fn scan_with_custom_paths_limits_scope() {
    if !is_vital_installed() {
        eprintln!("SKIPPED: Vital not installed");
        return;
    }

    // First find Vital's parent directory via the default scanner
    let default_scanner = PluginScanner::new();
    let all_bundles = default_scanner.discover_bundles();

    let vital_bundle = match all_bundles
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case("Vital"))
    {
        Some(b) => b,
        None => {
            eprintln!("SKIPPED: Vital.vst3 bundle not found");
            return;
        }
    };

    let vital_parent = vital_bundle
        .path
        .parent()
        .expect("Vital bundle should have a parent directory");

    // Scan only that directory
    let scoped_scanner = PluginScanner::with_paths(vec![vital_parent.to_path_buf()]);

    let events: Arc<Mutex<Vec<ScanProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);

    let scoped_bundles = scoped_scanner.discover_with_progress(Arc::new(move |p| {
        events_clone.lock().unwrap().push(p);
    }));

    // Should find Vital (and possibly siblings in the same directory)
    let found_vital = scoped_bundles
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case("Vital"));
    assert!(found_vital, "Scoped scan should still find Vital");

    // Should find fewer or equal plugins compared to the full scan
    assert!(
        scoped_bundles.len() <= all_bundles.len(),
        "Scoped scan ({}) should not find more plugins than full scan ({})",
        scoped_bundles.len(),
        all_bundles.len()
    );

    // Progress events should match scoped results
    let log = events.lock().unwrap();
    assert_eq!(log.len(), scoped_bundles.len());
    if let Some(last) = log.last() {
        assert_eq!(last.scanned, scoped_bundles.len());
        assert_eq!(last.total, scoped_bundles.len());
    }

    println!(
        "Scoped scan: {} plugins (full scan: {})",
        scoped_bundles.len(),
        all_bundles.len()
    );
}
