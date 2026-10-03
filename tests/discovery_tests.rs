//! Integration tests for VST3 plugin discovery and scanning.

mod common;
use common::create_fake_vst3_bundle;
use plugin_hostkit::discovery::Vst3Bundle;
use plugin_hostkit::scanner::PluginScanner;

#[test]
fn discover_multiple_bundles_sorted_alphabetically() {
    let tmp = tempfile::tempdir().unwrap();

    // Create bundles in non-alphabetical order
    create_fake_vst3_bundle(tmp.path(), "Zebra");
    create_fake_vst3_bundle(tmp.path(), "Alpha");
    create_fake_vst3_bundle(tmp.path(), "Massive");

    let scanner = PluginScanner::with_paths(vec![tmp.path().to_path_buf()]);
    let bundles = scanner.discover_bundles();

    assert_eq!(bundles.len(), 3);
    assert_eq!(bundles[0].name, "Alpha");
    assert_eq!(bundles[1].name, "Massive");
    assert_eq!(bundles[2].name, "Zebra");
}

#[test]
fn bundle_has_correct_binary_path() {
    let tmp = tempfile::tempdir().unwrap();
    create_fake_vst3_bundle(tmp.path(), "Synth1");

    let bundle_path = tmp.path().join("Synth1.vst3");
    let bundle = Vst3Bundle::from_path(&bundle_path).unwrap();

    assert_eq!(bundle.name, "Synth1");
    assert!(bundle.binary_path.is_some());
    let binary = bundle.binary_path.unwrap();
    assert!(binary.exists());
}

#[test]
fn empty_bundle_dir_has_no_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle_dir = tmp.path().join("Empty.vst3");
    std::fs::create_dir_all(&bundle_dir).unwrap();

    let bundle = Vst3Bundle::from_path(&bundle_dir).unwrap();
    assert_eq!(bundle.name, "Empty");
    assert!(bundle.binary_path.is_none());
}

#[test]
fn non_vst3_extension_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let not_a_bundle = tmp.path().join("SomePlugin.dll");
    std::fs::create_dir_all(&not_a_bundle).unwrap();

    assert!(Vst3Bundle::from_path(&not_a_bundle).is_none());
}

#[test]
fn file_not_directory_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("NotADir.vst3");
    std::fs::write(&file_path, b"not a directory").unwrap();

    assert!(Vst3Bundle::from_path(&file_path).is_none());
}

#[test]
fn scanner_skips_nonexistent_search_paths() {
    let scanner =
        PluginScanner::with_paths(vec![std::path::PathBuf::from("/definitely/does/not/exist")]);
    let bundles = scanner.discover_bundles();
    assert!(bundles.is_empty());
}

#[test]
fn bundle_to_info_generates_id() {
    let tmp = tempfile::tempdir().unwrap();
    create_fake_vst3_bundle(tmp.path(), "My Synth");

    let bundle_path = tmp.path().join("My Synth.vst3");
    let bundle = Vst3Bundle::from_path(&bundle_path).unwrap();
    let info = PluginScanner::bundle_to_info(&bundle);

    assert_eq!(info.id, "my-synth");
    assert_eq!(info.name, "My Synth");
}

#[test]
fn discover_with_progress_reports_all() {
    let tmp = tempfile::tempdir().unwrap();
    create_fake_vst3_bundle(tmp.path(), "PlugA");
    create_fake_vst3_bundle(tmp.path(), "PlugB");

    let scanner = PluginScanner::with_paths(vec![tmp.path().to_path_buf()]);

    let progress_log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log_clone = progress_log.clone();

    let bundles = scanner.discover_with_progress(std::sync::Arc::new(move |p| {
        log_clone.lock().unwrap().push(p.scanned);
    }));

    assert_eq!(bundles.len(), 2);
    let log = progress_log.lock().unwrap();
    assert_eq!(*log, vec![1, 2]);
}
