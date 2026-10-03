mod common;
use plugin_hostkit::{
    PluginScanner,
    scanner::{blacklist_count, set_blacklist_path, set_scanner_binary},
};
#[test]
fn overridden_blacklist_records_failed_child_and_freezes_options() {
    let tmp = tempfile::tempdir().unwrap();
    let blacklist = tmp.path().join("blacklist.json");
    set_blacklist_path(blacklist.clone()).unwrap();
    set_scanner_binary(std::path::PathBuf::from(env!(
        "CARGO_BIN_EXE_plugin-scanner"
    )))
    .unwrap();
    let path = common::create_fake_vst3_bundle(tmp.path(), "BrokenPlugin");
    let bundle = plugin_hostkit::Vst3Bundle::from_path(&path).unwrap();
    PluginScanner::bundle_to_info(&bundle);
    assert_eq!(blacklist_count(), 1);
    let entries: Vec<std::path::PathBuf> =
        serde_json::from_slice(&std::fs::read(&blacklist).unwrap()).unwrap();
    assert_eq!(entries, vec![bundle.binary_path.unwrap()]);
    let rejected = tmp.path().join("other.json");
    assert_eq!(set_blacklist_path(rejected.clone()), Err(rejected));
    let rejected = tmp.path().join("other-scanner");
    assert_eq!(set_scanner_binary(rejected.clone()), Err(rejected));
}
