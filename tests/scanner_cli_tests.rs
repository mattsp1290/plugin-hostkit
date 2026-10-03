#[test]
fn scanner_and_downstream_wrapper_reject_missing_path() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_plugin-scanner"))
        .arg("/nonexistent")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
    let build = std::process::Command::new(env!("CARGO"))
        .args(["build", "--locked", "--example", "custom_scanner"])
        .status()
        .unwrap();
    assert!(build.success());
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let status = std::process::Command::new(target.join("debug/examples/custom_scanner"))
        .arg("/nonexistent")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}
